//! Single- and multi-line text field (`TextBox` / `PasswordBox` look) with caret, selection,
//! clipboard shortcuts, word movement and shift-click / drag selection.

use super::{capped, Key, Response, Ui};
use crate::canvas::Rect;
use crate::text::{TextStyle, Weight};
use crate::theme::FONT_INPUT;
use std::ops::Range;

const BLINK: f64 = 0.53;

#[derive(Debug, Clone)]
pub struct TextEditOpts<'a> {
    pub multiline: bool,
    /// Shows bullets instead of the text and disables copy / cut.
    pub password: bool,
    /// Muted placeholder shown while empty.
    pub hint: &'a str,
    /// Visible lines of a multiline field.
    pub rows: usize,
    pub max_len: Option<usize>,
    /// Draw the border in the "invalid" color.
    pub invalid: bool,
    /// Transparent field with only a bottom rule (`MinimalWidgets.Underline`).
    pub underline: bool,
}

impl Default for TextEditOpts<'_> {
    fn default() -> Self {
        TextEditOpts {
            multiline: false,
            password: false,
            hint: "",
            rows: 4,
            max_len: None,
            invalid: false,
            underline: false,
        }
    }
}

impl<'a> TextEditOpts<'a> {
    pub fn single() -> Self {
        Self::default()
    }
    pub fn multiline(rows: usize) -> Self {
        TextEditOpts {
            multiline: true,
            rows: rows.max(1),
            ..Self::default()
        }
    }
    pub fn password() -> Self {
        TextEditOpts {
            password: true,
            ..Self::default()
        }
    }
    pub fn hint(mut self, hint: &'a str) -> Self {
        self.hint = hint;
        self
    }
    pub fn max_len(mut self, n: usize) -> Self {
        self.max_len = Some(n);
        self
    }
    pub fn invalid(mut self, v: bool) -> Self {
        self.invalid = v;
        self
    }
    pub fn underline(mut self) -> Self {
        self.underline = true;
        self
    }
}

#[derive(Clone, Default)]
pub(crate) struct EditState {
    pub caret: usize,
    pub anchor: usize,
    pub scroll_x: f32,
    pub scroll_y: f32,
    pub dragging: bool,
    pub was_focused: bool,
    pub blink0: f64,
    /// Preferred column (x offset) kept across Up / Down presses.
    pub want_x: Option<f32>,
}

fn byte_at(s: &str, ci: usize) -> usize {
    s.char_indices().nth(ci).map_or(s.len(), |(b, _)| b)
}

fn char_at(s: &str, byte: usize) -> usize {
    s[..byte.min(s.len())].chars().count()
}

fn word_left(chars: &[char], mut i: usize) -> usize {
    while i > 0 && chars[i - 1].is_whitespace() {
        i -= 1;
    }
    while i > 0 && !chars[i - 1].is_whitespace() {
        i -= 1;
    }
    i
}

fn word_right(chars: &[char], mut i: usize) -> usize {
    let n = chars.len();
    while i < n && !chars[i].is_whitespace() {
        i += 1;
    }
    while i < n && chars[i].is_whitespace() {
        i += 1;
    }
    i
}

/// Index of the visual line holding character `caret` (soft-wrap boundaries belong to the
/// next line).
fn line_of(text: &str, lines: &[Range<usize>], caret: usize) -> usize {
    let b = byte_at(text, caret);
    for (i, r) in lines.iter().enumerate() {
        if b < r.end {
            return i;
        }
        if b == r.end {
            let next_starts_here = lines.get(i + 1).is_some_and(|n| n.start == r.end);
            if !next_starts_here {
                return i;
            }
        }
    }
    lines.len().saturating_sub(1)
}

impl<'a> Ui<'a> {
    pub fn text_edit(&mut self, text: &mut String, multiline: bool) -> Response {
        let opts = if multiline {
            TextEditOpts::multiline(4)
        } else {
            TextEditOpts::single()
        };
        self.text_edit_with(text, &opts)
    }

