//! The Weather category (C# `WeatherSettingsPage`, the settings mode of `WeatherView`): pick
//! the city, the temperature unit and see what is saved. The geocoding lookup runs on a worker
//! thread; its answer is polled while the page is open.

use super::SettingsView;
use naby_core::command::Command;
use naby_core::weather::{self, WeatherPlace, WeatherService};
use naby_render::{Rect, Ui, Weight};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::Arc;

/// A blocking GET used for the lookup (`url -> body`).
pub type WeatherFetch = Arc<dyn Fn(&str) -> Result<String, String> + Send + Sync>;

/// The state of the city search.
#[derive(Default)]
pub(crate) struct WeatherSearch {
    pub results: Vec<WeatherPlace>,
    pub status: String,
    pending: Option<Receiver<Result<Vec<WeatherPlace>, String>>>,
}

impl SettingsView {
    /// Replaces the HTTP fetch used by the city lookup (tests inject canned answers).
    pub fn set_weather_fetch(&mut self, fetch: WeatherFetch) {
        self.weather_fetch = fetch;
    }

    /// Starts a lookup for `query` on a worker thread.
    pub(crate) fn start_weather_search(&mut self, query: &str) {
        let query = query.trim().to_owned();
        if query.chars().count() < 2 {
            self.weather.status = "Enter at least two letters.".to_owned();
            self.weather.results.clear();
            return;
        }
        let (tx, rx) = channel();
        let fetch = self.weather_fetch.clone();
        self.weather.status = "Searching…".to_owned();
        self.weather.pending = Some(rx);
        std::thread::spawn(move || {
            let result =
                WeatherService::search(&query, &|url| fetch(url)).map_err(|e| e.to_string());
            let _ = tx.send(result);
        });
    }

