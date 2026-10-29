//! Dock geometry: the island contour (C# `IslandGeometry.Create`) and dock placement
//! (C# `MonitorFollowPolicy.Place`, `OverlayWindow.Dimensions` sizes, `AutoTuckPolicy.Offset`).
//!
//! Monitor / work-area rectangles are physical pixels (as Win32 reports them); island sizes
//! are DIP and are multiplied by `dpi_scale` (1.0 = 96 dpi). Results are physical pixels.

use super::tuck::AutoTuckPolicy;
use crate::settings::{DockEdge, Settings};

/// Floating point rectangle (`x`, `y` top-left).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct RectF {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl RectF {
    pub const fn new(x: f64, y: f64, w: f64, h: f64) -> RectF {
        RectF { x, y, w, h }
    }

    pub fn right(&self) -> f64 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f64 {
        self.y + self.h
    }

    pub fn center(&self) -> (f64, f64) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    pub fn is_empty(&self) -> bool {
        self.w <= 0.0 || self.h <= 0.0
    }

    /// Half-open containment (left/top inclusive, right/bottom exclusive).
    pub fn contains(&self, px: f64, py: f64) -> bool {
        px >= self.x && py >= self.y && px < self.right() && py < self.bottom()
    }

    pub fn contains_rect(&self, other: &RectF) -> bool {
        other.x >= self.x - 1e-9
            && other.y >= self.y - 1e-9
            && other.right() <= self.right() + 1e-9
            && other.bottom() <= self.bottom() + 1e-9
    }

    pub fn translated(&self, dx: f64, dy: f64) -> RectF {
        RectF::new(self.x + dx, self.y + dy, self.w, self.h)
    }

    /// Overlap of two rectangles; zero-sized (anchored on `self`) when they do not touch.
    pub fn intersect(&self, other: &RectF) -> RectF {
        let l = self.x.max(other.x);
        let t = self.y.max(other.y);
        let r = self.right().min(other.right());
        let b = self.bottom().min(other.bottom());
        if r <= l || b <= t {
            RectF::new(l, t, 0.0, 0.0)
        } else {
            RectF::new(l, t, r - l, b - t)
        }
    }
}

/// Integer rectangle (monitor bounds, `System.Drawing.Rectangle`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RectI {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl RectI {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> RectI {
        RectI { x, y, w, h }
    }

    pub fn right(&self) -> i32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }

    pub fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x && py >= self.y && px < self.right() && py < self.bottom()
    }

    pub fn to_f(self) -> RectF {
        RectF::new(
            f64::from(self.x),
            f64::from(self.y),
            f64::from(self.w),
            f64::from(self.h),
        )
    }

    pub fn from_f(r: RectF) -> RectI {
        RectI::new(
            r.x.round() as i32,
            r.y.round() as i32,
            r.w.round() as i32,
            r.h.round() as i32,
        )
    }
}

/// Width and height in DIP.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SizeF {
    pub w: f64,
    pub h: f64,
}

/// Which of the island's surfaces is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DockState {
    Closed,
    Hover,
    Open,
    /// Automatically or manually tucked into the screen edge.
    Tucked,
}

/// The two stages of the hover peek (`TwoStageHover`): a short strip, then the full card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoverStage {
    Brief,
    Detailed,
}

/// Height of the brief (first-stage) hover strip, DIP.
pub const BRIEF_PEEK_HEIGHT: f64 = 100.0;
/// Margin kept between a peek / panel and the work-area edges, DIP (C# `workArea - 24`).
const SIDE_MARGIN: f64 = 24.0;
/// Margin kept below the panel, DIP (C# `workArea.Height - 16`).
const VERTICAL_MARGIN: f64 = 16.0;

// ---------------------------------------------------------------------------------------------
// Contour
// ---------------------------------------------------------------------------------------------

/// One drawing command of a contour path (WPF `StreamGeometry`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PathCmd {
    MoveTo(f64, f64),
    LineTo(f64, f64),
    CubicTo {
        c1: (f64, f64),
        c2: (f64, f64),
        end: (f64, f64),
    },
    Close,
}

