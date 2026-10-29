//! Single-instance guard, exactly as `App.cs`: a named mutex plus two auto-reset events.
//! A second launch signals the first to open Settings and exits; `--exit` signals it to quit.
//! The kernel object names keep the original `LumaIsland` spelling so a Rust build and a C#
//! build of the same user exclude each other.

/// Prefix of all three kernel objects (C# `"Local\\LumaIsland"`).
pub const PREFIX: &str = "Local\\LumaIsland";
pub const MUTEX_NAME: &str = "Local\\LumaIsland.Desktop";
pub const OPEN_EVENT_NAME: &str = "Local\\LumaIsland.OpenSettings";
pub const EXIT_EVENT_NAME: &str = "Local\\LumaIsland.Exit";

#[cfg(windows)]
pub use os::*;

#[cfg(windows)]
mod os {
    use super::*;
    use crate::util::wide;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE};
    use windows::Win32::System::Threading::{
        CreateEventW, CreateMutexW, OpenEventW, SetEvent, EVENT_MODIFY_STATE,
    };

    /// Held for the life of the process by the first instance.
    pub struct Instance {
        mutex: HANDLE,
        open: HANDLE,
        exit: HANDLE,
    }

    pub enum Acquire {
        First(Instance),
        AlreadyRunning,
    }

    impl Instance {
        /// Takes ownership of the instance mutex, or reports that another process has it.
        pub fn acquire() -> windows::core::Result<Acquire> {
            let mutex_name = wide(MUTEX_NAME);
            // SAFETY: names are NUL-terminated; handles are closed in Drop.
            unsafe {
                let mutex = CreateMutexW(None, true, PCWSTR(mutex_name.as_ptr()))?;
                if GetLastError() == ERROR_ALREADY_EXISTS {
                    let _ = CloseHandle(mutex);
                    return Ok(Acquire::AlreadyRunning);
                }
                let open =
                    CreateEventW(None, false, false, PCWSTR(wide(OPEN_EVENT_NAME).as_ptr()))?;
                let exit =
                    CreateEventW(None, false, false, PCWSTR(wide(EXIT_EVENT_NAME).as_ptr()))?;
                Ok(Acquire::First(Instance { mutex, open, exit }))
            }
        }

        /// Signalled when another launch wants Settings opened.
        pub fn open_event(&self) -> HANDLE {
            self.open
        }

        /// Signalled by `--exit`.
        pub fn exit_event(&self) -> HANDLE {
            self.exit
        }
    }

    impl Drop for Instance {
        fn drop(&mut self) {
            // SAFETY: closes the handles created in `acquire`.
            unsafe {
                let _ = CloseHandle(self.open);
                let _ = CloseHandle(self.exit);
                let _ = CloseHandle(self.mutex);
            }
        }
    }

    fn signal(name: &str) {
        let name = wide(name);
        // SAFETY: opens an existing event by name, signals it, closes it. A missing event
        // means no instance is running, which is fine.
        unsafe {
            if let Ok(event) = OpenEventW(EVENT_MODIFY_STATE, false, PCWSTR(name.as_ptr())) {
                let _ = SetEvent(event);
                let _ = CloseHandle(event);
            }
        }
    }

    /// Asks the running instance to open Settings.
    pub fn signal_open() {
        signal(OPEN_EVENT_NAME);
    }

    /// Asks the running instance to quit (`--exit`).
    pub fn signal_exit() {
        signal(EXIT_EVENT_NAME);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kernel_object_names_match_the_c_sharp_build() {
        assert_eq!(MUTEX_NAME, format!("{PREFIX}.Desktop"));
        assert_eq!(OPEN_EVENT_NAME, format!("{PREFIX}.OpenSettings"));
        assert_eq!(EXIT_EVENT_NAME, format!("{PREFIX}.Exit"));
        assert!(PREFIX.starts_with("Local\\"));
    }
}
