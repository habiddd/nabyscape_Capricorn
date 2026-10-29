//! Multi-monitor follow (C# `UI/Island/DisplayFollow.cs`): which monitor hosts the island,
//! the 250 ms pointer hysteresis, the slide between monitors, and the hidden-tuck edge hit test.

use super::easing::{Ease, MotionConfig};
use super::geometry::{RectF, RectI};
use crate::settings::{DisplayMode, DockEdge, Settings};

/// Pointer must stay on another monitor this long before the island follows (ms).
pub const FOLLOW_DWELL_MS: i64 = 250;
/// Period of the pointer poll while follow (or assistant / hidden-tuck tracking) is active.
pub const POINTER_POLL_SECONDS: f64 = 0.1;

/// A display, in physical pixels. `id` is the device name (`\\.\DISPLAY1`).
#[derive(Debug, Clone, PartialEq)]
pub struct Monitor {
    pub id: String,
    pub bounds: RectI,
    pub work: RectI,
    pub primary: bool,
    pub scale: f64,
    pub refresh_hz: u32,
}

impl Monitor {
    fn distance_sq(&self, x: i32, y: i32) -> i64 {
        let dx = (self.bounds.x - x)
            .max(0)
            .max(x - (self.bounds.right() - 1));
        let dy = (self.bounds.y - y)
            .max(0)
            .max(y - (self.bounds.bottom() - 1));
        i64::from(dx) * i64::from(dx) + i64::from(dy) * i64::from(dy)
    }
}

/// `Screen.FromPoint`: the monitor containing the point, else the nearest one.
pub fn monitor_at(monitors: &[Monitor], x: i32, y: i32) -> Option<&Monitor> {
    monitors
        .iter()
        .find(|m| m.bounds.contains(x, y))
        .or_else(|| monitors.iter().min_by_key(|m| m.distance_sq(x, y)))
}

/// Hysteresis for the follow target.
#[derive(Debug, Clone, Default)]
pub struct MonitorFollowPolicy {
    candidate: String,
    since: i64,
}

impl MonitorFollowPolicy {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&mut self) {
        self.candidate.clear();
        self.since = 0;
    }

    /// C# `MonitorFollowPolicy.Target`: the monitor id the island belongs on. Falls back to the
    /// primary monitor when the preferred one is not connected (stored selection is untouched).
    pub fn target<'a>(
        available: &[&str],
        primary: &'a str,
        mode: DisplayMode,
        stored: &'a str,
        followed: Option<&'a str>,
    ) -> &'a str {
        let preferred = match mode {
            DisplayMode::Follow => followed.unwrap_or(stored),
            DisplayMode::Primary => primary,
            DisplayMode::Selected => stored,
        };
        if available.contains(&preferred) {
            preferred
        } else {
            primary
        }
    }

    /// C# `Observe`: feed the monitor under the pointer; returns the monitor to move to once
    /// the pointer has stayed there for 250 ms (or immediately when `immediate`).
    pub fn observe(
        &mut self,
        current: &str,
        pointer: &str,
        now_ms: i64,
        blocked: bool,
        hidden: bool,
        immediate: bool,
    ) -> Option<String> {
        if hidden || blocked || pointer == current {
            self.reset();
            return None;
        }
        if immediate {
            self.reset();
            return Some(pointer.to_string());
        }
        if self.candidate != pointer {
            self.candidate = pointer.to_string();
            self.since = now_ms;
            return None;
        }
        if now_ms - self.since < FOLLOW_DWELL_MS {
            return None;
        }
        self.reset();
        Some(pointer.to_string())
    }
}

/// The window slide between two monitors (C# `followSlide`): cubic ease-out from the old window
/// position to the new one. `retarget` follows placement updates while sliding.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FollowSlide {
    start: (i32, i32),
    target: (i32, i32),
    began: f64,
    duration: f64,
}

impl FollowSlide {
    pub fn new(start: (i32, i32), target: (i32, i32), now: f64, duration_ms: f64) -> Self {
        FollowSlide {
            start,
            target,
            began: now,
            duration: (duration_ms / 1000.0).max(0.001),
        }
    }

    pub fn retarget(&mut self, target: (i32, i32)) {
        self.target = target;
    }

