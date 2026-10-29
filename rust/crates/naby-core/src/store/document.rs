//! Document load/save with schema sidecars, write-ahead version barrier and atomic replacement.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{Map, Value};

use super::{
    DocumentState, Store, StoreError, CURRENT_SCHEMA_VERSION, MAX_DOCUMENT_BYTES, MAX_SCHEMA_BYTES,
    SCHEMA_SIDECAR_SUFFIX,
};

/// Serializes document work across every `Store` in the process (the lock file covers other processes).
static GATE: Mutex<()> = Mutex::new(());
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

const DEVICE_NAMES: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Properties retired from `settings.json`; dropped on the next normal save.
const RETIRED_SETTINGS_KEYS: [&str; 4] = [
    "RendererMode",
    "ActivitySatellite",
    "SatelliteSource",
    "ActivitiesEnabled",
];

fn gate() -> MutexGuard<'static, ()> {
    GATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Schema 0 -> 1 for `settings`: keep the classic layout that predates configurable panel size,
/// and drop retired renderer preferences. Idempotent; other unknown fields are untouched.
pub(crate) fn settings_payload_migration(version: u32, payload: &mut Value) {
    let Value::Object(settings) = payload else {
        return;
    };
    if version == 0 && settings.get("CompactLayout") == Some(&Value::Bool(false)) {
        settings.entry("PanelWidth").or_insert(Value::from(452));
        settings.entry("PanelMaxHeight").or_insert(Value::from(634));
    }
    for retired in RETIRED_SETTINGS_KEYS {
        settings.remove(retired);
    }
}

/// True for a safe internal identifier: `[A-Za-z0-9][A-Za-z0-9_-]{0,63}`, not a Windows device name.
pub(crate) fn valid_name(name: &str) -> bool {
    let first_ok = name
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric());
    first_ok
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        && !DEVICE_NAMES.iter().any(|d| d.eq_ignore_ascii_case(name))
}

