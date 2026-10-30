//! The one animation timer. Nothing is armed unless a surface asked for a future frame, so an
//! idle app has zero timers and its message loop sleeps in `MsgWaitForMultipleObjectsEx`.
//!
//! `FrameSchedule` is the pure bookkeeping; `AnimationTimer` (Windows only) backs it with a
//! high-resolution waitable timer whose handle the message loop waits on.

use std::time::{Duration, Instant};

/// Remembers the single pending frame deadline.
#[derive(Debug, Default)]
pub struct FrameSchedule {
    due: Option<Instant>,
}

impl FrameSchedule {
    /// Deadlines closer than this are the same request (no re-arm).
    const TOLERANCE: Duration = Duration::from_micros(500);

    /// Replaces the pending deadline. Returns whether the OS timer must be re-armed.
    pub fn request(&mut self, at: Instant) -> bool {
        match self.due {
            Some(d)
                if d.saturating_duration_since(at) < Self::TOLERANCE
                    && at.saturating_duration_since(d) < Self::TOLERANCE =>
            {
                false
            }
            _ => {
                self.due = Some(at);
                true
            }
        }
    }

    /// Clears the request. Returns whether the OS timer must be cancelled.
    pub fn cancel(&mut self) -> bool {
        self.due.take().is_some()
    }

    /// Called when the timer signalled: consumes the request if it is due.
    pub fn take_due(&mut self, now: Instant) -> bool {
        match self.due {
            Some(d) if d <= now + Self::TOLERANCE => {
                self.due = None;
                true
            }
            _ => false,
        }
    }

    pub fn pending(&self) -> Option<Instant> {
        self.due
    }
}

/// `SetWaitableTimer` due time: negative 100 ns units relative to now; an already-passed
/// deadline fires immediately.
pub fn relative_due_100ns(at: Instant, now: Instant) -> i64 {
    let wait = at.saturating_duration_since(now);
    let ticks = (wait.as_nanos() / 100).min(i64::MAX as u128 - 1) as i64;
    -ticks.max(1)
}

#[cfg(windows)]
pub use os::AnimationTimer;

#[cfg(windows)]
mod os {
    use super::*;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::Threading::{
        CancelWaitableTimer, CreateWaitableTimerExW, SetWaitableTimer,
    };

    /// `CREATE_WAITABLE_TIMER_HIGH_RESOLUTION` (Windows 10 1803+).
    const HIGH_RESOLUTION: u32 = 0x2;
    /// `TIMER_ALL_ACCESS`.
    const ALL_ACCESS: u32 = 0x1F_0003;

    pub struct AnimationTimer {
        handle: HANDLE,
        schedule: FrameSchedule,
    }

    impl AnimationTimer {
        pub fn new() -> windows::core::Result<Self> {
            // SAFETY: creates an unnamed timer; the handle is closed in Drop.
            let handle = unsafe {
                CreateWaitableTimerExW(None, PCWSTR::null(), HIGH_RESOLUTION, ALL_ACCESS)
                    .or_else(|_| CreateWaitableTimerExW(None, PCWSTR::null(), 0, ALL_ACCESS))?
            };
            Ok(Self {
                handle,
                schedule: FrameSchedule::default(),
            })
        }

        /// The handle the message loop waits on.
        pub fn handle(&self) -> HANDLE {
            self.handle
        }

        /// Wakes the loop at `at` (replacing any earlier request).
        pub fn request_frame_at(&mut self, at: Instant) {
            if !self.schedule.request(at) {
                return;
            }
            let due = relative_due_100ns(at, Instant::now());
            // SAFETY: `due` outlives the call; no completion routine is registered.
            let armed = unsafe { SetWaitableTimer(self.handle, &due, 0, None, None, false) };
            if armed.is_err() {
                // Without a timer the surface would stall; forget the request so the next
                // call retries.
                self.schedule.cancel();
            }
        }

        /// Disarms the timer (nothing is animating).
        pub fn cancel(&mut self) {
            if self.schedule.cancel() {
                // SAFETY: cancels a timer this struct owns.
                let _ = unsafe { CancelWaitableTimer(self.handle) };
            }
        }

        /// Call when the handle signalled; true when a requested frame is due.
        pub fn take_due(&mut self) -> bool {
            self.schedule.take_due(Instant::now())
        }

        pub fn pending(&self) -> Option<Instant> {
            self.schedule.pending()
        }
    }

    impl Drop for AnimationTimer {
        fn drop(&mut self) {
            // SAFETY: closes the handle created in `new`.
            let _ = unsafe { CloseHandle(self.handle) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_replace_and_equal_requests_do_not_rearm() {
        let t0 = Instant::now();
        let mut s = FrameSchedule::default();
        assert!(s.pending().is_none());
        assert!(s.request(t0 + Duration::from_millis(16)));
        assert!(!s.request(t0 + Duration::from_millis(16)));
        assert!(!s.request(t0 + Duration::from_micros(16_100)));
        // A later request replaces an earlier one (no stale wake-ups).
        assert!(s.request(t0 + Duration::from_millis(100)));
        assert_eq!(s.pending(), Some(t0 + Duration::from_millis(100)));
    }

    #[test]
    fn cancel_reports_whether_anything_was_armed() {
        let mut s = FrameSchedule::default();
        assert!(!s.cancel());
        s.request(Instant::now());
        assert!(s.cancel());
        assert!(!s.cancel());
    }

    #[test]
    fn take_due_consumes_only_due_requests() {
        let t0 = Instant::now();
        let mut s = FrameSchedule::default();
        s.request(t0 + Duration::from_millis(50));
        assert!(!s.take_due(t0 + Duration::from_millis(10)));
        assert!(s.take_due(t0 + Duration::from_millis(50)));
        assert!(!s.take_due(t0 + Duration::from_millis(60)));
        assert!(s.pending().is_none());
    }

    #[test]
    fn due_time_is_negative_relative_and_never_zero() {
        let t0 = Instant::now();
        assert_eq!(
            relative_due_100ns(t0 + Duration::from_millis(16), t0),
            -160_000
        );
        assert_eq!(relative_due_100ns(t0, t0 + Duration::from_secs(1)), -1);
        assert_eq!(relative_due_100ns(t0, t0), -1);
    }
}
