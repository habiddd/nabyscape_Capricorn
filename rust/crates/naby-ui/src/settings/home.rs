//! The "Start here" page (C# `SettingsHome.cs`).

use super::SettingsView;
use naby_render::{Icon, Rect, Ui, Weight};

/// (title, detail, icon, target category)
const LINKS: [(&str, &str, Icon, &str); 5] = [
    (
        "Make the dock yours",
        "Change its shape, colors and appearance.",
        Icon::Settings,
        "Appearance",
    ),
    (
        "Move or resize the dock",
        "Choose a screen, position and comfortable size.",
        Icon::Screen,
        "Layout",
    ),
    (
        "Choose your widgets",
        "Keep what you use, then drag to arrange it.",
        Icon::NavLaunch,
        "Content",
    ),
    (
        "Set up Naby AI",
        "Chat, talk, or connect your own Hermes agent.",
        Icon::Chat,
        "AI",
    ),
    (
        "Manage the app",
        "Startup, updates, language and saved preferences.",
        Icon::Folder,
        "Application",
    ),
];

impl SettingsView {
    pub(super) fn settings_home<'a>(&mut self, ui: &mut Ui<'a>) {
        self.home_feature_statuses(ui);
        let th = ui.theme().clone();
        let pal = self.pal(ui);
        ui.gap(6.0);
        let intro = ui.text_style(20.0, Weight::Semibold, th.text);
        let text = self.tr("What would you like to change?").to_owned();
        ui.label_styled(&text, &intro);
        ui.gap(20.0);
        for (i, (title, detail, icon, target)) in LINKS.iter().enumerate() {
            let title_style = ui.text_style(15.0, Weight::Regular, th.text);
            let hint_style = ui.text_style(12.0, Weight::Regular, th.text_dim);
            let title_text = self.tr(title).to_owned();
            let detail_text = self.tr(detail).to_owned();
            let text_w = (ui.available_width() - 36.0 - 22.0 - 16.0 - 14.0 - 8.0).max(60.0);
            let tl = ui.line_height(&title_style);
            let hl = ui.line_height(&hint_style);
            let detail_lines = ui.fonts().wrap(&detail_text, &hint_style, text_w);
            let h = 32.0 + tl + 5.0 + detail_lines.len() as f32 * hl;
            let rect = ui.allocate(None, h);
            ui.push_id(i, |ui| {
                let resp = ui.hit_area(rect, true);
                self.record(&format!("home:{target}"), rect);
                self.hand |= resp.hovered;
                if ui.visible(rect) {
                    let c = ui.canvas();
                    c.fill_rrect(rect, th.radius, pal.link_bg);
                    if resp.hovered {
                        c.fill_rrect(rect, th.radius, th.hover_fill);
                    }
                    if pal.hc {
                        c.stroke_round_rect(rect, th.radius, th.text, 1.0);
                    }
                    c.icon(
                        *icon,
                        Rect::new(rect.x + 18.0, rect.y + (rect.h - 22.0) / 2.0, 22.0, 22.0),
                        th.text_dim,
                        1.6,
                    );
                    c.icon(
                        Icon::Next,
                        Rect::new(
                            rect.right() - 18.0 - 14.0,
                            rect.y + (rect.h - 14.0) / 2.0,
                            14.0,
                            14.0,
                        ),
                        th.text_dim,
                        1.6,
                    );
                    let x = rect.x + 18.0 + 22.0 + 16.0;
                    ui.draw_text(x, rect.y + 16.0, &title_text, &title_style, Some(text_w));
                    let mut y = rect.y + 16.0 + tl + 5.0;
                    for line in &detail_lines {
                        ui.draw_text(x, y, line, &hint_style, None);
                        y += hl;
                    }
                    ui.paint_focus_ring(&resp, th.radius);
                }
                if resp.clicked {
                    self.navigate(target);
                }
            });
            ui.gap(10.0);
        }
        ui.gap(16.0);
        self.muted(
            ui,
            "Looking for something specific? Search above, or choose a category on the left. Most changes save automatically. AI connections have their own Save button.",
            12.0,
        );
    }
}
