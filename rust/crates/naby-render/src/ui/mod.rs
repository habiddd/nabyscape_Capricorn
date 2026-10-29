//! Immediate-mode widget toolkit.
//!
//! A frame is `Ui::new(..)`, widget calls, then drop (or [`Ui::finish`]). Nothing is retained
//! except [`UiMemory`]: focus, the pressed widget, scroll offsets, text-edit carets and the
//! sizes needed for cards and scroll areas. Widget ids are derived from call order, so wrap
//! dynamic lists in [`Ui::push_id`].
//!
//! Hosts repaint on demand: after a frame, check [`UiMemory::take_repaint`] (run the frame
//! again without input events, or use [`run_frame`]) and [`UiMemory::repaint_after`] (arm a
//! one-shot timer for caret blink, scrollbar fade or a tooltip).

mod color_picker;
mod custom;
mod extras;
mod input;
mod scroll;
mod text_edit;
mod widgets;

pub use color_picker::{format_color, hsv_to_rgb, rgb_to_hsv};
pub use extras::tile_columns;
pub use input::{InputState, Key, KeyEvent};
pub use text_edit::TextEditOpts;

use crate::canvas::{Canvas, Rect};
use crate::text::{Fonts, TextStyle, Weight};
use crate::theme::Theme;
use naby_core::color::Rgba;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

/// Result of a widget call for this frame.
#[derive(Debug, Clone, Copy, Default)]
pub struct Response {
    pub id: u64,
    pub rect: Rect,
    pub hovered: bool,
    /// Pointer is held down on the widget.
    pub pressed: bool,
    /// Activated (pointer release over it, or Enter/Space while focused).
    pub clicked: bool,
    /// The bound value changed this frame.
    pub changed: bool,
    pub focused: bool,
    /// Enter pressed in a single-line text field.
    pub submitted: bool,
    /// A change is final (slider released, color picked, field left).
    pub committed: bool,
}

const MAP_CAP: usize = 512;

pub(crate) fn capped<V: Default>(map: &mut HashMap<u64, V>, id: u64) -> &mut V {
    if !map.contains_key(&id) && map.len() >= MAP_CAP {
        map.clear();
    }
    map.entry(id).or_default()
}

/// State that must survive between frames.
#[derive(Default)]
pub struct UiMemory {
    pub(crate) focus: Option<u64>,
    pub(crate) focus_visible: bool,
    pub(crate) active: Option<u64>,
    pub(crate) popup: Option<u64>,
    pub(crate) popup_rect: Option<Rect>,
    pub(crate) popup_index: usize,
    pub(crate) popup_scroll: f32,
    pub(crate) repaint: bool,
    pub(crate) repaint_after: Option<f64>,
    pub(crate) sizes: HashMap<u64, (f32, f32)>,
    pub(crate) scrolls: HashMap<u64, scroll::ScrollState>,
    pub(crate) edits: HashMap<u64, text_edit::EditState>,
    pub(crate) pickers: HashMap<u64, color_picker::PickerState>,
    pub(crate) hover: Option<(u64, f64)>,
    pub(crate) clipboard_out: Option<String>,
    pub(crate) text_focus: bool,
    pub(crate) last_pointer: Option<(f32, f32)>,
}

impl UiMemory {
    pub fn new() -> Self {
        Self::default()
    }

    /// Forgets everything (call when a surface hides).
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn focused(&self) -> Option<u64> {
        self.focus
    }

    pub fn clear_focus(&mut self) {
        self.focus = None;
        self.popup = None;
        self.popup_rect = None;
    }

    /// True while a text field has keyboard focus: the host should not treat typed keys as
    /// global shortcuts.
    pub fn wants_text_input(&self) -> bool {
        self.text_focus
    }

    /// True when the last frame changed state that needs one more paint (focus moved, a card
    /// grew). Reading clears the flag.
    pub fn take_repaint(&mut self) -> bool {
        std::mem::take(&mut self.repaint)
    }

