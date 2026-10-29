//! Voice logic that needs no audio device: activity state, utterance detection, sentence
//! splitting, turn phases/timings, realtime endpoint rules and the wake phrase
//! (`AssistantVoiceActivity.cs`, `AssistantVoiceTurn.cs`, `LocalAssistantVoice.Capture`,
//! `AssistantVoiceSession` statics, `AssistantWakeWord.cs`).
//!
//! The host feeds microphone chunks to [`Vad`], plays back what the speech engines return and
//! owns the realtime WebSocket; none of that is needed to test the rules here.

use url::Url;

use super::attachments::{is_valid_base64, MAXIMUM_INLINE_IMAGE};
use super::markdown::spoken;
use super::models::{Options, State, WakeOptions};
use super::providers::{escape_data_string, validate_endpoint};
use super::{len16, AssistantError, Result};

// ----- activity ---------------------------------------------------------------------------

const SPEECH_THRESHOLD: f32 = 0.015;
const PLAYBACK_HOLD_MS: i64 = 220;

/// Tracks whether the companion is listening, thinking or audibly speaking. Microphone energy
/// and audible playback are distinct signals (`AssistantVoiceActivity`).
#[derive(Debug, Default, Clone)]
pub struct VoiceActivity {
    playback_until: i64,
    thinking: bool,
    microphone: f32,
    output: f32,
}

impl VoiceActivity {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn thinking(&mut self) {
        self.thinking = true;
        self.playback_until = 0;
        self.output = 0.0;
    }

    /// A transcript line arrived: the user's turn starts thinking, the reply's ends it.
    pub fn transcript(&mut self, role_is_user: bool) {
        if role_is_user {
            self.thinking();
        } else {
            self.thinking = false;
        }
    }

    /// A level meter reading; `playback` distinguishes output from microphone peaks.
    pub fn level(&mut self, peak: f32, playback: bool, now_ms: i64) {
        let peak = if peak.is_finite() {
            peak.clamp(0.0, 1.0)
        } else {
            0.0
        };
        if !playback {
            self.microphone = peak;
            return;
        }
        self.output = peak;
        if peak >= SPEECH_THRESHOLD {
            self.playback_until = now_ms + PLAYBACK_HOLD_MS;
            self.thinking = false;
        }
    }

    pub fn state(&self, now_ms: i64) -> State {
        if now_ms < self.playback_until {
            State::Speaking
        } else if self.thinking {
            State::Thinking
        } else {
            State::Listening
        }
    }

    pub fn energy(&self, now_ms: i64) -> f32 {
        match self.state(now_ms) {
            State::Speaking => self.output,
            _ if self.thinking => 0.0,
            _ => self.microphone,
        }
    }
}

/// Peak amplitude (0..=1) of 16-bit little-endian PCM, including negative full scale.
pub fn peak_pcm16(bytes: &[u8]) -> f32 {
    let mut max = 0i32;
    for pair in bytes.chunks_exact(2) {
        let sample = i16::from_le_bytes([pair[0], pair[1]]) as i32;
        max = max.max(sample.abs());
    }
    max as f32 / 32768.0
}

// ----- utterance detection ----------------------------------------------------------------

/// Sample rate the local voice pipeline records at.
pub const CAPTURE_SAMPLE_RATE: usize = 16000;
/// A single utterance is cut at this length.
pub const MAXIMUM_UTTERANCE_MS: i64 = 25_000;
/// An utterance needs this many loud chunks to be sent for recognition.
pub const MINIMUM_VOICED_CHUNKS: u32 = 3;
/// Spoken replies are capped at two minutes of 24 kHz mono 16-bit audio.
pub const MAXIMUM_SPOKEN_PCM_BYTES: usize = 24000 * 2 * 120;

/// What a microphone chunk did to the current utterance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VadEvent {
    /// Nothing audible yet: do not keep this chunk.
    Idle,
    /// Part of an utterance in progress: keep this chunk.
    Speaking,
    /// The utterance ended with this chunk (keep it). `accepted` is false for clicks and
    /// breaths that were too short to recognize.
    Finished { accepted: bool },
}

/// Energy-based utterance detector (`LocalAssistantVoice.Capture`).
#[derive(Debug, Clone)]
pub struct Vad {
    sensitivity: f32,
    silence_ms: i64,
    heard: bool,
    quiet_ms: i64,
    voiced: u32,
    utterance_ms: i64,
}

