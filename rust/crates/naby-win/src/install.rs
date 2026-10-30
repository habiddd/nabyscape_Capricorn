//! `--install`: the per-user installer (C# `Installer.cs` + `InstallPaths.cs`).
//!
//! `nabyscape.exe` is its own setup. `NabyScape-<rid>-Setup.exe` is the same program with a ZIP
//! archive appended after the PE image (Whistle, the Piper engine and voice, notices); the installer
//! copies the running executable byte for byte to the install folder (so the update helper's
//! SHA-256 check of the installed copy still matches the downloaded setup) and extracts that
//! payload next to it. Without an appended archive the files next to the executable are used.
//!
//! The path rules, the file lists, the payload extraction and the install sequence are pure
//! (they run in tests on every platform; OS work sits behind [`InstallHost`]). The Windows host
//! (shortcuts, registry, running-process check) and the entry point are in `os`.

use std::fmt;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use naby_core::updates::{inspect_pe, Platform};

/// Per-user uninstall registration (`HKCU`), identical to the C# installer.
pub const REGISTRY_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\NabyScape";
/// The pre-rename registration, removed when it points at the folder being replaced.
pub const LEGACY_REGISTRY_KEY: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Uninstall\LumaIsland";
/// The installed program.
pub const EXECUTABLE: &str = "NabyScape.exe";
pub const SHORTCUT: &str = "NabyScape.lnk";
pub const UNINSTALL_SHORTCUT: &str = "Uninstall NabyScape.lnk";
/// Start-menu shortcuts of earlier product names; removed when they point at the old install.
pub const LEGACY_SHORTCUTS: [&str; 2] = ["Luma Island.lnk", "BillyNabila Island.lnk"];
pub const LEGACY_EXECUTABLES: [&str; 2] = ["LumaIsland.exe", "BillyNabilaIsland.exe"];
/// Host files of the intermediate C# split build (`RemoveRetiredHostFiles`).
pub const RETIRED_HOST_FILES: [&str; 5] = [
    "NabyScape.UI.exe",
    "NabyScape.Core.exe",
    "NabyScape.ico",
    "NATIVE-ThirdPartyNotices.txt",
    "RUST-Library-COPYRIGHT.html",
];
/// Files the setup carries next to the executable. Nothing else is ever extracted.
pub const PAYLOAD_FILES: [&str; 10] = [
    "needle.exe",
    "whistle.cact",
    "piper_windows_amd64.zip",
    "es_MX-ald-x_low.onnx",
    "es_MX-ald-x_low.onnx.json",
    "es_MX-ald-x_low.onnx.MODEL_CARD",
    "PIPER-NOTICE.txt",
    "LICENSE",
    "THIRD-PARTY-NOTICES.txt",
    "WHISTLE-NOTICE.txt",
];
/// Notice files of the C# releases, still removed when found.
pub const CSHARP_NOTICE_FILES: [&str; 6] = [
    "LICENSE",
    "LUCIDE-LICENSE.txt",
    "NAudio-LICENSE.txt",
    "System.Speech-LICENSE.txt",
    "DOTNET-LICENSE.txt",
    "DOTNET-ThirdPartyNotices.txt",
];
/// Largest single payload file and the whole extracted payload.
pub const PAYLOAD_FILE_LIMIT: u64 = 256 * 1024 * 1024;
pub const PAYLOAD_TOTAL_LIMIT: u64 = 512 * 1024 * 1024;

/// Every file the installer ever puts into an install folder; moving or removing an
/// installation deletes only these, never unrelated files.
pub fn shipped_files() -> Vec<&'static str> {
    let mut files = vec![EXECUTABLE];
    files.extend(LEGACY_EXECUTABLES);
    files.push("Uninstall.ps1");
    files.extend(CSHARP_NOTICE_FILES);
    files.extend(PAYLOAD_FILES);
    files.sort_unstable();
    files.dedup();
    files
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallError(pub String);

impl fmt::Display for InstallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for InstallError {}

impl From<io::Error> for InstallError {
    fn from(e: io::Error) -> Self {
        Self(e.to_string())
    }
}
impl From<naby_core::updates::UpdateError> for InstallError {
    fn from(e: naby_core::updates::UpdateError) -> Self {
        Self(e.to_string())
    }
}
impl From<zip::result::ZipError> for InstallError {
    fn from(e: zip::result::ZipError) -> Self {
        Self(format!("The setup payload is damaged ({e})."))
    }
}

pub type Result<T> = std::result::Result<T, InstallError>;

fn fail<T>(message: &str) -> Result<T> {
    Err(InstallError(message.to_owned()))
}

// ---- Windows path rules (string based, so they are testable everywhere) ----------------------

/// `Path.IsPathFullyQualified` for Windows paths: `X:\...` / `X:/...` or a UNC / device path.
pub fn is_fully_qualified(path: &str) -> bool {
    let b = path.as_bytes();
    if b.len() >= 2 && matches!(b[0], b'\\' | b'/') && matches!(b[1], b'\\' | b'/') {
        return true;
    }
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b[2], b'\\' | b'/')
}

/// `Path.GetFullPath` + `TrimEndingDirectorySeparator` for a fully qualified Windows path:
/// separators become `\`, `.` / `..` / empty parts are resolved, a drive root keeps its `\`.
pub fn normalize_windows_path(path: &str) -> String {
    let text = path.replace('/', "\\");
    let (prefix, rest) = if let Some(rest) = text.strip_prefix("\\\\") {
        ("\\\\".to_owned(), rest)
    } else if text.len() >= 2 && text.as_bytes()[1] == b':' {
        (
            format!("{}\\", &text[..2]),
            text[2..].trim_start_matches('\\'),
        )
    } else {
        (String::new(), text.as_str())
    };
    let mut parts: Vec<&str> = Vec::new();
    for part in rest.split('\\') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    format!("{prefix}{}", parts.join("\\"))
}

