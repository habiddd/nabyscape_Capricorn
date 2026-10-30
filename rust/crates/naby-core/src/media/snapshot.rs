//! Plain snapshot structs produced by the Windows host and read by views.
//! Every `Default` is the "unavailable" form.

use std::sync::Arc;
use std::time::Duration;

/// One media session as the dock shows it (SMTC on Windows).
#[derive(Debug, Clone)]
pub struct MediaSnapshot {
    pub available: bool,
    /// App user model id of the selected session (`MediaService.Selected.SourceAppUserModelId`).
    pub source_id: String,
    pub source_name: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub playing: bool,
    pub position: Duration,
    pub duration: Duration,
    pub can_toggle: bool,
    pub can_next: bool,
    pub can_previous: bool,
    pub can_seek: bool,
    /// Encoded cover image bytes. Equality is by identity, like the C# `ReferenceEquals`.
    pub artwork: Option<Arc<[u8]>>,
}

impl Default for MediaSnapshot {
    fn default() -> Self {
        MediaSnapshot {
            available: false,
            source_id: String::new(),
            source_name: String::new(),
            // The C# placeholder texts shown when nothing plays.
            title: "Nothing playing".to_owned(),
            artist: "Play something in your favorite app".to_owned(),
            album: String::new(),
            playing: false,
            position: Duration::ZERO,
            duration: Duration::ZERO,
            can_toggle: false,
            can_next: false,
            can_previous: false,
            can_seek: false,
            artwork: None,
        }
    }
}

impl PartialEq for MediaSnapshot {
    fn eq(&self, other: &Self) -> bool {
        let same_art = match (&self.artwork, &other.artwork) {
            (None, None) => true,
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            _ => false,
        };
        same_art
            && self.available == other.available
            && self.source_id == other.source_id
            && self.source_name == other.source_name
            && self.title == other.title
            && self.artist == other.artist
            && self.album == other.album
            && self.playing == other.playing
            && self.position == other.position
            && self.duration == other.duration
            && self.can_toggle == other.can_toggle
            && self.can_next == other.can_next
            && self.can_previous == other.can_previous
            && self.can_seek == other.can_seek
    }
}

impl MediaSnapshot {
    /// `(elapsed, duration)` in seconds, like `MediaService.Timeline()`.
    pub fn timeline(&self) -> (f64, f64) {
        if !self.available {
            return (0.0, 0.0);
        }
        (self.position.as_secs_f64(), self.duration.as_secs_f64())
    }
}

/// An active render endpoint.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AudioOutput {
    pub id: String,
    pub name: String,
}

/// Master volume, default output list and default microphone.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioSnapshot {
    pub available: bool,
    /// 0..=1 endpoint master scalar.
    pub volume: f32,
    pub muted: bool,
    /// Friendly name of the default output.
    pub device: String,
    pub mic_available: bool,
    pub mic_muted: bool,
    pub mic_name: String,
    pub outputs: Vec<AudioOutput>,
    pub default_output_id: String,
}

impl Default for AudioSnapshot {
    fn default() -> Self {
        AudioSnapshot {
            available: false,
            volume: 0.0,
            muted: false,
            device: String::new(),
            mic_available: false,
            mic_muted: false,
            mic_name: "No microphone".to_owned(),
            outputs: Vec::new(),
            default_output_id: String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_unavailable() {
        let m = MediaSnapshot::default();
        assert!(!m.available && !m.playing && m.artwork.is_none());
        assert_eq!(m.title, "Nothing playing");
        assert_eq!(m.timeline(), (0.0, 0.0));
        let a = AudioSnapshot::default();
        assert!(!a.available && !a.mic_available && a.outputs.is_empty());
        assert_eq!(a.mic_name, "No microphone");
    }

    #[test]
    fn artwork_compares_by_identity() {
        let bytes: Arc<[u8]> = Arc::from(vec![1u8, 2, 3]);
        let a = MediaSnapshot {
            artwork: Some(bytes.clone()),
            ..Default::default()
        };
        let b = MediaSnapshot {
            artwork: Some(bytes),
            ..Default::default()
        };
        let c = MediaSnapshot {
            artwork: Some(Arc::from(vec![1u8, 2, 3])),
            ..Default::default()
        };
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn timeline_reports_seconds_when_available() {
        let m = MediaSnapshot {
            available: true,
            position: Duration::from_millis(1500),
            duration: Duration::from_secs(180),
            ..Default::default()
        };
        assert_eq!(m.timeline(), (1.5, 180.0));
    }
}