impl Vad {
    /// `sensitivity` is clamped to 0.003..=0.15 and `silence_ms` to 400..=2000.
    pub fn new(sensitivity: f64, silence_ms: i32) -> Self {
        let sensitivity = if sensitivity.is_finite() {
            sensitivity
        } else {
            0.018
        };
        Self {
            sensitivity: sensitivity.clamp(0.003, 0.15) as f32,
            silence_ms: silence_ms.clamp(400, 2000) as i64,
            heard: false,
            quiet_ms: 0,
            voiced: 0,
            utterance_ms: 0,
        }
    }

    pub fn reset(&mut self) {
        self.heard = false;
        self.quiet_ms = 0;
        self.voiced = 0;
        self.utterance_ms = 0;
    }

    /// Whether an utterance is being collected.
    pub fn collecting(&self) -> bool {
        self.heard
    }

    /// Process one chunk of mono 16 kHz samples in -1.0..=1.0.
    pub fn push(&mut self, samples: &[f32]) -> VadEvent {
        let peak = samples
            .iter()
            .map(|s| if s.is_finite() { s.abs() } else { 0.0 })
            .fold(0.0f32, f32::max);
        let ms = (samples.len() * 1000 / CAPTURE_SAMPLE_RATE) as i64;
        if peak >= self.sensitivity {
            self.heard = true;
            self.quiet_ms = 0;
            self.voiced += 1;
        } else if self.heard {
            self.quiet_ms += ms;
        }
        if !self.heard {
            return VadEvent::Idle;
        }
        self.utterance_ms += ms;
        if self.quiet_ms >= self.silence_ms || self.utterance_ms >= MAXIMUM_UTTERANCE_MS {
            let accepted = self.voiced >= MINIMUM_VOICED_CHUNKS;
            self.reset();
            return VadEvent::Finished { accepted };
        }
        VadEvent::Speaking
    }
}

// ----- spoken sentences -------------------------------------------------------------------

const SPOKEN_LIMIT: usize = 6000;

/// Splits a streamed reply into sentence-sized speech segments. Punctuation survives token
/// boundaries, fenced code is skipped, CJK punctuation, decimals and a short list of titles are
/// recognized (`VoiceSentences`).
#[derive(Debug, Default)]
pub struct VoiceSentences {
    pending: Vec<char>,
    whole_reply: bool,
    fenced: bool,
    ticks: u32,
    spoken: usize,
    truncated: bool,
}

impl VoiceSentences {
    /// `whole_reply`: synthesize once at the end (legacy one-shot engines).
    pub fn new(whole_reply: bool) -> Self {
        Self {
            whole_reply,
            ..Self::default()
        }
    }

    /// True when the spoken reply was cut at 6000 characters.
    pub fn truncated(&self) -> bool {
        self.truncated
    }

    pub fn append(&mut self, text: &str, is_final: bool) -> Vec<String> {
        let mut out = Vec::new();
        for c in text.chars() {
            if c == '`' {
                self.ticks += 1;
                if self.ticks == 3 {
                    self.fenced = !self.fenced;
                    self.ticks = 0;
                }
                continue;
            }
            self.ticks = 0;
            if !self.fenced {
                self.pending.push(c);
            }
        }
        if self.whole_reply {
            if !is_final {
                return out;
            }
            let full = spoken(&self.pending.iter().collect::<String>());
            self.pending.clear();
            let total = full.chars().count();
            self.truncated = total > SPOKEN_LIMIT;
            if total > 0 {
                out.push(full.chars().take(SPOKEN_LIMIT).collect());
            }
            return out;
        }
        while !self.pending.is_empty() && self.spoken < SPOKEN_LIMIT {
            let mut boundary = 0usize;
            let scan = self.pending.len().min(360);
            for i in 0..scan {
                let c = self.pending[i];
                let mut end = matches!(c, '。' | '！' | '？' | '\n')
                    || (matches!(c, '.' | '!' | '?')
                        && ((i + 1 < self.pending.len() && self.pending[i + 1].is_whitespace())
                            || (is_final && i + 1 == self.pending.len())));
                if c == '.' && end {
                    let mut start = i;
                    while start > 0 && self.pending[start - 1].is_alphabetic() {
                        start -= 1;
                    }
                    let word: String = self.pending[start..i]
                        .iter()
                        .collect::<String>()
                        .to_lowercase();
                    if matches!(
                        word.as_str(),
                        "mr" | "mrs" | "ms" | "dr" | "prof" | "st" | "vs" | "etc"
                    ) {
                        end = false;
                    }
                }
                if end && i >= 12 {
                    boundary = i + 1;
                    break;
                }
            }
            if boundary == 0 && self.pending.len() > 360 {
                boundary = 360;
                while boundary > 200 && !self.pending[boundary - 1].is_whitespace() {
                    boundary -= 1;
                }
            }
            if boundary == 0 {
                if !is_final {
                    break;
                }
                boundary = self.pending.len();
            }
            let head: String = self.pending.drain(..boundary).collect();
            let sentence = spoken(&head);
            if sentence.is_empty() {
                continue;
            }
            let length = sentence.chars().count();
            let take = length.min(SPOKEN_LIMIT - self.spoken);
            if take < length {
                self.truncated = true;
            }
            self.spoken += take;
            if take > 0 {
                out.push(sentence.chars().take(take).collect());
            }
        }
        if self.spoken >= SPOKEN_LIMIT && !self.pending.is_empty() {
            self.truncated = true;
            self.pending.clear();
        }
        out
    }
}