/// Case-insensitive path equality after normalization (Windows file systems).
pub fn same_path(a: &str, b: &str) -> bool {
    normalize_windows_path(a).eq_ignore_ascii_case(&normalize_windows_path(b))
}

fn last_component(path: &str) -> &str {
    path.rsplit('\\').next().unwrap_or("")
}

/// Whether `path` is a symbolic link or junction.
pub fn is_link(path: &Path) -> bool {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return false;
    };
    if meta.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const REPARSE_POINT: u32 = 0x400;
        if meta.file_attributes() & REPARSE_POINT != 0 {
            return true;
        }
    }
    false
}

/// C# `InstallPaths`: where the app lives and which folders the installer accepts.
pub struct InstallPaths {
    local_app_data: String,
    is_link: Box<dyn Fn(&str) -> bool>,
}

impl InstallPaths {
    pub fn new(local_app_data: &str, is_link: impl Fn(&str) -> bool + 'static) -> Self {
        Self {
            local_app_data: normalize_windows_path(local_app_data),
            is_link: Box::new(is_link),
        }
    }

    /// The running system (`%LOCALAPPDATA%`, real link checks).
    pub fn system() -> Self {
        let local = naby_core::paths::local_app_data();
        Self::new(&local.to_string_lossy(), |p| is_link(Path::new(p)))
    }

    /// `%LOCALAPPDATA%\Programs\NabyScape`.
    pub fn default_dir(&self) -> String {
        format!("{}\\Programs\\NabyScape", self.local_app_data)
    }

    /// `%LOCALAPPDATA%\Programs\LumaIsland`, where releases before the rename installed.
    pub fn legacy_dir(&self) -> String {
        format!("{}\\Programs\\LumaIsland", self.local_app_data)
    }

    /// The personal data folder, which can never be an install folder.
    pub fn data_dir(&self) -> String {
        format!("{}\\NabyScape", self.local_app_data)
    }

    /// C# `InstallPaths.Validate`: a local, dedicated, link-free `...\NabyScape` folder.
    pub fn validate(&self, path: &str) -> Result<String> {
        if path.trim().is_empty() || !is_fully_qualified(path) || path.starts_with("\\\\") {
            return fail("Choose a local folder with a full drive path.");
        }
        let full = normalize_windows_path(path);
        if full.starts_with("\\\\") {
            return fail("Choose a local folder with a full drive path.");
        }
        if !full.eq_ignore_ascii_case(&self.legacy_dir())
            && !last_component(&full).eq_ignore_ascii_case("NabyScape")
        {
            return fail("Choose a dedicated NabyScape folder, such as D:\\Apps\\NabyScape.");
        }
        let mut cursor = full.as_str();
        loop {
            if (self.is_link)(cursor) {
                return fail("Choose a folder outside symbolic links or junctions.");
            }
            match cursor.rfind('\\') {
                Some(i) if i > 2 => cursor = &cursor[..i],
                Some(2) if cursor.len() > 3 => cursor = &cursor[..3],
                _ => break,
            }
        }
        if full.eq_ignore_ascii_case(&self.data_dir()) {
            return fail(
                "This folder holds your personal data. Choose a different installation folder.",
            );
        }
        Ok(full)
    }

    /// C# `InstallPaths.FromArguments`: `--install-dir <folder>` (exactly once) or `fallback`.
    pub fn from_arguments(&self, args: &[String], fallback: &str) -> Result<String> {
        let Some(index) = args.iter().position(|a| a == "--install-dir") else {
            return self.validate(fallback);
        };
        if index + 1 >= args.len() || args.iter().filter(|a| *a == "--install-dir").count() != 1 {
            return fail("--install-dir needs one installation folder.");
        }
        self.validate(&args[index + 1])
    }

    /// C# `InstallPaths.Current`: the registered `InstallLocation` (current, then pre-rename
    /// key), else the pre-rename folder when it still holds the app, else the default.
    /// `registered` reads `InstallLocation` of a registry key; `exists` tests a file.
    pub fn current(
        &self,
        registered: &dyn Fn(&str) -> Option<String>,
        exists: &dyn Fn(&str) -> bool,
    ) -> String {
        for key in [REGISTRY_KEY, LEGACY_REGISTRY_KEY] {
            if let Some(path) = registered(key) {
                if let Ok(valid) = self.validate(&path) {
                    return valid;
                }
            }
        }
        let legacy = self.legacy_dir();
        if exists(&format!("{legacy}\\{EXECUTABLE}")) {
            legacy
        } else {
            self.default_dir()
        }
    }

    /// C# `InstallPaths.OwnsExecutable`: `path` is a product executable inside `current`.
    pub fn owns_executable(path: &str, current: &str) -> bool {
        let full = normalize_windows_path(path);
        std::iter::once(EXECUTABLE)
            .chain(LEGACY_EXECUTABLES)
            .any(|name| full.eq_ignore_ascii_case(&format!("{current}\\{name}")))
    }
}

// ---- setup source checks ----------------------------------------------------------------------

