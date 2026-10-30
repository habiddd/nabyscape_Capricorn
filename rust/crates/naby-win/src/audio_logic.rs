//! Pure rules shared by the audio services (ported from `AudioDevices.cs`,
//! `MediaAudioMeter.cs`, `MixerAppIcon.cs` and `AudioMixer.cs`). No OS calls.
#![allow(dead_code)]

use naby_core::media::AudioOutput;

/// The failing operation, used to pick the recovery text shown to the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AudioOperation {
    ReadOutput,
    ListOutputs,
    SetOutput,
    SetVolume,
    ToggleMute,
    ReadMicrophone,
    ToggleMicrophoneMute,
}

impl AudioOperation {
    pub const ALL: [AudioOperation; 7] = [
        AudioOperation::ReadOutput,
        AudioOperation::ListOutputs,
        AudioOperation::SetOutput,
        AudioOperation::SetVolume,
        AudioOperation::ToggleMute,
        AudioOperation::ReadMicrophone,
        AudioOperation::ToggleMicrophoneMute,
    ];

    /// Operation-specific English recovery guidance; never contains the native error text.
    pub fn recovery_message(self) -> &'static str {
        match self {
            AudioOperation::ListOutputs => "Could not read audio outputs. Reconnect your device, then open Windows Sound Settings.",
            AudioOperation::SetOutput => "Could not switch audio output. Choose an active device or change the output in Windows Sound Settings.",
            AudioOperation::SetVolume => "Could not change volume. Reconnect your output device or adjust volume in Windows Sound Settings.",
            AudioOperation::ToggleMute => "Could not mute the output. Choose an active device and try again in Windows Sound Settings.",
            AudioOperation::ReadMicrophone => "Could not read the microphone. Reconnect it and choose an input in Windows Sound Settings.",
            AudioOperation::ToggleMicrophoneMute => "Could not mute the microphone. Reconnect it or choose an input in Windows Sound Settings.",
            AudioOperation::ReadOutput => "Could not read the audio device. Reconnect it and choose an output in Windows Sound Settings.",
        }
    }
}

/// Text used when a command arrives after the service closed.
pub const CLOSED_MESSAGE: &str =
    "The audio service has closed. Reopen BillyNabila Island to use audio controls.";

/// Endpoint roles in the order `AudioOutputPolicy.SetDefault` applies them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeviceRole {
    Console,
    Multimedia,
    Communications,
}

pub const OUTPUT_ROLES: [DeviceRole; 3] = [
    DeviceRole::Console,
    DeviceRole::Multimedia,
    DeviceRole::Communications,
];

impl DeviceRole {
    pub fn name(self) -> &'static str {
        match self {
            DeviceRole::Console => "Console",
            DeviceRole::Multimedia => "Multimedia",
            DeviceRole::Communications => "Communications",
        }
    }
}

