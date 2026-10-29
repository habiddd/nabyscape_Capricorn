//! Labels, buttons, switches, sliders, combo boxes and friends (looks from `Theme.xaml`).

use super::{Deferred, Dir, Key, Layout, PopupOutcome, Response, Ui};
use crate::canvas::{Canvas, Fill, Rect};
use crate::icons::Icon;
use crate::text::{TextStyle, Weight};
use crate::theme::{FONT_BODY, FONT_CAPTION, FONT_SMALL, RADIUS_CARD};
use naby_core::color::Rgba;
use std::ops::RangeInclusive;
use tiny_skia::{PathBuilder, Transform};

const BUTTON_MIN_H: f32 = 34.0;
const COMBO_MAX_POPUP: f32 = 240.0;

pub(super) fn combo_item_height(ui: &Ui, style: &TextStyle) -> f32 {
    ui.line_h(style) + 16.0
}

/// Draws the small "v" used by combo boxes (`M1,1 L4,4 7,1`, 7x4, round caps).
pub(super) fn chevron_down(canvas: &mut Canvas, cx: f32, cy: f32, color: Rgba) {
    let mut pb = PathBuilder::new();
    pb.move_to(cx - 3.5, cy - 2.0);
    pb.line_to(cx, cy + 2.0);
    pb.line_to(cx + 3.5, cy - 2.0);
    if let Some(p) = pb.finish() {
        canvas.stroke_path(&p, &Fill::Solid(color), 1.2, Transform::identity());
    }
}

impl<'a> Ui<'a> {
    /// False when `r` lies outside the clip of the enclosing scroll area (cheap culling, the
    /// port of `ElementVisibility.InViewport`).
    pub fn visible(&self, r: Rect) -> bool {
        self.clips.last().is_none_or(|c| c.intersect(&r).is_some())
    }

    fn in_row(&self) -> bool {
        self.layouts.last().is_some_and(|l| l.dir == Dir::Row)
    }

    // ---- text -------------------------------------------------------------------------

    pub fn label(&mut self, text: &str) -> Response {
        let st = self.text_style(FONT_BODY, Weight::Regular, self.theme.text);
        self.label_styled(text, &st)
    }

    /// Secondary text (`UI.Muted`, 12 px).
    pub fn label_dim(&mut self, text: &str) -> Response {
        let st = self.text_style(FONT_SMALL, Weight::Regular, self.theme.text_dim);
        self.label_styled(text, &st)
    }

    /// Heading (`UI.Heading`): semibold when the "bold headings" accessibility option is on.
    pub fn heading(&mut self, text: &str) -> Response {
        self.heading_sized(text, 18.0)
    }

    pub fn heading_sized(&mut self, text: &str, size: f32) -> Response {
        let weight = if self.theme.bold_headings {
            Weight::Semibold
        } else {
            Weight::Regular
        };
        let st = self.text_style(size, weight, self.theme.text);
        self.label_styled(text, &st)
    }

    /// Small muted caption above a group (`UI.Label`, 11 px, 10 px below).
    pub fn section_label(&mut self, text: &str) -> Response {
        let st = self.text_style(FONT_CAPTION, Weight::Regular, self.theme.text_dim);
        let r = self.label_styled(text, &st);
        self.gap(10.0 - self.layouts.last().map_or(0.0, |l| l.spacing).min(10.0));
        r
    }

    /// Text in an explicit style. In a column it wraps to the width; in a row it stays on one
    /// line and is ellipsized to the space left.
    pub fn label_styled(&mut self, text: &str, style: &TextStyle) -> Response {
        let lh = self.line_h(style);
        let rect = if self.in_row() {
            let avail = self.available_width();
            let w = self.fonts.measure_width(text, style).min(avail);
            let rect = self.alloc(w, lh, false);
            if self.visible(rect) {
                self.canvas
                    .text(self.fonts, rect.x, rect.y, text, style, Some(rect.w + 0.5));
            }
            rect
        } else {
            let avail = self.available_width();
            let lines = self.fonts.wrap(text, style, avail);
            let rect = self.alloc(avail, lines.len().max(1) as f32 * lh, true);
            if self.visible(rect) {
                for (i, l) in lines.iter().enumerate() {
                    self.canvas
                        .text(self.fonts, rect.x, rect.y + i as f32 * lh, l, style, None);
                }
            }
            rect
        };
        Response {
            rect,
            hovered: self.pointer_in(rect),
            ..Response::default()
        }
    }

