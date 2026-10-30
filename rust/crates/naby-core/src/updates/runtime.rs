//! Ported from `Application/{RuntimeUpdates,FeatureResources}.cs`: the runtime change flags,
//! the shared 30 Hz media-level sample gate and the "is media worth running" rule.

use crate::settings::{policy::hover_pages, Settings};

/// What changed in a runtime update (C# `[Flags] RuntimeChange`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RuntimeChange(pub u32);

impl RuntimeChange {
    pub const CLOCK: Self = Self(1);
    pub const MEDIA: Self = Self(2);
    pub const AUDIO: Self = Self(4);
    pub const COUNTDOWN: Self = Self(8);
    pub const PRODUCTIVITY: Self = Self(16);
    pub const CALENDAR: Self = Self(32);
    pub const SYSTEM: Self = Self(64);
    pub const SETTINGS: Self = Self(128);
    pub const ALL: Self = Self(255);

    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }
}

impl std::ops::BitOr for RuntimeChange {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

/// Audio level reading shared by every consumer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MediaLevelSnapshot {
    pub sequence: i64,
    pub timestamp: i64,
    pub peak: f32,
    pub muted: bool,
    pub sessions: i32,
    pub audible_sessions: i32,
}

/// All consumers use the same monotonic sample lease at 30 Hz; rendering still follows each
/// display frame. `frequency` is the timestamp tick rate (C# `Stopwatch.Frequency`).
#[derive(Debug, Clone)]
pub struct MediaSampleGate {
    interval: f64,
    sampled_at: i64,
    next_at: f64,
    source: Option<String>,
    valid: bool,
    sequence: i64,
}

impl MediaSampleGate {
    pub fn new(frequency: f64) -> Self {
        Self {
            interval: frequency / 30.0,
            sampled_at: 0,
            next_at: 0.0,
            source: None,
            valid: false,
            sequence: 0,
        }
    }

    pub fn sequence(&self) -> i64 {
        self.sequence
    }

    /// True when a sample taken at `stamp` from `source` should be used.
    pub fn accept(&mut self, source: Option<&str>, stamp: i64) -> bool {
        let same = self.valid && self.source.as_deref() == source && stamp >= self.sampled_at;
        let stamp_f = stamp as f64;
        if same && stamp_f < self.next_at {
            return false;
        }
        self.next_at = if same && stamp_f - self.next_at < self.interval * 2.0 {
            self.next_at
                + (((stamp_f - self.next_at) / self.interval).floor() + 1.0) * self.interval
        } else {
            stamp_f + self.interval
        };
        self.valid = true;
        self.source = source.map(str::to_owned);
        self.sampled_at = stamp;
        self.sequence += 1;
        true
    }

    pub fn reset(&mut self) {
        self.valid = false;
        self.sampled_at = 0;
        self.next_at = 0.0;
        self.source = None;
    }
}

/// C# `App.MediaFeaturesWanted`: media integration is on and the media widget or hover page is shown.
pub fn media_features_wanted(s: &Settings) -> bool {
    s.media_integration_enabled
        && (s.widgets.iter().any(|w| w.enabled && w.id == "media")
            || hover_pages(s).iter().any(|p| p == "media"))
}
