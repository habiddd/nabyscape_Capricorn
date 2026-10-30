//! The `clips` page (C# `SmartClipboardOverlay.BuildSmartClipboard`, `VisualClipboard`): the
//! clipboard history as tiles, a details bar for the selected clip with the copy variants
//! (original, plain text, cleaned link, HEX, RGB, image), pin and remove.

use super::tools_common::{
    base_key, button_in, button_width, fit_rect, fmt, ghost_icon, hash_str, key_of, mark,
    readonly_box, split_row, tr, Look, Shell,
};
use super::{Page, PageCtx};
use chrono::{DateTime, Local, Utc};
use naby_core::color::Rgba;
use naby_core::command::Command;
use naby_core::productivity::clipboard::{smart_classify, ClipKind, SmartClip};
use naby_render::{
    Canvas, Icon, Image, InputState, Rect, TextStyle, Theme, ThumbnailCache, Ui, Weight,
};

/// `TileFlow { MinimumTileWidth = 152, Gap = 16 }` and the 142 DIP tile.
const MIN_TILE: f32 = 152.0;
const TILE_GAP: f32 = 16.0;
const TILE_H: f32 = 142.0;
const META_H: f32 = 28.0;
/// Decoded-thumbnail budget of the page.
const THUMB_BYTES: usize = 2 * 1024 * 1024;

pub fn build() -> Box<dyn Page> {
    Box::new(ClipsPage {
        shell: Shell::new(),
        list: Shell::new(),
        st: State {
            selected: None,
            status: String::new(),
            thumbs: ThumbnailCache::new(THUMB_BYTES),
            decode: true,
            pending: false,
            scrolls: [0.0; 2],
        },
    })
}

struct State {
    selected: Option<String>,
    status: String,
    thumbs: ThumbnailCache,
    /// May decode one new thumbnail this frame (never in measure passes).
    decode: bool,
    /// A thumbnail is still missing after this frame.
    pending: bool,
    scrolls: [f32; 2],
}

struct ClipsPage {
    shell: Shell,
    /// Measures the scrolling content (tiles and details) to size its viewport.
    list: Shell,
    st: State,
}

#[derive(Clone)]
pub(crate) struct ClipRow {
    id: String,
    is_image: bool,
    text: String,
    pinned: bool,
    timestamp: DateTime<Utc>,
    smart: SmartClip,
}

struct Data {
    enabled: bool,
    paused: bool,
    count: usize,
    rows: Vec<ClipRow>,
}

#[derive(Debug, PartialEq)]
pub(crate) enum Action {
    SetPaused(bool),
    ClearHistory,
    TurnOff,
    EnableHistory,
    Pin(String),
    Remove(String),
    /// Copy the clip's text or a derived variant.
    Copy(String, Option<String>),
    CopyImage(String),
    SaveImage(String),
}

fn data_of(ctx: &PageCtx) -> Data {
    let p = ctx.snaps.productivity.borrow();
    let mut rows: Vec<ClipRow> = p
        .clips()
        .iter()
        .map(|c| ClipRow {
            id: c.id.clone(),
            is_image: c.is_image(),
            text: c.text.clone(),
            pinned: c.pinned,
            timestamp: c.timestamp,
            smart: smart_classify(c),
        })
        .collect();
    // Pinned first, then newest first.
    rows.sort_by(|a, b| b.pinned.cmp(&a.pinned).then(b.timestamp.cmp(&a.timestamp)));
    Data {
        enabled: p.clipboard_enabled(),
        paused: p.clipboard_paused(),
        count: rows.len(),
        rows,
    }
}

fn kind_word(kind: ClipKind) -> &'static str {
    match kind {
        ClipKind::Text => "Text",
        ClipKind::Link => "Link",
        ClipKind::Image => "Image",
        ClipKind::Color => "Color",
    }
}

/// Timestamp text of a tile: `HH:mm` today, `MMM d · HH:mm` otherwise.
pub(crate) fn stamp(ts: DateTime<Utc>, today: chrono::NaiveDate) -> String {
    let local = ts.with_timezone(&Local);
    if local.date_naive() == today {
        local.format("%H:%M").to_string()
    } else {
        local.format("%b %-d \u{b7} %H:%M").to_string()
    }
}

