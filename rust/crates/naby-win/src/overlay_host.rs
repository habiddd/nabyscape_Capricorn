//! Hosts the overlay dock: owns the services, builds [`Snapshots`], executes [`Command`]s and
//! adapts the platform-free [`Overlay`](naby_ui::Overlay) to the shell's `Surface` seam
//! (replaces the old placeholder surface).
//!
//! Everything that needs the OS lives in the Windows-only `OverlayHost`; the rules around it
//! (countdown and stopwatch operations, completion messages, display mapping, deadlines) are
//! plain functions tested on every platform.
//!
//! Threading: the host lives on the UI thread. Services publish through the shared `Waker`
//! (`WM_APP_WAKE`); the shell then calls `wake()`, which pulls every changed snapshot. Lyrics and
//! weather fetches run on short-lived worker threads and post their result the same way.

use crate::monitors::MonitorInfo;
use chrono::{DateTime, Duration, Utc};
use naby_core::motion::RectI as CoreRect;
use naby_core::productivity::{FocusConfig, FocusEvent, Timer};
use naby_ui::host::DisplayInfo;

/// What the shell must do on the host's behalf (things only the app shell can reach).
#[derive(Debug, Clone, PartialEq)]
pub enum ShellAction {
    /// Open Settings, optionally at a page id.
    OpenSettings(Option<String>),
    Quit,
    /// A tray balloon (Windows-routed notification or a message the dock cannot show).
    Balloon {
        title: String,
        detail: String,
    },
    OpenAssistant,
    OpenVoice,
    /// New preferences are in effect (already saved): re-register hotkeys, hooks, opacity.
    SettingsApplied(Box<naby_core::settings::Settings>),
    /// The island layout changed while it is being moved or resized (not saved yet): the shell
    /// only remembers the values, so a later toggle keeps the edit.
    SettingsPreview(Box<naby_core::settings::Settings>),
}

/// The dock's monitor as the views see it.
pub fn display_info(monitor: &MonitorInfo, refresh_hz: u32) -> DisplayInfo {
    let rect = |r: &crate::monitors::RectI| CoreRect::new(r.left, r.top, r.width(), r.height());
    DisplayInfo {
        id: monitor.id.clone(),
        bounds: rect(&monitor.bounds),
        work: rect(&monitor.work),
        scale: monitor.scale(),
        refresh_hz: if refresh_hz < 2 { 60 } else { refresh_hz },
    }
}

// ---- countdown / stopwatch operations --------------------------------------------------

/// Pause when running, resume when paused, otherwise start five minutes (`TogglePeekClock`).
pub fn timer_toggle(timer: &mut Timer, now: DateTime<Utc>) {
    if timer.running() {
        timer.pause(now);
    } else if timer.remaining(now) > 0.0 {
        timer.resume(now);
    } else {
        timer.start(300.0, now, "", false);
    }
}

/// Adds `seconds` to a countdown; an idle one starts with `seconds` (`ExtendPeekClock`).
pub fn countdown_extend(timer: &mut Timer, now: DateTime<Utc>, seconds: f64, phase: &str) {
    let remaining = timer.remaining(now);
    if remaining <= 0.0 {
        timer.start(seconds, now, phase, false);
        return;
    }
    timer.duration_seconds += seconds;
    timer.remaining_seconds = remaining + seconds;
    if timer.running() {
        timer.deadline =
            Some(now + Duration::milliseconds(((remaining + seconds) * 1000.0) as i64));
    }
}

/// Title and detail of a focus transition (`App.CheckCountdowns`).
pub fn focus_message(event: FocusEvent) -> (&'static str, &'static str) {
    match event {
        FocusEvent::BreakStarted => ("Focus complete", "Time for a short break."),
        FocusEvent::SessionComplete => (
            "Focus session complete",
            "Your focus session and break have finished.",
        ),
        FocusEvent::BreakFinished => ("Break finished", "Ready for your next focus session."),
    }
}

/// Earliest running countdown deadline, for scheduling completion checks.
pub fn next_countdown_deadline(timer: &Timer, focus: &Timer) -> Option<DateTime<Utc>> {
    [timer, focus]
        .into_iter()
        .filter(|t| t.running())
        .filter_map(|t| t.deadline)
        .min()
}

/// The focus durations from the preferences.
pub fn focus_config(s: &naby_core::settings::Settings) -> FocusConfig {
    FocusConfig {
        focus_minutes: s.focus_minutes.max(1) as u32,
        break_minutes: s.break_minutes.max(1) as u32,
    }
}

/// Static page id for an assistant / command navigation request.
pub fn static_page(id: &str) -> Option<&'static str> {
    const EXTRA: [&str; 5] = ["access", "status", "mixer", "lyrics", "tools"];
    naby_core::settings::WIDGET_NAMES
        .iter()
        .map(|(k, _)| *k)
        .chain(EXTRA)
        .find(|k| *k == id)
}

#[cfg(windows)]
pub use os::OverlayHost;

