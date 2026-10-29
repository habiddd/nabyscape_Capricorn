//! Ported from the C# NabyScape sources; see docs/superpowers/plans/2026-10-29-rust-port.md.
//! The user preference model (`Settings`), its normalization rules, bundled presets,
//! customization profiles, preference reset and first-run setup.
//!
//! C# sources: `Application/{Models,SettingsProduction,LanguageAccessibilitySettings,
//! MediaUxSettings,FlexibleExperience,PremiumPreferences}.cs` (all `partial class Settings`).

mod backdrop;
mod normalize;
pub mod policy;
pub mod presets;
pub mod profile;
pub mod reset;
pub mod welcome;

use chrono::{DateTime, FixedOffset};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};

pub use backdrop::{
    resolve_backdrop, BackdropConditions, BackdropResult, SETTINGS_BACKDROP_CHOICES,
};
pub use presets::ProfileError;

/// Main-panel widget ids and display names, in C# `Settings.Names` insertion order.
pub const WIDGET_NAMES: &[(&str, &str)] = &[
    ("voice", "Naby voice"),
    ("media", "Media"),
    ("volume", "Volume"),
    ("timer", "Timer"),
    ("stopwatch", "Stopwatch"),
    ("focus", "Focus mode"),
    ("clock", "Clock & date"),
    ("system", "System monitor"),
    ("battery", "Battery"),
    ("shortcuts", "Shortcuts"),
    ("tools", "Everyday tools"),
    ("files", "File shelf"),
    ("notes", "Notes"),
    ("tasks", "Tasks"),
    ("clips", "Clipboard"),
    ("calendar", "Calendar"),
    ("awake", "Keep awake"),
    ("links", "Clean links"),
    ("weather", "Weather"),
];

/// Everyday-tool ids and display names, in C# `Settings.ToolNames` insertion order.
pub const TOOL_NAMES: &[(&str, &str)] = &[
    ("files", "File shelf"),
    ("notes", "Notes"),
    ("tasks", "Tasks"),
    ("clips", "Clipboard"),
    ("calendar", "Calendar"),
    ("awake", "Keep awake"),
    ("links", "Clean links"),
    ("weather", "Weather"),
];

/// Ids that can appear as hover pages (C# `HoverPagePolicy.Ids`).
pub const HOVER_PAGE_IDS: &[&str] = &[
    "media",
    "volume",
    "timer",
    "focus",
    "calendar",
    "notes",
    "files",
    "system",
    "clips",
    "tasks",
    "stopwatch",
    "weather",
    "clock",
    "battery",
    "shortcuts",
    "awake",
    "links",
    "voice",
];

pub fn is_widget_name(id: &str) -> bool {
    WIDGET_NAMES.iter().any(|(k, _)| *k == id)
}

pub fn is_tool_name(id: &str) -> bool {
    TOOL_NAMES.iter().any(|(k, _)| *k == id)
}

/// Display name for a widget id (C# `Settings.Names`).
pub fn widget_display_name(id: &str) -> Option<&'static str> {
    WIDGET_NAMES.iter().find(|(k, _)| *k == id).map(|(_, v)| *v)
}

/// `^#[0-9a-fA-F]{6}$` exactly as .NET evaluates it (`$` also matches before a final `\n`).
pub fn valid_color(value: &str) -> bool {
    let v = value.strip_suffix('\n').unwrap_or(value);
    let b = v.as_bytes();
    b.len() == 7 && b[0] == b'#' && b[1..].iter().all(u8::is_ascii_hexdigit)
}

// ---- lenient (System.Text.Json-compatible) deserialization helpers -------------------------

/// A JSON `null` string becomes `""`, which every normalizer treats like the C# `null`.
fn null_string<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(Option::<String>::deserialize(d)?.unwrap_or_default())
}

/// A JSON `null` list becomes empty and `null` elements are dropped (C# filters `x != null`).
fn null_list<'de, D, T>(d: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Option::<Vec<Option<T>>>::deserialize(d)?
        .unwrap_or_default()
        .into_iter()
        .flatten()
        .collect())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct WidgetSetting {
    #[serde(deserialize_with = "null_string")]
    pub id: String,
    pub enabled: bool,
}

impl Default for WidgetSetting {
    fn default() -> Self {
        WidgetSetting {
            id: String::new(),
            enabled: true,
        }
    }
}

