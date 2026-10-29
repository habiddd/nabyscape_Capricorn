//! Colors, radii and font sizes from `Theme.xaml` / `AssistantTheme.xaml` and `UI.cs`.
//!
//! [`Theme::from_settings`] maps the user's preferences onto a theme the way `UI.ApplyControlTheme`
//! does; [`Theme::default`] and [`Theme::new`] build one without settings.

use naby_core::color::Rgba;
use naby_core::i18n::Accessibility;
use naby_core::settings::Settings;

/// Font sizes used by the shared widgets (WPF device-independent pixels).
pub const FONT_BODY: f32 = 13.0;
pub const FONT_SMALL: f32 = 12.0;
pub const FONT_CAPTION: f32 = 11.0;
pub const FONT_INPUT: f32 = 13.0;

/// Corner radii from the XAML templates.
pub const RADIUS_CONTROL: f32 = 8.0;
pub const RADIUS_CARD: f32 = 14.0;
pub const RADIUS_MENU: f32 = 10.0;
pub const RADIUS_ITEM: f32 = 6.0;
pub const RADIUS_SWATCH: f32 = 7.0;

#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    pub background: Rgba,
    pub accent: Rgba,
    pub accent_text: Rgba,
    pub control_fill: Rgba,
    pub control_border: Rgba,
    pub control_text: Rgba,
    pub selection_fill: Rgba,
    pub selection_text: Rgba,
    /// Primary text color (`#F2F3F5`).
    pub text: Rgba,
    /// Secondary text color (`UI.Muted`, `#A6ABB4`).
    pub text_dim: Rgba,
    pub checked_fill: Rgba,
    pub checked_text: Rgba,
    pub switch_knob: Rgba,
    pub track_fill: Rgba,
    pub progress_fill: Rgba,
    pub progress_track: Rgba,
    pub separator: Rgba,
    pub input_hover_border: Rgba,
    pub input_focus_border: Rgba,
    pub underline_border: Rgba,
    pub invalid: Rgba,
    /// Overlay painted over a hovered control (`#12FFFFFF`).
    pub hover_fill: Rgba,
    /// Default corner radius of controls.
    pub radius: f32,
    /// Accessibility text scale (1.0 = 100%).
    pub text_scale: f32,
    /// Headings use the semibold face.
    pub bold_headings: bool,
    pub high_contrast: bool,
    /// DPI scale of the surface (1.0 = 96 dpi). `Ui` lays out and paints in logical pixels and
    /// multiplies spacing, sizes and fonts by this when drawing; the host passes pointer input
    /// and the area in logical pixels too.
    pub dpi_scale: f32,
}

impl Default for Theme {
    fn default() -> Self {
        Theme::new(
            Rgba::rgb(0x00, 0x00, 0x00),
            Rgba::rgb(0xFF, 0xFF, 0xFF),
            Rgba::rgb(0x16, 0x17, 0x1B),
            Rgba::rgb(0x30, 0x32, 0x38),
            Rgba::rgb(0x34, 0x37, 0x3E),
        )
    }
}

impl Theme {
    /// Builds a theme from the five user-configurable colors; the rest derive as in
    /// `UI.ApplyControlTheme` (contrast text) and `Theme.xaml` (fixed neutrals).
    pub fn new(
        background: Rgba,
        accent: Rgba,
        control_fill: Rgba,
        control_border: Rgba,
        selection_fill: Rgba,
    ) -> Theme {
        Theme {
            background,
            accent,
            accent_text: Rgba::rgb(0x11, 0x13, 0x17),
            control_fill,
            control_border,
            control_text: Theme::contrast_text(control_fill),
            selection_fill,
            selection_text: Theme::contrast_text(selection_fill),
            text: Rgba::rgb(0xF2, 0xF3, 0xF5),
            text_dim: Rgba::rgb(0xA6, 0xAB, 0xB4),
            checked_fill: Rgba::rgb(0xEC, 0xED, 0xEF),
            checked_text: control_fill,
            switch_knob: Rgba::rgb(0xE6, 0xE7, 0xEB),
            track_fill: Rgba::rgb(0x48, 0x48, 0x4A),
            progress_fill: Rgba::rgb(0xFF, 0xFF, 0xFF),
            progress_track: Rgba::rgb(0x3A, 0x3A, 0x3C),
            separator: Rgba::rgb(0x35, 0x38, 0x3F),
            input_hover_border: Rgba::rgb(0x51, 0x56, 0x60),
            input_focus_border: Rgba::rgb(0x85, 0x8C, 0x98),
            underline_border: Rgba::rgb(0x3C, 0x40, 0x48),
            invalid: Rgba::rgb(0xF0, 0x8C, 0x85),
            hover_fill: Rgba::WHITE.with_alpha(0x12),
            radius: RADIUS_CONTROL,
            text_scale: 1.0,
            bold_headings: false,
            high_contrast: false,
            dpi_scale: 1.0,
        }
    }

