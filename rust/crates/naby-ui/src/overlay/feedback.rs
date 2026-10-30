//! Volume / brightness feedback in the closed pill (C# `OverlayFeedback.cs`,
//! `UpdateAudioFeedback`): an icon, a percentage and a thin level bar for two seconds.

use crate::host::Snapshots;
use naby_core::motion::{MotionConfig, Tween};
use naby_render::Icon;

/// How long the feedback stays after the last change.
pub const SHOW_SECONDS: f64 = 2.0;

#[derive(Default)]
pub struct Feedback {
    pub brightness: bool,
    pub text: String,
    pub muted: bool,
    until: f64,
    level: Option<Tween<f64>>,
    audio_seen: bool,
    last_volume: f32,
    last_muted: bool,
    last_device: String,
    last_brightness: u64,
}

impl Feedback {
    /// The pill is showing feedback at `now`.
    pub fn visible(&self, now: f64) -> bool {
        now < self.until
    }

    pub fn clear(&mut self) {
        self.until = 0.0;
        self.level = None;
    }

    pub fn icon(&self) -> Icon {
        if self.brightness {
            Icon::Weather
        } else if self.muted {
            Icon::Muted
        } else {
            Icon::Volume
        }
    }

    /// Level bar fill 0..=1 at `now`.
    pub fn level(&self, now: f64) -> f64 {
        self.level.map_or(0.0, |t| t.value(now))
    }

    pub fn next_frame(&self, now: f64) -> Option<f64> {
        if !self.visible(now) {
            return None;
        }
        let tween = self.level.and_then(|t| t.next_frame(now));
        // Wake at the latest when the two seconds are over.
        Some(tween.map_or(self.until, |t| t.min(self.until)))
    }

    fn show(&mut self, cfg: &MotionConfig, now: f64, brightness: bool, level: f64, muted: bool) {
        self.brightness = brightness;
        self.muted = muted;
        let level = level.clamp(0.0, 1.0);
        self.text = if muted {
            "Muted".to_owned()
        } else {
            format!("{:.0}%", level * 100.0)
        };
        self.until = now + SHOW_SECONDS;
        let from = self.level.map_or(0.0, |t| t.value(now));
        let to = if muted { 0.0 } else { level };
        self.level = Some(if cfg.enabled {
            Tween::new(
                from,
                to,
                now,
                0.17,
                naby_core::motion::easing::Ease::CUBIC_OUT,
            )
        } else {
            Tween::instant(to)
        });
    }

    /// Looks at the audio and brightness snapshots; `allowed` is the C# gate (not expanded, not
    /// peeking, visible, not tucked). Returns true when the pill should repaint.
    pub fn observe(&mut self, snaps: &Snapshots, cfg: &MotionConfig, allowed: bool) -> bool {
        let s = &snaps.settings;
        let mut changed = false;
        let audio = &snaps.audio;
        if audio.available {
            if self.audio_seen
                && allowed
                && s.reactive_feedback
                && s.volume_feedback
                && (audio.device != self.last_device
                    || audio.muted != self.last_muted
                    || (audio.volume - self.last_volume).abs() > 0.005)
            {
                self.show(cfg, snaps.now, false, f64::from(audio.volume), audio.muted);
                changed = true;
            }
            self.audio_seen = true;
            self.last_volume = audio.volume;
            self.last_muted = audio.muted;
            self.last_device.clone_from(&audio.device);
        }
        let b = &snaps.brightness;
        if b.sequence != self.last_brightness {
            self.last_brightness = b.sequence;
            if let Some(level) = b.level {
                if allowed && s.reactive_feedback && s.brightness_feedback {
                    self.show(cfg, snaps.now, true, f64::from(level) / 100.0, false);
                    changed = true;
                }
            }
        }
        changed
    }
}
