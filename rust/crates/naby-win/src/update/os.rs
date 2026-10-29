//! Windows side of the updater: setup inspection (PE + Authenticode + version resource), the
//! real [`RecoveryEnv`], hand-off preparation in the running app, and the helper process
//! (`--update-handoff`) that installs the update and restores the saved copy on failure.

use std::fs::{self, File};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::Arc;
use std::time::{Duration, Instant};

use naby_core::store::Store;
use naby_core::updates::{
    inspect_pe, require_automatic_install_path, verify_file, Platform, Transport, UpdateError,
    Version,
};
use naby_ui::settings::InstallKind;

use super::handoff::{self, failure_action, FailureAction, HandoffData};
use super::recovery::{Recovery, RecoveryEnv};
use super::service::{NotifyFn, ServiceConfig, UpdateService};
use super::{
    failed, hash_file, new_id, now_ticks, updates_folder, HandoffFailure, Result, ERROR_LOG,
    HANDOFF_FILE, HELPER_FILE, SETUP_FILE,
};
use crate::install::{
    current_dir, is_installed, same_path, InstallPaths, EXECUTABLE, REGISTRY_KEY,
};
use crate::winops::{self, reg, NamedEvent};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// C# `OfficialUpdates.InspectSetup`: a Windows executable for this architecture, a valid or
/// absent Authenticode signature, and the version resource of the release.
pub fn inspect_setup(path: &Path, expected: &Version) -> std::result::Result<(), UpdateError> {
    let platform = Platform::current()
        .ok_or_else(|| UpdateError::Invalid("Unsupported application architecture.".into()))?;
    let mut file = File::open(path)?;
    inspect_pe(&mut file, platform.pe_machine, platform.rid)?;
    winops::verify_authenticode_if_present(path)
        .map_err(|e| UpdateError::Invalid(e.to_string()))?;
    let found = winops::file_version(path).map_err(|e| UpdateError::Invalid(e.to_string()))?;
    if found != *expected {
        return Err(UpdateError::Invalid(
            "Setup version does not match the release.".into(),
        ));
    }
    Ok(())
}

/// C# `IsAutomaticInstallPath` / `IsInstalled` for the running executable.
pub fn install_kind() -> InstallKind {
    match std::env::current_exe() {
        Ok(exe) if is_installed(&exe) => {
            if require_automatic_install_path(true, &exe).is_ok() {
                InstallKind::Automatic
            } else {
                InstallKind::Legacy
            }
        }
        _ => InstallKind::Portable,
    }
}

/// The machine behind [`Recovery`].
pub struct WinEnv;

impl RecoveryEnv for WinEnv {
    fn updates_folder(&self) -> PathBuf {
        updates_folder()
    }
    fn local_app_data(&self) -> PathBuf {
        naby_core::paths::local_app_data()
    }
    fn install_dir(&self) -> PathBuf {
        PathBuf::from(current_dir())
    }
    fn legacy_install_dir(&self) -> PathBuf {
        PathBuf::from(InstallPaths::system().legacy_dir())
    }
    fn is_installed(&self, path: &Path) -> bool {
        is_installed(path)
    }
    fn file_version(&self, path: &Path) -> std::result::Result<Version, UpdateError> {
        winops::file_version(path).map_err(|e| UpdateError::Invalid(e.to_string()))
    }
    fn inspect(&self, path: &Path, expected: &Version) -> std::result::Result<(), UpdateError> {
        inspect_setup(path, expected)
    }
    fn run_installer(&self, setup: &Path, recovery: bool) -> Result<()> {
        handoff::run_installer(setup, recovery, &self.install_dir(), handoff::SETUP_TIMEOUT)
    }
    fn set_display_version(&self, version: &str) {
        let _ = reg::set_string(REGISTRY_KEY, "DisplayVersion", version);
    }
}

/// Builds the service for the running app with the real network and inspection.
pub fn new_service(store: Arc<Store>, notify: NotifyFn) -> UpdateService {
    let platform = Platform::current().unwrap_or(Platform::X64);
    let service = UpdateService::new(ServiceConfig {
        store,
        platform,
        current: Version::installed(),
        staging: updates_folder(),
        get: Arc::new(naby_core::updates::default_get),
        inspect: Arc::new(inspect_setup),
        notify,
        retry_delay: naby_core::updates::RETRY_DELAY,
        install_kind: install_kind(),
    });
    refresh_recovery(&service);
    service
}

/// Re-reads the recovery copy (description and availability) into the service.
pub fn refresh_recovery(service: &UpdateService) {
    let recovery = Recovery { env: &WinEnv };
    let has_previous = recovery.previous_id().is_some();
    service.set_recovery(recovery.description(), has_previous);
}

fn ready_event_name(pid: u32) -> String {
    format!("Local\\NabyScape.UpdateReady.{pid}")
}

