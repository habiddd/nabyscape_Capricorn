//! The dock state machine: hover timing, peek, open, notice, tuck and visibility transitions,
//! pointer and keyboard routing. Ports the behavioral half of C# `OverlayWindow.cs`,
//! `CompactOverlay.cs`, `AutoTuck.cs`, `PeekCarousel.cs` (navigation), `NotificationOverlay.cs`
//! and `EnhancedInteraction.cs` (smart activities), driven by the `naby_core::motion` policies.

use super::util::Hot;
use super::{DragState, Overlay};
use crate::host::{Signal, Snapshots};
use naby_core::command::Command;
use naby_core::motion::navigation::WheelTarget;
use naby_core::motion::peek::PeekRequest;
use naby_core::motion::{
    AutoTuckPolicy, Channel, FrameBudget, HoverAction, HoverInputs, HoverStage, IslandGeometry,
    MotionConfig, OverlaySettingsState, PanelNavigation, PeekPolicy, SizeF, Step, Tween, WheelNode,
};
use naby_core::settings::{policy, Settings};
use naby_render::{InputState, Key, Theme};

/// How long an automatic preview (track change, timer ending) stays (`activityDismiss`).
pub const ACTIVITY_SECONDS: f64 = 3.8;

impl Overlay {
    // ---- configuration --------------------------------------------------------------

    pub(crate) fn sync(&mut self, snaps: &Snapshots) {
        self.now = snaps.now;
        if snaps.display.version() != self.display_version {
            self.display_version = snaps.display.version();
            self.scale = snaps.display.scale.clamp(0.5, 8.0);
            self.dirty = true;
        }
        if snaps.settings.version() != self.settings_version {
            let first = self.settings_version == 0;
            self.settings_version = snaps.settings.version();
            self.apply_settings(snaps, first);
        }
        self.ensure_contour();
    }

    fn apply_settings(&mut self, snaps: &Snapshots, first: bool) {
        let s: &Settings = &snaps.settings;
        let previous = self.settings_state;
        let state = OverlaySettingsState::capture(s);
        self.settings_state = Some(state);
        self.cfg.motion = MotionConfig::from_settings(s, true);
        self.cfg.theme = Theme::from_settings(s);
        self.cfg.notch = s.preset == "Mac" && s.dock_edge() == naby_core::settings::DockEdge::Top;
        self.cfg.radius = s.radius;
        self.update_frame_limit(snaps);
        let rebuild =
            first || previous.is_none_or(|p| state.needs_rebuild(&p) || state.geometry_changed(&p));
        self.panel.pages.clear();
        self.peek.ids.clear();
        if rebuild {
            // C# `ApplySettings`: everything transient closes and the surface snaps.
            self.hover.stop_all();
            self.notice.showing = false;
            self.notice_showing = false;
            self.peeking = false;
            self.peek.detailed = false;
            self.peek.drag = None;
            self.tuck_preview = false;
            self.peek_fade = Tween::instant(0.0);
            self.compact_fade = Tween::instant(1.0);
            self.panel_fade = Tween::instant(if self.expanded { 1.0 } else { 0.0 });
            self.panel_slide = Tween::instant(if self.expanded { 0.0 } else { -8.0 });
            self.panel.wide = false;
            let target = self.compute_target(snaps);
            self.target_size = target;
            self.surf = target;
            self.lift = if self.expanded { 1.0 } else { 0.0 };
            self.motion.stop();
            self.motion.snap(Channel::SurfaceWidth, target.0);
            self.motion.snap(Channel::SurfaceHeight, target.1);
            self.motion.snap(Channel::GlassLift, self.lift);
            self.host_h = target.1;
            self.contour = None;
        } else {
            self.retarget(snaps, 200.0);
        }
        self.apply_environment_tuck(snaps);
        self.dirty = true;
    }

    /// Frame cap from the performance profile (`ConfigurePerformance`).
    pub(crate) fn update_frame_limit(&mut self, snaps: &Snapshots) {
        let budget = FrameBudget::new(&snaps.settings.performance, snaps.display.refresh_hz);
        let battery = snaps.battery.on_battery();
        let cpu = snaps.system.cpu.unwrap_or(0.0);
        self.cfg.motion.frame_limit = budget.limit(self.interacting(), battery, cpu);
    }

    // ---- small queries --------------------------------------------------------------

    pub(crate) fn tuck_requested(&self, s: &Settings) -> bool {
        self.tuck.requested(s, false)
    }

    pub(crate) fn interacting(&self) -> bool {
        self.pressed.is_some()
            || self.peek.drag.as_ref().is_some_and(|d| d.active)
            || self.peek.slider_drag
            || self.editing_dock
    }

