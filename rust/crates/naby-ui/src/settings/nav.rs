//! Left navigation, the search box with its results, and "reveal" of a search result
//! (C# `SettingsWindow` navigation buttons, `SettingsSearch.cs`).

use super::categories::{category, category_title, CATEGORIES};
use super::kit::Pal;
use super::search::{search, SearchItem};
use super::SettingsView;
use naby_render::{Icon, Key, Rect, TextEditOpts, Ui, Weight};

/// A search result waiting for its section to be drawn so it can scroll there.
#[derive(Debug, Clone)]
pub(crate) struct Reveal {
    pub item: SearchItem,
    pub tries: u8,
}

/// The accordion each hidden control lives in, so a search result can open it first.
const EXPANDER_OF: &[(&str, &str)] = &[
    ("Light direction", "Fine-tune light"),
    ("Color balance", "Fine-tune light"),
    ("Light reach", "Fine-tune light"),
    ("Ambient softness", "Fine-tune light"),
    ("Ambient spread", "Fine-tune light"),
    ("Panel width", "Panel dimensions and spacing"),
    ("Maximum panel height", "Panel dimensions and spacing"),
    ("Content inset", "Panel dimensions and spacing"),
    ("Content size", "Panel dimensions and spacing"),
    ("Section spacing", "Panel dimensions and spacing"),
    ("Hover delay", "Hover timing"),
    ("Detail delay", "Hover timing"),
    ("Swipe distance", "Hover timing"),
];

/// Maps a page request (`Command::OpenSettingsPage`) to a category id; unknown ids open Home.
pub(crate) fn resolve_page(page: &str) -> &'static str {
    let key = page.trim().to_ascii_lowercase();
    if let Some(c) = CATEGORIES.iter().find(|c| c.id.to_ascii_lowercase() == key) {
        return c.id;
    }
    match key.as_str() {
        "" | "home" | "start" => "Home",
        "widgets" | "tools" | "content" => "Content",
        "voice" | "assistant" | "naby" | "ai" | "chat" => "AI",
        "general" | "app" | "updates" | "about" | "diagnostics" => "Application",
        "look" | "colors" | "theme" => "Appearance",
        "position" | "size" | "dock" => "Layout",
        "music" | "sound" | "lyrics" | "media" => "Audio",
        "shortcuts" | "apps" => "Launch",
        "notices" | "alerts" => "Notifications",
        "hotkeys" | "mouse" | "gestures" => "Controls",
        "tuck" | "hide" => "Visibility",
        "animation" | "speed" => "Motion",
        "clock" => "System",
        _ => "Home",
    }
}

