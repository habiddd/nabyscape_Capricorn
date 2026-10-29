//! The notification card (C# `NotificationOverlay.cs`) and the edge marker shown while a
//! tucked dock has an unread notice.

use super::util::{fade, style, text_centered, Hot};
use super::Overlay;
use crate::host::Snapshots;
use naby_core::color::Rgba;
use naby_core::command::Command;
use naby_core::motion::SizeF;
use naby_core::notifications::{InternalNotice, NoticeKind};
use naby_core::settings::Settings;
use naby_render::icons::DEFAULT_STROKE;
use naby_render::{Canvas, Fill, Fonts, Icon, Rect, Weight};

fn kind_label(kind: NoticeKind) -> &'static str {
    match kind {
        NoticeKind::Timer => "TIMER",
        NoticeKind::Focus => "FOCUS",
        NoticeKind::Downloads => "DOWNLOADS",
        NoticeKind::Updates => "UPDATE",
        NoticeKind::Failure => "NABYSCAPE",
    }
}

fn kind_icon(kind: NoticeKind) -> Icon {
    match kind {
        NoticeKind::Timer => Icon::Timer,
        NoticeKind::Focus => Icon::Focus,
        NoticeKind::Downloads => Icon::Folder,
        NoticeKind::Updates => Icon::Settings,
        NoticeKind::Failure => Icon::NavStatus,
    }
}

/// Existing files of a downloads notice (at most 40).
fn existing_files(notice: &InternalNotice) -> Vec<String> {
    notice
        .files
        .iter()
        .flatten()
        .filter(|f| std::path::Path::new(f).exists())
        .take(40)
        .cloned()
        .collect()
}

/// Card width in DIP (`Math.Min(Math.Min(370, HoverWidth), Math.Max(180, work - 24))`).
pub fn card_width(settings: &Settings, area: SizeF) -> f64 {
    370.0f64
        .min(settings.hover_width)
        .min((area.w - 24.0).max(180.0))
}

/// Surface size (DIP) before text can be measured: one line per 6.2 DIP of detail text.
pub fn estimate_size(
    notice: &InternalNotice,
    _count: usize,
    settings: &Settings,
    area: SizeF,
    notch: bool,
) -> (f64, f64) {
    let width = card_width(settings, area);
    let cw = width - if notch { 64.0 } else { 32.0 };
    let per_line = (cw / 6.2).max(8.0);
    let lines = (notice.detail.chars().count() as f64 / per_line)
        .ceil()
        .max(1.0);
    let mut h = 28.0 + 12.0 + 22.0 + 6.0 + lines * 18.0 + 15.0 + 2.0 + 4.0 + 10.0 + 30.0;
    if notice.kind == NoticeKind::Downloads {
        h += 10.0 + 32.0 + 8.0 + 30.0;
    }
    let max = 370.0f64.min((area.h - 48.0).max(100.0));
    (width, h.min(max) + 32.0)
}

