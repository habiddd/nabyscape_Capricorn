//! Win32 helpers of the installer, updater and uninstaller: per-user registry values, Start-menu
//! shortcuts, process listing and control, kernel events, the file-version resource,
//! Authenticode verification and the few dialogs setup needs.

use std::ffi::c_void;
use std::io;
use std::path::{Path, PathBuf};

use naby_core::updates::Version;
use windows::core::{Interface, PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_FILE_NOT_FOUND, ERROR_NOT_FOUND, FILETIME, HANDLE, HWND, WAIT_OBJECT_0,
    WIN32_ERROR,
};
use windows::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW, VS_FIXEDFILEINFO,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoTaskMemFree, IPersistFile, CLSCTX_INPROC_SERVER, STGM_READ,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegDeleteValueW, RegGetValueW, RegOpenKeyExW,
    RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_DWORD, REG_OPTION_NON_VOLATILE,
    REG_SZ, RRF_RT_REG_SZ,
};
use windows::Win32::System::Threading::{
    CreateEventW, GetProcessTimes, OpenEventW, OpenProcess, SetEvent, TerminateProcess,
    WaitForSingleObject, EVENT_MODIFY_STATE, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
};
use windows::Win32::UI::Shell::{
    FOLDERID_Programs, IShellLinkW, SHBrowseForFolderW, SHGetKnownFolderPath, SHGetPathFromIDListW,
    ShellLink, BIF_NEWDIALOGSTYLE, BIF_RETURNONLYFSDIRS, BROWSEINFOW, KF_FLAG_DEFAULT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    MessageBoxW, IDCANCEL, IDNO, IDYES, MB_DEFBUTTON2, MB_ICONERROR, MB_ICONINFORMATION,
    MB_ICONQUESTION, MB_OK, MB_YESNO, MB_YESNOCANCEL,
};

use crate::util::{from_wide, wide};

/// Seconds between 1601-01-01 (FILETIME) and 0001-01-01 (.NET ticks), in 100 ns units.
const FILETIME_TO_DOTNET_TICKS: i64 = 504_911_232_000_000_000;

fn pcwstr(text: &[u16]) -> PCWSTR {
    PCWSTR(text.as_ptr())
}

fn os_error(what: &str, code: WIN32_ERROR) -> io::Error {
    io::Error::other(format!("{what} failed ({})", code.0))
}

// ---- registry ------------------------------------------------------------------------------------

/// Per-user (`HKCU`) registry values.
pub mod reg {
    use super::*;

    struct Key(HKEY);
    impl Drop for Key {
        fn drop(&mut self) {
            // SAFETY: closes the handle this wrapper opened.
            let _ = unsafe { RegCloseKey(self.0) };
        }
    }