    /// Placement area in physical pixels (monitor for a top dock, work area otherwise).
    pub(crate) fn area(&self, snaps: &Snapshots) -> naby_core::motion::RectF {
        naby_core::motion::placement_area(
            snaps.settings.dock_edge(),
            snaps.display.bounds,
            snaps.display.work,
        )
        .to_f()
    }

    pub(crate) fn area_dip(&self, snaps: &Snapshots) -> SizeF {
        let a = self.area(snaps);
        let s = f64::from(self.scale);
        SizeF {
            w: a.w / s,
            h: a.h / s,
        }
    }

    /// The peek page ids (`HoverPagePolicy.Pages` plus `files` while a file drag is over).
    pub(crate) fn peek_pages(&self, snaps: &Snapshots) -> Vec<String> {
        let mut ids = policy::hover_pages(&snaps.settings);
        if self.file_drag && !ids.iter().any(|i| i == "files") {
            ids.push("files".to_owned());
        }
        ids
    }

    pub(crate) fn hover_inputs(&self, snaps: &Snapshots) -> HoverInputs {
        HoverInputs {
            pointer_inside: self.pointer_inside,
            over_assistant_character: false,
            expanded: self.expanded,
            peeking: self.peeking,
            tucked: self.tuck.is_tucked(),
            user_hidden: self.user_hidden || self.env_hidden,
            tuck_requested: self.tuck_requested(&snaps.settings),
            cover_pressed: false,
            interaction_active: self.interacting(),
            file_drag_active: self.file_drag,
            assistant_reply_visible: snaps.assistant.reply.is_some(),
        }
    }

    // ---- surface targets ------------------------------------------------------------

    /// The size (DIP) the surface should have in the current mode.
    pub(crate) fn compute_target(&self, snaps: &Snapshots) -> (f64, f64) {
        let s = &snaps.settings;
        let area = self.area_dip(snaps);
        if self.notice_showing {
            return self.notice.size;
        }
        if self.peeking {
            let stage = if self.peek.detailed {
                HoverStage::Detailed
            } else {
                HoverStage::Brief
            };
            let size = IslandGeometry::hover_size(s, area, stage);
            return (size.w, size.h);
        }
        if self.expanded {
            let width = naby_core::media::policy::lyrics_layout_width(
                s.panel_width,
                s.panel_scale,
                s.panel_padding,
                area.w - 24.0,
                self.panel.wide,
            );
            let content = (self.panel.content_height > 0.0).then_some(self.panel.content_height);
            let size = IslandGeometry::open_size(s, area, content);
            return (width, size.h);
        }
        let closed = IslandGeometry::closed_size(s);
        (closed.w, closed.h)
    }

    /// Animates the surface to the current mode's size.
    pub(crate) fn retarget(&mut self, snaps: &Snapshots, ms: f64) {
        let target = self.compute_target(snaps);
        self.target_size = target;
        let enabled = self.cfg.motion.enabled;
        self.motion.set_fps(self.cfg.motion.frame_limit);
        self.motion
            .go(Channel::SurfaceWidth, self.surf.0, target.0, ms, enabled);
        self.motion
            .go(Channel::SurfaceHeight, self.surf.1, target.1, ms, enabled);
        let lift = if self.expanded || self.peeking || self.notice_showing {
            1.0
        } else {
            0.0
        };
        self.motion
            .go(Channel::GlassLift, self.lift, lift, ms, enabled);
        self.read_motion();
        self.host_h = self.host_h.max(self.surf.1).max(target.1);
        self.dirty = true;
    }

    pub(crate) fn read_motion(&mut self) {
        if let Some(w) = self.motion.value(Channel::SurfaceWidth) {
            self.surf.0 = w;
        }
        if let Some(h) = self.motion.value(Channel::SurfaceHeight) {
            self.surf.1 = h;
        }
        if let Some(l) = self.motion.value(Channel::GlassLift) {
            self.lift = l;
        }
    }

    fn anim_ms(&self, snaps: &Snapshots) -> f64 {
        snaps.settings.animation_ms
    }

    // ---- transitions ------------------------------------------------------------------