    pub fn target(&self) -> (i32, i32) {
        self.target
    }

    fn t(&self, now: f64) -> f64 {
        ((now - self.began) / self.duration).clamp(0.0, 1.0)
    }

    /// Window position at `now`.
    pub fn sample(&self, now: f64) -> (i32, i32) {
        let e = Ease::CUBIC_OUT.apply(self.t(now));
        let lerp = |a: i32, b: i32| (f64::from(a) + f64::from(b - a) * e).round() as i32;
        (
            lerp(self.start.0, self.target.0),
            lerp(self.start.1, self.target.1),
        )
    }

    pub fn finished(&self, now: f64) -> bool {
        self.t(now) >= 1.0
    }

    pub fn next_frame(&self, now: f64) -> Option<f64> {
        (!self.finished(now)).then_some(now + 1.0 / 60.0)
    }
}

/// A decision to move the island to another monitor.
#[derive(Debug, Clone, PartialEq)]
pub struct FollowMove {
    pub monitor_id: String,
    /// Slide from the old position (otherwise jump).
    pub animate: bool,
    pub duration_ms: f64,
}

/// Per-frame inputs for [`DisplayFollow::update`].
#[derive(Debug, Clone, Copy, Default)]
pub struct FollowContext {
    /// A drag, open menu, focused editor, pressed button or an ongoing slide blocks follow.
    pub interaction: bool,
    pub user_hidden: bool,
    pub visible: bool,
    pub tucked: bool,
    /// First observation after showing: move without waiting.
    pub immediate: bool,
}

/// Cursor → monitor follow with hysteresis and slide timing.
#[derive(Debug, Clone, Default)]
pub struct DisplayFollow {
    policy: MonitorFollowPolicy,
    slide: Option<FollowSlide>,
}

impl DisplayFollow {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&mut self) {
        self.policy.reset();
    }

    pub fn sliding(&self) -> bool {
        self.slide.is_some()
    }

    /// C# `ResolveFollow`: returns the monitor to move to, if any. `current` is the id of the
    /// monitor currently hosting the island.
    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        s: &Settings,
        cfg: &MotionConfig,
        monitors: &[Monitor],
        current: &str,
        cursor: (i32, i32),
        now_ms: i64,
        ctx: FollowContext,
    ) -> Option<FollowMove> {
        if s.display_mode() != DisplayMode::Follow {
            return None;
        }
        if ctx.user_hidden || !ctx.visible {
            self.policy.reset();
            return None;
        }
        let pointer = monitor_at(monitors, cursor.0, cursor.1)?;
        let blocked = ctx.interaction || self.slide.is_some();
        let next = self.policy.observe(
            current,
            &pointer.id,
            now_ms,
            blocked,
            ctx.user_hidden,
            ctx.immediate,
        )?;
        let animate = !ctx.immediate && s.follow_monitor_animation && cfg.enabled && !ctx.tucked;
        Some(FollowMove {
            monitor_id: next,
            animate,
            duration_ms: s.follow_monitor_duration,
        })
    }

    /// Starts the slide after the host recomputed placement for the new monitor.
    pub fn begin_slide(&mut self, from: (i32, i32), to: (i32, i32), now: f64, duration_ms: f64) {
        self.slide = Some(FollowSlide::new(from, to, now, duration_ms));
    }

    /// Placement changed while sliding (`followTarget = point`).
    pub fn retarget(&mut self, to: (i32, i32)) {
        if let Some(slide) = &mut self.slide {
            slide.retarget(to);
        }
    }

    /// Window position to apply at `now`; `None` when not sliding. Clears the slide when done.
    pub fn sample(&mut self, now: f64) -> Option<(i32, i32)> {
        let slide = self.slide?;
        let point = slide.sample(now);
        if slide.finished(now) {
            self.slide = None;
        }
        Some(point)
    }

    pub fn cancel_slide(&mut self) {
        self.slide = None;
    }

    pub fn next_frame(&self, now: f64) -> Option<f64> {
        self.slide.and_then(|s| s.next_frame(now))
    }

    /// Whether the 100 ms pointer poll must run (C# `ConfigureDisplayFollow`).
    pub fn pointer_poll_needed(
        s: &Settings,
        user_hidden: bool,
        visible: bool,
        assistant_pointer_tracking: bool,
        tucked: bool,
    ) -> bool {
        let hidden_tuck_hover = tucked && s.tuck_edge == 0.0 && !user_hidden;
        !(user_hidden
            || !visible
            || (s.display_mode() != DisplayMode::Follow
                && !assistant_pointer_tracking
                && !hidden_tuck_hover))
    }
}

