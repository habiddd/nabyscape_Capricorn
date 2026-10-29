//! Ported from `Features/Weather/{WeatherService,WeatherArtwork,WeatherVisuals,WeatherView}.cs`
//! (data, caching and code-to-label/icon logic only; painting belongs to the renderer).
//!
//! Open-Meteo geocoding and forecast, with an injected fetch closure so every path runs offline.
//! Network, HTTP-status and schema failures resolve to [`WeatherError`], never a panic.
//! Single-flight request sharing is the host's concern (one worker thread per key).

#[cfg(test)]
mod tests;

use std::io::Read;
use std::time::Duration;

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::settings::Settings;

/// Name of the persisted forecast cache document (`weather.json`).
pub const DOCUMENT_NAME: &str = "weather";
/// A cached forecast younger than this is served without a request.
pub const CACHE_MINUTES: i64 = 20;
/// Minimum gap between automatic refresh attempts.
pub const RETRY_MINUTES: i64 = 3;

const GEOCODING_URL: &str = "https://geocoding-api.open-meteo.com/v1/search";
const FORECAST_URL: &str = "https://api.open-meteo.com/v1/forecast";
const RESPONSE_LIMIT: u64 = 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// An injected GET: `url -> response body`, or an error text (offline, timeout, HTTP 4xx/5xx).
pub type Fetch<'a> = &'a dyn Fn(&str) -> Result<String, String>;

/// The default fetch: the shared agent, 15 s timeout, 1 MiB response cap, 2xx only.
pub fn default_fetch(url: &str) -> Result<String, String> {
    let response = crate::http::agent()
        .get(url)
        .timeout(REQUEST_TIMEOUT)
        .call()
        .map_err(|e| e.to_string())?;
    let mut body = String::new();
    response
        .into_reader()
        .take(RESPONSE_LIMIT + 1)
        .read_to_string(&mut body)
        .map_err(|e| e.to_string())?;
    if body.len() as u64 > RESPONSE_LIMIT {
        return Err("Weather response is too large.".into());
    }
    Ok(body)
}

