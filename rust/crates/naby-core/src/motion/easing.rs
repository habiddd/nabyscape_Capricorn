//! WPF easing functions and sampled tweens (C# `Motion.To/Fade/Enter/Progress`).

use crate::settings::Settings;
use std::f64::consts::PI;

/// WPF `EasingMode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EasingMode {
    In,
    Out,
    InOut,
}

/// The WPF easing families (default parameters match the WPF defaults).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EaseKind {
    Linear,
    Quadratic,
    Cubic,
    Quartic,
    Quintic,
    /// `PowerEase`, exponent clamped at zero.
    Power(f64),
    Sine,
    /// `ExponentialEase`, `Exponent` (WPF default 2).
    Exponential(f64),
    /// `BackEase`, `Amplitude` (WPF default 1).
    Back(f64),
    Circle,
}

/// An easing function: family plus mode.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ease {
    pub kind: EaseKind,
    pub mode: EasingMode,
}

impl Ease {
    /// `Motion.easeOut`: `ExponentialEase { Exponent = 5, EaseOut }` (movement).
    pub const EXPO_OUT_5: Ease = Ease {
        kind: EaseKind::Exponential(5.0),
        mode: EasingMode::Out,
    };
    /// `Motion.fadeOut`: `CubicEase { EaseOut }` (opacity, progress bars, follow slide).
    pub const CUBIC_OUT: Ease = Ease {
        kind: EaseKind::Cubic,
        mode: EasingMode::Out,
    };
    /// Ambient loop: `SineEase { EaseInOut }`.
    pub const SINE_IN_OUT: Ease = Ease {
        kind: EaseKind::Sine,
        mode: EasingMode::InOut,
    };
    pub const LINEAR: Ease = Ease {
        kind: EaseKind::Linear,
        mode: EasingMode::In,
    };

    pub const fn new(kind: EaseKind, mode: EasingMode) -> Ease {
        Ease { kind, mode }
    }

    fn ease_in(&self, t: f64) -> f64 {
        match self.kind {
            EaseKind::Linear => t,
            EaseKind::Quadratic => t * t,
            EaseKind::Cubic => t * t * t,
            EaseKind::Quartic => t * t * t * t,
            EaseKind::Quintic => t * t * t * t * t,
            EaseKind::Power(p) => t.powf(p.max(0.0)),
            EaseKind::Sine => 1.0 - (PI / 2.0 * (1.0 - t)).sin(),
            EaseKind::Exponential(e) => {
                if e.abs() < 1e-10 {
                    t
                } else {
                    ((e * t).exp() - 1.0) / (e.exp() - 1.0)
                }
            }
            EaseKind::Back(a) => {
                let a = a.max(0.0);
                t * t * t - t * a * (PI * t).sin()
            }
            EaseKind::Circle => 1.0 - (1.0 - t * t).max(0.0).sqrt(),
        }
    }

    /// `EasingFunctionBase.Ease`: maps normalized time (clamped to 0..1) to progress.
    pub fn apply(&self, t: f64) -> f64 {
        let t = if t.is_finite() {
            t.clamp(0.0, 1.0)
        } else {
            0.0
        };
        match self.mode {
            EasingMode::In => self.ease_in(t),
            EasingMode::Out => 1.0 - self.ease_in(1.0 - t),
            EasingMode::InOut => {
                if t < 0.5 {
                    self.ease_in(t * 2.0) * 0.5
                } else {
                    (1.0 - self.ease_in((1.0 - t) * 2.0)) * 0.5 + 0.5
                }
            }
        }
    }
}

/// Linear interpolation for tweened value types.
pub trait Lerp: Copy {
    fn lerp(a: Self, b: Self, t: f64) -> Self;
}

impl Lerp for f64 {
    fn lerp(a: f64, b: f64, t: f64) -> f64 {
        a + (b - a) * t
    }
}

impl Lerp for (f64, f64) {
    fn lerp(a: (f64, f64), b: (f64, f64), t: f64) -> (f64, f64) {
        (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t)
    }
}

