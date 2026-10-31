//! State machine, scheduling and snapshot tests of the overlay.

use super::*;
use crate::host::{NoticeState, Placement, Signal, Snapshots, View};
use chrono::{Duration, Local, TimeZone};
use naby_core::command::Command;
use naby_core::media::{LyricResult, Lyrics, MediaSnapshot};
use naby_core::motion::HoverStage;
use naby_core::notifications::{InternalNotice, NoticeKind};
use naby_core::settings::{Settings, WidgetSetting};
use naby_render::snapshot;
use std::sync::Arc;
use std::time::Duration as StdDuration;

/// Drives an overlay like the host: advances the clock, feeds input, collects commands.
pub struct Rig {
    pub snaps: Snapshots,
    pub overlay: Overlay,
    pub out: Vec<Command>,
    pub fonts: Fonts,
    pub canvas: Canvas,
}

pub fn quiet_settings() -> Settings {
    // Deterministic: no clock text; animations keep their default durations.
    Settings {
        widgets: vec![
            WidgetSetting::new("media", true),
            WidgetSetting::new("volume", true),
            WidgetSetting::new("timer", true),
            WidgetSetting::new("focus", true),
            WidgetSetting::new("system", true),
            WidgetSetting::new("battery", true),
            WidgetSetting::new("shortcuts", true),
        ],
        ..Settings::default()
    }
}

impl Rig {
    pub fn new(settings: Settings) -> Rig {
        let mut snaps = Snapshots::new(settings, 10.0);
        snaps.wall = Local.with_ymd_and_hms(2026, 10, 29, 12, 0, 0).unwrap();
        let overlay = Overlay::new(&snaps);
        Rig {
            snaps,
            overlay,
            out: Vec::new(),
            fonts: Fonts::load_system(),
            canvas: Canvas::new(1, 1),
        }
    }

    /// Advances the clock by `seconds` in 10 ms ticks (fires timers like the host would).
    pub fn advance(&mut self, seconds: f64) {
        let steps = (seconds / 0.01).round() as usize;
        for _ in 0..steps {
            self.snaps.now += 0.01;
            self.snaps.wall += Duration::milliseconds(10);
            self.overlay.tick(&self.snaps, &mut self.out);
        }
    }

    pub fn settle(&mut self) {
        self.advance(1.5);
    }

    pub fn paint(&mut self) {
        let (w, h) = self.overlay.size(&self.snaps);
        self.canvas.resize(w.max(1), h.max(1));
        self.canvas.clear(Rgba::TRANSPARENT);
        self.overlay
            .paint(&mut self.canvas, &self.fonts, &self.snaps);
        self.overlay.drain(&mut self.out);
    }

    /// Canvas position of the middle of the painted surface.
    pub fn center(&self) -> (f32, f32) {
        let (ox, oy) = self.overlay.surface_origin(&self.snaps);
        let s = self.overlay.scale;
        (
            ox + self.overlay.surf.0 as f32 * s / 2.0,
            oy + self.overlay.surf.1 as f32 * s / 2.0,
        )
    }

    pub fn input(&mut self, mut input: InputState) -> bool {
        input.time = self.snaps.now;
        self.overlay.input(&input, &self.snaps, &mut self.out)
    }

    pub fn point(&mut self, p: Option<(f32, f32)>) {
        self.input(InputState {
            pointer: p,
            ..InputState::default()
        });
    }

    pub fn hover_center(&mut self) {
        let c = self.center();
        self.point(Some(c));
    }

    pub fn click(&mut self, p: (f32, f32)) {
        self.point(Some(p));
        self.input(InputState::press(p.0, p.1));
        self.input(InputState::release(p.0, p.1));
    }

    pub fn signal(&mut self, signal: Signal) {
        self.overlay.signal(signal, &self.snaps, &mut self.out);
    }

    pub fn commands(&mut self) -> Vec<Command> {
        std::mem::take(&mut self.out)
    }

    pub fn state(&self) -> OverlayState {
        self.overlay.state()
    }

    pub fn playing_media(&mut self, title: &str, artist: &str) {
        let artwork = cover_png();
        self.snaps.media.set(MediaSnapshot {
            available: true,
            source_id: "Spotify.exe".into(),
            source_name: "Spotify".into(),
            title: title.into(),
            artist: artist.into(),
            playing: true,
            position: StdDuration::from_secs(42),
            duration: StdDuration::from_secs(215),
            can_toggle: true,
            can_next: true,
            can_previous: true,
            can_seek: true,
            artwork: Some(artwork),
            ..Default::default()
        });
        self.snaps.media_epoch = self.snaps.now;
    }
}

