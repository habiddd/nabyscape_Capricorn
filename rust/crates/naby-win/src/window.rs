//! The layered, topmost, no-activate popup that shows a painted `Canvas` (the dock, panels).
//!
//! Pixels go to the screen through `UpdateLayeredWindow` from one reused DIB section, so a
//! `present` allocates nothing and only reallocates when the size changes. Mouse and keyboard
//! messages are folded into an `InputAccumulator` and handed to the application as
//! `naby_render::InputState`. The window procedure never calls application code: it records
//! input and queues `HostEvent`s.
//!
//! Click-through: pixels with alpha 0 pass clicks to the window below (the system does this for
//! per-pixel-alpha layered windows; `WM_NCHITTEST` answers `HTTRANSPARENT` for pixels below
//! `HIT_ALPHA_MIN` as well).

use naby_render::{InputState, Key, KeyEvent};

/// Pixels with less alpha than this do not receive the pointer.
pub const HIT_ALPHA_MIN: u8 = 1;

/// Converts the canvas' premultiplied RGBA bytes into BGRA words for a 32-bit DIB.
/// `dst` must hold `src.len() / 4` pixels.
pub fn rgba_to_bgra(src: &[u8], dst: &mut [u32]) {
    for (px, out) in src.chunks_exact(4).zip(dst.iter_mut()) {
        *out = u32::from_le_bytes([px[2], px[1], px[0], px[3]]);
    }
}

/// Alpha of one pixel in a BGRA word.
pub fn bgra_alpha(word: u32) -> u8 {
    (word >> 24) as u8
}

/// Wheel movement in pixels: one notch (120) scrolls `lines` lines of 16 px at 100% scaling.
/// Positive values scroll content up (wheel rolled away from the user).
pub fn wheel_pixels(delta: i32, lines: u32, dpi: u32) -> f32 {
    let lines = if lines == 0 || lines == u32::MAX {
        3
    } else {
        lines
    };
    delta as f32 / 120.0 * lines as f32 * 16.0 * dpi.max(48) as f32 / 96.0
}

/// Windows virtual-key codes the UI toolkit understands.
pub fn map_key(vk: u32, shift: bool, ctrl: bool, alt: bool) -> Option<KeyEvent> {
    let key = match vk {
        0x09 => Key::Tab,
        0x0D => Key::Enter,
        0x20 => Key::Space,
        0x1B => Key::Escape,
        0x08 => Key::Backspace,
        0x2E => Key::Delete,
        0x25 => Key::Left,
        0x27 => Key::Right,
        0x26 => Key::Up,
        0x28 => Key::Down,
        0x24 => Key::Home,
        0x23 => Key::End,
        0x21 => Key::PageUp,
        0x22 => Key::PageDown,
        // Letters and digits only matter as Ctrl shortcuts; typed text arrives as WM_CHAR.
        0x30..=0x39 | 0x41..=0x5A if ctrl => Key::Char((vk as u8).to_ascii_lowercase() as char),
        _ => return None,
    };
    Some(KeyEvent {
        key,
        shift,
        ctrl,
        alt,
    })
}

/// Folds window messages into per-frame `InputState`s.
#[derive(Debug, Default)]
pub struct InputAccumulator {
    state: InputState,
    high_surrogate: Option<u16>,
}

impl InputAccumulator {
    pub fn pointer_move(&mut self, x: f32, y: f32) {
        self.state.pointer = Some((x, y));
    }

    /// The pointer left the window (not while a button is held with capture).
    pub fn pointer_leave(&mut self) {
        self.state.pointer = None;
    }

    pub fn button(&mut self, down: bool, x: f32, y: f32) {
        self.state.pointer = Some((x, y));
        if down {
            self.state.down = true;
            self.state.pressed = true;
        } else if self.state.down {
            self.state.down = false;
            self.state.released = true;
        }
    }

