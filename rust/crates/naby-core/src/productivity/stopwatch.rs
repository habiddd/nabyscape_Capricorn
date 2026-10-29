//! Stopwatch that survives restarts and sleep (C# `StopwatchState`).
//!
//! While running, elapsed time follows the steady clock (Windows uptime, which keeps counting
//! during suspend) so wall-clock jumps never change it. Across a restart the persisted UTC anchor
//! restores the time spent while the app was closed, never subtracting when the clock went back.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// The two clocks the stopwatch reads, supplied by the host so tests can fake them.
#[derive(Debug, Clone, Copy)]
pub struct Clock {
    pub wall: DateTime<Utc>,
    /// Monotonic milliseconds that include suspend time (C# `Environment.TickCount64`).
    pub steady_ms: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct Stopwatch {
    pub elapsed_seconds: f64,
    pub utc_anchor: Option<DateTime<Utc>>,
    pub running: bool,
    #[serde(skip)]
    stamp: i64,
    #[serde(skip)]
    recovered: bool,
}

const MAX_SECONDS: f64 = i64::MAX as f64 / 1e7 / 2.0;

impl Stopwatch {
    /// Applies the persisted state once per instance; later calls do nothing.
    pub fn recover(&mut self, clock: Clock) {
        if self.recovered {
            return;
        }
        self.elapsed_seconds = if self.elapsed_seconds.is_finite() {
            self.elapsed_seconds.clamp(0.0, MAX_SECONDS)
        } else {
            0.0
        };
        if let (true, Some(anchor)) = (self.running, self.utc_anchor) {
            let away = (clock.wall - anchor).num_milliseconds() as f64 / 1000.0;
            self.elapsed_seconds += away.max(0.0);
        }
        self.stamp = clock.steady_ms;
        self.recovered = true;
        self.utc_anchor = self.running.then_some(clock.wall);
    }

    pub fn elapsed(&mut self, clock: Clock) -> f64 {
        self.recover(clock);
        let running = if self.running {
            ((clock.steady_ms - self.stamp) as f64 / 1000.0).max(0.0)
        } else {
            0.0
        };
        self.elapsed_seconds + running
    }

    /// `mm:ss`, or `h:mm:ss` from one hour.
    pub fn display(&mut self, clock: Clock) -> String {
        let total = self.elapsed(clock).floor().min(i64::MAX as f64 / 2.0) as i64;
        if total >= 3600 {
            format!("{}:{:02}:{:02}", total / 3600, total / 60 % 60, total % 60)
        } else {
            format!("{:02}:{:02}", total / 60, total % 60)
        }
    }

    /// Folds the running time into `elapsed_seconds` and refreshes the anchor; call before saving.
    pub fn checkpoint(&mut self, clock: Clock) {
        self.elapsed_seconds = self.elapsed(clock);
        self.stamp = clock.steady_ms;
        self.utc_anchor = self.running.then_some(clock.wall);
    }

    pub fn toggle(&mut self, clock: Clock) {
        self.checkpoint(clock);
        self.running = !self.running;
        self.utc_anchor = self.running.then_some(clock.wall);
    }

    pub fn reset(&mut self, clock: Clock) {
        self.elapsed_seconds = 0.0;
        self.running = false;
        self.utc_anchor = None;
        self.stamp = clock.steady_ms;
        self.recovered = true;
    }

    /// Label of the primary button.
    pub fn action_label(&mut self, clock: Clock) -> &'static str {
        if self.running {
            "Pause"
        } else if self.elapsed(clock) > 0.0 {
            "Resume"
        } else {
            "Start"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone};

    fn clock(wall: DateTime<Utc>, steady_ms: i64) -> Clock {
        Clock { wall, steady_ms }
    }

