//! The message list of the chat: bubbles, Markdown, attachment cards and image replies
//! (`AssistantView.Refresh` + `AssistantAttachmentView`).
//!
//! Layout is cached per message and width; painting only walks the rows in view. Clicks are not
//! handled here: painting registers [`Action`] hit regions that the chat view resolves after the
//! frame.

use std::collections::HashMap;
use std::sync::Arc;

use naby_core::assistant::attachments::describe;
use naby_core::assistant::markdown;
use naby_core::assistant::Provider;
use naby_render::{Canvas, Fonts, Icon, InputState, Rect, TextStyle, Weight};

use super::blocks::{self, Ctx, Laid};
use super::previews::{Preview, PreviewCache};
use super::session::{ChatItem, ChatModel, ChatRole, ShownFile};
use super::setup::SetupTab;

/// What a click on a registered region does.
#[derive(Clone)]
pub enum Action {
    OpenUrl(String),
    CopyText(String),
    OpenPreview(Arc<ShownFile>),
    CopyImage(Arc<ShownFile>),
    SaveFile(Arc<ShownFile>),
    RemoveDraft(u64),
    CloseLightbox,
    CloseMenus,
    TogglePreview,
    PickShare(String),
    OpenSetup(SetupTab),
    OpenSettingsPage(&'static str),
    Noop,
}

impl std::fmt::Debug for Action {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Action::OpenUrl(u) => write!(f, "OpenUrl({u})"),
            Action::CopyText(_) => f.write_str("CopyText"),
            Action::OpenPreview(_) => f.write_str("OpenPreview"),
            Action::CopyImage(_) => f.write_str("CopyImage"),
            Action::SaveFile(_) => f.write_str("SaveFile"),
            Action::RemoveDraft(id) => write!(f, "RemoveDraft({id})"),
            Action::CloseLightbox => f.write_str("CloseLightbox"),
            Action::CloseMenus => f.write_str("CloseMenus"),
            Action::TogglePreview => f.write_str("TogglePreview"),
            Action::PickShare(id) => write!(f, "PickShare({id})"),
            Action::OpenSetup(tab) => write!(f, "OpenSetup({tab:?})"),
            Action::OpenSettingsPage(page) => write!(f, "OpenSettingsPage({page})"),
            Action::Noop => f.write_str("Noop"),
        }
    }
}

/// Hit regions registered while painting, resolved top-most first.
pub type Hits = Vec<(Rect, Action)>;

