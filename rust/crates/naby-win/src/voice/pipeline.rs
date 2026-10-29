//! The local voice turn pipeline, free of devices and engines so it is tested on Linux with fake
//! parts: microphone chunks go through the energy detector into utterances; an utterance is
//! transcribed (Whistle, a speech pack or an external server), the chat reply streams in, and
//! each finished sentence is synthesized and queued for playback.
//!
//! Capture runs only while the session is active; while a turn is processed the microphone
//! chunks are dropped (`Collector::reset`), as in the C# `LocalAssistantVoice`.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use naby_core::assistant::voice::{
    TurnEvent, TurnState, Vad, VadEvent, VoiceSentences, VoiceTiming, VoiceTurn,
    CAPTURE_SAMPLE_RATE,
};
use naby_core::assistant::State;

use super::whistle::Transcriber;

/// Audio kept from before the detector fired so the first syllable is not cut off.
const PRE_ROLL_MS: usize = 300;
/// A recognized utterance is capped at this many characters (`text[..8000]`).
pub const MAXIMUM_TRANSCRIPT_CHARS: usize = 8000;

pub const NO_SPEECH: &str = "No speech recognized. Try again or adjust microphone sensitivity.";

// ----- utterance collection -------------------------------------------------------------------

/// What a microphone chunk did.
#[derive(Debug, Clone, PartialEq)]
pub enum Collected {
    /// Silence before speech.
    Idle,
    /// Speech in progress.
    Speaking,
    /// A complete utterance (16 kHz mono floats).
    Utterance(Vec<f32>),
    /// A click or breath that was too short to recognize.
    Rejected,
}

/// Energy detector plus the audio of the utterance being collected.
pub struct Collector {
    vad: Vad,
    pre: VecDeque<f32>,
    buf: Vec<f32>,
}

impl Collector {
    pub fn new(sensitivity: f64, silence_ms: i32) -> Self {
        Self {
            vad: Vad::new(sensitivity, silence_ms),
            pre: VecDeque::new(),
            buf: Vec::new(),
        }
    }

    pub fn collecting(&self) -> bool {
        self.vad.collecting()
    }

    /// Drops any partial utterance (a turn started, or the session stopped).
    pub fn reset(&mut self) {
        self.vad.reset();
        self.pre.clear();
        self.buf.clear();
    }

    pub fn push(&mut self, chunk: &[f32]) -> Collected {
        let event = self.vad.push(chunk);
        let limit = CAPTURE_SAMPLE_RATE * PRE_ROLL_MS / 1000;
        match event {
            VadEvent::Idle => {
                self.pre.extend(chunk.iter().copied());
                while self.pre.len() > limit {
                    self.pre.pop_front();
                }
                Collected::Idle
            }
            VadEvent::Speaking | VadEvent::Finished { .. } => {
                if self.buf.is_empty() {
                    self.buf.extend(self.pre.drain(..));
                }
                self.buf.extend_from_slice(chunk);
                match event {
                    VadEvent::Finished { accepted } => {
                        let audio = std::mem::take(&mut self.buf);
                        self.pre.clear();
                        if accepted {
                            Collected::Utterance(audio)
                        } else {
                            Collected::Rejected
                        }
                    }
                    _ => Collected::Speaking,
                }
            }
        }
    }
}

// ----- turn parts -------------------------------------------------------------------------------

/// Generates the spoken reply: sends `text` to the chat provider and feeds the streamed text to
/// `on_text`, which returns false to abort (cancelled).
pub trait Replier {
    fn reply(
        &mut self,
        text: &str,
        on_text: &mut dyn FnMut(&str) -> bool,
        cancel: &AtomicBool,
    ) -> Result<(), String>;
}

/// Turns a sentence into 24 kHz mono 16-bit PCM.
pub trait Speaker {
    fn synthesize(&mut self, text: &str, cancel: &AtomicBool) -> Result<Vec<u8>, String>;
}

/// The playback queue.
pub trait Player {
    /// False when the bounded queue is full.
    fn append(&self, pcm: &[u8]) -> bool;
    fn clear(&self);
    /// Audio still queued, in milliseconds.
    fn pending_ms(&self) -> u32;
}

/// What a turn reports to the session.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Phase(State),
    Status(String),
    /// The recognized user line.
    User(String),
    /// A sentence about to be spoken (live caption).
    Spoken(String),
    Timing(VoiceTiming),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Finished,
    NoSpeech,
    Cancelled,
    Failed(String),
}