/// Applies the frame's actions; returns the text for the status line (when a copy happened).
fn apply(ctx: &PageCtx, st: &mut State, actions: Vec<Action>, out: &mut Vec<Command>) {
    for action in actions {
        match action {
            Action::SetPaused(paused) => ctx
                .snaps
                .productivity
                .borrow_mut()
                .set_clipboard_paused(paused),
            Action::ClearHistory => {
                ctx.snaps.productivity.borrow_mut().clear_clips();
                st.selected = None;
            }
            Action::TurnOff => {
                let mut s = (**ctx.snaps.settings.arc()).clone();
                s.clipboard_history = false;
                st.selected = None;
                out.push(Command::ApplySettings(Box::new(s)));
            }
            Action::EnableHistory => {
                let mut s = (**ctx.snaps.settings.arc()).clone();
                s.clipboard_history = true;
                out.push(Command::ApplySettings(Box::new(s)));
            }
            Action::Pin(id) => ctx.snaps.productivity.borrow_mut().toggle_clip_pin(&id),
            Action::Remove(id) => {
                ctx.snaps.productivity.borrow_mut().remove_clip(&id);
                if st.selected.as_deref() == Some(id.as_str()) {
                    st.selected = None;
                }
            }
            Action::Copy(id, variant) => {
                let text = ctx
                    .snaps
                    .productivity
                    .borrow()
                    .text_for_copy(&id, variant.as_deref());
                match text {
                    Some(text) => {
                        out.push(Command::CopyText(text));
                        st.status = tr(ctx, "Copied.").to_owned();
                    }
                    None => {
                        st.status =
                            tr(ctx, "Could not copy. The clipboard may be busy; try again.")
                                .to_owned();
                    }
                }
            }
            Action::CopyImage(id) => {
                out.push(Command::CopyClipImage(id));
                st.status = tr(ctx, "Copied.").to_owned();
            }
            Action::SaveImage(id) => {
                out.push(Command::SaveClipImage { id });
                st.status.clear();
            }
        }
    }
}

// ---- layout ---------------------------------------------------------------------------

fn muted(ui: &Ui, size: f32) -> TextStyle {
    ui.text_style(size, Weight::Regular, ui.theme().text_dim)
}

fn thumb(ui_ctx: &PageCtx, st: &mut State, id: &str, px: u32) -> Option<std::sync::Arc<Image>> {
    let key = format!("{id}|{px}");
    if let Some(hit) = st.thumbs.get(&key) {
        return Some(hit);
    }
    if !st.decode {
        st.pending = true;
        return None;
    }
    st.decode = false; // one decode per frame keeps the panel responsive
    let bytes = ui_ctx
        .snaps
        .productivity
        .borrow_mut()
        .clip_image_bytes(id)?;
    let image = Image::decode(&bytes, px, px)?;
    let image = std::sync::Arc::new(image);
    st.thumbs.insert(&key, image.clone());
    Some(image)
}

fn kind_icon(row: &ClipRow) -> Icon {
    match row.smart.kind {
        ClipKind::Image => Icon::Image,
        ClipKind::Link => Icon::Link,
        _ => Icon::FileText,
    }
}

fn contrast(hex: &str) -> Rgba {
    Rgba::parse_hex(hex).map_or(Rgba::WHITE, Theme::contrast_text)
}

