//! LRC parsing and line lookup (`LyricTiming` in `LyricsService.cs`).

use std::collections::HashSet;

use super::snapshot::MediaSnapshot;

/// Sources above this many UTF-16 units are rejected.
pub const MAX_SOURCE_UNITS: usize = 512 * 1024;
const MAX_LINES_READ: usize = 5000;
const MAX_STAMPS: usize = 10_000;
const MAX_TEXT_CHARS: usize = 500;

#[derive(Debug, Clone, PartialEq)]
pub struct LyricLine {
    /// Seconds from the start of the track (offset already applied).
    pub at: f64,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LrcError {
    TooLarge,
}

/// Time-ordered, de-duplicated lyric lines.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Lyrics {
    pub lines: Vec<LyricLine>,
}

/// Parses `source`; a source that is too large yields no lines.
pub fn parse(source: &str) -> Lyrics {
    try_parse(source).unwrap_or_default()
}

/// Like [`parse`] but reports oversized input.
pub fn try_parse(source: &str) -> Result<Lyrics, LrcError> {
    if source.encode_utf16().count() > MAX_SOURCE_UNITS {
        return Err(LrcError::TooLarge);
    }
    let offset = find_offset(source);
    let mut result: Vec<LyricLine> = Vec::new();
    for raw in source.split('\n').take(MAX_LINES_READ) {
        if result.len() >= MAX_STAMPS {
            break;
        }
        let (stamps, text) = split_stamps(raw);
        if stamps.is_empty() {
            continue;
        }
        let mut text = text.trim().to_owned();
        if text.chars().count() > MAX_TEXT_CHARS {
            text = text.chars().take(MAX_TEXT_CHARS).collect();
        }
        for stamp in stamps {
            if stamp.seconds >= 60 {
                continue;
            }
            let fraction = if stamp.fraction_digits == 0 {
                0.0
            } else {
                stamp.fraction as f64 / 10f64.powi(stamp.fraction_digits as i32)
            };
            let at = (stamp.minutes * 60 + stamp.seconds) as f64 + fraction + offset;
            if (0.0..=86_400.0).contains(&at) {
                result.push(LyricLine {
                    at,
                    text: text.clone(),
                });
            }
        }
    }
    result.sort_by(|a, b| a.at.total_cmp(&b.at));
    let mut seen: HashSet<(u64, String)> = HashSet::new();
    result.retain(|l| seen.insert((l.at.to_bits(), l.text.clone())));
    Ok(Lyrics { lines: result })
}

struct Stamp {
    minutes: u32,
    seconds: u32,
    fraction: u32,
    fraction_digits: u32,
}

