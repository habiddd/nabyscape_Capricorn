//! `--uninstall`: removes NabyScape (C# `tools/Uninstall-NabyScape.ps1`, now native).
//!
//! Plan composition and ownership rules are pure and shared by tests and the Windows flow:
//!
//! * [`data_roots`] names every folder the app owns: the profile (`NabyScape`, `LumaIsland`),
//!   the update staging/rollback folder and interrupted migration / speech-job leftovers, by
//!   exact name only.
//! * [`assert_owned`] refuses any path outside its dedicated root and any link (junction or
//!   symlink) on the way, so a planted junction can never redirect a delete.
//! * [`new_plan`] lists every file and folder first; [`remove_planned`] re-checks the whole plan
//!   before the first deletion and never deletes recursively.
//!
//! The Windows flow (`os`) copies the running program to a temporary file (it cannot delete
//! itself), asks "Also remove my data and downloaded ASR / TTS models", stops owned processes,
//! removes saved API keys, data, startup entry, shortcuts, installed files and the registry
//! entry, and finally removes the temporary copy.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::install::{
    is_link, same_path, EXECUTABLE, LEGACY_EXECUTABLES, LEGACY_SHORTCUTS, RETIRED_HOST_FILES,
    SHORTCUT, UNINSTALL_SHORTCUT,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UninstallError(pub String);

impl fmt::Display for UninstallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for UninstallError {}

impl From<io::Error> for UninstallError {
    fn from(e: io::Error) -> Self {
        Self(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, UninstallError>;

fn fail<T>(message: impl Into<String>) -> Result<T> {
    Err(UninstallError(message.into()))
}

/// Folders of earlier .NET single-file extraction under `%TEMP%\.net` (C# releases only).
const DOTNET_EXTRACTION_FOLDERS: [&str; 8] = [
    "NabyScape",
    "LumaIsland",
    "BillyNabilaIsland",
    "NabyScape-Setup",
    "NabyScape-win-x64-Setup",
    "NabyScape-win-arm64-Setup",
    "NabyScape-UpdateHelper",
    "NabyScape-Previous",
];

/// The command line of `--uninstall`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Options {
    /// `--quiet`: no dialogs, errors go to stderr.
    pub quiet: bool,
    /// `--keep-data`: remove the program but keep personal data and models.
    pub keep_data: bool,
    /// `--remove-data`: the default; accepted for symmetry.
    pub remove_data: bool,
    /// `--uninstall-target <folder>`: set on the relocated copy; the folder being removed.
    pub target: Option<String>,
}

pub fn parse_options(args: &[String]) -> Result<Options> {
    let mut options = Options::default();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--quiet" => options.quiet = true,
            "--keep-data" => options.keep_data = true,
            "--remove-data" => options.remove_data = true,
            "--uninstall-target" => options.target = iter.next().cloned(),
            _ => {}
        }
    }
    if options.keep_data && options.remove_data {
        return fail("Choose either KeepData or RemoveData.");
    }
    Ok(options)
}

impl Options {
    /// Whether the data checkbox starts checked (and, with `--quiet`, whether data is removed).
    pub fn remove_all(&self) -> bool {
        !self.keep_data
    }
}

// ---- ownership rules ---------------------------------------------------------------------------

fn is_hex32(text: &str) -> bool {
    text.len() == 32 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// `NabyScape.migration-<32 lowercase hex>` (an interrupted profile migration).
pub fn is_migration_folder(name: &str) -> bool {
    name.strip_prefix("NabyScape.migration-")
        .is_some_and(is_hex32)
}

/// `NabyScape-speech-<32 lowercase hex>` (a speech job's temp folder).
pub fn is_speech_job_folder(name: &str) -> bool {
    name.strip_prefix("NabyScape-speech-").is_some_and(is_hex32)
}

fn subdirectories(parent: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(parent) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(std::result::Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir() || t.is_symlink()))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort_unstable();
    names
}

