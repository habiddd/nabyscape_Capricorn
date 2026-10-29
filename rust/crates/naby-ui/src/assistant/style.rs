//! Colors and scale shared by the assistant views (`AssistantTheme.xaml`, `UI.cs` constants).

use naby_core::color::Rgba;
use naby_render::text::FontSource;
use naby_render::{Fonts, Theme};

/// Device pixels per design unit (`s`) and the text multiplier (`t`, DPI scale times the
/// accessibility text scale). Geometry is `dip * s`; font sizes are `dip * t`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Metrics {
    pub s: f32,
    pub t: f32,
}

impl Metrics {
    pub fn new(dpi_scale: f32, text_scale: f32) -> Self {
        let s = if dpi_scale.is_finite() {
            dpi_scale.clamp(0.5, 4.0)
        } else {
            1.0
        };
        let t = if text_scale.is_finite() {
            text_scale.clamp(0.5, 3.0)
        } else {
            1.0
        };
        Self { s, t: s * t }
    }

    pub fn px(&self, dip: f32) -> f32 {
        dip * self.s
    }

    pub fn font(&self, dip: f32) -> f32 {
        dip * self.t
    }
}

/// The assistant window palette. The window is darker than the dock; high contrast defers to
/// the system colors carried by the theme.
#[derive(Debug, Clone, PartialEq)]
pub struct Palette {
    pub window: Rgba,
    pub caption: Rgba,
    pub text: Rgba,
    pub dim: Rgba,
    pub control_fill: Rgba,
    pub control_border: Rgba,
    pub card: Rgba,
    pub card_border: Rgba,
    pub rule: Rgba,
    pub quote: Rgba,
    pub link: Rgba,
    pub code_inline: Rgba,
    pub error: Rgba,
    pub ok: Rgba,
    pub accent: Rgba,
    pub accent_text: Rgba,
    pub selection_fill: Rgba,
    pub selection_text: Rgba,
    pub high_contrast: bool,
}

impl Palette {
    pub fn from_theme(theme: &Theme) -> Self {
        if theme.high_contrast {
            return Self {
                window: theme.background,
                caption: theme.control_fill,
                text: theme.text,
                dim: theme.text,
                control_fill: theme.control_fill,
                control_border: theme.control_border,
                card: theme.control_fill,
                card_border: theme.control_border,
                rule: theme.control_border,
                quote: theme.control_border,
                link: theme.accent,
                code_inline: theme.control_fill,
                error: theme.invalid,
                ok: theme.text,
                accent: theme.accent,
                accent_text: theme.accent_text,
                selection_fill: theme.selection_fill,
                selection_text: theme.selection_text,
                high_contrast: true,
            };
        }
        Self {
            window: Rgba::rgb(0x0D, 0x0E, 0x10),
            caption: Rgba::rgb(0x15, 0x17, 0x1A),
            text: theme.text,
            dim: theme.text_dim,
            control_fill: theme.control_fill,
            control_border: theme.control_border,
            card: Rgba::rgb(0x17, 0x1A, 0x20),
            card_border: Rgba::rgb(0x2B, 0x30, 0x39),
            rule: Rgba::rgb(0x34, 0x3A, 0x45),
            quote: Rgba::rgb(0x5C, 0x6C, 0x81),
            link: Rgba::rgb(0xAD, 0xC8, 0xEA),
            code_inline: Rgba::rgb(0x28, 0x2E, 0x38),
            error: Rgba::rgb(0xEA, 0xB3, 0xAD),
            ok: Rgba::rgb(0xA9, 0xCD, 0xBD),
            accent: theme.accent,
            accent_text: theme.accent_text,
            selection_fill: theme.selection_fill,
            selection_text: theme.selection_text,
            high_contrast: false,
        }
    }
}

/// The theme the assistant draws with: the user's colors, text scaled by DPI and accessibility.
/// The composer sits in its own rounded frame, so text fields lose their own rule.
pub fn chat_theme(base: &Theme, metrics: &Metrics, borderless_fields: bool) -> Theme {
    let mut theme = base.clone();
    theme.text_scale = metrics.t;
    if borderless_fields && !theme.high_contrast {
        theme.underline_border = Rgba::TRANSPARENT;
        theme.input_focus_border = Rgba::TRANSPARENT;
        theme.input_hover_border = Rgba::TRANSPARENT;
    }
    theme
}

/// A monospace face for code (Consolas on Windows, DejaVu/Liberation Mono elsewhere). Loaded
/// lazily by `Fonts`; an empty `Fonts` is returned when none exists.
pub fn load_mono() -> Fonts {
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    if cfg!(windows) {
        let windir = std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into());
        let fonts = std::path::PathBuf::from(windir).join("Fonts");
        for name in ["CascadiaMono.ttf", "consola.ttf", "lucon.ttf"] {
            candidates.push(fonts.join(name));
        }
    } else {
        for path in [
            "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationMono-Regular.ttf",
            "/usr/share/fonts/TTF/DejaVuSansMono.ttf",
        ] {
            candidates.push(path.into());
        }
    }
    let regular: Vec<FontSource> = candidates
        .into_iter()
        .find(|p| p.is_file())
        .map(FontSource::Path)
        .into_iter()
        .collect();
    Fonts::from_sources(&regular, &[], &[], &[])
}
