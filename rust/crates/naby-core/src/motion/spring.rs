//! Critically damped surface motion (C# `SurfaceMotion`).
//!
//! Integrated analytically so rapid reversals keep their velocity. It drives only the painted
//! surface; the native window never resizes on a frame.

/// An animated surface property.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Channel {
    SurfaceWidth,
    SurfaceHeight,
    GlassLift,
    TuckX,
    TuckY,
}

#[derive(Debug, Clone, Copy)]
struct Track {
    channel: Channel,
    value: f64,
    velocity: f64,
    destination: f64,
}

/// What a call to [`SurfaceMotion::update`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Not running (or the frame was skipped because no time elapsed).
    Idle,
    /// Still moving; keep sampling.
    Moving,
    /// All channels reached their destination on this frame (C# completion callback).
    Finished,
}

/// A set of springs sharing one stiffness, advanced by `update(now)`.
#[derive(Debug, Clone)]
pub struct SurfaceMotion {
    tracks: Vec<Track>,
    active: bool,
    last: Option<f64>,
    omega: f64,
    fps: f64,
}

impl Default for SurfaceMotion {
    fn default() -> Self {
        SurfaceMotion {
            tracks: Vec::new(),
            active: false,
            last: None,
            omega: 16.0,
            fps: 60.0,
        }
    }
}

