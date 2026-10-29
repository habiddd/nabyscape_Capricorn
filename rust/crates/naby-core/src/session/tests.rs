use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use chrono::{DateTime, Duration, TimeZone, Utc};
use serde_json::{json, Value};

use super::*;
use crate::store::Store;

fn t0() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 29, 12, 0, 0).unwrap()
}

fn secs(n: i64) -> Duration {
    Duration::seconds(n)
}

#[test]
fn countdown_start_pause_resume_complete() {
    let mut c = Countdown::default();
    assert!(!c.running());
    c.start(90.0, t0(), "focus", false);
    assert!(c.running() && c.run_id != 0);
    assert_eq!(c.remaining(t0() + secs(30)), 60.0);
    assert_eq!(c.display(t0() + secs(30)), "01:00");
    assert!((c.progress(t0() + secs(45)) - 0.5).abs() < 1e-9);

    c.pause(t0() + secs(30));
    assert!(!c.running());
    assert_eq!(c.remaining(t0() + secs(500)), 60.0); // paused time does not drain
    c.resume(t0() + secs(100));
    assert_eq!(c.remaining(t0() + secs(130)), 30.0);

    assert!(!c.complete(t0() + secs(159)));
    assert!(c.complete(t0() + secs(160)));
    assert!(!c.complete(t0() + secs(161)), "completion fires once");
    assert_eq!(c.remaining(t0() + secs(500)), 0.0);
}

#[test]
fn countdown_minimum_one_second_and_run_ids() {
    let mut c = Countdown::default();
    c.start(0.0, t0(), "", false);
    assert_eq!(c.duration_seconds, 1.0);
    let id = c.run_id;
    c.start(5.0, t0(), "", true);
    assert_eq!(c.run_id, id, "preserve_run_id keeps the run");
    c.start(5.0, t0(), "", false);
    assert_ne!(c.run_id, id);
    let before = c.run_id;
    c.reset();
    assert_ne!(c.run_id, before);
    assert!(!c.running() && c.duration_seconds == 0.0 && c.phase.is_empty());
}

#[test]
fn countdown_display_formats_hours_and_rounds_up() {
    assert_eq!(format_seconds(0.0), "00:00");
    assert_eq!(format_seconds(59.2), "01:00");
    assert_eq!(format_seconds(3599.0), "59:59");
    assert_eq!(format_seconds(3600.0), "1:00:00");
    assert_eq!(format_seconds(3725.0), "1:02:05");
    assert_eq!(format_seconds(f64::NAN.max(0.0)), "00:00");
}

#[test]
fn countdown_json_keys_match_csharp() {
    let mut c = Countdown::default();
    c.start(60.0, t0(), "Work", false);
    let v: Value = serde_json::to_value(&c).unwrap();
    let mut keys: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    assert_eq!(
        keys,
        ["Deadline", "DurationSeconds", "Phase", "RemainingSeconds"]
    );
    let idle: Value = serde_json::to_value(Countdown::default()).unwrap();
    assert_eq!(idle["Deadline"], Value::Null);
    let back: Countdown = serde_json::from_value(v).unwrap();
    assert_eq!(back.run_id, 0);
    assert_eq!(back.deadline, c.deadline);
}

#[test]
fn countdown_survives_restart() {
    let tmp = tempfile::tempdir().unwrap();
    let mut session = SessionState::default();
    session.timer.start(600.0, t0(), "", false);
    session.focus.start(1500.0, t0(), "Focus", false);
    session.focus.pause(t0() + secs(100));
    Store::open(tmp.path().to_path_buf())
        .save_session(&mut session)
        .unwrap();

    // "Restart": a new store reads the file; the app was closed for 4 minutes.
    let restarted = Store::open(tmp.path().to_path_buf())
        .load_session()
        .unwrap();
    let now = t0() + secs(240);
    assert!(restarted.timer.running());
    assert_eq!(restarted.timer.remaining(now), 360.0);
    assert_eq!(restarted.timer.display(now), "06:00");
    // The paused focus run keeps exactly what was left when it was paused.
    assert!(!restarted.focus.running());
    assert_eq!(restarted.focus.remaining(now), 1400.0);
    assert_eq!(restarted.focus.phase, "Focus");
    // A deadline that passed while closed completes on the first check.
    let mut late = restarted.timer.clone();
    assert!(late.complete(t0() + secs(601)));

    let text = std::fs::read_to_string(tmp.path().join("session.json")).unwrap();
    let raw: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(raw["Timer"]["Deadline"], json!("2026-10-29T12:10:00Z"));
    assert!(raw["Stopwatch"].get("ElapsedSeconds").is_some());
}

#[test]
fn missing_session_loads_fresh_and_unknown_keys_survive() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::open(tmp.path().to_path_buf());
    let mut s = store.load_session().unwrap();
    assert_eq!(s, SessionState::default());
    assert!(
        !tmp.path().join("session.json").exists(),
        "loading never creates the file"
    );
    std::fs::write(
        tmp.path().join("session.json"),
        r#"{"Timer":{"Phase":"x"},"FromTheFuture":[1,2]}"#,
    )
    .unwrap();
    s = store.load_session().unwrap();
    assert_eq!(s.timer.phase, "x");
    store.save_session(&mut s).unwrap();
    let raw: Value =
        serde_json::from_str(&std::fs::read_to_string(tmp.path().join("session.json")).unwrap())
            .unwrap();
    assert_eq!(raw["FromTheFuture"], json!([1, 2]));
}

