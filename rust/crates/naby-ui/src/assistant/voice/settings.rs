//! The AI settings voice pages (`AssistantVoiceWorkspace`): **Voice setup**, **Wake word**,
//! **Models** and **Check audio**. The Settings window embeds [`VoiceSettingsView`] in its AI
//! category; the assistant window's setup shows the same view.
//!
//! Choices are saved through [`AssistantService::edit_options`]; installing, removing and checking
//! happen in the host and are reported back as [`SpeechPanel`] / [`AudioCheck`] snapshots.

use std::sync::Arc;

use naby_core::assistant::models::{LocalVoiceOptions, Options, WakeOptions};
use naby_core::assistant::speech::{
    asr_details, local_endpoint, local_voice_names, own_asr, own_tts, recommend_pack,
    use_speech_pack, voice_details, OWN_ASR_ENGINES, OWN_TTS_ENGINES,
};
use naby_core::assistant::voice::{save_wake_word, wake_threshold};
use naby_core::command::Command;
use naby_render::{TextEditOpts, Ui, Weight};

use super::engine::{self, Listening};
use super::model::{AudioCheckKind, VoiceHost};
use super::room::mic_test_line;
use crate::assistant::providers::{field, pill};
use crate::assistant::service::AssistantService;

/// The sub-pages (`Setup`, `Wake`, `Models`, `Audio`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VoicePage {
    Setup,
    Wake,
    Models,
    Audio,
}

impl VoicePage {
    pub const ALL: [VoicePage; 4] = [
        VoicePage::Setup,
        VoicePage::Wake,
        VoicePage::Models,
        VoicePage::Audio,
    ];

    pub fn label(self) -> &'static str {
        match self {
            VoicePage::Setup => "Voice setup",
            VoicePage::Wake => "Wake word",
            VoicePage::Models => "Models",
            VoicePage::Audio => "Check audio",
        }
    }
}

#[derive(Debug, Default)]
pub struct VoiceSettingsOut {
    pub commands: Vec<Command>,
}

const ENGINES: [(Listening, &str); 5] = [
    (Listening::Whistle, "Whistle \u{b7} built in"),
    (Listening::Pack, "Speech pack on this PC"),
    (Listening::External, "External server"),
    (Listening::OwnFiles, "My model files"),
    (Listening::Realtime, "Realtime provider"),
];
const SPEECH_LANGUAGES: [&str; 16] = [
    "en", "de", "fr", "es", "it", "nl", "pl", "id", "pt", "ja", "ko", "ru", "ar", "tr", "hi", "vi",
];
const ASR_PROTOCOLS: [&str; 2] = ["WhisperCpp", "OpenAI-compatible"];
const TTS_ENGINES: [&str; 3] = ["Windows voice", "Kokoro", "Piper"];
const PREFERENCES: [&str; 3] = ["Light", "Balanced", "Quality"];

/// Draft of the setup page (edited fields are applied by Save).
#[derive(Default)]
struct SetupDraft {
    seeded: bool,
    engine: usize,
    local: LocalVoiceOptions,
    language: usize,
    asr_protocol: usize,
    tts_engine: usize,
    own_asr_engine: usize,
    own_tts_engine: usize,
    sensitivity: f32,
    silence: f32,
    note: String,
    advanced: bool,
}

#[derive(Default)]
struct WakeDraft {
    seeded: bool,
    wake: WakeOptions,
    confidence: f32,
    recognizers: Vec<(String, String)>,
    pick: usize,
    note: String,
}

#[derive(Default)]
struct ModelsDraft {
    preference: usize,
    language: usize,
    /// `(listening id, voice id, gpu)` waiting for the install to finish.
    pending: Option<(String, String, bool)>,
    confirm_remove: Option<(String, bool)>,
    note: String,
}

pub struct VoiceSettingsView {
    service: AssistantService,
    host: Arc<dyn VoiceHost>,
    page: VoicePage,
    setup: SetupDraft,
    wake: WakeDraft,
    models: ModelsDraft,
    seen_service: u64,
    seen_voice: u64,
}

