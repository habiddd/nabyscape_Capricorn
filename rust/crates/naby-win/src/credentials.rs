//! Assistant API keys in the Windows Credential Manager (C# `AssistantKeys`). Keys belong to
//! the current Windows user and never touch settings or layout JSON.
//!
//! The id/validation rules and the in-memory store are pure; `AssistantKeys` (Windows only)
//! talks to `CredReadW` / `CredWriteW` / `CredDeleteW`.

use std::collections::HashMap;
use std::fmt;

/// Largest secret the C# code accepts.
pub const MAX_SECRET_BYTES: usize = 2560;
/// Credential `UserName`.
pub const USER_NAME: &str = "NabyScape";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialError {
    InvalidIdentity,
    InvalidSecret,
    /// The stored blob is larger than any key this app writes.
    StoredInvalid,
    Os(&'static str),
}

impl fmt::Display for CredentialError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidIdentity => "Invalid provider identity.",
            Self::InvalidSecret => "Enter a valid API key.",
            Self::StoredInvalid => "The stored credential is invalid.",
            Self::Os(message) => message,
        })
    }
}
impl std::error::Error for CredentialError {}

/// `NabyScape/Assistant/<id>`; `id` is a provider GUID in the 32-hex "N" format.
pub fn target_name(id: &str) -> Result<String, CredentialError> {
    if id.len() != 32 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(CredentialError::InvalidIdentity);
    }
    Ok(format!("NabyScape/Assistant/{id}"))
}

/// The bytes to store for a pasted key: trimmed UTF-8, 1..=2560 bytes, no line breaks or NUL
/// (checked on the untrimmed text like C#).
pub fn secret_bytes(value: &str) -> Result<Vec<u8>, CredentialError> {
    let bytes = value.trim().as_bytes();
    if bytes.is_empty() || bytes.len() > MAX_SECRET_BYTES || value.contains(['\r', '\n', '\0']) {
        return Err(CredentialError::InvalidSecret);
    }
    Ok(bytes.to_vec())
}

/// Where assistant provider keys live (C# `IAssistantKeys`).
pub trait SecretStore {
    fn exists(&self, id: &str) -> Result<bool, CredentialError>;
    fn read(&self, id: &str) -> Result<Option<String>, CredentialError>;
    fn write(&mut self, id: &str, value: &str) -> Result<(), CredentialError>;
    fn remove(&mut self, id: &str) -> Result<(), CredentialError>;
}

/// Volatile store for diagnostics and tests (C# `AssistantMemoryKeys`).
#[derive(Debug, Default)]
pub struct MemoryKeys {
    values: HashMap<String, String>,
}

impl SecretStore for MemoryKeys {
    fn exists(&self, id: &str) -> Result<bool, CredentialError> {
        Ok(self.values.contains_key(&target_name(id)?))
    }
    fn read(&self, id: &str) -> Result<Option<String>, CredentialError> {
        Ok(self.values.get(&target_name(id)?).cloned())
    }
    fn write(&mut self, id: &str, value: &str) -> Result<(), CredentialError> {
        let target = target_name(id)?;
        let bytes = secret_bytes(value)?;
        self.values
            .insert(target, String::from_utf8_lossy(&bytes).into_owned());
        Ok(())
    }
    fn remove(&mut self, id: &str) -> Result<(), CredentialError> {
        self.values.remove(&target_name(id)?);
        Ok(())
    }
}

#[cfg(windows)]
pub use os::AssistantKeys;

#[cfg(windows)]
mod os {
    use super::*;
    use crate::util::wide;
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::Foundation::{GetLastError, ERROR_NOT_FOUND};
    use windows::Win32::Security::Credentials::{
        CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_FLAGS,
        CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC,
    };

    /// Generic credentials in the Credential Manager, persisted for this machine.
    #[derive(Debug, Default, Clone, Copy)]
    pub struct AssistantKeys;

    fn wipe(bytes: &mut [u8]) {
        for b in bytes.iter_mut() {
            // SAFETY: volatile write so the compiler cannot elide clearing secret bytes.
            unsafe { std::ptr::write_volatile(b, 0) };
        }
    }

    impl AssistantKeys {
        pub fn new() -> Self {
            Self
        }

        /// Reads the credential record; `Ok(None)` when absent.
        fn with_record<T>(
            target: &str,
            f: impl FnOnce(&CREDENTIALW) -> Result<T, CredentialError>,
        ) -> Result<Option<T>, CredentialError> {
            let name = wide(target);
            let mut record: *mut CREDENTIALW = std::ptr::null_mut();
            // SAFETY: `name` is NUL-terminated; `record` receives a buffer freed by CredFree.
            let read =
                unsafe { CredReadW(PCWSTR(name.as_ptr()), CRED_TYPE_GENERIC, None, &mut record) };
            if read.is_err() || record.is_null() {
                return Ok(None);
            }
            // SAFETY: CredReadW succeeded, so `record` points at a valid CREDENTIALW.
            let result = f(unsafe { &*record });
            // SAFETY: frees the buffer allocated by CredReadW exactly once.
            unsafe { CredFree(record as *const _) };
            result.map(Some)
        }
    }

