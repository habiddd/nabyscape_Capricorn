//! Absolute-deadline countdown (C# `Countdown`). The deadline is persisted as a wall-clock UTC
//! instant, so remaining time is computed from the clock and survives a restart or sleep.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_RUN: AtomicU64 = AtomicU64::new(1);

fn next_run_id() -> u64 {
    NEXT_RUN.fetch_add(1, Ordering::Relaxed)
}

fn secs(seconds: f64) -> chrono::Duration {
    // Duration::try_milliseconds fails on overflow; clamp to something representable.
    chrono::Duration::try_milliseconds((seconds * 1000.0).clamp(-9.0e15, 9.0e15) as i64)
        .unwrap_or_else(chrono::Duration::zero)
}

fn seconds_between(later: DateTime<Utc>, earlier: DateTime<Utc>) -> f64 {
    (later - earlier).num_milliseconds() as f64 / 1000.0
}

/// The C# `Countdown` name (timer and focus runs share it).
pub type Countdown = Timer;

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

    /// Seconds left at `now` (never negative; non-finite persisted values count as zero).
    pub fn remaining(&self, now: DateTime<Utc>) -> f64 {
        let value = match self.deadline {
            Some(deadline) => seconds_between(deadline, now),
            None => self.remaining_seconds,
        };
        if value.is_finite() {
            value.max(0.0)
        } else {
            0.0
        }
    }

    pub fn start(&mut self, seconds: f64, now: DateTime<Utc>, phase: &str, preserve_run_id: bool) {
        if !preserve_run_id {
            self.run_id = next_run_id();
        }
        let seconds = if seconds.is_finite() {
            seconds.max(1.0)
        } else {
            1.0
        };
        self.duration_seconds = seconds;
        self.remaining_seconds = seconds;
        self.deadline = now.checked_add_signed(secs(seconds));
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
            self.deadline = now.checked_add_signed(secs(self.remaining_seconds));
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

/// C# `TimeSpan` formatting (rounded up to the next second): `h\:mm\:ss` with the hour component
/// 0-23 from one hour up, else `mm\:ss`.
pub fn format_seconds(seconds: f64) -> String {
    let total = if seconds.is_finite() {
        seconds.max(0.0).ceil().min(9.0e11) as u64
    } else {
        0
    };
    if total >= 3600 {
        format!(
            "{}:{:02}:{:02}",
            total / 3600 % 24,
            total / 60 % 60,
            total % 60
        )
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
    fn start_pause_resume_complete() {
        let mut c = Timer::default();
        assert!(!c.running());
        c.start(90.0, t0(), "focus", false);
        assert!(c.running() && c.run_id != 0);
        assert_eq!(c.remaining(t0() + secs(30)), 60.0);
        assert_eq!(c.display(t0() + secs(30)), "01:00");
        assert!((c.progress(t0() + secs(45)) - 0.5).abs() < 1e-9);

        c.pause(t0() + secs(30));
        assert!(!c.running());
        assert_eq!(c.remaining(t0() + secs(500)), 60.0);
        c.resume(t0() + secs(100));
        assert_eq!(c.remaining(t0() + secs(130)), 30.0);

        assert!(!c.complete(t0() + secs(159)));
        assert!(c.complete(t0() + secs(160)));
        assert!(!c.complete(t0() + secs(161)), "completion fires once");
        assert_eq!(c.remaining(t0() + secs(500)), 0.0);
    }

    #[test]
    fn minimum_one_second_and_run_ids() {
        let mut c = Timer::default();
        c.start(0.0, t0(), "", false);
        assert_eq!(c.duration_seconds, 1.0);
        c.start(f64::NAN, t0(), "", false);
        assert_eq!(c.duration_seconds, 1.0);
        c.start(f64::INFINITY, t0(), "", false);
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
    fn format_matches_timespan() {
        assert_eq!(format_seconds(0.0), "00:00");
        assert_eq!(format_seconds(59.2), "01:00");
        assert_eq!(format_seconds(3599.0), "59:59");
        assert_eq!(format_seconds(3600.0), "1:00:00");
        assert_eq!(format_seconds(3725.0), "1:02:05");
        // h in the C# format is the 0-23 hour component.
        assert_eq!(format_seconds(86_400.0 + 61.0), "0:01:01");
    }

    #[test]
    fn json_keys_match_csharp() {
        let mut c = Timer::default();
        c.start(60.0, t0(), "Work", false);
        let v = serde_json::to_value(&c).unwrap();
        let mut keys: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            ["Deadline", "DurationSeconds", "Phase", "RemainingSeconds"]
        );
        let idle = serde_json::to_value(Timer::default()).unwrap();
        assert_eq!(idle["Deadline"], serde_json::Value::Null);
        let back: Timer = serde_json::from_value(v).unwrap();
        assert_eq!(back.run_id, 0);
        assert_eq!(back.deadline, c.deadline);
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
