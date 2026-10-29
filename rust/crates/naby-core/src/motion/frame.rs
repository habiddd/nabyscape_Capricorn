//! Frame budget, pacing and telemetry (C# `Motion/FramePerformance.cs`).

use std::collections::VecDeque;

/// `Settings.Performance`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Performance {
    Auto,
    Hemat,
    Maksimal,
}

impl Performance {
    /// Unknown values behave as `Auto`, like the C# `switch` default arm.
    pub fn parse(text: &str) -> Performance {
        match text {
            "Hemat" => Performance::Hemat,
            "Maksimal" => Performance::Maksimal,
            _ => Performance::Auto,
        }
    }
}

/// C# `OverlayWindow.FrameLimitFor`: the fps cap for a profile.
pub fn frame_limit_for(
    profile: Performance,
    hz: u32,
    interactive: bool,
    battery: bool,
    cpu: f64,
) -> u32 {
    let hz = hz.clamp(30, 240);
    match profile {
        Performance::Hemat => hz.min(if interactive { 60 } else { 30 }),
        Performance::Maksimal => hz,
        Performance::Auto => {
            if battery || cpu > 85.0 {
                hz.min(60)
            } else if interactive {
                hz
            } else {
                hz.min(60)
            }
        }
    }
}

/// Inputs to the frame cap that change independently (C# `ConfigurePerformance`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameBudget {
    pub profile: Performance,
    pub refresh_hz: u32,
}

impl FrameBudget {
    /// A refresh rate below 2 Hz is treated as unknown and falls back to 60.
    pub fn new(profile: &str, refresh_hz: u32) -> FrameBudget {
        FrameBudget {
            profile: Performance::parse(profile),
            refresh_hz: if refresh_hz < 2 { 60 } else { refresh_hz },
        }
    }

    pub fn limit(&self, interactive: bool, battery: bool, cpu: f64) -> u32 {
        frame_limit_for(self.profile, self.refresh_hz, interactive, battery, cpu)
    }

    /// Whether the adaptive profile needs CPU samples (C# `NeedsAdaptiveCpuSample`).
    pub fn needs_cpu_sample(&self, visible: bool, tucked: bool, rendering_or_moving: bool) -> bool {
        visible && !tucked && self.profile == Performance::Auto && rendering_or_moving
    }
}

/// Drops callbacks that arrive faster than the cap (C# `FramePacer`).
#[derive(Debug, Clone)]
pub struct FramePacer {
    next: f64,
    previous_cap: u32,
}

impl Default for FramePacer {
    fn default() -> Self {
        FramePacer {
            next: -1.0,
            previous_cap: 0,
        }
    }
}

impl FramePacer {
    pub fn new() -> Self {
        Self::default()
    }

    /// `true` when a callback at `time` seconds should be rendered.
    pub fn accept(&mut self, time: f64, cap: u32) -> bool {
        let interval = 1.0 / f64::from(cap.clamp(15, 360));
        if self.next < 0.0 || self.previous_cap != cap || time > self.next + 1.0 {
            self.previous_cap = cap;
            self.next = time + interval;
            return true;
        }
        if time + 0.0007 < self.next {
            return false;
        }
        loop {
            self.next += interval;
            if self.next > time + 0.0007 {
                break;
            }
        }
        true
    }

    pub fn reset(&mut self) {
        self.next = -1.0;
    }
}

/// Rolling frame-interval statistics (C# `FrameTelemetry`).
#[derive(Debug, Clone, Default)]
pub struct FrameTelemetry {
    samples: VecDeque<f64>,
    last: Option<f64>,
}

