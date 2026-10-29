//! Union of every C# `Normalize*` rule on `Settings` (Models.cs `Normalize`,
//! `NormalizeLanguageAccessibility`, `NormalizeMediaUx`, `NormalizePremium`, `NormalizeFlexible`).

use super::{
    is_tool_name, is_widget_name, valid_color, Settings, WidgetSetting, HOVER_PAGE_IDS,
    SETTINGS_BACKDROP_CHOICES, TOOL_NAMES, WIDGET_NAMES,
};

fn clamp(value: f64, min: f64, max: f64, fallback: f64) -> f64 {
    if value.is_finite() {
        value.max(min).min(max)
    } else {
        fallback
    }
}

fn one_of(value: &str, allowed: &[&str]) -> bool {
    allowed.contains(&value)
}

fn set_unless(field: &mut String, allowed: &[&str], fallback: &str) {
    if !one_of(field, allowed) {
        *field = fallback.to_string();
    }
}

/// Single character `A`-`Z` (C# checks UTF-16 length 1 and the char range).
fn hotkey_letter(value: &str) -> bool {
    let mut it = value.chars();
    matches!((it.next(), it.next()), (Some('A'..='Z'), None))
}

const MODIFIERS: &[&str] = &["Ctrl+Alt", "Ctrl+Shift", "Alt+Shift"];

/// `uint.TryParse(text, NumberStyles.HexNumber)`: optional surrounding whitespace, then hex digits.
fn parses_as_hex(text: &str) -> bool {
    let ws = |c: char| matches!(c, ' ' | '\t' | '\n' | '\u{b}' | '\u{c}' | '\r');
    let trimmed = text
        .trim_start_matches(ws)
        .trim_end_matches(|c| ws(c) || c == '\0');
    !trimmed.is_empty() && trimmed.len() <= 8 && trimmed.bytes().all(|b| b.is_ascii_hexdigit())
}