#[derive(Debug, Clone, PartialEq)]
pub struct WeatherPlace {
    pub name: String,
    pub latitude: f64,
    pub longitude: f64,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct WeatherHour {
    /// `HH:mm` local to the forecast location.
    pub time: String,
    pub temperature: f64,
    pub code: i32,
    pub day: bool,
    pub rain_chance: f64,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct WeatherSnapshot {
    pub key: String,
    pub updated: DateTime<Utc>,
    pub temperature: f64,
    pub feels_like: f64,
    pub wind: f64,
    pub humidity: f64,
    pub high: f64,
    pub low: f64,
    pub rain_chance: f64,
    pub code: i32,
    pub day: bool,
    pub sunrise: String,
    pub sunset: String,
    pub hours: Vec<WeatherHour>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum WeatherError {
    /// No city has been chosen ("Choose a city first.").
    NoCity,
    /// Offline, timed out, or a non-success HTTP status.
    Network(String),
    /// The response was not the expected Open-Meteo document.
    Schema(String),
}

impl std::fmt::Display for WeatherError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoCity => f.write_str("Choose a city first."),
            Self::Network(m) | Self::Schema(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for WeatherError {}

/// C# `WeatherService.Key`: empty when no city coordinates are set.
pub fn key(latitude: Option<f64>, longitude: Option<f64>, units: &str) -> String {
    match (latitude, longitude) {
        (Some(lat), Some(lon)) => format!("{lat:.5},{lon:.5}|{units}"),
        _ => String::new(),
    }
}

/// [`key`] for the current preferences.
pub fn key_for(s: &Settings) -> String {
    key(s.weather_latitude, s.weather_longitude, &s.weather_units)
}

/// The chosen city, if its coordinates are set.
pub fn place_for(s: &Settings) -> Option<WeatherPlace> {
    Some(WeatherPlace {
        name: s.weather_city.clone(),
        latitude: s.weather_latitude?,
        longitude: s.weather_longitude?,
    })
}

/// `Uri.EscapeDataString`: everything except `A-Za-z0-9-_.~` is percent-encoded as UTF-8.
pub fn escape_data_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn number(value: &Value, name: &str) -> f64 {
    value
        .get(name)
        .and_then(Value::as_f64)
        .filter(|n| n.is_finite())
        .unwrap_or(0.0)
}

fn at(value: &Value, name: &str, index: usize) -> f64 {
    value
        .get(name)
        .and_then(|a| a.as_array())
        .and_then(|a| a.get(index))
        .and_then(Value::as_f64)
        .filter(|n| n.is_finite())
        .unwrap_or(0.0)
}

fn text(value: &Value, name: &str) -> String {
    value
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned()
}

fn schema(message: &str) -> WeatherError {
    WeatherError::Schema(message.to_owned())
}

fn parse_document(body: &str) -> Result<Value, WeatherError> {
    serde_json::from_str(body).map_err(|e| WeatherError::Schema(e.to_string()))
}

/// The open-meteo string at `daily.<name>[0]`; required, null reads as empty.
fn first_string(daily: &Value, name: &str) -> Result<String, WeatherError> {
    match daily
        .get(name)
        .and_then(|a| a.as_array())
        .and_then(|a| a.first())
    {
        Some(Value::String(s)) => Ok(s.clone()),
        Some(Value::Null) => Ok(String::new()),
        _ => Err(schema("Forecast is missing sunrise or sunset.")),
    }
}

/// `HH:mm` from an ISO local time such as `2026-10-29T14:00`.
fn clock_of(time: &str) -> String {
    if time.chars().count() >= 16 {
        time.chars().skip(11).take(5).collect()
    } else {
        time.to_owned()
    }
}

/// Weather lookups and the in-memory forecast cache.
#[derive(Debug, Default)]
pub struct WeatherService {
    cached: Option<WeatherSnapshot>,
    last_attempt: Option<DateTime<Utc>>,
}

impl WeatherService {
    /// A service seeded with the persisted `weather` document, if it loaded.
    pub fn with_cached(cached: Option<WeatherSnapshot>) -> Self {
        Self {
            cached,
            last_attempt: None,
        }
    }

    pub fn cached(&self) -> Option<&WeatherSnapshot> {
        self.cached.as_ref()
    }

    pub fn last_attempt(&self) -> Option<DateTime<Utc>> {
        self.last_attempt
    }

    /// C# `WeatherService.Search`: up to eight matching places for a city name.
    pub fn search(city: &str, fetch: Fetch<'_>) -> Result<Vec<WeatherPlace>, WeatherError> {
        let city = city.trim();
        if city.chars().count() < 2 {
            return Ok(Vec::new());
        }
        let city: String = city.chars().take(120).collect();
        let url = format!(
            "{GEOCODING_URL}?name={}&count=8&language=en&format=json",
            escape_data_string(&city)
        );
        let body = fetch(&url).map_err(WeatherError::Network)?;
        let doc = parse_document(&body)?;
        let Some(results) = doc.get("results") else {
            return Ok(Vec::new());
        };
        let results = results
            .as_array()
            .ok_or_else(|| schema("Unexpected geocoding results."))?;
        let mut places = Vec::new();
        for place in results {
            if !place.is_object() {
                return Err(schema("Unexpected geocoding result."));
            }
            let mut parts: Vec<String> = Vec::new();
            for field in ["name", "admin1", "country"] {
                let value = text(place, field);
                if !value.is_empty() && !parts.contains(&value) {
                    parts.push(value);
                }
            }
            let place = WeatherPlace {
                name: parts.join(", "),
                latitude: number(place, "latitude"),
                longitude: number(place, "longitude"),
            };
            if place.latitude.abs() <= 90.0 && place.longitude.abs() <= 180.0 {
                places.push(place);
            }
        }
        Ok(places)
    }

    /// C# `WeatherService.Download`: one uncached forecast request for `place`.
    pub fn fetch(
        place: &WeatherPlace,
        units: &str,
        fetch: Fetch<'_>,
        now: DateTime<Utc>,
    ) -> Result<WeatherSnapshot, WeatherError> {
        if !place.latitude.is_finite() || !place.longitude.is_finite() {
            return Err(WeatherError::NoCity);
        }
        let key = key(Some(place.latitude), Some(place.longitude), units);
        let url = format!(
            "{FORECAST_URL}?latitude={}&longitude={}&current=temperature_2m,relative_humidity_2m,apparent_temperature,is_day,weather_code,wind_speed_10m&hourly=temperature_2m,weather_code,is_day,precipitation_probability&daily=temperature_2m_max,temperature_2m_min,precipitation_probability_max,sunrise,sunset&timezone=auto&forecast_days=2&temperature_unit={}",
            place.latitude,
            place.longitude,
            if units == "Fahrenheit" { "fahrenheit" } else { "celsius" }
        );
        let body = fetch(&url).map_err(WeatherError::Network)?;
        Self::parse_forecast(&body, key, now)
    }

    /// Builds a snapshot from an Open-Meteo forecast document.
    pub fn parse_forecast(
        body: &str,
        key: String,
        now: DateTime<Utc>,
    ) -> Result<WeatherSnapshot, WeatherError> {
        let root = parse_document(body)?;
        let object = |name: &str| -> Result<&Value, WeatherError> {
            root.get(name)
                .filter(|v| v.is_object())
                .ok_or_else(|| WeatherError::Schema(format!("Forecast is missing '{name}'.")))
        };
        let (current, daily, hourly) = (object("current")?, object("daily")?, object("hourly")?);
        let mut snapshot = WeatherSnapshot {
            key,
            updated: now,
            temperature: number(current, "temperature_2m"),
            feels_like: number(current, "apparent_temperature"),
            wind: number(current, "wind_speed_10m"),
            humidity: number(current, "relative_humidity_2m"),
            code: number(current, "weather_code") as i32,
            day: number(current, "is_day") == 1.0,
            high: at(daily, "temperature_2m_max", 0),
            low: at(daily, "temperature_2m_min", 0),
            rain_chance: at(daily, "precipitation_probability_max", 0),
            sunrise: first_string(daily, "sunrise")?,
            sunset: first_string(daily, "sunset")?,
            hours: Vec::new(),
        };
        let now_text = text(current, "time");
        let times = hourly
            .get("time")
            .and_then(Value::as_array)
            .ok_or_else(|| schema("Forecast is missing hourly times."))?;
        for (i, time) in times.iter().enumerate() {
            if snapshot.hours.len() >= 6 {
                break;
            }
            let time = match time {
                Value::String(s) => s.as_str(),
                Value::Null => "",
                _ => return Err(schema("Unexpected hourly time.")),
            };
            if time < now_text.as_str() {
                continue;
            }
            snapshot.hours.push(WeatherHour {
                time: clock_of(time),
                temperature: at(hourly, "temperature_2m", i),
                code: at(hourly, "weather_code", i) as i32,
                day: at(hourly, "is_day", i) == 1.0,
                rain_chance: at(hourly, "precipitation_probability", i),
            });
        }
        Ok(snapshot)
    }

    /// C# `WeatherService.Get`: serves a fresh cached forecast unless `force`, otherwise fetches
    /// and caches. The caller persists the returned snapshot under [`DOCUMENT_NAME`].
    pub fn get(
        &mut self,
        place: Option<&WeatherPlace>,
        units: &str,
        force: bool,
        now: DateTime<Utc>,
        fetch: Fetch<'_>,
    ) -> Result<WeatherSnapshot, WeatherError> {
        let place = place.ok_or(WeatherError::NoCity)?;
        let key = key(Some(place.latitude), Some(place.longitude), units);
        if key.is_empty() {
            return Err(WeatherError::NoCity);
        }
        if !force {
            if let Some(saved) = &self.cached {
                if saved.key == key && now - saved.updated < ChronoDuration::minutes(CACHE_MINUTES)
                {
                    return Ok(saved.clone());
                }
            }
        }
        self.last_attempt = Some(now);
        let snapshot = Self::fetch(place, units, fetch, now)?;
        self.cached = Some(snapshot.clone());
        Ok(snapshot)
    }

    /// C# `WeatherView.Heartbeat`: refresh when idle, not tried for three minutes, and the shown
    /// forecast is missing or older than twenty minutes.
    pub fn should_auto_refresh(
        &self,
        now: DateTime<Utc>,
        busy: bool,
        shown_updated: Option<DateTime<Utc>>,
    ) -> bool {
        if busy {
            return false;
        }
        let attempt_old = self
            .last_attempt
            .is_none_or(|t| now - t > ChronoDuration::minutes(RETRY_MINUTES));
        attempt_old
            && shown_updated.is_none_or(|u| now - u > ChronoDuration::minutes(CACHE_MINUTES))
    }
}

/// C# `WeatherService.Condition`: the label for a WMO weather code.
pub fn condition(code: i32) -> &'static str {
    match code {
        0 => "Clear skies",
        1 => "Mostly clear",
        2 => "Partly cloudy",
        3 => "Cloudy",
        45 | 48 => "Foggy",
        51..=57 => "Drizzle",
        61..=67 => "Rain",
        71..=77 => "Snow",
        80..=82 => "Rain showers",
        85 | 86 => "Snow showers",
        c if c >= 95 => "Thunderstorms",
        _ => "Weather",
    }
}

/// Which layers `WeatherArtwork` draws for a code (C# `OnRender`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Artwork {
    pub rain: bool,
    pub snow: bool,
    pub cloud: bool,
    pub fog: bool,
    pub thunder: bool,
    /// Sun (day) or moon (night) disc: clear to partly cloudy.
    pub sky_body: bool,
    /// Sun rays: only when the sun is not behind clouds.
    pub sun_rays: bool,
}

/// Layer flags for the weather icon.
pub fn artwork(code: i32, day: bool) -> Artwork {
    let cloud = code >= 2;
    Artwork {
        rain: matches!(code, 51..=67 | 80..=82) || code >= 95,
        snow: matches!(code, 71..=77 | 85 | 86),
        cloud,
        fog: matches!(code, 45 | 48),
        thunder: code >= 95,
        sky_body: code <= 2,
        sun_rays: day && code <= 2 && !cloud,
    }
}

/// `WeatherSky`: wet weather darkens the day gradient.
pub fn sky_is_wet(code: i32) -> bool {
    code >= 51
}

/// Whole degrees with a degree sign (`{0:0}°`, rounding halves away from zero).
pub fn degrees(value: f64) -> String {
    format!(
        "{}°",
        if value.is_finite() {
            value.round() as i64
        } else {
            0
        }
    )
}

/// `Clock()` in `WeatherView.Render`: `HH:mm` of an ISO time, or an em dash.
pub fn clock_text(iso_time: &str) -> String {
    if iso_time.chars().count() >= 16 {
        iso_time.chars().skip(11).take(5).collect()
    } else {
        "—".to_owned()
    }
}
