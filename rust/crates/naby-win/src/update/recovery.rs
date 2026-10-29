//! The recovery copy (C# `UpdateRecovery.cs`): before an update installs, the running
//! executable and the profile's JSON documents are copied into
//! `NabyScape-Updates\rollback\<id>\` with a `snapshot.json` manifest of SHA-256 hashes. A
//! failed installation or start restores that copy; Settings can also restore it on request.
//!
//! Nothing here deletes recursively: only the fixed files a snapshot lists are removed.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Local, Utc};
use naby_core::updates::{require_automatic_install_path, verify_file, UpdateError, Version};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{failed, hash_file, new_id, valid_id, HandoffFailure, Result, UPDATES_FOLDER};
use crate::install::is_link;

pub const ROLLBACK_FOLDER: &str = "rollback";
pub const POINTER_FILE: &str = "previous.json";
pub const MANIFEST_FILE: &str = "snapshot.json";
pub const PREVIOUS_EXECUTABLE: &str = "NabyScape-Previous.exe";
const MANIFEST_LIMIT: u64 = 131_072;
const PROFILE_FILE_LIMIT: u64 = 32 * 1024 * 1024;
const PROFILE_TOTAL_LIMIT: u64 = 128 * 1024 * 1024;
const PROFILE_FILE_COUNT: usize = 256;
/// Installers before 2.22 ignore custom folders; their saved binary is restored in place.
const LAST_LEGACY_INSTALLER: Version = Version::new(2, 22, 0);

/// `snapshot.json` (same keys as C# `UpdateSnapshot`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct Snapshot {
    pub id: String,
    pub version: String,
    pub hash: String,
    pub profile_directory: String,
    pub created: DateTime<Utc>,
    pub profiles: BTreeMap<String, String>,
}

/// What recovery needs from the machine. The Windows implementation is `os::WinEnv`.
pub trait RecoveryEnv {
    fn updates_folder(&self) -> PathBuf;
    fn local_app_data(&self) -> PathBuf;
    /// Folder of the registered installation.
    fn install_dir(&self) -> PathBuf;
    /// The pre-rename install folder (`...\Programs\LumaIsland`).
    fn legacy_install_dir(&self) -> PathBuf;
    /// `OfficialUpdates.IsInstalled`.
    fn is_installed(&self, path: &Path) -> bool;
    /// `FileMajorPart.FileMinorPart.FileBuildPart` of an executable.
    fn file_version(&self, path: &Path) -> std::result::Result<Version, UpdateError>;
    /// `OfficialUpdates.InspectSetup`.
    fn inspect(&self, path: &Path, expected: &Version) -> std::result::Result<(), UpdateError>;
    /// Runs a setup/previous executable silently into [`RecoveryEnv::install_dir`].
    fn run_installer(&self, setup: &Path, recovery: bool) -> Result<()>;
    /// Writes `DisplayVersion` of the uninstall registration.
    fn set_display_version(&self, version: &str);
}

pub struct Recovery<'a> {
    pub env: &'a dyn RecoveryEnv,
}

/// `^[A-Za-z0-9][A-Za-z0-9._-]*\.json$`, at most 128 chars, never the manifest itself.
pub fn valid_profile_name(name: &str) -> bool {
    let b = name.as_bytes();
    name.len() <= 128
        && !name.eq_ignore_ascii_case(MANIFEST_FILE)
        && name.ends_with(".json")
        && b.first().is_some_and(u8::is_ascii_alphanumeric)
        && b.iter()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
}

fn trim_separators(path: &str) -> &str {
    path.trim_end_matches(['\\', '/'])
}

