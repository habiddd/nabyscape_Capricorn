//! The island dock: closed pill, hover peek carousel, open panel, notices and the states
//! between them (C# `OverlayWindow` and its partial files).
//!
//! `Overlay` implements [`View`](crate::host::View). Its state machine is driven by the host
//! through `input`, `signal` and `tick`; time only advances with `Snapshots::now`. Nothing is
//! scheduled while the dock is closed and static ([`Overlay::next_frame`] returns `None`).
//!
//! File map: `state` (transitions, hover/peek/tuck/notice policies), `surface` (glass and
//! ambient painting), `closed` (pill content), `hover` (peek carousel), `open` (panel and pages),
//! `notice` (notification card), `feedback` (volume/brightness pill), `wave` (waveform bars),
//! `util` (hit map and helpers).

mod closed;
pub mod detached;
mod dock_edit;
mod feedback;
mod hover;
mod layout;
mod notice;
mod open;
mod state;
mod surface;
pub mod util;
pub mod wave;

use crate::host::{Demand, Placement, Signal, Snapshots, View};
use crate::pages::Page;
use feedback::Feedback;
use naby_core::command::Command;
use naby_core::media::{AmbientEnvelope, CoverPalette};
use naby_core::motion::{
    AmbientLayer, Contour, DockState, HoverPolicy, HoverStage, MotionConfig, OverlaySettingsState,
    PanelNavigation, PanelSwipe, PeekWheelAccumulator, SurfaceMotion, TuckController, Tween,
};
use naby_render::{Canvas, Fonts, Image, InputState, Theme, ThumbnailCache, UiMemory};
use std::collections::HashMap;
use std::sync::Arc;
use util::{Hot, HotMap};
use wave::Wave;

/// What the dock is doing, for hosts and tests (C# `expanded` / `peeking` / `IsTucked` /
/// `userHidden` flags folded into one value).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayState {
    Closed,
    /// Hover peek: the 100 DIP strip (`Brief`) or the full card (`Detailed`).
    Hover(HoverStage),
    /// The notification card.
    Notice,
    Open,
    /// Slid into the screen edge; hovering reveals it.
    Tucked,
    /// Not shown at all (visibility hotkey, fullscreen or maximized app in front).
    Hidden,
}

impl OverlayState {
    /// The geometry state used for placement.
    pub fn dock_state(self) -> DockState {
        match self {
            OverlayState::Closed | OverlayState::Hidden => DockState::Closed,
            OverlayState::Hover(_) | OverlayState::Notice => DockState::Hover,
            OverlayState::Open => DockState::Open,
            OverlayState::Tucked => DockState::Tucked,
        }
    }
}

/// Settings-derived values rebuilt when the preferences change.
pub(crate) struct Cfg {
    pub motion: MotionConfig,
    pub theme: Theme,
    pub notch: bool,
    pub radius: f64,
}

#[derive(Default)]
pub(crate) struct DragState {
    /// Press position in canvas pixels.
    pub start: (f32, f32),
    /// Carousel offset (DIP) when the press began.
    pub origin: f64,
    pub active: bool,
}

pub(crate) struct Peek {
    /// Ids of the pages in the deck, in order.
    pub ids: Vec<String>,
    /// The page shown.
    pub id: String,
    /// Carousel offset in DIP; pages sit at `(i - index) * width + offset`.
    pub offset: Tween<f64>,
    pub drag: Option<DragState>,
    pub wheel: PeekWheelAccumulator,
    pub suppress_click: bool,
    /// A volume slider drag started on the media peek.
    pub slider_drag: bool,
    /// Brief-stage detail timer was armed for this peek.
    pub detailed: bool,
    /// Hover started from an auto preview (timer/media ending) rather than the pointer.
    pub automatic: bool,
    /// Preview kind chosen by an automatic preview.
    pub kind: String,
}

impl Default for Peek {
    fn default() -> Self {
        Peek {
            ids: Vec::new(),
            id: "media".to_owned(),
            offset: Tween::instant(0.0),
            drag: None,
            wheel: PeekWheelAccumulator::new(),
            suppress_click: false,
            slider_drag: false,
            detailed: false,
            automatic: false,
            kind: String::new(),
        }
    }
}

