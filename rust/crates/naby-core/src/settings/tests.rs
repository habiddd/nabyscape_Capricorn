//! Ports of the settings scenarios from the C# tests (PersistenceTests, SettingsUxTests,
//! PremiumExperienceTests, ProductionFoundationTests, UxCompletionTests, SelfTests,
//! MediaExperienceTests, InteractionHotfixTests, InstallationInputTests, Stage5Tests).

#![allow(clippy::field_reassign_with_default)]

use super::policy::{hover_pages, PREMIUM_KEYS};
use super::presets::{self, ProfileError};
use super::reset::{self, ResetCategory, ResetScope};
use super::welcome::{self, WelcomePreset, WelcomeState};
use super::{
    profile, resolve_backdrop, valid_color, BackdropConditions, Settings, WidgetSetting,
    HOVER_PAGE_IDS, TOOL_NAMES, WIDGET_NAMES,
};
use chrono::{TimeZone, Utc};
use serde_json::{json, Value};

fn ids(list: &[WidgetSetting]) -> Vec<&str> {
    list.iter().map(|w| w.id.as_str()).collect()
}

fn normalized(mut s: Settings) -> Settings {
    s.normalize();
    s
}

fn keys_of(s: &Settings) -> Vec<String> {
    match serde_json::to_value(s).unwrap() {
        Value::Object(m) => m.keys().cloned().collect(),
        _ => unreachable!(),
    }
}

#[test]
fn defaults_match_csharp() {
    let s = Settings::default();
    assert_eq!((s.width, s.height, s.radius), (228.0, 32.0, 16.0));
    assert_eq!(s.animation_ms, 420.0);
    assert_eq!(s.hotkey_key, "I");
    let expected: Vec<&str> = WIDGET_NAMES
        .iter()
        .map(|(k, _)| *k)
        .filter(|k| *k != "tools")
        .collect();
    assert_eq!(ids(&s.widgets), expected);
    for w in &s.widgets {
        assert_eq!(
            w.enabled,
            w.id != "stopwatch" && w.id != "voice",
            "{}",
            w.id
        );
    }
    assert_eq!(
        ids(&s.tools),
        TOOL_NAMES.iter().map(|(k, _)| *k).collect::<Vec<_>>()
    );
    assert!(s.tools.iter().all(|t| t.enabled));
    assert_eq!(
        ids(&s.hover_views),
        ["media", "timer", "focus", "calendar", "notes", "files", "system"]
    );
    assert!(!s.hover_views.last().unwrap().enabled);
    assert_eq!(s.shortcuts.len(), 2);
    assert_eq!(s.shortcuts[1].target, "ms-settings:");
    assert_eq!(s.notification_delivery, "Notch");
    assert_eq!(s.ambient_widgets.len(), 20);
    assert_eq!(s.ambient_widgets[18..], ["closed", "ai"]);
    assert!(s.display_mode.is_empty());
    assert_eq!(s.category_style, "Dots");
}

#[test]
fn serialized_keys_cover_bundled_preferences() {
    // The bundled factory file lists every writable C# property except Shortcuts.
    let prefs: Value = serde_json::from_str(include_str!(
        "../../../../../docs/presets/default-preferences.json"
    ))
    .unwrap();
    let mut file_keys: Vec<String> = prefs.as_object().unwrap().keys().cloned().collect();
    let mut ours = keys_of(&Settings::default());
    file_keys.sort();
    ours.sort();
    let mut expected = file_keys.clone();
    expected.push("Shortcuts".into());
    expected.sort();
    assert_eq!(ours, expected);
}

#[test]
fn normalize_clamps() {
    let s = normalized(Settings {
        width: 999.0,
        opacity: 0.1,
        hotkey_modifiers: "Win".into(),
        panel_width: f64::NAN,
        ..Settings::default()
    });
    assert_eq!(s.width, 400.0);
    assert_eq!(s.opacity, 0.65);
    assert_eq!(s.hotkey_modifiers, "Ctrl+Alt");
    assert_eq!(s.panel_width, 384.0);
}

#[test]
fn normalize_dimensions_colors_widgets_and_durations() {
    let s = normalized(Settings {
        width: 900.0,
        height: -4.0,
        opacity: 2.0,
        accent: "invalid".into(),
        focus_minutes: 0,
        widgets: vec![
            WidgetSetting::new("media", true),
            WidgetSetting::new("media", true),
            WidgetSetting::new("invalid", true),
        ],
        ..Settings::default()
    });
    assert_eq!((s.width, s.height, s.opacity), (400.0, 28.0, 1.0));
    assert_eq!((s.accent.as_str(), s.focus_minutes), ("#FFFFFF", 1));
    assert_eq!(s.widgets.len(), WIDGET_NAMES.len() - 1);
    assert!(s.widgets.iter().all(|w| w.id != "tools"));
}

#[test]
fn hex_colors_reject_malformed_input() {
    assert!(valid_color("#abcDEF"));
    assert!(!valid_color("red"));
    assert!(!valid_color("#00000000"));
    assert!(!valid_color(""));
}

