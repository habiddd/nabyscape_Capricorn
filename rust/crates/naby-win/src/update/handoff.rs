//! The quit-install-restart hand-off (C# `UpdateHandoff.cs`).
//!
//! The running app writes `handoff.json`, copies itself to `NabyScape-UpdateHelper.exe`, starts
//! that copy with `--update-handoff` and quits once the helper confirmed it saw the right
//! process (`Local\NabyScape.UpdateReady.<pid>`). The helper waits for the app to exit, saves a
//! recovery copy, runs the staged setup silently, checks the installed hash and starts the new
//! app with `--update-started <token>`; if anything fails it restores the recovery copy.
//!
//! This file holds the pure rules; the processes and kernel events are in `update::os`.

use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use naby_core::updates::{require_automatic_install_path, Version};
use serde::{Deserialize, Serialize};

use super::{failed, valid_id, HandoffFailure, Result};

/// How long setup may run before it is killed (C# `WaitForExit(120000)`).
pub const SETUP_TIMEOUT: Duration = Duration::from_secs(120);
/// How long a killed setup gets to disappear before recovery is blocked.
pub const KILL_TIMEOUT: Duration = Duration::from_secs(10);

/// `handoff.json` (same keys as C# `UpdateHandoffData`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct HandoffData {
    pub process_id: i64,
    /// Process start time in .NET ticks (UTC).
    pub started_utc_ticks: i64,
    pub installed_path: String,
    pub hash: String,
    pub version: String,
    pub profile_directory: String,
    pub restore_id: String,
}

impl HandoffData {
    /// A restore hand-off carries a recovery id and no setup.
    pub fn is_restore(&self) -> bool {
        !self.restore_id.is_empty()
    }
}

static PREPARING: AtomicBool = AtomicBool::new(false);

/// C# `TryBegin`: only one hand-off can be prepared at a time.
pub fn try_begin() -> bool {
    PREPARING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
}

/// C# `End`.
pub fn end() {
    PREPARING.store(false, Ordering::Release);
}

/// C# `Validate`: the hand-off must describe an installed copy, a plausible running process
/// and a newer stable version with a SHA-256 hash.
pub fn validate(
    data: &HandoffData,
    installed_version: Version,
    is_installed: &dyn Fn(&Path) -> bool,
    now_ticks: i64,
) -> Result<Version> {
    let path = Path::new(&data.installed_path);
    require_automatic_install_path(is_installed(path), path)?;
    let hash_ok = data.hash.len() == 64
        && data
            .hash
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'A'..=b'F'));
    let version: Option<Version> = data.version.parse().ok();
    match version {
        Some(version)
            if is_installed(path)
                && data.process_id > 0
                && data.started_utc_ticks > 0
                && data.started_utc_ticks <= now_ticks + 600_000_000
                && hash_ok
                && version > installed_version =>
        {
            Ok(version)
        }
        _ => failed("Invalid update handoff."),
    }
}

/// C# `SameProcess`: the process the helper sees is the one that wrote the hand-off.
pub fn same_process(
    data: &HandoffData,
    id: i64,
    started: i64,
    path: &str,
    is_installed: &dyn Fn(&Path) -> bool,
) -> bool {
    data.process_id == id
        && data.started_utc_ticks == started
        && data.installed_path.eq_ignore_ascii_case(path)
        && is_installed(Path::new(path))
}

/// C# `Complete`: wait for the app to quit, verify, then install; any failure aborts the rest.
pub fn complete(
    wait: impl FnOnce() -> bool,
    verify: impl FnOnce() -> Result<()>,
    install: impl FnOnce() -> i32,
) -> Result<()> {
    if !wait() {
        return failed("NabyScape did not quit. Installation cancelled.");
    }
    verify()?;
    match install() {
        0 => Ok(()),
        code => Err(HandoffFailure::Failed(format!(
            "Setup failed with exit code {code}."
        ))),
    }
}