impl Recovery<'_> {
    fn root(&self) -> PathBuf {
        self.env.updates_folder().join(ROLLBACK_FOLDER)
    }

    fn pointer(&self) -> PathBuf {
        self.env.updates_folder().join(POINTER_FILE)
    }

    fn directory_for(&self, id: &str) -> Result<PathBuf> {
        if !valid_id(id) {
            return failed("Invalid recovery identity.");
        }
        let directory = self.root().join(id);
        for path in [self.env.updates_folder(), self.root(), directory.clone()] {
            if path.exists() && is_link(&path) {
                return failed("Recovery folders must not be links.");
            }
        }
        Ok(directory)
    }

    /// C# `ProfileDirectory`: only the standard per-user profile folder, never a link.
    pub fn profile_directory(&self, path: &str) -> Result<PathBuf> {
        let trimmed = trim_separators(path);
        let local = self.env.local_app_data();
        let standard = [
            naby_core::paths::APP_FOLDER,
            naby_core::paths::LEGACY_FOLDER,
        ]
        .iter()
        .any(|name| trimmed.eq_ignore_ascii_case(&local.join(name).to_string_lossy()));
        if !standard {
            return failed("Automatic recovery requires the standard per-user profile folder.");
        }
        let full = PathBuf::from(trimmed);
        if full.exists() && is_link(&full) {
            return failed("Recovery profile folder must not be a link.");
        }
        Ok(full)
    }

    /// C# `Capture`: copies the executable and the profile's JSON files, publishes the pointer
    /// last. A failed capture removes what it created and leaves the old pointer intact.
    pub fn capture(&self, installed: &Path, profile: &str) -> Result<String> {
        require_automatic_install_path(self.env.is_installed(installed), installed)?;
        let profile = self.profile_directory(profile)?;
        let version = self.env.file_version(installed)?;
        let mut snapshot = Snapshot {
            id: new_id(),
            version: version.to_string(),
            profile_directory: profile.to_string_lossy().into_owned(),
            created: Utc::now(),
            ..Snapshot::default()
        };
        let directory = self.directory_for(&snapshot.id)?;
        let pointer = self.pointer();
        let pointer_temp = pointer.with_file_name(format!("{POINTER_FILE}.{}.tmp", snapshot.id));
        if directory.exists() || pointer_temp.exists() {
            return failed("Recovery staging already exists.");
        }
        let mut owned: Vec<PathBuf> = Vec::new();
        let mut directory_created = false;
        let mut pointer_created = false;
        let result = (|| -> Result<()> {
            fs::create_dir_all(&directory)?;
            directory_created = true;
            let executable = directory.join(PREVIOUS_EXECUTABLE);
            owned.push(executable.clone());
            fs::copy(installed, &executable)?;
            snapshot.hash = hash_file(&executable)?;
            let mut total = 0u64;
            let mut entries: Vec<_> = fs::read_dir(&profile)?
                .filter_map(std::result::Result::ok)
                .filter(|e| {
                    e.path()
                        .extension()
                        .is_some_and(|x| x.eq_ignore_ascii_case("json"))
                        && e.path().is_file()
                })
                .collect();
            entries.sort_by_key(fs::DirEntry::file_name);
            for entry in entries {
                let name = entry.file_name().to_string_lossy().into_owned();
                let path = entry.path();
                let length = entry.metadata()?.len();
                total += length;
                if !valid_profile_name(&name)
                    || is_link(&path)
                    || length > PROFILE_FILE_LIMIT
                    || snapshot.profiles.len() >= PROFILE_FILE_COUNT
                    || total > PROFILE_TOTAL_LIMIT
                {
                    return failed(
                        "The profile cannot be backed up safely. Installation cancelled.",
                    );
                }
                let target = directory.join(&name);
                owned.push(target.clone());
                fs::copy(&path, &target)?;
                snapshot.profiles.insert(name, hash_file(&target)?);
            }
            let manifest = directory.join(MANIFEST_FILE);
            owned.push(manifest.clone());
            fs::write(&manifest, serde_json::to_string(&snapshot)?)?;
            // Publish only after the whole snapshot verifies.
            self.read(&snapshot.id, true)?;
            pointer_created = true;
            fs::write(&pointer_temp, serde_json::to_string(&snapshot.id)?)?;
            fs::rename(&pointer_temp, &pointer)?;
            Ok(())
        })();
        match result {
            Ok(()) => Ok(snapshot.id),
            Err(failure) => {
                let mut cleaned = true;
                for file in &owned {
                    if file.exists() && fs::remove_file(file).is_err() {
                        cleaned = false;
                    }
                }
                if pointer_created
                    && pointer_temp.exists()
                    && fs::remove_file(&pointer_temp).is_err()
                {
                    cleaned = false;
                }
                if directory_created && directory.exists() && fs::remove_dir(&directory).is_err() {
                    cleaned = false;
                }
                if cleaned {
                    Err(failure)
                } else {
                    Err(HandoffFailure::Failed(format!(
                        "Snapshot capture failed and some staging files could not be removed from {}. The previous recovery pointer is unchanged.",
                        directory.display()
                    )))
                }
            }
        }
    }

    /// C# `Read`: loads and validates a manifest; with `verify` also checks every hash and the
    /// saved executable's PE / version.
    pub fn read(&self, id: &str, verify: bool) -> Result<Snapshot> {
        let directory = self.directory_for(id)?;
        let manifest = directory.join(MANIFEST_FILE);
        if fs::metadata(&manifest)?.len() > MANIFEST_LIMIT {
            return failed("Invalid recovery manifest.");
        }
        let snapshot: Snapshot = serde_json::from_str(&fs::read_to_string(&manifest)?)
            .map_err(|_| HandoffFailure::Failed("Missing recovery manifest.".into()))?;
        let version: Option<Version> = snapshot.version.parse().ok();
        let Some(version) = version else {
            return failed("Invalid recovery manifest.");
        };
        if snapshot.id != id || snapshot.profiles.len() > PROFILE_FILE_COUNT {
            return failed("Invalid recovery manifest.");
        }
        self.profile_directory(&snapshot.profile_directory)?;
        if !verify {
            return Ok(snapshot);
        }
        let executable = directory.join(PREVIOUS_EXECUTABLE);
        verify_file(&executable, &snapshot.hash)?;
        self.env.inspect(&executable, &version)?;
        let mut total = 0u64;
        for (name, hash) in &snapshot.profiles {
            if !valid_profile_name(name) {
                return failed("Invalid recovery profile name.");
            }
            let file = directory.join(name);
            let length = fs::metadata(&file)?.len();
            total += length;
            if is_link(&file) || length > PROFILE_FILE_LIMIT || total > PROFILE_TOTAL_LIMIT {
                return failed("Invalid recovery profile size.");
            }
            verify_file(&file, hash)?;
        }
        Ok(snapshot)
    }

    /// C# `PreviousId`: the id in `previous.json`, `None` when absent or unreadable.
    pub fn previous_id(&self) -> Option<String> {
        let pointer = self.pointer();
        if fs::metadata(&pointer).ok()?.len() > 128 {
            return None;
        }
        let id: String = serde_json::from_str(&fs::read_to_string(pointer).ok()?).ok()?;
        valid_id(&id).then_some(id)
    }

    /// C# `Description`: the line shown under the Restore button.
    pub fn description(&self) -> String {
        let Some(id) = self.previous_id() else {
            return "No previous version has been saved by the updater yet.".to_owned();
        };
        match self.read(&id, false) {
            Ok(snapshot) => format!(
                "Previous version: {} \u{b7} saved {}",
                snapshot.version,
                snapshot.created.with_timezone(&Local).format("%x %H:%M")
            ),
            Err(_) => "The saved recovery copy is unavailable or could not be verified.".to_owned(),
        }
    }

    /// C# `Restore`: reinstalls the saved executable, then puts the saved JSON files back.
    pub fn restore(&self, id: &str, installed: &Path) -> Result<()> {
        require_automatic_install_path(self.env.is_installed(installed), installed)?;
        let snapshot = self.read(id, true)?;
        let directory = self.directory_for(id)?;
        let saved = directory.join(PREVIOUS_EXECUTABLE);
        let version: Version = snapshot
            .version
            .parse()
            .map_err(|_| HandoffFailure::Failed("Invalid recovery manifest.".into()))?;
        let in_legacy_folder = installed.parent().is_some_and(|p| {
            p.to_string_lossy()
                .eq_ignore_ascii_case(&self.env.legacy_install_dir().to_string_lossy())
        });
        if version < LAST_LEGACY_INSTALLER && !in_legacy_folder {
            // Pre-2.22 installers ignore custom directories; restore their verified binary in
            // place instead of executing a setup that writes to LumaIsland.
            restore_legacy_binary(&saved, installed, &snapshot.hash)?;
            self.env.set_display_version(&snapshot.version);
        } else {
            self.env.run_installer(&saved, true)?;
        }
        verify_file(installed, &snapshot.hash)?;
        let profile = self.profile_directory(&snapshot.profile_directory)?;
        for name in snapshot.profiles.keys() {
            let target = profile.join(name);
            let temp = profile.join(format!("{name}.restore-{}", new_id()));
            let copied =
                fs::copy(directory.join(name), &temp).and_then(|_| fs::rename(&temp, &target));
            if copied.is_err() {
                let _ = fs::remove_file(&temp);
            }
            copied?;
        }
        // A schema introduced after this snapshot must not label restored legacy data as
        // migrated. Remove only the sidecar for a payload actually restored.
        for name in snapshot
            .profiles
            .keys()
            .filter(|n| !n.to_ascii_lowercase().ends_with(".schema.json"))
        {
            let stem = name.strip_suffix(".json").unwrap_or(name);
            let metadata = format!("{stem}.schema.json");
            if snapshot
                .profiles
                .keys()
                .any(|saved| saved.eq_ignore_ascii_case(&metadata))
            {
                continue;
            }
            let path = profile.join(&metadata);
            if !path.exists() {
                continue;
            }
            if is_link(&path) || fs::metadata(&path)?.len() > 4096 {
                return failed("Recovery could not safely reset schema metadata.");
            }
            if !schema_sidecar_is_valid(&fs::read_to_string(&path)?) {
                return failed(
                    "Recovery schema metadata is invalid; the recovery copy is preserved.",
                );
            }
            fs::remove_file(&path)?;
        }
        Ok(())
    }

    /// C# `Prune`: deletes every other verified snapshot's fixed files (never recursively);
    /// a snapshot that cannot be verified is kept for manual inspection.
    pub fn prune(&self, keep: &str) {
        let Ok(entries) = fs::read_dir(self.root()) else {
            return;
        };
        for entry in entries.filter_map(std::result::Result::ok) {
            let id = entry.file_name().to_string_lossy().into_owned();
            if id == keep || !valid_id(&id) {
                continue;
            }
            let Ok(old) = self.read(&id, true) else {
                continue;
            };
            let directory = entry.path();
            for name in old.profiles.keys() {
                let _ = fs::remove_file(directory.join(name));
            }
            let _ = fs::remove_file(directory.join(PREVIOUS_EXECUTABLE));
            let _ = fs::remove_file(directory.join(MANIFEST_FILE));
            let _ = fs::remove_dir(&directory);
        }
    }
}