fn tile(
    ui: &mut Ui,
    ctx: &PageCtx,
    st: &mut State,
    row: &ClipRow,
    today: chrono::NaiveDate,
    actions: &mut Vec<Action>,
) {
    let th = ui.theme().clone();
    let r = ui.allocate(None, ctx.px(TILE_H));
    let selected = st.selected.as_deref() == Some(row.id.as_str());
    let copy_size = ctx.px(27.0);
    let copy_rect = Rect::new(
        r.right() - ctx.px(7.0) - copy_size,
        r.y + ctx.px(7.0),
        copy_size,
        copy_size,
    );
    mark(&format!("clip-tile-{}", row.text), r);
    mark(&format!("clip-copy-{}", row.text), copy_rect);
    let open = ui.hit_area(r, true);
    let preview = Rect::new(r.x, r.y, r.w, (r.h - ctx.px(META_H)).max(1.0));
    if ui.visible(r) {
        match row.smart.kind {
            ClipKind::Image => {
                let px = naby_render::image::thumbnail_pixels(r.w, 1.0);
                match thumb(ctx, st, &row.id, px) {
                    Some(img) => {
                        let dst = fit_rect(img.width(), img.height(), preview);
                        ui.canvas().draw_image(&img, dst, 1.0, ctx.px(8.0));
                    }
                    None => {
                        ui.canvas()
                            .fill_rrect(preview, ctx.px(8.0), th.control_fill);
                        let s = ctx.px(30.0);
                        let (cx, cy) = preview.center();
                        ui.canvas().icon(
                            Icon::Image,
                            Rect::new(cx - s / 2.0, cy - s / 2.0, s, s),
                            th.text_dim,
                            naby_render::icons::DEFAULT_STROKE,
                        );
                    }
                }
            }
            ClipKind::Color => {
                let swatch = Rect::new(preview.x, preview.y, preview.w, preview.h - ctx.px(2.0));
                let fill = Rgba::parse_hex(&row.smart.hex).unwrap_or(th.control_fill);
                ui.canvas().fill_rrect(swatch, ctx.px(8.0), fill);
                let style = ui.text_style(15.0, Weight::Regular, contrast(&row.smart.hex));
                let w = ctx.fonts.measure_width(&row.smart.hex, &style);
                let lh = ctx.fonts.line_height(&style);
                ui.canvas().text(
                    ctx.fonts,
                    swatch.x + (swatch.w - w) / 2.0,
                    swatch.y + (swatch.h - lh) / 2.0,
                    &row.smart.hex,
                    &style,
                    None,
                );
            }
            _ => {
                ui.canvas()
                    .fill_rrect(preview, ctx.px(8.0), Rgba::rgb(0x15, 0x17, 0x1B));
                ui.canvas().stroke_round_rect(
                    preview,
                    ctx.px(8.0),
                    Rgba::rgb(0x24, 0x28, 0x2F),
                    1.0,
                );
                let style = ui.text_style(12.0, Weight::Regular, th.text);
                let shown: String = if row.text.chars().count() > 170 {
                    format!("{}\u{2026}", row.text.chars().take(170).collect::<String>())
                } else {
                    row.text.clone()
                };
                let inner = preview.inset(ctx.px(12.0));
                let max_h = ctx.px(86.0).min(inner.h);
                let lh = ctx.fonts.line_height(&style);
                let lines = ctx.fonts.wrap(&shown, &style, inner.w);
                let fit = ((max_h / lh).floor() as usize).max(1);
                for (i, line) in lines.iter().take(fit).enumerate() {
                    let last = i + 1 == fit && lines.len() > fit;
                    let text = if last {
                        format!("{}\u{2026}", line.trim_end())
                    } else {
                        line.clone()
                    };
                    ui.canvas().text(
                        ctx.fonts,
                        inner.x,
                        inner.y + i as f32 * lh,
                        &text,
                        &style,
                        Some(inner.w),
                    );
                }
            }
        }
        if open.hovered {
            ui.canvas()
                .fill_rrect(r, ctx.px(8.0), Rgba::WHITE.with_alpha(0x0A));
        }
        if selected {
            ui.canvas()
                .stroke_round_rect(r, ctx.px(8.0), Rgba::rgb(0x59, 0x61, 0x6F), 1.0);
        }
        if open.focused {
            ui.focus_outline(r, ctx.px(8.0));
        }
        // Metadata row.
        let meta_y = r.bottom() - ctx.px(META_H) + ctx.px(4.0);
        let icon = ctx.px(13.0);
        ui.canvas().icon(
            kind_icon(row),
            Rect::new(r.x + ctx.px(6.0), meta_y + ctx.px(2.0), icon, icon),
            th.text_dim,
            naby_render::icons::DEFAULT_STROKE,
        );
        let when = format!(
            "{}{}",
            if row.pinned {
                format!("{} \u{b7} ", tr(ctx, "Pinned"))
            } else {
                String::new()
            },
            stamp(row.timestamp, today)
        );
        let style = muted(ui, 10.0);
        ui.canvas().text(
            ctx.fonts,
            r.x + ctx.px(6.0 + 13.0 + 8.0),
            meta_y + ctx.px(1.0),
            &when,
            &style,
            Some((r.w - ctx.px(40.0)).max(0.0)),
        );
    }
    ui.tooltip(&open, tr(ctx, "Open details \u{b7} copy, pin or remove"));
    if open.clicked {
        st.selected = if selected { None } else { Some(row.id.clone()) };
        st.status.clear();
    }
    let kind = kind_word(row.smart.kind).to_lowercase();
    let name = format!("{} {}", tr(ctx, "Copy"), kind);
    let copy = ui.reveal_on_hover(r, false, |ui| {
        ghost_icon(
            ui,
            copy_rect,
            Icon::Copy,
            &name,
            ctx.px(13.0),
            th.text_dim,
            Some(Rgba::rgb(0x1A, 0x1D, 0x23).with_alpha(0xDA)),
            ctx.px(8.0),
        )
    });
    if copy.clicked {
        actions.push(if row.is_image {
            Action::CopyImage(row.id.clone())
        } else {
            Action::Copy(row.id.clone(), None)
        });
    }
}

/// Height of a read-only preview box for `text` at `width` (`MinHeight 38`, `MaxHeight 100`).
fn preview_height(ctx: &PageCtx, ui: &Ui, text: &str, width: f32) -> f32 {
    let style = ui.text_style(13.0, Weight::Regular, Rgba::WHITE);
    let lines = ctx
        .fonts
        .wrap(text, &style, (width - ctx.px(20.0)).max(1.0))
        .len();
    (lines as f32 * ctx.fonts.line_height(&style) + ctx.px(16.0) + 2.0)
        .clamp(ctx.px(38.0), ctx.px(100.0))
}