/// C# `Run` catch block: what the helper does after a failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureAction {
    /// Leave everything as it is (recovery was blocked, or the app cannot be started safely).
    Nothing,
    /// Install the recovery copy again and start it.
    RestoreSnapshot,
    /// Start the existing installed app (the original process has exited).
    Relaunch,
}

pub fn failure_action(
    blocked: bool,
    has_data: bool,
    snapshot_taken: bool,
    installation_started: bool,
    automatic_path: bool,
    parent_exited: bool,
) -> FailureAction {
    if blocked || !has_data {
        FailureAction::Nothing
    } else if snapshot_taken && installation_started {
        FailureAction::RestoreSnapshot
    } else if automatic_path && parent_exited {
        FailureAction::Relaunch
    } else {
        FailureAction::Nothing
    }
}

/// The token after `--update-started`, when it is a valid id.
pub fn started_token(args: &[String]) -> Option<String> {
    let index = args.iter().position(|a| a == "--update-started")?;
    args.get(index + 1).filter(|id| valid_id(id)).cloned()
}

/// Arguments of the silent setup the helper runs (`--install` only for a saved executable).
pub fn installer_arguments(recovery: bool, install_dir: &str) -> Vec<String> {
    let mut args = Vec::new();
    if recovery {
        args.push("--install".to_owned());
    }
    args.push("--silent".to_owned());
    args.push("--install-dir".to_owned());
    args.push(install_dir.to_owned());
    args
}

