//! The first-run setup guide (C# `WelcomeWindow.cs`): three steps (purpose, dock position and
//! widgets, ready). Presets only choose which widgets are on; they never enable microphone
//! listening, clipboard capture or model downloads.
//!
//! `WelcomeView` is platform free like `SettingsView`: the host feeds it logical-pixel input and
//! executes the commands it emits. The result of Finish/Skip is reported in
//! [`WelcomeOutcome::done`].

use naby_core::color::Rgba;
use naby_core::command::Command;
use naby_core::i18n;
use naby_core::settings::welcome::{self, WelcomePreset};
use naby_core::settings::{widget_display_name, Settings};
use naby_core::store::Store;
use naby_render::{Canvas, Fonts, InputState, Rect, Theme, Ui, UiMemory, Weight};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// How the guide ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WelcomeDone {
    /// "Skip setup": marked complete, nothing applied.
    Skipped,
    /// "Finish setup" / "Continue to AI setup" / "Set up AI": choices applied and saved.
    Finished { setup_ai: bool },
}

/// What a frame reports back to the host.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct WelcomeOutcome {
    pub repaint_after: Option<f64>,
    pub clipboard: Option<String>,
    pub wants_text_input: bool,
    pub wants_hand: bool,
    /// Set once; the host closes the window.
    pub done: Option<WelcomeDone>,
}

const PURPOSES: [(WelcomePreset, &str, &str); 4] = [
    (
        WelcomePreset::Music,
        "Music",
        "Playback, volume, clock and weather",
    ),
    (
        WelcomePreset::Productivity,
        "Productivity",
        "Focus, timer, notes, tasks and calendar",
    ),
    (
        WelcomePreset::Ai,
        "AI",
        "Naby voice, clock and notes · connect your provider next",
    ),
    (
        WelcomePreset::Custom,
        "Custom",
        "Recommended NabyScape setup · keep the complete default experience",
    ),
];

fn preset_key(p: WelcomePreset) -> &'static str {
    match p {
        WelcomePreset::Music => "Music",
        WelcomePreset::Productivity => "Productivity",
        WelcomePreset::Ai => "AI",
        WelcomePreset::Custom => "Custom",
    }
}

pub struct WelcomeView {
    settings: Settings,
    store: Option<Arc<Store>>,
    lang: String,
    step: usize,
    purpose: WelcomePreset,
    edge: String,
    edge_changed: bool,
    widgets: HashMap<String, bool>,
    changed: HashSet<String>,
    error: String,
    mem: UiMemory,
    out: Vec<Command>,
    done: Option<WelcomeDone>,
    repaint: bool,
    hand: bool,
    #[cfg(test)]
    rects: HashMap<String, Rect>,
}

impl WelcomeView {
    pub fn new(settings: Settings, store: Option<Arc<Store>>) -> WelcomeView {
        let widgets = settings
            .widgets
            .iter()
            .map(|w| (w.id.clone(), w.enabled))
            .collect();
        WelcomeView {
            lang: i18n::normalize_language(&settings.ui_language).to_owned(),
            edge: settings.dock_edge.clone(),
            settings,
            store,
            step: 0,
            purpose: WelcomePreset::Custom,
            edge_changed: false,
            widgets,
            changed: HashSet::new(),
            error: String::new(),
            mem: UiMemory::new(),
            out: Vec::new(),
            done: None,
            repaint: false,
            hand: false,
            #[cfg(test)]
            rects: HashMap::new(),
        }
    }

    pub fn step(&self) -> usize {
        self.step
    }

    pub fn purpose(&self) -> WelcomePreset {
        self.purpose
    }

    /// The window title.
    pub fn title(&self) -> String {
        self.tr("Welcome to NabyScape").to_owned()
    }

    /// The widget ids currently switched on in the guide, in the user's order.
    pub fn enabled_widgets(&self) -> Vec<String> {
        self.settings
            .widgets
            .iter()
            .filter(|w| self.widgets.get(&w.id).copied().unwrap_or(false))
            .map(|w| w.id.clone())
            .collect()
    }

