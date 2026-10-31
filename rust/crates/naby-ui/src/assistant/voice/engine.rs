//! Which engine listens, from the saved local voice options (`LocalVoiceOptions`).
//!
//! Whistle (built in) is the default listening engine for English, German, French, Spanish,
//! Italian, Dutch and Polish, with automatic language detection. A language outside those, or an
//! explicit choice of a speech pack, own model files, an external server or a realtime provider,
//! uses that instead. Speaking defaults to built-in Piper (one `piper.exe` per sentence, so it
//! holds no memory between sentences); Windows speech, a pack voice or an external TTS are choices.
//!
//! The choices live in the unknown-key map of `LocalVoiceOptions` (`ListeningEngine`,
//! `SpeakingEngine`, `PiperVoice`, `PiperSpeed`, `WhistleRuntime`), so older
//! `assistant.json` files keep working and nothing new needs modeling in `naby-core`.

use naby_core::assistant::models::{LocalVoiceOptions, Options};
use naby_core::assistant::speech::{asr_by_id, language_name, own_asr, tts_models, TtsModel};
use serde_json::Value;

/// Languages Whistle recognizes (auto-detect between them).
pub const WHISTLE_LANGUAGES: [&str; 7] = ["en", "de", "fr", "es", "it", "nl", "pl"];

/// The unknown-key name that stores an explicit choice.
pub const KEY: &str = "ListeningEngine";
/// The unknown-key names for the speaking choice, the Piper voice and its speed.
pub const SPEAKING_KEY: &str = "SpeakingEngine";
pub const PIPER_VOICE_KEY: &str = "PiperVoice";
pub const PIPER_SPEED_KEY: &str = "PiperSpeed";
/// The user's own Piper voice: `PiperVoice` is [`PIPER_CUSTOM`] and this key holds the `.onnx` path.
pub const PIPER_PATH_KEY: &str = "PiperVoicePath";
pub const PIPER_CUSTOM: &str = "custom";
/// The unknown-key name for how Whistle runs (`Exe` or `Pip`).
pub const WHISTLE_RUNTIME_KEY: &str = "WhistleRuntime";

/// How Whistle runs: the Cactus `needle.exe` (default) or the `cactus-needle` Python package.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WhistleRuntime {
    #[default]
    Exe,
    Pip,
}

impl WhistleRuntime {
    pub fn label(self) -> &'static str {
        match self {
            Self::Exe => "needle.exe",
            Self::Pip => "pip",
        }
    }
}

/// The saved Whistle runtime; anything unknown or missing is `Exe`.
pub fn whistle_runtime(local: &LocalVoiceOptions) -> WhistleRuntime {
    match local.extra.get(WHISTLE_RUNTIME_KEY).and_then(Value::as_str) {
        Some("Pip") => WhistleRuntime::Pip,
        _ => WhistleRuntime::Exe,
    }
}

/// Stores the Whistle runtime (`Exe` is the default, so it removes the key).
pub fn set_whistle_runtime(local: &mut LocalVoiceOptions, runtime: WhistleRuntime) {
    match runtime {
        WhistleRuntime::Exe => {
            local.extra.remove(WHISTLE_RUNTIME_KEY);
        }
        WhistleRuntime::Pip => {
            local
                .extra
                .insert(WHISTLE_RUNTIME_KEY.into(), Value::String("Pip".into()));
        }
    }
}

/// Whether Whistle can listen in `code` (`en`, `de-DE`, ...).
pub fn whistle_supports(code: &str) -> bool {
    let code = code.trim().to_ascii_lowercase();
    let base = code.split(['-', '_']).next().unwrap_or("");
    WHISTLE_LANGUAGES.contains(&base)
}

/// Display name of a language code (core's table lacks Polish).
pub fn language_label(code: &str) -> &'static str {
    if code == "pl" {
        "Polish"
    } else {
        language_name(code)
    }
}