fn started_event_name(token: &str) -> String {
    format!("Local\\NabyScape.UpdateStarted.{token}")
}

/// C# `UpdateHandoff.Prepare` after the app flushed its documents: validates, writes the
/// hand-off, starts the helper and waits for its confirmation. `Ok` means the caller quits now.
pub fn prepare_handoff(service: &UpdateService, restore: bool) -> Result<()> {
    let installed = std::env::current_exe()?;
    require_automatic_install_path(is_installed(&installed), &installed)?;
    let env = WinEnv;
    let recovery = Recovery { env: &env };
    let profile = recovery.profile_directory(&naby_core::paths::data_dir().to_string_lossy())?;
    let folder = updates_folder();
    let (restore_id, hash, version) = if restore {
        let id = recovery
            .previous_id()
            .ok_or_else(|| HandoffFailure::Failed("No previous version is available.".into()))?;
        recovery.read(&id, true)?;
        (id, String::new(), String::new())
    } else {
        let (Some(verified), Some(release)) = (service.verified(), service.available()) else {
            return failed("Download and verify an update first.");
        };
        let setup = folder.join(SETUP_FILE);
        if !same_path(&setup.to_string_lossy(), &verified.path.to_string_lossy()) {
            return failed("Invalid update staging location.");
        }
        verify_file(&setup, &verified.hash)?;
        inspect_setup(&setup, &release.version)?;
        (String::new(), verified.hash, release.version.to_string())
    };
    fs::create_dir_all(&folder)?;
    let pid = std::process::id();
    let data = HandoffData {
        process_id: i64::from(pid),
        started_utc_ticks: winops::process_start_ticks(pid).ok_or_else(|| {
            HandoffFailure::Failed("Could not read the process start time.".into())
        })?,
        installed_path: installed.to_string_lossy().into_owned(),
        hash,
        version,
        profile_directory: profile.to_string_lossy().into_owned(),
        restore_id,
    };
    fs::write(folder.join(HANDOFF_FILE), serde_json::to_string(&data)?)?;
    let helper = folder.join(HELPER_FILE);
    fs::copy(&installed, &helper)?;
    let ready = NamedEvent::create(&ready_event_name(pid))?;
    Command::new(&helper)
        .arg("--update-handoff")
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|_| HandoffFailure::Failed("Could not start updater helper.".into()))?;
    if !ready.wait(15_000) {
        return failed("Updater did not confirm process identity. Your app remains open.");
    }
    Ok(())
}

/// `--update-started <token>`: tells the helper that this (new) process finished starting.
pub fn signal_started(args: &[String]) {
    let Some(token) = handoff::started_token(args) else {
        return;
    };
    if std::env::current_exe().is_ok_and(|exe| is_installed(&exe)) {
        winops::signal_event(&started_event_name(&token));
    }
}

fn log_error(text: &str) {
    let folder = updates_folder();
    let _ = fs::create_dir_all(&folder);
    let line = format!("{} {text}\n", chrono::Utc::now().to_rfc3339());
    let _ = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(folder.join(ERROR_LOG))
        .and_then(|mut f| std::io::Write::write_all(&mut f, line.as_bytes()));
}

