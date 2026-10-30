//! Which engine listens, from the saved local voice options (`LocalVoiceOptions`).
//!
//! Whistle (built in) is the default listening engine for English, German, French, Spanish,
//! Italian, Dutch and Polish, with automatic language detection. A language outside those, or an
//! explicit choice of a speech pack, own model files, an external server or a realtime provider,
//! uses that instead. Speaking is unchanged: Windows speech, a pack voice, or an external TTS.
//!
//! The choice lives in the unknown-key map of `LocalVoiceOptions` (`ListeningEngine`), so older
//! `assistant.json` files keep working and nothing new needs modeling in `naby-core`.

use naby_core::assistant::models::{LocalVoiceOptions, Options};
use naby_core::assistant::speech::{asr_by_id, language_name, own_asr};
use serde_json::Value;

/// Languages Whistle recognizes (auto-detect between them).
pub const WHISTLE_LANGUAGES: [&str; 7] = ["en", "de", "fr", "es", "it", "nl", "pl"];

/// The unknown-key name that stores an explicit choice.
pub const KEY: &str = "ListeningEngine";

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
    } else if local.managed || local.tts_engine == "Windows voice" {
        Speaking::Windows
    } else {
        Speaking::External
    }
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
            assert_eq!(e.speaking, Speaking::Windows);
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
    fn keywords_cover_the_wake_phrase_and_app_names() {
        let mut o = Options::default();
        o.wake_word.phrase = "Hey Naby".into();
        let k = keywords(&o);
        assert_eq!(k[0], "Hey Naby");
        assert!(k.contains(&"NabyScape".to_owned()));
        assert_eq!(
            k.iter().filter(|w| w.eq_ignore_ascii_case("naby")).count(),
            1
        );
    }
}
