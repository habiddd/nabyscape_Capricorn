//! Window and display capture for the assistant's screen sharing (C# `AssistantShareCapture`,
//! `AssistantDesktop`, `AssistantScreenTarget` / `AssistantScreenFrame`).
//!
//! C# uses the Windows.Graphics.Capture picker and encodes a JPEG (quality .75) whose longest
//! side is at most 1280 px. This port lists the shareable windows and displays itself
//! ([`list_targets`]) and grabs pixels with `PrintWindow` / `BitBlt` into BGRA, then runs the
//! same pipeline: refuse frames above 128 MiB of pixels, scale down to 1280 px, drop alpha
//! (the C# resize also ignored it), JPEG-encode, and keep the result under the 2 MB picture
//! limit so `Attachment::inline_image` accepts it. The scaling and encoding are pure.
#![cfg_attr(not(windows), allow(dead_code))]

use naby_core::assistant::attachments::{data_url, Attachment, MAXIMUM_PICTURE_BYTES};

/// Longest side of an encoded frame.
pub const MAX_EDGE: u32 = 1280;
/// Raw BGRA frames above this are refused (C# 128 MiB).
pub const MAX_RAW_BYTES: u64 = 128 * 1024 * 1024;
/// JPEG quality of the first attempt (C# `ImageQuality` .75).
pub const JPEG_QUALITY: u8 = 75;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bounds {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    Window,
    Display,
}

/// Something that can be shared (C# `AssistantScreenTarget`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureTarget {
    /// `w:<hwnd>` for windows, `d:<device name>` for displays.
    pub id: String,
    pub name: String,
    pub bounds: Bounds,
    pub kind: TargetKind,
}

/// One encoded frame (C# `AssistantScreenFrame`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedFrame {
    pub target_id: String,
    pub target_name: String,
    pub width: u32,
    pub height: u32,
    pub mime: &'static str,
    pub bytes: Vec<u8>,
}

impl CapturedFrame {
    pub fn data_url(&self) -> String {
        data_url(self.mime, &self.bytes)
    }

    /// The frame as an inline image attachment named after the shared source.
    pub fn to_attachment(&self, note: &str) -> Result<Attachment, String> {
        Attachment::inline_image(&self.target_name, self.mime, &self.bytes, note)
            .map_err(|e| e.to_string())
    }
}

/// Why a capture produced no frame; the messages are the C# wording.
pub const ERR_TOO_LARGE: &str = "Choose a smaller display or window to share.";
pub const ERR_MINIMIZED: &str =
    "The shared source is unavailable or minimized. Restore it, or choose another source.";
pub const ERR_CLOSED: &str = "The shared window closed. Choose another window or display.";
pub const ERR_EMPTY: &str = "Restore the selected window before sharing it.";
pub const ERR_ENCODE: &str = "This image is too complex. Resize it before attaching.";

/// Target size: `min(1, 1280 / longest)` applied to both sides, at least 1 px.
pub fn scaled_size(width: u32, height: u32) -> (u32, u32) {
    let longest = width.max(height).max(1);
    if longest <= MAX_EDGE {
        return (width.max(1), height.max(1));
    }
    let scale = f64::from(MAX_EDGE) / f64::from(longest);
    (
        ((f64::from(width) * scale) as u32).max(1),
        ((f64::from(height) * scale) as u32).max(1),
    )
}

/// Area-averaging downscale of top-down BGRA to tightly packed RGB (alpha ignored).
pub fn downscale_bgra_to_rgb(
    bgra: &[u8],
    width: u32,
    height: u32,
    out_width: u32,
    out_height: u32,
) -> Vec<u8> {
    let (w, h, ow, oh) = (
        width as usize,
        height as usize,
        out_width as usize,
        out_height as usize,
    );
    let mut out = Vec::with_capacity(ow * oh * 3);
    for oy in 0..oh {
        let y0 = oy * h / oh;
        let y1 = ((oy + 1) * h / oh).max(y0 + 1).min(h);
        for ox in 0..ow {
            let x0 = ox * w / ow;
            let x1 = ((ox + 1) * w / ow).max(x0 + 1).min(w);
            let (mut r, mut g, mut b, mut n) = (0u32, 0u32, 0u32, 0u32);
            for y in y0..y1 {
                let row = &bgra[(y * w + x0) * 4..(y * w + x1) * 4];
                for p in row.chunks_exact(4) {
                    b += u32::from(p[0]);
                    g += u32::from(p[1]);
                    r += u32::from(p[2]);
                    n += 1;
                }
            }
            out.extend_from_slice(&[(r / n) as u8, (g / n) as u8, (b / n) as u8]);
        }
    }
    out
}

