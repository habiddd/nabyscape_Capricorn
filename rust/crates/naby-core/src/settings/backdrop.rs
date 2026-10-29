//! Settings window backdrop selection (C# `SettingsBackdrop`, pure parts only). The DWM call
//! itself is injected so the host owns the unsafe code.

use super::Settings;

/// Choices persisted in `SettingsMaterial` (the empty string means "Solid").
pub const SETTINGS_BACKDROP_CHOICES: &[&str] =
    &["Solid", "Simulated glass", "Native Mica", "Native Acrylic"];

/// Machine state that gates the native materials.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackdropConditions {
    pub supported: bool,
    pub high_contrast: bool,
    pub transparency: bool,
    pub energy_saver: bool,
    pub remote: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackdropResult {
    pub requested: String,
    pub actual: String,
    pub reason: String,
    pub hresult: i32,
}

impl BackdropResult {
    pub fn is_native(&self) -> bool {
        self.actual == "Native Mica" || self.actual == "Native Acrylic"
    }

    pub fn status(&self) -> String {
        format!(
            "Requested: {}. Actual: {}. {}",
            self.requested, self.actual, self.reason
        )
    }
}

impl Settings {
    /// C# `SettingsBackdrop.Requested`.
    pub fn requested_backdrop(&self) -> &str {
        if SETTINGS_BACKDROP_CHOICES.contains(&self.settings_material.as_str()) {
            &self.settings_material
        } else {
            "Solid"
        }
    }
}

/// C# `SettingsBackdrop.Resolve`. `apply(type)` returns an HRESULT (negative = failure);
/// the type is 2 for Mica and 3 for Acrylic (`DWMSBT_*`).
pub fn resolve_backdrop(
    request: &str,
    conditions: BackdropConditions,
    mut apply: impl FnMut(i32) -> i32,
) -> BackdropResult {
    let request = if SETTINGS_BACKDROP_CHOICES.contains(&request) {
        request
    } else {
        "Solid"
    };
    let result = |actual: &str, reason: &str, hresult: i32| BackdropResult {
        requested: request.to_string(),
        actual: actual.to_string(),
        reason: reason.to_string(),
        hresult,
    };
    if request == "Solid" || request == "Simulated glass" {
        return result(request, "Settings only; island material is independent.", 0);
    }
    let reason = if !conditions.supported {
        "Requires Windows 11 build 22621 or later."
    } else if conditions.high_contrast {
        "High contrast is enabled."
    } else if !conditions.transparency {
        "Windows transparency effects are off."
    } else if conditions.energy_saver {
        "Energy saver is active."
    } else if conditions.remote {
        "Remote sessions use the fallback."
    } else {
        ""
    };
    if !reason.is_empty() {
        return result("Solid", reason, 0);
    }
    let hr = apply(if request == "Native Mica" { 2 } else { 3 });
    if hr < 0 {
        result(
            "Solid",
            &format!("DWM could not enable the material (0x{:08X}).", hr as u32),
            hr,
        )
    } else {
        result(
            request,
            "DWM enabled; Windows controls the final appearance.",
            hr,
        )
    }
}
