//! File shelf model (C# `ShelfItem`, `AddFiles`, `RemoveFile`, shelf half of `Normalize`).
//! Only paths are stored; existence is checked live and broken references are kept.

use super::null_default;
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};

pub const MAX_SHELF: usize = 40;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct ShelfItem {
    #[serde(deserialize_with = "null_default")]
    pub path: String,
}

impl ShelfItem {
    /// File or folder name (empty for a filesystem root).
    pub fn name(&self) -> String {
        Path::new(&self.path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    pub fn exists(&self) -> bool {
        Path::new(&self.path).exists()
    }

    pub fn is_dir(&self) -> bool {
        Path::new(&self.path).is_dir()
    }

    /// Upper-case extension without the dot ("PNG"); empty for none.
    pub fn extension(&self) -> String {
        Path::new(&self.path)
            .extension()
            .map(|e| e.to_string_lossy().to_uppercase())
            .unwrap_or_default()
    }

    /// Second line of a shelf tile.
    pub fn subtitle(&self) -> String {
        if !self.exists() {
            return "File not found".into();
        }
        let extension = self.extension();
        if !extension.is_empty() {
            extension
        } else if self.is_dir() {
            "Folder".into()
        } else {
            "File".into()
        }
    }
}

/// Absolute, lexically normalized path without trailing separators (C# `GetFullPath` +
/// `TrimEndingDirectorySeparator`). `None` for blank or invalid input.
pub fn canonical_path(path: &str) -> Option<String> {
    if path.trim().is_empty() || path.contains('\0') {
        return None;
    }
    let raw = Path::new(path);
    let absolute = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(raw)
    };
    let mut out = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out.to_str().map(str::to_string)
}

pub(crate) fn same_path(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

/// Stages paths, skipping duplicates (case-insensitive) and invalid input; stops at 40.
/// Returns how many were added.
pub fn add_files<I, S>(shelf: &mut Vec<ShelfItem>, paths: I) -> usize
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut added = 0;
    for path in paths {
        if shelf.len() >= MAX_SHELF {
            break;
        }
        let Some(full) = canonical_path(path.as_ref()) else {
            continue;
        };
        if shelf.iter().any(|s| same_path(&s.path, &full)) {
            continue;
        }
        shelf.push(ShelfItem { path: full });
        added += 1;
    }
    added
}

/// Unstages a path. The file itself is never touched.
pub fn remove_file(shelf: &mut Vec<ShelfItem>, path: &str) -> bool {
    let Some(full) = canonical_path(path) else {
        return false;
    };
    let before = shelf.len();
    shelf.retain(|s| !same_path(&s.path, &full));
    shelf.len() != before
}

/// Canonicalizes, deduplicates and caps a stored shelf.
pub fn normalize(shelf: &mut Vec<ShelfItem>) {
    let mut out: Vec<ShelfItem> = Vec::new();
    for item in shelf.iter() {
        if out.len() >= MAX_SHELF {
            break;
        }
        let Some(path) = canonical_path(&item.path) else {
            continue;
        };
        if !out.iter().any(|s| same_path(&s.path, &path)) {
            out.push(ShelfItem { path });
        }
    }
    *shelf = out;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }
    fn s(p: &Path) -> String {
        p.to_str().unwrap().to_string()
    }

    #[test]
    fn handles_missing_paths_dedupe_and_cap() {
        let root = dir();
        let path = s(&root.path().join("missing.txt"));
        let mut shelf = Vec::new();
        assert_eq!(
            add_files(
                &mut shelf,
                [path.clone(), path.to_uppercase(), "\0".to_string()]
            ),
            1
        );
        assert!(!shelf[0].exists());
        assert_eq!(shelf[0].name(), "missing.txt");
        assert_eq!(shelf[0].subtitle(), "File not found");
        std::fs::write(&path, "file").unwrap();
        assert!(shelf[0].exists());
        assert_eq!(shelf[0].subtitle(), "TXT");
        add_files(
            &mut shelf,
            (0..50).map(|i| s(&root.path().join(format!("file{i}")))),
        );
        assert_eq!(shelf.len(), 40);
        assert!(remove_file(&mut shelf, &path));
        assert_eq!(shelf.len(), 39);
        assert!(Path::new(&path).exists());
    }

    #[test]
    fn broken_references_survive_a_json_roundtrip() {
        let root = dir();
        let path = s(&root.path().join("gone.txt"));
        let mut shelf = Vec::new();
        assert_eq!(add_files(&mut shelf, [&path]), 1);
        let restored: Vec<ShelfItem> =
            serde_json::from_str(&serde_json::to_string(&shelf).unwrap()).unwrap();
        assert_eq!(restored[0].path, path);
        assert!(!restored[0].exists());
    }

    #[test]
    fn trailing_separators_dedupe_and_roots_are_preserved() {
        let root = dir();
        let folder = root.path().join("folder");
        std::fs::create_dir(&folder).unwrap();
        let folder = s(&folder);
        let mut shelf = Vec::new();
        let with_sep = format!("{folder}{}", std::path::MAIN_SEPARATOR);
        assert_eq!(
            add_files(
                &mut shelf,
                [folder.clone(), with_sep.clone(), format!("{folder}/.")]
            ),
            1
        );
        assert_eq!(shelf[0].path, folder);
        assert!(shelf[0].exists() && shelf[0].is_dir());
        assert_eq!(shelf[0].subtitle(), "Folder");
        let fs_root = Path::new(&folder).ancestors().last().unwrap();
        assert_eq!(add_files(&mut shelf, [s(fs_root)]), 1);
        assert_eq!(shelf[1].path, s(fs_root));
        assert!(remove_file(&mut shelf, &with_sep));
        assert_eq!(shelf.len(), 1);
        assert!(Path::new(&folder).exists());
    }

    #[test]
    fn normalize_dedupes_and_drops_invalid() {
        let root = dir();
        let p = s(&root.path().join("a"));
        let mut shelf = vec![
            ShelfItem { path: p.clone() },
            ShelfItem {
                path: p.to_uppercase(),
            },
            ShelfItem { path: "\0".into() },
            ShelfItem {
                path: String::new(),
            },
        ];
        normalize(&mut shelf);
        assert_eq!(shelf, vec![ShelfItem { path: p }]);
    }
}
