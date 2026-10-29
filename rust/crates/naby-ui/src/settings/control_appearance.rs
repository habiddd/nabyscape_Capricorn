//! Control colors and the examples that show them (C# `SettingsControlAppearance.cs`).

use super::preview::Pv;
use super::SettingsView;
use naby_core::color::Rgba;
use naby_render::{Fill, Icon, Rect, Theme, Ui};

impl SettingsView {
    /// "Controls & outlines": button, outline and selection colors.
    pub(super) fn control_color_settings<'a>(&mut self, ui: &mut Ui<'a>) {
        self.section(ui, "Controls & outlines", |this, ui| {
            this.muted(
                ui,
                "Color buttons, input fields, selected tabs and mini-window outlines. Open a picker to see the change beside these controls. Island and accent colors are separate.",
                11.0,
            );
            ui.gap(6.0);
            let (fill, border, selection) = (
                this.s.control_fill.clone(),
                this.s.control_border.clone(),
                this.s.selection_fill.clone(),
            );
            this.color_control(ui, "Control background", &fill, &["#16171B", "#101010", "#15202A", "#211E1B"], |s, v| s.control_fill = v);
            this.color_control(ui, "Control outlines", &border, &["#303238", "#202125", "#4B5664", "#645C50"], |s, v| s.control_border = v);
            this.color_control(ui, "Selected controls", &selection, &["#34373E", "#22252B", "#304455", "#514A3D"], |s, v| s.selection_fill = v);
        });
    }
}

fn parse(s: &str, fallback: Rgba) -> Rgba {
    Rgba::parse_hex(s).unwrap_or(fallback)
}

/// "Smart clipboard" sample (`ClipboardExample`).
pub(super) fn clipboard_example(pv: &mut Pv, a: Rect) {
    let white = pv.white();
    let dim = pv.dim();
    let bold = pv.s.bold_headings;
    let mut y = a.y;
    y += pv.text(a.x, y, "Smart clipboard", 16.0, bold, white, None);
    y += 8.0;
    pv.switch(a.right() - 34.0, y, false);
    pv.text(a.x, y + 3.0, "Pause capture", 13.0, false, white, None);
    y += 28.0;
    let fill = parse(&pv.s.control_fill, pv.th.control_fill);
    pv.rr(Rect::new(a.x, y + 8.0, 42.0, 42.0), 8.0, fill);
    pv.icon(
        Icon::Image,
        a.x + 9.5,
        y + 17.5,
        23.0,
        Theme::contrast_text(fill),
    );
    pv.text(
        a.x + 54.0,
        y + 22.0,
        "Image · just now",
        12.0,
        false,
        white,
        Some(a.w - 56.0),
    );
    y += 66.0;
    let mut x = a.x;
    for kind in ["Text", "Link", "Image", "Color"] {
        let st = pv.style(10.0, false, pv.th.control_text);
        let w = pv.fonts.measure_width(pv.t(kind), &st) + 14.0;
        let r = Rect::new(x, y, w, 24.0);
        pv.rr(r, pv.th.radius, pv.th.control_fill);
        let label = pv.t(kind).to_owned();
        pv.text(
            x + 7.0,
            y + 5.0,
            &label,
            10.0,
            false,
            pv.th.control_text,
            None,
        );
        x += w + 4.0;
    }
    y += 34.0;
    let note = pv
        .t("Example items · your clipboard is untouched")
        .to_owned();
    pv.text(a.x, y, &note, 10.0, false, dim, Some(a.w));
}

