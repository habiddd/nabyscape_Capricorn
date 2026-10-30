//! Keeping the Settings window on a work area (C# `SettingsBounds.cs`): the pure rectangle rules
//! the Windows host applies after moves, resizes, DPI and display changes.

/// A rectangle in physical pixels (`left`, `top`, `width`, `height`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Bounds {
    pub const fn new(x: f64, y: f64, w: f64, h: f64) -> Bounds {
        Bounds { x, y, w, h }
    }

    pub fn right(&self) -> f64 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f64 {
        self.y + self.h
    }

    /// True when `other` lies completely inside.
    pub fn contains(&self, other: &Bounds) -> bool {
        other.x >= self.x - 1e-6
            && other.y >= self.y - 1e-6
            && other.right() <= self.right() + 1e-6
            && other.bottom() <= self.bottom() + 1e-6
    }
}

/// Minimum window size in logical pixels (`MinWidth` / `MinHeight`).
pub const MIN_SIZE: (f64, f64) = (480.0, 300.0);
/// Initial window size in logical pixels.
pub const INITIAL_SIZE: (f64, f64) = (1100.0, 780.0);

/// `ShouldFitSettingsBounds`: while the user drags or sizes the window it may cross displays;
/// containment is applied when the gesture ends or after a display/DPI recovery.
pub fn should_fit(interactive: bool, recovery: bool) -> bool {
    !interactive || recovery
}

/// `FitWorkingArea`: shrinks `bounds` to `area` (never below the minimum size) and moves it
/// inside. `scale` is the monitor DPI scale.
pub fn fit_working_area(bounds: Bounds, area: Bounds, scale: f64) -> Bounds {
    let mut width = bounds.w.min(area.w);
    let mut height = bounds.h.min(area.h);
    width = width.max((MIN_SIZE.0 * scale).min(area.w));
    height = height.max((MIN_SIZE.1 * scale).min(area.h));
    Bounds::new(
        bounds.x.clamp(area.x, (area.right() - width).max(area.x)),
        bounds.y.clamp(area.y, (area.bottom() - height).max(area.y)),
        width,
        height,
    )
}

/// The first-open rectangle: centered on `area`, 1100 x 780 logical pixels but never larger
/// than the area minus a 32 px margin.
pub fn initial_bounds(area: Bounds, scale: f64) -> Bounds {
    let width = (INITIAL_SIZE.0 * scale).min(area.w - 32.0);
    let height = (INITIAL_SIZE.1 * scale).min(area.h - 32.0);
    Bounds::new(
        area.x + (area.w - width) / 2.0,
        area.y + (area.h - height) / 2.0,
        width,
        height,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fits_small_detached_and_scaled_work_areas() {
        for scale in [1.0, 1.5, 2.0] {
            for area in [
                Bounds::new(0.0, 0.0, 480.0 * scale, 300.0 * scale),
                Bounds::new(-1920.0, 80.0, 1280.0, 720.0),
            ] {
                let result =
                    fit_working_area(Bounds::new(2800.0, -800.0, 1700.0, 1200.0), area, scale);
                assert!(area.contains(&result));
                assert!(result.w >= (480.0 * scale).min(area.w));
                assert!(result.h >= (300.0 * scale).min(area.h));
            }
        }
    }

    #[test]
    fn interactive_movement_crosses_displays_before_containment() {
        let left = Bounds::new(0.0, 0.0, 1920.0, 1080.0);
        let right = Bounds::new(1920.0, 0.0, 1920.0, 1080.0);
        for x in [1000.0, 1300.0, 1500.0, 1700.0, 1900.0, 2100.0] {
            let moving = Bounds::new(x, 100.0, 900.0, 700.0);
            let area = if x + moving.w / 2.0 < 1920.0 {
                left
            } else {
                right
            };
            let result = if should_fit(true, false) {
                fit_working_area(moving, area, 1.0)
            } else {
                moving
            };
            assert_eq!(result, moving);
        }
        let destination = Bounds::new(2100.0, 100.0, 900.0, 700.0);
        assert!(should_fit(false, false));
        assert_eq!(fit_working_area(destination, right, 1.0), destination);
        let detached = Bounds::new(2600.0, -100.0, 900.0, 700.0);
        assert!(should_fit(true, true));
        assert!(left.contains(&fit_working_area(detached, left, 1.0)));
        let scaled = Bounds::new(1700.0, 900.0, 1100.0, 780.0);
        assert!(right.contains(&fit_working_area(scaled, right, 1.5)));
    }

    #[test]
    fn first_open_is_centered_with_a_margin() {
        let area = Bounds::new(0.0, 0.0, 1920.0, 1040.0);
        let b = initial_bounds(area, 1.0);
        assert_eq!((b.w, b.h), (1100.0, 780.0));
        assert_eq!((b.x, b.y), (410.0, 130.0));
        let small = initial_bounds(Bounds::new(0.0, 0.0, 800.0, 600.0), 1.0);
        assert_eq!((small.w, small.h), (768.0, 568.0));
    }
}
