//! Absolute-deadline countdown (C# `Countdown`). The deadline is persisted as a wall-clock UTC
//! instant, so remaining time is computed from the clock and survives a restart or sleep.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_RUN: AtomicU64 = AtomicU64::new(1);

fn next_run_id() -> u64 {
    NEXT_RUN.fetch_add(1, Ordering::Relaxed)
}

fn seconds_between(later: DateTime<Utc>, earlier: DateTime<Utc>) -> f64 {
    (later - earlier).num_milliseconds() as f64 / 1000.0
}

/// Serializes like the C# `Countdown` (`Deadline`, `RemainingSeconds`, `DurationSeconds`, `Phase`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct Timer {
    /// Identifies a run; not persisted. Changes on `start` (unless preserved) and `reset`.
    #[serde(skip)]
    pub run_id: u64,
    pub deadline: Option<DateTime<Utc>>,
    pub remaining_seconds: f64,
    pub duration_seconds: f64,
    pub phase: String,
}

impl Timer {
    pub fn running(&self) -> bool {
        self.deadline.is_some()
    }

    pub fn remaining(&self, now: DateTime<Utc>) -> f64 {
        match self.deadline {
            Some(deadline) => seconds_between(deadline, now),
            None => self.remaining_seconds,
        }
        .max(0.0)
    }

    pub fn start(&mut self, seconds: f64, now: DateTime<Utc>, phase: &str, preserve_run_id: bool) {
        if !preserve_run_id {
            self.run_id = next_run_id();
        }
        self.duration_seconds = seconds.max(1.0);
        self.remaining_seconds = self.duration_seconds;
        self.deadline =
            Some(now + chrono::Duration::milliseconds((self.remaining_seconds * 1000.0) as i64));
        self.phase = phase.to_string();
    }

    /// 0..=1; legacy sessions without a saved duration fall back to the remaining seconds.
    pub fn progress(&self, now: DateTime<Utc>) -> f64 {
        let total = if self.duration_seconds > 0.0 {
            self.duration_seconds
        } else {
            self.remaining_seconds
        };
        (1.0 - self.remaining(now) / total.max(1.0)).clamp(0.0, 1.0)
    }

    pub fn pause(&mut self, now: DateTime<Utc>) {
        self.remaining_seconds = self.remaining(now);
        self.deadline = None;
    }

    pub fn resume(&mut self, now: DateTime<Utc>) {
        if self.remaining_seconds > 0.0 {
            self.deadline = Some(
                now + chrono::Duration::milliseconds((self.remaining_seconds * 1000.0) as i64),
            );
        }
    }

    pub fn reset(&mut self) {
        self.run_id = next_run_id();
        self.deadline = None;
        self.remaining_seconds = 0.0;
        self.duration_seconds = 0.0;
        self.phase.clear();
    }

    /// True exactly once when the deadline has passed (also after sleeping past it).
    pub fn complete(&mut self, now: DateTime<Utc>) -> bool {
        match self.deadline {
            Some(deadline) if now >= deadline => {
                self.deadline = None;
                self.remaining_seconds = 0.0;
                true
            }
            _ => false,
        }
    }

    pub fn display(&self, now: DateTime<Utc>) -> String {
        format_seconds(self.remaining(now))
    }
}

/// `mm:ss`, or `h:mm:ss` from one hour up. Rounds partial seconds up.
pub fn format_seconds(seconds: f64) -> String {
    let total = if seconds.is_finite() {
        seconds.max(0.0).ceil() as u64
    } else {
        0
    };
    if total >= 3600 {
        format!("{}:{:02}:{:02}", total / 3600, total / 60 % 60, total % 60)
    } else {
        format!("{:02}:{:02}", total / 60, total % 60)
    }
}