pub(crate) struct Panel {
    /// Page key (`media`, `focus`, `status`, tool id, ...).
    pub active: String,
    pub pages: HashMap<String, Box<dyn Page>>,
    pub memory: UiMemory,
    pub scroll: f32,
    pub content_height: f64,
    /// Input to feed the pages on the next paint.
    pub pending_input: Option<InputState>,
    pub nav: PanelNavigation,
    pub swipe: PanelSwipe,
    /// Hover-panel-only widget (a hover page opened a tool that is not enabled as a widget).
    pub forced_widget: Option<String>,
    pub wide: bool,
    /// Re-measure the page group on the next paint.
    pub measure: bool,
    /// Content height beyond the viewport, device pixels.
    pub overflow: f32,
    pub last_input: InputState,
}

impl Default for Panel {
    fn default() -> Self {
        Panel {
            active: "media".to_owned(),
            pages: HashMap::new(),
            memory: UiMemory::new(),
            scroll: 0.0,
            content_height: 0.0,
            pending_input: None,
            nav: PanelNavigation::new(),
            swipe: PanelSwipe::new(),
            forced_widget: None,
            wide: false,
            measure: false,
            overflow: 0.0,
            last_input: InputState::default(),
        }
    }
}

/// The notice currently drawn.
#[derive(Default)]
pub(crate) struct NoticeCard {
    pub serial: u64,
    pub entry: Option<naby_core::motion::NoticeEntry>,
    /// Surface size the card wants (DIP).
    pub size: (f64, f64),
    pub showing: bool,
    pub scroll: f32,
}

/// Closed-dock caches.
pub(crate) struct Closed {
    pub wave: Wave,
    pub leading: Wave,
    pub text: String,
    pub previous_text: String,
    pub text_changed_at: f64,
    pub lyric: naby_core::media::ClosedLyrics,
    pub peek_wave: Wave,
    pub last_sample: f64,
    pub last_sequence: u64,
    pub last_ambient: f64,
    pub last_ambient_sequence: u64,
}

impl Default for Closed {
    fn default() -> Self {
        Closed {
            wave: Wave::new(true, true),
            leading: Wave::new(false, false),
            text: String::new(),
            previous_text: String::new(),
            text_changed_at: f64::NEG_INFINITY,
            lyric: Default::default(),
            peek_wave: Wave::new(true, false),
            last_sample: 0.0,
            last_sequence: 0,
            last_ambient: 0.0,
            last_ambient_sequence: 0,
        }
    }
}

pub struct Overlay {
    pub(crate) now: f64,
    pub(crate) cfg: Cfg,
    pub(crate) settings_version: u64,
    pub(crate) display_version: u64,
    pub(crate) revision: u64,
    pub(crate) scale: f32,

    // ---- visibility / modes
    pub(crate) user_hidden: bool,
    pub(crate) env_hidden: bool,
    pub(crate) expanded: bool,
    pub(crate) peeking: bool,
    pub(crate) notice_showing: bool,
    pub(crate) tuck_preview: bool,
    pub(crate) file_drag: bool,
    pub(crate) focus_wanted: bool,
    pub(crate) editing_dock: bool,
    pub(crate) dock_edit: dock_edit::DockEdit,
    pub(crate) tuck: TuckController,

    // ---- surface
    pub(crate) motion: SurfaceMotion,
    pub(crate) surf: (f64, f64),
    pub(crate) lift: f64,
    pub(crate) host_h: f64,
    pub(crate) compact_fade: Tween<f64>,
    pub(crate) peek_fade: Tween<f64>,
    pub(crate) panel_fade: Tween<f64>,
    pub(crate) panel_slide: Tween<f64>,
    pub(crate) page_fade: Tween<f64>,
    pub(crate) page_slide: Tween<f64>,
    pub(crate) contour: Option<((i64, i64, i64, bool), Contour)>,
    pub(crate) light: (f64, f64),
    pub(crate) ambient: AmbientLayer,
    pub(crate) ambient_env: AmbientEnvelope,
    pub(crate) palette: CoverPalette,
    pub(crate) palette_key: usize,
    pub(crate) tint: naby_core::color::Rgba,

    // ---- pointer / hover
    pub(crate) hover: HoverPolicy,
    pub(crate) pointer: Option<(f32, f32)>,
    pub(crate) pointer_inside: bool,
    pub(crate) pressed: Option<Hot>,
    pub(crate) hot: HotMap,
    pub(crate) last_paint_origin: (f32, f32),