#[test]
fn corrupt_session_is_not_overwritten() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("session.json"), r#"{"Timer":"nope"}"#).unwrap();
    let store = Store::open(tmp.path().to_path_buf());
    assert!(store.load_session().is_err());
    let mut fresh = SessionState::default();
    assert!(store.save_session(&mut fresh).is_err());
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("session.json")).unwrap(),
        r#"{"Timer":"nope"}"#
    );
}

struct Fake {
    wall: AtomicI64,
    steady: AtomicI64,
}

fn fake_clocks(start: DateTime<Utc>) -> (Arc<Fake>, Clocks) {
    let fake = Arc::new(Fake {
        wall: AtomicI64::new(start.timestamp_millis()),
        steady: AtomicI64::new(0),
    });
    let (w, s) = (fake.clone(), fake.clone());
    let clocks = Clocks::new(
        move || {
            Utc.timestamp_millis_opt(w.wall.load(Ordering::SeqCst))
                .unwrap()
        },
        move || s.steady.load(Ordering::SeqCst),
    );
    (fake, clocks)
}

#[test]
fn stopwatch_counts_while_running_and_freezes_when_stopped() {
    let (fake, clocks) = fake_clocks(t0());
    let mut sw = StopwatchState::default();
    sw.use_clock(clocks);
    assert_eq!(sw.display(), "00:00");
    sw.toggle();
    assert!(sw.running);
    fake.steady.store(75_000, Ordering::SeqCst);
    fake.wall.fetch_add(75_000, Ordering::SeqCst);
    assert_eq!(sw.display(), "01:15");
    sw.toggle();
    fake.steady.store(500_000, Ordering::SeqCst);
    assert_eq!(sw.display(), "01:15");
    sw.reset();
    assert_eq!(sw.display(), "00:00");
    assert_eq!(sw.utc_anchor, None);
}

#[test]
fn stopwatch_adds_time_closed_or_asleep_on_recovery() {
    // Saved while running, 2 hours before "now" on the wall clock.
    let saved = StopwatchState {
        elapsed_seconds: 30.0,
        utc_anchor: Some(t0()),
        running: true,
        ..StopwatchState::default()
    };
    let json = serde_json::to_string(&saved).unwrap();
    let mut loaded: StopwatchState = serde_json::from_str(&json).unwrap();
    let (fake, clocks) = fake_clocks(t0() + secs(7200));
    loaded.use_clock(clocks);
    assert_eq!(loaded.display(), "2:00:30");
    // Recovery is applied once; further reads follow the steady clock.
    fake.steady.store(10_000, Ordering::SeqCst);
    assert_eq!(loaded.display(), "2:00:40");
    // A stopped stopwatch ignores the gap.
    let mut stopped: StopwatchState =
        serde_json::from_str(r#"{"ElapsedSeconds":5,"UtcAnchor":null,"Running":false}"#).unwrap();
    stopped.use_clock(fake_clocks(t0()).1);
    assert_eq!(stopped.display(), "00:05");
}

#[test]
fn stopwatch_sanitizes_hostile_values() {
    let mut sw: StopwatchState =
        serde_json::from_str(r#"{"ElapsedSeconds":-50,"Running":false}"#).unwrap();
    assert_eq!(sw.elapsed(), 0.0);
    let mut huge: StopwatchState =
        serde_json::from_str(r#"{"ElapsedSeconds":1e300,"Running":false}"#).unwrap();
    assert!(huge.elapsed().is_finite());
    assert!(!huge.display().is_empty());
    // A wall clock that moved backwards never subtracts time.
    let mut back = StopwatchState {
        elapsed_seconds: 10.0,
        utc_anchor: Some(t0() + secs(100)),
        running: true,
        ..Default::default()
    };
    back.use_clock(fake_clocks(t0()).1);
    assert_eq!(back.elapsed(), 10.0);
}

#[test]
fn stopwatch_checkpoint_persists_running_progress() {
    let tmp = tempfile::tempdir().unwrap();
    let (fake, clocks) = fake_clocks(t0());
    let mut session = SessionState::default();
    session.stopwatch.use_clock(clocks);
    session.stopwatch.toggle();
    fake.steady.store(20_000, Ordering::SeqCst);
    fake.wall.fetch_add(20_000, Ordering::SeqCst);
    let store = Store::open(tmp.path().to_path_buf());
    store.save_session(&mut session).unwrap();
    let saved = store.load_session().unwrap();
    assert!(saved.stopwatch.running);
    assert_eq!(saved.stopwatch.elapsed_seconds, 20.0);
    assert_eq!(saved.stopwatch.utc_anchor, Some(t0() + secs(20)));
}