impl VoiceSettingsView {
    pub fn new(service: AssistantService, host: Arc<dyn VoiceHost>) -> Self {
        Self {
            service,
            host,
            page: VoicePage::Setup,
            setup: SetupDraft::default(),
            wake: WakeDraft::default(),
            models: ModelsDraft {
                preference: 1,
                ..ModelsDraft::default()
            },
            seen_service: 0,
            seen_voice: u64::MAX,
        }
    }

    pub fn page(&self) -> VoicePage {
        self.page
    }

    pub fn open(&mut self, page: VoicePage) {
        self.page = page;
    }

    /// A worker published (install progress, a check result, a status change).
    pub fn dirty(&self) -> bool {
        self.seen_service != self.service.revision()
            || self.host.snapshot().revision != self.seen_voice
            || self.host.speech().busy
            || self.host.audio_check().running
            || self.host.mic_state().running
    }

    /// The settings window was closed or switched away: stop tests and drop drafts that hold
    /// nothing worth keeping.
    pub fn on_hide(&mut self) {
        self.host.mic_test(false);
        self.setup.seeded = false;
        self.wake.seeded = false;
        self.wake.recognizers.clear();
    }

    pub fn show(&mut self, ui: &mut Ui<'_>, out: &mut VoiceSettingsOut) {
        self.seen_service = self.service.revision();
        self.seen_voice = self.host.snapshot().revision;
        ui.row(|ui| {
            ui.spacing(4.0);
            for page in VoicePage::ALL {
                let clicked = if page == self.page {
                    ui.primary_button(page.label()).clicked
                } else {
                    ui.button(page.label()).clicked
                };
                if clicked {
                    self.page = page;
                }
            }
        });
        ui.gap(6.0);
        let ready = self.service.setup();
        ui.label_dim(&format!("{}\n{}", ready.voice.title, ready.voice.detail));
        ui.gap(6.0);
        match self.page {
            VoicePage::Setup => self.show_setup(ui, out),
            VoicePage::Wake => self.show_wake(ui, out),
            VoicePage::Models => self.show_models(ui),
            VoicePage::Audio => self.show_audio(ui),
        }
    }

    // ---- voice setup ----------------------------------------------------------------------

    pub(crate) fn seed_setup(&mut self) {
        if self.setup.seeded {
            return;
        }
        let options = self.service.options();
        let local = options.local_voice.clone();
        let mut resolved = engine::resolve(&local).listening;
        // Nothing set up yet: Whistle is the default listening engine.
        let realtime_ready = options.voice_provider().is_some_and(|p| {
            !p.realtime_url.trim().is_empty() && !p.realtime_model.trim().is_empty()
        });
        if resolved == Listening::Realtime && !realtime_ready {
            resolved = Listening::Whistle;
        }
        self.setup = SetupDraft {
            seeded: true,
            engine: ENGINES
                .iter()
                .position(|(e, _)| *e == resolved)
                .unwrap_or(0),
            language: SPEECH_LANGUAGES
                .iter()
                .position(|c| *c == local.speech_language)
                .unwrap_or(0),
            asr_protocol: ASR_PROTOCOLS
                .iter()
                .position(|p| *p == local.asr_protocol)
                .unwrap_or(0),
            tts_engine: TTS_ENGINES
                .iter()
                .position(|p| *p == local.tts_engine)
                .unwrap_or(0),
            own_asr_engine: OWN_ASR_ENGINES
                .iter()
                .position(|p| *p == local.own_asr_engine)
                .unwrap_or(1),
            own_tts_engine: OWN_TTS_ENGINES
                .iter()
                .position(|p| *p == local.own_tts_engine)
                .unwrap_or(0),
            sensitivity: local.sensitivity as f32,
            silence: local.silence_ms as f32,
            local,
            note: String::new(),
            advanced: false,
        };
    }

