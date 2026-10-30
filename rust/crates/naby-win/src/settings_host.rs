//! The Settings and Welcome windows: normal resizable top-level windows (not layered) with a DWM
//! dark title bar and a backdrop chosen by `SettingsMaterial`. The content (`SettingsView`,
//! `WelcomeView`) is painted by naby-render into a DIB that `WM_PAINT` blits.
//!
//! Pure rules (dialog specs, monitor labels, host facts, diagnostics text, window sizing) build
//! and test on every platform; the window itself is Windows-only.

use crate::monitors::MonitorInfo;
use naby_core::command::FilePurpose;
use naby_core::settings::Settings;
use naby_ui::settings::bounds::{fit_working_area, initial_bounds, Bounds};
use naby_ui::settings::{HostInfo, MonitorChoice};

/// How a file dialog behaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogMode {
    Open,
    Save,
    Folder,
}

/// Everything a file dialog needs, decided by what the file is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialogSpec {
    pub mode: DialogMode,
    pub title: &'static str,
    /// Filter display name and pattern, `None` for folder pickers.
    pub filter: Option<(&'static str, &'static str)>,
    pub default_name: &'static str,
    pub default_ext: &'static str,
}

/// The dialog for a [`FilePurpose`] (titles and filters of the WPF dialogs).
pub fn dialog_spec(purpose: FilePurpose) -> DialogSpec {
    let spec = |mode, title, filter, default_name, default_ext| DialogSpec {
        mode,
        title,
        filter,
        default_name,
        default_ext,
    };
    match purpose {
        FilePurpose::ImportLayout => spec(
            DialogMode::Open,
            "Import NabyScape layout",
            Some(("NabyScape layout", "*.json")),
            "",
            "",
        ),
        FilePurpose::ExportLayout => spec(
            DialogMode::Save,
            "Export NabyScape layout",
            Some(("NabyScape layout", "*.json")),
            "NabyScape-layout.json",
            "json",
        ),
        FilePurpose::CompletionSound => spec(
            DialogMode::Open,
            "Choose a completion sound (up to 15 seconds)",
            Some(("Wave audio (*.wav)", "*.wav")),
            "",
            "",
        ),
        FilePurpose::DownloadsFolder => {
            spec(DialogMode::Folder, "Monitor folder arrivals", None, "", "")
        }
        FilePurpose::ShortcutFile => spec(
            DialogMode::Open,
            "Choose file",
            Some(("Applications & files", "*.*")),
            "",
            "",
        ),
        FilePurpose::ShortcutFolder => spec(DialogMode::Folder, "Choose folder", None, "", ""),
        FilePurpose::DiagnosticsExport => spec(
            DialogMode::Save,
            "Export diagnostics report",
            Some(("JSON report (*.json)", "*.json")),
            "NabyScape-diagnostics.json",
            "json",
        ),
        FilePurpose::LyricsFile => spec(
            DialogMode::Open,
            "Load local timed lyrics",
            Some(("Timed lyrics (*.lrc)", "*.lrc")),
            "",
            "",
        ),
    }
}

/// `"\\.\DISPLAY1 · 1920 × 1080"` (the WPF monitor chooser entry).
pub fn monitor_choice(m: &MonitorInfo) -> MonitorChoice {
    MonitorChoice {
        device: m.id.clone(),
        label: format!(
            "{} \u{b7} {} \u{d7} {}",
            m.id,
            m.bounds.width(),
            m.bounds.height()
        ),
    }
}

/// The facts the Settings view shows that only the app shell knows.
pub fn host_info(
    settings: &Settings,
    monitors: &[MonitorInfo],
    hotkey_status: [String; 3],
    settings_dir: &str,
    dock_monitor: Option<&MonitorInfo>,
) -> HostInfo {
    let _ = settings;
    let mut info = HostInfo {
        monitors: monitors.iter().map(monitor_choice).collect(),
        hotkey_status,
        settings_dir: settings_dir.to_owned(),
        ..HostInfo::default()
    };
    if let Some(m) = dock_monitor {
        let scale = f64::from(m.scale());
        info.work_area_height = f64::from(m.work.height()) / scale;
        info.work_area_width = f64::from(m.work.width()) / scale;
        info.display_scale_percent = scale * 100.0;
    }
    info.update.status = "Updates are installed with the official setup in this build.".to_owned();
    info.diagnostics_report = diagnostics_report(&info);
    info
}

/// The private diagnostics preview: versions, architecture and coarse counts only.
pub fn diagnostics_report(info: &HostInfo) -> String {
    let report = serde_json::json!({
        "app": "NabyScape",
        "version": info.app_version,
        "platform": info.platform,
        "displays": info.monitors.len(),
        "note": "No usernames, paths, keys, conversations or saved profile content are included.",
    });
    serde_json::to_string_pretty(&report).unwrap_or_default()
}

/// The client size to open with and where, on the work area the cursor is on.
pub fn open_bounds(work: Bounds, scale: f64) -> Bounds {
    fit_working_area(initial_bounds(work, scale), work, scale)
}

#[cfg(windows)]
pub use os::{AuxContent, SettingsHost, SettingsOutput};

