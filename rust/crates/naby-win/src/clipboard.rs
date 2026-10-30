//! Clipboard history source (C# `ProductivityService` listener half, `SmartClipboard` exclusion
//! formats). A hidden message-only window on its own thread listens with
//! `AddClipboardFormatListener` while history is enabled; text (`CF_UNICODETEXT`) or a PNG made
//! from `CF_DIBV5`/`CF_DIB` is queued as a [`ClipboardEvent`] for the UI thread to feed into
//! `ProductivityService::ingest_text` / `ingest_image`. The pure decisions and the DIB/PNG
//! conversion live outside `cfg(windows)` so they are tested on every platform.
#![cfg_attr(not(windows), allow(dead_code))]

use naby_core::productivity::clipboard::{MAX_IMAGE_BYTES, MAX_IMAGE_PIXELS};

/// Formats whose presence means the owner asked not to be recorded (C# `_excludedFormats`).
pub const EXCLUDED_FORMAT_NAMES: [&str; 3] = [
    "ExcludeClipboardContentFromMonitorProcessing",
    "CanIncludeInClipboardHistory",
    "CanUploadToCloudClipboard",
];

/// What the listener saw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipboardEvent {
    Text(String),
    /// A PNG of at most [`MAX_IMAGE_BYTES`] and [`MAX_IMAGE_PIXELS`].
    Png(Vec<u8>),
    /// Reading failed (clipboard busy); the message is for `LastError`.
    Error(String),
}

/// C# `ClipboardMessage` early exits: disabled, paused, our own write, or an excluded owner.
pub fn should_skip(
    enabled: bool,
    paused: bool,
    writing: bool,
    sequence: u32,
    last_write_sequence: u32,
    excluded_format_present: bool,
) -> bool {
    !enabled
        || paused
        || writing
        || (sequence != 0 && sequence == last_write_sequence)
        || excluded_format_present
}

/// Delayed-rendering owners can change while data is fetched; a changed sequence number or a
/// newly excluded format after the read rejects the notification.
pub fn read_is_stale(before: u32, after: u32, excluded_format_present: bool) -> bool {
    before != after || excluded_format_present
}

