//! What the voice views read and ask for. The Windows host implements [`VoiceHost`] over its
//! audio and speech services; tests and snapshots use [`NoVoice`] or a scripted host.

use std::sync::Arc;

use naby_core::assistant::State;

/// One finished caption line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceLine {
    pub user: bool,
    pub text: String,
}

/// Wake word status (`AssistantWakeRuntime.WakeStatus`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WakeStatus {
    /// Saved and allowed (AI on, wake word enabled).
    pub enabled: bool,
    /// The wake microphone is on right now.
    pub listening: bool,
    /// Paused by an explicit Stop, a failure or a settings change.
    pub paused: bool,
    pub text: String,
    /// The saved wake phrase.
    pub phrase: String,
}

/// Everything the voice room draws; replaced as a whole when something changes.
#[derive(Debug, Clone, PartialEq)]
pub struct VoiceSnapshot {
    /// A conversation is connecting or running (the microphone is on).
    pub active: bool,
    pub phase: State,
    pub status: String,
    /// "Whistle · built in", "Whisper base · speech pack", ...
    pub engine: String,
    /// "Windows voice", a pack voice, an external server or the realtime provider.
    pub speaker: String,
    /// The engine can run now (false: the reason is in `engine_problem`).
    pub engine_ready: bool,
    pub engine_problem: String,
    /// Recent caption lines, oldest first (bounded by the host).
    pub lines: Vec<VoiceLine>,
    /// The user's words as they are being recognized.
    pub partial: String,
    pub mic_level: f32,
    pub output_level: f32,
    pub timing: Option<String>,
    pub timing_detail: Option<String>,
    pub microphone: String,
    pub wake: WakeStatus,
    /// Voice (and AI) is configured well enough to start.
    pub configured: bool,
    pub setup_hint: String,
    pub sharing: Option<String>,
    pub revision: u64,
}

impl Default for VoiceSnapshot {
    fn default() -> Self {
        Self {
            active: false,
            phase: State::Idle,
            status: "Your microphone stays off until you start.".into(),
            engine: "Whistle · built in".into(),
            speaker: "Windows voice".into(),
            engine_ready: true,
            engine_problem: String::new(),
            lines: Vec::new(),
            partial: String::new(),
            mic_level: 0.0,
            output_level: 0.0,
            timing: None,
            timing_detail: None,
            microphone: "Windows default microphone".into(),
            wake: WakeStatus::default(),
            configured: true,
            setup_hint: String::new(),
            sharing: None,
            revision: 0,
        }
    }
}

/// One input device in the picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MicChoice {
    /// Endpoint id; empty means the Windows default.
    pub id: String,
    pub name: String,
}

/// The four-second microphone test.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MicTest {
    pub running: bool,
    pub level: f32,
    pub result: String,
}

/// Installed packs and the install/remove work in progress (`AssistantSpeechInstall`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SpeechPanel {
    pub busy: bool,
    pub status: String,
    /// 0..=100
    pub progress: f64,
    pub can_cancel: bool,
    /// The last hardware scan.
    pub hardware: Option<naby_core::assistant::speech::Hardware>,
    /// Downloaded listening models.
    pub installed_asr: Vec<InstalledModel>,
    /// Downloaded voices.
    pub installed_tts: Vec<InstalledModel>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledModel {
    pub id: String,
    pub name: String,
    pub size: String,
    pub selected: bool,
}

/// Which half a "Check audio" run exercises.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioCheckKind {
    Microphone,
    Listening,
    Speaking,
}

/// The newest audio check result.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AudioCheck {
    pub running: bool,
    pub level: f32,
    pub message: String,
    pub ok: Option<bool>,
}

/// A Windows speech recognizer the wake word can use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recognizer {
    pub id: String,
    pub name: String,
}

/// The voice runtime as the views see it. Every method is cheap and thread-safe; work happens
/// on the host's own threads and bumps `VoiceSnapshot::revision` through the waker.
pub trait VoiceHost: Send + Sync {
    fn snapshot(&self) -> Arc<VoiceSnapshot>;
    /// Start the conversation (explicit opt-in to the microphone).
    fn start(&self);
    /// End conversation: wake listening resumes when enabled.
    fn end(&self);
    /// Explicit Stop: ends everything and pauses wake listening.
    fn stop(&self);
    fn send_text(&self, text: &str);
    fn microphones(&self) -> Vec<MicChoice>;
    fn mic_test(&self, on: bool);
    fn mic_state(&self) -> MicTest;
    fn set_wake_listening(&self, on: bool);
    fn recognizers(&self) -> Vec<Recognizer>;
    fn speech(&self) -> SpeechPanel;
    fn scan_hardware(&self);
    fn install_pack(&self, asr: &str, tts: &str, gpu: bool);
    fn remove_model(&self, id: &str, tts: bool);
    fn cancel_speech(&self);
    fn run_audio_check(&self, kind: AudioCheckKind);
    fn audio_check(&self) -> AudioCheck;
    /// Short Whistle status for the Models page ("Ready · built in" or the reason).
    fn whistle_status(&self) -> String;
    /// Whether Whistle's files are in place (the Models page offers Install otherwise).
    fn whistle_ready(&self) -> bool;
    /// Download and verify Whistle; progress is reported through [`SpeechPanel`].
    fn install_whistle(&self);
    /// Short status of the built-in Piper voice `voice` ("Ready", "Not installed", ...).
    fn piper_status(&self, voice: &str) -> String;
    /// Whether Piper and `voice` are unpacked and verified.
    fn piper_ready(&self, voice: &str) -> bool;
    /// Install Piper and `voice` (offline when bundled); progress is reported through [`SpeechPanel`].
    fn install_piper(&self, voice: &str);
}

/// A host with no audio at all (the voice room still draws, Start explains).
pub struct NoVoice;

impl VoiceHost for NoVoice {
    fn snapshot(&self) -> Arc<VoiceSnapshot> {
        Arc::new(VoiceSnapshot {
            engine_ready: false,
            engine_problem: "Voice is not available in this window.".into(),
            ..VoiceSnapshot::default()
        })
    }
    fn start(&self) {}
    fn end(&self) {}
    fn stop(&self) {}
    fn send_text(&self, _: &str) {}
    fn microphones(&self) -> Vec<MicChoice> {
        vec![MicChoice {
            id: String::new(),
            name: "Windows default microphone".into(),
        }]
    }
    fn mic_test(&self, _: bool) {}
    fn mic_state(&self) -> MicTest {
        MicTest::default()
    }
    fn set_wake_listening(&self, _: bool) {}
    fn recognizers(&self) -> Vec<Recognizer> {
        Vec::new()
    }
    fn speech(&self) -> SpeechPanel {
        SpeechPanel::default()
    }
    fn scan_hardware(&self) {}
    fn install_pack(&self, _: &str, _: &str, _: bool) {}
    fn remove_model(&self, _: &str, _: bool) {}
    fn cancel_speech(&self) {}
    fn run_audio_check(&self, _: AudioCheckKind) {}
    fn audio_check(&self) -> AudioCheck {
        AudioCheck::default()
    }
    fn whistle_status(&self) -> String {
        "Unavailable".into()
    }
    fn whistle_ready(&self) -> bool {
        false
    }
    fn install_whistle(&self) {}
    fn piper_status(&self, _: &str) -> String {
        "Unavailable".into()
    }
    fn piper_ready(&self, _: &str) -> bool {
        false
    }
    fn install_piper(&self, _: &str) {}
}