pub(super) fn normalize(s: &mut Settings) {
    // NormalizeLanguageAccessibility
    if s.ui_language != "es" && s.ui_language != "id" {
        s.ui_language = "en".to_string();
    }
    s.accessibility_text_scale = clamp(s.accessibility_text_scale, 1.0, 1.5, 1.0);

    if !SETTINGS_BACKDROP_CHOICES.contains(&s.settings_material.as_str()) {
        s.settings_material = String::new();
    }
    set_unless(
        &mut s.notification_delivery,
        &["Notch", "Windows", "Both", "Off"],
        "Notch",
    );
    s.tuck_edge = clamp(s.tuck_edge, 0.0, 12.0, 6.0);
    if !hotkey_letter(&s.tuck_hotkey_key) {
        s.tuck_hotkey_key = "T".to_string();
    }
    set_unless(&mut s.tuck_hotkey_modifiers, MODIFIERS, "Ctrl+Alt");
    s.width = clamp(s.width, 180.0, 400.0, 228.0);
    s.height = clamp(s.height, 28.0, 72.0, 32.0);
    s.radius = clamp(s.radius, 0.0, 40.0, 16.0);
    s.horizontal = clamp(s.horizontal, 0.0, 100.0, 50.0);
    s.top = clamp(s.top, 0.0, 3000.0, 0.0);
    s.opacity = clamp(s.opacity, 0.65, 1.0, 1.0);
    s.animation_ms = clamp(s.animation_ms, 0.0, 600.0, 420.0);
    s.panel_width = clamp(s.panel_width, 300.0, 960.0, 384.0);
    s.panel_max_height = clamp(s.panel_max_height, 240.0, 1100.0, 408.0);
    s.panel_padding = clamp(s.panel_padding, 16.0, 40.0, 32.0);
    s.panel_scale = clamp(s.panel_scale, 0.85, 1.25, 1.0);
    s.section_gap = clamp(s.section_gap, 8.0, 28.0, 18.0);
    s.hover_width = clamp(s.hover_width, 300.0, 760.0, 350.0);
    s.hover_height = clamp(s.hover_height, 184.0, 440.0, 192.0);
    s.hover_delay_ms = s.hover_delay_ms.clamp(80, 900);
    s.hover_detail_delay_ms = s.hover_detail_delay_ms.clamp(200, 1200);
    s.swipe_distance = clamp(s.swipe_distance, 24.0, 100.0, 48.0);
    s.glass_strength = clamp(s.glass_strength, 0.0, 1.0, 1.0);

    // Hover pages: known ids only, first occurrence wins, missing ones appended.
    let mut hover: Vec<WidgetSetting> = Vec::new();
    for v in std::mem::take(&mut s.hover_views) {
        if HOVER_PAGE_IDS.contains(&v.id.as_str()) && !hover.iter().any(|h| h.id == v.id) {
            hover.push(v);
        }
    }
    for id in HOVER_PAGE_IDS {
        if !hover.iter().any(|v| v.id == *id) {
            hover.push(WidgetSetting::new(
                id,
                !matches!(*id, "system" | "clips" | "tasks" | "stopwatch"),
            ));
        }
    }
    s.hover_views = hover;

    let mut tools: Vec<WidgetSetting> = Vec::new();
    for t in std::mem::take(&mut s.tools) {
        if is_tool_name(&t.id) && !tools.iter().any(|x| x.id == t.id) {
            tools.push(t);
        }
    }
    for (id, _) in TOOL_NAMES {
        if !tools.iter().any(|t| t.id == *id) {
            tools.push(WidgetSetting::new(id, true));
        }
    }
    s.tools = tools;

    set_unless(&mut s.performance, &["Auto", "Hemat", "Maksimal"], "Auto");
    set_unless(&mut s.wheel_action, &["volume", "pages", "none"], "volume");
    set_unless(
        &mut s.middle_action,
        &["toggle", "microphone", "none"],
        "toggle",
    );
    s.volume_step = s.volume_step.clamp(1, 10);
    if !hotkey_letter(&s.hotkey_key) {
        s.hotkey_key = "I".to_string();
    }
    set_unless(&mut s.hotkey_modifiers, MODIFIERS, "Ctrl+Alt");
    if !hotkey_letter(&s.visibility_hotkey_key) {
        s.visibility_hotkey_key = "H".to_string();
    }
    set_unless(&mut s.visibility_hotkey_modifiers, MODIFIERS, "Ctrl+Alt");
    set_unless(
        &mut s.closed_media_content,
        &["Title", "Waveform", "Lyrics", "None"],
        "Title",
    );
    set_unless(
        &mut s.closed_media_icon,
        &["Waveform", "Artwork", "None"],
        "Waveform",
    );
    s.category_style = "Dots".to_string();
    set_unless(
        &mut s.launch_style,
        &["IconsLabels", "Icons", "Text"],
        "IconsLabels",
    );

    let mut recent: Vec<String> = Vec::new();
    for c in std::mem::take(&mut s.recent_colors) {
        if valid_color(&c) && !recent.iter().any(|r| r.eq_ignore_ascii_case(&c)) {
            recent.push(c);
        }
        if recent.len() == 8 {
            break;
        }
    }
    s.recent_colors = recent;

    s.focus_minutes = s.focus_minutes.clamp(1, 240);
    s.break_minutes = s.break_minutes.clamp(1, 120);
    if s.preset != "Mac" && s.preset != "iPhone" {
        s.preset = "iPhone".to_string();
    }
    for (field, fallback) in [
        (&mut s.background, "#000000"),
        (&mut s.accent, "#FFFFFF"),
        (&mut s.control_fill, "#16171B"),
        (&mut s.control_border, "#303238"),
        (&mut s.selection_fill, "#34373E"),
    ] {
        if !valid_color(field) {
            *field = fallback.to_string();
        }
    }

    // Main-panel widgets, including migration of the legacy `tools` group.
    let mut widgets: Vec<WidgetSetting> = Vec::new();
    for w in std::mem::take(&mut s.widgets) {
        if is_widget_name(&w.id) && !widgets.iter().any(|x| x.id == w.id) {
            widgets.push(w);
        }
    }
    if let Some(mut at) = widgets.iter().position(|w| w.id == "tools") {
        let enabled = widgets[at].enabled;
        widgets.remove(at);
        for tool in &s.tools {
            if !widgets.iter().any(|w| w.id == tool.id) {
                widgets.insert(at, WidgetSetting::new(&tool.id, enabled && tool.enabled));
                at += 1;
            }
        }
    }
    for (id, _) in WIDGET_NAMES {
        if *id != "tools" && !widgets.iter().any(|w| w.id == *id) {
            let enabled =
                *id == "weather" || (!matches!(*id, "stopwatch" | "voice") && !is_tool_name(id));
            widgets.push(WidgetSetting::new(id, enabled));
        }
    }
    s.widgets = widgets;
    // Hiding a main-panel widget must not remove an independently enabled hover page.
    for tool in &mut s.tools {
        tool.enabled |= s.widgets.iter().any(|w| w.id == tool.id && w.enabled);
    }

    normalize_media_ux(s);
    normalize_premium(s);
    normalize_flexible(s);

    s.shortcuts
        .retain(|x| !x.name.trim().is_empty() && !x.target.trim().is_empty());
    s.shortcuts.truncate(40);
    if !matches!(s.display_mode.as_str(), "Primary" | "Selected" | "Follow") {
        s.display_mode = if s.monitor.is_empty() {
            "Primary"
        } else {
            "Selected"
        }
        .to_string();
    }
}

