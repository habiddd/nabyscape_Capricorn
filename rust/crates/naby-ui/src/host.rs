//! The seam between the platform host and every view.
//!
//! * [`Snapshots`] is everything a view reads. Fields are [`Snap`]s (cheap `Arc` clones with a
//!   version counter bumped only when the value really changed), so a view repaints when
//!   `snaps.revision()` moved or when one field's `version()` did.
//! * [`View`] is what the host drives: `input` (pointer/keys), `signal` (hotkeys, environment),
//!   `tick` (advance time and absorb new snapshots), `paint`, and the geometry/scheduling queries.
//!   Times are seconds on the host's monotonic clock (`Snapshots::now`, `InputState::time`).
//! * Productivity data and the local calendar are shared handles ([`ProductivityHandle`],
//!   [`CalendarHandle`]) that views mutate directly (see `naby_core::command`); everything that
//!   touches the OS is a [`Command`].

use chrono::{DateTime, Local};
use naby_core::command::Command;
use naby_core::media::{AudioSnapshot, LyricResult, MediaSnapshot, MixerSnapshot, BANDS};
use naby_core::motion::RectI;
use naby_core::notifications::InternalNotice;
use naby_core::productivity::{Calendar, CalendarError, CalendarEvent, ProductivityService};
use naby_core::session::SessionState;
use naby_core::settings::Settings;
use naby_core::system::battery::BatterySnapshot;
use naby_core::system::MonitorSnapshot;
use naby_core::weather::{WeatherPlace, WeatherSnapshot};
use naby_render::{Canvas, Fonts, InputState};
use std::cell::{Cell, RefCell};
use std::ops::Deref;
use std::rc::Rc;
use std::sync::Arc;

pub use naby_core::command::MediaSource;

// ---- versioned values -----------------------------------------------------------------

/// A value shared by `Arc` plus a version that moves only when the value changes.
#[derive(Debug)]
pub struct Snap<T> {
    value: Arc<T>,
    version: u64,
}

impl<T> Clone for Snap<T> {
    fn clone(&self) -> Self {
        Snap {
            value: Arc::clone(&self.value),
            version: self.version,
        }
    }
}

impl<T: Default> Default for Snap<T> {
    fn default() -> Self {
        Snap::new(T::default())
    }
}

impl<T> Snap<T> {
    pub fn new(value: T) -> Self {
        Snap {
            value: Arc::new(value),
            version: 1,
        }
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn arc(&self) -> &Arc<T> {
        &self.value
    }

    /// Replaces the value and always bumps the version (for types without `PartialEq`).
    pub fn replace(&mut self, value: T) {
        self.value = Arc::new(value);
        self.version += 1;
    }
}

impl<T: PartialEq> Snap<T> {
    /// Stores `value` when it differs; returns whether it did.
    pub fn set(&mut self, value: T) -> bool {
        if *self.value == value {
            return false;
        }
        self.replace(value);
        true
    }
}

impl<T> Deref for Snap<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.value
    }
}

// ---- snapshot payloads ----------------------------------------------------------------

/// The monitor the dock lives on (physical pixels; `scale` 1.0 = 96 dpi).
#[derive(Debug, Clone, PartialEq)]
pub struct DisplayInfo {
    pub id: String,
    pub bounds: RectI,
    pub work: RectI,
    pub scale: f32,
    pub refresh_hz: u32,
}

impl Default for DisplayInfo {
    fn default() -> Self {
        DisplayInfo {
            id: String::new(),
            bounds: RectI::new(0, 0, 1920, 1080),
            work: RectI::new(0, 0, 1920, 1040),
            scale: 1.0,
            refresh_hz: 60,
        }
    }
}

/// Measured output levels, refreshed by the host on animation frames while a view asks for them
/// (`Demand::spectrum` / `Demand::peak`). Not versioned: compare `sequence`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AudioLevels {
    pub bands: [f32; BANDS],
    /// Peak of the media source's audio sessions, 0..=1.
    pub peak: f32,
    /// Increments whenever `bands`/`peak` were refreshed.
    pub sequence: u64,
    /// `bands` hold a real measurement (loopback capture is running).
    pub spectrum_ready: bool,
    /// The media session is muted at the mixer: draw silence.
    pub meter_muted: bool,
}

impl Default for AudioLevels {
    fn default() -> Self {
        AudioLevels {
            bands: [0.0; BANDS],
            peak: 0.0,
            sequence: 0,
            spectrum_ready: false,
            meter_muted: false,
        }
    }
}

/// Weather as pages show it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct WeatherState {
    pub snapshot: Option<WeatherSnapshot>,
    pub status: WeatherStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum WeatherStatus {
    /// Nothing requested yet or no city chosen.
    #[default]
    Idle,
    Loading,
    /// The last fetch failed; the message is user facing.
    Failed(String),
}

