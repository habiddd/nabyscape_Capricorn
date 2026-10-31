//! Outside-click detection while the dock is shown (C# `OutsideClick`, a
//! `WH_MOUSE_LL` hook). The hook is installed only while the dock wants it and posts
//! `HostEvent::OutsideClick` with the screen position of every left or right button press; the
//! overlay decides whether the point is outside its contour.

/// `WM_LBUTTONDOWN` / `WM_RBUTTONDOWN`: the presses that dismiss the dock.
pub fn is_press(message: u32) -> bool {
    message == 0x0201 || message == 0x0204
}

#[cfg(windows)]
pub use os::OutsideClickHook;

#[cfg(windows)]
mod os {
    use super::is_press;
    use crate::event::HostEvent;
    use std::cell::RefCell;
    use std::sync::mpsc::Sender;
    use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, SetWindowsHookExW, UnhookWindowsHookEx, HHOOK, MSLLHOOKSTRUCT, WH_MOUSE_LL,
    };

    thread_local! {
        static TX: RefCell<Option<Sender<HostEvent>>> = const { RefCell::new(None) };
    }

    unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code >= 0 && is_press(wparam.0 as u32) {
            // SAFETY: for WH_MOUSE_LL `lparam` points to a MSLLHOOKSTRUCT valid for this call.
            let info = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
            TX.with(|tx| {
                if let Some(tx) = tx.borrow().as_ref() {
                    let _ = tx.send(HostEvent::OutsideClick {
                        x: info.pt.x,
                        y: info.pt.y,
                    });
                }
            });
        }
        // SAFETY: passes the event on unchanged.
        unsafe { CallNextHookEx(None, code, wparam, lparam) }
    }

    /// Keeps the hook installed; dropping it unhooks. Lives on the UI thread (its callbacks are
    /// delivered through that thread's message loop).
    pub struct OutsideClickHook {
        hook: HHOOK,
    }

    impl OutsideClickHook {
        pub fn install(tx: Sender<HostEvent>) -> Option<Self> {
            TX.with(|slot| *slot.borrow_mut() = Some(tx));
            // SAFETY: a low-level hook with a plain function callback; the module handle is ours.
            let hook = unsafe {
                let module = GetModuleHandleW(None).ok()?;
                SetWindowsHookExW(WH_MOUSE_LL, Some(hook_proc), Some(module.into()), 0)
            };
            match hook {
                Ok(hook) => Some(OutsideClickHook { hook }),
                Err(_) => {
                    TX.with(|slot| *slot.borrow_mut() = None);
                    None
                }
            }
        }
    }

    impl Drop for OutsideClickHook {
        fn drop(&mut self) {
            // SAFETY: unhooks the hook installed in `install`.
            let _ = unsafe { UnhookWindowsHookEx(self.hook) };
            TX.with(|slot| *slot.borrow_mut() = None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_left_and_right_presses_count() {
        assert!(is_press(0x0201));
        assert!(is_press(0x0204));
        assert!(!is_press(0x0202), "button up");
        assert!(!is_press(0x0200), "move");
        assert!(!is_press(0x0207), "middle button");
    }
}
