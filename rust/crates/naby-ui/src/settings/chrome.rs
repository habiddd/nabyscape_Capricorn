//! The window frame (C# `SettingsChrome.cs` and the grid built in the `SettingsWindow`
//! constructor): navigation column, search header, page header, scrolling form, preview rail and
//! the save/Undo footer. The native title bar and caption buttons belong to the Windows host.

use super::categories::{category, category_title, has_preview, CATEGORIES};
use super::kit::Pal;
use super::{ModalAction, SettingsView};
use naby_core::color::Rgba;
use naby_core::command::Command;
use naby_core::settings::reset::{self, ResetScope};
use naby_render::{Key, Rect, Ui, Weight};

/// Width of the live-preview rail (`workspace.ColumnDefinitions[1]`).
pub const RAIL_WIDTH: f32 = 284.0;
/// Width of the navigation column.
pub const NAV_WIDTH: f32 = 228.0;

/// Responsive breakpoints of `Responsive()`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Breakpoints {
    pub compact: bool,
    pub narrow: bool,
    pub short: bool,
    pub very_short: bool,
}

impl Breakpoints {
    pub fn of(area: Rect) -> Breakpoints {
        Breakpoints {
            compact: area.w < 760.0,
            narrow: area.w < 1100.0,
            short: area.h < 600.0,
            very_short: area.h < 340.0,
        }
    }
}

/// The title of the Settings window (`Title`).
pub fn window_title(lang: &str) -> String {
    naby_core::i18n::t(lang, "NabyScape · Settings").to_owned()
}

/// Names of the native caption buttons, for hosts that draw their own (`SettingsFrame`).
pub fn caption_button_names() -> [&'static str; 3] {
    ["Minimize Settings", "Maximize Settings", "Close Settings"]
}

