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

/// Moves an animation deadline onto the display's refresh grid, so frames land evenly instead of
/// drifting against it. `at` was computed after painting, so the time spent since `frame_start`
/// comes off it: otherwise "60 fps" means 60 fps minus the paint time. Far deadlines (clock ticks)
/// and immediate ones (input) stay as they are.
pub fn align_to_vblank(
    at: Instant,
    now: Instant,
    frame_start: Instant,
    vblank: Instant,
    period: Duration,
) -> Instant {
    if at <= now || at - now > Duration::from_millis(100) || period.is_zero() {
        return at;
    }
    let target = at
        .checked_sub(now.saturating_duration_since(frame_start))
        .unwrap_or(at);
    let period_s = period.as_secs_f64();
    let mut n = if target >= vblank {
        ((target - vblank).as_secs_f64() / period_s).round()
    } else {
        -((vblank - target).as_secs_f64() / period_s).round()
    };
    let grid = |n: f64| {
        if n >= 0.0 {
            vblank + period.mul_f64(n)
        } else {
            vblank.checked_sub(period.mul_f64(-n)).unwrap_or(vblank)
        }
    };
    while grid(n) <= now {
        n += 1.0;
    }
    grid(n)
}

#[cfg(windows)]
pub use os::{display_refresh, AnimationTimer};

#[cfg(windows)]
mod os {
    use super::*;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND};
    use windows::Win32::Graphics::Dwm::{DwmGetCompositionTimingInfo, DWM_TIMING_INFO};
    use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
    use windows::Win32::System::Threading::{
        CancelWaitableTimer, CreateWaitableTimerExW, SetWaitableTimer,
    };

    /// A vertical blank and the refresh period, from the desktop compositor. The blank goes stale
    /// while nothing on screen changes; its phase stays good and it refreshes once frames flow.
    pub fn display_refresh(now: Instant) -> Option<(Instant, Duration)> {
        let mut info = DWM_TIMING_INFO {
            cbSize: std::mem::size_of::<DWM_TIMING_INFO>() as u32,
            ..Default::default()
        };
        let (mut counter, mut frequency) = (0i64, 0i64);
        // SAFETY: the out pointers are live locals; a null window asks for the desktop's timing.
        unsafe {
            DwmGetCompositionTimingInfo(HWND::default(), &mut info).ok()?;
            QueryPerformanceCounter(&mut counter).ok()?;
            QueryPerformanceFrequency(&mut frequency).ok()?;
        }
        if frequency <= 0 || info.qpcRefreshPeriod == 0 {
            return None;
        }
        let seconds = |ticks: u64| Duration::from_secs_f64(ticks as f64 / frequency as f64);
        let vblank = info.qpcVBlank as i64;
        let vblank = if vblank >= counter {
            now + seconds((vblank - counter) as u64)
        } else {
            now.checked_sub(seconds((counter - vblank) as u64))?
        };
        Some((vblank, seconds(info.qpcRefreshPeriod)))
    }

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

    #[test]
    fn frames_land_on_the_refresh_grid() {
        let vblank = Instant::now();
        let period = Duration::from_micros(16_667);
        // Woke on a blank, painted for 5 ms, asked for "one frame later": the very next blank.
        let start = vblank;
        let now = start + Duration::from_millis(5);
        let at = now + period;
        assert_eq!(
            align_to_vblank(at, now, start, vblank, period),
            vblank + period
        );
        // A paint longer than the frame takes the first blank that is still ahead.
        let now = start + Duration::from_millis(20);
        let at = now + period;
        assert_eq!(
            align_to_vblank(at, now, start, vblank, period),
            vblank + period * 2
        );
        // 60 fps on a 120 Hz display keeps every second blank.
        let fast = Duration::from_micros(8_333);
        let now = start + Duration::from_millis(3);
        let at = now + period;
        assert_eq!(
            align_to_vblank(at, now, start, vblank, fast),
            vblank + fast * 2
        );
    }

    #[test]
    fn immediate_and_far_deadlines_are_left_alone() {
        let now = Instant::now() + Duration::from_secs(1);
        let period = Duration::from_micros(16_667);
        assert_eq!(align_to_vblank(now, now, now, now, period), now);
        let far = now + Duration::from_secs(30);
        assert_eq!(align_to_vblank(far, now, now, now, period), far);
        let soon = now + Duration::from_millis(5);
        assert_eq!(align_to_vblank(soon, now, now, now, Duration::ZERO), soon);
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "needs a desktop session"]
    fn the_compositor_reports_a_sane_refresh() {
        let now = Instant::now();
        let (vblank, period) = display_refresh(now).expect("timing");
        println!(
            "period {period:?}, vblank {:?} old",
            now.saturating_duration_since(vblank)
        );
        assert!(period > Duration::from_millis(2) && period < Duration::from_millis(50));
    }
}
