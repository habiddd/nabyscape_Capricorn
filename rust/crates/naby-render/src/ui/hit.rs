//! Focus outline for custom-drawn rows and tiles; `hit_area` and `pointer_over` live in
//! `custom.rs`.

use super::Ui;
use crate::canvas::Rect;

impl Ui<'_> {
    /// The keyboard focus outline of the toolkit (2 px accent, inset 2), drawn only while the
    /// focus came from the keyboard. Custom widgets call it when `Response::focused`.
    pub fn focus_outline(&mut self, rect: Rect, radius: f32) {
        self.focus_ring(rect, radius);
    }
}