/// "Control colors", "Typography" and "Panel navigation" samples (`ControlExample`).
pub(super) fn control_example(pv: &mut Pv, a: Rect, kind: &str) {
    let white = pv.white();
    let dim = pv.dim();
    let s = pv.s;
    match kind {
        "Typography" => {
            let mut y = a.y;
            y += pv.text(a.x, y, "Night Drive", 20.0, s.bold_media_title, white, None);
            y += pv.text(a.x, y, pv.t("Artist · media title"), 11.0, false, dim, None);
            y += 20.0;
            y += pv.text(a.x, y, "File shelf", 17.0, s.bold_headings, white, None) + 5.0;
            pv.text(
                a.x,
                y,
                pv.t("Staged files stay in their original location."),
                12.0,
                false,
                dim,
                Some(a.w),
            );
        }
        "Panel navigation" => {
            let names = ["Media", "Status", "Launch", "Tools"];
            let selection = parse(&s.selection_fill, pv.th.selection_fill);
            let fill = parse(&s.control_fill, pv.th.control_fill);
            let w = (a.w - 9.0) / 4.0;
            for (i, name) in names.iter().enumerate() {
                let r = Rect::new(a.x + i as f32 * (w + 3.0), a.y, w, 28.0);
                let selected = i == 0;
                pv.rr(r, pv.th.radius, if selected { selection } else { fill });
                let color = if selected {
                    Theme::contrast_text(selection)
                } else {
                    Theme::contrast_text(fill)
                };
                let label = pv.t(name).to_owned();
                let st = pv.style(10.0, false, color);
                let tw = pv.fonts.measure_width(&label, &st);
                pv.text(
                    r.x + (r.w - tw) / 2.0,
                    r.y + 7.0,
                    &label,
                    10.0,
                    false,
                    color,
                    Some(r.w - 4.0),
                );
            }
            let mut y = a.y + 28.0 + 18.0;
            y += pv.text(a.x, y, "Media", 18.0, false, white, None) + 8.0;
            let mut x = a.x;
            for label in ["Previous", "Next"] {
                let text = pv.t(label).to_owned();
                let st = pv.style(13.0, false, pv.th.control_text);
                let w = pv.fonts.measure_width(&text, &st) + 28.0;
                pv.rr(Rect::new(x, y, w, 34.0), pv.th.radius, pv.th.control_fill);
                pv.text(
                    x + 14.0,
                    y + 8.0,
                    &text,
                    13.0,
                    false,
                    pv.th.control_text,
                    None,
                );
                x += w + 6.0;
            }
            y += 42.0;
            pv.text(
                a.x,
                y,
                pv.t("Try the tabs · Shift+wheel on the real panel"),
                11.0,
                false,
                dim,
                Some(a.w),
            );
        }
        _ => {
            let fill = parse(&s.control_fill, pv.th.control_fill);
            let border = parse(&s.control_border, pv.th.control_border);
            let selection = parse(&s.selection_fill, pv.th.selection_fill);
            let mut y = a.y;
            y += pv.text(a.x, y, "Controls & outlines", 16.0, false, white, None) + 6.0;
            // Button sample.
            pv.rr(Rect::new(a.x, y, a.w, 34.0), pv.th.radius, fill);
            pv.outline(Rect::new(a.x, y, a.w, 34.0), pv.th.radius, border);
            let text = pv.t("Example button").to_owned();
            let st = pv.style(13.0, false, Theme::contrast_text(fill));
            let tw = pv.fonts.measure_width(&text, &st);
            pv.text(
                a.x + (a.w - tw) / 2.0,
                y + 8.0,
                &text,
                13.0,
                false,
                Theme::contrast_text(fill),
                None,
            );
            y += 34.0 + 8.0;
            // Input sample.
            pv.rr(Rect::new(a.x, y, a.w, 34.0), pv.th.radius, fill);
            pv.outline(Rect::new(a.x, y, a.w, 34.0), pv.th.radius, border);
            pv.text(
                a.x + 10.0,
                y + 8.0,
                pv.t("Example input"),
                13.0,
                false,
                Theme::contrast_text(fill),
                Some(a.w - 20.0),
            );
            y += 34.0 + 8.0;
            // Selected tab.
            pv.rr(Rect::new(a.x, y, a.w, 34.0), pv.th.radius, selection);
            let text = pv.t("Selected tab").to_owned();
            let st = pv.style(13.0, false, Theme::contrast_text(selection));
            let tw = pv.fonts.measure_width(&text, &st);
            pv.text(
                a.x + (a.w - tw) / 2.0,
                y + 8.0,
                &text,
                13.0,
                false,
                Theme::contrast_text(selection),
                None,
            );
            y += 34.0 + 8.0;
            // Switch and check.
            pv.text(
                a.x,
                y + 12.0,
                pv.t("Pause capture"),
                13.0,
                false,
                white,
                None,
            );
            pv.switch(a.right() - 34.0, y + 10.0, true);
            y += 40.0;
            pv.c.fill_circle(a.x + 9.0, y + 15.0, 9.0, &Fill::Solid(pv.th.checked_fill));
            pv.text(
                a.x + 26.0,
                y + 8.0,
                pv.t("Selected file"),
                13.0,
                false,
                white,
                None,
            );
            y += 36.0;
            pv.text(
                a.x,
                y,
                pv.t("Example controls · changes apply to the app"),
                11.0,
                false,
                dim,
                Some(a.w),
            );
        }
    }
}
