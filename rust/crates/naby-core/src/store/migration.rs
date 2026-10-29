//! One-time move of the pre-rename `LumaIsland` data folder to `NabyScape`.
//!
//! The legacy tree is copied to a staging sibling, validated, and published with a single
//! directory rename. The original is never modified, so any failure falls back to using it.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use serde_json::Value;

use super::document::parse_payload;
use super::Store;
use crate::session::SessionState;

/// Extra structural check applied to `<document>.json` while a migration is staged.
#[derive(Clone, Copy)]
pub struct MigrationValidator {
    pub document: &'static str,
    pub check: fn(&Value) -> bool,
}

fn array_or_null(v: Option<&Value>) -> bool {
    matches!(v, None | Some(Value::Null) | Some(Value::Array(_)))
}

fn built_in_validators() -> [MigrationValidator; 5] {
    [
        MigrationValidator {
            document: "settings",
            check: |v| v.is_object(),
        },
        MigrationValidator {
            document: "productivity",
            check: |v| {
                v.as_object().is_some_and(|o| {
                    ["Tasks", "Shelf", "Clips"]
                        .iter()
                        .all(|k| array_or_null(o.get(*k)))
                        && o.get("Note").is_none_or(|n| n.is_string() || n.is_null())
                })
            },
        },
        MigrationValidator {
            document: "session",
            check: |v| serde_json::from_value::<SessionState>(v.clone()).is_ok(),
        },
        MigrationValidator {
            document: "activities",
            check: |v| v.is_object(),
        },
        MigrationValidator {
            document: "calendar",
            check: |v| v.as_array().is_some_and(|a| a.iter().all(Value::is_object)),
        },
    ]
}

/// Lexical `GetFullPath`: absolute, `.`/`..` resolved, no file-system access.
fn normalize(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    let mut out = PathBuf::new();
    for c in absolute.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn invalid(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

fn copy_directory(source: &Path, destination: &Path) -> io::Result<()> {
    if fs::symlink_metadata(source)?.file_type().is_symlink() {
        return Err(invalid("Data folders cannot be links during migration."));
    }
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let meta = fs::symlink_metadata(entry.path())?;
        let target = destination.join(entry.file_name());
        if meta.file_type().is_symlink() {
            return Err(invalid("Data files cannot be links during migration."));
        }
        if meta.is_dir() {
            copy_directory(&entry.path(), &target)?;
        } else {
            if meta.permissions().readonly() {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!(
                        "A legacy data file is read-only: {}",
                        entry.file_name().to_string_lossy()
                    ),
                ));
            }
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

fn json_files(dir: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            json_files(&path, out)?;
        } else if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("json"))
        {
            out.push(path);
        }
    }
    Ok(())
}

fn read_json(file: &Path) -> io::Result<Value> {
    let bytes = fs::read(file)?;
    parse_payload(&bytes).map_err(|_| {
        invalid(&format!(
            "Invalid stored document: {}",
            file.file_name().unwrap_or_default().to_string_lossy()
        ))
    })
}

/// Points saved clipboard images that live inside the legacy cache at the new location.
fn rebase_clipboard_images(staging: &Path, legacy: &Path, destination: &Path) -> io::Result<()> {
    let file = staging.join("productivity.json");
    if !file.exists() {
        return Ok(());
    }
    let mut data = read_json(&file)?;
    let legacy = normalize(legacy);
    let cache = normalize(&legacy.join("clipboard-images"))
        .to_string_lossy()
        .to_lowercase();
    let cache_prefix = format!("{}{}", cache, std::path::MAIN_SEPARATOR);
    if let Some(clips) = data.get_mut("Clips").and_then(Value::as_array_mut) {
        for clip in clips {
            if clip.get("Kind").and_then(Value::as_str) != Some("image") {
                continue;
            }
            let path = clip
                .get("ImagePath")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if !Path::new(&path).is_absolute() {
                continue;
            }
            let full = normalize(Path::new(&path));
            if !full
                .to_string_lossy()
                .to_lowercase()
                .starts_with(&cache_prefix)
            {
                continue;
            }
            let relative: PathBuf = full
                .components()
                .skip(legacy.components().count())
                .collect();
            if !staging.join(&relative).is_file() {
                return Err(invalid("A saved clipboard image is missing."));
            }
            clip["ImagePath"] =
                Value::String(destination.join(&relative).to_string_lossy().into_owned());
        }
    }
    let bytes = serde_json::to_vec_pretty(&data).map_err(|e| invalid(&e.to_string()))?;
    fs::write(&file, bytes)
}

fn stage(
    legacy: &Path,
    destination: &Path,
    staging: &Path,
    extra: &[MigrationValidator],
) -> io::Result<()> {
    copy_directory(legacy, staging)?;
    let mut files = Vec::new();
    json_files(staging, &mut files)?;
    for file in &files {
        read_json(file)?; // every document must be a JSON object or array
    }
    for validator in built_in_validators().iter().chain(extra) {
        let file = staging.join(format!("{}.json", validator.document));
        if file.exists() && !(validator.check)(&read_json(&file)?) {
            return Err(invalid(&format!("{} data is invalid.", validator.document)));
        }
    }
    rebase_clipboard_images(staging, legacy, destination)
}

fn unique_suffix() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{:x}{:x}", std::process::id(), nanos)
}

/// Returns the folder to use and an optional user-facing warning.
fn resolve(
    legacy: &Path,
    destination: &Path,
    extra: &[MigrationValidator],
) -> (PathBuf, Option<String>) {
    let legacy = normalize(legacy);
    let destination = normalize(destination);
    if destination.is_dir() {
        let warning = legacy.is_dir().then(|| {
            "Both NabyScape and LumaIsland data exist. NabyScape data is in use; review the preserved LumaIsland backup before removing it."
                .to_string()
        });
        return (destination, warning);
    }
    if !legacy.is_dir() {
        return (destination, None);
    }
    let mut staging_name = destination.as_os_str().to_owned();
    staging_name.push(format!(".migration-{}", unique_suffix()));
    let staging = PathBuf::from(staging_name);
    let result = stage(&legacy, &destination, &staging, extra).and_then(|()| {
        // A same-volume directory rename publishes only the fully copied and validated tree.
        fs::rename(&staging, &destination)
    });
    match result {
        Ok(()) => (
            destination,
            Some("Local data moved to NabyScape. The original LumaIsland folder is preserved as a backup.".into()),
        ),
        Err(e) => {
            let _ = fs::remove_dir_all(&staging);
            (
                legacy,
                Some(format!(
                    "Data migration could not complete; the original LumaIsland data remains in use. Review storage permissions and retry. {e}"
                )),
            )
        }
    }
}

impl Store {
    /// Moves `legacy` to `destination` once (see module docs) and opens whichever folder is in use.
    /// The result's `warning()` explains a move, a preserved duplicate, or a fallback to `legacy`.
    pub fn migrate(legacy: &Path, destination: &Path) -> Store {
        Self::migrate_with(legacy, destination, &[])
    }

    /// Like [`Store::migrate`] with extra per-document validators (for example the typed settings
    /// check), run on the staged copy before it is published.
    pub fn migrate_with(
        legacy: &Path,
        destination: &Path,
        validators: &[MigrationValidator],
    ) -> Store {
        let (dir, warning) = resolve(legacy, destination, validators);
        Store::with_warning(dir, warning)
    }
}
