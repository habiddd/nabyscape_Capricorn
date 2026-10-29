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
fn countdown_survives_restart() {
    let tmp = tempfile::tempdir().unwrap();
    let mut session = SessionState::default();
    session.timer.start(600.0, t0(), "", false);
    session.focus.countdown.start(1500.0, t0(), "Focus", false);
    session.focus.countdown.pause(t0() + secs(100));
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
    assert_eq!(restarted.focus.countdown.remaining(now), 1400.0);
    assert_eq!(restarted.focus.countdown.phase, "Focus");
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

#[test]
fn stopwatch_checkpoint_persists_running_progress() {
    let tmp = tempfile::tempdir().unwrap();
    let mut session = SessionState::default();
    let start = Clock {
        wall: t0(),
        steady_ms: 0,
    };
    session.stopwatch.toggle(start);
    let later = Clock {
        wall: t0() + secs(20),
        steady_ms: 20_000,
    };
    let store = Store::open(tmp.path().to_path_buf());
    store.save_session_at(&mut session, later).unwrap();
    let saved = store.load_session().unwrap();
    assert!(saved.stopwatch.running);
    assert_eq!(saved.stopwatch.elapsed_seconds, 20.0);
    assert_eq!(saved.stopwatch.utc_anchor, Some(t0() + secs(20)));
}

#[test]
fn focus_session_is_a_transparent_countdown() {
    let mut session = SessionState::default();
    session.focus.start_work(Default::default(), t0());
    let v = serde_json::to_value(&session).unwrap();
    assert_eq!(v["Focus"]["Phase"], json!("work"));
    assert_eq!(v["Focus"]["DurationSeconds"], json!(1500.0));
    let back: SessionState = serde_json::from_value(v).unwrap();
    assert_eq!(back.focus.countdown.phase, "work");
}
