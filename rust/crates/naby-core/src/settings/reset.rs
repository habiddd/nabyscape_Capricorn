//! Preference reset and restore (C# `PreferenceReset`). [`reset_scope`] and [`restore_backup`]
//! are the pure parts; [`reset_stored`] and [`restore_stored`] add the `preference-backup`
//! document and the `settings` write through the [`Store`].

use super::{copy_keys, policy::PREMIUM_KEYS, presets, ProfileError, Settings, SETTINGS_DOCUMENT};
use crate::store::{Store, StoreError};
use serde_json::Value;
use std::fmt;

/// Name of the document holding the settings saved before a reset.
pub const BACKUP_DOCUMENT: &str = "preference-backup";

/// Why a stored reset or restore failed. The live settings are unchanged in every case.
#[derive(Debug)]
pub enum PreferenceError {
    Store(StoreError),
    /// The backup file could not be read.
    Io(std::io::Error),
    /// The backup is not a complete settings document.
    Invalid(ProfileError),
}

impl fmt::Display for PreferenceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PreferenceError::Store(e) => e.fmt(f),
            PreferenceError::Io(e) => write!(f, "The preference backup cannot be read: {e}"),
            PreferenceError::Invalid(ProfileError::Invalid(why)) => f.write_str(why),
            PreferenceError::Invalid(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for PreferenceError {}

impl From<StoreError> for PreferenceError {
    fn from(e: StoreError) -> Self {
        PreferenceError::Store(e)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetScope {
    /// Every category except clipboard history capture.
    All,
    Category(ResetCategory),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResetCategory {
    Ambience,
    Motion,
    Clipboard,
    Weather,
    Visibility,
    Notifications,
    Appearance,
    Layout,
    Content,
    Controls,
    Audio,
    System,
    Launch,
    Ai,
    Application,
}

impl ResetCategory {
    pub const ALL: [ResetCategory; 15] = [
        ResetCategory::Ambience,
        ResetCategory::Motion,
        ResetCategory::Clipboard,
        ResetCategory::Weather,
        ResetCategory::Visibility,
        ResetCategory::Notifications,
        ResetCategory::Appearance,
        ResetCategory::Layout,
        ResetCategory::Content,
        ResetCategory::Controls,
        ResetCategory::Audio,
        ResetCategory::System,
        ResetCategory::Launch,
        ResetCategory::Ai,
        ResetCategory::Application,
    ];

    /// The C# category name.
    pub fn name(self) -> &'static str {
        match self {
            ResetCategory::Ambience => "Ambience",
            ResetCategory::Motion => "Motion",
            ResetCategory::Clipboard => "Clipboard",
            ResetCategory::Weather => "Weather",
            ResetCategory::Visibility => "Visibility",
            ResetCategory::Notifications => "Notifications",
            ResetCategory::Appearance => "Appearance",
            ResetCategory::Layout => "Layout",
            ResetCategory::Content => "Content",
            ResetCategory::Controls => "Controls",
            ResetCategory::Audio => "Audio",
            ResetCategory::System => "System",
            ResetCategory::Launch => "Launch",
            ResetCategory::Ai => "AI",
            ResetCategory::Application => "Application",
        }
    }

    pub fn from_name(name: &str) -> Option<ResetCategory> {
        Self::ALL.into_iter().find(|c| c.name() == name)
    }

    /// Property names owned by this category.
    pub fn keys(self) -> &'static [&'static str] {
        match self {
            ResetCategory::Ambience => &[
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
            ],
            ResetCategory::Motion => &[
                "InteractionMotion",
                "AnimationMs",
                "ReduceMotion",
                "LiveActivity",
                "Performance",
            ],
            ResetCategory::Clipboard => &["ClipboardHistory"],
            ResetCategory::Weather => &[
                "WeatherCity",
                "WeatherLatitude",
                "WeatherLongitude",
                "WeatherUnits",
            ],
            ResetCategory::Visibility => &[
                "HideFullscreen",
                "HideMaximized",
                "AlwaysTucked",
                "TuckEdge",
                "TuckHotkeyEnabled",
                "TuckHotkeyKey",
                "TuckHotkeyModifiers",
                "VisibilityHotkeyEnabled",
                "VisibilityHotkeyKey",
                "VisibilityHotkeyModifiers",
                "SmartActivities",
                "BrowserMediaPreviews",
            ],
            ResetCategory::Notifications => &[
                "CountdownSound",
                "CountdownSoundVolume",
                "CountdownSoundFile",
                "NotificationSeconds",
                "NotificationAnimation",
                "QuietHours",
                "QuietStart",
                "QuietEnd",
                "NotificationsSnoozedUntil",
                "NotificationDelivery",
                "NotifyTimer",
                "NotifyFocus",
                "NotifyDownloads",
                "NotifyUpdates",
                "NotifyFailures",
                "DownloadsEnabled",
                "DownloadsFolder",
            ],
            ResetCategory::Appearance => &[
                "Preset",
                "Radius",
                "Background",
                "Accent",
                "Opacity",
                "SettingsMaterial",
                "GlassStyle",
                "GlassStrength",
                "AlbumAccent",
                "ControlFill",
                "ControlBorder",
                "SelectionFill",
                "CategoryStyle",
                "BoldMediaTitle",
                "BoldHeadings",
                "RecentColors",
            ],
            ResetCategory::Layout => &[
                "FollowMonitorAnimation",
                "FollowMonitorDuration",
                "DockUnlocked",
                "PanelAutoHeight",
                "Width",
                "Height",
                "Horizontal",
                "DockEdge",
                "DockVertical",
                "DockInset",
                "Top",
                "Monitor",
                "DisplayMode",
                "CompactLayout",
                "PanelWidth",
                "PanelMaxHeight",
                "PanelPadding",
                "PanelScale",
                "SectionGap",
            ],
            ResetCategory::Content => &[
                "HoverFollowsWidgets",
                "Widgets",
                "Tools",
                "HoverViews",
                "FocusMinutes",
                "BreakMinutes",
            ],
            ResetCategory::Controls => &[
                "HoverPeek",
                "TwoStageHover",
                "HoverWidth",
                "HoverHeight",
                "HoverDelayMs",
                "HoverDetailDelayMs",
                "SwipeDistance",
                "PanelSwipe",
                "DropReveal",
                "DetachablePreviews",
                "CoverGestures",
                "WheelAction",
                "MiddleAction",
                "VolumeStep",
                "HotkeyEnabled",
                "HotkeyKey",
                "HotkeyModifiers",
            ],
            ResetCategory::Audio => &[
                "MediaIntegrationEnabled",
                "WaveformSource",
                "WaveformSensitivity",
                "LyricsEnabled",
                "LyricsAnimation",
                "LyricsOffset",
                "ClosedMediaIcon",
                "ClosedMediaContent",
                "ReactiveFeedback",
                "WaveformIdleMotion",
                "VolumeFeedback",
                "BrightnessFeedback",
            ],
            ResetCategory::System => &[
                "Clock24Hour",
                "ClockSeconds",
                "ShowDate",
                "MonitorNetwork",
                "MonitorStorage",
                "MonitorProcesses",
            ],
            ResetCategory::Launch => &["LaunchStyle"],
            ResetCategory::Ai => &[],
            ResetCategory::Application => &[
                "AutoCheckUpdates",
                "AutoDownloadUpdates",
                "UiLanguage",
                "AccessibilityTextScale",
                "AccessibilityHighContrast",
            ],
        }
    }

    /// One-line description of what the category covers (C# `Summary` detail text).
    pub fn detail(self) -> &'static str {
        match self {
            ResetCategory::Appearance => "dock shape, colors and text style",
            ResetCategory::Ambience => {
                "ambient colors, light shape, audio response and closed-dock glow"
            }
            ResetCategory::Motion => "animation, reduced motion and performance preferences",
            ResetCategory::Clipboard => "clipboard capture preferences",
            ResetCategory::Weather => "weather city and temperature units",
            ResetCategory::Visibility => "hide, tuck, automatic previews and visibility shortcuts",
            ResetCategory::Notifications => {
                "notification delivery, sounds, quiet hours and download alerts"
            }
            ResetCategory::Layout => "island position, display and panel sizes",
            ResetCategory::Content => "widget/tool order, hover pages and focus durations",
            ResetCategory::Controls => "hover timing, gestures, tuck rules and global shortcuts",
            ResetCategory::Audio => "closed media display, waveform and feedback",
            ResetCategory::System => "clock format and monitor readings",
            ResetCategory::Launch => "launch appearance",
            ResetCategory::Application => {
                "update checks and automatic downloads, interface language, text size and contrast"
            }
            ResetCategory::Ai => "update checks, notification delivery and download monitoring",
        }
    }
}

/// Property names a scope resets (`All` omits `ClipboardHistory`, which is only reset explicitly).
pub fn scope_keys(scope: ResetScope) -> Vec<&'static str> {
    match scope {
        ResetScope::Category(c) => c.keys().to_vec(),
        ResetScope::All => {
            let mut keys: Vec<&'static str> = Vec::new();
            for c in ResetCategory::ALL {
                for k in c.keys() {
                    if *k != "ClipboardHistory" && !keys.contains(k) {
                        keys.push(k);
                    }
                }
            }
            keys
        }
    }
}