/// C# `CheckHiddenTuckEdge`: with a zero tuck edge the island is fully off screen; the pointer
/// touching the dock's edge strip (3 px) over the island's span reveals it. `area` is the
/// monitor rectangle for a top dock, otherwise the work area; sizes are DIP times `scale`.
pub fn hidden_tuck_edge_hit(s: &Settings, area: RectI, scale: f64, point: (i32, i32)) -> bool {
    let b = area.to_f();
    let width = s.width * scale;
    let height = s.height * scale;
    let center = b.x + (b.w - width) * s.horizontal / 100.0 + width / 2.0;
    let cy = b.y + (b.h - height) * s.dock_vertical / 100.0 + height / 2.0;
    let (px, py) = (f64::from(point.0), f64::from(point.1));
    match s.dock_edge() {
        DockEdge::Left => (px - b.x).abs() < 3.0 && (py - cy).abs() < height / 2.0,
        DockEdge::Right => (px - (b.right() - 1.0)).abs() < 3.0 && (py - cy).abs() < height / 2.0,
        DockEdge::Bottom => {
            (py - (b.bottom() - 1.0)).abs() < 3.0 && (px - center).abs() < width / 2.0
        }
        _ => (py - b.y).abs() < 3.0 && (px - center).abs() < width / 2.0,
    }
}