/// The details section at the top of the scrolling list (`clip-detail`).
fn detail(ui: &mut Ui, ctx: &PageCtx, st: &mut State, row: &ClipRow) {
    let th = ui.theme().clone();
    ui.gap(ctx.px(12.0));
    let head = format!(
        "{} \u{b7} {}",
        tr(ctx, kind_word(row.smart.kind)),
        row.timestamp
            .with_timezone(&Local)
            .format("%-m/%-d/%Y %-I:%M %p")
    );
    let style = muted(ui, 10.0);
    ui.label_styled(&head, &style);
    let wheel = ui.input().wheel;
    let width = ui.available_width();
    let preview = |ui: &mut Ui, slot: usize, text: &str, st: &mut State| {
        ui.gap(ctx.px(6.0));
        let h = preview_height(ctx, ui, text, width);
        readonly_box(ui, ctx, text, h, &mut st.scrolls[slot], wheel);
        ui.gap(ctx.px(3.0));
    };
    match row.smart.kind {
        ClipKind::Image => {
            ui.gap(ctx.px(7.0));
            let area = ui.allocate(None, ctx.px(112.0));
            let px = naby_render::image::thumbnail_pixels(area.w.min(ctx.px(400.0)), 1.0);
            if let Some(img) = thumb(ctx, st, &row.id, px) {
                let fit = fit_rect(img.width(), img.height(), area);
                // Left aligned like `HorizontalAlignment.Left`.
                let dst = Rect::new(area.x, fit.y, fit.w, fit.h);
                if ui.visible(area) {
                    ui.canvas().draw_image(&img, dst, 1.0, 0.0);
                }
            }
            ui.gap(ctx.px(4.0));
        }
        kind => {
            let title = if kind == ClipKind::Link {
                "Original link"
            } else {
                "Original text"
            };
            let _ = title;
            preview(ui, 0, &row.text, st);
            if kind == ClipKind::Link {
                ui.gap(ctx.px(6.0));
                let hint = muted(ui, 10.0);
                ui.label_styled(
                    tr(ctx, "Cleaned preview \u{b7} review before copying"),
                    &hint,
                );
                preview(ui, 1, &row.smart.cleaned_link, st);
            }
            if kind == ClipKind::Color {
                ui.gap(ctx.px(5.0));
                let sw = ui.allocate(None, ctx.px(24.0));
                let fill = Rgba::parse_hex(&row.smart.hex).unwrap_or(th.control_fill);
                if ui.visible(sw) {
                    ui.canvas().fill_rrect(sw, ctx.px(4.0), fill);
                }
                ui.gap(ctx.px(5.0));
                let info = muted(ui, 11.0);
                ui.label_styled(
                    &format!("{} \u{b7} {}", row.smart.hex, row.smart.rgb),
                    &info,
                );
            }
        }
    }
}

/// Everything inside the scrolling viewport: the details of the selection, then the tiles or
/// the empty line.
fn content(
    ui: &mut Ui,
    ctx: &PageCtx,
    st: &mut State,
    data: &Data,
    today: chrono::NaiveDate,
    actions: &mut Vec<Action>,
) {
    let th = ui.theme().clone();
    if data.rows.is_empty() {
        ui.gap(ctx.px(12.0));
        let style = ui.text_style(12.0, Weight::Regular, th.text_dim);
        ui.label_styled(tr(ctx, "Your next copied item will appear here."), &style);
        return;
    }
    if let Some(row) = st
        .selected
        .as_ref()
        .and_then(|id| data.rows.iter().find(|r| &r.id == id))
    {
        let row = row.clone();
        detail(ui, ctx, st, &row);
        ui.gap(ctx.px(TILE_GAP));
    }
    let rows = &data.rows;
    let gap = ctx.px(TILE_GAP);
    ui.tile_flow(
        ctx.px(MIN_TILE),
        gap,
        rows.len(),
        |_| false,
        |ui, i| tile(ui, ctx, st, &rows[i], today, actions),
    );
}

/// Width available to the content beside the scrollbar gutter.
const GUTTER: f32 = 14.0;

/// The selected-clip actions bar (`selectedBar`). Returns its height first through `plan`.
fn bar_plan(
    ui: &Ui,
    ctx: &PageCtx,
    row: &ClipRow,
    width: f32,
    status: &str,
) -> (Vec<(String, Action, bool)>, f32) {
    // (label, action, is_pin)
    let mut buttons: Vec<(String, Action, bool)> = Vec::new();
    let id = row.id.clone();
    if row.smart.kind == ClipKind::Image {
        buttons.push((
            tr(ctx, "Copy image").to_owned(),
            Action::CopyImage(id.clone()),
            false,
        ));
        buttons.push((
            tr(ctx, "Save as PNG\u{2026}").to_owned(),
            Action::SaveImage(id.clone()),
            false,
        ));
    } else {
        let link = row.smart.kind == ClipKind::Link;
        buttons.push((
            tr(ctx, if link { "Copy original" } else { "Copy" }).to_owned(),
            Action::Copy(id.clone(), None),
            false,
        ));
        if row.smart.kind == ClipKind::Text {
            buttons.push((
                tr(ctx, "Copy plain text").to_owned(),
                Action::Copy(id.clone(), Some(row.text.clone())),
                false,
            ));
        }
        if link {
            buttons.push((
                tr(ctx, "Copy cleaned link").to_owned(),
                Action::Copy(id.clone(), Some(row.smart.cleaned_link.clone())),
                false,
            ));
        }
        if row.smart.kind == ClipKind::Color {
            buttons.push((
                tr(ctx, "Copy HEX").to_owned(),
                Action::Copy(id.clone(), Some(row.smart.hex.clone())),
                false,
            ));
            buttons.push((
                tr(ctx, "Copy RGB").to_owned(),
                Action::Copy(id.clone(), Some(row.smart.rgb.clone())),
                false,
            ));
        }
    }
    buttons.push((
        tr(ctx, if row.pinned { "Unpin" } else { "Pin" }).to_owned(),
        Action::Pin(id.clone()),
        true,
    ));
    buttons.push((tr(ctx, "Remove").to_owned(), Action::Remove(id), false));
    let inner = width - ctx.px(18.0);
    let (mut x, mut lines) = (0.0, 1);
    for (label, _, _) in &buttons {
        let w = button_width(ctx, ui, label, 10.0, 9.0, false) + ctx.px(6.0);
        if x > 0.0 && x + w > inner {
            lines += 1;
            x = 0.0;
        }
        x += w;
    }
    let status_h = if status.is_empty() {
        0.0
    } else {
        ctx.fonts.line_height(&muted(ui, 10.0))
    };
    let h = ctx.px(5.0) * 2.0 + ctx.px(24.0) + lines as f32 * ctx.px(33.0) + status_h;
    (buttons, h)
}

