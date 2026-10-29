//! Pure parts of `MediaService` (`Application/Services.cs`): timeline math, text
//! fallbacks and artwork limits. The WinRT glue lives in `media.rs`.
#![allow(dead_code)]

/// Largest encoded thumbnail kept in memory. Bigger covers are dropped (the dock
/// shows its placeholder); the C# limit was 10 MiB before decoding to 160 px.
pub const ARTWORK_LIMIT: u64 = 8 * 1024 * 1024;

pub const NOTHING_PLAYING: &str = "Nothing playing";
pub const PLAY_SOMETHING: &str = "Play something in your favorite app";
pub const SOURCE_NONE: &str = "Media source is set to None";
pub const CONTROLS_UNAVAILABLE: &str = "Media controls unavailable";
pub const SESSION_UNAVAILABLE: &str = "Media session unavailable";
pub const CONTROL_UNSUPPORTED: &str = "This media app does not support this control";
pub const CONTROL_FAILED: &str = "Media control failed; try selecting the session again";
pub const SEEK_UNSUPPORTED: &str = "This app does not support seeking";

/// Seconds between the FILETIME epoch (1601) and the Unix epoch.
pub const UNIX_TO_FILETIME_SECS: i64 = 11_644_473_600;
const TICKS_PER_SEC: f64 = 10_000_000.0;

pub fn artwork_acceptable(size: u64) -> bool {
    size > 0 && size <= ARTWORK_LIMIT
}

pub fn display_title(title: &str) -> String {
    if title.trim().is_empty() {
        "Untitled media".to_owned()
    } else {
        title.to_owned()
    }
}

pub fn display_artist(artist: &str, source_id: &str) -> String {
    if artist.trim().is_empty() {
        source_id.to_owned()
    } else {
        artist.to_owned()
    }
}

/// Idle text for the artist line.
pub fn idle_artist(source_disabled: bool) -> &'static str {
    if source_disabled {
        SOURCE_NONE
    } else {
        PLAY_SOMETHING
    }
}

/// Identity of a track; artwork is re-read only when it changes.
pub fn artwork_key(source: &str, title: &str, artist: &str, album: &str) -> String {
    format!("{source}\n{title}\n{artist}\n{album}")
}

/// Raw SMTC timeline, all values in 100 ns ticks. `last_updated` and `now` are on the
/// same (FILETIME) epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawTimeline {
    pub start: i64,
    pub end: i64,
    pub position: i64,
    pub last_updated: i64,
}

/// `(elapsed, duration)` seconds at `now`, as `MediaService.ReadTimeline`:
/// the session's position plus the time since it was reported while playing.
pub fn read_timeline(raw: RawTimeline, playing: bool, now: i64) -> (f64, f64) {
    let duration = (raw.end - raw.start) as f64 / TICKS_PER_SEC;
    if duration <= 0.0 {
        return (0.0, 0.0);
    }
    let mut position = (raw.position - raw.start) as f64 / TICKS_PER_SEC;
    let age = (now - raw.last_updated) as f64 / TICKS_PER_SEC;
    if playing && (0.0..86_400.0).contains(&age) {
        position += age;
    }
    (position.clamp(0.0, duration), duration)
}

/// Position shown between timeline events: the last sample advanced by wall time
/// while playing (`MediaService.Timeline`).
pub fn extrapolate(sample: (f64, f64), playing: bool, since_sample_secs: f64) -> (f64, f64) {
    let (elapsed, duration) = sample;
    let advanced = elapsed
        + if playing {
            since_sample_secs.max(0.0)
        } else {
            0.0
        };
    (advanced.clamp(0.0, duration.max(0.0)), duration)
}

/// Target for `TryChangePlaybackPositionAsync`; `None` when the timeline has no span.
pub fn seek_ticks(start: i64, end: i64, fraction: f64) -> Option<i64> {
    let span = end - start;
    if span <= 0 {
        return None;
    }
    let f = if fraction.is_finite() {
        fraction.clamp(0.0, 1.0)
    } else {
        0.0
    };
    Some(start + (span as f64 * f) as i64)
}