/// Whether `file` exists. Directories and links are refused as corrupt, never followed.
pub(crate) fn exists(file: &Path) -> Result<bool, StoreError> {
    match fs::symlink_metadata(file) {
        Ok(meta) => {
            if meta.is_dir() || meta.file_type().is_symlink() || is_reparse_point(&meta) {
                return Err(StoreError::Corrupt);
            }
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}

#[cfg(windows)]
fn is_reparse_point(meta: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    meta.file_attributes() & 0x400 != 0
}

#[cfg(not(windows))]
fn is_reparse_point(_: &fs::Metadata) -> bool {
    false
}

pub(crate) fn read_bounded(file: &Path, limit: usize) -> Result<Vec<u8>, StoreError> {
    let mut stream = File::open(file)?;
    if stream.metadata()?.len() > limit as u64 {
        return Err(StoreError::TooLarge);
    }
    let mut bytes = Vec::new();
    // Read one byte past the limit so a file that grows while we read is still caught.
    (&mut stream)
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(StoreError::TooLarge);
    }
    Ok(bytes)
}

/// Accepts the UTF-8 BOM used by older tools, but never replaces invalid UTF-8 silently.
pub(crate) fn parse_payload(bytes: &[u8]) -> Result<Value, StoreError> {
    let body = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    let text = std::str::from_utf8(body).map_err(|_| StoreError::Corrupt)?;
    match serde_json::from_str::<Value>(text) {
        Ok(v @ (Value::Object(_) | Value::Array(_))) => Ok(v),
        _ => Err(StoreError::Corrupt),
    }
}

/// Schema version and the raw sidecar object. Missing sidecar means schema 0.
pub(crate) fn read_schema(file: &Path) -> Result<(u32, Map<String, Value>), StoreError> {
    if !exists(file)? {
        let mut m = Map::new();
        m.insert("SchemaVersion".into(), Value::from(0));
        return Ok((0, m));
    }
    let Value::Object(metadata) = parse_payload(&read_bounded(file, MAX_SCHEMA_BYTES)?)? else {
        return Err(StoreError::Corrupt);
    };
    let version = metadata
        .get("SchemaVersion")
        .and_then(Value::as_i64)
        .filter(|v| *v >= 0)
        .ok_or(StoreError::Corrupt)?;
    let mut pending = version;
    if let Some(p) = metadata.get("PendingSchemaVersion") {
        pending = p
            .as_i64()
            .filter(|p| *p >= version)
            .ok_or(StoreError::Corrupt)?;
    }
    // Checked even when the payload is absent or save is called without a preceding load.
    let newest = version.max(pending);
    if newest > CURRENT_SCHEMA_VERSION as i64 {
        return Err(StoreError::FutureSchema(newest.min(u32::MAX as i64) as u32));
    }
    Ok((version as u32, metadata))
}

fn temp_name(file: &Path) -> PathBuf {
    let n = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut name = file.as_os_str().to_owned();
    name.push(format!(".tmp-{:x}{:x}{:x}", std::process::id(), nanos, n));
    PathBuf::from(name)
}

fn with_suffix(file: &Path, suffix: &str) -> PathBuf {
    let mut name = file.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// Writes `bytes` to a sibling temp file and renames it over `file`, keeping the previous
/// payload as `<file>.last-good.bak`. The live file is never deleted first.
pub(crate) fn atomic_write(file: &Path, bytes: &[u8]) -> Result<(), StoreError> {
    let temp = temp_name(file);
    let result = (|| -> Result<(), StoreError> {
        {
            let mut stream = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)?;
            stream.write_all(bytes)?;
            stream.sync_all()?;
        }
        if exists(file)? {
            let backup = with_suffix(file, ".last-good.bak");
            exists(&backup)?; // refuses a directory or link in the backup slot
            fs::copy(file, &backup)?;
        }
        fs::rename(&temp, file)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp); // a stale temp is harmless and is never loaded
    }
    result
}

pub(crate) fn encode(value: &Value, limit: usize) -> Result<Vec<u8>, StoreError> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|_| StoreError::Corrupt)?;
    if bytes.len() > limit {
        return Err(StoreError::TooLarge);
    }
    Ok(bytes)
}

/// Merges fields of `previous` that the typed model does not know (`known` is the model's own
/// projection of `previous`) into `next`. Arrays are replaced wholesale on purpose.
fn preserve_unknown(previous: &Value, next: &Value, known: &Value) -> Value {
    if let (Value::Object(old), Value::Object(new), Value::Object(known)) = (previous, next, known)
    {
        let mut merged = new.clone();
        for (key, old_value) in old {
            if !known.contains_key(key) && !new.contains_key(key) {
                merged.insert(key.clone(), old_value.clone());
            } else if let (Some(new_value), Some(known_value)) = (new.get(key), known.get(key)) {
                merged.insert(
                    key.clone(),
                    preserve_unknown(old_value, new_value, known_value),
                );
            }
        }
        return Value::Object(merged);
    }
    next.clone()
}

/// Cross-process exclusive writer lease (`.store-write.lock`). Released on drop.
struct Lease {
    _file: File,
}

fn acquire_write_lease(dir: &Path) -> Result<Lease, StoreError> {
    let path = dir.join(".store-write.lock");
    let started = Instant::now();
    loop {
        exists(&path)?;
        match open_lock(&path) {
            Ok(file) => return Ok(Lease { _file: file }),
            Err(e) if started.elapsed() < Duration::from_secs(2) => {
                let _ = e;
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(e) => return Err(e.into()),
        }
    }
}

#[cfg(windows)]
fn open_lock(path: &Path) -> std::io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    // Share mode 0 equals the C# FileShare.None lease, so both builds exclude each other.
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(0)
        .open(path)
}

#[cfg(not(windows))]
fn open_lock(path: &Path) -> std::io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    file.try_lock().map_err(|e| match e {
        fs::TryLockError::WouldBlock => std::io::ErrorKind::WouldBlock.into(),
        fs::TryLockError::Error(e) => e,
    })?;
    Ok(file)
}