/// C# `RunInstaller`: runs `path` silently and waits up to `timeout`. A setup that does not
/// finish is killed; when it cannot be confirmed stopped, recovery is `Blocked` so no later
/// step mutates files under a setup that might still run.
pub fn run_installer(
    path: &Path,
    recovery: bool,
    install_dir: &Path,
    timeout: Duration,
) -> Result<()> {
    let mut command = Command::new(path);
    command
        .args(installer_arguments(
            recovery,
            &install_dir.to_string_lossy(),
        ))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command
        .spawn()
        .map_err(|_| HandoffFailure::Failed("Could not start setup.".into()))?;
    let deadline = Instant::now() + timeout;
    let failure = loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => {
                break HandoffFailure::Failed(format!(
                    "Setup failed with exit code {}.",
                    status.code().unwrap_or(-1)
                ));
            }
            Ok(None) if Instant::now() >= deadline => {
                break HandoffFailure::Failed(
                    "Setup timed out. Recovery files are preserved.".into(),
                );
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => break HandoffFailure::Failed("Setup state could not be read.".into()),
        }
    };
    // Dropping a child does not stop it: establish that it exited before any fallback runs.
    if matches!(child.try_wait(), Ok(Some(_))) {
        return Err(failure);
    }
    let _ = child.kill();
    let kill_deadline = Instant::now() + KILL_TIMEOUT;
    while Instant::now() < kill_deadline {
        if matches!(child.try_wait(), Ok(Some(_))) {
            return Err(failure);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Err(HandoffFailure::Blocked(
        "Setup could not be confirmed stopped. Automatic recovery is blocked; recovery files are preserved. Close setup before using the official installer manually.".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::update::now_ticks;

    const INSTALLED: &str = "/apps/NabyScape/NabyScape.exe";

    fn installed(path: &Path) -> bool {
        path.starts_with("/apps/NabyScape")
    }

    fn data() -> HandoffData {
        HandoffData {
            process_id: 42,
            started_utc_ticks: now_ticks(),
            installed_path: INSTALLED.into(),
            hash: "A".repeat(64),
            version: "99.0.0".into(),
            ..HandoffData::default()
        }
    }

    #[test]
    fn handoff_json_uses_the_c_sharp_keys() {
        let json = serde_json::to_value(data()).unwrap();
        for key in [
            "ProcessId",
            "StartedUtcTicks",
            "InstalledPath",
            "Hash",
            "Version",
            "ProfileDirectory",
            "RestoreId",
        ] {
            assert!(json.get(key).is_some(), "{key}");
        }
        let parsed: HandoffData = serde_json::from_str("{\"ProcessId\":7,\"Unknown\":1}").unwrap();
        assert_eq!(parsed.process_id, 7);
        assert!(!parsed.is_restore());
    }

    #[test]
    fn a_valid_handoff_returns_its_version() {
        assert_eq!(
            validate(&data(), Version::new(3, 0, 0), &installed, now_ticks()).unwrap(),
            Version::new(99, 0, 0)
        );
    }

    #[test]
    fn invalid_handoffs_are_rejected() {
        let now = now_ticks();
        let current = Version::new(3, 0, 0);
        for version in ["2.4.2", "3.0.0", "1.0.0", "99.0.0-beta", ""] {
            let mut d = data();
            d.version = version.into();
            assert!(
                validate(&d, current, &installed, now).is_err(),
                "version {version:?}"
            );
        }
        let mut other = data();
        other.installed_path = "/tmp/other.exe".into();
        assert!(
            validate(&other, current, &installed, now).is_err(),
            "arbitrary path"
        );
        let mut legacy_name = data();
        legacy_name.installed_path = "/apps/NabyScape/LumaIsland.exe".into();
        assert!(validate(&legacy_name, current, &installed, now).is_err());
        for hash in ["", "abc", &"a".repeat(64), &"G".repeat(64), &"A".repeat(65)] {
            let mut d = data();
            d.hash = hash.to_string();
            assert!(
                validate(&d, current, &installed, now).is_err(),
                "hash {hash:?}"
            );
        }
        let mut no_pid = data();
        no_pid.process_id = 0;
        assert!(validate(&no_pid, current, &installed, now).is_err());
        let mut future = data();
        future.started_utc_ticks = now + 20 * 600_000_000;
        assert!(validate(&future, current, &installed, now).is_err());
        let mut no_start = data();
        no_start.started_utc_ticks = 0;
        assert!(validate(&no_start, current, &installed, now).is_err());
    }

    #[test]
    fn process_identity_must_match_exactly() {
        let d = HandoffData {
            process_id: 42,
            started_utc_ticks: 123,
            installed_path: INSTALLED.into(),
            ..HandoffData::default()
        };
        assert!(same_process(&d, 42, 123, INSTALLED, &installed));
        assert!(!same_process(&d, 43, 123, INSTALLED, &installed));
        assert!(!same_process(&d, 42, 124, INSTALLED, &installed));
        assert!(!same_process(&d, 42, 123, "/tmp/NabyScape.exe", &installed));
    }

    #[test]
    fn only_one_handoff_is_prepared_at_a_time() {
        // Single test touching the global gate.
        assert!(try_begin());
        assert!(!try_begin());
        end();
        assert!(try_begin());
        end();
    }

    #[test]
    fn complete_runs_wait_verify_install_in_order_and_stops_on_failure() {
        use std::cell::RefCell;
        let steps = RefCell::new(Vec::new());
        complete(
            || {
                steps.borrow_mut().push(0);
                true
            },
            || {
                steps.borrow_mut().push(1);
                Ok(())
            },
            || {
                steps.borrow_mut().push(2);
                0
            },
        )
        .unwrap();
        assert_eq!(*steps.borrow(), [0, 1, 2]);
        let timeout = complete(
            || false,
            || panic!("must not verify"),
            || panic!("must not install"),
        );
        assert!(timeout.is_err());
        let failed = complete(|| true, || Ok(()), || 7).unwrap_err();
        assert!(failed.to_string().contains('7'));
        let unverified = complete(|| true, failed_verify, || panic!("must not install"));
        assert!(unverified.is_err());
    }

    fn failed_verify() -> Result<()> {
        failed("Setup checksum mismatch.")
    }

    #[test]
    fn recovery_decision_matches_the_helper_catch_block() {
        use FailureAction::*;
        // blocked recovery never touches files; a missing hand-off cannot be recovered
        assert_eq!(failure_action(true, true, true, true, true, true), Nothing);
        assert_eq!(
            failure_action(false, false, true, true, true, true),
            Nothing
        );
        // setup started after a snapshot: restore it
        assert_eq!(
            failure_action(false, true, true, true, true, false),
            RestoreSnapshot
        );
        // failed before installing: just reopen the old app, but only after the parent exited
        assert_eq!(
            failure_action(false, true, false, false, true, true),
            Relaunch
        );
        assert_eq!(
            failure_action(false, true, true, false, true, true),
            Relaunch
        );
        assert_eq!(
            failure_action(false, true, false, false, true, false),
            Nothing
        );
        assert_eq!(
            failure_action(false, true, false, false, false, true),
            Nothing
        );
    }

    #[test]
    fn started_token_requires_a_valid_id() {
        let args = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let id = "0123456789abcdef0123456789abcdef";
        assert_eq!(
            started_token(&args(&["--background", "--update-started", id])).as_deref(),
            Some(id)
        );
        assert_eq!(started_token(&args(&["--update-started"])), None);
        assert_eq!(started_token(&args(&["--update-started", "nope"])), None);
        assert_eq!(started_token(&args(&["--background"])), None);
    }

    #[test]
    fn installer_arguments_add_install_only_for_recovery() {
        assert_eq!(
            installer_arguments(false, "D:\\Apps\\NabyScape"),
            ["--silent", "--install-dir", "D:\\Apps\\NabyScape"]
        );
        assert_eq!(installer_arguments(true, "x")[0], "--install");
    }

    #[cfg(unix)]
    mod processes {
        use super::*;
        use std::os::unix::fs::PermissionsExt;

        fn script(dir: &Path, name: &str, body: &str) -> std::path::PathBuf {
            let path = dir.join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path
        }

        #[test]
        fn a_setup_exit_code_decides_success() {
            let dir = tempfile::tempdir().unwrap();
            let ok = script(dir.path(), "ok.sh", "exit 0");
            let bad = script(dir.path(), "bad.sh", "exit 3");
            run_installer(&ok, false, dir.path(), Duration::from_secs(5)).unwrap();
            let error = run_installer(&bad, true, dir.path(), Duration::from_secs(5)).unwrap_err();
            assert_eq!(
                error,
                HandoffFailure::Failed("Setup failed with exit code 3.".into())
            );
            assert!(run_installer(
                &dir.path().join("missing"),
                false,
                dir.path(),
                Duration::from_secs(1)
            )
            .is_err());
        }

        #[test]
        fn the_setup_receives_the_silent_install_arguments() {
            let dir = tempfile::tempdir().unwrap();
            let out = dir.path().join("args.txt");
            let recorder = script(
                dir.path(),
                "rec.sh",
                &format!("echo \"$@\" > '{}'", out.display()),
            );
            run_installer(
                &recorder,
                true,
                Path::new("/apps/NabyScape"),
                Duration::from_secs(5),
            )
            .unwrap();
            assert_eq!(
                std::fs::read_to_string(out).unwrap().trim(),
                "--install --silent --install-dir /apps/NabyScape"
            );
        }

        #[test]
        fn a_setup_that_hangs_is_killed_and_reported_as_timed_out() {
            let dir = tempfile::tempdir().unwrap();
            let slow = script(dir.path(), "slow.sh", "exec sleep 30");
            let started = Instant::now();
            let error =
                run_installer(&slow, false, dir.path(), Duration::from_millis(200)).unwrap_err();
            assert_eq!(
                error,
                HandoffFailure::Failed("Setup timed out. Recovery files are preserved.".into())
            );
            assert!(started.elapsed() < Duration::from_secs(10));
        }
    }
}