/// Options one turn needs.
pub struct TurnOptions<'a> {
    pub language: Option<&'a str>,
    pub keywords: &'a [String],
    pub stream_replies: bool,
}

/// Runs one voice turn to completion (blocking: call from a worker thread).
#[allow(clippy::too_many_arguments)]
pub fn run_turn(
    audio: Option<&[f32]>,
    typed: Option<&str>,
    transcriber: &mut dyn Transcriber,
    replier: &mut dyn Replier,
    speaker: &mut dyn Speaker,
    player: &dyn Player,
    options: &TurnOptions<'_>,
    turn: &mut VoiceTurn,
    cancel: &AtomicBool,
    emit: &mut dyn FnMut(Event),
) -> Outcome {
    let started = Instant::now();
    let ms = |t: Instant| t.elapsed().as_millis() as i64;
    let fail = |turn: &mut VoiceTurn, emit: &mut dyn FnMut(Event), message: String| {
        turn.apply(TurnEvent::Failed);
        emit(Event::Phase(State::Error));
        emit(Event::Status(message.clone()));
        Outcome::Failed(message)
    };

    let text = if let Some(pcm) = audio {
        turn.apply(TurnEvent::Utterance);
        emit(Event::Phase(State::Thinking));
        emit(Event::Status("Transcribing speech…".into()));
        let recognition = Instant::now();
        let transcript = match transcriber.transcribe(pcm, options.language, options.keywords) {
            Ok(t) => t,
            Err(e) => {
                return if cancel.load(Ordering::SeqCst) {
                    Outcome::Cancelled
                } else {
                    fail(turn, emit, e)
                };
            }
        };
        turn.mark_asr(ms(recognition));
        if cancel.load(Ordering::SeqCst) {
            return Outcome::Cancelled;
        }
        let text: String = transcript
            .text
            .trim()
            .chars()
            .take(MAXIMUM_TRANSCRIPT_CHARS)
            .collect();
        if text.is_empty() {
            turn.apply(TurnEvent::NoSpeech);
            emit(Event::Status(NO_SPEECH.into()));
            emit(Event::Phase(State::Listening));
            return Outcome::NoSpeech;
        }
        turn.apply(TurnEvent::Transcribed);
        text
    } else {
        turn.begin_text();
        typed.unwrap_or("").trim().to_owned()
    };
    emit(Event::User(text.clone()));
    emit(Event::Phase(State::Thinking));
    emit(Event::Status("Thinking…".into()));

    let reply_started = Instant::now();
    let mut sentences = VoiceSentences::new(!options.stream_replies);
    let mut chunks = 0usize;
    let mut speak_error: Option<String> = None;
    let mut speak = |sentence: String,
                     turn: &mut VoiceTurn,
                     emit: &mut dyn FnMut(Event),
                     chunks: &mut usize|
     -> Result<(), String> {
        emit(Event::Spoken(sentence.clone()));
        let synthesis = Instant::now();
        let pcm = speaker.synthesize(&sentence, cancel)?;
        if cancel.load(Ordering::SeqCst) {
            return Ok(());
        }
        turn.mark_first_tts(ms(synthesis));
        if !player.append(&pcm) {
            return Err(
                "The spoken reply exceeded the playback limit. Ask for a shorter answer.".into(),
            );
        }
        *chunks += 1;
        turn.mark_audio_ready(ms(started));
        if turn.apply(TurnEvent::AudioQueued) == TurnState::Speaking {
            emit(Event::Phase(State::Speaking));
            emit(Event::Status("Speaking…".into()));
        }
        Ok(())
    };

    let result = {
        let mut on_text = |delta: &str| -> bool {
            if cancel.load(Ordering::SeqCst) || speak_error.is_some() {
                return false;
            }
            if !delta.trim().is_empty() {
                turn.mark_first_text(ms(reply_started));
            }
            for sentence in sentences.append(delta, false) {
                if let Err(e) = speak(sentence, turn, emit, &mut chunks) {
                    speak_error = Some(e);
                    return false;
                }
            }
            true
        };
        replier.reply(&text, &mut on_text, cancel)
    };

    if cancel.load(Ordering::SeqCst) {
        player.clear();
        return Outcome::Cancelled;
    }
    if let Some(e) = speak_error {
        player.clear();
        return fail(turn, emit, e);
    }
    if let Err(e) = result {
        player.clear();
        return fail(turn, emit, e);
    }
    for sentence in sentences.append("", true) {
        if let Err(e) = speak(sentence, turn, emit, &mut chunks) {
            player.clear();
            return fail(turn, emit, e);
        }
    }

    // Let the queued audio finish before the microphone opens again.
    let mut guard = Instant::now();
    while player.pending_ms() > 0 {
        if cancel.load(Ordering::SeqCst) {
            player.clear();
            return Outcome::Cancelled;
        }
        // Playback stalls have a timeout.
        if guard.elapsed() > Duration::from_secs(150) {
            player.clear();
            return fail(
                turn,
                emit,
                "Voice playback stalled. Check Windows audio devices.".into(),
            );
        }
        std::thread::sleep(Duration::from_millis(20));
        if player.pending_ms() > 1000 {
            guard = Instant::now();
        }
    }
    turn.apply(TurnEvent::Finished);
    let timing = turn.finish(ms(started), chunks, options.stream_replies);
    emit(Event::Timing(timing));
    emit(Event::Phase(State::Listening));
    emit(Event::Status(
        "Listening · speak, then pause. Your chat provider generates replies.".into(),
    ));
    Outcome::Finished
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicU32;
    use std::sync::Mutex;

    use super::*;
    use crate::voice::whistle::Transcript;

    fn tone(ms: usize, amp: f32) -> Vec<f32> {
        vec![amp; ms * CAPTURE_SAMPLE_RATE / 1000]
    }

    type Seen = (usize, Option<String>, Vec<String>);

    struct Fake {
        text: Mutex<String>,
        seen: Mutex<Vec<Seen>>,
        fail: bool,
    }

    impl Fake {
        fn saying(text: &str) -> Self {
            Self {
                text: Mutex::new(text.into()),
                seen: Mutex::default(),
                fail: false,
            }
        }
    }

    impl Transcriber for &Fake {
        fn transcribe(
            &mut self,
            pcm: &[f32],
            language: Option<&str>,
            keywords: &[String],
        ) -> Result<Transcript, String> {
            self.seen.lock().unwrap().push((
                pcm.len(),
                language.map(str::to_owned),
                keywords.to_vec(),
            ));
            if self.fail {
                return Err("Whistle: model rejected".into());
            }
            Ok(Transcript {
                text: self.text.lock().unwrap().clone(),
                ..Transcript::default()
            })
        }
    }

    struct Words(Vec<&'static str>, Option<String>);

    impl Replier for Words {
        fn reply(
            &mut self,
            text: &str,
            on_text: &mut dyn FnMut(&str) -> bool,
            _: &AtomicBool,
        ) -> Result<(), String> {
            self.1 = Some(text.to_owned());
            for w in &self.0 {
                if !on_text(w) {
                    break;
                }
            }
            Ok(())
        }
    }

    struct Said(Vec<String>);

    impl Speaker for Said {
        fn synthesize(&mut self, text: &str, _: &AtomicBool) -> Result<Vec<u8>, String> {
            self.0.push(text.to_owned());
            Ok(vec![0u8; 4800])
        }
    }

    struct Queue {
        queued: AtomicU32,
        room: bool,
    }

    impl Player for Queue {
        fn append(&self, pcm: &[u8]) -> bool {
            self.queued.fetch_add(pcm.len() as u32, Ordering::SeqCst);
            self.room
        }
        fn clear(&self) {
            self.queued.store(0, Ordering::SeqCst);
        }
        fn pending_ms(&self) -> u32 {
            // Pretend the speaker has already played everything.
            0
        }
    }

    fn feed(collector: &mut Collector, chunks: &[(usize, f32)]) -> Vec<Collected> {
        chunks
            .iter()
            .map(|(ms, amp)| collector.push(&tone(*ms, *amp)))
            .collect()
    }

    #[test]
    fn silence_is_ignored_and_a_spoken_phrase_becomes_one_utterance() {
        let mut c = Collector::new(0.018, 800);
        assert!(feed(&mut c, &[(100, 0.001); 5])
            .iter()
            .all(|e| *e == Collected::Idle));
        let speech = feed(&mut c, &[(100, 0.2); 4]);
        assert!(speech.iter().all(|e| *e == Collected::Speaking));
        assert!(c.collecting());
        let tail = feed(&mut c, &[(100, 0.0); 8]);
        let Collected::Utterance(audio) = tail.last().unwrap().clone() else {
            panic!("{tail:?}")
        };
        // 300 ms pre-roll + 400 ms speech + 800 ms trailing silence.
        assert_eq!(audio.len(), (300 + 400 + 800) * CAPTURE_SAMPLE_RATE / 1000);
        assert!(!c.collecting());
    }

    #[test]
    fn a_click_is_rejected_and_the_buffer_is_dropped() {
        let mut c = Collector::new(0.018, 400);
        assert_eq!(c.push(&tone(50, 0.5)), Collected::Speaking);
        let rest = feed(&mut c, &[(100, 0.0); 4]);
        assert_eq!(*rest.last().unwrap(), Collected::Rejected);
        assert_eq!(c.push(&tone(100, 0.0)), Collected::Idle);
    }

    #[test]
    fn reset_drops_a_partial_utterance() {
        let mut c = Collector::new(0.018, 800);
        c.push(&tone(100, 0.3));
        assert!(c.collecting());
        c.reset();
        assert!(!c.collecting());
        assert_eq!(c.push(&tone(100, 0.0)), Collected::Idle);
    }

    #[test]
    fn vad_to_transcript_to_spoken_reply() {
        let mut c = Collector::new(0.018, 800);
        feed(&mut c, &[(100, 0.001); 3]);
        feed(&mut c, &[(100, 0.2); 4]);
        let Collected::Utterance(audio) = feed(&mut c, &[(100, 0.0); 8]).pop().unwrap() else {
            panic!()
        };
        let stt = Fake::saying("  what time is it  ");
        let mut transcriber = &stt;
        let mut replier = Words(vec!["It is ", "noon right now. ", "Anything else"], None);
        let mut speaker = Said(Vec::new());
        let player = Queue {
            queued: AtomicU32::new(0),
            room: true,
        };
        let mut turn = VoiceTurn::new();
        let keywords = vec!["Hey Naby".to_owned()];
        let options = TurnOptions {
            language: None,
            keywords: &keywords,
            stream_replies: true,
        };
        let mut events = Vec::new();
        let outcome = run_turn(
            Some(&audio),
            None,
            &mut transcriber,
            &mut replier,
            &mut speaker,
            &player,
            &options,
            &mut turn,
            &AtomicBool::new(false),
            &mut |e| events.push(e),
        );
        assert_eq!(outcome, Outcome::Finished);
        // Whistle received the audio, auto-detect and the keywords.
        let seen = stt.seen.lock().unwrap();
        assert_eq!(seen[0].0, audio.len());
        assert_eq!(seen[0].1, None);
        assert_eq!(seen[0].2, keywords);
        assert_eq!(replier.1.as_deref(), Some("what time is it"));
        assert_eq!(speaker.0, vec!["It is noon right now.", "Anything else"]);
        assert_eq!(player.queued.load(Ordering::SeqCst), 9600);
        assert_eq!(turn.state(), TurnState::Listening);
        let phases: Vec<State> = events
            .iter()
            .filter_map(|e| {
                if let Event::Phase(p) = e {
                    Some(*p)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(phases.first(), Some(&State::Thinking));
        assert!(phases.contains(&State::Speaking));
        assert_eq!(phases.last(), Some(&State::Listening));
        assert!(events.contains(&Event::User("what time is it".into())));
        let Some(Event::Timing(t)) = events.iter().find(|e| matches!(e, Event::Timing(_))) else {
            panic!()
        };
        assert!(t.streamed && t.total_ms >= 0 && t.asr_ms >= 0 && t.first_tts_ms >= 0);
    }

    #[test]
    fn empty_recognition_returns_to_listening_without_a_reply() {
        let stt = Fake::saying("   ");
        let mut transcriber = &stt;
        let mut replier = Words(vec!["never"], None);
        let mut speaker = Said(Vec::new());
        let player = Queue {
            queued: AtomicU32::new(0),
            room: true,
        };
        let mut turn = VoiceTurn::new();
        let options = TurnOptions {
            language: None,
            keywords: &[],
            stream_replies: true,
        };
        let mut events = Vec::new();
        let outcome = run_turn(
            Some(&[0.1; 100]),
            None,
            &mut transcriber,
            &mut replier,
            &mut speaker,
            &player,
            &options,
            &mut turn,
            &AtomicBool::new(false),
            &mut |e| events.push(e),
        );
        assert_eq!(outcome, Outcome::NoSpeech);
        assert!(replier.1.is_none() && speaker.0.is_empty());
        assert!(events.contains(&Event::Status(NO_SPEECH.into())));
        assert_eq!(turn.state(), TurnState::Listening);
    }

    #[test]
    fn a_recognition_failure_stops_the_turn_with_the_reason() {
        let mut stt = Fake::saying("x");
        stt.fail = true;
        let mut transcriber = &stt;
        let mut replier = Words(vec![], None);
        let mut speaker = Said(Vec::new());
        let player = Queue {
            queued: AtomicU32::new(0),
            room: true,
        };
        let mut turn = VoiceTurn::new();
        let options = TurnOptions {
            language: None,
            keywords: &[],
            stream_replies: false,
        };
        let mut events = Vec::new();
        let outcome = run_turn(
            Some(&[0.1; 100]),
            None,
            &mut transcriber,
            &mut replier,
            &mut speaker,
            &player,
            &options,
            &mut turn,
            &AtomicBool::new(false),
            &mut |e| events.push(e),
        );
        assert_eq!(outcome, Outcome::Failed("Whistle: model rejected".into()));
        assert_eq!(turn.state(), TurnState::Failed);
        assert!(events.contains(&Event::Phase(State::Error)));
    }

    #[test]
    fn a_full_playback_queue_fails_the_turn_and_clears_audio() {
        let stt = Fake::saying("hello");
        let mut transcriber = &stt;
        let mut replier = Words(vec!["A long enough sentence here. "], None);
        let mut speaker = Said(Vec::new());
        let player = Queue {
            queued: AtomicU32::new(0),
            room: false,
        };
        let mut turn = VoiceTurn::new();
        let options = TurnOptions {
            language: None,
            keywords: &[],
            stream_replies: true,
        };
        let outcome = run_turn(
            Some(&[0.1; 100]),
            None,
            &mut transcriber,
            &mut replier,
            &mut speaker,
            &player,
            &options,
            &mut turn,
            &AtomicBool::new(false),
            &mut |_| {},
        );
        assert!(matches!(outcome, Outcome::Failed(m) if m.contains("playback limit")));
        assert_eq!(player.queued.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn cancelling_mid_reply_speaks_nothing_more() {
        let stt = Fake::saying("hello");
        let mut transcriber = &stt;
        let cancel = AtomicBool::new(false);
        struct Stopper<'a>(&'a AtomicBool);
        impl Replier for Stopper<'_> {
            fn reply(
                &mut self,
                _: &str,
                on_text: &mut dyn FnMut(&str) -> bool,
                _: &AtomicBool,
            ) -> Result<(), String> {
                on_text("First sentence is here. ");
                self.0.store(true, Ordering::SeqCst);
                assert!(!on_text("Second sentence is here. "));
                Ok(())
            }
        }
        let mut replier = Stopper(&cancel);
        let mut speaker = Said(Vec::new());
        let player = Queue {
            queued: AtomicU32::new(0),
            room: true,
        };
        let mut turn = VoiceTurn::new();
        let options = TurnOptions {
            language: None,
            keywords: &[],
            stream_replies: true,
        };
        let outcome = run_turn(
            Some(&[0.1; 100]),
            None,
            &mut transcriber,
            &mut replier,
            &mut speaker,
            &player,
            &options,
            &mut turn,
            &cancel,
            &mut |_| {},
        );
        assert_eq!(outcome, Outcome::Cancelled);
        assert_eq!(speaker.0.len(), 1);
        assert_eq!(player.queued.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn typed_text_skips_recognition() {
        let stt = Fake::saying("unused");
        let mut transcriber = &stt;
        let mut replier = Words(vec!["Sure thing, done."], None);
        let mut speaker = Said(Vec::new());
        let player = Queue {
            queued: AtomicU32::new(0),
            room: true,
        };
        let mut turn = VoiceTurn::new();
        let options = TurnOptions {
            language: None,
            keywords: &[],
            stream_replies: false,
        };
        let outcome = run_turn(
            None,
            Some(" open weather "),
            &mut transcriber,
            &mut replier,
            &mut speaker,
            &player,
            &options,
            &mut turn,
            &AtomicBool::new(false),
            &mut |_| {},
        );
        assert_eq!(outcome, Outcome::Finished);
        assert!(stt.seen.lock().unwrap().is_empty());
        assert_eq!(replier.1.as_deref(), Some("open weather"));
        assert_eq!(speaker.0, vec!["Sure thing, done."]);
    }
}
