//! `Ctl`: one object a page lays itself out with, in **design units**, drawing and handling
//! input in the same pass.
//!
//! A page writes its layout once as a function of `&mut Ctl`. `Ctl::measure` runs that function
//! without a canvas (every draw call is a no-op, widgets report "untouched") to get the natural
//! height; `Ctl::draw` runs it again to paint and to react to the pointer. Controls are keyed by
//! `u32` ids chosen by the page; the persistent [`Interaction`] remembers the pressed control, the
//! keyboard focus and the active text field between frames.

use super::edit::EditState;
use crate::pages::PageCtx;
use naby_core::color::Rgba;
use naby_core::command::Command;
use naby_render::icons::DEFAULT_STROKE;
use naby_render::{Canvas, Fill, Icon, Image, InputState, Key, Rect, TextStyle, Weight};

#[cfg(test)]
thread_local! {
    /// Rectangles of the controls drawn by the last pass, for tests.
    pub static HITS: std::cell::RefCell<Vec<(u32, Rect, bool)>> = const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
pub(super) fn record(id: u32, rect: Rect, enabled: bool) {
    HITS.with(|h| h.borrow_mut().push((id, rect, enabled)));
}

/// Persistent interaction state of one page.
#[derive(Default)]
pub struct Interaction {
    pub(super) active: Option<u32>,
    pub(super) focus: Option<u32>,
    pub(super) focus_visible: bool,
    pub(super) order: Vec<u32>,
    prev_order: Vec<u32>,
    pub(super) claimed: bool,
    pub(super) edit: EditState,
}

impl Interaction {
    /// A control is pressed or dragged, or a text field has the keyboard: the panel must not
    /// swipe pages.
    pub fn captures(&self) -> bool {
        self.active.is_some() || self.edit.focused()
    }

    /// Forgets presses, focus and any edit (page hidden).
    pub fn reset(&mut self) {
        *self = Interaction::default();
    }

    /// When the text caret next flips (so an idle page with a focused field still blinks).
    pub fn next_frame(&self, now: f64) -> Option<f64> {
        self.edit.next_blink(now)
    }

    pub(super) fn begin_frame(&mut self, input: &InputState) {
        std::mem::swap(&mut self.prev_order, &mut self.order);
        self.order.clear();
        self.claimed = false;
        #[cfg(test)]
        HITS.with(|h| h.borrow_mut().clear());
        for k in &input.keys {
            if k.key == Key::Tab && !k.ctrl && !k.alt {
                self.move_focus(if k.shift { -1 } else { 1 });
                self.focus_visible = true;
            }
        }
    }

    pub(super) fn end_frame(&mut self, input: &InputState) {
        if input.released {
            self.active = None;
        }
        if input.pressed && !self.claimed {
            self.focus = None;
            self.edit.blur();
        }
    }

    fn move_focus(&mut self, dir: i32) {
        let n = self.prev_order.len();
        if n == 0 {
            return;
        }
        let at = self
            .focus
            .and_then(|f| self.prev_order.iter().position(|id| *id == f));
        let next = match (at, dir) {
            (Some(i), d) if d > 0 => (i + 1) % n,
            (Some(i), _) => (i + n - 1) % n,
            (None, d) if d > 0 => 0,
            (None, _) => n - 1,
        };
        self.focus = Some(self.prev_order[next]);
        self.edit.blur();
    }
}

/// What a button did this frame.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Btn {
    pub hovered: bool,
    pub pressed: bool,
    pub clicked: bool,
    pub focused: bool,
}

/// What a slider did this frame.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Slide {
    /// The value to show (0..=1): the dragged value while dragging.
    pub value: f32,
    /// The user moved it this frame.
    pub changed: bool,
    /// The change is final (pointer released, key pressed).
    pub committed: bool,
    pub dragging: bool,
    pub hovered: bool,
}

pub struct Ctl<'a> {
    canvas: Option<&'a mut Canvas>,
    pub ctx: &'a PageCtx<'a>,
    pub input: &'a InputState,
    out: Option<&'a mut Vec<Command>>,
    pub(super) st: &'a mut Interaction,
    /// Device pixels per design unit.
    pub s: f32,
    /// Seconds on the host clock.
    pub now: f64,
}