/// The sidecar `<name>.schema.json` holds only `SchemaVersion` (and optionally a null or
/// integer `PendingSchemaVersion`).
fn schema_sidecar_is_valid(text: &str) -> bool {
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(text) else {
        return false;
    };
    let is_i32 = |v: &Value| v.as_i64().is_some_and(|n| i32::try_from(n).is_ok());
    map.get("SchemaVersion").is_some_and(is_i32)
        && map
            .get("PendingSchemaVersion")
            .is_none_or(|v| v.is_null() || is_i32(v))
        && map
            .keys()
            .all(|k| k == "SchemaVersion" || k == "PendingSchemaVersion")
}

/// C# `RestoreLegacyBinary`: copy beside the target, verify, then replace it; a corrupt copy
/// never reaches the installed executable.
pub fn restore_legacy_binary(source: &Path, installed: &Path, hash: &str) -> Result<()> {
    let staged = installed.with_file_name(format!(
        "{}.restore-{}",
        installed.file_name().unwrap_or_default().to_string_lossy(),
        new_id()
    ));
    let result = fs::copy(source, &staged)
        .map_err(HandoffFailure::from)
        .and_then(|_| verify_file(&staged, hash).map_err(HandoffFailure::from))
        .and_then(|()| fs::rename(&staged, installed).map_err(HandoffFailure::from));
    let _ = fs::remove_file(&staged);
    result
}

