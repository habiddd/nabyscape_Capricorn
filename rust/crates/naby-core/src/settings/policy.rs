//! Small settings-derived policies that live next to the C# `Settings` partials:
//! `HoverPagePolicy`, `NotificationPolicy.Quiet`, `HotkeyPolicy.Conflict`, `PremiumPreferences.Keys`.

use super::{is_tool_name, Settings, HOVER_PAGE_IDS};
use chrono::{DateTime, Local, Timelike, Utc};

/// C# `PremiumPreferences.Keys`.
pub const PREMIUM_KEYS: &[&str] = &[
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
    "NotificationSeconds",
    "NotificationAnimation",
    "QuietHours",
    "QuietStart",
    "QuietEnd",
    "NotificationsSnoozedUntil",
];

/// C# `HoverPagePolicy.IsTool`.
pub fn hover_is_tool(id: &str) -> bool {
    is_tool_name(id)
}

/// C# `HoverPagePolicy.Available`.
pub fn hover_available(s: &Settings, id: &str) -> bool {
    if s.hover_follows_widgets {
        s.widgets.iter().any(|w| w.id == id && w.enabled)
    } else {
        !hover_is_tool(id) || s.tool_enabled(id) || s.tools.iter().any(|t| t.id == id && t.enabled)
    }
}

/// C# `HoverPagePolicy.Pages`: ordered ids of the hover pages currently shown.
pub fn hover_pages(s: &Settings) -> Vec<String> {
    if s.hover_follows_widgets {
        s.widgets
            .iter()
            .filter(|w| w.enabled && HOVER_PAGE_IDS.contains(&w.id.as_str()))
            .map(|w| w.id.clone())
            .collect()
    } else {
        s.hover_views
            .iter()
            .filter(|v| v.enabled && hover_available(s, &v.id))
            .map(|v| v.id.clone())
            .collect()
    }
}

impl Settings {
    /// C# `NotificationPolicy.Quiet` using an explicit local hour (0-23) for testability.
    pub fn quiet_at(&self, now: DateTime<Utc>, local_hour: u32) -> bool {
        if let Some(until) = self.notifications_snoozed_until {
            if until.with_timezone(&Utc) > now {
                return true;
            }
        }
        if !self.quiet_hours || self.quiet_start == self.quiet_end {
            return false;
        }
        let hour = local_hour as i32;
        if self.quiet_start < self.quiet_end {
            hour >= self.quiet_start && hour < self.quiet_end
        } else {
            hour >= self.quiet_start || hour < self.quiet_end
        }
    }

    /// C# `NotificationPolicy.Quiet` with the machine's local time zone.
    pub fn quiet(&self, now: DateTime<Utc>) -> bool {
        self.quiet_at(now, now.with_timezone(&Local).hour())
    }

    /// C# `HotkeyPolicy.Conflict`; `action` is `"Panel"`, `"Visibility"` or `"Tuck"`.
    /// Returns an empty string when there is no conflict.
    pub fn hotkey_conflict(&self, action: &str) -> String {
        let all = [
            (
                "Panel",
                self.hotkey_enabled,
                &self.hotkey_modifiers,
                &self.hotkey_key,
            ),
            (
                "Visibility",
                self.visibility_hotkey_enabled,
                &self.visibility_hotkey_modifiers,
                &self.visibility_hotkey_key,
            ),
            (
                "Tuck",
                self.tuck_hotkey_enabled,
                &self.tuck_hotkey_modifiers,
                &self.tuck_hotkey_key,
            ),
        ];
        let Some(own) = all.iter().find(|v| v.0 == action) else {
            return String::new();
        };
        match all
            .iter()
            .find(|v| v.0 != action && own.1 && v.1 && v.2 == own.2 && v.3 == own.3)
        {
            Some(other) => format!(
                "{action} shortcut conflicts with {}. Choose a different key or modifiers.",
                other.0
            ),
            None => String::new(),
        }
    }
}