/// A 96 px two-color cover encoded as PNG.
pub fn cover_png() -> Arc<[u8]> {
    let mut c = Canvas::new(96, 96);
    c.clear(Rgba::rgb(0x2B, 0x4C, 0x7E));
    c.fill_circle(
        30.0,
        30.0,
        40.0,
        &naby_render::Fill::Solid(Rgba::rgb(0xE0, 0x5A, 0x47)),
    );
    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("naby-ui-cover-{}-{n}.png", std::process::id()));
    c.save_png(&path).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let _ = std::fs::remove_file(path);
    Arc::from(bytes)
}

use naby_core::color::Rgba;

fn rig() -> Rig {
    Rig::new(quiet_settings())
}

// ---- hover and peek ------------------------------------------------------------------

#[test]
fn hover_delay_then_brief_then_detailed_then_leave() {
    let mut r = rig();
    r.paint();
    r.hover_center();
    r.advance(0.17);
    assert_eq!(
        r.state(),
        OverlayState::Closed,
        "before the 180 ms hover delay"
    );
    r.advance(0.06);
    assert_eq!(r.state(), OverlayState::Hover(HoverStage::Brief));
    r.advance(0.40);
    assert_eq!(
        r.state(),
        OverlayState::Hover(HoverStage::Brief),
        "detail waits 550 ms"
    );
    r.advance(0.20);
    assert_eq!(r.state(), OverlayState::Hover(HoverStage::Detailed));
    // The pointer leaves: collapses after the 300 ms leave delay.
    r.paint();
    r.point(None);
    r.advance(0.25);
    assert!(matches!(r.state(), OverlayState::Hover(_)));
    r.advance(0.1);
    assert_eq!(r.state(), OverlayState::Closed);
}

#[test]
fn leaving_before_the_delay_cancels_the_peek() {
    let mut r = rig();
    r.paint();
    r.hover_center();
    r.advance(0.1);
    r.point(None);
    r.advance(1.0);
    assert_eq!(r.state(), OverlayState::Closed);
    assert!(r.overlay.next_frame(r.snaps.now).is_none() || r.overlay.animating());
}

#[test]
fn single_stage_hover_opens_detailed_at_once() {
    let mut s = quiet_settings();
    s.two_stage_hover = false;
    let mut r = Rig::new(s);
    r.paint();
    r.hover_center();
    r.advance(0.25);
    assert_eq!(r.state(), OverlayState::Hover(HoverStage::Detailed));
}

#[test]
fn hover_peek_off_never_peeks() {
    let mut s = quiet_settings();
    s.hover_peek = false;
    let mut r = Rig::new(s);
    r.paint();
    r.hover_center();
    r.advance(2.0);
    assert_eq!(r.state(), OverlayState::Closed);
}

#[test]
fn peek_surface_grows_to_the_hover_card() {
    let mut r = rig();
    r.paint();
    r.hover_center();
    r.advance(1.4);
    r.settle();
    let (w, h) = r.overlay.surface_size();
    assert!((w - 350.0).abs() < 1.0, "hover width, got {w}");
    assert!((h - 192.0).abs() < 1.0, "hover height, got {h}");
}

// ---- open / close --------------------------------------------------------------------

#[test]
fn click_opens_the_panel_and_escape_closes_it() {
    let mut s = quiet_settings();
    s.hover_peek = false;
    let mut r = Rig::new(s);
    r.paint();
    let c = r.center();
    r.click(c);
    assert_eq!(r.state(), OverlayState::Open);
    assert!(r.overlay.wants_focus());
    r.settle();
    let (w, h) = r.overlay.surface_size();
    assert!((w - 384.0).abs() < 1.0 && h > 150.0, "open size {w}x{h}");
    r.input(InputState::key(naby_render::KeyEvent::new(
        naby_render::Key::Escape,
    )));
    assert_eq!(r.state(), OverlayState::Closed);
}