impl Store {
    fn document_paths(&self, name: &str) -> Result<(PathBuf, PathBuf), StoreError> {
        if !valid_name(name) {
            return Err(StoreError::InvalidName);
        }
        Ok((
            self.dir.join(format!("{name}.json")),
            self.dir.join(format!("{name}{SCHEMA_SIDECAR_SUFFIX}")),
        ))
    }

    fn migrate_payload(&self, name: &str, payload: &Value, version: u32) -> Value {
        let mut migrated = payload.clone();
        if let Some(migration) = self.migrations.get(&name.to_ascii_lowercase()) {
            migration(version, &mut migrated);
        }
        migrated
    }

    fn set_state(&self, name: &str, state: DocumentState) {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        s.documents.insert(name.to_ascii_lowercase(), state);
    }

    /// Records a failure: the document needs recovery (or is future-schema, read-only) and the
    /// user-facing warning is set.
    fn protect(&self, name: &str, error: &StoreError) {
        let future = matches!(error, StoreError::FutureSchema(_));
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        s.documents.insert(
            name.to_ascii_lowercase(),
            if future {
                DocumentState::FutureSchema
            } else {
                DocumentState::RecoveryRequired
            },
        );
        s.warning = Some(
            if future {
                "A newer settings schema was found. This document is read-only. Use a compatible NabyScape version or recover a preserved backup."
            } else {
                "A stored document could not be read or saved safely. Defaults may be in use; the original file is preserved. Review recovery guidance before changing files."
            }
            .into(),
        );
    }

    pub(super) fn load_document<T: DeserializeOwned>(
        &self,
        name: &str,
    ) -> Result<Option<T>, StoreError> {
        let (file, schema) = self.document_paths(name)?;
        let _gate = gate();
        match self.load_locked::<T>(name, &file, &schema) {
            Ok(value) => Ok(value),
            Err(error) => {
                let future = match &error {
                    StoreError::FutureSchema(v) => Some(*v),
                    _ => None,
                };
                self.state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .fallback_loads
                    .insert(name.to_ascii_lowercase(), future);
                self.protect(name, &error);
                self.preserve_unreadable_copy(&file, &schema);
                Err(error)
            }
        }
    }

    fn load_locked<T: DeserializeOwned>(
        &self,
        name: &str,
        file: &Path,
        schema: &Path,
    ) -> Result<Option<T>, StoreError> {
        for _ in 0..3 {
            let (version, metadata) = read_schema(schema)?;
            if !exists(file)? {
                self.set_state(name, DocumentState::Missing);
                self.clear_fallback(name);
                return Ok(None);
            }
            let bytes = read_bounded(file, MAX_DOCUMENT_BYTES)?;
            let (_, after) = read_schema(schema)?;
            // An external writer may publish a version barrier while we read; retry rather than
            // pair a new payload with stale metadata.
            if metadata != after {
                continue;
            }
            let payload = self.migrate_payload(name, &parse_payload(&bytes)?, version);
            let value: T = serde_json::from_value(payload).map_err(|_| StoreError::Corrupt)?;
            self.set_state(
                name,
                if version == 0 {
                    DocumentState::Legacy
                } else {
                    DocumentState::Current
                },
            );
            self.clear_fallback(name);
            // Loading never rewrites data; migration is committed by the next explicit save.
            return Ok(Some(value));
        }
        Err(StoreError::Io(std::io::Error::other(
            "the stored document is busy",
        )))
    }

    fn clear_fallback(&self, name: &str) {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .fallback_loads
            .remove(&name.to_ascii_lowercase());
    }

    /// One-time local recovery copy of an unreadable payload. A failure here never authorizes
    /// rewriting the original.
    fn preserve_unreadable_copy(&self, file: &Path, schema: &Path) {
        let _ = (|| -> Result<(), StoreError> {
            let backup = with_suffix(file, ".backup-original");
            if exists(&backup)? {
                return Ok(());
            }
            let _lease = acquire_write_lease(&self.dir)?;
            read_schema(schema)?; // never touch an unknown future or malformed metadata document
            if !exists(&backup)? && exists(file)? {
                atomic_write(&backup, &read_bounded(file, MAX_DOCUMENT_BYTES)?)?;
            }
            Ok(())
        })();
    }