    /// The same theme for a surface at `scale` (1.0 = 96 dpi, 1.5 = 144 dpi).
    pub fn with_dpi(mut self, scale: f32) -> Theme {
        self.dpi_scale = if scale.is_finite() {
            scale.clamp(0.5, 8.0)
        } else {
            1.0
        };
        self
    }

    /// The theme for `settings` (C# `UI.ApplyControlTheme` plus the accessibility preferences):
    /// Background, Accent, ControlFill, ControlBorder and SelectionFill colors (an invalid value
    /// keeps the built-in default), contrast text derived from the fills, the accessibility text
    /// scale (1.0 to 1.5) and `BoldHeadings`. `high_contrast` is set when the preference forces it;
    /// the host then replaces the palette with the system colors via [`Theme::high_contrast`]
    /// (it also does so when Windows itself reports high contrast). `Settings.Radius` is the
    /// dock's own corner radius, so control radii stay at the fixed XAML values.
    pub fn from_settings(settings: &Settings) -> Theme {
        let base = Theme::default();
        let pick = |value: &str, fallback: Rgba| Rgba::parse_hex(value).unwrap_or(fallback);
        let mut theme = Theme::new(
            pick(&settings.background, base.background),
            pick(&settings.accent, base.accent),
            pick(&settings.control_fill, base.control_fill),
            pick(&settings.control_border, base.control_border),
            pick(&settings.selection_fill, base.selection_fill),
        );
        theme.apply_preferences(settings);
        theme
    }

    /// [`Theme::high_contrast`] with the text scale and heading weight of `settings`.
    pub fn high_contrast_for(
        settings: &Settings,
        window: Rgba,
        window_text: Rgba,
        highlight: Rgba,
        highlight_text: Rgba,
    ) -> Theme {
        let mut theme = Theme::high_contrast(window, window_text, highlight, highlight_text);
        theme.apply_preferences(settings);
        theme.high_contrast = true;
        theme
    }

    fn apply_preferences(&mut self, settings: &Settings) {
        let accessibility = Accessibility::from_settings(settings);
        self.text_scale = accessibility.text_scale as f32;
        self.high_contrast = accessibility.force_high_contrast;
        self.bold_headings = settings.bold_headings;
    }

    /// System-color theme for Windows high-contrast mode (`window`, `window_text`,
    /// `highlight`, `highlight_text` come from `GetSysColor`).
    pub fn high_contrast(
        window: Rgba,
        window_text: Rgba,
        highlight: Rgba,
        highlight_text: Rgba,
    ) -> Theme {
        let mut t = Theme::new(window, highlight, window, window_text, highlight);
        t.control_text = window_text;
        t.selection_text = highlight_text;
        t.text = window_text;
        t.text_dim = window_text;
        t.accent_text = highlight_text;
        t.checked_fill = highlight;
        t.checked_text = highlight_text;
        t.switch_knob = window_text;
        t.track_fill = window_text;
        t.progress_fill = highlight;
        t.progress_track = window;
        t.separator = window_text;
        t.input_hover_border = highlight;
        t.input_focus_border = highlight;
        t.underline_border = window_text;
        t.invalid = window_text;
        t.hover_fill = Rgba::TRANSPARENT;
        t.high_contrast = true;
        t
    }

    /// `UI.ContrastText`: near-black on light fills, white on dark ones.
    pub fn contrast_text(fill: Rgba) -> Rgba {
        let luma = fill.r as f32 * 0.2126 + fill.g as f32 * 0.7152 + fill.b as f32 * 0.0722;
        if luma > 150.0 {
            Rgba::rgb(0x11, 0x13, 0x17)
        } else {
            Rgba::WHITE
        }
    }

