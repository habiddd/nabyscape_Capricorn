//! The pages inside the real overlay: each page open in the panel and each hover face, with
//! snapshots under `target/snapshots/` (`panel-*.png`, `hover-*.png`).

use super::harness::cover_png;
use super::overlay_rig::OverlayRig;
use crate::host::{NetRates, SystemInfo, WeatherState, WeatherStatus};
use chrono::Utc;
use naby_core::color::Rgba;
use naby_core::media::{AudioSnapshot, MediaSnapshot};
use naby_core::session::SessionState;
use naby_core::system::battery::{BatterySnapshot, RawPowerStatus};
use naby_core::system::{MonitorDrive, MonitorSnapshot};
use naby_core::weather::{WeatherHour, WeatherSnapshot};
use naby_render::snapshot;
use std::sync::Arc;
use std::time::Duration;

fn feed(r: &mut OverlayRig) {
    r.snaps.media.set(MediaSnapshot {
        available: true,
        source_id: "Spotify.exe".into(),
        source_name: "Spotify".into(),
        title: "Midnight City".into(),
        artist: "M83".into(),
        playing: true,
        position: Duration::from_secs(42),
        duration: Duration::from_secs(243),
        can_toggle: true,
        can_next: true,
        can_previous: true,
        can_seek: true,
        artwork: Some(cover_png()),
        ..Default::default()
    });
    r.snaps.media_epoch = r.snaps.now;
    r.snaps.audio.set(AudioSnapshot {
        available: true,
        volume: 0.63,
        device: "Speakers".into(),
        ..Default::default()
    });
    let mut session = SessionState::default();
    let now = r.snaps.wall.with_timezone(&Utc);
    session
        .timer
        .start(600.0, now - chrono::Duration::seconds(200), "", false);
    session
        .focus
        .countdown
        .start(1500.0, now - chrono::Duration::seconds(300), "work", false);
    session.stopwatch.elapsed_seconds = 754.0;
    r.snaps.session.replace(session);
    r.snaps
        .battery
        .replace(BatterySnapshot::from_raw(RawPowerStatus {
            ac_line_status: 0,
            battery_flag: 1,
            battery_life_percent: 63,
            system_status_flag: 0,
            battery_life_time: u32::MAX,
        }));
    let mut sample = MonitorSnapshot::unavailable("");
    sample.errors.clear();
    sample.total = Some(16 << 30);
    sample.available = Some(6 << 30);
    sample.uptime = Duration::from_secs(90_000);
    sample.drives = vec![MonitorDrive {
        name: "C:\\".into(),
        free: 100 << 30,
        total: 500 << 30,
    }];
    r.snaps.system.replace(SystemInfo {
        latest: Some(Arc::new(sample)),
        cpu: Some(37.5),
        cpu_history: (0..30)
            .map(|i| Some(20.0 + (i as f64 * 0.5).sin() * 12.0 + i as f64))
            .collect(),
        networks: vec![NetRates {
            id: "e".into(),
            name: "Ethernet".into(),
            received: Some(2e6),
            sent: Some(3e5),
        }],
        reads: 4,
    });
    let mut s = r.snaps.settings.arc().as_ref().clone();
    s.weather_city = "Lisbon".into();
    s.weather_latitude = Some(38.7);
    s.weather_longitude = Some(-9.1);
    r.snaps.settings.replace(s);
    r.snaps.weather.replace(WeatherState {
        snapshot: Some(WeatherSnapshot {
            key: "k".into(),
            updated: Utc::now(),
            temperature: 21.4,
            feels_like: 20.2,
            wind: 12.0,
            humidity: 63.0,
            high: 24.0,
            low: 15.0,
            rain_chance: 35.0,
            code: 2,
            day: true,
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
        }),
        status: WeatherStatus::Idle,
    });
}

/// How many pixels of the island differ from the black glass (content was drawn).
fn inked(r: &OverlayRig) -> usize {
    let (ox, oy) = r.overlay.surface_origin(&r.snaps);
    let s = r.overlay.scale;
    let rect = naby_render::Rect::new(
        ox,
        oy,
        r.overlay.surf.0 as f32 * s,
        r.overlay.surf.1 as f32 * s,
    );
    snapshot::count_differing(&r.canvas, rect.inset(8.0), Rgba::BLACK)
}

#[test]
fn every_page_paints_open_in_the_panel() {
    for id in [
        "media",
        "timer",
        "focus",
        "stopwatch",
        "clock",
        "system",
        "battery",
        "weather",
    ] {
        let Some(mut r) = OverlayRig::new() else {
            return;
        };
        feed(&mut r);
        r.open_page(id);
        assert!(inked(&r) > 2000, "{id} is blank");
        r.save(&format!("panel-{id}"));
    }
}

#[test]
fn every_hover_face_paints() {
    for id in [
        "media",
        "timer",
        "focus",
        "stopwatch",
        "clock",
        "system",
        "battery",
        "weather",
        "volume",
    ] {
        let Some(mut r) = OverlayRig::new() else {
            return;
        };
        feed(&mut r);
        r.hover_page(id);
        assert!(inked(&r) > 800, "{id} hover face is blank");
        r.save(&format!("hover-{id}"));
    }
}

#[test]
fn unavailable_states_paint_in_the_panel() {
    for id in [
        "media",
        "system",
        "battery",
        "weather",
        "clock",
        "timer",
        "focus",
        "stopwatch",
    ] {
        let Some(mut r) = OverlayRig::new() else {
            return;
        };
        r.open_page(id);
        assert!(inked(&r) > 500, "{id} is blank without data");
        r.save(&format!("panel-{id}-empty"));
    }
}