    /// Mouse capture was taken away mid-drag: end the press.
    pub fn capture_lost(&mut self) {
        if self.state.down {
            self.state.down = false;
            self.state.released = true;
        }
    }

    pub fn wheel(&mut self, dx: f32, dy: f32) {
        self.state.wheel.0 += dx;
        self.state.wheel.1 += dy;
    }

    pub fn modifiers(&mut self, shift: bool, ctrl: bool) {
        self.state.shift = shift;
        self.state.ctrl = ctrl;
    }

    pub fn key(&mut self, event: KeyEvent) {
        self.state.keys.push(event);
    }

    pub fn paste(&mut self, text: String) {
        self.state.paste = Some(text);
    }

    /// One UTF-16 unit from `WM_CHAR`; control characters are ignored and surrogate pairs are
    /// joined before being appended to the typed text.
    pub fn char_unit(&mut self, unit: u16) {
        let c = match unit {
            0xD800..=0xDBFF => {
                self.high_surrogate = Some(unit);
                return;
            }
            0xDC00..=0xDFFF => {
                let Some(high) = self.high_surrogate.take() else {
                    return;
                };
                char::decode_utf16([high, unit]).next().and_then(Result::ok)
            }
            _ => {
                self.high_surrogate = None;
                char::from_u32(unit as u32)
            }
        };
        if let Some(c) = c.filter(|c| !c.is_control()) {
            self.state.text.push(c);
        }
    }

    /// True when something happened that the surface has not seen yet.
    pub fn has_events(&self) -> bool {
        let s = &self.state;
        s.pressed
            || s.released
            || s.wheel != (0.0, 0.0)
            || !s.keys.is_empty()
            || !s.text.is_empty()
            || s.paste.is_some()
    }

    /// Returns the frame's input and clears the one-shot fields.
    pub fn take(&mut self, time: f64) -> InputState {
        self.state.time = time;
        InputState {
            pointer: self.state.pointer,
            down: self.state.down,
            pressed: std::mem::take(&mut self.state.pressed),
            released: std::mem::take(&mut self.state.released),
            wheel: std::mem::take(&mut self.state.wheel),
            keys: std::mem::take(&mut self.state.keys),
            text: std::mem::take(&mut self.state.text),
            paste: self.state.paste.take(),
            shift: self.state.shift,
            ctrl: self.state.ctrl,
            time,
        }
    }

    /// Forgets everything (window hidden).
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(windows)]
pub use os::LayeredWindow;
#[cfg(windows)]
pub(crate) use os::{ensure_class, set_user_data, user_data};