/// C# `Get-NabyDataRoots`: every folder whose contents "complete uninstall" may remove.
pub fn data_roots(local: &Path, temp: &Path) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = ["NabyScape", "LumaIsland", "NabyScape-Updates"]
        .iter()
        .map(|name| local.join(name))
        .collect();
    let same_parent = same_path(&local.to_string_lossy(), &temp.to_string_lossy());
    for name in subdirectories(local) {
        if is_migration_folder(&name) || (same_parent && is_speech_job_folder(&name)) {
            roots.push(local.join(name));
        }
    }
    if !same_parent {
        for name in subdirectories(temp) {
            if is_speech_job_folder(&name) {
                roots.push(temp.join(name));
            }
        }
    }
    let dotnet = temp.join(".net");
    roots.extend(
        DOTNET_EXTRACTION_FOLDERS
            .iter()
            .map(|name| dotnet.join(name)),
    );
    let mut unique: Vec<PathBuf> = Vec::new();
    for root in roots {
        if !unique
            .iter()
            .any(|u| same_path(&u.to_string_lossy(), &root.to_string_lossy()))
        {
            unique.push(root);
        }
    }
    unique
}

fn trimmed(path: &Path) -> String {
    path.to_string_lossy()
        .trim_end_matches(['\\', '/'])
        .to_owned()
}

/// C# `Assert-NabyOwnedPath`: `path` is `root` or lies inside it; `root` is not a drive or
/// filesystem root; neither `path` nor any ancestor is a link.
pub fn assert_owned(path: &Path, root: &Path) -> Result<PathBuf> {
    let full = trimmed(path);
    let base = trimmed(root);
    if base.is_empty() || Path::new(&base).parent().is_none() || base.ends_with(':') {
        return fail("Cleanup path is outside its dedicated application folder.");
    }
    let (lower_full, lower_base) = (full.to_ascii_lowercase(), base.to_ascii_lowercase());
    let inside = lower_full == lower_base
        || lower_full
            .strip_prefix(&lower_base)
            .is_some_and(|rest| rest.starts_with(['\\', '/']));
    if !inside {
        return fail("Cleanup path is outside its dedicated application folder.");
    }
    // No junction may redirect a validated root elsewhere: check every ancestor too.
    for cursor in Path::new(&full).ancestors() {
        if cursor.exists() && is_link(cursor) {
            return fail(format!(
                "Cleanup stopped because a linked folder or file was found: {}",
                cursor.display()
            ));
        }
    }
    Ok(PathBuf::from(full))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanEntry {
    pub path: PathBuf,
    pub root: PathBuf,
}

/// Everything a complete uninstall deletes, listed before anything is touched.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    pub files: Vec<PlanEntry>,
    /// Deepest paths first, so children go before parents.
    pub directories: Vec<PlanEntry>,
}

/// C# `New-NabyRemovalPlan`.
pub fn new_plan(roots: &[PathBuf]) -> Result<Plan> {
    let mut plan = Plan::default();
    for root in roots {
        let full = assert_owned(root, root)?;
        if !full.exists() {
            continue;
        }
        if !full.is_dir() {
            return fail(format!(
                "Expected an application folder: {}",
                full.display()
            ));
        }
        let mut pending = vec![full.clone()];
        while let Some(folder) = pending.pop() {
            assert_owned(&folder, &full)?;
            plan.directories.push(PlanEntry {
                path: folder.clone(),
                root: full.clone(),
            });
            for entry in fs::read_dir(&folder)? {
                let entry = entry?;
                let path = assert_owned(&entry.path(), &full)?;
                // `file_type` does not follow links; links were rejected above.
                if entry.file_type()?.is_dir() {
                    pending.push(path);
                } else {
                    plan.files.push(PlanEntry {
                        path,
                        root: full.clone(),
                    });
                }
            }
        }
    }
    plan.directories
        .sort_by_key(|e| std::cmp::Reverse(e.path.as_os_str().len()));
    Ok(plan)
}