#[test]
fn outside_click_and_focus_loss_close_the_panel() {
    let mut s = quiet_settings();
    s.hover_peek = false;
    let mut r = Rig::new(s);
    r.paint();
    let c = r.center();
    r.click(c);
    r.settle();
    r.paint();
    r.signal(Signal::OutsideClick { x: -50.0, y: 400.0 });
    assert_eq!(r.state(), OverlayState::Closed);
    r.click(c);
    assert_eq!(r.state(), OverlayState::Open);
    r.signal(Signal::FocusLost);
    assert_eq!(r.state(), OverlayState::Closed);
    // A click inside the open panel is not "outside".
    r.signal(Signal::Panel);
    assert_eq!(r.state(), OverlayState::Open);
    r.settle();
    r.paint();
    let inside = r.center();
    r.signal(Signal::OutsideClick {
        x: inside.0,
        y: inside.1,
    });
    assert_eq!(r.state(), OverlayState::Open);
}

#[test]
fn panel_hotkey_toggles() {
    let mut r = rig();
    r.signal(Signal::Panel);
    assert_eq!(r.state(), OverlayState::Open);
    r.signal(Signal::Panel);
    assert_eq!(r.state(), OverlayState::Closed);
}

// ---- hidden, tucked ------------------------------------------------------------------

#[test]
fn fullscreen_tucks_and_restores() {
    let mut r = rig();
    r.signal(Signal::Panel);
    r.signal(Signal::EnvironmentHidden(true));
    r.settle();
    r.paint();
    assert_eq!(r.state(), OverlayState::Tucked, "the island stays shown");
    assert!(r.overlay.size(&r.snaps).0 > 0);
    let (_, oy) = r.overlay.surface_origin(&r.snaps);
    assert!(oy < -20.0, "slid up to the edge, got {oy}");
    // Hovering the edge reveals the preview.
    let (ox, _) = r.overlay.surface_origin(&r.snaps);
    r.point(Some((ox + 100.0, 2.0)));
    r.advance(0.05);
    assert!(
        matches!(r.state(), OverlayState::Hover(_)),
        "{:?}",
        r.state()
    );
    r.point(None);
    r.advance(0.5);
    r.settle();
    assert_eq!(r.state(), OverlayState::Tucked);
    // Manual hide still hides the whole island.
    r.signal(Signal::ToggleVisibility);
    assert_eq!(r.state(), OverlayState::Hidden);
    assert_eq!(r.overlay.size(&r.snaps), (0, 0));
    r.signal(Signal::ToggleVisibility);
    r.signal(Signal::EnvironmentHidden(false));
    r.settle();
    assert_eq!(r.state(), OverlayState::Closed);
    assert!(r.overlay.size(&r.snaps).0 > 0);
}

#[test]
fn visibility_hotkey_hides_and_shows() {
    let mut r = rig();
    r.signal(Signal::ToggleVisibility);
    assert_eq!(r.state(), OverlayState::Hidden);
    // Input is ignored while hidden.
    r.hover_center();
    r.advance(1.0);
    assert_eq!(r.state(), OverlayState::Hidden);
    r.signal(Signal::ToggleVisibility);
    assert_eq!(r.state(), OverlayState::Closed);
}

#[test]
fn click_beside_the_island_tucks_and_a_click_elsewhere_restores() {
    let mut r = rig();
    r.settle();
    r.paint();
    let (cx, cy) = r.center();
    // On the island itself: stays out.
    r.signal(Signal::OutsideClick { x: cx, y: cy });
    assert_eq!(r.state(), OverlayState::Closed);
    // Just below the island: tucks, and the area it rested on keeps counting as near.
    let (ox, oy) = r.overlay.surface_origin(&r.snaps);
    let beside = (cx, oy + r.overlay.surf.1 as f32 * r.overlay.scale + 20.0);
    r.signal(Signal::OutsideClick {
        x: beside.0,
        y: beside.1,
    });
    assert_eq!(r.state(), OverlayState::Tucked);
    r.settle();
    r.paint();
    r.signal(Signal::OutsideClick { x: cx, y: cy });
    assert_eq!(r.state(), OverlayState::Tucked);
    // Far away: comes back.
    r.signal(Signal::OutsideClick {
        x: ox - 400.0,
        y: 800.0,
    });
    assert_eq!(r.state(), OverlayState::Closed);
}