#[cfg(windows)]
mod os {
    use super::*;
    use crate::event::{AuxWindow, HostEvent};
    use crate::util::wide;
    use crate::window::{
        ensure_class, map_key, rgba_to_bgra, set_user_data, user_data, wheel_pixels,
        InputAccumulator,
    };
    use naby_core::color::Rgba;
    use naby_core::command::{Command, SettingsRequest};
    use naby_core::settings::BackdropConditions;
    use naby_core::store::Store;
    use naby_render::{Canvas, Fonts};
    use naby_ui::host::Snapshots;
    use naby_ui::settings::{AppChoice, SettingsView};
    use naby_ui::welcome::{WelcomeDone, WelcomeView};
    use std::cell::{Cell, RefCell};
    use std::ffi::c_void;
    use std::rc::Rc;
    use std::sync::mpsc::{channel, Receiver, Sender};
    use std::sync::Arc;
    use std::time::Instant;
    use windows::core::{w, BOOL, PCWSTR};
    use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows::Win32::Graphics::Dwm::{
        DwmExtendFrameIntoClientArea, DwmSetWindowAttribute, DWMWA_SYSTEMBACKDROP_TYPE,
        DWMWA_USE_IMMERSIVE_DARK_MODE,
    };
    use windows::Win32::Graphics::Gdi::{
        BeginPaint, BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, EndPaint,
        GetSysColor, InvalidateRect, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
        DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ, PAINTSTRUCT, SRCCOPY,
    };
    use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_INPROC_SERVER};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
    use windows::Win32::System::Registry::{
        RegGetValueW, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
    };
    use windows::Win32::UI::Accessibility::HIGHCONTRASTW;
    use windows::Win32::UI::Controls::MARGINS;
    use windows::Win32::UI::HiDpi::GetDpiForWindow;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetKeyState, ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT,
        VK_CONTROL, VK_MENU, VK_SHIFT,
    };
    use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
    use windows::Win32::UI::Shell::{
        FileOpenDialog, FileSaveDialog, IFileDialog, IFileOpenDialog, IFileSaveDialog, IShellItem,
        FOS_FORCEFILESYSTEM, FOS_PATHMUSTEXIST, FOS_PICKFOLDERS, SIGDN_FILESYSPATH,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, GetSystemMetrics,
        GetWindowRect, IsIconic, KillTimer, LoadCursorW, SetCursor, SetForegroundWindow, SetTimer,
        SetWindowPos, ShowWindow, SystemParametersInfoW, CW_USEDEFAULT, IDC_ARROW, IDC_HAND,
        MINMAXINFO, SM_REMOTESESSION, SPI_GETHIGHCONTRAST, SPI_GETWHEELSCROLLLINES, SWP_NOACTIVATE,
        SWP_NOZORDER, SW_RESTORE, SW_SHOW, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, WINDOW_EX_STYLE,
        WM_CHAR, WM_DESTROY, WM_DPICHANGED, WM_ERASEBKGND, WM_GETMINMAXINFO, WM_KEYDOWN,
        WM_KILLFOCUS, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEHWHEEL, WM_MOUSEMOVE, WM_MOUSEWHEEL,
        WM_NCDESTROY, WM_PAINT, WM_SETCURSOR, WM_SIZE, WM_SYSKEYDOWN, WM_TIMER,
        WS_OVERLAPPEDWINDOW,
    };

    const CLASS: PCWSTR = w!("NabyScape.AuxWindow");
    const TIMER_ID: usize = 1;
    const WM_MOUSELEAVE: u32 = 0x02A3;
    const WM_CAPTURECHANGED: u32 = 0x0215;
    const WM_ENTERSIZEMOVE: u32 = 0x0231;
    const WM_EXITSIZEMOVE: u32 = 0x0232;
    const WM_DISPLAYCHANGE: u32 = 0x007E;
    const WM_SETTINGCHANGE: u32 = 0x001A;
    const HCF_HIGHCONTRASTON: u32 = 1;
    const COLOR_WINDOW: i32 = 5;
    const COLOR_WINDOWTEXT: i32 = 8;
    const COLOR_HIGHLIGHT: i32 = 13;
    const COLOR_HIGHLIGHTTEXT: i32 = 14;

    /// What the window shows.
    pub enum AuxContent {
        Settings(Box<SettingsView>),
        Welcome(Box<WelcomeView>),
    }

    /// What a pump produced for the application shell.
    #[derive(Debug, Clone, PartialEq)]
    pub enum SettingsOutput {
        /// Save and apply these preferences, then answer with [`SettingsHost::saved`].
        Apply(Box<Settings>),
        /// A command only the shell can run.
        Command(Command),
        /// A settings request the shell must handle (startup entry, updates, ...).
        Request(SettingsRequest),
        /// The Welcome window finished; close it.
        WelcomeDone(WelcomeDone),
    }

    struct Dib {
        bitmap: HBITMAP,
        bits: *mut u32,
        w: u32,
        h: u32,
    }

    struct Shared {
        tx: Sender<HostEvent>,
        kind: AuxWindow,
        input: RefCell<InputAccumulator>,
        queued: Cell<bool>,
        tracking: Cell<bool>,
        hand: Cell<bool>,
        resizing: Cell<bool>,
        /// `(dc, width, height)` of the DIB to blit; `dc == 0` before the first frame.
        blit: Cell<(isize, u32, u32)>,
        started: Instant,
    }

    impl Shared {
        fn notify(&self) {
            if !self.queued.replace(true) {
                let _ = self.tx.send(HostEvent::AuxInput(self.kind));
            }
        }
    }

    pub struct SettingsHost {
        hwnd: HWND,
        shared: Rc<Shared>,
        raw_shared: *const Shared,
        kind: AuxWindow,
        content: AuxContent,
        fonts: Fonts,
        canvas: Canvas,
        dc: HDC,
        default_bitmap: HGDIOBJ,
        dib: Option<Dib>,
        backdrop_key: Option<(String, BackdropConditions)>,
        apps: Option<Receiver<Vec<AppChoice>>>,
        store: Arc<Store>,
        info: HostInfo,
    }

    fn lo(l: LPARAM) -> i32 {
        (l.0 & 0xFFFF) as i16 as i32
    }
    fn hi(l: LPARAM) -> i32 {
        ((l.0 >> 16) & 0xFFFF) as i16 as i32
    }
    fn key_down(vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY) -> bool {
        // SAFETY: plain keyboard state query.
        unsafe { GetKeyState(vk.0 as i32) < 0 }
    }

    fn shared_of<'a>(hwnd: HWND) -> Option<&'a Shared> {
        let ptr = user_data(hwnd) as *const Shared;
        // SAFETY: the pointer comes from Rc::into_raw and is cleared at WM_NCDESTROY before the
        // owning host releases its count.
        (!ptr.is_null()).then(|| unsafe { &*ptr })
    }

    unsafe extern "system" fn wndproc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        let Some(s) = shared_of(hwnd) else {
            // SAFETY: default handling before/after our state exists.
            return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
        };
        match msg {
            WM_ERASEBKGND => LRESULT(1),
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                // SAFETY: standard paint cycle on our window; the source DC outlives the call.
                unsafe {
                    let dc = BeginPaint(hwnd, &mut ps);
                    let (src, w, h) = s.blit.get();
                    if src != 0 && w > 0 && h > 0 {
                        let src = HDC(src as *mut c_void);
                        // Copy, never blend: over a native backdrop the DIB's premultiplied alpha
                        // must replace the last frame, or old frames ghost through it.
                        let _ = BitBlt(dc, 0, 0, w as i32, h as i32, Some(src), 0, 0, SRCCOPY);
                    }
                    let _ = EndPaint(hwnd, &ps);
                }
                LRESULT(0)
            }
            WM_SIZE | WM_DPICHANGED | WM_DISPLAYCHANGE | WM_SETTINGCHANGE => {
                if msg == WM_DPICHANGED {
                    // SAFETY: lparam points at the suggested RECT for this window.
                    let r = unsafe { &*(lparam.0 as *const RECT) };
                    // SAFETY: moves our own window to the suggested rectangle.
                    unsafe {
                        let _ = SetWindowPos(
                            hwnd,
                            None,
                            r.left,
                            r.top,
                            r.right - r.left,
                            r.bottom - r.top,
                            SWP_NOZORDER | SWP_NOACTIVATE,
                        );
                    }
                }
                s.notify();
                if msg == WM_SETTINGCHANGE {
                    // SAFETY: default handling.
                    return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
                }
                LRESULT(0)
            }
            WM_ENTERSIZEMOVE => {
                s.resizing.set(true);
                LRESULT(0)
            }
            WM_EXITSIZEMOVE => {
                s.resizing.set(false);
                s.notify();
                LRESULT(0)
            }
            WM_GETMINMAXINFO => {
                // SAFETY: lparam is a MINMAXINFO for this message.
                let info = unsafe { &mut *(lparam.0 as *mut MINMAXINFO) };
                let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96) as f32 / 96.0;
                info.ptMinTrackSize.x = (480.0 * dpi) as i32;
                info.ptMinTrackSize.y = (300.0 * dpi) as i32;
                LRESULT(0)
            }
            WM_SETCURSOR => {
                if (lparam.0 & 0xFFFF) as u32 == 1 {
                    // SAFETY: stock cursors.
                    unsafe {
                        let id = if s.hand.get() { IDC_HAND } else { IDC_ARROW };
                        SetCursor(LoadCursorW(None, id).ok());
                    }
                    return LRESULT(1);
                }
                // SAFETY: default handling for the non-client area.
                unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
            }
            WM_MOUSEMOVE => {
                if !s.tracking.replace(true) {
                    let mut track = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    // SAFETY: valid TRACKMOUSEEVENT for our window.
                    if unsafe { TrackMouseEvent(&mut track) }.is_err() {
                        s.tracking.set(false);
                    }
                }
                let mut input = s.input.borrow_mut();
                input.modifiers(key_down(VK_SHIFT), key_down(VK_CONTROL));
                input.pointer_move(lo(lparam) as f32, hi(lparam) as f32);
                drop(input);
                s.notify();
                LRESULT(0)
            }
            WM_MOUSELEAVE => {
                s.tracking.set(false);
                s.input.borrow_mut().pointer_leave();
                s.notify();
                LRESULT(0)
            }
            WM_LBUTTONDOWN | WM_LBUTTONUP => {
                let down = msg == WM_LBUTTONDOWN;
                // SAFETY: capture keeps a drag inside this window until release.
                unsafe {
                    if down {
                        SetCapture(hwnd);
                    } else {
                        let _ = ReleaseCapture();
                    }
                }
                let mut input = s.input.borrow_mut();
                input.modifiers(key_down(VK_SHIFT), key_down(VK_CONTROL));
                input.button(down, lo(lparam) as f32, hi(lparam) as f32);
                drop(input);
                s.notify();
                LRESULT(0)
            }
            WM_CAPTURECHANGED => {
                s.input.borrow_mut().capture_lost();
                s.notify();
                LRESULT(0)
            }
            WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
                let delta = ((wparam.0 >> 16) & 0xFFFF) as i16 as i32;
                let mut lines = 3u32;
                // SAFETY: writes one u32 into `lines`.
                let _ = unsafe {
                    SystemParametersInfoW(
                        SPI_GETWHEELSCROLLLINES,
                        0,
                        Some(&mut lines as *mut u32 as *mut c_void),
                        SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
                    )
                };
                // SAFETY: plain query.
                let dpi = unsafe { GetDpiForWindow(hwnd) };
                let pixels = wheel_pixels(delta, lines, dpi);
                let mut input = s.input.borrow_mut();
                input.modifiers(key_down(VK_SHIFT), key_down(VK_CONTROL));
                if msg == WM_MOUSEWHEEL {
                    input.wheel(0.0, pixels);
                } else {
                    input.wheel(pixels, 0.0);
                }
                drop(input);
                s.notify();
                LRESULT(0)
            }
            WM_KEYDOWN | WM_SYSKEYDOWN => {
                let (shift, ctrl, alt) =
                    (key_down(VK_SHIFT), key_down(VK_CONTROL), key_down(VK_MENU));
                let vk = wparam.0 as u32;
                let mut input = s.input.borrow_mut();
                input.modifiers(shift, ctrl);
                if let Some(event) = map_key(vk, shift, ctrl, alt) {
                    input.key(event);
                    drop(input);
                    if ctrl && vk == 0x56 {
                        if let Some(text) = crate::text_clipboard::get_text(Some(hwnd)) {
                            s.input.borrow_mut().paste(text);
                        }
                    }
                    s.notify();
                    return LRESULT(0);
                }
                drop(input);
                // SAFETY: default handling (Alt+F4, system keys).
                unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
            }
            // Typed text, including IME results (the system turns committed IME strings into
            // WM_CHAR messages when WM_IME_CHAR reaches the default procedure).
            WM_CHAR => {
                s.input.borrow_mut().char_unit(wparam.0 as u16);
                s.notify();
                LRESULT(0)
            }
            WM_KILLFOCUS => {
                s.input.borrow_mut().capture_lost();
                s.notify();
                LRESULT(0)
            }
            WM_TIMER => {
                s.notify();
                LRESULT(0)
            }
            WM_DESTROY => {
                let _ = s.tx.send(HostEvent::AuxClosed(s.kind));
                LRESULT(0)
            }
            WM_NCDESTROY => {
                set_user_data(hwnd, 0);
                // SAFETY: final default handling.
                unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
            }
            // SAFETY: default handling for everything else.
            _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
        }
    }

    fn reg_dword(
        root: windows::Win32::System::Registry::HKEY,
        sub: &str,
        name: &str,
    ) -> Option<u32> {
        let (sub, name) = (wide(sub), wide(name));
        let mut value = 0u32;
        let mut size = 4u32;
        // SAFETY: reads one DWORD into a live local.
        let status = unsafe {
            RegGetValueW(
                root,
                PCWSTR(sub.as_ptr()),
                PCWSTR(name.as_ptr()),
                RRF_RT_REG_DWORD,
                None,
                Some(&mut value as *mut u32 as *mut c_void),
                Some(&mut size),
            )
        };
        (status.0 == 0).then_some(value)
    }

    fn windows_build() -> u32 {
        let (sub, name) = (
            wide("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion"),
            wide("CurrentBuildNumber"),
        );
        let mut buf = [0u16; 16];
        let mut size = (buf.len() * 2) as u32;
        // SAFETY: reads a short string into a live buffer.
        let status = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                PCWSTR(sub.as_ptr()),
                PCWSTR(name.as_ptr()),
                RRF_RT_REG_SZ,
                None,
                Some(buf.as_mut_ptr() as *mut c_void),
                Some(&mut size),
            )
        };
        if status.0 != 0 {
            return 0;
        }
        crate::util::from_wide(&buf).trim().parse().unwrap_or(0)
    }

    fn system_high_contrast() -> bool {
        let mut hc = HIGHCONTRASTW {
            cbSize: std::mem::size_of::<HIGHCONTRASTW>() as u32,
            ..Default::default()
        };
        // SAFETY: fills `hc`, whose size is declared in `cbSize`.
        let ok = unsafe {
            SystemParametersInfoW(
                SPI_GETHIGHCONTRAST,
                hc.cbSize,
                Some(&mut hc as *mut HIGHCONTRASTW as *mut c_void),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            )
        };
        ok.is_ok() && hc.dwFlags.0 & HCF_HIGHCONTRASTON != 0
    }

    fn sys_color(index: i32) -> Rgba {
        // SAFETY: plain system color query.
        let c =
            COLORREF(unsafe { GetSysColor(windows::Win32::Graphics::Gdi::SYS_COLOR_INDEX(index)) });
        Rgba::rgb(
            (c.0 & 0xFF) as u8,
            ((c.0 >> 8) & 0xFF) as u8,
            ((c.0 >> 16) & 0xFF) as u8,
        )
    }

    fn conditions() -> BackdropConditions {
        let mut power = SYSTEM_POWER_STATUS::default();
        // SAFETY: fills a live struct.
        let saver =
            unsafe { GetSystemPowerStatus(&mut power) }.is_ok() && power.SystemStatusFlag == 1;
        BackdropConditions {
            supported: windows_build() >= 22621,
            high_contrast: system_high_contrast(),
            transparency: reg_dword(
                HKEY_CURRENT_USER,
                "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize",
                "EnableTransparency",
            )
            .unwrap_or(1)
                != 0,
            energy_saver: saver,
            // SAFETY: plain metric query.
            remote: unsafe { GetSystemMetrics(SM_REMOTESESSION) } != 0,
        }
    }

    fn set_backdrop(hwnd: HWND, kind: i32) -> i32 {
        // SAFETY: passes a live i32 for DWMWA_SYSTEMBACKDROP_TYPE.
        match unsafe {
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_SYSTEMBACKDROP_TYPE,
                &kind as *const i32 as *const c_void,
                4,
            )
        } {
            Ok(()) => 0,
            Err(e) => e.code().0,
        }
    }

    impl SettingsHost {
        /// Creates and shows the window. Returns `None` when it cannot be created.
        pub fn open(
            kind: AuxWindow,
            content: AuxContent,
            store: Arc<Store>,
            tx: Sender<HostEvent>,
            near: Bounds,
            scale: f64,
        ) -> Option<SettingsHost> {
            ensure_class(CLASS, Some(wndproc), true);
            let title = match &content {
                AuxContent::Settings(v) => naby_ui::settings::window_title(
                    naby_core::i18n::normalize_language(&v.settings().ui_language),
                ),
                AuxContent::Welcome(v) => v.title(),
            };
            let size = match kind {
                AuxWindow::Settings => open_bounds(near, scale),
                AuxWindow::Welcome => {
                    let w = (580.0 * scale).min(near.w);
                    let h = (620.0 * scale).min(near.h);
                    Bounds::new(
                        near.x + (near.w - w) / 2.0,
                        near.y + (near.h - h) / 2.0,
                        w,
                        h,
                    )
                }
            };
            let title = wide(&title);
            // SAFETY: creates a top-level window owned by this thread; the user-data pointer
            // set below stays valid until Drop destroys the window.
            let hwnd = unsafe {
                let instance = GetModuleHandleW(None).ok()?;
                CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    CLASS,
                    PCWSTR(title.as_ptr()),
                    WS_OVERLAPPEDWINDOW,
                    size.x as i32,
                    size.y as i32,
                    size.w as i32,
                    size.h as i32,
                    None,
                    None,
                    Some(instance.into()),
                    None,
                )
                .ok()?
            };
            let _ = CW_USEDEFAULT;
            let shared = Rc::new(Shared {
                tx,
                kind,
                input: RefCell::new(InputAccumulator::default()),
                queued: Cell::new(false),
                tracking: Cell::new(false),
                hand: Cell::new(false),
                resizing: Cell::new(false),
                blit: Cell::new((0, 0, 0)),
                started: Instant::now(),
            });
            let raw_shared = Rc::into_raw(shared.clone());
            set_user_data(hwnd, raw_shared as isize);
            // SAFETY: creates a memory DC owned by this host.
            let dc = unsafe { CreateCompatibleDC(None) };
            let dark = BOOL(1);
            // SAFETY: passes a live BOOL for the dark title bar attribute.
            unsafe {
                let _ = DwmSetWindowAttribute(
                    hwnd,
                    DWMWA_USE_IMMERSIVE_DARK_MODE,
                    &dark as *const BOOL as *const c_void,
                    4,
                );
                let _ = ShowWindow(hwnd, SW_SHOW);
            }
            let host = SettingsHost {
                hwnd,
                shared,
                raw_shared,
                kind,
                content,
                fonts: Fonts::load_system(),
                canvas: Canvas::new(1, 1),
                dc,
                default_bitmap: HGDIOBJ::default(),
                dib: None,
                backdrop_key: None,
                apps: None,
                store,
                info: HostInfo::default(),
            };
            host.shared.notify();
            Some(host)
        }

        pub fn hwnd(&self) -> HWND {
            self.hwnd
        }

        pub fn kind(&self) -> AuxWindow {
            self.kind
        }

        pub fn settings_view(&mut self) -> Option<&mut SettingsView> {
            match &mut self.content {
                AuxContent::Settings(v) => Some(v),
                AuxContent::Welcome(_) => None,
            }
        }

        /// Brings an open window to the front (second launch, tray, `--settings`).
        pub fn raise(&self) {
            // SAFETY: plain window management calls on our own window.
            unsafe {
                if IsIconic(self.hwnd).as_bool() {
                    let _ = ShowWindow(self.hwnd, SW_RESTORE);
                }
                let _ = SetForegroundWindow(self.hwnd);
            }
        }

        pub fn set_info(&mut self, info: HostInfo) {
            self.info = info;
        }

        /// The shell saved (or failed to save) the preferences of the last `Apply`.
        pub fn saved(&mut self, ok: bool) {
            if let AuxContent::Settings(v) = &mut self.content {
                v.saved(ok);
            }
            self.shared.notify();
        }

        /// DPI scale of the window's monitor.
        pub fn scale(&self) -> f32 {
            // SAFETY: plain query.
            unsafe { GetDpiForWindow(self.hwnd) }.max(96) as f32 / 96.0
        }

        fn apply_backdrop(&mut self) {
            let AuxContent::Settings(view) = &self.content else {
                return;
            };
            let requested = view.backdrop_request().to_owned();
            let cond = conditions();
            if self.backdrop_key.as_ref() == Some(&(requested.clone(), cond)) {
                return;
            }
            self.backdrop_key = Some((requested.clone(), cond));
            let hwnd = self.hwnd;
            let result = naby_core::settings::resolve_backdrop(&requested, cond, |kind| {
                set_backdrop(hwnd, kind)
            });
            if !result.is_native() {
                set_backdrop(hwnd, 1);
            }
            let native = result.is_native();
            let margins = if native {
                MARGINS {
                    cxLeftWidth: -1,
                    cxRightWidth: -1,
                    cyTopHeight: -1,
                    cyBottomHeight: -1,
                }
            } else {
                MARGINS::default()
            };
            // SAFETY: extends the frame of our own window by a live MARGINS.
            unsafe {
                let _ = DwmExtendFrameIntoClientArea(self.hwnd, &margins);
            }
            self.info.backdrop = Some(result);
            self.info.window_alpha = if native { 0xB0 } else { 255 };
        }

        fn ensure_dib(&mut self, w: u32, h: u32) -> bool {
            if self.dib.as_ref().is_some_and(|d| (d.w, d.h) == (w, h)) {
                return true;
            }
            self.free_dib();
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w as i32,
                    biHeight: -(h as i32),
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut c_void = std::ptr::null_mut();
            // SAFETY: describes a top-down 32-bit DIB; `bits` receives its pixels.
            let bitmap = unsafe {
                CreateDIBSection(Some(self.dc), &info, DIB_RGB_COLORS, &mut bits, None, 0)
            };
            let Ok(bitmap) = bitmap else { return false };
            if bits.is_null() {
                // SAFETY: deletes the bitmap just created.
                let _ = unsafe { DeleteObject(bitmap.into()) };
                return false;
            }
            // SAFETY: selects the new bitmap into our private DC.
            let previous = unsafe { SelectObject(self.dc, bitmap.into()) };
            if self.default_bitmap.is_invalid() {
                self.default_bitmap = previous;
            }
            self.dib = Some(Dib {
                bitmap,
                bits: bits as *mut u32,
                w,
                h,
            });
            true
        }

        fn free_dib(&mut self) {
            self.shared.blit.set((0, 0, 0));
            if let Some(d) = self.dib.take() {
                // SAFETY: the bitmap is deselected before it is deleted.
                unsafe {
                    SelectObject(self.dc, self.default_bitmap);
                    let _ = DeleteObject(d.bitmap.into());
                }
            }
        }

        /// Handles input, timers, resizes and finished background work: runs one frame of the
        /// content, paints it and returns what the shell must do.
        pub fn pump(&mut self, snaps: Option<Snapshots>) -> Vec<SettingsOutput> {
            self.shared.queued.set(false);
            self.poll_apps();
            self.apply_backdrop();
            // SAFETY: plain size query into a live RECT.
            let mut rect = RECT::default();
            let _ = unsafe { GetClientRect(self.hwnd, &mut rect) };
            let (w, h) = (
                (rect.right - rect.left).max(1) as u32,
                (rect.bottom - rect.top).max(1) as u32,
            );
            let scale = self.scale();
            self.canvas.resize(w, h);
            let mut input = self
                .shared
                .input
                .borrow_mut()
                .take(self.shared.started.elapsed().as_secs_f64());
            // The toolkit works in logical pixels.
            input.pointer = input.pointer.map(|(x, y)| (x / scale, y / scale));
            input.wheel = (input.wheel.0 / scale, input.wheel.1 / scale);
            let mut commands: Vec<Command> = Vec::new();
            let mut outputs = Vec::new();
            let (repaint_after, clipboard, hand) = match &mut self.content {
                AuxContent::Settings(view) => {
                    let mut info = self.info.clone();
                    info.system_high_contrast = system_high_contrast().then(|| {
                        [
                            sys_color(COLOR_WINDOW),
                            sys_color(COLOR_WINDOWTEXT),
                            sys_color(COLOR_HIGHLIGHT),
                            sys_color(COLOR_HIGHLIGHTTEXT),
                        ]
                    });
                    view.set_host_info(info);
                    let snaps =
                        snaps.unwrap_or_else(|| Snapshots::new(view.settings().clone(), 0.0));
                    let out = view.frame(
                        &mut self.canvas,
                        &self.fonts,
                        &input,
                        &snaps,
                        scale,
                        &mut commands,
                    );
                    (out.repaint_after, out.clipboard, out.wants_hand)
                }
                AuxContent::Welcome(view) => {
                    let out =
                        view.frame(&mut self.canvas, &self.fonts, &input, scale, &mut commands);
                    if let Some(done) = out.done {
                        outputs.push(SettingsOutput::WelcomeDone(done));
                    }
                    (out.repaint_after, out.clipboard, out.wants_hand)
                }
            };
            self.shared.hand.set(hand);
            if let Some(text) = clipboard {
                let _ = crate::text_clipboard::set_text(Some(self.hwnd), &text);
            }
            self.present(w, h);
            // Commands: dialogs run here, the rest goes to the shell.
            for command in commands {
                match command {
                    Command::ApplySettings(s) => outputs.push(SettingsOutput::Apply(s)),
                    Command::Settings(SettingsRequest::PickFile(purpose)) => {
                        let path = pick_file(self.hwnd, purpose);
                        if let AuxContent::Settings(v) = &mut self.content {
                            v.file_picked(purpose, path);
                        }
                        self.shared.notify();
                    }
                    Command::Settings(SettingsRequest::PickInstalledApp) => self.start_app_scan(),
                    Command::Settings(SettingsRequest::RefreshDiagnostics) => {
                        self.info.diagnostics_report = diagnostics_report(&self.info);
                        self.shared.notify();
                    }
                    Command::Settings(request) => outputs.push(SettingsOutput::Request(request)),
                    other => outputs.push(SettingsOutput::Command(other)),
                }
            }
            // One-shot timer for what the content wants next.
            // SAFETY: timer calls on our own window.
            unsafe {
                match repaint_after {
                    Some(secs) => {
                        let ms = (secs * 1000.0).ceil().clamp(8.0, 60_000.0) as u32;
                        SetTimer(Some(self.hwnd), TIMER_ID, ms, None);
                    }
                    None => {
                        let _ = KillTimer(Some(self.hwnd), TIMER_ID);
                    }
                }
            }
            outputs
        }

        fn present(&mut self, w: u32, h: u32) {
            if !self.ensure_dib(w, h) {
                return;
            }
            let Some(dib) = &self.dib else { return };
            // SAFETY: the DIB holds exactly w*h 32-bit pixels and stays alive during the copy.
            let dst = unsafe { std::slice::from_raw_parts_mut(dib.bits, (w * h) as usize) };
            rgba_to_bgra(self.canvas.data(), dst);
            self.shared.blit.set((self.dc.0 as isize, w, h));
            // SAFETY: invalidates our own window; WM_PAINT blits the DIB.
            unsafe {
                let _ = InvalidateRect(Some(self.hwnd), None, false);
            }
        }

        /// Lists installed apps (Start Menu shortcuts) on a worker thread.
        fn start_app_scan(&mut self) {
            if self.apps.is_some() {
                return;
            }
            self.info.installed_apps = None;
            let (tx, rx) = channel();
            self.apps = Some(rx);
            std::thread::spawn(move || {
                let mut found = Vec::new();
                let cancel = std::sync::atomic::AtomicBool::new(false);
                for var in ["ProgramData", "APPDATA"] {
                    if let Some(base) = std::env::var_os(var) {
                        let dir = std::path::Path::new(&base)
                            .join("Microsoft\\Windows\\Start Menu\\Programs");
                        crate::launcher::scan_shortcuts(&dir, &mut found, &cancel);
                    }
                }
                let apps = crate::launcher::normalize(found)
                    .into_iter()
                    .map(|a| AppChoice {
                        name: a.name,
                        target: a.target,
                        app_id: a.app_id,
                    })
                    .collect();
                let _ = tx.send(apps);
            });
            // Poll for the answer.
            // SAFETY: timer on our own window.
            unsafe {
                SetTimer(Some(self.hwnd), TIMER_ID, 150, None);
            }
        }

        fn poll_apps(&mut self) {
            let Some(rx) = &self.apps else { return };
            match rx.try_recv() {
                Ok(apps) => {
                    self.info.installed_apps = Some(apps);
                    self.apps = None;
                    self.shared.notify();
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    // SAFETY: timer on our own window.
                    unsafe {
                        SetTimer(Some(self.hwnd), TIMER_ID, 150, None);
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.info.installed_apps = Some(Vec::new());
                    self.apps = None;
                }
            }
        }

        /// Keeps the window on its work area after a display or DPI change (`FitSettingsBounds`).
        pub fn fit_to(&self, work: Bounds, scale: f64) {
            let mut r = RECT::default();
            // SAFETY: plain query and move on our own window.
            unsafe {
                if GetWindowRect(self.hwnd, &mut r).is_err() {
                    return;
                }
                let now = Bounds::new(
                    r.left as f64,
                    r.top as f64,
                    (r.right - r.left) as f64,
                    (r.bottom - r.top) as f64,
                );
                let fitted = fit_working_area(now, work, scale);
                if fitted != now {
                    let _ = SetWindowPos(
                        self.hwnd,
                        None,
                        fitted.x as i32,
                        fitted.y as i32,
                        fitted.w as i32,
                        fitted.h as i32,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                }
            }
        }

        /// `true` while the user drags or sizes the window (do not contain it yet).
        pub fn interactive_move(&self) -> bool {
            self.shared.resizing.get()
        }

        pub fn store(&self) -> &Store {
            &self.store
        }
    }

    impl Drop for SettingsHost {
        fn drop(&mut self) {
            if let AuxContent::Settings(v) = &mut self.content {
                v.release();
            }
            if let AuxContent::Welcome(v) = &mut self.content {
                v.release();
            }
            self.free_dib();
            self.canvas.release();
            self.fonts.clear_cache();
            // SAFETY: destroys our window (WM_NCDESTROY clears the user data), deletes the DC,
            // then releases the count leaked into the user data pointer.
            unsafe {
                let _ = KillTimer(Some(self.hwnd), TIMER_ID);
                let _ = DestroyWindow(self.hwnd);
                let _ = DeleteDC(self.dc);
                drop(Rc::from_raw(self.raw_shared));
            }
        }
    }

    /// Shows the dialog for `purpose` and returns the chosen path.
    fn pick_file(owner: HWND, purpose: FilePurpose) -> Option<String> {
        let spec = dialog_spec(purpose);
        // SAFETY: standard COM file dialog use on the UI thread (COM is initialized by the app).
        unsafe {
            let dialog: IFileDialog = match spec.mode {
                DialogMode::Save => CoCreateInstance::<_, IFileSaveDialog>(
                    &FileSaveDialog,
                    None,
                    CLSCTX_INPROC_SERVER,
                )
                .ok()?
                .into(),
                _ => CoCreateInstance::<_, IFileOpenDialog>(
                    &FileOpenDialog,
                    None,
                    CLSCTX_INPROC_SERVER,
                )
                .ok()?
                .into(),
            };
            let title = wide(spec.title);
            dialog.SetTitle(PCWSTR(title.as_ptr())).ok()?;
            let mut options = dialog.GetOptions().ok()?;
            options |= FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST;
            if spec.mode == DialogMode::Folder {
                options |= FOS_PICKFOLDERS;
            }
            dialog.SetOptions(options).ok()?;
            let name = wide(spec.filter.map_or("", |f| f.0));
            let pattern = wide(spec.filter.map_or("", |f| f.1));
            if spec.filter.is_some() {
                let types = [COMDLG_FILTERSPEC {
                    pszName: PCWSTR(name.as_ptr()),
                    pszSpec: PCWSTR(pattern.as_ptr()),
                }];
                dialog.SetFileTypes(&types).ok()?;
            }
            if !spec.default_name.is_empty() {
                let file = wide(spec.default_name);
                let _ = dialog.SetFileName(PCWSTR(file.as_ptr()));
            }
            if !spec.default_ext.is_empty() {
                let ext = wide(spec.default_ext);
                let _ = dialog.SetDefaultExtension(PCWSTR(ext.as_ptr()));
            }
            dialog.Show(Some(owner)).ok()?;
            let item: IShellItem = dialog.GetResult().ok()?;
            let path = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
            let text = path.to_string().ok();
            CoTaskMemFree(Some(path.0 as *const c_void));
            text
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::monitors::RectI;

    fn monitor(id: &str, w: i32, h: i32, dpi: u32) -> MonitorInfo {
        MonitorInfo {
            id: id.to_owned(),
            bounds: RectI {
                left: 0,
                top: 0,
                right: w,
                bottom: h,
            },
            work: RectI {
                left: 0,
                top: 0,
                right: w,
                bottom: h - 40,
            },
            dpi,
            primary: true,
            handle: 1,
        }
    }

    #[test]
    fn dialogs_match_the_wpf_ones() {
        let export = dialog_spec(FilePurpose::ExportLayout);
        assert_eq!(export.mode, DialogMode::Save);
        assert_eq!(export.default_name, "NabyScape-layout.json");
        assert_eq!(
            dialog_spec(FilePurpose::ImportLayout).filter,
            Some(("NabyScape layout", "*.json"))
        );
        assert_eq!(
            dialog_spec(FilePurpose::DownloadsFolder).mode,
            DialogMode::Folder
        );
        assert_eq!(
            dialog_spec(FilePurpose::CompletionSound).filter.unwrap().1,
            "*.wav"
        );
        assert_eq!(
            dialog_spec(FilePurpose::DiagnosticsExport).default_ext,
            "json"
        );
    }

    #[test]
    fn monitor_entries_show_device_and_resolution() {
        let c = monitor_choice(&monitor("\\\\.\\DISPLAY2", 2560, 1440, 96));
        assert_eq!(c.device, "\\\\.\\DISPLAY2");
        assert_eq!(c.label, "\\\\.\\DISPLAY2 \u{b7} 2560 \u{d7} 1440");
    }

    #[test]
    fn host_info_uses_layout_pixels_of_the_dock_display() {
        let m = monitor("D1", 3840, 2160, 192);
        let info = host_info(
            &Settings::default(),
            std::slice::from_ref(&m),
            ["a".into(), "b".into(), "c".into()],
            "C:/data",
            Some(&m),
        );
        assert_eq!(info.work_area_height, (2160.0 - 40.0) / 2.0);
        assert_eq!(info.display_scale_percent, 200.0);
        assert_eq!(info.hotkey_status[1], "b");
        assert!(info.diagnostics_report.contains("NabyScape"));
        assert!(
            !info.diagnostics_report.contains("C:/data"),
            "no paths in the report"
        );
    }

    #[test]
    fn the_window_opens_inside_the_work_area() {
        let work = Bounds::new(0.0, 0.0, 1366.0, 728.0);
        let b = open_bounds(work, 1.0);
        assert!(work.contains(&b));
        assert!(b.w >= 480.0 && b.h >= 300.0);
    }
}
