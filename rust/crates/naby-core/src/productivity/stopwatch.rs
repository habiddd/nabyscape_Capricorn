//! Stopwatch that survives restarts and sleep (C# `StopwatchState`).
//!
//! While running, elapsed time follows the steady clock (Windows uptime, which keeps counting
//! during suspend) so wall-clock jumps never change it. Across a restart the persisted UTC anchor
//! restores the time spent while the app was closed, never subtracting when the clock went back.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::{Arc, OnceLock};
use std::time::Instant;

/// The two clocks the stopwatch reads, supplied by the host so tests can fake them.
#[derive(Debug, Clone, Copy)]
pub struct Clock {
    pub wall: DateTime<Utc>,
    /// Monotonic milliseconds that include suspend time (C# `Environment.TickCount64`).
    pub steady_ms: i64,
}

impl Clock {
    /// The system clocks. The steady clock counts from first use in this process.
    pub fn system() -> Self {
        static START: OnceLock<Instant> = OnceLock::new();
        let start = START.get_or_init(Instant::now);
        Clock {
            wall: Utc::now(),
            steady_ms: start.elapsed().as_millis() as i64,
        }
    }
}

/// Source of [`Clock`] readings; replaceable for tests or a host tick source (e.g. Windows
/// uptime, which keeps counting during suspend).
#[derive(Clone)]
pub struct Clocks {
    wall: Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>,
    steady_ms: Arc<dyn Fn() -> i64 + Send + Sync>,
}

impl Clocks {
    pub fn new(
        wall: impl Fn() -> DateTime<Utc> + Send + Sync + 'static,
        steady_ms: impl Fn() -> i64 + Send + Sync + 'static,
    ) -> Self {
        Clocks {
            wall: Arc::new(wall),
            steady_ms: Arc::new(steady_ms),
        }
    }

    /// Reads both clocks now.
    pub fn now(&self) -> Clock {
        Clock {
            wall: (self.wall)(),
            steady_ms: (self.steady_ms)(),
        }
    }
}

impl Default for Clocks {
    fn default() -> Self {
        Clocks::new(|| Clock::system().wall, || Clock::system().steady_ms)
    }
}

impl fmt::Debug for Clocks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Clocks")
    }
}

/// The C# `StopwatchState`. JSON keys: `ElapsedSeconds`, `UtcAnchor`, `Running`.
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

/// The C# `StopwatchState` name.
pub type StopwatchState = Stopwatch;

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone};
    use std::sync::atomic::{AtomicI64, Ordering};

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

    fn t0() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 29, 12, 0, 0).unwrap()
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
    fn counts_while_running_and_freezes_when_stopped() {
        let (fake, clocks) = fake_clocks(t0());
        let mut sw = Stopwatch::default();
        assert_eq!(sw.display(clocks.now()), "00:00");
        sw.toggle(clocks.now());
        assert!(sw.running);
        fake.steady.store(75_000, Ordering::SeqCst);
        fake.wall.fetch_add(75_000, Ordering::SeqCst);
        assert_eq!(sw.display(clocks.now()), "01:15");
        sw.toggle(clocks.now());
        fake.steady.store(500_000, Ordering::SeqCst);
        assert_eq!(sw.display(clocks.now()), "01:15");
        sw.reset(clocks.now());
        assert_eq!(sw.display(clocks.now()), "00:00");
        assert_eq!(sw.utc_anchor, None);
    }

    #[test]
    fn adds_time_closed_or_asleep_on_recovery() {
        let saved = Stopwatch {
            elapsed_seconds: 30.0,
            utc_anchor: Some(t0()),
            running: true,
            ..Stopwatch::default()
        };
        let json = serde_json::to_string(&saved).unwrap();
        let mut loaded: Stopwatch = serde_json::from_str(&json).unwrap();
        let (fake, clocks) = fake_clocks(t0() + Duration::seconds(7200));
        assert_eq!(loaded.display(clocks.now()), "2:00:30");
        // Recovery is applied once; further reads follow the steady clock.
        fake.steady.store(10_000, Ordering::SeqCst);
        assert_eq!(loaded.display(clocks.now()), "2:00:40");
        // A stopped stopwatch ignores the gap.
        let mut stopped: Stopwatch =
            serde_json::from_str(r#"{"ElapsedSeconds":5,"UtcAnchor":null,"Running":false}"#)
                .unwrap();
        assert_eq!(stopped.display(fake_clocks(t0()).1.now()), "00:05");
    }

    #[test]
    fn sanitizes_hostile_values() {
        let clocks = fake_clocks(t0()).1;
        let mut sw: Stopwatch =
            serde_json::from_str(r#"{"ElapsedSeconds":-50,"Running":false}"#).unwrap();
        assert_eq!(sw.elapsed(clocks.now()), 0.0);
        let mut huge: Stopwatch =
            serde_json::from_str(r#"{"ElapsedSeconds":1e300,"Running":false}"#).unwrap();
        assert!(huge.elapsed(clocks.now()).is_finite());
        assert!(!huge.display(clocks.now()).is_empty());
        let mut back = Stopwatch {
            elapsed_seconds: 10.0,
            utc_anchor: Some(t0() + Duration::seconds(100)),
            running: true,
            ..Default::default()
        };
        assert_eq!(back.elapsed(clocks.now()), 10.0);
    }

    #[test]
    fn system_clock_is_monotonic() {
        let a = Clock::system();
        let b = Clock::system();
        assert!(b.steady_ms >= a.steady_ms);
        let _ = Clocks::default().now();
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