/// City search for the weather location editor.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PlaceSearch {
    /// What was asked for.
    pub query: String,
    pub state: PlaceSearchState,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub enum PlaceSearchState {
    #[default]
    Idle,
    Searching,
    /// Up to eight places (possibly none).
    Found(Vec<WeatherPlace>),
    /// Offline or the lookup failed.
    Failed,
}

/// One media session the source picker offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaSourceChoice {
    /// App user model id.
    pub id: String,
    pub name: String,
}

/// The sessions to choose a media source from and which choice is active (`@auto`, `@none` or
/// an app id).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaSources {
    pub sessions: Vec<MediaSourceChoice>,
    pub selected: String,
}

impl Default for MediaSources {
    fn default() -> Self {
        MediaSources {
            sessions: Vec::new(),
            selected: naby_core::media::source::AUTO_ID.to_owned(),
        }
    }
}

/// One network adapter's current rates in bytes/second.
#[derive(Debug, Clone, PartialEq)]
pub struct NetRates {
    pub id: String,
    pub name: String,
    pub received: Option<f64>,
    pub sent: Option<f64>,
}

/// System monitor data (CPU percent is computed by the host from consecutive samples).
#[derive(Debug, Clone, Default)]
pub struct SystemInfo {
    pub latest: Option<Arc<MonitorSnapshot>>,
    pub cpu: Option<f64>,
    pub cpu_history: Vec<Option<f64>>,
    pub networks: Vec<NetRates>,
    /// Number of samples taken so far (the change marker).
    pub reads: u64,
}

impl PartialEq for SystemInfo {
    fn eq(&self, other: &Self) -> bool {
        self.reads == other.reads
            && self.cpu == other.cpu
            && match (&self.latest, &other.latest) {
                (None, None) => true,
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                _ => false,
            }
    }
}

/// The dock-visible notice queue.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct NoticeState {
    pub current: Option<InternalNotice>,
    pub count: usize,
    /// Increments each time a new front notice appears (even with identical text).
    pub serial: u64,
}

/// Brightness change feedback source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BrightnessState {
    pub level: Option<u8>,
    pub sequence: u64,
}

/// Assistant state as the dock needs it (closed-dock label, character, reply notice).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AssistantSummary {
    pub voice_connected: bool,
    pub busy: bool,
    pub sharing: bool,
    /// "Listening", "Thinking…", "Speaking", "Connecting…", "Voice needs attention" or empty.
    pub label: String,
    /// A reply is waiting to be shown in the dock.
    pub reply: Option<String>,
}

impl AssistantSummary {
    /// `ClosedVoiceActivity`.
    pub fn closed_activity(&self) -> bool {
        self.voice_connected || self.busy || self.sharing
    }
}

/// Keep-awake state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AwakeInfo {
    pub active: bool,
    pub keep_display_on: bool,
    /// Seconds from the snapshot's `now` until it stops, `None` for "until NabyScape quits".
    pub minutes_left: Option<u32>,
}

// ---- shared handles -------------------------------------------------------------------

/// Notes, tasks, shelf and clipboard history. Mutate directly; the service saves itself.
/// Compare [`ProductivityService::revision`] (mirrored in `Snapshots::productivity_revision`)
/// to know when to repaint.
pub type ProductivityHandle = Rc<RefCell<ProductivityService>>;

type CalendarSaver = Box<dyn FnMut(&[CalendarEvent]) -> Result<(), String>>;

/// The local calendar with its persistence. Mutations validate and save before they apply.
pub struct CalendarHandle {
    calendar: RefCell<Calendar>,
    saver: RefCell<CalendarSaver>,
    revision: Cell<u64>,
}

impl CalendarHandle {
    pub fn new(calendar: Calendar, saver: CalendarSaver) -> Rc<Self> {
        Rc::new(CalendarHandle {
            calendar: RefCell::new(calendar),
            saver: RefCell::new(saver),
            revision: Cell::new(1),
        })
    }

    /// A calendar that never persists (tests, previews).
    pub fn in_memory() -> Rc<Self> {
        Self::new(Calendar::default(), Box::new(|_| Ok(())))
    }

    pub fn revision(&self) -> u64 {
        self.revision.get()
    }

    pub fn events(&self) -> Vec<CalendarEvent> {
        self.calendar.borrow().events().to_vec()
    }

    pub fn add(
        &self,
        title: &str,
        start: chrono::DateTime<chrono::FixedOffset>,
        end: chrono::DateTime<chrono::FixedOffset>,
        meeting_url: &str,
    ) -> Result<CalendarEvent, CalendarError> {
        let mut saver = self.saver.borrow_mut();
        let result = self
            .calendar
            .borrow_mut()
            .add_with(title, start, end, meeting_url, |e| (saver)(e));
        if result.is_ok() {
            self.revision.set(self.revision.get() + 1);
        }
        result
    }