/// The island outline plus a flattened polygon for hit testing (`FillContains`).
#[derive(Debug, Clone, PartialEq)]
pub struct Contour {
    cmds: Vec<PathCmd>,
    poly: Vec<(f64, f64)>,
    bounds: RectF,
}

const KAPPA_INV: f64 = 0.448;
const FLATTEN_STEPS: usize = 24;

impl Contour {
    /// C# `IslandGeometry.Create`. With `notch` the shoulders are concave so the screen edge
    /// flows into the sides of the notch; otherwise a pill (rounded rectangle).
    pub fn create(width: f64, height: f64, radius: f64, notch: bool, offset: f64) -> Contour {
        let width = width.max(1.0);
        let height = height.max(1.0);
        let radius = radius.max(0.0);
        let mut cmds = Vec::new();
        if !notch {
            let r = radius.min(height / 2.0).min(width / 2.0);
            let k = r * KAPPA_INV;
            let (l, t, rt, b) = (offset, 0.0, offset + width, height);
            cmds.push(PathCmd::MoveTo(l + r, t));
            cmds.push(PathCmd::LineTo(rt - r, t));
            cmds.push(PathCmd::CubicTo {
                c1: (rt - k, t),
                c2: (rt, t + k),
                end: (rt, t + r),
            });
            cmds.push(PathCmd::LineTo(rt, b - r));
            cmds.push(PathCmd::CubicTo {
                c1: (rt, b - k),
                c2: (rt - k, b),
                end: (rt - r, b),
            });
            cmds.push(PathCmd::LineTo(l + r, b));
            cmds.push(PathCmd::CubicTo {
                c1: (l + k, b),
                c2: (l, b - k),
                end: (l, b - r),
            });
            cmds.push(PathCmd::LineTo(l, t + r));
            cmds.push(PathCmd::CubicTo {
                c1: (l, t + k),
                c2: (l + k, t),
                end: (l + r, t),
            });
            cmds.push(PathCmd::Close);
        } else {
            let shoulder = 16.0f64.min(height * 0.36);
            let r = radius.min((height - shoulder).min((width - 2.0 * shoulder) / 2.0));
            let r = r.max(0.0);
            let left = offset + shoulder;
            let right = offset + width - shoulder;
            cmds.push(PathCmd::MoveTo(offset, 0.0));
            cmds.push(PathCmd::LineTo(offset + width, 0.0));
            cmds.push(PathCmd::CubicTo {
                c1: (right, 0.0),
                c2: (right, shoulder * 0.38),
                end: (right, shoulder),
            });
            cmds.push(PathCmd::LineTo(right, height - r));
            cmds.push(PathCmd::CubicTo {
                c1: (right, height - r * KAPPA_INV),
                c2: (right - r * KAPPA_INV, height),
                end: (right - r, height),
            });
            cmds.push(PathCmd::LineTo(left + r, height));
            cmds.push(PathCmd::CubicTo {
                c1: (left + r * KAPPA_INV, height),
                c2: (left, height - r * KAPPA_INV),
                end: (left, height - r),
            });
            cmds.push(PathCmd::LineTo(left, shoulder));
            cmds.push(PathCmd::CubicTo {
                c1: (left, shoulder * 0.38),
                c2: (left, 0.0),
                end: (offset, 0.0),
            });
            cmds.push(PathCmd::Close);
        }
        let poly = flatten(&cmds);
        let bounds = poly_bounds(&poly);
        Contour { cmds, poly, bounds }
    }

    pub fn commands(&self) -> &[PathCmd] {
        &self.cmds
    }

    pub fn bounds(&self) -> RectF {
        self.bounds
    }

    /// C# `Geometry.FillContains(point)`: non-zero winding of the flattened outline.
    pub fn contains(&self, x: f64, y: f64) -> bool {
        let n = self.poly.len();
        if n < 3 {
            return false;
        }
        let mut winding = 0i32;
        for i in 0..n {
            let (x0, y0) = self.poly[i];
            let (x1, y1) = self.poly[(i + 1) % n];
            let cross = (x1 - x0) * (y - y0) - (x - x0) * (y1 - y0);
            if y0 <= y {
                if y1 > y && cross > 0.0 {
                    winding += 1;
                }
            } else if y1 <= y && cross < 0.0 {
                winding -= 1;
            }
        }
        winding != 0
    }