    fn show_setup(&mut self, ui: &mut Ui<'_>, _out: &mut VoiceSettingsOut) {
        self.seed_setup();
        let options = self.service.options();
        ui.heading_sized("How should Naby hear and speak?", 20.0);
        ui.label_dim(
            "Choose where your voice is processed. Your chat connection supplies the answers.",
        );
        let mut enabled = options.enabled;
        if ui.toggle(&mut enabled, "AI companion").changed {
            self.setup.note = match self.service.set_enabled(enabled) {
                Ok(()) => String::new(),
                Err(e) => e,
            };
        }
        ui.section_label("Listening");
        let labels: Vec<&str> = ENGINES.iter().map(|(_, l)| *l).collect();
        let mut pick = self.setup.engine;
        if ui.combo(&mut pick, &labels).changed {
            self.setup.engine = pick;
        }
        let selected = ENGINES[self.setup.engine.min(ENGINES.len() - 1)].0;
        match selected {
            Listening::Whistle => {
                ui.label_dim(&format!(
                    "Whistle runs on this PC and detects the language itself: {}. It loads when you start a conversation and unloads a minute after it ends. {}",
                    engine::whistle_language_names(),
                    self.host.whistle_status()
                ));
            }
            Listening::Pack => {
                let (listening, speaking) = local_voice_names(&self.setup.local);
                ui.label_dim(&format!(
                    "Selected pack: {listening} + {speaking}. Install or change it in Models."
                ));
            }
            Listening::External => self.external_fields(ui),
            Listening::OwnFiles => self.own_fields(ui),
            Listening::Realtime => {
                let provider = options.voice_provider();
                let text = match provider {
                    Some(p) if !p.realtime_model.is_empty() => format!(
                        "Uses the realtime model {} saved for {}. API usage may be billed by your provider.",
                        p.realtime_model, p.name
                    ),
                    _ => "Uses the realtime model and voice saved for a provider. Configure one in AI setup \u{2192} Chat.".to_owned(),
                };
                ui.label_dim(&text);
            }
            Listening::NeedsSetup => {}
        }

        if matches!(
            selected,
            Listening::Whistle | Listening::External | Listening::Pack
        ) {
            ui.section_label("Language");
            let names: Vec<&str> = SPEECH_LANGUAGES
                .iter()
                .map(|c| engine::language_label(c))
                .collect();
            let mut lang = self.setup.language;
            if ui.combo(&mut lang, &names).changed {
                self.setup.language = lang;
            }
            let code = SPEECH_LANGUAGES[self.setup.language.min(SPEECH_LANGUAGES.len() - 1)];
            if selected == Listening::Whistle && !engine::whistle_supports(code) {
                let invalid = ui.theme().invalid;
                let style = ui.text_style(11.0, Weight::Regular, invalid);
                let text = format!(
                    "Whistle covers {}. For {} choose a speech pack or an external server.",
                    engine::whistle_language_names(),
                    engine::language_label(code)
                );
                ui.label_styled(&text, &style);
            }
        }
        if matches!(selected, Listening::Whistle | Listening::External) {
            self.speaking_fields(ui);
        }
        if selected != Listening::Realtime {
            self.tuning_fields(ui);
        }
        ui.gap(6.0);
        if pill(ui, "Save voice setup", true) {
            self.setup.note = match self.save_setup() {
                Ok(()) => "Voice setup saved. The next conversation uses it.".into(),
                Err(e) => e,
            };
        }
        if !self.setup.note.is_empty() {
            let note = self.setup.note.clone();
            ui.label_dim(&note);
        }
    }

    fn speaking_fields(&mut self, ui: &mut Ui<'_>) {
        ui.section_label("Speaking");
        let mut pick = self.setup.tts_engine;
        if ui.combo(&mut pick, &TTS_ENGINES).changed {
            self.setup.tts_engine = pick;
        }
        if self.setup.tts_engine == 0 {
            ui.label_dim("Uses your default installed Windows voice. No TTS server is needed. Voice and language availability depends on Windows.");
        } else {
            field(
                ui,
                "Text-to-speech address (localhost)",
                &mut self.setup.local.tts_url,
                200,
            );
            field(ui, "TTS model", &mut self.setup.local.tts_model, 160);
            field(ui, "Voice", &mut self.setup.local.voice, 160);
        }
    }