// ----- turn phases and timing -------------------------------------------------------------

/// Milliseconds of each stage of a local voice turn; `-1` means the stage did not happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoiceTiming {
    pub turn: i64,
    pub asr_ms: i64,
    pub first_text_ms: i64,
    pub first_tts_ms: i64,
    pub audio_ready_ms: i64,
    pub total_ms: i64,
    pub streamed: bool,
}

fn seconds(ms: i64) -> String {
    if ms < 0 {
        "—".into()
    } else {
        format!("{:.1}s", ms as f64 / 1000.0)
    }
}

impl VoiceTiming {
    pub fn summary(&self) -> String {
        format!(
            "ASR {} · Reply {} · TTS {}",
            seconds(self.asr_ms),
            seconds(self.first_text_ms),
            seconds(self.first_tts_ms)
        )
    }

    pub fn detail(&self) -> String {
        format!(
            "Speech recognition: {}\nReply preparation to first accepted text: {}\nFirst voice segment synthesis: {}\nRecognition start to first queued audio: {}\nTurn including playback: {}\nReply includes shared-context preparation and tool work when used. Excludes the silence-detection pause and hardware output latency. Timings stay in memory; speech content is not logged.",
            seconds(self.asr_ms),
            seconds(self.first_text_ms),
            seconds(self.first_tts_ms),
            seconds(self.audio_ready_ms),
            seconds(self.total_ms)
        )
    }
}

/// What a local voice turn is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnState {
    Listening,
    Recognizing,
    Replying,
    Speaking,
    Failed,
    Stopped,
}

