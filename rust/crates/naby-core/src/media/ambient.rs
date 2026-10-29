//! Cover-ambient policy and the measured audio envelope (`AmbientAudio.cs`).

use super::palette::CoverPalette;
use crate::color::Rgba;

/// Fallback of an unparsable custom color (the C# normalize default `#7AA8DC`).
const FALLBACK_COLOR: Rgba = Rgba::rgb(0x7A, 0xA8, 0xDC);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AmbientScope {
    #[default]
    AllWidgets,
    MusicOnly,
    SelectedWidgets,
}

impl AmbientScope {
    /// Unknown text normalizes to "All widgets", like `Settings.Normalize`.
    pub fn parse(text: &str) -> Self {
        match text {
            "Music only" => AmbientScope::MusicOnly,
            "Selected widgets" => AmbientScope::SelectedWidgets,
            _ => AmbientScope::AllWidgets,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AmbientColorSource {
    #[default]
    AlbumCover,
    AccentColor,
    CustomColor,
    CustomBlend,
}

impl AmbientColorSource {
    pub fn parse(text: &str) -> Self {
        match text {
            "Accent color" => AmbientColorSource::AccentColor,
            "Custom color" => AmbientColorSource::CustomColor,
            "Custom blend" => AmbientColorSource::CustomBlend,
            _ => AmbientColorSource::AlbumCover,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AmbientResponse {
    Off,
    Soft,
    #[default]
    Responsive,
    Punchy,
}

impl AmbientResponse {
    pub fn parse(text: &str) -> Self {
        match text {
            "Off" => AmbientResponse::Off,
            "Soft" => AmbientResponse::Soft,
            "Punchy" => AmbientResponse::Punchy,
            _ => AmbientResponse::Responsive,
        }
    }
}

/// The ambient-related preferences, supplied by the caller from `Settings`.
#[derive(Debug, Clone, PartialEq)]
pub struct AmbientConfig {
    pub cover_ambient: bool,
    pub closed_ambient: bool,
    pub strength: f64,
    pub scope: AmbientScope,
    pub widgets: Vec<String>,
    pub color_source: AmbientColorSource,
    /// `AmbientPalette == "Single color"`.
    pub single_color: bool,
    pub custom_color: String,
    pub secondary_color: String,
    pub accent: String,
}

impl Default for AmbientConfig {
    fn default() -> Self {
        AmbientConfig {
            cover_ambient: true,
            closed_ambient: true,
            strength: 0.5,
            scope: AmbientScope::AllWidgets,
            widgets: Vec::new(),
            color_source: AmbientColorSource::AlbumCover,
            single_color: false,
            custom_color: "#7AA8DC".to_owned(),
            secondary_color: "#7AA8DC".to_owned(),
            accent: "#7AA8DC".to_owned(),
        }
    }
}

pub type AmbientColors = CoverPalette;

impl AmbientConfig {
    /// `AmbientPolicy.Includes`: does `widget` receive ambient light?
    pub fn includes(&self, widget: &str, source_disabled: bool) -> bool {
        if source_disabled || !self.cover_ambient || self.strength <= 0.0 {
            return false;
        }
        if widget == "closed" {
            return self.closed_ambient;
        }
        match self.scope {
            AmbientScope::MusicOnly => matches!(widget, "media" | "volume" | "closed"),
            AmbientScope::SelectedWidgets => self.widgets.iter().any(|w| w == widget),
            AmbientScope::AllWidgets => true,
        }
    }

    /// `AmbientPolicy.ColorsFor`: falls back to the accent when there is no artwork.
    pub fn colors_for(&self, cover: CoverPalette) -> AmbientColors {
        let parse = |s: &str| Rgba::parse_hex(s).unwrap_or(FALLBACK_COLOR);
        if self.color_source == AmbientColorSource::AlbumCover && cover.has_color() {
            return if self.single_color {
                CoverPalette {
                    primary: cover.primary,
                    secondary: cover.primary,
                }
            } else {
                cover
            };
        }
        if self.color_source == AmbientColorSource::CustomBlend {
            return CoverPalette {
                primary: parse(&self.custom_color),
                secondary: parse(&self.secondary_color),
            };
        }
        let color = parse(if self.color_source == AmbientColorSource::CustomColor {
            &self.custom_color
        } else {
            &self.accent
        });
        CoverPalette {
            primary: color,
            secondary: color,
        }
    }
}

/// Whether the shared 30 Hz ambient sampler should run (`AmbientAudioDriver.Reconcile`).
pub fn sampler_wanted(
    cover_ambient: bool,
    live_activity: bool,
    motion_enabled: bool,
    response: AmbientResponse,
    playing: bool,
    any_surface_active: bool,
) -> bool {
    cover_ambient
        && live_activity
        && motion_enabled
        && response != AmbientResponse::Off
        && playing
        && any_surface_active
}

/// Attack/release filtering of the measured media-session peak, never a simulated beat.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct AmbientEnvelope {
    level: f64,
}

impl AmbientEnvelope {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn level(&self) -> f64 {
        self.level
    }

    /// Advances the envelope by `seconds` and returns the ambient gain `0.32..=1.0`.
    pub fn sample(
        &mut self,
        peak: f64,
        seconds: f64,
        response: AmbientResponse,
        sensitivity: f64,
    ) -> f64 {
        let peak = if peak.is_finite() {
            peak.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let sensitivity = if sensitivity.is_finite() {
            sensitivity.clamp(0.5, 3.0)
        } else {
            1.4
        };
        let seconds = if seconds.is_finite() {
            seconds.clamp(0.001, 0.25)
        } else {
            1.0 / 30.0
        };
        let punchy = response == AmbientResponse::Punchy;
        let soft = response == AmbientResponse::Soft;
        let target = if peak < 0.002 {
            0.0
        } else {
            (peak * sensitivity)
                .clamp(0.0, 1.0)
                .powf(if punchy { 0.8 } else { 0.6 })
        };
        let attack = if soft {
            0.1
        } else if punchy {
            0.018
        } else {
            0.035
        };
        let release = if soft {
            0.45
        } else if punchy {
            0.13
        } else {
            0.25
        };
        self.level = target
            + (self.level - target)
                * (-seconds / if target > self.level { attack } else { release }).exp();
        if self.level < 0.001 {
            self.level = 0.0;
        }
        0.32 + 0.68 * self.level
    }

    pub fn reset(&mut self) {
        self.level = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f64 = 1.0 / 30.0;

    #[test]
    fn ambient_scopes_include_only_chosen_content_and_respect_master_switch() {
        let mut s = AmbientConfig::default();
        assert!(s.includes("notes", false) && s.includes("focus", false));
        s.scope = AmbientScope::MusicOnly;
        assert!(s.includes("media", false) && !s.includes("tasks", false));
        s.scope = AmbientScope::SelectedWidgets;
        s.widgets = vec!["notes".into()];
        assert!(s.includes("notes", false) && !s.includes("media", false));
        s.cover_ambient = false;
        assert!(!s.includes("notes", false));
        s.cover_ambient = true;
        s.strength = 0.0;
        assert!(!s.includes("notes", false));
    }

    #[test]
    fn media_none_suppresses_every_scope_without_changing_preferences() {
        let mut s = AmbientConfig {
            strength: 0.8,
            widgets: ["media", "focus", "notes", "closed"]
                .map(String::from)
                .to_vec(),
            ..Default::default()
        };
        for scope in [
            AmbientScope::AllWidgets,
            AmbientScope::MusicOnly,
            AmbientScope::SelectedWidgets,
        ] {
            s.scope = scope;
            for w in ["media", "focus", "notes", "closed", "ai"] {
                assert!(!s.includes(w, true), "{scope:?} {w}");
            }
        }
        s.scope = AmbientScope::AllWidgets;
        assert!(
            s.includes("focus", false) && s.cover_ambient && s.closed_ambient && s.strength == 0.8
        );
        s.closed_ambient = false;
        assert!(!s.includes("closed", false) && s.includes("media", false));
    }

    #[test]
    fn color_source_falls_back_without_artwork_and_custom_stays_independent() {
        let mut s = AmbientConfig {
            accent: "#123456".into(),
            custom_color: "#654321".into(),
            ..Default::default()
        };
        let cover = CoverPalette {
            primary: Rgba::rgb(255, 0, 0),
            secondary: Rgba::rgb(0, 0, 255),
        };
        assert_eq!(s.colors_for(cover), cover);
        assert_eq!(
            s.colors_for(CoverPalette::NONE).primary,
            Rgba::rgb(0x12, 0x34, 0x56)
        );
        s.single_color = true;
        assert_eq!(s.colors_for(cover).secondary, cover.primary);
        s.color_source = AmbientColorSource::AccentColor;
        assert_eq!(s.colors_for(cover).primary, Rgba::rgb(0x12, 0x34, 0x56));
        s.color_source = AmbientColorSource::CustomColor;
        assert_eq!(s.colors_for(cover).primary, Rgba::rgb(0x65, 0x43, 0x21));
        s.color_source = AmbientColorSource::CustomBlend;
        s.secondary_color = "#GGGGGG".into();
        let blend = s.colors_for(cover);
        assert_eq!(blend.primary, Rgba::rgb(0x65, 0x43, 0x21));
        assert_eq!(blend.secondary, FALLBACK_COLOR);
    }

    #[test]
    fn measured_ambient_attacks_then_decays_to_silence() {
        let mut e = AmbientEnvelope::new();
        assert_eq!(e.sample(0.0, DT, AmbientResponse::Responsive, 1.0), 0.32);
        let attack = e.sample(0.8, DT, AmbientResponse::Responsive, 1.0);
        assert!(attack > 0.65 && attack <= 1.0);
        let release = e.sample(0.0, DT, AmbientResponse::Responsive, 1.0);
        assert!(release < attack && release > 0.32);
        for _ in 0..90 {
            e.sample(0.0, DT, AmbientResponse::Responsive, 1.0);
        }
        assert_eq!(e.level(), 0.0);
    }

    #[test]
    fn response_profiles_and_sensitivity_alter_transients_within_bounds() {
        let mut soft = AmbientEnvelope::new();
        let mut punch = AmbientEnvelope::new();
        assert!(
            punch.sample(0.9, DT, AmbientResponse::Punchy, 1.0)
                > soft.sample(0.9, DT, AmbientResponse::Soft, 1.0)
        );
        let mut high = AmbientEnvelope::new();
        let mut low = AmbientEnvelope::new();
        assert!(
            high.sample(0.1, DT, AmbientResponse::Responsive, 3.0)
                > low.sample(0.1, DT, AmbientResponse::Responsive, 0.5)
        );
        assert!(high
            .sample(f64::NAN, f64::NAN, AmbientResponse::Responsive, f64::NAN)
            .is_finite());
        high.reset();
        assert_eq!(high.level(), 0.0);
        let mut e = AmbientEnvelope::new();
        for _ in 0..50 {
            let g = e.sample(f64::INFINITY, 99.0, AmbientResponse::Punchy, 1e9);
            assert!((0.32..=1.0).contains(&g));
        }
    }

    #[test]
    fn parsing_normalizes_unknown_text() {
        assert_eq!(AmbientScope::parse("other"), AmbientScope::AllWidgets);
        assert_eq!(AmbientScope::parse("Music only"), AmbientScope::MusicOnly);
        assert_eq!(
            AmbientColorSource::parse("other"),
            AmbientColorSource::AlbumCover
        );
        assert_eq!(AmbientResponse::parse("other"), AmbientResponse::Responsive);
        assert_eq!(AmbientResponse::parse("Off"), AmbientResponse::Off);
    }

    #[test]
    fn sampler_runs_only_when_every_condition_holds() {
        let r = AmbientResponse::Responsive;
        assert!(sampler_wanted(true, true, true, r, true, true));
        assert!(!sampler_wanted(
            true,
            true,
            true,
            AmbientResponse::Off,
            true,
            true
        ));
        assert!(!sampler_wanted(true, true, true, r, false, true));
        assert!(!sampler_wanted(true, true, true, r, true, false));
        assert!(!sampler_wanted(true, true, false, r, true, true));
    }
}