    pub fn remove(&self, id: &str) -> Result<bool, CalendarError> {
        let mut saver = self.saver.borrow_mut();
        let result = self.calendar.borrow_mut().remove_with(id, |e| (saver)(e));
        if matches!(result, Ok(true)) {
            self.revision.set(self.revision.get() + 1);
        }
        result
    }
}

// ---- Snapshots ------------------------------------------------------------------------

/// Everything views read. Cloning is cheap (Arc bumps). The host owns one instance, mutates its
/// fields as services publish, and passes `&Snapshots` to every `View` call.
#[derive(Clone)]
pub struct Snapshots {
    /// Seconds on the host's monotonic clock (same clock as `InputState::time`).
    pub now: f64,
    /// Wall clock for the clock page, closed-dock clock and date.
    pub wall: DateTime<Local>,
    pub settings: Snap<Settings>,
    pub display: Snap<DisplayInfo>,
    pub media: Snap<MediaSnapshot>,
    /// Host-clock time at which `media.position` was exact; the timeline extrapolates from it.
    pub media_epoch: f64,
    pub audio: Snap<AudioSnapshot>,
    pub mixer: Snap<MixerSnapshot>,
    pub lyrics: Snap<LyricResult>,
    pub levels: AudioLevels,
    pub weather: Snap<WeatherState>,
    /// City search results of the weather location editor.
    pub places: Snap<PlaceSearch>,
    /// Media sessions for the source picker.
    pub sources: Snap<MediaSources>,
    pub system: Snap<SystemInfo>,
    pub battery: Snap<BatterySnapshot>,
    pub session: Snap<SessionState>,
    pub notices: Snap<NoticeState>,
    pub brightness: Snap<BrightnessState>,
    pub assistant: Snap<AssistantSummary>,
    pub awake: Snap<AwakeInfo>,
    pub productivity: ProductivityHandle,
    pub productivity_revision: u64,
    pub calendar: Rc<CalendarHandle>,
    /// Bumped by the host when system-level environment facts the views cache changed (fonts,
    /// high-contrast) — rarely used.
    pub environment: u64,
}

impl Snapshots {
    /// Snapshots with every service "unavailable" and in-memory productivity/calendar.
    pub fn new(settings: Settings, now: f64) -> Snapshots {
        let dir = std::env::temp_dir().join("nabyscape-ui-detached");
        Snapshots {
            now,
            wall: Local::now(),
            settings: Snap::new(settings),
            display: Snap::default(),
            media: Snap::default(),
            media_epoch: now,
            audio: Snap::default(),
            mixer: Snap::default(),
            lyrics: Snap::new(LyricResult {
                lines: Default::default(),
                status: naby_core::media::lyrics::STATUS_OFF.to_owned(),
            }),
            levels: AudioLevels::default(),
            weather: Snap::default(),
            places: Snap::default(),
            sources: Snap::default(),
            system: Snap::default(),
            battery: Snap::new(BatterySnapshot::UNAVAILABLE),
            session: Snap::default(),
            notices: Snap::default(),
            brightness: Snap::default(),
            assistant: Snap::default(),
            awake: Snap::default(),
            productivity: Rc::new(RefCell::new(ProductivityService::new(
                &dir,
                Default::default(),
            ))),
            productivity_revision: 0,
            calendar: CalendarHandle::in_memory(),
            environment: 0,
        }
    }

    /// Sum of every version: it moves whenever any snapshot changed, so
    /// `if snaps.revision() != self.seen { repaint }` is a complete change check.
    pub fn revision(&self) -> u64 {
        self.settings.version()
            + self.display.version()
            + self.media.version()
            + self.audio.version()
            + self.mixer.version()
            + self.lyrics.version()
            + self.weather.version()
            + self.places.version()
            + self.sources.version()
            + self.system.version()
            + self.battery.version()
            + self.session.version()
            + self.notices.version()
            + self.brightness.version()
            + self.assistant.version()
            + self.awake.version()
            + self.productivity_revision
            + self.calendar.revision()
            + self.environment
    }

    /// `(elapsed, duration)` seconds of the media timeline at `self.now`, extrapolating a
    /// playing track from the snapshot instant (`MediaService.Timeline`).
    pub fn media_timeline(&self) -> (f64, f64) {
        let (elapsed, duration) = self.media.timeline();
        if !self.media.playing || duration <= 0.0 {
            return (elapsed, duration);
        }
        let since = (self.now - self.media_epoch).max(0.0);
        ((elapsed + since).min(duration), duration)
    }