    fn external_fields(&mut self, ui: &mut Ui<'_>) {
        ui.label_dim("Connect a speech service running on this PC (127.0.0.1). The optional local ASR bridge script is described in the local voice guide.");
        ui.section_label("ASR server type");
        let mut pick = self.setup.asr_protocol;
        if ui.combo(&mut pick, &ASR_PROTOCOLS).changed {
            self.setup.asr_protocol = pick;
        }
        field(ui, "ASR server address", &mut self.setup.local.asr_url, 200);
        field(
            ui,
            "Listening model ID",
            &mut self.setup.local.asr_model,
            160,
        );
    }

    fn own_fields(&mut self, ui: &mut Ui<'_>) {
        ui.label_dim("Use model files you already have. Files stay where they are; Naby never copies or deletes them.");
        ui.section_label("Listening model");
        let mut a = self.setup.own_asr_engine;
        if ui.combo(&mut a, &OWN_ASR_ENGINES).changed {
            self.setup.own_asr_engine = a;
        }
        field(
            ui,
            "Listening model path",
            &mut self.setup.local.own_asr_path,
            520,
        );
        ui.section_label("Speaking model");
        let mut t = self.setup.own_tts_engine;
        if ui.combo(&mut t, &OWN_TTS_ENGINES).changed {
            self.setup.own_tts_engine = t;
        }
        field(
            ui,
            "Speaking model path",
            &mut self.setup.local.own_tts_path,
            520,
        );
    }

    fn tuning_fields(&mut self, ui: &mut Ui<'_>) {
        let label = format!(
            "{} Microphone tuning",
            if self.setup.advanced {
                "\u{25BE}"
            } else {
                "\u{25B8}"
            }
        );
        if ui.button(&label).clicked {
            self.setup.advanced = !self.setup.advanced;
        }
        if !self.setup.advanced {
            return;
        }
        ui.section_label("Speech threshold \u{b7} lower for a quiet microphone");
        ui.slider(&mut self.setup.sensitivity, 0.003..=0.15, 0.001);
        ui.section_label("Pause before sending \u{b7} milliseconds");
        ui.slider(&mut self.setup.silence, 400.0..=2000.0, 50.0);
        let mut stream = self.setup.local.stream_replies;
        if ui
            .toggle(&mut stream, "Stream replies for faster speech")
            .changed
        {
            self.setup.local.stream_replies = stream;
        }
        ui.label_dim("Uses streamed text when your chat endpoint supports it. Saving ends the current voice session.");
    }

