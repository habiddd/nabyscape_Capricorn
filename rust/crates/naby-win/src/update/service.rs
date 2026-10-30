//! The update state machine (C# `OfficialUpdates` instance members): check, download, cancel,
//! on worker threads. Network, setup inspection and the "something changed" wake-up are
//! injected, so the whole life cycle runs offline in tests.
//!
//! One operation runs at a time (`busy`); a second request while busy is ignored, exactly like
//! the C# `Interlocked` guard. Operations time out after ten minutes.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use chrono::Utc;
use naby_core::i18n;
use naby_core::store::Store;
use naby_core::updates::{
    check_due, check_with, download_setup, HttpResponse, Platform, Release, Transport, UpdateCheck,
    UpdateError, VerifiedSetup, Version, REPOSITORY,
};
use naby_ui::settings::InstallKind;

/// A GET with the update policy of `naby_core::updates` applied on top.
pub type GetFn = Arc<dyn Fn(&str) -> Result<HttpResponse, String> + Send + Sync>;
/// `OfficialUpdates.InspectSetup` minus the PE check the download already does.
pub type InspectFn = Arc<dyn Fn(&Path, &Version) -> Result<(), UpdateError> + Send + Sync>;
/// Wakes the UI thread after any state change.
pub type NotifyFn = Arc<dyn Fn() + Send + Sync>;

/// Operations time out after this long (C# `CancellationTokenSource(TimeSpan.FromMinutes(10))`).
pub const OPERATION_TIMEOUT: Duration = Duration::from_secs(600);

pub struct ServiceConfig {
    pub store: Arc<Store>,
    pub platform: Platform,
    pub current: Version,
    pub staging: PathBuf,
    pub get: GetFn,
    pub inspect: InspectFn,
    pub notify: NotifyFn,
    pub retry_delay: Duration,
    pub install_kind: InstallKind,
}

/// What the app shell turns into notices (C# `UpdateAvailable` / `DownloadReady` events).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateEvent {
    Available(Release),
    DownloadReady,
}

/// A snapshot of the service for Settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateView {
    pub status: String,
    pub available: Option<String>,
    pub busy: bool,
    pub can_download: bool,
    pub can_install: bool,
    pub can_restore: bool,
    pub install_kind: InstallKind,
    pub recovery: String,
}

struct Shared {
    status: String,
    available: Option<Release>,
    verified: Option<VerifiedSetup>,
    events: Vec<UpdateEvent>,
    recovery: String,
    has_previous: bool,
    language: String,
}

struct Inner {
    cfg: ServiceConfig,
    shared: Mutex<Shared>,
    busy: AtomicBool,
    cancel: AtomicBool,
}

#[derive(Clone)]
pub struct UpdateService {
    inner: Arc<Inner>,
}