impl Overlay {
    /// Draws the card, measuring it with the real fonts; a size change retargets the surface.
    pub(crate) fn paint_notice(
        &mut self,
        canvas: &mut Canvas,
        fonts: &Fonts,
        snaps: &Snapshots,
        rect: Rect,
    ) {
        let Some(notice) = snaps.notices.current.clone() else {
            return;
        };
        let s = self.scale;
        let k = self.cfg.theme.text_scale;
        let text = self.cfg.theme.text;
        let dim = self.cfg.theme.text_dim;
        let entry = self.notice.entry;
        let now = self.now;
        let (alpha, dy) = entry.map_or((1.0, 0.0), |e| {
            (
                e.opacity.value(now) as f32,
                e.offset_y.value(now) as f32 * s,
            )
        });
        let area = self.area_dip(snaps);
        let width = card_width(&snaps.settings, area);
        let cw = (width - if self.cfg.notch { 64.0 } else { 32.0 }) as f32 * s;
        let x = rect.x + (rect.w - cw) / 2.0;
        let mut y = rect.y + 16.0 * s + dy;
        let top = y;
        let files = existing_files(&notice);
        // Registered first so the buttons drawn later sit on top of it.
        self.hot.add(
            Hot::NoticeCard,
            Rect::new(x, top, cw, self.notice.size.1 as f32 * s),
        );
        canvas.with_opacity(alpha, |canvas| {
            // Header.
            let icon_box = Rect::new(x, y, 28.0 * s, 28.0 * s);
            canvas.fill_rrect(icon_box, 9.0 * s, Rgba::WHITE.with_alpha(0x19));
            canvas.icon(
                kind_icon(notice.kind),
                icon_box.inset(7.0 * s),
                text,
                DEFAULT_STROKE,
            );
            let kst = style(10.0 * s * k, Weight::Regular, dim);
            let kh = fonts.line_height(&kst);
            canvas.text(
                fonts,
                x + 37.0 * s,
                y + (28.0 * s - kh) / 2.0,
                kind_label(notice.kind),
                &kst,
                None,
            );
            if snaps.notices.count > 1 {
                let q = format!("{} queued", snaps.notices.count);
                let qw = fonts.measure_width(&q, &kst);
                canvas.text(
                    fonts,
                    x + cw - qw,
                    y + (28.0 * s - kh) / 2.0,
                    &q,
                    &kst,
                    None,
                );
            }
            y += 40.0 * s;
            // Title and detail.
            let tst = style(18.0 * s * k, Weight::Semibold, text);
            let tlines = fonts.wrap(&notice.title, &tst, cw);
            let tlh = fonts.line_height(&tst);
            for line in &tlines {
                canvas.text(fonts, x, y, line, &tst, None);
                y += tlh;
            }
            y += 6.0 * s;
            let dst = style(12.0 * s * k, Weight::Regular, dim);
            let dlines = fonts.wrap(&notice.detail, &dst, cw);
            let dlh = 18.0 * s;
            let dh = fonts.line_height(&dst);
            for line in &dlines {
                canvas.text(fonts, x, y + (dlh - dh) / 2.0, line, &dst, None);
                y += dlh;
            }
            // Accent rule.
            y += 15.0 * s;
            let rule = Rect::new(x, y, cw, 2.0 * s);
            canvas.fill_round_rect(
                rule,
                1.0 * s,
                &Fill::Linear {
                    from: (rule.x, 0.0),
                    to: (rule.right(), 0.0),
                    stops: vec![
                        (0.0, Rgba::rgb(0xC4, 0xC9, 0xD2)),
                        (1.0, Rgba::rgb(0x30, 0x34, 0x3C)),
                    ],
                },
            );
            y += 6.0 * s;
            // Files.
            if !files.is_empty() {
                y += 10.0 * s;
                let name = std::path::Path::new(&files[0])
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let label = if files.len() > 1 {
                    format!("{name} +{}", files.len() - 1)
                } else {
                    name
                };
                let thumb = Rect::new(x, y, 32.0 * s, 32.0 * s);
                canvas.fill_rrect(thumb, 6.0 * s, Rgba::WHITE.with_alpha(0x19));
                canvas.icon(Icon::FileText, thumb.inset(7.0 * s), text, DEFAULT_STROKE);
                let chip = Rect::new(x + 42.0 * s, y + 1.0 * s, cw - 42.0 * s, 30.0 * s);
                self.ui_button(canvas, fonts, chip, &label, None, true);
                y += 32.0 * s + 8.0 * s;
                let row = Rect::new(x, y, cw, 30.0 * s);
                let w1 = fonts.measure_width(
                    "Show in folder",
                    &style(11.0 * s * k, Weight::Regular, text),
                ) + 18.0 * s;
                let b1 = Rect::new(row.x, row.y, w1, row.h);
                self.ui_button(
                    canvas,
                    fonts,
                    b1,
                    "Show in folder",
                    Some(Hot::NoticeFolder),
                    true,
                );
                let w2 = fonts
                    .measure_width("Add to shelf", &style(11.0 * s * k, Weight::Regular, text))
                    + 18.0 * s;
                let b2 = Rect::new(b1.right() + 6.0 * s, row.y, w2, row.h);
                self.ui_button(
                    canvas,
                    fonts,
                    b2,
                    "Add to shelf",
                    Some(Hot::NoticeShelf),
                    true,
                );
                y += 30.0 * s;
            } else if notice.kind == NoticeKind::Downloads {
                let st = style(11.0 * s * k, Weight::Regular, dim);
                canvas.text(
                    fonts,
                    x,
                    y + 4.0 * s,
                    "These files are no longer available.",
                    &st,
                    Some(cw),
                );
                y += 22.0 * s;
            }
            if notice.kind == NoticeKind::Updates {
                let b = Rect::new(x, y + 10.0 * s, 110.0 * s, 30.0 * s);
                self.ui_button(
                    canvas,
                    fonts,
                    b,
                    "Open Settings",
                    Some(Hot::NoticeSettings),
                    true,
                );
                y += 40.0 * s;
            }
            // Actions.
            y += 10.0 * s;
            let mut bx = x;
            let labels = [
                ("Dismiss", Hot::NoticeDismiss, true),
                ("Next", Hot::NoticeNext, snaps.notices.count > 1),
                ("Clear all", Hot::NoticeClear, true),
            ];
            for (label, hot, enabled) in labels {
                let w = fonts.measure_width(label, &style(11.0 * s * k, Weight::Regular, text))
                    + 18.0 * s;
                let b = Rect::new(bx, y, w, 30.0 * s);
                self.ui_button(canvas, fonts, b, label, Some(hot), enabled);
                bx += w + 6.0 * s;
            }
            y += 30.0 * s;
        });
        // Re-measure: the surface follows the card.
        let content = f64::from((y - top) / s);
        let max = 370.0f64.min((area.h - 48.0).max(100.0));
        let wanted = (width, content.min(max) + 32.0);
        if (wanted.1 - self.notice.size.1).abs() > 1.0
            || (wanted.0 - self.notice.size.0).abs() > 0.5
        {
            self.notice.size = wanted;
            self.retarget_pending = true;
        }
    }