    impl SecretStore for AssistantKeys {
        fn exists(&self, id: &str) -> Result<bool, CredentialError> {
            Ok(Self::with_record(&target_name(id)?, |_| Ok(()))?.is_some())
        }

        fn read(&self, id: &str) -> Result<Option<String>, CredentialError> {
            Self::with_record(&target_name(id)?, |record| {
                let len = record.CredentialBlobSize as usize;
                if len > MAX_SECRET_BYTES {
                    return Err(CredentialError::StoredInvalid);
                }
                if len == 0 || record.CredentialBlob.is_null() {
                    return Ok(String::new());
                }
                // SAFETY: the blob holds `CredentialBlobSize` readable bytes.
                let blob = unsafe { std::slice::from_raw_parts(record.CredentialBlob, len) };
                Ok(String::from_utf8_lossy(blob).into_owned())
            })
        }

        fn write(&mut self, id: &str, value: &str) -> Result<(), CredentialError> {
            let target = wide(&target_name(id)?);
            let user = wide(USER_NAME);
            let mut bytes = secret_bytes(value)?;
            let record = CREDENTIALW {
                Flags: CRED_FLAGS(0),
                Type: CRED_TYPE_GENERIC,
                TargetName: PWSTR(target.as_ptr() as *mut u16),
                CredentialBlobSize: bytes.len() as u32,
                CredentialBlob: bytes.as_mut_ptr(),
                Persist: CRED_PERSIST_LOCAL_MACHINE,
                UserName: PWSTR(user.as_ptr() as *mut u16),
                ..Default::default()
            };
            // SAFETY: the record and the buffers it points to live across the call; Windows
            // copies the data and does not write through the pointers.
            let result = unsafe { CredWriteW(&record, 0) };
            wipe(&mut bytes);
            result.map_err(|_| CredentialError::Os("Windows could not save the API key."))
        }

        fn remove(&mut self, id: &str) -> Result<(), CredentialError> {
            let target = wide(&target_name(id)?);
            // SAFETY: `target` is NUL-terminated.
            match unsafe { CredDeleteW(PCWSTR(target.as_ptr()), CRED_TYPE_GENERIC, None) } {
                Ok(()) => Ok(()),
                // SAFETY: reads the calling thread's last error right after the failure.
                Err(_) if unsafe { GetLastError() } == ERROR_NOT_FOUND => Ok(()),
                Err(_) => Err(CredentialError::Os("Windows could not remove the API key.")),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "0123456789abcdef0123456789ABCDEF";

    #[test]
    fn target_names_need_a_32_hex_identity() {
        assert_eq!(
            target_name(ID).unwrap(),
            "NabyScape/Assistant/0123456789abcdef0123456789ABCDEF"
        );
        for bad in [
            "",
            "abc",
            "0123456789abcdef0123456789abcdeg",
            &ID[..31],
            "{0123456789abcdef0123456789abcdef}",
        ] {
            assert_eq!(
                target_name(bad),
                Err(CredentialError::InvalidIdentity),
                "{bad}"
            );
        }
    }

    #[test]
    fn secrets_are_trimmed_and_bounded() {
        assert_eq!(secret_bytes("  sk-123 \t").unwrap(), b"sk-123");
        assert_eq!(secret_bytes("   "), Err(CredentialError::InvalidSecret));
        assert_eq!(secret_bytes(""), Err(CredentialError::InvalidSecret));
        assert!(secret_bytes(&"k".repeat(MAX_SECRET_BYTES)).is_ok());
        assert!(secret_bytes(&"k".repeat(MAX_SECRET_BYTES + 1)).is_err());
        assert!(secret_bytes("a\nb").is_err());
        assert!(secret_bytes("a\rb").is_err());
        assert!(secret_bytes("a\0b").is_err());
        // Even a trailing newline is rejected, as in C# (checked before trimming).
        assert!(secret_bytes("abc\n").is_err());
    }

    #[test]
    fn memory_store_round_trips() {
        let mut keys = MemoryKeys::default();
        assert_eq!(keys.exists(ID), Ok(false));
        keys.write(ID, " key ").unwrap();
        assert_eq!(keys.exists(ID), Ok(true));
        assert_eq!(keys.read(ID).unwrap().as_deref(), Some("key"));
        keys.remove(ID).unwrap();
        keys.remove(ID).unwrap();
        assert_eq!(keys.read(ID), Ok(None));
        assert!(keys.write("bad", "x").is_err());
    }
}
