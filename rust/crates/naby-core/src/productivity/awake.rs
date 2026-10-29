//! Keep-awake policy (C# `KeepAwakeService`). The host supplies the execution-state call
//! (`SetThreadExecutionState`) as a closure that returns 0 on failure; this type owns the
//! state, validation, expiry and the exact flags sent. The host must call it from a single thread
//! (the one that owns the requests), which the borrow rules already enforce for `&mut self`.

use chrono::{DateTime, Duration, Utc};

pub const CONTINUOUS: u32 = 0x8000_0000;
pub const SYSTEM_REQUIRED: u32 = 0x1;
pub const DISPLAY_REQUIRED: u32 = 0x2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AwakeError {
    InvalidDuration,
    /// Windows refused the request; the message is user-facing.
    Os(&'static str),
    Disposed,
}

impl std::fmt::Display for AwakeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidDuration => {
                f.write_str("Choose a keep-awake duration greater than zero and at most 24 hours.")
            }
            Self::Os(message) => f.write_str(message),
            Self::Disposed => f.write_str("Keep awake was already disposed."),
        }
    }
}
impl std::error::Error for AwakeError {}

pub struct KeepAwakeState<F: FnMut(u32) -> u32> {
    set_execution_state: F,
    active: bool,
    until: Option<DateTime<Utc>>,
    keep_display_on: bool,
    disposed: bool,
    changes: u32,
}

impl<F: FnMut(u32) -> u32> KeepAwakeState<F> {
    pub fn new(set_execution_state: F) -> Self {
        Self {
            set_execution_state,
            active: false,
            until: None,
            keep_display_on: false,
            disposed: false,
            changes: 0,
        }
    }

    pub fn active(&self) -> bool {
        self.active
    }
    pub fn until(&self) -> Option<DateTime<Utc>> {
        self.until
    }
    pub fn keep_display_on(&self) -> bool {
        self.keep_display_on
    }
    /// Number of state changes so far; hosts compare it to repaint (C# `Changed` event).
    pub fn changes(&self) -> u32 {
        self.changes
    }

    fn usable(&self) -> Result<(), AwakeError> {
        if self.disposed {
            Err(AwakeError::Disposed)
        } else {
            Ok(())
        }
    }

    /// `duration` of `None` keeps the machine awake until stopped; otherwise it must be in
    /// (0, 24h]. A failed call leaves the previous state untouched.
    pub fn start(
        &mut self,
        duration: Option<Duration>,
        keep_display_on: bool,
        now: DateTime<Utc>,
    ) -> Result<(), AwakeError> {
        self.usable()?;
        if let Some(d) = duration {
            if d <= Duration::zero() || d > Duration::hours(24) {
                return Err(AwakeError::InvalidDuration);
            }
        }
        let flags =
            CONTINUOUS | SYSTEM_REQUIRED | if keep_display_on { DISPLAY_REQUIRED } else { 0 };
        if (self.set_execution_state)(flags) == 0 {
            return Err(AwakeError::Os(
                "Windows could not enable keep awake. Try again from the island.",
            ));
        }
        self.active = true;
        self.until = duration.map(|d| now + d);
        self.keep_display_on = keep_display_on;
        self.changes += 1;
        Ok(())
    }

    pub fn stop(&mut self) -> Result<(), AwakeError> {
        self.usable()?;
        if !self.active {
            return Ok(());
        }
        if (self.set_execution_state)(CONTINUOUS) == 0 {
            return Err(AwakeError::Os(
                "Windows could not release keep awake. Try stopping it again before quitting.",
            ));
        }
        self.active = false;
        self.until = None;
        self.keep_display_on = false;
        self.changes += 1;
        Ok(())
    }

    /// Releases the request once the deadline passed.
    pub fn tick(&mut self, now: DateTime<Utc>) -> Result<(), AwakeError> {
        self.usable()?;
        match self.until {
            Some(until) if self.active && now >= until => self.stop(),
            _ => Ok(()),
        }
    }