    /// A control-style button (`UI.Button`): fill, border, hover light.
    pub(crate) fn ui_button(
        &mut self,
        canvas: &mut Canvas,
        fonts: &Fonts,
        r: Rect,
        label: &str,
        hot: Option<Hot>,
        enabled: bool,
    ) {
        let s = self.scale;
        let k = self.cfg.theme.text_scale;
        let t = &self.cfg.theme;
        let (fill, border, color, hover) = (
            t.control_fill,
            t.control_border,
            t.control_text,
            t.hover_fill,
        );
        canvas.with_opacity(if enabled { 1.0 } else { 0.4 }, |canvas| {
            canvas.fill_rrect(r, 8.0 * s, fill);
            canvas.stroke_round_rect(r, 8.0 * s, border, 1.0);
            let hovered = enabled && self.pointer.is_some_and(|p| r.contains_pt(p));
            if hovered {
                canvas.fill_rrect(r, 8.0 * s, hover);
            }
            text_centered(
                canvas,
                fonts,
                r.inset_xy(6.0 * s, 0.0),
                label,
                &style(11.0 * s * k, Weight::Regular, color),
            );
        });
        if let (Some(hot), true) = (hot, enabled) {
            self.hot.add(hot, r);
        }
    }

    pub(crate) fn activate_notice(&mut self, hot: Hot, snaps: &Snapshots) {
        let files: Vec<String> = snaps
            .notices
            .current
            .as_ref()
            .map(existing_files)
            .unwrap_or_default();
        match hot {
            Hot::NoticeDismiss => {
                self.collapse(snaps);
                self.push(Command::NoticeDismiss);
            }
            Hot::NoticeNext => {
                self.collapse(snaps);
                self.push(Command::NoticeNext);
            }
            Hot::NoticeClear => {
                self.collapse(snaps);
                self.push(Command::NoticeClearAll);
            }
            Hot::NoticeFolder => {
                if let Some(first) = files.first() {
                    self.push(Command::RevealInFolder(first.clone()));
                }
            }
            Hot::NoticeShelf => {
                if !files.is_empty() {
                    self.push(Command::ShelfAddFiles(files));
                }
            }
            Hot::NoticeSettings => self.push(Command::OpenSettings),
            _ => {}
        }
    }

    /// A white bar under a tucked dock that has an unread notice (`RefreshNoticeEdge`).
    pub(crate) fn paint_notice_edge(&self, canvas: &mut Canvas, snaps: &Snapshots, rect: Rect) {
        if !self.tuck.is_tucked() || snaps.notices.current.is_none() {
            return;
        }
        let s = self.scale;
        let bar = Rect::new(
            rect.x + (rect.w - 18.0 * s) / 2.0,
            rect.y + (snaps.settings.height as f32 - 3.0) * s,
            18.0 * s,
            2.0 * s,
        );
        canvas.fill_rrect(bar, 1.0 * s, fade(Rgba::WHITE, 1.0));
    }
}
