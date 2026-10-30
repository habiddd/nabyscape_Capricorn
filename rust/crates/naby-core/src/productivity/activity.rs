//! Which countdown the closed island shows (C# `OverlayWindow.ActiveCountdown` and the
//! closed-summary text). The audio-waveform half of `LiveActivity` belongs to the media tasks.

use super::focus::FocusSession;
use super::timer::Timer;
use chrono::{DateTime, Utc};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveCountdown {
    Focus,
    Timer,
}

/// Focus wins over the timer; a countdown counts only while its widget is on and time remains
/// (running or paused).
pub fn active_countdown(
    focus_widget_on: bool,
    timer_widget_on: bool,
    focus: &FocusSession,
    timer: &Timer,
    now: DateTime<Utc>,
) -> Option<ActiveCountdown> {
    if focus_widget_on && focus.active(now) {
        Some(ActiveCountdown::Focus)
    } else if timer_widget_on && timer.remaining(now) > 0.0 {
        Some(ActiveCountdown::Timer)
    } else {
        None
    }
}

/// The closed-island label, e.g. `Focus  24:59`, `Break  04:10` or `Timer  01:30`.
pub fn countdown_summary(
    active: ActiveCountdown,
    focus: &FocusSession,
    timer: &Timer,
    now: DateTime<Utc>,
) -> String {
    match active {
        ActiveCountdown::Focus => format!(
            "{}  {}",
            if focus.is_break() { "Break" } else { "Focus" },
            focus.countdown.display(now)
        ),
        ActiveCountdown::Timer => format!("Timer  {}", timer.display(now)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn t0() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 14, 12, 0, 0).unwrap()
    }

    #[test]
    fn focus_wins_then_timer_then_nothing() {
        let mut focus = FocusSession::default();
        let mut timer = Timer::default();
        assert_eq!(active_countdown(true, true, &focus, &timer, t0()), None);
        timer.start(90.0, t0(), "", false);
        assert_eq!(
            active_countdown(true, true, &focus, &timer, t0()),
            Some(ActiveCountdown::Timer)
        );
        focus.start_work(Default::default(), t0());
        assert_eq!(
            active_countdown(true, true, &focus, &timer, t0()),
            Some(ActiveCountdown::Focus)
        );
        // Widget gates.
        assert_eq!(
            active_countdown(false, true, &focus, &timer, t0()),
            Some(ActiveCountdown::Timer)
        );
        assert_eq!(active_countdown(false, false, &focus, &timer, t0()), None);
        // A paused countdown still has time left and stays selected.
        focus.toggle(Default::default(), t0());
        assert_eq!(
            active_countdown(true, true, &focus, &timer, t0()),
            Some(ActiveCountdown::Focus)
        );
    }

    #[test]
    fn summary_text() {
        let mut focus = FocusSession::default();
        let mut timer = Timer::default();
        timer.start(90.0, t0(), "", false);
        assert_eq!(
            countdown_summary(ActiveCountdown::Timer, &focus, &timer, t0()),
            "Timer  01:30"
        );
        focus.start_work(Default::default(), t0());
        assert_eq!(
            countdown_summary(ActiveCountdown::Focus, &focus, &timer, t0()),
            "Focus  25:00"
        );
        focus.countdown.phase = "break".into();
        assert!(
            countdown_summary(ActiveCountdown::Focus, &focus, &timer, t0()).starts_with("Break  ")
        );
    }
}