impl SurfaceMotion {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether any channel is still moving (C# `Active`).
    pub fn active(&self) -> bool {
        self.active
    }

    /// Frame rate the host samples at while active (the `Motion.FrameLimit`).
    pub fn set_fps(&mut self, fps: u32) {
        self.fps = f64::from(fps.clamp(15, 360));
    }

    /// Current value of `channel`, if it has ever been driven.
    pub fn value(&self, channel: Channel) -> Option<f64> {
        self.tracks
            .iter()
            .find(|t| t.channel == channel)
            .map(|t| t.value)
    }

    pub fn velocity(&self, channel: Channel) -> f64 {
        self.tracks
            .iter()
            .find(|t| t.channel == channel)
            .map_or(0.0, |t| t.velocity)
    }

    /// C# `Go`: drive `channel` towards `destination`. `current` seeds a channel the first time
    /// it is used (the property's current value). With motion disabled the value jumps.
    pub fn go(
        &mut self,
        channel: Channel,
        current: f64,
        destination: f64,
        milliseconds: f64,
        motion_enabled: bool,
    ) {
        let index = match self.tracks.iter().position(|t| t.channel == channel) {
            Some(i) => i,
            None => {
                self.tracks.push(Track {
                    channel,
                    value: current,
                    velocity: 0.0,
                    destination,
                });
                self.tracks.len() - 1
            }
        };
        let track = &mut self.tracks[index];
        track.destination = destination;
        self.omega = 7.0 / (milliseconds / 1000.0).max(0.12);
        if !motion_enabled {
            track.value = destination;
            track.velocity = 0.0;
            return;
        }
        if !self.active {
            self.last = None;
            self.active = true;
        }
    }

    /// C# `Snap`: stop driving `channel` and force it to `value`.
    pub fn snap(&mut self, channel: Channel, value: f64) {
        self.tracks.retain(|t| t.channel != channel);
        self.tracks.push(Track {
            channel,
            value,
            velocity: 0.0,
            destination: value,
        });
    }

    /// Stops everything without reporting completion (C# `Dispose`).
    pub fn stop(&mut self) {
        self.active = false;
        self.last = None;
        self.tracks.clear();
    }

    /// Advances to `now`. `motion_enabled == false` snaps every channel to its destination.
    pub fn update(&mut self, now: f64, motion_enabled: bool) -> Step {
        if !self.active {
            return Step::Idle;
        }
        let dt = match self.last {
            Some(last) => (now - last).clamp(0.0, 0.05),
            None => 1.0 / 60.0,
        };
        self.last = Some(now);
        if dt == 0.0 {
            return Step::Idle;
        }
        let omega = self.omega;
        let mut finished = true;
        for t in &mut self.tracks {
            if !motion_enabled {
                t.value = t.destination;
                t.velocity = 0.0;
                continue;
            }
            let delta = t.value - t.destination;
            let temp = (t.velocity + omega * delta) * dt;
            let decay = (-omega * dt).exp();
            t.value = t.destination + (delta + temp) * decay;
            t.velocity = (t.velocity - omega * temp) * decay;
            if (t.value - t.destination).abs() < 0.05 && t.velocity.abs() < 0.5 {
                t.value = t.destination;
                t.velocity = 0.0;
            } else {
                finished = false;
            }
        }
        if finished {
            self.active = false;
            self.last = None;
            Step::Finished
        } else {
            Step::Moving
        }
    }

    /// When the next `update` is needed; `None` while nothing moves.
    pub fn next_frame(&self, now: f64) -> Option<f64> {
        self.active.then(|| now + 1.0 / self.fps)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(m: &mut SurfaceMotion, seconds: f64, step: f64) -> Step {
        let mut now = 0.0;
        let mut last = Step::Idle;
        while now < seconds {
            now += step;
            let s = m.update(now, true);
            if s != Step::Idle {
                last = s;
            }
            if s == Step::Finished {
                break;
            }
        }
        last
    }

    #[test]
    fn converges_without_overshooting() {
        let mut m = SurfaceMotion::new();
        m.go(Channel::SurfaceWidth, 300.0, 600.0, 420.0, true);
        let mut max = 0.0f64;
        let mut now = 0.0;
        loop {
            now += 1.0 / 60.0;
            let s = m.update(now, true);
            max = max.max(m.value(Channel::SurfaceWidth).unwrap());
            if s == Step::Finished {
                break;
            }
            assert!(now < 3.0, "must settle");
        }
        assert!(max <= 600.0 + 1e-9, "critically damped never overshoots");
        assert_eq!(m.value(Channel::SurfaceWidth), Some(600.0));
        assert!(!m.active());
        assert_eq!(m.next_frame(now), None);
    }

    #[test]
    fn reversal_keeps_velocity() {
        let mut m = SurfaceMotion::new();
        m.go(Channel::SurfaceHeight, 44.0, 300.0, 420.0, true);
        let mut now = 0.0;
        for _ in 0..6 {
            now += 1.0 / 60.0;
            m.update(now, true);
        }
        let v = m.velocity(Channel::SurfaceHeight);
        assert!(v > 0.0);
        m.go(Channel::SurfaceHeight, 0.0, 44.0, 420.0, true);
        assert_eq!(
            m.velocity(Channel::SurfaceHeight),
            v,
            "velocity survives a retarget"
        );
        assert_eq!(run(&mut m, 5.0, 1.0 / 60.0), Step::Finished);
        assert_eq!(m.value(Channel::SurfaceHeight), Some(44.0));
    }

    #[test]
    fn disabled_motion_jumps_and_stays_idle() {
        let mut m = SurfaceMotion::new();
        m.go(Channel::GlassLift, 0.0, 1.0, 420.0, false);
        assert_eq!(m.value(Channel::GlassLift), Some(1.0));
        assert!(!m.active());
        assert_eq!(m.update(1.0, false), Step::Idle);
        // Switching motion off mid-flight snaps on the next frame.
        m.go(Channel::GlassLift, 1.0, 0.0, 420.0, true);
        assert!(m.active());
        assert_eq!(m.update(0.01, false), Step::Finished);
        assert_eq!(m.value(Channel::GlassLift), Some(0.0));
    }

    #[test]
    fn frame_cap_and_dt_clamp() {
        let mut m = SurfaceMotion::new();
        m.set_fps(120);
        m.go(Channel::TuckY, 0.0, -38.0, 420.0, true);
        assert_eq!(m.next_frame(1.0), Some(1.0 + 1.0 / 120.0));
        m.update(0.0, true);
        // A long stall advances by at most 50 ms of simulated time.
        let before = m.value(Channel::TuckY).unwrap();
        m.update(10.0, true);
        let after = m.value(Channel::TuckY).unwrap();
        assert!(after < before && after > -38.0);
        // Same timestamp twice is a skipped frame.
        assert_eq!(m.update(10.0, true), Step::Idle);
    }

    #[test]
    fn snap_and_stop() {
        let mut m = SurfaceMotion::new();
        m.go(Channel::SurfaceWidth, 100.0, 200.0, 300.0, true);
        m.snap(Channel::SurfaceWidth, 150.0);
        assert_eq!(m.value(Channel::SurfaceWidth), Some(150.0));
        m.stop();
        assert!(!m.active());
        assert_eq!(m.value(Channel::SurfaceWidth), None);
    }
}
