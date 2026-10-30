//! Small shared helpers of the overlay: hit rectangles registered while painting, text helpers
//! and formatting.

use naby_core::color::Rgba;
use naby_render::{Canvas, Fonts, Rect, TextStyle, Weight};

/// Everything in the overlay that can be pressed or dragged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hot {
    /// The whole closed dock (opens the panel).
    Surface,
    PeekViewport,
    PeekPlay,
    PeekPrev,
    PeekNext,
    PeekMute,
    PeekVolume,
    PeekOutput,
    PeekMixer,
    PeekDot(u8),
    PeekArrow(i8),
    PeekToggle,
    PeekExtend,
    PeekReset,
    PeekEdit,
    PeekDetach,
    PeekOpen,
    PeekExtra(u8),
    NoticeDismiss,
    NoticeNext,
    NoticeClear,
    NoticeFolder,
    NoticeShelf,
    NoticeSettings,
    NoticeCard,
    PanelTab(u8),
    PanelSettings,
    PanelAssistant,
    PanelBody,
}

/// Hit rectangles of the last paint, topmost last. Reused between frames (no allocation once
/// warmed up).
#[derive(Default)]
pub struct HotMap {
    items: Vec<(Hot, Rect)>,
}

impl HotMap {
    pub fn clear(&mut self) {
        self.items.clear();
    }

    pub fn add(&mut self, hot: Hot, rect: Rect) {
        self.items.push((hot, rect));
    }

    /// The topmost rectangle containing the point.
    pub fn hit(&self, p: (f32, f32)) -> Option<Hot> {
        self.items
            .iter()
            .rev()
            .find(|(_, r)| r.contains_pt(p))
            .map(|(h, _)| *h)
    }

    pub fn rect(&self, hot: Hot) -> Option<Rect> {
        self.items.iter().find(|(h, _)| *h == hot).map(|(_, r)| *r)
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

/// Draws `text` centered in `rect` (single line, ellipsized to the rect width).
pub fn text_centered(
    canvas: &mut Canvas,
    fonts: &Fonts,
    rect: Rect,
    text: &str,
    style: &TextStyle,
) {
    let shown = fonts.ellipsize(text, style, rect.w);
    let (w, h) = fonts.measure(&shown, style);
    canvas.text(
        fonts,
        rect.x + (rect.w - w) / 2.0,
        rect.y + (rect.h - h) / 2.0,
        &shown,
        style,
        None,
    );
}

/// Draws `text` vertically centered in a row starting at `x`.
pub fn text_left(
    canvas: &mut Canvas,
    fonts: &Fonts,
    x: f32,
    row: Rect,
    text: &str,
    style: &TextStyle,
    max_w: f32,
) -> f32 {
    let h = fonts.line_height(style);
    canvas.text(
        fonts,
        x,
        row.y + (row.h - h) / 2.0,
        text,
        style,
        Some(max_w),
    )
}

/// Text style helper.
pub fn style(size: f32, weight: Weight, color: Rgba) -> TextStyle {
    TextStyle {
        size,
        weight,
        color,
    }
}

/// `m:ss` or `h:mm:ss` for a time in seconds (`TimeSpan.ToString(@"m\:ss")`).
pub fn format_clock(seconds: f64, long: bool) -> String {
    let total = if seconds.is_finite() {
        seconds.max(0.0) as u64
    } else {
        0
    };
    if long {
        format!("{}:{:02}:{:02}", total / 3600, total / 60 % 60, total % 60)
    } else {
        format!("{}:{:02}", total / 60, total % 60)
    }
}

/// Opacity-scaled copy of a color.
pub fn fade(c: Rgba, opacity: f32) -> Rgba {
    c.with_alpha((f32::from(c.a) * opacity.clamp(0.0, 1.0)).round() as u8)
}

/// Seconds until the next multiple of `period` seconds of wall-clock time, never zero.
pub fn until_boundary(seconds_into_period: f64, period: f64) -> f64 {
    let left = period - seconds_into_period.rem_euclid(period);
    if left < 0.001 {
        period
    } else {
        left
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_format_matches_timespan() {
        assert_eq!(format_clock(65.0, false), "1:05");
        assert_eq!(format_clock(3725.0, true), "1:02:05");
        assert_eq!(format_clock(-4.0, false), "0:00");
        assert_eq!(format_clock(f64::NAN, false), "0:00");
    }

    #[test]
    fn hot_map_returns_the_topmost_hit() {
        let mut map = HotMap::default();
        map.add(Hot::Surface, Rect::new(0.0, 0.0, 100.0, 40.0));
        map.add(Hot::PeekPlay, Rect::new(10.0, 10.0, 20.0, 20.0));
        assert_eq!(map.hit((15.0, 15.0)), Some(Hot::PeekPlay));
        assert_eq!(map.hit((60.0, 15.0)), Some(Hot::Surface));
        assert_eq!(map.hit((200.0, 15.0)), None);
        map.clear();
        assert!(map.is_empty());
    }

    #[test]
    fn boundary_math() {
        assert!((until_boundary(59.0, 60.0) - 1.0).abs() < 1e-9);
        assert_eq!(until_boundary(0.0, 60.0), 60.0);
    }
}
