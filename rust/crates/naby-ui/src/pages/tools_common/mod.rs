//! Shared plumbing of the tool pages (shortcuts, files, notes, tasks, clips, calendar, awake,
//! links): one `Ui` frame per page paint, measured heights, localized captions and the few
//! visuals the XAML styles give several pages (ghost icon buttons, split text rows).
//!
//! The `Ui` toolkit lays out in unscaled pixels and scales only its text; the helpers here
//! scale the sizes the pages choose themselves (`PageCtx::px`).

use super::PageCtx;
use naby_core::color::Rgba;
use naby_core::command::Command;
use naby_core::i18n;
use naby_render::{
    Canvas, Icon, InputState, Rect, Response, TextStyle, Theme, Ui, UiMemory, Weight,
};

/// The theme the `Ui` of a page uses: the dock's theme with its text scale multiplied by the
/// page scale, so every `Ui` text size is already in device pixels.
pub fn theme_for(ctx: &PageCtx) -> Theme {
    let mut theme = ctx.theme.clone();
    theme.text_scale = ctx.theme.text_scale * ctx.scale;
    theme
}

/// `Localization.Text`: the caption in the interface language (English when untranslated).
pub fn tr<'a>(ctx: &PageCtx, english: &'a str) -> &'a str {
    i18n::t(&ctx.snaps.settings.ui_language, english)
}

/// `Localization.Format`.
pub fn fmt(ctx: &PageCtx, template: &str, args: &[&dyn std::fmt::Display]) -> String {
    i18n::format(&ctx.snaps.settings.ui_language, template, args)
}

/// What every tool page keeps: the `Ui` memory, the measured height and the wake time of the
/// next caret blink or fade.
pub struct Shell {
    pub mem: UiMemory,
    measure_mem: UiMemory,
    scratch: Canvas,
    height: f32,
    measured_for: Option<(i64, u64)>,
    wake_at: Option<f64>,
    held: bool,
}

impl Default for Shell {
    fn default() -> Self {
        Shell {
            mem: UiMemory::new(),
            measure_mem: UiMemory::new(),
            scratch: Canvas::new(1, 1),
            height: 0.0,
            measured_for: None,
            wake_at: None,
            held: false,
        }
    }
}

impl Shell {
    pub fn new() -> Shell {
        Shell::default()
    }

    /// Natural height for `width`: the last frame's height, measured again (without events, on a
    /// scratch canvas) when the width or `key` (a signature of everything the content depends
    /// on) changed.
    pub fn measure(
        &mut self,
        ctx: &PageCtx,
        width: f32,
        key: u64,
        build: impl FnOnce(&mut Ui),
    ) -> f32 {
        let tag = (width.round() as i64, key);
        if self.measured_for != Some(tag) {
            self.measured_for = Some(tag);
            let theme = theme_for(ctx);
            let input = InputState {
                time: ctx.snaps.now,
                ..InputState::default()
            };
            let area = Rect::new(0.0, 0.0, width, 1.0e5);
            let used = {
                let mut ui = Ui::new(
                    &mut self.scratch,
                    ctx.fonts,
                    &theme,
                    &input,
                    &mut self.measure_mem,
                    area,
                );
                ui.spacing(0.0);
                build(&mut ui);
                ui.cursor_pos().1
            };
            self.height = used.ceil();
        }
        self.height.max(ctx.px(36.0))
    }

    /// Runs one frame into `rect`. Returns true when the page wants another frame right away
    /// (focus moved, a card grew, the height changed).
    pub fn run(
        &mut self,
        canvas: &mut Canvas,
        ctx: &PageCtx,
        rect: Rect,
        input: &InputState,
        out: &mut Vec<Command>,
        build: impl FnOnce(&mut Ui),
    ) -> bool {
        // The overlay replays the last input without events between real ones; the clock must
        // still move for caret blink and tooltips.
        let mut input = input.clone();
        input.time = ctx.snaps.now;
        if !input.down {
            self.held = false;
        } else if input.pressed && input.pointer.is_some_and(|p| rect.contains_pt(p)) {
            self.held = true;
        }
        let theme = theme_for(ctx);
        let used = {
            let mut ui = Ui::new(canvas, ctx.fonts, &theme, &input, &mut self.mem, rect);
            ui.spacing(0.0);
            build(&mut ui);
            ui.cursor_pos().1 - rect.y
        };
        if let Some(text) = self.mem.take_clipboard() {
            out.push(Command::CopyText(text));
        }
        let again = self.mem.take_repaint();
        self.wake_at = self.mem.repaint_after().map(|s| input.time + s);
        let used = used.ceil();
        let changed = (used - self.height).abs() > 0.5;
        if changed {
            self.height = used;
        }
        again || changed
    }

    /// A text field has focus or a pointer press that started on the page is still held.
    pub fn captures_pointer(&self) -> bool {
        self.held || self.mem.wants_text_input()
    }

