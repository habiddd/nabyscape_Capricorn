//! Settings search (C# `SettingsSearch.cs`): the index of categories, sections and individual
//! controls, and the token match with its ranking.

use super::categories::{category_sections, friendly_section, CATEGORIES};
use naby_core::i18n;

/// One searchable target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchItem {
    pub title: String,
    pub category: &'static str,
    pub section: &'static str,
    /// The control (by its accessible name) to focus, empty for a whole section.
    pub control: &'static str,
    pub keywords: String,
}

/// The extra, hand-written entries (title, category, section, control, terms).
const EXTRA: &[(&str, &str, &str, &str, &str)] = &[
    (
        "Live lyrics on closed dock",
        "Audio",
        "Closed media display",
        "Closed media content",
        "lyrics lirik youtube apple song title none waveform",
    ),
    (
        "Feature usage",
        "Content",
        "Feature usage",
        "Connect to music apps",
        "resource ram essentials background disable features music download",
    ),
    (
        "Light on the closed dock",
        "Ambience",
        "Ambient light",
        "Show ambience on closed dock",
        "closed compact dock ambience tertutup hitam tuck glow",
    ),
    (
        "Ambient brightness",
        "Ambience",
        "Ambient light",
        "Ambient intensity",
        "intensity brightness terang cahaya warna",
    ),
    (
        "Light color & cover palette",
        "Ambience",
        "Ambient light",
        "Ambient color source",
        "single satu warna color cover blend spotify album custom",
    ),
    (
        "Light spread",
        "Ambience",
        "Ambient light",
        "Ambient spread",
        "spread lebar luas sebar cahaya",
    ),
    (
        "Light reach",
        "Ambience",
        "Ambient light",
        "Light reach",
        "height tinggi reach full panel",
    ),
    (
        "Light direction",
        "Ambience",
        "Ambient light",
        "Light direction",
        "direction arah glow atas bawah kiri kanan surround",
    ),
    (
        "Light softness",
        "Ambience",
        "Ambient light",
        "Ambient softness",
        "softness lembut fade gradient",
    ),
    (
        "Color balance",
        "Ambience",
        "Ambient light",
        "Color balance",
        "balance campur gabungan warna blend",
    ),
    (
        "Light reacts to music",
        "Ambience",
        "Ambient light",
        "Ambient audio response",
        "audio reactive react beat lagu musik responsive punchy",
    ),
    (
        "Ambient movement",
        "Ambience",
        "Ambient light",
        "Ambient movement",
        "motion breathe drift gerak animasi",
    ),
    (
        "Dock screen edge",
        "Layout",
        "Dock placement",
        "Dock side",
        "position posisi dock pindah kanan kiri atas bawah floating",
    ),
    (
        "Precise dock position & center",
        "Layout",
        "Move & resize",
        "Horizontal position percent",
        "center tengah pixel piksel presisi precise drag drop position posisi alignment",
    ),
    (
        "Smooth monitor follow",
        "Layout",
        "Display",
        "Slide smoothly between monitors",
        "monitor screen layar follow cursor kursor smooth animation slide",
    ),
    (
        "Drag & reorder widgets",
        "Content",
        "Enabled widgets & order",
        "",
        "drag drop urutan widget reorder susun",
    ),
    (
        "Hover shows active widgets",
        "Content",
        "Hover pages",
        "Show all active widgets in hover",
        "hover semua widget aktif active preview swipe slide",
    ),
    (
        "Dock size",
        "Layout",
        "Closed island",
        "Closed width",
        "width height ukuran lebar tinggi compact",
    ),
    (
        "Panel size",
        "Layout",
        "Open panel",
        "Panel width",
        "resize size ukuran besar panel",
    ),
    (
        "Automatic hide & tuck",
        "Visibility",
        "Automatic tucking",
        "",
        "tuck hide fullscreen fullscreen otomatis sembunyi dock",
    ),
    (
        "YouTube previews",
        "Visibility",
        "Automatic activity",
        "Automatic browser media previews",
        "youtube browser hover popup preview chrome ganggu",
    ),
    (
        "Keyboard shortcut",
        "Controls",
        "Global hotkey",
        "",
        "keyboard hotkey shortcut tombol pintasan buka dock",
    ),
    (
        "Lyrics",
        "Audio",
        "Lyrics",
        "",
        "lyrics lirik spotify karaoke lagu",
    ),
    (
        "Audio waveform",
        "Audio",
        "Visualizer",
        "",
        "waveform visualizer spectrum gelombang audio reaktif",
    ),
    (
        "Speaker & volume",
        "Audio",
        "Volume mixer",
        "",
        "speaker volume suara mixer microphone output device spotify",
    ),
    (
        "Clipboard capture",
        "Clipboard",
        "Clipboard history",
        "Save text & image history locally",
        "clipboard history salin copy paste riwayat gambar capture",
    ),
    (
        "Weather city",
        "Weather",
        "",
        "Weather city search",
        "cuaca kota city location lokasi weather",
    ),
    (
        "Weather temperature units",
        "Weather",
        "",
        "Weather temperature units",
        "suhu temperature celsius fahrenheit derajat",
    ),
    (
        "AI connection & API key",
        "AI",
        "AI companion",
        "Chat provider",
        "naby hermes gemini provider api key endpoint model voice chat",
    ),
    (
        "Choose a chat model",
        "AI",
        "AI companion",
        "Chat model",
        "discover models paste manual ai model",
    ),
    (
        "Voice setup & local models",
        "AI",
        "AI companion",
        "Speech connection",
        "asr tts microphone voice speech local hardware install model",
    ),
    (
        "Wake word · hands-free voice",
        "AI",
        "AI companion",
        "Wake word settings",
        "wake word hands free hey naby microphone paused resume listening",
    ),
    (
        "Uninstall ASR & TTS models",
        "AI",
        "AI companion",
        "Installed speech models",
        "uninstall remove model free space storage delete tts asr download cache",
    ),
    (
        "Uninstall NabyScape",
        "Application",
        "Local storage",
        "Uninstall NabyScape…",
        "uninstall remove application delete all data models cleanup fresh install hapus aplikasi",
    ),
    (
        "Image input & support test",
        "AI",
        "AI companion",
        "Image input",
        "image input vision send pictures photo support test",
    ),
    (
        "Test microphone and speech",
        "AI",
        "AI companion",
        "Speech checks",
        "microphone not working test transcription asr tts play sample speaker voice",
    ),
    (
        "Timer & focus duration",
        "Content",
        "Focus sessions",
        "",
        "focus timer pomodoro menit durasi break istirahat",
    ),
    (
        "Notification sounds",
        "Notifications",
        "Completion sound",
        "",
        "notifikasi notif suara sound bunyi timer fokus",
    ),
    (
        "Quiet hours",
        "Notifications",
        "Notification presentation",
        "",
        "quiet hours mute silent senyap diam jadwal malam",
    ),
    (
        "Launch at Windows startup",
        "Application",
        "Behavior",
        "Start automatically when signing in to Windows",
        "startup autostart windows mulai otomatis login",
    ),
];

