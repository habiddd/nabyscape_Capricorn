//! Hover peek interaction (C# `UI/Island/PeekInteractionPolicy.cs` and the carousel maths of
//! `PeekCarousel.cs`).

use super::geometry::{Contour, RectF};
use crate::settings::Settings;

/// Each wheel gesture advances one page; a pause begins the next gesture.
#[derive(Debug, Clone, Default)]
pub struct PeekWheelAccumulator {
    total: f64,
    last: i64,
    gated: bool,
}

impl PeekWheelAccumulator {
    /// Quiet time (ms) that separates two gestures.
    pub const GESTURE_GAP_MS: i64 = 300;
    /// Accumulated wheel delta that advances one page.
    pub const THRESHOLD: f64 = 100.0;

    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds one wheel event (`delta` in wheel units, `now_ms` monotonic). Returns `-1`, `0`
    /// or `1`: the page direction to move.
    pub fn push(&mut self, delta: i32, now_ms: i64) -> i32 {
        if now_ms - self.last >= Self::GESTURE_GAP_MS {
            self.total = 0.0;
            self.gated = false;
        }
        self.last = now_ms;
        if self.gated || delta == 0 {
            return 0;
        }
        if self.total != 0.0 && self.total.signum() != f64::from(delta).signum() {
            self.total = 0.0;
        }
        self.total += f64::from(delta);
        if self.total.abs() < Self::THRESHOLD {
            return 0;
        }
        self.gated = true;
        self.total.signum() as i32
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// Foreground-window bookkeeping and wheel ownership. Window handles are opaque `usize`
/// values; `0` means "none" (C# `IntPtr.Zero`).
pub struct VisibilityPolicy;

impl VisibilityPolicy {
    /// Own/menu and shell foregrounds must not replace the external window sampled before them.
    pub fn latest_external_foreground(
        external_foreground: usize,
        previous: usize,
        previous_eligible: bool,
    ) -> usize {
        if external_foreground != 0 {
            external_foreground
        } else if previous_eligible {
            previous
        } else {
            0
        }
    }

    pub fn retain_override(current: usize, external_foreground: usize) -> usize {
        if current != 0 && external_foreground != 0 && current != external_foreground {
            0
        } else {
            current
        }
    }

    pub fn carousel_owns_wheel(
        peeking: bool,
        horizontal: bool,
        focused_note: bool,
        over_note: bool,
    ) -> bool {
        peeking && (horizontal || !focused_note || !over_note)
    }
}

/// Safe content area inside the (notched) hover contour.
pub struct PeekLayout;

impl PeekLayout {
    /// C# `PeekLayout.SafeBounds`: the widest rectangle between y = 16 and `height - 20`
    /// whose top-left and bottom-left corners lie inside the contour.
    pub fn safe_bounds(width: f64, height: f64, radius: f64, notch: bool) -> RectF {
        let contour = Contour::create(width, height, radius, notch, 0.0);
        let top = 16.0;
        let bottom = height - 20.0;
        let mut inset = 32.0;
        while inset < width / 2.0 - 1.0
            && (!contour.contains(inset, top) || !contour.contains(inset, bottom))
        {
            inset += 1.0;
        }
        RectF::new(
            inset,
            top,
            (width - 2.0 * inset).max(1.0),
            (bottom - top).max(1.0),
        )
    }

    /// Width of one carousel page for a peek surface of `surface_width` DIP.
    pub fn page_width(surface_width: f64) -> f64 {
        surface_width - 64.0
    }
}

/// Inputs of the "may a peek start" decision.
#[derive(Debug, Clone, Copy, Default)]
pub struct PeekRequest {
    pub pages: usize,
    pub automatic_preview: bool,
    pub tuck_preview: bool,
    pub file_drag_active: bool,
    pub expanded: bool,
    pub peeking: bool,
    pub user_hidden: bool,
    pub tuck_requested: bool,
}

/// Peek decisions and carousel arithmetic.
pub struct PeekPolicy;

impl PeekPolicy {
    /// C# `PreviewActivity` guard: whether a peek may start.
    pub fn can_start(s: &Settings, r: &PeekRequest) -> bool {
        let forced = r.tuck_preview || r.file_drag_active;
        !(r.pages == 0
            || (!s.compact_layout && !forced)
            || (!s.hover_peek && !r.automatic_preview && !forced)
            || r.expanded
            || r.peeking
            || r.user_hidden
            || (r.tuck_requested && !forced))
    }

    /// First stage of a new peek: detailed at once without two-stage hover or for automatic
    /// previews, otherwise the 100 DIP strip until the detail delay elapses.
    pub fn starts_detailed(s: &Settings, automatic: bool) -> bool {
        !s.two_stage_hover || automatic
    }

    /// Next page id for a keyboard / wheel / button step (clamped, no wrap).
    pub fn navigate<'a>(ids: &'a [String], current: &str, direction: i32) -> Option<&'a str> {
        if ids.is_empty() {
            return None;
        }
        let index = ids
            .iter()
            .position(|id| id == current)
            .map_or(-1, |i| i as i32);
        let next = (index + direction.signum()).clamp(0, ids.len() as i32 - 1);
        Some(ids[next as usize].as_str())
    }