    // ---- buttons ----------------------------------------------------------------------

    pub fn button(&mut self, text: &str) -> Response {
        self.button_impl(Some(text), None, false, None)
    }

    /// Filled with the accent color, semibold text.
    pub fn primary_button(&mut self, text: &str) -> Response {
        self.button_impl(Some(text), None, true, None)
    }

    /// Round 42 px icon button; `name` is the accessible name and tooltip.
    pub fn icon_button(&mut self, icon: Icon, name: &str) -> Response {
        self.icon_button_sized(icon, name, 42.0, false)
    }

    pub fn icon_button_sized(
        &mut self,
        icon: Icon,
        name: &str,
        size: f32,
        primary: bool,
    ) -> Response {
        let r = self.button_impl(None, Some(icon), primary, Some(size));
        self.tooltip(&r, name);
        r
    }

    fn button_impl(
        &mut self,
        text: Option<&str>,
        icon: Option<Icon>,
        primary: bool,
        round: Option<f32>,
    ) -> Response {
        let th = self.theme;
        let weight = if primary {
            Weight::Semibold
        } else {
            Weight::Regular
        };
        let fg = if primary {
            th.accent_text
        } else {
            th.control_text
        };
        let st = self.text_style(FONT_BODY, weight, fg);
        let lh = self.line_h(&st);
        let tw = text.map_or(0.0, |t| self.fonts.measure_width(t, &st));
        let rect = match round {
            Some(s) => self.allocate(Some(s), s),
            None => self.alloc(tw + 28.0, BUTTON_MIN_H.max(lh + 18.0), false),
        };
        let id = self.next_id();
        let focused = self.focusable(id, rect);
        let it = self.interact(id, rect, true);
        let radius = round.map_or(th.radius, |s| s / 2.0);
        let mut clicked = it.clicked;
        if focused && (self.key_hit(Key::Enter) || self.key_hit(Key::Space)) {
            clicked = true;
        }
        if self.visible(rect) {
            let bg = if primary { th.accent } else { th.control_fill };
            self.canvas.fill_rrect(rect, radius, bg);
            if it.hovered {
                self.canvas.fill_rrect(rect, radius, th.hover_fill);
            }
            if it.pressed {
                self.canvas
                    .fill_rrect(rect, radius, Rgba::BLACK.with_alpha(0x14));
            }
            if th.high_contrast {
                self.canvas
                    .stroke_round_rect(rect, radius, th.control_border, 1.0);
            }
            if let Some(icon) = icon {
                let s = 20.0;
                let (cx, cy) = rect.center();
                self.canvas
                    .icon(icon, Rect::new(cx - s / 2.0, cy - s / 2.0, s, s), fg, 1.6);
            }
            if let Some(t) = text {
                let x = rect.x + ((rect.w - tw) / 2.0).max(14.0);
                let y = rect.y + (rect.h - lh) / 2.0;
                self.canvas
                    .text(self.fonts, x, y, t, &st, Some((rect.w - 28.0).max(0.0)));
            }
            if focused {
                self.focus_ring(rect, radius);
            }
        }
        let mut r = self.response(id, rect, &it, focused);
        r.clicked = clicked;
        r
    }

    // ---- switches ---------------------------------------------------------------------