#[test]
fn enhancement_settings_validate_unknown_values() {
    let s = normalized(Settings {
        performance: "bad".into(),
        wheel_action: "bad".into(),
        hotkey_key: "5".into(),
        volume_step: 999,
        ..Settings::default()
    });
    assert_eq!(
        (s.performance.as_str(), s.wheel_action.as_str()),
        ("Auto", "volume")
    );
    assert_eq!((s.hotkey_key.as_str(), s.volume_step), ("I", 10));
    assert!(!s.clipboard_history);
}

#[test]
fn layout_repairs_non_finite_geometry_and_preserves_hover_order() {
    let s = normalized(Settings {
        panel_width: f64::NAN,
        panel_scale: 9.0,
        hover_height: -1.0,
        swipe_distance: 0.0,
        hover_delay_ms: 0,
        hover_views: vec![
            WidgetSetting::new("notes", false),
            WidgetSetting::new("notes", true),
            WidgetSetting::new("unknown", true),
        ],
        ..Settings::default()
    });
    assert_eq!(s.panel_width, 384.0);
    assert_eq!(s.panel_scale, 1.25);
    assert_eq!(s.hover_height, 184.0);
    assert_eq!(s.swipe_distance, 24.0);
    assert_eq!(s.hover_delay_ms, 80);
    assert_eq!(s.hover_views.len(), HOVER_PAGE_IDS.len());
    assert_eq!(s.hover_views[0].id, "notes");
    assert!(!s.hover_views[0].enabled);
}

#[test]
fn legacy_tools_group_migrates_into_widgets_and_is_idempotent() {
    let s = normalized(Settings {
        widgets: vec![
            WidgetSetting::new("media", true),
            WidgetSetting::new("tools", true),
            WidgetSetting::new("focus", true),
        ],
        tools: vec![
            WidgetSetting::new("notes", true),
            WidgetSetting::new("files", false),
        ],
        ..Settings::default()
    });
    assert_eq!(s.widgets[1].id, "notes");
    assert_eq!(s.widgets[2].id, "files");
    assert!(!s.tool_enabled("files"));
    assert!(s.tool_enabled("notes"));
    assert!(s.widgets.iter().all(|w| w.id != "tools"));
    let before: Vec<_> = s
        .widgets
        .iter()
        .map(|w| (w.id.clone(), w.enabled))
        .collect();
    let again = normalized(s);
    let after: Vec<_> = again
        .widgets
        .iter()
        .map(|w| (w.id.clone(), w.enabled))
        .collect();
    assert_eq!(before, after);
}

#[test]
fn tools_normalization_dedupes_and_fills() {
    let s = normalized(Settings {
        widgets: vec![WidgetSetting::new("tools", true)],
        tools: vec![
            WidgetSetting::new("files", false),
            WidgetSetting::new("files", true),
            WidgetSetting::new("bad", true),
        ],
        ..Settings::default()
    });
    assert_eq!(s.tools.len(), TOOL_NAMES.len());
    assert!(!s.tool_enabled("files") && s.tool_enabled("notes"));
    assert_eq!(s.hover_views.len(), HOVER_PAGE_IDS.len());
    let hover = |id: &str| s.hover_views.iter().find(|v| v.id == id).unwrap().enabled;
    assert!(!hover("clips") && !hover("tasks"));
}

#[test]
fn missing_widgets_get_the_c_sharp_enabled_defaults() {
    let s = normalized(Settings {
        widgets: vec![],
        ..Settings::default()
    });
    let en = |id: &str| s.widgets.iter().find(|w| w.id == id).unwrap().enabled;
    assert!(en("weather") && en("media") && en("clock"));
    assert!(!en("stopwatch") && !en("voice") && !en("notes") && !en("files"));
}

#[test]
fn hotkeys_colors_and_enums_fall_back() {
    let s = normalized(Settings {
        closed_media_content: "invalid".into(),
        closed_media_icon: "invalid".into(),
        category_style: "invalid".into(),
        launch_style: "Icons".into(),
        visibility_hotkey_key: "?".into(),
        visibility_hotkey_modifiers: "bad".into(),
        recent_colors: vec!["bad".into(), "#182028".into(), "#182028".into()],
        control_fill: "bad".into(),
        control_border: "bad".into(),
        selection_fill: "bad".into(),
        ..Settings::default()
    });
    assert_eq!(
        (
            s.closed_media_content.as_str(),
            s.closed_media_icon.as_str()
        ),
        ("Title", "Waveform")
    );
    assert_eq!(
        (s.category_style.as_str(), s.launch_style.as_str()),
        ("Dots", "Icons")
    );
    assert_eq!(
        (
            s.visibility_hotkey_key.as_str(),
            s.visibility_hotkey_modifiers.as_str()
        ),
        ("H", "Ctrl+Alt")
    );
    assert_eq!(s.recent_colors, ["#182028"]);
    assert_eq!(
        (
            s.control_fill.as_str(),
            s.control_border.as_str(),
            s.selection_fill.as_str()
        ),
        ("#16171B", "#303238", "#34373E")
    );
}

