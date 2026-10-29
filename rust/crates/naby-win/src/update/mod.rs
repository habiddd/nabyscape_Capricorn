//! Automatic updates (C# `OfficialUpdates.cs`, `UpdateHandoff.cs`, `UpdateRecovery.cs`).
//!
//! * [`service`]: the check / download / cancel state machine on worker threads, with an
//!   injected transport and setup inspection so every transition runs offline.
//! * [`handoff`]: the data and rules of the quit-install-restart hand-off to the update helper.
//! * [`recovery`]: the saved previous version (executable + JSON profiles) and its restore.
//! * `os` (Windows): Authenticode / file-version inspection, the helper process and the
//!   Windows implementation of the injected environment.
//!
//! The staging and recovery folder, the JSON documents, the kernel event names and the helper
//! file name are those of the C# releases, so an old app can update to this one and back.

pub mod handoff;
pub mod recovery;
pub mod service;

#[cfg(windows)]
pub mod os;

use std::fmt;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use naby_core::updates::{hex_upper, Sha256, UpdateError};

pub use handoff::HandoffData;
pub use service::{UpdateEvent, UpdateService, UpdateView};

/// `%LOCALAPPDATA%\NabyScape-Updates`: staged setup, hand-off, helper copy, rollback.
pub const UPDATES_FOLDER: &str = "NabyScape-Updates";
/// File name of the helper copy the running app starts with `--update-handoff`.
pub const HELPER_FILE: &str = "NabyScape-UpdateHelper.exe";
/// The staged, verified setup.
pub const SETUP_FILE: &str = "NabyScape-Setup.exe";
pub const HANDOFF_FILE: &str = "handoff.json";
pub const ERROR_LOG: &str = "update-error.log";

pub fn updates_folder() -> PathBuf {
    naby_core::paths::local_app_data().join(UPDATES_FOLDER)
}

/// 32 lowercase hex digits (a .NET `Guid` "N"), unique per call and process.
pub fn new_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let mut hasher = Sha256::new();
    hasher.update(&nanos.to_le_bytes());
    hasher.update(&std::process::id().to_le_bytes());
    hasher.update(&COUNTER.fetch_add(1, Ordering::Relaxed).to_le_bytes());
    // Stack and heap addresses add entropy between processes started in the same tick.
    let local = 0u8;
    hasher.update(&(std::ptr::addr_of!(local) as usize).to_le_bytes());
    hex_upper(&hasher.finish()[..16]).to_ascii_lowercase()
}

/// Whether `id` has the shape of [`new_id`].
pub fn valid_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// .NET `DateTime.UtcNow.Ticks` (100 ns since 0001-01-01), the unit of `StartedUtcTicks`.
pub fn now_ticks() -> i64 {
    const UNIX_EPOCH_TICKS: i64 = 621_355_968_000_000_000;
    let since = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    UNIX_EPOCH_TICKS + (since.as_nanos() / 100) as i64
}

/// Uppercase SHA-256 of a file, streamed.
pub fn hash_file(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(hex_upper(&hasher.finish()))
}

/// Failure of the hand-off or recovery steps. `Blocked` is C# `RecoveryBlockedException`:
/// something could not be confirmed stopped, so no automatic recovery may touch the files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HandoffFailure {
    Blocked(String),
    Failed(String),
}

impl fmt::Display for HandoffFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Blocked(m) | Self::Failed(m) => f.write_str(m),
        }
    }
}
impl std::error::Error for HandoffFailure {}

impl From<io::Error> for HandoffFailure {
    fn from(e: io::Error) -> Self {
        Self::Failed(e.to_string())
    }
}
impl From<UpdateError> for HandoffFailure {
    fn from(e: UpdateError) -> Self {
        Self::Failed(e.to_string())
    }
}
impl From<serde_json::Error> for HandoffFailure {
    fn from(e: serde_json::Error) -> Self {
        Self::Failed(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, HandoffFailure>;

pub(crate) fn failed<T>(message: &str) -> Result<T> {
    Err(HandoffFailure::Failed(message.to_owned()))
}

/// The Settings > Updates facts for a service view.
pub fn to_info(view: &UpdateView, version: &str, platform: &str) -> naby_ui::settings::UpdateInfo {
    naby_ui::settings::UpdateInfo {
        installed_version: version.to_owned(),
        platform: platform.to_owned(),
        status: view.status.clone(),
        available: view.available.clone(),
        busy: view.busy,
        can_download: view.can_download,
        can_install: view.can_install,
        can_restore: view.can_restore,
        install_kind: view.install_kind,
        recovery: view.recovery.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_lowercase_hex() {
        let a = new_id();
        let b = new_id();
        assert!(valid_id(&a) && valid_id(&b));
        assert_ne!(a, b);
        assert!(!valid_id("ABCDEF0123456789ABCDEF0123456789"));
        assert!(!valid_id("short"));
        assert!(!valid_id(&"g".repeat(32)));
    }

    #[test]
    fn dotnet_ticks_are_after_year_2020() {
        // 2020-01-01 = 637134336000000000 ticks.
        assert!(now_ticks() > 637_134_336_000_000_000);
    }

    #[test]
    fn hash_file_is_uppercase_sha256() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f");
        std::fs::write(&path, b"abc").unwrap();
        assert_eq!(
            hash_file(&path).unwrap(),
            "BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD"
        );
    }
}