    /// C# `FinishPeekSwipe`: target index after releasing a drag of `offset` DIP.
    pub fn swipe_target(index: usize, count: usize, offset: f64, swipe_distance: f64) -> usize {
        if count == 0 {
            return 0;
        }
        let step = if offset < -swipe_distance {
            1
        } else if offset > swipe_distance {
            -1
        } else {
            0
        };
        (index as i64 + step).clamp(0, count as i64 - 1) as usize
    }

    /// C# `DragPeek`: displayed offset while dragging. Dragging past either end resists (x0.22);
    /// with motion off the carousel does not follow the pointer.
    pub fn drag_offset(
        distance: f64,
        index: usize,
        count: usize,
        page_width: f64,
        motion_enabled: bool,
    ) -> f64 {
        let mut offset = distance.clamp(-page_width, page_width);
        if (index == 0 && distance > 0.0) || (count > 0 && index == count - 1 && distance < 0.0) {
            offset *= 0.22;
        }
        if motion_enabled {
            offset
        } else {
            0.0
        }
    }

    /// Horizontal position of page `i` relative to the active page `index`.
    pub fn page_x(i: usize, index: usize, page_width: f64, offset: f64) -> f64 {
        (i as f64 - index as f64) * page_width + offset
    }

    /// Page dot: `(width, opacity)`.
    pub fn dot(active: bool) -> (f64, f64) {
        if active {
            (13.0, 0.9)
        } else {
            (4.0, 0.25)
        }
    }

