//! Ported from `UI/Settings/SettingsFeatureStatus.cs`: the "Your setup" readiness model.
//!
//! Each feature reduces plain inputs (settings plus a few live flags the host supplies) to a
//! state line, a detail sentence and the label of the button that fixes it. Copy is the C#
//! English caption, translated through [`crate::i18n::t`] at paint time.

use crate::settings::Settings;
use crate::weather::{self, WeatherSnapshot};

/// One row of the setup list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeatureStatus {
    pub state: &'static str,
    pub detail: &'static str,
    /// Caption of the row's button.
    pub action: &'static str,
}

impl FeatureStatus {
    const fn new(state: &'static str, detail: &'static str, action: &'static str) -> Self {
        Self {
            state,
            detail,
            action,
        }
    }

    /// The row heading, `"{title} · {state}"`.
    pub fn heading(&self, title: &str) -> String {
        format!("{title} · {}", self.state)
    }
}

/// Row titles in display order.
pub const FEATURES: [&str; 5] = ["Music", "AI chat", "Voice", "Lyrics", "Weather"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MusicInput {
    pub integration_enabled: bool,
    pub widget_enabled: bool,
    /// A media session is available.
    pub available: bool,
}

impl MusicInput {
    pub fn from_settings(s: &Settings, available: bool) -> Self {
        Self {
            integration_enabled: s.media_integration_enabled,
            widget_enabled: s.widgets.iter().any(|w| w.enabled && w.id == "media"),
            available,
        }
    }
}

pub fn music(i: &MusicInput) -> FeatureStatus {
    if !i.integration_enabled {
        FeatureStatus::new(
            "Off",
            "Music apps are disconnected. Playback in your music app is unaffected.",
            "Enable music",
        )
    } else if !i.widget_enabled {
        FeatureStatus::new(
            "Hidden",
            "Enable the music widget to show playback controls.",
            "Choose widgets",
        )
    } else if !i.available {
        FeatureStatus::new(
            "Waiting for music",
            "Play a song in your music app, then select its media session.",
            "Music settings",
        )
    } else {
        FeatureStatus::new(
            "Ready",
            "Playback controls use the selected music session.",
            "Music settings",
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AiInput {
    pub enabled: bool,
    pub chat_configured: bool,
    /// The chat setup title reports the last check needs attention.
    pub needs_attention: bool,
    pub chat_verified: bool,
}

impl AiInput {
    /// `needs_attention` is C# `Setup.Chat.Title.Contains("attention", OrdinalIgnoreCase)`.
    pub fn attention_from_title(title: &str) -> bool {
        title.to_lowercase().contains("attention")
    }
}

pub fn ai_chat(i: &AiInput) -> FeatureStatus {
    if !i.enabled {
        FeatureStatus::new(
            "Off",
            "AI is optional. Connect a provider when you are ready.",
            "Set up AI",
        )
    } else if !i.chat_configured {
        FeatureStatus::new(
            "Needs setup",
            "Choose a provider or connect Hermes, then save your connection.",
            "Connect AI",
        )
    } else if i.needs_attention {
        FeatureStatus::new(
            "Needs attention",
            "The last connection check failed. Review the connection and try again.",
            "Review connection",
        )
    } else if !i.chat_verified {
        FeatureStatus::new(
            "Saved · unchecked",
            "Your connection is saved. Check it before starting a conversation.",
            "Review connection",
        )
    } else {
        FeatureStatus::new(
            "Ready",
            "The selected chat connection has completed a reply.",
            "Open chat",
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoiceInput {
    pub voice_connected: bool,
    pub ai_enabled: bool,
    pub speech_busy: bool,
    /// Local voice is enabled, managed, and not using the user's own models.
    pub managed_local_voice: bool,
    pub chat_configured: bool,
    pub voice_configured: bool,
    pub local_voice_enabled: bool,
}

pub fn voice(i: &VoiceInput) -> FeatureStatus {
    if i.voice_connected {
        FeatureStatus::new(
            "Microphone on",
            "An active voice conversation is using your microphone.",
            "Voice settings",
        )
    } else if !i.ai_enabled {
        FeatureStatus::new(
            "Off",
            "Voice starts only after you choose to enable and configure AI.",
            "Set up voice",
        )
    } else if i.speech_busy {
        FeatureStatus::new(
            "Downloading",
            "Speech components are being prepared. Progress is in Voice settings.",
            "View progress",
        )
    } else if i.managed_local_voice && i.chat_configured && !i.voice_configured {
        FeatureStatus::new(
            "Model download required",
            "Install the selected listening and speaking pack in Voice setup.",
            "Install models",
        )
    } else if !i.voice_configured {
        FeatureStatus::new(
            "Needs setup",
            "Choose a voice connection or install your listening and speaking models.",
            "Set up voice",
        )
    } else {
        FeatureStatus::new(
            "Configured · unchecked",
            "Your setup is saved. Check audio or start a conversation to verify it.",
            if i.local_voice_enabled {
                "Check audio"
            } else {
                "Open voice"
            },
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LyricsInput<'a> {
    pub lyrics_enabled: bool,
    pub closed_dock_shows_lyrics: bool,
    pub media_available: bool,
    /// The player provided enough track details and playback timing (`LyricTiming.CanUse`).
    pub can_time: bool,
    pub has_lines: bool,
    /// The lookup's status text.
    pub status: &'a str,
}

impl<'a> LyricsInput<'a> {
    pub fn from_settings(
        s: &Settings,
        media_available: bool,
        can_time: bool,
        has_lines: bool,
        status: &'a str,
    ) -> Self {
        Self {
            lyrics_enabled: s.lyrics_enabled,
            closed_dock_shows_lyrics: s.closed_media_content == "Lyrics",
            media_available,
            can_time,
            has_lines,
            status,
        }
    }
}

pub fn lyrics(i: &LyricsInput<'_>) -> FeatureStatus {
    let status = i.status;
    let lower = status.to_lowercase();
    if !i.lyrics_enabled && !i.closed_dock_shows_lyrics {
        FeatureStatus::new(
            "Off",
            "Enable panel lyrics or choose Live lyrics for the closed dock.",
            "Choose lyrics",
        )
    } else if !i.media_available {
        FeatureStatus::new(
            "Waiting for music",
            "Play a song to look up timed lyrics.",
            "Music settings",
        )
    } else if !i.can_time {
        FeatureStatus::new(
            "Unavailable",
            "This player has not provided enough track details or playback timing.",
            "Music settings",
        )
    } else if i.has_lines {
        FeatureStatus::new(
            "Ready",
            "Timed lyrics are available for the selected track.",
            "Lyrics settings",
        )
    } else if status.starts_with("Finding") {
        FeatureStatus::new(
            "Finding lyrics",
            "Looking up timed lyrics for this song.",
            "Lyrics settings",
        )
    } else if status.starts_with("No synced") || status.starts_with("Instrumental") {
        FeatureStatus::new(
            "Unavailable for this song",
            "No timed lyrics were found. The closed dock uses the song title.",
            "Lyrics settings",
        )
    } else if lower.contains("unavailable") || lower.contains("timed out") {
        FeatureStatus::new(
            "Needs attention",
            "Lyrics lookup could not complete. Open the lyrics panel to retry.",
            "Lyrics settings",
        )
    } else {
        FeatureStatus::new(
            "Waiting for lyrics",
            "Show the lyrics view to look up this song. Some songs have no timed lyrics.",
            "Lyrics settings",
        )
    }
}

/// Weather readiness: a city is chosen, and whether its forecast is already saved.
pub fn weather(s: &Settings, cached: Option<&WeatherSnapshot>) -> FeatureStatus {
    let key = weather::key_for(s);
    if key.is_empty() {
        FeatureStatus::new(
            "Needs a city",
            "Choose your city to see the forecast.",
            "Choose city",
        )
    } else if cached.is_some_and(|c| c.key == key) {
        FeatureStatus::new(
            "Forecast saved",
            "Open Weather to view the forecast and its update time.",
            "Open weather",
        )
    } else {
        FeatureStatus::new(
            "Needs a forecast",
            "Your city is selected. Open Weather to load its forecast.",
            "Open weather",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n;

    fn music_in(integration: bool, widget: bool, available: bool) -> MusicInput {
        MusicInput {
            integration_enabled: integration,
            widget_enabled: widget,
            available,
        }
    }

    #[test]
    fn music_states() {
        assert_eq!(music(&music_in(false, true, true)).state, "Off");
        assert_eq!(music(&music_in(false, true, true)).action, "Enable music");
        assert_eq!(music(&music_in(true, false, true)).state, "Hidden");
        assert_eq!(
            music(&music_in(true, true, false)).state,
            "Waiting for music"
        );
        let ready = music(&music_in(true, true, true));
        assert_eq!((ready.state, ready.action), ("Ready", "Music settings"));
        assert_eq!(ready.heading("Music"), "Music · Ready");
    }

    #[test]
    fn music_input_follows_settings() {
        let mut s = Settings {
            media_integration_enabled: true,
            ..Settings::default()
        };
        for w in &mut s.widgets {
            w.enabled = w.id == "media";
        }
        let i = MusicInput::from_settings(&s, true);
        assert!(i.integration_enabled && i.widget_enabled && i.available);
        for w in &mut s.widgets {
            w.enabled = false;
        }
        assert!(!MusicInput::from_settings(&s, true).widget_enabled);
    }

    #[test]
    fn ai_states_in_priority_order() {
        let base = AiInput {
            enabled: true,
            chat_configured: true,
            needs_attention: false,
            chat_verified: true,
        };
        assert_eq!(
            ai_chat(&AiInput {
                enabled: false,
                ..base
            })
            .state,
            "Off"
        );
        assert_eq!(
            ai_chat(&AiInput {
                chat_configured: false,
                ..base
            })
            .state,
            "Needs setup"
        );
        assert_eq!(
            ai_chat(&AiInput {
                needs_attention: true,
                ..base
            })
            .state,
            "Needs attention"
        );
        // Attention outranks "unchecked".
        assert_eq!(
            ai_chat(&AiInput {
                needs_attention: true,
                chat_verified: false,
                ..base
            })
            .state,
            "Needs attention"
        );
        assert_eq!(
            ai_chat(&AiInput {
                chat_verified: false,
                ..base
            })
            .state,
            "Saved · unchecked"
        );
        assert_eq!(ai_chat(&base).state, "Ready");
        assert!(AiInput::attention_from_title("Connection Needs ATTENTION"));
        assert!(!AiInput::attention_from_title("Connected"));
    }

    #[test]
    fn voice_states_in_priority_order() {
        let base = VoiceInput {
            voice_connected: false,
            ai_enabled: true,
            speech_busy: false,
            managed_local_voice: false,
            chat_configured: true,
            voice_configured: true,
            local_voice_enabled: false,
        };
        assert_eq!(
            voice(&VoiceInput {
                voice_connected: true,
                ai_enabled: false,
                ..base
            })
            .state,
            "Microphone on"
        );
        assert_eq!(
            voice(&VoiceInput {
                ai_enabled: false,
                ..base
            })
            .state,
            "Off"
        );
        assert_eq!(
            voice(&VoiceInput {
                speech_busy: true,
                ..base
            })
            .state,
            "Downloading"
        );
        let missing = VoiceInput {
            managed_local_voice: true,
            voice_configured: false,
            ..base
        };
        assert_eq!(voice(&missing).state, "Model download required");
        assert_eq!(
            voice(&VoiceInput {
                chat_configured: false,
                ..missing
            })
            .state,
            "Needs setup"
        );
        assert_eq!(
            voice(&VoiceInput {
                voice_configured: false,
                ..base
            })
            .state,
            "Needs setup"
        );
        assert_eq!(voice(&base).action, "Open voice");
        assert_eq!(
            voice(&VoiceInput {
                local_voice_enabled: true,
                ..base
            })
            .action,
            "Check audio"
        );
        assert_eq!(voice(&base).state, "Configured · unchecked");
    }

    fn lyrics_in(status: &str) -> LyricsInput<'_> {
        LyricsInput {
            lyrics_enabled: true,
            closed_dock_shows_lyrics: false,
            media_available: true,
            can_time: true,
            has_lines: false,
            status,
        }
    }

    #[test]
    fn lyrics_states_in_priority_order() {
        let off = LyricsInput {
            lyrics_enabled: false,
            ..lyrics_in("")
        };
        assert_eq!(lyrics(&off).state, "Off");
        assert_eq!(
            lyrics(&LyricsInput {
                closed_dock_shows_lyrics: true,
                ..off
            })
            .state,
            "Waiting for lyrics"
        );
        assert_eq!(
            lyrics(&LyricsInput {
                media_available: false,
                ..lyrics_in("")
            })
            .state,
            "Waiting for music"
        );
        assert_eq!(
            lyrics(&LyricsInput {
                can_time: false,
                ..lyrics_in("")
            })
            .state,
            "Unavailable"
        );
        assert_eq!(
            lyrics(&LyricsInput {
                has_lines: true,
                ..lyrics_in("Finding lyrics")
            })
            .state,
            "Ready"
        );
        assert_eq!(
            lyrics(&lyrics_in("Finding lyrics…")).state,
            "Finding lyrics"
        );
        assert_eq!(
            lyrics(&lyrics_in("No synced lyrics")).state,
            "Unavailable for this song"
        );
        assert_eq!(
            lyrics(&lyrics_in("Instrumental")).state,
            "Unavailable for this song"
        );
        assert_eq!(
            lyrics(&lyrics_in("Lyrics service Unavailable")).state,
            "Needs attention"
        );
        assert_eq!(
            lyrics(&lyrics_in("Request timed out")).state,
            "Needs attention"
        );
        assert_eq!(lyrics(&lyrics_in("")).state, "Waiting for lyrics");
        // Prefix checks are case-sensitive, like the C# ordinal comparisons.
        assert_eq!(lyrics(&lyrics_in("finding")).state, "Waiting for lyrics");
        let s = Settings {
            lyrics_enabled: false,
            closed_media_content: "Lyrics".into(),
            ..Settings::default()
        };
        let i = LyricsInput::from_settings(&s, true, true, false, "");
        assert!(!i.lyrics_enabled && i.closed_dock_shows_lyrics);
    }

    #[test]
    fn weather_states() {
        let mut s = Settings::default();
        assert_eq!(weather(&s, None).state, "Needs a city");
        s.weather_latitude = Some(1.0);
        s.weather_longitude = Some(2.0);
        assert_eq!(weather(&s, None).state, "Needs a forecast");
        let other = WeatherSnapshot {
            key: "other".into(),
            ..Default::default()
        };
        assert_eq!(weather(&s, Some(&other)).state, "Needs a forecast");
        let saved = WeatherSnapshot {
            key: weather::key_for(&s),
            ..Default::default()
        };
        assert_eq!(weather(&s, Some(&saved)).state, "Forecast saved");
    }

    #[test]
    fn captions_translate_like_the_catalog_and_fall_back_to_english() {
        let open_chat = ai_chat(&AiInput {
            enabled: true,
            chat_configured: true,
            needs_attention: false,
            chat_verified: true,
        });
        assert_eq!(open_chat.action, "Open chat");
        assert_eq!(
            i18n::t("es", open_chat.action),
            i18n::entry("Open chat").unwrap().1
        );
        // Captions the C# catalog does not list stay English in every language.
        assert!(i18n::entry("Enable music").is_none());
        assert_eq!(i18n::t("id", "Enable music"), "Enable music");
        assert_eq!(FEATURES, ["Music", "AI chat", "Voice", "Lyrics", "Weather"]);
    }
}
