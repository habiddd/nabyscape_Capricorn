//! Auto-tuck policy and slide state (C# `UI/Island/AutoTuck.cs`).

use super::easing::MotionConfig;
use super::geometry::{Contour, RectF};
use super::spring::{Channel, Step, SurfaceMotion};
use crate::settings::{DockEdge, Settings};

/// Pure tuck decisions.
pub struct AutoTuckPolicy;

impl AutoTuckPolicy {
    /// Default visible sliver of a tucked island, DIP.
    pub const EDGE_HEIGHT: f64 = 6.0;

    /// Vertical translation (DIP, negative = up) that leaves `edge` DIP (clamped 0..12) visible.
    /// Short surfaces are never moved past the edge.
    pub fn offset(compact_height: f64, edge: f64) -> f64 {
        -(compact_height - edge.clamp(0.0, 12.0)).max(0.0)
    }

    /// C# `AutoTuckPolicy.ShouldTuck`: tuck when requested, not manually hidden and idle.
    /// (`expanded` and `peeking` do not matter on their own: only `engaged` keeps it revealed.)
    pub fn should_tuck(
        requested: bool,
        manually_hidden: bool,
        _expanded: bool,
        _peeking: bool,
        engaged: bool,
    ) -> bool {
        requested && !manually_hidden && !engaged
    }

    /// C# `AutoTuckPolicy.CanHover`: only a tucked, automatically hidden edge reveals on hover.
    pub fn can_hover(manually_hidden: bool, requested: bool, tucked: bool) -> bool {
        !manually_hidden && requested && tucked
    }

    /// Translation `(dx, dy)` in DIP of the tucked island (C# `SetTucked`): side docks slide
    /// horizontally by `Width - TuckEdge`, all others vertically.
    pub fn destination(s: &Settings) -> (f64, f64) {
        match s.dock_edge() {
            DockEdge::Left => (-(s.width - s.tuck_edge), 0.0),
            DockEdge::Right => (s.width - s.tuck_edge, 0.0),
            DockEdge::Bottom => (0.0, -Self::offset(s.height, s.tuck_edge)),
            DockEdge::Top | DockEdge::Floating => (0.0, Self::offset(s.height, s.tuck_edge)),
        }
    }

    /// C# `ApplyEnvironmentTuck`: `Some(true)` tuck, `Some(false)` untuck, `None` leave as is.
    pub fn environment(
        requested: bool,
        user_hidden: bool,
        expanded: bool,
        peeking: bool,
    ) -> Option<bool> {
        if requested && !user_hidden && !expanded && !peeking {
            Some(true)
        } else if !requested {
            Some(false)
        } else {
            None
        }
    }
}

/// What the host must do after [`TuckController::set_tucked`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TuckOutcome {
    /// The tucked flag flipped.
    pub changed: bool,
    /// Tucking: contract the painted surface to the closed size, hide the peek / panel and
    /// stop live rendering.
    pub contract_surface: bool,
    /// Untucking: restore live activity and the idle dock.
    pub reveal: bool,
    /// Nothing changed, nothing to animate.
    pub unchanged: bool,
}

/// Tuck flag, manual tuck and the slide animation.
#[derive(Debug, Clone, Default)]
pub struct TuckController {
    tucked: bool,
    manual: bool,
    destination: (f64, f64),
    resting: (f64, f64),
    motion: SurfaceMotion,
}

impl TuckController {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_tucked(&self) -> bool {
        self.tucked
    }

    pub fn manual(&self) -> bool {
        self.manual
    }

    /// C# `TuckRequested`.
    pub fn requested(&self, s: &Settings, environment_hidden: bool) -> bool {
        self.manual || s.always_tucked || environment_hidden
    }

    /// C# `ToggleTuck` state flip; returns the new manual flag.
    pub fn toggle_manual(&mut self) -> bool {
        self.manual = !self.manual;
        self.manual
    }

    pub fn can_reveal_preview(&self, user_hidden: bool, requested: bool) -> bool {
        AutoTuckPolicy::can_hover(user_hidden, requested, self.tucked)
    }

    /// Current slide translation in DIP (C# `tuckSlide.X/Y`).
    pub fn offset(&self) -> (f64, f64) {
        (
            self.motion.value(Channel::TuckX).unwrap_or(self.resting.0),
            self.motion.value(Channel::TuckY).unwrap_or(self.resting.1),
        )
    }

