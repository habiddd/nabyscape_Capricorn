//! Setup readiness, the setup overview model and capture summaries
//! (`AssistantReadiness.cs`, `AssistantSetupOverview.cs`, `AssistantExperienceState.cs`).

use std::collections::HashMap;

use chrono::{DateTime, Local};

use super::models::{Options, Provider};
use super::speech::{local_endpoint, local_voice_names, own_asr, own_tts};
use super::{AssistantError, Result};

/// Whether chat or voice is configured and whether a check proved it works.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupState {
    pub configured: bool,
    pub verified: bool,
    pub title: String,
    pub detail: String,
}

impl SetupState {
    fn new(configured: bool, verified: bool, title: &str, detail: &str) -> Self {
        Self {
            configured,
            verified,
            title: title.into(),
            detail: detail.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupReadiness {
    pub chat: SetupState,
    pub voice: SetupState,
}

#[derive(Debug, Clone)]
struct ConnectionCheck {
    configuration: String,
    success: bool,
    checked: DateTime<Local>,
    detail: String,
}

/// Results of connection checks, kept in memory per saved provider. A result only applies while
/// the provider's id, backend, protocol, address and model are unchanged.
#[derive(Debug, Default)]
pub struct ConnectionChecks {
    checks: HashMap<String, ConnectionCheck>,
}

const MAXIMUM_CHECKS: usize = 12;

impl ConnectionChecks {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&mut self, provider: &Provider, success: bool, detail: &str) {
        self.record_at(provider, success, detail, Local::now());
    }

    pub fn record_at(
        &mut self,
        provider: &Provider,
        success: bool,
        detail: &str,
        checked: DateTime<Local>,
    ) {
        if self.checks.len() >= MAXIMUM_CHECKS && !self.checks.contains_key(&provider.id) {
            self.checks.clear();
        }
        self.checks.insert(
            provider.id.clone(),
            ConnectionCheck {
                configuration: provider.connection_identity(),
                success,
                checked,
                detail: detail.into(),
            },
        );
    }

    pub fn forget(&mut self, id: &str) {
        self.checks.remove(id);
    }

    pub fn len(&self) -> usize {
        self.checks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.checks.is_empty()
    }

    /// Chat readiness for a provider (`ChatSetupFor`).
    pub fn chat_setup(&self, chat: Option<&Provider>) -> SetupState {
        let Some(chat) =
            chat.filter(|c| !c.base_url.trim().is_empty() && !c.model.trim().is_empty())
        else {
            return SetupState::new(
                false,
                false,
                "Choose a chat connection",
                "Add an API provider or connect Hermes, then save a chat model.",
            );
        };
        if let Some(check) = self
            .checks
            .get(&chat.id)
            .filter(|c| c.configuration == chat.connection_identity())
        {
            return SetupState {
                configured: true,
                verified: check.success,
                title: if check.success {
                    "Ready · chat checked".into()
                } else {
                    "Connection needs attention".into()
                },
                detail: format!(
                    "{}\nChecked {} on this device. Voice and tools are checked separately.",
                    check.detail,
                    check.checked.format("%H:%M")
                ),
            };
        }
        SetupState {
            configured: true,
            verified: false,
            title: "Saved · not tested".into(),
            detail: format!(
                "{} · {}\nCheck the saved connection to confirm this model can reply.",
                chat.name, chat.model
            ),
        }
    }
}

/// The host's view of speech component work.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SpeechService {
    pub busy: bool,
    pub status: String,
    /// Whether the selected managed pack's files and engines are present.
    pub installed: bool,
}

/// Overall readiness (`AssessSetup`).
pub fn assess_setup(
    options: &Options,
    checks: &ConnectionChecks,
    speech: &SpeechService,
) -> SetupReadiness {
    let chat = checks.chat_setup(options.provider());
    let local = &options.local_voice;
    let voice = if speech.busy {
        SetupState::new(false, false, "Updating speech components", &speech.status)
    } else if !local.enabled {
        let provider = options.voice_provider();
        let configured = provider.is_some_and(|p| {
            !p.realtime_url.trim().is_empty() && !p.realtime_model.trim().is_empty()
        });
        match (configured, provider) {
            (true, Some(p)) => SetupState::new(
                true,
                false,
                "Saved · voice not tested",
                &format!(
                    "The saved realtime model is {}. Start a conversation to connect; the microphone is off until then.",
                    p.realtime_model
                ),
            ),
            _ => SetupState::new(
                false,
                false,
                "Choose a voice connection",
                "Choose automatic local speech, your own models, or a realtime provider. Voice is optional.",
            ),
        }
    } else if !chat.configured {
        SetupState::new(
            false,
            false,
            "A chat model is still needed",
            "Local speech listens and speaks on this PC. An API provider or Hermes generates the reply.",
        )
    } else if local.managed {
        match (own_asr(local), own_tts(local)) {
            (Ok(asr), Ok(tts)) => {
                if speech.installed {
                    SetupState::new(
                        true,
                        false,
                        "Installed · speech not tested",
                        &format!(
                            "{} + {}\nFiles and engines are present. Hardware is checked again when you start voice.",
                            asr.name, tts.name
                        ),
                    )
                } else {
                    SetupState::new(
                        false,
                        false,
                        "Finish local speech setup",
                        "Install the selected pack or prepare your model files in Voice setup.",
                    )
                }
            }
            _ => SetupState::new(
                false,
                false,
                "Check your model files",
                "Open Voice setup to select valid listening and speaking models and prepare their engines.",
            ),
        }
    } else {
        let valid = local_endpoint(&local.asr_url).is_ok()
            && (local.tts_engine == "Windows voice" || local_endpoint(&local.tts_url).is_ok());
        if valid {
            SetupState::new(
                true,
                false,
                "External speech configured",
                "Start your local services and use Check local speech services in Voice setup. Saving an address does not start or verify a model.",
            )
        } else {
            SetupState::new(
                false,
                false,
                "Check local service addresses",
                "Use valid localhost addresses for listening and speaking in Voice setup.",
            )
        }
    };
    SetupReadiness { chat, voice }
}

// ----- setup overview ---------------------------------------------------------------------

/// The settings categories (tab order).
pub const SETUP_SECTIONS: [&str; 5] = ["Overview", "Chat", "Voice", "Hermes", "Advanced"];

/// What "Continue setup" does next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NextSetup {
    EnableCompanion,
    SetUpChat,
    CheckChat,
    OpenChat,
}

