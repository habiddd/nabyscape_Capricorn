use super::*;
use chrono::TimeZone;
use std::cell::{Cell, RefCell};

const SAMPLE: &str = r#"{
  "current": {"time":"2026-10-29T14:15","temperature_2m":21.4,"relative_humidity_2m":63,
              "apparent_temperature":20.9,"is_day":1,"weather_code":2,"wind_speed_10m":11.6},
  "hourly": {"time":["2026-10-29T13:00","2026-10-29T14:00","2026-10-29T15:00","2026-10-29T16:00",
                     "2026-10-29T17:00","2026-10-29T18:00","2026-10-29T19:00","2026-10-29T20:00"],
             "temperature_2m":[20,21,22,23,24,25,26,27],
             "weather_code":[0,1,2,3,61,80,95,0],
             "is_day":[1,1,1,1,1,0,0,0],
             "precipitation_probability":[0,5,10,20,40,60,80,10]},
  "daily": {"temperature_2m_max":[25.2],"temperature_2m_min":[14.1],"precipitation_probability_max":[80],
            "sunrise":["2026-10-29T06:41"],"sunset":["2026-10-29T17:52"]}
}"#;

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 29, 12, 0, 0).unwrap()
}

fn paris() -> WeatherPlace {
    WeatherPlace {
        name: "Paris".into(),
        latitude: 48.85341,
        longitude: 2.3488,
    }
}

#[test]
fn weather_parse_sample_json() {
    let s = WeatherService::parse_forecast(SAMPLE, "k".into(), now()).unwrap();
    assert_eq!(s.key, "k");
    assert_eq!(s.temperature, 21.4);
    assert_eq!(s.feels_like, 20.9);
    assert_eq!(s.humidity, 63.0);
    assert_eq!(s.wind, 11.6);
    assert_eq!(s.code, 2);
    assert!(s.day);
    assert_eq!((s.high, s.low, s.rain_chance), (25.2, 14.1, 80.0));
    assert_eq!(
        (s.sunrise.as_str(), s.sunset.as_str()),
        ("2026-10-29T06:41", "2026-10-29T17:52")
    );
    // Hours before the current time are skipped; at most six remain.
    let times: Vec<_> = s.hours.iter().map(|h| h.time.as_str()).collect();
    assert_eq!(
        times,
        ["15:00", "16:00", "17:00", "18:00", "19:00", "20:00"]
    );
    assert_eq!(s.hours[0].temperature, 22.0);
    assert_eq!(s.hours[2].code, 61);
    assert_eq!(s.hours[2].rain_chance, 40.0);
    assert!(s.hours[2].day && !s.hours[3].day);
    assert_eq!(s.updated, now());
}

#[test]
fn weather_url_matches_csharp() {
    let seen = RefCell::new(String::new());
    let fetch = |url: &str| {
        *seen.borrow_mut() = url.to_owned();
        Ok(SAMPLE.to_owned())
    };
    WeatherService::fetch(&paris(), "Fahrenheit", &fetch, now()).unwrap();
    let url = seen.borrow().clone();
    assert!(url.starts_with("https://api.open-meteo.com/v1/forecast?latitude=48.85341&longitude=2.3488&current=temperature_2m,relative_humidity_2m,apparent_temperature,is_day,weather_code,wind_speed_10m&hourly=temperature_2m,weather_code,is_day,precipitation_probability&daily=temperature_2m_max,temperature_2m_min,precipitation_probability_max,sunrise,sunset&timezone=auto&forecast_days=2&temperature_unit="));
    assert!(url.ends_with("fahrenheit"));
    WeatherService::fetch(&paris(), "Celsius", &fetch, now()).unwrap();
    assert!(seen.borrow().ends_with("celsius"));
}

#[test]
fn weather_missing_numbers_default_to_zero() {
    let body = r#"{"current":{"temperature_2m":"hot","weather_code":null},
                   "hourly":{"time":[]},"daily":{"sunrise":[null],"sunset":["2026-10-29T17:52"]}}"#;
    let s = WeatherService::parse_forecast(body, String::new(), now()).unwrap();
    assert_eq!((s.temperature, s.code, s.high), (0.0, 0, 0.0));
    assert!(!s.day && s.hours.is_empty());
    assert_eq!(s.sunrise, "");
}

#[test]
fn weather_bad_documents_are_errors_not_panics() {
    for body in [
        "",
        "not json",
        "[]",
        "{}",
        r#"{"current":{},"hourly":{},"daily":{}}"#,
        r#"{"current":{},"hourly":{"time":[1]},"daily":{"sunrise":["a"],"sunset":["b"]}}"#,
        r#"{"current":1,"hourly":2,"daily":3}"#,
    ] {
        assert!(
            matches!(
                WeatherService::parse_forecast(body, String::new(), now()),
                Err(WeatherError::Schema(_))
            ),
            "{body}"
        );
    }
}

