//! Semantic router: a short instruction that needs no answer ("pausa la música") runs here,
//! without the chat model; everything else goes to chat as before. Needle (a small on-device
//! function-calling model) picks the action and its arguments, but its output is not trusted on
//! its own: in tests it played music for "no quiero que pongas música" and paused it for "me gusta
//! esta canción". So the words of the request must back the action, before Needle runs (which
//! spares chat its ~1 s) and again after. Any doubt means chat, which is always safe.

use serde_json::Value;

use crate::command::Command;

/// The actions offered to Needle (`needle.exe --tools`). `triggers` force a call on Spanish
/// phrasing the model otherwise withholds; [`decide`] still has the last word.
pub const TOOLS: &str = r#"[
{"name":"media_control","description":"Pause, play, skip to the next song or go back to the previous song. Pausa, reproduce, siguiente o anterior cancion.",
 "triggers":["\\b(pausa|pausar|reproduce|reanuda|siguiente|anterior|salta)\\b.*\\b(musica|cancion|tema|pista|video)\\b","\\b(pause|play|resume|skip|next|previous)\\b.*\\b(music|song|track|video)\\b"],
 "parameters":{"type":"object","properties":{"action":{"type":"string","enum":["pause","play","next","previous"],"description":"pausa=pause, reproduce=play, siguiente=next, anterior=previous"}},"required":["action"]}},
{"name":"set_volume","description":"Set the computer volume to a percentage. Pon el volumen a un porcentaje.",
 "triggers":["\\bvolumen\\b.*\\d","\\bvolume\\b.*\\d"],
 "parameters":{"type":"object","properties":{"percent":{"type":"integer","minimum":0,"maximum":100}},"required":["percent"]}},
{"name":"open_page","description":"Open a page of the app. Abre o muestra una pagina: musica, clima, calendario, temporizador, sistema, notas.",
 "triggers":["\\b(abre|abrir|muestra|ensename)\\b","\\b(open|show)\\b"],
 "parameters":{"type":"object","properties":{"page":{"type":"string","enum":["media","weather","calendar","timer","system","notes"],"description":"musica=media, clima=weather, calendario=calendar, temporizador=timer, sistema=system, notas=notes"}},"required":["page"]}},
{"name":"start_timer","description":"Start a countdown timer for a number of minutes. Pon un temporizador de varios minutos.",
 "triggers":["\\b(temporizador|cuenta atras)\\b","\\btimer\\b"],
 "parameters":{"type":"object","properties":{"minutes":{"type":"integer","minimum":1,"maximum":1440}},"required":["minutes"]}},
{"name":"toggle_mute","description":"Mute or unmute the computer sound. Silencia o quita el silencio del sonido.",
 "triggers":["\\b(silencia|silenciar|mutea|mutear)\\b","\\b(mute|unmute)\\b"],
 "parameters":{"type":"object","properties":{}}}
]"#;

/// Needle's score is unreliable in Spanish (its docs say so), so this only drops its clear
/// guesses (0.01-0.18 in tests); the word checks do the real work. Correct calls scored 0.32-0.91.
const MINIMUM_CONFIDENCE: f64 = 0.3;
/// Instructions are short; a long sentence is a conversation.
const MAXIMUM_WORDS: usize = 8;