impl WidgetSetting {
    pub fn new(id: &str, enabled: bool) -> Self {
        WidgetSetting {
            id: id.to_string(),
            enabled,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct Shortcut {
    #[serde(deserialize_with = "null_string")]
    pub name: String,
    #[serde(deserialize_with = "null_string")]
    pub target: String,
    pub app_id: Option<String>,
}

/// Dock edge (C# `DockEdge` string).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DockEdge {
    Top,
    Bottom,
    Left,
    Right,
    Floating,
}

/// How the dock picks its display (C# `DisplayMode` string).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayMode {
    Primary,
    Selected,
    Follow,
}

/// Every persisted preference. One field per C# property; defaults are the C# initializers.
/// String-valued options stay `String` for file compatibility; use the typed accessors.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct Settings {
    // ---- Models.cs
    #[serde(deserialize_with = "null_string")]
    pub notification_delivery: String,
    pub notify_timer: bool,
    pub notify_focus: bool,
    pub notify_downloads: bool,
    pub notify_updates: bool,
    pub notify_failures: bool,
    pub downloads_enabled: bool,
    #[serde(deserialize_with = "null_string")]
    pub downloads_folder: String,
    #[serde(deserialize_with = "null_string")]
    pub preset: String,
    pub width: f64,
    pub height: f64,
    pub radius: f64,
    pub compact_layout: bool,
    pub hover_peek: bool,
    pub drop_reveal: bool,
    pub detachable_previews: bool,
    pub cover_gestures: bool,
    pub glass_style: bool,
    #[serde(deserialize_with = "null_string")]
    pub settings_material: String,
    pub live_activity: bool,
    #[serde(deserialize_with = "null_string")]
    pub closed_media_icon: String,
    #[serde(deserialize_with = "null_string")]
    pub closed_media_content: String,
    pub reactive_feedback: bool,
    pub smart_activities: bool,
    pub album_accent: bool,
    pub two_stage_hover: bool,
    pub panel_width: f64,
    pub panel_max_height: f64,
    pub panel_padding: f64,
    pub panel_scale: f64,
    pub section_gap: f64,
    pub hover_width: f64,
    pub hover_height: f64,
    pub hover_delay_ms: i32,
    pub hover_detail_delay_ms: i32,
    pub swipe_distance: f64,
    pub glass_strength: f64,
    pub clock24_hour: bool,
    pub clock_seconds: bool,
    pub show_date: bool,
    pub monitor_network: bool,
    pub monitor_storage: bool,
    pub monitor_processes: bool,
    #[serde(deserialize_with = "null_list")]
    pub hover_views: Vec<WidgetSetting>,
    pub clipboard_history: bool,
    #[serde(deserialize_with = "null_list")]
    pub tools: Vec<WidgetSetting>,
    #[serde(deserialize_with = "null_string")]
    pub performance: String,
    #[serde(deserialize_with = "null_string")]
    pub wheel_action: String,
    #[serde(deserialize_with = "null_string")]
    pub middle_action: String,
    pub volume_step: i32,
    pub hotkey_enabled: bool,
    #[serde(deserialize_with = "null_string")]
    pub hotkey_key: String,
    #[serde(deserialize_with = "null_string")]
    pub hotkey_modifiers: String,
    pub horizontal: f64,
    pub top: f64,
    #[serde(deserialize_with = "null_string")]
    pub monitor: String,
    #[serde(deserialize_with = "null_string")]
    pub display_mode: String,
    #[serde(deserialize_with = "null_string")]
    pub background: String,
    #[serde(deserialize_with = "null_string")]
    pub accent: String,
    #[serde(deserialize_with = "null_string")]
    pub control_fill: String,
    #[serde(deserialize_with = "null_string")]
    pub control_border: String,
    #[serde(deserialize_with = "null_string")]
    pub selection_fill: String,
    pub panel_swipe: bool,
    pub opacity: f64,
    pub animation_ms: f64,
    pub reduce_motion: bool,
    pub hide_fullscreen: bool,
    pub hide_maximized: bool,
    pub always_tucked: bool,
    pub tuck_edge: f64,
    pub tuck_hotkey_enabled: bool,
    #[serde(deserialize_with = "null_string")]
    pub tuck_hotkey_key: String,
    #[serde(deserialize_with = "null_string")]
    pub tuck_hotkey_modifiers: String,
    pub visibility_hotkey_enabled: bool,
    #[serde(deserialize_with = "null_string")]
    pub visibility_hotkey_key: String,
    #[serde(deserialize_with = "null_string")]
    pub visibility_hotkey_modifiers: String,
    #[serde(deserialize_with = "null_string")]
    pub category_style: String,
    #[serde(deserialize_with = "null_string")]
    pub launch_style: String,
    pub bold_media_title: bool,
    pub bold_headings: bool,
    pub waveform_idle_motion: bool,
    pub volume_feedback: bool,
    pub brightness_feedback: bool,
    #[serde(deserialize_with = "null_list")]
    pub recent_colors: Vec<String>,
    pub auto_check_updates: bool,
    pub auto_start: bool,
    pub focus_minutes: i32,
    pub break_minutes: i32,
    #[serde(deserialize_with = "null_list")]
    pub widgets: Vec<WidgetSetting>,
    #[serde(deserialize_with = "null_list")]
    pub shortcuts: Vec<Shortcut>,
    // ---- SettingsProduction.cs
    pub auto_download_updates: bool,
    // ---- LanguageAccessibilitySettings.cs
    #[serde(deserialize_with = "null_string")]
    pub ui_language: String,
    pub accessibility_text_scale: f64,
    pub accessibility_high_contrast: bool,
    // ---- MediaUxSettings.cs
    pub browser_media_previews: bool,
    pub dock_unlocked: bool,
    pub panel_auto_height: bool,
    #[serde(deserialize_with = "null_string")]
    pub waveform_source: String,
    pub waveform_sensitivity: f64,
    pub media_integration_enabled: bool,
    pub lyrics_enabled: bool,
    #[serde(deserialize_with = "null_string")]
    pub lyrics_animation: String,
    pub lyrics_offset: f64,
    #[serde(deserialize_with = "null_string")]
    pub countdown_sound: String,
    pub countdown_sound_volume: f64,
    #[serde(deserialize_with = "null_string")]
    pub countdown_sound_file: String,
    // ---- FlexibleExperience.cs
    pub hover_follows_widgets: bool,
    pub follow_monitor_animation: bool,
    pub follow_monitor_duration: f64,
    pub closed_ambient: bool,
    #[serde(deserialize_with = "null_string")]
    pub dock_edge: String,
    pub dock_vertical: f64,
    pub dock_inset: f64,
    #[serde(deserialize_with = "null_string")]
    pub ambient_palette: String,
    #[serde(deserialize_with = "null_string")]
    pub ambient_direction: String,
    pub ambient_height: f64,
    pub ambient_balance: f64,
    pub ambient_softness: f64,
    #[serde(deserialize_with = "null_string")]
    pub ambient_secondary_color: String,
    #[serde(deserialize_with = "null_string")]
    pub weather_city: String,
    pub weather_latitude: Option<f64>,
    pub weather_longitude: Option<f64>,
    #[serde(deserialize_with = "null_string")]
    pub weather_units: String,
    // ---- PremiumPreferences.cs
    pub cover_ambient: bool,
    pub ambient_strength: f64,
    pub ambient_spread: f64,
    #[serde(deserialize_with = "null_string")]
    pub ambient_motion: String,
    #[serde(deserialize_with = "null_string")]
    pub ambient_scope: String,
    #[serde(deserialize_with = "null_list")]
    pub ambient_widgets: Vec<String>,
    #[serde(deserialize_with = "null_string")]
    pub ambient_color_source: String,
    #[serde(deserialize_with = "null_string")]
    pub ambient_custom_color: String,
    #[serde(deserialize_with = "null_string")]
    pub ambient_audio_response: String,
    pub ambient_sensitivity: f64,
    #[serde(deserialize_with = "null_string")]
    pub interaction_motion: String,
    pub notification_seconds: f64,
    #[serde(deserialize_with = "null_string")]
    pub notification_animation: String,
    pub quiet_hours: bool,
    pub quiet_start: i32,
    pub quiet_end: i32,
    pub notifications_snoozed_until: Option<DateTime<FixedOffset>>,