#[test]
fn weather_offline_and_http_errors() {
    let offline = |_: &str| Err("offline".to_owned());
    assert_eq!(
        WeatherService::fetch(&paris(), "Celsius", &offline, now()),
        Err(WeatherError::Network("offline".into()))
    );
    assert!(WeatherService::search("Paris", &offline).is_err());
    let mut service = WeatherService::default();
    assert!(service
        .get(Some(&paris()), "Celsius", false, now(), &offline)
        .is_err());
    assert!(service.cached().is_none());
    assert_eq!(service.last_attempt(), Some(now()));
    let nan = WeatherPlace {
        latitude: f64::NAN,
        ..paris()
    };
    assert_eq!(
        WeatherService::fetch(&nan, "Celsius", &offline, now()),
        Err(WeatherError::NoCity)
    );
}

#[test]
fn weather_cache_for_twenty_minutes() {
    let calls = Cell::new(0);
    let fetch = |_: &str| {
        calls.set(calls.get() + 1);
        Ok(SAMPLE.to_owned())
    };
    let mut service = WeatherService::default();
    let place = paris();
    let first = service
        .get(Some(&place), "Celsius", false, now(), &fetch)
        .unwrap();
    assert_eq!(first.key, "48.85341,2.34880|Celsius");
    let again = service
        .get(
            Some(&place),
            "Celsius",
            false,
            now() + ChronoDuration::minutes(19),
            &fetch,
        )
        .unwrap();
    assert_eq!((calls.get(), again.updated), (1, now()));
    service
        .get(
            Some(&place),
            "Celsius",
            false,
            now() + ChronoDuration::minutes(21),
            &fetch,
        )
        .unwrap();
    assert_eq!(calls.get(), 2);
    service
        .get(
            Some(&place),
            "Celsius",
            true,
            now() + ChronoDuration::minutes(22),
            &fetch,
        )
        .unwrap();
    assert_eq!(calls.get(), 3);
    // Another unit is another key.
    service
        .get(
            Some(&place),
            "Fahrenheit",
            false,
            now() + ChronoDuration::minutes(22),
            &fetch,
        )
        .unwrap();
    assert_eq!(calls.get(), 4);
    // A failure keeps the previous cache.
    let offline = |_: &str| Err("down".to_owned());
    assert!(service
        .get(Some(&place), "Fahrenheit", true, now(), &offline)
        .is_err());
    assert!(service.cached().is_some());
    assert_eq!(
        service.get(None, "Celsius", false, now(), &fetch),
        Err(WeatherError::NoCity)
    );
}

#[test]
fn weather_cache_document_roundtrip() {
    let s = WeatherService::parse_forecast(SAMPLE, "k".into(), now()).unwrap();
    let json = serde_json::to_string(&s).unwrap();
    assert!(
        json.contains("\"FeelsLike\"")
            && json.contains("\"RainChance\"")
            && json.contains("\"Hours\"")
    );
    let back: WeatherSnapshot = serde_json::from_str(&json).unwrap();
    assert_eq!(back, s);
    // A document written by C# (offset timestamp, extra keys) loads.
    let csharp = r#"{"Key":"a","Updated":"2026-10-29T14:00:00+02:00","Temperature":1.5,"Extra":1,"Hours":[{"Time":"15:00","Temperature":2,"Code":3,"Day":true,"RainChance":4}]}"#;
    let loaded: WeatherSnapshot = serde_json::from_str(csharp).unwrap();
    assert_eq!(
        loaded.updated,
        Utc.with_ymd_and_hms(2026, 10, 29, 12, 0, 0).unwrap()
    );
    assert_eq!(loaded.hours[0].code, 3);
    assert_eq!(DOCUMENT_NAME, "weather");
}

#[test]
fn weather_key_matches_csharp() {
    assert_eq!(
        key(Some(1.5), Some(-2.25), "Celsius"),
        "1.50000,-2.25000|Celsius"
    );
    assert_eq!(key(None, Some(1.0), "Celsius"), "");
    assert_eq!(key(Some(1.0), None, "Celsius"), "");
    let mut s = Settings::default();
    assert_eq!(key_for(&s), "");
    assert!(place_for(&s).is_none());
    s.weather_latitude = Some(10.0);
    s.weather_longitude = Some(20.0);
    s.weather_city = "X".into();
    assert_eq!(key_for(&s), "10.00000,20.00000|Celsius");
    assert_eq!(place_for(&s).unwrap().name, "X");
}