/// `\[(\d{1,3}):(\d{2})(?:[.:](\d{1,3}))?\]` starting at `b[i] == '['`; returns the end index.
fn stamp_at(b: &[u8], i: usize) -> Option<(usize, Stamp)> {
    let digits = |from: usize, max: usize| -> usize {
        b[from..]
            .iter()
            .take(max)
            .take_while(|c| c.is_ascii_digit())
            .count()
    };
    let num = |from: usize, n: usize| -> u32 {
        b[from..from + n]
            .iter()
            .fold(0u32, |a, c| a * 10 + (c - b'0') as u32)
    };
    let mut p = i + 1;
    let n1 = digits(p, 3);
    if n1 == 0 {
        return None;
    }
    let minutes = num(p, n1);
    p += n1;
    if b.get(p) != Some(&b':') {
        return None;
    }
    p += 1;
    if digits(p, 2) != 2 {
        return None;
    }
    let seconds = num(p, 2);
    p += 2;
    let mut stamp = Stamp {
        minutes,
        seconds,
        fraction: 0,
        fraction_digits: 0,
    };
    match b.get(p) {
        Some(b']') => Some((p + 1, stamp)),
        Some(b'.') | Some(b':') => {
            p += 1;
            let n = digits(p, 3);
            if n == 0 {
                return None;
            }
            stamp.fraction = num(p, n);
            stamp.fraction_digits = n as u32;
            p += n;
            if b.get(p) == Some(&b']') {
                Some((p + 1, stamp))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// All timestamps in `line` plus the line with them removed (not yet trimmed).
fn split_stamps(line: &str) -> (Vec<Stamp>, String) {
    let b = line.as_bytes();
    let mut stamps = Vec::new();
    let mut text = String::with_capacity(line.len());
    let mut copied = 0;
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'[' {
            if let Some((end, stamp)) = stamp_at(b, i) {
                text.push_str(&line[copied..i]);
                copied = end;
                stamps.push(stamp);
                i = end;
                continue;
            }
        }
        i += 1;
    }
    text.push_str(&line[copied..]);
    (stamps, text)
}

/// First `[offset:±ms]` tag anywhere in the source, in seconds.
fn find_offset(source: &str) -> f64 {
    const TAG: &[u8] = b"[offset:";
    let b = source.as_bytes();
    let mut i = 0;
    while i + TAG.len() <= b.len() {
        if b[i] == b'[' && b[i..i + TAG.len()].eq_ignore_ascii_case(TAG) {
            let mut p = i + TAG.len();
            let negative = b.get(p) == Some(&b'-');
            if matches!(b.get(p), Some(b'+') | Some(b'-')) {
                p += 1;
            }
            let n = b[p..]
                .iter()
                .take(6)
                .take_while(|c| c.is_ascii_digit())
                .count();
            if n > 0 && b.get(p + n) == Some(&b']') {
                let v = b[p..p + n]
                    .iter()
                    .fold(0i64, |a, c| a * 10 + (c - b'0') as i64);
                return (if negative { -v } else { v }) as f64 / 1000.0;
            }
        }
        i += 1;
    }
    0.0
}

impl Lyrics {
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    /// Index of the last line starting at or before `seconds`, or `None` before the first.
    pub fn index(&self, seconds: f64) -> Option<usize> {
        let (mut low, mut high) = (0isize, self.lines.len() as isize - 1);
        let mut result = None;
        while low <= high {
            let middle = (low + high) / 2;
            if self.lines[middle as usize].at <= seconds {
                result = Some(middle as usize);
                low = middle + 1;
            } else {
                high = middle - 1;
            }
        }
        result
    }

    /// Like [`index`](Self::index) but `None` for blank, music-note, instrumental and
    /// section markers: timestamped breaks only, never guessed from line length.
    pub fn vocal_index(&self, seconds: f64) -> Option<usize> {
        let index = self.index(seconds)?;
        let text = self.lines[index].text.trim();
        if text.is_empty()
            || text
                .trim_matches(|c| matches!(c, '♪' | '♫' | '♬' | '♩' | ' ' | '\t'))
                .is_empty()
            || text == "🎵"
            || text == "🎶"
        {
            return None;
        }
        let marker = text.trim_matches(|c| matches!(c, '[' | ']' | '(' | ')' | ' ' | '\t'));
        let is = |name: &str| marker.eq_ignore_ascii_case(name);
        if is("instrumental") || is("instrumental break") {
            return None;
        }
        let section = text.starts_with(['[', '(']);
        if section && (is("music") || is("interlude") || is("intro") || is("outro") || is("solo")) {
            return None;
        }
        Some(index)
    }
}

/// `LyricTiming.CanUse`: a session with usable title, artist (not the app id) and a timeline.
pub fn can_use(media: &MediaSnapshot) -> bool {
    media.available
        && !media.title.trim().is_empty()
        && !media.artist.trim().is_empty()
        && media.artist != media.source_id
        && media.timeline().1 > 0.0
}

pub fn is_spotify(source: Option<&str>) -> bool {
    source.is_some_and(|s| s.to_ascii_lowercase().contains("spotify"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn lrc_offsets_and_blank_lines() {
        let l = parse(
            "[offset:-100]\n[00:01.00][00:03.125]朝のサンプル\n[00:62.00]invalid\n[ar:fixture]\n[00:04.5]Evening sample",
        );
        assert_eq!(l.len(), 3);
        assert!((l.lines[0].at - 0.9).abs() < 0.001 && l.lines[0].text == "朝のサンプル");
        assert!((l.lines[1].at - 3.025).abs() < 0.001);
        assert_eq!(l.index(0.1), None);
        assert_eq!(l.index(3.1), Some(1));
        assert_eq!(l.index(100.0), Some(2));
    }

    #[test]
    fn blank_timestamped_lines_are_kept_as_breaks() {
        let l = parse("[00:05]a\r\n[00:10]\r\n[00:12]b");
        assert_eq!(l.len(), 3);
        assert_eq!(l.lines[0].text, "a");
        assert_eq!(l.lines[1].text, "");
    }

    #[test]
    fn closed_lyrics_vocal_index_skips_breaks_and_markers() {
        assert_eq!(Lyrics::default().vocal_index(10.0), None);
        let l = parse(
            "[00:05]First vocal line\n[00:10]\n[00:15][Instrumental]\n[00:20]Next vocal line\n[00:25]♪ ♫\n[00:30](Music)\n[00:35]Intro",
        );
        assert_eq!(l.vocal_index(0.0), None);
        assert_eq!(l.vocal_index(5.0), Some(0));
        assert_eq!(l.vocal_index(9.99), Some(0));
        for at in [10.0, 14.99, 15.0, 19.99, 25.0, 30.0] {
            assert_eq!(l.vocal_index(at), None, "{at}");
        }
        assert_eq!(l.vocal_index(20.0), Some(3));
        assert_eq!(l.vocal_index(24.99), Some(3));
        // "Intro" without brackets is plain text, not a section marker.
        assert_eq!(l.vocal_index(35.0), Some(6));
        assert_eq!(l.vocal_index(7.0), Some(0));
        for marker in [
            "Instrumental",
            "[instrumental break]",
            "(Interlude)",
            "[Intro]",
            "[Outro]",
            "[Solo]",
            "🎵",
            "🎶",
        ] {
            let l = Lyrics {
                lines: vec![LyricLine {
                    at: 0.0,
                    text: marker.to_owned(),
                }],
            };
            assert_eq!(l.vocal_index(1.0), None, "{marker}");
        }
        let long = Lyrics {
            lines: vec![LyricLine {
                at: 0.0,
                text: "A long vocal line".to_owned(),
            }],
        };
        assert_eq!(long.vocal_index(80.0), Some(0));
    }

    #[test]
    fn limits_and_hostile_input_never_panic() {
        assert!(try_parse(&"x".repeat(MAX_SOURCE_UNITS + 1)).is_err());
        assert!(parse(&"x".repeat(MAX_SOURCE_UNITS + 1)).is_empty());
        // Unterminated/odd stamps, multibyte right after a bracket, huge numbers.
        for s in [
            "[",
            "[0",
            "[00:",
            "[00:1",
            "[00:12.",
            "[00:12.1234]x",
            "[1234:00]x",
            "[00:00]é[00:0é",
            "[offset:99999999999]",
            "[offset:+",
            "[é:00]",
        ] {
            let _ = parse(s);
        }
        let long = format!("[00:01]{}", "é".repeat(900));
        assert_eq!(parse(&long).lines[0].text.chars().count(), 500);
        // More than 5000 lines are not read.
        let many: String = (0..6000).map(|_| "[00:01]x\n").collect();
        let l = parse(&many);
        assert_eq!(l.len(), 1); // all identical (at, text) collapse
        let distinct: String = (0..6000)
            .map(|i| format!("[00:{:02}.{:03}]{i}\n", i % 60, i % 1000))
            .collect();
        assert!(parse(&distinct).len() <= 5000);
    }

    #[test]
    fn stamps_out_of_range_dropped_and_duplicates_collapse() {
        let l = parse("[offset:-5000]\n[00:01]early\n[00:06]a\n[00:06]a\n[00:06]b");
        assert_eq!(l.len(), 2);
        assert_eq!(l.lines[0].text, "a");
        assert_eq!(l.lines[1].text, "b");
    }

    #[test]
    fn can_use_requires_metadata_and_timeline() {
        let mut m = MediaSnapshot {
            available: true,
            title: "T".into(),
            artist: "A".into(),
            source_id: "Spotify.exe".into(),
            duration: Duration::from_secs(100),
            ..Default::default()
        };
        assert!(can_use(&m));
        m.artist = "Spotify.exe".into();
        assert!(!can_use(&m));
        m.artist = " ".into();
        assert!(!can_use(&m));
        m.artist = "A".into();
        m.duration = Duration::ZERO;
        assert!(!can_use(&m));
        assert!(is_spotify(Some("SpotifyAB.SpotifyMusic")) && !is_spotify(None));
    }
}
