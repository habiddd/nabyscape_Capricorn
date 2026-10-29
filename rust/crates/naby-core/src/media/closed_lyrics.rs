//! Closed-dock lyric line selection and phrase splitting (`ClosedLyrics.cs`).
//!
//! The dock shows one phrase that fits its width. Long lines are split into phrases
//! and the phrase is chosen by progress through the line, never by guessing from audio.
//! Before the first line, in blank/instrumental breaks, or without lyrics, the dock
//! falls back to the waveform.

use super::lrc::Lyrics;

/// Pixels reserved around the text inside the closed summary host.
pub const HOST_PADDING: f32 = 18.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LyricsAnimation {
    #[default]
    Slide,
    Focus,
    Karaoke,
}

impl LyricsAnimation {
    pub fn parse(text: &str) -> Self {
        match text {
            "Focus" => LyricsAnimation::Focus,
            "Karaoke" => LyricsAnimation::Karaoke,
            _ => LyricsAnimation::Slide,
        }
    }
}

/// The lyric preferences the closed dock needs (`Settings.LyricsOffset`, `LyricsAnimation`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ClosedLyricsConfig {
    /// Seconds added to the playback position, clamped to +-10 like `Settings.Normalize`.
    pub offset_seconds: f64,
    pub animation: LyricsAnimation,
}