#[test]
fn media_ux_normalization() {
    let s = normalized(Settings {
        waveform_sensitivity: f64::NAN,
        lyrics_offset: 99.0,
        countdown_sound_volume: -1.0,
        countdown_sound: "bad".into(),
        waveform_source: "bad".into(),
        lyrics_animation: "bad".into(),
        top: 2900.0,
        ..Settings::default()
    });
    assert_eq!(s.waveform_sensitivity, 1.35);
    assert_eq!(s.lyrics_offset, 10.0);
    assert_eq!(s.countdown_sound_volume, 0.0);
    assert_eq!(s.countdown_sound, "Chime");
    assert_eq!(s.waveform_source, "Spectrum");
    assert_eq!(s.lyrics_animation, "Slide");
    assert_eq!(s.top, 2900.0);
    for mode in ["Title", "Waveform", "Lyrics", "None"] {
        let s = normalized(Settings {
            closed_media_content: mode.into(),
            closed_media_icon: "None".into(),
            ..Settings::default()
        });
        assert_eq!(s.closed_media_content, mode);
        assert_eq!(s.closed_media_icon, "None");
        assert!(s.media_integration_enabled);
    }
}

#[test]
fn premium_normalization_and_ambient_widgets() {
    let s = normalized(Settings {
        ambient_strength: f64::NAN,
        ambient_spread: 20.0,
        ambient_motion: "unknown".into(),
        interaction_motion: "unknown".into(),
        notification_seconds: f64::INFINITY,
        notification_animation: "unknown".into(),
        quiet_start: -1,
        quiet_end: 30,
        ..Settings::default()
    });
    assert_eq!((s.ambient_strength, s.ambient_spread), (0.5, 2.5));
    assert_eq!(
        (s.ambient_motion.as_str(), s.interaction_motion.as_str()),
        ("Drift", "Subtle")
    );
    assert_eq!(
        (s.notification_seconds, s.notification_animation.as_str()),
        (12.0, "Lift")
    );
    assert_eq!((s.quiet_start, s.quiet_end), (0, 23));

    let s = normalized(Settings {
        ambient_scope: "other".into(),
        ambient_widgets: ["notes", "notes", "unknown", "ai"]
            .map(String::from)
            .to_vec(),
        ambient_color_source: "other".into(),
        ambient_custom_color: "#GGGGGG".into(),
        ambient_audio_response: "other".into(),
        ambient_sensitivity: f64::NAN,
        ..Settings::default()
    });
    assert_eq!(s.ambient_scope, "All widgets");
    assert_eq!(s.ambient_widgets, ["notes", "ai"]);
    assert_eq!(s.ambient_color_source, "Album cover");
    assert_eq!(s.ambient_custom_color, "#7AA8DC");
    assert_eq!(s.ambient_audio_response, "Responsive");
    assert_eq!(s.ambient_sensitivity, 1.4);
}

#[test]
fn flexible_normalization_weather_and_dock() {
    let s = normalized(Settings {
        weather_city: format!("  {}  ", "x".repeat(200)),
        weather_latitude: Some(91.0),
        weather_longitude: Some(10.0),
        weather_units: "Kelvin".into(),
        dock_edge: "Middle".into(),
        follow_monitor_duration: 5.0,
        ..Settings::default()
    });
    assert_eq!(s.weather_city.len(), 120);
    assert_eq!((s.weather_latitude, s.weather_longitude), (None, None));
    assert_eq!(s.weather_units, "Celsius");
    assert_eq!(s.dock_edge, "Top");
    assert_eq!(s.follow_monitor_duration, 180.0);
    let ok = normalized(Settings {
        weather_latitude: Some(-6.2),
        weather_longitude: Some(106.8),
        ..Settings::default()
    });
    assert_eq!(ok.weather_latitude, Some(-6.2));
    let half = normalized(Settings {
        weather_latitude: Some(1.0),
        ..Settings::default()
    });
    assert_eq!(half.weather_latitude, None);
}

#[test]
fn language_and_accessibility() {
    let s = normalized(Settings {
        ui_language: "invalid".into(),
        accessibility_text_scale: f64::NAN,
        accessibility_high_contrast: true,
        ..Settings::default()
    });
    assert_eq!(
        (s.ui_language.as_str(), s.accessibility_text_scale),
        ("en", 1.0)
    );
    assert!(s.accessibility_high_contrast);
    for lang in ["es", "id", "en"] {
        let s = normalized(Settings {
            ui_language: lang.into(),
            accessibility_text_scale: 9.0,
            ..Settings::default()
        });
        assert_eq!(
            (s.ui_language.as_str(), s.accessibility_text_scale),
            (lang, 1.5)
        );
    }
}

#[test]
fn tuck_and_display_mode() {
    let mut s = Settings {
        tuck_edge: 1.0,
        ..Settings::default()
    };
    s.normalize();
    assert_eq!(s.tuck_edge, 1.0);
    s.tuck_edge = 99.0;
    s.normalize();
    assert_eq!(s.tuck_edge, 12.0);
    let legacy = normalized(Settings {
        monitor: "screen-B".into(),
        ..Settings::default()
    });
    assert_eq!(legacy.display_mode, "Selected");
    assert_eq!(normalized(Settings::default()).display_mode, "Primary");
}

#[test]
fn shortcuts_are_filtered_and_capped() {
    let mut shortcuts = vec![
        super::Shortcut {
            name: " ".into(),
            target: "x".into(),
            app_id: None,
        },
        super::Shortcut {
            name: "a".into(),
            target: "".into(),
            app_id: None,
        },
    ];
    for i in 0..50 {
        shortcuts.push(super::Shortcut {
            name: format!("n{i}"),
            target: "t".into(),
            app_id: None,
        });
    }
    let s = normalized(Settings {
        shortcuts,
        ..Settings::default()
    });
    assert_eq!(s.shortcuts.len(), 40);
    assert_eq!(s.shortcuts[0].name, "n0");
}