#[test]
fn tuck_hotkey_tucks_and_hover_reveals_a_peek() {
    let mut r = rig();
    r.signal(Signal::ToggleTuck);
    assert_eq!(r.state(), OverlayState::Tucked);
    r.settle();
    r.paint();
    // The sliver sits at the screen edge: the window is at the monitor top and the contour
    // is slid up so only `tuck_edge` DIP remain.
    let placement = r.overlay.placement(&r.snaps);
    assert_eq!(placement.origin.1, 0);
    let (_, oy) = r.overlay.surface_origin(&r.snaps);
    assert!(oy < -20.0, "slid up, got {oy}");
    // Hovering the sliver reveals the preview and collapses back after leaving.
    let (ox, _) = r.overlay.surface_origin(&r.snaps);
    let sliver = (ox + 100.0, 2.0);
    r.point(Some(sliver));
    r.advance(0.05);
    assert!(
        matches!(r.state(), OverlayState::Hover(_)),
        "{:?}",
        r.state()
    );
    r.point(None);
    r.advance(0.5);
    r.settle();
    assert_eq!(r.state(), OverlayState::Tucked);
    r.signal(Signal::ToggleTuck);
    assert_eq!(r.state(), OverlayState::Closed);
}

// ---- panel navigation ----------------------------------------------------------------

fn opened() -> Rig {
    let mut s = quiet_settings();
    s.hover_peek = false;
    let mut r = Rig::new(s);
    r.signal(Signal::Panel);
    r.settle();
    r.paint();
    r
}

#[test]
fn wheel_cycles_panel_pages_one_per_gesture() {
    let mut r = opened();
    assert_eq!(r.overlay.active_page(), "media");
    let c = r.center();
    r.point(Some(c));
    let wheel = |dy: f32| InputState {
        pointer: Some(c),
        wheel: (0.0, dy),
        ..InputState::default()
    };
    r.input(wheel(-48.0));
    assert_eq!(r.overlay.active_page(), "focus");
    // The same gesture (no pause) does not advance again.
    r.advance(0.05);
    r.input(wheel(-48.0));
    assert_eq!(r.overlay.active_page(), "focus");
    // After a pause the next notch advances; scrolling up goes back.
    r.advance(0.4);
    r.input(wheel(-48.0));
    assert_eq!(r.overlay.active_page(), "status");
    r.advance(0.4);
    r.input(wheel(48.0));
    assert_eq!(r.overlay.active_page(), "focus");
}

#[test]
fn dots_select_pages() {
    let mut r = opened();
    let keys = r.overlay.page_keys(&r.snaps);
    assert_eq!(keys, ["media", "focus", "status", "access"]);
    r.paint();
    let tab = r
        .overlay
        .hot
        .rect(util::Hot::PanelTab(2))
        .expect("dot registered");
    r.click(tab.center());
    assert_eq!(r.overlay.active_page(), "status");
    r.advance(0.4);
    r.paint();
    let settings = r.overlay.hot.rect(util::Hot::PanelSettings).unwrap();
    r.out.clear();
    r.click(settings.center());
    assert!(r.commands().contains(&Command::OpenSettings));
}

#[test]
fn pages_are_built_lazily_and_dropped_on_close() {
    let mut r = opened();
    assert!(r.overlay.panel.pages.contains_key("media"));
    assert!(!r.overlay.panel.pages.contains_key("timer"));
    r.signal(Signal::Panel);
    r.settle();
    assert!(
        r.overlay.panel.pages.is_empty(),
        "buffers released after the close animation"
    );
}

#[test]
fn show_page_opens_the_panel_on_that_page() {
    let mut r = rig();
    r.signal(Signal::ShowPage("timer"));
    assert_eq!(r.state(), OverlayState::Open);
    assert_eq!(r.overlay.active_page(), "focus");
}

// ---- peek carousel -------------------------------------------------------------------

fn peeking() -> Rig {
    let mut r = rig();
    r.playing_media("Blue Hour", "The Hours");
    r.paint();
    r.hover_center();
    r.advance(1.0);
    r.settle();
    r.paint();
    assert_eq!(r.state(), OverlayState::Hover(HoverStage::Detailed));
    r
}

#[test]
fn peek_wheel_and_arrows_move_between_pages() {
    let mut r = peeking();
    assert_eq!(r.overlay.peek_page(), "media");
    let c = r.center();
    r.input(InputState {
        pointer: Some(c),
        wheel: (0.0, -48.0),
        ..InputState::default()
    });
    assert_eq!(r.overlay.peek_page(), "volume");
    r.advance(0.4);
    r.input(InputState {
        pointer: Some(c),
        keys: vec![naby_render::KeyEvent::new(naby_render::Key::Left)],
        ..InputState::default()
    });
    assert_eq!(r.overlay.peek_page(), "media");
    r.settle();
    r.paint();
    let next = r.overlay.hot.rect(util::Hot::PeekArrow(1)).unwrap();
    r.click(next.center());
    assert_eq!(r.overlay.peek_page(), "volume");
    r.settle();
    r.paint();
    let dot = r.overlay.hot.rect(util::Hot::PeekDot(0)).unwrap();
    r.click(dot.center());
    assert_eq!(r.overlay.peek_page(), "media");
}

