//! The assistant window: a resizable top-level window with DWM styling and the app's own caption
//! bar, painted from `naby_ui::assistant::AssistantWindowView` (C# `AssistantWindow`,
//! `AssistantWindowChrome`, `AssistantApp`, `AssistantRuntime` host half).
//!
//! The window exists only while it is open. Closing it stops the conversation, drops the view,
//! the canvas and the glyph cache; only the persisted options (the shared `AssistantService`)
//! stay. Nothing polls: the view's `next_frame` arms one `WM_TIMER` at a time, and worker
//! threads (replies, previews, provider checks, attachment import) call the app `Waker`.
//!
//! Window procedures only record input and post `HostEvent::Assistant`. The one exception is
//! `WM_SIZE`, which repaints synchronously so a live resize never shows stale pixels; it uses
//! `try_borrow_mut`, so it can never re-enter state the app loop is using.
//!
//! The geometry rules (initial placement, display change reflow) are pure and tested here.

use crate::monitors::RectI;

/// Window size in design units when it first opens (`Place(this, settings, 720, 740)`).
pub const DESIRED_SIZE: (f32, f32) = (720.0, 740.0);

/// Where a new window goes and its size limits, all in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub min_width: i32,
    pub min_height: i32,
}

/// `AssistantWindowChrome.Place`: centered horizontally on the work area, near the top
/// (12..60 DIP), never larger than the work area minus a 24 DIP margin.
pub fn place(work: RectI, scale: f32, desired: (f32, f32)) -> Placement {
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let work_w = work.width() as f32 / scale;
    let work_h = work.height() as f32 / scale;
    let avail_w = (work_w - 24.0).max(240.0);
    let avail_h = (work_h - 24.0).max(180.0);
    let width = desired.0.min(avail_w);
    let height = desired.1.min(avail_h);
    let left = work.left as f32 / scale + (work_w - width) / 2.0;
    let top = work.top as f32 / scale + ((work_h - height) / 2.0).clamp(12.0, 60.0);
    let px = |dip: f32| (dip * scale).round() as i32;
    Placement {
        x: px(left),
        y: px(top),
        width: px(width),
        height: px(height),
        min_width: px(360.0f32.min(avail_w)),
        min_height: px(300.0f32.min(avail_h)),
    }
}

/// `Reflow` after a display, DPI or location change: shrink to fit the work area (minus 16 DIP)
/// and move back inside it. `rect` and `work` are physical pixels.
pub fn reflow(rect: RectI, work: RectI, scale: f32) -> RectI {
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let margin = (16.0 * scale) as i32;
    let max_w = (work.width() - margin).max((240.0 * scale) as i32);
    let max_h = (work.height() - margin).max((180.0 * scale) as i32);
    let width = rect.width().min(max_w);
    let height = rect.height().min(max_h);
    let left = rect
        .left
        .clamp(work.left, (work.right - width).max(work.left));
    let top = rect
        .top
        .clamp(work.top, (work.bottom - height).max(work.top));
    RectI::new(left, top, left + width, top + height)
}

/// Delay in whole milliseconds until `at` (a host clock time in seconds), never zero.
pub fn timer_millis(at: f64, now: f64) -> u32 {
    ((at - now) * 1000.0).ceil().clamp(1.0, 86_400_000.0) as u32
}

/// The attachment file dialog filter, `description\0patterns\0...\0\0`.
pub fn open_filter() -> Vec<u16> {
    let text = "Supported attachments\0*.png;*.jpg;*.jpeg;*.gif;*.bmp;*.tif;*.tiff;*.webp;*.pdf;*.mp4;*.webm;*.mov;*.mkv;*.avi;*.mp3;*.wav;*.m4a;*.ogg;*.flac;*.docx;*.pptx;*.xlsx;*.odt;*.txt;*.md;*.csv;*.tsv;*.json;*.xml;*.py;*.cs;*.js;*.ts;*.log\0All files\0*.*\0\0";
    text.encode_utf16().collect()
}

/// Splits the buffer `GetOpenFileNameW` fills with `ALLOWMULTISELECT | EXPLORER`: one NUL
/// terminated path, or a directory followed by file names, ending at a double NUL.
pub fn parse_selection(buffer: &[u16]) -> Vec<String> {
    let mut parts: Vec<String> = Vec::new();
    let mut start = 0;
    for (i, unit) in buffer.iter().enumerate() {
        if *unit == 0 {
            if i == start {
                break;
            }
            parts.push(String::from_utf16_lossy(&buffer[start..i]));
            start = i + 1;
        }
    }
    match parts.len() {
        0 => Vec::new(),
        1 => parts,
        _ => {
            let directory = parts.remove(0);
            parts
                .into_iter()
                .map(|name| format!("{}\\{name}", directory.trim_end_matches('\\')))
                .collect()
        }
    }
}