/// The names for the languages Whistle covers.
pub fn whistle_language_names() -> String {
    WHISTLE_LANGUAGES
        .iter()
        .map(|c| language_label(c))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Who turns the user's speech into text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Listening {
    /// A realtime provider (OpenAI Realtime / Gemini Live) does everything.
    Realtime,
    /// Whistle, built in.
    Whistle,
    /// An installed speech pack.
    Pack,
    /// The user's own model files.
    OwnFiles,
    /// An external ASR server on this PC.
    External,
    /// Nothing usable: Whistle was chosen but the language is not covered and no other engine
    /// is set up.
    NeedsSetup,
}

/// Who speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Speaking {
    Realtime,
    /// Windows.Media.SpeechSynthesis.
    Windows,
    /// Built-in Piper, one process per sentence.
    Piper,
    /// A pack voice (Piper, Kokoro, Kitten, Supertonic).
    Pack,
    /// An external TTS server.
    External,
}

/// Resolved engines plus a note explaining a fallback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Engines {
    pub listening: Listening,
    pub speaking: Speaking,
    pub note: Option<String>,
}

fn untouched_external(local: &LocalVoiceOptions) -> bool {
    let d = LocalVoiceOptions::default();
    local.asr_protocol == d.asr_protocol
        && local.asr_url == d.asr_url
        && local.asr_model == d.asr_model
}

/// The explicit choice saved by the voice setup, if any.
pub fn chosen(local: &LocalVoiceOptions) -> Option<Listening> {
    match local.extra.get(KEY).and_then(Value::as_str)? {
        "Whistle" => Some(Listening::Whistle),
        "Pack" => Some(Listening::Pack),
        "OwnFiles" => Some(Listening::OwnFiles),
        "External" => Some(Listening::External),
        _ => None,
    }
}

/// Stores an explicit listening choice (does not touch the other fields).
pub fn set_choice(local: &mut LocalVoiceOptions, engine: Listening) {
    let name = match engine {
        Listening::Whistle => "Whistle",
        Listening::Pack => "Pack",
        Listening::OwnFiles => "OwnFiles",
        Listening::External => "External",
        Listening::Realtime | Listening::NeedsSetup => {
            local.extra.remove(KEY);
            return;
        }
    };
    local.extra.insert(KEY.into(), Value::String(name.into()));
}

/// Applies the engine choice to the options as the setup page's buttons do. Returns the options
/// to save.
pub fn apply_choice(options: &Options, engine: Listening) -> Options {
    let mut next = options.clone();
    let local = &mut next.local_voice;
    match engine {
        Listening::Realtime => {
            local.enabled = false;
            local.extra.remove(KEY);
        }
        Listening::Whistle => {
            local.enabled = true;
            local.managed = false;
            local.own_models = false;
            set_choice(local, Listening::Whistle);
        }
        Listening::Pack => {
            local.enabled = true;
            local.managed = true;
            local.own_models = false;
            set_choice(local, Listening::Pack);
        }
        Listening::OwnFiles => {
            local.enabled = true;
            local.managed = true;
            local.own_models = true;
            set_choice(local, Listening::OwnFiles);
        }
        Listening::External => {
            local.enabled = true;
            local.managed = false;
            local.own_models = false;
            set_choice(local, Listening::External);
        }
        Listening::NeedsSetup => {}
    }
    next
}

/// The speaking side (independent of the listening engine).
pub fn speaking(local: &LocalVoiceOptions) -> Speaking {
    if !local.enabled {
        Speaking::Realtime
    } else if local.managed && local.managed_tts != "windows" {
        Speaking::Pack
    } else if local.managed {
        Speaking::Windows
    } else {
        let saved = local.extra.get(SPEAKING_KEY).and_then(Value::as_str);
        if saved == Some("Piper") {
            Speaking::Piper
        } else if local.tts_engine != "Windows voice" {
            Speaking::External
        } else if saved == Some("Windows") {
            Speaking::Windows
        } else {
            Speaking::Piper
        }
    }
}

/// Stores the speaking choice. Piper and Windows both save `TtsEngine = "Windows voice"`; an
/// external server is chosen by the caller setting `tts_engine`.
pub fn set_speaking(local: &mut LocalVoiceOptions, speaking: Speaking) {
    let name = match speaking {
        Speaking::Piper => "Piper",
        Speaking::Windows => "Windows",
        _ => {
            local.extra.remove(SPEAKING_KEY);
            return;
        }
    };
    local.tts_engine = "Windows voice".into();
    local
        .extra
        .insert(SPEAKING_KEY.into(), Value::String(name.into()));
}