fn selected_bar(
    ui: &mut Ui,
    ctx: &PageCtx,
    st: &mut State,
    row: &ClipRow,
    actions: &mut Vec<Action>,
) {
    let th = ui.theme().clone();
    ui.gap(ctx.px(7.0));
    let width = ui.available_width();
    let (buttons, h) = bar_plan(ui, ctx, row, width, &st.status);
    let rect = ui.allocate(None, h);
    ui.gap(ctx.px(5.0));
    if ui.visible(rect) {
        ui.canvas()
            .fill_rrect(rect, ctx.px(10.0), Rgba::rgb(0x14, 0x17, 0x1C));
        ui.canvas()
            .stroke_round_rect(rect, ctx.px(10.0), Rgba::rgb(0x2D, 0x33, 0x3D), 1.0);
    }
    let pad = (ctx.px(9.0), ctx.px(5.0));
    let inner = Rect::new(
        rect.x + pad.0,
        rect.y + pad.1,
        rect.w - 2.0 * pad.0,
        rect.h - 2.0 * pad.1,
    );
    // Header: "Selected <kind>" and the close button.
    let close = ctx.px(24.0);
    let head = format!(
        "{} {}",
        tr(ctx, "Selected"),
        tr(ctx, kind_word(row.smart.kind)).to_lowercase()
    );
    let style = muted(ui, 10.0);
    let lh = ctx.fonts.line_height(&style);
    if ui.visible(rect) {
        ui.canvas().text(
            ctx.fonts,
            inner.x,
            inner.y + (close - lh) / 2.0,
            &head,
            &style,
            Some(inner.w - close),
        );
    }
    let close_rect = Rect::new(inner.right() - close, inner.y, close, close);
    mark("clip-close", close_rect);
    let c = ghost_icon(
        ui,
        close_rect,
        Icon::X,
        tr(ctx, "Close clipboard selection"),
        ctx.px(12.0),
        th.text_dim,
        None,
        ctx.px(8.0),
    );
    if c.clicked {
        st.selected = None;
        st.status.clear();
        return;
    }
    // Wrapped action buttons.
    let (mut x, mut y) = (inner.x, inner.y + close + ctx.px(3.0));
    for (label, action, _) in buttons {
        let w = button_width(ctx, ui, &label, 10.0, 9.0, false);
        if x > inner.x && x + w > inner.right() {
            x = inner.x;
            y += ctx.px(33.0);
        }
        let br = Rect::new(x, y, w, ctx.px(27.0));
        mark(&format!("clip-action-{label}"), br);
        let r = button_in(ui, ctx, br, &label, 10.0, Look::Filled, true);
        if r.clicked {
            actions.push(action);
        }
        x += w + ctx.px(6.0);
    }
    if !st.status.is_empty() && ui.visible(rect) {
        ui.canvas().text(
            ctx.fonts,
            inner.x,
            inner.bottom() - lh,
            &st.status,
            &style,
            Some(inner.w),
        );
    }
}