fn jpeg(rgb: &[u8], width: u32, height: u32, quality: u8) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let encoder = jpeg_encoder::Encoder::new(&mut out, quality);
    encoder
        .encode(
            rgb,
            u16::try_from(width).ok()?,
            u16::try_from(height).ok()?,
            jpeg_encoder::ColorType::Rgb,
        )
        .ok()?;
    Some(out)
}

/// Rejects empty frames and frames above [`MAX_RAW_BYTES`] of BGRA.
pub fn check_size(width: u32, height: u32) -> Result<(), String> {
    if width == 0 || height == 0 {
        return Err(ERR_EMPTY.to_owned());
    }
    if u64::from(width) * u64::from(height) * 4 > MAX_RAW_BYTES {
        return Err(ERR_TOO_LARGE.to_owned());
    }
    Ok(())
}

/// The C# `Encode` pipeline on a top-down BGRA frame: size guard, scale to [`MAX_EDGE`], JPEG at
/// quality 75, then lower quality and smaller sizes until it fits [`MAXIMUM_PICTURE_BYTES`].
pub fn encode_frame(
    target_id: &str,
    target_name: &str,
    bgra: &[u8],
    width: u32,
    height: u32,
) -> Result<CapturedFrame, String> {
    check_size(width, height)?;
    if bgra.len() as u64 != u64::from(width) * u64::from(height) * 4 {
        return Err(ERR_EMPTY.to_owned());
    }
    let (mut w, mut h) = scaled_size(width, height);
    for attempt in 0..8 {
        let rgb = downscale_bgra_to_rgb(bgra, width, height, w, h);
        let quality = match attempt {
            0 | 1 => JPEG_QUALITY,
            2 | 3 => 60,
            _ => 45,
        };
        if let Some(bytes) = jpeg(&rgb, w, h, quality) {
            if bytes.len() as u64 <= MAXIMUM_PICTURE_BYTES {
                return Ok(CapturedFrame {
                    target_id: target_id.to_owned(),
                    target_name: target_name.to_owned(),
                    width: w,
                    height: h,
                    mime: "image/jpeg",
                    bytes,
                });
            }
        }
        if attempt % 2 == 1 {
            w = (w * 3 / 4).max(1);
            h = (h * 3 / 4).max(1);
        }
    }
    Err(ERR_ENCODE.to_owned())
}

#[cfg(windows)]
pub use os::{list_targets, CaptureService, CaptureState};