/// The built-in Piper voices that cover `language`, smallest first.
pub fn piper_voices(language: &str) -> Vec<&'static TtsModel> {
    let base = language.split(['-', '_']).next().unwrap_or("");
    let mut voices: Vec<&TtsModel> = tts_models()
        .iter()
        .filter(|m| m.engine == "piper" && m.id != "windows" && m.package.is_none())
        .filter(|m| m.supports(base))
        .collect();
    voices.sort_by_key(|m| m.bytes);
    voices
}

/// The saved Piper voice if it speaks the setup language, else the smallest one that does.
pub fn piper_voice(local: &LocalVoiceOptions) -> Option<&'static TtsModel> {
    let voices = piper_voices(&local.speech_language);
    let saved = local.extra.get(PIPER_VOICE_KEY).and_then(Value::as_str);
    saved
        .and_then(|id| voices.iter().copied().find(|m| m.id == id))
        .or_else(|| voices.first().copied())
}

/// "female", "male" or "" for a catalog voice, from the end of its name.
pub fn piper_gender(m: &TtsModel) -> &'static str {
    if m.name.ends_with(" \u{b7} female") {
        "female"
    } else if m.name.ends_with(" \u{b7} male") {
        "male"
    } else {
        ""
    }
}

/// The path of the user's own Piper voice when one is saved.
pub fn piper_custom(local: &LocalVoiceOptions) -> Option<String> {
    let custom = local.extra.get(PIPER_VOICE_KEY).and_then(Value::as_str) == Some(PIPER_CUSTOM);
    let path = local.extra.get(PIPER_PATH_KEY).and_then(Value::as_str);
    path.filter(|p| custom && !p.is_empty()).map(str::to_owned)
}

/// The saved Piper speed (1.0 is normal), kept inside 0.5 to 2.0.
pub fn piper_speed(local: &LocalVoiceOptions) -> f64 {
    let speed = local.extra.get(PIPER_SPEED_KEY).and_then(Value::as_f64);
    speed
        .filter(|s| s.is_finite())
        .unwrap_or(1.0)
        .clamp(0.5, 2.0)
}

/// Stores the Piper voice and speed.
pub fn set_piper(local: &mut LocalVoiceOptions, voice: &str, speed: f64) {
    local
        .extra
        .insert(PIPER_VOICE_KEY.into(), Value::String(voice.into()));
    let speed = if speed.is_finite() {
        speed.clamp(0.5, 2.0)
    } else {
        1.0
    };
    local
        .extra
        .insert(PIPER_SPEED_KEY.into(), Value::from(speed));
}

/// Stores the path of the user's own Piper voice (used when the voice is [`PIPER_CUSTOM`]).
pub fn set_piper_path(local: &mut LocalVoiceOptions, path: &str) {
    local
        .extra
        .insert(PIPER_PATH_KEY.into(), Value::String(path.into()));
}

/// Picks the listening engine and speaking engine for the saved options.
pub fn resolve(local: &LocalVoiceOptions) -> Engines {
    let speaking = speaking(local);
    if !local.enabled {
        return Engines {
            listening: Listening::Realtime,
            speaking,
            note: None,
        };
    }
    let wanted = chosen(local).unwrap_or(if local.managed {
        if local.own_models {
            Listening::OwnFiles
        } else {
            Listening::Pack
        }
    } else if untouched_external(local) {
        Listening::Whistle
    } else {
        Listening::External
    });
    if wanted != Listening::Whistle {
        return Engines {
            listening: wanted,
            speaking,
            note: None,
        };
    }
    if whistle_supports(&local.speech_language) {
        return Engines {
            listening: Listening::Whistle,
            speaking,
            note: None,
        };
    }
    // Whistle does not cover the chosen language: use what else is set up.
    let language = language_label(&local.speech_language);
    let covers = whistle_language_names();
    if local.managed {
        let listening = if local.own_models {
            Listening::OwnFiles
        } else {
            Listening::Pack
        };
        Engines {
            listening,
            speaking,
            note: Some(format!(
                "Whistle covers {covers}; {language} uses your speech pack instead."
            )),
        }
    } else if !untouched_external(local) {
        Engines {
            listening: Listening::External,
            speaking,
            note: Some(format!(
                "Whistle covers {covers}; {language} uses your external speech server instead."
            )),
        }
    } else {
        Engines {
            listening: Listening::NeedsSetup,
            speaking,
            note: Some(format!(
                "Whistle covers {covers}. For {language}, install a speech pack or connect a speech service."
            )),
        }
    }
}