fn layout(
    ui: &mut Ui,
    ctx: &PageCtx,
    st: &mut State,
    list: &mut Shell,
    data: &Data,
    actions: &mut Vec<Action>,
) {
    let th = ui.theme().clone();
    let today = ctx.snaps.wall.date_naive();
    st.pending = false;
    if !data
        .rows
        .iter()
        .any(|r| Some(&r.id) == st.selected.as_ref())
    {
        st.selected = None;
    }
    // Header: heading and count on the left, pause switch and menu on the right.
    let heading_style = ui.text_style(
        18.0,
        if th.bold_headings {
            Weight::Semibold
        } else {
            Weight::Regular
        },
        th.text,
    );
    let count_text = fmt(ctx, "{0} items", &[&data.count]);
    let count_style = muted(ui, 10.0);
    if !data.enabled {
        split_row(
            ui,
            ctx,
            Some((tr(ctx, "Smart clipboard"), heading_style)),
            Some((count_text.as_str(), count_style)),
        );
        ui.gap(ctx.px(10.0));
        let intro = ui.text_style(12.0, Weight::Regular, th.text_dim);
        ui.label_styled(
            tr(
                ctx,
                "History is off. Enable it to save the text and images you copy next, only on this device.",
            ),
            &intro,
        );
        ui.gap(ctx.px(12.0));
        let b = ui.primary_button(tr(ctx, "Enable history"));
        mark("clip-enable", b.rect);
        if b.clicked {
            actions.push(Action::EnableHistory);
        }
        return;
    }
    let mut paused = data.paused;
    let mut menu = None;
    ui.header(
        |ui| {
            ui.row_centered(ctx.fonts.line_height(&heading_style), |ui| {
                ui.label_styled(tr(ctx, "Smart clipboard"), &heading_style);
                ui.gap(ctx.px(10.0));
                ui.label_styled(&count_text, &count_style);
            });
        },
        |ui| {
            ui.row(|ui| {
                ui.spacing(ctx.px(4.0));
                let r = ui.width(ctx.px(150.0), |ui| {
                    ui.toggle(&mut paused, tr(ctx, "Pause capture"))
                });
                mark("clip-pause", r.rect);
                menu = ui.more_menu(
                    tr(ctx, "Clipboard actions"),
                    &[tr(ctx, "Clear history"), tr(ctx, "Turn off")],
                );
            });
        },
    );
    if paused != data.paused {
        actions.push(Action::SetPaused(paused));
    }
    match menu {
        Some(0) => actions.push(Action::ClearHistory),
        Some(1) => actions.push(Action::TurnOff),
        _ => {}
    }
    // Selected-clip bar.
    let selected = st
        .selected
        .as_ref()
        .and_then(|id| data.rows.iter().find(|r| &r.id == id))
        .cloned();
    if let Some(row) = &selected {
        selected_bar(ui, ctx, st, row, actions);
    }
    // The list: tiles and details in a viewport that scrolls when taller than its budget.
    ui.gap(ctx.px(5.0));
    let width = ui.available_width();
    let s = ctx.settings();
    let reserved = 150.0 + if selected.is_some() { 120.0 } else { 0.0 };
    let budget = (((s.panel_max_height - reserved) / s.panel_scale.max(0.1)).clamp(72.0, 380.0)
        as f32)
        * ctx.scale;
    let key = key_of(&[
        base_key(ctx),
        hash_str(st.selected.as_deref().unwrap_or("")),
        data.paused as u64,
        st.thumbs.len() as u64,
    ]);
    let natural = {
        let st_ref = &mut *st;
        let was = st_ref.decode;
        st_ref.decode = false;
        let h = list.measure(ctx, width, key, |ui| {
            content(ui, ctx, st_ref, data, today, &mut Vec::new());
        });
        st_ref.decode = was;
        h
    };
    if natural <= budget + 0.5 {
        content(ui, ctx, st, data, today, actions);
    } else {
        let inner_key = key ^ 0x5151;
        let inner_w = width - GUTTER;
        let _ = (inner_key, inner_w);
        ui.scroll_area_h("clips-list", budget, |ui| {
            content(ui, ctx, st, data, today, actions);
        });
    }
}