    fn create(sub: &str) -> io::Result<Key> {
        let sub = wide(sub);
        let mut handle = HKEY::default();
        // SAFETY: out-pointer references a live local; the key is closed by `Key`.
        let status = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                pcwstr(&sub),
                None,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE,
                None,
                &mut handle,
                None,
            )
        };
        if status.0 != 0 {
            return Err(os_error("create registry key", status));
        }
        Ok(Key(handle))
    }

    /// Reads a string value; `None` when the key or value is missing or not a string.
    pub fn get_string(sub: &str, name: &str) -> Option<String> {
        let (sub, name) = (wide(sub), wide(name));
        let mut size = 0u32;
        // SAFETY: first call only asks for the byte size; no buffer is written.
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                pcwstr(&sub),
                pcwstr(&name),
                RRF_RT_REG_SZ,
                None,
                None,
                Some(&mut size),
            )
        };
        if status.0 != 0 || size == 0 {
            return None;
        }
        let mut buffer = vec![0u16; (size as usize).div_ceil(2) + 1];
        let mut size = (buffer.len() * 2) as u32;
        // SAFETY: `buffer` has `size` bytes and stays alive during the call.
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                pcwstr(&sub),
                pcwstr(&name),
                RRF_RT_REG_SZ,
                None,
                Some(buffer.as_mut_ptr().cast::<c_void>()),
                Some(&mut size),
            )
        };
        (status.0 == 0).then(|| from_wide(&buffer))
    }

    pub fn set_string(sub: &str, name: &str, value: &str) -> io::Result<()> {
        let key = create(sub)?;
        let name = wide(name);
        let data = wide(value);
        // SAFETY: reinterprets the UTF-16 buffer as bytes for the duration of the call.
        let bytes =
            unsafe { std::slice::from_raw_parts(data.as_ptr().cast::<u8>(), data.len() * 2) };
        // SAFETY: `name` is NUL-terminated and `bytes` stays alive across the call.
        let status = unsafe { RegSetValueExW(key.0, pcwstr(&name), None, REG_SZ, Some(bytes)) };
        if status.0 != 0 {
            return Err(os_error("write registry value", status));
        }
        Ok(())
    }

    pub fn set_dword(sub: &str, name: &str, value: u32) -> io::Result<()> {
        let key = create(sub)?;
        let name = wide(name);
        let bytes = value.to_le_bytes();
        // SAFETY: `name` is NUL-terminated; `bytes` is the 4-byte DWORD payload.
        let status = unsafe { RegSetValueExW(key.0, pcwstr(&name), None, REG_DWORD, Some(&bytes)) };
        if status.0 != 0 {
            return Err(os_error("write registry value", status));
        }
        Ok(())
    }

    /// Deletes a value; a missing key or value is fine.
    pub fn delete_value(sub: &str, name: &str) -> io::Result<()> {
        let sub = wide(sub);
        let mut handle = HKEY::default();
        // SAFETY: out-pointer references a live local; the key is closed by `Key`.
        let status = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                pcwstr(&sub),
                None,
                KEY_SET_VALUE,
                &mut handle,
            )
        };
        if status == ERROR_FILE_NOT_FOUND {
            return Ok(());
        }
        if status.0 != 0 {
            return Err(os_error("open registry key", status));
        }
        let key = Key(handle);
        let name = wide(name);
        // SAFETY: `name` is NUL-terminated.
        let status = unsafe { RegDeleteValueW(key.0, pcwstr(&name)) };
        if status == ERROR_FILE_NOT_FOUND || status.0 == 0 {
            Ok(())
        } else {
            Err(os_error("delete registry value", status))
        }
    }

    /// Deletes a key with everything under it; a missing key is fine.
    pub fn delete_tree(sub: &str) -> io::Result<()> {
        let sub = wide(sub);
        // SAFETY: deletes only the named per-user subtree.
        let status = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, pcwstr(&sub)) };
        if status == ERROR_FILE_NOT_FOUND || status.0 == 0 {
            Ok(())
        } else {
            Err(os_error("delete registry key", status))
        }
    }
}

// ---- shortcuts ----------------------------------------------------------------------------------

/// The per-user Start-menu `Programs` folder.
pub fn programs_folder() -> Option<PathBuf> {
    // SAFETY: the returned buffer is copied and freed with CoTaskMemFree exactly once.
    unsafe {
        let path = SHGetKnownFolderPath(&FOLDERID_Programs, KF_FLAG_DEFAULT, None).ok()?;
        let text = path.to_string().ok();
        CoTaskMemFree(Some(path.0 as *const c_void));
        text.map(PathBuf::from)
    }
}

/// Writes a `.lnk` (WScript `CreateShortcut` + `Save`). COM must be initialized on the thread.
pub fn create_shortcut(
    path: &Path,
    target: &Path,
    arguments: &str,
    working_directory: &Path,
    description: &str,
    icon: &Path,
) -> io::Result<()> {
    let com = |e: windows::core::Error| io::Error::other(e.message());
    let w = |p: &Path| wide(&p.to_string_lossy());
    let (file, target, dir, icon) = (w(path), w(target), w(working_directory), w(icon));
    let (arguments, description) = (wide(arguments), wide(description));
    // SAFETY: COM calls on an initialized thread; all strings are NUL-terminated and outlive
    // the calls.
    unsafe {
        let link: IShellLinkW =
            CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).map_err(com)?;
        link.SetPath(pcwstr(&target)).map_err(com)?;
        link.SetArguments(pcwstr(&arguments)).map_err(com)?;
        link.SetWorkingDirectory(pcwstr(&dir)).map_err(com)?;
        link.SetDescription(pcwstr(&description)).map_err(com)?;
        link.SetIconLocation(pcwstr(&icon), 0).map_err(com)?;
        let persist: IPersistFile = link.cast().map_err(com)?;
        persist.Save(pcwstr(&file), true).map_err(com)?;
    }
    Ok(())
}