#[test]
fn weather_search_geocoding() {
    let body = r#"{"results":[
        {"name":"Paris","admin1":"Île-de-France","country":"France","latitude":48.85,"longitude":2.35},
        {"name":"Singapore","admin1":"Singapore","country":"Singapore","latitude":1.29,"longitude":103.85},
        {"name":"Bad","latitude":91,"longitude":0},
        {"name":"Worse","latitude":0,"longitude":181},
        {"country":"Nowhere"}]}"#;
    let seen = RefCell::new(String::new());
    let fetch = |url: &str| {
        *seen.borrow_mut() = url.to_owned();
        Ok(body.to_owned())
    };
    let places = WeatherService::search("  São Paulo ", &fetch).unwrap();
    assert_eq!(
        *seen.borrow(),
        "https://geocoding-api.open-meteo.com/v1/search?name=S%C3%A3o%20Paulo&count=8&language=en&format=json"
    );
    let names: Vec<_> = places.iter().map(|p| p.name.as_str()).collect();
    // Duplicate parts collapse; out-of-range coordinates are dropped; missing numbers become 0.
    assert_eq!(
        names,
        ["Paris, Île-de-France, France", "Singapore", "Nowhere"]
    );
    assert_eq!((places[2].latitude, places[2].longitude), (0.0, 0.0));
}

#[test]
fn weather_search_edge_cases() {
    let never = |_: &str| panic!("must not be called");
    assert!(WeatherService::search("", &never).unwrap().is_empty());
    assert!(WeatherService::search(" a ", &never).unwrap().is_empty());
    let none = |_: &str| Ok("{}".to_owned());
    assert!(WeatherService::search("Paris", &none).unwrap().is_empty());
    let junk = |_: &str| Ok("<html>".to_owned());
    assert!(matches!(
        WeatherService::search("Paris", &junk),
        Err(WeatherError::Schema(_))
    ));
    let wrong = |_: &str| Ok(r#"{"results":{}}"#.to_owned());
    assert!(WeatherService::search("Paris", &wrong).is_err());
    let long = "é".repeat(500);
    let seen = RefCell::new(String::new());
    let capture = |url: &str| {
        *seen.borrow_mut() = url.to_owned();
        Ok("{}".to_owned())
    };
    WeatherService::search(&long, &capture).unwrap();
    assert_eq!(seen.borrow().matches("%C3%A9").count(), 120);
}

#[test]
fn weather_condition_labels() {
    for (code, label) in [
        (0, "Clear skies"),
        (1, "Mostly clear"),
        (2, "Partly cloudy"),
        (3, "Cloudy"),
        (45, "Foggy"),
        (48, "Foggy"),
        (51, "Drizzle"),
        (57, "Drizzle"),
        (61, "Rain"),
        (67, "Rain"),
        (71, "Snow"),
        (77, "Snow"),
        (80, "Rain showers"),
        (82, "Rain showers"),
        (85, "Snow showers"),
        (86, "Snow showers"),
        (95, "Thunderstorms"),
        (99, "Thunderstorms"),
        (4, "Weather"),
        (58, "Weather"),
        (-1, "Weather"),
    ] {
        assert_eq!(condition(code), label, "code {code}");
    }
}

#[test]
fn weather_artwork_layers() {
    let a = artwork(0, true);
    assert!(a.sky_body && a.sun_rays && !a.cloud && !a.rain);
    assert!(!artwork(0, false).sun_rays);
    let a = artwork(2, true);
    assert!(a.sky_body && a.cloud && !a.sun_rays);
    let a = artwork(61, true);
    assert!(a.rain && a.cloud && !a.sky_body && !a.snow);
    assert!(artwork(73, false).snow);
    assert!(artwork(45, true).fog && artwork(48, true).fog);
    let a = artwork(95, true);
    assert!(a.thunder && a.rain);
    assert!(sky_is_wet(51) && !sky_is_wet(3));
}

#[test]
fn weather_formatting() {
    assert_eq!(degrees(21.5), "22°");
    assert_eq!(degrees(-0.4), "0°");
    assert_eq!(degrees(f64::NAN), "0°");
    assert_eq!(clock_text("2026-10-29T06:41"), "06:41");
    assert_eq!(clock_text(""), "—");
}

#[test]
fn weather_auto_refresh_policy() {
    let mut service = WeatherService::default();
    assert!(service.should_auto_refresh(now(), false, None));
    assert!(!service.should_auto_refresh(now(), true, None));
    let offline = |_: &str| Err("x".to_owned());
    let _ = service.get(Some(&paris()), "Celsius", false, now(), &offline);
    let later = now() + ChronoDuration::minutes(2);
    assert!(!service.should_auto_refresh(later, false, None));
    let later = now() + ChronoDuration::minutes(4);
    assert!(service.should_auto_refresh(later, false, None));
    assert!(!service.should_auto_refresh(later, false, Some(later - ChronoDuration::minutes(5))));
    assert!(service.should_auto_refresh(later, false, Some(later - ChronoDuration::minutes(25))));
}