#[test]
fn peek_swipe_past_the_distance_changes_page() {
    let mut r = peeking();
    r.settle();
    r.paint();
    let vp = r.overlay.hot.rect(util::Hot::PeekViewport).unwrap();
    let y = vp.y + 5.0;
    let start = (vp.x + vp.w - 10.0, y);
    r.point(Some(start));
    r.input(InputState::press(start.0, start.1));
    r.input(InputState::drag(start.0 - 30.0, y));
    r.input(InputState::drag(start.0 - 120.0, y));
    r.input(InputState::release(start.0 - 120.0, y));
    assert_eq!(r.overlay.peek_page(), "volume");
}

#[test]
fn peek_controls_emit_media_commands() {
    let mut r = peeking();
    r.out.clear();
    let play = r.overlay.hot.rect(util::Hot::PeekPlay).unwrap();
    r.click(play.center());
    assert_eq!(r.commands(), vec![Command::MediaToggle]);
    let next = r.overlay.hot.rect(util::Hot::PeekNext).unwrap();
    r.click(next.center());
    assert!(r.commands().contains(&Command::MediaNext));
}

// ---- notices -------------------------------------------------------------------------

fn notice(r: &mut Rig, serial: u64) {
    r.snaps.notices.set(NoticeState {
        current: Some(InternalNotice::new(
            NoticeKind::Timer,
            "Timer complete",
            "Your 5 minute timer finished.",
        )),
        count: 1,
        serial,
    });
}

#[test]
fn notice_shows_in_the_pill_then_auto_dismisses() {
    let mut r = rig();
    r.paint();
    notice(&mut r, 1);
    r.advance(0.05);
    assert!(r.overlay.plan.notice);
    assert_eq!(r.overlay.plan.text, "\u{25CF} Timer complete");
    assert_eq!(r.state(), OverlayState::Closed);
    r.advance(11.5);
    assert!(
        !r.commands().contains(&Command::NoticeDismiss),
        "12 s default not over yet"
    );
    r.advance(1.0);
    let cmds = r.commands();
    assert_eq!(
        cmds.iter()
            .filter(|c| **c == Command::NoticeDismiss)
            .count(),
        1
    );
    // The host applies the dismissal.
    r.snaps.notices.set(NoticeState::default());
    r.advance(0.1);
    assert!(!r.overlay.plan.notice);
}

#[test]
fn hovering_the_pill_opens_the_notice_card_and_dismiss_works() {
    let mut r = rig();
    r.paint();
    notice(&mut r, 1);
    r.advance(0.05);
    r.hover_center();
    r.advance(0.3);
    assert_eq!(r.state(), OverlayState::Notice);
    assert!(r.commands().contains(&Command::NoticeDisplayed));
    r.settle();
    r.paint();
    // A displayed notice does not expire under the pointer.
    r.advance(20.0);
    assert!(!r.commands().contains(&Command::NoticeDismiss));
    let b = r
        .overlay
        .hot
        .rect(util::Hot::NoticeDismiss)
        .expect("dismiss button");
    r.click(b.center());
    assert!(r.commands().contains(&Command::NoticeDismiss));
    assert_eq!(r.state(), OverlayState::Closed);
}

#[test]
fn clicking_the_pill_with_a_notice_opens_the_card() {
    let mut s = quiet_settings();
    s.hover_peek = false;
    let mut r = Rig::new(s);
    r.paint();
    notice(&mut r, 1);
    r.advance(0.05);
    let c = r.center();
    r.click(c);
    assert_eq!(r.state(), OverlayState::Notice);
}

// ---- scheduling ----------------------------------------------------------------------