/// A value animated between two endpoints, sampled by time. While a start delay is pending the
/// `from` value is held. A zero duration is an instant jump (reduce motion).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tween<T: Lerp> {
    pub from: T,
    pub to: T,
    pub start: f64,
    pub delay: f64,
    pub duration: f64,
    pub ease: Ease,
    /// Frame rate the host should sample at while running.
    pub fps: f64,
}

impl<T: Lerp> Tween<T> {
    pub fn new(from: T, to: T, start: f64, duration_secs: f64, ease: Ease) -> Self {
        Tween {
            from,
            to,
            start,
            delay: 0.0,
            duration: duration_secs.max(0.0),
            ease,
            fps: 60.0,
        }
    }

    /// A tween that is already finished at `value`.
    pub fn instant(value: T) -> Self {
        Tween::new(value, value, 0.0, 0.0, Ease::LINEAR)
    }

    pub fn with_delay(mut self, delay_secs: f64) -> Self {
        self.delay = delay_secs.max(0.0);
        self
    }

    pub fn with_fps(mut self, fps: f64) -> Self {
        self.fps = fps.clamp(1.0, 1000.0);
        self
    }

    fn begin(&self) -> f64 {
        self.start + self.delay
    }

    /// Eased progress 0..1 at `now`.
    pub fn progress(&self, now: f64) -> f64 {
        if now < self.begin() {
            return 0.0;
        }
        if self.duration <= 0.0 {
            return 1.0;
        }
        self.ease.apply((now - self.begin()) / self.duration)
    }

    pub fn value(&self, now: f64) -> T {
        T::lerp(self.from, self.to, self.progress(now))
    }

    pub fn finished(&self, now: f64) -> bool {
        self.duration <= 0.0 && now >= self.begin() || now >= self.begin() + self.duration
    }

    /// When the next sample is needed: the start of a pending delay, one frame later while
    /// running, `None` once finished.
    pub fn next_frame(&self, now: f64) -> Option<f64> {
        if self.finished(now) {
            None
        } else if now < self.begin() {
            Some(self.begin())
        } else {
            Some(now + 1.0 / self.fps)
        }
    }
}

/// `Settings.InteractionMotion`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interaction {
    Off,
    Subtle,
    Expressive,
}

impl Interaction {
    pub fn parse(text: &str) -> Interaction {
        match text {
            "Off" => Interaction::Off,
            "Expressive" => Interaction::Expressive,
            _ => Interaction::Subtle,
        }
    }
}

/// C# static `Motion` state: enabled flag, base duration, interaction style and frame cap.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MotionConfig {
    pub enabled: bool,
    pub duration_ms: f64,
    pub interaction: Interaction,
    pub frame_limit: u32,
}

impl Default for MotionConfig {
    fn default() -> Self {
        MotionConfig {
            enabled: true,
            duration_ms: 420.0,
            interaction: Interaction::Subtle,
            frame_limit: 180,
        }
    }
}

impl MotionConfig {
    /// C# `Motion.Configure`. `system_animations` is the OS "client area animations" switch.
    pub fn from_settings(s: &Settings, system_animations: bool) -> MotionConfig {
        MotionConfig {
            enabled: !s.reduce_motion && system_animations && s.animation_ms > 0.0,
            duration_ms: s.animation_ms,
            interaction: Interaction::parse(&s.interaction_motion),
            frame_limit: 180,
        }
    }

    /// Duration in seconds, zero when motion is disabled.
    pub fn seconds(&self, ms: f64) -> f64 {
        if self.enabled {
            (ms / 1000.0).max(0.0)
        } else {
            0.0
        }
    }

    fn fps(&self) -> f64 {
        f64::from(self.frame_limit.clamp(15, 360))
    }