/// C# `ValidateArchitecture` (the part that reads the setup): the file name must not name
/// another architecture and the PE header must be a Windows executable for `platform`.
pub fn validate_setup_source(source: &Path, platform: &Platform) -> Result<()> {
    let name = source
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    for rid in [Platform::X64.rid, Platform::ARM64.rid] {
        if name.to_ascii_lowercase().contains(rid) && rid != platform.rid {
            return fail("The setup filename does not match its executable architecture. Download the correct official setup.");
        }
    }
    let mut file = File::open(source)?;
    inspect_pe(&mut file, platform.pe_machine, platform.rid).map_err(|_| {
        InstallError(
            "Setup architecture verification failed. Download the correct official setup.".into(),
        )
    })
}

// ---- payload ----------------------------------------------------------------------------------

/// Extracts the files appended to `source` (a ZIP after the PE image) into `target`, then copies
/// whichever payload files sit next to `source` that the archive did not provide. Each file is
/// written to `<name>.new` and renamed over the old one. Returns the file names written.
pub fn extract_payload(source: &Path, target: &Path) -> Result<Vec<String>> {
    let mut written: Vec<String> = Vec::new();
    let mut total = 0u64;
    if let Ok(file) = File::open(source) {
        if let Ok(mut archive) = zip::ZipArchive::new(file) {
            for index in 0..archive.len() {
                let mut entry = archive.by_index(index)?;
                if entry.is_dir() {
                    continue;
                }
                let name = entry.name().to_owned();
                if !PAYLOAD_FILES.contains(&name.as_str()) {
                    return Err(InstallError(format!(
                        "The setup contains an unexpected file ({name})."
                    )));
                }
                let size = entry.size();
                total += size;
                if size > PAYLOAD_FILE_LIMIT || total > PAYLOAD_TOTAL_LIMIT {
                    return fail("The setup payload is larger than expected.");
                }
                let mut bytes = Vec::with_capacity(size as usize);
                entry
                    .by_ref()
                    .take(PAYLOAD_FILE_LIMIT + 1)
                    .read_to_end(&mut bytes)?;
                if bytes.len() as u64 != size {
                    return fail("The setup payload is damaged.");
                }
                write_replacing(&target.join(&name), &bytes)?;
                written.push(name);
            }
        }
    }
    if let Some(folder) = source.parent() {
        for name in PAYLOAD_FILES {
            let from = folder.join(name);
            let to = target.join(name);
            if written.iter().any(|w| w == name) || !from.is_file() || same_file(&from, &to) {
                continue;
            }
            copy_replacing(&from, &to)?;
            written.push(name.to_owned());
        }
    }
    Ok(written)
}

fn same_file(a: &Path, b: &Path) -> bool {
    a.to_string_lossy()
        .eq_ignore_ascii_case(&b.to_string_lossy())
}

fn staging_name(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".new");
    path.with_file_name(name)
}

fn write_replacing(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let staged = staging_name(path);
    let result = File::create(&staged)
        .and_then(|mut f| f.write_all(bytes).and_then(|()| f.flush()))
        .and_then(|()| fs::rename(&staged, path));
    if result.is_err() {
        let _ = fs::remove_file(&staged);
    }
    result
}

/// C# `File.Copy(source, target + ".new"); File.Move(.., target, true)`.
pub fn copy_replacing(from: &Path, to: &Path) -> io::Result<()> {
    let staged = staging_name(to);
    let result = fs::copy(from, &staged).and_then(|_| fs::rename(&staged, to));
    if result.is_err() {
        let _ = fs::remove_file(&staged);
    }
    result
}

// ---- the install sequence ---------------------------------------------------------------------

/// OS work of an installation. The Windows implementation lives in `os`; tests record calls.
pub trait InstallHost {
    /// Fails when a NabyScape process runs from one of `folders` ("Quit NabyScape from the
    /// system tray before updating.").
    fn ensure_not_running(&self, folders: &[PathBuf]) -> Result<()>;
    /// `NabyScape.lnk`, `Uninstall NabyScape.lnk`, and removal of pre-rename shortcuts that
    /// point into `previous`.
    fn create_shortcuts(&self, target: &Path, executable: &Path, previous: &Path) -> Result<()>;
    fn write_uninstall_entry(&self, entry: &UninstallEntry) -> Result<()>;
    /// Writes (`true`) or removes the Run value, and always removes the pre-rename values.
    fn set_startup(&self, enabled: bool, executable: &Path) -> Result<()>;
    /// Deletes the pre-rename uninstall key if it points at `previous`.
    fn remove_legacy_registration(&self, previous: &Path);
}

/// The values under `HKCU\...\Uninstall\NabyScape` (C# names and meanings).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UninstallEntry {
    pub display_version: String,
    pub install_location: String,
    pub display_icon: String,
    pub uninstall_string: String,
    pub quiet_uninstall_string: String,
    pub estimated_size_kb: u32,
}

/// The registry command lines: the installed program in `--uninstall` mode.
pub fn uninstall_commands(executable: &str) -> (String, String) {
    (
        format!("\"{executable}\" --uninstall"),
        format!("\"{executable}\" --uninstall --quiet"),
    )
}

pub struct InstallRequest<'a> {
    /// The validated destination folder.
    pub target: &'a Path,
    /// The folder of the existing registration (`InstallPaths::current`).
    pub previous: &'a Path,
    /// The running setup executable.
    pub source: &'a Path,
    pub startup: bool,
    pub version: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallOutcome {
    pub executable: PathBuf,
    pub payload: Vec<String>,
}

