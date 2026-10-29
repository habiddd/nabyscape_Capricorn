//! Timer, focus and stopwatch state persisted in the `session` document.
//!
//! Deadlines are stored as absolute UTC instants so a running countdown survives a restart or
//! sleep: the remaining time is always recomputed from the wall clock.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::store::{Store, StoreError};

/// Name of the session document.
pub const SESSION_DOCUMENT: &str = "session";

static NEXT_RUN_ID: AtomicU64 = AtomicU64::new(1);

fn new_run_id() -> u64 {
    NEXT_RUN_ID.fetch_add(1, Ordering::Relaxed)
}

fn secs(seconds: f64) -> Duration {
    // Duration::try_milliseconds fails on overflow; clamp to something representable.
    Duration::try_milliseconds((seconds * 1000.0).clamp(-9.0e15, 9.0e15) as i64)
        .unwrap_or(Duration::zero())
}

/// A pausable countdown. JSON keys: `Deadline`, `RemainingSeconds`, `DurationSeconds`, `Phase`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase", default)]
pub struct Countdown {
    /// Identifies the current run; not persisted (0 after load).
    #[serde(skip)]
    pub run_id: u64,
    pub deadline: Option<DateTime<Utc>>,
    pub remaining_seconds: f64,
    pub duration_seconds: f64,
    pub phase: String,
}

impl Countdown {
    pub fn running(&self) -> bool {
        self.deadline.is_some()
    }

    /// Seconds left at `now` (never negative).
    pub fn remaining(&self, now: DateTime<Utc>) -> f64 {
        let value = match self.deadline {
            Some(deadline) => (deadline - now).num_milliseconds() as f64 / 1000.0,
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
            self.run_id = new_run_id();
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

    /// Fraction elapsed, 0..=1.
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
        self.run_id = new_run_id();
        self.deadline = None;
        self.remaining_seconds = 0.0;
        self.duration_seconds = 0.0;
        self.phase.clear();
    }

    /// True exactly once when the deadline has passed.
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

    /// `mm:ss`, or `h:mm:ss` from one hour up (rounded up to the next second).
    pub fn display(&self, now: DateTime<Utc>) -> String {
        format_seconds(self.remaining(now))
    }
}

/// C# `TimeSpan` formatting: `h\:mm\:ss` (hour component 0-23) from one hour, else `mm\:ss`.
pub fn format_seconds(seconds: f64) -> String {
    let total = seconds.max(0.0).ceil().min(9.0e11) as i64;
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

/// Wall and monotonic clocks for [`StopwatchState`]; replaceable for tests or a host tick source.
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
}

impl Default for Clocks {
    fn default() -> Self {
        let start = Instant::now();
        Clocks::new(Utc::now, move || start.elapsed().as_millis() as i64)
    }
}

impl fmt::Debug for Clocks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Clocks")
    }
}

impl PartialEq for Clocks {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

/// Stopwatch that keeps counting across restarts: while running, `UtcAnchor` records when the
/// persisted `ElapsedSeconds` was last checkpointed, and `recover` adds the gap on load.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase", default)]
pub struct StopwatchState {
    pub elapsed_seconds: f64,
    pub utc_anchor: Option<DateTime<Utc>>,
    pub running: bool,
    #[serde(skip)]
    clocks: Clocks,
    #[serde(skip)]
    stamp: i64,
    #[serde(skip)]
    recovered: bool,
}

impl StopwatchState {
    pub fn use_clock(&mut self, clocks: Clocks) {
        self.clocks = clocks;
        self.recovered = false;
    }

    fn wall(&self) -> DateTime<Utc> {
        (self.clocks.wall)()
    }

    fn steady(&self) -> i64 {
        (self.clocks.steady_ms)()
    }

    /// Folds the time since the persisted anchor into `elapsed_seconds`, once per load.
    pub fn recover(&mut self) {
        if self.recovered {
            return;
        }
        self.elapsed_seconds = if self.elapsed_seconds.is_finite() {
            self.elapsed_seconds.clamp(0.0, 4.6e11)
        } else {
            0.0
        };
        if let (true, Some(anchor)) = (self.running, self.utc_anchor) {
            self.elapsed_seconds +=
                ((self.wall() - anchor).num_milliseconds() as f64 / 1000.0).max(0.0);
        }
        self.stamp = self.steady();
        self.recovered = true;
        self.utc_anchor = self.running.then(|| self.wall());
    }

    /// Total elapsed seconds now.
    pub fn elapsed(&mut self) -> f64 {
        self.recover();
        let live = if self.running {
            ((self.steady() - self.stamp) as f64 / 1000.0).max(0.0)
        } else {
            0.0
        };
        self.elapsed_seconds + live
    }

    /// `mm:ss`, or `h:mm:ss` (unbounded hours) from one hour.
    pub fn display(&mut self) -> String {
        let total = self.elapsed().floor().min(4.6e18) as i64;
        if total >= 3600 {
            format!("{}:{:02}:{:02}", total / 3600, total / 60 % 60, total % 60)
        } else {
            format!("{:02}:{:02}", total / 60, total % 60)
        }
    }

    /// Persists progress so far; call before saving.
    pub fn checkpoint(&mut self) {
        self.elapsed_seconds = self.elapsed();
        self.stamp = self.steady();
        self.utc_anchor = self.running.then(|| self.wall());
    }

    pub fn toggle(&mut self) {
        self.checkpoint();
        self.running = !self.running;
        self.utc_anchor = self.running.then(|| self.wall());
    }

    pub fn reset(&mut self) {
        self.elapsed_seconds = 0.0;
        self.running = false;
        self.utc_anchor = None;
        self.stamp = self.steady();
        self.recovered = true;
    }
}

/// The `session` document. JSON keys: `Stopwatch`, `Timer`, `Focus`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase", default)]
pub struct SessionState {
    pub stopwatch: StopwatchState,
    pub timer: Countdown,
    pub focus: Countdown,
}

impl Store {
    /// Loads the session, or a fresh one when none was saved yet.
    pub fn load_session(&self) -> Result<SessionState, StoreError> {
        self.load_or_default(SESSION_DOCUMENT)
    }

    /// Checkpoints the stopwatch (so a running one carries a fresh anchor) and saves the session.
    pub fn save_session(&self, session: &mut SessionState) -> Result<(), StoreError> {
        session.stopwatch.checkpoint();
        self.save(SESSION_DOCUMENT, session)
    }
}

#[cfg(test)]
mod tests;