    /// Whether the slide is still animating (C# `TuckMoving`).
    pub fn moving(&self) -> bool {
        self.motion.active()
    }

    /// C# `SetTucked`. A manual hide cancels an in-flight reveal even when its destination is
    /// already zero.
    pub fn set_tucked(
        &mut self,
        s: &Settings,
        value: bool,
        user_hidden: bool,
        cfg: &MotionConfig,
        now: f64,
    ) -> TuckOutcome {
        let _ = now;
        let changed = self.tucked != value;
        self.tucked = value;
        let dest = if value {
            AutoTuckPolicy::destination(s)
        } else {
            (0.0, 0.0)
        };
        if user_hidden {
            self.destination = dest;
            self.motion.stop();
            self.resting = dest;
            return TuckOutcome {
                changed,
                ..TuckOutcome::default()
            };
        }
        if !changed
            && (dest.1 - self.destination.1).abs() < 0.001
            && (dest.0 - self.destination.0).abs() < 0.001
            && (cfg.enabled || !self.motion.active())
        {
            return TuckOutcome {
                unchanged: true,
                ..TuckOutcome::default()
            };
        }
        self.destination = dest;
        let current = self.offset();
        if !cfg.enabled {
            self.motion.stop();
            self.resting = dest;
        } else {
            self.motion.set_fps(cfg.frame_limit);
            self.motion
                .go(Channel::TuckY, current.1, dest.1, s.animation_ms, true);
            self.motion
                .go(Channel::TuckX, current.0, dest.0, s.animation_ms, true);
        }
        TuckOutcome {
            changed,
            contract_surface: value && changed,
            reveal: !value && changed,
            unchanged: false,
        }
    }

    /// Advances the slide.
    pub fn update(&mut self, now: f64, cfg: &MotionConfig) -> Step {
        let step = self.motion.update(now, cfg.enabled);
        if step == Step::Finished {
            self.resting = self.offset();
        }
        step
    }

    pub fn next_frame(&self, now: f64) -> Option<f64> {
        self.motion.next_frame(now)
    }
}

