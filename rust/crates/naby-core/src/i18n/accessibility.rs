//! Ported from `UI/Shared/Accessibility.cs`: the platform-free scale and contrast rules.
//! Tree walking, automation peers and brush replacement belong to the renderer.

use crate::settings::Settings;

pub const MIN_TEXT_SCALE: f64 = 1.0;
pub const MAX_TEXT_SCALE: f64 = 1.5;

/// Accessibility preferences applied at startup (C# `Accessibility.Configure`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Accessibility {
    pub text_scale: f64,
    pub force_high_contrast: bool,
}

impl Default for Accessibility {
    fn default() -> Self {
        Self {
            text_scale: 1.0,
            force_high_contrast: false,
        }
    }
}

impl Accessibility {
    pub fn from_settings(s: &Settings) -> Self {
        let scale = s.accessibility_text_scale;
        Self {
            text_scale: if scale.is_finite() {
                scale.clamp(MIN_TEXT_SCALE, MAX_TEXT_SCALE)
            } else {
                1.0
            },
            force_high_contrast: s.accessibility_high_contrast,
        }
    }

    /// Forced by the preference or reported by the system.
    pub fn high_contrast(&self, system_high_contrast: bool) -> bool {
        self.force_high_contrast || system_high_contrast
    }

    /// C# `Accessibility.SetFontSize`: explicit sizes are written in base units.
    pub fn font_size(&self, base: f64) -> f64 {
        scaled_font_size(base, self.text_scale)
    }

    /// Percentage label, e.g. `"150%"`.
    pub fn scale_percent_text(&self) -> String {
        format!("{}%", (self.text_scale * 100.0).round() as i64)
    }
}

/// `base * scale` with invalid input falling back to the unscaled size.
pub fn scaled_font_size(base: f64, scale: f64) -> f64 {
    if !base.is_finite() {
        0.0
    } else if scale.is_finite() && scale > 0.0 {
        base * scale
    } else {
        base
    }
}