pub struct ShortcutInfo {
    pub target: String,
    pub arguments: String,
}

/// Target and arguments of a `.lnk`, `None` when it cannot be read.
pub fn read_shortcut(path: &Path) -> Option<ShortcutInfo> {
    let file = wide(&path.to_string_lossy());
    // SAFETY: COM calls on an initialized thread; buffers are sized for the API.
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        let persist: IPersistFile = link.cast().ok()?;
        persist.Load(pcwstr(&file), STGM_READ).ok()?;
        let mut target = [0u16; 1040];
        link.GetPath(&mut target, std::ptr::null_mut(), 0).ok()?;
        let mut arguments = [0u16; 2048];
        link.GetArguments(&mut arguments).ok()?;
        Some(ShortcutInfo {
            target: from_wide(&target),
            arguments: from_wide(&arguments),
        })
    }
}

// ---- processes ----------------------------------------------------------------------------------

pub struct ProcessEntry {
    pub pid: u32,
    pub exe_name: String,
}

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: closes the handle this wrapper owns.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// Every running process (id and executable name).
pub fn processes() -> Vec<ProcessEntry> {
    // SAFETY: standard ToolHelp enumeration; the snapshot handle is closed by `Handle` and the
    // entry struct is writable with `dwSize` set.
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return Vec::new();
        };
        let snapshot = Handle(snapshot);
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut out = Vec::new();
        let mut more = Process32FirstW(snapshot.0, &mut entry).is_ok();
        while more {
            out.push(ProcessEntry {
                pid: entry.th32ProcessID,
                exe_name: from_wide(&entry.szExeFile),
            });
            more = Process32NextW(snapshot.0, &mut entry).is_ok();
        }
        out
    }
}

/// Stops a process and waits up to `timeout_ms` for it to disappear.
pub fn terminate_and_wait(pid: u32, timeout_ms: u32) -> io::Result<()> {
    // SAFETY: the handle is opened for terminate/wait only and closed by `Handle`.
    unsafe {
        let process = match OpenProcess(PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, false, pid) {
            Ok(handle) => Handle(handle),
            // Already gone (or inaccessible): nothing left to stop that we could stop.
            Err(_) => return Ok(()),
        };
        let _ = TerminateProcess(process.0, 1);
        if WaitForSingleObject(process.0, timeout_ms) == WAIT_OBJECT_0 {
            Ok(())
        } else {
            Err(io::Error::other("the process did not stop"))
        }
    }
}

/// Waits for a process to exit; `true` when it did (or does not exist) within `timeout_ms`.
pub fn wait_for_exit(pid: u32, timeout_ms: u32) -> bool {
    // SAFETY: synchronize-only handle, closed by `Handle`.
    unsafe {
        match OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
            Ok(handle) => {
                let process = Handle(handle);
                WaitForSingleObject(process.0, timeout_ms) == WAIT_OBJECT_0
            }
            Err(_) => true,
        }
    }
}

/// A process' start time as .NET UTC ticks (the unit of `StartedUtcTicks`).
pub fn process_start_ticks(pid: u32) -> Option<i64> {
    // SAFETY: query-only handle, closed by `Handle`; the FILETIME out-parameters are locals.
    unsafe {
        let process = Handle(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?);
        let (mut created, mut exited, mut kernel, mut user) = (
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
        );
        GetProcessTimes(process.0, &mut created, &mut exited, &mut kernel, &mut user).ok()?;
        let filetime = (i64::from(created.dwHighDateTime) << 32) | i64::from(created.dwLowDateTime);
        Some(filetime + FILETIME_TO_DOTNET_TICKS)
    }
}