    fn tr<'k>(&self, key: &'k str) -> &'k str {
        i18n::t(&self.lang, key)
    }

    /// `ChoosePurpose`.
    pub fn choose_purpose(&mut self, purpose: WelcomePreset) {
        self.purpose = purpose;
        match purpose.enabled_widgets() {
            None => {
                for w in &self.settings.widgets {
                    self.widgets.insert(w.id.clone(), w.enabled);
                }
                self.changed.clear();
            }
            Some(enabled) => {
                let ids: Vec<String> = self.widgets.keys().cloned().collect();
                for id in ids {
                    self.widgets
                        .insert(id.clone(), enabled.contains(&id.as_str()));
                    self.changed.insert(id);
                }
            }
        }
        self.repaint = true;
    }

    pub fn choose_edge(&mut self, edge: &str) {
        self.edge = edge.to_owned();
        self.edge_changed = true;
        self.repaint = true;
    }

    pub fn set_widget(&mut self, id: &str, on: bool) {
        self.widgets.insert(id.to_owned(), on);
        self.changed.insert(id.to_owned());
    }

    pub fn next(&mut self) {
        if self.step < 2 {
            self.step += 1;
            self.repaint = true;
        } else {
            self.finish(self.purpose == WelcomePreset::Ai);
        }
    }

    pub fn back(&mut self) {
        self.step = self.step.saturating_sub(1);
        self.repaint = true;
    }

    /// `MarkComplete`.
    fn mark_complete(&mut self) -> bool {
        let Some(store) = &self.store else {
            return true;
        };
        match welcome::complete(store) {
            Ok(()) => true,
            Err(e) => {
                store.log(&e);
                self.error =
                    "Could not save setup. Check your settings folder and try again.".to_owned();
                false
            }
        }
    }

    /// "Skip setup".
    pub fn skip(&mut self) {
        if self.mark_complete() {
            self.done = Some(WelcomeDone::Skipped);
        }
    }

    /// `Finish(setupAi)`: normalizes a detached copy so a failed write cannot change the live
    /// preferences, saves it, then marks the guide complete.
    pub fn finish(&mut self, setup_ai: bool) {
        let mut candidate = self.settings.clone();
        if self.edge_changed {
            candidate.dock_edge = self.edge.clone();
        }
        for w in &mut candidate.widgets {
            if self.changed.contains(&w.id) {
                if let Some(on) = self.widgets.get(&w.id) {
                    w.enabled = *on;
                }
            }
        }
        if self.purpose != WelcomePreset::Custom {
            candidate.hover_follows_widgets = true;
        }
        candidate.normalize();
        if let Some(store) = &self.store {
            if let Err(e) = store.save_settings(&candidate) {
                store.log(&e);
                self.error =
                    "Could not save your choices. Your previous settings are unchanged.".to_owned();
                return;
            }
        }
        self.settings = candidate.clone();
        self.out.push(Command::ApplySettings(Box::new(candidate)));
        if !self.mark_complete() {
            return;
        }
        self.done = Some(WelcomeDone::Finished { setup_ai });
    }

    /// Draws one frame. `canvas` is physical pixels, `input` logical (physical / `scale`).
    pub fn frame(
        &mut self,
        canvas: &mut Canvas,
        fonts: &Fonts,
        input: &InputState,
        scale: f32,
        out: &mut Vec<Command>,
    ) -> WelcomeOutcome {
        let scale = if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        };
        self.lang = i18n::normalize_language(&self.settings.ui_language).to_owned();
        let theme = Theme::from_settings(&self.settings).with_dpi(scale);
        let (cw, ch) = canvas.size();
        let area = Rect::new(0.0, 0.0, cw as f32 / scale, ch as f32 / scale);
        let window = if theme.high_contrast {
            theme.background
        } else {
            Rgba::parse_hex("#111214").unwrap_or(Rgba::BLACK)
        };
        let mut mem = std::mem::take(&mut self.mem);
        for pass in 0..4 {
            let quiet;
            let real = if pass == 0 {
                input
            } else {
                quiet = input.without_events();
                &quiet
            };
            self.repaint = false;
            self.hand = false;
            canvas.clear(window);
            {
                let mut ui = Ui::new(canvas, fonts, &theme, real, &mut mem, area);
                self.layout(&mut ui, area);
            }
            if !(mem.take_repaint() | std::mem::take(&mut self.repaint)) {
                break;
            }
        }
        let outcome = WelcomeOutcome {
            repaint_after: mem.repaint_after(),
            clipboard: mem.take_clipboard(),
            wants_text_input: mem.wants_text_input(),
            wants_hand: self.hand,
            done: self.done,
        };
        self.mem = mem;
        out.append(&mut self.out);
        outcome
    }

    /// Frees the widget state (window closed).
    pub fn release(&mut self) {
        self.mem.clear();
    }

    fn layout(&mut self, ui: &mut Ui, area: Rect) {
        let th = ui.theme().clone();
        let footer_h = 12.0 + 34.0 + 20.0;
        let body = Rect::new(area.x, area.y, area.w, (area.h - footer_h).max(0.0));
        let footer = Rect::new(area.x, body.bottom(), area.w, footer_h);
        ui.child(body, |ui| {
            ui.scroll_area("welcome", |ui| {
                ui.spacing(0.0);
                ui.row(|ui| {
                    ui.spacing(0.0);
                    ui.gap(28.0);
                    ui.column_w((body.w - 28.0 - 28.0 - 14.0).max(120.0), |ui| {
                        ui.spacing(0.0);
                        ui.gap(20.0);
                        self.steps(ui, &th);
                        ui.gap(20.0);
                    });
                });
            });
        });
        // Footer: Skip setup, Back, Next.
        let mut skip = false;
        let mut back = false;
        let mut next = false;
        ui.child(
            Rect::new(footer.x + 24.0, footer.y + 12.0, footer.w - 48.0, 34.0),
            |ui| {
                ui.row(|ui| {
                    ui.spacing(8.0);
                    let r = ui.button(self.tr("Skip setup"));
                    self.hand |= r.hovered;
                    self.record("Skip setup", r.rect);
                    skip = r.clicked;
                    if self.step > 0 {
                        let r = ui.button(self.tr("Back"));
                        self.hand |= r.hovered;
                        self.record("Back", r.rect);
                        back = r.clicked;
                    }
                    let label = if self.step == 2 {
                        if self.purpose == WelcomePreset::Ai {
                            "Continue to AI setup"
                        } else {
                            "Finish setup"
                        }
                    } else {
                        "Next"
                    };
                    let r = ui.primary_button(self.tr(label));
                    self.hand |= r.hovered;
                    self.record("Next", r.rect);
                    next = r.clicked;
                });
            },
        );
        if skip {
            self.skip();
        } else if back {
            self.back();
        } else if next {
            self.next();
        }
    }

    fn text(&self, ui: &mut Ui, text: &str, size: f32, muted: bool) {
        let th = ui.theme().clone();
        let st = ui.text_style(
            size,
            Weight::Regular,
            if muted { th.text_dim } else { th.text },
        );
        ui.label_styled(text, &st);
    }

    fn steps(&mut self, ui: &mut Ui, th: &Theme) {
        let progress = format!(
            "{}  /  {} {} 3",
            self.tr("GET STARTED"),
            self.step + 1,
            self.tr("OF")
        );
        self.text(ui, &progress, 12.0, true);
        ui.gap(8.0);
        let heading = match self.step {
            0 => "What brings you here?",
            1 => "Make it yours.",
            _ => "Your dock is ready.",
        };
        let heading = self.tr(heading).to_owned();
        self.text(ui, &heading, 28.0, false);
        ui.gap(8.0);
        match self.step {
            0 => self.purpose_step(ui, th),
            1 => self.position_step(ui, th),
            _ => self.ready_step(ui, th),
        }
        if !self.error.is_empty() {
            ui.gap(8.0);
            let e = self.tr(&self.error.clone()).to_owned();
            self.text(ui, &e, 12.0, true);
        }
    }

    fn purpose_step(&mut self, ui: &mut Ui, th: &Theme) {
        self.text(
            ui,
            self.tr("Choose a starting point. You can change every choice later."),
            13.0,
            true,
        );
        let mut picked = None;
        for (preset, title, hint) in PURPOSES {
            ui.gap(10.0);
            let selected = self.purpose == preset;
            let name = if preset == WelcomePreset::Custom {
                "Recommended"
            } else {
                title
            };
            let heading = format!(
                "{}{}",
                self.tr(name),
                if selected { " · Selected" } else { "" }
            );
            let hint_text = self.tr(hint).to_owned();
            let hs = ui.text_style(12.0, Weight::Regular, th.text_dim);
            let ts = ui.text_style(16.0, Weight::Regular, th.text);
            let w = ui.available_width() - 32.0;
            let hint_lines = ui.fonts().wrap(&hint_text, &hs, w);
            let h =
                32.0 + ui.line_height(&ts) + 5.0 + hint_lines.len() as f32 * ui.line_height(&hs);
            let rect = ui.allocate(None, h);
            ui.push_id(title, |ui| {
                let resp = ui.hit_area(rect, true);
                self.record(&format!("purpose:{}", preset_key(preset)), rect);
                self.hand |= resp.hovered;
                let fill = if selected {
                    Rgba::parse_hex(&self.settings.selection_fill).unwrap_or(th.selection_fill)
                } else {
                    Rgba::parse_hex(&self.settings.control_fill).unwrap_or(th.control_fill)
                };
                let fg = Theme::contrast_text(fill);
                let c = ui.canvas();
                c.fill_rrect(rect, th.radius, fill);
                if resp.hovered {
                    c.fill_rrect(rect, th.radius, th.hover_fill);
                }
                let ts = ui.text_style(16.0, Weight::Regular, fg);
                ui.draw_text(
                    rect.x + 16.0,
                    rect.y + 16.0,
                    &heading,
                    &ts,
                    Some(rect.w - 32.0),
                );
                let mut y = rect.y + 16.0 + ui.line_height(&ts) + 5.0;
                for line in &hint_lines {
                    ui.draw_text(rect.x + 16.0, y, line, &hs, None);
                    y += ui.line_height(&hs);
                }
                ui.paint_focus_ring(&resp, th.radius);
                if resp.clicked {
                    picked = Some(preset);
                }
            });
        }
        if let Some(p) = picked {
            self.choose_purpose(p);
        }
        ui.gap(10.0);
        self.text(
            ui,
            self.tr("Choose a purpose to change widgets, or keep Recommended to use the complete default setup. Microphone listening starts only when you choose it."),
            12.0,
            true,
        );
    }

    fn position_step(&mut self, ui: &mut Ui, th: &Theme) {
        ui.gap(8.0);
        self.text(ui, self.tr("Dock position"), 16.0, false);
        ui.gap(4.0);
        let mut chosen = None;
        let mut x = 0.0;
        let avail = ui.available_width();
        let mut y_rows: Vec<Vec<&str>> = vec![Vec::new()];
        for side in ["Top", "Bottom", "Left", "Right"] {
            let w =
                28.0 + ui.fonts().measure_width(
                    self.tr(side),
                    &ui.text_style(13.0, Weight::Regular, th.text),
                ) + 16.0;
            if x + w > avail && !y_rows.last().is_some_and(Vec::is_empty) {
                y_rows.push(Vec::new());
                x = 0.0;
            }
            x += w;
            y_rows.last_mut().expect("row").push(side);
        }
        for (ri, row) in y_rows.iter().enumerate() {
            ui.push_id(ri, |ui| {
                ui.gap(6.0);
                ui.row(|ui| {
                    ui.spacing(0.0);
                    for side in row {
                        let label = self.tr(side).to_owned();
                        let st = ui.text_style(13.0, Weight::Regular, th.text);
                        let w = 28.0 + ui.fonts().measure_width(&label, &st) + 16.0;
                        let rect = ui.allocate(Some(w), 34.0);
                        ui.push_id(*side, |ui| {
                            let resp = ui.hit_area(rect, true);
                            self.record(&format!("edge:{side}"), rect);
                            self.hand |= resp.hovered;
                            let (cx, cy) = (rect.x + 14.0, rect.y + rect.h / 2.0);
                            let c = ui.canvas();
                            c.stroke_circle(cx, cy, 8.0, th.control_border, 1.2);
                            if self.edge == *side {
                                c.fill_circle(
                                    cx,
                                    cy,
                                    4.5,
                                    &naby_render::Fill::Solid(th.checked_fill),
                                );
                            }
                            ui.draw_text(
                                rect.x + 28.0,
                                rect.y + (rect.h - ui.line_height(&st)) / 2.0,
                                &label,
                                &st,
                                Some(rect.w - 28.0),
                            );
                            ui.paint_focus_ring(&resp, 8.0);
                            if resp.clicked {
                                chosen = Some(*side);
                            }
                        });
                    }
                });
            });
        }
        if let Some(side) = chosen {
            self.choose_edge(side);
        }
        ui.gap(14.0);
        self.text(ui, self.tr("Your widgets"), 16.0, false);
        let list: Vec<String> = self
            .settings
            .widgets
            .iter()
            .filter(|w| {
                self.purpose == WelcomePreset::Custom
                    || self.widgets.get(&w.id).copied().unwrap_or(false)
            })
            .map(|w| w.id.clone())
            .collect();
        for id in list {
            ui.gap(6.0);
            let name = widget_display_name(&id).unwrap_or(&id).to_owned();
            let label = self.tr(&name).to_owned();
            let mut on = self.widgets.get(&id).copied().unwrap_or(false);
            let before = on;
            ui.push_id(id.as_str(), |ui| {
                let resp = ui.checkbox(&mut on, &label);
                self.record(&format!("widget:{id}"), resp.rect);
                self.hand |= resp.hovered;
            });
            if on != before {
                self.set_widget(&id, on);
            }
        }
        ui.gap(8.0);
        self.text(ui, self.tr("All widgets remain available in Settings. Weather asks for a city; AI asks for a connection."), 12.0, true);
    }

    fn ready_step(&mut self, ui: &mut Ui, th: &Theme) {
        self.text(
            ui,
            self.tr("Click the dock to open your widgets. Hover for a quick look. Right-click the tray icon to open Settings."),
            14.0,
            true,
        );
        ui.gap(10.0);
        let names: Vec<String> = self
            .settings
            .widgets
            .iter()
            .filter(|w| self.widgets.get(&w.id).copied().unwrap_or(false))
            .map(|w| {
                let n = widget_display_name(&w.id).unwrap_or(&w.id);
                self.tr(n).to_owned()
            })
            .collect();
        let line = names.join(" · ");
        self.text(ui, &line, 13.0, false);
        if self.purpose != WelcomePreset::Ai {
            ui.gap(12.0);
            let card = ui.allocate(None, 118.0);
            let c = ui.canvas();
            c.fill_rrect(card, 14.0, th.control_fill);
            c.stroke_round_rect(card, 14.0, th.control_border, 1.0);
            let title = ui.text_style(11.0, Weight::Regular, th.text_dim);
            ui.draw_text(
                card.x + 16.0,
                card.y + 16.0,
                self.tr("AI is optional"),
                &title,
                None,
            );
            let body = ui.text_style(13.0, Weight::Regular, th.text_dim);
            let lines = ui.fonts().wrap(
                self.tr("Connect your own provider or Hermes whenever you're ready. Voice starts only when you choose it."),
                &body,
                card.w - 32.0,
            );
            let lh = ui.line_height(&body);
            for (i, l) in lines.iter().enumerate() {
                ui.draw_text(card.x + 16.0, card.y + 38.0 + i as f32 * lh, l, &body, None);
            }
            let button = Rect::new(card.x + 16.0, card.bottom() - 16.0 - 34.0, 140.0, 34.0);
            let mut go = false;
            ui.child(button, |ui| {
                let r = ui.button(self.tr("Set up AI"));
                self.record("Set up AI", r.rect);
                self.hand |= r.hovered;
                go = r.clicked;
            });
            if go {
                self.finish(true);
            }
            ui.gap(10.0);
        }
        ui.gap(6.0);
        let s = &self.settings;
        let line = format!(
            "{}: {} · {}: {}. {}",
            self.tr("Clipboard history"),
            self.tr(if s.clipboard_history { "On" } else { "Off" }),
            self.tr("Download alerts"),
            self.tr(if s.downloads_enabled { "On" } else { "Off" }),
            self.tr("Change these in Settings at any time.")
        );
        self.text(ui, &line, 12.0, true);
        ui.gap(6.0);
        self.text(
            ui,
            self.tr("You can reopen this guide from Settings > General."),
            12.0,
            true,
        );
    }

    #[cfg(test)]
    fn record(&mut self, key: &str, rect: Rect) {
        self.rects.insert(key.to_owned(), rect);
    }

    #[cfg(not(test))]
    fn record(&mut self, _key: &str, _rect: Rect) {}
}

#[cfg(test)]
mod tests;