    /// C# `Motion.To`: exponential ease-out; no animation when disabled or already there.
    pub fn to(&self, from: f64, to: f64, now: f64, ms: f64, delay_ms: f64) -> Tween<f64> {
        if !self.enabled || (from - to).abs() < 0.001 {
            return Tween::instant(to);
        }
        Tween::new(from, to, now, ms / 1000.0, Ease::EXPO_OUT_5)
            .with_delay(delay_ms / 1000.0)
            .with_fps(self.fps())
    }

    /// C# `Motion.Fade`: cubic ease-out, instant unless the opacity really changes.
    pub fn fade(&self, from: f64, to: f64, now: f64, ms: f64, delay_ms: f64) -> Tween<f64> {
        if !self.enabled || (from - to).abs() <= 0.001 {
            return Tween::instant(to);
        }
        Tween::new(from, to, now, ms / 1000.0, Ease::CUBIC_OUT)
            .with_delay(delay_ms / 1000.0)
            .with_fps(self.fps())
    }

    /// C# `Motion.Progress`: 360 ms cubic ease-out, only for changes above 0.05.
    pub fn progress(&self, from: f64, to: f64, now: f64) -> Tween<f64> {
        if !self.enabled || (from - to).abs() <= 0.05 {
            return Tween::instant(to);
        }
        Tween::new(from, to, now, 0.36, Ease::CUBIC_OUT).with_fps(self.fps())
    }

    /// C# `Motion.Enter`: fade in over 220 ms while sliding `distance` DIP up over 300 ms.
    pub fn enter(&self, distance: f64, delay_ms: f64, now: f64) -> Enter {
        if !self.enabled {
            return Enter {
                opacity: Tween::instant(1.0),
                offset_y: Tween::instant(0.0),
            };
        }
        Enter {
            opacity: self.fade(0.0, 1.0, now, 220.0, delay_ms),
            offset_y: self.to(distance, 0.0, now, 300.0, delay_ms),
        }
    }

    /// Whether button / slider feedback is active (`Motion.Enabled && Interaction != Off`).
    pub fn interactive(&self) -> bool {
        self.enabled && self.interaction != Interaction::Off
    }
}

/// Result of [`MotionConfig::enter`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Enter {
    pub opacity: Tween<f64>,
    pub offset_y: Tween<f64>,
}

