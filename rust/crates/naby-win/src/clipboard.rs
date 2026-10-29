//! Plain-text clipboard access for the immediate-mode UI (`InputState::paste` in, copied text
//! out of `Ui::take_clipboard`).

/// Longest paste handed to a text field; guards against a pathological clipboard.
pub const MAX_PASTE_CHARS: usize = 1 << 20;

/// Normalizes pasted text for single- and multi-line fields: `\r\n` becomes `\n`, lone `\r`
/// is dropped, and the text is capped at `MAX_PASTE_CHARS` characters.
pub fn clean_paste(text: &str) -> String {
    let mut out = String::with_capacity(text.len().min(4096));
    let mut chars = text.chars().take(MAX_PASTE_CHARS).peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() != Some(&'\n') {
                    out.push('\n');
                }
            }
            '\0' => break,
            c => out.push(c),
        }
    }
    out
}

#[cfg(windows)]
pub use os::*;

#[cfg(windows)]
mod os {
    use super::clean_paste;
    use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND};
    use windows::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData,
    };
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};

    const CF_UNICODETEXT: u32 = 13;

    struct Open;
    impl Open {
        fn new(owner: Option<HWND>) -> Option<Open> {
            // SAFETY: balanced by CloseClipboard in Drop.
            unsafe { OpenClipboard(owner) }.ok().map(|_| Open)
        }
    }
    impl Drop for Open {
        fn drop(&mut self) {
            // SAFETY: closes the clipboard this guard opened.
            let _ = unsafe { CloseClipboard() };
        }
    }

    /// Current clipboard text, if any.
    pub fn get_text(owner: Option<HWND>) -> Option<String> {
        let _open = Open::new(owner)?;
        // SAFETY: the clipboard is open; the returned handle is owned by the system and only
        // read while locked.
        unsafe {
            let handle = GetClipboardData(CF_UNICODETEXT).ok()?;
            let global = HGLOBAL(handle.0);
            let ptr = GlobalLock(global) as *const u16;
            if ptr.is_null() {
                return None;
            }
            let max = 1 << 21;
            let mut len = 0;
            while len < max && *ptr.add(len) != 0 {
                len += 1;
            }
            let text = String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len));
            let _ = GlobalUnlock(global);
            Some(clean_paste(&text))
        }
    }

    /// Replaces the clipboard with `text`.
    pub fn set_text(owner: Option<HWND>, text: &str) -> bool {
        let units: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        let Some(_open) = Open::new(owner) else {
            return false;
        };
        // SAFETY: allocates a movable block, fills it while locked, and hands ownership to
        // the clipboard on success (freed here on failure).
        unsafe {
            let Ok(global) = GlobalAlloc(GMEM_MOVEABLE, units.len() * 2) else {
                return false;
            };
            let dst = GlobalLock(global) as *mut u16;
            if dst.is_null() {
                let _ = GlobalFree(Some(global));
                return false;
            }
            std::ptr::copy_nonoverlapping(units.as_ptr(), dst, units.len());
            let _ = GlobalUnlock(global);
            let _ = EmptyClipboard();
            if SetClipboardData(CF_UNICODETEXT, Some(HANDLE(global.0))).is_ok() {
                true
            } else {
                let _ = GlobalFree(Some(global));
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_endings_are_normalized() {
        assert_eq!(clean_paste("a\r\nb\rc\nd"), "a\nb\nc\nd");
        assert_eq!(clean_paste("x\0y"), "x");
        assert_eq!(clean_paste(""), "");
    }
}