    /// Whether a main-panel widget is switched on.
    pub fn widget_on(&self, id: &str) -> bool {
        self.settings.widget_enabled(id)
    }
}

// ---- View -----------------------------------------------------------------------------

/// What a view needs the host to keep running for it. The host reads it after every call and
/// starts/stops the matching service, so nothing samples while nothing is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Demand {
    /// Loopback spectrum bands (waveform bars, ambient reactive glow).
    pub spectrum: bool,
    /// Media peak meter only.
    pub peak: bool,
    /// Load lyrics for the current track.
    pub lyrics: bool,
    /// Keep the mixer snapshot fresh.
    pub mixer: bool,
    /// Hold a `SystemLease` (CPU/RAM sampling); the host sets options from the active tab.
    pub system: bool,
    /// Fetch and refresh the weather.
    pub weather: bool,
}

impl Demand {
    pub fn merge(self, other: Demand) -> Demand {
        Demand {
            spectrum: self.spectrum || other.spectrum,
            peak: self.peak || other.peak,
            lyrics: self.lyrics || other.lyrics,
            mixer: self.mixer || other.mixer,
            system: self.system || other.system,
            weather: self.weather || other.weather,
        }
    }
}

/// Things the host tells a view that are not pointer or keyboard input.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Signal {
    /// The panel hotkey or the tray "Open island" entry: reveal and open the panel.
    Panel,
    /// The visibility hotkey / tray "Hide / show".
    ToggleVisibility,
    /// The tuck hotkey / tray "Tuck".
    ToggleTuck,
    /// A fullscreen/maximized foreground window should hide the dock (or no longer does).
    EnvironmentHidden(bool),
    /// A mouse press landed outside the dock window (screen coordinates translated to window
    /// pixels; may be negative). Collapses an open panel or peek unless the point is inside.
    OutsideClick { x: f32, y: f32 },
    /// The window lost keyboard focus it held while open.
    FocusLost,
    /// Monitors or work areas changed; recompute geometry.
    DisplayChanged,
    /// Hover peek / panel should show a given page (tray, assistant navigation).
    ShowPage(&'static str),
    /// Files are being dragged over the dock (drop-reveal); `false` when the drag ended or left.
    FileDrag(bool),
    /// Show the next queued notice card (after the previous one was dismissed with "Next").
    OpenNotice,
    /// Escape (also available through `InputState::keys`).
    Escape,
}

/// Where the host should put the layered window and how big it is, in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    pub origin: (i32, i32),
    pub size: (u32, u32),
}

/// A dock surface the host paints into its layered window.
///
/// Call order per frame: `input`/`signal` as events arrive, `tick` (snapshots changed or the
/// `next_frame` time arrived), then if anything returned `true`: `placement`/`size`, `paint`.
/// After each call the host re-reads `next_frame` and arms one timer for it (or none).
pub trait View {
    /// Pointer/keyboard for this frame (`input.time` is the host clock). True = repaint.
    fn input(&mut self, input: &InputState, snaps: &Snapshots, out: &mut Vec<Command>) -> bool;
    /// Host events (hotkeys, fullscreen, outside clicks). True = repaint.
    fn signal(&mut self, signal: Signal, snaps: &Snapshots, out: &mut Vec<Command>) -> bool;
    /// Advances animations and timers to `snaps.now` and absorbs changed snapshots.
    fn tick(&mut self, snaps: &Snapshots, out: &mut Vec<Command>) -> bool;
    /// Draws the whole window; `canvas` is exactly `size()` pixels and cleared to transparent.
    fn paint(&mut self, canvas: &mut Canvas, fonts: &Fonts, snaps: &Snapshots);
    /// Window size in physical pixels (`0 x 0` while hidden).
    fn size(&self, snaps: &Snapshots) -> (u32, u32);
    /// Top-left of the window in screen pixels and its size.
    fn placement(&self, snaps: &Snapshots) -> Placement;
    /// When the view next wants `tick` + `paint` (animation, countdown, hover delay);
    /// `None` when idle: the host arms no timer.
    fn next_frame(&self, now: f64) -> Option<f64>;
    /// Services the view needs right now.
    fn demand(&self, _snaps: &Snapshots) -> Demand {
        Demand::default()
    }
    /// The window should take keyboard focus (panel open, text field focused).
    fn wants_focus(&self) -> bool {
        false
    }
    /// A drag, text edit or pressed control is in progress: the host must not move the window
    /// to another monitor and notifications stay up.
    fn interaction_active(&self) -> bool {
        false
    }
    /// Commands produced while painting (pages run their widgets during `paint`); the host
    /// calls this after every paint.
    fn drain(&mut self, _out: &mut Vec<Command>) {}
    /// The window is hidden or tucked away: drop pixel buffers, caches and focus.
    fn on_hide(&mut self);
}