impl UpdateService {
    pub fn new(cfg: ServiceConfig) -> Self {
        Self {
            inner: Arc::new(Inner {
                cfg,
                shared: Mutex::new(Shared {
                    status: "Ready to check for updates.".to_owned(),
                    available: None,
                    verified: None,
                    events: Vec::new(),
                    recovery: String::new(),
                    has_previous: false,
                    language: "en".to_owned(),
                }),
                busy: AtomicBool::new(false),
                cancel: AtomicBool::new(false),
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Shared> {
        self.inner.lock()
    }

    pub fn set_language(&self, language: &str) {
        self.lock().language = language.to_owned();
    }

    /// The recovery line and whether a previous version exists (computed by the host, which
    /// owns the recovery files).
    pub fn set_recovery(&self, description: String, has_previous: bool) {
        let mut shared = self.lock();
        shared.recovery = description;
        shared.has_previous = has_previous;
        drop(shared);
        (self.inner.cfg.notify)();
    }

    /// Replaces the status line (hand-off failures).
    pub fn set_status(&self, text: &str) {
        self.inner.report(text.to_owned());
    }

    pub fn busy(&self) -> bool {
        self.inner.busy.load(Ordering::Acquire)
    }

    pub fn available(&self) -> Option<Release> {
        self.lock().available.clone()
    }

    pub fn verified(&self) -> Option<VerifiedSetup> {
        self.lock().verified.clone()
    }

    pub fn staging(&self) -> &Path {
        &self.inner.cfg.staging
    }

    pub fn take_events(&self) -> Vec<UpdateEvent> {
        std::mem::take(&mut self.lock().events)
    }

    /// The URL "Release notes" opens: the available release, else the releases page.
    pub fn notes_url(&self) -> String {
        self.lock()
            .available
            .as_ref()
            .map_or_else(|| format!("{REPOSITORY}/releases"), |r| r.notes.clone())
    }

    pub fn view(&self) -> UpdateView {
        let shared = self.lock();
        let busy = self.busy();
        UpdateView {
            status: shared.status.clone(),
            available: shared.available.as_ref().map(|r| r.version.to_string()),
            busy,
            can_download: !busy && shared.available.is_some(),
            can_install: !busy && shared.verified.is_some(),
            can_restore: !busy && shared.has_previous,
            install_kind: self.inner.cfg.install_kind,
            recovery: shared.recovery.clone(),
        }
    }

    /// C# `Cancel`.
    pub fn cancel(&self) {
        self.inner.cancel.store(true, Ordering::SeqCst);
    }

    /// C# `Check`: manual checks always run; automatic ones need `enabled` and 24 h since the
    /// last attempt. With `auto_download` a found update is downloaded right away. `None` when
    /// another operation is running; otherwise the worker's handle.
    pub fn check(
        &self,
        manual: bool,
        enabled: bool,
        auto_download: bool,
    ) -> Option<JoinHandle<()>> {
        self.spawn(move |inner| {
            if inner.run_check(manual, enabled) && !manual && auto_download && inner.has_available()
            {
                inner.run_download();
            }
        })
    }

    /// C# `Download`: fetch checksums, download, verify, inspect, stage.
    pub fn download(&self) -> Option<JoinHandle<()>> {
        self.lock().available.as_ref()?;
        self.spawn(|inner| inner.run_download())
    }

    /// Runs `work` on a worker thread while the service counts as busy, so no check or
    /// download starts meanwhile (the hand-off preparation uses this).
    pub fn spawn_exclusive(
        &self,
        work: impl FnOnce(&UpdateService) + Send + 'static,
    ) -> Option<JoinHandle<()>> {
        let service = self.clone();
        self.spawn(move |_| work(&service))
    }

    fn spawn(&self, work: impl FnOnce(&Inner) + Send + 'static) -> Option<JoinHandle<()>> {
        if self.inner.busy.swap(true, Ordering::AcqRel) {
            return None;
        }
        self.inner.cancel.store(false, Ordering::SeqCst);
        let inner = self.inner.clone();
        Some(std::thread::spawn(move || {
            work(&inner);
            inner.busy.store(false, Ordering::Release);
            (inner.cfg.notify)();
        }))
    }
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, Shared> {
        self.shared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn report(&self, text: String) {
        self.lock().status = text;
        (self.cfg.notify)();
    }

    fn language(&self) -> String {
        self.lock().language.clone()
    }

    fn has_available(&self) -> bool {
        self.lock().available.is_some()
    }

    /// Returns true when the metadata was read (so an automatic download may follow).
    fn run_check(&self, manual: bool, enabled: bool) -> bool {
        let mut state = self.cfg.store.load_update_state();
        let now = Utc::now();
        if !check_due(&state, manual, enabled, now) {
            return false;
        }
        // The attempt is recorded before the request so a crash cannot retry in a loop.
        state.last_attempt = now;
        if let Err(error) = self.cfg.store.save_update_state(&state) {
            self.cfg.store.log(&error);
        }
        self.report("Checking official releases\u{2026}".to_owned());
        let get = self.cfg.get.clone();
        let fetch = move |url: &str| get(url);
        let transport = Transport::new(&fetch).with_retry_delay(self.cfg.retry_delay);
        let outcome = check_with(self.cfg.current, &self.cfg.platform, transport);
        let language = self.language();
        let text = outcome.status_text(&language);
        let mut shared = self.lock();
        shared.verified = None;
        let ok = match outcome {
            UpdateCheck::Available(release) => {
                shared.available = Some(release.clone());
                shared.events.push(UpdateEvent::Available(release));
                true
            }
            UpdateCheck::UpToDate => {
                shared.available = None;
                true
            }
            UpdateCheck::Failed(_) => false,
        };
        shared.status = text;
        drop(shared);
        (self.cfg.notify)();
        ok
    }

    fn run_download(&self) {
        let Some(release) = self.lock().available.clone() else {
            return;
        };
        self.lock().verified = None;
        self.report("Downloading checksums\u{2026}".to_owned());
        let language = self.language();
        let started = Instant::now();
        let get = self.cfg.get.clone();
        let fetch = move |url: &str| get(url);
        let transport = Transport::new(&fetch).with_retry_delay(self.cfg.retry_delay);
        let mut progress = |percent: u64| {
            if started.elapsed() > OPERATION_TIMEOUT {
                self.cancel.store(true, Ordering::SeqCst);
            }
            self.report(i18n::format(
                &language,
                "Downloading update\u{2026} {0}%",
                &[&percent],
            ));
        };
        let result = download_setup(
            &release,
            &self.cfg.platform,
            &self.cfg.staging,
            transport,
            &*self.cfg.inspect,
            &mut progress,
            &self.cancel,
        );
        match result {
            Ok(setup) => {
                let mut shared = self.lock();
                shared.verified = Some(setup);
                shared.events.push(UpdateEvent::DownloadReady);
                shared.status =
                    "Download verified. Choose Install and restart when ready.".to_owned();
            }
            Err(UpdateError::Cancelled) => {
                self.lock().status = UpdateError::Cancelled.to_string();
            }
            Err(error) => {
                self.cfg.store.log(&error);
                let text = i18n::format(&language, "Update failed: {0}", &[&error]);
                self.lock().status = text;
            }
        }
        (self.cfg.notify)();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use naby_core::updates::{sha256_hex, REPOSITORY};
    use std::sync::atomic::AtomicUsize;

    const SETUP: &[u8] = b"abcd";

    fn release_json(version: &str) -> String {
        format!(
            "{{\"tag_name\":\"v{version}\",\"html_url\":\"{REPOSITORY}/releases/tag/v{version}\",\"draft\":false,\"prerelease\":false,\"assets\":[{{\"name\":\"NabyScape-win-x64-Setup.exe\",\"browser_download_url\":\"{REPOSITORY}/releases/download/v{version}/NabyScape-win-x64-Setup.exe\",\"size\":{}}},{{\"name\":\"SHA256SUMS\",\"browser_download_url\":\"{REPOSITORY}/releases/download/v{version}/SHA256SUMS\",\"size\":100}}]}}",
            SETUP.len()
        )
    }

    struct Fixture {
        service: UpdateService,
        _dir: tempfile::TempDir,
        requests: Arc<AtomicUsize>,
        inspections: Arc<AtomicUsize>,
        wakes: Arc<AtomicUsize>,
        store: Arc<Store>,
    }

    fn fixture_with(
        get: impl Fn(&str, usize) -> Result<HttpResponse, String> + Send + Sync + 'static,
    ) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(dir.path().join("data")));
        let requests = Arc::new(AtomicUsize::new(0));
        let inspections = Arc::new(AtomicUsize::new(0));
        let wakes = Arc::new(AtomicUsize::new(0));
        let (r, i, w) = (requests.clone(), inspections.clone(), wakes.clone());
        let service = UpdateService::new(ServiceConfig {
            store: store.clone(),
            platform: Platform::X64,
            current: Version::new(3, 0, 0),
            staging: dir.path().join("staging"),
            get: Arc::new(move |url| {
                let n = r.fetch_add(1, Ordering::SeqCst);
                get(url, n)
            }),
            inspect: Arc::new(move |_, _| {
                i.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }),
            notify: Arc::new(move || {
                w.fetch_add(1, Ordering::SeqCst);
            }),
            retry_delay: Duration::from_millis(1),
            install_kind: InstallKind::Automatic,
        });
        Fixture {
            service,
            _dir: dir,
            requests,
            inspections,
            wakes,
            store,
        }
    }

    fn normal(url: &str, _n: usize) -> Result<HttpResponse, String> {
        if url.contains("/releases/latest") {
            Ok(HttpResponse::new(200, release_json("3.1.0")))
        } else if url.ends_with("SHA256SUMS") {
            Ok(HttpResponse::new(
                200,
                format!(
                    "{}  NabyScape-win-x64-Setup.exe\n",
                    sha256_hex(SETUP).to_uppercase()
                ),
            ))
        } else {
            Ok(HttpResponse::new(200, SETUP))
        }
    }

    fn join(handle: Option<JoinHandle<()>>) {
        handle.expect("operation started").join().unwrap();
    }

    #[test]
    fn manual_check_finds_a_newer_release_and_reports_it_once() {
        let f = fixture_with(normal);
        assert!(f.service.view().available.is_none());
        join(f.service.check(true, false, false));
        let view = f.service.view();
        assert_eq!(view.status, "Version 3.1.0 is available.");
        assert_eq!(view.available.as_deref(), Some("3.1.0"));
        assert!(view.can_download && !view.can_install && !view.busy);
        let events = f.service.take_events();
        assert!(
            matches!(events.as_slice(), [UpdateEvent::Available(r)] if r.version == Version::new(3, 1, 0))
        );
        assert!(f.service.take_events().is_empty());
        assert!(
            f.wakes.load(Ordering::SeqCst) >= 2,
            "the UI is woken for each state change"
        );
        assert_eq!(
            f.service.notes_url(),
            format!("{REPOSITORY}/releases/tag/v3.1.0")
        );
    }

    #[test]
    fn up_to_date_and_failures_have_their_own_status() {
        let f = fixture_with(|url, n| {
            let _ = (url, n);
            Ok(HttpResponse::new(200, release_json("3.0.0")))
        });
        join(f.service.check(true, false, false));
        assert_eq!(
            f.service.view().status,
            "You have the latest stable version."
        );
        assert!(f.service.view().available.is_none());
        assert_eq!(f.service.notes_url(), format!("{REPOSITORY}/releases"));

        let offline = fixture_with(|_, _| Err("offline".to_owned()));
        join(offline.service.check(true, false, false));
        assert_eq!(offline.service.view().status, "Update failed: offline");
        let status_500 = fixture_with(|_, _| Ok(HttpResponse::new(500, "")));
        join(status_500.service.check(true, false, false));
        assert!(status_500
            .service
            .view()
            .status
            .starts_with("Update failed: "));
        assert!(
            !status_500.service.view().busy,
            "a failed check never leaves a stuck spinner"
        );
    }

    #[test]
    fn automatic_checks_follow_the_preference_and_the_24_hour_throttle() {
        let f = fixture_with(normal);
        // Disabled: nothing is requested and the throttle state is untouched.
        join(f.service.check(false, false, false));
        assert_eq!(f.requests.load(Ordering::SeqCst), 0);
        assert_eq!(
            f.store.load_update_state().last_attempt,
            chrono::DateTime::<Utc>::default()
        );
        // Enabled and never attempted: it checks and records the attempt.
        join(f.service.check(false, true, false));
        assert_eq!(f.requests.load(Ordering::SeqCst), 1);
        assert!(
            Utc::now() - f.store.load_update_state().last_attempt < chrono::Duration::minutes(1)
        );
        // Within 24 h an automatic check is skipped, a manual one is not.
        join(f.service.check(false, true, false));
        assert_eq!(f.requests.load(Ordering::SeqCst), 1);
        join(f.service.check(true, true, false));
        assert_eq!(f.requests.load(Ordering::SeqCst), 2);
        // Older than 24 h: due again.
        let mut state = f.store.load_update_state();
        state.last_attempt = Utc::now() - chrono::Duration::hours(25);
        f.store.save_update_state(&state).unwrap();
        join(f.service.check(false, true, false));
        assert_eq!(f.requests.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn a_failed_attempt_still_counts_for_the_throttle() {
        let f = fixture_with(|_, _| Err("offline".to_owned()));
        join(f.service.check(false, true, false));
        assert!(
            Utc::now() - f.store.load_update_state().last_attempt < chrono::Duration::minutes(1)
        );
    }

    #[test]
    fn download_verifies_stages_and_announces_the_setup() {
        let f = fixture_with(normal);
        assert!(
            f.service.download().is_none(),
            "nothing to download before a check"
        );
        join(f.service.check(true, false, false));
        f.service.take_events();
        join(f.service.download());
        let view = f.service.view();
        assert_eq!(
            view.status,
            "Download verified. Choose Install and restart when ready."
        );
        assert!(view.can_install);
        let setup = f.service.verified().unwrap();
        assert_eq!(std::fs::read(&setup.path).unwrap(), SETUP);
        assert_eq!(setup.hash, sha256_hex(SETUP).to_uppercase());
        assert_eq!(f.inspections.load(Ordering::SeqCst), 1);
        assert_eq!(f.service.take_events(), [UpdateEvent::DownloadReady]);
        assert!(!f
            .service
            .staging()
            .join("NabyScape-Setup.exe.partial")
            .exists());
        // A second download reuses the verified file without fetching it again.
        let before = f.requests.load(Ordering::SeqCst);
        join(f.service.download());
        assert_eq!(
            f.requests.load(Ordering::SeqCst),
            before + 1,
            "only the checksums are fetched"
        );
        assert_eq!(f.inspections.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_checksum_mismatch_or_truncated_download_keeps_the_app_unchanged() {
        let f = fixture_with(|url, n| {
            let _ = n;
            if url.ends_with("SHA256SUMS") {
                Ok(HttpResponse::new(
                    200,
                    format!("{}  NabyScape-win-x64-Setup.exe", "A".repeat(64)),
                ))
            } else if url.contains("/releases/latest") {
                Ok(HttpResponse::new(200, release_json("3.1.0")))
            } else {
                Ok(HttpResponse::new(200, SETUP))
            }
        });
        join(f.service.check(true, false, false));
        join(f.service.download());
        let view = f.service.view();
        assert!(view
            .status
            .starts_with("Update failed: Setup checksum mismatch"));
        assert!(!view.can_install && f.service.verified().is_none());
        assert!(!f
            .service
            .staging()
            .join("NabyScape-Setup.exe.partial")
            .exists());
        assert!(!f.service.staging().join("NabyScape-Setup.exe").exists());
        assert_eq!(
            f.inspections.load(Ordering::SeqCst),
            0,
            "inspection runs only after the checksum"
        );
    }

    #[test]
    fn automatic_download_follows_a_scheduled_check_but_never_a_manual_one() {
        let f = fixture_with(normal);
        join(f.service.check(false, true, true));
        assert!(
            f.service.verified().is_some(),
            "auto-download after the startup check"
        );
        assert_eq!(
            f.service.take_events(),
            [
                UpdateEvent::Available(f.service.available().unwrap()),
                UpdateEvent::DownloadReady
            ]
        );
        let manual = fixture_with(normal);
        join(manual.service.check(true, true, true));
        assert!(manual.service.verified().is_none());
        let no_update = fixture_with(|_, _| Ok(HttpResponse::new(200, release_json("3.0.0"))));
        join(no_update.service.check(false, true, true));
        assert!(no_update.service.verified().is_none());
    }

    #[test]
    fn only_one_operation_runs_at_a_time() {
        use std::sync::mpsc::channel;
        let (release_tx, release_rx) = channel::<()>();
        let release_rx = Mutex::new(release_rx);
        let f = fixture_with(move |url, _| {
            if url.contains("/releases/latest") {
                let _ = release_rx
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(5));
            }
            normal(url, 0)
        });
        let first = f.service.check(true, false, false);
        assert!(f.service.busy());
        assert!(f.service.view().busy && !f.service.view().can_download);
        assert!(
            f.service.check(true, false, false).is_none(),
            "a second check is ignored"
        );
        assert!(f.service.download().is_none());
        assert!(f.service.spawn_exclusive(|_| ()).is_none());
        release_tx.send(()).unwrap();
        join(first);
        assert!(!f.service.busy());
        join(
            f.service
                .spawn_exclusive(|s| s.set_status("handoff failed")),
        );
        assert_eq!(f.service.view().status, "handoff failed");
    }

    #[test]
    fn cancel_stops_a_running_download_and_keeps_the_current_app() {
        let service_slot: Arc<Mutex<Option<UpdateService>>> = Arc::new(Mutex::new(None));
        let slot = service_slot.clone();
        let f = fixture_with(move |url, _| {
            if url.ends_with("SHA256SUMS") {
                // Cancel as soon as the checksums are requested; the setup read notices it.
                if let Some(service) = slot.lock().unwrap().as_ref() {
                    service.cancel();
                }
            }
            normal(url, 0)
        });
        join(f.service.check(true, false, false));
        *service_slot.lock().unwrap() = Some(f.service.clone());
        // `spawn` resets the flag, so cancel from inside the transport (after the reset).
        join(f.service.download());
        let view = f.service.view();
        assert_eq!(
            view.status,
            "Update cancelled or timed out. Your current app is unchanged."
        );
        assert!(f.service.verified().is_none() && !view.can_install);
        assert!(!f
            .service
            .staging()
            .join("NabyScape-Setup.exe.partial")
            .exists());
        // A later download works again: the flag is cleared at the start of each operation.
        *service_slot.lock().unwrap() = None;
        join(f.service.download());
        assert!(f.service.verified().is_some());
    }

    #[test]
    fn view_maps_into_settings_info() {
        let f = fixture_with(normal);
        f.service
            .set_recovery("Previous version: 2.24.0".to_owned(), true);
        join(f.service.check(true, false, false));
        let info = crate::update::to_info(&f.service.view(), "3.0.0", "win-x64");
        assert_eq!(info.installed_version, "3.0.0");
        assert_eq!(info.platform, "win-x64");
        assert_eq!(info.available.as_deref(), Some("3.1.0"));
        assert!(info.can_download && info.can_restore && !info.can_install);
        assert_eq!(info.install_kind, InstallKind::Automatic);
        assert_eq!(info.recovery, "Previous version: 2.24.0");
    }

    #[test]
    fn status_text_is_localized_with_the_ui_language() {
        let f = fixture_with(normal);
        f.service.set_language("es");
        join(f.service.check(true, false, false));
        assert_eq!(
            f.service.view().status,
            "La versi\u{f3}n 3.1.0 est\u{e1} disponible."
        );
    }
}