    /// Releases the request and refuses further use. Safe to call twice; if Windows refuses the
    /// release the state stays active so the caller can retry.
    pub fn dispose(&mut self) -> Result<(), AwakeError> {
        if self.disposed {
            return Ok(());
        }
        self.stop()?;
        self.disposed = true;
        Ok(())
    }
}

impl<F: FnMut(u32) -> u32> Drop for KeepAwakeState<F> {
    fn drop(&mut self) {
        let _ = self.dispose();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 14, 5, 0, 0).unwrap()
    }

    #[test]
    fn starts_disabled_expires_resets_flags_and_releases_on_dispose() {
        let flags = Rc::new(RefCell::new(Vec::new()));
        let log = flags.clone();
        let mut service = KeepAwakeState::new(move |v| {
            log.borrow_mut().push(v);
            1
        });
        assert!(
            !service.active()
                && service.until().is_none()
                && !service.keep_display_on()
                && flags.borrow().is_empty()
        );
        service
            .start(Some(Duration::minutes(15)), true, now())
            .unwrap();
        assert!(
            service.active()
                && service.until() == Some(now() + Duration::minutes(15))
                && service.keep_display_on()
        );
        service.tick(now() + Duration::minutes(14)).unwrap();
        assert_eq!(flags.borrow().len(), 1);
        service.tick(now() + Duration::minutes(15)).unwrap();
        assert!(!service.active() && service.until().is_none() && !service.keep_display_on());
        service.start(None, false, now()).unwrap();
        service.tick(now() + Duration::days(1)).unwrap();
        assert!(service.active() && service.until().is_none());
        service.dispose().unwrap();
        service.dispose().unwrap();
        assert_eq!(
            *flags.borrow(),
            vec![0x8000_0003, 0x8000_0000, 0x8000_0001, 0x8000_0000]
        );
        assert_eq!(service.changes(), 4);
        assert_eq!(service.start(None, false, now()), Err(AwakeError::Disposed));
    }

    #[test]
    fn validates_durations_and_preserves_state_on_os_failure() {
        let fail = Rc::new(Cell::new(true));
        let calls = Rc::new(Cell::new(0));
        let (f, c) = (fail.clone(), calls.clone());
        let mut service = KeepAwakeState::new(move |_| {
            c.set(c.get() + 1);
            if f.get() {
                0
            } else {
                1
            }
        });
        assert_eq!(
            service.start(Some(Duration::zero()), false, now()),
            Err(AwakeError::InvalidDuration)
        );
        assert_eq!(
            service.start(Some(Duration::hours(25)), false, now()),
            Err(AwakeError::InvalidDuration)
        );
        assert_eq!(calls.get(), 0);
        assert!(matches!(
            service.start(None, true, now()),
            Err(AwakeError::Os(_))
        ));
        assert!(!service.active());

        fail.set(false);
        service
            .start(Some(Duration::hours(24)), false, now())
            .unwrap();
        fail.set(true);
        assert!(service.start(None, true, now()).is_err());
        assert!(
            service.active()
                && !service.keep_display_on()
                && service.until() == Some(now() + Duration::days(1))
        );
        assert!(service.tick(now() + Duration::days(1)).is_err());
        assert!(service.active());
        assert!(service.dispose().is_err());
        assert!(service.active());
        fail.set(false);
        service.dispose().unwrap();
        assert!(!service.active());
    }

    #[test]
    fn dropping_releases_the_request() {
        let flags = Rc::new(RefCell::new(Vec::new()));
        let log = flags.clone();
        {
            let mut service = KeepAwakeState::new(move |v| {
                log.borrow_mut().push(v);
                1
            });
            service.start(None, false, now()).unwrap();
        }
        assert_eq!(*flags.borrow(), vec![0x8000_0001, 0x8000_0000]);
    }
}