/// `BuildSettingsSearchIndex`.
pub fn build_index() -> Vec<SearchItem> {
    let mut items = Vec::new();
    for c in &CATEGORIES {
        items.push(SearchItem {
            title: c.title.to_owned(),
            category: c.id,
            section: "",
            control: "",
            keywords: format!("{} {}", c.terms, c.description),
        });
        for section in category_sections(c.id) {
            items.push(SearchItem {
                title: friendly_section(section).to_owned(),
                category: c.id,
                section,
                control: "",
                keywords: format!("{} {}", section, c.title),
            });
        }
    }
    for (title, category, section, control, terms) in EXTRA {
        items.push(SearchItem {
            title: (*title).to_owned(),
            category,
            section,
            control,
            keywords: (*terms).to_owned(),
        });
    }
    items
}

fn contains_ci(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

/// Words that do not narrow a query (`ShowSettingsSearch`).
const FILLER: &[&str] = &[
    "settings",
    "setting",
    "pengaturan",
    "ubah",
    "change",
    "cara",
];

/// The best matches (at most 12) for `query`: every token must occur in the localized title,
/// the English title or the keywords; ranked by title prefix, title substring, then specific
/// controls over whole sections.
pub fn search<'a>(index: &'a [SearchItem], lang: &str, query: &str) -> Vec<&'a SearchItem> {
    let query = query.trim();
    if query.is_empty() {
        return Vec::new();
    }
    let mut tokens: Vec<&str> = query
        .split(' ')
        .filter(|t| !t.is_empty() && !FILLER.contains(t))
        .collect();
    if tokens.is_empty() {
        tokens.push(query);
    }
    let mut matches: Vec<&SearchItem> = index
        .iter()
        .filter(|item| {
            let hay = format!(
                "{} {} {}",
                i18n::t(lang, &item.title),
                item.title,
                item.keywords
            );
            tokens.iter().all(|t| contains_ci(&hay, t))
        })
        .collect();
    let q = query.to_lowercase();
    // Stable sort, descending on each key in turn (LINQ OrderByDescending / ThenByDescending).
    matches.sort_by_key(|item| {
        let title = item.title.to_lowercase();
        (
            std::cmp::Reverse(title.starts_with(&q)),
            std::cmp::Reverse(title.contains(&q)),
            std::cmp::Reverse(!item.control.is_empty()),
        )
    });
    matches.truncate(12);
    matches
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_a_setting_by_its_label_and_ranks_prefixes_first() {
        let index = build_index();
        let hits = search(&index, "en", "quiet hours");
        assert_eq!(hits[0].title, "Quiet hours");
        assert_eq!(hits[0].category, "Notifications");
        let lyrics = search(&index, "en", "lyrics");
        assert!(lyrics.iter().any(|i| i.category == "Audio"));
        assert!(lyrics.len() <= 12);
        assert_eq!(lyrics[0].title, "Lyrics", "the exact title ranks first");
    }

    #[test]
    fn filler_words_are_ignored_and_localized_titles_match() {
        let index = build_index();
        let plain = search(&index, "en", "change weather");
        assert!(plain.iter().any(|i| i.category == "Weather"));
        let spanish = search(&index, "es", "Aspecto");
        assert!(spanish.iter().any(|i| i.category == "Appearance"));
        assert!(search(&index, "en", "   ").is_empty());
        assert!(search(&index, "en", "zzzzzz-no-such-setting").is_empty());
    }

    #[test]
    fn every_extra_target_names_a_real_category() {
        for item in build_index() {
            assert!(
                CATEGORIES.iter().any(|c| c.id == item.category),
                "{}",
                item.title
            );
        }
    }
}