    pub fn text_edit_with(&mut self, text: &mut String, o: &TextEditOpts) -> Response {
        let th = self.theme;
        let style = self.text_style(FONT_INPUT, Weight::Regular, th.control_text);
        let lh = self.line_h(&style);
        let multiline = o.multiline;
        let (pad_x, pad_y) = if o.underline { (5.0, 4.0) } else { (10.0, 8.0) };
        let h = if multiline {
            o.rows as f32 * lh + 2.0 * pad_y + 2.0
        } else if o.underline {
            32.0_f32.max(lh + 2.0 * pad_y + 2.0)
        } else {
            34.0_f32.max(lh + 2.0 * pad_y + 2.0)
        };
        let rect = self.alloc(160.0, h, true);
        let id = self.next_id();
        let tab_focus = self.focusable(id, rect);
        let it = self.interact(id, rect, true);
        let focused = tab_focus || self.mem.focus == Some(id);
        let content = rect.inset_xy(pad_x + 1.0, pad_y + 1.0);
        let input = self.input;
        let mut st = self.mem.edits.get(&id).cloned().unwrap_or_default();
        let mut changed = false;
        let mut submitted = false;
        let mut n = text.chars().count();
        st.caret = st.caret.min(n);
        st.anchor = st.anchor.min(n);
        if focused && !st.was_focused && !it.pressed_now && !input.pressed {
            // Keyboard focus selects everything, like a WPF TextBox.
            st.anchor = 0;
            st.caret = n;
        }

        let password = o.password && !multiline;
        let display = |t: &str| -> String {
            if password {
                "\u{2022}".repeat(t.chars().count())
            } else {
                t.to_owned()
            }
        };

        // ---- mouse ----------------------------------------------------------------------
        if it.pressed_now || (st.dragging && input.down) {
            if let Some((px, py)) = input.pointer {
                let disp = display(text);
                let idx = if multiline {
                    let lines = self.fonts.wrap_ranges(&disp, &style, content.w);
                    let li = (((py - content.y + st.scroll_y) / lh).floor().max(0.0) as usize)
                        .min(lines.len().saturating_sub(1));
                    let r = lines[li].clone();
                    let cs = char_at(&disp, r.start);
                    cs + self.fonts.hit_index(&disp[r], &style, px - content.x)
                } else {
                    self.fonts
                        .hit_index(&disp, &style, px - content.x + st.scroll_x)
                };
                st.caret = idx.min(n);
                if it.pressed_now {
                    st.dragging = true;
                    if !input.shift {
                        st.anchor = st.caret;
                    }
                }
                st.want_x = None;
                st.blink0 = input.time;
            }
        }
        if !input.down {
            st.dragging = false;
        }

        // ---- keyboard -----------------------------------------------------------------
        if focused && self.enabled {
            let lines = if multiline {
                self.fonts.wrap_ranges(text, &style, content.w)
            } else {
                Vec::new()
            };
            for k in &input.keys {
                let sel = st.anchor != st.caret;
                let (lo, hi) = (st.anchor.min(st.caret), st.anchor.max(st.caret));
                let mut moved: Option<usize> = None;
                let mut keep_want = false;
                match k.key {
                    Key::Char('a') if k.ctrl => {
                        st.anchor = 0;
                        st.caret = n;
                        continue;
                    }
                    Key::Char('c') | Key::Char('x') if k.ctrl => {
                        if sel && !password {
                            let s = byte_at(text, lo);
                            let e = byte_at(text, hi);
                            self.mem.clipboard_out = Some(text[s..e].to_owned());
                            if k.key == Key::Char('x') {
                                text.replace_range(s..e, "");
                                n = text.chars().count();
                                st.caret = lo;
                                st.anchor = lo;
                                changed = true;
                            }
                        }
                        continue;
                    }
                    Key::Char('v') if k.ctrl => {
                        if let Some(p) = &input.paste {
                            changed |= insert(text, &mut st, p, o, multiline);
                            n = text.chars().count();
                        }
                        continue;
                    }
                    Key::Left => {
                        moved = Some(if k.ctrl {
                            let chars: Vec<char> = text.chars().collect();
                            word_left(&chars, st.caret)
                        } else if sel && !k.shift {
                            lo
                        } else {
                            st.caret.saturating_sub(1)
                        });
                    }
                    Key::Right => {
                        moved = Some(if k.ctrl {
                            let chars: Vec<char> = text.chars().collect();
                            word_right(&chars, st.caret)
                        } else if sel && !k.shift {
                            hi
                        } else {
                            (st.caret + 1).min(n)
                        });
                    }
                    Key::Home => {
                        moved = Some(if multiline && !k.ctrl {
                            let li = line_of(text, &lines, st.caret);
                            char_at(text, lines[li].start)
                        } else {
                            0
                        });
                    }
                    Key::End => {
                        moved = Some(if multiline && !k.ctrl {
                            let li = line_of(text, &lines, st.caret);
                            let mut e = char_at(text, lines[li].end);
                            // Stay before the soft-wrap boundary so the caret remains on this line.
                            if lines
                                .get(li + 1)
                                .is_some_and(|nx| nx.start == lines[li].end)
                                && e > 0
                            {
                                e -= 1;
                            }
                            e.max(char_at(text, lines[li].start))
                        } else {
                            n
                        });
                    }
                    Key::Up | Key::Down if multiline => {
                        let li = line_of(text, &lines, st.caret);
                        let line_start = char_at(text, lines[li].start);
                        let x = st.want_x.unwrap_or_else(|| {
                            let seg = &text[lines[li].start..byte_at(text, st.caret)];
                            self.fonts.measure_width(seg, &style)
                        });
                        let target = if k.key == Key::Up {
                            li.checked_sub(1)
                        } else {
                            (li + 1 < lines.len()).then_some(li + 1)
                        };
                        moved = Some(match target {
                            Some(t) => {
                                let r = lines[t].clone();
                                char_at(text, r.start) + self.fonts.hit_index(&text[r], &style, x)
                            }
                            None if k.key == Key::Up => 0,
                            None => n,
                        });
                        let _ = line_start;
                        st.want_x = Some(x);
                        keep_want = true;
                    }
                    Key::Backspace | Key::Delete => {
                        if sel {
                            let (s, e) = (byte_at(text, lo), byte_at(text, hi));
                            text.replace_range(s..e, "");
                            st.caret = lo;
                        } else {
                            let chars: Vec<char> = text.chars().collect();
                            let (from, to) = if k.key == Key::Backspace {
                                (
                                    if k.ctrl {
                                        word_left(&chars, st.caret)
                                    } else {
                                        st.caret.saturating_sub(1)
                                    },
                                    st.caret,
                                )
                            } else {
                                (
                                    st.caret,
                                    if k.ctrl {
                                        word_right(&chars, st.caret)
                                    } else {
                                        (st.caret + 1).min(n)
                                    },
                                )
                            };
                            if from < to {
                                text.replace_range(byte_at(text, from)..byte_at(text, to), "");
                                st.caret = from;
                            }
                        }
                        st.anchor = st.caret;
                        n = text.chars().count();
                        changed = true;
                    }
                    Key::Enter => {
                        if multiline && !k.ctrl {
                            changed |= insert(text, &mut st, "\n", o, true);
                            n = text.chars().count();
                        } else {
                            submitted = true;
                        }
                    }
                    _ => continue,
                }
                if let Some(m) = moved {
                    st.caret = m.min(n);
                    if !k.shift {
                        st.anchor = st.caret;
                    }
                }
                if !keep_want {
                    st.want_x = None;
                }
                st.blink0 = input.time;
            }
            if !input.text.is_empty() {
                changed |= insert(text, &mut st, &input.text, o, multiline);
                n = text.chars().count();
                st.blink0 = input.time;
            }
            self.mark_text_focus();
        }
        if changed {
            st.blink0 = input.time;
        }
        st.caret = st.caret.min(n);
        st.anchor = st.anchor.min(n);

        // ---- paint ----------------------------------------------------------------------
        if self.visible(rect) {
            let disp = display(text);
            let border = if o.invalid {
                th.invalid
            } else if focused {
                th.input_focus_border
            } else if it.hovered {
                th.input_hover_border
            } else if o.underline {
                th.underline_border
            } else {
                th.control_border
            };
            if o.underline {
                self.canvas
                    .fill_rect(Rect::new(rect.x, rect.bottom() - 1.0, rect.w, 1.0), border);
            } else {
                self.canvas.fill_rrect(rect, th.radius, th.control_fill);
                self.canvas.stroke_round_rect(rect, th.radius, border, 1.0);
            }
            self.canvas.push_clip_rect(content);
            let sel_color = th
                .selection_fill
                .with_alpha((th.selection_fill.a as f32 * 0.85) as u8);
            let (lo, hi) = (st.anchor.min(st.caret), st.anchor.max(st.caret));
            let blink_on = ((input.time - st.blink0) / BLINK).floor() as i64 % 2 == 0;
            let hint_style = TextStyle {
                color: th.text_dim,
                ..style
            };
            if multiline {
                let lines = self.fonts.wrap_ranges(&disp, &style, content.w);
                let cl = line_of(&disp, &lines, st.caret);
                let view_lines = (content.h / lh).floor().max(1.0);
                let top_line = (st.scroll_y / lh).round();
                if (cl as f32) < top_line {
                    st.scroll_y = cl as f32 * lh;
                } else if cl as f32 >= top_line + view_lines {
                    st.scroll_y = (cl as f32 - view_lines + 1.0) * lh;
                }
                st.scroll_y = st
                    .scroll_y
                    .clamp(0.0, (lines.len() as f32 * lh - content.h).max(0.0));
                if disp.is_empty() && !o.hint.is_empty() {
                    self.canvas.text(
                        self.fonts,
                        content.x,
                        content.y,
                        o.hint,
                        &hint_style,
                        Some(content.w),
                    );
                }
                for (i, r) in lines.iter().enumerate() {
                    let y = content.y + i as f32 * lh - st.scroll_y;
                    if y + lh < content.y || y > content.bottom() {
                        continue;
                    }
                    let seg = &disp[r.clone()];
                    let cs = char_at(&disp, r.start);
                    let ce = cs + seg.chars().count();
                    if lo != hi && lo < ce.max(cs + 1) && hi > cs {
                        let a = lo.max(cs) - cs;
                        let b = hi.min(ce) - cs;
                        let adv = self.fonts.advances(seg, &style);
                        let x0 = adv[a.min(adv.len() - 1)];
                        let mut x1 = adv[b.min(adv.len() - 1)];
                        if hi > ce {
                            x1 += 4.0;
                        }
                        self.canvas.fill_rect(
                            Rect::new(content.x + x0, y, (x1 - x0).max(0.0), lh),
                            sel_color,
                        );
                    }
                    self.canvas
                        .text(self.fonts, content.x, y, seg, &style, None);
                    if focused && blink_on && i == cl {
                        let upto = &disp[r.start..byte_at(&disp, st.caret).max(r.start)];
                        let cx = content.x + self.fonts.measure_width(upto, &style);
                        self.canvas
                            .fill_rect(Rect::new(cx, y, 1.0, lh), th.control_text);
                    }
                }
            } else {
                let adv = self.fonts.advances(&disp, &style);
                let total = *adv.last().unwrap_or(&0.0);
                let cx = adv[st.caret.min(adv.len() - 1)];
                if cx - st.scroll_x > content.w - 2.0 {
                    st.scroll_x = cx - content.w + 2.0;
                }
                if cx < st.scroll_x {
                    st.scroll_x = cx;
                }
                st.scroll_x = st.scroll_x.clamp(0.0, (total - content.w + 2.0).max(0.0));
                let ty = content.y + (content.h - lh) / 2.0;
                let ox = content.x - st.scroll_x;
                if lo != hi {
                    let x0 = adv[lo.min(adv.len() - 1)];
                    let x1 = adv[hi.min(adv.len() - 1)];
                    self.canvas
                        .fill_rect(Rect::new(ox + x0, ty, (x1 - x0).max(0.0), lh), sel_color);
                }
                if disp.is_empty() {
                    if !o.hint.is_empty() {
                        self.canvas.text(
                            self.fonts,
                            content.x,
                            ty,
                            o.hint,
                            &hint_style,
                            Some(content.w),
                        );
                    }
                } else {
                    self.canvas.text(self.fonts, ox, ty, &disp, &style, None);
                }
                if focused && blink_on {
                    self.canvas
                        .fill_rect(Rect::new(ox + cx, ty, 1.0, lh), th.control_text);
                }
            }
            self.canvas.pop_clip();
            if focused {
                self.mem
                    .want_repaint_after(BLINK - ((input.time - st.blink0) % BLINK));
            }
        }

        let committed = submitted || (st.was_focused && !focused);
        st.was_focused = focused;
        *capped(&mut self.mem.edits, id) = st;
        let mut r = self.response(id, rect, &it, focused);
        r.changed = changed;
        r.submitted = submitted;
        r.committed = committed;
        r
    }
}

