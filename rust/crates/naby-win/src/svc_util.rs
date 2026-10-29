//! Small Win32 helpers shared by the host services in this crate.

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Com::{
    CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED, COINIT_MULTITHREADED,
};

/// NUL-terminated UTF-16 copy of `text`.
pub fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Borrow of a NUL-terminated buffer for API calls.
pub fn pcwstr(buffer: &[u16]) -> PCWSTR {
    PCWSTR(buffer.as_ptr())
}

/// UTF-16 up to the first NUL (or the whole slice).
pub fn from_wide(buffer: &[u16]) -> String {
    let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end])
}

/// Reads a NUL-terminated wide string owned by the OS.
///
/// # Safety
/// `pointer` must be null or point at a readable NUL-terminated UTF-16 string.
pub unsafe fn from_pwstr(pointer: *const u16) -> String {
    if pointer.is_null() {
        return String::new();
    }
    let mut len = 0usize;
    // SAFETY: the caller promises a NUL-terminated string, so every read up to the NUL is valid.
    while unsafe { *pointer.add(len) } != 0 {
        len += 1;
    }
    // SAFETY: `len` elements were just read from this allocation.
    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(pointer, len) })
}

/// COM initialised for the lifetime of the value (per thread).
pub struct ComGuard(bool);

impl ComGuard {
    /// Multithreaded apartment (WMI, free-threaded work).
    pub fn mta() -> Self {
        // SAFETY: plain COM initialisation of the calling thread; paired with CoUninitialize in Drop.
        Self(unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok())
    }

    /// Single-threaded apartment (shell items, OLE).
    pub fn sta() -> Self {
        // SAFETY: as in `mta`.
        Self(unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok())
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: balances the successful CoInitializeEx in the constructor, on the same thread.
            unsafe { CoUninitialize() };
        }
    }
}

/// Owned kernel handle that is closed on drop and may move between threads.
pub struct OwnedHandle(pub HANDLE);

// SAFETY: kernel handles are process-wide ids, usable from any thread.
unsafe impl Send for OwnedHandle {}
// SAFETY: as above; the wrapped operations are thread-safe kernel calls.
unsafe impl Sync for OwnedHandle {}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: the handle is owned by this value and closed exactly once.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