#[cfg(windows)]
mod os {
    use super::*;
    use crate::app::Surface;
    use crate::audio::AudioService;
    use crate::awake::{new_keep_awake, KeepAwake};
    use crate::battery::BatteryService;
    use crate::brightness::BrightnessService;
    use crate::clipboard::{ClipboardEvent, ClipboardService};
    use crate::detached_host::DetachedHost;
    use crate::dialogs;
    use crate::downloads::{DownloadEvent, DownloadWatcher};
    use crate::dragdrop::{self, DropEvent, DropRegistration};
    use crate::launcher::{CatalogState, LauncherService};
    use crate::loopback::{LoopbackLease, LoopbackService, Need};
    use crate::media::MediaService;
    use crate::mixer::MixerService;
    use crate::shell;
    use crate::sounds::{CountdownSoundConfig, SoundService};
    use crate::system::{SystemLease, SystemOptions, SystemService};
    use crate::wake::Waker;
    use chrono::Local;
    use naby_core::command::{Command, MediaSource};
    use naby_core::media::lyrics::default_fetch as lyrics_fetch;
    use naby_core::media::{lrc, FetchError, LyricsRequest, LyricsService};
    use naby_core::notifications::{InternalNotice, NoticeKind, Notifications};
    use naby_core::productivity::{check_countdowns, ProductivityService};
    use naby_core::session::{Clock, SessionState};
    use naby_core::settings::Settings;
    use naby_core::store::Store;
    use naby_core::weather::{self, WeatherError, WeatherService, WeatherSnapshot};
    use naby_render::{Canvas, Fonts, Image, InputState};
    use naby_ui::host::{
        AppCatalog, AppEntry, AssistantSummary, AudioLevels, AwakeInfo, BrightnessState,
        CalendarHandle, Demand, IconProvider, MediaSourceChoice, MediaSources, NetRates,
        NoticeState, PlaceSearch, PlaceSearchState, Signal, Snapshots, SystemInfo, View,
        WeatherState, WeatherStatus,
    };
    use naby_ui::{Overlay, OverlayState};
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::mpsc::{channel, Receiver, Sender};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration as StdDuration, Instant};
    use windows::Win32::Foundation::HWND;

    /// Seconds a battery reading may be reused.
    const BATTERY_REUSE: f64 = 15.0;
    /// How often the weather page re-checks whether its forecast is stale.
    const WEATHER_CHECK: f64 = 180.0;
    /// `LyricsService` consumer id of the dock.
    const LYRICS_OWNER: u64 = 1;

    type LyricsOutcome = (LyricsRequest, Result<String, FetchError>);
    type WeatherOutcome = (String, Result<WeatherSnapshot, WeatherError>);
    type PlacesOutcome = (String, Result<Vec<weather::WeatherPlace>, WeatherError>);

    /// Shared with the OLE drop target callbacks, which run inside the message pump.
    #[derive(Default)]
    struct DropState {
        dragging: bool,
        accept: bool,
        dropped: Vec<String>,
    }

    pub struct OverlayHost {
        store: Arc<Store>,
        wake: Waker,
        base: Instant,
        settings: Settings,
        snaps: Snapshots,
        overlay: Overlay,
        out: Vec<Command>,
        actions: Vec<ShellAction>,
        // ---- services
        media: Option<MediaService>,
        audio: AudioService,
        mixer: Option<MixerService>,
        loopback: LoopbackService,
        lease: Option<(Need, LoopbackLease)>,
        brightness: BrightnessService,
        sounds: SoundService,
        battery: BatteryService,
        battery_at: f64,
        system: SystemService,
        system_lease: Option<SystemLease>,
        clipboard: ClipboardService,
        downloads: DownloadWatcher,
        drop_registration: Option<DropRegistration>,
        drops: Rc<RefCell<DropState>>,
        keep_awake: KeepAwake,
        // ---- state
        notifications: Notifications,
        notice_serial: u64,
        session: SessionState,
        lyrics: LyricsService,
        lyrics_tx: Sender<LyricsOutcome>,
        lyrics_rx: Receiver<LyricsOutcome>,
        lyrics_owned: bool,
        weather: Arc<Mutex<WeatherService>>,
        weather_tx: Sender<WeatherOutcome>,
        weather_rx: Receiver<WeatherOutcome>,
        weather_busy: bool,
        weather_check_at: f64,
        places_tx: Sender<PlacesOutcome>,
        places_rx: Receiver<PlacesOutcome>,
        demand: Demand,
        last_media_source: String,
        files_drag: bool,
        repaint_requested: bool,
        // ---- tool pages
        launcher: Rc<RefCell<LauncherService>>,
        icon_epoch: Arc<AtomicU64>,
        catalog_source: Option<(usize, Arc<Vec<AppEntry>>)>,
        picker_open: bool,
        detached: DetachedHost,
        dock_editing: bool,
        pending_drag: Option<Vec<String>>,
    }

    /// Launcher icons and file previews for the views (never blocks: a miss queues the
    /// extraction and the wake it causes repaints).
    struct LauncherIcons(Rc<RefCell<LauncherService>>);

    impl IconProvider for LauncherIcons {
        fn shortcut_icon(
            &self,
            shortcut: &naby_core::settings::Shortcut,
            pixels: u32,
        ) -> Option<Arc<Image>> {
            self.0.try_borrow_mut().ok()?.icon(shortcut, pixels)
        }

        fn file_thumbnail(&self, path: &str, pixels: u32) -> Option<Arc<Image>> {
            self.0.try_borrow_mut().ok()?.file_thumbnail(path, pixels)
        }

        fn clear(&self) {
            if let Ok(launcher) = self.0.try_borrow() {
                launcher.clear_icons();
            }
        }
    }

    fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
        m.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The "Timed lyrics for the current track" file dialog (`*.lrc`): the chosen path.
    fn pick_lrc_file() -> Option<String> {
        use windows::core::w;
        use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_INPROC_SERVER};
        use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
        use windows::Win32::UI::Shell::{
            FileOpenDialog, IFileOpenDialog, FOS_FILEMUSTEXIST, FOS_FORCEFILESYSTEM,
            SIGDN_FILESYSPATH,
        };
        // SAFETY: plain COM calls on the UI thread (COM is initialised by the shell); the dialog
        // and the shell item are released when dropped, and the returned display-name buffer is
        // freed with `CoTaskMemFree` after it has been copied.
        unsafe {
            let dialog: IFileOpenDialog =
                CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
            let filters = [COMDLG_FILTERSPEC {
                pszName: w!("Timed lyrics (*.lrc)"),
                pszSpec: w!("*.lrc"),
            }];
            dialog.SetFileTypes(&filters).ok()?;
            dialog
                .SetTitle(w!("Timed lyrics for the current track"))
                .ok()?;
            let options = dialog.GetOptions().ok()?;
            dialog
                .SetOptions(options | FOS_FORCEFILESYSTEM | FOS_FILEMUSTEXIST)
                .ok()?;
            dialog.Show(None).ok()?;
            let item = dialog.GetResult().ok()?;
            let name = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
            let path = name.to_string().ok();
            CoTaskMemFree(Some(name.0.cast_const().cast()));
            path
        }
    }

    impl OverlayHost {
        pub fn new(
            store: Arc<Store>,
            wake: Waker,
            settings: Settings,
            display: DisplayInfo,
        ) -> OverlayHost {
            let base = Instant::now();
            let mut snaps = Snapshots::new(settings.clone(), 0.0);
            snaps.display.replace(display);
            snaps.productivity = Rc::new(RefCell::new(ProductivityService::open(&store)));
            let calendar_store = Arc::clone(&store);
            snaps.calendar = CalendarHandle::new(
                store.load_calendar(),
                Box::new(move |events| {
                    calendar_store
                        .save_calendar(events)
                        .map_err(|e| e.to_string())
                }),
            );
            let mut session = store.load_session().unwrap_or_default();
            session.stopwatch.recover(Clock::system());
            snaps.session.replace(session.clone());
            let (lyrics_tx, lyrics_rx) = channel();
            let (weather_tx, weather_rx) = channel();
            let icon_epoch = Arc::new(AtomicU64::new(0));
            let icon_wake: Waker = {
                let epoch = Arc::clone(&icon_epoch);
                let wake = wake.clone();
                Arc::new(move || {
                    epoch.fetch_add(1, Ordering::AcqRel);
                    wake();
                })
            };
            let launcher = Rc::new(RefCell::new(LauncherService::start(icon_wake)));
            snaps.icons = Rc::new(LauncherIcons(Rc::clone(&launcher)));
            let (places_tx, places_rx) = channel();
            let overlay = Overlay::new(&snaps);
            let mut host = OverlayHost {
                media: naby_core::updates::runtime::media_features_wanted(&settings)
                    .then(|| MediaService::start(wake.clone())),
                audio: AudioService::start(wake.clone()),
                mixer: None,
                loopback: LoopbackService::start(wake.clone()),
                lease: None,
                brightness: BrightnessService::start(
                    wake.clone(),
                    settings.reactive_feedback && settings.brightness_feedback,
                ),
                sounds: SoundService::start(wake.clone()),
                battery: BatteryService::start(wake.clone()),
                battery_at: f64::NEG_INFINITY,
                system: SystemService::start(wake.clone()),
                system_lease: None,
                clipboard: ClipboardService::start(wake.clone()),
                downloads: DownloadWatcher::start(wake.clone()),
                drop_registration: None,
                drops: Rc::new(RefCell::new(DropState::default())),
                keep_awake: new_keep_awake(),
                notifications: Notifications::new(),
                notice_serial: 0,
                session,
                lyrics: LyricsService::new(),
                lyrics_tx,
                lyrics_rx,
                lyrics_owned: false,
                weather: Arc::new(Mutex::new(WeatherService::with_cached(
                    store.load_weather_cache(),
                ))),
                weather_tx,
                weather_rx,
                weather_busy: false,
                weather_check_at: f64::INFINITY,
                places_tx,
                places_rx,
                demand: Demand::default(),
                last_media_source: String::new(),
                files_drag: false,
                repaint_requested: false,
                launcher,
                icon_epoch,
                catalog_source: None,
                picker_open: false,
                detached: DetachedHost::new(Arc::clone(&store)),
                dock_editing: false,
                pending_drag: None,
                store,
                wake,
                base,
                settings,
                snaps,
                overlay,
                out: Vec::new(),
                actions: Vec::new(),
            };
            host.configure_services();
            host.seed_weather();
            host.pull_all();
            host.run_overlay();
            host
        }

        /// Registers the dock window as a file drop target (shelf drops, drop-reveal).
        pub fn attach_window(&mut self, hwnd: HWND) {
            let state = Rc::clone(&self.drops);
            let wake = self.wake.clone();
            let handler: dragdrop::DropHandler = Rc::new(move |event: &DropEvent| {
                let mut s = state.borrow_mut();
                let accept = s.accept;
                match event {
                    DropEvent::Enter { .. } | DropEvent::Over { .. } => {
                        if accept && !s.dragging {
                            s.dragging = true;
                            wake();
                        }
                        accept
                    }
                    DropEvent::Leave => {
                        if s.dragging {
                            s.dragging = false;
                            wake();
                        }
                        false
                    }
                    DropEvent::Drop { files, .. } => {
                        s.dragging = false;
                        if accept {
                            s.dropped.extend(files.iter().cloned());
                        }
                        wake();
                        accept
                    }
                }
            });
            self.drop_registration = dragdrop::register(hwnd, handler).ok();
        }

        fn now(&self) -> f64 {
            self.base.elapsed().as_secs_f64()
        }

        fn stamp(&mut self) {
            self.snaps.now = self.now();
            self.snaps.wall = Local::now();
        }

        // ---- settings -------------------------------------------------------------------

        fn configure_services(&mut self) {
            let s = &self.settings;
            let want_media = naby_core::updates::runtime::media_features_wanted(s);
            if want_media && self.media.is_none() {
                self.media = Some(MediaService::start(self.wake.clone()));
            } else if !want_media {
                self.media = None;
            }
            self.brightness
                .set_enabled(s.reactive_feedback && s.brightness_feedback);
            let clip = s.clipboard_history;
            self.clipboard.set_enabled(clip);
            self.snaps
                .productivity
                .borrow_mut()
                .set_clipboard_enabled(clip);
            let folder =
                (!s.downloads_folder.trim().is_empty()).then(|| s.downloads_folder.clone());
            self.downloads
                .configure(s.downloads_enabled, folder.as_deref());
        }

        fn seed_weather(&mut self) {
            let key = weather::key_for(&self.settings);
            let cached = lock(&self.weather)
                .cached()
                .filter(|c| !key.is_empty() && c.key == key)
                .cloned();
            self.snaps.weather.set(WeatherState {
                snapshot: cached,
                status: WeatherStatus::Idle,
            });
        }

        /// New preferences from outside (settings window, startup) or from a command.
        fn apply_settings_inner(&mut self, settings: &Settings) {
            let weather_changed = weather::key_for(&self.settings) != weather::key_for(settings);
            self.settings = settings.clone();
            self.snaps.settings.set(settings.clone());
            self.configure_services();
            if weather_changed {
                self.seed_weather();
                self.weather_check_at = 0.0;
            }
        }

        // ---- pulling snapshots ----------------------------------------------------------

        /// Reads every service and publishes what changed.
        fn pull_all(&mut self) {
            self.stamp();
            self.pull_media();
            self.snaps.audio.set(self.audio.snapshot());
            if let Some(mixer) = &self.mixer {
                self.snaps.mixer.set(mixer.snapshot());
            }
            let b = self.brightness.snapshot();
            self.snaps.brightness.set(BrightnessState {
                level: b.level,
                sequence: b.sequence,
            });
            self.pull_battery();
            self.pull_system();
            self.pull_lyrics();
            self.pull_weather();
            self.pull_places();
            self.pull_clipboard();
            self.pull_downloads();
            self.pull_drops();
            self.publish_awake();
            self.pull_launcher();
            self.snaps.productivity_revision = self.snaps.productivity.borrow().revision();
            self.sync_drop_accept();
        }

        /// Publishes the app catalog and moves `icons_revision` when an icon arrived.
        fn pull_launcher(&mut self) {
            self.snaps.icons_revision = self.icon_epoch.load(Ordering::Acquire);
            let state = match self.launcher.borrow().snapshot() {
                CatalogState::Closed => AppCatalog::Closed,
                CatalogState::Loading => AppCatalog::Loading,
                CatalogState::Failed(message) => AppCatalog::Failed(message),
                CatalogState::Ready(apps) => {
                    let id = Arc::as_ptr(&apps) as usize;
                    let entries = match &self.catalog_source {
                        Some((source, entries)) if *source == id => Arc::clone(entries),
                        _ => {
                            let entries = Arc::new(
                                apps.iter()
                                    .map(|a| AppEntry {
                                        name: a.name.clone(),
                                        target: a.target.clone(),
                                        app_id: a.app_id.clone(),
                                    })
                                    .collect::<Vec<_>>(),
                            );
                            self.catalog_source = Some((id, Arc::clone(&entries)));
                            entries
                        }
                    };
                    AppCatalog::Ready(entries)
                }
            };
            if !matches!(state, AppCatalog::Ready(_)) {
                self.catalog_source = None;
            }
            self.snaps.catalog.set(state);
        }

        /// Opens or closes the app catalog (the shortcuts page shows its picker).
        fn set_picker(&mut self, open: bool) {
            if open == self.picker_open {
                return;
            }
            self.picker_open = open;
            let mut launcher = self.launcher.borrow_mut();
            if open {
                launcher.open_picker();
            } else {
                launcher.close_picker();
            }
        }

        fn pull_media(&mut self) {
            let now = self.snaps.now;
            let Some(media) = &self.media else {
                self.snaps.media.set(Default::default());
                self.snaps.sources.set(MediaSources::default());
                return;
            };
            self.snaps.sources.set(MediaSources {
                sessions: media
                    .sessions()
                    .into_iter()
                    .map(|s| MediaSourceChoice {
                        id: s.id,
                        name: s.name,
                    })
                    .collect(),
                selected: media.source_selection_id(),
            });
            let mut snap = media.snapshot();
            let (elapsed, _) = media.timeline();
            snap.position = StdDuration::from_secs_f64(elapsed.max(0.0));
            if self.snaps.media.set(snap) {
                self.snaps.media_epoch = now;
            }
            let source = self.snaps.media.source_id.clone();
            if source != self.last_media_source {
                self.loopback
                    .set_source((!source.is_empty()).then_some(source.as_str()));
                self.last_media_source = source;
            }
            self.load_lyrics();
        }

        fn pull_battery(&mut self) {
            let now = self.snaps.now;
            if now - self.battery_at >= BATTERY_REUSE {
                self.battery_at = now;
                self.snaps.battery.set(self.battery.snapshot());
            }
        }

        fn pull_system(&mut self) {
            if self.system_lease.is_none() {
                return;
            }
            let state = self.system.snapshot();
            self.snaps.system.set(SystemInfo {
                latest: state.latest.clone(),
                cpu: state.cpu,
                cpu_history: state.cpu_history.iter().collect(),
                networks: state
                    .networks
                    .iter()
                    .map(|n| NetRates {
                        id: n.id.clone(),
                        name: n.name.clone(),
                        received: n.received,
                        sent: n.sent,
                    })
                    .collect(),
                reads: state.reads,
            });
        }

        fn pull_lyrics(&mut self) {
            while let Ok((request, outcome)) = self.lyrics_rx.try_recv() {
                self.lyrics.complete(&request, outcome);
            }
            if self.lyrics.take_changed() {
                self.snaps.lyrics.set(self.lyrics.result().clone());
            }
        }

        fn load_lyrics(&mut self) {
            if !self.lyrics_owned {
                return;
            }
            let media = self.snaps.media.clone();
            let duration = media.timeline().1;
            if let Some(request) = self.lyrics.begin(
                &media.title,
                &media.artist,
                duration,
                true,
                lrc::can_use(&media),
            ) {
                let tx = self.lyrics_tx.clone();
                let wake = self.wake.clone();
                let _ = std::thread::Builder::new()
                    .name("naby-lyrics".into())
                    .spawn(move || {
                        let outcome = lyrics_fetch(&request.url);
                        let _ = tx.send((request, outcome));
                        wake();
                    });
            }
            if self.lyrics.take_changed() {
                self.snaps.lyrics.set(self.lyrics.result().clone());
            }
        }

        fn pull_weather(&mut self) {
            while let Ok((key, outcome)) = self.weather_rx.try_recv() {
                self.weather_busy = false;
                if key != weather::key_for(&self.settings) {
                    continue;
                }
                let previous = self.snaps.weather.snapshot.clone();
                self.snaps.weather.set(match outcome {
                    Ok(snapshot) => WeatherState {
                        snapshot: Some(snapshot),
                        status: WeatherStatus::Idle,
                    },
                    Err(error) => WeatherState {
                        snapshot: previous,
                        status: WeatherStatus::Failed(error.to_string()),
                    },
                });
            }
        }

        fn pull_places(&mut self) {
            while let Ok((query, outcome)) = self.places_rx.try_recv() {
                if query != self.snaps.places.query {
                    continue;
                }
                self.snaps.places.set(PlaceSearch {
                    query,
                    state: match outcome {
                        Ok(places) => PlaceSearchState::Found(places),
                        Err(_) => PlaceSearchState::Failed,
                    },
                });
            }
        }

        /// City search for the weather location editor (`WeatherView.Search`).
        fn search_places(&mut self, query: String) {
            self.snaps.places.set(PlaceSearch {
                query: query.clone(),
                state: PlaceSearchState::Searching,
            });
            let tx = self.places_tx.clone();
            let wake = self.wake.clone();
            let spawned = std::thread::Builder::new()
                .name("naby-places".into())
                .spawn(move || {
                    let result = WeatherService::search(&query, &weather::default_fetch);
                    let _ = tx.send((query, result));
                    wake();
                });
            if spawned.is_err() {
                self.snaps.places.set(PlaceSearch {
                    query: self.snaps.places.query.clone(),
                    state: PlaceSearchState::Failed,
                });
            }
        }

        fn request_weather(&mut self, force: bool) {
            if self.weather_busy {
                return;
            }
            let Some(place) = weather::place_for(&self.settings) else {
                let snapshot = self.snaps.weather.snapshot.clone();
                self.snaps.weather.set(WeatherState {
                    snapshot,
                    status: WeatherStatus::Failed(WeatherError::NoCity.to_string()),
                });
                return;
            };
            self.weather_busy = true;
            let snapshot = self.snaps.weather.snapshot.clone();
            self.snaps.weather.set(WeatherState {
                snapshot,
                status: WeatherStatus::Loading,
            });
            let key = weather::key_for(&self.settings);
            let units = self.settings.weather_units.clone();
            let service = Arc::clone(&self.weather);
            let store = Arc::clone(&self.store);
            let tx = self.weather_tx.clone();
            let wake = self.wake.clone();
            let spawned = std::thread::Builder::new()
                .name("naby-weather".into())
                .spawn(move || {
                    let result = lock(&service).get(
                        Some(&place),
                        &units,
                        force,
                        Utc::now(),
                        &weather::default_fetch,
                    );
                    if let Ok(snapshot) = &result {
                        let _ = store.save_weather_cache(snapshot);
                    }
                    let _ = tx.send((key, result));
                    wake();
                });
            if spawned.is_err() {
                self.weather_busy = false;
            }
        }

        fn pull_clipboard(&mut self) {
            for event in self.clipboard.drain() {
                match event {
                    ClipboardEvent::Text(text) => {
                        self.snaps
                            .productivity
                            .borrow_mut()
                            .ingest_text(Some(&text));
                    }
                    ClipboardEvent::Png(png) => {
                        self.snaps.productivity.borrow_mut().ingest_image(&png);
                    }
                    ClipboardEvent::Error(message) => self.store.log(&message),
                }
            }
        }

        fn pull_downloads(&mut self) {
            for event in self.downloads.drain() {
                match event {
                    DownloadEvent::Arrived(paths) => {
                        if paths.is_empty() || !self.settings.downloads_enabled {
                            continue;
                        }
                        let title = if paths.len() == 1 {
                            "Download ready".to_owned()
                        } else {
                            format!("{} downloads ready", paths.len())
                        };
                        self.post_notice(InternalNotice::with_files(
                            NoticeKind::Downloads,
                            &title,
                            "Folder arrival inferred \u{b7} drag originals or add to shelf",
                            paths,
                        ));
                    }
                    DownloadEvent::Failed(message) => {
                        if self.settings.downloads_enabled {
                            self.post_notice(InternalNotice::new(
                                NoticeKind::Failure,
                                "Downloads unavailable",
                                &message,
                            ));
                        }
                    }
                }
            }
        }

        fn pull_drops(&mut self) {
            let (dragging, dropped) = {
                let mut s = self.drops.borrow_mut();
                (s.dragging, std::mem::take(&mut s.dropped))
            };
            if dragging != self.files_drag {
                self.files_drag = dragging;
                self.overlay
                    .signal(Signal::FileDrag(dragging), &self.snaps, &mut self.out);
            }
            if !dropped.is_empty() {
                self.snaps.productivity.borrow_mut().add_files(dropped);
                self.files_drag = false;
                self.overlay
                    .signal(Signal::FileDrag(false), &self.snaps, &mut self.out);
            }
        }

        /// C# `CanShelfDrop`: files tool on, dock not hidden, and drop-reveal or the shelf shown.
        fn sync_drop_accept(&mut self) {
            let state = self.overlay.state();
            let accept = state != OverlayState::Hidden
                && self.settings.tool_enabled("files")
                && (self.settings.drop_reveal
                    || (state == OverlayState::Open && self.overlay.active_page() == "files")
                    || (matches!(state, OverlayState::Hover(_))
                        && self.overlay.peek_page() == "files"));
            self.drops.borrow_mut().accept = accept;
        }

        fn publish_awake(&mut self) {
            let now = Utc::now();
            let info = AwakeInfo {
                active: self.keep_awake.active(),
                keep_display_on: self.keep_awake.keep_display_on(),
                minutes_left: self
                    .keep_awake
                    .until()
                    .map(|u| ((u - now).num_seconds().max(0) as u32).div_ceil(60)),
            };
            self.snaps.awake.set(info);
        }

        fn publish_notices(&mut self) {
            let current = self.notifications.current().cloned();
            let changed = current != self.snaps.notices.current;
            if changed {
                self.notice_serial += 1;
            }
            self.snaps.notices.set(NoticeState {
                current,
                count: self.notifications.count(),
                serial: self.notice_serial,
            });
        }

        fn publish_session(&mut self) {
            self.snaps.session.set(self.session.clone());
        }

        // ---- notices --------------------------------------------------------------------

        fn post_notice(&mut self, notice: InternalNotice) {
            let hidden = self.overlay.user_hidden();
            let delivery = self
                .notifications
                .post(&self.settings, notice, hidden, Utc::now());
            for n in delivery.windows {
                self.actions.push(ShellAction::Balloon {
                    title: n.title,
                    detail: n.detail,
                });
            }
            if delivery.changed {
                self.publish_notices();
            }
        }

        fn fail(&mut self, title: &str, detail: &str) {
            self.post_notice(InternalNotice::new(NoticeKind::Failure, title, detail));
        }

        // ---- countdowns -----------------------------------------------------------------

        fn save_session(&mut self) {
            if let Err(error) = self.store.save_session(&mut self.session) {
                self.store.log(&error);
                self.fail("Not saved yet", "Could not save the timer state.");
            }
            self.publish_session();
        }

        fn play_sound(&mut self) {
            let s = &self.settings;
            self.sounds.play(&CountdownSoundConfig {
                sound: s.countdown_sound.clone(),
                volume: s.countdown_sound_volume,
                custom_file: s.countdown_sound_file.clone(),
            });
        }

        /// Completes finished timers and advances the focus cycle (`App.CheckCountdowns`).
        fn check_countdowns(&mut self) {
            let now = Utc::now();
            let config = focus_config(&self.settings);
            let report = check_countdowns(
                &mut self.session.timer,
                &mut self.session.focus,
                config,
                now,
            );
            if !report.changed() {
                return;
            }
            let quiet = self.settings.quiet(now);
            if report.timer_finished {
                self.post_notice(InternalNotice::new(
                    NoticeKind::Timer,
                    "Timer finished",
                    "Your timer has finished.",
                ));
                if self.settings.notify_timer && !quiet {
                    self.play_sound();
                }
            }
            if let Some(event) = report.focus {
                let (title, detail) = focus_message(event);
                self.post_notice(InternalNotice::new(NoticeKind::Focus, title, detail));
                if self.settings.notify_focus && !quiet {
                    self.play_sound();
                }
            }
            self.save_session();
        }

        // ---- demand ---------------------------------------------------------------------

        /// Starts or stops services to match what the views need right now.
        fn reconcile_demand(&mut self) {
            let wanted = self
                .overlay
                .demand(&self.snaps)
                .merge(self.detached.demand(&self.snaps));
            self.set_picker(wanted.catalog);
            if wanted == self.demand {
                return;
            }
            let old = self.demand;
            self.demand = wanted;
            // Loopback: peak only or the spectrum too.
            let need = if wanted.spectrum {
                Some(Need::Spectrum)
            } else if wanted.peak {
                Some(Need::Peak)
            } else {
                None
            };
            if self.lease.as_ref().map(|(n, _)| *n) != need {
                self.lease = need.map(|n| (n, self.loopback.acquire(n)));
                if self.lease.is_none() {
                    self.snaps.levels = AudioLevels::default();
                }
            }
            // Lyrics.
            if wanted.lyrics && !self.lyrics_owned {
                self.lyrics_owned = true;
                self.lyrics.acquire(LYRICS_OWNER);
                self.load_lyrics();
            } else if !wanted.lyrics && self.lyrics_owned {
                self.lyrics_owned = false;
                self.lyrics.release(LYRICS_OWNER);
                self.snaps.lyrics.set(self.lyrics.result().clone());
            }
            // Mixer.
            if wanted.mixer && self.mixer.is_none() {
                let mixer = MixerService::start(self.wake.clone());
                mixer.refresh();
                self.mixer = Some(mixer);
            } else if !wanted.mixer && old.mixer {
                self.mixer = None;
                self.snaps.mixer.set(Default::default());
            }
            // System monitor.
            if wanted.system && self.system_lease.is_none() {
                let s = &self.settings;
                self.system.set_options(SystemOptions::for_tab(
                    "overview",
                    s.monitor_network,
                    s.monitor_storage,
                    s.monitor_processes,
                ));
                self.system_lease = Some(self.system.lease());
            } else if !wanted.system {
                self.system_lease = None;
            }
            // Weather.
            if wanted.weather && !old.weather {
                self.weather_check_at = 0.0;
            }
        }

        fn weather_heartbeat(&mut self) {
            if !self.demand.weather || self.now() < self.weather_check_at {
                return;
            }
            self.weather_check_at = self.now() + WEATHER_CHECK;
            let shown = self.snaps.weather.snapshot.as_ref().map(|w| w.updated);
            let stale =
                lock(&self.weather).should_auto_refresh(Utc::now(), self.weather_busy, shown);
            if shown.is_none() || stale {
                self.request_weather(false);
            }
        }

        // ---- ticking --------------------------------------------------------------------

        fn pull_levels(&mut self) {
            if self.lease.is_none() {
                return;
            }
            let l = self.loopback.levels();
            self.snaps.levels = AudioLevels {
                bands: l.bands,
                peak: l.peak,
                sequence: l.band_sequence.wrapping_add(l.peak_sequence),
                spectrum_ready: l.spectrum_ready,
                meter_muted: l.muted,
            };
        }

        /// Ticks the overlay with the current snapshots and runs what it asked for.
        fn run_overlay(&mut self) -> bool {
            self.stamp();
            self.pull_levels();
            let mut changed = self.overlay.tick(&self.snaps, &mut self.out);
            changed |= self.execute_pending();
            self.reconcile_demand();
            self.weather_heartbeat();
            self.sync_drop_accept();
            self.run_pending_drag();
            changed |= self.pump_detached_windows();
            changed
        }

        fn execute_pending(&mut self) -> bool {
            let mut any = false;
            for _ in 0..8 {
                if self.out.is_empty() {
                    break;
                }
                let batch = std::mem::take(&mut self.out);
                for command in batch {
                    self.execute(command);
                    any = true;
                }
            }
            any
        }

        // ---- commands -------------------------------------------------------------------

        fn execute(&mut self, command: Command) {
            let now = Utc::now();
            match command {
                Command::OpenSettings => self.actions.push(ShellAction::OpenSettings(None)),
                Command::OpenSettingsPage(page) => {
                    self.actions.push(ShellAction::OpenSettings(Some(page)))
                }
                Command::Quit => self.actions.push(ShellAction::Quit),
                Command::OpenAssistant => self.actions.push(ShellAction::OpenAssistant),
                Command::OpenVoice => self.actions.push(ShellAction::OpenVoice),
                Command::NavigatePage(id) => {
                    if let Some(page) = static_page(&id) {
                        self.overlay
                            .signal(Signal::ShowPage(page), &self.snaps, &mut self.out);
                    }
                }
                Command::DetachPreview { page, at, .. } => {
                    // Pages that cannot be detached are ignored, as `CanOpen` does.
                    if self.detached.open(&page, Some(at), &self.snaps) {
                        self.repaint_requested = true;
                    }
                }
                Command::ToggleTuck => {
                    self.overlay
                        .signal(Signal::ToggleTuck, &self.snaps, &mut self.out);
                }
                Command::ToggleVisibility => {
                    self.overlay
                        .signal(Signal::ToggleVisibility, &self.snaps, &mut self.out);
                }
                Command::ApplySettings(settings) => self.apply_command_settings(*settings),
                Command::SetDockEditing(on) => self.dock_editing = on,
                Command::CollapseDock => {
                    self.overlay
                        .signal(Signal::Escape, &self.snaps, &mut self.out);
                }
                Command::DragFilesOut(paths) => {
                    // The OLE drag loop must not run inside a paint: do it after this pass.
                    self.pending_drag = Some(paths);
                    self.repaint_requested = true;
                }
                Command::PickShelfFiles => {
                    let files = dialogs::pick_files("Add to file shelf");
                    if !files.is_empty() {
                        self.snaps.productivity.borrow_mut().add_files(files);
                    }
                }
                Command::SaveClipImage { id } => {
                    if let Some(path) =
                        dialogs::save_png_as("Save clipboard image", "Clipboard image.png")
                    {
                        let saved = self
                            .snaps
                            .productivity
                            .borrow_mut()
                            .save_clip_image(&id, std::path::Path::new(&path));
                        if !saved {
                            self.fail("Could not save image", "Check the location and try again.");
                        }
                    }
                }
                Command::OpenAppPicker => self.set_picker(true),
                Command::CloseAppPicker => self.set_picker(false),
                // Settings-window requests are handled by the settings host.
                Command::Settings(_) => {}

                Command::MediaToggle => self.with_media(|m| m.toggle()),
                Command::MediaPlay => self.with_media(|m| m.play()),
                Command::MediaPause => self.with_media(|m| m.pause()),
                Command::MediaNext => self.with_media(|m| m.next()),
                Command::MediaPrevious => self.with_media(|m| m.previous()),
                Command::MediaSeek(fraction) => self.with_media(|m| m.seek(fraction)),
                Command::MediaSelectSource(source) => self.with_media(|m| match &source {
                    MediaSource::None => m.select_source(None),
                    MediaSource::Auto => m.select_automatic(),
                    MediaSource::App(id) => m.select_source(Some(id)),
                }),

                Command::SetVolume(v) => self.audio.set_volume(v),
                Command::ToggleMute => self.audio.toggle_mute(),
                Command::ToggleMicMute => self.audio.toggle_microphone_mute(),
                Command::SetOutput(id) => self.audio.set_output(&id),
                Command::RefreshAudio => self.audio.refresh(),
                Command::MixerRefresh => {
                    let wake = self.wake.clone();
                    self.mixer
                        .get_or_insert_with(|| MixerService::start(wake))
                        .refresh();
                }
                Command::MixerSet {
                    target,
                    level,
                    muted,
                } => {
                    if let Some(mixer) = &self.mixer {
                        mixer.set(target, level, muted);
                    }
                }
                Command::MixerAdjust { scope, delta } => {
                    if let Some(mixer) = &self.mixer {
                        mixer.adjust(scope, delta);
                    }
                }

                Command::TimerStart { seconds } => {
                    self.session.timer.start(seconds, now, "", false);
                    self.save_session();
                }
                Command::TimerToggle => {
                    timer_toggle(&mut self.session.timer, now);
                    self.save_session();
                }
                Command::TimerPause => {
                    self.session.timer.pause(now);
                    self.save_session();
                }
                Command::TimerResume => {
                    self.session.timer.resume(now);
                    self.save_session();
                }
                Command::TimerReset => {
                    self.session.timer.reset();
                    self.save_session();
                }
                Command::TimerExtend { seconds } => {
                    countdown_extend(&mut self.session.timer, now, seconds, "");
                    self.save_session();
                }
                Command::FocusToggle => {
                    let config = focus_config(&self.settings);
                    self.session.focus.toggle(config, now);
                    self.save_session();
                }
                Command::FocusReset => {
                    self.session.focus.reset();
                    self.save_session();
                }
                Command::FocusExtend { seconds } => {
                    countdown_extend(&mut self.session.focus.countdown, now, seconds, "work");
                    self.save_session();
                }
                Command::StopwatchToggle => {
                    self.session.stopwatch.toggle(Clock::system());
                    self.save_session();
                }
                Command::StopwatchReset => {
                    self.session.stopwatch.reset(Clock::system());
                    self.save_session();
                }

                Command::NoticeDisplayed => {
                    self.notifications.mark_displayed();
                }
                Command::NoticeDismiss => {
                    self.notifications.dismiss();
                    self.publish_notices();
                }
                Command::NoticeNext => {
                    self.notifications.dismiss();
                    self.publish_notices();
                    if self.notifications.current().is_some() {
                        self.overlay
                            .signal(Signal::OpenNotice, &self.snaps, &mut self.out);
                    }
                }
                Command::NoticeClearAll => {
                    self.notifications.clear();
                    self.publish_notices();
                }
                Command::Notify {
                    kind,
                    title,
                    detail,
                    files,
                } => {
                    let mut notice = InternalNotice::new(kind, &title, &detail);
                    notice.files = files;
                    self.post_notice(notice);
                }

                Command::OpenPath(path) => {
                    self.shell_result("Could not open", shell::open_path(&path))
                }
                Command::OpenUrl(url) => {
                    self.shell_result("Could not open link", shell::open_url(&url))
                }
                Command::RevealInFolder(path) => {
                    self.shell_result("Could not show file", shell::reveal_in_explorer(&path))
                }
                Command::CopyText(text) => {
                    let result = if self.clipboard.enabled() {
                        self.clipboard.set_text(&text)
                    } else if crate::text_clipboard::set_text(None, &text) {
                        Ok(())
                    } else {
                        Err("The clipboard is busy. Try again.".to_owned())
                    };
                    if let Err(message) = result {
                        self.fail("Could not copy", &message);
                    }
                }
                Command::CopyClipImage(id) => {
                    let bytes = self.snaps.productivity.borrow_mut().clip_image_bytes(&id);
                    match bytes {
                        Some(png) => {
                            if let Err(message) = self.clipboard.set_image(&png) {
                                self.fail("Could not copy", &message);
                            }
                        }
                        None => self.fail("Could not copy", "The clip image is unavailable."),
                    }
                }
                Command::ShelfAddFiles(paths) => {
                    self.snaps.productivity.borrow_mut().add_files(paths);
                }
                Command::LaunchShortcut { target, app_id } => {
                    self.shell_result("Could not open", shell::launch(&target, app_id.as_deref()))
                }

                Command::AwakeStart {
                    minutes,
                    keep_display_on,
                } => {
                    let duration = minutes.map(|m| Duration::minutes(i64::from(m)));
                    if let Err(error) = self.keep_awake.start(duration, keep_display_on, now) {
                        self.fail("Keep awake", &error.to_string());
                    }
                    self.publish_awake();
                }
                Command::AwakeStop => {
                    if let Err(error) = self.keep_awake.stop() {
                        self.fail("Keep awake", &error.to_string());
                    }
                    self.publish_awake();
                }

                Command::LyricsRetry => {
                    self.lyrics.retry();
                    self.load_lyrics();
                }
                Command::LyricsImport { path } => self.import_lyrics(&path),
                Command::LyricsPickFile => {
                    if let Some(path) = pick_lrc_file() {
                        self.import_lyrics(&path);
                    }
                }
                Command::WeatherRefresh { force } => self.request_weather(force),
                Command::WeatherSearch { query } => self.search_places(query),
                Command::SystemTab(tab) => {
                    let s = &self.settings;
                    self.system.set_options(SystemOptions::for_tab(
                        &tab,
                        s.monitor_network,
                        s.monitor_storage,
                        s.monitor_processes,
                    ));
                }
            }
        }

        fn with_media(&mut self, f: impl FnOnce(&MediaService)) {
            if let Some(media) = &self.media {
                f(media);
            }
        }

        fn shell_result(&mut self, title: &str, result: Result<(), shell::ShellError>) {
            if let Err(error) = result {
                self.fail(title, &error.0);
            }
        }

        fn import_lyrics(&mut self, path: &str) {
            if std::fs::metadata(path).is_ok_and(|m| m.len() > 512 * 1024) {
                self.fail(
                    "Could not import lyrics",
                    "Choose an LRC file under 512 KB.",
                );
                return;
            }
            let text = match std::fs::read_to_string(path) {
                Ok(text) => text,
                Err(error) => {
                    self.fail("Could not read lyrics", &error.to_string());
                    return;
                }
            };
            let media = self.snaps.media.clone();
            if let Err(message) =
                self.lyrics
                    .import(&media.title, &media.artist, media.timeline().1, &text)
            {
                self.fail("Could not import lyrics", message);
            } else if !self.settings.lyrics_enabled {
                // Loading a local file switches lyrics on (`LyricsFiles.Import`).
                let mut settings = self.settings.clone();
                settings.lyrics_enabled = true;
                self.apply_command_settings(settings);
            }
            self.snaps.lyrics.set(self.lyrics.result().clone());
        }

        fn apply_command_settings(&mut self, mut settings: Settings) {
            settings.normalize();
            // While the island is being moved or resized the steps stay in memory; the final
            // layout (the apply that locks the dock again) is saved and announced.
            let transient = self.dock_editing && settings.dock_unlocked;
            if !transient {
                match self.store.save_settings(&settings) {
                    Ok(()) => {}
                    Err(error) => {
                        self.store.log(&error);
                        self.fail("Not saved yet", "Could not save the settings.");
                    }
                }
            }
            self.apply_settings_inner(&settings);
            self.actions.push(if transient {
                ShellAction::SettingsPreview(Box::new(settings))
            } else {
                ShellAction::SettingsApplied(Box::new(settings))
            });
        }

        /// Runs a requested drag-out of shelf files (blocks in OLE's modal loop).
        fn run_pending_drag(&mut self) {
            let Some(paths) = self.pending_drag.take() else {
                return;
            };
            if dragdrop::drag_files_out(&paths) == dragdrop::DragOutcome::Failed {
                self.fail(
                    "Could not drag files",
                    "The files could not be dragged out.",
                );
            }
        }

        /// Input, requests and frames of the detached previews.
        fn pump_detached_windows(&mut self) -> bool {
            let hidden = self.overlay.user_hidden();
            self.detached.set_hidden(hidden);
            let presented = self.detached.pump(&self.snaps, &mut self.out);
            self.execute_pending();
            presented
        }
    }

    impl Surface for OverlayHost {
        fn paint(&mut self, canvas: &mut Canvas, fonts: &Fonts) {
            self.stamp();
            self.overlay.paint(canvas, fonts, &self.snaps);
            self.overlay.drain(&mut self.out);
            // Widgets run during paint; what they asked for may change what the next frame shows.
            if self.execute_pending() {
                self.repaint_requested = true;
            }
        }

        fn input(&mut self, input: &InputState) -> bool {
            let mut input = input.clone();
            input.time = self.now();
            self.stamp();
            let mut changed = self.overlay.input(&input, &self.snaps, &mut self.out);
            changed |= self.execute_pending();
            self.reconcile_demand();
            changed
        }

        fn size(&self) -> (u32, u32) {
            self.overlay.size(&self.snaps)
        }

        fn placement(&self) -> Option<(i32, i32)> {
            let p = self.overlay.placement(&self.snaps);
            (p.size.0 > 0).then_some(p.origin)
        }

        fn next_frame(&self) -> Option<Instant> {
            let now = self.now();
            let mut next = self.overlay.next_frame(now);
            if self.repaint_requested {
                next = Some(now);
            }
            let mut add = |seconds: f64| {
                next = Some(next.map_or(seconds, |n: f64| n.min(seconds)));
            };
            let utc = Utc::now();
            if let Some(deadline) =
                next_countdown_deadline(&self.session.timer, &self.session.focus.countdown)
            {
                add(now + (deadline - utc).num_milliseconds().max(0) as f64 / 1000.0 + 0.02);
            }
            if let Some(until) = self.keep_awake.until().filter(|_| self.keep_awake.active()) {
                add(now + (until - utc).num_milliseconds().max(0) as f64 / 1000.0 + 0.02);
            }
            if self.demand.weather {
                add(self.weather_check_at.max(now));
            }
            if let Some(t) = self.detached.next_frame(now) {
                add(t);
            }
            next.map(|t| self.base + StdDuration::from_secs_f64(t.max(0.0)))
        }

        fn apply_settings(&mut self, settings: &Settings) {
            self.apply_settings_inner(settings);
            self.pull_all();
            self.detached.apply_settings(&self.snaps);
            self.run_overlay();
        }

        fn wake(&mut self) -> bool {
            self.pull_all();
            self.run_overlay()
        }

        fn tick(&mut self) -> bool {
            self.stamp();
            self.repaint_requested = false;
            self.check_countdowns();
            if self.keep_awake.active() && self.keep_awake.tick(Utc::now()).is_ok() {
                self.publish_awake();
            }
            self.run_overlay()
        }

        fn set_display(&mut self, display: DisplayInfo) {
            self.snaps.display.set(display);
            self.run_overlay();
        }

        fn signal(&mut self, signal: Signal) -> bool {
            self.stamp();
            let mut changed = self.overlay.signal(signal, &self.snaps, &mut self.out);
            changed |= self.execute_pending();
            self.reconcile_demand();
            self.sync_drop_accept();
            self.pump_detached_windows();
            changed
        }

        fn pump_detached(&mut self) -> bool {
            self.stamp();
            let presented = self.pump_detached_windows();
            self.reconcile_demand();
            presented
        }

        fn wants_focus(&self) -> bool {
            self.overlay.wants_focus()
        }

        fn wants_outside_clicks(&self) -> bool {
            matches!(
                self.overlay.state(),
                OverlayState::Open | OverlayState::Hover(_) | OverlayState::Notice
            )
        }

        fn interaction_active(&self) -> bool {
            self.overlay.interaction_active() || self.detached.interaction_active()
        }

        fn on_hide(&mut self) {
            self.overlay.on_hide();
        }

        fn snapshots(&self) -> Option<Snapshots> {
            Some(self.snaps.clone())
        }

        fn take_actions(&mut self) -> Vec<ShellAction> {
            std::mem::take(&mut self.actions)
        }

        fn run_commands(&mut self, commands: Vec<Command>) -> bool {
            self.out.extend(commands);
            let changed = self.execute_pending();
            self.reconcile_demand();
            changed
        }

        fn set_assistant_summary(&mut self, summary: AssistantSummary) -> bool {
            self.snaps.assistant.set(summary)
        }
    }

    impl OverlayHost {
        /// Assistant state the closed dock reflects (voice, busy, sharing).
        pub fn set_assistant(&mut self, summary: AssistantSummary) {
            self.snaps.assistant.set(summary);
        }

        /// The overlay's state (tests, diagnostics).
        pub fn state(&self) -> OverlayState {
            self.overlay.state()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use naby_core::productivity::FocusSession;

    fn t0() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 29, 12, 0, 0).unwrap()
    }

    #[test]
    fn timer_toggle_starts_five_minutes_pauses_and_resumes() {
        let mut timer = Timer::default();
        timer_toggle(&mut timer, t0());
        assert!(timer.running());
        assert_eq!(timer.remaining(t0()), 300.0);
        timer_toggle(&mut timer, t0() + Duration::seconds(60));
        assert!(!timer.running());
        assert_eq!(timer.remaining(t0() + Duration::seconds(90)), 240.0);
        timer_toggle(&mut timer, t0() + Duration::seconds(120));
        assert!(timer.running());
        assert_eq!(timer.remaining(t0() + Duration::seconds(120)), 240.0);
    }

    #[test]
    fn extending_a_running_countdown_moves_its_deadline() {
        let mut timer = Timer::default();
        timer.start(100.0, t0(), "", false);
        countdown_extend(&mut timer, t0() + Duration::seconds(40), 300.0, "");
        assert_eq!(timer.remaining(t0() + Duration::seconds(40)), 360.0);
        assert_eq!(timer.duration_seconds, 400.0);
        assert_eq!(timer.deadline, Some(t0() + Duration::seconds(400)));
        // An idle countdown starts with the extension.
        let mut idle = Timer::default();
        countdown_extend(&mut idle, t0(), 300.0, "work");
        assert!(idle.running() && idle.phase == "work");
        assert_eq!(idle.remaining(t0()), 300.0);
    }

    #[test]
    fn a_paused_countdown_keeps_paused_when_extended() {
        let mut timer = Timer::default();
        timer.start(100.0, t0(), "", false);
        timer.pause(t0() + Duration::seconds(10));
        countdown_extend(&mut timer, t0() + Duration::seconds(20), 60.0, "");
        assert!(!timer.running());
        assert_eq!(timer.remaining(t0() + Duration::seconds(99)), 150.0);
    }

    #[test]
    fn the_next_deadline_is_the_earliest_running_one() {
        let mut timer = Timer::default();
        let mut focus = FocusSession::default();
        assert_eq!(next_countdown_deadline(&timer, &focus.countdown), None);
        timer.start(600.0, t0(), "", false);
        focus.start_work(FocusConfig::default(), t0());
        assert_eq!(
            next_countdown_deadline(&timer, &focus.countdown),
            Some(t0() + Duration::seconds(600))
        );
        timer.pause(t0());
        assert_eq!(
            next_countdown_deadline(&timer, &focus.countdown),
            Some(t0() + Duration::minutes(25))
        );
    }

    #[test]
    fn focus_messages_match_the_csharp_texts() {
        assert_eq!(
            focus_message(FocusEvent::BreakStarted),
            ("Focus complete", "Time for a short break.")
        );
        assert_eq!(focus_message(FocusEvent::BreakFinished).0, "Break finished");
        assert_eq!(
            focus_message(FocusEvent::SessionComplete).0,
            "Focus session complete"
        );
    }

    #[test]
    fn display_info_maps_monitor_geometry_and_scale() {
        let monitor = MonitorInfo {
            id: "\\\\.\\DISPLAY1".into(),
            bounds: crate::monitors::RectI::new(100, 50, 2660, 1490),
            work: crate::monitors::RectI::new(100, 50, 2660, 1450),
            dpi: 144,
            primary: true,
            handle: 0,
        };
        let d = display_info(&monitor, 144);
        assert_eq!(d.bounds, CoreRect::new(100, 50, 2560, 1440));
        assert_eq!(d.work, CoreRect::new(100, 50, 2560, 1400));
        assert_eq!(d.scale, 1.5);
        assert_eq!(display_info(&monitor, 0).refresh_hz, 60);
    }

    #[test]
    fn navigation_ids_resolve_to_static_pages() {
        assert_eq!(static_page("timer"), Some("timer"));
        assert_eq!(static_page("status"), Some("status"));
        assert_eq!(static_page("nope"), None);
    }

    #[test]
    fn focus_config_clamps_to_one_minute() {
        let s = naby_core::settings::Settings {
            focus_minutes: 0,
            break_minutes: -3,
            ..Default::default()
        };
        assert_eq!(
            focus_config(&s),
            FocusConfig {
                focus_minutes: 1,
                break_minutes: 1
            }
        );
    }
}