/// Replaces the selection with `ins` (filtered, length-limited). Returns true when text changed.
fn insert(
    text: &mut String,
    st: &mut EditState,
    ins: &str,
    o: &TextEditOpts,
    multiline: bool,
) -> bool {
    let mut filtered: String = ins
        .chars()
        .filter(|c| !c.is_control() || (multiline && *c == '\n'))
        .collect();
    if multiline {
        filtered = filtered.replace("\r\n", "\n");
    }
    let (lo, hi) = (st.anchor.min(st.caret), st.anchor.max(st.caret));
    let remove = hi - lo;
    if let Some(max) = o.max_len {
        let have = text.chars().count() - remove;
        let room = max.saturating_sub(have);
        filtered = filtered.chars().take(room).collect();
    }
    if filtered.is_empty() && remove == 0 {
        return false;
    }
    let (s, e) = (byte_at(text, lo), byte_at(text, hi));
    text.replace_range(s..e, &filtered);
    st.caret = lo + filtered.chars().count();
    st.anchor = st.caret;
    true
}

#[cfg(test)]
mod unit {
    use super::*;

    #[test]
    fn word_moves() {
        let c: Vec<char> = "ab  cd".chars().collect();
        assert_eq!(word_right(&c, 0), 4);
        assert_eq!(word_left(&c, 6), 4);
        assert_eq!(word_left(&c, 4), 0);
    }

    #[test]
    fn insert_respects_limit_and_selection() {
        let mut t = "hello".to_owned();
        let mut st = EditState {
            caret: 5,
            anchor: 1,
            ..Default::default()
        };
        let o = TextEditOpts::single().max_len(6);
        assert!(insert(&mut t, &mut st, "ipp\n", &o, false));
        assert_eq!(t, "hipp");
        assert_eq!(st.caret, 4);
        assert!(insert(&mut t, &mut st, "xyz", &o, false));
        assert_eq!(t, "hippxy");
        assert!(!insert(&mut t, &mut st, "q", &o, false));
    }

    #[test]
    fn multibyte_indexing() {
        assert_eq!(byte_at("aéb", 2), 3);
        assert_eq!(char_at("aéb", 3), 2);
    }
}