impl SettingsView {
    /// The navigation column: brand, grouped categories.
    pub(super) fn nav_list<'a>(&mut self, ui: &mut Ui<'a>, rect: Rect) {
        let th = ui.theme().clone();
        ui.child(rect, |ui| {
            ui.scroll_area("nav", |ui| {
                ui.spacing(0.0);
                ui.gap(26.0);
                let brand = ui.text_style(20.0, Weight::Semibold, th.text);
                let lh = ui.line_height(&brand);
                let r = ui.allocate(None, lh);
                ui.draw_text(r.x + 26.0, r.y, "NabyScape", &brand, Some(r.w - 30.0));
                ui.gap(28.0);
                let mut last_group = "";
                for c in &CATEGORIES {
                    if c.group != last_group {
                        let top = if c.group == "Personalize" { 0.0 } else { 18.0 };
                        ui.gap(top);
                        let st = ui.text_style(10.0, Weight::Regular, th.text_dim);
                        let h = ui.line_height(&st);
                        let r = ui.allocate(None, h);
                        ui.draw_text(r.x + 26.0, r.y, self.tr(c.group), &st, Some(r.w - 30.0));
                        ui.gap(6.0);
                        last_group = c.group;
                    }
                    let slot = ui.allocate(None, 42.0);
                    let item = Rect::new(
                        slot.x + 14.0,
                        slot.y + 1.0,
                        (slot.w - 14.0 - 6.0).max(0.0),
                        40.0,
                    );
                    let selected = c.id == self.current;
                    ui.push_id(c.id, |ui| {
                        let resp = ui.hit_area(item, true);
                        self.record(&format!("nav:{}", c.id), item);
                        if resp.hovered {
                            self.hand = true;
                        }
                        if ui.visible(item) {
                            let canvas = ui.canvas();
                            if selected {
                                canvas.fill_rrect(
                                    item,
                                    8.0,
                                    naby_core::color::Rgba::parse_hex(&self.s.selection_fill)
                                        .unwrap_or(th.selection_fill),
                                );
                            } else if resp.hovered {
                                canvas.fill_rrect(item, 8.0, th.hover_fill);
                            }
                            let st = ui.text_style(
                                13.0,
                                if selected {
                                    Weight::Semibold
                                } else {
                                    Weight::Regular
                                },
                                if selected { th.text } else { th.text_dim },
                            );
                            let h = ui.line_height(&st);
                            ui.draw_text(
                                item.x + 12.0,
                                item.y + (item.h - h) / 2.0,
                                self.tr(c.title),
                                &st,
                                Some(item.w - 24.0),
                            );
                            ui.paint_focus_ring(&resp, 8.0);
                        }
                        if resp.clicked && !selected {
                            self.navigate(c.id);
                        }
                        let tip = self.tr(c.description).to_owned();
                        ui.tooltip(&resp, &tip);
                    });
                }
                ui.gap(18.0);
            });
        });
    }

    /// The category chooser shown instead of the navigation column in a narrow window.
    pub(super) fn category_combo<'a>(&mut self, ui: &mut Ui<'a>, rect: Rect) {
        ui.child(rect, |ui| {
            let titles: Vec<String> = CATEGORIES
                .iter()
                .map(|c| self.tr(c.title).to_owned())
                .collect();
            let refs: Vec<&str> = titles.iter().map(String::as_str).collect();
            let mut idx = CATEGORIES
                .iter()
                .position(|c| c.id == self.current)
                .unwrap_or(0);
            let resp = ui.combo(&mut idx, &refs);
            self.record("Settings category", resp.rect);
            self.hand |= resp.hovered;
            if resp.changed && CATEGORIES[idx].id != self.current {
                self.navigate(CATEGORIES[idx].id);
            }
        });
    }

    /// The search field with its icon and clear button.
    pub(super) fn search_header<'a>(&mut self, ui: &mut Ui<'a>, rect: Rect, pal: &Pal) {
        let th = ui.theme().clone();
        ui.canvas().fill_rrect(rect, 10.0, pal.search_bg);
        ui.canvas()
            .stroke_round_rect(rect, 10.0, pal.search_border, 1.0);
        let (canvas, _) = ui.canvas_fonts();
        canvas.icon(
            Icon::Search,
            Rect::new(rect.x + 12.0, rect.y + (rect.h - 15.0) / 2.0, 15.0, 15.0),
            th.text_dim,
            1.6,
        );
        let clear_w = if self.search.is_empty() { 0.0 } else { 34.0 };
        let field = Rect::new(rect.x + 36.0, rect.y, rect.w - 36.0 - clear_w - 6.0, rect.h);
        let hint = self.tr("Search settings…  Ctrl+F").to_owned();
        let mut text = std::mem::take(&mut self.search);
        let mut resp = Default::default();
        ui.child(field, |ui| {
            resp = ui.text_edit_with(&mut text, &TextEditOpts::single().hint(&hint).underline());
        });
        self.search = text;
        let resp: naby_render::Response = resp;
        self.search_id = resp.id;
        self.search_focus = resp.focused;
        self.record("Search settings", resp.rect);
        if !self.search.is_empty() {
            let x = Rect::new(
                rect.right() - 34.0,
                rect.y + (rect.h - 28.0) / 2.0,
                28.0,
                28.0,
            );
            ui.child(x, |ui| {
                let r =
                    ui.icon_button_sized(Icon::X, self.tr("Clear settings search"), 28.0, false);
                self.hand |= r.hovered;
                if r.clicked {
                    self.search.clear();
                }
            });
        }
        // Escape clears the query; Down / Enter jump to the first result.
        let input = ui.input().clone();
        if resp.focused {
            for k in &input.keys {
                match k.key {
                    Key::Escape if !self.search.is_empty() => {
                        self.search.clear();
                    }
                    Key::Enter | Key::Down if !self.search.is_empty() => {
                        self.search_enter = Some(k.key == Key::Enter);
                    }
                    _ => {}
                }
            }
        }
        // Ctrl+F focuses the search field from anywhere.
        if input
            .keys
            .iter()
            .any(|k| k.ctrl && !k.alt && k.key == Key::Char('f'))
            && self.search_id != 0
        {
            ui.focus_id(self.search_id);
        }
    }

    /// The result list drawn in place of the page while a query is typed.
    pub(super) fn search_results<'a>(&mut self, ui: &mut Ui<'a>, rect: Rect, pal: &Pal) {
        let th = ui.theme().clone();
        let hits: Vec<SearchItem> = search(&self.search_index, &self.lang, &self.search)
            .into_iter()
            .cloned()
            .collect();
        let row_h = 46.0;
        let inner_h = if hits.is_empty() {
            48.0
        } else {
            hits.len() as f32 * (row_h + 2.0)
        };
        let card_h = (inner_h + 16.0).min(rect.h.clamp(80.0, 430.0));
        let card = Rect::new(rect.x, rect.y, rect.w, card_h);
        ui.canvas().fill_rrect(card, 12.0, pal.results_bg);
        ui.canvas()
            .stroke_round_rect(card, 12.0, pal.results_border, 1.0);
        let inner = card.inset(8.0);
        let mut chosen: Option<SearchItem> = None;
        if self.search_enter.is_some() {
            if let Some(first) = hits.first() {
                chosen = Some(first.clone());
            }
        }
        self.search_enter = None;
        ui.child(inner, |ui| {
            ui.spacing(0.0);
            if hits.is_empty() {
                ui.gap(14.0);
                let st = ui.text_style(12.0, Weight::Regular, th.text_dim);
                let r = ui.allocate(None, ui.line_height(&st) * 2.0);
                let text = self
                    .tr("No settings found. Try color, dock, lyrics, weather or notifications.");
                let lines = ui.fonts().wrap(text, &st, r.w - 24.0);
                for (i, line) in lines.iter().enumerate() {
                    ui.draw_text(
                        r.x + 12.0,
                        r.y + i as f32 * ui.line_height(&st),
                        line,
                        &st,
                        None,
                    );
                }
            }
            for (i, item) in hits.iter().enumerate() {
                let slot = ui.allocate(None, row_h + 2.0);
                let r = Rect::new(slot.x, slot.y + 1.0, slot.w, row_h);
                ui.push_id(i, |ui| {
                    let resp = ui.hit_area(r, true);
                    self.hand |= resp.hovered;
                    if resp.hovered || resp.focused {
                        ui.canvas().fill_rrect(r, 8.0, th.hover_fill);
                    }
                    let title = ui.text_style(12.0, Weight::Regular, th.text);
                    let path = ui.text_style(10.0, Weight::Regular, th.text_dim);
                    let th_h = ui.line_height(&title);
                    let ph = ui.line_height(&path);
                    let top = r.y + (r.h - th_h - 3.0 - ph) / 2.0;
                    ui.draw_text(
                        r.x + 12.0,
                        top,
                        self.tr(&item.title),
                        &title,
                        Some(r.w - 24.0),
                    );
                    let mut crumb = category_title(&self.lang, item.category);
                    if !item.section.is_empty() {
                        crumb.push_str("  /  ");
                        crumb.push_str(self.tr(super::categories::friendly_section(item.section)));
                    }
                    ui.draw_text(
                        r.x + 12.0,
                        top + th_h + 3.0,
                        &crumb,
                        &path,
                        Some(r.w - 24.0),
                    );
                    ui.paint_focus_ring(&resp, 8.0);
                    if resp.clicked {
                        chosen = Some(item.clone());
                    }
                });
            }
        });
        if let Some(item) = chosen {
            self.open_search_result(item);
        }
    }

    /// `OpenSettingsSearchResult`: clear the query, open the category and scroll to the target.
    pub(super) fn open_search_result(&mut self, item: SearchItem) {
        self.search.clear();
        let id = category(item.category).id;
        self.navigate(id);
        if id == "Weather" && item.control.starts_with("Weather city") {
            self.weather_focus = true;
        }
        if let Some((_, key)) = EXPANDER_OF.iter().find(|(c, _)| *c == item.control) {
            self.open.insert((*key).to_owned());
        }
        self.reveal = Some(Reveal { item, tries: 0 });
        self.repaint = true;
    }

    /// Scrolls the page to the pending search result once its section has been drawn.
    pub(super) fn process_reveal<'a>(&mut self, ui: &mut Ui<'a>, viewport_h: f32) {
        let Some(reveal) = self.reveal.clone() else {
            return;
        };
        let control = if reveal.item.control.is_empty() {
            None
        } else {
            self.positions
                .get(&format!("c:{}", reveal.item.control))
                .copied()
        };
        let section = self
            .positions
            .get(&format!("s:{}", reveal.item.section))
            .copied();
        let target = control.or(section);
        match target {
            Some(y) => {
                let y = (y - 12.0).max(0.0);
                let _ = viewport_h;
                ui.memory().set_scroll_offset("page", y);
                if control.is_some() {
                    if let Some(id) = self.control_ids.get(reveal.item.control).copied() {
                        ui.focus_id(id);
                    }
                } else if !reveal.item.section.is_empty() {
                    self.configure_preview_for_section(reveal.item.section);
                }
                self.set_feedback(&format!("Opened · {}", reveal.item.title), false);
                self.reveal = None;
            }
            None => {
                if reveal.tries >= 3 {
                    self.reveal = None;
                } else {
                    self.reveal = Some(Reveal {
                        tries: reveal.tries + 1,
                        ..reveal
                    });
                    self.repaint = true;
                }
            }
        }
    }
}