/// Deletes `RETIRED_HOST_FILES` found in `folder`, except `source` itself.
pub fn remove_retired_host_files(folder: &Path, source: &Path) -> io::Result<()> {
    for name in RETIRED_HOST_FILES {
        let path = folder.join(name);
        if !same_file(&path, source) && path.is_file() {
            fs::remove_file(&path)?;
        }
    }
    Ok(())
}

/// C# `RemovePreviousFiles`: moving an installation removes only shipped files, never
/// unrelated ones, and the old folder only when nothing is left in it.
pub fn remove_previous_files(folder: &Path, source: &Path) {
    let _ = remove_retired_host_files(folder, source);
    for name in shipped_files() {
        let path = folder.join(name);
        if same_file(&path, source) {
            continue;
        }
        if path.is_file() {
            let _ = fs::remove_file(&path);
        }
    }
    let _ = fs::remove_dir(folder);
}

/// C# `Installer.Install`.
pub fn install(request: &InstallRequest<'_>, host: &dyn InstallHost) -> Result<InstallOutcome> {
    let InstallRequest {
        target,
        previous,
        source,
        startup,
        version,
    } = *request;
    let source_name = source
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if !source_name.starts_with("nabyscape") {
        return fail("Use NabyScape-Setup.exe to install the app.");
    }
    host.ensure_not_running(&[target.to_owned(), previous.to_owned()])?;
    fs::create_dir_all(target)?;
    let executable = target.join(EXECUTABLE);
    if !same_file(source, &executable) {
        copy_replacing(source, &executable)?;
    }
    remove_retired_host_files(target, source)?;
    // A leftover script uninstaller of the C# releases would be stale and misleading.
    let stale_script = target.join("Uninstall.ps1");
    if stale_script.is_file() {
        let _ = fs::remove_file(stale_script);
    }
    let payload = extract_payload(source, target)?;
    host.create_shortcuts(target, &executable, previous)?;
    let (uninstall_string, quiet_uninstall_string) =
        uninstall_commands(&executable.to_string_lossy());
    let mut size = fs::metadata(&executable).map(|m| m.len()).unwrap_or(0);
    for name in &payload {
        size += fs::metadata(target.join(name))
            .map(|m| m.len())
            .unwrap_or(0);
    }
    host.write_uninstall_entry(&UninstallEntry {
        display_version: version.to_owned(),
        install_location: target.to_string_lossy().into_owned(),
        display_icon: executable.to_string_lossy().into_owned(),
        uninstall_string,
        quiet_uninstall_string,
        estimated_size_kb: (size / 1024).min(u64::from(u32::MAX)) as u32,
    })?;
    host.set_startup(startup, &executable)?;
    host.remove_legacy_registration(previous);
    if !same_file(previous, target) {
        remove_previous_files(previous, source);
    }
    for name in LEGACY_EXECUTABLES {
        let legacy = target.join(name);
        if legacy.is_file() && !same_file(source, &legacy) {
            fs::remove_file(&legacy)?;
        }
    }
    Ok(InstallOutcome {
        executable,
        payload,
    })
}

// ---- command line -------------------------------------------------------------------------------

/// `--silent`, `--launch` and the validated folder of an install run.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InstallArgs {
    pub silent: bool,
    pub launch: bool,
}

pub fn parse_args(args: &[String]) -> InstallArgs {
    InstallArgs {
        silent: args.iter().any(|a| a == "--silent"),
        launch: args.iter().any(|a| a == "--launch"),
    }
}

/// Whether the executable name makes it a setup (`NabyScape-win-x64-Setup.exe`).
pub fn is_setup_name(exe: &str) -> bool {
    let name = exe.rsplit(['\\', '/']).next().unwrap_or(exe);
    let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
    stem.to_ascii_lowercase().ends_with("-setup")
}

#[cfg(windows)]
pub use os::*;

#[cfg(windows)]
mod os {
    use super::*;
    use crate::winops::{self, reg};
    use naby_core::settings::presets;

    /// The folder registered by the installed copy (`InstallPaths.Current`).
    pub fn current_dir() -> String {
        let paths = InstallPaths::system();
        paths.current(&|key| reg::get_string(key, "InstallLocation"), &|file| {
            Path::new(file).is_file()
        })
    }

    /// `OfficialUpdates.IsInstalled`: `path` is a product executable of the installed folder.
    pub fn is_installed(path: &Path) -> bool {
        InstallPaths::owns_executable(&path.to_string_lossy(), &current_dir())
    }

    /// C# `Installer.HasSavedPreferences`.
    pub fn has_saved_preferences() -> bool {
        [
            naby_core::paths::data_dir(),
            naby_core::paths::legacy_data_dir(),
        ]
        .iter()
        .any(|dir| dir.join("settings.json").is_file())
    }

    struct WinHost;

    impl InstallHost for WinHost {
        fn ensure_not_running(&self, folders: &[PathBuf]) -> Result<()> {
            let me = std::process::id();
            let names = [
                "LumaIsland.exe",
                "BillyNabilaIsland.exe",
                "NabyScape.exe",
                "NabyScape.UI.exe",
                "NabyScape.Core.exe",
            ];
            for process in winops::processes() {
                if process.pid == me {
                    continue;
                }
                let Some(image) = crate::devices::process_image(process.pid) else {
                    continue;
                };
                let running_there = folders.iter().any(|folder| {
                    names
                        .iter()
                        .any(|name| same_path(&image, &folder.join(name).to_string_lossy()))
                });
                if running_there {
                    return fail("Quit NabyScape from the system tray before updating.");
                }
            }
            Ok(())
        }