    /// Seconds until something animates (caret blink, scrollbar fade, tooltip delay); `None`
    /// when the frame is static so no timer needs to run. Reading clears it.
    pub fn repaint_after(&mut self) -> Option<f64> {
        self.repaint_after.take()
    }

    /// Text the last frame copied or cut; the host puts it on the clipboard.
    pub fn take_clipboard(&mut self) -> Option<String> {
        self.clipboard_out.take()
    }

    /// Resets the scroll offset of the scroll area with this id (hash of the id passed to
    /// `scroll_area`).
    pub fn scroll_to_top(&mut self) {
        for s in self.scrolls.values_mut() {
            s.offset = 0.0;
        }
        self.repaint = true;
    }

    pub(crate) fn want_repaint_after(&mut self, secs: f64) {
        let secs = secs.max(0.016);
        self.repaint_after = Some(self.repaint_after.map_or(secs, |v| v.min(secs)));
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Dir {
    Column,
    Row,
}

pub(crate) struct Layout {
    pub dir: Dir,
    pub region: Rect,
    pub cursor: (f32, f32),
    pub spacing: f32,
    pub count: usize,
    pub used_w: f32,
    pub used_h: f32,
    pub fixed_w: Option<f32>,
    pub row_h: Option<f32>,
}

impl Layout {
    fn new(dir: Dir, region: Rect) -> Layout {
        Layout {
            dir,
            region,
            cursor: (region.x, region.y),
            spacing: 0.0,
            count: 0,
            used_w: 0.0,
            used_h: 0.0,
            fixed_w: None,
            row_h: None,
        }
    }

    /// Where the next item starts and how much width is left for it.
    fn origin(&self) -> (f32, f32, f32) {
        let gap = if self.count > 0 { self.spacing } else { 0.0 };
        match self.dir {
            Dir::Column => (
                self.region.x,
                self.cursor.1 + gap,
                self.fixed_w.unwrap_or(self.region.w),
            ),
            Dir::Row => (
                self.cursor.0 + gap,
                self.region.y,
                self.fixed_w
                    .unwrap_or((self.region.right() - self.cursor.0 - gap).max(0.0)),
            ),
        }
    }

    fn alloc(&mut self, intrinsic_w: f32, h: f32, fill_row: bool) -> Rect {
        let (x, y, avail) = self.origin();
        let rect = match self.dir {
            Dir::Column => Rect::new(x, y, avail, h),
            Dir::Row => {
                let w = self
                    .fixed_w
                    .unwrap_or(if fill_row { avail } else { intrinsic_w });
                let dy = self.row_h.map_or(0.0, |rh| ((rh - h) / 2.0).max(0.0));
                Rect::new(x, y + dy, w, h)
            }
        };
        self.advance(&rect);
        rect
    }

    fn advance(&mut self, r: &Rect) {
        match self.dir {
            Dir::Column => {
                self.cursor.1 = r.bottom();
                self.used_w = self.used_w.max(r.w);
                self.used_h = self.cursor.1 - self.region.y;
            }
            Dir::Row => {
                self.cursor.0 = r.right();
                self.used_w = self.cursor.0 - self.region.x;
                self.used_h = self.used_h.max(self.row_h.unwrap_or(0.0)).max(r.h);
            }
        }
        self.count += 1;
    }

    fn commit(&mut self, w: f32, h: f32) {
        let (x, y, _) = self.origin();
        self.advance(&Rect::new(x, y, w, h));
    }
}

struct IdScope {
    id: u64,
    counter: u32,
}

fn mix(a: u64, b: u64) -> u64 {
    let mut h = (a ^ 0x9E37_79B9_7F4A_7C15).wrapping_mul(0x0000_0100_0000_01B3) ^ b;
    h = h.rotate_left(27).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    h ^ (h >> 31)
}

#[derive(Default)]
struct FocusPass {
    tab: i8,
    first: Option<(u64, Rect)>,
    last: Option<(u64, Rect)>,
    prev: Option<(u64, Rect)>,
    grab_next: bool,
    found_focused: bool,
    wrap_to_last: bool,
}

pub(crate) enum Deferred {
    Popup {
        rect: Rect,
        items: Vec<String>,
        hover: Option<usize>,
        selected: Option<usize>,
        scroll: f32,
        menu: bool,
    },
    Tooltip {
        at: (f32, f32),
        text: String,
    },
}

pub(crate) struct ScrollCtx {
    pub viewport: Rect,
    pub reveal: Option<Rect>,
}

/// One frame of widgets. See the module docs.
pub struct Ui<'a> {
    pub(crate) canvas: &'a mut Canvas,
    prev_scale: f32,
    pub(crate) fonts: &'a Fonts,
    pub(crate) theme: &'a Theme,
    pub(crate) input: &'a InputState,
    pub(crate) mem: &'a mut UiMemory,
    pub(crate) layouts: Vec<Layout>,
    ids: Vec<IdScope>,
    pub(crate) enabled: bool,
    focus: FocusPass,
    pub(crate) deferred: Vec<Deferred>,
    pub(crate) clips: Vec<Rect>,
    pub(crate) scroll_ctx: Vec<ScrollCtx>,
    claimed_press: bool,
    saw_text_focus: bool,
}

impl<'a> Ui<'a> {
    pub fn new(
        canvas: &'a mut Canvas,
        fonts: &'a Fonts,
        theme: &'a Theme,
        input: &'a InputState,
        memory: &'a mut UiMemory,
        area: Rect,
    ) -> Ui<'a> {
        let tab = input
            .keys
            .iter()
            .find(|k| k.key == Key::Tab && !k.ctrl && !k.alt)
            .map_or(0, |k| if k.shift { -1 } else { 1 });
        if tab != 0 {
            memory.focus_visible = true;
        }
        memory.repaint_after = None;
        let prev_scale = canvas.scale();
        canvas.set_scale(theme.dpi_scale);
        let mut root = Layout::new(Dir::Column, area);
        root.spacing = 8.0;
        Ui {
            canvas,
            prev_scale,
            fonts,
            theme,
            input,
            mem: memory,
            layouts: vec![root],
            ids: vec![IdScope { id: 0, counter: 0 }],
            enabled: true,
            focus: FocusPass {
                tab,
                ..FocusPass::default()
            },
            deferred: Vec::new(),
            clips: Vec::new(),
            scroll_ctx: Vec::new(),
            claimed_press: false,
            saw_text_focus: false,
        }
    }