/// Deletes a file; a read-only one loses the attribute first (`Remove-Item -Force`).
fn remove_file_forced(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
            let mut permissions = fs::metadata(path)?.permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            permissions.set_readonly(false);
            fs::set_permissions(path, permissions)?;
            fs::remove_file(path)
        }
        other => other,
    }
}

/// C# `Remove-NabyPlannedData`: preflight the whole plan, then delete files, then the folders
/// (non-recursively: a folder that gained content meanwhile is an error, not collateral).
pub fn remove_planned(plan: &Plan) -> Result<()> {
    for entry in plan.files.iter().chain(&plan.directories) {
        assert_owned(&entry.path, &entry.root)?;
    }
    for entry in &plan.files {
        let path = assert_owned(&entry.path, &entry.root)?;
        if path.is_file() {
            remove_file_forced(&path)?;
        }
    }
    for entry in &plan.directories {
        let path = assert_owned(&entry.path, &entry.root)?;
        if path.is_dir() {
            fs::remove_dir(&path)?;
        }
    }
    Ok(())
}

// ---- shortcuts, startup, installed files ----------------------------------------------------

/// Whether a Start-menu shortcut belongs to the installation in `install_dir`: it launches one
/// of its executables, or is the uninstall shortcut of the C# script uninstaller.
pub fn shortcut_owned(name: &str, target: &str, arguments: &str, install_dir: &str) -> bool {
    let owned_executable = std::iter::once(EXECUTABLE)
        .chain(LEGACY_EXECUTABLES)
        .any(|exe| same_path(target, &format!("{install_dir}\\{exe}")));
    let script = format!("\"{install_dir}\\Uninstall.ps1\"");
    let owned_script = name == UNINSTALL_SHORTCUT && arguments.contains(&script);
    owned_executable || owned_script
}

/// The Start-menu shortcut names that may be removed.
pub fn shortcut_names() -> Vec<&'static str> {
    let mut names = vec![SHORTCUT, UNINSTALL_SHORTCUT];
    names.extend(LEGACY_SHORTCUTS);
    names
}

/// A Run value belongs to this installation when its command line names the install folder.
pub fn startup_value_owned(value: &str, install_dir: &str) -> bool {
    value
        .to_ascii_lowercase()
        .contains(&install_dir.to_ascii_lowercase())
}

/// Files removed from the install folder; unrelated files keep the folder alive.
pub fn installed_files() -> Vec<&'static str> {
    let mut files = crate::install::shipped_files();
    files.extend(RETIRED_HOST_FILES);
    files.sort_unstable();
    files.dedup();
    files
}

/// Removes the shipped files of `folder` (each ownership-checked) and the folder when empty.
pub fn remove_installed_files(folder: &Path) -> Result<()> {
    assert_owned(folder, folder)?;
    for name in installed_files() {
        let path = assert_owned(&folder.join(name), folder)?;
        if path.is_file() {
            remove_file_forced(&path)?;
        }
    }
    if folder.is_dir() && fs::read_dir(folder)?.next().is_none() {
        fs::remove_dir(folder)?;
    }
    Ok(())
}

/// Saved API keys: `NabyScape/Assistant/<32 hex>` generic credentials only.
pub fn credential_target_owned(target: &str) -> bool {
    target
        .strip_prefix("NabyScape/Assistant/")
        .is_some_and(|id| crate::credentials::target_name(id).is_ok())
}

/// The command line the first stage uses to hand over to the temporary copy.
pub fn relocation_arguments(options: &Options, install_dir: &str) -> Vec<String> {
    let mut args = vec!["--uninstall".to_owned()];
    if options.quiet {
        args.push("--quiet".to_owned());
    }
    if options.keep_data {
        args.push("--keep-data".to_owned());
    }
    if options.remove_data {
        args.push("--remove-data".to_owned());
    }
    args.push("--uninstall-target".to_owned());
    args.push(install_dir.to_owned());
    args
}

