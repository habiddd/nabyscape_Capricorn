//! A single-line text field for the small editors of the pages (timer minutes, city search).
//! Caret, selection by drag or shift, copy/cut/paste commands, blink schedule.

use super::ctl::Ctl;
use naby_core::color::Rgba;
use naby_core::command::Command;
use naby_render::{Key, Rect, Weight};

const BLINK: f64 = 0.53;

#[derive(Default)]
pub struct EditState {
    id: Option<u32>,
    /// Caret and selection anchor as char indices.
    caret: usize,
    anchor: usize,
    blink_from: f64,
}

impl EditState {
    pub fn focused(&self) -> bool {
        self.id.is_some()
    }

    pub fn focused_id(&self) -> Option<u32> {
        self.id
    }

    pub fn blur(&mut self) {
        self.id = None;
    }

    pub fn next_blink(&self, now: f64) -> Option<f64> {
        self.id?;
        let phase = (now - self.blink_from).rem_euclid(BLINK * 2.0);
        Some(now + (BLINK - phase.rem_euclid(BLINK)).max(0.016))
    }

    fn selection(&self) -> (usize, usize) {
        (self.caret.min(self.anchor), self.caret.max(self.anchor))
    }
}

/// Options of [`Ctl::line_edit`].
pub struct EditOpts {
    pub dip: f32,
    pub max_chars: usize,
    pub centered: bool,
    /// Characters the field accepts.
    pub accept: fn(char) -> bool,
}

impl Default for EditOpts {
    fn default() -> Self {
        EditOpts {
            dip: 12.0,
            max_chars: 120,
            centered: false,
            accept: |c| !c.is_control(),
        }
    }
}

/// What the field did this frame.
#[derive(Debug, Clone, Copy, Default)]
pub struct EditOut {
    pub changed: bool,
    pub submitted: bool,
    pub focused: bool,
    /// Focus left the field this frame.
    pub left: bool,
}

fn byte_at(text: &str, chars: usize) -> usize {
    text.char_indices()
        .nth(chars)
        .map_or(text.len(), |(b, _)| b)
}