#[test]
fn recommended_preset_loads() {
    let s = presets::recommended();
    assert_eq!((s.width, s.height, s.radius), (400.0, 41.0, 13.0));
    assert_eq!(s.performance, "Maksimal");
    assert_eq!(
        (s.panel_width, s.panel_max_height, s.panel_scale),
        (526.0, 417.0, 1.04)
    );
    assert_eq!((s.hover_width, s.hover_height), (447.0, 207.0));
    assert!(!s.glass_style);
    assert_eq!(s.glass_strength, 0.99);
    assert_eq!(
        (s.category_style.as_str(), s.launch_style.as_str()),
        ("Dots", "Icons")
    );
    assert_eq!((s.animation_ms, s.wheel_action.as_str()), (400.0, "pages"));
    assert!(s.hide_maximized);
    assert_eq!(s.tuck_edge, 2.0);
    assert!(s.closed_ambient && s.ambient_scope == "Music only" && s.ambient_motion == "Breathe");
    assert_eq!(
        (
            s.closed_media_icon.as_str(),
            s.closed_media_content.as_str()
        ),
        ("Artwork", "Waveform")
    );
    assert!(!s.detachable_previews);
    assert_eq!(
        ids(&s.widgets).join(","),
        "media,volume,timer,focus,clock,weather,system,battery,shortcuts,stopwatch,files,notes,tasks,clips,calendar,awake,links,voice"
    );
    assert!(s.widgets.iter().all(|w| w.enabled != (w.id == "voice")));
    assert!(!s.widgets[17].enabled && s.widgets[17].id == "voice");
    assert!(s.auto_check_updates && !s.auto_download_updates);
    assert!(s.clipboard_history && s.auto_start && s.downloads_enabled);
    assert!(s.downloads_folder.is_empty() && s.monitor.is_empty() && s.recent_colors.is_empty());
    assert!(s.lyrics_enabled && s.notification_delivery == "Both");
    assert!(
        s.quiet_hours && (s.quiet_start, s.quiet_end) == (22, 7) && s.notification_seconds == 10.0
    );
    assert_eq!(s.shortcuts.len(), 2);
    assert_eq!(s.display_mode, "Primary");
    assert_eq!(s.weather_latitude, None);
    assert_eq!(
        hover_pages(&s),
        s.widgets
            .iter()
            .filter(|w| w.enabled && HOVER_PAGE_IDS.contains(&w.id.as_str()))
            .map(|w| w.id.clone())
            .collect::<Vec<_>>()
    );
    // Fresh copy every call.
    let mut changed = presets::recommended();
    changed.widgets.clear();
    assert_eq!(presets::recommended().widgets.len(), 18);
}

#[test]
fn midnight_notch_and_default_layout_apply() {
    let m = presets::midnight_notch();
    assert_eq!(m.preset, "Mac");
    assert!(m.width >= 180.0 && m.width <= 400.0);
    // Partial layout keeps everything it does not carry.
    assert_eq!(m.shortcuts.len(), 2);
    let d = presets::default_layout();
    assert_eq!(d.width, 400.0);
    assert_eq!(d.height, 41.0);
    assert!(!d.glass_style);
    assert!(!d.auto_start);
}