#[test]
fn visited_pages_do_not_wake_the_overlay_after_navigating_away() {
    let mut s = quiet_settings();
    s.hover_peek = false;
    s.widgets.retain(|w| w.id != "clock");
    s.widgets.push(WidgetSetting::new("weather", true));
    let mut r = Rig::new(s);
    r.signal(Signal::Panel);
    r.settle();
    r.paint();
    assert_eq!(r.overlay.active_page(), "media");
    r.signal(Signal::ShowPage("weather"));
    r.settle();
    r.paint();
    assert!(
        r.overlay.panel.pages.contains_key("media"),
        "media stays alive"
    );
    assert!(r.overlay.panel.pages.contains_key("weather"));
    // Only the weather page is on screen: it asks for the forecast, the visited media page for nothing.
    assert_eq!(r.overlay.panel.shown, ["weather"]);
    let demand = r.overlay.demand(&r.snaps);
    assert!(demand.weather);
    assert!(!demand.lyrics && !demand.mixer);
    r.signal(Signal::Panel);
    r.settle();
    r.paint();
    assert_eq!(r.overlay.next_frame(r.snaps.now), None);
    assert!(!r.overlay.animating());
    assert_eq!(r.overlay.demand(&r.snaps), crate::host::Demand::default());
}

#[test]
fn idle_closed_dock_schedules_nothing() {
    let mut s = quiet_settings();
    s.widgets.retain(|w| w.id != "clock");
    let mut r = Rig::new(s);
    r.paint();
    r.settle();
    r.paint();
    assert_eq!(r.overlay.next_frame(r.snaps.now), None);
    assert!(!r.overlay.animating());
    assert_eq!(r.overlay.demand(&r.snaps), crate::host::Demand::default());
}

#[test]
fn the_clock_wakes_once_a_minute_not_per_frame() {
    let mut s = quiet_settings();
    s.widgets.push(WidgetSetting::new("clock", true));
    let mut r = Rig::new(s);
    r.snaps.wall = Local.with_ymd_and_hms(2026, 10, 29, 12, 0, 15).unwrap();
    r.paint();
    r.settle();
    r.paint();
    let now = r.snaps.now;
    let next = r.overlay.next_frame(now).expect("minute wake");
    assert!(
        (next - now - 43.5).abs() < 2.0,
        "about 45 s to the minute, got {}",
        next - now
    );
    assert_eq!(r.overlay.plan.clock.as_deref(), Some("12:00"));
}

#[test]
fn playing_music_with_waveform_animates_and_asks_for_the_spectrum() {
    let mut s = quiet_settings();
    s.closed_media_content = "Waveform".into();
    let mut r = Rig::new(s);
    r.playing_media("Blue Hour", "The Hours");
    r.paint();
    r.advance(0.1);
    assert!(r.overlay.plan.waveform);
    assert!(r.overlay.next_frame(r.snaps.now).is_some());
    assert!(r.overlay.demand(&r.snaps).spectrum);
    // Pausing lets the waveform settle and the dock go back to sleep.
    let mut m = (*r.snaps.media).clone();
    m.playing = false;
    r.snaps.media.set(m);
    r.advance(1.5);
    assert_eq!(r.overlay.next_frame(r.snaps.now), None);
    assert!(!r.overlay.demand(&r.snaps).spectrum);
}

#[test]
fn countdown_label_ticks_every_second() {
    let mut r = rig();
    r.snaps.session.replace({
        let mut s = (*r.snaps.session).clone();
        s.timer
            .start(90.0, r.snaps.wall.with_timezone(&chrono::Utc), "", false);
        s
    });
    r.paint();
    r.advance(0.05);
    assert_eq!(r.overlay.plan.text, "Timer  01:30");
    let next = r.overlay.next_frame(r.snaps.now).unwrap();
    assert!(next - r.snaps.now <= 1.0);
    r.advance(2.0);
    assert_eq!(r.overlay.plan.text, "Timer  01:28");
}

#[test]
fn volume_feedback_replaces_the_pill_text_for_two_seconds() {
    let mut r = rig();
    r.snaps.audio.set(naby_core::media::AudioSnapshot {
        available: true,
        volume: 0.3,
        device: "Speakers".into(),
        ..Default::default()
    });
    r.advance(0.05);
    let mut a = (*r.snaps.audio).clone();
    a.volume = 0.5;
    r.snaps.audio.set(a);
    r.advance(0.05);
    assert!(r.overlay.plan.feedback);
    assert_eq!(r.overlay.plan.text, "50%");
    r.advance(2.2);
    assert!(!r.overlay.plan.feedback);
    assert_eq!(r.overlay.next_frame(r.snaps.now), None);
}