impl FrameTelemetry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a frame at `time` seconds; intervals of 250 ms or more (idle gaps) are ignored.
    pub fn sample(&mut self, time: f64) {
        if let Some(last) = self.last {
            let ms = (time - last) * 1000.0;
            if ms > 0.0 && ms < 250.0 {
                self.samples.push_back(ms);
                while self.samples.len() > 240 {
                    self.samples.pop_front();
                }
            }
        }
        self.last = Some(time);
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// The settings-page summary for the current frame cap.
    pub fn status(&self, frame_limit: u32) -> String {
        if self.samples.len() < 2 {
            return "Waiting for animation activity\u{2026}".to_string();
        }
        let mut sorted: Vec<f64> = self.samples.iter().copied().collect();
        sorted.sort_by(f64::total_cmp);
        let n = sorted.len();
        let mean = sorted.iter().sum::<f64>() / n as f64;
        let p95 = sorted[(n - 1).min((n as f64 * 0.95) as usize)];
        let threshold = 1000.0 / f64::from(frame_limit.max(1)) * 1.8;
        let late = sorted.iter().filter(|x| **x > threshold).count();
        format!(
            "{:.0} updates/s \u{b7} p95 {:.1} ms\n{late}/{n} late intervals \u{b7} cap {frame_limit} FPS\nMeasured frame callbacks, not GPU presentation.",
            1000.0 / mean,
            p95
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn performance_profiles_adapt_to_battery_load_and_interaction() {
        use Performance::*;
        assert_eq!(frame_limit_for(Auto, 180, true, false, 20.0), 180);
        assert_eq!(frame_limit_for(Auto, 180, false, false, 20.0), 60);
        assert_eq!(frame_limit_for(Auto, 180, true, true, 20.0), 60);
        assert_eq!(frame_limit_for(Auto, 180, true, false, 90.0), 60);
        assert_eq!(frame_limit_for(Hemat, 180, false, false, 20.0), 30);
        assert_eq!(frame_limit_for(Hemat, 180, true, false, 20.0), 60);
        assert_eq!(frame_limit_for(Maksimal, 180, true, true, 90.0), 180);
        // Refresh rate is clamped to 30..240.
        assert_eq!(frame_limit_for(Maksimal, 500, true, false, 0.0), 240);
        assert_eq!(frame_limit_for(Maksimal, 10, true, false, 0.0), 30);
        assert_eq!(frame_limit_for(Auto, 30, false, false, 0.0), 30);
    }

    #[test]
    fn budget_defaults_unknown_refresh_to_60() {
        let b = FrameBudget::new("Maksimal", 0);
        assert_eq!(b.refresh_hz, 60);
        assert_eq!(b.limit(true, false, 0.0), 60);
        assert_eq!(FrameBudget::new("garbage", 144).profile, Performance::Auto);
        assert!(FrameBudget::new("Auto", 60).needs_cpu_sample(true, false, true));
        assert!(!FrameBudget::new("Hemat", 60).needs_cpu_sample(true, false, true));
        assert!(!FrameBudget::new("Auto", 60).needs_cpu_sample(true, true, true));
    }

    #[test]
    fn frame_pacing_preserves_60_hz_cadence_on_a_180_hz_callback_stream() {
        let mut p = FramePacer::new();
        let mut accepted = 0;
        for i in 0..=180 {
            if p.accept(f64::from(i) / 180.0, 60) {
                accepted += 1;
            }
        }
        assert!((60..=62).contains(&accepted), "accepted {accepted}");
    }

    #[test]
    fn pacer_resets_on_cap_change_and_long_gap() {
        let mut p = FramePacer::new();
        assert!(p.accept(0.0, 60));
        assert!(!p.accept(0.001, 60));
        assert!(p.accept(0.002, 30), "cap change restarts");
        assert!(p.accept(10.0, 30), "gap over one second restarts");
        p.reset();
        assert!(p.accept(10.001, 30));
    }

    #[test]
    fn telemetry_ignores_idle_gaps_and_summarises() {
        let mut t = FrameTelemetry::new();
        assert!(t.status(60).starts_with("Waiting"));
        let mut time = 0.0;
        t.sample(time);
        for _ in 0..100 {
            time += 1.0 / 60.0;
            t.sample(time);
        }
        time += 2.0; // idle gap: dropped
        t.sample(time);
        assert_eq!(t.len(), 100);
        let status = t.status(60);
        assert!(status.starts_with("60 updates/s"), "{status}");
        assert!(status.contains("0/100 late intervals"));
        assert!(status.contains("cap 60 FPS"));
        for _ in 0..400 {
            time += 0.01;
            t.sample(time);
        }
        assert_eq!(t.len(), 240, "bounded history");
    }
}