impl Enter {
    pub fn next_frame(&self, now: f64) -> Option<f64> {
        match (self.opacity.next_frame(now), self.offset_y.next_frame(now)) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn endpoints_are_exact_for_every_family() {
        let kinds = [
            EaseKind::Linear,
            EaseKind::Quadratic,
            EaseKind::Cubic,
            EaseKind::Quartic,
            EaseKind::Quintic,
            EaseKind::Power(3.0),
            EaseKind::Sine,
            EaseKind::Exponential(5.0),
            EaseKind::Back(1.0),
            EaseKind::Circle,
        ];
        for kind in kinds {
            for mode in [EasingMode::In, EasingMode::Out, EasingMode::InOut] {
                let e = Ease::new(kind, mode);
                assert!(close(e.apply(0.0), 0.0), "{kind:?}/{mode:?} at 0");
                assert!(close(e.apply(1.0), 1.0), "{kind:?}/{mode:?} at 1");
            }
        }
    }

    #[test]
    fn wpf_reference_values() {
        // ExponentialEase(5) EaseOut at 0.5: 1 - (e^2.5 - 1) / (e^5 - 1)
        let expo = Ease::EXPO_OUT_5.apply(0.5);
        assert!(close(
            expo,
            1.0 - (2.5f64.exp() - 1.0) / (5.0f64.exp() - 1.0)
        ));
        assert!(expo > 0.92 && expo < 0.93);
        assert!(close(Ease::CUBIC_OUT.apply(0.5), 0.875));
        assert!(close(Ease::SINE_IN_OUT.apply(0.5), 0.5));
        assert!(close(
            Ease::SINE_IN_OUT.apply(0.25),
            0.5 * (1.0 - (PI / 2.0 * 0.5).sin())
        ));
        assert!(close(
            Ease::new(EaseKind::Quintic, EasingMode::In).apply(0.5),
            0.03125
        ));
        // BackEase dips below zero near the start of EaseIn.
        assert!(Ease::new(EaseKind::Back(1.0), EasingMode::In).apply(0.2) < 0.0);
        // Out-of-range and non-finite input is clamped.
        assert!(close(Ease::CUBIC_OUT.apply(-3.0), 0.0));
        assert!(close(Ease::CUBIC_OUT.apply(9.0), 1.0));
        assert!(close(Ease::CUBIC_OUT.apply(f64::NAN), 0.0));
    }

    #[test]
    fn exponential_with_zero_exponent_is_linear() {
        let e = Ease::new(EaseKind::Exponential(0.0), EasingMode::In);
        assert!(close(e.apply(0.3), 0.3));
    }

    #[test]
    fn tween_samples_and_reports_next_frame() {
        let cfg = MotionConfig::default();
        let t = cfg.to(0.0, 100.0, 10.0, 200.0, 100.0);
        assert!(close(t.value(10.0), 0.0));
        assert!(close(t.value(10.05), 0.0), "held during the delay");
        assert_eq!(t.next_frame(10.0), Some(10.1));
        let mid = t.value(10.2);
        assert!(mid > 0.0 && mid < 100.0);
        assert_eq!(t.next_frame(10.2), Some(10.2 + 1.0 / 180.0));
        assert!(close(t.value(10.3), 100.0));
        assert_eq!(t.next_frame(10.3), None);
        assert!(t.finished(11.0));
    }

    #[test]
    fn small_changes_do_not_animate() {
        let cfg = MotionConfig::default();
        let t = cfg.to(5.0, 5.0005, 0.0, 300.0, 0.0);
        assert!(close(t.value(0.0), 5.0005));
        assert_eq!(t.next_frame(0.0), None);
        let p = cfg.progress(10.0, 10.04, 0.0);
        assert_eq!(p.next_frame(0.0), None);
        assert!(cfg.progress(10.0, 20.0, 0.0).next_frame(0.0).is_some());
    }

    #[test]
    fn reduce_motion_zero_duration() {
        let s = Settings {
            reduce_motion: true,
            ..Settings::default()
        };
        let cfg = MotionConfig::from_settings(&s, true);
        assert!(!cfg.enabled);
        assert_eq!(cfg.seconds(420.0), 0.0);
        let t = cfg.to(0.0, 50.0, 3.0, 420.0, 0.0);
        assert!(close(t.value(3.0), 50.0));
        assert_eq!(t.next_frame(3.0), None);
        let enter = cfg.enter(8.0, 0.0, 3.0);
        assert!(close(enter.opacity.value(3.0), 1.0));
        assert!(close(enter.offset_y.value(3.0), 0.0));
        assert_eq!(enter.next_frame(3.0), None);
        // A zero animation duration or the OS switch disables motion as well.
        let zero = Settings {
            animation_ms: 0.0,
            ..Settings::default()
        };
        assert!(!MotionConfig::from_settings(&zero, true).enabled);
        assert!(!MotionConfig::from_settings(&Settings::default(), false).enabled);
        assert!(MotionConfig::from_settings(&Settings::default(), true).enabled);
    }

    #[test]
    fn enter_slides_and_fades() {
        let cfg = MotionConfig::default();
        let e = cfg.enter(9.0, 0.0, 0.0);
        assert!(close(e.opacity.value(0.0), 0.0));
        assert!(close(e.offset_y.value(0.0), 9.0));
        assert!(close(e.opacity.value(0.22), 1.0));
        assert!(e.offset_y.value(0.22) > 0.0);
        assert!(close(e.offset_y.value(0.3), 0.0));
        assert!(e.next_frame(0.25).is_some());
        assert_eq!(e.next_frame(0.3), None);
    }

    #[test]
    fn pair_tween_interpolates() {
        let t = Tween::new((0.0, 10.0), (10.0, 0.0), 0.0, 1.0, Ease::LINEAR);
        assert_eq!(t.value(0.5), (5.0, 5.0));
    }
}