    fn poll_weather_search(&mut self) {
        let Some(rx) = &self.weather.pending else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(places)) => {
                self.weather.status = if places.is_empty() {
                    "No matching city found.".to_owned()
                } else {
                    String::new()
                };
                self.weather.results = places;
                self.weather.pending = None;
                self.repaint = true;
            }
            Ok(Err(message)) => {
                self.weather.status = if message.is_empty() {
                    "City search is unavailable.".to_owned()
                } else {
                    message
                };
                self.weather.results.clear();
                self.weather.pending = None;
                self.repaint = true;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.weather.pending = None;
            }
        }
    }

    pub(super) fn weather_settings_page<'a>(&mut self, ui: &mut Ui<'a>) {
        self.poll_weather_search();
        if self.weather.pending.is_some() {
            ui.request_repaint_after(0.1);
        }
        let th = ui.theme().clone();
        let pal = self.pal(ui);
        let prev = self.section_h.get("weather-card").copied().unwrap_or(0.0);
        let outer = ui.allocate(None, prev);
        self.register("s:Weather", outer.y);
        if prev > 0.0 {
            let c = ui.canvas();
            c.fill_rrect(
                outer,
                14.0,
                if pal.hc {
                    pal.window
                } else {
                    naby_core::color::Rgba::rgb(0x16, 0x19, 0x1F)
                },
            );
            c.stroke_round_rect(
                outer,
                14.0,
                if pal.hc {
                    th.text
                } else {
                    naby_core::color::Rgba::rgb(0x2A, 0x2F, 0x39)
                },
                1.0,
            );
        }
        let inner = Rect::new(
            outer.x + 16.0,
            outer.y + 16.0,
            (outer.w - 32.0).max(0.0),
            100_000.0,
        );
        let mut end_y = inner.y;
        ui.child(inner, |ui| {
            ui.spacing(0.0);
            let city = this_city(&self.s);
            let heading = if city.is_empty() {
                self.tr("Choose your city").to_owned()
            } else {
                city
            };
            self.text_final(ui, &heading, 18.0, Weight::Semibold);
            ui.gap(6.0);
            // What is saved.
            let summary = match self.snaps.as_ref().and_then(|s| s.weather.snapshot.clone()) {
                Some(w) if w.key == weather::key_for(&self.s) => format!(
                    "{} · {}",
                    weather::degrees(w.temperature),
                    self.tr(weather::condition(w.code))
                ),
                _ if weather::key_for(&self.s).is_empty() => {
                    self.tr("Choose your city to see the forecast.").to_owned()
                }
                _ => self
                    .tr("Your city is selected. Open Weather to load its forecast.")
                    .to_owned(),
            };
            self.muted_final(ui, &summary, 12.0);
            ui.gap(14.0);
            // City search.
            self.muted(ui, "City", 12.0);
            ui.gap(4.0);
            let mut query = std::mem::take(&mut self.weather_query);
            let hint = self.tr("Search for a city").to_owned();
            let mut resp = naby_render::Response::default();
            ui.row(|ui| {
                ui.spacing(8.0);
                let w = (ui.available_width() - 110.0).max(80.0);
                ui.width(w, |ui| {
                    resp = ui.text_edit_with(
                        &mut query,
                        &naby_render::TextEditOpts::single().hint(&hint).max_len(120),
                    );
                });
            });
            self.record("Weather city search", resp.rect);
            self.wire(&resp, "Weather city search");
            self.control_ids
                .insert("Weather city search".to_owned(), resp.id);
            if self.weather_focus {
                ui.focus_id(resp.id);
                self.weather_focus = false;
            }
            self.weather_query = query.clone();
            let search_label = self.tr("Search").to_owned();
            let mut go = resp.submitted;
            let btn = Rect::new(resp.rect.right() + 8.0, resp.rect.y, 100.0, resp.rect.h);
            ui.child(btn, |ui| {
                let r = ui.primary_button(&search_label);
                self.hand |= r.hovered;
                self.record("Search city", r.rect);
                go |= r.clicked;
            });
            if go {
                self.start_weather_search(&query);
            }
            ui.gap(8.0);
            if !self.weather.status.is_empty() {
                let text = self.tr(&self.weather.status.clone()).to_owned();
                self.muted_final(ui, &text, 11.0);
                ui.gap(6.0);
            }
            let places = self.weather.results.clone();
            let mut chosen: Option<WeatherPlace> = None;
            for (i, place) in places.iter().enumerate() {
                let row = ui.allocate(None, 36.0);
                ui.push_id(i, |ui| {
                    let r = ui.hit_area(row, true);
                    self.hand |= r.hovered;
                    self.record(&format!("city:{i}"), row);
                    if r.hovered || r.focused {
                        ui.canvas().fill_rrect(row, 8.0, th.hover_fill);
                    }
                    let st = ui.text_style(12.0, Weight::Regular, th.text);
                    let lh = ui.line_height(&st);
                    ui.draw_text(
                        row.x + 12.0,
                        row.y + (row.h - lh) / 2.0,
                        &place.name,
                        &st,
                        Some(row.w - 24.0),
                    );
                    ui.paint_focus_ring(&r, 8.0);
                    if r.clicked {
                        chosen = Some(place.clone());
                    }
                });
            }
            if let Some(place) = chosen {
                self.s.weather_city = place.name.clone();
                self.s.weather_latitude = Some(place.latitude);
                self.s.weather_longitude = Some(place.longitude);
                self.weather.results.clear();
                self.weather.status = String::new();
                self.save();
                self.out.push(Command::WeatherRefresh { force: true });
            }
            ui.gap(10.0);
            self.muted(ui, "Temperature units", 12.0);
            let units = self.s.weather_units.clone();
            self.combo_plain(
                ui,
                "Weather temperature units",
                &[
                    ("Celsius (°C)", "Celsius"),
                    ("Fahrenheit (°F)", "Fahrenheit"),
                ],
                &units,
                |s, v| s.weather_units = v.to_owned(),
            );
            if self.button(ui, "Refresh weather") {
                self.out.push(Command::WeatherRefresh { force: true });
            }
            end_y = ui.cursor_pos().1;
        });
        let h = end_y - outer.y + 16.0;
        if (h - prev).abs() > 0.5 {
            self.section_h.insert("weather-card".to_owned(), h);
            self.repaint = true;
        }
        ui.gap(16.0);
    }
}

fn this_city(s: &naby_core::settings::Settings) -> String {
    if s.weather_latitude.is_some() && s.weather_longitude.is_some() {
        s.weather_city.clone()
    } else {
        String::new()
    }
}