    /// Opens the panel (C# `Reveal`).
    pub(crate) fn reveal(&mut self, snaps: &Snapshots) {
        self.user_hidden = false;
        self.hide_notice();
        self.end_peek_gesture();
        self.hover.stop_all();
        self.peek.automatic = false;
        self.set_tucked(snaps, false);
        if self.expanded {
            self.focus_wanted = true;
            self.dirty = true;
            return;
        }
        self.expanded = true;
        self.peeking = false;
        self.peek.detailed = false;
        self.ensure_panel_page(snaps);
        self.panel.scroll = 0.0;
        self.panel.pending_input = None;
        self.panel.content_height = 0.0;
        let ms = self.anim_ms(snaps);
        let now = self.now;
        self.compact_fade =
            self.cfg
                .motion
                .fade(self.compact_fade.value(now), 0.0, now, 120.0, 0.0);
        self.peek_fade = self
            .cfg
            .motion
            .fade(self.peek_fade.value(now), 0.0, now, 90.0, 0.0);
        self.panel_fade = self.cfg.motion.fade(0.0, 1.0, now, ms.min(260.0), 0.0);
        self.panel_slide = self.cfg.motion.to(-8.0, 0.0, now, 320.0, 0.0);
        self.focus_wanted = true;
        self.retarget(snaps, ms);
        self.measure_panel_on_next_paint();
    }

    /// Closes whatever is open (C# `Collapse`).
    pub(crate) fn collapse(&mut self, snaps: &Snapshots) {
        self.hide_notice();
        self.end_peek_gesture();
        self.hover.stop_all();
        self.peek.automatic = false;
        self.panel.forced_widget = None;
        if !self.expanded && !self.peeking {
            self.apply_environment_tuck(snaps);
            return;
        }
        self.expanded = false;
        self.peeking = false;
        self.peek.detailed = false;
        self.focus_wanted = false;
        let now = self.now;
        let cfg = self.cfg.motion;
        self.compact_fade = cfg.fade(self.compact_fade.value(now), 1.0, now, 120.0, 0.0);
        self.peek_fade = cfg.fade(self.peek_fade.value(now), 0.0, now, 90.0, 0.0);
        self.panel_fade = cfg.fade(self.panel_fade.value(now), 0.0, now, 100.0, 0.0);
        self.panel_slide = cfg.to(self.panel_slide.value(now), -8.0, now, 150.0, 0.0);
        let ms = (self.anim_ms(snaps) * 0.78).max(160.0);
        self.retarget(snaps, ms);
        if self.tuck_requested(&snaps.settings) {
            self.apply_environment_tuck(snaps);
        }
        for page in self.panel.pages.values_mut() {
            page.on_hide();
        }
        self.panel.memory.clear();
    }

    pub(crate) fn hide_notice(&mut self) {
        if self.notice_showing {
            self.notice_showing = false;
            self.notice.showing = false;
            self.peeking = false;
        }
    }

    pub(crate) fn end_peek_gesture(&mut self) {
        self.peek.drag = None;
        self.peek.slider_drag = false;
        self.peek.wheel.reset();
        self.pressed = None;
    }

    /// Hover peek (C# `PreviewActivity`).
    pub(crate) fn preview_activity(&mut self, snaps: &Snapshots) {
        let s = &snaps.settings;
        let hidden = self.user_hidden || self.env_hidden;
        if snaps.notices.current.is_some()
            && !self.expanded
            && !self.peeking
            && !self.file_drag
            && !hidden
        {
            self.open_notice(snaps, false);
            return;
        }
        let ids = self.peek_pages(snaps);
        let request = PeekRequest {
            pages: ids.len(),
            automatic_preview: self.peek.automatic,
            tuck_preview: self.tuck_preview,
            file_drag_active: self.file_drag,
            expanded: self.expanded,
            peeking: self.peeking,
            user_hidden: hidden,
            tuck_requested: self.tuck_requested(s),
        };
        if !PeekPolicy::can_start(s, &request) {
            return;
        }
        self.peeking = true;
        self.peek.detailed = PeekPolicy::starts_detailed(s, self.peek.automatic);
        self.hover.peek_started(s, self.now, self.peek.detailed);
        if self.peek.automatic && ids.contains(&self.peek.kind) {
            self.peek.id = self.peek.kind.clone();
        } else if !ids.contains(&self.peek.id) {
            self.peek.id = ids[0].clone();
        }
        self.peek.ids = ids;
        self.peek.offset = Tween::instant(0.0);
        let now = self.now;
        let cfg = self.cfg.motion;
        self.compact_fade = cfg.fade(self.compact_fade.value(now), 0.0, now, 80.0, 0.0);
        self.peek_fade = cfg.fade(self.peek_fade.value(now), 1.0, now, 160.0, 0.0);
        self.retarget(snaps, s.animation_ms);
    }