impl<'a> Ctl<'a> {
    /// A painting pass.
    pub fn draw(
        canvas: &'a mut Canvas,
        ctx: &'a PageCtx<'a>,
        input: &'a InputState,
        out: &'a mut Vec<Command>,
        st: &'a mut Interaction,
    ) -> Ctl<'a> {
        st.begin_frame(input);
        Ctl {
            canvas: Some(canvas),
            ctx,
            input,
            out: Some(out),
            st,
            s: ctx.scale,
            now: ctx.snaps.now,
        }
    }

    /// A layout-only pass: nothing is drawn and no widget reacts.
    pub fn measure(
        ctx: &'a PageCtx<'a>,
        input: &'a InputState,
        st: &'a mut Interaction,
    ) -> Ctl<'a> {
        Ctl {
            canvas: None,
            ctx,
            input,
            out: None,
            st,
            s: ctx.scale,
            now: ctx.snaps.now,
        }
    }

    pub fn is_dry(&self) -> bool {
        self.canvas.is_none()
    }

    pub fn px(&self, dip: f32) -> f32 {
        dip * self.s
    }

    pub fn push(&mut self, command: Command) {
        if let Some(out) = self.out.as_mut() {
            out.push(command);
        }
    }

    pub fn theme(&self) -> &naby_render::Theme {
        self.ctx.theme
    }

    /// Interface language code (`Settings.UiLanguage`).
    pub fn lang(&self) -> &str {
        &self.ctx.settings().ui_language
    }

    /// `Localization.Text`: the caption in the interface language.
    pub fn tr<'k>(&self, key: &'k str) -> &'k str {
        naby_core::i18n::t(self.lang(), key)
    }

    // ---- text -------------------------------------------------------------------------

    pub fn style(&self, dip: f32, weight: Weight, color: Rgba) -> TextStyle {
        self.ctx.style(dip, weight, color)
    }

    pub fn text_w(&self, text: &str, dip: f32, weight: Weight) -> f32 {
        self.ctx
            .fonts
            .measure_width(text, &self.style(dip, weight, Rgba::WHITE))
    }

    pub fn line_h(&self, dip: f32, weight: Weight) -> f32 {
        let st = self.style(dip, weight, Rgba::WHITE);
        let h = self.ctx.fonts.line_height(&st);
        if h > 0.0 {
            h
        } else {
            (st.size * 1.3).ceil()
        }
    }

    /// Draws one line with its top-left at (`x`, `y`); returns the drawn width.
    #[allow(clippy::too_many_arguments)]
    pub fn text(
        &mut self,
        x: f32,
        y: f32,
        text: &str,
        dip: f32,
        weight: Weight,
        color: Rgba,
        max_w: Option<f32>,
    ) -> f32 {
        let style = self.style(dip, weight, color);
        let fonts = self.ctx.fonts;
        match self.canvas.as_mut() {
            Some(c) => c.text(fonts, x, y, text, &style, max_w),
            None => 0.0,
        }
    }

    /// One line right-aligned to `right`.
    #[allow(clippy::too_many_arguments)]
    pub fn text_right(
        &mut self,
        right: f32,
        y: f32,
        text: &str,
        dip: f32,
        weight: Weight,
        color: Rgba,
        max_w: Option<f32>,
    ) -> f32 {
        if self.is_dry() {
            return 0.0;
        }
        let style = self.style(dip, weight, color);
        let w = self.ctx.fonts.measure_width(text, &style);
        let w = max_w.map_or(w, |m| w.min(m));
        self.text(right - w, y, text, dip, weight, color, max_w)
    }

    /// One line centered in `rect` (both ways).
    pub fn text_in(
        &mut self,
        rect: Rect,
        text: &str,
        dip: f32,
        weight: Weight,
        color: Rgba,
    ) -> f32 {
        if self.is_dry() {
            return 0.0;
        }
        let style = self.style(dip, weight, color);
        let shown = self.ctx.fonts.ellipsize(text, &style, rect.w);
        let (w, h) = self.ctx.fonts.measure(&shown, &style);
        let h = if h > 0.0 { h } else { style.size * 1.3 };
        self.text(
            rect.x + (rect.w - w) / 2.0,
            rect.y + (rect.h - h) / 2.0,
            &shown,
            dip,
            weight,
            color,
            None,
        )
    }

    /// Digits in fixed cells (WPF `Typography.NumeralAlignment=Tabular`); other characters take
    /// their natural width. Returns the width.
    pub fn text_tabular_w(&self, text: &str, dip: f32, weight: Weight) -> f32 {
        let cell = self.text_w("0", dip, weight);
        text.chars()
            .map(|c| {
                if c.is_ascii_digit() {
                    cell
                } else {
                    let mut buf = [0u8; 4];
                    self.text_w(c.encode_utf8(&mut buf), dip, weight)
                }
            })
            .sum()
    }

    /// Draws `text` with tabular digits starting at `x`.
    pub fn text_tabular(
        &mut self,
        x: f32,
        y: f32,
        text: &str,
        dip: f32,
        weight: Weight,
        color: Rgba,
    ) -> f32 {
        let cell = self.text_w("0", dip, weight);
        let mut cx = x;
        for c in text.chars() {
            let mut buf = [0u8; 4];
            let s = c.encode_utf8(&mut buf);
            if c.is_ascii_digit() {
                let w = self.text_w(s, dip, weight);
                self.text(cx + (cell - w) / 2.0, y, s, dip, weight, color, None);
                cx += cell;
            } else {
                let w = self.text_w(s, dip, weight);
                self.text(cx, y, s, dip, weight, color, None);
                cx += w;
            }
        }
        cx - x
    }

    /// Word-wrapped text in `width` px; returns the height used. `line_gap` is extra space
    /// between lines in px.
    #[allow(clippy::too_many_arguments)]
    pub fn paragraph(
        &mut self,
        x: f32,
        y: f32,
        width: f32,
        text: &str,
        dip: f32,
        weight: Weight,
        color: Rgba,
    ) -> f32 {
        let style = self.style(dip, weight, color);
        let lh = self.line_h(dip, weight);
        let fonts = self.ctx.fonts;
        let lines = fonts.wrap(text, &style, width.max(1.0));
        if let Some(c) = self.canvas.as_mut() {
            for (i, line) in lines.iter().enumerate() {
                c.text(fonts, x, y + i as f32 * lh, line, &style, None);
            }
        }
        lines.len().max(1) as f32 * lh
    }

    // ---- shapes -----------------------------------------------------------------------

    pub fn alpha(&self) -> f32 {
        self.canvas.as_ref().map_or(1.0, |c| c.alpha())
    }

    pub fn set_alpha(&mut self, alpha: f32) {
        if let Some(c) = self.canvas.as_mut() {
            c.set_alpha(alpha);
        }
    }

    /// Runs `f` with everything multiplied by `opacity`.
    pub fn faded<R>(&mut self, opacity: f32, f: impl FnOnce(&mut Ctl<'a>) -> R) -> R {
        let old = self.alpha();
        self.set_alpha(old * opacity.clamp(0.0, 1.0));
        let r = f(self);
        self.set_alpha(old);
        r
    }

    pub fn fill(&mut self, rect: Rect, color: Rgba) {
        if let Some(c) = self.canvas.as_mut() {
            c.fill_rect(rect, color);
        }
    }

    pub fn rrect(&mut self, rect: Rect, radius_dip: f32, color: Rgba) {
        let r = radius_dip * self.s;
        if let Some(c) = self.canvas.as_mut() {
            c.fill_rrect(rect, r, color);
        }
    }

    pub fn rrect_fill(&mut self, rect: Rect, radius_dip: f32, fill: &Fill) {
        let r = radius_dip * self.s;
        if let Some(c) = self.canvas.as_mut() {
            c.fill_round_rect(rect, r, fill);
        }
    }

    /// Border inside `rect`.
    pub fn rrect_stroke(&mut self, rect: Rect, radius_dip: f32, color: Rgba, width_dip: f32) {
        let (r, w) = (radius_dip * self.s, width_dip * self.s);
        if let Some(c) = self.canvas.as_mut() {
            c.stroke_round_rect(rect, r, color, w);
        }
    }

    pub fn circle(&mut self, cx: f32, cy: f32, r: f32, color: Rgba) {
        if let Some(c) = self.canvas.as_mut() {
            c.fill_circle(cx, cy, r, &Fill::Solid(color));
        }
    }

    pub fn circle_stroke(&mut self, cx: f32, cy: f32, r: f32, color: Rgba, width: f32) {
        if let Some(c) = self.canvas.as_mut() {
            c.stroke_circle(cx, cy, r, color, width);
        }
    }

    pub fn arc(&mut self, center: (f32, f32), r: f32, sweep: f32, color: Rgba, width: f32) {
        if let Some(c) = self.canvas.as_mut() {
            c.stroke_arc(center, r, 0.0, sweep, color, width);
        }
    }

    pub fn icon(&mut self, icon: Icon, rect: Rect, color: Rgba) {
        if let Some(c) = self.canvas.as_mut() {
            c.icon(icon, rect, color, DEFAULT_STROKE);
        }
    }

    /// An icon of `dip` design units centered on (`cx`, `cy`).
    pub fn icon_at(&mut self, icon: Icon, cx: f32, cy: f32, dip: f32, color: Rgba) {
        let s = dip * self.s;
        self.icon(icon, Rect::new(cx - s / 2.0, cy - s / 2.0, s, s), color);
    }

    /// An image cropped to fill `rect` (`Stretch=UniformToFill`) with rounded corners.
    pub fn cover(&mut self, image: &Image, rect: Rect, radius_dip: f32, opacity: f32) {
        let r = radius_dip * self.s;
        let Some(c) = self.canvas.as_mut() else {
            return;
        };
        let (iw, ih) = (image.width() as f32, image.height() as f32);
        if iw <= 0.0 || ih <= 0.0 {
            return;
        }
        let k = (rect.w / iw).max(rect.h / ih);
        let (w, h) = (iw * k, ih * k);
        let dst = Rect::new(
            rect.x + (rect.w - w) / 2.0,
            rect.y + (rect.h - h) / 2.0,
            w,
            h,
        );
        c.push_clip_round_rect(rect, r);
        c.draw_image(image, dst, opacity, 0.0);
        c.pop_clip();
    }

    /// A round-capped line through `points` (device pixels).
    pub fn stroke_polyline(&mut self, points: &[(f32, f32)], color: Rgba, width: f32) {
        let Some(c) = self.canvas.as_mut() else {
            return;
        };
        if points.len() < 2 {
            return;
        }
        let mut pb = tiny_skia::PathBuilder::new();
        for (i, p) in points.iter().enumerate() {
            if i == 0 {
                pb.move_to(p.0, p.1);
            } else {
                pb.line_to(p.0, p.1);
            }
        }
        if let Some(path) = pb.finish() {
            c.stroke_path(
                &path,
                &Fill::Solid(color),
                width,
                tiny_skia::Transform::identity(),
            );
        }
    }

    /// Fills a path built in device pixels.
    pub fn fill_path(&mut self, path: &tiny_skia::Path, fill: &Fill) {
        if let Some(c) = self.canvas.as_mut() {
            c.fill_path(path, fill);
        }
    }

    /// Strokes a path built in device pixels.
    pub fn stroke_path(&mut self, path: &tiny_skia::Path, fill: &Fill, width: f32) {
        if let Some(c) = self.canvas.as_mut() {
            c.stroke_path(path, fill, width, tiny_skia::Transform::identity());
        }
    }

    /// Fills `path` through `t` (the gradient follows the transform too).
    pub fn fill_path_t(
        &mut self,
        path: &tiny_skia::Path,
        fill: &Fill,
        rule: tiny_skia::FillRule,
        t: tiny_skia::Transform,
    ) {
        if let Some(c) = self.canvas.as_mut() {
            c.fill_path_with(path, fill, rule, t);
        }
    }

    /// Strokes `path` through `t`; `width` is in path units.
    pub fn stroke_path_t(
        &mut self,
        path: &tiny_skia::Path,
        fill: &Fill,
        width: f32,
        t: tiny_skia::Transform,
    ) {
        if let Some(c) = self.canvas.as_mut() {
            c.stroke_path(path, fill, width, t);
        }
    }

    /// Paints the waveform bars of `wave` into `rect`.
    pub fn wave(&mut self, wave: &crate::overlay::wave::Wave, rect: Rect, color: Rgba) {
        let s = self.s;
        if let Some(c) = self.canvas.as_mut() {
            wave.paint(c, rect, s, color);
        }
    }

    /// Runs `f` as a layout-only pass: nothing is drawn, no control reacts, no command is sent.
    pub fn dry<R>(&mut self, f: impl FnOnce(&mut Ctl<'a>) -> R) -> R {
        let canvas = self.canvas.take();
        let out = self.out.take();
        let r = f(self);
        self.canvas = canvas;
        self.out = out;
        r
    }

    /// Clips to a device-space path until [`Ctl::unclip`].
    pub fn clip_path(&mut self, path: Option<&tiny_skia::Path>) {
        if let Some(c) = self.canvas.as_mut() {
            c.push_clip_path(path);
        }
    }

    pub fn clip(&mut self, rect: Rect) {
        if let Some(c) = self.canvas.as_mut() {
            c.push_clip_rect(rect);
        }
    }

    pub fn clip_rounded(&mut self, rect: Rect, radius_dip: f32) {
        let r = radius_dip * self.s;
        if let Some(c) = self.canvas.as_mut() {
            c.push_clip_round_rect(rect, r);
        }
    }

    pub fn unclip(&mut self) {
        if let Some(c) = self.canvas.as_mut() {
            c.pop_clip();
        }
    }

    // ---- pointer ----------------------------------------------------------------------

    pub fn pointer_in(&self, rect: Rect) -> bool {
        !self.is_dry() && self.input.pointer.is_some_and(|p| rect.contains_pt(p))
    }

    /// The `KeyboardFocusOutline` of Theme.xaml: a 2 DIP accent outline inset 2 DIP.
    pub fn focus_ring(&mut self, rect: Rect, radius_dip: f32) {
        let accent = self.ctx.theme.accent;
        let r = rect.inset(2.0 * self.s);
        self.rrect_stroke(r, radius_dip.min(8.0), accent, 2.0);
    }

    fn enter_key(&self) -> bool {
        self.input
            .keys
            .iter()
            .any(|k| matches!(k.key, Key::Enter | Key::Space) && !k.ctrl && !k.alt)
    }

    /// Registers a pressable control and reports pointer and keyboard activation.
    pub fn button(&mut self, id: u32, rect: Rect, enabled: bool) -> Btn {
        #[cfg(test)]
        if !self.is_dry() {
            record(id, rect, enabled);
        }
        if self.is_dry() || !enabled {
            return Btn::default();
        }
        self.st.order.push(id);
        let hovered = self.pointer_in(rect);
        if hovered && self.input.pressed {
            self.st.active = Some(id);
            self.st.focus = Some(id);
            self.st.focus_visible = false;
            self.st.claimed = true;
            self.st.edit.blur();
        }
        let pressed = self.st.active == Some(id) && hovered && self.input.down;
        let focused = self.st.focus == Some(id);
        let mut clicked = self.input.released && self.st.active == Some(id) && hovered;
        if focused && self.st.focus_visible && self.enter_key() {
            clicked = true;
        }
        Btn {
            hovered,
            pressed,
            clicked,
            focused,
        }
    }

    /// A round or rounded icon button (`Icons.Button`): `fill` None is transparent. The
    /// theme's hover wash and 0.92 / 0.4 opacities apply.
    #[allow(clippy::too_many_arguments)]
    pub fn icon_button(
        &mut self,
        id: u32,
        rect: Rect,
        icon: Icon,
        icon_dip: f32,
        fill: Option<Rgba>,
        color: Rgba,
        enabled: bool,
    ) -> Btn {
        let b = self.button(id, rect, enabled);
        let radius = rect.w.min(rect.h) / 2.0 / self.s;
        self.button_chrome(rect, radius, fill, enabled, &b);
        let (cx, cy) = rect.center();
        let alpha = if enabled { 1.0 } else { 0.4 };
        self.faded(alpha, |c| c.icon_at(icon, cx, cy, icon_dip, color));
        b
    }

    /// Fill, hover wash, press dimming and focus ring of a button.
    pub fn button_chrome(
        &mut self,
        rect: Rect,
        radius_dip: f32,
        fill: Option<Rgba>,
        enabled: bool,
        b: &Btn,
    ) {
        let hover = self.ctx.theme.hover_fill;
        let alpha = if !enabled {
            0.4
        } else if b.pressed {
            0.92
        } else {
            1.0
        };
        self.faded(alpha, |c| {
            if let Some(f) = fill {
                c.rrect(rect, radius_dip, f);
            }
            if b.hovered && enabled {
                c.rrect(rect, radius_dip, hover);
            }
        });
        if b.focused && self.st.focus_visible && enabled {
            self.focus_ring(rect, radius_dip);
        }
    }

    /// A text button without a fill (`Background=Transparent`) whose label is centered.
    #[allow(clippy::too_many_arguments)]
    pub fn text_button(
        &mut self,
        id: u32,
        rect: Rect,
        label: &str,
        dip: f32,
        weight: Weight,
        color: Rgba,
        fill: Option<Rgba>,
        enabled: bool,
    ) -> Btn {
        let b = self.button(id, rect, enabled);
        self.button_chrome(rect, 8.0, fill, enabled, &b);
        let alpha = if enabled { 1.0 } else { 0.4 };
        self.faded(alpha, |c| {
            c.text_in(rect, label, dip, weight, color);
        });
        b
    }

    /// The themed horizontal slider (24 DIP tall): 4 DIP track, 14 DIP thumb. `value` is 0..=1;
    /// `step` is the keyboard step.
    pub fn slider(&mut self, id: u32, rect: Rect, value: f32, step: f32, enabled: bool) -> Slide {
        let value = if value.is_finite() {
            value.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let s = self.s;
        let (tl, tw) = (rect.x + 7.0 * s, (rect.w - 14.0 * s).max(1.0));
        let mut out = Slide {
            value,
            ..Slide::default()
        };
        #[cfg(test)]
        if !self.is_dry() {
            record(id, rect, enabled);
        }
        if !self.is_dry() && enabled {
            self.st.order.push(id);
            out.hovered = self.pointer_in(rect);
            if out.hovered && self.input.pressed {
                self.st.active = Some(id);
                self.st.focus = Some(id);
                self.st.focus_visible = false;
                self.st.claimed = true;
                self.st.edit.blur();
            }
            if self.st.active == Some(id) {
                out.dragging = self.input.down;
                if let Some((px, _)) = self.input.pointer {
                    let v = ((px - tl) / tw).clamp(0.0, 1.0);
                    if (v - value).abs() > f32::EPSILON || self.input.pressed {
                        out.value = v;
                        out.changed = true;
                    }
                }
                if self.input.released {
                    out.committed = true;
                    out.dragging = false;
                }
            }
            if self.st.focus == Some(id) && self.st.focus_visible {
                let mut v = out.value;
                for k in &self.input.keys {
                    let m = if k.shift { 10.0 } else { 1.0 };
                    match k.key {
                        Key::Left | Key::Down => v -= step * m,
                        Key::Right | Key::Up => v += step * m,
                        Key::PageDown => v -= step * 10.0,
                        Key::PageUp => v += step * 10.0,
                        Key::Home => v = 0.0,
                        Key::End => v = 1.0,
                        _ => continue,
                    }
                    out.changed = true;
                    out.committed = true;
                }
                out.value = v.clamp(0.0, 1.0);
            }
        }
        if !self.is_dry() {
            let th = self.ctx.theme;
            let cy = rect.y + rect.h / 2.0;
            let cx = tl + out.value * tw;
            let focused = self.st.focus == Some(id) && self.st.focus_visible && enabled;
            let alpha = if !enabled {
                0.4
            } else if focused {
                0.8
            } else {
                1.0
            };
            let (track, accent) = (th.track_fill, th.accent);
            self.faded(alpha, |c| {
                c.rrect(Rect::new(tl, cy - 2.0 * s, tw, 4.0 * s), 2.0, track);
                c.rrect(Rect::new(tl, cy - 2.0 * s, cx - tl, 4.0 * s), 2.0, accent);
                c.circle(cx, cy, 7.0 * s, accent);
                c.circle_stroke(
                    cx,
                    cy,
                    7.0 * s - 0.25 * s,
                    Rgba::WHITE.with_alpha(0x36),
                    0.5 * s,
                );
            });
            if focused {
                self.focus_ring(rect, 8.0);
            }
        }
        out
    }
}

impl Drop for Ctl<'_> {
    fn drop(&mut self) {
        if self.canvas.is_some() {
            self.st.end_frame(self.input);
        }
    }
}