    /// Keys this port does not model; preserved verbatim on save.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

fn s(v: &str) -> String {
    v.to_string()
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            notification_delivery: s("Notch"),
            notify_timer: true,
            notify_focus: true,
            notify_downloads: true,
            notify_updates: true,
            notify_failures: true,
            downloads_enabled: false,
            downloads_folder: String::new(),
            preset: s("Mac"),
            width: 228.0,
            height: 32.0,
            radius: 16.0,
            compact_layout: true,
            hover_peek: true,
            drop_reveal: true,
            detachable_previews: true,
            cover_gestures: true,
            glass_style: true,
            settings_material: String::new(),
            live_activity: true,
            closed_media_icon: s("Waveform"),
            closed_media_content: s("Title"),
            reactive_feedback: true,
            smart_activities: true,
            album_accent: true,
            two_stage_hover: true,
            panel_width: 384.0,
            panel_max_height: 408.0,
            panel_padding: 32.0,
            panel_scale: 1.0,
            section_gap: 18.0,
            hover_width: 350.0,
            hover_height: 192.0,
            hover_delay_ms: 180,
            hover_detail_delay_ms: 550,
            swipe_distance: 48.0,
            glass_strength: 1.0,
            clock24_hour: true,
            clock_seconds: false,
            show_date: true,
            monitor_network: true,
            monitor_storage: true,
            monitor_processes: true,
            hover_views: default_hover_views(),
            clipboard_history: false,
            tools: TOOL_NAMES
                .iter()
                .map(|(id, _)| WidgetSetting::new(id, true))
                .collect(),
            performance: s("Auto"),
            wheel_action: s("volume"),
            middle_action: s("toggle"),
            volume_step: 2,
            hotkey_enabled: true,
            hotkey_key: s("I"),
            hotkey_modifiers: s("Ctrl+Alt"),
            horizontal: 50.0,
            top: 0.0,
            monitor: String::new(),
            display_mode: String::new(),
            background: s("#000000"),
            accent: s("#FFFFFF"),
            control_fill: s("#16171B"),
            control_border: s("#303238"),
            selection_fill: s("#34373E"),
            panel_swipe: true,
            opacity: 1.0,
            animation_ms: 420.0,
            reduce_motion: false,
            hide_fullscreen: true,
            hide_maximized: false,
            always_tucked: false,
            tuck_edge: 6.0,
            tuck_hotkey_enabled: true,
            tuck_hotkey_key: s("T"),
            tuck_hotkey_modifiers: s("Ctrl+Alt"),
            visibility_hotkey_enabled: true,
            visibility_hotkey_key: s("H"),
            visibility_hotkey_modifiers: s("Ctrl+Alt"),
            category_style: s("Dots"),
            launch_style: s("IconsLabels"),
            bold_media_title: true,
            bold_headings: true,
            waveform_idle_motion: true,
            volume_feedback: true,
            brightness_feedback: true,
            recent_colors: Vec::new(),
            auto_check_updates: true,
            auto_start: false,
            focus_minutes: 25,
            break_minutes: 5,
            widgets: default_widgets(),
            shortcuts: vec![
                Shortcut {
                    name: s("Explorer"),
                    target: s("explorer.exe"),
                    app_id: None,
                },
                Shortcut {
                    name: s("Settings"),
                    target: s("ms-settings:"),
                    app_id: None,
                },
            ],
            auto_download_updates: false,
            ui_language: s("en"),
            accessibility_text_scale: 1.0,
            accessibility_high_contrast: false,
            browser_media_previews: true,
            dock_unlocked: false,
            panel_auto_height: true,
            waveform_source: s("Spectrum"),
            waveform_sensitivity: 1.35,
            media_integration_enabled: true,
            lyrics_enabled: false,
            lyrics_animation: s("Slide"),
            lyrics_offset: 0.0,
            countdown_sound: s("Chime"),
            countdown_sound_volume: 0.7,
            countdown_sound_file: String::new(),
            hover_follows_widgets: true,
            follow_monitor_animation: true,
            follow_monitor_duration: 360.0,
            closed_ambient: false,
            dock_edge: s("Top"),
            dock_vertical: 50.0,
            dock_inset: 8.0,
            ambient_palette: s("Cover blend"),
            ambient_direction: s("Bottom"),
            ambient_height: 1.0,
            ambient_balance: 0.5,
            ambient_softness: 0.5,
            ambient_secondary_color: s("#CB72D9"),
            weather_city: String::new(),
            weather_latitude: None,
            weather_longitude: None,
            weather_units: s("Celsius"),
            cover_ambient: true,
            ambient_strength: 0.5,
            ambient_spread: 0.75,
            ambient_motion: s("Drift"),
            ambient_scope: s("All widgets"),
            ambient_widgets: WIDGET_NAMES
                .iter()
                .map(|(id, _)| *id)
                .filter(|id| *id != "tools")
                .chain(["closed", "ai"])
                .map(str::to_string)
                .collect(),
            ambient_color_source: s("Album cover"),
            ambient_custom_color: s("#7AA8DC"),
            ambient_audio_response: s("Responsive"),
            ambient_sensitivity: 1.4,
            interaction_motion: s("Subtle"),
            notification_seconds: 12.0,
            notification_animation: s("Lift"),
            quiet_hours: false,
            quiet_start: 22,
            quiet_end: 7,
            notifications_snoozed_until: None,
            extra: Map::new(),
        }
    }
}