    /// Second hover stage: grow the strip to the card (C# `PromotePeek`).
    pub(crate) fn promote_peek(&mut self, snaps: &Snapshots) {
        if !self.peeking || self.notice_showing {
            return;
        }
        self.hover.cancel_detail();
        self.peek.automatic = false;
        if !self.peek.detailed {
            self.peek.detailed = true;
            self.retarget(snaps, snaps.settings.animation_ms);
        }
    }

    /// Smart activity: a new track or a timer about to end (C# `AutomaticActivity`).
    pub(crate) fn automatic_activity(&mut self, snaps: &Snapshots, kind: &str) {
        let s = &snaps.settings;
        if !s.smart_activities
            || self.expanded
            || self.user_hidden
            || self.env_hidden
            || self.tuck_requested(s)
            || (self.peeking && !self.peek.automatic)
        {
            return;
        }
        if !s.widget_enabled(kind) {
            return;
        }
        if kind == "media"
            && !naby_core::media::policy::allow_automatic(
                s.browser_media_previews,
                Some(snaps.media.source_id.as_str()).filter(|s| !s.is_empty()),
            )
        {
            return;
        }
        self.peek.kind = kind.to_owned();
        self.peek.automatic = true;
        if !self.peeking {
            self.preview_activity(snaps);
        } else {
            self.peek.id = kind.to_owned();
        }
        self.activity_until = self.now + ACTIVITY_SECONDS;
    }

    /// Shows the notice card (C# `OpenNotice`).
    pub(crate) fn open_notice(&mut self, snaps: &Snapshots, requested: bool) {
        if requested && self.expanded {
            self.collapse(snaps);
        }
        let Some(current) = snaps.notices.current.as_ref() else {
            return;
        };
        let hidden = self.user_hidden || self.env_hidden;
        if hidden
            || self.expanded
            || self.file_drag
            || self.interacting()
            || self.feedback.visible(self.now)
        {
            return;
        }
        if self.peeking && !self.notice_showing {
            return;
        }
        self.push(Command::NoticeDisplayed);
        self.hover.stop_all();
        self.set_tucked(snaps, false);
        self.peeking = true;
        self.notice_showing = true;
        self.notice.showing = true;
        self.notice.serial = snaps.notices.serial;
        self.notice.scroll = 0.0;
        self.notice.size = super::notice::estimate_size(
            current,
            snaps.notices.count,
            &snaps.settings,
            self.area_dip(snaps),
            self.cfg.notch,
        );
        self.notice.entry = Some(naby_core::motion::notice_entry(
            &self.cfg.motion,
            &snaps.settings.notification_animation,
            self.now,
        ));
        let now = self.now;
        self.compact_fade = self
            .cfg
            .motion
            .fade(self.compact_fade.value(now), 0.0, now, 80.0, 0.0);
        self.peek_fade = Tween::instant(0.0);
        self.retarget(snaps, snaps.settings.animation_ms);
    }

    // ---- tuck / visibility ------------------------------------------------------------

    pub(crate) fn set_tucked(&mut self, snaps: &Snapshots, value: bool) {
        let outcome = self.tuck.set_tucked(
            &snaps.settings,
            value,
            self.user_hidden || self.env_hidden,
            &self.cfg.motion,
            self.now,
        );
        if outcome.contract_surface {
            self.peeking = false;
            self.expanded = false;
            self.notice_showing = false;
            self.notice.showing = false;
            self.peek.detailed = false;
            self.compact_fade = Tween::instant(1.0);
            self.peek_fade = Tween::instant(0.0);
            self.panel_fade = Tween::instant(0.0);
            self.feedback.clear();
            self.retarget(snaps, snaps.settings.animation_ms);
        }
        if !outcome.unchanged {
            self.dirty = true;
        }
    }

    pub(crate) fn apply_environment_tuck(&mut self, snaps: &Snapshots) {
        let s = &snaps.settings;
        match AutoTuckPolicy::environment(
            self.tuck_requested(s),
            self.user_hidden || self.env_hidden,
            self.expanded,
            self.peeking,
        ) {
            Some(true) => self.set_tucked(snaps, true),
            Some(false) => self.set_tucked(snaps, false),
            None => {}
        }
    }

    fn toggle_tuck(&mut self, snaps: &Snapshots) {
        let manual = self.tuck.toggle_manual();
        if self.user_hidden || self.env_hidden {
            return;
        }
        self.collapse(snaps);
        self.set_tucked(snaps, manual);
    }

    /// Pointer entered a tucked dock: peek out of the edge (C# `RevealTuckedPreview`).
    pub(crate) fn reveal_tucked_preview(&mut self, snaps: &Snapshots) {
        let requested = self.tuck_requested(&snaps.settings);
        if !self.tuck.can_reveal_preview(self.user_hidden, requested) {
            return;
        }
        self.set_tucked(snaps, false);
        self.tuck_preview = true;
        self.preview_activity(snaps);
        self.tuck_preview = false;
    }