/// `Save…` suggestions: a safe file name with the extension of the attachment's type.
pub fn save_name(name: &str, mime: &str, is_image: bool, is_text: bool) -> (String, &'static str) {
    let extension = match mime {
        "image/jpeg" => ".jpg",
        "image/webp" => ".webp",
        "image/gif" => ".gif",
        "application/pdf" => ".pdf",
        "video/webm" => ".webm",
        "video/quicktime" => ".mov",
        "video/x-matroska" => ".mkv",
        "video/x-msvideo" => ".avi",
        "video/mp4" => ".mp4",
        "audio/mpeg" => ".mp3",
        "audio/wav" => ".wav",
        "audio/mp4" => ".m4a",
        "audio/ogg" => ".ogg",
        "audio/flac" => ".flac",
        _ if is_image => ".png",
        _ if is_text => ".txt",
        _ => ".bin",
    };
    let mut clean: String = name
        .chars()
        .map(|c| {
            if "<>:\"/\\|?*".contains(c) || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    if clean.trim().is_empty() {
        clean = "Attachment".to_owned();
    }
    if !clean.to_ascii_lowercase().ends_with(extension) {
        let stem = clean
            .rsplit_once('.')
            .map_or(clean.as_str(), |(s, _)| s)
            .to_owned();
        clean = format!("{stem}{extension}");
    }
    (clean, extension)
}

#[cfg(windows)]
pub use os::{AssistantHost, HostKeys};

#[cfg(windows)]
mod os {
    use super::*;
    use crate::capture::{self, CaptureTarget, TargetKind};
    use crate::clipboard::{read_paste, set_image_png, PasteContent};
    use crate::credentials::{AssistantKeys, SecretStore};
    use crate::event::HostEvent;
    use crate::monitors::{choose_monitor, MonitorInfo};
    use crate::wake::Waker;
    use crate::widget_bridge::WidgetBridge;
    use crate::window::{
        ensure_class, map_key, rgba_to_bgra, set_user_data, user_data, wheel_pixels,
        InputAccumulator,
    };
    use naby_core::assistant;
    use naby_core::assistant::persistence::Keys;
    use naby_core::command::Command;
    use naby_core::settings::Settings;
    use naby_core::store::Store;
    use naby_render::{Canvas, Fonts, InputState, Theme};
    use naby_ui::assistant::chat::HostRequest;
    use naby_ui::assistant::island::summary_of;
    use naby_ui::assistant::ports::{CommandQueue, ScreenSource, ShareFrame, ShareTarget};
    use naby_ui::assistant::service::{AssistantService, ServiceConfig};
    use naby_ui::assistant::session::ChatSession;
    use naby_ui::assistant::widget_tools::WidgetTools;
    use naby_ui::assistant::window::{
        chrome_hit, AssistantWindowView, Chrome, WindowCommand, WindowEnv, WindowOut,
    };
    use naby_ui::host::AssistantSummary;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
    use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
    use std::sync::mpsc::Sender;
    use std::sync::{Arc, Mutex};
    use std::time::Instant;
    use windows::core::{w, PCWSTR, PWSTR};
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
    use windows::Win32::Graphics::Dwm::DwmSetWindowAttribute;
    use windows::Win32::Graphics::Gdi::{
        BeginPaint, EndPaint, GetDC, ReleaseDC, SetDIBitsToDevice, BITMAPINFO, BITMAPINFOHEADER,
        BI_RGB, DIB_RGB_COLORS, PAINTSTRUCT,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::Controls::Dialogs::{
        GetOpenFileNameW, GetSaveFileNameW, OFN_ALLOWMULTISELECT, OFN_EXPLORER, OFN_FILEMUSTEXIST,
        OFN_NOCHANGEDIR, OFN_OVERWRITEPROMPT, OFN_PATHMUSTEXIST, OPENFILENAMEW,
    };
    use windows::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetKeyState, ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT,
        VK_CONTROL, VK_MENU, VK_SHIFT,
    };
    use windows::Win32::UI::WindowsAndMessaging::WM_ACTIVATE;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, GetWindowRect, IsIconic,
        IsZoomed, KillTimer, LoadCursorW, SetCursor, SetForegroundWindow, SetTimer,
        SetWindowDisplayAffinity, SetWindowPos, ShowWindow, SystemParametersInfoW, HTBOTTOM,
        HTBOTTOMLEFT, HTBOTTOMRIGHT, HTCAPTION, HTCLIENT, HTLEFT, HTRIGHT, HTTOP, HTTOPLEFT,
        HTTOPRIGHT, IDC_ARROW, MINMAXINFO, NCCALCSIZE_PARAMS, SIZE_MINIMIZED,
        SPI_GETWHEELSCROLLLINES, SWP_NOACTIVATE, SWP_NOZORDER, SW_MAXIMIZE, SW_MINIMIZE,
        SW_RESTORE, SW_SHOW, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, WINDOW_DISPLAY_AFFINITY,
        WINDOW_EX_STYLE, WM_CAPTURECHANGED, WM_CHAR, WM_CLOSE, WM_DISPLAYCHANGE, WM_DPICHANGED,
        WM_ERASEBKGND, WM_GETMINMAXINFO, WM_KEYDOWN, WM_KILLFOCUS, WM_LBUTTONDOWN, WM_LBUTTONUP,
        WM_MOUSEHWHEEL, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NCCALCSIZE, WM_NCDESTROY, WM_NCHITTEST,
        WM_PAINT, WM_SETCURSOR, WM_SIZE, WM_SYSKEYDOWN, WM_TIMER, WS_CLIPCHILDREN,
        WS_OVERLAPPEDWINDOW,
    };

    const CLASS: PCWSTR = w!("NabyScape.Assistant");
    const TIMER_ID: usize = 1;
    const WM_MOUSELEAVE: u32 = 0x02A3;
    /// `WDA_EXCLUDEFROMCAPTURE`: the window never appears in screen sharing.
    const EXCLUDE_FROM_CAPTURE: u32 = 0x11;

    // ---- keys ----------------------------------------------------------------------------

    /// Provider API keys in the Windows Credential Manager.
    pub struct HostKeys;

    fn credential(error: crate::credentials::CredentialError) -> assistant::AssistantError {
        assistant::AssistantError::invalid(error.to_string())
    }

    impl Keys for HostKeys {
        fn read(&self, id: &str) -> assistant::Result<Option<String>> {
            AssistantKeys::new().read(id).map_err(credential)
        }
        fn exists(&self, id: &str) -> assistant::Result<bool> {
            AssistantKeys::new().exists(id).map_err(credential)
        }
        fn write(&self, id: &str, value: &str) -> assistant::Result<()> {
            AssistantKeys::new().write(id, value).map_err(credential)
        }
        fn remove(&self, id: &str) -> assistant::Result<()> {
            AssistantKeys::new().remove(id).map_err(credential)
        }
    }

    // ---- screen sharing ----------------------------------------------------------------------

    /// Window and display capture behind the share picker.
    struct HostScreen {
        selected: Mutex<Option<CaptureTarget>>,
        /// The assistant window itself is never offered for sharing.
        own: AtomicIsize,
    }

    impl ScreenSource for HostScreen {
        fn targets(&self) -> Vec<ShareTarget> {
            let own = format!("w:{}", self.own.load(Ordering::Relaxed));
            capture::list_targets()
                .into_iter()
                .filter(|t| t.id != own)
                .map(|t| ShareTarget {
                    display: t.kind == TargetKind::Display,
                    id: t.id,
                    name: t.name,
                })
                .collect()
        }

        fn select(&self, id: &str) -> Result<String, String> {
            let target = capture::list_targets()
                .into_iter()
                .find(|t| t.id == id)
                .ok_or("That window or display is no longer available.")?;
            let name = target.name.clone();
            *self.selected.lock().unwrap_or_else(|e| e.into_inner()) = Some(target);
            Ok(name)
        }

        fn stop(&self) {
            *self.selected.lock().unwrap_or_else(|e| e.into_inner()) = None;
        }

        fn sharing(&self) -> Option<String> {
            self.selected
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
                .map(|t| t.name.clone())
        }

        fn alive(&self) -> bool {
            self.selected
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
                .is_some_and(capture::is_alive)
        }

        fn capture(&self, _: &AtomicBool) -> Result<ShareFrame, String> {
            let target = self
                .selected
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
                .ok_or("Choose a window or display to share.")?;
            let frame = capture::capture_now(&target)?;
            Ok(ShareFrame {
                data_url: frame.data_url(),
                width: frame.width,
                height: frame.height,
            })
        }
    }

    // ---- window state ----------------------------------------------------------------------------

    struct Frame {
        width: u32,
        height: u32,
        pixels: Vec<u32>,
    }

    struct Inner {
        view: AssistantWindowView,
        canvas: Canvas,
        fonts: Fonts,
        settings: Settings,
        theme: Theme,
    }

    struct Shared {
        tx: Sender<HostEvent>,
        started: Instant,
        hwnd: Cell<HWND>,
        input: RefCell<InputAccumulator>,
        queued: Cell<bool>,
        tracking: Cell<bool>,
        frame: RefCell<Frame>,
        minimized: Cell<bool>,
        active: Cell<bool>,
        closing: Cell<bool>,
        paste: Cell<bool>,
        drops: RefCell<Vec<String>>,
        pending: RefCell<WindowOut>,
        inner: RefCell<Inner>,
    }

    impl Shared {
        fn notify(&self) {
            if !self.queued.replace(true) {
                let _ = self.tx.send(HostEvent::Assistant);
            }
        }

        fn now(&self) -> f64 {
            self.started.elapsed().as_secs_f64()
        }

        fn dpi(&self) -> u32 {
            // SAFETY: plain query of our own window.
            unsafe { GetDpiForWindow(self.hwnd.get()) }.max(96)
        }

        fn client(&self) -> (u32, u32) {
            let mut rect = RECT::default();
            // SAFETY: fills a local RECT for our window.
            let _ = unsafe { GetClientRect(self.hwnd.get(), &mut rect) };
            (
                (rect.right - rect.left).max(0) as u32,
                (rect.bottom - rect.top).max(0) as u32,
            )
        }

        /// Paints one frame with `input` and shows it. Re-entrant calls (the app loop holds the
        /// state) are skipped; the next event repaints.
        fn render(&self, input: Option<InputState>) {
            let Ok(mut inner) = self.inner.try_borrow_mut() else {
                return;
            };
            let hwnd = self.hwnd.get();
            if self.minimized.get() {
                inner.view.on_hide();
                inner.canvas.release();
                inner.fonts.clear_cache();
                *self.frame.borrow_mut() = Frame {
                    width: 0,
                    height: 0,
                    pixels: Vec::new(),
                };
                // SAFETY: stops our own timer.
                unsafe {
                    let _ = KillTimer(Some(hwnd), TIMER_ID);
                }
                return;
            }
            let (width, height) = self.client();
            if width == 0 || height == 0 {
                return;
            }
            let now = self.now();
            let input = input.unwrap_or_else(|| InputState {
                time: now,
                ..InputState::default()
            });
            let scale = self.dpi() as f32 / 96.0;
            // SAFETY: plain query of our own window.
            let maximized = unsafe { IsZoomed(hwnd) }.as_bool();
            let Inner {
                view,
                canvas,
                fonts,
                settings,
                theme,
            } = &mut *inner;
            canvas.resize(width, height);
            let env = WindowEnv {
                settings,
                theme,
                scale,
                maximized,
                visible: true,
                active: self.active.get(),
            };
            let mut out = WindowOut::default();
            view.frame(canvas, fonts, &input, &env, &mut out);
            {
                let mut frame = self.frame.borrow_mut();
                frame.width = width;
                frame.height = height;
                frame.pixels.resize((width * height) as usize, 0);
                rgba_to_bgra(canvas.data(), &mut frame.pixels);
            }
            self.blit(hwnd);
            let mut pending = self.pending.borrow_mut();
            pending.commands.append(&mut out.commands);
            pending.requests.append(&mut out.requests);
            pending.window.append(&mut out.window);
            pending.clipboard = out.clipboard.take().or(pending.clipboard.take());
            let next = if view.dirty() {
                Some(now)
            } else {
                view.next_frame(now)
            };
            // SAFETY: arms or stops our own one-shot timer.
            unsafe {
                match next {
                    Some(at) => {
                        SetTimer(Some(hwnd), TIMER_ID, timer_millis(at, now), None);
                    }
                    None => {
                        let _ = KillTimer(Some(hwnd), TIMER_ID);
                    }
                }
            }
        }

        fn blit(&self, hwnd: HWND) {
            let frame = self.frame.borrow();
            if frame.width == 0 || frame.height == 0 {
                return;
            }
            // SAFETY: draws the cached BGRA frame into our own window's DC.
            unsafe {
                let dc = GetDC(Some(hwnd));
                if dc.is_invalid() {
                    return;
                }
                paint_frame(dc, &frame);
                ReleaseDC(Some(hwnd), dc);
            }
        }
    }

    /// # Safety
    /// `dc` must be a valid device context.
    unsafe fn paint_frame(dc: windows::Win32::Graphics::Gdi::HDC, frame: &Frame) {
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: frame.width as i32,
                biHeight: -(frame.height as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        // SAFETY: the pixel buffer holds width*height 32-bit pixels for the described DIB.
        unsafe {
            SetDIBitsToDevice(
                dc,
                0,
                0,
                frame.width,
                frame.height,
                0,
                0,
                0,
                frame.height,
                frame.pixels.as_ptr().cast(),
                &info,
                DIB_RGB_COLORS,
            );
        }
    }

    fn shared_of<'a>(hwnd: HWND) -> Option<&'a Shared> {
        let ptr = user_data(hwnd) as *const Shared;
        // SAFETY: produced by Rc::into_raw in `open`, cleared at WM_NCDESTROY before the owning
        // handle drops its count.
        (!ptr.is_null()).then(|| unsafe { &*ptr })
    }

    fn lo(l: LPARAM) -> i32 {
        (l.0 & 0xFFFF) as i16 as i32
    }

    fn hi(l: LPARAM) -> i32 {
        ((l.0 >> 16) & 0xFFFF) as i16 as i32
    }

    fn down(vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY) -> bool {
        // SAFETY: plain keyboard state query.
        let state = unsafe { GetKeyState(vk.0 as i32) };
        state < 0
    }

    unsafe extern "system" fn wndproc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        let Some(s) = shared_of(hwnd) else {
            // SAFETY: default handling before the state exists and after it is gone.
            return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
        };
        match msg {
            WM_NCCALCSIZE if wparam.0 != 0 => {
                // The whole window is client area; a maximized window is pulled back inside the
                // monitor by the invisible frame width.
                // SAFETY: lparam points at the NCCALCSIZE_PARAMS the system passes.
                unsafe {
                    if IsZoomed(hwnd).as_bool() {
                        let params = &mut *(lparam.0 as *mut NCCALCSIZE_PARAMS);
                        let dpi = GetDpiForWindow(hwnd).max(96);
                        let frame = GetSystemMetricsForDpi(
                            windows::Win32::UI::WindowsAndMessaging::SM_CXFRAME,
                            dpi,
                        ) + GetSystemMetricsForDpi(
                            windows::Win32::UI::WindowsAndMessaging::SM_CXPADDEDBORDER,
                            dpi,
                        );
                        let rect = &mut params.rgrc[0];
                        rect.left += frame;
                        rect.right -= frame;
                        rect.top += frame;
                        rect.bottom -= frame;
                    }
                }
                LRESULT(0)
            }
            WM_NCHITTEST => {
                let mut rect = RECT::default();
                // SAFETY: fills a local RECT for our window.
                let _ = unsafe { GetWindowRect(hwnd, &mut rect) };
                let scale = s.dpi() as f32 / 96.0;
                // SAFETY: plain query.
                let maximized = unsafe { IsZoomed(hwnd) }.as_bool();
                let hit = chrome_hit(
                    (lo(lparam) - rect.left) as f32,
                    (hi(lparam) - rect.top) as f32,
                    (rect.right - rect.left) as f32,
                    (rect.bottom - rect.top) as f32,
                    scale,
                    maximized,
                );
                LRESULT(match hit {
                    Chrome::Client => HTCLIENT,
                    Chrome::Caption => HTCAPTION,
                    Chrome::Left => HTLEFT,
                    Chrome::Right => HTRIGHT,
                    Chrome::Top => HTTOP,
                    Chrome::Bottom => HTBOTTOM,
                    Chrome::TopLeft => HTTOPLEFT,
                    Chrome::TopRight => HTTOPRIGHT,
                    Chrome::BottomLeft => HTBOTTOMLEFT,
                    Chrome::BottomRight => HTBOTTOMRIGHT,
                } as isize)
            }
            WM_GETMINMAXINFO => {
                // SAFETY: lparam points at a MINMAXINFO supplied by the system.
                unsafe {
                    let info = &mut *(lparam.0 as *mut MINMAXINFO);
                    let scale = GetDpiForWindow(hwnd).max(96) as f32 / 96.0;
                    info.ptMinTrackSize = POINT {
                        x: (360.0 * scale) as i32,
                        y: (300.0 * scale) as i32,
                    };
                }
                LRESULT(0)
            }
            WM_SETCURSOR => {
                if (lparam.0 & 0xFFFF) as u32 == HTCLIENT {
                    // SAFETY: stock cursor.
                    unsafe { SetCursor(LoadCursorW(None, IDC_ARROW).ok()) };
                    LRESULT(1)
                } else {
                    // SAFETY: default handling shows the resize cursors.
                    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
                }
            }
            WM_SIZE => {
                let minimized = wparam.0 as u32 == SIZE_MINIMIZED;
                let changed = s.minimized.replace(minimized) != minimized;
                if minimized && changed {
                    s.notify();
                } else {
                    s.render(None);
                }
                LRESULT(0)
            }
            WM_PAINT => {
                // SAFETY: standard paint cycle over the cached frame.
                unsafe {
                    let mut paint = PAINTSTRUCT::default();
                    let dc = BeginPaint(hwnd, &mut paint);
                    let frame = s.frame.borrow();
                    if frame.width > 0 {
                        paint_frame(dc, &frame);
                    }
                    let _ = EndPaint(hwnd, &paint);
                }
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_ACTIVATE => {
                let active = (wparam.0 & 0xFFFF) != 0;
                if s.active.replace(active) != active {
                    s.notify();
                }
                LRESULT(0)
            }
            WM_DPICHANGED => {
                // SAFETY: lparam points at the suggested rectangle.
                unsafe {
                    let rect = &*(lparam.0 as *const RECT);
                    let _ = SetWindowPos(
                        hwnd,
                        None,
                        rect.left,
                        rect.top,
                        rect.right - rect.left,
                        rect.bottom - rect.top,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                }
                LRESULT(0)
            }
            WM_DISPLAYCHANGE => {
                s.notify();
                // SAFETY: default handling too.
                unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
            }
            WM_TIMER if wparam.0 == TIMER_ID => {
                // SAFETY: one-shot timer: stop it, the frame re-arms it.
                unsafe {
                    let _ = KillTimer(Some(hwnd), TIMER_ID);
                }
                s.notify();
                LRESULT(0)
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
                {
                    let mut input = s.input.borrow_mut();
                    input.modifiers(down(VK_SHIFT), down(VK_CONTROL));
                    input.pointer_move(lo(lparam) as f32, hi(lparam) as f32);
                }
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
                let pressed = msg == WM_LBUTTONDOWN;
                // SAFETY: capture keeps a drag inside this window until release.
                unsafe {
                    if pressed {
                        SetCapture(hwnd);
                    } else {
                        let _ = ReleaseCapture();
                    }
                }
                {
                    let mut input = s.input.borrow_mut();
                    input.modifiers(down(VK_SHIFT), down(VK_CONTROL));
                    input.button(pressed, lo(lparam) as f32, hi(lparam) as f32);
                }
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
                        Some(&mut lines as *mut u32 as *mut std::ffi::c_void),
                        SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
                    )
                };
                let pixels = wheel_pixels(delta, lines, s.dpi());
                // Wheel messages carry screen coordinates; the pointer position is already known.
                let mut input = s.input.borrow_mut();
                input.modifiers(down(VK_SHIFT), down(VK_CONTROL));
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
                let (shift, ctrl, alt) = (down(VK_SHIFT), down(VK_CONTROL), down(VK_MENU));
                let vk = wparam.0 as u32;
                if let Some(event) = map_key(vk, shift, ctrl, alt) {
                    {
                        let mut input = s.input.borrow_mut();
                        input.modifiers(shift, ctrl);
                        input.key(event);
                    }
                    if ctrl && vk == 0x56 {
                        s.paste.set(true);
                    }
                    s.notify();
                    return LRESULT(0);
                }
                // SAFETY: default handling (system keys, accelerators).
                unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
            }
            WM_CHAR => {
                s.input.borrow_mut().char_unit(wparam.0 as u16);
                s.notify();
                LRESULT(0)
            }
            WM_KILLFOCUS => {
                s.input.borrow_mut().capture_lost();
                LRESULT(0)
            }
            WM_CLOSE => {
                s.closing.set(true);
                s.notify();
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

    // ---- the host --------------------------------------------------------------------------------

    struct OpenWindow {
        shared: Rc<Shared>,
        raw: *const Shared,
        screen: Arc<HostScreen>,
        _drops: Option<crate::dragdrop::DropRegistration>,
    }

    pub struct AssistantHost {
        summary_key: (u64, u64),
        service: AssistantService,
        queue: CommandQueue,
        tools: WidgetTools,
        events: Sender<HostEvent>,
        settings: Settings,
        window: Option<OpenWindow>,
        summary: AssistantSummary,
        seen_service: u64,
        _bridge: Option<WidgetBridge>,
    }

    impl AssistantHost {
        pub fn new(
            store: Arc<Store>,
            waker: Waker,
            events: Sender<HostEvent>,
            settings: &Settings,
        ) -> Self {
            let service =
                AssistantService::new(ServiceConfig::new(store, Arc::new(HostKeys), waker.clone()));
            let queue = CommandQueue::new(waker);
            let tools = WidgetTools::new(queue.clone());
            let mut host = Self {
                summary_key: (0, 0),
                service,
                queue,
                tools,
                events,
                settings: settings.clone(),
                window: None,
                summary: AssistantSummary::default(),
                seen_service: 0,
                _bridge: None,
            };
            host.refresh_tools();
            host._bridge = WidgetBridge::start(Arc::new(host.tools.clone()));
            host
        }

        pub fn service(&self) -> &AssistantService {
            &self.service
        }

        pub fn is_open(&self) -> bool {
            self.window.is_some()
        }

        /// The widget tools and the options moved: recompute the offered pages.
        fn refresh_tools(&mut self) {
            let options = self.service.options();
            self.tools
                .update(&self.settings, options.enabled, options.widget_navigation);
            self.seen_service = self.service.revision();
        }

        pub fn apply_settings(&mut self, settings: &Settings) {
            self.settings = settings.clone();
            self.refresh_tools();
            if let Some(open) = &self.window {
                if let Ok(mut inner) = open.shared.inner.try_borrow_mut() {
                    inner.theme = Theme::from_settings(settings);
                    inner.settings = settings.clone();
                }
                open.shared.render(None);
            }
        }

        /// A display, DPI or work-area change: keep the window inside the visible desktop.
        pub fn display_changed(&mut self, monitors: &[MonitorInfo]) {
            let Some(open) = &self.window else { return };
            let hwnd = open.shared.hwnd.get();
            // SAFETY: plain queries of our own window.
            if unsafe { IsIconic(hwnd) }.as_bool() || unsafe { IsZoomed(hwnd) }.as_bool() {
                return;
            }
            let mut rect = RECT::default();
            // SAFETY: fills a local RECT.
            let _ = unsafe { GetWindowRect(hwnd, &mut rect) };
            let current = RectI::new(rect.left, rect.top, rect.right, rect.bottom);
            let (cx, cy) = current.center();
            let Some(monitor) = crate::monitors::monitor_at(monitors, cx, cy)
                .or_else(|| monitors.iter().position(|m| m.primary))
                .map(|i| &monitors[i])
            else {
                return;
            };
            let target = reflow(current, monitor.work, monitor.scale());
            if target != current {
                // SAFETY: moves our own window.
                unsafe {
                    let _ = SetWindowPos(
                        hwnd,
                        None,
                        target.left,
                        target.top,
                        target.width(),
                        target.height(),
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                }
            }
        }

        /// Opens the window (or brings it forward) on the dock's monitor.
        pub fn open(&mut self, monitors: &[MonitorInfo]) {
            if let Some(open) = &self.window {
                let hwnd = open.shared.hwnd.get();
                // SAFETY: shows and focuses our own window.
                unsafe {
                    if IsIconic(hwnd).as_bool() {
                        let _ = ShowWindow(hwnd, SW_RESTORE);
                    } else {
                        let _ = ShowWindow(hwnd, SW_SHOW);
                    }
                    let _ = SetForegroundWindow(hwnd);
                }
                if let Ok(mut inner) = open.shared.inner.try_borrow_mut() {
                    inner.view.open_chat();
                }
                open.shared.render(None);
                return;
            }
            self.create(monitors);
        }

        fn create(&mut self, monitors: &[MonitorInfo]) {
            let monitor = choose_monitor(
                monitors,
                self.settings.display_mode(),
                &self.settings.monitor,
                None,
            )
            .and_then(|i| monitors.get(i))
            .or_else(|| monitors.iter().find(|m| m.primary))
            .or_else(|| monitors.first());
            let (work, scale) =
                monitor.map_or((RectI::new(0, 0, 1920, 1040), 1.0), |m| (m.work, m.scale()));
            let placement = place(work, scale, DESIRED_SIZE);

            ensure_class(CLASS, Some(wndproc), true);
            let waker_tx = self.events.clone();
            let screen = Arc::new(HostScreen {
                selected: Mutex::new(None),
                own: AtomicIsize::new(0),
            });
            let session = Arc::new(ChatSession::with_widget_tools(
                self.service.clone(),
                self.tools.clone(),
                screen.clone(),
                Arc::new(crate::attachment_prep::HostPreparer),
            ));
            let view = AssistantWindowView::new(self.service.clone(), session);
            let shared = Rc::new(Shared {
                tx: waker_tx,
                started: Instant::now(),
                hwnd: Cell::new(HWND::default()),
                input: RefCell::new(InputAccumulator::default()),
                queued: Cell::new(false),
                tracking: Cell::new(false),
                frame: RefCell::new(Frame {
                    width: 0,
                    height: 0,
                    pixels: Vec::new(),
                }),
                minimized: Cell::new(false),
                active: Cell::new(true),
                closing: Cell::new(false),
                paste: Cell::new(false),
                drops: RefCell::new(Vec::new()),
                pending: RefCell::new(WindowOut::default()),
                inner: RefCell::new(Inner {
                    view,
                    canvas: Canvas::new(1, 1),
                    fonts: Fonts::load_system(),
                    settings: self.settings.clone(),
                    theme: Theme::from_settings(&self.settings),
                }),
            });
            let raw = Rc::into_raw(shared.clone());
            // SAFETY: creates a top-level window on this thread; the user-data pointer set right
            // after stays valid until `close` destroys the window and releases the leaked count.
            let hwnd = unsafe {
                let instance = GetModuleHandleW(None).unwrap_or_default();
                CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    CLASS,
                    w!("NabyScape \u{b7} Companion"),
                    WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
                    placement.x,
                    placement.y,
                    placement.width,
                    placement.height,
                    None,
                    None,
                    Some(instance.into()),
                    None,
                )
            };
            let Ok(hwnd) = hwnd else {
                // SAFETY: releases the count leaked above; the window never existed.
                drop(unsafe { Rc::from_raw(raw) });
                return;
            };
            shared.hwnd.set(hwnd);
            screen.own.store(hwnd.0 as isize, Ordering::Relaxed);
            set_user_data(hwnd, raw as isize);
            // SAFETY: DWM attributes and capture exclusion on our own window (C# chrome).
            unsafe {
                let dark: i32 = 1;
                let rounded: i32 = 2;
                let _ = DwmSetWindowAttribute(
                    hwnd,
                    windows::Win32::Graphics::Dwm::DWMWINDOWATTRIBUTE(20),
                    (&dark as *const i32).cast(),
                    4,
                );
                let _ = DwmSetWindowAttribute(
                    hwnd,
                    windows::Win32::Graphics::Dwm::DWMWINDOWATTRIBUTE(33),
                    (&rounded as *const i32).cast(),
                    4,
                );
                let _ =
                    SetWindowDisplayAffinity(hwnd, WINDOW_DISPLAY_AFFINITY(EXCLUDE_FROM_CAPTURE));
            }
            // Files dropped on the window go to the draft.
            let weak = Rc::downgrade(&shared);
            let handler: Rc<dyn Fn(&crate::dragdrop::DropEvent) -> bool> = Rc::new(move |event| {
                let Some(shared) = weak.upgrade() else {
                    return false;
                };
                match event {
                    crate::dragdrop::DropEvent::Drop { files, .. } => {
                        shared.drops.borrow_mut().extend(files.iter().cloned());
                        shared.notify();
                        true
                    }
                    crate::dragdrop::DropEvent::Enter { .. }
                    | crate::dragdrop::DropEvent::Over { .. } => true,
                    crate::dragdrop::DropEvent::Leave => false,
                }
            });
            let registration = crate::dragdrop::register(hwnd, handler).ok();
            // SAFETY: shows and focuses the new window.
            unsafe {
                let _ = ShowWindow(hwnd, SW_SHOW);
                let _ = SetForegroundWindow(hwnd);
            }
            shared.render(None);
            self.window = Some(OpenWindow {
                shared,
                raw,
                screen,
                _drops: registration,
            });
        }

        /// Destroys the window and everything it held (conversation, previews, glyph cache).
        pub fn close(&mut self) {
            let Some(open) = self.window.take() else {
                return;
            };
            if let Ok(mut inner) = open.shared.inner.try_borrow_mut() {
                inner.view.shutdown();
            }
            open.screen.stop();
            let hwnd = open.shared.hwnd.get();
            drop(open._drops);
            // SAFETY: destroys our window (WM_NCDESTROY clears the user data), then releases the
            // strong count that `create` leaked into it.
            unsafe {
                let _ = KillTimer(Some(hwnd), TIMER_ID);
                let _ = DestroyWindow(hwnd);
                drop(Rc::from_raw(open.raw));
            }
        }

        fn minimize_or_restore(&self, command: WindowCommand) {
            let Some(open) = &self.window else { return };
            let hwnd = open.shared.hwnd.get();
            // SAFETY: window state changes on our own window.
            unsafe {
                match command {
                    WindowCommand::Minimize => {
                        let _ = ShowWindow(hwnd, SW_MINIMIZE);
                    }
                    WindowCommand::ToggleMaximize => {
                        let _ = ShowWindow(
                            hwnd,
                            if IsZoomed(hwnd).as_bool() {
                                SW_RESTORE
                            } else {
                                SW_MAXIMIZE
                            },
                        );
                    }
                    WindowCommand::Close => {}
                }
            }
        }

        /// Handles `HostEvent::Assistant`: input, drops, paste, minimize, close. Returns the
        /// commands the dock must run.
        pub fn on_event(&mut self) -> Vec<Command> {
            let Some(open) = &self.window else {
                return Vec::new();
            };
            let shared = open.shared.clone();
            shared.queued.set(false);
            if shared.closing.get() {
                self.close();
                return Vec::new();
            }
            let hwnd = shared.hwnd.get();
            if shared.minimized.get() {
                if let Ok(inner) = shared.inner.try_borrow() {
                    inner.view.session().stop_session();
                }
                shared.render(None);
                return Vec::new();
            }
            // Files dropped on the window and the clipboard (Ctrl+V) become attachments.
            let drops: Vec<String> = std::mem::take(&mut *shared.drops.borrow_mut());
            let mut paste_text = None;
            if shared.paste.replace(false) {
                match read_paste() {
                    Ok(PasteContent::Png(bytes)) => self.session().attach_image(bytes),
                    Ok(PasteContent::Files(files)) => self.session().attach_files(files),
                    Ok(PasteContent::Text(text)) => paste_text = Some(text),
                    Ok(PasteContent::Empty) => {}
                    Err(message) => self.service.set_status(&message),
                }
            }
            if !drops.is_empty() {
                self.session().attach_files(drops);
            }
            let mut input = shared.input.borrow_mut().take(shared.now());
            if let Some(text) = paste_text {
                input.paste = Some(crate::text_clipboard::clean_paste(&text));
            }
            shared.render(Some(input));
            let _ = hwnd;
            self.drain_window()
        }

        fn session(&self) -> Arc<ChatSession> {
            match &self.window {
                Some(open) => match open.shared.inner.try_borrow() {
                    Ok(inner) => inner.view.session().clone(),
                    Err(_) => unreachable_session(&self.service),
                },
                None => unreachable_session(&self.service),
            }
        }

        /// Runs what the last frames asked for: window buttons, dialogs, clipboard text.
        fn drain_window(&mut self) -> Vec<Command> {
            let Some(open) = &self.window else {
                return Vec::new();
            };
            let shared = open.shared.clone();
            let (commands, requests, window, clipboard) = {
                let mut pending = shared.pending.borrow_mut();
                (
                    std::mem::take(&mut pending.commands),
                    std::mem::take(&mut pending.requests),
                    std::mem::take(&mut pending.window),
                    pending.clipboard.take(),
                )
            };
            if let Some(text) = clipboard {
                crate::text_clipboard::set_text(Some(shared.hwnd.get()), &text);
            }
            for command in window {
                match command {
                    WindowCommand::Close => {
                        self.close();
                        return commands;
                    }
                    other => self.minimize_or_restore(other),
                }
            }
            for request in requests {
                self.request(request);
            }
            commands
        }

        fn request(&self, request: HostRequest) {
            let Some(open) = &self.window else { return };
            let owner = open.shared.hwnd.get();
            match request {
                HostRequest::PickFiles => {
                    let files = pick_files(owner);
                    if !files.is_empty() {
                        self.session().attach_files(files);
                    }
                }
                HostRequest::CopyImage(shown) => {
                    let loader = self.session().media();
                    let queue = self.queue.clone();
                    std::thread::spawn(move || {
                        let cancel = AtomicBool::new(false);
                        let result = loader
                            .load(&shown.file, &cancel)
                            .and_then(|bytes| to_png(&bytes))
                            .and_then(|png| set_image_png(&png));
                        if result.is_err() {
                            queue.push(notify_failure(
                                "Could not copy",
                                "Could not copy this image. Try Save instead.",
                            ));
                        }
                    });
                }
                HostRequest::SaveAttachment(shown) => {
                    let file = &shown.file;
                    let (name, extension) = super::save_name(
                        &file.name,
                        &file.mime,
                        file.is_image(),
                        file.text.is_some(),
                    );
                    let Some(path) = save_dialog(owner, &name, extension) else {
                        return;
                    };
                    let loader = self.session().media();
                    let queue = self.queue.clone();
                    let file = file.clone();
                    std::thread::spawn(move || {
                        let cancel = AtomicBool::new(false);
                        let bytes = match &file.text {
                            Some(text) => Ok(text.clone().into_bytes()),
                            None => loader.load(&file, &cancel),
                        };
                        let result =
                            bytes.and_then(|b| std::fs::write(&path, b).map_err(|e| e.to_string()));
                        if result.is_err() {
                            queue.push(notify_failure(
                                "Could not save",
                                "Could not save the file. Check its link and the save folder.",
                            ));
                        }
                    });
                }
            }
        }

        /// A worker published something (reply, preview, check) or a widget tool queued a
        /// command. Returns commands for the dock and the dock summary when it changed.
        pub fn on_wake(&mut self) -> (Vec<Command>, Option<AssistantSummary>) {
            if self.seen_service != self.service.revision() {
                self.refresh_tools();
            }
            let mut commands = self.queue.drain();
            if let Some(open) = &self.window {
                let shared = open.shared.clone();
                let dirty = shared
                    .inner
                    .try_borrow()
                    .map(|inner| inner.view.dirty())
                    .unwrap_or(false);
                if dirty && !shared.minimized.get() {
                    shared.render(None);
                    commands.append(&mut self.drain_window());
                }
            }
            let summary = self.compute_summary();
            let changed = (summary != self.summary).then(|| summary.clone());
            self.summary = summary;
            (commands, changed)
        }

        /// The dock summary, recomputed only when the conversation or options changed.
        fn compute_summary(&mut self) -> AssistantSummary {
            let Some(open) = &self.window else {
                self.summary_key = (0, 0);
                return AssistantSummary::default();
            };
            let Ok(inner) = open.shared.inner.try_borrow() else {
                return self.summary.clone();
            };
            let session = inner.view.session();
            let key = (session.revision(), self.service.revision());
            if key == self.summary_key {
                return self.summary.clone();
            }
            self.summary_key = key;
            summary_of(&session.snapshot(), false, None)
        }

        pub fn summary(&self) -> &AssistantSummary {
            &self.summary
        }
    }

    impl Drop for AssistantHost {
        fn drop(&mut self) {
            self.close();
        }
    }

    fn unreachable_session(service: &AssistantService) -> Arc<ChatSession> {
        // Only reachable while the view is borrowed re-entrantly, which the callers avoid; a
        // detached session keeps the signature total without touching the window.
        Arc::new(ChatSession::new(
            service.clone(),
            Arc::new(naby_ui::assistant::ports::NoTools),
            Arc::new(naby_ui::assistant::ports::NoScreen),
            Arc::new(naby_ui::assistant::ports::BasicPreparer),
        ))
    }

    fn notify_failure(title: &str, detail: &str) -> Command {
        Command::Notify {
            kind: naby_core::notifications::NoticeKind::Failure,
            title: title.to_owned(),
            detail: detail.to_owned(),
            files: None,
        }
    }

    /// Any supported picture as PNG bytes for the clipboard.
    fn to_png(bytes: &[u8]) -> Result<Vec<u8>, String> {
        if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
            return Ok(bytes.to_vec());
        }
        let image =
            naby_render::Image::decode(bytes, 8192, 8192).ok_or("This image cannot be copied.")?;
        // Decoded pixels are premultiplied; opaque pictures are identical.
        crate::clipboard::encode_png(image.width(), image.height(), image.data())
            .ok_or_else(|| "This image cannot be copied.".to_owned())
    }

    fn pick_files(owner: HWND) -> Vec<String> {
        let mut buffer = vec![0u16; 32 * 1024];
        let filter = open_filter();
        let title: Vec<u16> = "Attach files to Naby\0".encode_utf16().collect();
        let mut dialog = OPENFILENAMEW {
            lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
            hwndOwner: owner,
            lpstrFilter: PCWSTR(filter.as_ptr()),
            lpstrFile: PWSTR(buffer.as_mut_ptr()),
            nMaxFile: buffer.len() as u32,
            lpstrTitle: PCWSTR(title.as_ptr()),
            Flags: OFN_ALLOWMULTISELECT
                | OFN_EXPLORER
                | OFN_FILEMUSTEXIST
                | OFN_PATHMUSTEXIST
                | OFN_NOCHANGEDIR,
            ..Default::default()
        };
        // SAFETY: the dialog structure and the buffers it points to outlive the modal call.
        if unsafe { GetOpenFileNameW(&mut dialog) }.as_bool() {
            parse_selection(&buffer)
        } else {
            Vec::new()
        }
    }

    fn save_dialog(owner: HWND, name: &str, extension: &str) -> Option<String> {
        let mut buffer = vec![0u16; 1024];
        for (i, unit) in name.encode_utf16().take(buffer.len() - 1).enumerate() {
            buffer[i] = unit;
        }
        let filter: Vec<u16> = format!("Attachment (*{extension})\0*{extension}\0\0")
            .encode_utf16()
            .collect();
        let title: Vec<u16> = "Save attachment\0".encode_utf16().collect();
        let default: Vec<u16> = format!("{}\0", extension.trim_start_matches('.'))
            .encode_utf16()
            .collect();
        let mut dialog = OPENFILENAMEW {
            lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
            hwndOwner: owner,
            lpstrFilter: PCWSTR(filter.as_ptr()),
            lpstrFile: PWSTR(buffer.as_mut_ptr()),
            nMaxFile: buffer.len() as u32,
            lpstrTitle: PCWSTR(title.as_ptr()),
            lpstrDefExt: PCWSTR(default.as_ptr()),
            Flags: OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR | OFN_EXPLORER,
            ..Default::default()
        };
        // SAFETY: as in `pick_files`.
        if unsafe { GetSaveFileNameW(&mut dialog) }.as_bool() {
            parse_selection(&buffer).into_iter().next()
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_opens_centered_near_the_top_and_fits_small_screens() {
        let work = RectI::new(0, 0, 1920, 1040);
        let p = place(work, 1.0, DESIRED_SIZE);
        assert_eq!((p.width, p.height), (720, 740));
        assert_eq!(p.x, 600);
        assert_eq!(p.y, 60, "(1040 - 740) / 2 = 150 is capped at 60");
        assert_eq!((p.min_width, p.min_height), (360, 300));
        // A short screen shrinks the window and keeps a 12 DIP gap.
        let short = place(RectI::new(0, 0, 1366, 728), 1.0, DESIRED_SIZE);
        assert_eq!(short.height, 704);
        assert_eq!(short.y, 12);
        // A tiny work area never goes below the minimum usable size.
        let tiny = place(RectI::new(0, 0, 300, 200), 1.0, DESIRED_SIZE);
        assert_eq!((tiny.width, tiny.height), (276, 180));
        // 150% scaling multiplies the pixels but not the design size.
        let big = place(RectI::new(0, 0, 3840, 2080), 2.0, DESIRED_SIZE);
        assert_eq!((big.width, big.height), (1440, 1480));
        assert_eq!(big.x, 1200);
        // Secondary monitors keep their offset.
        let second = place(RectI::new(1920, 0, 3840, 1040), 1.0, DESIRED_SIZE);
        assert_eq!(second.x, 1920 + 600);
        assert!(!format!("{p:?}").is_empty());
    }

    #[test]
    fn reflow_shrinks_and_moves_the_window_back_inside() {
        let work = RectI::new(0, 0, 1280, 720);
        let wide = reflow(RectI::new(-200, 600, 1700, 1500), work, 1.0);
        assert_eq!(wide.width(), 1264);
        assert_eq!(wide.height(), 704);
        assert_eq!((wide.left, wide.top), (0, 16));
        let inside = RectI::new(100, 100, 820, 840 - 140);
        assert_eq!(reflow(inside, work, 1.0), inside);
        let off = reflow(RectI::new(1200, 50, 1920, 600), work, 1.0);
        assert_eq!(off.right, 1280);
        assert_eq!(off.width(), 720);
    }

    #[test]
    fn timers_never_fire_at_zero_and_round_up() {
        assert_eq!(timer_millis(1.0, 1.0), 1);
        assert_eq!(timer_millis(1.0501, 1.0), 51);
        assert_eq!(timer_millis(0.5, 1.0), 1);
        assert_eq!(timer_millis(1.0e12, 0.0), 86_400_000);
    }

    #[test]
    fn multi_select_results_join_directory_and_names() {
        let single: Vec<u16> = "C:\\docs\\a.txt\0\0".encode_utf16().collect();
        assert_eq!(parse_selection(&single), vec!["C:\\docs\\a.txt"]);
        let many: Vec<u16> = "C:\\docs\0a.txt\0b.png\0\0".encode_utf16().collect();
        assert_eq!(
            parse_selection(&many),
            vec!["C:\\docs\\a.txt", "C:\\docs\\b.png"]
        );
        let root: Vec<u16> = "C:\\\0a.txt\0b.txt\0\0".encode_utf16().collect();
        assert_eq!(parse_selection(&root), vec!["C:\\a.txt", "C:\\b.txt"]);
        assert!(parse_selection(&[0, 0]).is_empty());
        assert!(parse_selection(&[]).is_empty());
        let filter = String::from_utf16(&open_filter()).unwrap();
        assert!(filter.starts_with("Supported attachments\0*.png;"));
        assert!(filter.ends_with("All files\0*.*\0\0"));
    }

    #[test]
    fn save_names_are_safe_and_carry_the_right_extension() {
        assert_eq!(
            save_name("chart.png", "image/png", true, false),
            ("chart.png".into(), ".png")
        );
        assert_eq!(
            save_name("photo", "image/jpeg", true, false),
            ("photo.jpg".into(), ".jpg")
        );
        assert_eq!(
            save_name("a/b:c?.jpg", "image/jpeg", true, false).0,
            "a_b_c_.jpg"
        );
        assert_eq!(
            save_name("report.docx", "text/plain", false, true),
            ("report.txt".into(), ".txt")
        );
        assert_eq!(
            save_name("  ", "application/pdf", false, false),
            ("Attachment.pdf".into(), ".pdf")
        );
        assert_eq!(save_name("clip", "video/mp4", false, false).1, ".mp4");
        assert_eq!(
            save_name("blob", "application/octet-stream", false, false).1,
            ".bin"
        );
    }
}
