//! Start with Windows (C# `StartupService`): one value under
//! `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.

/// Registry sub-key of the per-user Run list.
pub const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
/// The value this product writes.
pub const VALUE_NAME: &str = "NabyScape";
/// Values written by earlier product names; read as "enabled" and removed on every change.
pub const LEGACY_VALUE_NAMES: [&str; 2] = ["BillyNabilaIsland", "LumaIsland"];

/// The command stored in the Run value: the quoted executable plus `--background`, so a login
/// start does not announce itself.
pub fn command_line(exe: &str) -> String {
    format!("\"{exe}\" --background")
}

#[cfg(windows)]
pub use os::*;

#[cfg(windows)]
mod os {
    use super::*;
    use crate::util::wide;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, WIN32_ERROR};
    use windows::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW,
        RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE,
        REG_OPTION_NON_VOLATILE, REG_SZ,
    };

    struct Key(HKEY);
    impl Drop for Key {
        fn drop(&mut self) {
            // SAFETY: closes the handle this wrapper opened.
            let _ = unsafe { RegCloseKey(self.0) };
        }
    }

    fn check(status: WIN32_ERROR, what: &str) -> std::io::Result<()> {
        if status.0 == 0 {
            Ok(())
        } else {
            Err(std::io::Error::other(format!(
                "{what} failed ({})",
                status.0
            )))
        }
    }

    fn has_value(key: &Key, name: &str) -> bool {
        let name = wide(name);
        // SAFETY: querying only the existence of a value; no data buffer requested.
        unsafe { RegQueryValueExW(key.0, PCWSTR(name.as_ptr()), None, None, None, None) }.0 == 0
    }

    /// Whether any current or legacy Run value exists.
    pub fn enabled() -> bool {
        let sub = wide(RUN_KEY);
        let mut handle = HKEY::default();
        // SAFETY: out-pointer references a live local; the key is closed by `Key`.
        let status = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(sub.as_ptr()),
                None,
                KEY_QUERY_VALUE,
                &mut handle,
            )
        };
        if status.0 != 0 {
            return false;
        }
        let key = Key(handle);
        std::iter::once(VALUE_NAME)
            .chain(LEGACY_VALUE_NAMES)
            .any(|n| has_value(&key, n))
    }

    /// Adds or removes the Run value (and always removes the legacy names).
    pub fn set(enabled: bool) -> std::io::Result<()> {
        let sub = wide(RUN_KEY);
        let mut handle = HKEY::default();
        // SAFETY: out-pointer references a live local; the key is closed by `Key`.
        let status = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(sub.as_ptr()),
                None,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE | KEY_QUERY_VALUE,
                None,
                &mut handle,
                None,
            )
        };
        check(status, "open Run key")?;
        let key = Key(handle);
        let name = wide(VALUE_NAME);
        if enabled {
            let exe = std::env::current_exe()?;
            let command = wide(&command_line(&exe.to_string_lossy()));
            // SAFETY: reinterprets the UTF-16 buffer as bytes for the duration of the call.
            let bytes = unsafe {
                std::slice::from_raw_parts(command.as_ptr() as *const u8, command.len() * 2)
            };
            // SAFETY: `name` is NUL-terminated and `bytes` stays alive across the call.
            let status =
                unsafe { RegSetValueExW(key.0, PCWSTR(name.as_ptr()), None, REG_SZ, Some(bytes)) };
            check(status, "write Run value")?;
        } else {
            delete(&key, &name)?;
        }
        for legacy in LEGACY_VALUE_NAMES {
            delete(&key, &wide(legacy))?;
        }
        Ok(())
    }

    fn delete(key: &Key, name: &[u16]) -> std::io::Result<()> {
        // SAFETY: `name` is NUL-terminated.
        let status = unsafe { RegDeleteValueW(key.0, PCWSTR(name.as_ptr())) };
        if status == ERROR_FILE_NOT_FOUND {
            return Ok(());
        }
        check(status, "delete Run value")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_command_matches_the_c_sharp_value() {
        assert_eq!(
            command_line(r"C:\Users\n\AppData\Local\Programs\NabyScape\NabyScape.exe"),
            "\"C:\\Users\\n\\AppData\\Local\\Programs\\NabyScape\\NabyScape.exe\" --background"
        );
    }

    #[test]
    fn legacy_names_are_the_two_previous_products() {
        assert_eq!(VALUE_NAME, "NabyScape");
        assert_eq!(LEGACY_VALUE_NAMES, ["BillyNabilaIsland", "LumaIsland"]);
    }
}
