//! The Settings categories and section-to-category rules (C# `SettingsCategories.cs`).

use naby_core::i18n;

/// One navigation entry (`SettingsCategory`). `sections` limits which section cards of the
/// shared page builder this category shows.
#[derive(Debug, Clone, Copy)]
pub struct Category {
    pub id: &'static str,
    pub title: &'static str,
    pub group: &'static str,
    pub description: &'static str,
    pub terms: &'static str,
    pub sections: Option<&'static [&'static str]>,
}

const fn cat(
    id: &'static str,
    title: &'static str,
    group: &'static str,
    description: &'static str,
    terms: &'static str,
    sections: Option<&'static [&'static str]>,
) -> Category {
    Category {
        id,
        title,
        group,
        description,
        terms,
        sections,
    }
}

pub const CATEGORIES: [Category; 16] = [
    cat(
        "Home",
        "Start here",
        "Settings",
        "Make NabyScape work the way you want.",
        "home start setup help beginner",
        None,
    ),
    cat(
        "Appearance",
        "Look & colors",
        "Personalize",
        "Choose the dock's shape, colors and text style.",
        "appearance theme warna tampilan font shape glass",
        Some(&[
            "Shape",
            "Settings material",
            "Color & material",
            "Surface",
            "Controls & outlines",
            "Typography",
            "Text and navigation",
        ]),
    ),
    cat(
        "Ambience",
        "Ambience",
        "Personalize",
        "Choose where the light appears and how it follows your music.",
        "ambient ambience cahaya warna glow spread softness light reach closed dock tertutup",
        Some(&["Ambient light"]),
    ),
    cat(
        "Motion",
        "Motion & speed",
        "Personalize",
        "Adjust animation and choose a balance between smoothness and power.",
        "animation animasi motion fps speed performance ram cpu hemat reduce",
        Some(&["Motion", "Animation performance"]),
    ),
    cat(
        "Layout",
        "Position & size",
        "Dock",
        "Place the dock on a screen edge or move it freely.",
        "position posisi pindah atas bawah kanan kiri monitor size resize ukuran",
        None,
    ),
    cat(
        "Visibility",
        "Show & hide",
        "Dock",
        "Choose when the dock hides, tucks away or shows a preview.",
        "hide show tuck sembunyi tutup youtube fullscreen maximized hover preview",
        Some(&[
            "Automatic tucking",
            "Manual tuck shortcut",
            "Hide or show the island",
            "Automatic activity",
        ]),
    ),
    cat(
        "Controls",
        "Controls",
        "Dock",
        "Set mouse gestures, hover behavior and keyboard shortcuts.",
        "mouse keyboard shortcut hotkey swipe scroll gesture klik tombol",
        None,
    ),
    cat(
        "Content",
        "Widgets & tools",
        "Features",
        "Turn widgets on or off, reorder them and configure focus sessions.",
        "widget tools urutan notes task timer focus stopwatch pomodoro",
        None,
    ),
    cat(
        "Audio",
        "Music & sound",
        "Features",
        "Customize music controls, lyrics and the audio visualizer.",
        "music spotify volume musik suara waveform spectrum lirik lyrics audio microphone",
        None,
    ),
    cat(
        "Clipboard",
        "Clipboard",
        "Features",
        "Control whether copied text and images are saved on this device.",
        "clipboard salin copy paste capture history riwayat gambar",
        Some(&["Clipboard history"]),
    ),
    cat(
        "Weather",
        "Weather",
        "Features",
        "Choose your city and see the weather in Celsius or Fahrenheit.",
        "weather cuaca kota city temperature suhu rain hujan",
        None,
    ),
    cat(
        "System",
        "Clock & system",
        "Features",
        "Choose the clock format and computer information shown in widgets.",
        "clock jam tanggal date battery cpu ram network storage system",
        None,
    ),
    cat(
        "Launch",
        "Shortcuts",
        "Features",
        "Choose your favorite apps, folders and websites.",
        "apps launch aplikasi shortcuts explorer",
        None,
    ),
    cat(
        "AI",
        "Naby AI",
        "Features",
        "Connect an AI provider and manage companion preferences.",
        "ai naby hermes gemini live voice provider api key chat model token",
        None,
    ),
    cat(
        "Notifications",
        "Notifications",
        "App",
        "Choose which alerts appear, their sounds and quiet hours.",
        "notifications notif pemberitahuan alert sound bunyi quiet hours downloads",
        Some(&[
            "Notifications",
            "Notification presentation",
            "Downloads",
            "Completion sound",
        ]),
    ),
    cat(
        "Application",
        "General",
        "App",
        "Manage startup, updates, local data and app preferences.",
        "general startup update version install storage data reset backup mulai windows",
        None,
    ),
];

/// `Category(id)`: unknown ids fall back to Home.
pub fn category(id: &str) -> &'static Category {
    CATEGORIES
        .iter()
        .find(|c| c.id == id)
        .unwrap_or(&CATEGORIES[0])
}

/// The localized title of a category id.
pub fn category_title(lang: &str, id: &str) -> String {
    i18n::t(lang, category(id).title).to_owned()
}

