//! The stand-in surface of the app shell: the closed dock as a plain black pill with the
//! product name. The overlay task replaces it behind the same `Surface` seam.

use crate::app::Surface;
use naby_core::color::Rgba;
use naby_core::settings::Settings;
use naby_render::{Canvas, Fonts, InputState, Rect, TextStyle, Weight};
use std::time::Instant;

pub const TITLE: &str = "NabyScape";

pub struct PlaceholderSurface {
    width: f64,
    height: f64,
    radius: f64,
    scale: f32,
}

impl PlaceholderSurface {
    pub fn new(settings: &Settings, scale: f32) -> Self {
        let mut s = Self {
            width: 0.0,
            height: 0.0,
            radius: 0.0,
            scale,
        };
        s.apply_settings(settings);
        s
    }
}

impl Surface for PlaceholderSurface {
    fn paint(&mut self, canvas: &mut Canvas, fonts: &Fonts) {
        let (w, h) = self.size();
        canvas.clear(Rgba::TRANSPARENT);
        let pill = Rect::new(0.0, 0.0, w as f32, h as f32);
        let radius = (self.radius as f32 * self.scale).min(h as f32 / 2.0);
        canvas.fill_rrect(pill, radius, Rgba::BLACK);
        let style = TextStyle::new(12.5 * self.scale, Rgba::WHITE).weight(Weight::Semibold);
        let (tw, th) = fonts.measure(TITLE, &style);
        canvas.text(
            fonts,
            ((w as f32 - tw) / 2.0).max(0.0),
            ((h as f32 - th) / 2.0).max(0.0),
            TITLE,
            &style,
            Some(w as f32 - 2.0 * radius.min(w as f32 / 4.0)),
        );
    }

    fn input(&mut self, _input: &InputState) -> bool {
        false
    }

    fn size(&self) -> (u32, u32) {
        (
            ((self.width * self.scale as f64).round() as u32).max(1),
            ((self.height * self.scale as f64).round() as u32).max(1),
        )
    }

    fn next_frame(&self) -> Option<Instant> {
        None
    }

    fn set_scale(&mut self, scale: f32) {
        self.scale = scale.max(0.5);
    }

    fn apply_settings(&mut self, settings: &Settings) {
        self.width = settings.width;
        self.height = settings.height;
        self.radius = settings.radius;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> Settings {
        Settings {
            width: 228.0,
            height: 32.0,
            radius: 16.0,
            ..Settings::default()
        }
    }

    #[test]
    fn size_follows_settings_and_dpi_scale() {
        let mut s = PlaceholderSurface::new(&settings(), 1.0);
        assert_eq!(s.size(), (228, 32));
        s.set_scale(1.5);
        assert_eq!(s.size(), (342, 48));
        let mut wide = settings();
        wide.width = 300.0;
        s.apply_settings(&wide);
        assert_eq!(s.size(), (450, 48));
    }

    #[test]
    fn paints_a_black_pill_with_transparent_corners() {
        let mut s = PlaceholderSurface::new(&settings(), 1.0);
        let (w, h) = s.size();
        let mut canvas = Canvas::new(w, h);
        s.paint(&mut canvas, &Fonts::empty());
        assert_eq!(canvas.pixel(0, 0).a, 0, "corner outside the rounded rect");
        let edge = canvas.pixel(w / 2, 1);
        assert_eq!((edge.r, edge.g, edge.b, edge.a), (0, 0, 0, 255));
        let side = canvas.pixel(w - 1, h / 2);
        assert!(side.a > 200);
    }

    #[test]
    fn is_static_and_ignores_input() {
        let mut s = PlaceholderSurface::new(&settings(), 1.0);
        assert!(s.next_frame().is_none());
        assert!(!s.input(&InputState::press(3.0, 3.0)));
        assert!(!s.wake());
    }
}
