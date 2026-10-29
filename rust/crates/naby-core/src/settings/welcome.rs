//! First-run setup (C# `WelcomeSetup`, `WelcomeState` and the purpose choice in `WelcomeWindow`).
//! Presets only choose which widgets are on; they never enable microphone listening, clipboard
//! capture or model downloads.

use super::Settings;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct WelcomeState {
    pub pending: bool,
    pub completed: bool,
}

/// What the user picked on the first welcome step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WelcomePreset {
    Music,
    Productivity,
    Ai,
    /// "Recommended": keep the complete default experience.
    Custom,
}

impl WelcomePreset {
    /// Widget ids switched on by the preset; `None` keeps the current set.
    pub fn enabled_widgets(self) -> Option<&'static [&'static str]> {
        match self {
            WelcomePreset::Music => Some(&["media", "volume", "clock", "weather"]),
            WelcomePreset::Productivity => {
                Some(&["timer", "focus", "clock", "notes", "tasks", "calendar"])
            }
            WelcomePreset::Ai => Some(&["voice", "clock", "notes"]),
            WelcomePreset::Custom => None,
        }
    }
}

/// Result of [`should_offer`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Offer {
    pub show: bool,
    /// The caller should persist `WelcomeState { pending: true, .. }` (a brand-new profile).
    pub save_pending: bool,
}

/// C# `WelcomeSetup.ShouldOffer`, with the file-existence and existing-profile facts supplied
/// by the caller (the store layer owns the files).
pub fn should_offer(
    state: &WelcomeState,
    state_file_exists: bool,
    existing_profile: bool,
) -> Offer {
    if state.completed {
        return Offer {
            show: false,
            save_pending: false,
        };
    }
    if state.pending {
        return Offer {
            show: true,
            save_pending: false,
        };
    }
    if state_file_exists || existing_profile {
        return Offer {
            show: false,
            save_pending: false,
        };
    }
    Offer {
        show: true,
        save_pending: true,
    }
}

/// State to persist when setup finishes or is skipped.
pub fn completed_state() -> WelcomeState {
    WelcomeState {
        pending: false,
        completed: true,
    }
}

/// Applies the chosen purpose (C# `WelcomeWindow.ChoosePurpose` + `Finish`): widget switches,
/// hover pages follow the widgets, then normalize.
pub fn apply(settings: &mut Settings, preset: WelcomePreset) {
    apply_with(settings, preset, None);
}

/// Like [`apply`], additionally setting the dock edge chosen on the second step.
pub fn apply_with(settings: &mut Settings, preset: WelcomePreset, dock_edge: Option<&str>) {
    if let Some(enabled) = preset.enabled_widgets() {
        for widget in &mut settings.widgets {
            widget.enabled = enabled.contains(&widget.id.as_str());
        }
        settings.hover_follows_widgets = true;
    }
    if let Some(edge) = dock_edge {
        settings.dock_edge = edge.to_string();
    }
    settings.normalize();
}