/// Decoded straight-alpha RGBA pixels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DibError {
    Truncated,
    Unsupported,
    TooLarge,
    Invalid,
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn i32_at(b: &[u8], at: usize) -> Option<i32> {
    Some(i32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// Expands the bits selected by `mask` of `value` to 0..=255.
fn channel(value: u32, mask: u32) -> u8 {
    if mask == 0 {
        return 0;
    }
    let shift = mask.trailing_zeros();
    let max = mask >> shift;
    let part = (value & mask) >> shift;
    if max == 0xFF {
        part as u8
    } else {
        ((u64::from(part) * 255 + u64::from(max) / 2) / u64::from(max)) as u8
    }
}

/// Converts a packed DIB (`BITMAPINFOHEADER`, `BITMAPV4HEADER` or `BITMAPV5HEADER` followed by
/// the colour table and pixels, as in `CF_DIB`/`CF_DIBV5`) to RGBA. Supports 1/4/8-bit palettes,
/// 16/24/32-bit `BI_RGB` and 16/32-bit `BI_BITFIELDS`; RLE and embedded JPEG/PNG are rejected.
pub fn dib_to_rgba(dib: &[u8]) -> Result<RgbaImage, DibError> {
    let header = u32_at(dib, 0).ok_or(DibError::Truncated)? as usize;
    if !matches!(header, 40 | 52 | 56 | 108 | 124) {
        return Err(DibError::Unsupported);
    }
    let width = i32_at(dib, 4).ok_or(DibError::Truncated)?;
    let raw_height = i32_at(dib, 8).ok_or(DibError::Truncated)?;
    let bpp = u32::from(u16_at(dib, 14).ok_or(DibError::Truncated)?);
    let compression = u32_at(dib, 16).ok_or(DibError::Truncated)?;
    let colors_used = u32_at(dib, 32).ok_or(DibError::Truncated)? as usize;
    if width <= 0 || raw_height == 0 || raw_height == i32::MIN {
        return Err(DibError::Invalid);
    }
    let (w, h) = (width as u32, raw_height.unsigned_abs());
    if u64::from(w) * u64::from(h) > MAX_IMAGE_PIXELS {
        return Err(DibError::TooLarge);
    }
    let top_down = raw_height < 0;
    if compression != 0 && compression != 3 {
        return Err(DibError::Unsupported);
    }
    if compression == 3 && bpp != 16 && bpp != 32 {
        return Err(DibError::Unsupported);
    }
    if !matches!(bpp, 1 | 4 | 8 | 16 | 24 | 32) {
        return Err(DibError::Unsupported);
    }
    // Masks: inside V4/V5 headers, or three DWORDs after a plain header with BI_BITFIELDS.
    let mut offset = header;
    let mut masks = match bpp {
        16 => [0x7C00, 0x03E0, 0x001F, 0],
        _ => [0x00FF_0000, 0x0000_FF00, 0x0000_00FF, 0],
    };
    if compression == 3 {
        if header >= 52 {
            masks[0] = u32_at(dib, 40).ok_or(DibError::Truncated)?;
            masks[1] = u32_at(dib, 44).ok_or(DibError::Truncated)?;
            masks[2] = u32_at(dib, 48).ok_or(DibError::Truncated)?;
            if header >= 56 {
                masks[3] = u32_at(dib, 52).ok_or(DibError::Truncated)?;
            }
        } else {
            for (i, slot) in masks.iter_mut().take(3).enumerate() {
                *slot = u32_at(dib, 40 + 4 * i).ok_or(DibError::Truncated)?;
            }
            offset += 12;
        }
    }
    let palette_len = if bpp <= 8 {
        if colors_used == 0 {
            1usize << bpp
        } else {
            colors_used.min(1usize << bpp)
        }
    } else {
        0
    };
    let palette_at = offset;
    offset += palette_len * 4;
    let stride = (w as usize * bpp as usize).div_ceil(32) * 4;
    let needed = stride
        .checked_mul(h as usize)
        .and_then(|n| n.checked_add(offset))
        .ok_or(DibError::TooLarge)?;
    if dib.len() < needed {
        return Err(DibError::Truncated);
    }
    let mut rgba = vec![0u8; w as usize * h as usize * 4];
    let alpha_mask = if bpp == 32 { masks[3] } else { 0 };
    let mut any_alpha = false;
    for y in 0..h as usize {
        let src_row = if top_down { y } else { h as usize - 1 - y };
        let row = &dib[offset + src_row * stride..offset + (src_row + 1) * stride];
        let out = &mut rgba[y * w as usize * 4..(y + 1) * w as usize * 4];
        for x in 0..w as usize {
            let px = &mut out[x * 4..x * 4 + 4];
            match bpp {
                1 | 4 | 8 => {
                    let per = 8 / bpp as usize;
                    let byte = row[x / per];
                    let shift = 8 - bpp as usize * (x % per + 1);
                    let index = ((byte >> shift) & ((1u16 << bpp) - 1) as u8) as usize;
                    if index < palette_len {
                        let c = palette_at + index * 4;
                        px.copy_from_slice(&[dib[c + 2], dib[c + 1], dib[c], 255]);
                    } else {
                        px.copy_from_slice(&[0, 0, 0, 255]);
                    }
                }
                16 => {
                    let v = u32::from(u16::from_le_bytes([row[x * 2], row[x * 2 + 1]]));
                    px.copy_from_slice(&[
                        channel(v, masks[0]),
                        channel(v, masks[1]),
                        channel(v, masks[2]),
                        255,
                    ]);
                }
                24 => px.copy_from_slice(&[row[x * 3 + 2], row[x * 3 + 1], row[x * 3], 255]),
                _ => {
                    let v = u32::from_le_bytes([
                        row[x * 4],
                        row[x * 4 + 1],
                        row[x * 4 + 2],
                        row[x * 4 + 3],
                    ]);
                    let a = if alpha_mask != 0 {
                        channel(v, alpha_mask)
                    } else {
                        255
                    };
                    any_alpha |= a != 0;
                    px.copy_from_slice(&[
                        channel(v, masks[0]),
                        channel(v, masks[1]),
                        channel(v, masks[2]),
                        a,
                    ]);
                }
            }
        }
    }
    // A 32-bit bitmap whose alpha channel is entirely zero carries no transparency.
    if alpha_mask != 0 && !any_alpha {
        rgba.chunks_exact_mut(4).for_each(|p| p[3] = 255);
    }
    Ok(RgbaImage {
        width: w,
        height: h,
        rgba,
    })
}

/// Encodes straight-alpha RGBA as a PNG.
pub fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Option<Vec<u8>> {
    if width == 0 || height == 0 || rgba.len() != width as usize * height as usize * 4 {
        return None;
    }
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
    let mut writer = encoder.write_header().ok()?;
    writer.write_image_data(rgba).ok()?;
    writer.finish().ok()?;
    Some(out)
}

/// A DIB straight from the clipboard, as a size-bounded PNG: `None` when it is unreadable,
/// larger than [`MAX_IMAGE_PIXELS`] or its PNG exceeds [`MAX_IMAGE_BYTES`] (C# skips these).
pub fn dib_to_png(dib: &[u8]) -> Option<Vec<u8>> {
    let image = dib_to_rgba(dib).ok()?;
    let png = encode_png(image.width, image.height, &image.rgba)?;
    (png.len() <= MAX_IMAGE_BYTES).then_some(png)
}

/// Decodes a PNG to straight RGBA.
pub fn decode_png(png: &[u8]) -> Option<RgbaImage> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(png));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().ok()?;
    let (w, h) = {
        let info = reader.info();
        (info.width, info.height)
    };
    if u64::from(w) * u64::from(h) > MAX_IMAGE_PIXELS {
        return None;
    }
    let mut buffer = vec![0u8; reader.output_buffer_size()];
    let frame = reader.next_frame(&mut buffer).ok()?;
    let data = &buffer[..frame.buffer_size()];
    let mut rgba = Vec::with_capacity(w as usize * h as usize * 4);
    match frame.color_type {
        png::ColorType::Rgba => rgba.extend_from_slice(data),
        png::ColorType::Rgb => data
            .chunks_exact(3)
            .for_each(|p| rgba.extend_from_slice(&[p[0], p[1], p[2], 255])),
        png::ColorType::Grayscale => data
            .iter()
            .for_each(|&g| rgba.extend_from_slice(&[g, g, g, 255])),
        png::ColorType::GrayscaleAlpha => data
            .chunks_exact(2)
            .for_each(|p| rgba.extend_from_slice(&[p[0], p[0], p[0], p[1]])),
        png::ColorType::Indexed => return None,
    }
    (rgba.len() == w as usize * h as usize * 4).then_some(RgbaImage {
        width: w,
        height: h,
        rgba,
    })
}