    #[test]
    fn stopwatch_survives_sleep() {
        let mut wall = Utc.with_ymd_and_hms(2026, 10, 17, 0, 0, 0).unwrap();
        let mut steady = 1000;
        let mut watch = Stopwatch::default();
        watch.toggle(clock(wall, steady));

        // Running elapsed follows the monotonic clock even when the wall clock moves backwards.
        steady += 65_000;
        wall -= Duration::seconds(100);
        assert_eq!(watch.elapsed(clock(wall, steady)), 65.0);
        assert_eq!(watch.display(clock(wall, steady)), "01:05");

        // Pause freezes elapsed across sleep.
        watch.toggle(clock(wall, steady));
        steady += 4000;
        wall += Duration::hours(1);
        assert_eq!(watch.elapsed(clock(wall, steady)), 65.0);
        assert!(!watch.running);

        // Uptime includes sleep while running; hours format.
        watch.toggle(clock(wall, steady));
        steady += 3_600_000;
        wall += Duration::hours(1);
        watch.checkpoint(clock(wall, steady));
        assert_eq!(watch.elapsed(clock(wall, steady)), 3665.0);
        assert_eq!(watch.display(clock(wall, steady)), "1:01:05");

        // A persisted running stopwatch recovers the UTC time spent closed, exactly once.
        let json = serde_json::to_string(&watch).unwrap();
        let mut recovered: Stopwatch = serde_json::from_str(&json).unwrap();
        let later = wall + Duration::seconds(25);
        recovered.recover(clock(later, steady));
        assert_eq!(recovered.elapsed(clock(later, steady)), 3690.0);
        recovered.recover(clock(later, steady));
        assert_eq!(recovered.elapsed(clock(later, steady)), 3690.0);

        // A clock that went backwards never subtracts.
        let mut backwards: Stopwatch = serde_json::from_str(&json).unwrap();
        let earlier = wall - Duration::days(2);
        backwards.recover(clock(earlier, steady));
        assert_eq!(backwards.elapsed(clock(earlier, steady)), 3665.0);

        // Reset clears everything.
        watch.reset(clock(wall, steady));
        assert_eq!(watch.elapsed(clock(wall, steady)), 0.0);
        assert!(!watch.running && watch.utc_anchor.is_none());
        assert_eq!(watch.display(clock(wall, steady)), "00:00");
    }

    #[test]
    fn corrupt_elapsed_recovers_safely() {
        let now = Utc.with_ymd_and_hms(2026, 10, 17, 0, 0, 0).unwrap();
        let mut watch = Stopwatch {
            elapsed_seconds: f64::NAN,
            ..Default::default()
        };
        watch.recover(clock(now, 0));
        assert_eq!(watch.elapsed(clock(now, 0)), 0.0);
        let mut negative = Stopwatch {
            elapsed_seconds: -5.0,
            ..Default::default()
        };
        assert_eq!(negative.elapsed(clock(now, 0)), 0.0);
    }

    #[test]
    fn serializes_with_csharp_keys() {
        let now = Utc.with_ymd_and_hms(2026, 10, 17, 0, 0, 0).unwrap();
        let mut watch = Stopwatch::default();
        watch.toggle(clock(now, 0));
        let value = serde_json::to_value(&watch).unwrap();
        assert_eq!(value["Running"], true);
        assert!(value.get("ElapsedSeconds").is_some() && value.get("UtcAnchor").is_some());
        let legacy: Stopwatch =
            serde_json::from_str(r#"{"ElapsedSeconds":12.5,"UtcAnchor":null,"Running":false}"#)
                .unwrap();
        assert_eq!(legacy.elapsed_seconds, 12.5);
    }

    #[test]
    fn action_label_follows_state() {
        let now = Utc.with_ymd_and_hms(2026, 10, 17, 0, 0, 0).unwrap();
        let mut watch = Stopwatch::default();
        assert_eq!(watch.action_label(clock(now, 0)), "Start");
        watch.toggle(clock(now, 0));
        assert_eq!(watch.action_label(clock(now, 1000)), "Pause");
        watch.toggle(clock(now, 5000));
        assert_eq!(watch.action_label(clock(now, 5000)), "Resume");
    }
}