/// Timer editor bounds: 0.1 to 1440 minutes; returns seconds.
pub fn parse_timer_minutes(text: &str) -> Option<f64> {
    let n: f64 = text.trim().parse().ok()?;
    (n.is_finite() && (0.1..=1440.0).contains(&n)).then_some(n * 60.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn t0() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 14, 12, 0, 0).unwrap()
    }
    fn secs(n: i64) -> chrono::Duration {
        chrono::Duration::seconds(n)
    }

    #[test]
    fn keeps_absolute_deadline() {
        let mut c = Timer::default();
        c.start(120.0, t0(), "", false);
        assert_eq!(c.remaining(t0() + secs(70)), 50.0);
    }

    #[test]
    fn pause_resume_preserves_remaining() {
        let mut c = Timer::default();
        c.start(120.0, t0(), "", false);
        c.pause(t0() + secs(30));
        let later = t0() + chrono::Duration::hours(1);
        assert_eq!(c.remaining(later), 90.0);
        c.resume(later);
        assert_eq!(c.remaining(later + secs(20)), 70.0);
    }

    #[test]
    fn progress_survives_pause_and_restart() {
        let mut c = Timer::default();
        c.start(120.0, t0(), "", false);
        assert!(
            (c.progress(t0() + chrono::Duration::milliseconds(30_500)) - 30.5 / 120.0).abs() < 1e-4
        );
        c.pause(t0() + secs(30));
        let mut c: Timer = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        let later = t0() + chrono::Duration::hours(1);
        assert_eq!(c.progress(later), 0.25);
        c.resume(later);
        assert_eq!(c.progress(later + secs(30)), 0.5);
    }

    #[test]
    fn legacy_session_progress_is_bounded() {
        let c = Timer {
            remaining_seconds: 60.0,
            deadline: Some(t0() + secs(30)),
            ..Default::default()
        };
        assert_eq!(c.progress(t0()), 0.5);
        assert_eq!(c.progress(t0() + chrono::Duration::hours(2)), 1.0);
    }

    #[test]
    fn sleep_past_deadline_completes_once() {
        let mut c = Timer::default();
        c.start(60.0, t0(), "", false);
        assert!(c.complete(t0() + chrono::Duration::hours(2)));
        assert!(!c.complete(t0() + chrono::Duration::hours(3)));
        assert_eq!(c.remaining(t0() + chrono::Duration::hours(3)), 0.0);
    }

    #[test]
    fn reset_clears_state() {
        let mut c = Timer::default();
        c.start(60.0, t0(), "work", false);
        c.reset();
        assert!(!c.running() && c.remaining_seconds == 0.0 && c.phase.is_empty());
    }

    #[test]
    fn display_handles_hours() {
        let mut c = Timer::default();
        c.start(3661.0, t0(), "", false);
        assert_eq!(c.display(t0()), "1:01:01");
        assert_eq!(format_seconds(59.2), "01:00");
        assert_eq!(format_seconds(f64::NAN), "00:00");
    }

    #[test]
    fn countdown_survives_restart() {
        let mut c = Timer::default();
        c.start(300.0, t0(), "work", false);
        let json = serde_json::to_string(&c).unwrap();
        assert!(json.contains("\"Deadline\"") && json.contains("\"RemainingSeconds\""));
        let restored: Timer = serde_json::from_str(&json).unwrap();
        assert!(restored.running());
        assert_eq!(restored.remaining(t0() + secs(100)), 200.0);
        // C# writes offsets such as +07:00; they normalize to the same instant.
        let csharp = r#"{"Deadline":"2026-10-14T19:05:00+07:00","RemainingSeconds":300,"DurationSeconds":300,"Phase":"work"}"#;
        let from_csharp: Timer = serde_json::from_str(csharp).unwrap();
        assert_eq!(from_csharp.remaining(t0()), 300.0);
    }

    #[test]
    fn timer_minutes_bounds() {
        assert_eq!(parse_timer_minutes("5"), Some(300.0));
        assert_eq!(parse_timer_minutes("0.1"), Some(6.0));
        assert_eq!(parse_timer_minutes("0.05"), None);
        assert_eq!(parse_timer_minutes("1441"), None);
        assert_eq!(parse_timer_minutes("abc"), None);
    }
}