#[test]
fn roundtrip_preserves_unknown_keys() {
    let s: Settings = serde_json::from_str(r#"{"Width":300,"FutureKey":{"a":1}}"#).unwrap();
    assert_eq!(s.width, 300.0);
    assert_eq!(s.height, 32.0);
    let saved = s.to_json();
    let compact: Value = serde_json::from_str(&saved).unwrap();
    assert_eq!(compact["FutureKey"], json!({"a": 1}));
    assert!(serde_json::to_string(&s)
        .unwrap()
        .contains(r#""FutureKey":{"a":1}"#));
    let mut n = s;
    n.normalize();
    assert_eq!(n.extra["FutureKey"], json!({"a": 1}));
    assert_eq!(n.width, 300.0);
}

#[test]
fn csharp_written_document_loads_normalizes_and_saves_back() {
    // Shape System.Text.Json writes for 2.24 (null optionals, offset timestamp, extra keys).
    let doc = r#"{
      "Width": 401.5, "Preset": "iPhone", "WeatherLatitude": null, "WeatherCity": null,
      "HoverViews": [{"Id":"media","Enabled":false}, null],
      "Widgets": [{"Id":"media","Enabled":false}],
      "Shortcuts": [{"Name":"A","Target":"a.exe","AppId":null}],
      "NotificationsSnoozedUntil": "2026-10-29T12:30:45.1234567+07:00",
      "Assistant": {"Providers": [1,2,3]}, "SomeFuture": null, "HoverDelayMs": 200
    }"#;
    let mut s: Settings = serde_json::from_str(doc).unwrap();
    assert!(s.notifications_snoozed_until.is_some());
    s.normalize();
    assert_eq!(s.width, 400.0);
    assert_eq!(s.weather_city, "");
    assert!(!s.widgets[0].enabled);
    assert_eq!(s.widgets.len(), WIDGET_NAMES.len() - 1);
    assert_eq!(s.hover_views[0].id, "media");
    let saved: Value = serde_json::from_str(&s.to_json()).unwrap();
    assert_eq!(saved["Assistant"], json!({"Providers": [1, 2, 3]}));
    assert!(saved.get("SomeFuture").is_some());
    assert_eq!(saved["WeatherLatitude"], Value::Null);
    let again: Settings = serde_json::from_value(saved).unwrap();
    assert_eq!(again, s);
}

#[test]
fn non_object_values_do_not_panic() {
    assert!(serde_json::from_str::<Settings>("[]").is_err());
    assert!(serde_json::from_str::<Settings>("").is_err());
    assert!(serde_json::from_str::<Settings>(r#"{"Width":"wide"}"#).is_err());
}

#[test]
fn tool_enabled_prefers_widget_entry() {
    let mut s = Settings::default();
    assert!(s.tool_enabled("notes"));
    s.widgets
        .iter_mut()
        .find(|w| w.id == "notes")
        .unwrap()
        .enabled = false;
    assert!(!s.tool_enabled("notes"));
    assert!(s.widget_enabled("media") && !s.widget_enabled("nope"));
    // No entry: legacy group decides.
    s.widgets.retain(|w| w.id != "notes");
    assert!(!s.tool_enabled("notes"));
    s.widgets.push(WidgetSetting::new("tools", true));
    assert!(s.tool_enabled("notes"));
}

#[test]
fn hover_pages_follow_widgets_or_hover_views() {
    let mut s = normalized(Settings {
        hover_follows_widgets: false,
        ..Settings::default()
    });
    for v in &mut s.hover_views {
        v.enabled = true;
    }
    for w in &mut s.widgets {
        w.enabled = false;
    }
    let before: Vec<_> = s
        .widgets
        .iter()
        .map(|w| (w.id.clone(), w.enabled))
        .collect();
    assert_eq!(hover_pages(&s).len(), HOVER_PAGE_IDS.len());
    assert!(!s.clipboard_history);
    assert_eq!(
        before,
        s.widgets
            .iter()
            .map(|w| (w.id.clone(), w.enabled))
            .collect::<Vec<_>>()
    );
    for i in 0..s.tools.len() {
        let id = s.tools[i].id.clone();
        s.tools[i].enabled = false;
        // Widgets are all off, so the widget entry also reports the tool as disabled.
        let pages = hover_pages(&s);
        assert!(!pages.contains(&id));
        assert_eq!(pages.len(), HOVER_PAGE_IDS.len() - 1);
        s.tools[i].enabled = true;
    }
    s.hover_views
        .iter_mut()
        .find(|v| v.id == "system")
        .unwrap()
        .enabled = false;
    assert!(!hover_pages(&s).contains(&"system".to_string()));
    s.hover_views.reverse();
    assert_eq!(
        hover_pages(&s),
        s.hover_views
            .iter()
            .filter(|v| v.enabled)
            .map(|v| v.id.clone())
            .collect::<Vec<_>>()
    );
}

// ---- profile -----------------------------------------------------------------------------

#[test]
fn profile_export_excludes_private_values_and_roundtrips() {
    let mut original = Settings {
        panel_width: 520.0,
        panel_scale: 1.1,
        background: "#182028".into(),
        auto_start: true,
        clipboard_history: true,
        lyrics_enabled: true,
        downloads_folder: "C:/private-fixture".into(),
        countdown_sound_file: r"C:\private\sound.wav".into(),
        ..Settings::default()
    };
    original.shortcuts = vec![super::Shortcut {
        name: "Private".into(),
        target: "https://private.example".into(),
        app_id: Some("PrivateAppId".into()),
    }];
    original.hover_views.reverse();
    let encoded = profile::export(&original);
    for banned in [
        "Private",
        "private",
        "AutoStart",
        "ClipboardHistory",
        "LyricsEnabled",
        "DownloadsEnabled",
        "Hotkey",
        "AutoCheckUpdates",
    ] {
        assert!(!encoded.contains(banned), "{banned}");
    }
    let doc: Value = serde_json::from_str(&encoded).unwrap();
    assert_eq!(doc["format"], "luma.layout.v1");
    let mut target = Settings::default();
    profile::import_into(&encoded, &mut target).unwrap();
    assert_eq!((target.panel_width, target.panel_scale), (520.0, 1.1));
    assert_eq!(target.background, "#182028");
    assert_eq!(target.hover_views[0].id, "system");
    assert!(!target.auto_start && !target.clipboard_history);
    assert_eq!(target.shortcuts.len(), 2);
    let fresh = profile::import(&encoded).unwrap();
    assert_eq!(fresh.panel_width, 520.0);
}

#[test]
fn partial_profile_preserves_omitted_values_and_normalizes() {
    let mut target = Settings {
        width: 331.0,
        background: "#182028".into(),
        ..Settings::default()
    };
    profile::import_into(
        r#"{"format":"luma.layout.v1","values":{"PanelWidth":99999,"AutoStart":true}}"#,
        &mut target,
    )
    .unwrap();
    assert_eq!(target.panel_width, 960.0);
    assert_eq!(target.width, 331.0);
    assert_eq!(target.background, "#182028");
    assert!(!target.auto_start);
}

#[test]
fn invalid_profiles_do_not_mutate() {
    let mut target = Settings {
        width: 331.0,
        ..Settings::default()
    };
    let snapshot = target.clone();
    for (source, expect) in [
        (
            r#"{"format":"luma.layout.v1","values":{}}"#,
            ProfileError::NoValues,
        ),
        (
            r#"{"format":"other","values":{"Width":1}}"#,
            ProfileError::NotALayout,
        ),
        (r#"{"format":"luma.layout.v1"}"#, ProfileError::NotALayout),
        ("[]", ProfileError::NotALayout),
        (r#"{"format":5,"values":{}}"#, ProfileError::NotALayout),
    ] {
        assert_eq!(
            profile::import_into(source, &mut target),
            Err(expect),
            "{source}"
        );
    }
    assert!(matches!(
        profile::import_into("{", &mut target),
        Err(ProfileError::Invalid(_))
    ));
    assert!(matches!(
        profile::import_into(
            r#"{"format":"luma.layout.v1","values":{"Width":"x"}}"#,
            &mut target
        ),
        Err(ProfileError::Invalid(_))
    ));
    let huge = format!(
        r#"{{"format":"luma.layout.v1","values":{{"Monitor":"{}"}}}}"#,
        "a".repeat(262_144)
    );
    assert_eq!(
        profile::import_into(&huge, &mut target),
        Err(ProfileError::TooLarge)
    );
    assert_eq!(target, snapshot);
}

#[test]
fn profile_monitor_maps_display_mode() {
    let mut s = Settings::default();
    profile::import_into(
        r#"{"format":"luma.layout.v1","values":{"Monitor":"screen-C"}}"#,
        &mut s,
    )
    .unwrap();
    assert_eq!(
        (s.display_mode.as_str(), s.monitor.as_str()),
        ("Selected", "screen-C")
    );
    s.display_mode = "Follow".into();
    let exported = profile::export(&s);
    let mut other = Settings::default();
    profile::import_into(&exported, &mut other).unwrap();
    assert_eq!(other.display_mode, "Follow");
    profile::import_into(
        r#"{"format":"luma.layout.v1","values":{"Monitor":null}}"#,
        &mut s,
    )
    .unwrap();
    assert_eq!(
        (s.display_mode.as_str(), s.monitor.as_str()),
        ("Primary", "")
    );
}

#[test]
fn profile_keeps_existing_values_for_unlisted_fields_and_category_style() {
    let mut target = Settings {
        panel_width: 520.0,
        panel_max_height: 700.0,
        ..Settings::default()
    };
    profile::import_into(
        r#"{"format":"luma.layout.v1","values":{"CompactLayout":false}}"#,
        &mut target,
    )
    .unwrap();
    assert!(!target.compact_layout);
    assert_eq!(
        (target.panel_width, target.panel_max_height),
        (520.0, 700.0)
    );
    let mut imported = Settings::default();
    profile::import_into(
        r#"{"format":"luma.layout.v1","values":{"CategoryStyle":"Icons"}}"#,
        &mut imported,
    )
    .unwrap();
    assert_eq!(imported.category_style, "Dots");
}

#[test]
fn profile_material_and_glass_are_independent() {
    let settings = Settings {
        settings_material: "Native Acrylic".into(),
        glass_style: false,
        ..Settings::default()
    };
    let mut imported = Settings::default();
    profile::import_into(&profile::export(&settings), &mut imported).unwrap();
    assert_eq!(imported.settings_material, "Native Acrylic");
    assert!(!imported.glass_style);
    profile::import_into(
        r#"{"format":"luma.layout.v1","values":{"GlassStyle":true}}"#,
        &mut imported,
    )
    .unwrap();
    assert_eq!(imported.settings_material, "Native Acrylic");
    assert!(imported.glass_style);
    imported.settings_material = "invalid".into();
    imported.normalize();
    assert_eq!(imported.requested_backdrop(), "Solid");
    assert!(profile::CUSTOMIZATION_KEYS.contains(&"SettingsMaterial"));
    assert!(ResetCategory::Appearance
        .keys()
        .contains(&"SettingsMaterial"));
}

#[test]
fn profile_tools_and_premium_roundtrip() {
    let mut s = normalized(Settings {
        widgets: vec![WidgetSetting::new("tools", true)],
        tools: vec![WidgetSetting::new("files", false)],
        ..Settings::default()
    });
    s.ambient_strength = 0.82;
    s.ambient_motion = "Breathe".into();
    s.interaction_motion = "Expressive".into();
    s.quiet_hours = true;
    s.notifications_snoozed_until = Some(Utc::now().fixed_offset());
    let mut target = Settings::default();
    profile::import_into(&profile::export(&s), &mut target).unwrap();
    assert!(!target.tool_enabled("files") && target.tool_enabled("tasks"));
    assert_eq!(target.ambient_strength, 0.82);
    assert_eq!(target.ambient_motion, "Breathe");
    assert_eq!(target.interaction_motion, "Expressive");
    // Quiet hours and the snooze are not part of a layout profile.
    assert!(!target.quiet_hours && target.notifications_snoozed_until.is_none());
}

// ---- reset -------------------------------------------------------------------------------

#[test]
fn reset_keys_are_unique_and_cover_every_preference() {
    let all: Vec<&str> = ResetCategory::ALL
        .iter()
        .flat_map(|c| c.keys().iter().copied())
        .collect();
    let mut sorted = all.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), all.len(), "duplicate reset key");
    let excluded = ["AutoStart", "ClipboardHistory", "Shortcuts"];
    for key in keys_of(&Settings::default()) {
        assert!(
            excluded.contains(&key.as_str()) || all.contains(&key.as_str()),
            "{key} is in no reset category"
        );
    }
    assert!(ResetCategory::Ambience.keys().contains(&"AmbientWidgets"));
    assert!(PREMIUM_KEYS.contains(&"AmbientSensitivity"));
    assert_eq!(ResetCategory::from_name("AI"), Some(ResetCategory::Ai));
}

#[test]
fn reset_category_all_and_restore_preserve_capture_startup_and_shortcuts() {
    let mut settings = Settings {
        width: 333.0,
        accent: "#112233".into(),
        settings_material: "Native Mica".into(),
        auto_start: true,
        clipboard_history: true,
        shortcuts: vec![super::Shortcut {
            name: "Keep".into(),
            target: "external".into(),
            app_id: None,
        }],
        ..Settings::default()
    };
    let backup = settings.to_json();
    reset::reset_scope(
        &mut settings,
        ResetScope::Category(ResetCategory::Appearance),
    );
    assert_eq!(settings.width, 333.0);
    assert_eq!(settings.accent, "#FFFFFF");
    assert_eq!(settings.requested_backdrop(), "Solid");
    assert!(settings.auto_start && settings.clipboard_history);
    assert_eq!(settings.shortcuts[0].target, "external");

    reset::restore_backup(&mut settings, &backup).unwrap();
    assert_eq!(
        (settings.accent.as_str(), settings.width),
        ("#112233", 333.0)
    );
    assert_eq!(settings.settings_material, "Native Mica");

    reset::reset_scope(&mut settings, ResetScope::All);
    assert_eq!(
        (settings.width, settings.accent.as_str()),
        (400.0, "#FFFFFF")
    );
    assert!(settings.auto_start && settings.clipboard_history);
    assert_eq!(settings.shortcuts[0].target, "external");

    settings.clipboard_history = false;
    settings.auto_start = false;
    reset::restore_backup(&mut settings, &backup).unwrap();
    assert_eq!(settings.width, 333.0);
    // Restore never touches the unlisted startup flag, and clipboard history is excluded from All.
    assert!(!settings.clipboard_history && !settings.auto_start);
}

#[test]
fn reset_clipboard_category_resets_history_flag() {
    let mut s = Settings {
        clipboard_history: true,
        ..Settings::default()
    };
    reset::reset_scope(&mut s, ResetScope::Category(ResetCategory::Clipboard));
    assert!(
        s.clipboard_history,
        "factory defaults enable clipboard history"
    );
    let mut s = Settings::default();
    reset::reset_scope(&mut s, ResetScope::All);
    assert!(!s.clipboard_history);
}

#[test]
fn pre_material_backup_restores_solid_surface() {
    let mut old = serde_json::to_value(Settings {
        width: 333.0,
        ..Settings::default()
    })
    .unwrap();
    old.as_object_mut().unwrap().remove("SettingsMaterial");
    let mut s = Settings {
        settings_material: "Native Acrylic".into(),
        ..Settings::default()
    };
    reset::restore_backup(&mut s, &old.to_string()).unwrap();
    assert_eq!(s.width, 333.0);
    assert_eq!(s.requested_backdrop(), "Solid");
}

#[test]
fn invalid_backup_leaves_settings_intact() {
    let mut s = Settings {
        width: 333.0,
        ..Settings::default()
    };
    let before = s.clone();
    assert!(reset::restore_backup(&mut s, r#"{"Width":299}"#).is_err());
    assert!(reset::restore_backup(&mut s, "not json").is_err());
    assert!(reset::restore_backup(&mut s, "[]").is_err());
    assert_eq!(s, before);
    let mut nulled = serde_json::to_value(Settings::default()).unwrap();
    nulled["Radius"] = Value::Null;
    assert!(reset::restore_backup(&mut s, &nulled.to_string()).is_err());
    assert_eq!(s, before);
}

#[test]
fn reset_summary_reports_counts() {
    let text = reset::summary(ResetScope::Category(ResetCategory::Clipboard));
    assert!(text.starts_with(
        "Reset Clipboard to defaults (1 preferences): clipboard capture preferences."
    ));
    assert!(reset::summary(ResetScope::All).starts_with("Reset all preferences to defaults ("));
}

// ---- policies ----------------------------------------------------------------------------

#[test]
fn quiet_hours_window_and_snooze() {
    let at = |h: u32| (Utc.with_ymd_and_hms(2026, 10, 14, h, 0, 0).unwrap(), h);
    let mut s = Settings {
        quiet_hours: true,
        quiet_start: 22,
        quiet_end: 7,
        ..Settings::default()
    };
    let q = |s: &Settings, h: u32| {
        let (now, hour) = at(h);
        s.quiet_at(now, hour)
    };
    assert!(q(&s, 22) && q(&s, 0) && q(&s, 6) && !q(&s, 7) && !q(&s, 21));
    s.quiet_start = 9;
    s.quiet_end = 17;
    assert!(q(&s, 9) && !q(&s, 17) && !q(&s, 22));
    s.quiet_end = s.quiet_start;
    assert!(!q(&s, 9));
    let (now, hour) = at(12);
    s.notifications_snoozed_until = Some((now + chrono::Duration::hours(1)).fixed_offset());
    assert!(s.quiet_at(now, hour));
    s.notifications_snoozed_until = Some((now - chrono::Duration::hours(1)).fixed_offset());
    assert!(!s.quiet_at(now, hour));
}

#[test]
fn hotkey_conflicts() {
    let mut s = Settings::default();
    s.tuck_hotkey_key = "I".into();
    assert!(s.hotkey_conflict("Tuck").contains("Panel"));
    assert!(s.hotkey_conflict("Panel").contains("Tuck"));
    s.hotkey_enabled = false;
    s.tuck_hotkey_key = "H".into();
    assert!(s.hotkey_conflict("Tuck").contains("Visibility"));
    s.tuck_hotkey_enabled = false;
    assert_eq!(s.hotkey_conflict("Visibility"), "");
}

#[test]
fn backdrop_gates_and_native_errors_fail_closed() {
    let ready = BackdropConditions {
        supported: true,
        high_contrast: false,
        transparency: true,
        energy_saver: false,
        remote: false,
    };
    let mut calls = 0;
    for blocked in [
        BackdropConditions {
            supported: false,
            ..ready
        },
        BackdropConditions {
            high_contrast: true,
            ..ready
        },
        BackdropConditions {
            transparency: false,
            ..ready
        },
        BackdropConditions {
            energy_saver: true,
            ..ready
        },
        BackdropConditions {
            remote: true,
            ..ready
        },
    ] {
        let r = resolve_backdrop("Native Acrylic", blocked, |_| {
            calls += 1;
            0
        });
        assert!(!r.is_native() && r.actual == "Solid" && !r.reason.is_empty());
    }
    assert_eq!(calls, 0);
    for (name, ty) in [("Native Mica", 2), ("Native Acrylic", 3)] {
        let mut seen = 0;
        let ok = resolve_backdrop(name, ready, |t| {
            seen = t;
            0
        });
        assert!(ok.is_native() && seen == ty);
        let failure = resolve_backdrop(name, ready, |_| 0x8007_0057_u32 as i32);
        assert!(
            !failure.is_native()
                && failure.actual == "Solid"
                && failure.status().contains("80070057")
        );
    }
    assert_eq!(resolve_backdrop("Solid", ready, |_| 1).actual, "Solid");
    assert_eq!(
        resolve_backdrop("Simulated glass", ready, |_| 1).actual,
        "Simulated glass"
    );
    assert_eq!(resolve_backdrop("unknown", ready, |_| 1).actual, "Solid");
    assert_eq!(Settings::default().requested_backdrop(), "Solid");
}

// ---- welcome -----------------------------------------------------------------------------

#[test]
fn welcome_offer_logic() {
    let new = WelcomeState::default();
    let offer = welcome::should_offer(&new, false, false);
    assert!(offer.show && offer.save_pending);
    let pending = WelcomeState {
        pending: true,
        completed: false,
    };
    assert!(welcome::should_offer(&pending, true, false).show);
    assert!(!welcome::should_offer(&welcome::completed_state(), true, false).show);
    // Upgrades from an existing profile are not interrupted.
    assert!(!welcome::should_offer(&new, false, true).show);
    assert!(!welcome::should_offer(&new, true, false).show);
    let doc = serde_json::to_string(&welcome::completed_state()).unwrap();
    assert_eq!(doc, r#"{"Pending":false,"Completed":true}"#);
}

#[test]
fn welcome_preset_never_enables_mic_or_clipboard() {
    for preset in [
        WelcomePreset::Music,
        WelcomePreset::Productivity,
        WelcomePreset::Ai,
        WelcomePreset::Custom,
    ] {
        let mut s = presets::recommended();
        s.clipboard_history = false;
        s.downloads_enabled = false;
        s.lyrics_enabled = false;
        let before = (
            s.clipboard_history,
            s.downloads_enabled,
            s.auto_download_updates,
            s.reactive_feedback,
        );
        welcome::apply(&mut s, preset);
        assert_eq!(
            before,
            (
                s.clipboard_history,
                s.downloads_enabled,
                s.auto_download_updates,
                s.reactive_feedback
            )
        );
        assert!(s.extra.is_empty());
        assert!(!s.clipboard_history && !s.downloads_enabled);
    }
    let mut s = Settings::default();
    welcome::apply_with(&mut s, WelcomePreset::Music, Some("Left"));
    let on: Vec<&str> = s
        .widgets
        .iter()
        .filter(|w| w.enabled)
        .map(|w| w.id.as_str())
        .collect();
    assert_eq!(on, ["media", "volume", "clock", "weather"]);
    assert_eq!(s.dock_edge, "Left");
    assert!(s.hover_follows_widgets);
    let mut ai = Settings::default();
    welcome::apply(&mut ai, WelcomePreset::Ai);
    assert_eq!(ai.widgets.iter().filter(|w| w.enabled).count(), 3);
    assert!(ai.widget_enabled("voice"));
    let mut custom = Settings::default();
    welcome::apply(&mut custom, WelcomePreset::Custom);
    assert_eq!(custom, normalized(Settings::default()));
}