// ---- kernel events ---------------------------------------------------------------------------

/// A named manual-reset event this process created (signalled by another process).
pub struct NamedEvent(Handle);

impl NamedEvent {
    pub fn create(name: &str) -> io::Result<Self> {
        let name = wide(name);
        // SAFETY: creates a named manual-reset event; the handle is closed by `Handle`.
        let handle = unsafe { CreateEventW(None, true, false, pcwstr(&name)) }
            .map_err(|e| io::Error::other(e.message()))?;
        Ok(Self(Handle(handle)))
    }

    /// Waits for the event; `true` when it was signalled within `timeout_ms`.
    pub fn wait(&self, timeout_ms: u32) -> bool {
        // SAFETY: waits on a handle this value owns.
        unsafe { WaitForSingleObject((self.0).0, timeout_ms) == WAIT_OBJECT_0 }
    }
}

/// Signals an existing named event; `false` when it does not exist.
pub fn signal_event(name: &str) -> bool {
    let name = wide(name);
    // SAFETY: opens an existing event by name, signals it, closes it.
    unsafe {
        match OpenEventW(EVENT_MODIFY_STATE, false, pcwstr(&name)) {
            Ok(handle) => {
                let handle = Handle(handle);
                SetEvent(handle.0).is_ok()
            }
            Err(_) => false,
        }
    }
}

// ---- file version and signature --------------------------------------------------------------

/// `FileMajorPart.FileMinorPart.FileBuildPart` of an executable's version resource.
pub fn file_version(path: &Path) -> io::Result<Version> {
    let file = wide(&path.to_string_lossy());
    // SAFETY: the buffer is sized by GetFileVersionInfoSizeW; the fixed-info pointer returned by
    // VerQueryValueW points into that buffer and is read before the buffer is dropped.
    unsafe {
        let size = GetFileVersionInfoSizeW(pcwstr(&file), None);
        if size == 0 {
            return Err(io::Error::other(
                "The executable has no version information.",
            ));
        }
        let mut data = vec![0u8; size as usize];
        GetFileVersionInfoW(
            pcwstr(&file),
            None,
            size,
            data.as_mut_ptr().cast::<c_void>(),
        )
        .map_err(|e| io::Error::other(e.message()))?;
        let mut info: *mut c_void = std::ptr::null_mut();
        let mut length = 0u32;
        let root = wide("\\");
        let found = VerQueryValueW(
            data.as_ptr().cast::<c_void>(),
            pcwstr(&root),
            &mut info,
            &mut length,
        );
        if !found.as_bool()
            || info.is_null()
            || (length as usize) < std::mem::size_of::<VS_FIXEDFILEINFO>()
        {
            return Err(io::Error::other(
                "The executable has no version information.",
            ));
        }
        let fixed = std::ptr::read_unaligned(info.cast::<VS_FIXEDFILEINFO>());
        Ok(Version::new(
            fixed.dwFileVersionMS >> 16,
            fixed.dwFileVersionMS & 0xFFFF,
            fixed.dwFileVersionLS >> 16,
        ))
    }
}

