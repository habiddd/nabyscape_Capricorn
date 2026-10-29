//! A bare clickable region for custom-drawn rows and tiles (the pages draw the visuals and use
//! this for pointer, keyboard focus and Enter / Space activation).

use super::{Key, Response, Ui};
use crate::canvas::Rect;

impl Ui<'_> {
    /// Registers `rect` as an interactive area. It takes part in Tab traversal when
    /// `focusable`, claims the press (so a focused text field loses focus) and reports
    /// `hovered`, `pressed` and `clicked`; Enter / Space activate it while focused. The caller
    /// draws everything, including the focus ring if it wants one (`Response::focused`).
    pub fn hit_area(&mut self, rect: Rect, focusable: bool) -> Response {
        let id = self.next_id();
        let focused = focusable && self.focusable(id, rect);
        let it = self.interact(id, rect, focusable);
        let mut clicked = it.clicked;
        if focused && (self.key_hit(Key::Enter) || self.key_hit(Key::Space)) {
            clicked = true;
        }
        let mut r = self.response(id, rect, &it, focused);
        r.clicked = clicked;
        r
    }

    /// True when the pointer is over `rect`, inside every enclosing scroll viewport and not
    /// under an open popup menu.
    pub fn pointer_over(&self, rect: Rect) -> bool {
        self.pointer_in(rect)
    }

    /// The keyboard focus outline of the toolkit (2 px accent, inset 2), drawn only while the
    /// focus came from the keyboard. Custom widgets call it when `Response::focused`.
    pub fn focus_outline(&mut self, rect: Rect, radius: f32) {
        self.focus_ring(rect, radius);
    }
}
