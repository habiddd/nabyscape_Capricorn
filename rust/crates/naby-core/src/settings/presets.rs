//! Bundled presets (`docs/presets/*.json`) and layout application.
//!
//! * `default-preferences.json` is the factory preference set (C# `FactoryDefaults.Create`,
//!   what a fresh install and an explicit reset use): see [`recommended`].
//! * `midnight-notch.json` and `default-layout.json` are `luma.layout.v1` layout files.

use super::{copy_keys, profile, Settings};
use serde_json::Value;
use std::fmt;

const DEFAULT_PREFERENCES: &str =
    include_str!("../../../../../docs/presets/default-preferences.json");
const MIDNIGHT_NOTCH: &str = include_str!("../../../../../docs/presets/midnight-notch.json");
const DEFAULT_LAYOUT: &str = include_str!("../../../../../docs/presets/default-layout.json");

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileError {
    /// "Layout file is too large."
    TooLarge,
    /// "Choose a NabyScape layout file."
    NotALayout,
    /// "The layout has no customization values."
    NoValues,
    /// Malformed JSON or a value of the wrong type.
    Invalid(String),
}

impl fmt::Display for ProfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProfileError::TooLarge => f.write_str("Layout file is too large."),
            ProfileError::NotALayout => f.write_str("Choose a NabyScape layout file."),
            ProfileError::NoValues => f.write_str("The layout has no customization values."),
            ProfileError::Invalid(why) => write!(f, "The layout is invalid: {why}"),
        }
    }
}

impl std::error::Error for ProfileError {}

/// The bundled factory preferences, normalized (C# `FactoryDefaults.Create`).
pub fn recommended() -> Settings {
    let mut settings: Settings = serde_json::from_str(DEFAULT_PREFERENCES)
        .expect("bundled default-preferences.json is valid");
    settings.normalize();
    settings
}

/// Factory settings with the bundled Midnight Notch layout applied.
pub fn midnight_notch() -> Settings {
    let mut settings = Settings::default();
    let layout: Value = serde_json::from_str(MIDNIGHT_NOTCH).expect("bundled midnight-notch.json");
    apply_layout(&mut settings, &layout).expect("bundled midnight-notch.json applies");
    settings
}

/// Factory settings with the bundled default layout applied.
pub fn default_layout() -> Settings {
    let mut settings = Settings::default();
    let layout: Value = serde_json::from_str(DEFAULT_LAYOUT).expect("bundled default-layout.json");
    apply_layout(&mut settings, &layout).expect("bundled default-layout.json applies");
    settings
}

/// C# `CustomizationProfile.Import`, given the parsed document. Only keys present in
/// `document["values"]` change; the result is normalized as a whole and `target` is only
/// modified when every step succeeds.
pub fn apply_layout(target: &mut Settings, document: &Value) -> Result<(), ProfileError> {
    let root = document.as_object().ok_or(ProfileError::NotALayout)?;
    if root.get("format").and_then(Value::as_str) != Some(profile::FORMAT) {
        return Err(ProfileError::NotALayout);
    }
    let values = root
        .get("values")
        .and_then(Value::as_object)
        .ok_or(ProfileError::NotALayout)?;
    let all = profile::all_keys();
    let keys: Vec<&str> = all
        .iter()
        .copied()
        .filter(|k| values.contains_key(*k))
        .collect();
    if keys.is_empty() {
        return Err(ProfileError::NoValues);
    }
    let incoming: Settings = serde_json::from_value(Value::Object(values.clone()))
        .map_err(|e| ProfileError::Invalid(e.to_string()))?;
    let mut candidate = target.clone();
    copy_keys(&incoming, &mut candidate, &keys);
    if values.contains_key("Monitor") && !values.contains_key("DisplayMode") {
        candidate.display_mode = if incoming.monitor.is_empty() {
            "Primary"
        } else {
            "Selected"
        }
        .to_string();
    }
    candidate.normalize();
    copy_keys(&candidate, target, &all);
    Ok(())
}