/// C# `PreferenceReset.Summary`.
pub fn summary(scope: ResetScope) -> String {
    let (label, detail) = match scope {
        ResetScope::Category(c) => (c.name().to_string(), c.detail().to_string()),
        ResetScope::All => (
            "all preferences".to_string(),
            ResetCategory::ALL
                .iter()
                .filter(|c| !c.keys().is_empty())
                .map(|c| format!("{}: {}", c.name(), c.detail()))
                .collect::<Vec<_>>()
                .join("; "),
        ),
    };
    format!(
        "Reset {label} to defaults ({} preferences): {detail}. Startup, clipboard capture/history, shortcuts, Notes, Tasks, shelf, calendar and activity targets are kept. A preference backup is saved first.",
        scope_keys(scope).len()
    )
}

/// Resets the scope to the bundled factory defaults and normalizes. Startup, shortcuts and
/// (for `All`) clipboard history are never touched.
pub fn reset_scope(settings: &mut Settings, scope: ResetScope) {
    let defaults = presets::recommended();
    let mut candidate = settings.clone();
    copy_keys(&defaults, &mut candidate, &scope_keys(scope));
    candidate.normalize();
    *settings = candidate;
}

/// C# `PreferenceReset.Restore`: validates a `preference-backup.json` document and copies its
/// preferences over `settings`. `settings` is unchanged on error.
pub fn restore_backup(settings: &mut Settings, backup_json: &str) -> Result<(), ProfileError> {
    let value: Value = serde_json::from_str(backup_json)
        .map_err(|e| ProfileError::Invalid(format!("The preference backup is invalid: {e}")))?;
    let object = value
        .as_object()
        .filter(|o| o.contains_key("Width"))
        .ok_or_else(|| ProfileError::Invalid("The preference backup is invalid.".to_string()))?;
    let keys = scope_keys(ResetScope::All);
    for key in &keys {
        if PREMIUM_KEYS.contains(key)
            || matches!(*key, "DisplayMode" | "SettingsMaterial")
            || ResetCategory::Application.keys().contains(key)
        {
            continue;
        }
        let nullable = matches!(*key, "WeatherLatitude" | "WeatherLongitude");
        match object.get(*key) {
            Some(v) if !(v.is_null() && !nullable) => {}
            _ => {
                return Err(ProfileError::Invalid(format!(
                    "The preference backup is incomplete: {key}"
                )))
            }
        }
    }
    let restored: Settings = serde_json::from_value(value.clone())
        .map_err(|e| ProfileError::Invalid(format!("The preference backup is invalid: {e}")))?;
    let mut candidate = settings.clone();
    copy_keys(&restored, &mut candidate, &keys);
    candidate.normalize();
    *settings = candidate;
    Ok(())
}

/// C# `PreferenceReset.Reset`: saves the current settings as `preference-backup`, then resets
/// the scope and saves `settings`. The backup must succeed before either the stored or the live
/// preferences change.
pub fn reset_stored(
    store: &Store,
    settings: &mut Settings,
    scope: ResetScope,
) -> Result<(), PreferenceError> {
    let mut candidate = settings.clone();
    reset_scope(&mut candidate, scope);
    store.save(BACKUP_DOCUMENT, settings)?;
    store.save(SETTINGS_DOCUMENT, &candidate)?;
    *settings = candidate;
    Ok(())
}

/// C# `PreferenceReset.Restore`: restores from the stored `preference-backup.json` and saves
/// `settings`.
pub fn restore_stored(store: &Store, settings: &mut Settings) -> Result<(), PreferenceError> {
    let text = std::fs::read_to_string(store.dir().join(format!("{BACKUP_DOCUMENT}.json")))
        .map_err(PreferenceError::Io)?;
    let mut candidate = settings.clone();
    restore_backup(&mut candidate, &text).map_err(PreferenceError::Invalid)?;
    store.save(SETTINGS_DOCUMENT, &candidate)?;
    *settings = candidate;
    Ok(())
}