impl Ctl<'_> {
    /// The text field `id` has the keyboard.
    pub fn editing(&self, id: u32) -> bool {
        self.st.edit.focused_id() == Some(id)
    }

    /// Draws the text (no frame) inside `rect` and edits `text` in place. `placeholder` shows
    /// while empty and unfocused.
    pub fn line_edit(
        &mut self,
        id: u32,
        rect: Rect,
        text: &mut String,
        placeholder: &str,
        opts: &EditOpts,
        enabled: bool,
    ) -> EditOut {
        let mut out = EditOut::default();
        let s = self.s;
        let was_focused = self.st.edit.id == Some(id);
        #[cfg(test)]
        if !self.is_dry() {
            super::ctl::record(id, rect, enabled);
        }
        if !self.is_dry() && enabled {
            self.st.order.push(id);
            // Tab focus lands here.
            if self.st.focus == Some(id) && self.st.edit.id != Some(id) && self.st.focus_visible {
                self.st.edit.id = Some(id);
                self.st.edit.anchor = 0;
                self.st.edit.caret = text.chars().count();
                self.st.edit.blink_from = self.now;
            }
            let hovered = self.pointer_in(rect);
            let style = self.style(opts.dip, Weight::Regular, Rgba::WHITE);
            let pad = if rect.w < 48.0 * s { 1.0 * s } else { 6.0 * s };
            let origin = |w: f32| -> f32 {
                if opts.centered && w <= rect.w - 2.0 * pad {
                    rect.x + (rect.w - w) / 2.0
                } else {
                    rect.x + pad
                }
            };
            if hovered && self.input.pressed {
                let first = self.st.edit.id != Some(id);
                self.st.focus = Some(id);
                self.st.focus_visible = false;
                self.st.claimed = true;
                self.st.active = Some(id);
                self.st.edit.id = Some(id);
                self.st.edit.blink_from = self.now;
                if first {
                    self.st.edit.anchor = 0;
                    self.st.edit.caret = text.chars().count();
                } else if let Some((px, _)) = self.input.pointer {
                    let w = self.ctx.fonts.measure_width(text, &style);
                    let i = self.ctx.fonts.hit_index(text, &style, px - origin(w));
                    self.st.edit.caret = i;
                    if !self.input.shift {
                        self.st.edit.anchor = i;
                    }
                }
            } else if self.st.active == Some(id) && self.input.down && !self.input.pressed {
                if let Some((px, _)) = self.input.pointer {
                    let w = self.ctx.fonts.measure_width(text, &style);
                    self.st.edit.caret = self.ctx.fonts.hit_index(text, &style, px - origin(w));
                }
            }
            if self.st.edit.id == Some(id) {
                self.edit_keys(text, opts, &mut out);
            }
            out.focused = self.st.edit.id == Some(id);
        }
        out.left = was_focused && self.st.edit.id != Some(id) && !self.is_dry();
        if self.is_dry() {
            return out;
        }
        // ---- paint ----
        let th = self.ctx.theme;
        let style = self.style(opts.dip, Weight::Regular, th.control_text);
        let pad = if rect.w < 48.0 * s { 1.0 * s } else { 6.0 * s };
        let fonts = self.ctx.fonts;
        let lh = self.line_h(opts.dip, Weight::Regular);
        let focused = self.st.edit.id == Some(id);
        let tw = fonts.measure_width(text, &style);
        let avail = (rect.w - 2.0 * pad).max(1.0);
        let caret_x = if focused {
            let idx = self.st.edit.caret.min(text.chars().count());
            let b = byte_at(text, idx);
            fonts.measure_width(&text[..b], &style)
        } else {
            0.0
        };
        // Scroll so the caret stays visible when the text is wider than the field.
        let shift = if tw > avail {
            (caret_x - avail + 2.0 * s).max(0.0).min(tw - avail)
        } else {
            0.0
        };
        let x0 = if opts.centered && tw <= avail {
            rect.x + (rect.w - tw) / 2.0
        } else {
            rect.x + pad - shift
        };
        let y0 = rect.y + (rect.h - lh) / 2.0;
        let dim = th.text_dim;
        self.clip(rect.inset_xy(2.0 * s, 0.0));
        if focused {
            let (a, b) = self.st.edit.selection();
            if a != b {
                let xa = fonts.measure_width(&text[..byte_at(text, a)], &style);
                let xb = fonts.measure_width(&text[..byte_at(text, b)], &style);
                let sel = th.selection_fill.with_alpha(0xD9);
                self.rrect(Rect::new(x0 + xa, y0, xb - xa, lh), 2.0, sel);
            }
        }
        if text.is_empty() && !focused {
            let ph = opts.dip.min(12.0);
            let w = self.text_w(placeholder, ph, Weight::Regular);
            let px = if opts.centered {
                rect.x + (rect.w - w) / 2.0
            } else {
                rect.x + pad
            };
            self.text(px, y0, placeholder, ph, Weight::Regular, dim, Some(avail));
        } else {
            let color = th.control_text;
            self.text(x0, y0, text, opts.dip, Weight::Regular, color, None);
        }
        if focused {
            let on = ((self.now - self.st.edit.blink_from).rem_euclid(BLINK * 2.0)) < BLINK;
            if on {
                let a = self.st.edit.selection();
                if a.0 == a.1 {
                    let cx = (x0 + caret_x).round();
                    self.fill(Rect::new(cx, y0, (1.0 * s).max(1.0), lh), th.control_text);
                }
            }
        }
        self.unclip();
        out
    }

    fn edit_keys(&mut self, text: &mut String, opts: &EditOpts, out: &mut EditOut) {
        let mut chars = text.chars().count();
        let input = self.input;
        let state = &mut self.st.edit;
        state.caret = state.caret.min(chars);
        state.anchor = state.anchor.min(chars);
        let insert = |state: &mut EditState, text: &mut String, add: &str, chars: &mut usize| {
            let (a, b) = state.selection();
            let b0 = byte_at(text, a);
            let b1 = byte_at(text, b);
            text.replace_range(b0..b1, "");
            *chars -= b - a;
            let room = opts.max_chars.saturating_sub(*chars);
            let take: String = add
                .chars()
                .filter(|c| (opts.accept)(*c))
                .take(room)
                .collect();
            let n = take.chars().count();
            text.insert_str(b0, &take);
            *chars += n;
            state.caret = a + n;
            state.anchor = state.caret;
            state.blink_from = 0.0;
        };
        let before = text.clone();
        let mut copy: Option<String> = None;
        let mut submitted = false;
        for k in &input.keys {
            let sel = state.selection();
            match k.key {
                Key::Left => {
                    let to = if sel.0 != sel.1 && !k.shift {
                        sel.0
                    } else {
                        state.caret.saturating_sub(1)
                    };
                    state.caret = to;
                    if !k.shift {
                        state.anchor = to;
                    }
                }
                Key::Right => {
                    let to = if sel.0 != sel.1 && !k.shift {
                        sel.1
                    } else {
                        (state.caret + 1).min(chars)
                    };
                    state.caret = to;
                    if !k.shift {
                        state.anchor = to;
                    }
                }
                Key::Home => {
                    state.caret = 0;
                    if !k.shift {
                        state.anchor = 0;
                    }
                }
                Key::End => {
                    state.caret = chars;
                    if !k.shift {
                        state.anchor = chars;
                    }
                }
                Key::Backspace => {
                    if sel.0 == sel.1 && state.caret > 0 {
                        state.anchor = state.caret - 1;
                    }
                    insert(state, text, "", &mut chars);
                }
                Key::Delete => {
                    if sel.0 == sel.1 && state.caret < chars {
                        state.anchor = state.caret + 1;
                    }
                    insert(state, text, "", &mut chars);
                }
                Key::Enter => submitted = true,
                Key::Char('a') if k.ctrl => {
                    state.anchor = 0;
                    state.caret = chars;
                }
                Key::Char('c') if k.ctrl && sel.0 != sel.1 => {
                    copy = Some(text.chars().skip(sel.0).take(sel.1 - sel.0).collect());
                }
                Key::Char('x') if k.ctrl && sel.0 != sel.1 => {
                    copy = Some(text.chars().skip(sel.0).take(sel.1 - sel.0).collect());
                    insert(state, text, "", &mut chars);
                }
                _ => {}
            }
        }
        if let Some(paste) = &input.paste {
            insert(state, text, paste, &mut chars);
        }
        if !input.text.is_empty() && !input.ctrl {
            insert(state, text, &input.text, &mut chars);
        }
        if submitted {
            out.submitted = true;
        }
        out.changed = *text != before;
        if out.changed {
            state.blink_from = self.now;
        }
        if let Some(c) = copy {
            self.push(Command::CopyText(c));
        }
    }
}
