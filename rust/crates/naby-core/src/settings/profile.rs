//! Customization profile (`luma.layout.v1`) export and import (C# `CustomizationProfile`).

use super::{presets, ProfileError, Settings};
use serde_json::{Map, Value};

/// File format tag, kept as `luma.layout.v1` for compatibility with existing files.
pub const FORMAT: &str = "luma.layout.v1";

/// Largest layout document accepted, in UTF-16 units like the C# check.
pub const MAX_SOURCE_LEN: usize = 262_144;

/// C# `CustomizationProfile.CustomizationKeys`.
pub const CUSTOMIZATION_KEYS: &[&str] = &[
    "HoverFollowsWidgets",
    "FollowMonitorAnimation",
    "FollowMonitorDuration",
    "DockUnlocked",
    "PanelAutoHeight",
    "BrowserMediaPreviews",
    "WaveformSource",
    "WaveformSensitivity",
    "LyricsAnimation",
    "LyricsOffset",
    "HideFullscreen",
    "HideMaximized",
    "AlwaysTucked",
    "TuckEdge",
    "Preset",
    "Width",
    "Height",
    "Radius",
    "Top",
    "Horizontal",
    "DockEdge",
    "DockVertical",
    "DockInset",
    "Monitor",
    "DisplayMode",
    "Background",
    "Accent",
    "Opacity",
    "SettingsMaterial",
    "GlassStyle",
    "GlassStrength",
    "CoverAmbient",
    "ClosedAmbient",
    "AmbientStrength",
    "AmbientSpread",
    "AmbientMotion",
    "AmbientScope",
    "AmbientWidgets",
    "AmbientColorSource",
    "AmbientCustomColor",
    "AmbientSecondaryColor",
    "AmbientPalette",
    "AmbientDirection",
    "AmbientHeight",
    "AmbientBalance",
    "AmbientSoftness",
    "AmbientAudioResponse",
    "AmbientSensitivity",
    "InteractionMotion",
    "AlbumAccent",
    "PanelWidth",
    "PanelMaxHeight",
    "PanelPadding",
    "PanelScale",
    "SectionGap",
    "HoverWidth",
    "HoverHeight",
    "HoverDelayMs",
    "HoverDetailDelayMs",
    "SwipeDistance",
    "HoverViews",
    "Widgets",
    "CompactLayout",
    "HoverPeek",
    "TwoStageHover",
    "AnimationMs",
    "ReduceMotion",
    "LiveActivity",
    "ClosedMediaIcon",
    "ClosedMediaContent",
    "Performance",
    "WheelAction",
    "MiddleAction",
    "VolumeStep",
    "Clock24Hour",
    "ClockSeconds",
    "ShowDate",
    "MonitorNetwork",
    "MonitorStorage",
    "MonitorProcesses",
    "CategoryStyle",
    "LaunchStyle",
    "BoldMediaTitle",
    "BoldHeadings",
    "WaveformIdleMotion",
    "VolumeFeedback",
    "BrightnessFeedback",
];

/// Keys appended to [`CUSTOMIZATION_KEYS`] by both export and import.
pub const EXTRA_KEYS: &[&str] = &[
    "DropReveal",
    "DetachablePreviews",
    "CoverGestures",
    "ControlFill",
    "ControlBorder",
    "SelectionFill",
    "PanelSwipe",
    "Tools",
];

/// Every key a profile may carry (customization keys followed by the extra ones).
pub fn all_keys() -> Vec<&'static str> {
    CUSTOMIZATION_KEYS
        .iter()
        .chain(EXTRA_KEYS)
        .copied()
        .collect()
}

/// Serializes the customization subset (never shortcuts, hotkeys, startup, clipboard, paths).
pub fn export(settings: &Settings) -> String {
    let full = match serde_json::to_value(settings) {
        Ok(Value::Object(map)) => map,
        _ => Map::new(),
    };
    let mut values = Map::new();
    for key in all_keys() {
        if let Some(v) = full.get(key) {
            values.insert(key.to_string(), v.clone());
        }
    }
    let mut root = Map::new();
    root.insert("format".to_string(), Value::String(FORMAT.to_string()));
    root.insert("values".to_string(), Value::Object(values));
    serde_json::to_string_pretty(&Value::Object(root)).unwrap_or_else(|_| "{}".to_string())
}

/// Parses a profile and applies it to default settings (a fresh install importing a file).
pub fn import(source: &str) -> Result<Settings, ProfileError> {
    let mut settings = Settings::default();
    import_into(source, &mut settings)?;
    Ok(settings)
}

/// Parses a profile and applies only the keys it contains to `target`. `target` is left
/// untouched when the profile is rejected.
pub fn import_into(source: &str, target: &mut Settings) -> Result<(), ProfileError> {
    if source.encode_utf16().count() > MAX_SOURCE_LEN {
        return Err(ProfileError::TooLarge);
    }
    let document: Value =
        serde_json::from_str(source).map_err(|e| ProfileError::Invalid(e.to_string()))?;
    presets::apply_layout(target, &document)
}