#[cfg(windows)]
pub use os::run;

#[cfg(windows)]
mod os {
    use super::*;
    use crate::install::{InstallPaths, REGISTRY_KEY};
    use crate::winops::{self, reg};
    use std::process::Command;

    const TITLE: &str = "NabyScape";

    fn local_app_data() -> PathBuf {
        naby_core::paths::local_app_data()
    }

    /// Entry point of `--uninstall`; returns the exit code.
    pub fn run(args: &[String]) -> i32 {
        let options = match parse_options(args) {
            Ok(options) => options,
            Err(error) => return report(&Options::default(), &error),
        };
        match flow(&options) {
            Ok(()) => 0,
            Err(error) => report(&options, &error),
        }
    }

    fn report(options: &Options, error: &UninstallError) -> i32 {
        if options.quiet {
            eprintln!("{error}");
        } else {
            winops::message(
                TITLE,
                &format!("Uninstall could not finish. {error}"),
                winops::Icon::Error,
            );
        }
        1
    }

    fn flow(options: &Options) -> Result<()> {
        let registered = reg::get_string(REGISTRY_KEY, "InstallLocation")
            .ok_or_else(|| UninstallError("NabyScape is not installed for this user.".into()))?;
        let target = InstallPaths::system()
            .validate(&registered)
            .map_err(|e| UninstallError(e.to_string()))?;
        let me = std::env::current_exe()?;
        match &options.target {
            None => {
                // First stage: the running program lives in the folder being removed.
                let inside = me
                    .parent()
                    .is_some_and(|p| same_path(&p.to_string_lossy(), &target));
                if !inside {
                    return fail("Invalid uninstaller location.");
                }
                relocate(&me, options, &target)
            }
            Some(requested) => {
                if !same_path(requested, &target) {
                    return fail("Invalid uninstaller location.");
                }
                uninstall(options, &me, &target)
            }
        }
    }

    /// Copies this program to the temp folder and continues there.
    fn relocate(me: &Path, options: &Options, target: &str) -> Result<()> {
        let copy = std::env::temp_dir().join(format!(
            "NabyScape-Uninstall-{}.exe",
            crate::update::new_id()
        ));
        fs::copy(me, &copy)?;
        Command::new(&copy)
            .args(relocation_arguments(options, target))
            .spawn()
            .map_err(|e| UninstallError(format!("Could not start the uninstaller: {e}")))?;
        Ok(())
    }

    fn uninstall(options: &Options, me: &Path, target: &str) -> Result<()> {
        let _com = crate::svc_util::ComGuard::sta();
        let install_dir = PathBuf::from(target);
        assert_owned(&install_dir, &install_dir)?;
        let mut remove_all = options.remove_all();
        if !options.quiet {
            match winops::confirm_uninstall(remove_all) {
                Some(choice) => remove_all = choice,
                None => {
                    cleanup_copy(me);
                    return Ok(());
                }
            }
        }
        let local = local_app_data();
        let temp = std::env::temp_dir();
        let roots = data_roots(&local, &temp);
        // Validate every root before stopping anything.
        for root in &roots {
            assert_owned(root, root)?;
        }
        stop_processes(&install_dir, &roots, me)?;
        if remove_all {
            let plan = new_plan(&roots)?;
            winops::delete_saved_keys(credential_target_owned)?;
            remove_planned(&plan)?;
        }
        remove_startup_entries(target);
        remove_shortcuts(target);
        remove_installed_files(&install_dir)?;
        reg::delete_tree(REGISTRY_KEY)?;
        if !options.quiet {
            winops::message(TITLE, "NabyScape has been uninstalled.", winops::Icon::Info);
        }
        cleanup_copy(me);
        Ok(())
    }