    /// Ends the frame (same as dropping the `Ui`).
    pub fn finish(self) {}

    // ---- accessors --------------------------------------------------------------------

    pub fn canvas(&mut self) -> &mut Canvas {
        self.canvas
    }
    pub fn fonts(&self) -> &Fonts {
        self.fonts
    }
    pub fn theme(&self) -> &Theme {
        self.theme
    }
    pub fn input(&self) -> &InputState {
        self.input
    }
    pub fn memory(&mut self) -> &mut UiMemory {
        self.mem
    }
    pub fn time(&self) -> f64 {
        self.input.time
    }

    /// Text style for body-sized text in the primary color, scaled by `theme.text_scale`.
    pub fn text_style(&self, size: f32, weight: Weight, color: Rgba) -> TextStyle {
        TextStyle {
            size: self.theme.scaled(size),
            weight,
            color,
        }
    }

    pub(crate) fn line_h(&self, style: &TextStyle) -> f32 {
        let h = self.fonts.line_height(style);
        if h > 0.0 {
            h
        } else {
            (style.size * 1.3).ceil()
        }
    }

    // ---- ids --------------------------------------------------------------------------

    pub(crate) fn next_id(&mut self) -> u64 {
        let s = self.ids.last_mut().expect("root id scope");
        s.counter += 1;
        mix(s.id, s.counter as u64)
    }