/// `WinVerifyTrust` (Authenticode, revocation checked): C# `UpdateSignature.VerifyIfPresent`.
/// An unsigned file is accepted (existing official releases are unsigned); a present signature
/// must pass Windows trust and revocation policy.
pub fn verify_authenticode_if_present(path: &Path) -> io::Result<()> {
    use windows::core::GUID;
    use windows::Win32::Security::WinTrust::{
        WinVerifyTrust, WINTRUST_DATA, WINTRUST_DATA_0, WINTRUST_FILE_INFO, WTD_CHOICE_FILE,
        WTD_REVOKE_WHOLECHAIN, WTD_UI_NONE,
    };
    /// `TRUST_E_NOSIGNATURE`
    const NO_SIGNATURE: i32 = 0x800B_0100_u32 as i32;
    let file = wide(&path.to_string_lossy());
    let mut file_info = WINTRUST_FILE_INFO {
        cbStruct: std::mem::size_of::<WINTRUST_FILE_INFO>() as u32,
        pcwszFilePath: pcwstr(&file),
        ..Default::default()
    };
    let mut data = WINTRUST_DATA {
        cbStruct: std::mem::size_of::<WINTRUST_DATA>() as u32,
        dwUIChoice: WTD_UI_NONE,
        fdwRevocationChecks: WTD_REVOKE_WHOLECHAIN,
        dwUnionChoice: WTD_CHOICE_FILE,
        Anonymous: WINTRUST_DATA_0 {
            pFile: &mut file_info,
        },
        ..Default::default()
    };
    // WINTRUST_ACTION_GENERIC_VERIFY_V2
    let mut action = GUID::from_u128(0x00AAC56B_CD44_11D0_8CC2_00C04FC295EE);
    // SAFETY: `data` and `file_info` live across the call; no UI is shown (INVALID_HANDLE_VALUE
    // as the window means "no interactive UI").
    let result = unsafe {
        WinVerifyTrust(
            HWND(-1isize as *mut c_void),
            &mut action,
            (&mut data as *mut WINTRUST_DATA).cast::<c_void>(),
        )
    };
    if result != 0 && result != NO_SIGNATURE {
        return Err(io::Error::other(format!(
            "Windows could not validate the setup signature (0x{:08X}).",
            result as u32
        )));
    }
    Ok(())
}

// ---- saved API keys -----------------------------------------------------------------------------

/// Deletes the generic credentials `NabyScape/Assistant/*` whose target `owned` accepts
/// (C# `NabyScapeUninstallCredentials.Remove`).
pub fn delete_saved_keys(owned: fn(&str) -> bool) -> io::Result<()> {
    use windows::Win32::Security::Credentials::{
        CredDeleteW, CredEnumerateW, CredFree, CREDENTIALW, CRED_ENUMERATE_FLAGS, CRED_TYPE_GENERIC,
    };
    let filter = wide("NabyScape/Assistant/*");
    let mut count = 0u32;
    let mut list: *mut *mut CREDENTIALW = std::ptr::null_mut();
    // SAFETY: enumerates credentials into a system-owned array that is released with CredFree
    // below; each entry pointer is only read within `count`.
    unsafe {
        if let Err(error) = CredEnumerateW(
            pcwstr(&filter),
            Some(CRED_ENUMERATE_FLAGS(0)),
            &mut count,
            &mut list,
        ) {
            return if error.code() == ERROR_NOT_FOUND.to_hresult() {
                Ok(())
            } else {
                Err(io::Error::other(error.message()))
            };
        }
        let mut result = Ok(());
        for index in 0..count as usize {
            let entry = &**list.add(index);
            if entry.Type != CRED_TYPE_GENERIC {
                continue;
            }
            let target = entry.TargetName.to_string().unwrap_or_default();
            if !owned(&target) {
                continue;
            }
            let name = wide(&target);
            if let Err(error) = CredDeleteW(pcwstr(&name), CRED_TYPE_GENERIC, None) {
                if error.code() != ERROR_NOT_FOUND.to_hresult() {
                    result = Err(io::Error::other(error.message()));
                    break;
                }
            }
        }
        CredFree(list.cast::<c_void>().cast_const());
        result
    }
}

// ---- dialogs ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Info,
    Error,
    Question,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Buttons {
    YesNoCancel,
    YesNoDefaultYes,
    YesNoDefaultNo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    Yes,
    No,
    Cancel,
}

fn icon_flag(icon: Icon) -> windows::Win32::UI::WindowsAndMessaging::MESSAGEBOX_STYLE {
    match icon {
        Icon::Info => MB_ICONINFORMATION,
        Icon::Error => MB_ICONERROR,
        Icon::Question => MB_ICONQUESTION,
    }
}

/// An OK-only message box.
pub fn message(title: &str, text: &str, icon: Icon) {
    let (title, text) = (wide(title), wide(text));
    // SAFETY: modal message box with NUL-terminated strings.
    unsafe {
        MessageBoxW(None, pcwstr(&text), pcwstr(&title), MB_OK | icon_flag(icon));
    }
}