/// Name of the folder under `%LOCALAPPDATA%` that holds all of the above.
pub const fn folder_name() -> &'static str {
    UPDATES_FOLDER
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::io::Write;

    struct Env {
        root: PathBuf,
        installed_version: Version,
        installs: RefCell<Vec<(PathBuf, bool)>>,
        display_version: RefCell<Option<String>>,
        inspect_fails: Cell<bool>,
    }

    impl Env {
        fn new(root: &Path) -> Self {
            Self {
                root: root.to_owned(),
                installed_version: Version::new(2, 24, 0),
                installs: RefCell::default(),
                display_version: RefCell::default(),
                inspect_fails: Cell::new(false),
            }
        }
        fn installed(&self) -> PathBuf {
            self.install_dir().join("NabyScape.exe")
        }
        fn profile(&self) -> PathBuf {
            self.root.join("Local").join("NabyScape")
        }
    }

    impl RecoveryEnv for Env {
        fn updates_folder(&self) -> PathBuf {
            self.root.join("Local").join(UPDATES_FOLDER)
        }
        fn local_app_data(&self) -> PathBuf {
            self.root.join("Local")
        }
        fn install_dir(&self) -> PathBuf {
            self.root.join("Local").join("Programs").join("NabyScape")
        }
        fn legacy_install_dir(&self) -> PathBuf {
            self.root.join("Local").join("Programs").join("LumaIsland")
        }
        fn is_installed(&self, path: &Path) -> bool {
            path.starts_with(self.install_dir())
        }
        fn file_version(&self, _p: &Path) -> std::result::Result<Version, UpdateError> {
            Ok(self.installed_version)
        }
        fn inspect(&self, _p: &Path, _v: &Version) -> std::result::Result<(), UpdateError> {
            if self.inspect_fails.get() {
                Err(UpdateError::Invalid(
                    "Setup version does not match the release.".into(),
                ))
            } else {
                Ok(())
            }
        }
        fn run_installer(&self, setup: &Path, recovery: bool) -> Result<()> {
            self.installs
                .borrow_mut()
                .push((setup.to_owned(), recovery));
            // The "installer" puts the saved binary in place.
            fs::copy(setup, self.installed())?;
            Ok(())
        }
        fn set_display_version(&self, version: &str) {
            *self.display_version.borrow_mut() = Some(version.to_owned());
        }
    }

    fn fixture() -> (tempfile::TempDir, Env) {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::new(dir.path());
        fs::create_dir_all(env.install_dir()).unwrap();
        fs::create_dir_all(env.profile()).unwrap();
        fs::write(env.installed(), b"old executable").unwrap();
        fs::write(env.profile().join("settings.json"), b"{\"a\":1}").unwrap();
        fs::write(
            env.profile().join("settings.schema.json"),
            b"{\"SchemaVersion\":1}",
        )
        .unwrap();
        fs::write(env.profile().join("notes.txt"), b"not json").unwrap();
        (dir, env)
    }

    fn profile_text(env: &Env) -> String {
        env.profile().to_string_lossy().into_owned()
    }

    #[test]
    fn profile_names_follow_the_c_sharp_pattern() {
        for ok in ["settings.json", "a-b_c.1.json", "A.json"] {
            assert!(valid_profile_name(ok), "{ok}");
        }
        for bad in [
            "snapshot.json",
            "SNAPSHOT.JSON",
            ".json",
            "-a.json",
            "a b.json",
            "a.txt",
            "a/b.json",
            "é.json",
        ] {
            assert!(!valid_profile_name(bad), "{bad}");
        }
        assert!(!valid_profile_name(&format!("{}.json", "a".repeat(124))));
    }

    #[test]
    fn capture_saves_executable_and_json_documents_then_publishes_the_pointer() {
        let (_dir, env) = fixture();
        let recovery = Recovery { env: &env };
        let id = recovery
            .capture(&env.installed(), &profile_text(&env))
            .unwrap();
        assert_eq!(recovery.previous_id().as_deref(), Some(id.as_str()));
        let snapshot = recovery.read(&id, true).unwrap();
        assert_eq!(snapshot.version, "2.24.0");
        assert_eq!(
            snapshot
                .profiles
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["settings.json", "settings.schema.json"],
            "only .json documents are saved"
        );
        let directory = env.updates_folder().join(ROLLBACK_FOLDER).join(&id);
        assert_eq!(
            fs::read(directory.join(PREVIOUS_EXECUTABLE)).unwrap(),
            b"old executable"
        );
        assert!(recovery
            .description()
            .starts_with("Previous version: 2.24.0 \u{b7} saved "));
    }

    #[test]
    fn missing_pointer_reads_as_no_previous_version() {
        let (_dir, env) = fixture();
        let recovery = Recovery { env: &env };
        assert_eq!(recovery.previous_id(), None);
        assert_eq!(
            recovery.description(),
            "No previous version has been saved by the updater yet."
        );
        fs::create_dir_all(env.updates_folder()).unwrap();
        fs::write(env.updates_folder().join(POINTER_FILE), b"\"not-an-id\"").unwrap();
        assert_eq!(recovery.previous_id(), None);
        fs::write(env.updates_folder().join(POINTER_FILE), b"{broken").unwrap();
        assert_eq!(recovery.previous_id(), None);
    }

    #[test]
    fn capture_refuses_portable_copies_and_foreign_profiles() {
        let (dir, env) = fixture();
        let recovery = Recovery { env: &env };
        let portable = dir.path().join("portable").join("NabyScape.exe");
        fs::create_dir_all(portable.parent().unwrap()).unwrap();
        fs::write(&portable, b"x").unwrap();
        assert!(recovery.capture(&portable, &profile_text(&env)).is_err());
        let elsewhere = dir.path().join("Other").to_string_lossy().into_owned();
        assert!(recovery
            .capture(&env.installed(), &elsewhere)
            .unwrap_err()
            .to_string()
            .contains("standard per-user profile folder"));
        // A legacy-named executable cannot update automatically.
        let legacy_exe = env.install_dir().join("LumaIsland.exe");
        fs::write(&legacy_exe, b"x").unwrap();
        assert!(recovery.capture(&legacy_exe, &profile_text(&env)).is_err());
    }

    #[test]
    fn unsafe_profile_documents_cancel_the_capture_and_leave_nothing_behind() {
        let (_dir, env) = fixture();
        let recovery = Recovery { env: &env };
        let good = recovery
            .capture(&env.installed(), &profile_text(&env))
            .unwrap();
        fs::write(env.profile().join("bad name.json"), b"{}").unwrap();
        let error = recovery
            .capture(&env.installed(), &profile_text(&env))
            .unwrap_err();
        assert!(error.to_string().contains("cannot be backed up safely"));
        // The old pointer survived and only the good snapshot's folder exists.
        assert_eq!(recovery.previous_id().as_deref(), Some(good.as_str()));
        let left: Vec<_> = fs::read_dir(env.updates_folder().join(ROLLBACK_FOLDER))
            .unwrap()
            .collect();
        assert_eq!(left.len(), 1);
        let pointers = fs::read_dir(env.updates_folder())
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".tmp")
            })
            .count();
        assert_eq!(pointers, 0);
    }

    #[test]
    fn a_tampered_snapshot_fails_verification() {
        let (_dir, env) = fixture();
        let recovery = Recovery { env: &env };
        let id = recovery
            .capture(&env.installed(), &profile_text(&env))
            .unwrap();
        let directory = env.updates_folder().join(ROLLBACK_FOLDER).join(&id);
        fs::write(directory.join("settings.json"), b"{\"a\":2}").unwrap();
        assert!(recovery
            .read(&id, true)
            .unwrap_err()
            .to_string()
            .contains("checksum"));
        // Without verification only the manifest is checked.
        assert!(recovery.read(&id, false).is_ok());
        assert!(recovery.restore(&id, &env.installed()).is_err());
        assert_eq!(fs::read(env.installed()).unwrap(), b"old executable");
    }

    #[test]
    fn a_saved_executable_that_fails_inspection_is_rejected() {
        let (_dir, env) = fixture();
        let recovery = Recovery { env: &env };
        let id = recovery
            .capture(&env.installed(), &profile_text(&env))
            .unwrap();
        env.inspect_fails.set(true);
        assert!(recovery.read(&id, true).is_err());
    }

    #[test]
    fn ids_and_manifests_are_validated() {
        let (_dir, env) = fixture();
        let recovery = Recovery { env: &env };
        for bad in ["", "../x", "ABCDEF0123456789ABCDEF0123456789", "abc"] {
            assert!(recovery.read(bad, false).is_err(), "{bad}");
        }
        let id = recovery
            .capture(&env.installed(), &profile_text(&env))
            .unwrap();
        let manifest = env
            .updates_folder()
            .join(ROLLBACK_FOLDER)
            .join(&id)
            .join(MANIFEST_FILE);
        let mut snapshot: Snapshot =
            serde_json::from_str(&fs::read_to_string(&manifest).unwrap()).unwrap();
        snapshot.id = new_id();
        fs::write(&manifest, serde_json::to_string(&snapshot).unwrap()).unwrap();
        assert!(
            recovery.read(&id, false).is_err(),
            "manifest id must match its folder"
        );
        snapshot.id = id.clone();
        snapshot.profile_directory = "C:\\elsewhere".into();
        fs::write(&manifest, serde_json::to_string(&snapshot).unwrap()).unwrap();
        assert!(recovery.read(&id, false).is_err());
    }

    #[test]
    fn restore_reinstalls_and_puts_profiles_back_without_stale_schema_sidecars() {
        let (_dir, env) = fixture();
        let recovery = Recovery { env: &env };
        // Capture a state whose settings.json has no sidecar of its own.
        fs::remove_file(env.profile().join("settings.schema.json")).unwrap();
        let id = recovery
            .capture(&env.installed(), &profile_text(&env))
            .unwrap();
        // The new version migrated the data and wrote a sidecar; the update changed the exe.
        fs::write(env.profile().join("settings.json"), b"{\"a\":99}").unwrap();
        fs::write(
            env.profile().join("settings.schema.json"),
            b"{\"SchemaVersion\":2,\"PendingSchemaVersion\":null}",
        )
        .unwrap();
        fs::write(env.installed(), b"new executable").unwrap();
        recovery.restore(&id, &env.installed()).unwrap();
        assert_eq!(fs::read(env.installed()).unwrap(), b"old executable");
        assert_eq!(
            fs::read(env.profile().join("settings.json")).unwrap(),
            b"{\"a\":1}"
        );
        assert!(!env.profile().join("settings.schema.json").exists());
        assert_eq!(env.installs.borrow().len(), 1);
        assert!(
            env.installs.borrow()[0].1,
            "the saved executable is run with --install"
        );
        assert!(env.display_version.borrow().is_none());
        // Restore temp files are cleaned up.
        assert!(fs::read_dir(env.profile()).unwrap().all(|e| !e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".restore-")));
    }

    #[test]
    fn restore_keeps_a_sidecar_that_was_part_of_the_snapshot_and_rejects_foreign_ones() {
        let (_dir, env) = fixture();
        let recovery = Recovery { env: &env };
        let id = recovery
            .capture(&env.installed(), &profile_text(&env))
            .unwrap();
        recovery.restore(&id, &env.installed()).unwrap();
        assert!(env.profile().join("settings.schema.json").exists());
        // A sidecar with unexpected content blocks the cleanup instead of being deleted.
        let (_dir2, env2) = fixture();
        let recovery2 = Recovery { env: &env2 };
        fs::remove_file(env2.profile().join("settings.schema.json")).unwrap();
        let id2 = recovery2
            .capture(&env2.installed(), &profile_text(&env2))
            .unwrap();
        fs::write(
            env2.profile().join("settings.schema.json"),
            b"{\"SchemaVersion\":2,\"Extra\":true}",
        )
        .unwrap();
        assert!(recovery2.restore(&id2, &env2.installed()).is_err());
        assert!(env2.profile().join("settings.schema.json").exists());
    }

    #[test]
    fn pre_2_22_snapshots_are_restored_in_place_without_running_their_setup() {
        let (_dir, mut env) = fixture();
        env.installed_version = Version::new(2, 21, 0);
        let recovery = Recovery { env: &env };
        let id = recovery
            .capture(&env.installed(), &profile_text(&env))
            .unwrap();
        fs::write(env.installed(), b"new executable").unwrap();
        recovery.restore(&id, &env.installed()).unwrap();
        assert!(env.installs.borrow().is_empty());
        assert_eq!(fs::read(env.installed()).unwrap(), b"old executable");
        assert_eq!(env.display_version.borrow().as_deref(), Some("2.21.0"));
    }

    #[test]
    fn restore_legacy_binary_rejects_corrupt_bytes_and_keeps_the_target() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("previous.exe");
        let target = dir.path().join("NabyScape.exe");
        fs::write(&source, b"saved executable").unwrap();
        fs::write(&target, b"current executable").unwrap();
        assert!(restore_legacy_binary(&source, &target, &"A".repeat(64)).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"current executable");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
        let hash = hash_file(&source).unwrap();
        restore_legacy_binary(&source, &target, &hash).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"saved executable");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    #[test]
    fn prune_removes_other_snapshots_only() {
        let (_dir, env) = fixture();
        let recovery = Recovery { env: &env };
        let first = recovery
            .capture(&env.installed(), &profile_text(&env))
            .unwrap();
        let second = recovery
            .capture(&env.installed(), &profile_text(&env))
            .unwrap();
        recovery.prune(&second);
        let root = env.updates_folder().join(ROLLBACK_FOLDER);
        assert!(!root.join(&first).exists());
        assert!(root.join(&second).join(MANIFEST_FILE).exists());
        // An unverifiable snapshot is preserved for inspection.
        let third = recovery
            .capture(&env.installed(), &profile_text(&env))
            .unwrap();
        fs::OpenOptions::new()
            .append(true)
            .open(root.join(&second).join(PREVIOUS_EXECUTABLE))
            .unwrap()
            .write_all(b"x")
            .unwrap();
        recovery.prune(&third);
        assert!(root.join(&second).exists());
    }

    #[cfg(unix)]
    #[test]
    fn recovery_folders_must_not_be_links() {
        let (dir, env) = fixture();
        let recovery = Recovery { env: &env };
        let elsewhere = dir.path().join("elsewhere");
        fs::create_dir_all(&elsewhere).unwrap();
        fs::create_dir_all(env.updates_folder()).unwrap();
        std::os::unix::fs::symlink(&elsewhere, env.updates_folder().join(ROLLBACK_FOLDER)).unwrap();
        assert!(recovery
            .capture(&env.installed(), &profile_text(&env))
            .unwrap_err()
            .to_string()
            .contains("links"));
        assert_eq!(fs::read_dir(&elsewhere).unwrap().count(), 0);
    }

    #[test]
    fn schema_sidecar_rules() {
        assert!(schema_sidecar_is_valid("{\"SchemaVersion\":1}"));
        assert!(schema_sidecar_is_valid(
            "{\"SchemaVersion\":1,\"PendingSchemaVersion\":null}"
        ));
        assert!(schema_sidecar_is_valid(
            "{\"SchemaVersion\":1,\"PendingSchemaVersion\":2}"
        ));
        assert!(!schema_sidecar_is_valid("{\"PendingSchemaVersion\":2}"));
        assert!(!schema_sidecar_is_valid("{\"SchemaVersion\":\"1\"}"));
        assert!(!schema_sidecar_is_valid("{\"SchemaVersion\":1,\"x\":1}"));
        assert!(!schema_sidecar_is_valid("[1]"));
        assert!(!schema_sidecar_is_valid("nope"));
    }
}