    /// Scopes ids inside `f` by `salt` (use for list rows so state follows the item).
    pub fn push_id<R>(&mut self, salt: impl Hash, f: impl FnOnce(&mut Ui<'a>) -> R) -> R {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        salt.hash(&mut h);
        let parent = self.ids.last().map_or(0, |s| s.id);
        self.ids.push(IdScope {
            id: mix(parent, h.finish()),
            counter: 0,
        });
        let r = f(self);
        self.ids.pop();
        r
    }

    pub(crate) fn hashed_id(&self, salt: impl Hash) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        salt.hash(&mut h);
        mix(self.ids.last().map_or(0, |s| s.id), h.finish())
    }

    // ---- layout -----------------------------------------------------------------------

    fn cur(&mut self) -> &mut Layout {
        self.layouts.last_mut().expect("layout")
    }

    /// Gap between consecutive items of the current container.
    pub fn spacing(&mut self, s: f32) {
        self.cur().spacing = s;
    }

    /// Blank space along the container's direction, on top of the item spacing.
    pub fn gap(&mut self, amount: f32) {
        let l = self.cur();
        match l.dir {
            Dir::Column => l.cursor.1 += amount,
            Dir::Row => l.cursor.0 += amount,
        }
    }

    pub fn available_width(&self) -> f32 {
        let l = self.layouts.last().expect("layout");
        l.origin().2
    }

    /// Height from the cursor to the bottom of the container region.
    pub fn available_height(&self) -> f32 {
        let l = self.layouts.last().expect("layout");
        (l.region.bottom() - l.origin().1).max(0.0)
    }

    pub fn cursor_pos(&self) -> (f32, f32) {
        let l = self.layouts.last().expect("layout");
        let o = l.origin();
        (o.0, o.1)
    }

    /// Reserves a `w` x `h` block (full width in a column when `w` is `None`).
    pub fn allocate(&mut self, w: Option<f32>, h: f32) -> Rect {
        let l = self.cur();
        match w {
            Some(w) => {
                let saved = l.fixed_w.replace(w);
                let r = l.alloc(w, h, false);
                l.fixed_w = saved;
                r
            }
            None => l.alloc(0.0, h, true),
        }
    }

    pub(crate) fn alloc(&mut self, intrinsic_w: f32, h: f32, fill_row: bool) -> Rect {
        self.cur().alloc(intrinsic_w, h, fill_row)
    }