/// `CategoryId(title)`: accepts the English or the localized title.
pub fn category_id(lang: &str, title: &str) -> String {
    CATEGORIES
        .iter()
        .find(|c| c.title == title || i18n::t(lang, c.title) == title)
        .map_or_else(|| title.to_owned(), |c| c.id.to_owned())
}

/// `SettingsRoot(id)`: which shared page builder draws the category.
pub fn root(id: &str) -> &str {
    match id {
        "Ambience" | "Motion" => "Appearance",
        "Visibility" => "Controls",
        "Clipboard" => "Content",
        "Notifications" => "Application",
        other => other,
    }
}

/// `FriendlySection(title)`: the heading shown for a section.
pub fn friendly_section(title: &str) -> &str {
    match title {
        "Shape" => "Dock shape",
        "Color & material" => "Colors",
        "Settings material" => "Settings window",
        "Surface" => "Glass appearance",
        "Controls & outlines" => "Buttons & input fields",
        "Typography" => "Text weight",
        "Text and navigation" => "Panel dots",
        "Animation performance" => "Smoothness & power",
        "Display" => "Screen",
        "Closed island" => "Compact dock",
        "Open panel" => "Expanded dock",
        "Enabled widgets & order" => "Your widgets",
        "Notch gestures" => "Mouse actions",
        "Global hotkey" => "Open dock shortcut",
        "Automatic activity" => "Automatic previews",
        "Clipboard history" => "Clipboard capture",
        "Completion sound" => "Timer & focus sound",
        other => other,
    }
}

/// Sections listed in the search index for a category (`CategorySections`).
pub fn category_sections(id: &str) -> &'static [&'static str] {
    let c = category(id);
    if let Some(s) = c.sections {
        return s;
    }
    match id {
        "Layout" => &[
            "Display",
            "Dock placement",
            "Move & resize",
            "Closed island",
            "Open panel",
            "Hover preview",
            "Layout profile",
        ],
        "Content" => &[
            "Feature usage",
            "Enabled widgets & order",
            "Individual tools",
            "Hover pages",
            "Focus sessions",
            "Stopwatch",
        ],
        "Controls" => &[
            "Hover & swipe",
            "Panel navigation",
            "Drag to Island",
            "Detachable previews",
            "Closed cover gestures",
            "Notch gestures",
            "Global hotkey",
        ],
        "Audio" => &[
            "Visualizer",
            "Lyrics",
            "Closed media display",
            "Volume mixer",
            "Waveform source",
            "Live signal",
            "Live feedback",
            "Audio behavior",
            "Header wheel",
        ],
        "System" => &["Clock", "Monitor details"],
        "Launch" => &["Launch display", "Shortcuts"],
        "AI" => &["AI companion"],
        "Application" => &[
            "Language & accessibility",
            "Updates",
            "Behavior",
            "Local storage",
            "Private diagnostics",
            "Reset preferences",
            "About NabyScape",
        ],
        _ => &[],
    }
}

/// Sections the shared builder draws that this category must not show.
fn excluded(id: &str) -> &'static [&'static str] {
    match id {
        "Content" => &["Clipboard history"],
        "Application" => category("Notifications").sections.unwrap_or(&[]),
        "Controls" => category("Visibility").sections.unwrap_or(&[]),
        _ => &[],
    }
}

/// `FilterSettingsCategory`: whether the category shows the section card `title`.
pub fn shows_section(id: &str, title: &str) -> bool {
    let c = category(id);
    if let Some(allowed) = c.sections {
        if !allowed.contains(&title) {
            return false;
        }
    }
    !excluded(id).contains(&title)
}

/// Categories whose page shows a live-preview rail (`noPreview` is false in `Responsive`).
pub fn has_preview(id: &str) -> bool {
    matches!(
        id,
        "Appearance" | "Ambience" | "Layout" | "Motion" | "Audio"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn category_ids_are_unique_and_unknown_ids_open_home() {
        for (i, a) in CATEGORIES.iter().enumerate() {
            for b in &CATEGORIES[i + 1..] {
                assert_ne!(a.id, b.id);
            }
        }
        assert_eq!(category("nope").id, "Home");
        assert_eq!(category_id("en", "Look & colors"), "Appearance");
        assert_eq!(category_id("es", "Aspecto y colores"), "Appearance");
    }

    #[test]
    fn shared_builders_filter_their_sections() {
        assert!(shows_section("Appearance", "Shape"));
        assert!(!shows_section("Appearance", "Ambient light"));
        assert!(shows_section("Ambience", "Ambient light"));
        assert!(!shows_section("Content", "Clipboard history"));
        assert!(shows_section("Clipboard", "Clipboard history"));
        assert!(!shows_section("Application", "Notifications"));
        assert!(shows_section("Notifications", "Downloads"));
        assert!(!shows_section("Controls", "Automatic tucking"));
        assert!(shows_section("Visibility", "Automatic tucking"));
        assert_eq!(root("Ambience"), "Appearance");
        assert_eq!(friendly_section("Shape"), "Dock shape");
    }
}