    /// Validates the draft and saves it (`Save voice setup`).
    pub(crate) fn save_setup(&mut self) -> Result<(), String> {
        self.seed_setup();
        let selected = ENGINES[self.setup.engine.min(ENGINES.len() - 1)].0;
        let mut local = self.setup.local.clone();
        local.speech_language =
            SPEECH_LANGUAGES[self.setup.language.min(SPEECH_LANGUAGES.len() - 1)].into();
        local.asr_protocol = ASR_PROTOCOLS[self.setup.asr_protocol.min(1)].into();
        local.tts_engine = TTS_ENGINES[self.setup.tts_engine.min(2)].into();
        local.own_asr_engine = OWN_ASR_ENGINES[self.setup.own_asr_engine.min(3)].into();
        local.own_tts_engine = OWN_TTS_ENGINES[self.setup.own_tts_engine.min(5)].into();
        local.sensitivity = f64::from(self.setup.sensitivity).clamp(0.003, 0.15);
        local.silence_ms = self.setup.silence.round().clamp(400.0, 2000.0) as i32;
        if local.asr_model.len() > 160 || local.tts_model.len() > 160 || local.voice.len() > 160 {
            return Err("Model and voice IDs must be 160 characters or fewer.".into());
        }
        match selected {
            Listening::External => {
                local_endpoint(&local.asr_url).map_err(|e| e.message().to_owned())?;
                if local.tts_engine != "Windows voice" {
                    local_endpoint(&local.tts_url).map_err(|e| e.message().to_owned())?;
                }
            }
            Listening::Whistle => {
                if local.tts_engine != "Windows voice" {
                    local_endpoint(&local.tts_url).map_err(|e| e.message().to_owned())?;
                }
            }
            Listening::OwnFiles => {
                let mut probe = local.clone();
                probe.own_models = true;
                probe.managed = true;
                own_asr(&probe).map_err(|e| e.message().to_owned())?;
                own_tts(&probe).map_err(|e| e.message().to_owned())?;
            }
            _ => {}
        }
        self.service.edit_options(|options| {
            let mut next = engine::apply_choice(options, selected);
            let target = &mut next.local_voice;
            target.speech_language = local.speech_language.clone();
            target.sensitivity = local.sensitivity;
            target.silence_ms = local.silence_ms;
            target.stream_replies = local.stream_replies;
            if matches!(selected, Listening::External | Listening::Whistle) {
                target.tts_engine = local.tts_engine.clone();
                target.tts_url = local.tts_url.clone();
                target.tts_model = local.tts_model.clone();
                target.voice = local.voice.clone();
            }
            if selected == Listening::External {
                target.asr_protocol = local.asr_protocol.clone();
                target.asr_url = local.asr_url.clone();
                target.asr_model = local.asr_model.clone();
            }
            if selected == Listening::OwnFiles {
                target.own_asr_engine = local.own_asr_engine.clone();
                target.own_asr_path = local.own_asr_path.clone();
                target.own_tts_engine = local.own_tts_engine.clone();
                target.own_tts_path = local.own_tts_path.clone();
            }
            *options = next;
            Ok(())
        })?;
        self.setup.seeded = false;
        // Saving a voice setting ends the current voice session (and re-arms wake listening).
        if self.host.snapshot().active {
            self.host.end();
        }
        Ok(())
    }

    // ---- wake word ------------------------------------------------------------------------

    fn seed_wake(&mut self) {
        if self.wake.seeded {
            return;
        }
        let options = self.service.options();
        self.wake.wake = options.wake_word.clone();
        self.wake.confidence = wake_threshold(options.wake_word.confidence) as f32;
        self.wake.recognizers = self
            .host
            .recognizers()
            .into_iter()
            .map(|r| (r.id, r.name))
            .collect();
        self.wake.pick = self
            .wake
            .recognizers
            .iter()
            .position(|(id, _)| *id == options.wake_word.recognizer_id)
            .map_or(0, |i| i + 1);
        self.wake.seeded = true;
    }