    // ---- content
    pub(crate) peek: Peek,
    pub(crate) panel: Panel,
    pub(crate) notice: NoticeCard,
    pub(crate) closed: Closed,
    pub(crate) feedback: Feedback,
    pub(crate) art: ThumbnailCache,
    pub(crate) art_image: Option<(usize, Arc<Image>)>,
    pub(crate) outbox: Vec<Command>,
    pub(crate) dirty: bool,
    pub(crate) seen_notice_serial: u64,
    pub(crate) target_size: (f64, f64),
    pub(crate) settings_state: Option<OverlaySettingsState>,
    pub(crate) activity_until: f64,
    pub(crate) notice_deadline: f64,
    pub(crate) notice_dismiss_sent: bool,
    pub(crate) notice_pending: bool,
    pub(crate) feedback_was_visible: bool,
    pub(crate) last_track: String,
    pub(crate) warned_timer: Option<chrono::DateTime<chrono::Utc>>,
    pub(crate) warned_focus: Option<chrono::DateTime<chrono::Utc>>,
    pub(crate) plan: closed::ClosedPlan,
    pub(crate) wall_frac: f64,
    pub(crate) wall_second: u32,
    pub(crate) closed_wave_visible: bool,
    pub(crate) closed_icon_wave: bool,
    pub(crate) wants_closed_lyrics: bool,
    pub(crate) countdown_visible: bool,
    pub(crate) clock_visible: bool,
    pub(crate) lyrics_ticking: bool,
    pub(crate) panel_ticks: bool,
    pub(crate) live_work: bool,
    pub(crate) shown_ambient: bool,
    pub(crate) lyric_anim_until: f64,
    pub(crate) retarget_pending: bool,
}

impl Overlay {
    pub fn new(snaps: &Snapshots) -> Overlay {
        let mut overlay = Overlay {
            now: snaps.now,
            cfg: Cfg {
                motion: MotionConfig::default(),
                theme: Theme::default(),
                notch: false,
                radius: 16.0,
            },
            settings_version: 0,
            display_version: 0,
            revision: 0,
            scale: 1.0,
            user_hidden: false,
            env_hidden: false,
            expanded: false,
            peeking: false,
            notice_showing: false,
            tuck_preview: false,
            file_drag: false,
            focus_wanted: false,
            editing_dock: false,
            dock_edit: dock_edit::DockEdit::default(),
            tuck: TuckController::new(),
            motion: SurfaceMotion::new(),
            surf: (snaps.settings.width, snaps.settings.height),
            lift: 0.0,
            host_h: snaps.settings.height,
            compact_fade: Tween::instant(1.0),
            peek_fade: Tween::instant(0.0),
            panel_fade: Tween::instant(0.0),
            panel_slide: Tween::instant(-8.0),
            page_fade: Tween::instant(1.0),
            page_slide: Tween::instant(0.0),
            contour: None,
            light: (0.5, 0.25),
            ambient: AmbientLayer::new(),
            ambient_env: AmbientEnvelope::new(),
            palette: CoverPalette::NONE,
            palette_key: 0,
            tint: naby_core::color::Rgba::TRANSPARENT,
            hover: HoverPolicy::new(),
            pointer: None,
            pointer_inside: false,
            pressed: None,
            hot: HotMap::default(),
            last_paint_origin: (0.0, 0.0),
            peek: Peek::default(),
            panel: Panel::default(),
            notice: NoticeCard::default(),
            closed: Closed::default(),
            feedback: Feedback::default(),
            art: ThumbnailCache::new(2 * 1024 * 1024),
            art_image: None,
            outbox: Vec::new(),
            dirty: true,
            seen_notice_serial: snaps.notices.serial,
            target_size: (snaps.settings.width, snaps.settings.height),
            settings_state: None,
            activity_until: f64::INFINITY,
            notice_deadline: f64::INFINITY,
            notice_dismiss_sent: false,
            notice_pending: false,
            feedback_was_visible: false,
            last_track: String::new(),
            warned_timer: None,
            warned_focus: None,
            plan: closed::ClosedPlan::default(),
            wall_frac: 0.0,
            wall_second: 0,
            closed_wave_visible: false,
            closed_icon_wave: false,
            wants_closed_lyrics: false,
            countdown_visible: false,
            clock_visible: false,
            lyrics_ticking: false,
            panel_ticks: false,
            live_work: false,
            shown_ambient: false,
            lyric_anim_until: 0.0,
            retarget_pending: false,
        };
        overlay.sync(snaps);
        overlay
            .motion
            .snap(naby_core::motion::Channel::SurfaceWidth, overlay.surf.0);
        overlay
            .motion
            .snap(naby_core::motion::Channel::SurfaceHeight, overlay.surf.1);
        overlay
    }