#[cfg(windows)]
mod os {
    use super::*;
    use crate::svc_util::from_wide;
    use crate::wake::Waker;
    use std::sync::{Arc, Mutex};
    use windows::core::BOOL;
    use windows::Win32::Foundation::{HWND, LPARAM, RECT, TRUE};
    use windows::Win32::Graphics::Dwm::{
        DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS,
    };
    use windows::Win32::Graphics::Gdi::{
        BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, EnumDisplayMonitors,
        GetDC, GetMonitorInfoW, GetWindowDC, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER,
        BI_RGB, CAPTUREBLT, DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ, HMONITOR, MONITORINFO,
        MONITORINFOEXW, SRCCOPY,
    };
    use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};
    use windows::Win32::System::Threading::GetCurrentProcessId;
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindow, GetWindowLongPtrW, GetWindowRect, GetWindowTextW,
        GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible, GWL_EXSTYLE, GW_OWNER,
        WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
    };

    const PW_RENDERFULLCONTENT: u32 = 2;
    const MONITORINFOF_PRIMARY: u32 = 1;

    fn window_id(hwnd: HWND) -> String {
        format!("w:{}", hwnd.0 as isize)
    }

    fn parse_window(id: &str) -> Option<HWND> {
        let value: isize = id.strip_prefix("w:")?.parse().ok()?;
        Some(HWND(value as *mut _))
    }

    fn window_bounds(hwnd: HWND) -> Option<RECT> {
        let mut rect = RECT::default();
        // SAFETY: both calls write into the local RECT whose size is passed.
        unsafe {
            if DwmGetWindowAttribute(
                hwnd,
                DWMWA_EXTENDED_FRAME_BOUNDS,
                (&mut rect as *mut RECT).cast(),
                std::mem::size_of::<RECT>() as u32,
            )
            .is_ok()
                && rect.right > rect.left
            {
                return Some(rect);
            }
            GetWindowRect(hwnd, &mut rect).ok()?;
        }
        Some(rect)
    }

    unsafe extern "system" fn collect_window(hwnd: HWND, lparam: LPARAM) -> BOOL {
        // SAFETY: `lparam` is the &mut Vec passed by `list_targets`, alive for the enumeration.
        let out = unsafe { &mut *(lparam.0 as *mut Vec<CaptureTarget>) };
        // SAFETY: window queries on a handle EnumWindows just produced.
        unsafe {
            if !IsWindowVisible(hwnd).as_bool() {
                return TRUE;
            }
            let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
            let owned = GetWindow(hwnd, GW_OWNER).is_ok_and(|o| !o.0.is_null());
            if ex & WS_EX_TOOLWINDOW.0 != 0 && ex & WS_EX_APPWINDOW.0 == 0
                || owned && ex & WS_EX_APPWINDOW.0 == 0
            {
                return TRUE;
            }
            let mut cloaked = 0u32;
            if DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, (&mut cloaked as *mut u32).cast(), 4)
                .is_ok()
                && cloaked != 0
            {
                return TRUE;
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if pid == GetCurrentProcessId() {
                return TRUE;
            }
            let mut title = [0u16; 256];
            let len = GetWindowTextW(hwnd, &mut title);
            if len <= 0 {
                return TRUE;
            }
            let Some(rect) = window_bounds(hwnd) else {
                return TRUE;
            };
            if rect.right <= rect.left || rect.bottom <= rect.top {
                return TRUE;
            }
            out.push(CaptureTarget {
                id: window_id(hwnd),
                name: from_wide(&title),
                bounds: Bounds {
                    x: rect.left,
                    y: rect.top,
                    width: rect.right - rect.left,
                    height: rect.bottom - rect.top,
                },
                kind: TargetKind::Window,
            });
        }
        TRUE
    }

    unsafe extern "system" fn collect_monitor(
        monitor: HMONITOR,
        _dc: HDC,
        rect: *mut RECT,
        lparam: LPARAM,
    ) -> BOOL {
        // SAFETY: `lparam` is the &mut Vec passed by `list_targets`; `rect` is valid per the API.
        let out = unsafe { &mut *(lparam.0 as *mut Vec<CaptureTarget>) };
        let rect = unsafe { *rect };
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        // SAFETY: MONITORINFOEXW starts with MONITORINFO and cbSize announces the larger struct.
        if unsafe {
            GetMonitorInfoW(
                monitor,
                (&mut info as *mut MONITORINFOEXW).cast::<MONITORINFO>(),
            )
        }
        .as_bool()
        {
            let device = from_wide(&info.szDevice);
            let number = out.iter().filter(|t| t.kind == TargetKind::Display).count() + 1;
            let primary = info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0;
            let (w, h) = (rect.right - rect.left, rect.bottom - rect.top);
            out.push(CaptureTarget {
                id: format!("d:{device}"),
                name: format!(
                    "Display {number}{} ({w}×{h})",
                    if primary { " · main" } else { "" }
                ),
                bounds: Bounds {
                    x: rect.left,
                    y: rect.top,
                    width: w,
                    height: h,
                },
                kind: TargetKind::Display,
            });
        }
        TRUE
    }

    /// Displays first (main display first among them), then application windows in z-order.
    pub fn list_targets() -> Vec<CaptureTarget> {
        let mut displays: Vec<CaptureTarget> = Vec::new();
        let mut windows: Vec<CaptureTarget> = Vec::new();
        // SAFETY: the callbacks only dereference the Vec pointers passed here, which outlive the
        // synchronous enumerations.
        unsafe {
            let _ = EnumDisplayMonitors(
                None,
                None,
                Some(collect_monitor),
                LPARAM((&mut displays as *mut Vec<CaptureTarget>) as isize),
            );
            let _ = EnumWindows(
                Some(collect_window),
                LPARAM((&mut windows as *mut Vec<CaptureTarget>) as isize),
            );
        }
        displays.sort_by_key(|d| !d.name.contains("main"));
        displays.extend(windows);
        displays
    }

    /// Whether the shared source still exists (C# `SourceClosed`).
    pub fn is_alive(target: &CaptureTarget) -> bool {
        match target.kind {
            TargetKind::Display => list_targets().iter().any(|t| t.id == target.id),
            // SAFETY: IsWindow accepts any handle value.
            TargetKind::Window => {
                parse_window(&target.id).is_some_and(|h| unsafe { IsWindow(Some(h)) }.as_bool())
            }
        }
    }

    /// Owns a top-down 32-bit DIB section selected into a memory DC.
    struct Surface {
        dc: HDC,
        bitmap: HBITMAP,
        old: HGDIOBJ,
        pixels: *const u8,
        width: i32,
        height: i32,
    }

    impl Surface {
        fn new(screen: HDC, width: i32, height: i32) -> Option<Self> {
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    biHeight: -height,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            // SAFETY: creates a DIB section and a DC that this value owns and releases in Drop.
            unsafe {
                let dc = CreateCompatibleDC(Some(screen));
                if dc.is_invalid() {
                    return None;
                }
                let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
                let Ok(bitmap) =
                    CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0)
                else {
                    let _ = DeleteDC(dc);
                    return None;
                };
                if bits.is_null() {
                    let _ = DeleteObject(HGDIOBJ(bitmap.0));
                    let _ = DeleteDC(dc);
                    return None;
                }
                let old = SelectObject(dc, HGDIOBJ(bitmap.0));
                Some(Self {
                    dc,
                    bitmap,
                    old,
                    pixels: bits.cast_const().cast(),
                    width,
                    height,
                })
            }
        }

        fn bgra(&self) -> Vec<u8> {
            // SAFETY: the DIB section owns width*height*4 readable bytes while selected.
            unsafe {
                std::slice::from_raw_parts(
                    self.pixels,
                    self.width as usize * self.height as usize * 4,
                )
                .to_vec()
            }
        }
    }

    impl Drop for Surface {
        fn drop(&mut self) {
            // SAFETY: restores the DC's original bitmap, then deletes the objects created in new().
            unsafe {
                SelectObject(self.dc, self.old);
                let _ = DeleteObject(HGDIOBJ(self.bitmap.0));
                let _ = DeleteDC(self.dc);
            }
        }
    }

    fn grab(target: &CaptureTarget) -> Result<(Vec<u8>, u32, u32), String> {
        let bounds = match target.kind {
            TargetKind::Display => target.bounds,
            TargetKind::Window => {
                let hwnd = parse_window(&target.id).ok_or_else(|| ERR_CLOSED.to_owned())?;
                // SAFETY: handle-validity queries.
                unsafe {
                    if !IsWindow(Some(hwnd)).as_bool() {
                        return Err(ERR_CLOSED.to_owned());
                    }
                    if IsIconic(hwnd).as_bool() {
                        return Err(ERR_MINIMIZED.to_owned());
                    }
                }
                let rect = window_bounds(hwnd).ok_or_else(|| ERR_CLOSED.to_owned())?;
                Bounds {
                    x: rect.left,
                    y: rect.top,
                    width: rect.right - rect.left,
                    height: rect.bottom - rect.top,
                }
            }
        };
        check_size(bounds.width.max(0) as u32, bounds.height.max(0) as u32)?;
        // SAFETY: GDI calls on DCs obtained and released in this scope; the surface owns its DC.
        unsafe {
            let screen = GetDC(None);
            if screen.is_invalid() {
                return Err(ERR_MINIMIZED.to_owned());
            }
            let result = (|| {
                let surface = Surface::new(screen, bounds.width, bounds.height)?;
                let ok = if target.kind == TargetKind::Window {
                    let hwnd = parse_window(&target.id)?;
                    // Full content renders layered, DirectX and UWP windows that plain BitBlt misses.
                    let mut drawn =
                        PrintWindow(hwnd, surface.dc, PRINT_WINDOW_FLAGS(PW_RENDERFULLCONTENT))
                            .as_bool();
                    if !drawn {
                        let window_dc = GetWindowDC(Some(hwnd));
                        if !window_dc.is_invalid() {
                            drawn = BitBlt(
                                surface.dc,
                                0,
                                0,
                                bounds.width,
                                bounds.height,
                                Some(window_dc),
                                0,
                                0,
                                SRCCOPY,
                            )
                            .is_ok();
                            ReleaseDC(Some(hwnd), window_dc);
                        }
                    }
                    drawn
                } else {
                    BitBlt(
                        surface.dc,
                        0,
                        0,
                        bounds.width,
                        bounds.height,
                        Some(screen),
                        bounds.x,
                        bounds.y,
                        SRCCOPY | CAPTUREBLT,
                    )
                    .is_ok()
                };
                ok.then(|| (surface.bgra(), bounds.width as u32, bounds.height as u32))
            })();
            ReleaseDC(None, screen);
            result.ok_or_else(|| ERR_MINIMIZED.to_owned())
        }
    }

    /// Grabs and encodes one frame of `target`, blocking.
    pub fn capture_now(target: &CaptureTarget) -> Result<CapturedFrame, String> {
        let (bgra, w, h) = grab(target)?;
        encode_frame(&target.id, &target.name, &bgra, w, h)
    }

    /// What the assistant UI shows.
    #[derive(Debug, Clone)]
    pub enum CaptureState {
        Idle,
        Capturing,
        Frame(Arc<CapturedFrame>),
        Failed(String),
    }

    struct Shared {
        state: Mutex<(u64, CaptureState)>,
        wake: Waker,
    }

    /// Captures on a short-lived worker thread so a hung target window cannot freeze the UI.
    /// A stale result (a newer request, or `stop`) is discarded.
    pub struct CaptureService {
        shared: Arc<Shared>,
        selected: Option<CaptureTarget>,
    }

    impl CaptureService {
        pub fn start(wake: Waker) -> Self {
            Self {
                shared: Arc::new(Shared {
                    state: Mutex::new((0, CaptureState::Idle)),
                    wake,
                }),
                selected: None,
            }
        }

        pub fn selected(&self) -> Option<&CaptureTarget> {
            self.selected.as_ref()
        }

        /// Chooses what to share. Clears the previous frame.
        pub fn select(&mut self, target: CaptureTarget) {
            self.stop();
            self.selected = Some(target);
        }

        /// Stops sharing and drops the frame.
        pub fn stop(&mut self) {
            self.selected = None;
            let mut state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
            state.0 += 1;
            state.1 = CaptureState::Idle;
        }

        /// Whether the shared source is gone.
        pub fn source_closed(&self) -> bool {
            self.selected.as_ref().is_some_and(|t| !is_alive(t))
        }

        /// Starts a capture of the selected source (ignored while one is running).
        pub fn request_frame(&mut self) {
            let Some(target) = self.selected.clone() else {
                let mut state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
                state.1 = CaptureState::Failed("Choose a window or display to share.".to_owned());
                return;
            };
            let generation = {
                let mut state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
                if matches!(state.1, CaptureState::Capturing) {
                    return;
                }
                state.0 += 1;
                state.1 = CaptureState::Capturing;
                state.0
            };
            let shared = self.shared.clone();
            let spawned = std::thread::Builder::new()
                .name("NabyScape capture".into())
                .spawn({
                    let shared = shared.clone();
                    move || {
                        let result = capture_now(&target);
                        let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
                        if state.0 == generation {
                            state.1 = match result {
                                Ok(frame) => CaptureState::Frame(Arc::new(frame)),
                                Err(message) => CaptureState::Failed(message),
                            };
                            drop(state);
                            (shared.wake)();
                        }
                    }
                });
            if spawned.is_err() {
                let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
                state.1 = CaptureState::Failed(ERR_MINIMIZED.to_owned());
            }
        }

        pub fn snapshot(&self) -> CaptureState {
            self.shared
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .1
                .clone()
        }
    }

    impl Drop for CaptureService {
        fn drop(&mut self) {
            // Invalidates any running capture; its thread ends on its own when the API returns.
            self.stop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(w: u32, h: u32) -> Vec<u8> {
        let mut v = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                v.extend_from_slice(&[
                    (x % 256) as u8,
                    (y % 256) as u8,
                    ((x + y) % 256) as u8,
                    255,
                ]);
            }
        }
        v
    }

    #[test]
    fn frames_scale_so_the_longest_side_is_1280() {
        assert_eq!(scaled_size(1920, 1080), (1280, 720));
        assert_eq!(scaled_size(1080, 1920), (720, 1280));
        assert_eq!(
            scaled_size(800, 600),
            (800, 600),
            "small frames are never enlarged"
        );
        assert_eq!(scaled_size(1280, 1280), (1280, 1280));
        assert_eq!(scaled_size(10_000, 1), (1280, 1), "never below one pixel");
        assert_eq!(scaled_size(3840, 2160), (1280, 720));
    }

    #[test]
    fn oversized_frames_are_refused_before_any_pixel_work() {
        assert_eq!(check_size(20_000, 20_000), Err(ERR_TOO_LARGE.to_owned()));
        assert_eq!(check_size(5_800, 5_800), Err(ERR_TOO_LARGE.to_owned()));
        assert!(
            check_size(5_760, 5_760).is_ok(),
            "exactly 128 MiB is allowed"
        );
        assert_eq!(check_size(0, 10), Err(ERR_EMPTY.to_owned()));
        assert_eq!(
            encode_frame("w:1", "Big", &[], 20_000, 20_000),
            Err(ERR_TOO_LARGE.to_owned()),
            "refused before the buffer is even looked at"
        );
    }

    #[test]
    fn empty_or_mismatched_buffers_are_errors_not_panics() {
        assert!(encode_frame("a", "a", &[], 0, 0).is_err());
        assert!(encode_frame("a", "a", &[0; 7], 1, 1).is_err());
    }

    #[test]
    fn encoded_frame_is_a_bounded_jpeg_with_the_scaled_size() {
        let (w, h) = (1600, 900);
        let frame = encode_frame("d:\\\\.\\DISPLAY1", "Display 1", &gradient(w, h), w, h).unwrap();
        assert_eq!((frame.width, frame.height), (1280, 720));
        assert_eq!(frame.mime, "image/jpeg");
        assert_eq!(&frame.bytes[..2], &[0xFF, 0xD8], "JPEG SOI");
        assert!(frame.bytes.len() as u64 <= MAXIMUM_PICTURE_BYTES);
        assert_eq!(
            naby_render::Image::probe_size(&frame.bytes),
            Some((1280, 720))
        );
        assert!(frame.data_url().starts_with("data:image/jpeg;base64,"));
        assert!(frame.to_attachment("Shared screen").is_ok());
    }

    #[test]
    fn noisy_frames_shrink_until_they_fit_two_megabytes() {
        // Pseudo-random noise compresses badly, forcing the quality/size fallback path.
        let (w, h) = (2560u32, 1440u32);
        let mut seed = 0x1234_5678u32;
        let noise: Vec<u8> = (0..(w * h * 4))
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (seed >> 24) as u8
            })
            .collect();
        let frame = encode_frame("w:2", "Noise", &noise, w, h).unwrap();
        assert!(frame.bytes.len() as u64 <= MAXIMUM_PICTURE_BYTES);
        assert!(frame.width <= MAX_EDGE && frame.height <= MAX_EDGE);
    }

    #[test]
    fn downscale_averages_blocks_and_ignores_alpha() {
        // 2x2: black, white / white, black with garbage alpha -> one mid-gray pixel.
        let bgra = [0, 0, 0, 7, 255, 255, 255, 9, 255, 255, 255, 0, 0, 0, 0, 255];
        assert_eq!(
            downscale_bgra_to_rgb(&bgra, 2, 2, 1, 1),
            vec![127, 127, 127]
        );
        // Identity scale keeps channel order (BGRA -> RGB).
        assert_eq!(
            downscale_bgra_to_rgb(&[10, 20, 30, 255], 1, 1, 1, 1),
            vec![30, 20, 10]
        );
    }
}