        fn create_shortcuts(
            &self,
            target: &Path,
            executable: &Path,
            previous: &Path,
        ) -> Result<()> {
            let programs = winops::programs_folder().ok_or_else(|| {
                InstallError("The Windows shortcut service is unavailable.".into())
            })?;
            winops::create_shortcut(
                &programs.join(SHORTCUT),
                executable,
                "",
                target,
                "NabyScape for Windows",
                executable,
            )?;
            winops::create_shortcut(
                &programs.join(UNINSTALL_SHORTCUT),
                executable,
                "--uninstall",
                target,
                "Remove NabyScape and optionally all downloaded speech models and personal data",
                executable,
            )?;
            for name in LEGACY_SHORTCUTS {
                let link = programs.join(name);
                if !link.is_file() {
                    continue;
                }
                if let Some(info) = winops::read_shortcut(&link) {
                    let owned = std::iter::once(EXECUTABLE)
                        .chain(LEGACY_EXECUTABLES)
                        .any(|exe| same_path(&info.target, &previous.join(exe).to_string_lossy()));
                    if owned {
                        let _ = fs::remove_file(&link);
                    }
                }
            }
            Ok(())
        }

        fn write_uninstall_entry(&self, entry: &UninstallEntry) -> Result<()> {
            let key = REGISTRY_KEY;
            reg::set_string(key, "DisplayName", "NabyScape")?;
            reg::set_string(key, "DisplayVersion", &entry.display_version)?;
            reg::set_string(key, "Publisher", "NabyScape")?;
            reg::set_string(key, "InstallLocation", &entry.install_location)?;
            reg::set_string(key, "DisplayIcon", &entry.display_icon)?;
            reg::set_string(key, "UninstallString", &entry.uninstall_string)?;
            reg::set_string(key, "QuietUninstallString", &entry.quiet_uninstall_string)?;
            reg::set_dword(key, "NoModify", 1)?;
            reg::set_dword(key, "NoRepair", 1)?;
            reg::set_dword(key, "EstimatedSize", entry.estimated_size_kb)?;
            Ok(())
        }

        fn set_startup(&self, enabled: bool, executable: &Path) -> Result<()> {
            use crate::startup::{command_line, LEGACY_VALUE_NAMES, RUN_KEY, VALUE_NAME};
            if enabled {
                reg::set_string(
                    RUN_KEY,
                    VALUE_NAME,
                    &command_line(&executable.to_string_lossy()),
                )?;
            } else {
                reg::delete_value(RUN_KEY, VALUE_NAME)?;
            }
            for legacy in LEGACY_VALUE_NAMES {
                reg::delete_value(RUN_KEY, legacy)?;
            }
            Ok(())
        }

        fn remove_legacy_registration(&self, previous: &Path) {
            let registered = reg::get_string(LEGACY_REGISTRY_KEY, "InstallLocation");
            if registered.is_some_and(|old| same_path(&old, &previous.to_string_lossy())) {
                let _ = reg::delete_tree(LEGACY_REGISTRY_KEY);
            }
        }
    }

    fn launch(folder: &Path) -> io::Result<()> {
        std::process::Command::new(folder.join(EXECUTABLE))
            .spawn()
            .map(|_| ())
    }

    /// The Run value for an install that keeps the user's choice: enabled already, or a fresh
    /// profile whose recommended preset starts with Windows.
    fn default_startup() -> bool {
        crate::startup::enabled() || (!has_saved_preferences() && presets::recommended().auto_start)
    }

    /// Installs from the running executable; returns the installed folder.
    pub fn install_here(target: &str, startup: Option<bool>) -> Result<PathBuf> {
        let source = std::env::current_exe()?;
        let platform = Platform::current().ok_or_else(|| {
            InstallError(
                "Use the NabyScape setup matching your Windows architecture (x64 or ARM64).".into(),
            )
        })?;
        validate_setup_source(&source, &platform)?;
        let previous = current_dir();
        let target_path = PathBuf::from(target);
        let outcome = install(
            &InstallRequest {
                target: &target_path,
                previous: Path::new(&previous),
                source: &source,
                startup: startup.unwrap_or_else(default_startup),
                version: env!("CARGO_PKG_VERSION"),
            },
            &WinHost,
        )?;
        let _ = outcome;
        Ok(target_path)
    }

    /// Entry point of `--install` (and of a `...-Setup.exe` name). Returns the exit code.
    pub fn run(args: &[String]) -> i32 {
        let flags = parse_args(args);
        let _com = crate::svc_util::ComGuard::sta();
        let paths = InstallPaths::system();
        let current = current_dir();
        if flags.silent {
            let result = paths.from_arguments(args, &current).and_then(|target| {
                let folder = install_here(&target, None)?;
                if flags.launch {
                    launch(&folder)?;
                }
                Ok(())
            });
            return match result {
                Ok(()) => 0,
                Err(error) => {
                    let log = std::env::current_exe()
                        .ok()
                        .and_then(|e| e.parent().map(|p| p.join("install-error.txt")));
                    if let Some(log) = log {
                        let _ = fs::write(log, error.to_string());
                    }
                    1
                }
            };
        }
        interactive(args, &paths, &current)
    }