/// A bottom-up 32-bit `BITMAPV5HEADER` DIB with straight alpha, for `CF_DIBV5`.
pub fn rgba_to_dibv5(image: &RgbaImage) -> Vec<u8> {
    let (w, h) = (image.width as usize, image.height as usize);
    let size = (w * h * 4) as u32;
    let mut out = Vec::with_capacity(124 + size as usize);
    let push = |out: &mut Vec<u8>, v: u32| out.extend_from_slice(&v.to_le_bytes());
    push(&mut out, 124);
    push(&mut out, image.width);
    push(&mut out, image.height); // positive: bottom-up
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    push(&mut out, 3); // BI_BITFIELDS
    push(&mut out, size);
    push(&mut out, 2835);
    push(&mut out, 2835);
    push(&mut out, 0);
    push(&mut out, 0);
    push(&mut out, 0x00FF_0000);
    push(&mut out, 0x0000_FF00);
    push(&mut out, 0x0000_00FF);
    push(&mut out, 0xFF00_0000);
    push(&mut out, 0x7352_4742); // LCS_sRGB
    out.extend_from_slice(&[0u8; 36]); // endpoints
    out.extend_from_slice(&[0u8; 12]); // gamma
    push(&mut out, 4); // LCS_GM_IMAGES
    out.extend_from_slice(&[0u8; 12]); // profile data, size, reserved
    debug_assert_eq!(out.len(), 124);
    for y in (0..h).rev() {
        for p in image.rgba[y * w * 4..(y + 1) * w * 4].chunks_exact(4) {
            out.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
        }
    }
    out
}

/// What a paste into the assistant composer found (C# `AssistantView.PasteClipboard`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PasteContent {
    /// Encoded PNG bytes (the "PNG" / "image/png" formats, or a bitmap converted to PNG).
    Png(Vec<u8>),
    Files(Vec<String>),
    Text(String),
    Empty,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasteKind {
    Png,
    ImagePng,
    Files,
    Bitmap,
    Text,
}

/// C# paste priority: the PNG formats, then dropped files, then a bitmap, then text.
pub fn paste_choice(
    png: bool,
    image_png: bool,
    files: bool,
    bitmap: bool,
    text: bool,
) -> Option<PasteKind> {
    if png {
        Some(PasteKind::Png)
    } else if image_png {
        Some(PasteKind::ImagePng)
    } else if files {
        Some(PasteKind::Files)
    } else if bitmap {
        Some(PasteKind::Bitmap)
    } else if text {
        Some(PasteKind::Text)
    } else {
        None
    }
}

#[cfg(windows)]
pub use os::{read_paste, set_image_png, ClipboardService};

