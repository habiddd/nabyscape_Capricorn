//! The `weather` page (`WeatherView.cs`) and its hover glance (`WeatherGlance`): a header with a
//! city line and the location editor (search, units, results), the sky card with the drawn
//! artwork and readings, the hourly forecast, and the refresh / status footer.

pub mod art;

use super::common::{Ctl, EditOpts, Interaction, Presence};
use super::{Page, PageCtx};
use crate::host::{Demand, PlaceSearchState, WeatherStatus};
use chrono::Local;
use naby_core::color::Rgba;
use naby_core::command::Command;
use naby_core::motion::MotionConfig;
use naby_core::system::format_decimal;
use naby_core::weather as wx;
use naby_render::{Canvas, Icon, InputState, Rect, Weight};

const GEAR: u32 = 1;
const SEARCH: u32 = 2;
const FIELD: u32 = 3;
const UNITS: u32 = 4;
const UNIT_ITEM: u32 = 5;
const REFRESH: u32 = 8;
const CREDIT: u32 = 9;
const PLACE: u32 = 20;

const MUTED_BLUE: Rgba = Rgba::rgb(0xB6, 0xC7, 0xD9);
const RANGE: Rgba = Rgba::rgb(0xB7, 0xCA, 0xDC);
const METRIC_LABEL: Rgba = Rgba::rgb(0x97, 0xAD, 0xC2);
const METRIC_VALUE: Rgba = Rgba::rgb(0xDF, 0xE8, 0xEF);
const HOURLY_FILL: Rgba = Rgba::rgb(0x11, 0x18, 0x20);
const HOURLY_BORDER: Rgba = Rgba::rgb(0x27, 0x34, 0x40);
const FIELD_HOVER: Rgba = Rgba::rgb(0x51, 0x56, 0x60);
const FIELD_FOCUS: Rgba = Rgba::rgb(0x85, 0x8C, 0x98);

pub fn build() -> Box<dyn Page> {
    Box::new(WeatherPage::default())
}

