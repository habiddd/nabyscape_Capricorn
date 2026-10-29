//! Versioned JSON document store, byte-compatible with the C# `Store`.
//!
//! Every document `<name>` is `<name>.json` plus a `<name>.schema.json` sidecar. Writes are atomic
//! (temp file + rename, previous payload kept as `.last-good.bak`), a document that failed to load
//! is never overwritten until a later `load` succeeds, and documents over 32 MiB are refused.

mod document;
mod migration;

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;

pub use migration::MigrationValidator;

/// Schema version this build reads and writes.
pub const CURRENT_SCHEMA_VERSION: u32 = 1;
/// Suffix of the schema sidecar file.
pub const SCHEMA_SIDECAR_SUFFIX: &str = ".schema.json";
/// Largest document in bytes.
pub const MAX_DOCUMENT_BYTES: usize = 32 * 1024 * 1024;
/// Largest schema sidecar in bytes.
pub(crate) const MAX_SCHEMA_BYTES: usize = 64 * 1024;

/// Why a document operation failed.
#[derive(Debug)]
pub enum StoreError {
    /// The document name is not a safe internal identifier.
    InvalidName,
    /// The document (or an attempted write) exceeds the 32 MiB limit.
    TooLarge,
    /// The payload or sidecar is not valid JSON of the expected shape.
    Corrupt,
    /// The document was written by a newer schema (carries that version). Read-only.
    FutureSchema(u32),
    /// An earlier load of this document failed; saving is blocked until a load succeeds.
    RecoveryRequired,
    /// Any other file-system failure.
    Io(std::io::Error),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreError::InvalidName => {
                f.write_str("a safe internal document identifier is required")
            }
            StoreError::TooLarge => f.write_str("the stored document exceeds the size limit"),
            StoreError::Corrupt => f.write_str("the stored document is not valid"),
            StoreError::FutureSchema(v) => write!(f, "the stored document uses newer schema {v}"),
            StoreError::RecoveryRequired => {
                f.write_str("the stored document requires recovery before it can be changed")
            }
            StoreError::Io(e) => write!(f, "storage error: {e}"),
        }
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            StoreError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for StoreError {
    fn from(e: std::io::Error) -> Self {
        StoreError::Io(e)
    }
}

/// Last observed condition of a document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentState {
    Missing,
    Legacy,
    Current,
    RecoveryRequired,
    FutureSchema,
}

/// Counts of observed documents by state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StoreHealth {
    pub observed_documents: usize,
    pub legacy_documents: usize,
    pub current_documents: usize,
    pub recovery_required_documents: usize,
    pub future_schema_documents: usize,
    pub has_storage_warning: bool,
}

/// Rewrites a loaded payload given `(schema_version, payload)`; must be idempotent.
pub type PayloadMigration = fn(u32, &mut Value);

#[derive(Default)]
pub(crate) struct State {
    /// Lower-cased document name to last observed state.
    pub(crate) documents: HashMap<String, DocumentState>,
    /// Documents whose last load failed; `Some(v)` when the cause was future schema `v`.
    pub(crate) fallback_loads: HashMap<String, Option<u32>>,
    pub(crate) warning: Option<String>,
}

/// A directory of versioned JSON documents.
pub struct Store {
    pub(crate) dir: PathBuf,
    pub(crate) state: Mutex<State>,
    pub(crate) migrations: HashMap<String, PayloadMigration>,
}

impl Store {
    /// Opens `dir` (created if needed) without any legacy migration.
    pub fn open(dir: PathBuf) -> Store {
        Self::with_warning(dir, None)
    }

    /// Opens `%LOCALAPPDATA%\NabyScape`, first moving a `LumaIsland` folder there once.
    pub fn open_default() -> Store {
        Self::migrate(&crate::paths::legacy_data_dir(), &crate::paths::data_dir())
    }

    pub(crate) fn with_warning(dir: PathBuf, warning: Option<String>) -> Store {
        // Creation failures surface on the first load/save; opening never panics.
        let _ = std::fs::create_dir_all(&dir);
        let mut migrations: HashMap<String, PayloadMigration> = HashMap::new();
        migrations.insert("settings".into(), document::settings_payload_migration);
        Store {
            dir,
            state: Mutex::new(State {
                warning,
                ..State::default()
            }),
            migrations,
        }
    }

    /// Registers a payload migration for one document name (replacing any default).
    pub fn register_payload_migration(&mut self, name: &str, migration: PayloadMigration) {
        self.migrations.insert(name.to_ascii_lowercase(), migration);
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Loads `name`. `Ok(None)` means the document does not exist; callers apply defaults.
    ///
    /// An `Err` leaves the files untouched, keeps a one-time `.backup-original` copy and blocks
    /// `save` of this document until a later `load` succeeds. Loading never rewrites data.
    pub fn load<T: DeserializeOwned>(&self, name: &str) -> Result<Option<T>, StoreError> {
        self.load_document(name)
    }

    /// Loads `name`, using `T::default()` when it is missing.
    pub fn load_or_default<T: DeserializeOwned + Default>(
        &self,
        name: &str,
    ) -> Result<T, StoreError> {
        Ok(self.load(name)?.unwrap_or_default())
    }

    /// Saves `value` as `name`, preserving fields of the stored document the model does not know.
    pub fn save<T: Serialize + DeserializeOwned>(
        &self,
        name: &str,
        value: &T,
    ) -> Result<(), StoreError> {
        self.save_document(name, value)
    }

    pub fn health(&self) -> StoreHealth {
        let s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let count = |k: DocumentState| s.documents.values().filter(|v| **v == k).count();
        StoreHealth {
            observed_documents: s.documents.len(),
            legacy_documents: count(DocumentState::Legacy),
            current_documents: count(DocumentState::Current),
            recovery_required_documents: count(DocumentState::RecoveryRequired),
            future_schema_documents: count(DocumentState::FutureSchema),
            has_storage_warning: s.warning.is_some(),
        }
    }

    /// State last observed for `name`, if it was loaded or saved.
    pub fn document_state(&self, name: &str) -> Option<DocumentState> {
        let s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        s.documents.get(&name.to_ascii_lowercase()).copied()
    }

    pub fn warning(&self) -> Option<String> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .warning
            .clone()
    }

    /// Appends to the local `errors.log` (never part of a diagnostics report).
    pub fn log(&self, error: &dyn fmt::Display) {
        use std::io::Write;
        let line = format!("{} {}\n", chrono::Local::now().to_rfc3339(), error);
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.join("errors.log"))
        {
            let _ = f.write_all(line.as_bytes());
        }
    }
}

#[cfg(test)]
mod tests;