    /// Asks the running app to quit, then stops whatever still runs from the install folder or a
    /// data root.
    fn stop_processes(install_dir: &Path, roots: &[PathBuf], me: &Path) -> Result<()> {
        let exe = install_dir.join(EXECUTABLE);
        if exe.is_file() {
            let _ = Command::new(&exe)
                .arg("--exit")
                .spawn()
                .and_then(|mut c| c.wait());
        }
        std::thread::sleep(std::time::Duration::from_millis(800));
        let mine = std::process::id();
        let mut prefixes: Vec<String> = vec![format!("{}\\", trimmed(install_dir))];
        prefixes.extend(roots.iter().map(|r| format!("{}\\", trimmed(r))));
        for process in winops::processes() {
            if process.pid == mine {
                continue;
            }
            let Some(image) = crate::devices::process_image(process.pid) else {
                continue;
            };
            if same_path(&image, &me.to_string_lossy()) {
                continue;
            }
            let lower = image.to_ascii_lowercase();
            if prefixes
                .iter()
                .any(|p| lower.starts_with(&p.to_ascii_lowercase()))
            {
                winops::terminate_and_wait(process.pid, 10_000).map_err(|_| {
                    UninstallError(
                        "An application process did not stop. Retry after closing NabyScape."
                            .into(),
                    )
                })?;
            }
        }
        Ok(())
    }

    fn remove_startup_entries(target: &str) {
        use crate::startup::{LEGACY_VALUE_NAMES, RUN_KEY, VALUE_NAME};
        for name in std::iter::once(VALUE_NAME).chain(LEGACY_VALUE_NAMES) {
            if reg::get_string(RUN_KEY, name).is_some_and(|v| startup_value_owned(&v, target)) {
                let _ = reg::delete_value(RUN_KEY, name);
            }
        }
    }

    fn remove_shortcuts(target: &str) {
        let Some(programs) = winops::programs_folder() else {
            return;
        };
        for name in shortcut_names() {
            let link = programs.join(name);
            if !link.is_file() {
                continue;
            }
            if let Some(info) = winops::read_shortcut(&link) {
                if shortcut_owned(name, &info.target, &info.arguments, target) {
                    let _ = fs::remove_file(&link);
                }
            }
        }
    }

