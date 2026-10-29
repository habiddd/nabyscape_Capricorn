//! Locations of the per-user data folders.

use std::path::PathBuf;

/// Folder name used by the current product.
pub const APP_FOLDER: &str = "NabyScape";
/// Folder name used by releases before the rename.
pub const LEGACY_FOLDER: &str = "LumaIsland";

/// The base folder for per-user application data
/// (`%LOCALAPPDATA%` on Windows, `$XDG_DATA_HOME` or `~/.local/share` elsewhere).
pub fn local_app_data() -> PathBuf {
    if let Some(p) = std::env::var_os("LOCALAPPDATA").filter(|p| !p.is_empty()) {
        return PathBuf::from(p);
    }
    if let Some(p) = std::env::var_os("XDG_DATA_HOME").filter(|p| !p.is_empty()) {
        return PathBuf::from(p);
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .unwrap_or_default();
    PathBuf::from(home).join(".local").join("share")
}

/// `%LOCALAPPDATA%\NabyScape`, shared with the C# releases so preferences carry over.
pub fn data_dir() -> PathBuf {
    local_app_data().join(APP_FOLDER)
}

/// `%LOCALAPPDATA%\LumaIsland`, read once for migration.
pub fn legacy_data_dir() -> PathBuf {
    local_app_data().join(LEGACY_FOLDER)
}