#[derive(Debug, Clone, Copy, PartialEq)]
struct Sig {
    width: f32,
    s: f32,
    t: f32,
    contrast: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CardAction {
    Preview,
    Copy,
    Save,
}

struct CardButton {
    rect: Rect,
    label: &'static str,
    action: CardAction,
}

struct Card {
    shown: Arc<ShownFile>,
    /// Relative to the content origin of the message.
    rect: Rect,
    name: String,
    note_lines: Vec<String>,
    preview: Option<Rect>,
    buttons: Vec<CardButton>,
}

struct ItemLayout {
    sig: Sig,
    text: Laid,
    cards: Vec<Card>,
    height: f32,
}

struct LiveLayout {
    sig: Sig,
    length: usize,
    laid: Laid,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RowKind {
    Item(usize),
    Live,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Row {
    pub kind: RowKind,
    pub y: f32,
    pub h: f32,
}

#[derive(Default)]
pub struct Transcript {
    layouts: HashMap<u64, ItemLayout>,
    live: Option<LiveLayout>,
    pub rows: Vec<Row>,
    pub content_h: f32,
}

impl Transcript {
    pub fn release(&mut self) {
        self.layouts.clear();
        self.live = None;
        self.rows.clear();
        self.content_h = 0.0;
    }

    /// Lays every message out for `width` and records the rows.
    pub fn prepare(&mut self, cx: &Ctx, model: &ChatModel, previews: &PreviewCache, width: f32) {
        let sig = Sig {
            width,
            s: cx.m.s,
            t: cx.m.t,
            contrast: cx.pal.high_contrast,
        };
        let live_ids: std::collections::HashSet<u64> = model.items.iter().map(|i| i.id).collect();
        self.layouts.retain(|id, _| live_ids.contains(id));
        self.rows.clear();
        let mut y = 0.0f32;
        for (index, item) in model.items.iter().enumerate() {
            let stale = self.layouts.get(&item.id).is_none_or(|l| l.sig != sig);
            if stale {
                let layout = layout_item(cx, item, width, sig, previews);
                self.layouts.insert(item.id, layout);
            }
            let h = self.layouts[&item.id].height;
            self.rows.push(Row {
                kind: RowKind::Item(index),
                y,
                h,
            });
            y += h;
        }
        match model.live.as_deref().filter(|t| !t.is_empty()) {
            Some(text) => {
                let stale = self
                    .live
                    .as_ref()
                    .is_none_or(|l| l.sig != sig || l.length != text.len());
                if stale {
                    let inner = assistant_width(cx, width);
                    let laid = blocks::layout_markdown(cx, &markdown::parse(text), inner);
                    self.live = Some(LiveLayout {
                        sig,
                        length: text.len(),
                        laid,
                    });
                }
                let h = self.live.as_ref().map_or(0.0, |l| l.laid.height)
                    + cx.m.px(20.0)
                    + cx.m.px(8.0);
                self.rows.push(Row {
                    kind: RowKind::Live,
                    y,
                    h,
                });
                y += h;
            }
            None => self.live = None,
        }
        self.content_h = y;
    }

    /// Paints the rows that intersect `view` (canvas coordinates) with the content scrolled by
    /// `scroll`, and registers hit regions.
    #[allow(clippy::too_many_arguments)]
    pub fn paint(
        &self,
        cx: &Ctx,
        canvas: &mut Canvas,
        view: Rect,
        scroll: f32,
        model: &ChatModel,
        previews: &PreviewCache,
        input: &InputState,
        hits: &mut Hits,
    ) -> Option<usize> {
        canvas.push_clip_rect(view);
        let pointer = input.pointer.filter(|p| view.contains_pt(*p));
        let mut hovered_item = None;
        for row in &self.rows {
            let top = view.y + row.y - scroll;
            if top > view.bottom() || top + row.h < view.y {
                continue;
            }
            match row.kind {
                RowKind::Item(index) => {
                    let item = &model.items[index];
                    let Some(layout) = self.layouts.get(&item.id) else {
                        continue;
                    };
                    let hover = pointer.is_some_and(|p| p.1 >= top && p.1 < top + row.h);
                    if hover {
                        hovered_item = Some(index);
                    }
                    paint_item(
                        cx, canvas, item, layout, view, top, hover, previews, input, hits,
                    );
                }
                RowKind::Live => {
                    if let Some(live) = &self.live {
                        let x = view.x + cx.m.px(2.0);
                        let y = top + cx.m.px(10.0);
                        blocks::paint(&live.laid, canvas, cx, x, y, (view.y, view.bottom()));
                    }
                }
            }
        }
        canvas.pop_clip();
        hovered_item
    }
}

fn assistant_width(cx: &Ctx, width: f32) -> f32 {
    (width - cx.m.px(14.0)).max(cx.m.px(80.0))
}

fn user_inner_width(cx: &Ctx, width: f32) -> f32 {
    (width - cx.m.px(42.0) - cx.m.px(24.0)).max(cx.m.px(80.0))
}

fn layout_item(
    cx: &Ctx,
    item: &ChatItem,
    width: f32,
    sig: Sig,
    previews: &PreviewCache,
) -> ItemLayout {
    let m = cx.m;
    let (inner, pad_x, pad_y) = match item.role {
        ChatRole::User => (user_inner_width(cx, width), m.px(12.0), m.px(9.0)),
        ChatRole::Assistant => (assistant_width(cx, width), 0.0, m.px(10.0)),
    };
    let text = if item.text.trim().is_empty() {
        Laid::default()
    } else if item.role == ChatRole::Assistant {
        blocks::layout_markdown(cx, &markdown::parse(&item.text), inner)
    } else {
        blocks::layout_plain(cx, &item.text, inner)
    };
    let mut y = text.height;
    let card_w = inner.min(m.px(460.0));
    let mut cards = Vec::new();
    for shown in &item.attachments {
        let mut card = layout_card(cx, shown, card_w, previews);
        card.rect.y = y + m.px(4.0);
        y = card.rect.bottom() + m.px(4.0);
        cards.push(card);
    }
    let body = y;
    let height = m.px(3.0) + body + 2.0 * pad_y + m.px(5.0);
    let _ = pad_x;
    ItemLayout {
        sig,
        text,
        cards,
        height,
    }
}

fn layout_card(cx: &Ctx, shown: &Arc<ShownFile>, width: f32, previews: &PreviewCache) -> Card {
    let m = cx.m;
    let pad = m.px(10.0);
    let inner = width - 2.0 * pad;
    let name_style = TextStyle::new(m.font(11.0), cx.pal.text);
    let note_style = TextStyle::new(m.font(10.0), cx.pal.dim);
    let file = &shown.file;
    let mut note_lines = cx.fonts.wrap(&file.note, &note_style, inner);
    note_lines.truncate(3);
    let note_h = if note_lines.is_empty() {
        0.0
    } else {
        m.px(4.0) + note_lines.len() as f32 * cx.fonts.line_height(&note_style)
    };
    let header_h = cx.fonts.line_height(&name_style).max(m.px(16.0));
    let mut y = pad + header_h + note_h;
    let preview = PreviewCache::has_preview(shown).then(|| {
        let (nw, nh) = previews.natural(shown).unwrap_or((4, 3));
        let (nw, nh) = (nw.max(1) as f32, nh.max(1) as f32);
        // The natural size is in design units; it never grows past that.
        let scale = ((inner / m.s) / nw).min(240.0 / nh).min(1.0);
        let rect = Rect::new(pad, y + m.px(8.0), nw * scale * m.s, nh * scale * m.s);
        y = rect.bottom();
        rect
    });
    // Action pills.
    let pill_style = TextStyle::new(m.font(11.0), cx.pal.text);
    let mut labels: Vec<(&'static str, CardAction)> = Vec::new();
    if preview.is_some() {
        labels.push(("Open preview", CardAction::Preview));
    }
    if file.is_image() {
        labels.push(("Copy image", CardAction::Copy));
    }
    labels.push(("Save\u{2026}", CardAction::Save));
    let mut buttons = Vec::new();
    let mut x = pad;
    let mut row_y = y + m.px(6.0);
    let pill_h = m.px(26.0);
    for (label, action) in labels {
        let w = cx.fonts.measure_width(label, &pill_style) + m.px(26.0);
        if x + w > width - pad && x > pad {
            x = pad;
            row_y += pill_h + m.px(4.0);
        }
        buttons.push(CardButton {
            rect: Rect::new(x, row_y, w, pill_h),
            label,
            action,
        });
        x += w + m.px(5.0);
    }
    let height = row_y + pill_h + pad;
    Card {
        shown: shown.clone(),
        rect: Rect::new(0.0, 0.0, width, height),
        name: file.name.clone(),
        note_lines,
        preview,
        buttons,
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_item(
    cx: &Ctx,
    canvas: &mut Canvas,
    item: &ChatItem,
    layout: &ItemLayout,
    view: Rect,
    top: f32,
    hover: bool,
    previews: &PreviewCache,
    input: &InputState,
    hits: &mut Hits,
) {
    let m = cx.m;
    let pal = cx.pal;
    let body_top = top + m.px(3.0);
    let (x, pad_x, pad_y, width) = match item.role {
        ChatRole::User => {
            let left = m.px(42.0);
            (view.x + left, m.px(12.0), m.px(9.0), view.w - left)
        }
        ChatRole::Assistant => (
            view.x + m.px(2.0),
            0.0,
            m.px(10.0),
            assistant_width(cx, view.w),
        ),
    };
    let body_h = layout.height - m.px(3.0) - m.px(5.0);
    if item.role == ChatRole::User {
        let bubble = Rect::new(x, body_top, width, body_h);
        canvas.fill_rrect(bubble, m.px(12.0), pal.control_fill);
        if pal.high_contrast {
            canvas.stroke_round_rect(bubble, m.px(12.0), pal.control_border, m.s.max(1.0));
        }
    }
    let ox = x + pad_x;
    let oy = body_top + pad_y;
    let band = (view.y, view.bottom());
    blocks::paint(&layout.text, canvas, cx, ox, oy, band);
    let pointer = input.pointer;
    let inside = |r: Rect| pointer.is_some_and(|p| r.contains_pt(p) && view.contains_pt(p));
    for link in &layout.text.links {
        let r = link.rect.translate(ox, oy);
        hits.push((r, Action::OpenUrl(link.url.clone())));
        if inside(r) {
            canvas.fill_rrect(r, m.px(3.0), pal.link.with_alpha(0x22));
        }
    }
    for copy in &layout.text.copies {
        let r = copy.rect.translate(ox, oy);
        if inside(r) {
            canvas.fill_rrect(r, m.px(5.0), pal.selection_fill.with_alpha(0x55));
            let style = TextStyle::new(m.font(10.0), pal.text);
            canvas.text(
                cx.fonts,
                r.x + m.px(6.0),
                r.y + (r.h - cx.fonts.line_height(&style)) / 2.0,
                "Copy",
                &style,
                None,
            );
        }
        hits.push((r, Action::CopyText(copy.text.clone())));
    }
    for card in &layout.cards {
        paint_card(cx, canvas, card, ox, oy, view, previews, input, hits);
    }
    // "Copy message" while the pointer is over a message.
    if hover && !item.text.trim().is_empty() {
        let size = m.px(24.0);
        let corner = if item.role == ChatRole::User {
            Rect::new(
                x + width - size - m.px(6.0),
                body_top + m.px(4.0),
                size,
                size,
            )
        } else {
            Rect::new(x + width - size, body_top + m.px(2.0), size, size)
        };
        if corner.y + size > view.y && corner.y < view.bottom() {
            let over = inside(corner);
            canvas.fill_rrect(
                corner,
                m.px(7.0),
                if over { pal.selection_fill } else { pal.card },
            );
            canvas.stroke_round_rect(corner, m.px(7.0), pal.card_border, m.s.max(1.0));
            let icon = corner.inset(m.px(5.0));
            canvas.icon(
                Icon::Copy,
                icon,
                if over { pal.selection_text } else { pal.dim },
                1.6,
            );
            hits.push((corner, Action::CopyText(item.text.clone())));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_card(
    cx: &Ctx,
    canvas: &mut Canvas,
    card: &Card,
    ox: f32,
    oy: f32,
    view: Rect,
    previews: &PreviewCache,
    input: &InputState,
    hits: &mut Hits,
) {
    let m = cx.m;
    let pal = cx.pal;
    let rect = card.rect.translate(ox, oy);
    if rect.bottom() < view.y || rect.y > view.bottom() {
        return;
    }
    canvas.fill_rrect(rect, m.px(10.0), pal.card);
    canvas.stroke_round_rect(rect, m.px(10.0), pal.card_border, m.s.max(1.0));
    let pad = m.px(10.0);
    let file = &card.shown.file;
    let icon = if file.is_image() {
        Icon::Image
    } else {
        Icon::FileText
    };
    canvas.icon(
        icon,
        Rect::new(rect.x + pad, rect.y + pad, m.px(15.0), m.px(15.0)),
        pal.dim,
        1.6,
    );
    let name_style = TextStyle::new(m.font(11.0), pal.text);
    canvas.text(
        cx.fonts,
        rect.x + pad + m.px(23.0),
        rect.y + pad,
        &card.name,
        &name_style,
        Some(rect.w - 2.0 * pad - m.px(23.0)),
    );
    let note_style = TextStyle::new(m.font(10.0), pal.dim);
    let line_h = cx.fonts.line_height(&note_style);
    let mut y = rect.y + pad + cx.fonts.line_height(&name_style).max(m.px(16.0)) + m.px(4.0);
    for line in &card.note_lines {
        canvas.text(cx.fonts, rect.x + pad, y, line, &note_style, None);
        y += line_h;
    }
    if let Some(preview) = card.preview {
        let target = preview.translate(rect.x, rect.y);
        let pixels = ((target.w.max(target.h) / 32.0).ceil() * 32.0) as u32;
        match previews.get(&card.shown, pixels) {
            Preview::Ready(image) => {
                canvas.draw_image(&image, target, 1.0, m.px(6.0));
                hits.push((target, Action::OpenPreview(card.shown.clone())));
            }
            Preview::Loading => {
                canvas.fill_rrect(target, m.px(6.0), pal.control_fill);
                let style = TextStyle::new(m.font(10.0), pal.dim);
                canvas.text(
                    cx.fonts,
                    target.x + m.px(8.0),
                    target.y + m.px(8.0),
                    "Loading preview\u{2026}",
                    &style,
                    None,
                );
            }
            Preview::Failed => {
                canvas.fill_rrect(target, m.px(6.0), pal.control_fill);
                let style = TextStyle::new(m.font(10.0), pal.dim);
                for (i, line) in cx
                    .fonts
                    .wrap("Preview unavailable. The image may need a different codec, or its link may have expired. Save the file to view it.", &style, target.w - m.px(16.0))
                    .iter()
                    .take(4)
                    .enumerate()
                {
                    canvas.text(cx.fonts, target.x + m.px(8.0), target.y + m.px(8.0) + i as f32 * line_h, line, &style, None);
                }
            }
            Preview::None => {}
        }
    }
    let pill_style = TextStyle::new(m.font(11.0), pal.text);
    for button in &card.buttons {
        let r = button.rect.translate(rect.x, rect.y);
        let over = input
            .pointer
            .is_some_and(|p| r.contains_pt(p) && view.contains_pt(p));
        canvas.fill_rrect(
            r,
            m.px(8.0),
            if over {
                pal.selection_fill
            } else {
                pal.control_fill
            },
        );
        let color = if over { pal.selection_text } else { pal.text };
        let style = TextStyle {
            color,
            ..pill_style
        };
        let w = cx.fonts.measure_width(button.label, &style);
        canvas.text(
            cx.fonts,
            r.x + (r.w - w) / 2.0,
            r.y + (r.h - cx.fonts.line_height(&style)) / 2.0,
            button.label,
            &style,
            None,
        );
        hits.push((
            r,
            match button.action {
                CardAction::Preview => Action::OpenPreview(card.shown.clone()),
                CardAction::Copy => Action::CopyImage(card.shown.clone()),
                CardAction::Save => Action::SaveFile(card.shown.clone()),
            },
        ));
    }
}

/// Text under a draft chip for the selected provider.
pub fn draft_note(file: &ShownFile, provider: Option<&Provider>) -> String {
    describe(&file.file, provider)
}

/// Measures one line of `fonts`, used for sizing in the chat view.
pub fn line_height(fonts: &Fonts, size: f32) -> f32 {
    fonts.line_height(&TextStyle {
        size,
        weight: Weight::Regular,
        color: naby_core::color::Rgba::WHITE,
    })
}