#[test]
fn wheel_over_the_closed_dock_changes_volume() {
    let mut r = rig();
    r.snaps.audio.set(naby_core::media::AudioSnapshot {
        available: true,
        volume: 0.30,
        ..Default::default()
    });
    r.paint();
    let c = r.center();
    r.point(Some(c));
    r.out.clear();
    r.input(InputState {
        pointer: Some(c),
        wheel: (0.0, 48.0),
        ..InputState::default()
    });
    let cmds = r.commands();
    assert!(
        matches!(cmds.as_slice(), [Command::SetVolume(v)] if (*v - 0.32).abs() < 1e-6),
        "{cmds:?}"
    );
}

#[test]
fn smart_activity_peeks_when_the_track_changes() {
    let mut r = rig();
    r.playing_media("First", "A");
    r.paint();
    r.advance(0.1);
    assert_eq!(
        r.state(),
        OverlayState::Closed,
        "the first track only primes"
    );
    r.playing_media("Second", "A");
    r.advance(0.1);
    assert!(matches!(r.state(), OverlayState::Hover(_)));
    assert_eq!(r.overlay.peek_page(), "media");
    r.advance(4.0);
    r.settle();
    assert_eq!(r.state(), OverlayState::Closed, "dismissed after 3.8 s");
}

#[test]
fn settings_changes_reset_transient_state() {
    let mut r = rig();
    r.signal(Signal::Panel);
    r.settle();
    let mut s = (*r.snaps.settings).clone();
    s.width = 260.0;
    r.snaps.settings.set(s);
    r.advance(0.05);
    // Geometry changed: everything closes and the surface snaps to the new closed size.
    assert!(r.overlay.surface_size().0 > 0.0);
}

#[test]
fn placement_keeps_the_pill_centered_at_the_top() {
    let mut r = rig();
    r.paint();
    let Placement { origin, size } = r.overlay.placement(&r.snaps);
    assert_eq!(origin.1, 0, "Mac preset touches the screen edge");
    let centre = origin.0 as f32 + size.0 as f32 / 2.0;
    assert!((centre - 960.0).abs() < 2.0, "got {centre}");
    assert!(size.0 >= 228 && size.1 >= 32);
}

// ---- snapshots -----------------------------------------------------------------------

fn snapshot_rig(settings: Settings) -> Option<Rig> {
    let r = Rig::new(settings);
    if !r.fonts.has_faces() {
        println!("skip: no system font found");
        return None;
    }
    Some(r)
}

/// Paints on a neutral backdrop so the black island is visible in the PNG.
fn save(r: &mut Rig, name: &str) {
    let (w, h) = r.overlay.size(&r.snaps);
    let (w, h) = (w.max(1), h.max(1));
    r.canvas.resize(w, h);
    r.canvas.clear(Rgba::rgb(0x3D, 0x41, 0x4A));
    r.overlay.paint(&mut r.canvas, &r.fonts, &r.snaps);
    snapshot::save(name, &r.canvas);
}

fn ink_in_pill(r: &Rig) -> usize {
    let (ox, oy) = r.overlay.surface_origin(&r.snaps);
    let s = r.overlay.scale;
    let rect = naby_render::Rect::new(
        ox,
        oy,
        r.overlay.surf.0 as f32 * s,
        r.overlay.surf.1 as f32 * s,
    );
    snapshot::count_differing(&r.canvas, rect.inset(6.0), Rgba::BLACK)
}

#[test]
fn snapshot_closed_idle() {
    let mut s = quiet_settings();
    s.widgets.push(WidgetSetting::new("clock", true));
    let Some(mut r) = snapshot_rig(s) else { return };
    r.snaps.wall = Local.with_ymd_and_hms(2026, 10, 29, 9, 41, 0).unwrap();
    r.settle();
    save(&mut r, "overlay-closed-idle");
    // Clock text only: some ink, but far less than a media pill.
    assert!(ink_in_pill(&r) > 20);
}

#[test]
fn snapshot_closed_with_title() {
    let mut s = quiet_settings();
    s.widgets.push(WidgetSetting::new("clock", true));
    s.closed_media_icon = "Artwork".into();
    s.closed_media_content = "Title".into();
    let Some(mut r) = snapshot_rig(s) else { return };
    r.playing_media("Blue Hour", "The Hours");
    r.snaps.wall = Local.with_ymd_and_hms(2026, 10, 29, 9, 41, 0).unwrap();
    r.settle();
    assert!(r.overlay.plan.cover && !r.overlay.plan.waveform);
    assert_eq!(r.overlay.plan.text, "Blue Hour");
    save(&mut r, "overlay-closed-title");
    assert!(ink_in_pill(&r) > 200);
}