/// The engine line shown in the voice room and the Models page.
pub fn label(local: &LocalVoiceOptions) -> String {
    let engines = resolve(local);
    match engines.listening {
        Listening::Realtime => "Realtime provider".into(),
        Listening::Whistle => "Whistle · built in".into(),
        Listening::Pack => match asr_by_id(&local.managed_asr) {
            Ok(model) => format!("{} · speech pack", model.name),
            Err(_) => "Speech pack".into(),
        },
        Listening::OwnFiles => match own_asr(local) {
            Ok(model) => format!("{} · your files", model.name),
            Err(_) => "Your model files".into(),
        },
        Listening::External => format!("External server · {}", local.asr_protocol),
        Listening::NeedsSetup => "Choose a listening engine".into(),
    }
}

/// The keywords that bias recognition: the wake phrase, its words and the app names.
pub fn keywords(options: &Options) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut add = |s: &str| {
        let s: String = s.chars().filter(|c| !c.is_control()).take(64).collect();
        let s = s.trim().to_owned();
        if s.len() >= 2 && out.len() < 32 && !out.iter().any(|k| k.eq_ignore_ascii_case(&s)) {
            out.push(s);
        }
    };
    let phrase = &options.wake_word.phrase;
    add(phrase);
    for word in phrase.split_whitespace() {
        add(word);
    }
    add("Naby");
    add("Navi");
    add("Oye Naby");
    add("NabyScape");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local() -> LocalVoiceOptions {
        LocalVoiceOptions {
            enabled: true,
            ..LocalVoiceOptions::default()
        }
    }

    #[test]
    fn whistle_is_the_default_local_listening_engine_for_its_languages() {
        for code in ["en", "de", "fr", "es", "it", "nl", "pl", "en-GB"] {
            let mut l = local();
            l.speech_language = code.into();
            let e = resolve(&l);
            assert_eq!(e.listening, Listening::Whistle, "{code}");
            assert_eq!(e.speaking, Speaking::Piper);
            assert_eq!(e.note, None);
        }
        assert_eq!(label(&local()), "Whistle · built in");
    }

    #[test]
    fn a_language_outside_whistle_falls_back_to_what_is_configured() {
        let mut l = local();
        l.speech_language = "id".into();
        let e = resolve(&l);
        assert_eq!(e.listening, Listening::NeedsSetup);
        assert!(e.note.unwrap().contains("Indonesian"));
        assert_eq!(label(&l), "Choose a listening engine");
        // A configured pack covers it.
        l.managed = true;
        l.managed_asr = "base".into();
        let e = resolve(&l);
        assert_eq!(e.listening, Listening::Pack);
        // Choosing Whistle explicitly with an unsupported language falls back, with a note.
        set_choice(&mut l, Listening::Whistle);
        let e = resolve(&l);
        assert_eq!(e.listening, Listening::Pack);
        assert!(e.note.unwrap().contains("speech pack"));
        // So does an external server the user configured.
        let mut l = local();
        l.speech_language = "id".into();
        l.asr_url = "http://127.0.0.1:9000/".into();
        assert_eq!(resolve(&l).listening, Listening::External);
    }

    #[test]
    fn picking_another_engine_overrides_whistle() {
        let mut l = local();
        set_choice(&mut l, Listening::External);
        assert_eq!(resolve(&l).listening, Listening::External);
        l.managed = true;
        l.own_models = true;
        set_choice(&mut l, Listening::OwnFiles);
        assert_eq!(resolve(&l).listening, Listening::OwnFiles);
        l.enabled = false;
        let e = resolve(&l);
        assert_eq!(
            (e.listening, e.speaking),
            (Listening::Realtime, Speaking::Realtime)
        );
        assert_eq!(label(&l), "Realtime provider");
    }

    #[test]
    fn existing_installations_keep_their_engines() {
        // Saved by the C# app: managed pack, no ListeningEngine key.
        let mut l = local();
        l.managed = true;
        l.managed_asr = "base".into();
        l.managed_tts = "windows".into();
        assert_eq!(resolve(&l).listening, Listening::Pack);
        assert_eq!(resolve(&l).speaking, Speaking::Windows);
        assert!(label(&l).starts_with("Whisper base") && label(&l).ends_with("· speech pack"));
        // Saved external server with a changed address.
        let mut l = local();
        l.asr_model = "large-v3-turbo".into();
        assert_eq!(resolve(&l).listening, Listening::External);
        // Pack voices speak.
        let mut l = local();
        l.managed = true;
        l.managed_tts = "kokoro".into();
        assert_eq!(speaking(&l), Speaking::Pack);
        let mut l = local();
        l.tts_engine = "Kokoro".into();
        assert_eq!(speaking(&l), Speaking::External);
    }

    #[test]
    fn apply_choice_edits_only_the_engine_fields() {
        let mut options = Options::default();
        options.local_voice.speech_language = "de".into();
        let whistle = apply_choice(&options, Listening::Whistle);
        assert!(whistle.local_voice.enabled && !whistle.local_voice.managed);
        assert_eq!(resolve(&whistle.local_voice).listening, Listening::Whistle);
        assert_eq!(whistle.local_voice.speech_language, "de");
        let pack = apply_choice(&whistle, Listening::Pack);
        assert!(pack.local_voice.managed && !pack.local_voice.own_models);
        assert_eq!(resolve(&pack.local_voice).listening, Listening::Pack);
        let realtime = apply_choice(&pack, Listening::Realtime);
        assert!(!realtime.local_voice.enabled);
        assert!(realtime.local_voice.extra.get(KEY).is_none());
        // The stored name survives a JSON round trip with PascalCase keys.
        let json = serde_json::to_string(&whistle).unwrap();
        assert!(json.contains("\"ListeningEngine\":\"Whistle\""));
        let back = Options::from_json(json.as_bytes()).unwrap();
        assert_eq!(chosen(&back.local_voice), Some(Listening::Whistle));
    }

    #[test]
    fn piper_is_the_default_speaker_and_explicit_choices_win() {
        let mut l = local();
        assert_eq!(speaking(&l), Speaking::Piper);
        set_speaking(&mut l, Speaking::Windows);
        assert_eq!(speaking(&l), Speaking::Windows);
        set_speaking(&mut l, Speaking::Piper);
        assert_eq!(speaking(&l), Speaking::Piper);
        l.tts_engine = "Kokoro".into();
        set_speaking(&mut l, Speaking::External);
        assert_eq!(speaking(&l), Speaking::External);
        // A speech pack keeps its own voice, or the Windows one.
        let mut l = local();
        l.managed = true;
        l.managed_tts = "windows".into();
        assert_eq!(speaking(&l), Speaking::Windows);
    }

    #[test]
    fn piper_voices_carry_a_gender_and_verified_hashes() {
        let amy = tts_models()
            .iter()
            .find(|m| m.id == "en_US-amy-low")
            .unwrap();
        assert_eq!(piper_gender(amy), "female");
        let ryan = tts_models()
            .iter()
            .find(|m| m.id == "en_US-ryan-low")
            .unwrap();
        assert_eq!(piper_gender(ryan), "male");
        let sharvard = tts_models()
            .iter()
            .find(|m| m.id == "es_ES-sharvard-medium")
            .unwrap();
        assert_eq!(piper_gender(sharvard), "");
        for m in tts_models()
            .iter()
            .filter(|m| m.engine == "piper" && m.package.is_none() && m.id != "windows")
        {
            assert!(
                [&m.hash, &m.config_hash, &m.card_hash]
                    .iter()
                    .all(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit())),
                "{}",
                m.id
            );
            // Every voice except the two with a mixed or unconfirmed speaker names its gender.
            if !["es_ES-sharvard-medium", "id_ID-news_tts-medium"].contains(&m.id.as_str()) {
                assert!(!piper_gender(m).is_empty(), "{}", m.id);
            }
        }
        for lang in ["en", "de", "fr", "es", "it", "pl", "ru"] {
            let genders: Vec<_> = piper_voices(lang).into_iter().map(piper_gender).collect();
            assert!(genders.contains(&"female") || lang == "es", "{lang}");
            assert!(genders.contains(&"male"), "{lang}");
        }
    }

    #[test]
    fn piper_voice_follows_the_language() {
        let mut l = local();
        l.speech_language = "es".into();
        assert_eq!(piper_voice(&l).unwrap().id, "es_MX-ald-x_low");
        set_piper(&mut l, "es_ES-carlfm-x_low", 1.25);
        assert_eq!(piper_voice(&l).unwrap().id, "es_ES-carlfm-x_low");
        l.speech_language = "en-GB".into();
        assert_eq!(piper_voice(&l).unwrap().id, "en_US-amy-low");
        l.speech_language = "id".into();
        assert_eq!(piper_voice(&l).unwrap().id, "id_ID-news_tts-medium");
        l.speech_language = "ja".into();
        assert!(piper_voice(&l).is_none());
        set_piper(&mut l, "x", 9.0);
        assert_eq!(piper_speed(&l), 2.0);
        set_piper(&mut l, "x", 0.1);
        assert_eq!(piper_speed(&l), 0.5);
        l.extra.remove(PIPER_SPEED_KEY);
        assert_eq!(piper_speed(&l), 1.0);
        set_piper(&mut l, "es_MX-ald-x_low", 1.5);
        let json = serde_json::to_string(&l).unwrap();
        assert!(json.contains("\"PiperVoice\":\"es_MX-ald-x_low\""));
        let back: LocalVoiceOptions = serde_json::from_str(&json).unwrap();
        assert_eq!(piper_speed(&back), 1.5);
        assert_eq!(piper_custom(&back), None);
        set_piper(&mut l, PIPER_CUSTOM, 1.0);
        assert_eq!(piper_custom(&l), None);
        set_piper_path(&mut l, r"C:oicesmine.onnx");
        assert_eq!(piper_custom(&l).as_deref(), Some(r"C:oicesmine.onnx"));
        set_piper(&mut l, "es_MX-ald-x_low", 1.0);
        assert_eq!(piper_custom(&l), None);
    }

    #[test]
    fn whistle_runtime_defaults_to_exe_and_round_trips() {
        use serde_json::json;
        let mut l = local();
        assert_eq!(whistle_runtime(&l), WhistleRuntime::Exe);
        l.extra
            .insert(WHISTLE_RUNTIME_KEY.into(), json!("Nonsense"));
        assert_eq!(whistle_runtime(&l), WhistleRuntime::Exe);
        set_whistle_runtime(&mut l, WhistleRuntime::Pip);
        let text = serde_json::to_string(&l).unwrap();
        assert!(text.contains("\"WhistleRuntime\":\"Pip\""));
        let back: LocalVoiceOptions = serde_json::from_str(&text).unwrap();
        assert_eq!(whistle_runtime(&back), WhistleRuntime::Pip);
        set_whistle_runtime(&mut l, WhistleRuntime::Exe);
        assert!(!serde_json::to_string(&l)
            .unwrap()
            .contains("WhistleRuntime"));
    }

    #[test]
    fn keywords_cover_the_wake_phrase_and_app_names() {
        let mut o = Options::default();
        o.wake_word.phrase = "Hey Naby".into();
        let k = keywords(&o);
        assert_eq!(k[0], "Hey Naby");
        assert!(k.contains(&"NabyScape".to_owned()));
        assert!(k.contains(&"Navi".to_owned()) && k.contains(&"Oye Naby".to_owned()));
        assert_eq!(
            k.iter().filter(|w| w.eq_ignore_ascii_case("naby")).count(),
            1
        );
    }
}
