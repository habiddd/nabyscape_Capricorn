//! Hooks for views that paint their own controls (settings navigation, link rows, drag handles)
//! but still want the toolkit's pointer capture, Tab order and focus ring.

use super::Key;
use super::{mix, Response, Ui, UiMemory};
use crate::canvas::Rect;
use std::hash::{Hash, Hasher};

impl<'a> Ui<'a> {
    /// A custom interactive region over `rect`. It behaves like a button without painting
    /// anything: `hovered`, `pressed`, `clicked` (pointer release over it, or Enter / Space while
    /// focused) and `focused`. Pass `focusable` false for pointer-only regions (drag surfaces).
    /// Draw the control yourself, then call [`Ui::paint_focus_ring`] for keyboard focus.
    pub fn hit_area(&mut self, rect: Rect, focusable: bool) -> Response {
        let id = self.next_id();
        let focused = focusable && self.focusable(id, rect);
        let it = self.interact(id, rect, focusable);
        let mut r = self.response(id, rect, &it, focused);
        if focused && (self.key_hit(Key::Enter) || self.key_hit(Key::Space)) {
            r.clicked = true;
        }
        r.committed = r.clicked;
        r
    }

    /// The keyboard focus outline (shown only after Tab was used) for a custom control.
    pub fn paint_focus_ring(&mut self, resp: &Response, radius: f32) {
        if resp.focused {
            self.focus_ring(resp.rect, radius);
        }
    }

    /// The canvas together with the fonts, for custom painting (`Ui::canvas` and `Ui::fonts`
    /// cannot be borrowed at once).
    pub fn canvas_fonts(&mut self) -> (&mut crate::canvas::Canvas, &'a crate::text::Fonts) {
        (&mut *self.canvas, self.fonts)
    }

    /// Draws one line of text at (`x`, `y`) in logical pixels; returns the drawn width.
    pub fn draw_text(
        &mut self,
        x: f32,
        y: f32,
        text: &str,
        style: &crate::text::TextStyle,
        max_width: Option<f32>,
    ) -> f32 {
        self.canvas.text(self.fonts, x, y, text, style, max_width)
    }

    /// Height of one line of `style`.
    pub fn line_height(&self, style: &crate::text::TextStyle) -> f32 {
        self.line_h(style)
    }

    /// Whether the pointer is over `rect` (and not blocked by an open popup or outside the clip).
    pub fn pointer_over(&self, rect: Rect) -> bool {
        self.pointer_in(rect)
    }

    /// Asks the host to run the frame once more without input events.
    pub fn request_repaint(&mut self) {
        self.mem.repaint = true;
    }

    /// Asks for a repaint (or timer) in `secs` seconds.
    pub fn request_repaint_after(&mut self, secs: f64) {
        self.mem.want_repaint_after(secs);
    }

    /// Moves keyboard focus to the widget registered next with this id salt: the caller passes
    /// the `Response::id` it saw earlier.
    pub fn focus_id(&mut self, id: u64) {
        self.mem.focus = Some(id);
        self.mem.focus_visible = true;
        self.mem.repaint = true;
    }

    /// `true` while `id` (a `Response::id`) has keyboard focus.
    pub fn is_focused(&self, id: u64) -> bool {
        self.mem.focus == Some(id)
    }
}

fn scroll_key(salt: impl Hash) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    salt.hash(&mut h);
    // `Ui::hashed_id` under the root id scope.
    mix(0, h.finish())
}

impl UiMemory {
    /// Current offset of the scroll area opened with `salt` at the root id scope.
    pub fn scroll_offset(&self, salt: impl Hash) -> f32 {
        self.scrolls
            .get(&scroll_key(salt))
            .map_or(0.0, |s| s.offset)
    }

    /// Sets the offset of the scroll area opened with `salt` at the root id scope (clamped by
    /// the area on its next frame).
    pub fn set_scroll_offset(&mut self, salt: impl Hash, offset: f32) {
        super::capped(&mut self.scrolls, scroll_key(salt)).offset = offset.max(0.0);
        self.repaint = true;
    }

    /// Keyboard focus moved off every widget (for example after navigating to another page).
    pub fn drop_focus(&mut self) {
        self.clear_focus();
        self.repaint = true;
    }
}