    fn show_wake(&mut self, ui: &mut Ui<'_>, out: &mut VoiceSettingsOut) {
        self.seed_wake();
        let snap = self.host.snapshot();
        ui.heading_sized("Wake Naby with your voice", 20.0);
        ui.label_dim("Choose a phrase and turn hands-free listening on or off. Detection uses an installed Windows speech recognizer, separately from your listening and speaking models. Wake audio never leaves this PC.");
        let mut enabled = self.wake.wake.enabled;
        if ui
            .toggle(&mut enabled, "Enable wake word on this PC")
            .changed
        {
            self.wake.wake.enabled = enabled;
        }
        field(ui, "Wake phrase", &mut self.wake.wake.phrase, 64);
        ui.section_label("Windows recognition language");
        let mut names: Vec<String> = vec!["Automatic \u{b7} prefer English".into()];
        names.extend(self.wake.recognizers.iter().map(|(_, n)| n.clone()));
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let mut pick = self.wake.pick.min(refs.len() - 1);
        if ui.combo(&mut pick, &refs).changed {
            self.wake.pick = pick;
        }
        if self.wake.recognizers.is_empty() {
            ui.label_dim("Windows speech recognizers could not be listed. Check installed speech languages and microphone permissions.");
        }
        ui.section_label(&format!(
            "Confidence threshold \u{b7} {}%",
            (self.wake.confidence * 100.0).round() as i32
        ));
        ui.slider(&mut self.wake.confidence, 0.55..=0.95, 0.01);
        ui.row(|ui| {
            ui.spacing(6.0);
            if ui.button("Refresh recognizers").clicked {
                self.wake.seeded = false;
            }
            if ui.button("Windows speech settings").clicked {
                out.commands
                    .push(Command::OpenUrl("ms-settings:speech".into()));
            }
        });
        ui.gap(4.0);
        if pill(ui, "Save & apply wake word", true) {
            self.wake.note = match self.save_wake() {
                Ok(()) => self.host.snapshot().wake.text.clone(),
                Err(e) => e,
            };
        }
        if snap.wake.enabled {
            let label = if snap.wake.paused {
                "Resume wake word"
            } else {
                "Pause wake word"
            };
            if pill(ui, label, false) {
                self.host.set_wake_listening(snap.wake.paused);
            }
        }
        let status = if self.wake.note.is_empty() {
            snap.wake.text.clone()
        } else {
            self.wake.note.clone()
        };
        if !status.is_empty() {
            ui.label_dim(&status);
        }
        ui.label_dim("The default phrase also accepts close pronunciations. This is phrase recognition, not speaker verification. Enabled recognition uses additional Windows resources.");
    }

    #[cfg(test)]
    pub(crate) fn set_wake_enabled(&mut self, on: bool) {
        self.seed_wake();
        self.wake.wake.enabled = on;
    }

    #[cfg(test)]
    pub(crate) fn set_wake_phrase(&mut self, phrase: &str) {
        self.seed_wake();
        self.wake.wake.phrase = phrase.into();
    }

    pub(crate) fn save_wake(&mut self) -> Result<(), String> {
        let options = self.service.options();
        let configured = self.service.setup().voice.configured;
        let mut next = self.wake.wake.clone();
        next.confidence = f64::from(self.wake.confidence);
        next.recognizer_id = match self.wake.pick {
            0 => String::new(),
            i => self
                .wake
                .recognizers
                .get(i - 1)
                .map(|(id, _)| id.clone())
                .unwrap_or_default(),
        };
        let saved =
            save_wake_word(&options, next, configured).map_err(|e| e.message().to_owned())?;
        let enabled = saved.enabled;
        self.service.edit_options(|o: &mut Options| {
            o.wake_word = saved;
            Ok(())
        })?;
        self.wake.seeded = false;
        self.host.set_wake_listening(enabled);
        Ok(())
    }

    // ---- models ---------------------------------------------------------------------------