fn default_hover_views() -> Vec<WidgetSetting> {
    [
        "media", "timer", "focus", "calendar", "notes", "files", "system",
    ]
    .iter()
    .map(|id| WidgetSetting::new(id, *id != "system"))
    .collect()
}

/// C# `Settings.Defaults()`: every widget except the legacy `tools` group.
pub fn default_widgets() -> Vec<WidgetSetting> {
    WIDGET_NAMES
        .iter()
        .map(|(id, _)| *id)
        .filter(|id| *id != "tools")
        .map(|id| WidgetSetting::new(id, id != "stopwatch" && id != "voice"))
        .collect()
}

impl Settings {
    /// Repairs every value in place; see the `normalize` module for the individual rules.
    pub fn normalize(&mut self) {
        normalize::normalize(self);
    }

    /// C# `ToolEnabled`: a main-panel widget entry wins; otherwise the legacy `tools` group
    /// must be enabled together with the individual tool.
    pub fn tool_enabled(&self, id: &str) -> bool {
        if let Some(w) = self.widgets.iter().find(|w| w.id == id) {
            return w.enabled;
        }
        self.widgets.iter().any(|w| w.id == "tools" && w.enabled)
            && self.tools.iter().any(|t| t.id == id && t.enabled)
    }

    /// Whether a main-panel widget is switched on (`false` when absent).
    pub fn widget_enabled(&self, id: &str) -> bool {
        self.widgets.iter().any(|w| w.id == id && w.enabled)
    }

