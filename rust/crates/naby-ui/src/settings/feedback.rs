//! The "Saved · ..." line under the page (C# `SettingsChangeFeedback.cs`): it names the
//! preferences that really changed, never file paths, locations, shortcuts or other free-form
//! content.

use naby_core::i18n;
use naby_core::settings::Settings;
use serde::de::{Deserializer, MapAccess, Visitor};
use serde_json::Value;
use std::fmt;

const LABELS: &[(&str, &str)] = &[
    ("CoverAmbient", "Ambient light"),
    ("ClosedAmbient", "Closed dock ambience"),
    ("AmbientStrength", "Ambient intensity"),
    ("AutoCheckUpdates", "Automatic update checks"),
    ("AutoDownloadUpdates", "Automatic update downloads"),
    ("Width", "Closed dock width"),
    ("Height", "Closed dock height"),
    ("TuckEdge", "Tucked edge height"),
    ("Widgets", "Widgets & order"),
    ("HoverViews", "Hover pages"),
    ("Tools", "Tools & order"),
    ("Shortcuts", "Shortcuts"),
    ("Performance", "Animation performance"),
    ("AutoStart", "Start with Windows"),
];

/// The JSON object of `settings` with its keys in declaration order (C# enumerates properties
/// in that order; `serde_json::Map` would sort them).
pub fn ordered_entries(settings: &Settings) -> Vec<(String, Value)> {
    struct Entries(Vec<(String, Value)>);
    impl<'de> Visitor<'de> for Entries {
        type Value = Vec<(String, Value)>;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("a settings object")
        }
        fn visit_map<A: MapAccess<'de>>(mut self, mut map: A) -> Result<Self::Value, A::Error> {
            while let Some((k, v)) = map.next_entry::<String, Value>()? {
                self.0.push((k, v));
            }
            Ok(self.0)
        }
    }
    let Ok(text) = serde_json::to_string(settings) else {
        return Vec::new();
    };
    let mut de = serde_json::Deserializer::from_str(&text);
    de.deserialize_map(Entries(Vec::new())).unwrap_or_default()
}

/// `Regex.Replace(name, "(?<=[a-z0-9])([A-Z])", " $1")`.
fn spaced(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    let mut prev: Option<char> = None;
    for c in name.chars() {
        if c.is_ascii_uppercase()
            && prev.is_some_and(|p| p.is_ascii_lowercase() || p.is_ascii_digit())
        {
            out.push(' ');
        }
        out.push(c);
        prev = Some(c);
    }
    out
}

/// A JSON number the way .NET prints it: integral doubles without a fraction.
fn number_text(n: &serde_json::Number) -> String {
    match n.as_f64() {
        Some(f) if f.is_finite() && f.fract() == 0.0 && f.abs() < 1e15 => format!("{}", f as i64),
        Some(f) => format!("{f}"),
        None => n.to_string(),
    }
}

/// `SettingsChangeFeedback.Describe(before, settings)`.
pub fn describe(before: &Value, settings: &Settings, lang: &str) -> String {
    let mut changes: Vec<String> = Vec::new();
    for (name, value) in ordered_entries(settings) {
        if before.get(&name) == Some(&value) {
            continue;
        }
        let label = LABELS
            .iter()
            .find(|(k, _)| *k == name)
            .map_or_else(|| spaced(&name), |(_, l)| (*l).to_owned());
        let shown = match &value {
            Value::Bool(true) => i18n::t(lang, "On").to_owned(),
            Value::Bool(false) => i18n::t(lang, "Off").to_owned(),
            Value::Number(n) if name == "AmbientStrength" => {
                format!("{:.0}%", n.as_f64().unwrap_or(0.0) * 100.0)
            }
            Value::Number(n) => number_text(n),
            _ => String::new(),
        };
        let label = i18n::t(lang, &label).to_owned();
        changes.push(if shown.is_empty() {
            format!("{label} updated")
        } else {
            format!("{label}: {shown}")
        });
    }
    if changes.is_empty() {
        return "Saved on this device.".to_owned();
    }
    let mut text = format!(
        "Saved · {}",
        changes
            .iter()
            .take(3)
            .cloned()
            .collect::<Vec<_>>()
            .join(" · ")
    );
    if changes.len() > 3 {
        text.push_str(&format!(" · +{} more", changes.len() - 3));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_changed_preferences_without_exposing_personal_content() {
        let mut settings = Settings::default();
        let before = serde_json::to_value(&settings).unwrap();
        settings.cover_ambient = !settings.cover_ambient;
        settings.width = 444.0;
        settings.downloads_folder = "C:/private/example".into();
        let message = describe(&before, &settings, "en");
        assert!(message.starts_with("Saved · "));
        assert!(message.contains("Ambient light: "));
        assert!(message.contains("Closed dock width: 444"));
        assert!(!message.contains("private"));
    }

    #[test]
    fn unchanged_settings_say_so_and_long_lists_are_summarized() {
        let mut settings = Settings::default();
        let before = serde_json::to_value(&settings).unwrap();
        assert_eq!(describe(&before, &settings, "en"), "Saved on this device.");
        settings.width = 300.0;
        settings.height = 40.0;
        settings.radius = 3.0;
        settings.opacity = 0.7;
        settings.bold_headings = !settings.bold_headings;
        let message = describe(&before, &settings, "en");
        assert!(message.ends_with("· +2 more"), "{message}");
        assert_eq!(message.matches(" · ").count(), 4);
    }

    #[test]
    fn labels_follow_declaration_order_and_spell_out_names() {
        assert_eq!(spaced("HotkeyEnabled"), "Hotkey Enabled");
        assert_eq!(spaced("Clock24Hour"), "Clock24 Hour");
        let entries = ordered_entries(&Settings::default());
        assert_eq!(entries[0].0, "NotificationDelivery");
    }

    #[test]
    fn ambient_intensity_is_a_percentage() {
        let mut settings = Settings::default();
        let before = serde_json::to_value(&settings).unwrap();
        settings.ambient_strength = 0.35;
        assert!(describe(&before, &settings, "en").contains("Ambient intensity: 35%"));
    }
}