    /// Deletes the temporary copy after this process exits.
    fn cleanup_copy(me: &Path) {
        let in_temp = me
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with("NabyScape-Uninstall-"));
        if !in_temp {
            return;
        }
        let _ = Command::new("cmd.exe")
            .args(["/c", "ping -n 3 127.0.0.1 >nul & del /f /q"])
            .arg(me)
            .creation_flags_hidden()
            .spawn();
    }

    trait HiddenCommand {
        fn creation_flags_hidden(&mut self) -> &mut Self;
    }
    impl HiddenCommand for Command {
        fn creation_flags_hidden(&mut self) -> &mut Self {
            use std::os::windows::process::CommandExt;
            self.creation_flags(0x0800_0000)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    #[test]
    fn complete_cleanup_removes_app_owned_profiles_models_and_updates_but_not_imported_files() {
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join("Local");
        let temp = local.join("Temp");
        let outside = dir.path().join("PersonalModels");
        fs::create_dir_all(&temp).unwrap();
        fs::create_dir_all(&outside).unwrap();
        let sentinel = outside.join("imported-model.onnx");
        fs::write(&sentinel, "personal model").unwrap();
        let roots = data_roots(&local, &temp);
        for folder in [
            "NabyScape/local-speech/models",
            "NabyScape/native-runtime/fixture-hash",
            "NabyScape/native-host-spool",
            "LumaIsland/local-speech/voices",
            "NabyScape-Updates/rollback",
        ] {
            write(&local.join(folder).join("owned.bin"), "downloaded by app");
        }
        let plan = new_plan(&roots).unwrap();
        assert_eq!(
            plan.files.len(),
            5,
            "managed model, runtime cache and update roots are included"
        );
        remove_planned(&plan).unwrap();
        for name in ["NabyScape", "LumaIsland", "NabyScape-Updates"] {
            assert!(!local.join(name).exists(), "{name} remains");
        }
        assert_eq!(
            fs::read_to_string(&sentinel).unwrap().trim(),
            "personal model"
        );
    }

    #[test]
    fn paths_outside_the_dedicated_root_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("Local").join("NabyScape");
        fs::create_dir_all(&root).unwrap();
        let outside = dir
            .path()
            .join("PersonalModels")
            .join("imported-model.onnx");
        write(&outside, "x");
        assert!(assert_owned(&outside, &root).is_err());
        // A sibling that only shares the prefix is outside too.
        let sibling = dir.path().join("Local").join("NabyScape-other").join("a");
        assert!(assert_owned(&sibling, &root).is_err());
        assert!(assert_owned(&root.join("settings.json"), &root).is_ok());
        assert!(assert_owned(&root, &root).is_ok());
        // A filesystem root is never a valid cleanup root.
        assert!(assert_owned(Path::new("/"), Path::new("/")).is_err());
        assert!(assert_owned(Path::new("/tmp"), Path::new("")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn links_are_rejected_before_any_model_or_settings_deletion() {
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join("Local");
        let outside = dir.path().join("PersonalModels");
        fs::create_dir_all(&outside).unwrap();
        let sentinel = outside.join("imported-model.onnx");
        fs::write(&sentinel, "personal model").unwrap();
        let app = local.join("NabyScape");
        let preserved = app.join("settings.json");
        write(&preserved, "keep until preflight passes");
        std::os::unix::fs::symlink(&outside, app.join("external-model-link")).unwrap();
        assert!(new_plan(std::slice::from_ref(&app)).is_err());
        assert!(sentinel.exists() && preserved.exists());
        // A link planted after planning is caught by the preflight, before the first deletion.
        fs::remove_file(app.join("external-model-link")).unwrap();
        let plan = new_plan(std::slice::from_ref(&app)).unwrap();
        std::os::unix::fs::symlink(&outside, app.join("late-link")).unwrap();
        let late = Plan {
            files: plan.files.clone(),
            directories: vec![PlanEntry {
                path: app.join("late-link"),
                root: app.clone(),
            }],
        };
        assert!(remove_planned(&late).is_err());
        assert!(preserved.exists() && sentinel.exists());
        fs::remove_file(app.join("late-link")).unwrap();
        remove_planned(&new_plan(std::slice::from_ref(&app)).unwrap()).unwrap();
        assert!(!app.exists());
        assert!(sentinel.exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_linked_ancestor_of_a_root_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        fs::create_dir_all(real.join("NabyScape")).unwrap();
        let via = dir.path().join("via");
        std::os::unix::fs::symlink(&real, &via).unwrap();
        let root = via.join("NabyScape");
        assert!(assert_owned(&root, &root).is_err());
        assert!(new_plan(&[root]).is_err());
    }

    #[test]
    fn only_exact_migration_and_speech_job_names_are_selected() {
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join("Local");
        let temp = local.join("Temp");
        let hex = "0123456789abcdef0123456789abcdef";
        let migration = local.join(format!("NabyScape.migration-{hex}"));
        let speech = temp.join(format!("NabyScape-speech-{hex}"));
        let other = temp.join("NabyScape-unrelated-folder");
        let upper = temp.join("NabyScape-speech-0123456789ABCDEF0123456789ABCDEF");
        let short = local.join("NabyScape.migration-abc");
        let wrong_parent = local.join(format!("NabyScape-speech-{hex}"));
        for folder in [&migration, &speech, &other, &upper, &short, &wrong_parent] {
            fs::create_dir_all(folder).unwrap();
        }
        let roots = data_roots(&local, &temp);
        assert!(roots.contains(&migration) && roots.contains(&speech));
        for excluded in [&other, &upper, &short, &wrong_parent] {
            assert!(!roots.contains(excluded), "{}", excluded.display());
        }
        // Fixed roots are always listed, once.
        for name in ["NabyScape", "LumaIsland", "NabyScape-Updates"] {
            assert_eq!(roots.iter().filter(|r| **r == local.join(name)).count(), 1);
        }
        assert!(roots.contains(&temp.join(".net").join("NabyScape-UpdateHelper")));
    }

    #[test]
    fn name_matchers() {
        let hex = "f".repeat(32);
        assert!(is_migration_folder(&format!("NabyScape.migration-{hex}")));
        assert!(!is_migration_folder("NabyScape.migration-"));
        assert!(!is_migration_folder(&format!("nabyscape.migration-{hex}")));
        assert!(is_speech_job_folder(&format!("NabyScape-speech-{hex}")));
        assert!(!is_speech_job_folder(&format!("NabyScape-speech-{hex}x")));
    }

    #[test]
    fn a_folder_that_gained_content_after_planning_is_an_error_not_collateral() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("NabyScape");
        write(&root.join("a.json"), "1");
        let plan = new_plan(std::slice::from_ref(&root)).unwrap();
        write(&root.join("new.json"), "2");
        assert!(remove_planned(&plan).is_err());
        assert!(root.join("new.json").exists());
    }

    #[test]
    fn missing_roots_are_skipped_and_a_file_root_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("NabyScape");
        assert_eq!(
            new_plan(std::slice::from_ref(&missing)).unwrap(),
            Plan::default()
        );
        let file = dir.path().join("LumaIsland");
        fs::write(&file, "x").unwrap();
        assert!(new_plan(&[file]).is_err());
    }

    #[test]
    fn plans_remove_deepest_folders_first() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("NabyScape");
        write(&root.join("a").join("b").join("c").join("f.bin"), "x");
        let plan = new_plan(std::slice::from_ref(&root)).unwrap();
        assert_eq!(
            plan.directories.first().unwrap().path,
            root.join("a").join("b").join("c")
        );
        assert_eq!(plan.directories.last().unwrap().path, root);
        remove_planned(&plan).unwrap();
        assert!(!root.exists());
    }

    #[test]
    fn options_follow_the_script_switches() {
        let o = parse_options(&args(&["--uninstall"])).unwrap();
        assert!(!o.quiet && o.remove_all(), "data removal is the default");
        let o = parse_options(&args(&["--uninstall", "--quiet", "--keep-data"])).unwrap();
        assert!(o.quiet && !o.remove_all());
        let o = parse_options(&args(&[
            "--uninstall",
            "--remove-data",
            "--uninstall-target",
            "D:\\Apps\\NabyScape",
        ]))
        .unwrap();
        assert!(o.remove_all());
        assert_eq!(o.target.as_deref(), Some("D:\\Apps\\NabyScape"));
        assert_eq!(
            parse_options(&args(&["--keep-data", "--remove-data"]))
                .unwrap_err()
                .0,
            "Choose either KeepData or RemoveData."
        );
    }

    #[test]
    fn relocation_preserves_the_choices() {
        let options = Options {
            quiet: true,
            keep_data: true,
            ..Options::default()
        };
        assert_eq!(
            relocation_arguments(&options, "D:\\Apps\\NabyScape"),
            [
                "--uninstall",
                "--quiet",
                "--keep-data",
                "--uninstall-target",
                "D:\\Apps\\NabyScape"
            ]
        );
        let reparsed = parse_options(&relocation_arguments(&options, "D:\\x")).unwrap();
        assert_eq!(reparsed.quiet, options.quiet);
        assert_eq!(reparsed.keep_data, options.keep_data);
    }

    #[test]
    fn shortcuts_are_removed_only_when_they_point_into_this_installation() {
        let dir = "D:\\Apps\\NabyScape";
        assert!(shortcut_owned(
            "NabyScape.lnk",
            "D:\\Apps\\NabyScape\\NabyScape.exe",
            "",
            dir
        ));
        assert!(shortcut_owned(
            "NabyScape.lnk",
            "d:\\apps\\nabyscape\\nabyscape.exe",
            "",
            dir
        ));
        assert!(shortcut_owned(
            "Luma Island.lnk",
            "D:\\Apps\\NabyScape\\LumaIsland.exe",
            "",
            dir
        ));
        assert!(shortcut_owned(
            "Uninstall NabyScape.lnk",
            "D:\\Apps\\NabyScape\\NabyScape.exe",
            "--uninstall",
            dir
        ));
        assert!(shortcut_owned(
            "Uninstall NabyScape.lnk",
            "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe",
            "-NoProfile -File \"D:\\Apps\\NabyScape\\Uninstall.ps1\"",
            dir
        ));
        // Another installation's or an unrelated shortcut stays.
        assert!(!shortcut_owned(
            "NabyScape.lnk",
            "E:\\Other\\NabyScape\\NabyScape.exe",
            "",
            dir
        ));
        assert!(!shortcut_owned(
            "NabyScape.lnk",
            "D:\\Apps\\NabyScape\\other.exe",
            "",
            dir
        ));
        assert!(!shortcut_owned(
            "Some.lnk",
            "powershell.exe",
            "-File \"D:\\Apps\\NabyScape\\Uninstall.ps1\"",
            dir
        ));
        assert_eq!(
            shortcut_names(),
            [
                "NabyScape.lnk",
                "Uninstall NabyScape.lnk",
                "Luma Island.lnk",
                "BillyNabila Island.lnk"
            ]
        );
    }

    #[test]
    fn startup_values_are_removed_only_for_this_folder() {
        let dir = "D:\\Apps\\NabyScape";
        assert!(startup_value_owned(
            "\"D:\\Apps\\NabyScape\\NabyScape.exe\" --background",
            dir
        ));
        assert!(startup_value_owned(
            "\"d:\\apps\\nabyscape\\NabyScape.exe\" --background",
            dir
        ));
        assert!(!startup_value_owned(
            "\"E:\\Other\\NabyScape.exe\" --background",
            dir
        ));
    }

    #[test]
    fn credentials_are_matched_by_exact_target_shape() {
        let id = "0123456789abcdefABCDEF0123456789";
        assert!(credential_target_owned(&format!(
            "NabyScape/Assistant/{id}"
        )));
        assert!(!credential_target_owned("NabyScape/Assistant/short"));
        assert!(!credential_target_owned(&format!("Other/Assistant/{id}")));
        assert!(!credential_target_owned(&format!(
            "NabyScape/Assistant/{id}/x"
        )));
    }

    #[test]
    fn installed_file_removal_keeps_unrelated_files_and_removes_an_emptied_folder() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("NabyScape");
        for name in [
            "NabyScape.exe",
            "needle.exe",
            "whistle.cact",
            "LICENSE",
            "Uninstall.ps1",
            "NabyScape.UI.exe",
        ] {
            write(&folder.join(name), "x");
        }
        write(&folder.join("mine.txt"), "keep");
        remove_installed_files(&folder).unwrap();
        let left: Vec<_> = fs::read_dir(&folder)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(left, ["mine.txt"]);
        fs::remove_file(folder.join("mine.txt")).unwrap();
        write(&folder.join("NabyScape.exe"), "x");
        remove_installed_files(&folder).unwrap();
        assert!(!folder.exists());
    }

    #[cfg(unix)]
    #[test]
    fn read_only_files_are_still_removed() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("NabyScape");
        let file = root.join("locked.json");
        write(&file, "x");
        fs::set_permissions(&file, fs::Permissions::from_mode(0o444)).unwrap();
        // Unix removal depends on the directory, so this only checks the path stays correct.
        remove_planned(&new_plan(std::slice::from_ref(&root)).unwrap()).unwrap();
        assert!(!root.exists());
    }
}