    fn set_visibility(&mut self, snaps: &Snapshots, show: bool) {
        self.collapse(snaps);
        self.user_hidden = !show;
        self.set_tucked(snaps, false);
        if !show {
            self.release_buffers();
        }
        self.dirty = true;
    }

    /// Drops everything that only exists for a shown dock (C# hides the window).
    pub(crate) fn release_buffers(&mut self) {
        for page in self.panel.pages.values_mut() {
            page.on_hide();
        }
        self.panel.pages.clear();
        self.panel.memory.clear();
        self.panel.pending_input = None;
        self.art.clear();
        self.art_image = None;
        self.contour = None;
        self.pressed = None;
        self.pointer = None;
        self.pointer_inside = false;
        self.hot.clear();
        self.hover.stop_all();
    }

    // ---- signals ----------------------------------------------------------------------

    pub(crate) fn handle_signal(&mut self, signal: Signal, snaps: &Snapshots) -> bool {
        match signal {
            Signal::Panel => {
                if self.expanded {
                    self.collapse(snaps);
                } else {
                    self.reveal(snaps);
                }
            }
            Signal::ToggleVisibility => {
                let show = self.user_hidden;
                self.set_visibility(snaps, show);
            }
            Signal::ToggleTuck => self.toggle_tuck(snaps),
            Signal::EnvironmentHidden(hidden) => {
                if self.env_hidden == hidden {
                    return false;
                }
                if hidden {
                    self.collapse(snaps);
                    self.release_buffers();
                }
                self.env_hidden = hidden;
            }
            Signal::OutsideClick { x, y } => {
                if (self.expanded || self.peeking)
                    && !self.contour_contains(self.surface_origin(snaps), (x, y))
                {
                    self.collapse(snaps);
                } else {
                    return false;
                }
            }
            Signal::FocusLost => {
                if self.expanded || self.peeking {
                    self.collapse(snaps);
                } else {
                    return false;
                }
            }
            Signal::DisplayChanged => {
                self.contour = None;
                self.host_h = self.target_size.1;
                self.retarget(snaps, 0.0);
            }
            Signal::ShowPage(id) => {
                self.open_page(snaps, id);
            }
            Signal::FileDrag(on) => {
                self.file_drag = on;
                if on && !self.expanded {
                    if self.tuck.is_tucked() {
                        self.reveal_tucked_preview(snaps);
                    }
                    if !self.peeking {
                        self.preview_activity(snaps);
                    }
                    self.select_peek(snaps, "files");
                    self.promote_peek(snaps);
                } else if !on {
                    self.hover.start_leave(self.now);
                }
            }
            Signal::OpenNotice => self.open_notice(snaps, true),
            Signal::Escape => {
                if self.expanded || self.peeking {
                    self.collapse(snaps);
                }
            }
        }
        self.dirty = true;
        true
    }

    // ---- input ------------------------------------------------------------------------

    pub(crate) fn handle_input(&mut self, input: &InputState, snaps: &Snapshots) -> bool {
        if self.user_hidden || self.env_hidden {
            return false;
        }
        let mut changed = false;
        self.pointer = input.pointer;
        self.panel.last_input = input.without_events();
        let origin = self.surface_origin(snaps);
        let inside_before = self.pointer_inside;
        self.pointer_inside = input
            .pointer
            .is_some_and(|p| self.contour_contains(origin, p));
        let inputs = self.hover_inputs(snaps);
        if self.pointer_inside && !inside_before {
            if self.peeking {
                self.peek.automatic = false;
                self.activity_until = f64::INFINITY;
            }
            if let Some(HoverAction::RevealTucked) =
                self.hover.pointer_enter(&snaps.settings, self.now, &inputs)
            {
                self.reveal_tucked_preview(snaps);
            }
            changed = true;
        } else if !self.pointer_inside && inside_before {
            self.hover.pointer_leave(self.now, &inputs);
            changed = true;
        }
        // Reflection follows the pointer on glass surfaces.
        if self.pointer_inside && (self.peeking || self.expanded) && snaps.settings.glass_style {
            if let Some(p) = input.pointer {
                let s = f64::from(self.scale);
                let x = ((f64::from(p.0 - origin.0)) / s / self.surf.0.max(1.0)).clamp(0.0, 1.0);
                let y = ((f64::from(p.1 - origin.1)) / s / self.surf.1.max(1.0)).clamp(0.0, 1.0);
                if (x - self.light.0).abs() + (y - self.light.1).abs() > 0.001 {
                    self.light = (x, y);
                    changed = true;
                }
            }
        }
        if input.keys.iter().any(|k| k.key == Key::Escape) {
            if self.expanded || self.peeking {
                self.collapse(snaps);
                changed = true;
            }
        } else {
            changed |= self.handle_keys(input, snaps);
        }
        changed |= self.handle_wheel(input, snaps);
        changed |= self.handle_buttons(input, snaps);
        if self.expanded && self.panel_wants_input(input) {
            self.panel.pending_input = Some(input.clone());
            changed = true;
        }
        changed
    }