#[cfg(windows)]
mod os {
    use super::*;
    use crate::svc_util::{pcwstr, wide};
    use crate::wake::Waker;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};
    use std::sync::{mpsc, Arc, Mutex};
    use std::thread::JoinHandle;
    use std::time::Duration;
    use windows::Win32::Foundation::{HANDLE, HGLOBAL, HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::DataExchange::{
        AddClipboardFormatListener, CloseClipboard, EmptyClipboard, GetClipboardData,
        GetClipboardSequenceNumber, IsClipboardFormatAvailable, OpenClipboard,
        RegisterClipboardFormatW, RemoveClipboardFormatListener, SetClipboardData,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Memory::{
        GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
    };
    use windows::Win32::System::Ole::{CF_DIB, CF_DIBV5, CF_HDROP, CF_UNICODETEXT};
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
        PostMessageW, PostQuitMessage, RegisterClassW, TranslateMessage, UnregisterClassW,
        HWND_MESSAGE, MSG, WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLIPBOARDUPDATE, WM_CLOSE, WM_DESTROY,
        WNDCLASSW,
    };

    const CLASS_NAME: &str = "NabyScapeClipboard";
    /// Clipboard text is bounded far below this before it is stored; this only stops a
    /// pathological multi-gigabyte owner from being copied.
    const MAX_READ_BYTES: usize = 64 * 1024 * 1024;

    struct Shared {
        paused: AtomicBool,
        writing: AtomicBool,
        last_write_sequence: AtomicU32,
        hwnd: AtomicIsize,
        events: Mutex<VecDeque<ClipboardEvent>>,
        wake: Waker,
        excluded: Vec<u32>,
    }

    thread_local! {
        static LISTENER: RefCell<Option<Arc<Shared>>> = const { RefCell::new(None) };
    }

    struct Listener {
        shared: Arc<Shared>,
        thread: JoinHandle<()>,
    }

    /// Clipboard capture and copy-back. Creating the service starts nothing; the listener
    /// thread exists only between `set_enabled(true)` and `set_enabled(false)` / drop.
    pub struct ClipboardService {
        wake: Waker,
        paused: bool,
        listener: Option<Listener>,
        last_error: Option<String>,
        /// Sequence recorded by our own writes, kept across listener restarts.
        write_sequence: Arc<AtomicU32>,
    }

    impl ClipboardService {
        pub fn start(wake: Waker) -> Self {
            Self {
                wake,
                paused: false,
                listener: None,
                last_error: None,
                write_sequence: Arc::new(AtomicU32::new(0)),
            }
        }

        pub fn enabled(&self) -> bool {
            self.listener.is_some()
        }

        pub fn last_error(&self) -> Option<&str> {
            self.last_error.as_deref()
        }

        /// Attaches or detaches the listener. Returns false (and records `last_error`) when the
        /// listener could not be created.
        pub fn set_enabled(&mut self, enabled: bool) -> bool {
            if !enabled {
                self.stop();
                return true;
            }
            if self.listener.is_some() {
                return true;
            }
            match self.spawn() {
                Ok(listener) => {
                    listener.shared.paused.store(self.paused, Ordering::SeqCst);
                    self.listener = Some(listener);
                    self.last_error = None;
                    true
                }
                Err(message) => {
                    self.last_error = Some(message);
                    false
                }
            }
        }

        /// Paused capture keeps the listener but ignores notifications.
        pub fn set_paused(&mut self, paused: bool) {
            self.paused = paused;
            if let Some(l) = &self.listener {
                l.shared.paused.store(paused, Ordering::SeqCst);
            }
        }

        /// Events captured since the last call.
        pub fn drain(&mut self) -> Vec<ClipboardEvent> {
            let Some(l) = &self.listener else {
                return Vec::new();
            };
            let drained: Vec<ClipboardEvent> = l
                .shared
                .events
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .drain(..)
                .collect();
            for e in &drained {
                if let ClipboardEvent::Error(m) = e {
                    self.last_error = Some(m.clone());
                }
            }
            drained
        }

        /// Puts `text` on the clipboard as Unicode text only (no HTML/RTF), without the
        /// listener recording it back.
        pub fn set_text(&mut self, text: &str) -> Result<(), String> {
            let mut bytes: Vec<u8> = Vec::with_capacity(text.len() * 2 + 2);
            for unit in text.encode_utf16().chain(std::iter::once(0)) {
                bytes.extend_from_slice(&unit.to_le_bytes());
            }
            self.write(&[(u32::from(CF_UNICODETEXT.0), bytes)])
        }

        /// Puts a PNG on the clipboard as `CF_DIBV5` plus the registered "PNG" format.
        pub fn set_image(&mut self, png: &[u8]) -> Result<(), String> {
            let image =
                decode_png(png).ok_or_else(|| "The clip image is unreadable.".to_owned())?;
            let png_format = register_format("PNG");
            let mut formats = vec![(u32::from(CF_DIBV5.0), rgba_to_dibv5(&image))];
            if png_format != 0 {
                formats.push((png_format, png.to_vec()));
            }
            self.write(&formats)
        }

        fn write(&mut self, formats: &[(u32, Vec<u8>)]) -> Result<(), String> {
            if let Some(l) = &self.listener {
                l.shared.writing.store(true, Ordering::SeqCst);
            }
            let result = write_clipboard(formats);
            // SAFETY: plain query with no arguments.
            let sequence = unsafe { GetClipboardSequenceNumber() };
            self.write_sequence.store(sequence, Ordering::SeqCst);
            if let Some(l) = &self.listener {
                l.shared
                    .last_write_sequence
                    .store(sequence, Ordering::SeqCst);
                l.shared.writing.store(false, Ordering::SeqCst);
            }
            match &result {
                Ok(()) => self.last_error = None,
                Err(m) => self.last_error = Some(m.clone()),
            }
            result
        }

        fn spawn(&self) -> Result<Listener, String> {
            let excluded: Vec<u32> = EXCLUDED_FORMAT_NAMES
                .iter()
                .map(|n| register_format(n))
                .collect();
            let shared = Arc::new(Shared {
                paused: AtomicBool::new(self.paused),
                writing: AtomicBool::new(false),
                last_write_sequence: AtomicU32::new(self.write_sequence.load(Ordering::SeqCst)),
                hwnd: AtomicIsize::new(0),
                events: Mutex::new(VecDeque::new()),
                wake: self.wake.clone(),
                excluded,
            });
            let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
            let thread_shared = shared.clone();
            let thread = std::thread::Builder::new()
                .name("NabyScape clipboard".into())
                .spawn(move || run_listener(thread_shared, ready_tx))
                .map_err(|e| e.to_string())?;
            match ready_rx.recv_timeout(Duration::from_secs(3)) {
                Ok(Ok(())) => Ok(Listener { shared, thread }),
                Ok(Err(message)) => {
                    let _ = thread.join();
                    Err(message)
                }
                Err(_) => Err("The clipboard listener did not start.".to_owned()),
            }
        }

        fn stop(&mut self) {
            if let Some(listener) = self.listener.take() {
                let hwnd = listener.shared.hwnd.load(Ordering::SeqCst);
                if hwnd != 0 {
                    // SAFETY: the window belongs to the listener thread, which is still running
                    // (it is joined below); posting WM_CLOSE to it is thread-safe.
                    unsafe {
                        let _ = PostMessageW(
                            Some(HWND(hwnd as *mut _)),
                            WM_CLOSE,
                            WPARAM(0),
                            LPARAM(0),
                        );
                    }
                }
                let _ = listener.thread.join();
            }
        }
    }

    impl Drop for ClipboardService {
        fn drop(&mut self) {
            self.stop();
        }
    }

    fn register_format(name: &str) -> u32 {
        let name = wide(name);
        // SAFETY: `name` is NUL-terminated and outlives the call.
        unsafe { RegisterClipboardFormatW(pcwstr(&name)) }
    }

    fn excluded_present(excluded: &[u32]) -> bool {
        excluded.iter().any(|&f| {
            // SAFETY: format availability query, no memory preconditions.
            f != 0 && unsafe { IsClipboardFormatAvailable(f) }.is_ok()
        })
    }

    /// Opens the clipboard, retrying while another process holds it.
    fn open(hwnd: Option<HWND>) -> bool {
        for attempt in 0..5 {
            // SAFETY: OpenClipboard has no memory preconditions; paired with CloseClipboard.
            if unsafe { OpenClipboard(hwnd) }.is_ok() {
                return true;
            }
            if attempt < 4 {
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        false
    }

    /// Puts a PNG on the clipboard (as `CF_DIBV5` plus the registered "PNG" format) without going
    /// through the history service: the assistant's "Copy image".
    pub fn set_image_png(png: &[u8]) -> Result<(), String> {
        let image = decode_png(png).ok_or_else(|| "The image is unreadable.".to_owned())?;
        let mut formats = vec![(u32::from(CF_DIBV5.0), rgba_to_dibv5(&image))];
        let png_format = register_format("PNG");
        if png_format != 0 {
            formats.push((png_format, png.to_vec()));
        }
        write_clipboard(&formats)
    }

    fn write_clipboard(formats: &[(u32, Vec<u8>)]) -> Result<(), String> {
        const BUSY: &str = "The clipboard is busy. Try again.";
        if !open(None) {
            return Err(BUSY.to_owned());
        }
        let mut result = Ok(());
        // SAFETY: the clipboard is open; every allocation handed to SetClipboardData is moveable
        // global memory that the system takes ownership of on success.
        unsafe {
            if EmptyClipboard().is_err() {
                result = Err(BUSY.to_owned());
            } else {
                for (format, bytes) in formats {
                    let Ok(memory) = GlobalAlloc(GMEM_MOVEABLE, bytes.len()) else {
                        result = Err("Not enough memory to copy.".to_owned());
                        break;
                    };
                    let target = GlobalLock(memory);
                    if target.is_null() {
                        result = Err("Not enough memory to copy.".to_owned());
                        break;
                    }
                    std::ptr::copy_nonoverlapping(bytes.as_ptr(), target.cast::<u8>(), bytes.len());
                    let _ = GlobalUnlock(memory);
                    if SetClipboardData(*format, Some(HANDLE(memory.0))).is_err() {
                        result = Err(BUSY.to_owned());
                        break;
                    }
                }
            }
            let _ = CloseClipboard();
        }
        result
    }

    /// Copies the bytes of a global-memory clipboard format (clipboard must be open).
    fn read_format(format: u32) -> Option<Vec<u8>> {
        // SAFETY: the clipboard is open on this thread; the handle is clipboard-owned global
        // memory, locked only while copied, and read within GlobalSize bounds.
        unsafe {
            let handle = GetClipboardData(format).ok()?;
            let memory = HGLOBAL(handle.0);
            let size = GlobalSize(memory);
            if size == 0 || size > MAX_READ_BYTES {
                return None;
            }
            let source = GlobalLock(memory);
            if source.is_null() {
                return None;
            }
            let bytes = std::slice::from_raw_parts(source.cast::<u8>(), size).to_vec();
            let _ = GlobalUnlock(memory);
            Some(bytes)
        }
    }

    /// Reads the clipboard for the assistant composer, whatever the history setting: a PNG
    /// stream (at most 16 MB like C#), dropped files, a bitmap converted to PNG, or text.
    /// `Err` means the clipboard stayed busy (C# "The clipboard is busy. Try pasting again.").
    pub fn read_paste() -> Result<PasteContent, String> {
        let png = register_format("PNG");
        let image_png = register_format("image/png");
        let mut opened = false;
        for attempt in 0..3 {
            // SAFETY: OpenClipboard has no memory preconditions; closed below.
            if unsafe { OpenClipboard(None) }.is_ok() {
                opened = true;
                break;
            }
            if attempt < 2 {
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        if !opened {
            return Err("The clipboard is busy. Try pasting again.".to_owned());
        }
        // SAFETY: format availability queries, no memory preconditions.
        let has = |f: u32| f != 0 && unsafe { IsClipboardFormatAvailable(f) }.is_ok();
        let hdrop = u32::from(CF_HDROP.0);
        let bitmap_format = [u32::from(CF_DIBV5.0), u32::from(CF_DIB.0)]
            .into_iter()
            .find(|&f| has(f));
        let choice = paste_choice(
            has(png),
            has(image_png),
            has(hdrop),
            bitmap_format.is_some(),
            has(u32::from(CF_UNICODETEXT.0)),
        );
        let content =
            match choice {
                Some(PasteKind::Png) => read_format(png).map(PasteContent::Png),
                Some(PasteKind::ImagePng) => read_format(image_png).map(PasteContent::Png),
                Some(PasteKind::Files) => read_format(hdrop)
                    .map(|b| PasteContent::Files(crate::dragdrop::parse_hdrop(&b))),
                Some(PasteKind::Bitmap) => bitmap_format
                    .and_then(read_format)
                    .and_then(|b| dib_to_rgba(&b).ok())
                    .and_then(|i| encode_png(i.width, i.height, &i.rgba))
                    .map(PasteContent::Png),
                Some(PasteKind::Text) => read_format(u32::from(CF_UNICODETEXT.0))
                    .map(|b| PasteContent::Text(read_text(&b))),
                None => None,
            };
        // SAFETY: closes the clipboard opened above.
        unsafe {
            let _ = CloseClipboard();
        }
        match content {
            Some(PasteContent::Png(b))
                if b.len() as u64 > naby_core::assistant::attachments::MAXIMUM_FILE_BYTES =>
            {
                Err("The clipboard image exceeds 16 MB.".to_owned())
            }
            Some(c) => Ok(c),
            None => Ok(PasteContent::Empty),
        }
    }

    fn read_text(bytes: &[u8]) -> String {
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .take_while(|&u| u != 0)
            .collect();
        String::from_utf16_lossy(&units)
    }

    fn push(shared: &Shared, event: ClipboardEvent) {
        shared
            .events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(event);
        (shared.wake)();
    }

    /// Everything C# `ClipboardMessage` does for one `WM_CLIPBOARDUPDATE`.
    fn on_update(shared: &Shared, hwnd: HWND) {
        // SAFETY: argument-free query.
        let sequence = unsafe { GetClipboardSequenceNumber() };
        if should_skip(
            true,
            shared.paused.load(Ordering::SeqCst),
            shared.writing.load(Ordering::SeqCst),
            sequence,
            shared.last_write_sequence.load(Ordering::SeqCst),
            excluded_present(&shared.excluded),
        ) {
            return;
        }
        if !open(Some(hwnd)) {
            push(
                shared,
                ClipboardEvent::Error("The clipboard is busy.".to_owned()),
            );
            return;
        }
        let mut text = None;
        let mut dib = None;
        // SAFETY: format availability queries, no memory preconditions.
        let has = |f: u32| unsafe { IsClipboardFormatAvailable(f) }.is_ok();
        if has(u32::from(CF_UNICODETEXT.0)) {
            text = read_format(u32::from(CF_UNICODETEXT.0)).map(|b| read_text(&b));
        } else if has(u32::from(CF_DIBV5.0)) {
            dib = read_format(u32::from(CF_DIBV5.0));
        } else if has(u32::from(CF_DIB.0)) {
            dib = read_format(u32::from(CF_DIB.0));
        }
        // SAFETY: closes the clipboard opened above.
        unsafe {
            let _ = CloseClipboard();
        }
        // Reject if the owner changed while data was fetched.
        // SAFETY: argument-free query.
        let after = unsafe { GetClipboardSequenceNumber() };
        if read_is_stale(sequence, after, excluded_present(&shared.excluded)) {
            return;
        }
        if let Some(text) = text {
            push(shared, ClipboardEvent::Text(text));
        } else if let Some(png) = dib.as_deref().and_then(dib_to_png) {
            push(shared, ClipboardEvent::Png(png));
        }
    }

    unsafe extern "system" fn window_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match message {
            WM_CLIPBOARDUPDATE => {
                let shared = LISTENER.with(|l| l.borrow().clone());
                if let Some(shared) = shared {
                    on_update(&shared, hwnd);
                }
                LRESULT(0)
            }
            WM_CLOSE => {
                // SAFETY: destroys the window this procedure belongs to, on its own thread.
                unsafe {
                    let _ = RemoveClipboardFormatListener(hwnd);
                    let _ = DestroyWindow(hwnd);
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                // SAFETY: ends this thread's message loop.
                unsafe { PostQuitMessage(0) };
                LRESULT(0)
            }
            // SAFETY: default handling of a message this procedure received.
            _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
        }
    }

    fn run_listener(shared: Arc<Shared>, ready: mpsc::Sender<Result<(), String>>) {
        let class = wide(CLASS_NAME);
        // SAFETY: standard window-class registration and message-only window creation on the
        // thread that runs the message loop; every pointer outlives its call.
        unsafe {
            let instance = GetModuleHandleW(None).unwrap_or_default();
            let wc = WNDCLASSW {
                lpfnWndProc: Some(window_proc),
                hInstance: instance.into(),
                lpszClassName: pcwstr(&class),
                ..Default::default()
            };
            RegisterClassW(&wc);
            let created = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                pcwstr(&class),
                pcwstr(&class),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                Some(instance.into()),
                None,
            );
            let hwnd = match created {
                Ok(h) => h,
                Err(e) => {
                    let _ = ready.send(Err(e.message()));
                    let _ = UnregisterClassW(pcwstr(&class), Some(instance.into()));
                    return;
                }
            };
            if let Err(e) = AddClipboardFormatListener(hwnd) {
                let _ = DestroyWindow(hwnd);
                let _ = UnregisterClassW(pcwstr(&class), Some(instance.into()));
                let _ = ready.send(Err(e.message()));
                return;
            }
            LISTENER.with(|l| *l.borrow_mut() = Some(shared.clone()));
            shared.hwnd.store(hwnd.0 as isize, Ordering::SeqCst);
            let _ = ready.send(Ok(()));
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            shared.hwnd.store(0, Ordering::SeqCst);
            LISTENER.with(|l| *l.borrow_mut() = None);
            let _ = UnregisterClassW(pcwstr(&class), Some(instance.into()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dib_header(bpp: u16, w: i32, h: i32, compression: u32, colors: u32) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&40u32.to_le_bytes());
        v.extend_from_slice(&w.to_le_bytes());
        v.extend_from_slice(&h.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&bpp.to_le_bytes());
        v.extend_from_slice(&compression.to_le_bytes());
        v.extend_from_slice(&[0; 12]);
        v.extend_from_slice(&colors.to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes());
        v
    }

    #[test]
    fn excluded_formats_are_the_csharp_three() {
        assert_eq!(EXCLUDED_FORMAT_NAMES.len(), 3);
        assert!(EXCLUDED_FORMAT_NAMES.contains(&"ExcludeClipboardContentFromMonitorProcessing"));
        assert!(EXCLUDED_FORMAT_NAMES.contains(&"CanIncludeInClipboardHistory"));
        assert!(EXCLUDED_FORMAT_NAMES.contains(&"CanUploadToCloudClipboard"));
    }

    #[test]
    fn skip_decisions_follow_the_csharp_guard() {
        assert!(!should_skip(true, false, false, 7, 0, false));
        assert!(should_skip(false, false, false, 7, 0, false), "disabled");
        assert!(should_skip(true, true, false, 7, 0, false), "paused");
        assert!(
            should_skip(true, false, true, 7, 0, false),
            "own write in progress"
        );
        assert!(
            should_skip(true, false, false, 7, 7, false),
            "own write sequence"
        );
        assert!(
            !should_skip(true, false, false, 0, 0, false),
            "sequence 0 is never ours"
        );
        assert!(
            should_skip(true, false, false, 8, 7, true),
            "password manager format"
        );
        assert!(read_is_stale(7, 8, false));
        assert!(read_is_stale(7, 7, true));
        assert!(!read_is_stale(7, 7, false));
    }

    #[test]
    fn bottom_up_24_bit_dib_converts_with_padding() {
        // 2x2, rows padded to 8 bytes; bottom row first: (blue, white) then (red, green).
        let mut dib = dib_header(24, 2, 2, 0, 0);
        dib.extend_from_slice(&[255, 0, 0, 255, 255, 255, 0, 0]);
        dib.extend_from_slice(&[0, 0, 255, 0, 255, 0, 0, 0]);
        let image = dib_to_rgba(&dib).expect("valid dib");
        assert_eq!((image.width, image.height), (2, 2));
        assert_eq!(&image.rgba[0..4], &[255, 0, 0, 255], "top-left red");
        assert_eq!(&image.rgba[4..8], &[0, 255, 0, 255], "top-right green");
        assert_eq!(&image.rgba[8..12], &[0, 0, 255, 255], "bottom-left blue");
        assert_eq!(
            &image.rgba[12..16],
            &[255, 255, 255, 255],
            "bottom-right white"
        );
    }

    #[test]
    fn top_down_dib_keeps_row_order() {
        let mut dib = dib_header(24, 1, -2, 0, 0);
        dib.extend_from_slice(&[0, 0, 255, 0]); // red first row
        dib.extend_from_slice(&[255, 0, 0, 0]); // blue second row
        let image = dib_to_rgba(&dib).unwrap();
        assert_eq!(&image.rgba[0..4], &[255, 0, 0, 255]);
        assert_eq!(&image.rgba[4..8], &[0, 0, 255, 255]);
    }

    #[test]
    fn plain_32_bit_dib_is_opaque_and_bitfields_alpha_is_kept() {
        let mut dib = dib_header(32, 1, 1, 0, 0);
        dib.extend_from_slice(&[10, 20, 30, 0]); // B,G,R,unused
        assert_eq!(dib_to_rgba(&dib).unwrap().rgba, vec![30, 20, 10, 255]);

        let image = RgbaImage {
            width: 1,
            height: 1,
            rgba: vec![30, 20, 10, 128],
        };
        let v5 = rgba_to_dibv5(&image);
        assert_eq!(v5.len(), 124 + 4);
        assert_eq!(dib_to_rgba(&v5).unwrap().rgba, vec![30, 20, 10, 128]);
    }

    #[test]
    fn all_zero_alpha_in_a_32_bit_bitfield_dib_means_no_transparency() {
        let image = RgbaImage {
            width: 2,
            height: 1,
            rgba: vec![1, 2, 3, 0, 4, 5, 6, 0],
        };
        let rgba = dib_to_rgba(&rgba_to_dibv5(&image)).unwrap().rgba;
        assert_eq!(rgba, vec![1, 2, 3, 255, 4, 5, 6, 255]);
    }

    #[test]
    fn eight_bit_palette_dib_uses_the_color_table() {
        let mut dib = dib_header(8, 2, 1, 0, 2);
        dib.extend_from_slice(&[0, 0, 255, 0, 0, 255, 0, 0]); // red, green (BGRX)
        dib.extend_from_slice(&[1, 0, 0, 0]);
        let image = dib_to_rgba(&dib).unwrap();
        assert_eq!(image.rgba, vec![0, 255, 0, 255, 255, 0, 0, 255]);
    }

    #[test]
    fn rgb565_bitfields_scale_to_eight_bits() {
        let mut dib = dib_header(16, 1, 1, 3, 0);
        for m in [0xF800u32, 0x07E0, 0x001F] {
            dib.extend_from_slice(&m.to_le_bytes());
        }
        dib.extend_from_slice(&0xF800u16.to_le_bytes());
        dib.extend_from_slice(&[0, 0]);
        assert_eq!(dib_to_rgba(&dib).unwrap().rgba, vec![255, 0, 0, 255]);
    }

    #[test]
    fn malformed_dibs_are_rejected_not_panicked_on() {
        assert_eq!(dib_to_rgba(&[]), Err(DibError::Truncated));
        assert_eq!(dib_to_rgba(&[1, 2, 3, 4]), Err(DibError::Unsupported));
        let truncated = dib_header(24, 100, 100, 0, 0);
        assert_eq!(dib_to_rgba(&truncated), Err(DibError::Truncated));
        let huge = dib_header(32, 20_000, 20_000, 0, 0);
        assert_eq!(dib_to_rgba(&huge), Err(DibError::TooLarge));
        assert_eq!(
            dib_to_rgba(&dib_header(24, 0, 1, 0, 0)),
            Err(DibError::Invalid)
        );
        assert_eq!(
            dib_to_rgba(&dib_header(24, 1, 1, 1, 0)),
            Err(DibError::Unsupported),
            "RLE"
        );
    }

    #[test]
    fn paste_priority_matches_the_composer() {
        assert_eq!(
            paste_choice(true, true, true, true, true),
            Some(PasteKind::Png)
        );
        assert_eq!(
            paste_choice(false, true, true, true, true),
            Some(PasteKind::ImagePng)
        );
        assert_eq!(
            paste_choice(false, false, true, true, true),
            Some(PasteKind::Files)
        );
        assert_eq!(
            paste_choice(false, false, false, true, true),
            Some(PasteKind::Bitmap)
        );
        assert_eq!(
            paste_choice(false, false, false, false, true),
            Some(PasteKind::Text)
        );
        assert_eq!(paste_choice(false, false, false, false, false), None);
    }

    #[test]
    fn png_round_trip_and_size_bound() {
        let image = RgbaImage {
            width: 3,
            height: 2,
            rgba: (0..24).collect(),
        };
        let png = encode_png(3, 2, &image.rgba).unwrap();
        assert_eq!(decode_png(&png).unwrap(), image);
        assert!(encode_png(3, 2, &[0; 5]).is_none());
        let mut dib = dib_header(24, 1, 1, 0, 0);
        dib.extend_from_slice(&[0, 0, 255, 0]);
        let png = dib_to_png(&dib).unwrap();
        assert_eq!(naby_render::Image::probe_size(&png), Some((1, 1)));
    }
}
