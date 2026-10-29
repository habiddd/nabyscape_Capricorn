//! Colors, radii and font sizes from `Theme.xaml` / `AssistantTheme.xaml` and `UI.cs`.
//!
//! `Theme::from_settings` is added during integration; until then build a theme with
//! [`Theme::default`] or [`Theme::new`].

use naby_core::color::Rgba;

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
        }
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
    fn contrast_text_flips_on_light_fills() {
        assert_eq!(Theme::contrast_text(Rgba::WHITE).to_hex(), "#111317");
        assert_eq!(Theme::contrast_text(Rgba::BLACK), Rgba::WHITE);
    }
}