    fn handle_keys(&mut self, input: &InputState, snaps: &Snapshots) -> bool {
        let mut changed = false;
        if self.peeking && !self.notice_showing {
            for k in &input.keys {
                match k.key {
                    Key::Left => {
                        self.navigate_peek(snaps, -1);
                        changed = true;
                    }
                    Key::Right => {
                        self.navigate_peek(snaps, 1);
                        changed = true;
                    }
                    _ => {}
                }
            }
        }
        changed
    }

    fn handle_wheel(&mut self, input: &InputState, snaps: &Snapshots) -> bool {
        let (wx, wy) = input.wheel;
        if (wx == 0.0 && wy == 0.0) || !self.pointer_inside {
            return false;
        }
        let s = &snaps.settings;
        let to_units = |pixels: f32| (f64::from(pixels) * 2.5 / f64::from(self.scale)) as i32;
        let now_ms = (self.now * 1000.0) as i64;
        match PanelNavigation::wheel_target(s, self.peeking, self.expanded) {
            WheelTarget::PeekCarousel => {
                if self.notice_showing {
                    return false;
                }
                let (delta, horizontal) = if wx != 0.0 {
                    (to_units(wx), true)
                } else {
                    (to_units(wy), false)
                };
                let dir = self
                    .peek
                    .wheel
                    .push(if horizontal { delta } else { -delta }, now_ms);
                if dir != 0 {
                    self.navigate_peek(snaps, dir);
                }
                true
            }
            WheelTarget::PanelPages => {
                let (delta, horizontal) = if wx != 0.0 {
                    (to_units(wx), true)
                } else {
                    (to_units(wy), false)
                };
                let overflow = self.panel_overflow(snaps);
                let nodes = [WheelNode::ScrollViewer {
                    content_scroll: false,
                    scrollable_height: overflow,
                    exempt: false,
                }];
                let can_swipe = PanelNavigation::can_swipe(s, self.expanded, false, false);
                let outcome = self.panel.nav.route_wheel(
                    can_swipe,
                    delta,
                    horizontal,
                    if horizontal { &[] } else { &nodes },
                    false,
                    now_ms,
                );
                if outcome.direction != 0 {
                    self.cycle_panel_page(snaps, outcome.direction);
                    return true;
                }
                if !outcome.handled && overflow > 1.0 && !horizontal {
                    self.scroll_panel(-wy, overflow as f32);
                    return true;
                }
                outcome.handled
            }
            WheelTarget::Volume => {
                if !snaps.audio.available || !self.tuck_free() {
                    return false;
                }
                let step = f64::from(s.volume_step.max(1)) / 100.0;
                let delta = if wy > 0.0 || wx > 0.0 { step } else { -step };
                let volume = (f64::from(snaps.audio.volume) + delta).clamp(0.0, 1.0);
                self.push(Command::SetVolume(volume));
                true
            }
            WheelTarget::Pages => {
                let dir = if wy > 0.0 { -1 } else { 1 };
                self.cycle_panel_page(snaps, dir);
                true
            }
            WheelTarget::None => false,
        }
    }

    fn tuck_free(&self) -> bool {
        !self.tuck.is_tucked() && !self.user_hidden
    }

    fn handle_buttons(&mut self, input: &InputState, snaps: &Snapshots) -> bool {
        let mut changed = false;
        let p = input.pointer;
        if input.pressed {
            if let Some(p) = p {
                let hot = self.hot.hit(p).or_else(|| {
                    (self.pointer_inside && !self.expanded && !self.peeking).then_some(Hot::Surface)
                });
                self.pressed = hot;
                if let (Some(Hot::PeekViewport), Some(p)) = (hot, Some(p)) {
                    self.peek.suppress_click = false;
                    self.peek.drag = Some(DragState {
                        start: p,
                        origin: self.peek_offset_value(),
                        active: false,
                    });
                } else if hot == Some(Hot::PeekVolume) {
                    self.peek.slider_drag = true;
                    self.drag_peek_volume(p.0, snaps);
                } else if hot == Some(Hot::PanelBody) {
                    let can =
                        PanelNavigation::can_swipe(&snaps.settings, self.expanded, false, false);
                    self.panel
                        .swipe
                        .press((f64::from(p.0), f64::from(p.1)), can, false);
                }
                changed = true;
            }
        }
        if input.down && !input.pressed {
            if let Some(p) = p {
                changed |= self.drag_move(p, snaps);
            }
        }
        if input.released {
            changed |= self.release(input, snaps);
        }
        changed
    }