impl NextSetup {
    pub fn caption(self) -> &'static str {
        match self {
            Self::EnableCompanion => "Enable AI companion",
            Self::SetUpChat => "Set up chat",
            Self::CheckChat => "Check chat connection",
            Self::OpenChat => "Open chat",
        }
    }
}

pub fn next_setup(options: &Options, ready: &SetupReadiness) -> NextSetup {
    if !options.enabled {
        NextSetup::EnableCompanion
    } else if !ready.chat.configured {
        NextSetup::SetUpChat
    } else if !ready.chat.verified {
        NextSetup::CheckChat
    } else {
        NextSetup::OpenChat
    }
}

pub fn overview_title(ready: &SetupReadiness) -> &'static str {
    if ready.chat.verified {
        "Your companion is ready"
    } else {
        "How would you like to use Naby?"
    }
}

pub const OVERVIEW_DESCRIPTION: &str = "Choose what you want to do. Your saved setup stays available when you switch between chat and voice.";

/// The voice line on the overview.
pub fn voice_readiness_title(voice_connected: bool, ready: &SetupReadiness) -> String {
    if voice_connected {
        "Voice connected · microphone on".into()
    } else {
        ready.voice.title.clone()
    }
}

// ----- capture summary --------------------------------------------------------------------

/// Live capture state for the status line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Capture {
    pub wake_listening: bool,
    pub wake_armed: bool,
    pub wake_enabled: bool,
    pub speech_check_busy: bool,
    pub voice_connected: bool,
    /// Name of the shared window/display, when sharing.
    pub sharing: Option<String>,
}

pub fn capture_summary(c: &Capture) -> String {
    let wake = if c.wake_listening {
        "Wake listening"
    } else if c.wake_armed {
        "Wake preparing"
    } else if c.wake_enabled {
        "Wake paused"
    } else {
        "Wake off"
    };
    let mic = if c.speech_check_busy {
        "Speech test active"
    } else if c.voice_connected {
        "Mic on"
    } else {
        "Mic off"
    };
    let screen = c.sharing.as_ref().map_or_else(
        || "Screen not shared".to_string(),
        |name| format!("Sharing {name}"),
    );
    format!("{wake} · {mic} · {screen}")
}

/// The multi-line current-setup summary shown under the overview.
pub fn setup_summary(options: &Options, capture: &Capture) -> String {
    let (listening, speaking) = local_voice_names(&options.local_voice);
    format!(
        "AI replies: {}\nListening: {}\nSpeaking: {}\n{}",
        options
            .provider()
            .map_or("Not configured", |p| p.name.as_str()),
        listening,
        speaking,
        capture_summary(capture)
    )
}