    /// Label on the left, 34x20 switch on the right (`SettingsSwitch`), 40 px tall.
    pub fn toggle(&mut self, value: &mut bool, label: &str) -> Response {
        let th = self.theme;
        let st = self.text_style(FONT_BODY, Weight::Regular, th.text);
        let lw = self.fonts.measure_width(label, &st);
        let rect = self.alloc(lw + 12.0 + 46.0, 40.0, false);
        let id = self.next_id();
        let focused = self.focusable(id, rect);
        let it = self.interact(id, rect, true);
        let mut clicked = it.clicked;
        if focused && (self.key_hit(Key::Enter) || self.key_hit(Key::Space)) {
            clicked = true;
        }
        if clicked {
            *value = !*value;
        }
        if self.visible(rect) {
            let lh = self.line_h(&st);
            self.canvas.text(
                self.fonts,
                rect.x,
                rect.y + (rect.h - lh) / 2.0,
                label,
                &st,
                Some((rect.w - 58.0).max(0.0)),
            );
            let track = Rect::new(
                rect.right() - 40.0,
                rect.y + (rect.h - 20.0) / 2.0,
                34.0,
                20.0,
            );
            let on = *value;
            self.canvas.fill_rrect(
                track,
                10.0,
                if on { th.checked_fill } else { th.track_fill },
            );
            let kx = if on {
                track.right() - 17.0
            } else {
                track.x + 3.0
            };
            self.canvas.fill_circle(
                kx + 7.0,
                track.y + 10.0,
                7.0,
                &Fill::Solid(if on { th.checked_text } else { th.switch_knob }),
            );
            if it.hovered {
                self.canvas.fill_rrect(track, 10.0, th.hover_fill);
            }
            if focused {
                self.focus_ring(rect, 8.0);
            }
        }
        let mut r = self.response(id, rect, &it, focused);
        r.clicked = clicked;
        r.changed = clicked;
        r.committed = clicked;
        r
    }

    /// Round check ring with a label (`CheckBox` / `TaskToggle`).
    pub fn checkbox(&mut self, value: &mut bool, label: &str) -> Response {
        let th = self.theme;
        let st = self.text_style(FONT_BODY, Weight::Regular, th.text);
        let lw = self.fonts.measure_width(label, &st);
        let rect = self.alloc(26.0 + lw, 30.0, false);
        let id = self.next_id();
        let focused = self.focusable(id, rect);
        let it = self.interact(id, rect, true);
        let mut clicked = it.clicked;
        if focused && (self.key_hit(Key::Enter) || self.key_hit(Key::Space)) {
            clicked = true;
        }
        if clicked {
            *value = !*value;
        }
        if self.visible(rect) {
            let (cx, cy) = (rect.x + 9.0, rect.y + rect.h / 2.0);
            if *value {
                self.canvas
                    .fill_circle(cx, cy, 9.0, &Fill::Solid(th.checked_fill));
                let mut pb = PathBuilder::new();
                pb.move_to(cx - 4.0, cy + 0.5);
                pb.line_to(cx - 1.2, cy + 3.5);
                pb.line_to(cx + 4.2, cy - 3.0);
                if let Some(p) = pb.finish() {
                    self.canvas.stroke_path(
                        &p,
                        &Fill::Solid(th.checked_text),
                        1.8,
                        Transform::identity(),
                    );
                }
            } else {
                self.canvas
                    .stroke_circle(cx, cy, 8.6, th.control_border, 1.2);
                if it.hovered {
                    self.canvas
                        .fill_circle(cx, cy, 8.0, &Fill::Solid(th.hover_fill));
                }
            }
            let lh = self.line_h(&st);
            self.canvas.text(
                self.fonts,
                rect.x + 26.0,
                rect.y + (rect.h - lh) / 2.0,
                label,
                &st,
                Some((rect.w - 26.0).max(0.0)),
            );
            if focused {
                self.focus_ring(rect, 8.0);
            }
        }
        let mut r = self.response(id, rect, &it, focused);
        r.clicked = clicked;
        r.changed = clicked;
        r.committed = clicked;
        r
    }

    // ---- slider -----------------------------------------------------------------------