fn normalize_media_ux(s: &mut Settings) {
    set_unless(
        &mut s.waveform_source,
        &["Spectrum", "Envelope"],
        "Spectrum",
    );
    set_unless(
        &mut s.lyrics_animation,
        &["Slide", "Focus", "Karaoke"],
        "Slide",
    );
    set_unless(
        &mut s.countdown_sound,
        &["Chime", "Bell", "Custom", "Off"],
        "Chime",
    );
    s.waveform_sensitivity = clamp(s.waveform_sensitivity, 0.5, 3.0, 1.35);
    s.lyrics_offset = clamp(s.lyrics_offset, -10.0, 10.0, 0.0);
    s.countdown_sound_volume = clamp(s.countdown_sound_volume, 0.0, 1.0, 0.7);
}

fn normalize_premium(s: &mut Settings) {
    s.ambient_strength = clamp(s.ambient_strength, 0.0, 1.0, 0.5);
    s.ambient_spread = clamp(s.ambient_spread, 0.25, 2.5, 0.75);
    s.notification_seconds = clamp(s.notification_seconds, 4.0, 30.0, 12.0);
    s.quiet_start = s.quiet_start.clamp(0, 23);
    s.quiet_end = s.quiet_end.clamp(0, 23);
    set_unless(
        &mut s.ambient_motion,
        &["Still", "Breathe", "Drift"],
        "Drift",
    );
    set_unless(
        &mut s.ambient_scope,
        &["All widgets", "Music only", "Selected widgets"],
        "All widgets",
    );
    let mut ids: Vec<String> = Vec::new();
    for id in std::mem::take(&mut s.ambient_widgets) {
        let known = id == "closed" || id == "ai" || (is_widget_name(&id) && id != "tools");
        if known && !ids.contains(&id) {
            ids.push(id);
        }
    }
    s.ambient_widgets = ids;
    set_unless(
        &mut s.ambient_color_source,
        &[
            "Album cover",
            "Accent color",
            "Custom color",
            "Custom blend",
        ],
        "Album cover",
    );
    let c = &s.ambient_custom_color;
    let custom_ok = c.chars().count() == 7
        && c.starts_with('#')
        && c.is_char_boundary(1)
        && parses_as_hex(&c[1..]);
    if !custom_ok {
        s.ambient_custom_color = "#7AA8DC".to_string();
    }
    set_unless(
        &mut s.ambient_audio_response,
        &["Off", "Soft", "Responsive", "Punchy"],
        "Responsive",
    );
    s.ambient_sensitivity = clamp(s.ambient_sensitivity, 0.5, 3.0, 1.4);
    set_unless(
        &mut s.interaction_motion,
        &["Off", "Subtle", "Expressive"],
        "Subtle",
    );
    set_unless(
        &mut s.notification_animation,
        &["Lift", "Fade", "Soft scale"],
        "Lift",
    );
}

fn normalize_flexible(s: &mut Settings) {
    s.follow_monitor_duration = clamp(s.follow_monitor_duration, 180.0, 700.0, 360.0);
    set_unless(
        &mut s.dock_edge,
        &["Top", "Bottom", "Left", "Right", "Floating"],
        "Top",
    );
    s.dock_vertical = clamp(s.dock_vertical, 0.0, 100.0, 50.0);
    s.dock_inset = clamp(s.dock_inset, 0.0, 200.0, 8.0);
    set_unless(
        &mut s.ambient_palette,
        &["Single color", "Cover blend"],
        "Cover blend",
    );
    set_unless(
        &mut s.ambient_direction,
        &["Bottom", "Top", "Left", "Right", "Surround"],
        "Bottom",
    );
    s.ambient_height = clamp(s.ambient_height, 0.25, 2.0, 1.0);
    s.ambient_balance = clamp(s.ambient_balance, 0.0, 1.0, 0.5);
    s.ambient_softness = clamp(s.ambient_softness, 0.0, 1.0, 0.5);
    if !valid_color(&s.ambient_secondary_color) {
        s.ambient_secondary_color = "#CB72D9".to_string();
    }
    // Trim, then cap at 120 UTF-16 code units without splitting a surrogate pair.
    let trimmed = s.weather_city.trim();
    let mut city = String::new();
    let mut units = 0usize;
    for ch in trimmed.chars() {
        units += ch.len_utf16();
        if units > 120 {
            break;
        }
        city.push(ch);
    }
    s.weather_city = city;
    let valid = match (s.weather_latitude, s.weather_longitude) {
        (Some(lat), Some(lon)) => {
            lat.is_finite() && lon.is_finite() && lat.abs() <= 90.0 && lon.abs() <= 180.0
        }
        _ => false,
    };
    if !valid {
        s.weather_latitude = None;
        s.weather_longitude = None;
    }
    set_unless(&mut s.weather_units, &["Celsius", "Fahrenheit"], "Celsius");
}