    /// The state for hosts and tests.
    pub fn state(&self) -> OverlayState {
        if self.user_hidden || self.env_hidden {
            OverlayState::Hidden
        } else if self.expanded {
            OverlayState::Open
        } else if self.notice_showing {
            OverlayState::Notice
        } else if self.peeking {
            OverlayState::Hover(if self.peek.detailed {
                HoverStage::Detailed
            } else {
                HoverStage::Brief
            })
        } else if self.tuck.is_tucked() {
            OverlayState::Tucked
        } else {
            OverlayState::Closed
        }
    }

    /// Hidden by the visibility hotkey (not by a fullscreen app).
    pub fn user_hidden(&self) -> bool {
        self.user_hidden
    }

    pub fn is_expanded(&self) -> bool {
        self.expanded
    }

    pub fn is_peeking(&self) -> bool {
        self.peeking
    }

    pub fn is_tucked(&self) -> bool {
        self.tuck.is_tucked()
    }

    /// The panel page key shown (`media`, `focus`, `status`, ...).
    pub fn active_page(&self) -> &str {
        &self.panel.active
    }

    /// The hover deck page shown.
    pub fn peek_page(&self) -> &str {
        &self.peek.id
    }

    /// Current painted surface size in DIP.
    pub fn surface_size(&self) -> (f64, f64) {
        self.surf
    }

    /// Whether any surface animation is running.
    pub fn animating(&self) -> bool {
        self.motion.active() || self.tuck.moving()
    }

    /// Pointer inside the visible contour (C# `PointerInside`).
    pub fn pointer_inside(&self) -> bool {
        self.pointer_inside
    }
}

impl View for Overlay {
    fn input(&mut self, input: &InputState, snaps: &Snapshots, out: &mut Vec<Command>) -> bool {
        self.sync(snaps);
        self.dock_edit_sync(snaps);
        let (consumed, edited) = self.dock_edit_input(input, snaps);
        let changed = if consumed {
            edited
        } else {
            self.handle_input(input, snaps) | edited
        };
        self.flush(out);
        changed
    }

    fn signal(&mut self, signal: Signal, snaps: &Snapshots, out: &mut Vec<Command>) -> bool {
        self.sync(snaps);
        self.dock_edit_sync(snaps);
        let changed = if self.dock_edit_signal(signal, snaps) {
            true
        } else {
            self.handle_signal(signal, snaps)
        };
        self.flush(out);
        changed
    }

    fn tick(&mut self, snaps: &Snapshots, out: &mut Vec<Command>) -> bool {
        self.sync(snaps);
        self.dock_edit_sync(snaps);
        let changed = self.advance(snaps);
        self.flush(out);
        changed
    }

    fn paint(&mut self, canvas: &mut Canvas, fonts: &Fonts, snaps: &Snapshots) {
        self.sync(snaps);
        self.dock_edit_sync(snaps);
        self.paint_all(canvas, fonts, snaps);
        self.paint_dock_edit(canvas, fonts, snaps);
        self.dirty = false;
    }

    fn size(&self, snaps: &Snapshots) -> (u32, u32) {
        self.placement(snaps).size
    }

    fn placement(&self, snaps: &Snapshots) -> Placement {
        self.host_placement(snaps)
    }

    fn next_frame(&self, now: f64) -> Option<f64> {
        self.next_wake(now)
    }

    fn demand(&self, snaps: &Snapshots) -> Demand {
        self.current_demand(snaps)
    }

    fn wants_focus(&self) -> bool {
        // The dock editing mode reads the arrow keys, Enter and Escape.
        (self.expanded || self.dock_edit.active) && !self.user_hidden && !self.env_hidden
    }

    fn interaction_active(&self) -> bool {
        self.pressed.is_some()
            || self.peek.drag.as_ref().is_some_and(|d| d.active)
            || self.peek.slider_drag
            || self.panel.swipe.active()
            || self.editing_dock
    }

    fn drain(&mut self, out: &mut Vec<Command>) {
        self.flush(out);
    }

    fn on_hide(&mut self) {
        self.release_buffers();
    }
}

impl Overlay {
    pub(crate) fn flush(&mut self, out: &mut Vec<Command>) {
        out.append(&mut self.outbox);
    }

    pub(crate) fn push(&mut self, command: Command) {
        self.outbox.push(command);
    }
}

#[cfg(test)]
pub(crate) mod tests;