/// Busy flags that stop a speech check from starting.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Busy {
    pub speech_check: bool,
    pub speech_install: bool,
    pub voice_connected: bool,
    pub chat: bool,
}

/// `BeginSpeechCheck`'s gate.
pub fn can_begin_speech_check(busy: Busy) -> Result<()> {
    if busy.speech_check || busy.speech_install || busy.voice_connected || busy.chat {
        return Err(AssistantError::invalid(
            "Finish the current voice operation first.",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn provider(model: &str) -> Provider {
        Provider {
            name: "Mine".into(),
            base_url: "https://api.example/v1/".into(),
            model: model.into(),
            ..Provider::default()
        }
    }

    fn options(p: &Provider) -> Options {
        Options {
            selected_provider: p.id.clone(),
            providers: vec![p.clone()],
            ..Options::default()
        }
    }

    #[test]
    fn chat_readiness_tracks_the_saved_connection() {
        let mut checks = ConnectionChecks::new();
        let empty = checks.chat_setup(None);
        assert_eq!(
            (empty.configured, empty.verified, empty.title.as_str()),
            (false, false, "Choose a chat connection")
        );
        assert!(!checks.chat_setup(Some(&provider(" "))).configured);
        let mut p = provider("gpt");
        let saved = checks.chat_setup(Some(&p));
        assert_eq!(saved.title, "Saved · not tested");
        assert_eq!(
            saved.detail,
            "Mine · gpt\nCheck the saved connection to confirm this model can reply."
        );
        let at = Local.with_ymd_and_hms(2026, 10, 29, 9, 5, 0).unwrap();
        checks.record_at(&p, true, "Mine · gpt returned a chat response.", at);
        let ready = checks.chat_setup(Some(&p));
        assert!(ready.configured && ready.verified);
        assert_eq!(ready.title, "Ready · chat checked");
        assert_eq!(
            ready.detail,
            "Mine · gpt returned a chat response.\nChecked 09:05 on this device. Voice and tools are checked separately."
        );
        // Any change to the saved connection invalidates the check.
        p.model = "other".into();
        assert_eq!(checks.chat_setup(Some(&p)).title, "Saved · not tested");
        p.model = "gpt".into();
        p.base_url = "https://elsewhere.example/v1/".into();
        assert!(!checks.chat_setup(Some(&p)).verified);
        p.base_url = "https://api.example/v1/".into();
        checks.record_at(&p, false, "HTTP 401", at);
        let failed = checks.chat_setup(Some(&p));
        assert!(
            (failed.configured, failed.verified) == (true, false)
                && failed.title == "Connection needs attention"
        );
        checks.forget(&p.id);
        assert!(checks.is_empty());
        // The memory is bounded.
        for _ in 0..12 {
            checks.record(&provider("m"), true, "ok");
        }
        assert_eq!(checks.len(), 12);
        checks.record(&provider("m"), true, "ok");
        assert_eq!(checks.len(), 1);
    }

    #[test]
    fn voice_readiness_covers_each_mode() {
        let p = provider("gpt");
        let mut o = options(&p);
        let checks = ConnectionChecks::new();
        let idle = SpeechService::default();
        // Realtime provider.
        assert_eq!(
            assess_setup(&o, &checks, &idle).voice.title,
            "Choose a voice connection"
        );
        o.providers[0].realtime_url = "wss://api.example/rt".into();
        o.providers[0].realtime_model = "rt-1".into();
        let v = assess_setup(&o, &checks, &idle).voice;
        assert_eq!(
            (v.configured, v.title.as_str()),
            (true, "Saved · voice not tested")
        );
        assert!(v.detail.contains("rt-1"));
        // Speech work in progress wins.
        let busy = SpeechService {
            busy: true,
            status: "Downloading 40%".into(),
            installed: false,
        };
        let v = assess_setup(&o, &checks, &busy).voice;
        assert_eq!(
            (v.configured, v.title.as_str(), v.detail.as_str()),
            (false, "Updating speech components", "Downloading 40%")
        );
        // Local speech needs a chat model first.
        o.local_voice.enabled = true;
        let no_chat = Options {
            providers: vec![],
            selected_provider: String::new(),
            ..o.clone()
        };
        assert_eq!(
            assess_setup(&no_chat, &checks, &idle).voice.title,
            "A chat model is still needed"
        );
        // Managed pack.
        o.local_voice.managed = true;
        assert_eq!(
            assess_setup(&o, &checks, &idle).voice.title,
            "Finish local speech setup"
        );
        let installed = SpeechService {
            installed: true,
            ..SpeechService::default()
        };
        let v = assess_setup(&o, &checks, &installed).voice;
        assert_eq!(v.title, "Installed · speech not tested");
        assert!(v
            .detail
            .starts_with("Whisper base · everyday + Windows voice"));
        o.local_voice.own_models = true;
        o.local_voice.own_asr_path = "relative".into();
        assert_eq!(
            assess_setup(&o, &checks, &installed).voice.title,
            "Check your model files"
        );
        // External services.
        o.local_voice.managed = false;
        o.local_voice.own_models = false;
        assert_eq!(
            assess_setup(&o, &checks, &idle).voice.title,
            "External speech configured"
        );
        o.local_voice.asr_url = "https://remote.example/".into();
        assert_eq!(
            assess_setup(&o, &checks, &idle).voice.title,
            "Check local service addresses"
        );
        o.local_voice.asr_url = "http://127.0.0.1:8080/".into();
        o.local_voice.tts_engine = "Kokoro".into();
        o.local_voice.tts_url = "http://example.com/".into();
        assert_eq!(
            assess_setup(&o, &checks, &idle).voice.title,
            "Check local service addresses"
        );
        o.local_voice.tts_engine = "Windows voice".into();
        assert!(assess_setup(&o, &checks, &idle).voice.configured);
    }

    #[test]
    fn overview_steps_and_summaries() {
        let p = provider("gpt");
        let mut o = options(&p);
        let mut checks = ConnectionChecks::new();
        let ready =
            |o: &Options, c: &ConnectionChecks| assess_setup(o, c, &SpeechService::default());
        assert_eq!(next_setup(&o, &ready(&o, &checks)), NextSetup::CheckChat);
        assert_eq!(
            overview_title(&ready(&o, &checks)),
            "How would you like to use Naby?"
        );
        checks.record(&p, true, "ok");
        assert_eq!(next_setup(&o, &ready(&o, &checks)), NextSetup::OpenChat);
        assert_eq!(
            overview_title(&ready(&o, &checks)),
            "Your companion is ready"
        );
        assert_eq!(NextSetup::OpenChat.caption(), "Open chat");
        o.enabled = false;
        assert_eq!(
            next_setup(&o, &ready(&o, &checks)),
            NextSetup::EnableCompanion
        );
        o.enabled = true;
        o.providers.clear();
        assert_eq!(next_setup(&o, &ready(&o, &checks)), NextSetup::SetUpChat);
        assert_eq!(NextSetup::SetUpChat.caption(), "Set up chat");
        assert_eq!(
            voice_readiness_title(true, &ready(&o, &checks)),
            "Voice connected · microphone on"
        );
        assert_eq!(
            voice_readiness_title(false, &ready(&o, &checks)),
            "Choose a voice connection"
        );
        assert_eq!(SETUP_SECTIONS[0], "Overview");

        let mut capture = Capture::default();
        assert_eq!(
            capture_summary(&capture),
            "Wake off · Mic off · Screen not shared"
        );
        capture.wake_enabled = true;
        assert_eq!(
            capture_summary(&capture),
            "Wake paused · Mic off · Screen not shared"
        );
        capture.wake_armed = true;
        assert!(capture_summary(&capture).starts_with("Wake preparing"));
        capture.wake_listening = true;
        capture.voice_connected = true;
        capture.sharing = Some("Display 1".into());
        assert_eq!(
            capture_summary(&capture),
            "Wake listening · Mic on · Sharing Display 1"
        );
        capture.speech_check_busy = true;
        assert!(capture_summary(&capture).contains("Speech test active"));
        let o = options(&p);
        assert_eq!(
            setup_summary(&o, &Capture::default()),
            "AI replies: Mine\nListening: Realtime provider\nSpeaking: Realtime provider\nWake off · Mic off · Screen not shared"
        );
        assert!(setup_summary(&Options::default(), &Capture::default())
            .starts_with("AI replies: Not configured"));
    }

    #[test]
    fn speech_checks_wait_for_other_work() {
        assert!(can_begin_speech_check(Busy::default()).is_ok());
        for busy in [
            Busy {
                speech_check: true,
                ..Busy::default()
            },
            Busy {
                speech_install: true,
                ..Busy::default()
            },
            Busy {
                voice_connected: true,
                ..Busy::default()
            },
            Busy {
                chat: true,
                ..Busy::default()
            },
        ] {
            assert_eq!(
                can_begin_speech_check(busy).unwrap_err().to_string(),
                "Finish the current voice operation first."
            );
        }
    }
}