    /// Size in pixels after applying the accessibility text scale.
    pub fn scaled(&self, size: f32) -> f32 {
        size * self.text_scale
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use naby_core::settings::presets;

    #[test]
    fn defaults_match_theme_xaml() {
        let t = Theme::default();
        assert_eq!(t.control_fill.to_hex(), "#16171B");
        assert_eq!(t.control_border.to_hex(), "#303238");
        assert_eq!(t.selection_fill.to_hex(), "#34373E");
        assert_eq!(t.text_dim.to_hex(), "#A6ABB4");
        assert_eq!(t.control_text, Rgba::WHITE);
        assert_eq!(t.radius, 8.0);
    }

    #[test]
    fn recommended_preset_maps_colors_and_preferences() {
        let settings = presets::recommended();
        let t = Theme::from_settings(&settings);
        assert_eq!(t.background, Rgba::parse_hex(&settings.background).unwrap());
        assert_eq!(t.accent, Rgba::parse_hex(&settings.accent).unwrap());
        assert_eq!(
            t.control_fill,
            Rgba::parse_hex(&settings.control_fill).unwrap()
        );
        assert_eq!(
            t.control_border,
            Rgba::parse_hex(&settings.control_border).unwrap()
        );
        assert_eq!(
            t.selection_fill,
            Rgba::parse_hex(&settings.selection_fill).unwrap()
        );
        assert_eq!(t.control_text, Theme::contrast_text(t.control_fill));
        assert_eq!(t.checked_text, t.control_fill);
        assert_eq!(t.text_scale, 1.0);
        assert!(!t.high_contrast);
        assert_eq!(t.bold_headings, settings.bold_headings);
        assert_eq!(t.radius, RADIUS_CONTROL);
    }

    #[test]
    fn custom_colors_scale_and_contrast_follow_settings() {
        let settings = Settings {
            control_fill: "#F5F5F5".into(),
            selection_fill: "#102030".into(),
            accent: "#FF8800".into(),
            accessibility_text_scale: 1.25,
            accessibility_high_contrast: true,
            bold_headings: true,
            ..Settings::default()
        };
        let t = Theme::from_settings(&settings);
        assert_eq!(t.accent.to_hex(), "#FF8800");
        assert_eq!(
            t.control_text.to_hex(),
            "#111317",
            "dark text on a light fill"
        );
        assert_eq!(t.selection_text, Rgba::WHITE);
        assert_eq!(t.scaled(12.0), 15.0);
        assert!(t.high_contrast && t.bold_headings);
        let hc = Theme::high_contrast_for(
            &settings,
            Rgba::BLACK,
            Rgba::WHITE,
            Rgba::rgb(0, 255, 255),
            Rgba::BLACK,
        );
        assert!(hc.high_contrast && hc.bold_headings);
        assert_eq!(hc.text_scale, 1.25);
        assert_eq!(hc.control_border, Rgba::WHITE);
    }

    #[test]
    fn invalid_colors_and_scales_fall_back() {
        let settings = Settings {
            control_fill: "nope".into(),
            accessibility_text_scale: f64::NAN,
            ..Settings::default()
        };
        let t = Theme::from_settings(&settings);
        assert_eq!(t.control_fill, Theme::default().control_fill);
        assert_eq!(t.text_scale, 1.0);
        let big = Theme::from_settings(&Settings {
            accessibility_text_scale: 9.0,
            ..Settings::default()
        });
        assert_eq!(big.text_scale, 1.5);
    }

    #[test]
    fn dpi_scale_defaults_to_one_and_clamps() {
        assert_eq!(Theme::default().dpi_scale, 1.0);
        assert_eq!(Theme::default().with_dpi(1.5).dpi_scale, 1.5);
        assert_eq!(Theme::default().with_dpi(f32::NAN).dpi_scale, 1.0);
        assert_eq!(Theme::default().with_dpi(99.0).dpi_scale, 8.0);
    }

    #[test]
    fn contrast_text_flips_on_light_fills() {
        assert_eq!(Theme::contrast_text(Rgba::WHITE).to_hex(), "#111317");
        assert_eq!(Theme::contrast_text(Rgba::BLACK), Rgba::WHITE);
    }
}