/// Runs `apply` for every role even when one fails; any failure is reported afterwards
/// (`AudioOutputPolicy.ApplyRoles`).
pub fn apply_roles(
    mut apply: impl FnMut(DeviceRole) -> Result<(), String>,
) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();
    for role in OUTPUT_ROLES {
        if let Err(e) = apply(role) {
            errors.push(format!(
                "Could not change audio output for {}: {e}",
                role.name()
            ));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Output list order: case-insensitive by name.
pub fn sort_outputs(outputs: &mut [AudioOutput]) {
    outputs.sort_by_key(|o| o.name.to_lowercase());
}

/// Whether a media source id (SMTC app id) belongs to a process name (`MediaProcessMatches`).
pub fn media_process_matches(source: &str, process: &str) -> bool {
    let source = source.to_lowercase();
    let process = process.to_lowercase();
    process.chars().count() > 2
        && (source == process
            || source == format!("{process}.exe")
            || source.starts_with(&format!("{process}."))
            || source.starts_with(&format!("{process}_"))
            || source.contains(&format!(".{process}_"))
            || source.ends_with(&format!("!{process}")))
}

/// Two-letter stand-in shown while a mixer icon is pending (`MixerAppIcon.Monogram`).
pub fn monogram(name: &str) -> String {
    let words: Vec<&str> = name
        .split([' ', '-', '_'])
        .filter(|w| !w.is_empty())
        .collect();
    let letters: String = if words.len() == 1 {
        words[0]
            .chars()
            .filter(|c| c.is_alphanumeric())
            .take(2)
            .collect()
    } else {
        words
            .iter()
            .filter_map(|w| w.chars().find(|c| c.is_alphanumeric()))
            .take(2)
            .collect()
    };
    if letters.is_empty() {
        "\u{2022}".to_owned()
    } else {
        letters.to_uppercase()
    }
}

/// `Path.GetFileNameWithoutExtension`: after the last separator, minus the last extension.
pub fn file_stem(path: &str) -> &str {
    let name = path.rsplit(['\\', '/', ':']).next().unwrap_or(path);
    match name.rfind('.') {
        Some(i) => &name[..i],
        None => name,
    }
}

/// Display name when the packaged-app lookup fails (`MediaService.SessionName`).
pub fn session_name_fallback(app_user_model_id: &str) -> String {
    if app_user_model_id.to_lowercase().contains("chrome") {
        "Google Chrome".to_owned()
    } else {
        file_stem(app_user_model_id).to_owned()
    }
}

/// Name for a mixer session (`AudioMixer.Snapshot`): system sounds, the session's own
/// display name, the process name, or the PID.
pub fn session_display_name(
    display: &str,
    system_sounds: bool,
    process_name: Option<&str>,
    pid: u32,
) -> String {
    if system_sounds {
        "System sounds".to_owned()
    } else if !display.trim().is_empty() {
        display.to_owned()
    } else if let Some(name) = process_name.filter(|n| !n.is_empty()) {
        name.to_owned()
    } else {
        format!("App (PID {pid})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_output_values_keep_identity_and_name() {
        let a = AudioOutput {
            id: "endpoint".into(),
            name: "Speaker".into(),
        };
        assert_eq!(a, a.clone());
        assert_eq!((a.id.as_str(), a.name.as_str()), ("endpoint", "Speaker"));
    }

    #[test]
    fn output_switching_applies_all_three_roles_in_order() {
        let mut roles = Vec::new();
        apply_roles(|r| {
            roles.push(r);
            Ok(())
        })
        .unwrap();
        assert_eq!(
            roles,
            [
                DeviceRole::Console,
                DeviceRole::Multimedia,
                DeviceRole::Communications
            ]
        );
    }

    #[test]
    fn failed_role_still_attempts_the_rest_and_reports() {
        let mut roles = Vec::new();
        let err = apply_roles(|r| {
            roles.push(r);
            if r == DeviceRole::Console {
                Err("fixture".to_owned())
            } else {
                Ok(())
            }
        })
        .unwrap_err();
        assert_eq!(roles.len(), 3);
        assert_eq!(err, ["Could not change audio output for Console: fixture"]);
    }

    #[test]
    fn recovery_messages_are_distinct_and_point_to_sound_settings() {
        let mut seen = std::collections::HashSet::new();
        for op in AudioOperation::ALL {
            let m = op.recovery_message();
            assert!(m.contains("Windows Sound Settings"), "{m}");
            assert!(!m.contains("fixture"));
            assert!(seen.insert(m), "duplicate {m}");
        }
        assert_eq!(seen.len(), AudioOperation::ALL.len());
    }

    #[test]
    fn outputs_sort_case_insensitively() {
        let out = |id: &str, name: &str| AudioOutput {
            id: id.into(),
            name: name.into(),
        };
        let mut v = vec![
            out("2", "speakers"),
            out("1", "Headphones"),
            out("3", "Monitor"),
        ];
        sort_outputs(&mut v);
        let names: Vec<_> = v.iter().map(|o| o.name.as_str()).collect();
        assert_eq!(names, ["Headphones", "Monitor", "speakers"]);
    }

    #[test]
    fn process_matching_follows_csharp_rules() {
        assert!(media_process_matches("Spotify.exe", "spotify"));
        assert!(media_process_matches("spotify", "Spotify"));
        assert!(media_process_matches("Spotify_abc!App", "spotify"));
        assert!(media_process_matches("a.zunemusic_8weky", "zunemusic"));
        assert!(media_process_matches("Pkg!chrome", "chrome"));
        assert!(!media_process_matches("chrome.exe", "ch"));
        assert!(!media_process_matches("vlc.exe", "chrome"));
        assert!(!media_process_matches(
            "Microsoft.ZuneMusic_8weky!Microsoft.ZuneMusic",
            "music"
        ));
    }

    #[test]
    fn monogram_picks_two_letters() {
        assert_eq!(monogram("Google Chrome"), "GC");
        assert_eq!(monogram("spotify"), "SP");
        assert_eq!(monogram("my-cool_app x"), "MC");
        assert_eq!(monogram("---"), "\u{2022}");
        assert_eq!(monogram("(a)"), "A");
    }

    #[test]
    fn session_names_fall_back_like_csharp() {
        assert_eq!(session_name_fallback("Spotify.exe"), "Spotify");
        assert_eq!(session_name_fallback("chrome.EXE"), "Google Chrome");
        assert_eq!(session_name_fallback("MSEdge"), "MSEdge");
        assert_eq!(
            session_name_fallback("Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic"),
            "Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft"
        );
        assert_eq!(file_stem(r"C:\Apps\vlc.exe"), "vlc");
    }

    #[test]
    fn mixer_session_names() {
        assert_eq!(session_display_name("x", true, None, 4), "System sounds");
        assert_eq!(session_display_name("  ", false, Some("vlc"), 4), "vlc");
        assert_eq!(session_display_name("", false, None, 42), "App (PID 42)");
        assert_eq!(
            session_display_name("Music", false, Some("vlc"), 4),
            "Music"
        );
    }
}