    pub fn wants_text_input(&self) -> bool {
        self.mem.wants_text_input()
    }

    pub fn next_frame(&self) -> Option<f64> {
        self.wake_at
    }

    /// The panel closed: forget focus, scroll and decoded sizes.
    pub fn reset(&mut self) {
        self.mem.clear();
        self.measure_mem.clear();
        self.measured_for = None;
        self.wake_at = None;
        self.held = false;
    }
}

/// Everything most pages' measured height depends on: preferences, scale, productivity and
/// calendar revisions.
pub fn base_key(ctx: &PageCtx) -> u64 {
    key_of(&[
        ctx.snaps.settings.version(),
        (ctx.scale * 1000.0) as u64,
        ctx.snaps.productivity.borrow().revision(),
        ctx.snaps.calendar.revision(),
        ctx.snaps.icons_revision,
        ctx.snaps.catalog.version(),
    ])
}

/// Combines hashes into a measure key.
pub fn key_of(parts: &[u64]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for p in parts {
        h ^= *p;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

pub fn hash_str(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

// ---- drawing helpers --------------------------------------------------------------------

/// One line with `left` at the left edge and `right` right-aligned (a `DockPanel` heading).
/// Returns the row rectangle.
pub fn split_row(
    ui: &mut Ui,
    ctx: &PageCtx,
    left: Option<(&str, TextStyle)>,
    right: Option<(&str, TextStyle)>,
) -> Rect {
    let fonts = ctx.fonts;
    let lh_l = left.as_ref().map_or(0.0, |(_, s)| fonts.line_height(s));
    let lh_r = right.as_ref().map_or(0.0, |(_, s)| fonts.line_height(s));
    let h = lh_l.max(lh_r).max(1.0);
    let rect = ui.allocate(None, h);
    let right_w = right
        .as_ref()
        .map_or(0.0, |(t, s)| fonts.measure_width(t, s));
    if let Some((t, s)) = &left {
        let max = (rect.w - right_w - 8.0).max(0.0);
        let y = rect.y + (h - lh_l) / 2.0;
        ui.canvas().text(fonts, rect.x, y, t, s, Some(max));
    }
    if let Some((t, s)) = &right {
        let y = rect.y + (h - lh_r) / 2.0;
        ui.canvas()
            .text(fonts, rect.right() - right_w, y, t, s, Some(right_w + 1.0));
    }
    rect
}

/// A borderless icon button (`Icons.Button` with a transparent background): hover lightens it,
/// `fill` paints a solid background like the shelf's remove button.
#[allow(clippy::too_many_arguments)]
pub fn ghost_icon(
    ui: &mut Ui,
    rect: Rect,
    icon: Icon,
    name: &str,
    glyph: f32,
    color: Rgba,
    fill: Option<Rgba>,
    radius: f32,
) -> Response {
    let r = ui.hit_area(rect, true);
    let theme = ui.theme().clone();
    if ui.visible(rect) {
        if let Some(f) = fill {
            ui.canvas().fill_rrect(rect, radius, f);
        }
        if r.hovered || r.pressed {
            ui.canvas().fill_rrect(rect, radius, theme.hover_fill);
        }
        let (cx, cy) = rect.center();
        ui.canvas().icon(
            icon,
            Rect::new(cx - glyph / 2.0, cy - glyph / 2.0, glyph, glyph),
            if r.hovered { theme.text } else { color },
            naby_render::icons::DEFAULT_STROKE,
        );
        if r.focused {
            ui.focus_outline(rect, radius);
        }
    }
    ui.tooltip(&r, name);
    r
}

/// Look of [`text_button`].
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Look {
    /// Transparent until hovered.
    Ghost,
    /// `ControlFill` background (`UI.Button`).
    Filled,
    /// `Accent` background, semibold (`UI.Button(.., primary: true)`).
    Primary,
}

/// Natural width of a [`text_button`].
pub fn button_width(ctx: &PageCtx, ui: &Ui, label: &str, font: f32, pad_x: f32, bold: bool) -> f32 {
    let st = ui.text_style(
        font,
        if bold {
            Weight::Semibold
        } else {
            Weight::Regular
        },
        Rgba::WHITE,
    );
    ctx.fonts.measure_width(label, &st) + 2.0 * ctx.px(pad_x)
}

/// Draws and registers a button in an explicit rectangle (see [`text_button`]).
pub fn button_in(
    ui: &mut Ui,
    ctx: &PageCtx,
    rect: Rect,
    label: &str,
    font: f32,
    look: Look,
    enabled: bool,
) -> Response {
    let th = ui.theme().clone();
    let weight = if look == Look::Primary {
        Weight::Semibold
    } else {
        Weight::Regular
    };
    let fg = if look == Look::Primary {
        th.accent_text
    } else {
        th.control_text
    };
    let st = ui.text_style(font, weight, fg);
    let tw = ctx.fonts.measure_width(label, &st);
    let lh = ctx.fonts.line_height(&st);
    let r = ui.enabled(enabled, |ui| ui.hit_area(rect, true));
    if ui.visible(rect) {
        let alpha = ui.canvas().alpha();
        if !enabled {
            ui.canvas().set_alpha(alpha * 0.4);
        }
        let bg = match look {
            Look::Ghost => None,
            Look::Filled => Some(th.control_fill),
            Look::Primary => Some(th.accent),
        };
        if let Some(bg) = bg {
            ui.canvas().fill_rrect(rect, th.radius, bg);
        }
        if enabled && (r.hovered || r.pressed) {
            ui.canvas().fill_rrect(rect, th.radius, th.hover_fill);
        }
        ui.canvas().text(
            ctx.fonts,
            rect.x + ((rect.w - tw) / 2.0).max(0.0),
            rect.y + (rect.h - lh) / 2.0,
            label,
            &st,
            Some(rect.w),
        );
        if r.focused && enabled {
            ui.focus_outline(rect, th.radius);
        }
        ui.canvas().set_alpha(alpha);
    }
    r
}

/// A button of natural width with explicit font size, height and side padding (design units,
/// scaled here), allocated in the current row or column. Disabled buttons are dimmed and inert.
#[allow(clippy::too_many_arguments)]
pub fn text_button(
    ui: &mut Ui,
    ctx: &PageCtx,
    label: &str,
    font: f32,
    height: f32,
    pad_x: f32,
    look: Look,
    enabled: bool,
) -> Response {
    let w = button_width(ctx, ui, label, font, pad_x, look == Look::Primary);
    let st = ui.text_style(font, Weight::Regular, Rgba::WHITE);
    let h = ctx.px(height).max(ctx.fonts.line_height(&st) + ctx.px(6.0));
    let rect = ui.allocate(Some(w), h);
    button_in(ui, ctx, rect, label, font, look, enabled)
}

/// Fits an image of `iw` x `ih` into `dst` keeping its aspect ratio, centered
/// (`Stretch.Uniform`).
pub fn fit_rect(iw: u32, ih: u32, dst: Rect) -> Rect {
    if iw == 0 || ih == 0 || dst.is_empty() {
        return dst;
    }
    let k = (dst.w / iw as f32).min(dst.h / ih as f32);
    let (w, h) = (iw as f32 * k, ih as f32 * k);
    Rect::new(dst.x + (dst.w - w) / 2.0, dst.y + (dst.h - h) / 2.0, w, h)
}

/// A read-only, wrapped text box (`TextBox` with `IsReadOnly`): fixed `height`, clipped, the
/// wheel scrolls it. `scroll` is the page-kept offset in pixels.
pub fn readonly_box(
    ui: &mut Ui,
    ctx: &PageCtx,
    text: &str,
    height: f32,
    scroll: &mut f32,
    wheel: (f32, f32),
) -> Rect {
    let th = ui.theme().clone();
    let rect = ui.allocate(None, height);
    let pad = (ctx.px(10.0), ctx.px(8.0));
    let style = ui.text_style(13.0, Weight::Regular, th.control_text);
    let inner = Rect::new(
        rect.x + pad.0,
        rect.y + pad.1,
        (rect.w - 2.0 * pad.0).max(1.0),
        (rect.h - 2.0 * pad.1).max(1.0),
    );
    let lines = ctx.fonts.wrap(text, &style, inner.w);
    let lh = ctx.fonts.line_height(&style);
    let total = lines.len() as f32 * lh;
    let max = (total - inner.h).max(0.0);
    if ui.pointer_over(rect) && wheel.1 != 0.0 {
        *scroll -= wheel.1;
    }
    *scroll = scroll.clamp(0.0, max);
    if ui.visible(rect) {
        ui.canvas().fill_rrect(rect, th.radius, th.control_fill);
        ui.canvas()
            .stroke_round_rect(rect, th.radius, th.control_border, 1.0);
        ui.canvas().push_clip_rect(inner);
        for (i, line) in lines.iter().enumerate() {
            let y = inner.y + i as f32 * lh - *scroll;
            if y + lh < inner.y || y > inner.bottom() {
                continue;
            }
            ui.canvas().text(ctx.fonts, inner.x, y, line, &style, None);
        }
        ui.canvas().pop_clip();
        if max > 0.0 {
            let track = Rect::new(rect.right() - 6.0, rect.y + 4.0, 3.0, rect.h - 8.0);
            let thumb_h = (track.h * inner.h / total).max(12.0);
            let ty = track.y + (track.h - thumb_h) * (*scroll / max);
            ui.canvas().fill_rrect(
                Rect::new(track.x, ty, track.w, thumb_h),
                1.5,
                th.text_dim.with_alpha(160),
            );
        }
    }
    rect
}

/// A round-capped polyline (the check marks and arrows of the XAML templates), stamped with
/// circles so it needs no path types.
pub fn stroke_poly(canvas: &mut Canvas, points: &[(f32, f32)], width: f32, color: Rgba) {
    for pair in points.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let len = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
        let steps = ((len / (width * 0.4).max(0.25)).ceil() as usize).max(1);
        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            canvas.fill_circle(
                a.0 + (b.0 - a.0) * t,
                a.1 + (b.1 - a.1) * t,
                width / 2.0,
                &naby_render::Fill::Solid(color),
            );
        }
    }
}

/// The check mark of `TaskToggle` (`M4,9 L7,12 13,5`, 1.8 stroke) inside a 19 DIP ring whose
/// top-left corner is (`x`, `y`).
pub fn check_mark(canvas: &mut Canvas, x: f32, y: f32, scale: f32, color: Rgba) {
    let o = 1.2 * scale;
    let p = |px: f32, py: f32| (x + o + px * scale, y + o + py * scale);
    stroke_poly(
        canvas,
        &[p(4.0, 9.0), p(7.0, 12.0), p(13.0, 5.0)],
        1.8 * scale,
        color,
    );
}

/// The round check of `TaskToggle` (19 DIP ring, accent fill and tick when `done`) with its
/// top-left corner at (`x`, `y`).
pub fn check_ring(canvas: &mut Canvas, x: f32, y: f32, done: bool, th: &Theme, scale: f32) {
    let ring = 19.0 * scale;
    let (cx, cy) = (x + ring / 2.0, y + ring / 2.0);
    if done {
        canvas.fill_circle(cx, cy, ring / 2.0, &naby_render::Fill::Solid(th.accent));
        check_mark(canvas, x, y, scale, th.accent_text);
    } else {
        canvas.stroke_circle(
            cx,
            cy,
            ring / 2.0 - 0.6 * scale,
            th.control_border,
            1.2 * scale,
        );
    }
}

/// A pill-shaped hint button of the hover faces ("All tasks", "Open clipboard"): text on a
/// faint light fill. Returns its rectangle.
pub fn peek_button(canvas: &mut Canvas, ctx: &PageCtx, x: f32, y: f32, label: &str) -> Rect {
    let style = ctx.style(11.0, Weight::Regular, ctx.theme.text);
    let w = ctx.fonts.measure_width(label, &style) + ctx.px(18.0);
    let r = Rect::new(x, y, w, ctx.px(24.0));
    canvas.fill_rrect(r, ctx.px(7.0), Rgba::WHITE.with_alpha(0x14));
    let lh = ctx.fonts.line_height(&style);
    canvas.text(
        ctx.fonts,
        r.x + ctx.px(9.0),
        r.y + (r.h - lh) / 2.0,
        label,
        &style,
        Some(w),
    );
    r
}

/// Strike-through across `width` at the middle of a text line starting at (`x`, `y`).
pub fn strike(canvas: &mut Canvas, x: f32, y: f32, width: f32, line_h: f32, color: Rgba) {
    canvas.fill_rect(Rect::new(x, y + line_h * 0.55, width, 1.0), color);
}

/// Semibold-aware weight for headings (`BoldHeadings`).
pub fn heading_weight(ctx: &PageCtx) -> Weight {
    if ctx.theme.bold_headings {
        Weight::Semibold
    } else {
        Weight::Regular
    }
}

/// `#14161A` composer / panel fill and `#30343B` border used by the island's input boxes.
pub const BOX_FILL: Rgba = Rgba::rgb(0x14, 0x16, 0x1A);
pub const BOX_BORDER: Rgba = Rgba::rgb(0x30, 0x34, 0x3B);

// ---- test hooks --------------------------------------------------------------------------

#[cfg(test)]
thread_local! {
    static MARKS: std::cell::RefCell<Vec<(String, Rect)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Records where a control was drawn so tests can click it. Compiles to nothing outside tests.
#[inline]
#[allow(unused_variables)]
pub fn mark(name: &str, rect: Rect) {
    #[cfg(test)]
    MARKS.with(|m| m.borrow_mut().push((name.to_owned(), rect)));
}

/// The rectangle of the most recent `mark(name)` of the last frames (tests).
#[cfg(test)]
pub fn marked(name: &str) -> Option<Rect> {
    MARKS.with(|m| {
        m.borrow()
            .iter()
            .rev()
            .find(|(n, _)| n == name)
            .map(|(_, r)| *r)
    })
}

#[cfg(test)]
pub fn clear_marks() {
    MARKS.with(|m| m.borrow_mut().clear());
}

#[cfg(test)]
pub mod test_support;

#[cfg(test)]
mod panel_tests;