    fn drag_move(&mut self, p: (f32, f32), snaps: &Snapshots) -> bool {
        let scale = f64::from(self.scale);
        if self.peek.slider_drag {
            return self.drag_peek_volume(p.0, snaps);
        }
        if let Some(drag) = self.peek.drag.as_mut() {
            let dx = f64::from(p.0 - drag.start.0);
            let dy = f64::from(p.1 - drag.start.1);
            if !drag.active {
                if dy.abs() > 10.0 * scale && dy.abs() > dx.abs() {
                    self.peek.drag = None;
                    return false;
                }
                if dx.abs() < 8.0 * scale {
                    return false;
                }
                drag.active = true;
                self.peek.suppress_click = true;
                self.promote_peek(snaps);
            }
            let origin = self.peek.drag.as_ref().map_or(0.0, |d| d.origin);
            self.drag_peek(snaps, dx / scale + origin);
            return true;
        }
        if self.expanded && !self.panel.pages.values().any(|p| p.captures_pointer()) {
            let can = PanelNavigation::can_swipe(&snaps.settings, true, false, false);
            if let Some(dir) = self.panel.swipe.moved(
                (f64::from(p.0), f64::from(p.1)),
                true,
                can,
                snaps.settings.swipe_distance * scale,
            ) {
                self.cycle_panel_page(snaps, dir);
                return true;
            }
        }
        false
    }

    fn release(&mut self, input: &InputState, snaps: &Snapshots) -> bool {
        let pressed = self.pressed.take();
        let over = input.pointer.and_then(|p| self.hot.hit(p));
        let was_slider = std::mem::take(&mut self.peek.slider_drag);
        let swiped = self.panel.swipe.release();
        if let Some(drag) = self.peek.drag.take() {
            if drag.active {
                let offset = self.peek_offset_value();
                self.finish_peek_swipe(snaps, offset);
                return true;
            }
        }
        if was_slider || swiped {
            return true;
        }
        let Some(hot) = pressed else {
            return false;
        };
        let fallback = (hot == Hot::Surface && self.pointer_inside).then_some(Hot::Surface);
        if over != Some(hot) && fallback != Some(hot) {
            return true;
        }
        self.activate(hot, snaps);
        true
    }

    /// A press and release on the same hot spot.
    fn activate(&mut self, hot: Hot, snaps: &Snapshots) {
        match hot {
            Hot::Surface => {
                if self.tuck.is_tucked() || self.tuck_requested(&snaps.settings) && !self.expanded {
                    self.reveal(snaps);
                } else if snaps.notices.current.is_some() && !self.expanded {
                    self.open_notice(snaps, true);
                } else if self.expanded {
                    self.collapse(snaps);
                } else {
                    self.reveal(snaps);
                }
            }
            Hot::PeekViewport => {
                if !std::mem::take(&mut self.peek.suppress_click) {
                    self.open_peek_page(snaps);
                }
            }
            other => self.activate_control(other, snaps),
        }
    }

    // ---- time -------------------------------------------------------------------------