    fn show_models(&mut self, ui: &mut Ui<'_>) {
        let options = self.service.options();
        let panel = self.host.speech();
        ui.heading_sized("Models on this PC", 20.0);
        ui.label_dim("See what is installed and remove models you no longer use. Models load only while a conversation is active.");

        // Whistle: built in, nothing to install or remove.
        ui.section_label("Built in");
        ui.label("Whistle \u{b7} built in");
        ui.label_dim(&format!(
            "Listening for {}. Language is detected automatically. About 17 MB, shipped with NabyScape and checked against a pinned checksum before it loads.",
            engine::whistle_language_names()
        ));
        ui.label_dim(&self.host.whistle_status());

        ui.section_label("Speech packs");
        ui.label_dim("Packs add listening and speaking models for other languages or higher accuracy. Downloads are verified and stay on this PC.");
        let mut preference = self.models.preference;
        ui.segmented(&PREFERENCES, &mut preference);
        self.models.preference = preference;
        let names: Vec<&str> = SPEECH_LANGUAGES
            .iter()
            .map(|c| engine::language_label(c))
            .collect();
        let mut lang = self.models.language;
        ui.combo(&mut lang, &names);
        self.models.language = lang;
        let code = SPEECH_LANGUAGES[lang.min(SPEECH_LANGUAGES.len() - 1)];
        let preference_name = PREFERENCES[self.models.preference.min(2)];
        ui.row(|ui| {
            ui.spacing(6.0);
            if ui
                .enabled(!panel.busy, |ui| ui.button("Scan this PC"))
                .clicked
            {
                self.host.scan_hardware();
            }
            if panel.busy && panel.can_cancel && ui.button("Cancel").clicked {
                self.host.cancel_speech();
            }
        });
        match &panel.hardware {
            None => {
                ui.label_dim("Scan your PC to find a suitable listening model.");
            }
            Some(hardware) => {
                ui.label_dim(&hardware.summary());
                let pack = recommend_pack(hardware, code, preference_name, true);
                ui.label(&format!("{} + {}", pack.asr.name, pack.tts.name));
                ui.label_dim(&format!(
                    "{}\n{}\n{}",
                    asr_details(pack.asr),
                    voice_details(pack.tts),
                    pack.reason
                ));
                let installed = panel.installed_asr.iter().any(|m| m.id == pack.asr.id)
                    && (pack.tts.id == "windows"
                        || panel.installed_tts.iter().any(|m| m.id == pack.tts.id));
                let use_pending = self
                    .models
                    .pending
                    .as_ref()
                    .is_some_and(|(a, t, _)| *a == pack.asr.id && *t == pack.tts.id);
                if installed {
                    if pill(ui, "Use installed pack", true) {
                        self.models.note = self.use_pack(
                            &options,
                            pack.asr,
                            pack.tts,
                            pack.gpu,
                            preference_name,
                            code,
                        );
                    }
                } else if ui
                    .enabled(!panel.busy && !use_pending, |ui| {
                        ui.primary_button("Install & use")
                    })
                    .clicked
                {
                    self.models.pending =
                        Some((pack.asr.id.clone(), pack.tts.id.clone(), pack.gpu));
                    self.host.install_pack(&pack.asr.id, &pack.tts.id, pack.gpu);
                }
            }
        }
        // Finish an install the user asked to "use".
        if let Some((a, t, gpu)) = self.models.pending.clone() {
            let done = !panel.busy
                && panel.installed_asr.iter().any(|m| m.id == a)
                && (t == "windows" || panel.installed_tts.iter().any(|m| m.id == t));
            if done {
                if let (Ok(asr), Ok(tts)) = (
                    naby_core::assistant::speech::asr_by_id(&a),
                    naby_core::assistant::speech::tts_by_id(&t),
                ) {
                    self.models.note =
                        self.use_pack(&options, asr, tts, gpu, preference_name, code);
                }
                self.models.pending = None;
            } else if !panel.busy && !panel.status.is_empty() && panel.status.contains("failed") {
                self.models.pending = None;
            }
        }
        if panel.busy {
            ui.progress_h((panel.progress / 100.0).clamp(0.0, 1.0) as f32, 6.0);
        }
        if !panel.status.is_empty() {
            ui.label_dim(&panel.status);
        }
        if !self.models.note.is_empty() {
            let note = self.models.note.clone();
            ui.label_dim(&note);
        }

        ui.section_label("Installed speech models");
        ui.label_dim("Remove listening or speaking models you no longer need. Matching download caches are removed too. Shared speech engines stay installed. Model files you imported from another folder stay where they are. Windows voices are managed in Windows Settings.");
        if panel.installed_asr.is_empty() && panel.installed_tts.is_empty() {
            ui.label_dim("No downloaded speech models on this PC.");
        }
        let rows: Vec<(String, bool, super::model::InstalledModel)> = panel
            .installed_asr
            .iter()
            .map(|m| ("Listening".to_owned(), false, m.clone()))
            .chain(
                panel
                    .installed_tts
                    .iter()
                    .map(|m| ("Speaking".to_owned(), true, m.clone())),
            )
            .collect();
        for (kind, tts, model) in rows {
            let selected = if model.selected {
                " \u{b7} Currently selected"
            } else {
                ""
            };
            ui.push_id((kind.as_str(), model.id.as_str()), |ui| {
                ui.label(&format!("{} \u{b7} {}", model.name, kind));
                ui.label_dim(&format!("{}{}", model.size, selected));
                if self.models.confirm_remove == Some((model.id.clone(), tts)) {
                    let warning = if model.selected {
                        " This is your selected model; local voice will be unavailable until you select an installed pack or reinstall it."
                    } else {
                        ""
                    };
                    ui.label_dim(&format!(
                        "Uninstall {}? Its downloaded files and matching cache will be deleted and about {} freed.{warning}",
                        model.name, model.size
                    ));
                    ui.row(|ui| {
                        ui.spacing(6.0);
                        if ui.enabled(!panel.busy, |ui| ui.primary_button("Uninstall")).clicked {
                            self.host.remove_model(&model.id, tts);
                            self.models.confirm_remove = None;
                        }
                        if ui.button("Keep").clicked {
                            self.models.confirm_remove = None;
                        }
                    });
                } else if ui.enabled(!panel.busy, |ui| ui.button("Uninstall")).clicked {
                    self.models.confirm_remove = Some((model.id.clone(), tts));
                }
            });
        }
    }