impl Page for ClipsPage {
    fn id(&self) -> &'static str {
        "clips"
    }

    fn height(&mut self, ctx: &PageCtx, width: f32) -> f32 {
        let data = data_of(ctx);
        let key = key_of(&[
            base_key(ctx),
            hash_str(self.st.selected.as_deref().unwrap_or("")),
            hash_str(&self.st.status),
            data.paused as u64,
        ]);
        let was = self.st.decode;
        self.st.decode = false;
        let (st, list) = (&mut self.st, &mut self.list);
        let h = self.shell.measure(ctx, width, key, |ui| {
            layout(ui, ctx, st, list, &data, &mut Vec::new());
        });
        self.st.decode = was;
        h
    }

    fn frame(
        &mut self,
        canvas: &mut Canvas,
        ctx: &PageCtx,
        rect: Rect,
        input: &InputState,
        out: &mut Vec<Command>,
    ) -> bool {
        let data = data_of(ctx);
        let mut actions = Vec::new();
        self.st.decode = true;
        let (st, list) = (&mut self.st, &mut self.list);
        let again = self.shell.run(canvas, ctx, rect, input, out, |ui| {
            layout(ui, ctx, st, list, &data, &mut actions);
        });
        let acted = !actions.is_empty();
        apply(ctx, &mut self.st, actions, out);
        again || acted || self.st.pending || !self.st.decode
    }

    /// The hover face (`HoverTools.BuildToolPeek`): the heading, the two newest clips (one
    /// before the detailed stage) and an "Open clipboard" hint.
    fn peek(
        &mut self,
        canvas: &mut Canvas,
        ctx: &PageCtx,
        rect: Rect,
        _input: &InputState,
        _out: &mut Vec<Command>,
        detailed: bool,
    ) -> Option<bool> {
        let th = ctx.theme;
        let heading = ctx.style(13.0, super::tools_common::heading_weight(ctx), th.text);
        canvas.text(
            ctx.fonts,
            rect.x,
            rect.y,
            tr(ctx, "Clipboard"),
            &heading,
            Some(rect.w),
        );
        let mut y = rect.y + ctx.fonts.line_height(&heading) + ctx.px(6.0);
        let (enabled, count, newest) = {
            let p = ctx.snaps.productivity.borrow();
            let newest: Vec<(String, bool, String)> = p
                .clips()
                .iter()
                .take(2)
                .map(|c| (c.id.clone(), c.is_image(), c.label()))
                .collect();
            (p.clipboard_enabled(), p.clips().len(), newest)
        };
        let note = ctx.style(11.0, Weight::Regular, th.text_dim);
        if !enabled {
            canvas.text(
                ctx.fonts,
                rect.x,
                y,
                tr(
                    ctx,
                    "History is off. Enable it in Settings \u{2192} Controls.",
                ),
                &note,
                Some(rect.w),
            );
            y += ctx.fonts.line_height(&note);
        } else if count == 0 {
            canvas.text(
                ctx.fonts,
                rect.x,
                y,
                tr(ctx, "Nothing captured yet"),
                &note,
                Some(rect.w),
            );
            y += ctx.fonts.line_height(&note);
        } else {
            for (i, (id, is_image, label)) in newest.iter().enumerate() {
                if i > 0 && !detailed {
                    break;
                }
                let row_h = ctx.px(28.0);
                let mut x = rect.x;
                if *is_image {
                    let side = ctx.px(24.0);
                    let px = naby_render::image::thumbnail_pixels(side, 1.0);
                    self.st.decode = true;
                    if let Some(img) = thumb(ctx, &mut self.st, id, px) {
                        let dst = fit_rect(
                            img.width(),
                            img.height(),
                            Rect::new(x, y + (row_h - side) / 2.0, side, side),
                        );
                        canvas.draw_image(&img, dst, 1.0, 0.0);
                    }
                    x += side + ctx.px(8.0);
                }
                let st = ctx.style(12.0, Weight::Regular, th.text);
                let lh = ctx.fonts.line_height(&st);
                let shown = if *is_image {
                    tr(ctx, "Image")
                } else {
                    label.as_str()
                };
                canvas.text(
                    ctx.fonts,
                    x,
                    y + (row_h - lh) / 2.0,
                    shown,
                    &st,
                    Some((rect.right() - x).max(0.0)),
                );
                y += row_h;
            }
        }
        super::tools_common::peek_button(
            canvas,
            ctx,
            rect.x,
            y + ctx.px(5.0),
            tr(ctx, "Open clipboard"),
        );
        Some(self.st.pending)
    }

    fn next_frame(&self, now: f64) -> Option<f64> {
        self.shell.next_frame().map(|t| t.max(now))
    }

    fn captures_pointer(&self) -> bool {
        self.shell.captures_pointer()
    }

    fn on_hide(&mut self) {
        self.shell.reset();
        self.list.reset();
        self.st.thumbs.clear();
        self.st.selected = None;
        self.st.status.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pages::tools_common::test_support::Rig;
    use naby_core::productivity::ProductivityService;

    fn rig_with_history(dir: &std::path::Path) -> Option<Rig> {
        let mut rig = Rig::new("clips")?;
        rig.tall();
        let mut service = ProductivityService::new(dir, Default::default());
        service.set_clipboard_enabled(true);
        service.ingest_text(Some("hello world"));
        service.ingest_text(Some("https://example.com/a?utm_source=x&id=3"));
        service.ingest_text(Some("#3366CC"));
        *rig.snaps.productivity.borrow_mut() = service;
        Some(rig)
    }

    fn texts(rig: &Rig) -> Vec<String> {
        rig.snaps
            .productivity
            .borrow()
            .clips()
            .iter()
            .map(|c| c.text.clone())
            .collect()
    }

    #[test]
    fn history_off_offers_to_enable_it() {
        let Some(mut rig) = Rig::new("clips") else {
            return;
        };
        rig.paint();
        assert!(rig.has("clip-enable"));
        rig.save("page-clips-off");
        rig.click("clip-enable");
        let out = rig.commands();
        let Some(Command::ApplySettings(s)) = out.first() else {
            panic!("{out:?}");
        };
        assert!(s.clipboard_history);
    }

    #[test]
    fn an_enabled_empty_history_shows_the_hint() {
        let Some(mut rig) = Rig::new("clips") else {
            return;
        };
        rig.snaps
            .productivity
            .borrow_mut()
            .set_clipboard_enabled(true);
        rig.paint();
        assert!(rig.has("clip-pause") && !rig.has("clip-enable"));
        rig.save("page-clips-empty");
    }

    #[test]
    fn tiles_select_copy_variants_and_pin() {
        let dir = tempfile::tempdir().unwrap();
        let Some(mut rig) = rig_with_history(dir.path()) else {
            return;
        };
        rig.paint();
        rig.save("page-clips-list");
        // Quick copy on a tile.
        rig.hover("clip-tile-hello world");
        rig.click("clip-copy-hello world");
        assert_eq!(
            rig.commands(),
            vec![Command::CopyText("hello world".into())]
        );
        // Select the link: cleaned copy.
        rig.click("clip-tile-https://example.com/a?utm_source=x&id=3");
        rig.paint();
        rig.save("page-clips-link");
        assert!(rig.has("clip-action-Copy cleaned link"));
        rig.click("clip-action-Copy cleaned link");
        assert_eq!(
            rig.commands(),
            vec![Command::CopyText("https://example.com/a?id=3".into())]
        );
        rig.click("clip-action-Copy original");
        assert_eq!(
            rig.commands(),
            vec![Command::CopyText(
                "https://example.com/a?utm_source=x&id=3".into()
            )]
        );
        // Pin it: pinned clips list first.
        rig.click("clip-action-Pin");
        assert!(rig
            .snaps
            .productivity
            .borrow()
            .clips()
            .iter()
            .any(|c| c.pinned));
        rig.paint();
        assert!(rig.has("clip-action-Unpin"));
    }

    #[test]
    fn color_clips_copy_hex_and_rgb() {
        let dir = tempfile::tempdir().unwrap();
        let Some(mut rig) = rig_with_history(dir.path()) else {
            return;
        };
        rig.paint();
        rig.click("clip-tile-#3366CC");
        rig.paint();
        rig.save("page-clips-color");
        rig.click("clip-action-Copy HEX");
        rig.click("clip-action-Copy RGB");
        assert_eq!(
            rig.commands(),
            vec![
                Command::CopyText("#3366CC".into()),
                Command::CopyText("rgb(51, 102, 204)".into())
            ]
        );
    }

    #[test]
    fn removing_and_clearing_and_pausing_update_the_service() {
        let dir = tempfile::tempdir().unwrap();
        let Some(mut rig) = rig_with_history(dir.path()) else {
            return;
        };
        rig.paint();
        rig.click("clip-tile-hello world");
        rig.click("clip-action-Remove");
        assert_eq!(texts(&rig).len(), 2);
        rig.paint();
        rig.click("clip-pause");
        assert!(rig.snaps.productivity.borrow().clipboard_paused());
        rig.paint();
        rig.save("page-clips-paused");
    }

    #[test]
    fn image_clips_decode_a_thumbnail_and_copy_as_image() {
        let dir = tempfile::tempdir().unwrap();
        let Some(mut rig) = rig_with_history(dir.path()) else {
            return;
        };
        let mut pixels = Canvas::new(40, 24);
        pixels.clear(Rgba::rgb(200, 60, 40));
        let png_path = dir.path().join("src.png");
        pixels.save_png(&png_path).unwrap();
        let png = std::fs::read(&png_path).unwrap();
        assert!(rig.snaps.productivity.borrow_mut().ingest_image(&png));
        rig.paint();
        rig.paint();
        let id = rig.snaps.productivity.borrow().clips()[0].id.clone();
        assert!(rig.snaps.productivity.borrow().clips()[0].is_image());
        rig.save("page-clips-image");
        rig.click("clip-tile-");
        rig.paint();
        rig.save("page-clips-image-selected");
        rig.click("clip-action-Copy image");
        assert!(rig.commands().contains(&Command::CopyClipImage(id.clone())));
        rig.click("clip-action-Save as PNG\u{2026}");
        assert!(rig.commands().contains(&Command::SaveClipImage { id }));
    }

    #[test]
    fn the_hover_face_covers_off_empty_and_filled_histories() {
        let dir = tempfile::tempdir().unwrap();
        let Some(mut rig) = Rig::new("clips") else {
            return;
        };
        let area = Rect::new(0.0, 0.0, 380.0, 160.0);
        assert_eq!(rig.peek(true), Some(false));
        rig.save("page-clips-peek-off");
        rig.snaps
            .productivity
            .borrow_mut()
            .set_clipboard_enabled(true);
        rig.peek(true);
        rig.save("page-clips-peek-empty");
        let empty = rig.ink(area);
        let mut rig2 = rig_with_history(dir.path()).unwrap();
        rig2.peek(false);
        let brief = rig2.ink(area);
        rig2.save("page-clips-peek-brief");
        rig2.peek(true);
        rig2.save("page-clips-peek-detailed");
        assert!(brief > empty / 2 && rig2.ink(area) > brief);
    }
}
