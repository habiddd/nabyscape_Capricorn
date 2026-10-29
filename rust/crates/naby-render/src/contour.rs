//! Painting the island outline ([`naby_core::motion::Contour`]) with the canvas.

use crate::canvas::{Canvas, Fill};
use naby_core::motion::{Contour, PathCmd};
use tiny_skia::{Path, PathBuilder, Transform};

/// Builds a canvas path from `contour` scaled by `scale` and translated by `(dx, dy)` pixels.
pub fn contour_path(contour: &Contour, dx: f32, dy: f32, scale: f32) -> Option<Path> {
    let mut pb = PathBuilder::new();
    let p = |x: f64, y: f64| (x as f32 * scale + dx, y as f32 * scale + dy);
    for cmd in contour.commands() {
        match *cmd {
            PathCmd::MoveTo(x, y) => {
                let (x, y) = p(x, y);
                pb.move_to(x, y);
            }
            PathCmd::LineTo(x, y) => {
                let (x, y) = p(x, y);
                pb.line_to(x, y);
            }
            PathCmd::CubicTo { c1, c2, end } => {
                let (a, b) = p(c1.0, c1.1);
                let (c, d) = p(c2.0, c2.1);
                let (e, f) = p(end.0, end.1);
                pb.cubic_to(a, b, c, d, e, f);
            }
            PathCmd::Close => pb.close(),
        }
    }
    pb.finish()
}

impl Canvas {
    /// Fills the island outline.
    pub fn fill_contour(&mut self, contour: &Contour, dx: f32, dy: f32, scale: f32, fill: &Fill) {
        if let Some(path) = contour_path(contour, dx, dy, scale) {
            self.fill_path(&path, fill);
        }
    }

    /// Strokes the island outline with a `width` pixel line.
    pub fn stroke_contour(
        &mut self,
        contour: &Contour,
        dx: f32,
        dy: f32,
        scale: f32,
        fill: &Fill,
        width: f32,
    ) {
        if let Some(path) = contour_path(contour, dx, dy, scale) {
            self.stroke_path(&path, fill, width, Transform::identity());
        }
    }

    /// Clips to the island outline until [`Canvas::pop_clip`].
    pub fn push_clip_contour(&mut self, contour: &Contour, dx: f32, dy: f32, scale: f32) {
        let path = contour_path(contour, dx, dy, scale);
        self.push_clip_path(path.as_ref());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use naby_core::color::Rgba;

    #[test]
    fn notch_and_pill_fill_their_interior_only() {
        for notch in [false, true] {
            let contour = Contour::create(100.0, 40.0, 16.0, notch, 0.0);
            let mut canvas = Canvas::new(120, 60);
            canvas.fill_contour(&contour, 10.0, 5.0, 1.0, &Fill::Solid(Rgba::WHITE));
            assert_eq!(canvas.pixel(60, 25).a, 255, "centre is filled");
            assert_eq!(canvas.pixel(2, 2).a, 0, "outside stays clear");
        }
    }

    #[test]
    fn clip_limits_painting_to_the_contour() {
        let contour = Contour::create(100.0, 40.0, 20.0, false, 0.0);
        let mut canvas = Canvas::new(100, 40);
        canvas.push_clip_contour(&contour, 0.0, 0.0, 1.0);
        canvas.fill_rect(canvas.bounds(), Rgba::WHITE);
        canvas.pop_clip();
        assert_eq!(canvas.pixel(50, 20).a, 255);
        assert_eq!(canvas.pixel(0, 0).a, 0, "rounded corner is clipped");
    }
}