impl SettingsView {
    /// Draws the whole window for one pass.
    pub(super) fn layout<'a>(&mut self, ui: &mut Ui<'a>, area: Rect, pal: &Pal) {
        let th = ui.theme().clone();
        let bp = Breakpoints::of(area);
        self.hand = false;
        let nav_w = if bp.compact { 0.0 } else { NAV_WIDTH };
        let mut top = 0.0;
        if !bp.compact {
            let nav = Rect::new(0.0, 0.0, NAV_WIDTH, area.h);
            ui.canvas().fill_rect(nav, pal.nav);
            ui.canvas()
                .fill_rect(Rect::new(NAV_WIDTH - 1.0, 0.0, 1.0, area.h), pal.nav_border);
            self.nav_list(ui, nav);
        } else {
            let m = if bp.very_short { 3.0 } else { 7.0 };
            let r = Rect::new(16.0, m, area.w - 32.0, 34.0);
            self.category_combo(ui, r);
            top = r.bottom() + m;
        }
        let main = Rect::new(nav_w, top, area.w - nav_w, area.h - top);

        // ---- footer ---------------------------------------------------------------------
        let (pl, pt, pr, pb) = if bp.very_short {
            (20.0, 4.0, 20.0, 6.0)
        } else {
            (26.0, 12.0, 26.0, 14.0)
        };
        let footer_h = pt + 34.0 + pb;
        let footer = Rect::new(main.x, main.bottom() - footer_h, main.w, footer_h);
        ui.canvas().fill_rect(footer, pal.footer);
        ui.canvas()
            .fill_rect(Rect::new(footer.x, footer.y, footer.w, 1.0), pal.nav_border);
        let footer_body = Rect::new(footer.x + pl, footer.y + pt, footer.w - pl - pr, 34.0);

        // ---- search ---------------------------------------------------------------------
        let (sl, st, sr, sb) = if bp.very_short {
            (20.0, 4.0, 20.0, 4.0)
        } else {
            (20.0, 13.0, 20.0, 9.0)
        };
        let search_rect = Rect::new(main.x + sl, main.y + st, (main.w - sl - sr).max(0.0), 34.0);
        let ws_top = search_rect.bottom() + sb;
        let workspace = Rect::new(main.x, ws_top, main.w, (footer.y - ws_top).max(0.0));

        // ---- the page or the search results ----------------------------------------------
        let searching = !self.search.trim().is_empty();
        let preview_on = has_preview(&self.current);
        let rail_w = if bp.narrow || !preview_on {
            0.0
        } else {
            RAIL_WIDTH
        };
        if searching {
            let card = Rect::new(
                workspace.x + 20.0,
                workspace.y,
                (workspace.w - 40.0).max(0.0),
                workspace.h,
            );
            self.search_results(ui, card, pal);
        } else {
            self.page_area(ui, workspace, rail_w, bp, preview_on, pal);
        }
        // The search field is drawn last so its results card sits below it.
        self.search_header(ui, search_rect, pal);

        // ---- preview rail ---------------------------------------------------------------
        if rail_w > 0.0 && !searching {
            let rail = Rect::new(workspace.right() - rail_w, workspace.y, rail_w, workspace.h);
            ui.canvas()
                .fill_rect(Rect::new(rail.x, rail.y, 1.0, rail.h), pal.nav_border);
            self.preview_rail(ui, rail, pal);
        }

        self.footer_ui(ui, footer_body, &th);
    }

    fn page_area<'a>(
        &mut self,
        ui: &mut Ui<'a>,
        workspace: Rect,
        rail_w: f32,
        bp: Breakpoints,
        preview_on: bool,
        pal: &Pal,
    ) {
        let th = ui.theme().clone();
        // Page header: heading and description.
        let hx = workspace.x + if bp.narrow { 20.0 } else { 30.0 };
        let mut y = workspace.y + if bp.short { 3.0 } else { 18.0 };
        let hw = (workspace.w - rail_w - if bp.narrow { 40.0 } else { 60.0 }).max(40.0);
        let title_size = if bp.short { 18.0 } else { 26.0 };
        let title_style = ui.text_style(title_size, Weight::Semibold, th.text);
        let title = category_title(&self.lang, &self.current);
        let title_h = ui.line_height(&title_style);
        ui.draw_text(hx, y, &title, &title_style, Some(hw));
        y += title_h;
        if !bp.short {
            y += 6.0;
            let desc_style = ui.text_style(13.0, Weight::Regular, th.text_dim);
            let desc = self.tr(category(&self.current).description).to_owned();
            let lh = ui.line_height(&desc_style);
            for line in ui.fonts().wrap(&desc, &desc_style, hw) {
                ui.draw_text(hx, y, &line, &desc_style, None);
                y += lh;
            }
        }
        y += if bp.short { 3.0 } else { 16.0 };
        let mut form = Rect::new(
            workspace.x,
            y,
            (workspace.w - rail_w).max(0.0),
            (workspace.bottom() - y).max(0.0),
        );

        // A collapsible preview strip in narrow windows.
        if bp.narrow && preview_on && form.h > 120.0 {
            let used = self.preview_strip(
                ui,
                Rect::new(form.x + 20.0, form.y, form.w - 40.0, form.h),
                pal,
            );
            form.y += used;
            form.h -= used;
        }

        let left = if bp.narrow { 20.0 } else { 30.0 };
        let avail = workspace.w - rail_w - 80.0;
        let page_w = avail
            .clamp(260.0, 900.0)
            .min((form.w - left - 22.0).max(120.0));
        let viewport_h = form.h;
        ui.child(form, |ui| {
            ui.scroll_area("page", |ui| {
                ui.spacing(0.0);
                self.content_origin = ui.cursor_pos().1;
                ui.row(|ui| {
                    ui.spacing(0.0);
                    ui.gap(left);
                    ui.column_w(page_w, |ui| {
                        ui.spacing(0.0);
                        ui.gap(8.0);
                        self.page_body(ui);
                        ui.gap(32.0);
                    });
                });
            });
        });
        self.process_reveal(ui, viewport_h);
    }

    /// The page for the current category (`Navigate`'s switch).
    fn page_body<'a>(&mut self, ui: &mut Ui<'a>) {
        match self.root().to_owned().as_str() {
            "Home" => self.settings_home(ui),
            "Weather" => self.weather_settings_page(ui),
            "Appearance" => self.appearance(ui),
            "Layout" => self.layout_settings(ui),
            "Content" => self.content_settings(ui),
            "Launch" => self.launch_settings(ui),
            "Controls" => self.interactions(ui),
            "Audio" => self.audio_settings(ui),
            "System" => self.system_settings(ui),
            "AI" => self.ai_settings(ui),
            _ => self.general(ui),
        }
        // "Reset this category" ends every page that owns preferences.
        let scope = naby_core::settings::reset::ResetCategory::from_name(&self.current);
        if let Some(c) = scope {
            if !c.keys().is_empty() && self.button(ui, "Reset this category") {
                self.ask_reset(ResetScope::Category(c));
            }
        }
    }

    /// Status line, Retry and Undo (`saveRow`).
    fn footer_ui<'a>(&mut self, ui: &mut Ui<'a>, body: Rect, th: &naby_render::Theme) {
        let undo_label = self.tr("Undo").to_owned();
        let retry_label = self.tr("Retry save").to_owned();
        let st = ui.text_style(13.0, Weight::Regular, th.text);
        let undo_w = if self.undo.available() {
            ui.fonts().measure_width(&undo_label, &st) + 28.0 + 8.0
        } else {
            0.0
        };
        let retry_w = if self.retry {
            ui.fonts().measure_width(&retry_label, &st) + 28.0 + 8.0
        } else {
            0.0
        };
        let buttons_w = undo_w + retry_w;
        // Status text.
        let color = if th.high_contrast {
            th.text
        } else if self.feedback_error {
            Rgba::parse_hex("#F0B8B2").unwrap_or(th.text)
        } else {
            Rgba::parse_hex("#F2F3F5").unwrap_or(th.text)
        };
        let status_style = ui.text_style(11.0, Weight::Regular, color);
        let text = self.tr(&self.feedback.clone()).to_owned();
        let tw = (body.w - buttons_w - 8.0).max(40.0);
        let lines = ui.fonts().wrap(&text, &status_style, tw);
        let lh = ui.line_height(&status_style);
        let shown = lines.len().min(2);
        let top = body.y + (body.h - shown as f32 * lh) / 2.0;
        for (i, line) in lines.iter().take(shown).enumerate() {
            let line = if i + 1 == shown && lines.len() > shown {
                format!("{}…", line.trim_end())
            } else {
                line.clone()
            };
            ui.draw_text(body.x, top + i as f32 * lh, &line, &status_style, Some(tw));
        }
        // Buttons, right aligned.
        if buttons_w > 0.0 {
            let r = Rect::new(body.right() - buttons_w, body.y, buttons_w, body.h);
            let mut do_retry = false;
            let mut do_undo = false;
            ui.child(r, |ui| {
                ui.row_centered(34.0, |ui| {
                    ui.spacing(8.0);
                    if self.retry {
                        let resp = ui.button(&retry_label);
                        self.hand |= resp.hovered;
                        self.record("Retry save", resp.rect);
                        do_retry = resp.clicked;
                    }
                    if self.undo.available() {
                        let resp = ui.button(&undo_label);
                        self.hand |= resp.hovered;
                        self.record("Undo", resp.rect);
                        let tip = self.tr("Undo the last saved appearance, layout or widget change. Saved content, connections and background permissions are preserved.").to_owned();
                        ui.tooltip(&resp, &tip);
                        do_undo = resp.clicked;
                    }
                });
            });
            if do_retry {
                self.retry_save();
            }
            if do_undo {
                self.undo_last();
            }
        }
        // Ctrl+Z undoes the last batch when no text field is being edited.
        let input = ui.input().clone();
        if !self.mem_wants_text()
            && input
                .keys
                .iter()
                .any(|k| k.ctrl && !k.shift && k.key == Key::Char('z'))
            && self.undo.available()
        {
            self.undo_last();
        }
    }

    fn mem_wants_text(&self) -> bool {
        self.mem.wants_text_input() || self.search_focus
    }

    /// The confirmation dialog drawn above everything (C# `MessageBox`).
    pub(super) fn modal_ui<'a>(&mut self, ui: &mut Ui<'a>, area: Rect, pal: &Pal) {
        let Some(modal) = self.modal.clone() else {
            return;
        };
        let th = ui.theme().clone();
        ui.canvas().fill_rect(area, Rgba::BLACK.with_alpha(150));
        let w = (area.w - 40.0).clamp(240.0, 460.0);
        let body_style = ui.text_style(12.0, Weight::Regular, th.text);
        let title_style = ui.text_style(16.0, Weight::Semibold, th.text);
        let body_text = modal.body.clone();
        let lines = ui.fonts().wrap(&body_text, &body_style, w - 40.0);
        let lh = ui.line_height(&body_style);
        let h = 20.0
            + ui.line_height(&title_style)
            + 12.0
            + lines.len() as f32 * lh
            + 20.0
            + 34.0
            + 20.0;
        let card = Rect::new(
            area.x + (area.w - w) / 2.0,
            area.y + ((area.h - h) / 2.0).max(8.0),
            w,
            h,
        );
        ui.canvas().fill_rrect(card, 12.0, pal.section_bg);
        ui.canvas()
            .stroke_round_rect(card, 12.0, pal.section_border, 1.0);
        let mut y = card.y + 20.0;
        ui.draw_text(
            card.x + 20.0,
            y,
            self.tr(&modal.title),
            &title_style,
            Some(w - 40.0),
        );
        y += ui.line_height(&title_style) + 12.0;
        for line in &lines {
            ui.draw_text(card.x + 20.0, y, line, &body_style, None);
            y += lh;
        }
        y += 20.0;
        let buttons = Rect::new(card.x + 20.0, y, w - 40.0, 34.0);
        let mut confirmed = false;
        let mut cancelled = false;
        ui.child(buttons, |ui| {
            ui.row(|ui| {
                ui.spacing(8.0);
                let ok = ui.primary_button(self.tr(&modal.confirm));
                self.record("Modal OK", ok.rect);
                confirmed = ok.clicked;
                let cancel = ui.button(self.tr("Cancel"));
                self.record("Modal Cancel", cancel.rect);
                cancelled = cancel.clicked;
            });
        });
        if ui.input().keys.iter().any(|k| k.key == Key::Escape) {
            cancelled = true;
        }
        if confirmed {
            self.modal = None;
            self.run_modal(modal.action);
        } else if cancelled {
            self.modal = None;
        }
        self.repaint = true;
    }

    fn run_modal(&mut self, action: ModalAction) {
        match action {
            ModalAction::Reset(scope) => self.reset_preferences(scope),
            ModalAction::Restore => self.restore_preferences(),
            ModalAction::RestorePreviousVersion => {
                self.out.push(Command::Settings(
                    naby_core::command::SettingsRequest::Update(
                        naby_core::command::UpdateAction::RestorePrevious,
                    ),
                ));
            }
        }
    }

    /// Called by tests and the host to answer a modal without drawing.
    #[allow(dead_code)]
    pub(crate) fn summary_for(scope: ResetScope) -> String {
        reset::summary(scope)
    }
}

#[allow(dead_code)]
fn ids() -> usize {
    CATEGORIES.len()
}