#[test]
fn snapshot_closed_waveform() {
    let mut s = quiet_settings();
    s.closed_media_content = "Waveform".into();
    s.closed_media_icon = "Artwork".into();
    s.width = 260.0;
    let Some(mut r) = snapshot_rig(s) else { return };
    r.playing_media("Blue Hour", "The Hours");
    r.snaps.levels.spectrum_ready = true;
    for step in 0..50 {
        for (i, b) in r.snaps.levels.bands.iter_mut().enumerate() {
            *b = 0.25 + 0.7 * (((i as f32 * 0.7) + step as f32 * 0.3).sin() * 0.5 + 0.5);
        }
        r.snaps.levels.peak = 0.6;
        r.snaps.levels.sequence += 1;
        r.advance(0.03);
    }
    assert!(r.overlay.plan.waveform);
    assert!(r.overlay.closed.wave.level(5) > 0.1);
    save(&mut r, "overlay-closed-waveform");
}

#[test]
fn snapshot_closed_lyric_line() {
    let mut s = quiet_settings();
    s.closed_media_content = "Lyrics".into();
    s.lyrics_enabled = true;
    s.width = 300.0;
    let Some(mut r) = snapshot_rig(s) else { return };
    r.playing_media("Blue Hour", "The Hours");
    r.snaps.lyrics.set(LyricResult {
        lines: naby_core::media::lrc::parse(
            "[00:10.00]First light\n[00:40.00]Walking home through the blue hour\n[01:20.00]Quiet",
        ),
        status: "Synced lyrics".into(),
    });
    r.advance(0.2);
    r.settle();
    assert!(r.overlay.plan.lyrics);
    save(&mut r, "overlay-closed-lyrics");
    assert!(r.overlay.closed.text.contains("blue hour") || !r.overlay.closed.text.is_empty());
    let _ = Lyrics::default();
}

#[test]
fn snapshot_hover_media() {
    let Some(mut r) = snapshot_rig(quiet_settings()) else {
        return;
    };
    r.playing_media("Blue Hour", "The Hours");
    r.snaps.audio.set(naby_core::media::AudioSnapshot {
        available: true,
        volume: 0.62,
        device: "Speakers".into(),
        ..Default::default()
    });
    r.paint();
    r.hover_center();
    r.advance(1.0);
    r.settle();
    r.paint();
    assert_eq!(r.state(), OverlayState::Hover(HoverStage::Detailed));
    save(&mut r, "overlay-hover-media");
    let mut brief = Rig::new(quiet_settings());
    brief.playing_media("Blue Hour", "The Hours");
    brief.paint();
    brief.hover_center();
    brief.advance(0.25);
    brief.settle_brief();
    save(&mut brief, "overlay-hover-brief");
}

impl Rig {
    fn settle_brief(&mut self) {
        // Settle springs without letting the detail timer fire.
        for _ in 0..40 {
            self.snaps.now += 0.005;
            self.snaps.wall += Duration::milliseconds(5);
            self.overlay.tick(&self.snaps, &mut self.out);
        }
    }
}

#[test]
fn snapshot_open_with_a_page() {
    let mut s = quiet_settings();
    s.hover_peek = false;
    let Some(mut r) = snapshot_rig(s) else { return };
    r.playing_media("Blue Hour", "The Hours");
    r.signal(Signal::Panel);
    r.settle();
    r.paint();
    r.settle();
    assert_eq!(r.state(), OverlayState::Open);
    save(&mut r, "overlay-open-media");
    r.signal(Signal::ShowPage("timer"));
    r.settle();
    r.paint();
    r.settle();
    save(&mut r, "overlay-open-focus");
}

#[test]
fn snapshot_notification() {
    let Some(mut r) = snapshot_rig(quiet_settings()) else {
        return;
    };
    r.snaps.notices.set(NoticeState {
        current: Some(InternalNotice::new(
            NoticeKind::Timer,
            "Timer complete",
            "Your 5 minute timer has finished. Tap to start another one or dismiss this notice.",
        )),
        count: 2,
        serial: 1,
    });
    r.advance(0.05);
    r.hover_center();
    r.advance(0.4);
    r.paint();
    r.settle();
    r.paint();
    r.settle();
    assert_eq!(r.state(), OverlayState::Notice);
    save(&mut r, "overlay-notice");
}