    /// Containment after translating the contour by `(dx, dy)` and clipping to `clip`
    /// (the tucked, partially visible island).
    pub fn contains_translated(&self, dx: f64, dy: f64, clip: &RectF, x: f64, y: f64) -> bool {
        clip.contains(x, y) && self.contains(x - dx, y - dy)
    }

    /// Bounds of the part of the contour left inside `clip` after translating it by `(dx, dy)`.
    pub fn visible_bounds(&self, dx: f64, dy: f64, clip: &RectF) -> RectF {
        self.bounds.translated(dx, dy).intersect(clip)
    }
}

fn flatten(cmds: &[PathCmd]) -> Vec<(f64, f64)> {
    let mut out: Vec<(f64, f64)> = Vec::new();
    let mut current = (0.0, 0.0);
    for cmd in cmds {
        match *cmd {
            PathCmd::MoveTo(x, y) | PathCmd::LineTo(x, y) => {
                out.push((x, y));
                current = (x, y);
            }
            PathCmd::CubicTo { c1, c2, end } => {
                for step in 1..=FLATTEN_STEPS {
                    let t = step as f64 / FLATTEN_STEPS as f64;
                    let u = 1.0 - t;
                    let x = u * u * u * current.0
                        + 3.0 * u * u * t * c1.0
                        + 3.0 * u * t * t * c2.0
                        + t * t * t * end.0;
                    let y = u * u * u * current.1
                        + 3.0 * u * u * t * c1.1
                        + 3.0 * u * t * t * c2.1
                        + t * t * t * end.1;
                    out.push((x, y));
                }
                current = end;
            }
            PathCmd::Close => {}
        }
    }
    out
}

fn poly_bounds(poly: &[(f64, f64)]) -> RectF {
    let mut min_x = f64::MAX;
    let mut min_y = f64::MAX;
    let mut max_x = f64::MIN;
    let mut max_y = f64::MIN;
    for &(x, y) in poly {
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x);
        max_y = max_y.max(y);
    }
    if poly.is_empty() {
        return RectF::default();
    }
    RectF::new(min_x, min_y, max_x - min_x, max_y - min_y)
}

// ---------------------------------------------------------------------------------------------
// Placement
// ---------------------------------------------------------------------------------------------

/// C# `MonitorFollowPolicy.Place`: top-left of the (larger, transparent) host window in
/// physical pixels. `width`/`height` are the host size, `painted_width` the island's own width
/// (0 = same as the host). `bounds` is the monitor rectangle for a top dock and the work area
/// otherwise (see [`placement_area`]).
pub fn place_host(
    s: &Settings,
    bounds: RectI,
    scale: f64,
    width: i32,
    height: i32,
    tucked: bool,
    painted_width: i32,
) -> (i32, i32) {
    let compact = s.width * scale;
    let painted = if painted_width > 0 {
        painted_width
    } else {
        width
    };
    let inset = ((width - painted) / 2).max(0);
    let gap = (s.dock_inset * scale).round_ties_even() as i32;
    let edge = s.dock_edge();
    let mac_top = s.preset == "Mac" && edge == DockEdge::Top;
    let mut x = bounds.x
        + ((f64::from(bounds.w) - compact) * s.horizontal / 100.0 + compact / 2.0
            - f64::from(width) / 2.0) as i32;
    let mut y = bounds.y
        + (if tucked || mac_top {
            0.0
        } else {
            s.top * scale
        }) as i32;
    match edge {
        DockEdge::Bottom => {
            y = bounds.bottom() - height - if tucked { 0 } else { gap };
        }
        DockEdge::Left | DockEdge::Right => {
            let gap = if tucked { 0 } else { gap };
            x = if edge == DockEdge::Left {
                bounds.x - inset + gap
            } else {
                bounds.right() - painted - inset - gap
            };
            y = bounds.y + (f64::from(bounds.h - height) * s.dock_vertical / 100.0) as i32;
        }
        _ => {}
    }
    let min_x = bounds.x - inset;
    let max_x = min_x.max(bounds.right() - width + inset);
    let min_y = bounds.y;
    let max_y = min_y.max(bounds.bottom() - height);
    (x.max(min_x).min(max_x), y.max(min_y).min(max_y))
}