    /// Horizontal slider. `step <= 0` means continuous. Drag, arrow keys (Shift or PageUp /
    /// PageDown for 10 steps) and Home / End change the value.
    pub fn slider(&mut self, value: &mut f32, range: RangeInclusive<f32>, step: f32) -> Response {
        let th = self.theme;
        let (lo, hi) = (*range.start(), *range.end());
        let rect = self.alloc(160.0, 24.0, true);
        let id = self.next_id();
        let focused = self.focusable(id, rect);
        let it = self.interact(id, rect, true);
        let old = *value;
        let snap = |v: f32| -> f32 {
            let v = v.clamp(lo, hi);
            if step > 0.0 {
                (lo + ((v - lo) / step).round() * step).clamp(lo, hi)
            } else {
                v
            }
        };
        let (track_l, track_w) = (rect.x + 7.0, (rect.w - 14.0).max(1.0));
        let mut committed = false;
        if it.active && (it.pressed || it.pressed_now) {
            if let Some((px, _)) = self.input.pointer {
                let t = ((px - track_l) / track_w).clamp(0.0, 1.0);
                *value = snap(lo + t * (hi - lo));
            }
        }
        if it.active && self.input.released {
            committed = true;
        }
        if focused {
            let unit = if step > 0.0 { step } else { (hi - lo) / 100.0 };
            let mut v = *value;
            for k in &self.input.keys {
                let m = if k.shift { 10.0 } else { 1.0 };
                match k.key {
                    Key::Left | Key::Down => v -= unit * m,
                    Key::Right | Key::Up => v += unit * m,
                    Key::PageDown => v -= unit * 10.0,
                    Key::PageUp => v += unit * 10.0,
                    Key::Home => v = lo,
                    Key::End => v = hi,
                    _ => continue,
                }
                committed = true;
            }
            *value = snap(v);
        } else {
            *value = snap(*value);
        }
        let changed = (*value - old).abs() > f32::EPSILON;
        if self.visible(rect) {
            let frac = if hi > lo {
                (*value - lo) / (hi - lo)
            } else {
                0.0
            };
            let cx = track_l + frac.clamp(0.0, 1.0) * track_w;
            let cy = rect.y + rect.h / 2.0;
            // The WPF template dims a keyboard-focused slider to 80%.
            let alpha = if focused && self.mem.focus_visible {
                0.8
            } else {
                1.0
            };
            self.canvas.with_opacity(alpha, |c| {
                c.fill_rrect(
                    Rect::new(track_l, cy - 2.0, track_w, 4.0),
                    2.0,
                    th.track_fill,
                );
                c.fill_rrect(
                    Rect::new(track_l, cy - 2.0, cx - track_l, 4.0),
                    2.0,
                    th.accent,
                );
                c.fill_circle(cx, cy, 7.0, &Fill::Solid(th.accent));
                c.stroke_circle(cx, cy, 7.0, Rgba::WHITE.with_alpha(0x36), 0.5);
            });
            if focused {
                self.focus_ring(rect, 8.0);
            }
        }
        let mut r = self.response(id, rect, &it, focused);
        r.changed = changed;
        r.committed = committed;
        r
    }

    // ---- segmented control ------------------------------------------------------------

    /// Pill group of mutually exclusive options.
    pub fn segmented(&mut self, items: &[&str], selected: &mut usize) -> Response {
        let th = self.theme;
        let st = self.text_style(FONT_SMALL, Weight::Regular, th.control_text);
        let lh = self.line_h(&st);
        let n = items.len().max(1);
        let seg_w = items
            .iter()
            .map(|s| self.fonts.measure_width(s, &st))
            .fold(0.0f32, f32::max)
            + 28.0;
        let rect = self.alloc(seg_w * n as f32 + 8.0, 34.0_f32.max(lh + 14.0), false);
        let id = self.next_id();
        let focused = self.focusable(id, rect);
        let it = self.interact(id, rect, true);
        let old = *selected;
        let inner = rect.inset(3.0);
        let w = inner.w / n as f32;
        if it.pressed_now || it.clicked {
            if let Some((px, _)) = self.input.pointer {
                let i = (((px - inner.x) / w).floor().max(0.0) as usize).min(n - 1);
                if it.clicked || it.pressed_now {
                    *selected = i;
                }
            }
        }
        if focused {
            for k in &self.input.keys {
                match k.key {
                    Key::Left | Key::Up => *selected = selected.saturating_sub(1),
                    Key::Right | Key::Down => *selected = (*selected + 1).min(n - 1),
                    Key::Home => *selected = 0,
                    Key::End => *selected = n - 1,
                    _ => {}
                }
            }
        }
        *selected = (*selected).min(n - 1);
        if self.visible(rect) {
            self.canvas.fill_rrect(rect, th.radius, th.control_fill);
            self.canvas
                .stroke_round_rect(rect, th.radius, th.control_border, 1.0);
            let hover_i = if it.hovered {
                self.input
                    .pointer
                    .map(|p| (((p.0 - inner.x) / w).floor().max(0.0) as usize).min(n - 1))
            } else {
                None
            };
            for (i, label) in items.iter().enumerate() {
                let seg = Rect::new(inner.x + i as f32 * w, inner.y, w, inner.h);
                let sel = i == *selected;
                if sel {
                    self.canvas
                        .fill_rrect(seg, th.radius - 2.0, th.selection_fill);
                } else if hover_i == Some(i) {
                    self.canvas.fill_rrect(seg, th.radius - 2.0, th.hover_fill);
                }
                let color = if sel { th.selection_text } else { th.text_dim };
                let s2 = TextStyle { color, ..st };
                let tw = self.fonts.measure_width(label, &s2).min(seg.w - 8.0);
                self.canvas.text(
                    self.fonts,
                    seg.x + (seg.w - tw) / 2.0,
                    seg.y + (seg.h - lh) / 2.0,
                    label,
                    &s2,
                    Some((seg.w - 8.0).max(0.0)),
                );
            }
            if focused {
                self.focus_ring(rect, th.radius);
            }
        }
        let mut r = self.response(id, rect, &it, focused);
        r.changed = *selected != old;
        r.committed = r.changed;
        r
    }

