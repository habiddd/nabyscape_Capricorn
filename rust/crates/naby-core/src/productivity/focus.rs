//! Focus session: a work phase followed by a break (C# `App.CheckCountdowns` and the focus half of
//! `CountdownView`). Durations come from `FocusConfig`; the host maps its settings onto it.

use super::timer::Timer;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

pub const WORK: &str = "work";
pub const BREAK: &str = "break";

/// The two settings the focus session reads (C# `Settings.FocusMinutes` / `BreakMinutes`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FocusConfig {
    pub focus_minutes: u32,
    pub break_minutes: u32,
}

impl Default for FocusConfig {
    fn default() -> Self {
        Self {
            focus_minutes: 25,
            break_minutes: 5,
        }
    }
}

impl FocusConfig {
    /// Settings normalization: focus 1-240, break 1-120.
    pub fn clamped(self) -> Self {
        Self {
            focus_minutes: self.focus_minutes.clamp(1, 240),
            break_minutes: self.break_minutes.clamp(1, 120),
        }
    }

    /// Editor validation: both fields whole minutes within range.
    pub fn parse(focus: &str, rest: &str) -> Option<Self> {
        let focus_minutes: u32 = focus.trim().parse().ok()?;
        let break_minutes: u32 = rest.trim().parse().ok()?;
        ((1..=240).contains(&focus_minutes) && (1..=120).contains(&break_minutes)).then_some(Self {
            focus_minutes,
            break_minutes,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusEvent {
    /// Work finished and the break started ("Focus complete").
    BreakStarted,
    /// Work and the whole break elapsed while the app slept ("Focus session complete").
    SessionComplete,
    /// The break ended ("Break finished").
    BreakFinished,
}

/// Persists exactly like the C# `Countdown` stored under `SessionState.Focus`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FocusSession {
    pub countdown: Timer,
}

impl FocusSession {
    pub fn is_break(&self) -> bool {
        self.countdown.phase == BREAK
    }

    pub fn running(&self) -> bool {
        self.countdown.running()
    }

    /// Anything left on the clock, running or paused.
    pub fn active(&self, now: DateTime<Utc>) -> bool {
        self.countdown.remaining(now) > 0.0
    }

    pub fn start_work(&mut self, config: FocusConfig, now: DateTime<Utc>) {
        let seconds = f64::from(config.clamped().focus_minutes) * 60.0;
        self.countdown.start(seconds, now, WORK, false);
    }

    /// The single primary action of the view: pause when running, resume when paused, otherwise
    /// start a new work phase.
    pub fn toggle(&mut self, config: FocusConfig, now: DateTime<Utc>) {
        if self.countdown.running() {
            self.countdown.pause(now);
        } else if self.countdown.remaining(now) > 0.0 {
            self.countdown.resume(now);
        } else {
            self.start_work(config, now);
        }
    }

    pub fn reset(&mut self) {
        self.countdown.reset();
    }

    /// Status text announced by the view.
    pub fn status(&self, now: DateTime<Utc>) -> &'static str {
        let active = self.active(now);
        if active {
            if !self.running() {
                "Paused"
            } else if self.is_break() {
                "Break"
            } else {
                "Focusing"
            }
        } else if self.countdown.duration_seconds > 0.0 {
            "Complete"
        } else {
            "Ready"
        }
    }

    /// Advances the cycle when the deadline passed. The break is anchored to the actual work
    /// deadline, so sleeping through the work phase never grants a fresh full break.
    pub fn tick(&mut self, config: FocusConfig, now: DateTime<Utc>) -> Option<FocusEvent> {
        let deadline = self.countdown.deadline;
        if !self.countdown.complete(now) {
            return None;
        }
        if self.countdown.phase == WORK {
            let break_seconds = f64::from(config.clamped().break_minutes) * 60.0;
            let end = deadline.unwrap_or(now) + Duration::seconds(break_seconds as i64);
            if end > now {
                let remaining = (end - now).num_milliseconds() as f64 / 1000.0;
                self.countdown.start(remaining, now, BREAK, true);
                self.countdown.duration_seconds = break_seconds;
                Some(FocusEvent::BreakStarted)
            } else {
                self.countdown.reset();
                Some(FocusEvent::SessionComplete)
            }
        } else {
            self.countdown.reset();
            Some(FocusEvent::BreakFinished)
        }
    }
}

/// What a countdown check produced, for the host to notify about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CountdownReport {
    pub timer_finished: bool,
    pub focus: Option<FocusEvent>,
}

impl CountdownReport {
    pub fn changed(&self) -> bool {
        self.timer_finished || self.focus.is_some()
    }
}

/// C# `App.CheckCountdowns`: completes the timer and advances the focus cycle.
pub fn check_countdowns(
    timer: &mut Timer,
    focus: &mut FocusSession,
    config: FocusConfig,
    now: DateTime<Utc>,
) -> CountdownReport {
    CountdownReport {
        timer_finished: timer.complete(now),
        focus: focus.tick(config, now),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn t0() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 14, 12, 0, 0).unwrap()
    }
    const CFG: FocusConfig = FocusConfig {
        focus_minutes: 25,
        break_minutes: 5,
    };