    /// Height of the peek surface in DIP.
    pub fn height(s: &Settings, detailed: bool) -> f64 {
        if detailed {
            s.hover_height
        } else {
            super::geometry::BRIEF_PEEK_HEIGHT
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_wheel_deltas_accumulate() {
        let mut w = PeekWheelAccumulator::new();
        assert_eq!(w.push(30, 1000), 0);
        assert_eq!(w.push(30, 1010), 0);
        assert_eq!(w.push(40, 1020), 1);
    }

    #[test]
    fn a_continuous_wheel_gesture_advances_once() {
        let mut w = PeekWheelAccumulator::new();
        w.push(30, 1000);
        w.push(30, 1010);
        w.push(40, 1020);
        assert_eq!(w.push(120, 1040), 0);
        assert_eq!(w.push(120, 1100), 0);
    }

    #[test]
    fn a_new_wheel_gesture_can_reverse() {
        let mut w = PeekWheelAccumulator::new();
        w.push(30, 1000);
        w.push(30, 1010);
        w.push(40, 1020);
        w.push(120, 1040);
        w.push(120, 1100);
        assert_eq!(w.push(-120, 1400), -1);
    }

    #[test]
    fn direction_changes_discard_stale_deltas() {
        let mut w = PeekWheelAccumulator::new();
        w.push(120, 1000);
        w.reset();
        assert_eq!(w.push(60, 2000), 0);
        assert_eq!(w.push(-60, 2010), 0);
        assert_eq!(w.push(-40, 2020), -1);
        w.reset();
        assert_eq!(w.push(0, 5000), 0, "zero delta never advances");
    }

    #[test]
    fn foreground_tracking() {
        let (a, b) = (101usize, 202usize);
        let v = VisibilityPolicy::latest_external_foreground;
        assert_eq!(
            v(0, a, true),
            a,
            "tray show uses the last eligible external foreground"
        );
        assert_eq!(v(b, a, true), b, "tracking replaces the previous app");
        assert_eq!(
            v(0, a, false),
            0,
            "stale retained foreground falls back to no override"
        );
        assert_eq!(v(0, 0, false), 0);
        assert_eq!(
            v(b, a, false),
            b,
            "new external foreground wins over a stale one"
        );
        let r = VisibilityPolicy::retain_override;
        assert_eq!(r(a, 0), a, "own windows retain the override");
        assert_eq!(r(a, a), a, "same app retains the override");
        assert_eq!(r(a, b), 0, "another external app clears the override");
    }

    #[test]
    fn carousel_wheel_ownership() {
        let c = VisibilityPolicy::carousel_owns_wheel;
        assert!(
            !c(true, false, true, true),
            "focused note keeps vertical wheel over the editor"
        );
        assert!(c(true, false, true, false));
        assert!(c(true, true, true, true));
        assert!(
            !c(false, false, false, false),
            "expanded and closed bypass the preview wheel"
        );
    }

    #[test]
    fn safe_hover_rectangle_fits_every_contour() {
        for notch in [false, true] {
            for radius in [0.0, 16.0, 40.0] {
                for height in [100.0, 184.0, 280.0] {
                    let bounds = PeekLayout::safe_bounds(300.0, height, radius, notch);
                    let contour = Contour::create(300.0, height, radius, notch, 0.0);
                    assert!(bounds.w >= 216.0, "{notch}/{radius}/{height}: {bounds:?}");
                    assert!(
                        contour.contains(bounds.x, bounds.y),
                        "{notch}/{radius}/{height} TL"
                    );
                    assert!(
                        contour.contains(bounds.right(), bounds.bottom()),
                        "{notch}/{radius}/{height} BR"
                    );
                    if height == 184.0 {
                        assert!(bounds.h >= 148.0);
                    }
                }
            }
        }
    }

    #[test]
    fn peek_start_guard() {
        let s = Settings {
            hover_peek: true,
            compact_layout: true,
            ..Settings::default()
        };
        let ok = PeekRequest {
            pages: 3,
            ..PeekRequest::default()
        };
        assert!(PeekPolicy::can_start(&s, &ok));
        assert!(!PeekPolicy::can_start(&s, &PeekRequest { pages: 0, ..ok }));
        assert!(!PeekPolicy::can_start(
            &s,
            &PeekRequest {
                expanded: true,
                ..ok
            }
        ));
        assert!(!PeekPolicy::can_start(
            &s,
            &PeekRequest {
                peeking: true,
                ..ok
            }
        ));
        assert!(!PeekPolicy::can_start(
            &s,
            &PeekRequest {
                user_hidden: true,
                ..ok
            }
        ));
        assert!(!PeekPolicy::can_start(
            &s,
            &PeekRequest {
                tuck_requested: true,
                ..ok
            }
        ));
        // A tuck preview or a file drag overrides tuck, layout and the hover switch.
        let off = Settings {
            hover_peek: false,
            compact_layout: false,
            ..s.clone()
        };
        assert!(!PeekPolicy::can_start(&off, &ok));
        assert!(PeekPolicy::can_start(
            &off,
            &PeekRequest {
                tuck_preview: true,
                tuck_requested: true,
                ..ok
            }
        ));
        assert!(PeekPolicy::can_start(
            &off,
            &PeekRequest {
                file_drag_active: true,
                ..ok
            }
        ));
        // Automatic previews ignore only the hover switch, not the compact-layout requirement.
        let no_hover = Settings {
            hover_peek: false,
            ..s
        };
        assert!(PeekPolicy::can_start(
            &no_hover,
            &PeekRequest {
                automatic_preview: true,
                ..ok
            }
        ));
    }

    #[test]
    fn two_stage_start_and_heights() {
        let s = Settings::default();
        assert!(s.two_stage_hover);
        assert!(!PeekPolicy::starts_detailed(&s, false));
        assert!(PeekPolicy::starts_detailed(&s, true));
        let single = Settings {
            two_stage_hover: false,
            ..s.clone()
        };
        assert!(PeekPolicy::starts_detailed(&single, false));
        assert_eq!(PeekPolicy::height(&s, false), 100.0);
        assert_eq!(PeekPolicy::height(&s, true), s.hover_height);
    }

    #[test]
    fn carousel_navigation_clamps() {
        let ids: Vec<String> = ["media", "timer", "notes"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(PeekPolicy::navigate(&ids, "media", 1), Some("timer"));
        assert_eq!(PeekPolicy::navigate(&ids, "media", -1), Some("media"));
        assert_eq!(PeekPolicy::navigate(&ids, "notes", 5), Some("notes"));
        assert_eq!(PeekPolicy::navigate(&ids, "gone", 1), Some("media"));
        assert_eq!(PeekPolicy::navigate(&[], "x", 1), None);
        assert_eq!(PeekPolicy::swipe_target(1, 3, -60.0, 48.0), 2);
        assert_eq!(PeekPolicy::swipe_target(1, 3, 60.0, 48.0), 0);
        assert_eq!(
            PeekPolicy::swipe_target(1, 3, 48.0, 48.0),
            1,
            "needs to exceed the distance"
        );
        assert_eq!(PeekPolicy::swipe_target(0, 3, 90.0, 48.0), 0);
        assert_eq!(PeekPolicy::swipe_target(2, 3, -90.0, 48.0), 2);
        assert_eq!(PeekPolicy::swipe_target(0, 0, -90.0, 48.0), 0);
    }

    #[test]
    fn drag_resists_at_the_ends_and_respects_reduced_motion() {
        assert_eq!(PeekPolicy::drag_offset(100.0, 1, 3, 286.0, true), 100.0);
        assert!((PeekPolicy::drag_offset(100.0, 0, 3, 286.0, true) - 22.0).abs() < 1e-9);
        assert!((PeekPolicy::drag_offset(-100.0, 2, 3, 286.0, true) + 22.0).abs() < 1e-9);
        assert_eq!(PeekPolicy::drag_offset(900.0, 1, 3, 286.0, true), 286.0);
        assert_eq!(PeekPolicy::drag_offset(100.0, 1, 3, 286.0, false), 0.0);
        assert_eq!(PeekPolicy::page_x(2, 1, 286.0, -10.0), 276.0);
        assert_eq!(PeekPolicy::dot(true), (13.0, 0.9));
        assert_eq!(PeekPolicy::dot(false), (4.0, 0.25));
        assert_eq!(PeekLayout::page_width(350.0), 286.0);
    }
}