/// Current time as 100 ns ticks since 1601, the epoch of WinRT `DateTime`.
pub fn now_filetime_ticks() -> i64 {
    let since_unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    (UNIX_TO_FILETIME_SECS + since_unix.as_secs() as i64) * 10_000_000
        + i64::from(since_unix.subsec_nanos() / 100)
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: i64 = 10_000_000;

    #[test]
    fn timeline_adds_report_age_only_while_playing() {
        let raw = RawTimeline {
            start: 0,
            end: 200 * S,
            position: 50 * S,
            last_updated: 1000 * S,
        };
        assert_eq!(read_timeline(raw, true, 1004 * S), (54.0, 200.0));
        assert_eq!(read_timeline(raw, false, 1004 * S), (50.0, 200.0));
    }

    #[test]
    fn timeline_ignores_future_or_ancient_reports_and_clamps() {
        let raw = RawTimeline {
            start: 10 * S,
            end: 110 * S,
            position: 105 * S,
            last_updated: 1000 * S,
        };
        // 95 s in, report is 10 s old -> clamped to the 100 s duration.
        assert_eq!(read_timeline(raw, true, 1010 * S), (100.0, 100.0));
        assert_eq!(read_timeline(raw, true, 999 * S).0, 95.0);
        assert_eq!(read_timeline(raw, true, (1000 + 86_400) * S).0, 95.0);
        let before_start = RawTimeline { position: 0, ..raw };
        assert_eq!(read_timeline(before_start, false, 0).0, 0.0);
    }

    #[test]
    fn no_duration_means_no_timeline() {
        let raw = RawTimeline {
            start: 5,
            end: 5,
            position: 5,
            last_updated: 5,
        };
        assert_eq!(read_timeline(raw, true, 5), (0.0, 0.0));
    }

    #[test]
    fn extrapolation_advances_playing_only_and_clamps() {
        assert_eq!(extrapolate((10.0, 100.0), true, 2.5), (12.5, 100.0));
        assert_eq!(extrapolate((10.0, 100.0), false, 2.5), (10.0, 100.0));
        assert_eq!(extrapolate((99.0, 100.0), true, 30.0), (100.0, 100.0));
        assert_eq!(extrapolate((0.0, 0.0), true, 5.0), (0.0, 0.0));
    }

    #[test]
    fn seek_maps_fraction_into_the_span() {
        assert_eq!(seek_ticks(10 * S, 110 * S, 0.5), Some(60 * S));
        assert_eq!(seek_ticks(0, 100, 7.0), Some(100));
        assert_eq!(seek_ticks(0, 100, -1.0), Some(0));
        assert_eq!(seek_ticks(0, 100, f64::NAN), Some(0));
        assert_eq!(seek_ticks(5, 5, 0.5), None);
    }

    #[test]
    fn text_fallbacks() {
        assert_eq!(display_title("  "), "Untitled media");
        assert_eq!(display_title("Song"), "Song");
        assert_eq!(display_artist("", "Spotify.exe"), "Spotify.exe");
        assert_eq!(display_artist("Band", "x"), "Band");
        assert_eq!(idle_artist(true), SOURCE_NONE);
        assert_eq!(idle_artist(false), PLAY_SOMETHING);
    }

    #[test]
    fn artwork_limits() {
        assert!(!artwork_acceptable(0));
        assert!(artwork_acceptable(1));
        assert!(artwork_acceptable(ARTWORK_LIMIT));
        assert!(!artwork_acceptable(ARTWORK_LIMIT + 1));
        assert_ne!(
            artwork_key("a", "t", "x", "al"),
            artwork_key("a", "t", "x", "al2")
        );
    }

    #[test]
    fn filetime_now_is_after_2020() {
        // 2020-01-01 as FILETIME ticks.
        assert!(now_filetime_ticks() > (11_644_473_600 + 1_577_836_800) * S);
    }
}