    #[test]
    fn focus_cycles_work_break() {
        let mut f = FocusSession::default();
        f.start_work(CFG, t0());
        assert!(f.running() && f.countdown.phase == WORK && f.countdown.duration_seconds == 1500.0);
        assert_eq!(f.status(t0()), "Focusing");
        assert_eq!(f.tick(CFG, t0() + Duration::minutes(24)), None);

        let run = f.countdown.run_id;
        let work_end = t0() + Duration::minutes(25);
        assert_eq!(
            f.tick(CFG, work_end + Duration::seconds(2)),
            Some(FocusEvent::BreakStarted)
        );
        assert!(f.running() && f.is_break() && f.countdown.run_id == run);
        assert_eq!(f.countdown.duration_seconds, 300.0);
        assert_eq!(
            f.countdown.remaining(work_end + Duration::seconds(2)),
            298.0
        );
        assert_eq!(f.status(work_end), "Break");

        let break_end = work_end + Duration::minutes(5);
        assert_eq!(
            f.tick(CFG, break_end + Duration::seconds(1)),
            Some(FocusEvent::BreakFinished)
        );
        assert!(!f.running() && f.countdown.phase.is_empty());
        assert_eq!(f.status(break_end), "Ready");
    }

    #[test]
    fn sleeping_past_work_and_break_completes_session() {
        let mut f = FocusSession::default();
        f.countdown
            .start(1.0, t0() - Duration::hours(2), WORK, false);
        assert_eq!(f.tick(CFG, t0()), Some(FocusEvent::SessionComplete));
        assert!(!f.running() && f.countdown.phase.is_empty());
    }

    #[test]
    fn work_shortly_expired_enters_remaining_break() {
        let mut f = FocusSession::default();
        f.countdown
            .start(1.0, t0() - Duration::seconds(2), WORK, false);
        assert_eq!(f.tick(CFG, t0()), Some(FocusEvent::BreakStarted));
        assert!(f.running() && f.is_break());
    }

    #[test]
    fn break_expiry_returns_to_ready() {
        let mut f = FocusSession::default();
        f.countdown
            .start(1.0, t0() - Duration::seconds(2), BREAK, false);
        assert_eq!(f.tick(CFG, t0()), Some(FocusEvent::BreakFinished));
        assert!(!f.running() && f.countdown.phase.is_empty());
    }

    #[test]
    fn toggle_pauses_resumes_and_starts() {
        let mut f = FocusSession::default();
        f.toggle(CFG, t0());
        assert!(f.running());
        f.toggle(CFG, t0() + Duration::minutes(1));
        assert!(!f.running() && f.active(t0()));
        assert_eq!(f.status(t0()), "Paused");
        f.toggle(CFG, t0() + Duration::minutes(10));
        assert!(f.running());
        assert_eq!(f.countdown.remaining(t0() + Duration::minutes(10)), 1440.0);
    }

    #[test]
    fn check_countdowns_reports_timer_and_focus() {
        let mut timer = Timer::default();
        timer.start(2.0, t0(), "", false);
        let mut focus = FocusSession::default();
        let r = check_countdowns(&mut timer, &mut focus, CFG, t0() + Duration::seconds(1));
        assert!(!r.changed());
        let r = check_countdowns(&mut timer, &mut focus, CFG, t0() + Duration::seconds(3));
        assert!(r.timer_finished && r.focus.is_none() && !timer.running());
    }

    #[test]
    fn config_validation() {
        assert_eq!(FocusConfig::parse("25", "5"), Some(CFG));
        assert_eq!(FocusConfig::parse("0", "5"), None);
        assert_eq!(FocusConfig::parse("25", "121"), None);
        assert_eq!(FocusConfig::parse("2.5", "5"), None);
        assert_eq!(
            FocusConfig {
                focus_minutes: 999,
                break_minutes: 0
            }
            .clamped(),
            FocusConfig {
                focus_minutes: 240,
                break_minutes: 1
            }
        );
    }

    #[test]
    fn serializes_as_plain_countdown() {
        let mut f = FocusSession::default();
        f.start_work(CFG, t0());
        let json = serde_json::to_value(&f).unwrap();
        assert_eq!(json["Phase"], "work");
    }
}