    // ---- popups, combo box and menu ------------------------------------------------------

    /// Shared list popup under (or above) `anchor` for combo boxes and menus. Keeps the
    /// highlighted row in `mem.popup_index`; the caller stores the open state with
    /// [`Ui::commit_popup`]. Rows are picked with a press; Enter / Space pick the highlighted
    /// row, Escape closes, arrows move (when `focused`).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn popup_list(
        &mut self,
        anchor: Rect,
        items: &[&str],
        selected: Option<usize>,
        width: f32,
        right_align: bool,
        focused: bool,
        menu: bool,
    ) -> PopupOutcome {
        let n = items.len();
        let st = self.text_style(FONT_SMALL, Weight::Regular, self.theme.control_text);
        let ih = combo_item_height(self, &st);
        let pad = if menu { 6.0 } else { 4.0 };
        let canvas = self.canvas.bounds();
        let full = n as f32 * ih + 2.0 * pad;
        let below = canvas.h - anchor.bottom() - 6.0;
        let want = full.min(COMBO_MAX_POPUP);
        let (y, h) = if want > below && anchor.y > below {
            let h = want.min(anchor.y - 6.0).max(ih);
            (anchor.y - 2.0 - h, h)
        } else {
            (anchor.bottom() + 2.0, want.min(below.max(ih)))
        };
        let x = if right_align {
            anchor.right() - width
        } else {
            anchor.x
        };
        let x = x.min(canvas.w - width).max(0.0);
        let pr = Rect::new(x, y, width, h);
        let inner = pr.inset(pad);
        let max_scroll = (n as f32 * ih - inner.h).max(0.0);
        let mut scroll = self.mem.popup_scroll.clamp(0.0, max_scroll);
        let mut open = true;
        let mut picked = None;
        let pointer = self.input.pointer;
        let in_popup = pointer.filter(|p| pr.contains_pt(*p));
        if let Some(p) = in_popup {
            scroll = (scroll - self.input.wheel.1).clamp(0.0, max_scroll);
            let idx = (((p.1 - inner.y + scroll) / ih).floor().max(0.0) as usize).min(n - 1);
            if self.mem.last_pointer != Some(p) || self.input.pressed {
                self.mem.popup_index = idx;
            }
            if self.input.pressed {
                self.claimed_press = true;
                picked = Some(idx);
                open = false;
            }
        } else if self.input.pressed && !pointer.is_some_and(|p| anchor.contains_pt(p)) {
            open = false;
        }
        if focused {
            for k in &self.input.keys {
                match k.key {
                    Key::Down => self.mem.popup_index = (self.mem.popup_index + 1).min(n - 1),
                    Key::Up => self.mem.popup_index = self.mem.popup_index.saturating_sub(1),
                    Key::Home => self.mem.popup_index = 0,
                    Key::End => self.mem.popup_index = n - 1,
                    Key::Enter | Key::Space => {
                        picked = Some(self.mem.popup_index.min(n - 1));
                        open = false;
                    }
                    Key::Escape => open = false,
                    _ => {}
                }
            }
        }
        // Keep the highlighted row visible.
        let top = self.mem.popup_index as f32 * ih;
        if top < scroll {
            scroll = top;
        } else if top + ih > scroll + inner.h {
            scroll = top + ih - inner.h;
        }
        self.mem.popup_scroll = scroll.clamp(0.0, max_scroll);
        if open {
            self.deferred.push(Deferred::Popup {
                rect: pr,
                items: items.iter().map(|s| (*s).to_owned()).collect(),
                hover: Some(self.mem.popup_index),
                selected,
                scroll: self.mem.popup_scroll,
                menu,
            });
        }
        PopupOutcome {
            open,
            picked,
            rect: open.then_some(pr),
        }
    }

    /// Records whether the popup owned by `id` is open (it blocks widgets underneath).
    pub(super) fn commit_popup(&mut self, id: u64, open: bool, rect: Option<Rect>) {
        if open {
            self.mem.popup = Some(id);
            self.mem.popup_rect = rect.or(self.mem.popup_rect);
        } else if self.mem.popup == Some(id) {
            self.mem.popup = None;
            self.mem.popup_rect = None;
            self.mem.repaint = true;
        }
    }

    /// Drop-down list. Closed: Up / Down change the choice; Enter / Space opens the list;
    /// open: arrows move, Enter picks, Escape closes.
    pub fn combo(&mut self, selected: &mut usize, items: &[&str]) -> Response {
        let th = self.theme;
        let st = self.text_style(FONT_SMALL, Weight::Regular, th.control_text);
        let lh = self.line_h(&st);
        let rect = self.alloc(180.0, BUTTON_MIN_H.max(lh + 16.0), true);
        let id = self.next_id();
        let focused = self.focusable(id, rect);
        let it = self.interact(id, rect, true);
        let old = *selected;
        let n = items.len();
        let mut open = self.mem.popup == Some(id);
        let mut popup_rect = None;
        if n > 0 {
            *selected = (*selected).min(n - 1);
            let was_open = open;
            if it.pressed_now {
                open = !open;
                if open {
                    self.mem.popup_index = *selected;
                    self.mem.popup_scroll = 0.0;
                }
            }
            if open && (was_open || it.pressed_now) {
                let o =
                    self.popup_list(rect, items, Some(*selected), rect.w, false, focused, false);
                open = o.open;
                popup_rect = o.rect;
                if let Some(p) = o.picked {
                    *selected = p;
                }
            } else if focused && !open {
                for k in &self.input.keys {
                    match k.key {
                        Key::Enter | Key::Space => {
                            open = true;
                            self.mem.popup_index = *selected;
                            self.mem.popup_scroll = 0.0;
                            self.mem.repaint = true;
                        }
                        Key::Down if k.alt => {
                            open = true;
                            self.mem.popup_index = *selected;
                            self.mem.popup_scroll = 0.0;
                            self.mem.repaint = true;
                        }
                        Key::Down => *selected = (*selected + 1).min(n - 1),
                        Key::Up => *selected = selected.saturating_sub(1),
                        Key::Home => *selected = 0,
                        Key::End => *selected = n - 1,
                        _ => {}
                    }
                }
            }
        } else {
            open = false;
        }
        self.commit_popup(id, open, popup_rect);
        if self.visible(rect) {
            let border = if focused && self.mem.focus_visible {
                th.input_focus_border
            } else if it.hovered {
                th.input_hover_border
            } else {
                th.control_border
            };
            self.canvas.fill_rrect(rect, th.radius, th.control_fill);
            self.canvas.stroke_round_rect(rect, th.radius, border, 1.0);
            let label = items.get(*selected).copied().unwrap_or("");
            self.canvas.text(
                self.fonts,
                rect.x + 11.0,
                rect.y + (rect.h - lh) / 2.0,
                label,
                &st,
                Some((rect.w - 43.0).max(0.0)),
            );
            chevron_down(
                self.canvas,
                rect.right() - 16.5,
                rect.y + rect.h / 2.0,
                th.control_text.with_alpha(0xA6),
            );
            if focused {
                self.focus_ring(rect, th.radius);
            }
        }
        let mut r = self.response(id, rect, &it, focused);
        r.changed = *selected != old;
        r.committed = r.changed;
        r
    }

    /// The "..." overflow button (`MinimalWidgets.More`): opens a menu of `labels`; returns
    /// the index picked this frame.
    pub fn more_menu(&mut self, name: &str, labels: &[&str]) -> Option<usize> {
        let th = self.theme;
        let rect = self.allocate(Some(30.0), 30.0);
        let id = self.next_id();
        let focused = self.focusable(id, rect);
        let it = self.interact(id, rect, true);
        let mut open = self.mem.popup == Some(id);
        let was_open = open;
        let mut picked = None;
        let mut popup_rect = None;
        if it.pressed_now {
            open = !open;
            if open {
                self.mem.popup_index = 0;
                self.mem.popup_scroll = 0.0;
            }
        } else if focused && !open && (self.key_hit(Key::Enter) || self.key_hit(Key::Space)) {
            open = true;
            self.mem.popup_index = 0;
            self.mem.popup_scroll = 0.0;
            self.mem.repaint = true;
        }
        if open && !labels.is_empty() && (was_open || it.pressed_now) {
            let st = self.text_style(FONT_SMALL, Weight::Regular, th.control_text);
            let widest = labels
                .iter()
                .map(|l| self.fonts.measure_width(l, &st))
                .fold(0.0f32, f32::max);
            let o = self.popup_list(
                rect,
                labels,
                None,
                (widest + 44.0).max(160.0),
                true,
                focused,
                true,
            );
            open = o.open;
            popup_rect = o.rect;
            picked = o.picked;
        }
        self.commit_popup(id, open, popup_rect);
        if self.visible(rect) {
            if it.hovered || open {
                self.canvas.fill_rrect(rect, 8.0, th.hover_fill);
            }
            let st = self.text_style(21.0, Weight::Regular, th.text);
            let dots = "\u{22EF}";
            let tw = self.fonts.measure_width(dots, &st);
            if tw > 0.0 && self.fonts.has_glyph('\u{22EF}', Weight::Regular) {
                let lh = self.line_h(&st);
                self.canvas.text(
                    self.fonts,
                    rect.x + (rect.w - tw) / 2.0,
                    rect.y + (rect.h - lh) / 2.0,
                    dots,
                    &st,
                    None,
                );
            } else {
                // Font without U+22EF: three dots.
                let (cx, cy) = rect.center();
                for dx in [-6.0, 0.0, 6.0] {
                    self.canvas
                        .fill_circle(cx + dx, cy, 1.8, &Fill::Solid(th.text));
                }
            }
            if focused {
                self.focus_ring(rect, 8.0);
            }
        }
        let r = self.response(id, rect, &it, focused);
        self.tooltip(&r, name);
        picked
    }

    // ---- small visuals ------------------------------------------------------------------

    /// Thin progress bar (`ProgressBar` template): 4 px, rounded 2.
    pub fn progress(&mut self, fraction: f32) -> Response {
        self.progress_h(fraction, 4.0)
    }

    pub fn progress_h(&mut self, fraction: f32, height: f32) -> Response {
        let th = self.theme;
        let rect = self.alloc(120.0, height, true);
        if self.visible(rect) {
            let f = if fraction.is_finite() {
                fraction.clamp(0.0, 1.0)
            } else {
                0.0
            };
            let r = (height / 2.0).min(2.0);
            self.canvas.fill_rrect(rect, r, th.progress_track);
            if f > 0.0 {
                self.canvas.fill_rrect(
                    Rect::new(rect.x, rect.y, (rect.w * f).max(height.min(rect.w)), rect.h),
                    r,
                    th.progress_fill,
                );
            }
        }
        Response {
            rect,
            hovered: self.pointer_in(rect),
            ..Response::default()
        }
    }

    /// Circular progress (`ProgressRing`): 2.3 px arcs from 12 o'clock, white on `#343840`.
    pub fn progress_ring(&mut self, fraction: f32, size: f32) -> Response {
        self.progress_ring_colored(fraction, size, Rgba::WHITE)
    }

    pub fn progress_ring_colored(&mut self, fraction: f32, size: f32, color: Rgba) -> Response {
        let rect = self.allocate(Some(size), size);
        let r = size / 2.0 - 3.0;
        if self.visible(rect) && r >= 2.0 {
            let f = if fraction.is_finite() {
                fraction.clamp(0.0, 1.0)
            } else {
                0.0
            };
            let (cx, cy) = rect.center();
            self.canvas
                .stroke_circle(cx, cy, r, Rgba::rgb(0x34, 0x38, 0x40), 2.3);
            if f > 0.9999 {
                self.canvas.stroke_circle(cx, cy, r, color, 2.3);
            } else if f >= 0.0001 {
                let top = (cx, cy - r);
                let a = f * std::f32::consts::TAU;
                let end = (cx + r * a.sin(), cy - r * a.cos());
                let mut pb = PathBuilder::new();
                pb.move_to(top.0, top.1);
                crate::icons::arc_to(&mut pb, top, r, r, 0.0, f > 0.5, true, end);
                if let Some(p) = pb.finish() {
                    self.canvas
                        .stroke_path(&p, &Fill::Solid(color), 2.3, Transform::identity());
                }
            }
        }
        Response {
            rect,
            ..Response::default()
        }
    }

    /// Round color chip (26 px) that acts as a button.
    pub fn color_swatch(&mut self, color: Rgba) -> Response {
        self.color_swatch_sized(color, 26.0)
    }

    pub fn color_swatch_sized(&mut self, color: Rgba, size: f32) -> Response {
        let th = self.theme;
        let rect = self.allocate(Some(size), size);
        let id = self.next_id();
        let focused = self.focusable(id, rect);
        let it = self.interact(id, rect, true);
        let mut clicked = it.clicked;
        if focused && (self.key_hit(Key::Enter) || self.key_hit(Key::Space)) {
            clicked = true;
        }
        if self.visible(rect) {
            let (cx, cy) = rect.center();
            self.canvas
                .fill_circle(cx, cy, size / 2.0, &Fill::Solid(color));
            self.canvas
                .stroke_circle(cx, cy, size / 2.0 - 0.5, th.control_border, 1.0);
            if it.hovered {
                self.canvas
                    .stroke_circle(cx, cy, size / 2.0 - 1.5, th.text, 1.5);
            }
            if focused {
                self.focus_ring(rect.inset(-2.0), size / 2.0 + 2.0);
            }
        }
        let mut r = self.response(id, rect, &it, focused);
        r.clicked = clicked;
        let hex = color.to_hex();
        self.tooltip(&r, &hex);
        r
    }

    /// 1 px divider with 8 px side and 5 px vertical margins.
    pub fn separator(&mut self) {
        let th = self.theme;
        let rect = self.alloc(0.0, 11.0, true);
        if self.visible(rect) {
            self.canvas.fill_rect(
                Rect::new(rect.x + 8.0, rect.y + 5.0, (rect.w - 16.0).max(0.0), 1.0),
                th.separator,
            );
        }
    }

    /// Rounded panel (`UI.Card`): title caption, 16 px padding, 12 px margin below. Its height
    /// comes from the previous frame, so the first frame requests a repaint.
    pub fn card<R>(&mut self, title: &str, f: impl FnOnce(&mut Ui<'a>) -> R) -> R {
        let th = self.theme;
        let id = self.next_id();
        let (x, y, w) = self.layouts.last().expect("layout").origin();
        let prev_h = self.mem.sizes.get(&id).map_or(0.0, |s| s.1);
        let outer = Rect::new(x, y, w, prev_h);
        if prev_h > 0.0 && self.visible(outer) {
            self.canvas.fill_rrect(outer, RADIUS_CARD, th.control_fill);
            self.canvas
                .stroke_round_rect(outer, RADIUS_CARD, th.control_border, 1.0);
        }
        let mut l = Layout::new(
            Dir::Column,
            Rect::new(x + 16.0, y + 16.0, (w - 32.0).max(0.0), 1.0e6),
        );
        l.spacing = self.layouts.last().expect("layout").spacing;
        self.layouts.push(l);
        if !title.is_empty() {
            self.section_label(title);
        }
        let r = f(self);
        let done = self.layouts.pop().expect("card layout");
        let h = done.used_h + 32.0;
        let slot = super::capped(&mut self.mem.sizes, id);
        if (slot.1 - h).abs() > 0.5 {
            self.mem.repaint = true;
        }
        *slot = (w, h);
        self.layouts.last_mut().expect("layout").commit(w, h + 12.0);
        r
    }
}