/// The rectangle placement is computed against: the full monitor for a top dock (the notch
/// reaches the very edge), the work area (taskbar excluded) for every other edge.
pub fn placement_area(edge: DockEdge, monitor: RectI, work: RectI) -> RectI {
    if edge == DockEdge::Top {
        monitor
    } else {
        work
    }
}

/// Dock sizing and placement.
pub struct IslandGeometry;

impl IslandGeometry {
    /// Closed (compact) size, DIP.
    pub fn closed_size(s: &Settings) -> SizeF {
        SizeF {
            w: s.width,
            h: s.height,
        }
    }

    /// Hover peek size, DIP: width limited to the screen minus a 24 DIP margin; the brief stage
    /// is a 100 DIP strip, the detailed stage is `HoverHeight`.
    pub fn hover_size(s: &Settings, area_dip: SizeF, stage: HoverStage) -> SizeF {
        SizeF {
            w: s.hover_width.min(area_dip.w - SIDE_MARGIN),
            h: match stage {
                HoverStage::Brief => BRIEF_PEEK_HEIGHT,
                HoverStage::Detailed => s.hover_height,
            },
        }
    }

    /// Expanded panel size, DIP (C# `Dimensions`). `content_height` is the measured height of
    /// the selected page in DIP; it only matters with `PanelAutoHeight` in compact layout.
    pub fn open_size(s: &Settings, area_dip: SizeF, content_height: Option<f64>) -> SizeF {
        let w = s.panel_width.min(area_dip.w - SIDE_MARGIN);
        let available = area_dip.h - VERTICAL_MARGIN;
        let mut desired = s.panel_max_height;
        if s.compact_layout && s.panel_auto_height {
            if let Some(content) = content_height {
                desired =
                    (content * s.panel_scale + 44.0).clamp(184.0, s.panel_max_height.max(184.0));
            }
        }
        SizeF {
            w,
            h: s.height.max(desired.min(available)),
        }
    }

    /// Width of the fixed host window in physical pixels: the widest surface it must contain
    /// (C# `WindowHostWidth`).
    pub fn host_width_px(
        s: &Settings,
        area_dip: SizeF,
        scale: f64,
        content_height: Option<f64>,
    ) -> i32 {
        let widest = Self::open_size(s, area_dip, content_height)
            .w
            .max(s.width)
            .max(Self::hover_size(s, area_dip, HoverStage::Detailed).w);
        (widest * scale).ceil() as i32
    }

    /// Painted rectangle (physical pixels) of the island in `state`, with the detailed hover
    /// stage and no measured content height. `area` is the placement rectangle
    /// ([`placement_area`]); `dpi_scale` 1.0 is 96 dpi.
    pub fn compute(s: &Settings, state: DockState, area: RectF, dpi_scale: f32) -> RectF {
        Self::compute_with(s, state, HoverStage::Detailed, None, area, dpi_scale)
    }