/// C# `Launch`: starts the installed app with a startup acknowledgement token and, when
/// `require_ready`, waits 45 s for it (older versions do not acknowledge).
fn launch(installed: &Path, require_ready: bool) -> Result<()> {
    let token = new_id();
    let ready = NamedEvent::create(&started_event_name(&token))?;
    let mut child: Child = Command::new(installed)
        .args(["--background", "--update-started", &token])
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|_| HandoffFailure::Failed("Could not reopen NabyScape.".into()))?;
    if !require_ready || ready.wait(45_000) {
        return Ok(());
    }
    if child.try_wait().ok().flatten().is_none() {
        let image = crate::devices::process_image(child.id());
        if !image.is_some_and(|i| same_path(&i, &installed.to_string_lossy())) {
            return Err(HandoffFailure::Blocked(
                "The updated process identity changed; automatic recovery stopped.".into(),
            ));
        }
        let _ = child.kill();
        let deadline = Instant::now() + Duration::from_secs(10);
        while child.try_wait().ok().flatten().is_none() {
            if Instant::now() > deadline {
                return Err(HandoffFailure::Blocked(
                    "The updated app could not stop; automatic recovery stopped.".into(),
                ));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    failed("The updated app did not complete startup. Restoring the saved version.")
}

struct HelperState {
    data: Option<HandoffData>,
    snapshot: Option<String>,
    installation_started: bool,
}

fn helper_steps(state: &mut HelperState) -> Result<()> {
    let me = std::env::current_exe()?;
    let folder = updates_folder();
    if !same_path(
        &me.to_string_lossy(),
        &folder.join(HELPER_FILE).to_string_lossy(),
    ) {
        return failed("Invalid updater helper location.");
    }
    let handoff_path = folder.join(HANDOFF_FILE);
    if fs::metadata(&handoff_path)?.len() > 8192 {
        return failed("Invalid update handoff.");
    }
    let data: HandoffData = serde_json::from_str(&fs::read_to_string(&handoff_path)?)
        .map_err(|_| HandoffFailure::Failed("Missing update handoff.".into()))?;
    state.data = Some(data.clone());
    let installed = PathBuf::from(&data.installed_path);
    require_automatic_install_path(is_installed(&installed), &installed)?;
    let restore = data.is_restore();
    let version = if restore {
        None
    } else {
        Some(handoff::validate(
            &data,
            Version::installed(),
            &|p| is_installed(p),
            now_ticks(),
        )?)
    };
    let env = WinEnv;
    let recovery = Recovery { env: &env };
    recovery.profile_directory(&data.profile_directory)?;
    if restore {
        let saved = recovery.read(&data.restore_id, true)?;
        if !saved
            .profile_directory
            .eq_ignore_ascii_case(&data.profile_directory)
        {
            return failed("Recovery profile does not match the running app.");
        }
    }
    let pid = data.process_id as u32;
    let identity = winops::process_start_ticks(pid).zip(crate::devices::process_image(pid));
    let Some((started, image)) = identity else {
        return failed("Update process identity changed.");
    };
    if !handoff::same_process(&data, i64::from(pid), started, &image, &|p| is_installed(p)) {
        return failed("Update process identity changed.");
    }
    winops::signal_event(&ready_event_name(pid));
    let installed_copy = |hash: &str| -> Result<()> {
        verify_file(&installed, hash)?;
        Ok(())
    };
    if !winops::wait_for_exit(pid, 120_000) {
        return failed("NabyScape did not quit. Installation cancelled.");
    }
    if restore {
        let id = recovery.capture(&installed, &data.profile_directory)?;
        state.snapshot = Some(id.clone());
        state.installation_started = true;
        recovery.restore(&data.restore_id, &installed)?;
        launch(&installed, false)?;
        recovery.prune(&id);
    } else {
        let setup = folder.join(SETUP_FILE);
        let version = version.expect("validated for a non-restore hand-off");
        verify_file(&setup, &data.hash)?;
        inspect_setup(&setup, &version)?;
        let id = recovery.capture(&installed, &data.profile_directory)?;
        state.snapshot = Some(id.clone());
        state.installation_started = true;
        handoff::run_installer(&setup, false, &env.install_dir(), handoff::SETUP_TIMEOUT)?;
        installed_copy(&data.hash)?;
        launch(&installed, true)?;
        recovery.prune(&id);
    }
    let _ = fs::remove_file(handoff_path);
    Ok(())
}

/// `--update-handoff`: the helper's main. Returns the process exit code.
pub fn run_helper() -> i32 {
    let mut state = HelperState {
        data: None,
        snapshot: None,
        installation_started: false,
    };
    let Err(error) = helper_steps(&mut state) else {
        return 0;
    };
    log_error(&error.to_string());
    let blocked = matches!(error, HandoffFailure::Blocked(_));
    let Some(data) = state.data.clone() else {
        return 1;
    };
    let installed = PathBuf::from(&data.installed_path);
    let parent_exited = || {
        let pid = data.process_id as u32;
        winops::process_start_ticks(pid).is_none_or(|t| t != data.started_utc_ticks)
            || winops::wait_for_exit(pid, 0)
    };
    let action = failure_action(
        blocked,
        true,
        state.snapshot.is_some(),
        state.installation_started,
        require_automatic_install_path(is_installed(&installed), &installed).is_ok(),
        parent_exited(),
    );
    let recovered: Result<()> = match action {
        FailureAction::Nothing => Ok(()),
        FailureAction::RestoreSnapshot => {
            let env = WinEnv;
            let recovery = Recovery { env: &env };
            state
                .snapshot
                .as_deref()
                .map_or(Ok(()), |id| recovery.restore(id, &installed))
                .and_then(|()| launch(&installed, false))
        }
        FailureAction::Relaunch => launch(&installed, false),
    };
    if let Err(e) = recovered {
        log_error(&format!("Recovery: {e}"));
    }
    1
}

/// The transport used by tests of the real service wiring.
pub fn real_transport() -> Transport<'static> {
    Transport::real()
}

/// The installed executable name, for messages.
pub fn installed_executable() -> PathBuf {
    PathBuf::from(current_dir()).join(EXECUTABLE)
}

/// SHA-256 of the running executable (diagnostics).
pub fn running_hash() -> Option<String> {
    hash_file(&std::env::current_exe().ok()?).ok()
}