    fn nested<R>(
        &mut self,
        dir: Dir,
        width: Option<f32>,
        row_h: Option<f32>,
        f: impl FnOnce(&mut Ui<'a>) -> R,
    ) -> R {
        self.nested_sized(dir, width, row_h, f).0
    }

    /// Like `nested`, also returning the width and height the group used.
    pub(crate) fn nested_sized<R>(
        &mut self,
        dir: Dir,
        width: Option<f32>,
        row_h: Option<f32>,
        f: impl FnOnce(&mut Ui<'a>) -> R,
    ) -> (R, f32, f32) {
        let (x, y, avail) = self.layouts.last().expect("layout").origin();
        let w = width.unwrap_or(avail);
        let mut l = Layout::new(dir, Rect::new(x, y, w, 1.0e6));
        l.row_h = row_h;
        l.spacing = self.layouts.last().expect("layout").spacing;
        self.layouts.push(l);
        let r = f(self);
        let done = self.layouts.pop().expect("nested layout");
        let h = done.used_h.max(row_h.unwrap_or(0.0));
        let w = if dir == Dir::Row || width.is_some() {
            done.used_w.max(width.unwrap_or(0.0))
        } else {
            done.used_w
        };
        self.cur().commit(w, h);
        (r, w, h)
    }

    /// Vertical group; items stack top to bottom and fill the width.
    pub fn column<R>(&mut self, f: impl FnOnce(&mut Ui<'a>) -> R) -> R {
        self.nested(Dir::Column, None, None, f)
    }

    /// Vertical group with a fixed width.
    pub fn column_w<R>(&mut self, w: f32, f: impl FnOnce(&mut Ui<'a>) -> R) -> R {
        self.nested(Dir::Column, Some(w), None, f)
    }

    /// Horizontal group; items take their natural width (inputs, sliders and progress bars
    /// fill what is left) and align to the top.
    pub fn row<R>(&mut self, f: impl FnOnce(&mut Ui<'a>) -> R) -> R {
        self.nested(Dir::Row, None, None, f)
    }

    /// Horizontal group of fixed height with items centered vertically.
    pub fn row_centered<R>(&mut self, height: f32, f: impl FnOnce(&mut Ui<'a>) -> R) -> R {
        self.nested(Dir::Row, None, Some(height), f)
    }

    /// Makes every item inside `f` exactly `w` wide.
    pub fn width<R>(&mut self, w: f32, f: impl FnOnce(&mut Ui<'a>) -> R) -> R {
        let saved = self.cur().fixed_w.replace(w);
        let r = f(self);
        self.cur().fixed_w = saved;
        r
    }

    /// Lays out `f` in an absolute rectangle without moving the parent's cursor.
    pub fn child<R>(&mut self, rect: Rect, f: impl FnOnce(&mut Ui<'a>) -> R) -> R {
        let mut l = Layout::new(Dir::Column, rect);
        l.spacing = self.layouts.last().expect("layout").spacing;
        self.layouts.push(l);
        let r = f(self);
        self.layouts.pop();
        r
    }

    /// Runs `f` with widgets disabled (dimmed to 40%, no interaction) when `enabled` is false.
    pub fn enabled<R>(&mut self, enabled: bool, f: impl FnOnce(&mut Ui<'a>) -> R) -> R {
        if enabled {
            return f(self);
        }
        let was = self.enabled;
        self.enabled = false;
        let alpha = self.canvas.alpha();
        self.canvas.set_alpha(alpha * 0.4);
        let r = f(self);
        self.canvas.set_alpha(alpha);
        self.enabled = was;
        r
    }

    // ---- interaction ------------------------------------------------------------------

    pub(crate) fn pointer_in(&self, rect: Rect) -> bool {
        let Some(p) = self.input.pointer else {
            return false;
        };
        rect.contains_pt(p)
            && self.clips.iter().all(|c| c.contains_pt(p))
            && !self.mem.popup_rect.is_some_and(|r| r.contains_pt(p))
    }

    pub(crate) fn key_hit(&self, key: Key) -> bool {
        self.input
            .keys
            .iter()
            .any(|k| k.key == key && !k.ctrl && !k.alt)
    }

    /// Registers a focusable widget (in call order) and tells whether it has focus. Handles
    /// Tab / Shift+Tab traversal.
    pub(crate) fn focusable(&mut self, id: u64, rect: Rect) -> bool {
        if !self.enabled {
            return false;
        }
        let fp = &mut self.focus;
        if fp.first.is_none() {
            fp.first = Some((id, rect));
        }
        if fp.grab_next {
            fp.grab_next = false;
            self.mem.focus = Some(id);
            self.mem.repaint = true;
            self.reveal(rect);
        } else if fp.tab != 0 && self.mem.focus == Some(id) && !fp.found_focused {
            fp.found_focused = true;
            if fp.tab > 0 {
                fp.grab_next = true;
            } else if let Some((prev, prect)) = fp.prev {
                self.mem.focus = Some(prev);
                self.mem.repaint = true;
                self.reveal(prect);
            } else {
                fp.wrap_to_last = true;
            }
        }
        let fp = &mut self.focus;
        fp.prev = Some((id, rect));
        fp.last = Some((id, rect));
        self.mem.focus == Some(id)
    }

    fn reveal(&mut self, rect: Rect) {
        if let Some(ctx) = self.scroll_ctx.last_mut() {
            ctx.reveal = Some(rect);
        }
    }

    /// Pointer interaction for `rect`; a press on it claims the click and (if `focusable`)
    /// focuses it.
    pub(crate) fn interact(&mut self, id: u64, rect: Rect, focusable: bool) -> Interaction {
        let hovered = self.enabled && self.pointer_in(rect);
        let mut pressed_now = false;
        if hovered && self.input.pressed {
            self.mem.active = Some(id);
            self.claimed_press = true;
            pressed_now = true;
            if focusable {
                self.mem.focus = Some(id);
                self.mem.focus_visible = false;
            }
        }
        let active = self.enabled && self.mem.active == Some(id);
        Interaction {
            hovered,
            pressed: active && (self.input.down || pressed_now),
            clicked: active && self.input.released && hovered,
            pressed_now,
            active,
        }
    }

    pub(crate) fn mark_text_focus(&mut self) {
        self.saw_text_focus = true;
    }

    /// Keyboard focus outline (`KeyboardFocusOutline` style: 2px accent, inset 2).
    pub(crate) fn focus_ring(&mut self, rect: Rect, radius: f32) {
        if self.mem.focus_visible {
            let c = self.theme.accent;
            self.canvas
                .stroke_round_rect(rect.inset(2.0), radius, c, 2.0);
        }
    }

    pub(crate) fn response(&self, id: u64, rect: Rect, i: &Interaction, focused: bool) -> Response {
        Response {
            id,
            rect,
            hovered: i.hovered,
            pressed: i.pressed,
            clicked: i.clicked,
            focused,
            ..Response::default()
        }
    }

    /// Queues a tooltip for `resp` shown after the pointer rests on it for 0.5 s.
    pub fn tooltip(&mut self, resp: &Response, text: &str) {
        if !resp.hovered || resp.pressed {
            if self.mem.hover.is_some_and(|(id, _)| id == resp.id) {
                self.mem.hover = None;
            }
            return;
        }
        let now = self.input.time;
        let since = match self.mem.hover {
            Some((id, t)) if id == resp.id => t,
            _ => {
                self.mem.hover = Some((resp.id, now));
                now
            }
        };
        let wait = 0.5 - (now - since);
        if wait > 0.0 {
            self.mem.want_repaint_after(wait);
        } else if let Some(p) = self.input.pointer {
            self.deferred.push(Deferred::Tooltip {
                at: (p.0 + 12.0, p.1 + 18.0),
                text: text.to_owned(),
            });
        }
    }

    fn paint_deferred(&mut self) {
        let items = std::mem::take(&mut self.deferred);
        if items.is_empty() {
            return;
        }
        let th = self.theme.clone();
        let bounds = self.canvas.bounds();
        for d in items {
            match d {
                Deferred::Popup {
                    rect,
                    items,
                    hover,
                    selected,
                    scroll,
                    menu,
                } => {
                    let (radius, pad, item_radius, text_pad) = if menu {
                        (10.0, 6.0, 7.0, 10.0)
                    } else {
                        (8.0, 4.0, 6.0, 10.0)
                    };
                    self.canvas.fill_rrect(rect, radius, th.control_fill);
                    self.canvas
                        .stroke_round_rect(rect, radius, th.control_border, 1.0);
                    let inner = rect.inset(pad);
                    self.canvas.push_clip_rect(inner);
                    let style = self.text_style(12.0, Weight::Regular, th.control_text);
                    let ih = widgets::combo_item_height(self, &style);
                    for (i, it) in items.iter().enumerate() {
                        let r = Rect::new(inner.x, inner.y + i as f32 * ih - scroll, inner.w, ih);
                        if r.bottom() < inner.y || r.y > inner.bottom() {
                            continue;
                        }
                        if hover == Some(i) {
                            self.canvas.fill_rrect(r, item_radius, th.selection_fill);
                        } else if selected == Some(i) {
                            self.canvas.fill_rrect(
                                r,
                                item_radius,
                                th.selection_fill.with_alpha(0x80),
                            );
                        }
                        let color = if hover == Some(i) {
                            th.selection_text
                        } else {
                            th.control_text
                        };
                        let st = TextStyle { color, ..style };
                        let ty = r.y + (r.h - self.fonts.line_height(&st)) / 2.0;
                        self.canvas.text(
                            self.fonts,
                            r.x + text_pad,
                            ty,
                            it,
                            &st,
                            Some(r.w - 2.0 * text_pad),
                        );
                    }
                    self.canvas.pop_clip();
                }
                Deferred::Tooltip { at, text } => {
                    let style = self.text_style(11.0, Weight::Regular, th.control_text);
                    let lines = self.fonts.wrap(&text, &style, 280.0);
                    let lh = self.line_h(&style);
                    let w = lines
                        .iter()
                        .map(|l| self.fonts.measure_width(l, &style))
                        .fold(0.0f32, f32::max)
                        + 20.0;
                    let h = lines.len() as f32 * lh + 14.0;
                    let x = at.0.min(bounds.w - w).max(0.0);
                    let y = if at.1 + h > bounds.h {
                        (at.1 - h - 28.0).max(0.0)
                    } else {
                        at.1
                    };
                    let r = Rect::new(x, y, w, h);
                    self.canvas.fill_rrect(r, 8.0, th.control_fill);
                    self.canvas
                        .stroke_round_rect(r, 8.0, th.control_border, 1.0);
                    for (i, l) in lines.iter().enumerate() {
                        self.canvas.text(
                            self.fonts,
                            x + 10.0,
                            y + 7.0 + i as f32 * lh,
                            l,
                            &style,
                            None,
                        );
                    }
                }
            }
        }
    }

    fn end_frame(&mut self) {
        self.paint_deferred();
        let fp = &self.focus;
        let mut new_focus: Option<(u64, Rect)> = None;
        if fp.tab != 0 {
            if fp.grab_next {
                new_focus = fp.first;
            } else if fp.wrap_to_last {
                new_focus = fp.last;
            } else if !fp.found_focused {
                new_focus = if fp.tab > 0 { fp.first } else { fp.last };
            }
        }
        if let Some((id, _)) = new_focus {
            self.mem.focus = Some(id);
            self.mem.repaint = true;
        }
        if self.input.pressed && !self.claimed_press {
            if self.mem.focus.is_some() {
                self.mem.repaint = true;
            }
            self.mem.focus = None;
            if self.mem.popup.is_some() {
                self.mem.popup = None;
                self.mem.popup_rect = None;
                self.mem.repaint = true;
            }
        }
        if self.input.released || !self.input.down {
            self.mem.active = None;
        }
        self.mem.text_focus = self.saw_text_focus;
        self.mem.last_pointer = self.input.pointer;
    }
}

impl Drop for Ui<'_> {
    fn drop(&mut self) {
        self.end_frame();
        self.canvas.set_scale(self.prev_scale);
    }
}

pub(crate) struct PopupOutcome {
    pub open: bool,
    pub picked: Option<usize>,
    pub rect: Option<Rect>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Interaction {
    pub hovered: bool,
    pub pressed: bool,
    pub clicked: bool,
    pub pressed_now: bool,
    pub active: bool,
}

/// Paints a frame with `build`, repainting (without input events) while the memory asks for
/// it, at most three passes. Clears to `clear` before every pass when given.
#[allow(clippy::too_many_arguments)]
pub fn run_frame(
    canvas: &mut Canvas,
    fonts: &Fonts,
    theme: &Theme,
    input: &InputState,
    memory: &mut UiMemory,
    area: Rect,
    clear: Option<Rgba>,
    mut build: impl FnMut(&mut Ui),
) {
    let quiet = input.without_events();
    for pass in 0..3 {
        if let Some(c) = clear {
            canvas.clear(c);
        }
        {
            let inp = if pass == 0 { input } else { &quiet };
            let mut ui = Ui::new(canvas, fonts, theme, inp, memory, area);
            build(&mut ui);
        }
        if !memory.take_repaint() {
            break;
        }
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_extra;