    fn use_pack(
        &self,
        options: &Options,
        asr: &naby_core::assistant::speech::AsrModel,
        tts: &naby_core::assistant::speech::TtsModel,
        gpu: bool,
        preference: &str,
        language: &str,
    ) -> String {
        let result = use_speech_pack(options, asr, tts, gpu, preference, language, true);
        match result {
            Ok(mut next) => {
                engine::set_choice(&mut next.local_voice, Listening::Pack);
                match self.service.edit_options(|o| {
                    *o = next;
                    Ok(())
                }) {
                    Ok(()) => {
                        self.setup_reset();
                        format!("{} + {} selected.", asr.name, tts.name)
                    }
                    Err(e) => e,
                }
            }
            Err(e) => e.message().to_owned(),
        }
    }

    fn setup_reset(&self) {}

    // ---- check audio ------------------------------------------------------------------------

    fn show_audio(&mut self, ui: &mut Ui<'_>) {
        let check = self.host.audio_check();
        let test = self.host.mic_state();
        let active = self.host.snapshot().active;
        ui.heading_sized("Check your audio", 20.0);
        ui.label_dim("Tests use your saved setup. The microphone test records four seconds on this PC; transcription uses only your listening engine, never the chat model. Play sample speaks a fixed sentence. Nothing is added to chat or saved as an audio file.");
        if active {
            ui.label_dim("End the current conversation before testing.");
        }
        ui.row(|ui| {
            ui.spacing(6.0);
            let busy = active || check.running;
            if ui.enabled(!busy, |ui| ui.button("Test microphone")).clicked {
                self.host.run_audio_check(AudioCheckKind::Microphone);
            }
            if ui
                .enabled(!busy, |ui| ui.button("Test transcription"))
                .clicked
            {
                self.host.run_audio_check(AudioCheckKind::Listening);
            }
            if ui.enabled(!busy, |ui| ui.button("Play sample")).clicked {
                self.host.run_audio_check(AudioCheckKind::Speaking);
            }
        });
        if check.running {
            ui.progress_h(check.level.clamp(0.0, 1.0), 6.0);
        }
        let message = if check.message.is_empty() {
            "Not tested on this visit.".to_owned()
        } else {
            check.message.clone()
        };
        match check.ok {
            Some(false) => {
                let invalid = ui.theme().invalid;
                let style = ui.text_style(12.0, Weight::Regular, invalid);
                ui.label_styled(&message, &style);
            }
            _ => {
                ui.label_dim(&message);
            }
        }
        mic_test_line(ui, &test);
        let _ = TextEditOpts::single();
    }
}