/// Convenience for hosts: the rectangle [`hidden_tuck_edge_hit`] and placement use.
pub fn follow_area(s: &Settings, m: &Monitor) -> RectF {
    super::geometry::placement_area(s.dock_edge(), m.bounds, m.work).to_f()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mon(id: &str, x: i32, y: i32, w: i32, h: i32, primary: bool) -> Monitor {
        Monitor {
            id: id.into(),
            bounds: RectI::new(x, y, w, h),
            work: RectI::new(x, y, w, h - 40),
            primary,
            scale: 1.0,
            refresh_hz: 60,
        }
    }

    fn follow_settings() -> Settings {
        Settings {
            display_mode: "Follow".into(),
            ..Settings::default()
        }
    }

    #[test]
    fn mixed_negative_landscape_and_portrait_identities_resolve() {
        let got = MonitorFollowPolicy::target(
            &["left-negative", "portrait-high-DPI"],
            "left-negative",
            DisplayMode::Follow,
            "left-negative",
            Some("portrait-high-DPI"),
        );
        assert_eq!(got, "portrait-high-DPI");
    }

    #[test]
    fn hotplug_falls_back_to_primary_without_mutating_stored_selection() {
        let t = MonitorFollowPolicy::target(&["A"], "A", DisplayMode::Selected, "B", None);
        assert_eq!(t, "A");
        let t = MonitorFollowPolicy::target(&["A"], "A", DisplayMode::Follow, "B", Some("C"));
        assert_eq!(t, "A");
    }

    #[test]
    fn reconnected_selected_monitor_is_resolved_again() {
        let t = MonitorFollowPolicy::target(&["A", "B"], "A", DisplayMode::Selected, "B", None);
        assert_eq!(t, "B");
    }

    #[test]
    fn primary_ignores_stored_selected_monitor() {
        let t = MonitorFollowPolicy::target(&["A", "B"], "A", DisplayMode::Primary, "B", None);
        assert_eq!(t, "A");
    }

    #[test]
    fn follow_hysteresis_scenarios() {
        let mut p = MonitorFollowPolicy::new();
        // first observation waits; 249 ms waits; 250 ms moves
        assert_eq!(p.observe("A", "B", 1000, false, false, false), None);
        assert_eq!(p.observe("A", "B", 1249, false, false, false), None);
        assert_eq!(
            p.observe("A", "B", 1250, false, false, false).as_deref(),
            Some("B")
        );
        // monitor identity change restarts hysteresis
        p.observe("A", "B", 2000, false, false, false);
        p.observe("A", "C", 2200, false, false, false);
        assert_eq!(p.observe("A", "C", 2449, false, false, false), None);
        assert_eq!(
            p.observe("A", "C", 2450, false, false, false).as_deref(),
            Some("C")
        );
        // blocked interaction resets the pending target
        p.observe("A", "B", 3000, false, false, false);
        assert_eq!(p.observe("A", "B", 3400, true, false, false), None);
        assert_eq!(p.observe("A", "B", 3500, false, false, false), None);
        assert_eq!(
            p.observe("A", "B", 3750, false, false, false).as_deref(),
            Some("B")
        );
        // manual hide cancels pending movement; show resolves immediately
        p.observe("A", "B", 4000, false, false, false);
        assert_eq!(p.observe("A", "B", 4500, false, true, false), None);
        assert_eq!(
            p.observe("A", "B", 4600, false, false, true).as_deref(),
            Some("B")
        );
        // return to source cancels pending movement
        p.observe("A", "B", 5000, false, false, false);
        assert_eq!(p.observe("A", "A", 5300, false, false, false), None);
        assert_eq!(p.observe("A", "B", 5400, false, false, false), None);
        // blocked show waits until interaction ends
        assert_eq!(p.observe("A", "B", 6000, true, false, true), None);
    }

    #[test]
    fn follow_moves_to_monitor_under_cursor() {
        let monitors = [
            mon("A", 0, 0, 1920, 1080, true),
            mon("B", 1920, 0, 1920, 1080, false),
        ];
        let s = follow_settings();
        let cfg = MotionConfig::from_settings(&s, true);
        let ctx = FollowContext {
            visible: true,
            ..FollowContext::default()
        };
        let mut f = DisplayFollow::new();
        assert!(f
            .update(&s, &cfg, &monitors, "A", (100, 100), 0, ctx)
            .is_none());
        assert!(f
            .update(&s, &cfg, &monitors, "A", (2000, 100), 1000, ctx)
            .is_none());
        assert!(f
            .update(&s, &cfg, &monitors, "A", (2000, 100), 1249, ctx)
            .is_none());
        let mv = f
            .update(&s, &cfg, &monitors, "A", (2000, 100), 1250, ctx)
            .unwrap();
        assert_eq!(mv.monitor_id, "B");
        assert!(mv.animate);
        assert_eq!(mv.duration_ms, 360.0);
        // Immediate (just shown) never animates.
        let imm = FollowContext {
            immediate: true,
            ..ctx
        };
        let mv = f
            .update(&s, &cfg, &monitors, "A", (2000, 100), 1300, imm)
            .unwrap();
        assert!(!mv.animate);
        // Tucked or disabled animation jumps.
        let tucked = FollowContext {
            tucked: true,
            ..ctx
        };
        f.update(&s, &cfg, &monitors, "A", (2000, 100), 2000, tucked);
        let mv = f
            .update(&s, &cfg, &monitors, "A", (2000, 100), 2250, tucked)
            .unwrap();
        assert!(!mv.animate);
        // Not in Follow mode: never moves.
        let primary = Settings::default();
        assert!(f
            .update(&primary, &cfg, &monitors, "A", (2000, 100), 9000, imm)
            .is_none());
        // Hidden / invisible resets.
        let hidden = FollowContext {
            user_hidden: true,
            ..ctx
        };
        assert!(f
            .update(&s, &cfg, &monitors, "A", (2000, 100), 9000, hidden)
            .is_none());
    }

    #[test]
    fn follow_blocks_during_interaction_and_slide() {
        let monitors = [
            mon("A", 0, 0, 1920, 1080, true),
            mon("B", 1920, 0, 1920, 1080, false),
        ];
        let s = follow_settings();
        let cfg = MotionConfig::default();
        let busy = FollowContext {
            visible: true,
            interaction: true,
            ..FollowContext::default()
        };
        let mut f = DisplayFollow::new();
        f.update(&s, &cfg, &monitors, "A", (2000, 5), 0, busy);
        assert!(f
            .update(&s, &cfg, &monitors, "A", (2000, 5), 5000, busy)
            .is_none());
        f.begin_slide((0, 0), (100, 0), 0.0, 360.0);
        assert!(f.sliding());
        let idle = FollowContext {
            visible: true,
            ..FollowContext::default()
        };
        f.update(&s, &cfg, &monitors, "A", (2000, 5), 6000, idle);
        assert!(f
            .update(&s, &cfg, &monitors, "A", (2000, 5), 7000, idle)
            .is_none());
    }

    #[test]
    fn monitor_at_uses_nearest_when_outside() {
        let monitors = [
            mon("L", -1920, 0, 1920, 1080, false),
            mon("R", 0, 0, 1920, 1080, true),
        ];
        assert_eq!(monitor_at(&monitors, -5, 5).unwrap().id, "L");
        assert_eq!(monitor_at(&monitors, 5, 5).unwrap().id, "R");
        assert_eq!(monitor_at(&monitors, -4000, 5).unwrap().id, "L");
        assert_eq!(monitor_at(&monitors, 9000, 5000).unwrap().id, "R");
        assert!(monitor_at(&[], 0, 0).is_none());
    }

    #[test]
    fn slide_eases_and_follows_retarget() {
        let mut f = DisplayFollow::new();
        f.begin_slide((0, 0), (1000, 200), 10.0, 400.0);
        assert_eq!(f.sample(10.0), Some((0, 0)));
        let mid = f.sample(10.2).unwrap();
        assert_eq!(mid.0, (1000.0 * 0.875f64).round() as i32);
        assert!(f.next_frame(10.2).is_some());
        f.retarget((2000, 200));
        assert_eq!(f.sample(10.4), Some((2000, 200)));
        assert!(!f.sliding(), "cleared once finished");
        assert_eq!(f.sample(11.0), None);
        assert_eq!(f.next_frame(11.0), None);
        f.begin_slide((0, 0), (1, 1), 0.0, 100.0);
        f.cancel_slide();
        assert!(!f.sliding());
    }

    #[test]
    fn pointer_poll_only_when_needed() {
        let follow = follow_settings();
        let plain = Settings::default();
        assert!(DisplayFollow::pointer_poll_needed(
            &follow, false, true, false, false
        ));
        assert!(!DisplayFollow::pointer_poll_needed(
            &follow, true, true, false, false
        ));
        assert!(!DisplayFollow::pointer_poll_needed(
            &follow, false, false, false, false
        ));
        assert!(!DisplayFollow::pointer_poll_needed(
            &plain, false, true, false, false
        ));
        assert!(DisplayFollow::pointer_poll_needed(
            &plain, false, true, true, false
        ));
        let hidden_edge = Settings {
            tuck_edge: 0.0,
            ..Settings::default()
        };
        assert!(DisplayFollow::pointer_poll_needed(
            &hidden_edge,
            false,
            true,
            false,
            true
        ));
        assert!(!DisplayFollow::pointer_poll_needed(
            &hidden_edge,
            false,
            true,
            false,
            false
        ));
    }

    #[test]
    fn hidden_tuck_edge_hit_zones() {
        let area = RectI::new(0, 0, 1920, 1080);
        let top = Settings {
            width: 300.0,
            horizontal: 50.0,
            dock_edge: "Top".into(),
            ..Settings::default()
        };
        assert!(hidden_tuck_edge_hit(&top, area, 1.0, (960, 0)));
        assert!(hidden_tuck_edge_hit(&top, area, 1.0, (960 + 149, 2)));
        assert!(!hidden_tuck_edge_hit(&top, area, 1.0, (960 + 151, 0)));
        assert!(!hidden_tuck_edge_hit(&top, area, 1.0, (960, 3)));
        let left = Settings {
            dock_edge: "Left".into(),
            dock_vertical: 50.0,
            ..top.clone()
        };
        assert!(hidden_tuck_edge_hit(&left, area, 1.0, (0, 540)));
        assert!(!hidden_tuck_edge_hit(&left, area, 1.0, (0, 100)));
        let right = Settings {
            dock_edge: "Right".into(),
            ..left.clone()
        };
        assert!(hidden_tuck_edge_hit(&right, area, 1.0, (1919, 540)));
        let bottom = Settings {
            dock_edge: "Bottom".into(),
            ..top
        };
        assert!(hidden_tuck_edge_hit(&bottom, area, 1.0, (960, 1079)));
        assert!(!hidden_tuck_edge_hit(&bottom, area, 1.0, (960, 0)));
    }
}