/// Everything drawn from the forecast, formatted once per snapshot.
#[derive(Debug, Clone, PartialEq, Default)]
struct Model {
    day: bool,
    code: i32,
    today: &'static str,
    temperature: String,
    condition: &'static str,
    range: String,
    metrics: [(&'static str, String); 3],
    hours: Vec<Hour>,
}

#[derive(Debug, Clone, PartialEq)]
struct Hour {
    time: String,
    temperature: String,
    code: i32,
    day: bool,
}

fn whole(v: f64) -> String {
    format_decimal(v, 0)
}

fn model_of(data: &wx::WeatherSnapshot) -> Model {
    Model {
        day: data.day,
        code: data.code,
        today: if data.day {
            "TODAY \u{b7} DAYLIGHT"
        } else {
            "TONIGHT"
        },
        temperature: wx::degrees(data.temperature),
        condition: wx::condition(data.code),
        range: format!(
            "Feels {}  \u{b7}  H {}  L {}",
            wx::degrees(data.feels_like),
            wx::degrees(data.high),
            wx::degrees(data.low)
        ),
        metrics: [
            ("Rain", format!("{}%", whole(data.rain_chance))),
            ("Humidity", format!("{}%", whole(data.humidity))),
            ("Wind", format!("{} km/h", whole(data.wind))),
        ],
        hours: data
            .hours
            .iter()
            .map(|h| Hour {
                time: h.time.clone(),
                temperature: wx::degrees(h.temperature),
                code: h.code,
                day: h.day,
            })
            .collect(),
    }
}

#[derive(Default)]
struct WeatherPage {
    ui: Interaction,
    presence: Presence,
    st: Body,
}

#[derive(Default)]
struct Body {
    /// The location editor is open.
    locate: Option<bool>,
    search: String,
    units_open: bool,
    searched: bool,
    local_status: Option<String>,
    model_version: u64,
    model: Option<Model>,
    animate: bool,
}

impl Body {
    fn sync(&mut self, ctx: &PageCtx) {
        let key_empty = wx::key_for(ctx.settings()).is_empty();
        if self.locate.is_none() {
            self.locate = Some(key_empty);
        }
        let version = ctx.snaps.weather.version();
        if version != self.model_version
            || (self.model.is_none() && ctx.snaps.weather.snapshot.is_some())
        {
            self.model_version = version;
            self.model = ctx.snaps.weather.snapshot.as_ref().map(model_of);
        }
        let motion = MotionConfig::from_settings(ctx.settings(), true);
        self.animate = self.model.is_some() && motion.enabled && !ctx.settings().reduce_motion;
    }

    fn status(&self, ctx: &PageCtx) -> String {
        if let Some(text) = &self.local_status {
            return text.clone();
        }
        if self.searched {
            match &ctx.snaps.places.state {
                PlaceSearchState::Searching => return "Looking for cities\u{2026}".to_owned(),
                PlaceSearchState::Found(places) if places.is_empty() => {
                    return "No cities found. Try a nearby city or include the country.".to_owned()
                }
                PlaceSearchState::Found(_) => return "Choose your city above.".to_owned(),
                PlaceSearchState::Failed => {
                    return "City search is unavailable. Please try again.".to_owned()
                }
                PlaceSearchState::Idle => {}
            }
        }
        if wx::key_for(ctx.settings()).is_empty() {
            return "Choose a city to see today's weather.".to_owned();
        }
        let weather = &ctx.snaps.weather;
        match &weather.status {
            WeatherStatus::Loading => "Updating weather\u{2026}".to_owned(),
            WeatherStatus::Failed(_) => match weather.snapshot {
                Some(_) => "Saved outlook \u{b7} connection unavailable".to_owned(),
                None => "Weather unavailable. Refresh to try again.".to_owned(),
            },
            WeatherStatus::Idle => match &weather.snapshot {
                Some(s) => format!(
                    "Updated {}",
                    s.updated.with_timezone(&Local).format("%H:%M")
                ),
                None => String::new(),
            },
        }
    }

    fn start_search(&mut self, c: &mut Ctl) {
        let query = self.search.trim().to_owned();
        if query.chars().count() < 2 {
            self.local_status = Some("Enter at least two letters of your city.".to_owned());
            return;
        }
        self.local_status = None;
        self.searched = true;
        c.push(Command::WeatherSearch { query });
    }

    fn choose_place(&mut self, c: &mut Ctl, name: &str, latitude: f64, longitude: f64) {
        let mut next = c.ctx.settings().clone();
        next.weather_city = name.to_owned();
        next.weather_latitude = Some(latitude);
        next.weather_longitude = Some(longitude);
        c.push(Command::ApplySettings(Box::new(next)));
        self.locate = Some(false);
        self.searched = false;
        self.local_status = None;
    }

    fn layout(&mut self, c: &mut Ctl, rect: Rect) -> f32 {
        let th = c.theme().clone();
        let s = c.s;
        let settings = c.ctx.settings().clone();
        let key_empty = wx::key_for(&settings).is_empty();
        let weather = &c.ctx.snaps.weather;
        let busy = matches!(weather.status, WeatherStatus::Loading);
        let mut y = rect.y;

        // ---- header: title, city, settings button -------------------------------------------
        let hw = if th.bold_headings {
            Weight::Semibold
        } else {
            Weight::Regular
        };
        let title_h = c.line_h(16.0, hw) + 1.0 * s + c.line_h(11.0, Weight::Regular);
        let header_h = title_h.max(28.0 * s);
        let city = if settings.weather_city.is_empty() {
            "Your daily outlook".to_owned()
        } else {
            settings.weather_city.clone()
        };
        let gear = Rect::new(
            rect.right() - 28.0 * s,
            y + (header_h - 28.0 * s) / 2.0,
            28.0 * s,
            28.0 * s,
        );
        let text_w = (gear.x - rect.x - 8.0 * s).max(1.0);
        let ty = y + (header_h - title_h) / 2.0;
        c.text(rect.x, ty, "Weather", 16.0, hw, th.text, Some(text_w));
        c.text(
            rect.x,
            ty + c.line_h(16.0, hw) + 1.0 * s,
            &city,
            11.0,
            Weight::Regular,
            th.text_dim,
            Some(text_w),
        );
        let g = c.icon_button(GEAR, gear, Icon::Settings, 20.0, None, Rgba::WHITE, true);
        if g.clicked {
            self.locate = Some(!self.locate.unwrap_or(false));
        }
        y += header_h + 6.0 * s;

        // ---- location editor --------------------------------------------------------------------
        if self.locate == Some(true) {
            y = self.location(c, rect, y);
            y += 8.0 * s;
        }

        // ---- forecast --------------------------------------------------------------------------------
        match self.model.clone() {
            None => {
                y += 8.0 * s;
                y += c.paragraph(
                    rect.x,
                    y,
                    rect.w,
                    "Your daily outlook starts with a city. Search above to get started.",
                    11.0,
                    Weight::Regular,
                    th.text_dim,
                );
                y += 8.0 * s;
            }
            Some(m) => {
                y = self.scene(c, rect, y, &m);
                y = Self::hourly(c, rect, y + 6.0 * s, &m);
            }
        }

        // ---- footer -----------------------------------------------------------------------------------
        y += 6.0 * s;
        let row_h = 26.0 * s;
        let retry = Rect::new(rect.right() - row_h, y, row_h, row_h);
        let r = c.icon_button(
            REFRESH,
            retry,
            Icon::Reset,
            20.0,
            None,
            Rgba::WHITE,
            !key_empty && !busy,
        );
        if r.clicked {
            c.push(Command::WeatherRefresh { force: true });
        }
        let credit_w = c.text_w("Open-Meteo", 9.0, Weight::Regular) + 8.0 * s;
        let credit = Rect::new(
            retry.x - credit_w,
            y + (row_h - 20.0 * s) / 2.0,
            credit_w,
            20.0 * s,
        );
        let cb = c.text_button(
            CREDIT,
            credit,
            "Open-Meteo",
            9.0,
            Weight::Regular,
            th.text_dim,
            None,
            true,
        );
        if cb.clicked {
            c.push(Command::OpenUrl("https://open-meteo.com/".to_owned()));
        }
        let status = self.status(c.ctx);
        let sy = y + (row_h - c.line_h(10.0, Weight::Regular)) / 2.0;
        c.text(
            rect.x,
            sy,
            &status,
            10.0,
            Weight::Regular,
            th.text_dim,
            Some((credit.x - rect.x - 6.0 * s).max(1.0)),
        );
        y += row_h;
        y - rect.y
    }

    /// Search box, units and results; returns the y below them.
    fn location(&mut self, c: &mut Ctl, rect: Rect, mut y: f32) -> f32 {
        let th = c.theme().clone();
        let s = c.s;
        let lh = c.line_h(13.0, Weight::Regular);
        let field_h = lh + 18.0 * s;
        let button_h = (c.line_h(13.0, Weight::Regular) + 12.0 * s).max(34.0 * s);
        let row_h = field_h.max(button_h);
        let bw = c.text_w("Search", 13.0, Weight::Regular) + 24.0 * s;
        let find = Rect::new(rect.right() - bw, y, bw, row_h);
        let field = Rect::new(rect.x, y, find.x - rect.x - 8.0 * s, row_h);
        // The text box: ControlFill, 1 DIP border that brightens on hover and focus.
        let hovered = c.pointer_in(field);
        let focused = c.editing(FIELD);
        c.rrect(field, 8.0, th.control_fill);
        let border = if focused {
            FIELD_FOCUS
        } else if hovered {
            FIELD_HOVER
        } else {
            th.control_border
        };
        c.rrect_stroke(field, 8.0, border, 1.0);
        let opts = EditOpts {
            dip: 13.0,
            max_chars: 120,
            centered: false,
            accept: |ch| !ch.is_control(),
        };
        let inner = field.inset_xy(4.0 * s, 0.0);
        let out = c.line_edit(
            FIELD,
            inner,
            &mut self.search,
            "Search city or town\u{2026}",
            &opts,
            true,
        );
        if out.submitted {
            self.start_search(c);
        }
        let b = c.text_button(
            SEARCH,
            find,
            "Search",
            13.0,
            Weight::Regular,
            th.control_text,
            Some(th.control_fill),
            true,
        );
        if b.clicked {
            self.start_search(c);
        }
        y += row_h + 8.0 * s;

        // The units "combo box".
        let combo = Rect::new(rect.x, y, rect.w, 30.0 * s);
        let cb = c.button(UNITS, combo, true);
        c.rrect(combo, 8.0, th.control_fill);
        c.rrect_stroke(combo, 8.0, th.control_border, 1.0);
        if cb.hovered {
            c.rrect(combo, 8.0, th.hover_fill);
        }
        let units = c.ctx.settings().weather_units.clone();
        let ty = combo.y + (combo.h - c.line_h(12.0, Weight::Regular)) / 2.0;
        c.text(
            combo.x + 11.0 * s,
            ty,
            &units,
            12.0,
            Weight::Regular,
            th.control_text,
            Some(combo.w - 43.0 * s),
        );
        let (cx, cy) = (combo.right() - 13.0 * s - 3.5 * s, combo.y + combo.h / 2.0);
        c.faded(0.65, |c| {
            c.stroke_polyline(
                &[
                    (cx - 3.5 * s, cy - 2.0 * s),
                    (cx, cy + 2.0 * s),
                    (cx + 3.5 * s, cy - 2.0 * s),
                ],
                th.control_text,
                1.2 * s,
            );
        });
        if cb.clicked {
            self.units_open = !self.units_open;
        }
        y += combo.h;
        if self.units_open {
            y += 2.0 * s;
            for (i, unit) in ["Celsius", "Fahrenheit"].iter().enumerate() {
                let h = c.line_h(12.0, Weight::Regular) + 16.0 * s;
                let item = Rect::new(rect.x, y, rect.w, h);
                let b = c.button(UNIT_ITEM + i as u32, item, true);
                let selected = *unit == units;
                if selected {
                    c.rrect(item, 6.0, th.selection_fill);
                } else if b.hovered {
                    c.rrect(item, 6.0, th.hover_fill);
                }
                let color = if selected {
                    th.selection_text
                } else {
                    th.control_text
                };
                c.text(
                    item.x + 10.0 * s,
                    y + (h - c.line_h(12.0, Weight::Regular)) / 2.0,
                    unit,
                    12.0,
                    Weight::Regular,
                    color,
                    Some(item.w - 20.0 * s),
                );
                if b.clicked {
                    self.units_open = false;
                    if !selected {
                        let mut next = c.ctx.settings().clone();
                        next.weather_units = (*unit).to_owned();
                        c.push(Command::ApplySettings(Box::new(next)));
                    }
                }
                y += h;
            }
        }
        y += 8.0 * s;

        // The cities found.
        if self.searched {
            if let PlaceSearchState::Found(places) = &c.ctx.snaps.places.state {
                let places = places.clone();
                for (i, place) in places.iter().enumerate() {
                    y += 2.0 * s;
                    let h = (c.line_h(11.0, Weight::Regular) + 18.0 * s).max(34.0 * s);
                    let r = Rect::new(rect.x, y, rect.w, h);
                    let b = c.button(PLACE + i as u32, r, true);
                    c.button_chrome(r, 8.0, Some(th.control_fill), true, &b);
                    c.text(
                        r.x + 14.0 * s,
                        y + (h - c.line_h(11.0, Weight::Regular)) / 2.0,
                        &place.name,
                        11.0,
                        Weight::Regular,
                        Rgba::WHITE,
                        Some(r.w - 28.0 * s),
                    );
                    if b.clicked {
                        self.choose_place(c, &place.name, place.latitude, place.longitude);
                    }
                    y += h + 2.0 * s;
                }
            }
        }
        y
    }

    /// The sky card; returns the y below it.
    fn scene(&mut self, c: &mut Ctl, rect: Rect, y: f32, m: &Model) -> f32 {
        let th = c.theme().clone();
        let s = c.s;
        let pad = (14.0 * s, 8.0 * s, 12.0 * s, 8.0 * s);
        let inner_w = rect.w - pad.0 - pad.2;
        let col_w = 108.0 * s;
        let read_h = c.line_h(9.0, Weight::Regular)
            + c.line_h(38.0, Weight::Regular)
            + c.line_h(12.0, Weight::Regular)
            + 4.0 * s
            + c.line_h(10.0, Weight::Regular);
        let hero_h = read_h.max(76.0 * s);
        let metric_h = c
            .line_h(9.0, Weight::Regular)
            .max(c.line_h(10.0, Weight::Regular));
        let card_h = pad.1 + hero_h + 7.0 * s + metric_h + pad.3;
        let card = Rect::new(rect.x, y, rect.w, card_h);
        if !c.is_dry() {
            art::draw_sky(c, card, m.day, m.code);
            c.rrect_stroke(card, 16.0, art::card_border(m.day), 0.7);
        }
        let (x0, mut cy) = (rect.x + pad.0, y + pad.1);
        let readings_w = (inner_w - col_w).max(1.0);
        cy += 0.0;
        c.text(
            x0,
            cy,
            m.today,
            9.0,
            Weight::Regular,
            MUTED_BLUE,
            Some(readings_w),
        );
        cy += c.line_h(9.0, Weight::Regular);
        c.text(
            x0,
            cy,
            &m.temperature,
            38.0,
            Weight::Regular,
            th.text,
            Some(readings_w),
        );
        cy += c.line_h(38.0, Weight::Regular);
        c.text(
            x0,
            cy,
            m.condition,
            12.0,
            Weight::Regular,
            th.text,
            Some(readings_w),
        );
        cy += c.line_h(12.0, Weight::Regular) + 4.0 * s;
        c.text(
            x0,
            cy,
            &m.range,
            10.0,
            Weight::Regular,
            RANGE,
            Some(readings_w),
        );
        // The illustration, centered in the 108 DIP column.
        let art_rect = Rect::new(
            x0 + readings_w + (col_w - 92.0 * s) / 2.0,
            y + pad.1 + (hero_h - 76.0 * s) / 2.0,
            92.0 * s,
            76.0 * s,
        );
        let drift = if self.animate { art::drift(c.now) } else { 0.0 };
        art::draw_artwork(c, art_rect, m.code, m.day, drift);
        // Rain / humidity / wind in three equal cells.
        let my = y + pad.1 + hero_h + 7.0 * s;
        let cell = inner_w / 3.0;
        for (i, (label, value)) in m.metrics.iter().enumerate() {
            let x = x0 + i as f32 * cell;
            let lw = c.text_w(&format!("{label} "), 9.0, Weight::Regular);
            let ly = my + (metric_h - c.line_h(9.0, Weight::Regular)) / 2.0;
            c.text(x, ly, label, 9.0, Weight::Regular, METRIC_LABEL, Some(cell));
            let vy = my + (metric_h - c.line_h(10.0, Weight::Regular)) / 2.0;
            c.text(
                x + lw,
                vy,
                value,
                10.0,
                Weight::Regular,
                METRIC_VALUE,
                Some((cell - lw).max(1.0)),
            );
        }
        y + card_h
    }

    /// The hourly strip; returns the y below it.
    fn hourly(c: &mut Ctl, rect: Rect, y: f32, m: &Model) -> f32 {
        let th = c.theme().clone();
        let s = c.s;
        let col_h = c.line_h(9.0, Weight::Regular)
            + 3.0 * s
            + 18.0 * s
            + 1.0 * s
            + c.line_h(11.0, Weight::Regular);
        let h = 5.0 * s + col_h + 10.0 * s + 4.0 * s;
        let card = Rect::new(rect.x, y, rect.w, h);
        if !c.is_dry() {
            c.rrect(card, 12.0, HOURLY_FILL);
            c.rrect_stroke(card, 12.0, HOURLY_BORDER, 0.6);
        }
        let n = m.hours.len().max(1) as f32;
        let inner = Rect::new(
            card.x + 10.0 * s,
            card.y + 5.0 * s,
            card.w - 20.0 * s,
            col_h,
        );
        let cell = inner.w / n;
        for (i, hour) in m.hours.iter().enumerate() {
            let cx = inner.x + (i as f32 + 0.5) * cell;
            let mut ty = inner.y;
            let tw = c.text_w(&hour.time, 9.0, Weight::Regular);
            c.text(
                cx - tw / 2.0,
                ty,
                &hour.time,
                9.0,
                Weight::Regular,
                th.text_dim,
                Some(cell),
            );
            ty += c.line_h(9.0, Weight::Regular) + 3.0 * s;
            art::draw_artwork(
                c,
                Rect::new(cx - 11.0 * s, ty, 22.0 * s, 18.0 * s),
                hour.code,
                hour.day,
                0.0,
            );
            ty += 18.0 * s + 1.0 * s;
            let vw = c.text_w(&hour.temperature, 11.0, Weight::Regular);
            c.text(
                cx - vw / 2.0,
                ty,
                &hour.temperature,
                11.0,
                Weight::Regular,
                th.text,
                Some(cell),
            );
        }
        y + h
    }
}

impl Page for WeatherPage {
    fn id(&self) -> &'static str {
        "weather"
    }

    fn height(&mut self, ctx: &PageCtx, width: f32) -> f32 {
        self.st.sync(ctx);
        let input = InputState::default();
        let mut c = Ctl::measure(ctx, &input, &mut self.ui);
        self.st.layout(&mut c, Rect::new(0.0, 0.0, width, 0.0))
    }

    fn frame(
        &mut self,
        canvas: &mut Canvas,
        ctx: &PageCtx,
        rect: Rect,
        input: &InputState,
        out: &mut Vec<Command>,
    ) -> bool {
        self.presence.mark(ctx.snaps.now);
        self.st.sync(ctx);
        let mut c = Ctl::draw(canvas, ctx, input, out, &mut self.ui);
        self.st.layout(&mut c, rect);
        false
    }

    fn peek(
        &mut self,
        canvas: &mut Canvas,
        ctx: &PageCtx,
        rect: Rect,
        input: &InputState,
        out: &mut Vec<Command>,
        _detailed: bool,
    ) -> Option<bool> {
        self.st.sync(ctx);
        let mut c = Ctl::draw(canvas, ctx, input, out, &mut self.ui);
        glance(&mut c, rect, self.st.model.as_ref(), ctx);
        Some(false)
    }

    fn demand(&self) -> Demand {
        Demand {
            weather: self.presence.active(),
            ..Demand::default()
        }
    }

    fn next_frame(&self, now: f64) -> Option<f64> {
        self.presence.observe(now);
        if !self.presence.active() {
            return None;
        }
        let drift = self.st.animate.then(|| now + 1.0 / 24.0);
        [drift, self.ui.next_frame(now)]
            .into_iter()
            .flatten()
            .reduce(f64::min)
    }

    fn captures_pointer(&self) -> bool {
        self.ui.captures()
    }

    fn on_hide(&mut self) {
        self.presence.hide();
        self.ui.reset();
        self.st = Body::default();
    }
}

/// `WeatherGlance`: sky, "Weather" caption, temperature and a detail line next to the artwork.
fn glance(c: &mut Ctl, rect: Rect, model: Option<&Model>, ctx: &PageCtx) {
    let th = c.theme().clone();
    let s = c.s;
    if let Some(m) = model {
        art::draw_sky(c, rect, m.day, m.code);
    }
    let weather = &ctx.snaps.weather;
    let (temperature, detail) = match (model, ctx.snaps.weather.snapshot.as_ref()) {
        (Some(m), Some(data)) => (
            m.temperature.clone(),
            format!(
                "{} \u{b7} H {} L {}",
                m.condition,
                wx::degrees(data.high),
                wx::degrees(data.low)
            ),
        ),
        _ if wx::key_for(ctx.settings()).is_empty() => (
            "\u{2014}\u{b0}".to_owned(),
            "Choose a city in Weather".to_owned(),
        ),
        _ if matches!(weather.status, WeatherStatus::Failed(_)) => (
            "\u{2014}\u{b0}".to_owned(),
            "Open Weather for your outlook".to_owned(),
        ),
        _ => (
            "\u{2014}\u{b0}".to_owned(),
            "Choose a city in Weather".to_owned(),
        ),
    };
    let inner = Rect::new(
        rect.x + 14.0 * s,
        rect.y + 8.0 * s,
        rect.w - 26.0 * s,
        rect.h - 16.0 * s,
    );
    let info_w = (inner.w - 90.0 * s).max(1.0);
    let mut y = inner.y;
    c.text(
        inner.x,
        y,
        "Weather",
        10.0,
        Weight::Regular,
        th.text_dim,
        Some(info_w),
    );
    y += c.line_h(10.0, Weight::Regular);
    c.text(
        inner.x,
        y,
        &temperature,
        34.0,
        Weight::Regular,
        th.text,
        Some(info_w),
    );
    y += c.line_h(34.0, Weight::Regular);
    c.text(
        inner.x,
        y,
        &detail,
        11.0,
        Weight::Regular,
        th.text_dim,
        Some(info_w),
    );
    if let Some(m) = model {
        let art_rect = Rect::new(
            inner.x + info_w + (90.0 * s - 84.0 * s) / 2.0,
            inner.y + (inner.h - 82.0 * s) / 2.0,
            84.0 * s,
            82.0 * s,
        );
        art::draw_artwork(c, art_rect, m.code, m.day, 0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::{PlaceSearch, WeatherState};
    use crate::pages::common::harness::Harness;
    use chrono::Utc;
    use naby_core::weather::{WeatherHour, WeatherPlace, WeatherSnapshot};

    fn forecast(code: i32, day: bool) -> WeatherSnapshot {
        WeatherSnapshot {
            key: "k".into(),
            updated: Utc::now(),
            temperature: 21.4,
            feels_like: 20.2,
            wind: 12.0,
            humidity: 63.0,
            high: 24.0,
            low: 15.0,
            rain_chance: 35.0,
            code,
            day,
            sunrise: "2026-10-29T06:40".into(),
            sunset: "2026-10-29T17:20".into(),
            hours: (0..6)
                .map(|i| WeatherHour {
                    time: format!("{:02}:00", 13 + i),
                    temperature: 21.0 - i as f64,
                    code: [0, 2, 3, 61, 71, 95][i],
                    day: i < 4,
                    rain_chance: 10.0,
                })
                .collect(),
        }
    }

    fn with_city(h: &mut Harness) {
        let mut s = h.snaps.settings.arc().as_ref().clone();
        s.weather_city = "Lisbon".into();
        s.weather_latitude = Some(38.72);
        s.weather_longitude = Some(-9.14);
        h.snaps.settings.replace(s);
        h.refresh_theme();
    }

    fn show(h: &mut Harness, code: i32, day: bool) {
        with_city(h);
        h.snaps.weather.replace(WeatherState {
            snapshot: Some(forecast(code, day)),
            status: WeatherStatus::Idle,
        });
    }

    #[test]
    fn asks_the_host_for_the_forecast_only_while_shown() {
        let Some(mut h) = Harness::new("weather") else {
            return;
        };
        assert!(!h.page.demand().weather);
        h.idle();
        assert!(h.page.demand().weather);
        h.page.next_frame(h.snaps.now + 30.0);
        assert!(!h.page.demand().weather);
    }

    #[test]
    fn refresh_forces_a_new_forecast_and_is_off_without_a_city() {
        let Some(mut h) = Harness::new("weather") else {
            return;
        };
        h.idle();
        h.click_id(REFRESH);
        assert!(h.commands().is_empty(), "no city yet");
        show(&mut h, 0, true);
        h.idle();
        h.click_id(REFRESH);
        assert_eq!(h.commands(), vec![Command::WeatherRefresh { force: true }]);
        h.snaps.weather.replace(WeatherState {
            snapshot: Some(forecast(0, true)),
            status: WeatherStatus::Loading,
        });
        h.idle();
        h.click_id(REFRESH);
        assert!(h.commands().is_empty(), "already refreshing");
    }

    #[test]
    fn the_location_editor_searches_and_picks_a_city() {
        let Some(mut h) = Harness::new("weather") else {
            return;
        };
        h.idle();
        // No city: the editor is open already.
        h.click_id(SEARCH);
        assert!(h.commands().is_empty(), "needs two letters");
        h.click_id(FIELD);
        h.frame(InputState::typed("Lis"));
        h.idle();
        h.click_id(SEARCH);
        assert_eq!(
            h.commands(),
            vec![Command::WeatherSearch {
                query: "Lis".into()
            }]
        );
        h.snaps.places.replace(PlaceSearch {
            query: "Lis".into(),
            state: PlaceSearchState::Found(vec![
                WeatherPlace {
                    name: "Lisbon, Portugal".into(),
                    latitude: 38.72,
                    longitude: -9.14,
                },
                WeatherPlace {
                    name: "Lisle, Illinois".into(),
                    latitude: 41.8,
                    longitude: -88.07,
                },
            ]),
        });
        h.idle();
        h.click_id(PLACE + 1);
        let commands = h.commands();
        match &commands[..] {
            [Command::ApplySettings(s)] => {
                assert_eq!(s.weather_city, "Lisle, Illinois");
                assert_eq!(s.weather_latitude, Some(41.8));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn units_are_chosen_from_the_combo() {
        let Some(mut h) = Harness::new("weather") else {
            return;
        };
        h.idle();
        h.click_id(UNITS);
        h.idle();
        h.click_id(UNIT_ITEM + 1);
        match &h.commands()[..] {
            [Command::ApplySettings(s)] => assert_eq!(s.weather_units, "Fahrenheit"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn the_gear_toggles_the_editor_and_the_credit_opens_open_meteo() {
        let Some(mut h) = Harness::new("weather") else {
            return;
        };
        show(&mut h, 2, true);
        h.idle();
        assert!(!h.has_control(FIELD), "editor starts closed with a city");
        h.click_id(GEAR);
        h.idle();
        assert!(h.has_control(FIELD));
        h.click_id(CREDIT);
        assert_eq!(
            h.commands(),
            vec![Command::OpenUrl("https://open-meteo.com/".into())]
        );
    }

    #[test]
    fn status_line_follows_the_fetch() {
        let Some(mut h) = Harness::new("weather") else {
            return;
        };
        let mut body = Body::default();
        h.idle();
        assert_eq!(
            body.status(&h.ctx()),
            "Choose a city to see today's weather."
        );
        show(&mut h, 0, true);
        h.snaps.weather.replace(WeatherState {
            snapshot: None,
            status: WeatherStatus::Failed("x".into()),
        });
        assert_eq!(
            body.status(&h.ctx()),
            "Weather unavailable. Refresh to try again."
        );
        h.snaps.weather.replace(WeatherState {
            snapshot: Some(forecast(0, true)),
            status: WeatherStatus::Failed("x".into()),
        });
        assert_eq!(
            body.status(&h.ctx()),
            "Saved outlook \u{b7} connection unavailable"
        );
        body.local_status = Some("Enter at least two letters of your city.".into());
        assert_eq!(
            body.status(&h.ctx()),
            "Enter at least two letters of your city."
        );
    }

    #[test]
    fn drift_animates_only_while_shown_and_motion_is_on() {
        let Some(mut h) = Harness::new("weather") else {
            return;
        };
        show(&mut h, 0, true);
        h.idle();
        assert!(h.page.next_frame(h.snaps.now).is_some());
        let mut s = h.snaps.settings.arc().as_ref().clone();
        s.reduce_motion = true;
        h.snaps.settings.replace(s);
        h.idle();
        assert_eq!(
            h.page.next_frame(h.snaps.now),
            None,
            "reduce motion is static"
        );
    }

    #[test]
    fn snapshots_of_every_state() {
        let Some(mut h) = Harness::new("weather") else {
            return;
        };
        h.idle();
        h.save("page-weather-empty");
        for (code, day, name) in [
            (0, true, "clear-day"),
            (2, true, "partly-cloudy"),
            (61, true, "rain"),
            (71, false, "snow-night"),
            (95, false, "thunder-night"),
            (45, true, "fog"),
            (0, false, "clear-night"),
        ] {
            show(&mut h, code, day);
            h.idle();
            assert!(h.ink() > 4000, "{name}");
            h.save(&format!("page-weather-{name}"));
        }
        h.snaps.weather.replace(WeatherState {
            snapshot: None,
            status: WeatherStatus::Failed("offline".into()),
        });
        h.idle();
        h.save("page-weather-failed");
    }

    #[test]
    fn the_hover_glance_paints_every_state() {
        let Some(mut h) = Harness::new("weather") else {
            return;
        };
        let ctx_h = |h: &mut Harness, name: &str| {
            h.canvas.resize(400, 140);
            h.canvas.clear(Rgba::BLACK);
            let ctx = PageCtx {
                fonts: &h.fonts,
                snaps: &h.snaps,
                theme: &h.theme,
                scale: 1.0,
            };
            let drawn = h.page.peek(
                &mut h.canvas,
                &ctx,
                Rect::new(20.0, 10.0, 360.0, 120.0),
                &InputState::default(),
                &mut h.out,
                true,
            );
            assert_eq!(drawn, Some(false));
            assert!(h.ink() > 300);
            h.save(name);
        };
        ctx_h(&mut h, "peek-weather-empty");
        show(&mut h, 2, true);
        ctx_h(&mut h, "peek-weather");
        h.snaps.weather.replace(WeatherState {
            snapshot: None,
            status: WeatherStatus::Failed("x".into()),
        });
        ctx_h(&mut h, "peek-weather-failed");
    }
}