const PAUSE: &[&str] = &["pausa", "pausar", "pauses", "pause", "deten", "detener"];
const PLAY: &[&str] = &[
    "reproduce",
    "reproducir",
    "reanuda",
    "reanudar",
    "play",
    "resume",
];
const NEXT: &[&str] = &["siguiente", "salta", "saltar", "next", "skip"];
const PREVIOUS: &[&str] = &["anterior", "previous"];
const MEDIA_OBJECT: &[&str] = &[
    "musica", "cancion", "tema", "pista", "video", "music", "song", "track",
];
const MUTE: &[&str] = &["silencia", "silenciar", "mutea", "mutear", "mute", "unmute"];
const VOLUME: &[&str] = &["volumen", "volume"];
const TIMER: &[&str] = &["temporizador", "timer"];
const MINUTE: &[&str] = &["minuto", "minutos", "minute", "minutes"];
const OPEN: &[&str] = &[
    "abre",
    "abreme",
    "abrir",
    "muestra",
    "muestrame",
    "ensename",
    "open",
    "show",
];
/// `open_page` values and the words that name them.
const PAGES: &[(&str, &[&str])] = &[
    ("media", &["musica", "media", "music"]),
    ("weather", &["clima", "tiempo", "weather"]),
    ("calendar", &["calendario", "agenda", "calendar"]),
    ("timer", &["temporizador", "timer"]),
    ("system", &["sistema", "system"]),
    ("notes", &["notas", "notes"]),
];
const NEGATION: &[&str] = &["no", "nunca", "not", "dont", "never"];
const QUESTION: &[&str] = &[
    "que", "como", "cuando", "cuanto", "cuanta", "cuantos", "donde", "quien", "cual", "what",
    "how", "why", "when", "where", "who", "which",
];

/// What a routed instruction does.
#[derive(Debug, Clone, PartialEq)]
pub enum Routed {
    Run(Command),
    /// A widget page, opened through the navigation tool so its rules still apply.
    Page(String),
}

/// Lowercase words without accents or punctuation; "don't" becomes "dont".
fn words(text: &str) -> Vec<String> {
    let plain: String = text
        .to_lowercase()
        .chars()
        .filter(|c| *c != '\'' && *c != '\u{2019}')
        .map(|c| match c {
            'á' => 'a',
            'é' => 'e',
            'í' => 'i',
            'ó' => 'o',
            'ú' | 'ü' => 'u',
            c if c.is_alphanumeric() => c,
            _ => ' ',
        })
        .collect();
    plain.split_whitespace().map(str::to_owned).collect()
}

fn any(words: &[String], list: &[&str]) -> bool {
    words.iter().any(|w| list.contains(&w.as_str()))
}

fn not_an_instruction(text: &str, words: &[String]) -> bool {
    text.contains(['?', '¿'])
        || words.is_empty()
        || words.len() > MAXIMUM_WORDS
        || any(words, NEGATION)
        || QUESTION.contains(&words[0].as_str())
        || (words[0] == "por" && words.get(1).is_some_and(|w| w == "que"))
}

/// A verb on its own ("pausa") or with something to act on ("pausa la música"), but not
/// "reproduce un video de gatos en YouTube": that one needs the chat model.
fn media_backed(words: &[String], verbs: &[&str]) -> bool {
    any(words, verbs) && (words.len() <= 2 || (any(words, MEDIA_OBJECT) && words.len() <= 5))
}

fn page_backed(words: &[String], page: &str) -> bool {
    any(words, OPEN)
        && words.len() <= 5
        && PAGES
            .iter()
            .any(|(id, names)| *id == page && any(words, names))
}

/// A number the model reports must be the one said; a request in words ("cinco") has no digits
/// to compare, and there the engine's own grounding check applies.
fn number_backed(words: &[String], value: i64) -> bool {
    let mut said = words
        .iter()
        .filter_map(|w| w.parse::<i64>().ok())
        .peekable();
    said.peek().is_none() || said.any(|n| n == value)
}

/// The request as Needle gets it: its triggers and its Spanish are far better without accents.
pub fn prompt(text: &str) -> String {
    words(text).join(" ")
}

fn mute_backed(words: &[String]) -> bool {
    any(words, MUTE) && words.len() <= 5
}