/// C# `VisiblePoint`/`VisibleContour`: whether `(x, y)` (host-local) hits the painted, possibly
/// tucked and clipped island. `clip` is the dock boundary inside the host window.
pub fn visible_point(contour: &Contour, offset: (f64, f64), clip: &RectF, x: f64, y: f64) -> bool {
    contour.contains_translated(offset.0, offset.1, clip, x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_tuck_leaves_six_dip_at_the_display_edge() {
        assert_eq!(AutoTuckPolicy::offset(44.0, 6.0), -38.0);
        assert_eq!(AutoTuckPolicy::offset(28.0, 6.0), -22.0);
        assert_eq!(AutoTuckPolicy::offset(72.0, 6.0), -66.0);
        // The edge is clamped to 0..12.
        assert_eq!(AutoTuckPolicy::offset(44.0, 30.0), -32.0);
        assert_eq!(AutoTuckPolicy::offset(44.0, -3.0), -44.0);
    }

    #[test]
    fn short_surfaces_are_never_moved_below_the_edge() {
        assert_eq!(AutoTuckPolicy::offset(2.0, 6.0), 0.0);
    }

    #[test]
    fn inactive_state_tucks_and_engagement_retains_reveal() {
        for expanded in [false, true] {
            for preview in [false, true] {
                assert!(AutoTuckPolicy::should_tuck(
                    true, false, expanded, preview, false
                ));
                assert!(!AutoTuckPolicy::should_tuck(
                    true, false, expanded, preview, true
                ));
            }
        }
    }

    #[test]
    fn manual_hide_cannot_become_an_automatic_tuck() {
        assert!(!AutoTuckPolicy::should_tuck(
            true, true, false, false, false
        ));
        assert!(!AutoTuckPolicy::can_hover(true, true, true));
    }

    #[test]
    fn ending_suppression_restores_the_normal_state() {
        assert!(!AutoTuckPolicy::should_tuck(
            false, false, false, false, false
        ));
        assert!(!AutoTuckPolicy::can_hover(false, false, true));
    }

    #[test]
    fn only_a_tucked_automatic_edge_can_reveal_a_hover() {
        assert!(AutoTuckPolicy::can_hover(false, true, true));
        assert!(!AutoTuckPolicy::can_hover(false, true, false));
    }

    #[test]
    fn translated_contour_retains_a_narrow_clickable_center() {
        for notch in [false, true] {
            let contour = Contour::create(300.0, 44.0, 22.0, notch, 0.0);
            let clip = RectF::new(0.0, 0.0, 300.0, 400.0);
            let off = (0.0, AutoTuckPolicy::offset(44.0, 6.0));
            assert!(visible_point(&contour, off, &clip, 150.0, 2.0), "{notch}");
            assert!(!visible_point(&contour, off, &clip, 150.0, 8.0), "{notch}");
            assert!(!visible_point(&contour, off, &clip, 1.0, 2.0), "{notch}");
            let b = contour.visible_bounds(off.0, off.1, &clip);
            assert!((b.h - 6.0).abs() < 0.01, "{notch}: {b:?}");
        }
    }

    #[test]
    fn destination_by_edge() {
        let mut s = Settings {
            width: 300.0,
            height: 44.0,
            tuck_edge: 6.0,
            dock_edge: "Top".into(),
            ..Settings::default()
        };
        assert_eq!(AutoTuckPolicy::destination(&s), (0.0, -38.0));
        s.dock_edge = "Bottom".into();
        assert_eq!(AutoTuckPolicy::destination(&s), (0.0, 38.0));
        s.dock_edge = "Left".into();
        assert_eq!(AutoTuckPolicy::destination(&s), (-294.0, 0.0));
        s.dock_edge = "Right".into();
        assert_eq!(AutoTuckPolicy::destination(&s), (294.0, 0.0));
    }

    #[test]
    fn environment_decision() {
        assert_eq!(
            AutoTuckPolicy::environment(true, false, false, false),
            Some(true)
        );
        assert_eq!(AutoTuckPolicy::environment(true, false, true, false), None);
        assert_eq!(AutoTuckPolicy::environment(true, true, false, false), None);
        assert_eq!(
            AutoTuckPolicy::environment(false, false, true, true),
            Some(false)
        );
    }

    #[test]
    fn controller_slides_and_settles() {
        let s = Settings::default();
        let cfg = MotionConfig::from_settings(&s, true);
        let mut c = TuckController::new();
        let out = c.set_tucked(&s, true, false, &cfg, 0.0);
        assert!(out.changed && out.contract_surface && !out.reveal);
        assert!(c.moving());
        assert!(c.next_frame(0.0).is_some());
        let mut now = 0.0;
        while c.update(now, &cfg) != Step::Finished {
            now += 1.0 / 60.0;
            assert!(now < 5.0);
        }
        let (dx, dy) = AutoTuckPolicy::destination(&s);
        assert_eq!(c.offset(), (dx, dy));
        assert!(!c.moving());
        assert_eq!(c.next_frame(now), None);
        // Setting the same state again is a no-op.
        assert!(c.set_tucked(&s, true, false, &cfg, now).unchanged);
        // Reveal slides back.
        let out = c.set_tucked(&s, false, false, &cfg, now);
        assert!(out.reveal && c.moving());
    }

    #[test]
    fn manual_hide_snaps_and_reduce_motion_does_not_animate() {
        let s = Settings::default();
        let cfg = MotionConfig::from_settings(&s, true);
        let mut c = TuckController::new();
        c.set_tucked(&s, true, false, &cfg, 0.0);
        assert!(c.moving());
        // A manual hide cancels the in-flight slide.
        c.set_tucked(&s, true, true, &cfg, 0.1);
        assert!(!c.moving());
        assert_eq!(c.offset(), AutoTuckPolicy::destination(&s));
        let reduced = Settings {
            reduce_motion: true,
            ..Settings::default()
        };
        let cfg = MotionConfig::from_settings(&reduced, true);
        let mut c = TuckController::new();
        let out = c.set_tucked(&reduced, true, false, &cfg, 0.0);
        assert!(out.changed && !c.moving());
        assert_eq!(c.offset(), AutoTuckPolicy::destination(&reduced));
    }

    #[test]
    fn requested_combines_manual_always_and_environment() {
        let mut s = Settings::default();
        let mut c = TuckController::new();
        assert!(!c.requested(&s, false));
        assert!(c.requested(&s, true));
        s.always_tucked = true;
        assert!(c.requested(&s, false));
        s.always_tucked = false;
        assert!(c.toggle_manual());
        assert!(c.requested(&s, false));
        assert!(!c.toggle_manual());
    }
}