impl TurnState {
    /// The orb/status phase shown for this state.
    pub fn phase(self) -> State {
        match self {
            Self::Listening => State::Listening,
            Self::Recognizing | Self::Replying => State::Thinking,
            Self::Speaking => State::Speaking,
            Self::Failed => State::Error,
            Self::Stopped => State::Idle,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnEvent {
    /// The detector closed an accepted utterance.
    Utterance,
    /// Recognition returned nothing.
    NoSpeech,
    /// Recognition returned text; the chat reply starts.
    Transcribed,
    /// The first synthesized segment was queued for playback.
    AudioQueued,
    /// The reply finished (or text-only turn completed).
    Finished,
    Failed,
    Stop,
}

/// The local voice turn state machine, with timing marks (`LocalAssistantVoice.Turn`).
#[derive(Debug, Clone)]
pub struct VoiceTurn {
    state: TurnState,
    serial: i64,
    timing: VoiceTiming,
}

impl Default for VoiceTurn {
    fn default() -> Self {
        Self::new()
    }
}

impl VoiceTurn {
    pub fn new() -> Self {
        Self {
            state: TurnState::Listening,
            serial: 0,
            timing: VoiceTiming {
                turn: 0,
                asr_ms: -1,
                first_text_ms: -1,
                first_tts_ms: -1,
                audio_ready_ms: -1,
                total_ms: -1,
                streamed: false,
            },
        }
    }

    pub fn state(&self) -> TurnState {
        self.state
    }

    /// True while a turn is being processed (the microphone waits).
    pub fn processing(&self) -> bool {
        matches!(
            self.state,
            TurnState::Recognizing | TurnState::Replying | TurnState::Speaking
        )
    }

    pub fn timing(&self) -> VoiceTiming {
        self.timing
    }

    /// Apply an event; impossible transitions are ignored and leave the state unchanged.
    pub fn apply(&mut self, event: TurnEvent) -> TurnState {
        use TurnEvent as E;
        use TurnState as S;
        self.state = match (self.state, event) {
            (S::Stopped, _) => S::Stopped,
            (_, E::Stop) => S::Stopped,
            (S::Failed, _) => S::Failed,
            (_, E::Failed) => S::Failed,
            (S::Listening, E::Utterance) => {
                self.serial += 1;
                self.timing = VoiceTurn::new().timing;
                self.timing.turn = self.serial;
                S::Recognizing
            }
            (S::Recognizing, E::NoSpeech) => S::Listening,
            (S::Recognizing, E::Transcribed) => S::Replying,
            (S::Replying, E::AudioQueued) => S::Speaking,
            (S::Replying | S::Speaking, E::Finished) => S::Listening,
            (state, _) => state,
        };
        self.state
    }

    /// Typed text (no recognition) starts directly at the reply stage.
    pub fn begin_text(&mut self) {
        if self.state == TurnState::Listening {
            self.serial += 1;
            self.timing = VoiceTurn::new().timing;
            self.timing.turn = self.serial;
            self.state = TurnState::Replying;
        }
    }

    pub fn mark_asr(&mut self, ms: i64) {
        self.timing.asr_ms = ms;
    }

    /// First non-blank reply text arrived `ms` after the reply started.
    pub fn mark_first_text(&mut self, ms: i64) {
        if self.timing.first_text_ms < 0 {
            self.timing.first_text_ms = ms;
        }
    }

    pub fn mark_first_tts(&mut self, ms: i64) {
        if self.timing.first_tts_ms < 0 {
            self.timing.first_tts_ms = ms;
        }
    }

    pub fn mark_audio_ready(&mut self, ms: i64) {
        if self.timing.audio_ready_ms < 0 {
            self.timing.audio_ready_ms = ms;
        }
    }

    /// `chunks > 1` streamed replies count as streamed.
    pub fn finish(&mut self, total_ms: i64, chunks: usize, stream_replies: bool) -> VoiceTiming {
        self.timing.streamed = stream_replies && chunks > 1;
        self.timing.total_ms = total_ms;
        self.timing
    }
}

// ----- realtime voice ---------------------------------------------------------------------

pub const REALTIME_PROTOCOLS: [&str; 2] = ["OpenAIRealtime", "GeminiLive"];

/// The documented WebSocket URL for a realtime protocol.
pub fn default_realtime_url(protocol: &str) -> &'static str {
    match protocol {
        "OpenAIRealtime" => "wss://api.openai.com/v1/realtime",
        "GeminiLive" => "wss://generativelanguage.googleapis.com/ws/google.ai.generativelanguage.v1beta.GenerativeService.BidiGenerateContent",
        _ => "",
    }
}

fn http_equivalent(uri: &Url, keep_query: bool) -> Result<Url> {
    let rest = &uri.as_str()[uri.scheme().len()..];
    let scheme = if uri.scheme() == "wss" {
        "https"
    } else {
        "http"
    };
    let mut converted = Url::parse(&format!("{scheme}{rest}"))
        .map_err(|_| AssistantError::invalid("Enter a valid realtime WebSocket URL."))?;
    if !keep_query {
        converted.set_query(None);
    }
    Ok(converted)
}

/// Check a realtime WebSocket URL the user saved: `wss` (or loopback `ws`), no credentials,
/// query or fragment, and a public or localhost destination.
pub fn validate_realtime_url(value: &str) -> Result<()> {
    let invalid = || AssistantError::invalid("Enter a valid realtime WebSocket URL.");
    let uri = Url::parse(value).map_err(|_| invalid())?;
    if !matches!(uri.scheme(), "ws" | "wss")
        || !uri.username().is_empty()
        || uri.password().is_some()
        || uri.fragment().is_some()
    {
        return Err(invalid());
    }
    validate_endpoint(http_equivalent(&uri, true)?.as_str())?;
    Ok(())
}

/// The URL a realtime session connects to (`AssistantVoiceSession.ValidateEndpoint`): OpenAI-style
/// endpoints carry only `?model=<configured model>`; Gemini carries no query at all.
pub fn realtime_endpoint(value: &str, model: &str, gemini: bool) -> Result<Url> {
    let uri = Url::parse(value)
        .ok()
        .filter(|u| matches!(u.scheme(), "ws" | "wss"))
        .ok_or_else(|| {
            AssistantError::invalid(
                "Configure a secure WSS voice endpoint or loopback WS endpoint.",
            )
        })?;
    validate_endpoint(http_equivalent(&uri, false)?.as_str())?;
    if let Some(query) = uri.query().filter(|q| !q.is_empty()) {
        let expected = format!("model={}", escape_data_string(model));
        if gemini || query != expected {
            return Err(AssistantError::invalid(
                "Voice endpoints must not contain API keys or other query parameters.",
            ));
        }
    }
    let mut result = uri;
    if gemini {
        result.set_query(None);
    } else {
        result.set_query(Some(&format!("model={}", escape_data_string(model))));
    }
    Ok(result)
}

/// Validate a shared-screen frame for a realtime session (`ValidateImage`).
pub fn validate_voice_image(data_url: &str) -> Result<()> {
    let comma = data_url.find(',');
    let bounded = len16(data_url) <= MAXIMUM_INLINE_IMAGE;
    let typed = data_url.starts_with("data:image/jpeg;base64,")
        || data_url.starts_with("data:image/png;base64,");
    match comma {
        Some(c) if bounded && typed => {
            if is_valid_base64(&data_url[c + 1..]) {
                Ok(())
            } else {
                Err(AssistantError::invalid("The shared image is invalid."))
            }
        }
        _ => Err(AssistantError::invalid(
            "Supply a bounded JPEG or PNG image data URL.",
        )),
    }
}

// ----- wake word --------------------------------------------------------------------------

pub const DEFAULT_WAKE_PHRASE: &str = "Hey Naby";

/// Normalize and validate a wake phrase: 3 to 64 letters, spaces, apostrophes or hyphens.
pub fn wake_phrase(value: &str) -> Result<String> {
    let phrase = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let n = len16(&phrase);
    if !(3..=64).contains(&n)
        || phrase
            .chars()
            .any(|c| !c.is_alphabetic() && !matches!(c, ' ' | '-' | '\''))
    {
        return Err(AssistantError::invalid(
            "Use a wake phrase of 3–64 letters, spaces, apostrophes or hyphens.",
        ));
    }
    Ok(phrase)
}

/// The phrases the recognizer grammar accepts (the default phrase also takes common mishearings).
pub fn wake_grammar(phrase: &str) -> Vec<String> {
    if phrase.eq_ignore_ascii_case(DEFAULT_WAKE_PHRASE) {
        vec![
            phrase.to_string(),
            "Hey Nabi".into(),
            "Hey Nabby".into(),
            "Hey Navy".into(),
        ]
    } else {
        vec![phrase.to_string()]
    }
}

/// Confidence needed to activate, clamped to 55 to 95 percent.
pub fn wake_threshold(confidence: f64) -> f64 {
    if confidence.is_finite() {
        confidence.clamp(0.55, 0.95)
    } else {
        0.72
    }
}

/// Whether a voice connection is complete enough to arm the wake word (`WakeVoiceReady`).
/// `voice_configured` is the readiness of the selected speech setup.
pub fn wake_voice_ready(options: &Options, voice_configured: bool) -> bool {
    let selected = if options.local_voice.enabled {
        options.provider()
    } else {
        options.voice_provider()
    };
    voice_configured
        && selected.is_some_and(|p| {
            if options.local_voice.enabled {
                !p.model.trim().is_empty()
            } else {
                !p.realtime_model.trim().is_empty() && !p.realtime_url.trim().is_empty()
            }
        })
}

/// Validate a wake word save (`SaveWakeWord`) and return the options to store.
pub fn save_wake_word(
    options: &Options,
    mut next: WakeOptions,
    voice_configured: bool,
) -> Result<WakeOptions> {
    next.phrase = wake_phrase(&next.phrase)?;
    next.confidence = wake_threshold(next.confidence);
    if next.enabled && !options.enabled {
        return Err(AssistantError::invalid(
            "Enable AI companion before enabling the wake word.",
        ));
    }
    if next.enabled && !wake_voice_ready(options, voice_configured) {
        return Err(AssistantError::invalid(
            "Choose and save a working voice connection before enabling the wake word. Local speech needs a selected chat model; realtime voice needs a model and WebSocket URL.",
        ));
    }
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assistant::models::Provider;

    fn chunk(amplitude: f32, ms: usize) -> Vec<f32> {
        vec![amplitude; ms * CAPTURE_SAMPLE_RATE / 1000]
    }

    #[test]
    fn vad_detects_speech_then_silence() {
        let mut vad = Vad::new(0.018, 800);
        for _ in 0..10 {
            assert_eq!(vad.push(&chunk(0.001, 100)), VadEvent::Idle);
        }
        assert!(!vad.collecting());
        for _ in 0..3 {
            assert_eq!(vad.push(&chunk(0.2, 100)), VadEvent::Speaking);
        }
        assert!(vad.collecting());
        // A short pause keeps the utterance open; speech resumes the count.
        for _ in 0..7 {
            assert_eq!(vad.push(&chunk(0.0, 100)), VadEvent::Speaking);
        }
        assert_eq!(vad.push(&chunk(0.2, 100)), VadEvent::Speaking);
        for _ in 0..7 {
            assert_eq!(vad.push(&chunk(0.0, 100)), VadEvent::Speaking);
        }
        assert_eq!(
            vad.push(&chunk(0.0, 100)),
            VadEvent::Finished { accepted: true }
        );
        assert!(!vad.collecting());
        // The next utterance starts clean.
        assert_eq!(vad.push(&chunk(0.0, 100)), VadEvent::Idle);
    }

    #[test]
    fn vad_rejects_clicks_caps_long_speech_and_clamps_settings() {
        let mut vad = Vad::new(0.018, 800);
        assert_eq!(vad.push(&chunk(0.5, 20)), VadEvent::Speaking);
        let mut last = VadEvent::Idle;
        for _ in 0..40 {
            last = vad.push(&chunk(0.0, 20));
            if matches!(last, VadEvent::Finished { .. }) {
                break;
            }
        }
        assert_eq!(last, VadEvent::Finished { accepted: false });
        // Continuous speech is cut at 25 seconds.
        let mut vad = Vad::new(0.018, 800);
        let mut finished_at = None;
        for i in 0..300 {
            if let VadEvent::Finished { accepted } = vad.push(&chunk(0.3, 100)) {
                assert!(accepted);
                finished_at = Some(i);
                break;
            }
        }
        assert_eq!(finished_at, Some(249));
        // Settings are clamped: a huge sensitivity still hears loud speech, silence below 400 ms
        // is raised, and non-finite audio counts as silence.
        let mut vad = Vad::new(9.0, 1);
        assert_eq!(vad.push(&chunk(0.2, 100)), VadEvent::Speaking);
        assert_eq!(vad.push(&[f32::NAN; 1600]), VadEvent::Speaking);
        let mut vad = Vad::new(f64::NAN, 100);
        for _ in 0..3 {
            vad.push(&chunk(0.5, 100));
        }
        for _ in 0..3 {
            assert_eq!(vad.push(&chunk(0.0, 100)), VadEvent::Speaking);
        }
        assert_eq!(
            vad.push(&chunk(0.0, 100)),
            VadEvent::Finished { accepted: true }
        );
    }

    #[test]
    fn voice_activity_preserves_thinking_and_audible_speech() {
        let mut a = VoiceActivity::new();
        a.level(0.0, true, 1000);
        assert_eq!(a.state(1000), State::Listening);
        a.thinking();
        for i in 0..20 {
            let now = 1000 + i * 50;
            a.level(0.8, false, now);
            a.level(0.0, true, now);
            assert_eq!(a.state(now), State::Thinking);
        }
        a.level(0.7, true, 2000);
        a.level(0.2, false, 2050);
        assert_eq!(a.state(2050), State::Speaking);
        assert_eq!(a.energy(2050), 0.7);
        a.transcript(false);
        a.level(0.0, true, 2100);
        a.level(1.0, false, 2150);
        assert_eq!(a.state(2150), State::Speaking);
        assert_eq!(a.state(2221), State::Listening);
        a.transcript(true);
        a.level(0.0, true, 2300);
        a.level(0.9, false, 2350);
        assert_eq!(a.state(2350), State::Thinking);
        a.transcript(false);
        assert_eq!(a.state(2400), State::Listening);
        a.level(0.01, true, 2450);
        assert_eq!(a.state(2450), State::Listening);
        a.level(f32::NAN, true, 2500);
        a.level(1.0, true, 2550);
        a.reset();
        assert_eq!(a.state(2550), State::Listening);
        assert_eq!(a.energy(2550), 0.0);
    }

    #[test]
    fn pcm_meter_measures_signed_samples_including_negative_full_scale() {
        assert_eq!(peak_pcm16(&[0, 0, 0, 128]), 1.0);
        assert_eq!(peak_pcm16(&[0; 20]), 0.0);
        assert_eq!(peak_pcm16(&[0xff, 0x7f]), 32767.0 / 32768.0);
        assert_eq!(peak_pcm16(&[1]), 0.0);
    }

    fn sentences(parts: &[&str], whole: bool) -> (Vec<String>, bool) {
        let mut s = VoiceSentences::new(whole);
        let mut out = Vec::new();
        for part in parts {
            out.extend(s.append(part, false));
        }
        out.extend(s.append("", true));
        (out, s.truncated())
    }

    #[test]
    fn sentences_are_split_across_token_boundaries() {
        let (out, truncated) = sentences(
            &[
                "Hello the",
                "re, this is a first sentence. And a sec",
                "ond one! Last bit",
            ],
            false,
        );
        assert_eq!(
            out,
            [
                "Hello there, this is a first sentence.",
                "And a second one!",
                "Last bit"
            ]
        );
        assert!(!truncated);
        // Titles and decimals do not end a sentence.
        let (out, _) = sentences(
            &["Ask Dr. Smith about version 3.5 of the plan today. Done."],
            false,
        );
        assert_eq!(
            out,
            [
                "Ask Dr. Smith about version 3.5 of the plan today.",
                "Done."
            ]
        );
        // Fenced code is skipped, Markdown punctuation removed.
        let (out, _) = sentences(
            &["Use **bold** text here.\n```\nlet x = 1;\n```\nThen continue with more."],
            false,
        );
        assert_eq!(out, ["Use bold text here.", "Then continue with more."]);
        // CJK punctuation splits.
        let (out, _) = sentences(&["これは最初の文章です。これは二番目の文章です。"], false);
        assert!(!out.is_empty() && out[0].ends_with('。'));
        // Whole-reply engines get one segment.
        let (out, _) = sentences(&["One sentence. ", "Two sentences."], true);
        assert_eq!(out, ["One sentence. Two sentences."]);
    }

    #[test]
    fn long_replies_are_chunked_and_capped_at_six_thousand_characters() {
        let word = "word ".repeat(2000);
        let (out, truncated) = sentences(&[&word], false);
        assert!(out.iter().all(|s| s.chars().count() <= 360));
        assert!(out.iter().map(|s| s.chars().count()).sum::<usize>() <= 6000);
        assert!(truncated);
        let (out, truncated) = sentences(&[&"x".repeat(7000)], true);
        assert_eq!(out[0].chars().count(), 6000);
        assert!(truncated);
        let (out, truncated) = sentences(&["short"], false);
        assert_eq!(out, ["short"]);
        assert!(!truncated);
    }

    #[test]
    fn turns_move_through_the_documented_phases_with_timings() {
        let mut turn = VoiceTurn::new();
        assert_eq!(turn.state().phase(), State::Listening);
        assert!(!turn.processing());
        assert_eq!(turn.apply(TurnEvent::Utterance), TurnState::Recognizing);
        assert!(turn.processing());
        turn.mark_asr(860);
        assert_eq!(turn.apply(TurnEvent::Transcribed).phase(), State::Thinking);
        turn.mark_first_text(1200);
        turn.mark_first_text(9999);
        turn.mark_first_tts(300);
        turn.mark_audio_ready(2350);
        assert_eq!(turn.apply(TurnEvent::AudioQueued).phase(), State::Speaking);
        assert_eq!(turn.apply(TurnEvent::Finished), TurnState::Listening);
        let timing = turn.finish(5400, 3, true);
        assert!(timing.streamed);
        assert_eq!(timing.summary(), "ASR 0.9s · Reply 1.2s · TTS 0.3s");
        assert!(timing.detail().contains("Turn including playback: 5.4s"));
        let silent = VoiceTurn::new().timing();
        assert_eq!(silent.summary(), "ASR — · Reply — · TTS —");
        // No speech returns to listening; typed text skips recognition; impossible moves are ignored.
        let mut turn = VoiceTurn::new();
        turn.apply(TurnEvent::Utterance);
        assert_eq!(turn.apply(TurnEvent::NoSpeech), TurnState::Listening);
        assert_eq!(turn.apply(TurnEvent::AudioQueued), TurnState::Listening);
        turn.begin_text();
        assert_eq!(turn.state(), TurnState::Replying);
        assert_eq!(turn.timing().turn, 2);
        assert_eq!(turn.apply(TurnEvent::Failed).phase(), State::Error);
        assert_eq!(turn.apply(TurnEvent::Finished), TurnState::Failed);
        assert_eq!(turn.apply(TurnEvent::Stop), TurnState::Stopped);
        assert_eq!(turn.apply(TurnEvent::Utterance), TurnState::Stopped);
    }

    #[test]
    fn realtime_endpoints_reject_remote_ws_credentials_private_ip_and_api_key_queries() {
        for endpoint in [
            "http://provider.example",
            "ws://provider.example/",
            "wss://u:p@provider.example/",
            "wss://192.168.1.2/",
            "wss://provider.example/?key=secret",
            "wss://provider.example/#secret",
            "wss://provider.example/?model=other",
            "wss://provider.example/?model=configured-model&key=secret",
        ] {
            assert!(
                matches!(
                    realtime_endpoint(endpoint, "configured-model", false),
                    Err(AssistantError::Invalid(_))
                ),
                "{endpoint}"
            );
        }
        assert_eq!(
            realtime_endpoint("ws://127.0.0.1:1234/realtime", "configured-model", false)
                .unwrap()
                .query(),
            Some("model=configured-model")
        );
        assert_eq!(
            realtime_endpoint(
                "wss://provider.example/realtime?model=configured-model",
                "configured-model",
                false
            )
            .unwrap()
            .path(),
            "/realtime"
        );
        assert_eq!(
            realtime_endpoint("wss://provider.example/realtime", "a b", false)
                .unwrap()
                .query(),
            Some("model=a%20b")
        );
        assert_eq!(
            realtime_endpoint("wss://provider.example/live", "m", true)
                .unwrap()
                .query(),
            None
        );
        assert!(realtime_endpoint("wss://provider.example/live?model=m", "m", true).is_err());
        assert_eq!(REALTIME_PROTOCOLS, ["OpenAIRealtime", "GeminiLive"]);
        assert_eq!(
            default_realtime_url("OpenAIRealtime"),
            "wss://api.openai.com/v1/realtime"
        );
        assert!(default_realtime_url("GeminiLive")
            .starts_with("wss://generativelanguage.googleapis.com/ws/"));
        assert_eq!(default_realtime_url("x"), "");
        assert!(validate_realtime_url("wss://provider.example/realtime").is_ok());
        assert!(validate_realtime_url("ws://localhost:1234/realtime").is_ok());
        for bad in [
            "https://x.example/",
            "wss://a:b@x.example/",
            "wss://x.example/#f",
            "wss://x.example/?model=m",
            "ws://x.example/",
            "wss://10.0.0.1/",
        ] {
            assert!(validate_realtime_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn shared_frames_must_be_bounded_inline_images() {
        assert!(validate_voice_image("data:image/jpeg;base64,AQID").is_ok());
        assert!(validate_voice_image("data:image/png;base64,AQID").is_ok());
        assert_eq!(
            validate_voice_image("data:image/gif;base64,AQID")
                .unwrap_err()
                .to_string(),
            "Supply a bounded JPEG or PNG image data URL."
        );
        assert_eq!(
            validate_voice_image("data:image/jpeg;base64,AQ=")
                .unwrap_err()
                .to_string(),
            "The shared image is invalid."
        );
        let huge = format!("data:image/jpeg;base64,{}", "A".repeat(3 * 1024 * 1024));
        assert!(validate_voice_image(&huge).is_err());
    }

    #[test]
    fn wake_phrase_rules() {
        assert_eq!(wake_phrase("  Hey   Naby ").unwrap(), "Hey Naby");
        assert_eq!(wake_phrase("O'Brien-ok").unwrap(), "O'Brien-ok");
        for bad in ["hi", "", "a1b2c", "Hey! Naby", &"a".repeat(65)] {
            assert!(wake_phrase(bad).is_err(), "{bad}");
        }
        assert_eq!(wake_grammar("hey naby").len(), 4);
        assert_eq!(wake_grammar("Okay Computer"), ["Okay Computer"]);
        assert_eq!(wake_threshold(0.1), 0.55);
        assert_eq!(wake_threshold(2.0), 0.95);
        assert_eq!(wake_threshold(f64::NAN), 0.72);
    }

    #[test]
    fn wake_word_needs_a_ready_voice_connection() {
        let mut provider = Provider {
            model: "chat".into(),
            realtime_model: "rt".into(),
            realtime_url: "wss://p.example/rt".into(),
            ..Provider::default()
        };
        let mut options = Options {
            selected_provider: provider.id.clone(),
            providers: vec![provider.clone()],
            ..Options::default()
        };
        let enable = WakeOptions {
            enabled: true,
            phrase: " Hey  Naby".into(),
            confidence: 0.99,
            ..WakeOptions::default()
        };
        assert!(wake_voice_ready(&options, true));
        assert!(!wake_voice_ready(&options, false));
        let saved = save_wake_word(&options, enable.clone(), true).unwrap();
        assert_eq!(
            (saved.phrase.as_str(), saved.confidence),
            ("Hey Naby", 0.95)
        );
        assert!(save_wake_word(&options, enable.clone(), false).is_err());
        // Local speech needs a chat model instead.
        options.local_voice.enabled = true;
        assert!(wake_voice_ready(&options, true));
        provider.model.clear();
        options.providers = vec![provider];
        assert!(!wake_voice_ready(&options, true));
        // Disabling never needs a connection; enabling needs the companion on.
        let off = WakeOptions {
            enabled: false,
            ..enable.clone()
        };
        assert!(save_wake_word(&options, off, false).is_ok());
        options.enabled = false;
        assert!(save_wake_word(&options, enable, true).is_err());
    }
}