#[cfg(windows)]
mod os {
    use super::*;
    use crate::clipboard;
    use crate::event::HostEvent;
    use naby_render::Canvas;
    use std::cell::{Cell, RefCell};
    use std::ffi::c_void;
    use std::rc::Rc;
    use std::sync::mpsc::Sender;
    use std::time::Instant;
    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM};
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, SelectObject, AC_SRC_ALPHA,
        AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS, HBITMAP,
        HDC, HGDIOBJ,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::HiDpi::GetDpiForWindow;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetKeyState, ReleaseCapture, SetCapture, SetFocus, TrackMouseEvent, TME_LEAVE,
        TRACKMOUSEEVENT, VK_CONTROL, VK_MENU, VK_SHIFT,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, GetWindowLongPtrW, LoadCursorW,
        RegisterClassExW, SetCursor, SetForegroundWindow, SetWindowLongPtrW, SetWindowPos,
        ShowWindow, SystemParametersInfoW, UpdateLayeredWindow, GWLP_USERDATA, GWL_EXSTYLE,
        HTCLIENT, HTTRANSPARENT, HWND_TOPMOST, IDC_ARROW, SPI_GETWHEELSCROLLLINES, SWP_NOACTIVATE,
        SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SW_HIDE, SW_SHOWNOACTIVATE,
        SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, ULW_ALPHA, WINDOW_EX_STYLE, WINDOW_STYLE,
        WM_CAPTURECHANGED, WM_CHAR, WM_DPICHANGED, WM_ERASEBKGND, WM_KEYDOWN, WM_KILLFOCUS,
        WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEACTIVATE, WM_MOUSEHWHEEL, WM_MOUSEMOVE,
        WM_MOUSEWHEEL, WM_NCDESTROY, WM_NCHITTEST, WM_SETCURSOR, WM_SYSKEYDOWN, WNDCLASSEXW,
        WNDPROC, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
    };

    const CLASS: PCWSTR = w!("NabyScape.Layered");
    const MA_NOACTIVATE: isize = 3;
    /// `WM_MOUSELEAVE` (not exported by the bindings).
    const WM_MOUSELEAVE: u32 = 0x02A3;

    /// Registers a window class once per process (later calls are no-ops).
    pub(crate) fn ensure_class(name: PCWSTR, proc: WNDPROC, arrow_cursor: bool) {
        // SAFETY: standard class registration; failure just means it already exists.
        unsafe {
            let instance = GetModuleHandleW(None).unwrap_or_default();
            let class = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: proc,
                hInstance: instance.into(),
                hCursor: if arrow_cursor {
                    LoadCursorW(None, IDC_ARROW).unwrap_or_default()
                } else {
                    Default::default()
                },
                lpszClassName: name,
                ..Default::default()
            };
            RegisterClassExW(&class);
        }
    }

    pub(crate) fn set_user_data(hwnd: HWND, value: isize) {
        // SAFETY: stores a pointer-sized value in the window's user data slot.
        unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, value) };
    }

    pub(crate) fn user_data(hwnd: HWND) -> isize {
        // SAFETY: reads the user data slot.
        unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) }
    }

    fn lo(l: LPARAM) -> i32 {
        (l.0 & 0xFFFF) as i16 as i32
    }
    fn hi(l: LPARAM) -> i32 {
        ((l.0 >> 16) & 0xFFFF) as i16 as i32
    }

    fn key_down(vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY) -> bool {
        // SAFETY: plain keyboard state query.
        let state = unsafe { GetKeyState(vk.0 as i32) };
        state < 0
    }

    #[derive(Clone, Copy)]
    struct HitMap {
        bits: *const u32,
        w: u32,
        h: u32,
    }

    /// State shared between the window object and its window procedure (one thread).
    struct Shared {
        tx: Sender<HostEvent>,
        input: RefCell<InputAccumulator>,
        queued: Cell<bool>,
        tracking: Cell<bool>,
        text_focus: Cell<bool>,
        origin: Cell<(i32, i32)>,
        hit: Cell<Option<HitMap>>,
        started: Instant,
    }

    impl Shared {
        fn notify(&self) {
            if !self.queued.replace(true) {
                let _ = self.tx.send(HostEvent::Input);
            }
        }
    }

    struct Dib {
        bitmap: HBITMAP,
        bits: *mut u32,
        w: u32,
        h: u32,
    }

    pub struct LayeredWindow {
        hwnd: HWND,
        shared: Rc<Shared>,
        raw_shared: *const Shared,
        dc: HDC,
        default_bitmap: HGDIOBJ,
        dib: Option<Dib>,
        visible: bool,
        opacity: u8,
    }

    impl LayeredWindow {
        /// Creates the (hidden) window. `tx` receives `HostEvent::Input`, `WindowDpi` and
        /// `TextFocusLost`.
        pub fn new(tx: Sender<HostEvent>) -> windows::core::Result<LayeredWindow> {
            ensure_class(CLASS, Some(wndproc), true);
            // SAFETY: creates a hidden popup owned by this thread; the user-data pointer set
            // below stays valid until `Drop` (which destroys the window first).
            unsafe {
                let instance = GetModuleHandleW(None)?;
                let hwnd = CreateWindowExW(
                    WINDOW_EX_STYLE(
                        WS_EX_LAYERED.0 | WS_EX_TOPMOST.0 | WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0,
                    ),
                    CLASS,
                    w!("NabyScape"),
                    WINDOW_STYLE(WS_POPUP.0),
                    0,
                    0,
                    1,
                    1,
                    None,
                    None,
                    Some(instance.into()),
                    None,
                )?;
                let shared = Rc::new(Shared {
                    tx,
                    input: RefCell::new(InputAccumulator::default()),
                    queued: Cell::new(false),
                    tracking: Cell::new(false),
                    text_focus: Cell::new(false),
                    origin: Cell::new((0, 0)),
                    hit: Cell::new(None),
                    started: Instant::now(),
                });
                let raw_shared = Rc::into_raw(shared.clone());
                set_user_data(hwnd, raw_shared as isize);
                let dc = CreateCompatibleDC(None);
                Ok(LayeredWindow {
                    hwnd,
                    shared,
                    raw_shared,
                    dc,
                    default_bitmap: HGDIOBJ::default(),
                    dib: None,
                    visible: false,
                    opacity: 255,
                })
            }
        }

        pub fn hwnd(&self) -> HWND {
            self.hwnd
        }

        pub fn is_visible(&self) -> bool {
            self.visible
        }

        /// Window opacity 0..=1 (Settings.Opacity); applied on the next `present`.
        pub fn set_opacity(&mut self, opacity: f32) {
            self.opacity = (opacity.clamp(0.0, 1.0) * 255.0).round() as u8;
        }

        /// DPI of the monitor the window is on.
        pub fn dpi(&self) -> u32 {
            // SAFETY: plain query.
            unsafe { GetDpiForWindow(self.hwnd) }.max(96)
        }

        fn ensure_dib(&mut self, w: u32, h: u32) -> bool {
            if let Some(d) = &self.dib {
                if (d.w, d.h) == (w, h) {
                    return true;
                }
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
            // SAFETY: `info` describes a top-down 32-bit DIB; `bits` receives its pixels.
            let bitmap = unsafe {
                CreateDIBSection(Some(self.dc), &info, DIB_RGB_COLORS, &mut bits, None, 0)
            };
            let Ok(bitmap) = bitmap else {
                return false;
            };
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
            self.shared.hit.set(Some(HitMap {
                bits: bits as *const u32,
                w,
                h,
            }));
            self.dib = Some(Dib {
                bitmap,
                bits: bits as *mut u32,
                w,
                h,
            });
            true
        }

        fn free_dib(&mut self) {
            self.shared.hit.set(None);
            if let Some(d) = self.dib.take() {
                // SAFETY: the bitmap is deselected before it is deleted.
                unsafe {
                    SelectObject(self.dc, self.default_bitmap);
                    let _ = DeleteObject(d.bitmap.into());
                }
            }
        }

        /// Copies `canvas` to the screen with its top-left at (`x`, `y`), showing the window
        /// without activating it. Allocation-free unless the size changed.
        pub fn present(&mut self, canvas: &Canvas, x: i32, y: i32) {
            let (w, h) = canvas.size();
            if !self.ensure_dib(w, h) {
                return;
            }
            let Some(dib) = &self.dib else { return };
            // SAFETY: the DIB holds exactly w*h 32-bit pixels and stays alive during the copy.
            let dst = unsafe { std::slice::from_raw_parts_mut(dib.bits, (w * h) as usize) };
            rgba_to_bgra(canvas.data(), dst);
            let position = POINT { x, y };
            let size = SIZE {
                cx: w as i32,
                cy: h as i32,
            };
            let source = POINT { x: 0, y: 0 };
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: self.opacity,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            self.shared.origin.set((x, y));
            // SAFETY: all pointers reference live locals; the source DC holds our DIB.
            unsafe {
                let _ = UpdateLayeredWindow(
                    self.hwnd,
                    None,
                    Some(&position),
                    Some(&size),
                    Some(self.dc),
                    Some(&source),
                    COLORREF(0),
                    Some(&blend),
                    ULW_ALPHA,
                );
                if !self.visible {
                    let _ = SetWindowPos(
                        self.hwnd,
                        Some(HWND_TOPMOST),
                        0,
                        0,
                        0,
                        0,
                        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                    );
                    let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
                    self.visible = true;
                }
            }
        }

        /// Moves the window without repainting.
        pub fn move_to(&mut self, x: i32, y: i32) {
            self.shared.origin.set((x, y));
            // SAFETY: plain window move.
            unsafe {
                let _ = SetWindowPos(
                    self.hwnd,
                    None,
                    x,
                    y,
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
        }

        pub fn hide(&mut self) {
            if self.visible {
                self.set_text_input(false);
                // SAFETY: plain show/hide.
                let _ = unsafe { ShowWindow(self.hwnd, SW_HIDE) };
                self.visible = false;
            }
            self.shared.input.borrow_mut().reset();
            self.shared.queued.set(false);
        }

        /// Hides the window and frees its pixel memory; the next `present` rebuilds it.
        pub fn release(&mut self) {
            self.hide();
            self.free_dib();
        }

        /// Bytes held by the DIB section.
        pub fn memory_bytes(&self) -> usize {
            self.dib
                .as_ref()
                .map_or(0, |d| d.w as usize * d.h as usize * 4)
        }

        /// Takes the pointer / key / text input gathered since the last call.
        pub fn take_input(&mut self) -> InputState {
            let time = self.shared.started.elapsed().as_secs_f64();
            self.shared.queued.set(false);
            self.shared.input.borrow_mut().take(time)
        }

        /// Lets the window take keyboard focus (text fields) or give it up again. While it
        /// has focus the window is activatable; otherwise clicks never steal focus.
        pub fn set_text_input(&mut self, on: bool) {
            if self.shared.text_focus.get() == on {
                return;
            }
            self.shared.text_focus.set(on);
            // SAFETY: style and focus changes on our own window.
            unsafe {
                let style = GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE);
                let flag = WS_EX_NOACTIVATE.0 as isize;
                let style = if on { style & !flag } else { style | flag };
                SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, style);
                if on {
                    let _ = SetForegroundWindow(self.hwnd);
                    let _ = SetFocus(Some(self.hwnd));
                }
            }
        }

        pub fn has_text_input(&self) -> bool {
            self.shared.text_focus.get()
        }
    }

    impl Drop for LayeredWindow {
        fn drop(&mut self) {
            self.free_dib();
            // SAFETY: destroys our window (WM_NCDESTROY clears the user data), deletes the
            // DC, then releases the strong count leaked into the user data pointer.
            unsafe {
                let _ = DestroyWindow(self.hwnd);
                let _ = DeleteDC(self.dc);
                drop(Rc::from_raw(self.raw_shared));
            }
        }
    }

    fn shared_of<'a>(hwnd: HWND) -> Option<&'a Shared> {
        let ptr = user_data(hwnd) as *const Shared;
        // SAFETY: the pointer was produced by Rc::into_raw and is cleared at WM_NCDESTROY,
        // before the owning window object drops its count.
        (!ptr.is_null()).then(|| unsafe { &*ptr })
    }

    fn mouse_modifiers(s: &Shared) {
        s.input
            .borrow_mut()
            .modifiers(key_down(VK_SHIFT), key_down(VK_CONTROL));
    }

    unsafe extern "system" fn wndproc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        let Some(s) = shared_of(hwnd) else {
            // SAFETY: default handling for messages before/after our state exists.
            return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
        };
        match msg {
            WM_NCHITTEST => {
                let (ox, oy) = s.origin.get();
                let x = lo(lparam) - ox;
                let y = hi(lparam) - oy;
                if let Some(map) = s.hit.get() {
                    if x >= 0 && y >= 0 && (x as u32) < map.w && (y as u32) < map.h {
                        // SAFETY: the index is inside the DIB, which outlives the HitMap.
                        let word =
                            unsafe { *map.bits.add(y as usize * map.w as usize + x as usize) };
                        if bgra_alpha(word) < HIT_ALPHA_MIN {
                            return LRESULT(HTTRANSPARENT as isize);
                        }
                    }
                }
                LRESULT(HTCLIENT as isize)
            }
            WM_MOUSEACTIVATE => {
                if s.text_focus.get() {
                    // SAFETY: default handling activates the window for text editing.
                    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
                } else {
                    LRESULT(MA_NOACTIVATE)
                }
            }
            WM_SETCURSOR => {
                // SAFETY: stock cursor.
                unsafe { SetCursor(LoadCursorW(None, IDC_ARROW).ok()) };
                LRESULT(1)
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
                mouse_modifiers(s);
                s.input
                    .borrow_mut()
                    .pointer_move(lo(lparam) as f32, hi(lparam) as f32);
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
                mouse_modifiers(s);
                let down = msg == WM_LBUTTONDOWN;
                // SAFETY: capture keeps the drag inside this window until release.
                unsafe {
                    if down {
                        SetCapture(hwnd);
                    } else {
                        let _ = ReleaseCapture();
                    }
                }
                s.input
                    .borrow_mut()
                    .button(down, lo(lparam) as f32, hi(lparam) as f32);
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
                mouse_modifiers(s);
                let mut input = s.input.borrow_mut();
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
                let shift = key_down(VK_SHIFT);
                let ctrl = key_down(VK_CONTROL);
                let alt = key_down(VK_MENU);
                let vk = wparam.0 as u32;
                let mut input = s.input.borrow_mut();
                input.modifiers(shift, ctrl);
                if let Some(event) = map_key(vk, shift, ctrl, alt) {
                    input.key(event);
                    if ctrl && vk == 0x56 {
                        drop(input);
                        // Clipboard access can pump messages; the borrow is released first.
                        if let Some(text) = clipboard::get_text(Some(hwnd)) {
                            s.input.borrow_mut().paste(text);
                        }
                    } else {
                        drop(input);
                    }
                    s.notify();
                    return LRESULT(0);
                }
                drop(input);
                // SAFETY: default handling (system keys, accelerators).
                unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
            }
            WM_CHAR => {
                s.input.borrow_mut().char_unit(wparam.0 as u16);
                s.notify();
                LRESULT(0)
            }
            WM_KILLFOCUS => {
                if s.text_focus.replace(false) {
                    let _ = s.tx.send(HostEvent::TextFocusLost);
                    // SAFETY: restore the no-activate style now that focus is gone.
                    unsafe {
                        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
                        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style | WS_EX_NOACTIVATE.0 as isize);
                    }
                }
                LRESULT(0)
            }
            WM_DPICHANGED => {
                let _ = s.tx.send(HostEvent::WindowDpi((wparam.0 & 0xFFFF) as u32));
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_NCDESTROY => {
                set_user_data(hwnd, 0);
                // SAFETY: final default handling.
                unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
            }
            // SAFETY: default handling for everything else.
            _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgba_becomes_bgra_with_alpha_in_the_top_byte() {
        let src = [10u8, 20, 30, 255, 0, 0, 0, 0];
        let mut dst = [0u32; 2];
        rgba_to_bgra(&src, &mut dst);
        assert_eq!(dst[0].to_le_bytes(), [30, 20, 10, 255]);
        assert_eq!(bgra_alpha(dst[0]), 255);
        assert_eq!(bgra_alpha(dst[1]), 0);
    }

    #[test]
    fn wheel_scales_with_lines_and_dpi() {
        assert_eq!(wheel_pixels(120, 3, 96), 48.0);
        assert_eq!(wheel_pixels(-120, 3, 96), -48.0);
        assert_eq!(wheel_pixels(120, 3, 192), 96.0);
        assert_eq!(wheel_pixels(60, 3, 96), 24.0);
        // "Scroll one screen at a time" falls back to three lines.
        assert_eq!(wheel_pixels(120, u32::MAX, 96), 48.0);
    }

    #[test]
    fn navigation_keys_map_and_letters_need_ctrl() {
        assert_eq!(map_key(0x25, false, false, false).unwrap().key, Key::Left);
        assert!(map_key(0x09, true, false, false).unwrap().shift);
        assert_eq!(map_key(0x0D, false, false, false).unwrap().key, Key::Enter);
        assert!(map_key(0x41, false, false, false).is_none());
        let ctrl_a = map_key(0x41, false, true, false).unwrap();
        assert_eq!((ctrl_a.key, ctrl_a.ctrl), (Key::Char('a'), true));
        assert_eq!(
            map_key(0x35, false, true, false).unwrap().key,
            Key::Char('5')
        );
        assert!(map_key(0x70, false, false, false).is_none());
    }

    #[test]
    fn press_move_release_produce_edges_once() {
        let mut acc = InputAccumulator::default();
        acc.pointer_move(5.0, 6.0);
        acc.button(true, 5.0, 6.0);
        let first = acc.take(1.0);
        assert!(first.pressed && first.down && !first.released);
        assert_eq!(first.pointer, Some((5.0, 6.0)));
        acc.pointer_move(9.0, 6.0);
        let drag = acc.take(1.1);
        assert!(drag.down && !drag.pressed);
        acc.button(false, 9.0, 6.0);
        let up = acc.take(1.2);
        assert!(up.released && !up.down);
        assert!(!acc.take(1.3).released);
        // A stray button-up without a press is ignored.
        acc.button(false, 0.0, 0.0);
        assert!(!acc.take(1.4).released);
    }

    #[test]
    fn leaving_clears_the_pointer_and_capture_loss_ends_the_press() {
        let mut acc = InputAccumulator::default();
        acc.button(true, 1.0, 1.0);
        acc.capture_lost();
        let s = acc.take(0.0);
        assert!(s.released && !s.down);
        acc.pointer_leave();
        assert_eq!(acc.take(0.0).pointer, None);
    }

    #[test]
    fn wheel_accumulates_until_taken() {
        let mut acc = InputAccumulator::default();
        acc.wheel(0.0, 48.0);
        acc.wheel(0.0, 48.0);
        acc.wheel(-16.0, 0.0);
        assert!(acc.has_events());
        assert_eq!(acc.take(0.0).wheel, (-16.0, 96.0));
        assert!(!acc.has_events());
    }

    #[test]
    fn typed_text_joins_surrogates_and_skips_controls() {
        let mut acc = InputAccumulator::default();
        for unit in "hi \u{1F600}!".encode_utf16() {
            acc.char_unit(unit);
        }
        acc.char_unit(0x08); // backspace
        acc.char_unit(0x0D); // enter
        acc.char_unit(0x16); // ctrl+v
        acc.char_unit(0xDC00); // lone low surrogate
        acc.char_unit(0xD83D); // dangling high surrogate
        acc.char_unit(b'x' as u16); // cancels the dangling one
        assert_eq!(acc.take(0.0).text, "hi \u{1F600}!x");
    }

    #[test]
    fn modifiers_and_paste_travel_with_the_frame() {
        let mut acc = InputAccumulator::default();
        acc.modifiers(true, true);
        acc.paste("clip".into());
        acc.key(KeyEvent::new(Key::Char('v')).ctrl());
        let s = acc.take(2.5);
        assert!(s.shift && s.ctrl);
        assert_eq!(s.paste.as_deref(), Some("clip"));
        assert_eq!(s.keys.len(), 1);
        assert_eq!(s.time, 2.5);
        let next = acc.take(2.6);
        assert!(next.paste.is_none() && next.keys.is_empty() && next.shift);
        acc.reset();
        assert!(!acc.take(0.0).shift);
    }
}