    /// [`IslandGeometry::compute`] with explicit hover stage and measured content height.
    /// Always fits inside `area`, so a new (smaller) monitor or work area simply recomputes.
    pub fn compute_with(
        s: &Settings,
        state: DockState,
        stage: HoverStage,
        content_height: Option<f64>,
        area: RectF,
        dpi_scale: f32,
    ) -> RectF {
        let scale = if dpi_scale.is_finite() && dpi_scale > 0.0 {
            f64::from(dpi_scale)
        } else {
            1.0
        };
        let area_dip = SizeF {
            w: area.w / scale,
            h: area.h / scale,
        };
        let size = match state {
            DockState::Closed | DockState::Tucked => Self::closed_size(s),
            DockState::Hover => Self::hover_size(s, area_dip, stage),
            DockState::Open => Self::open_size(s, area_dip, content_height),
        };
        let w_px = ((size.w * scale).ceil() as i32).clamp(1, (area.w.floor() as i32).max(1));
        let h_px = ((size.h * scale).ceil() as i32).clamp(1, (area.h.floor() as i32).max(1));
        let host = Self::host_width_px(s, area_dip, scale, content_height).max(w_px);
        let tucked = state == DockState::Tucked;
        let bounds = RectI::from_f(area);
        let (hx, hy) = place_host(s, bounds, scale, host, h_px, tucked, w_px);
        let inset = ((host - w_px) / 2).max(0);
        let mut rect = RectF::new(
            f64::from(hx + inset),
            f64::from(hy),
            f64::from(w_px),
            f64::from(h_px),
        );
        if tucked {
            let (dx, dy) = AutoTuckPolicy::destination(s);
            rect = rect.translated(dx * scale, dy * scale).intersect(&area);
        }
        rect
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor() -> RectF {
        RectF::new(0.0, 0.0, 1920.0, 1080.0)
    }

    fn base() -> Settings {
        Settings::default()
    }

    #[test]
    fn closed_rect_centered_top() {
        let s = base();
        let r = IslandGeometry::compute(&s, DockState::Closed, monitor(), 1.0);
        assert_eq!(r.w, s.width);
        assert_eq!(r.h, s.height);
        assert!((r.x + r.w / 2.0 - 960.0).abs() <= 1.0, "centered: {r:?}");
        // Default is a top dock with the Mac preset or a small top offset.
        assert!(r.y >= 0.0 && r.y <= s.top.max(0.0) + 1.0);
        // DPI scaling multiplies the size and keeps the centre.
        let r2 = IslandGeometry::compute(&s, DockState::Closed, monitor(), 1.5);
        assert_eq!(r2.w, (s.width * 1.5).ceil());
        assert!((r2.x + r2.w / 2.0 - 960.0).abs() <= 1.0);
    }

    #[test]
    fn top_dock_with_mac_preset_touches_the_screen_edge() {
        let s = Settings {
            preset: "Mac".into(),
            dock_edge: "Top".into(),
            top: 40.0,
            ..base()
        };
        let r = IslandGeometry::compute(&s, DockState::Closed, monitor(), 1.0);
        assert_eq!(r.y, 0.0);
        let floating = Settings {
            preset: "iPhone".into(),
            dock_edge: "Floating".into(),
            top: 40.0,
            ..base()
        };
        let r = IslandGeometry::compute(&floating, DockState::Closed, monitor(), 2.0);
        assert_eq!(r.y, 80.0);
    }

    #[test]
    fn dock_edge_left_vertical_percent() {
        let s = Settings {
            dock_edge: "Left".into(),
            dock_vertical: 25.0,
            dock_inset: 10.0,
            ..base()
        };
        let area = RectF::new(0.0, 0.0, 1920.0, 1040.0);
        let r = IslandGeometry::compute(&s, DockState::Closed, area, 1.0);
        assert_eq!(r.x, 10.0, "inset from the left edge");
        let expected = ((1040.0 - s.height) * 0.25).trunc();
        assert_eq!(r.y, expected);
        let right = Settings {
            dock_edge: "Right".into(),
            dock_vertical: 100.0,
            ..s.clone()
        };
        let r = IslandGeometry::compute(&right, DockState::Closed, area, 1.0);
        assert_eq!(r.right(), 1920.0 - 10.0);
        assert_eq!(r.bottom(), 1040.0);
    }

    #[test]
    fn bottom_dock_sits_above_the_inset() {
        let s = Settings {
            dock_edge: "Bottom".into(),
            dock_inset: 12.0,
            ..base()
        };
        let area = RectF::new(0.0, 0.0, 1920.0, 1040.0);
        let r = IslandGeometry::compute(&s, DockState::Closed, area, 1.0);
        assert_eq!(r.bottom(), 1040.0 - 12.0);
    }

    #[test]
    fn tucked_leaves_tuck_edge_visible() {
        let s = Settings {
            tuck_edge: 6.0,
            dock_edge: "Top".into(),
            ..base()
        };
        let r = IslandGeometry::compute(&s, DockState::Tucked, monitor(), 1.0);
        assert_eq!((r.y, r.h), (0.0, 6.0));
        let r = IslandGeometry::compute(&s, DockState::Tucked, monitor(), 1.5);
        assert_eq!(r.h, 9.0, "6 DIP at 150%");
        for (edge, vertical) in [("Left", false), ("Right", false), ("Bottom", true)] {
            let s = Settings {
                dock_edge: edge.into(),
                tuck_edge: 6.0,
                ..base()
            };
            let area = RectF::new(0.0, 0.0, 1920.0, 1040.0);
            let r = IslandGeometry::compute(&s, DockState::Tucked, area, 1.0);
            if vertical {
                assert_eq!(r.h, 6.0, "{edge}");
                assert_eq!(r.bottom(), 1040.0);
            } else {
                assert_eq!(r.w, 6.0, "{edge}");
                assert!(r.x == 0.0 || r.right() == 1920.0);
            }
        }
        // A hidden (zero) edge leaves nothing visible.
        let hidden = Settings {
            tuck_edge: 0.0,
            ..base()
        };
        assert!(IslandGeometry::compute(&hidden, DockState::Tucked, monitor(), 1.0).is_empty());
    }

    #[test]
    fn state_sizes_follow_settings() {
        let s = base();
        let area = RectF::new(0.0, 0.0, 1920.0, 1080.0);
        let hover = IslandGeometry::compute(&s, DockState::Hover, area, 1.0);
        assert_eq!((hover.w, hover.h), (s.hover_width, s.hover_height));
        let brief =
            IslandGeometry::compute_with(&s, DockState::Hover, HoverStage::Brief, None, area, 1.0);
        assert_eq!(brief.h, 100.0);
        let open = IslandGeometry::compute(&s, DockState::Open, area, 1.0);
        assert_eq!((open.w, open.h), (s.panel_width, s.panel_max_height));
        assert!(
            (open.x + open.w / 2.0 - 960.0).abs() <= 1.0,
            "stays centered"
        );
        // Auto height follows measured content within 184..PanelMaxHeight.
        let auto = Settings {
            panel_auto_height: true,
            compact_layout: true,
            panel_scale: 1.0,
            ..base()
        };
        let small = IslandGeometry::compute_with(
            &auto,
            DockState::Open,
            HoverStage::Detailed,
            Some(10.0),
            area,
            1.0,
        );
        assert_eq!(small.h, 184.0);
        let big = IslandGeometry::compute_with(
            &auto,
            DockState::Open,
            HoverStage::Detailed,
            Some(5000.0),
            area,
            1.0,
        );
        assert_eq!(big.h, auto.panel_max_height);
    }

    #[test]
    fn recomputes_inside_a_smaller_work_area() {
        let s = Settings {
            horizontal: 100.0,
            ..base()
        };
        for state in [
            DockState::Closed,
            DockState::Hover,
            DockState::Open,
            DockState::Tucked,
        ] {
            for dpi in [1.0f32, 1.25, 2.0] {
                let area = RectF::new(-1280.0, 200.0, 1280.0, 720.0);
                let r = IslandGeometry::compute(&s, state, area, dpi);
                assert!(
                    area.contains_rect(&r),
                    "{state:?} {dpi}: {r:?} not in {area:?}"
                );
            }
        }
        // A work area narrower than the panel shrinks the panel instead of overflowing.
        let narrow = RectF::new(0.0, 0.0, 360.0, 640.0);
        let r = IslandGeometry::compute(&s, DockState::Open, narrow, 1.0);
        assert!(narrow.contains_rect(&r));
        assert_eq!(r.w, 360.0 - 24.0);
    }

    #[test]
    fn bad_dpi_falls_back_to_one() {
        let s = base();
        let a = IslandGeometry::compute(&s, DockState::Closed, monitor(), 0.0);
        let b = IslandGeometry::compute(&s, DockState::Closed, monitor(), 1.0);
        assert_eq!(a, b);
        assert_eq!(
            IslandGeometry::compute(&s, DockState::Closed, monitor(), f32::NAN),
            b
        );
    }

    #[test]
    fn dock_can_reach_monitor_sides_despite_its_larger_transparent_host() {
        // C# MediaExperienceTests "Dock visible edges can reach monitor sides".
        let mut s = Settings {
            width: 228.0,
            horizontal: 0.0,
            top: 700.0,
            preset: "iPhone".into(),
            ..base()
        };
        s.dock_edge = "Floating".into();
        let bounds = RectI::new(0, 0, 1280, 800);
        let (x, y) = place_host(&s, bounds, 1.0, 760, 400, false, 228);
        assert!(x + (760 - 228) / 2 == 0 && y == 400);
        s.horizontal = 100.0;
        let (x, _) = place_host(&s, bounds, 1.0, 760, 400, false, 228);
        assert_eq!(x + (760 + 228) / 2, 1280);
    }

    #[test]
    fn mixed_bounds_placement_contains_island() {
        // C# Stage5Tests: negative, portrait and offset monitors at several scales.
        let all = [
            RectI::new(-1920, 0, 1920, 1080),
            RectI::new(0, -640, 1080, 1920),
            RectI::new(1920, 80, 1280, 720),
        ];
        for bounds in all {
            for scale in [1.0f64, 1.5, 2.0] {
                for horizontal in [0.0, 50.0, 100.0] {
                    let s = Settings {
                        horizontal,
                        top: 150.0,
                        dock_edge: "Floating".into(),
                        ..base()
                    };
                    let width = (640.0 * scale).min(f64::from(bounds.w)) as i32;
                    let height = (408.0 * scale).min(f64::from(bounds.h)) as i32;
                    let (x, y) = place_host(&s, bounds, scale, width, height, false, 0);
                    assert!(
                        x >= bounds.x
                            && y >= bounds.y
                            && x + width <= bounds.right()
                            && y + height <= bounds.bottom(),
                        "{bounds:?} {scale} {horizontal}"
                    );
                }
            }
        }
    }

    #[test]
    fn placement_area_uses_monitor_for_top_and_work_area_otherwise() {
        let monitor = RectI::new(0, 0, 1920, 1080);
        let work = RectI::new(0, 0, 1920, 1040);
        assert_eq!(placement_area(DockEdge::Top, monitor, work), monitor);
        assert_eq!(placement_area(DockEdge::Bottom, monitor, work), work);
    }

    #[test]
    fn contour_pill_and_notch_hit_testing() {
        let pill = Contour::create(300.0, 44.0, 22.0, false, 0.0);
        assert!(pill.contains(150.0, 22.0));
        assert!(!pill.contains(0.5, 0.5), "rounded corner is cut");
        assert!(!pill.contains(150.0, 50.0));
        assert!(!pill.contains(-1.0, 22.0));
        let notch = Contour::create(300.0, 44.0, 22.0, true, 0.0);
        assert!(notch.contains(150.0, 1.0));
        assert!(
            notch.contains(8.0, 0.3),
            "the shoulder flows into the screen edge"
        );
        assert!(
            !notch.contains(1.0, 30.0),
            "below the shoulder the sides are inset"
        );
        assert!(notch.contains(20.0, 30.0));
        assert_eq!(notch.bounds().w, 300.0);
        assert!(matches!(notch.commands()[0], PathCmd::MoveTo(0.0, 0.0)));
        assert!(matches!(notch.commands().last(), Some(PathCmd::Close)));
        // Offset translates the figure.
        let moved = Contour::create(100.0, 40.0, 10.0, false, 50.0);
        assert!(moved.contains(100.0, 20.0) && !moved.contains(20.0, 20.0));
        // Degenerate sizes still build a valid contour.
        let tiny = Contour::create(0.0, 0.0, 99.0, true, 0.0);
        assert!(tiny.bounds().w >= 1.0);
    }
}