    pub(super) fn save_document<T: Serialize + DeserializeOwned>(
        &self,
        name: &str,
        value: &T,
    ) -> Result<(), StoreError> {
        let (file, schema) = self.document_paths(name)?;
        let _gate = gate();
        match self.save_locked(name, &file, &schema, value) {
            Ok(()) => Ok(()),
            Err(error) => {
                self.protect(name, &error);
                Err(error)
            }
        }
    }

    fn save_locked<T: Serialize + DeserializeOwned>(
        &self,
        name: &str,
        file: &Path,
        schema: &Path,
        value: &T,
    ) -> Result<(), StoreError> {
        // Revalidating disk cannot make values returned by a failed load trustworthy.
        let fallback = self
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .fallback_loads
            .get(&name.to_ascii_lowercase())
            .copied();
        if let Some(future) = fallback {
            return Err(match future {
                Some(v) => StoreError::FutureSchema(v),
                None => StoreError::RecoveryRequired,
            });
        }
        let _lease = acquire_write_lease(&self.dir)?;
        let (version, mut metadata) = read_schema(schema)?;
        let mut previous: Option<Value> = None;
        let mut known: Option<Value> = None;
        let mut original: Option<Vec<u8>> = None;
        if exists(file)? {
            let bytes = read_bounded(file, MAX_DOCUMENT_BYTES)?;
            let migrated = self.migrate_payload(name, &parse_payload(&bytes)?, version);
            // Defaults can never overwrite an unreadable payload. The typed projection tells
            // unknown model fields from map entries the user deliberately removed.
            let typed: T =
                serde_json::from_value(migrated.clone()).map_err(|_| StoreError::Corrupt)?;
            known = Some(serde_json::to_value(&typed).map_err(|_| StoreError::Corrupt)?);
            previous = Some(migrated);
            original = Some(bytes);
        }
        let mut next = serde_json::to_value(value).map_err(|_| StoreError::Corrupt)?;
        if !matches!(next, Value::Object(_) | Value::Array(_)) {
            return Err(StoreError::Corrupt);
        }
        if let (Some(previous), Some(known)) = (&previous, &known) {
            next = preserve_unknown(previous, &next, known);
        }
        let bytes = encode(&next, MAX_DOCUMENT_BYTES)?;
        if let (Some(original), 0) = (&original, version) {
            let backup = with_suffix(file, ".migration-v0.bak");
            if !exists(&backup)? {
                // Immutable: never replace a prior recovery source.
                atomic_write(&backup, original)?;
            }
        }

        // Write-ahead version barrier: publish the target version before changing the payload so
        // older readers fail closed even after a crash. With v0/pending1, readers apply the
        // idempotent migration to either old or new payload.
        metadata.insert(
            "PendingSchemaVersion".into(),
            Value::from(CURRENT_SCHEMA_VERSION),
        );
        let pending = encode(&Value::Object(metadata.clone()), MAX_SCHEMA_BYTES)?;
        metadata.insert("SchemaVersion".into(), Value::from(CURRENT_SCHEMA_VERSION));
        metadata.remove("PendingSchemaVersion");
        let committed = encode(&Value::Object(metadata), MAX_SCHEMA_BYTES)?;
        atomic_write(schema, &pending)?;
        atomic_write(file, &bytes)?;
        // Payload publication is the commit boundary. A pending=current barrier stays readable
        // and a later save finalizes it.
        self.set_state(name, DocumentState::Current);
        if atomic_write(schema, &committed).is_err() {
            self.state.lock().unwrap_or_else(|e| e.into_inner()).warning = Some(
                "The document was saved. Schema metadata finalization is pending and will be retried on a later save.".into(),
            );
        }
        Ok(())
    }
}