    pub fn dock_edge(&self) -> DockEdge {
        match self.dock_edge.as_str() {
            "Bottom" => DockEdge::Bottom,
            "Left" => DockEdge::Left,
            "Right" => DockEdge::Right,
            "Floating" => DockEdge::Floating,
            _ => DockEdge::Top,
        }
    }

    pub fn display_mode(&self) -> DisplayMode {
        match self.display_mode.as_str() {
            "Selected" => DisplayMode::Selected,
            "Follow" => DisplayMode::Follow,
            _ => DisplayMode::Primary,
        }
    }

    /// C# `AmbientLook.From`.
    pub fn ambient_look(&self) -> AmbientLook {
        AmbientLook {
            direction: self.ambient_direction.clone(),
            height: self.ambient_height,
            balance: self.ambient_balance,
            softness: self.ambient_softness,
        }
    }

    /// Serializes to the JSON text the app persists (pretty, PascalCase, unknown keys kept).
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string())
    }
}

/// Ambient light shape (C# `AmbientLook`).
#[derive(Debug, Clone, PartialEq)]
pub struct AmbientLook {
    pub direction: String,
    pub height: f64,
    pub balance: f64,
    pub softness: f64,
}

impl Default for AmbientLook {
    fn default() -> Self {
        AmbientLook {
            direction: "Bottom".to_string(),
            height: 1.0,
            balance: 0.5,
            softness: 0.5,
        }
    }
}

/// Copies the named properties from `source` to `target` through their JSON form, so the
/// operation is generic over fields exactly like the C# reflection helpers.
pub(crate) fn copy_keys(source: &Settings, target: &mut Settings, keys: &[&str]) {
    let (Ok(Value::Object(src)), Ok(Value::Object(mut dst))) =
        (serde_json::to_value(source), serde_json::to_value(&*target))
    else {
        return;
    };
    for key in keys {
        if let Some(v) = src.get(*key) {
            dst.insert((*key).to_string(), v.clone());
        }
    }
    if let Ok(next) = serde_json::from_value::<Settings>(Value::Object(dst)) {
        *target = next;
    }
}

#[cfg(test)]
mod tests;