/// A question box.
pub fn ask(title: &str, text: &str, buttons: Buttons) -> Answer {
    let (title, text) = (wide(title), wide(text));
    let style = match buttons {
        Buttons::YesNoCancel => MB_YESNOCANCEL,
        Buttons::YesNoDefaultYes => MB_YESNO,
        Buttons::YesNoDefaultNo => MB_YESNO | MB_DEFBUTTON2,
    } | MB_ICONQUESTION;
    // SAFETY: modal message box with NUL-terminated strings.
    let answer = unsafe { MessageBoxW(None, pcwstr(&text), pcwstr(&title), style) };
    if answer == IDYES {
        Answer::Yes
    } else if answer == IDNO {
        Answer::No
    } else {
        let _ = IDCANCEL;
        Answer::Cancel
    }
}

/// The system folder picker; `None` when cancelled.
pub fn pick_folder(title: &str) -> Option<String> {
    let title = wide(title);
    let mut name = [0u16; 260];
    let browse = BROWSEINFOW {
        lpszTitle: pcwstr(&title),
        pszDisplayName: PWSTR(name.as_mut_ptr()),
        ulFlags: BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE,
        ..Default::default()
    };
    // SAFETY: the dialog fills `name`; the returned item-id list is converted and freed once.
    unsafe {
        let item = SHBrowseForFolderW(&browse);
        if item.is_null() {
            return None;
        }
        let mut path = [0u16; 260];
        let ok = SHGetPathFromIDListW(item, &mut path).as_bool();
        CoTaskMemFree(Some(item as *const c_void));
        ok.then(|| from_wide(&path))
    }
}

/// The uninstall confirmation with the "Also remove my data and downloaded ASR / TTS models"
/// checkbox. `Some(checked)` when the user chose Uninstall, `None` for Cancel.
pub fn confirm_uninstall(remove_all: bool) -> Option<bool> {
    use windows::Win32::UI::Controls::{
        TaskDialogIndirect, TASKDIALOGCONFIG, TASKDIALOG_BUTTON, TDF_ALLOW_DIALOG_CANCELLATION,
        TDF_POSITION_RELATIVE_TO_WINDOW, TDF_VERIFICATION_FLAG_CHECKED,
    };
    let title = wide("Uninstall NabyScape");
    let instruction = wide("Remove NabyScape from this PC?");
    let content = wide(
        "Includes settings, history, caches, downloaded engines and saved API keys. Imported files outside NabyScape and shared Windows components are kept.",
    );
    let check = wide("Also remove my data and downloaded ASR / TTS models");
    let uninstall = wide("Uninstall");
    let cancel = wide("Cancel");
    let buttons = [
        TASKDIALOG_BUTTON {
            nButtonID: 1,
            pszButtonText: pcwstr(&uninstall),
        },
        TASKDIALOG_BUTTON {
            nButtonID: 2,
            pszButtonText: pcwstr(&cancel),
        },
    ];
    let mut flags = TDF_ALLOW_DIALOG_CANCELLATION | TDF_POSITION_RELATIVE_TO_WINDOW;
    if remove_all {
        flags |= TDF_VERIFICATION_FLAG_CHECKED;
    }
    let config = TASKDIALOGCONFIG {
        cbSize: std::mem::size_of::<TASKDIALOGCONFIG>() as u32,
        dwFlags: flags,
        pszWindowTitle: pcwstr(&title),
        pszMainInstruction: pcwstr(&instruction),
        pszContent: pcwstr(&content),
        pszVerificationText: pcwstr(&check),
        cButtons: buttons.len() as u32,
        pButtons: buttons.as_ptr(),
        nDefaultButton: 1,
        ..Default::default()
    };
    let mut pressed = 0i32;
    let mut checked = windows::core::BOOL(0);
    // SAFETY: the config and everything it points to live across the modal call.
    let result =
        unsafe { TaskDialogIndirect(&config, Some(&mut pressed), None, Some(&mut checked)) };
    (result.is_ok() && pressed == 1).then(|| checked.as_bool())
}
