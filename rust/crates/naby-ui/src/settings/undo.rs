//! Undo of the last saved appearance, layout or widget batch (C# `SettingsUndo.cs`).

use naby_core::settings::Settings;
use serde_json::{Map, Value};

/// The preferences Undo covers (`UndoKeys`). Saved content, connections and background
/// permissions are never part of a batch.
pub const UNDO_KEYS: &[&str] = &[
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
    "AlbumAccent",
    "ControlFill",
    "ControlBorder",
    "SelectionFill",
    "PanelWidth",
    "PanelMaxHeight",
    "PanelPadding",
    "PanelScale",
    "SectionGap",
    "HoverWidth",
    "HoverHeight",
    "AnimationMs",
    "ReduceMotion",
    "LiveActivity",
    "ClosedMediaIcon",
    "ClosedMediaContent",
    "Performance",
    "CategoryStyle",
    "LaunchStyle",
    "BoldMediaTitle",
    "BoldHeadings",
    "Widgets",
    "Tools",
    "HoverViews",
    "HoverFollowsWidgets",
];

/// The previous values of the last batch that changed any [`UNDO_KEYS`] preference.
#[derive(Debug, Default, Clone)]
pub struct Undo {
    values: Map<String, Value>,
}

impl Undo {
    /// `CaptureSettingsUndo(before)`: records the keys that differ between `before` and
    /// `after`. A save that changed none (window placement, unrelated switches) keeps the
    /// previous batch.
    pub fn capture(&mut self, before: &Value, after: &Value) {
        let mut changes = Map::new();
        for key in UNDO_KEYS {
            if let (Some(old), Some(next)) = (before.get(*key), after.get(*key)) {
                if old != next {
                    changes.insert((*key).to_owned(), old.clone());
                }
            }
        }
        if !changes.is_empty() {
            self.values = changes;
        }
    }

    pub fn available(&self) -> bool {
        !self.values.is_empty()
    }

    pub fn clear(&mut self) {
        self.values.clear();
    }

    /// Writes the remembered values into `settings`; returns false when there is nothing to undo
    /// or the stored values no longer fit the model (nothing is changed then).
    pub fn apply(&self, settings: &mut Settings) -> bool {
        if self.values.is_empty() {
            return false;
        }
        let Ok(Value::Object(mut current)) = serde_json::to_value(&*settings) else {
            return false;
        };
        for (key, value) in &self.values {
            current.insert(key.clone(), value.clone());
        }
        match serde_json::from_value::<Settings>(Value::Object(current)) {
            Ok(restored) => {
                *settings = restored;
                true
            }
            Err(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json(s: &Settings) -> Value {
        serde_json::to_value(s).unwrap()
    }

    #[test]
    fn restores_only_the_last_saved_batch() {
        let mut settings = Settings::default();
        let first = json(&settings);
        settings.width = 300.0;
        settings.glass_style = !settings.glass_style;
        let second = json(&settings);
        let mut undo = Undo::default();
        undo.capture(&first, &second);
        assert!(undo.available());
        settings.hotkey_key = "Z".into();
        settings.width = 333.0;
        undo.apply(&mut settings);
        assert_eq!(settings.width, 228.0);
        assert_eq!(settings.glass_style, Settings::default().glass_style);
        assert_eq!(settings.hotkey_key, "Z", "unrelated preferences stay");
    }

    #[test]
    fn unrelated_saves_do_not_erase_the_last_batch() {
        let mut undo = Undo::default();
        let mut s = Settings::default();
        let a = json(&s);
        s.radius = 4.0;
        undo.capture(&a, &json(&s));
        let b = json(&s);
        s.clipboard_history = !s.clipboard_history;
        undo.capture(&b, &json(&s));
        assert!(undo.available(), "clipboard history is not an undo key");
        let mut restored = s.clone();
        undo.apply(&mut restored);
        assert_eq!(restored.radius, 16.0);
        assert_eq!(restored.clipboard_history, s.clipboard_history);
    }
}