    /// Advances animations and timers; absorbs snapshot changes. True = repaint.
    pub(crate) fn advance(&mut self, snaps: &Snapshots) -> bool {
        let mut changed = std::mem::take(&mut self.dirty);
        let now = snaps.now;
        let hidden = self.user_hidden || self.env_hidden;
        if hidden {
            return changed;
        }
        // Springs.
        let enabled = self.cfg.motion.enabled;
        let step = self.motion.update(now, enabled);
        if step != Step::Idle {
            self.read_motion();
            changed = true;
        }
        if step == Step::Finished {
            self.on_motion_finished(snaps);
        }
        if self.tuck.moving() || self.tuck.next_frame(now).is_some() {
            self.tuck.update(now, &self.cfg.motion);
            changed = true;
        }
        // Hover and detail timers.
        for _ in 0..6 {
            let inputs = self.hover_inputs(snaps);
            let Some(action) = self.hover.poll(now, &inputs) else {
                break;
            };
            changed = true;
            match action {
                HoverAction::ShowPreview => self.preview_activity(snaps),
                HoverAction::RevealTucked => self.reveal_tucked_preview(snaps),
                HoverAction::Collapse => self.collapse(snaps),
                HoverAction::PromoteDetail => self.promote_peek(snaps),
                HoverAction::ShowAssistantPreview => {}
            }
        }
        // Automatic previews dismiss themselves unless the pointer is on them.
        if self.peek.automatic && self.peeking && now >= self.activity_until {
            if self.pointer_inside {
                self.peek.automatic = false;
            } else {
                self.collapse(snaps);
                changed = true;
            }
        }
        // Peek carousel offset and fades.
        if !self.peek.offset.finished(now)
            || !self.compact_fade.finished(now)
            || !self.peek_fade.finished(now)
            || !self.panel_fade.finished(now)
            || !self.panel_slide.finished(now)
            || !self.page_fade.finished(now)
            || !self.page_slide.finished(now)
        {
            changed = true;
        }
        if snaps.revision() != self.revision {
            self.revision = snaps.revision();
            changed = true;
        }
        changed |= self.refresh_content(snaps);
        if std::mem::take(&mut self.retarget_pending) {
            self.retarget(snaps, 200.0);
            changed = true;
        }
        changed
    }

    fn on_motion_finished(&mut self, snaps: &Snapshots) {
        self.host_h = self.target_size.1;
        if !self.expanded && !self.peeking {
            for page in self.panel.pages.values_mut() {
                page.on_hide();
            }
            self.panel.pages.clear();
            self.panel.memory.clear();
            self.panel.pending_input = None;
            if !self.tuck_requested(&snaps.settings) {
                self.art.trim_idle(std::time::Duration::from_secs(30));
            }
        }
    }

    /// Everything derived from snapshots: notices, feedback, closed text, ambient.
    fn refresh_content(&mut self, snaps: &Snapshots) -> bool {
        let mut changed = false;
        let now = snaps.now;
        // Notices: a new front notice, dismissal when it expires.
        let notices = &snaps.notices;
        if notices.serial != self.seen_notice_serial {
            self.seen_notice_serial = notices.serial;
            self.notice_deadline = now + snaps.settings.notification_seconds;
            self.notice_dismiss_sent = false;
            changed = true;
        }
        if notices.current.is_none() && self.notice_showing {
            self.collapse(snaps);
            changed = true;
        }
        if notices.current.is_some() {
            let busy = self.expanded
                || self.peeking
                || self.feedback.visible(now)
                || self.file_drag
                || self.interacting()
                || self.pointer_inside;
            if busy {
                self.notice_deadline = now + snaps.settings.notification_seconds;
            } else if now >= self.notice_deadline && !self.notice_dismiss_sent {
                self.notice_dismiss_sent = true;
                self.push(Command::NoticeDismiss);
            }
        }
        // Volume / brightness feedback.
        let allowed = !self.expanded
            && !self.peeking
            && !self.tuck.is_tucked()
            && !self.user_hidden
            && !self.env_hidden;
        changed |= self.feedback.observe(snaps, &self.cfg.motion, allowed);
        if self.feedback.visible(now) != self.feedback_was_visible {
            self.feedback_was_visible = self.feedback.visible(now);
            changed = true;
        }
        // Smart activities.
        self.check_smart_activities(snaps);
        changed |= self.refresh_closed(snaps);
        self.refresh_ambient(snaps);
        changed
    }

    fn check_smart_activities(&mut self, snaps: &Snapshots) {
        let m = &snaps.media;
        let track = if m.available {
            format!("{}|{}", m.source_id, m.title)
        } else {
            String::new()
        };
        if track != self.last_track {
            let primed = !self.last_track.is_empty();
            self.last_track = track.clone();
            if primed && !track.is_empty() && m.playing {
                self.automatic_activity(snaps, "media");
            }
        }
        let wall = snaps.wall.with_timezone(&chrono::Utc);
        let session = &snaps.session;
        for (id, running, remaining, deadline) in [
            (
                "timer",
                session.timer.running(),
                session.timer.remaining(wall),
                session.timer.deadline,
            ),
            (
                "focus",
                session.focus.running(),
                session.focus.countdown.remaining(wall),
                session.focus.countdown.deadline,
            ),
        ] {
            if !snaps.widget_on(id) || !running || remaining > 30.0 || remaining <= 0.0 {
                continue;
            }
            let slot = if id == "timer" {
                &mut self.warned_timer
            } else {
                &mut self.warned_focus
            };
            if *slot == deadline {
                continue;
            }
            *slot = deadline;
            self.automatic_activity(snaps, id);
        }
    }
}