    /// The interactive setup: the same choices as the WPF window (folder, start with Windows,
    /// open afterwards) as system dialogs.
    fn interactive(args: &[String], paths: &InstallPaths, current: &str) -> i32 {
        const TITLE: &str = "Install NabyScape";
        let suggested = if same_path(current, &paths.legacy_dir()) {
            paths.default_dir()
        } else {
            current.to_owned()
        };
        let mut folder = match paths.from_arguments(args, &suggested) {
            Ok(folder) => folder,
            Err(error) => {
                winops::message(
                    TITLE,
                    &format!("Installation failed: {error}"),
                    winops::Icon::Error,
                );
                return 1;
            }
        };
        loop {
            let text = format!(
                "NabyScape\nA little space. More possibilities.\n\nInstalls for this Windows user only. Your personal settings are kept separately.\n\nInstall folder:\n{folder}\n\nYes: install here\nNo: choose another folder\nCancel: quit"
            );
            match winops::ask(TITLE, &text, winops::Buttons::YesNoCancel) {
                winops::Answer::Yes => break,
                winops::Answer::No => {
                    if let Some(picked) = winops::pick_folder("Choose where to install NabyScape") {
                        let picked = picked.trim_end_matches('\\').to_owned();
                        let named = last_component(&picked).eq_ignore_ascii_case("NabyScape");
                        let candidate = if named {
                            picked
                        } else {
                            format!("{picked}\\NabyScape")
                        };
                        match paths.validate(&candidate) {
                            Ok(valid) => folder = valid,
                            Err(error) => {
                                winops::message(TITLE, &error.to_string(), winops::Icon::Error)
                            }
                        }
                    }
                }
                _ => return 0,
            }
        }
        let startup_default = default_startup();
        let startup = winops::ask(
            TITLE,
            "Start NabyScape with Windows?",
            if startup_default {
                winops::Buttons::YesNoDefaultYes
            } else {
                winops::Buttons::YesNoDefaultNo
            },
        ) == winops::Answer::Yes;
        match install_here(&folder, Some(startup)) {
            Ok(target) => {
                let open = winops::ask(
                    TITLE,
                    "NabyScape is installed. Open it now?",
                    winops::Buttons::YesNoDefaultYes,
                ) == winops::Answer::Yes;
                if open {
                    let _ = launch(&target);
                }
                0
            }
            Err(error) => {
                winops::message(
                    TITLE,
                    &format!("Installation failed: {error}"),
                    winops::Icon::Error,
                );
                1
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    const LOCAL: &str = r"C:\Users\Naby\AppData\Local";

    fn paths() -> InstallPaths {
        InstallPaths::new(LOCAL, |_| false)
    }

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn installer_accepts_dedicated_custom_folders_and_rejects_root_data_relative_and_network_paths()
    {
        let p = paths();
        let custom = r"D:\Temp\NabyScape-install-fixture\NabyScape";
        assert!(p.default_dir().ends_with(r"\Programs\NabyScape"));
        assert_eq!(p.validate(custom).unwrap(), custom);
        assert_eq!(
            p.from_arguments(
                &args(&["--silent", "--install-dir", custom]),
                &p.default_dir()
            )
            .unwrap(),
            custom
        );
        assert_eq!(
            p.from_arguments(&[], &p.legacy_dir()).unwrap(),
            p.legacy_dir()
        );
        for bad in [
            r"C:\",
            r"C:\Users",
            "NabyScape",
            r"\\server\Apps\NabyScape",
            "//server/Apps/NabyScape",
            r"C:NabyScape",
            "",
            "   ",
            r"C:\Users\Naby\AppData\Local\NabyScape",
            r"c:\users\naby\appdata\local\nabyscape",
            r"C:\Users\Naby\AppData\Local\Programs\NabyScape\..\Other",
        ] {
            assert!(p.validate(bad).is_err(), "{bad} was accepted");
        }
        assert!(p.from_arguments(&args(&["--install-dir"]), custom).is_err());
        assert!(p
            .from_arguments(
                &args(&["--install-dir", custom, "--install-dir", custom]),
                custom
            )
            .is_err());
    }

    #[test]
    fn paths_are_normalized_like_get_full_path() {
        let p = paths();
        assert_eq!(
            p.validate(r"D:/Apps/./Tools/../NabyScape/").unwrap(),
            r"D:\Apps\NabyScape"
        );
        assert_eq!(
            p.validate(r"d:\apps\nabyscape").unwrap(),
            r"d:\apps\nabyscape"
        );
        assert_eq!(normalize_windows_path(r"C:\"), r"C:\");
        assert_eq!(normalize_windows_path(r"C:\a\..\.."), r"C:\");
        assert!(same_path(r"C:\A\b\", r"c:\a\B"));
    }

    #[test]
    fn legacy_folder_is_accepted_whatever_its_name() {
        let p = paths();
        assert_eq!(p.validate(&p.legacy_dir()).unwrap(), p.legacy_dir());
    }

    #[test]
    fn links_and_junctions_are_rejected_at_any_ancestor() {
        let p = InstallPaths::new(LOCAL, |path| path.eq_ignore_ascii_case(r"D:\Junction"));
        assert!(p.validate(r"D:\Junction\Apps\NabyScape").is_err());
        assert!(p.validate(r"D:\Other\NabyScape").is_ok());
    }

    #[test]
    fn current_prefers_the_registered_valid_location() {
        let p = paths();
        let registered = |key: &str| (key == REGISTRY_KEY).then(|| r"D:\Apps\NabyScape".to_owned());
        assert_eq!(p.current(&registered, &|_| false), r"D:\Apps\NabyScape");
        // An invalid registration is skipped, the pre-rename key is tried next.
        let legacy_key = |key: &str| match key {
            REGISTRY_KEY => Some("garbage".to_owned()),
            LEGACY_REGISTRY_KEY => Some(r"E:\Old\NabyScape".to_owned()),
            _ => None,
        };
        assert_eq!(p.current(&legacy_key, &|_| false), r"E:\Old\NabyScape");
        // Nothing registered: the pre-rename folder wins only while it holds the app.
        assert_eq!(p.current(&|_| None, &|_| false), p.default_dir());
        assert_eq!(
            p.current(&|_| None, &|f| f.ends_with(r"LumaIsland\NabyScape.exe")),
            p.legacy_dir()
        );
    }

    #[test]
    fn product_executables_of_the_current_folder_are_owned() {
        let current = r"D:\Apps\NabyScape";
        assert!(InstallPaths::owns_executable(
            r"d:\apps\nabyscape\NABYSCAPE.EXE",
            current
        ));
        assert!(InstallPaths::owns_executable(
            r"D:\Apps\NabyScape\LumaIsland.exe",
            current
        ));
        assert!(!InstallPaths::owns_executable(
            r"D:\Elsewhere\NabyScape.exe",
            current
        ));
        assert!(!InstallPaths::owns_executable(
            r"D:\Apps\NabyScape\other.exe",
            current
        ));
    }

    #[test]
    fn registry_commands_run_the_installed_program() {
        let (normal, quiet) = uninstall_commands(r"D:\Apps\NabyScape\NabyScape.exe");
        assert_eq!(normal, r#""D:\Apps\NabyScape\NabyScape.exe" --uninstall"#);
        assert_eq!(
            quiet,
            r#""D:\Apps\NabyScape\NabyScape.exe" --uninstall --quiet"#
        );
        assert_eq!(
            REGISTRY_KEY,
            r"Software\Microsoft\Windows\CurrentVersion\Uninstall\NabyScape"
        );
    }

    #[test]
    fn setup_names_and_flags() {
        assert!(is_setup_name(
            r"C:\Users\n\Downloads\NabyScape-win-x64-Setup.exe"
        ));
        assert!(is_setup_name("nabyscape-setup.EXE"));
        assert!(!is_setup_name(r"C:\Apps\NabyScape\NabyScape.exe"));
        assert!(!is_setup_name("NabyScape-Setup-helper.exe"));
        assert_eq!(
            parse_args(&args(&["--install", "--silent", "--launch"])),
            InstallArgs {
                silent: true,
                launch: true
            }
        );
        assert_eq!(parse_args(&args(&["--install"])), InstallArgs::default());
    }

    #[test]
    fn shipped_files_cover_every_installed_name_once() {
        let files = shipped_files();
        for name in [
            "NabyScape.exe",
            "LumaIsland.exe",
            "BillyNabilaIsland.exe",
            "Uninstall.ps1",
            "LICENSE",
            "needle.exe",
            "whistle.cact",
            "piper_windows_amd64.zip",
            "PIPER-NOTICE.txt",
        ] {
            assert_eq!(files.iter().filter(|f| **f == name).count(), 1, "{name}");
        }
    }

    // ---- a recording host over a temp directory ----

    #[derive(Default)]
    struct Recorder {
        calls: RefCell<Vec<String>>,
        running: bool,
    }

    impl InstallHost for Recorder {
        fn ensure_not_running(&self, folders: &[PathBuf]) -> Result<()> {
            self.calls
                .borrow_mut()
                .push(format!("running-check {}", folders.len()));
            if self.running {
                return fail("Quit NabyScape from the system tray before updating.");
            }
            Ok(())
        }
        fn create_shortcuts(&self, _t: &Path, _e: &Path, _p: &Path) -> Result<()> {
            self.calls.borrow_mut().push("shortcuts".into());
            Ok(())
        }
        fn write_uninstall_entry(&self, entry: &UninstallEntry) -> Result<()> {
            self.calls.borrow_mut().push(format!(
                "uninstall {} {} kb={}",
                entry.display_version, entry.install_location, entry.estimated_size_kb
            ));
            Ok(())
        }
        fn set_startup(&self, enabled: bool, _e: &Path) -> Result<()> {
            self.calls.borrow_mut().push(format!("startup {enabled}"));
            Ok(())
        }
        fn remove_legacy_registration(&self, _p: &Path) {
            self.calls.borrow_mut().push("legacy-registration".into());
        }
    }

    fn setup_with_payload(path: &Path, exe: &[u8], files: &[(&str, &[u8])]) {
        let mut bytes = exe.to_vec();
        if !files.is_empty() {
            let mut cursor = io::Cursor::new(Vec::new());
            {
                let mut writer = zip::ZipWriter::new(&mut cursor);
                let options = zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Stored);
                for (name, data) in files {
                    writer.start_file(*name, options).unwrap();
                    writer.write_all(data).unwrap();
                }
                writer.finish().unwrap();
            }
            bytes.extend(cursor.into_inner());
        }
        fs::write(path, bytes).unwrap();
    }

    #[test]
    fn install_copies_the_exe_extracts_the_payload_and_registers() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("NabyScape-win-x64-Setup.exe");
        setup_with_payload(
            &source,
            b"MZ-fake-exe",
            &[
                ("needle.exe", b"exe-bytes"),
                ("whistle.cact", b"model"),
                ("LICENSE", b"mit"),
            ],
        );
        let target = dir.path().join("Apps").join("NabyScape");
        let previous = dir.path().join("Programs").join("NabyScape");
        let host = Recorder::default();
        let outcome = install(
            &InstallRequest {
                target: &target,
                previous: &previous,
                source: &source,
                startup: true,
                version: "3.0.0",
            },
            &host,
        )
        .unwrap();
        // The installed program is the setup, byte for byte (the update helper hashes it).
        assert_eq!(
            fs::read(&outcome.executable).unwrap(),
            fs::read(&source).unwrap()
        );
        assert_eq!(fs::read(target.join("needle.exe")).unwrap(), b"exe-bytes");
        assert_eq!(fs::read(target.join("whistle.cact")).unwrap(), b"model");
        assert_eq!(fs::read(target.join("LICENSE")).unwrap(), b"mit");
        assert!(!target.join("whistle.cact.new").exists());
        let calls = host.calls.borrow();
        assert_eq!(calls[0], "running-check 2");
        assert_eq!(calls[1], "shortcuts");
        assert!(calls[2].starts_with("uninstall 3.0.0 "));
        assert_eq!(&calls[3..], ["startup true", "legacy-registration"]);
    }

    #[test]
    fn install_is_refused_while_the_app_runs_and_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("NabyScape-Setup.exe");
        setup_with_payload(&source, b"MZ", &[]);
        let target = dir.path().join("NabyScape");
        let host = Recorder {
            running: true,
            ..Recorder::default()
        };
        let error = install(
            &InstallRequest {
                target: &target,
                previous: &target,
                source: &source,
                startup: false,
                version: "3.0.0",
            },
            &host,
        )
        .unwrap_err();
        assert!(error.0.contains("Quit NabyScape"));
        assert!(!target.exists());
    }

    #[test]
    fn install_requires_a_nabyscape_named_source() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("random.exe");
        fs::write(&source, b"MZ").unwrap();
        let target = dir.path().join("NabyScape");
        let error = install(
            &InstallRequest {
                target: &target,
                previous: &target,
                source: &source,
                startup: false,
                version: "3.0.0",
            },
            &Recorder::default(),
        )
        .unwrap_err();
        assert_eq!(error.0, "Use NabyScape-Setup.exe to install the app.");
    }

    #[test]
    fn payload_falls_back_to_files_next_to_the_executable() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("NabyScape.exe");
        fs::write(&source, b"MZ-plain").unwrap();
        fs::write(dir.path().join("needle.exe"), b"sibling exe").unwrap();
        fs::write(dir.path().join("unrelated.txt"), b"x").unwrap();
        let target = dir.path().join("out");
        fs::create_dir(&target).unwrap();
        let written = extract_payload(&source, &target).unwrap();
        assert_eq!(written, ["needle.exe"]);
        assert_eq!(fs::read(target.join("needle.exe")).unwrap(), b"sibling exe");
        assert!(!target.join("unrelated.txt").exists());
    }

    #[test]
    fn payload_rejects_unexpected_names_and_path_tricks() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["evil.dll", "../needle.exe", "sub/whistle.cact"] {
            let source = dir.path().join("NabyScape-Setup.exe");
            setup_with_payload(&source, b"MZ", &[(name, b"x")]);
            let target = dir.path().join("out");
            fs::create_dir_all(&target).unwrap();
            assert!(extract_payload(&source, &target).is_err(), "{name}");
            assert_eq!(fs::read_dir(&target).unwrap().count(), 0, "{name}");
        }
    }

    #[test]
    fn moving_an_installation_removes_only_shipped_files() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("old");
        fs::create_dir(&old).unwrap();
        for name in [
            "NabyScape.exe",
            "Uninstall.ps1",
            "LICENSE",
            "whistle.cact",
            "NabyScape.UI.exe",
            "keep-me.txt",
        ] {
            fs::write(old.join(name), b"x").unwrap();
        }
        let source = dir.path().join("NabyScape-Setup.exe");
        fs::write(&source, b"MZ").unwrap();
        remove_previous_files(&old, &source);
        let left: Vec<_> = fs::read_dir(&old)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(left, ["keep-me.txt"]);
        fs::remove_file(old.join("keep-me.txt")).unwrap();
        remove_previous_files(&old, &source);
        assert!(!old.exists(), "an emptied folder is removed");
    }

    #[test]
    fn setup_source_must_be_a_pe_for_this_architecture() {
        let dir = tempfile::tempdir().unwrap();
        let mut pe = vec![0u8; 0x100];
        pe[0] = b'M';
        pe[1] = b'Z';
        pe[0x3C] = 0x80;
        pe[0x80..0x84].copy_from_slice(&0x4550u32.to_le_bytes());
        pe[0x84..0x86].copy_from_slice(&Platform::X64.pe_machine.to_le_bytes());
        let good = dir.path().join("NabyScape-win-x64-Setup.exe");
        fs::write(&good, &pe).unwrap();
        validate_setup_source(&good, &Platform::X64).unwrap();
        assert!(validate_setup_source(&good, &Platform::ARM64).is_err());
        // The file name must not name another architecture.
        let renamed = dir.path().join("NabyScape-win-arm64-Setup.exe");
        fs::write(&renamed, &pe).unwrap();
        assert!(validate_setup_source(&renamed, &Platform::X64).is_err());
        let junk = dir.path().join("NabyScape-Setup.exe");
        fs::write(
            &junk,
            b"not a pe at all, but long enough to read a header from .......................",
        )
        .unwrap();
        assert!(validate_setup_source(&junk, &Platform::X64).is_err());
    }
}