impl ClosedLyricsConfig {
    pub fn offset(&self) -> f64 {
        if self.offset_seconds.is_finite() {
            self.offset_seconds.clamp(-10.0, 10.0)
        } else {
            0.0
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClosedLine {
    /// Nothing to sing right now: show the waveform instead.
    Waveform,
    /// The phrase to draw (empty when the host is too narrow).
    Text(String),
}

/// Phrase cache for the current line and layout.
#[derive(Debug, Default)]
pub struct ClosedLyrics {
    key: String,
    phrases: Vec<String>,
}

impl ClosedLyrics {
    pub fn new() -> Self {
        Self::default()
    }

    /// Index of the sung line, or `None` for the waveform fallback. `usable` is
    /// [`lrc::can_use`](super::lrc::can_use) of the current media.
    pub fn index(
        cfg: &ClosedLyricsConfig,
        lyrics: &Lyrics,
        usable: bool,
        elapsed: f64,
    ) -> Option<usize> {
        if !usable {
            return None;
        }
        lyrics.vocal_index(elapsed + cfg.offset())
    }

    /// What the closed dock shows now. `host_width` is the text host width in pixels;
    /// `measure` returns a text's width (including trailing whitespace and overhang) and
    /// `style_key` must change whenever the font, size, weight or scale changes so the
    /// phrase cache is rebuilt.
    #[allow(clippy::too_many_arguments)]
    pub fn line(
        &mut self,
        cfg: &ClosedLyricsConfig,
        lyrics: &Lyrics,
        usable: bool,
        elapsed: f64,
        duration: f64,
        host_width: f32,
        style_key: u64,
        measure: &dyn Fn(&str) -> f32,
    ) -> ClosedLine {
        let Some(index) = Self::index(cfg, lyrics, usable, elapsed) else {
            return ClosedLine::Waveform;
        };
        let line = &lyrics.lines[index];
        let end = lyrics.lines.get(index + 1).map_or(duration, |next| next.at);
        let at = elapsed + cfg.offset();
        ClosedLine::Text(self.phrase(&line.text, line.at, end, at, host_width, style_key, measure))
    }

    #[allow(clippy::too_many_arguments)]
    fn phrase(
        &mut self,
        line: &str,
        start: f64,
        end: f64,
        at: f64,
        host_width: f32,
        style_key: u64,
        measure: &dyn Fn(&str) -> f32,
    ) -> String {
        let width = host_width.floor() - HOST_PADDING;
        if width.is_nan() || width < 1.0 {
            return String::new();
        }
        let key = format!("{line}|{width}|{style_key}");
        if self.key != key {
            self.phrases = split_phrases(line, width, measure);
            self.key = key;
        }
        pick_phrase(&self.phrases, start, end, at).to_owned()
    }
}

/// Greedy word wrap into phrases no wider than `width`. Words wider than `width` break
/// between text elements (grapheme-like clusters), which also covers unspaced scripts.
pub fn split_phrases(line: &str, width: f32, measure: &dyn Fn(&str) -> f32) -> Vec<String> {
    let mut phrases = Vec::new();
    let mut current = String::new();
    for word in line.split_whitespace() {
        let candidate = if current.is_empty() {
            word.to_owned()
        } else {
            format!("{current} {word}")
        };
        if measure(&candidate) <= width {
            current = candidate;
            continue;
        }
        if !current.is_empty() {
            phrases.push(std::mem::take(&mut current));
        }
        if measure(word) <= width {
            current = word.to_owned();
            continue;
        }
        for element in text_elements(word) {
            if !current.is_empty() && measure(&format!("{current}{element}")) > width {
                phrases.push(std::mem::take(&mut current));
            }
            current.push_str(element);
        }
    }
    if !current.is_empty() {
        phrases.push(current);
    }
    phrases
}

/// Phrase for the playback position `at` inside a line spanning `start..end`.
pub fn pick_phrase(phrases: &[String], start: f64, end: f64, at: f64) -> &str {
    match phrases {
        [] => "",
        [only] => only,
        _ => {
            let total: usize = phrases.iter().map(|p| p.chars().count().max(1)).sum();
            let progress = ((at - start) / (end - start).max(0.1)).clamp(0.0, 0.999_999);
            let progress = if progress.is_nan() { 0.0 } else { progress };
            let target = progress * total as f64;
            let mut cumulative = 0usize;
            for phrase in phrases {
                cumulative += phrase.chars().count().max(1);
                if target < cumulative as f64 {
                    return phrase;
                }
            }
            &phrases[phrases.len() - 1]
        }
    }
}

/// Approximate extended grapheme clusters: combining marks, variation selectors,
/// zero-width joiners (and what they join) and regional-indicator pairs stay together.
fn text_elements(word: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut joined = false;
    let mut regional = 0;
    for (i, c) in word.char_indices() {
        let extends = matches!(c as u32, 0x0300..=0x036F | 0x0483..=0x0489 | 0x0591..=0x05BD
            | 0x064B..=0x065F | 0x0900..=0x0903 | 0x093A..=0x094F | 0x0E31 | 0x0E34..=0x0E3A
            | 0x0E47..=0x0E4E | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF
            | 0xFE00..=0xFE0F | 0x1F3FB..=0x1F3FF | 0xE0020..=0xE007F | 0x3099 | 0x309A)
            || c == '\u{200D}'
            || c == '\u{200C}';
        let regional_indicator = matches!(c as u32, 0x1F1E6..=0x1F1FF);
        let attach = i > 0 && (extends || joined || (regional_indicator && regional % 2 == 1));
        if i > 0 && !attach {
            out.push(&word[start..i]);
            start = i;
        }
        joined = c == '\u{200D}';
        regional = if regional_indicator { regional + 1 } else { 0 };
    }
    if start < word.len() {
        out.push(&word[start..]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::lrc;

    /// 10 px per char, the same as a monospace fixture font.
    fn mono(s: &str) -> f32 {
        s.chars().count() as f32 * 10.0
    }

    fn cfg() -> ClosedLyricsConfig {
        ClosedLyricsConfig::default()
    }

    #[test]
    fn closed_lyrics_falls_back_to_waveform() {
        let mut c = ClosedLyrics::new();
        let empty = Lyrics::default();
        assert_eq!(
            c.line(&cfg(), &empty, true, 10.0, 100.0, 300.0, 0, &mono),
            ClosedLine::Waveform
        );
        let l = lrc::parse(
            "[00:05]First vocal line\n[00:10]\n[00:15][Instrumental]\n[00:20]Next vocal line",
        );
        // Before the first line.
        assert_eq!(
            c.line(&cfg(), &l, true, 1.0, 100.0, 300.0, 0, &mono),
            ClosedLine::Waveform
        );
        // Blank and instrumental breaks.
        for at in [10.0, 12.0, 15.0, 19.9] {
            assert_eq!(
                c.line(&cfg(), &l, true, at, 100.0, 300.0, 0, &mono),
                ClosedLine::Waveform,
                "{at}"
            );
        }
        // Metadata not usable: always the waveform.
        assert_eq!(
            c.line(&cfg(), &l, false, 6.0, 100.0, 300.0, 0, &mono),
            ClosedLine::Waveform
        );
        assert_eq!(
            c.line(&cfg(), &l, true, 6.0, 100.0, 300.0, 0, &mono),
            ClosedLine::Text("First vocal line".into())
        );
        // Seeking back restores vocals.
        assert_eq!(
            c.line(&cfg(), &l, true, 22.0, 100.0, 300.0, 0, &mono),
            ClosedLine::Text("Next vocal line".into())
        );
        assert_eq!(
            c.line(&cfg(), &l, true, 7.0, 100.0, 300.0, 0, &mono),
            ClosedLine::Text("First vocal line".into())
        );
    }

    #[test]
    fn lyrics_offset_shifts_the_selected_line() {
        let l = lrc::parse("[00:05]First\n[00:10]Second");
        let mut c = ClosedLyrics::new();
        let shifted = ClosedLyricsConfig {
            offset_seconds: 3.0,
            ..cfg()
        };
        assert_eq!(
            c.line(&shifted, &l, true, 7.5, 60.0, 300.0, 0, &mono),
            ClosedLine::Text("Second".into())
        );
        let huge = ClosedLyricsConfig {
            offset_seconds: 99.0,
            ..cfg()
        };
        assert_eq!(huge.offset(), 10.0);
        let nan = ClosedLyricsConfig {
            offset_seconds: f64::NAN,
            ..cfg()
        };
        assert_eq!(nan.offset(), 0.0);
    }

    #[test]
    fn long_line_splits_into_phrases() {
        let line = "alpha beta gamma delta epsilon zeta";
        let phrases = split_phrases(line, 120.0, &mono);
        assert_eq!(phrases, ["alpha beta", "gamma delta", "epsilon zeta"]);
        assert!(phrases.iter().all(|p| mono(p) <= 120.0));
        assert_eq!(phrases.join(" "), line);
    }

    #[test]
    fn phrase_advances_with_progress_through_the_line() {
        let l = lrc::parse("[00:10]alpha beta gamma delta epsilon zeta\n[00:20]next");
        let mut c = ClosedLyrics::new();
        // Host 138 px -> 120 px of text.
        let at = |c: &mut ClosedLyrics, t: f64| c.line(&cfg(), &l, true, t, 100.0, 138.0, 1, &mono);
        assert_eq!(at(&mut c, 10.0), ClosedLine::Text("alpha beta".into()));
        assert_eq!(at(&mut c, 14.0), ClosedLine::Text("gamma delta".into()));
        assert_eq!(at(&mut c, 19.99), ClosedLine::Text("epsilon zeta".into()));
        // A changed style key rebuilds the phrases even for the same line.
        let narrow = c.line(&cfg(), &l, true, 10.0, 100.0, 78.0, 2, &mono);
        assert_eq!(narrow, ClosedLine::Text("alpha".into()));
    }

    #[test]
    fn unspaced_and_oversized_words_break_between_elements() {
        let phrases = split_phrases("abcdefghij", 40.0, &mono);
        assert_eq!(phrases, ["abcd", "efgh", "ij"]);
        let jp = split_phrases("朝のサンプルです", 50.0, &mono);
        assert_eq!(jp, ["朝のサンプ", "ルです"]);
        // Combining mark stays with its base; ZWJ sequences are not split.
        let combined = split_phrases("e\u{301}e\u{301}e\u{301}", 20.0, &|s: &str| {
            s.chars().filter(|c| (*c as u32) < 0x300).count() as f32 * 10.0
        });
        assert_eq!(combined, ["e\u{301}e\u{301}", "e\u{301}"]);
        let family = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}";
        assert_eq!(text_elements(family), [family]);
        let flags = "\u{1F1EF}\u{1F1F5}\u{1F1FA}\u{1F1F8}";
        assert_eq!(text_elements(flags).len(), 2);
    }

    #[test]
    fn narrow_host_and_degenerate_inputs_do_not_panic() {
        let l = lrc::parse("[00:05]hello");
        let mut c = ClosedLyrics::new();
        assert_eq!(
            c.line(&cfg(), &l, true, 6.0, 10.0, 10.0, 0, &mono),
            ClosedLine::Text(String::new())
        );
        assert_eq!(
            c.line(&cfg(), &l, true, 6.0, 0.0, f32::NAN, 0, &mono),
            ClosedLine::Text(String::new())
        );
        assert!(split_phrases("", 100.0, &mono).is_empty());
        assert!(split_phrases("   ", 100.0, &mono).is_empty());
        assert_eq!(pick_phrase(&[], 0.0, 1.0, 0.5), "");
        let two = ["ab".to_owned(), "cd".to_owned()];
        assert_eq!(pick_phrase(&two, 5.0, 5.0, 100.0), "cd");
        assert_eq!(pick_phrase(&two, 0.0, 10.0, f64::NAN), "ab");
        assert_eq!(LyricsAnimation::parse("Karaoke"), LyricsAnimation::Karaoke);
        assert_eq!(LyricsAnimation::parse("bad"), LyricsAnimation::Slide);
    }
}