/// Whether the request can be an instruction at all. False sends it straight to chat.
pub fn worth_asking(text: &str) -> bool {
    let words = words(text);
    !not_an_instruction(text, &words)
        && (any(&words, VOLUME)
            || mute_backed(&words)
            || (any(&words, TIMER) && any(&words, MINUTE))
            || [PAUSE, PLAY, NEXT, PREVIOUS]
                .iter()
                .any(|verbs| media_backed(&words, verbs))
            || PAGES.iter().any(|(id, _)| page_backed(&words, id)))
}

/// The action in Needle's output, when the request's words back it.
pub fn decide(text: &str, needle_output: &str) -> Option<Routed> {
    let words = words(text);
    if not_an_instruction(text, &words) {
        return None;
    }
    let root: Value = serde_json::from_str(needle_output).ok()?;
    if root.get("confidence")?.as_f64()? < MINIMUM_CONFIDENCE {
        return None;
    }
    // Withheld calls (`suppressed_calls`) are never run.
    let [call] = root.get("function_calls")?.as_array()?.as_slice() else {
        return None;
    };
    let arguments = call.get("arguments")?;
    let backed = |verbs| media_backed(&words, verbs).then_some(());
    match call.get("name")?.as_str()? {
        "media_control" => Some(Routed::Run(match arguments.get("action")?.as_str()? {
            "pause" => backed(PAUSE).map(|()| Command::MediaPause)?,
            "play" => backed(PLAY).map(|()| Command::MediaPlay)?,
            "next" => backed(NEXT).map(|()| Command::MediaNext)?,
            "previous" => backed(PREVIOUS).map(|()| Command::MediaPrevious)?,
            _ => return None,
        })),
        "toggle_mute" => mute_backed(&words).then_some(Routed::Run(Command::ToggleMute)),
        "set_volume" => {
            let percent = arguments.get("percent")?.as_i64()?;
            ((0..=100).contains(&percent) && any(&words, VOLUME) && number_backed(&words, percent))
                .then(|| Routed::Run(Command::SetVolume(percent as f64 / 100.0)))
        }
        "start_timer" => {
            let minutes = arguments.get("minutes")?.as_i64()?;
            ((1..=1440).contains(&minutes)
                && any(&words, TIMER)
                && any(&words, MINUTE)
                && number_backed(&words, minutes))
            .then_some(Routed::Run(Command::TimerStart {
                seconds: minutes as f64 * 60.0,
            }))
        }
        "open_page" => {
            let page = arguments.get("page")?.as_str()?;
            page_backed(&words, page).then(|| Routed::Page(page.to_owned()))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn output(name: &str, arguments: Value, confidence: f64) -> String {
        json!({"function_calls":[{"name":name,"arguments":arguments}],"suppressed_calls":[],"confidence":confidence})
            .to_string()
    }

    /// What the real model answered to each request, from a run on 2026-10-31.
    #[test]
    fn real_needle_answers_run_only_when_the_words_back_them() {
        let pause = || json!({"action":"pause"});
        for (text, out, want) in [
            (
                "pausa la música",
                output("media_control", pause(), 0.70),
                Some(Routed::Run(Command::MediaPause)),
            ),
            (
                "pon el volumen al 40",
                output("set_volume", json!({"percent":40}), 0.81),
                Some(Routed::Run(Command::SetVolume(0.4))),
            ),
            (
                "abre el clima",
                output("open_page", json!({"page":"weather"}), 0.32),
                Some(Routed::Page("weather".into())),
            ),
            (
                "Pon un temporizador de cinco minutos.",
                output("start_timer", json!({"minutes":5}), 0.74),
                Some(Routed::Run(Command::TimerStart { seconds: 300.0 })),
            ),
            (
                "siguiente canción",
                output("media_control", json!({"action":"next"}), 0.71),
                Some(Routed::Run(Command::MediaNext)),
            ),
            (
                "silencia el audio",
                output("toggle_mute", json!({}), 0.9),
                Some(Routed::Run(Command::ToggleMute)),
            ),
            (
                "el silencio es importante",
                output("toggle_mute", json!({}), 0.9),
                None,
            ),
            // The tuned model scores these 1.0: its score means nothing, the words catch them.
            (
                "no quiero que pauses nada",
                output("media_control", pause(), 1.0),
                None,
            ),
            (
                "el siguiente capítulo es largo",
                output("media_control", json!({"action":"next"}), 1.0),
                None,
            ),
            (
                "abre tu mente a nuevas ideas",
                output("open_page", json!({"page":"media"}), 1.0),
                None,
            ),
            (
                "enséñame francés",
                output("open_page", json!({"page":"timer"}), 0.71),
                None,
            ),
            // The model got these wrong, confidently; the words catch them.
            (
                "salta esta canción",
                output("media_control", pause(), 0.66),
                None,
            ),
            (
                "no pauses la música",
                output("media_control", pause(), 0.51),
                None,
            ),
            (
                "me gusta esta canción",
                output("media_control", pause(), 0.47),
                None,
            ),
            (
                "recomiéndame una canción",
                output("media_control", pause(), 0.38),
                None,
            ),
            (
                "cómo estás hoy",
                output("media_control", pause(), 0.16),
                None,
            ),
            ("qué hora es", output("media_control", pause(), 0.21), None),
            (
                "what is the weather usually like in Spain",
                output("open_page", json!({"page":"weather"}), 0.44),
                None,
            ),
            (
                "tell me a joke",
                output("media_control", json!({"action":"play"}), 0.28),
                None,
            ),
            (
                "muestra me como se hace una tortilla",
                output("open_page", json!({"page":"media"}), 0.08),
                None,
            ),
            // A number that was not said, or too low a score.
            (
                "pon el volumen al 40",
                output("set_volume", json!({"percent":80}), 0.9),
                None,
            ),
            (
                "cuanto dura un temporizador de cocina",
                output("start_timer", json!({"minutes":1}), 0.9),
                None,
            ),
            (
                "pon el volumen al 40",
                output("set_volume", json!({"percent":40}), 0.1),
                None,
            ),
            (
                "sube el volumen al 400",
                output("set_volume", json!({"percent":400}), 0.9),
                None,
            ),
        ] {
            assert_eq!(decide(text, &out), want, "{text}");
        }
    }

    #[test]
    fn withheld_calls_and_bad_output_never_run() {
        let held = json!({"function_calls":[],"suppressed_calls":[{"name":"media_control","arguments":{"action":"pause"}}],"confidence":0.9}).to_string();
        assert_eq!(decide("pausa la música", &held), None);
        assert_eq!(decide("pausa la música", "not json"), None);
        assert_eq!(decide("pausa la música", "{}"), None);
        let two = json!({"function_calls":[{"name":"a","arguments":{}},{"name":"b","arguments":{}}],"confidence":0.9}).to_string();
        assert_eq!(decide("pausa la música", &two), None);
    }

    #[test]
    fn conversation_goes_straight_to_chat_without_the_model() {
        for text in [
            "explícame qué es la fotosíntesis",
            "cuál es la capital de Francia",
            "escríbeme un poema sobre el mar",
            "hola",
            "gracias",
            "no quiero que pongas música",
            "¿pausa la música?",
            "por qué sube el volumen de los anuncios",
            "reproduce un video de gatos en YouTube ahora",
            "pon un temporizador",
        ] {
            assert!(!worth_asking(text), "{text}");
        }
        for text in [
            "pausa la música",
            "pausa",
            "por favor pausa la música",
            "next song",
            "abre las notas",
            "set the volume to 20",
            "set a timer for 10 minutes",
        ] {
            assert!(worth_asking(text), "{text}");
        }
    }

    #[test]
    fn the_tools_are_valid_json() {
        let tools: Value = serde_json::from_str(TOOLS).unwrap();
        assert_eq!(
            tools.as_array().unwrap().len(),
            5,
            "Needle retrieves above five"
        );
    }
}
