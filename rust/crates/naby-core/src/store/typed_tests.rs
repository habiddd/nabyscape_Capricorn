//! Typed-document helpers on `Store` (settings, reset/restore, welcome, productivity, calendar,
//! weather, update state, assistant). Ports the Store+Settings scenarios of the C#
//! `PersistenceTests` and `ProductionFoundationTests` that need a real store directory.

use std::sync::Arc;

use chrono::DateTime;
use serde_json::{json, Value};

use super::{Store, StoreError};
use crate::assistant::models::Options;
use crate::productivity::calendar::CalendarEvent;
use crate::productivity::{ProductivityData, ProductivityService};
use crate::settings::reset::{self, PreferenceError, ResetCategory, ResetScope};
use crate::settings::welcome;
use crate::settings::{presets, Settings, SettingsLoad, Shortcut};
use crate::updates::UpdateState;
use crate::weather::WeatherSnapshot;

fn open() -> (tempfile::TempDir, Store) {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::open(tmp.path().to_path_buf());
    (tmp, store)
}

fn read(tmp: &tempfile::TempDir, name: &str) -> String {
    std::fs::read_to_string(tmp.path().join(name)).unwrap()
}

// ---- settings ----------------------------------------------------------------------------

#[test]
fn missing_settings_load_the_recommended_preset() {
    let (tmp, store) = open();
    let (settings, how) = store.load_settings();
    assert!(matches!(how, SettingsLoad::Missing));
    assert_eq!(settings, presets::recommended());
    assert!(
        settings.clipboard_history,
        "factory defaults enable clipboard history"
    );
    assert!(
        !tmp.path().join("settings.json").exists(),
        "loading never creates the file"
    );
}

#[test]
fn update_download_preference_matches_owner_snapshot_and_keeps_opt_out() {
    let (tmp, store) = open();
    assert!(
        !store.load_settings().0.auto_download_updates,
        "new profile"
    );
    store
        .save_settings(&Settings {
            auto_download_updates: false,
            ..Settings::default()
        })
        .unwrap();
    assert!(
        !store.load_settings().0.auto_download_updates,
        "existing opt-out"
    );
    std::fs::remove_file(tmp.path().join("settings.json")).unwrap();
    let _ = std::fs::remove_file(tmp.path().join("settings.schema.json"));
    std::fs::write(tmp.path().join("settings.json"), "{}").unwrap();
    assert!(
        !store.load_settings().0.auto_download_updates,
        "legacy profile"
    );
}

#[test]
fn save_then_load_settings_normalizes_and_roundtrips() {
    let (_tmp, store) = open();
    let mut settings = presets::recommended();
    settings.width = 333.0;
    settings.accent = "#112233".into();
    store.save_settings(&settings).unwrap();
    let (loaded, how) = store.load_settings();
    assert!(matches!(how, SettingsLoad::Loaded));
    assert_eq!(loaded.width, 333.0);
    assert_eq!(loaded.accent, "#112233");
    // Out-of-range values are clamped on the way in.
    store
        .save_settings(&Settings {
            width: 1.0e9,
            ..Settings::default()
        })
        .unwrap();
    assert!(store.load_settings().0.width < 1.0e9);
}

#[test]
fn future_schema_settings_fall_back_without_consent_and_are_not_overwritten() {
    let (tmp, store) = open();
    let original = r#"{"Width":333,"FutureContent":"preserve"}"#;
    let metadata = r#"{"SchemaVersion":999}"#;
    std::fs::write(tmp.path().join("settings.json"), original).unwrap();
    std::fs::write(tmp.path().join("settings.schema.json"), metadata).unwrap();
    let (fallback, how) = store.load_settings();
    assert!(matches!(
        how,
        SettingsLoad::Fallback(StoreError::FutureSchema(999))
    ));
    assert!(
        !fallback.clipboard_history && !fallback.downloads_enabled,
        "unreadable profiles must not enable background capture"
    );
    assert!(store.save_settings(&fallback).is_err());
    assert_eq!(read(&tmp, "settings.json"), original);
    assert_eq!(read(&tmp, "settings.schema.json"), metadata);
    assert_eq!(store.health().future_schema_documents, 1);
}

#[test]
fn corrupt_settings_stay_protected_until_explicit_successful_reload() {
    let (tmp, store) = open();
    let path = tmp.path().join("settings.json");
    std::fs::write(&path, "{broken").unwrap();
    let (fallback, how) = store.load_settings();
    assert!(matches!(how, SettingsLoad::Fallback(StoreError::Corrupt)));
    assert!(!fallback.clipboard_history);
    let repaired = serde_json::to_string(&Settings {
        width: 333.0,
        ..Settings::default()
    })
    .unwrap();
    std::fs::write(&path, &repaired).unwrap();
    assert!(store.save_settings(&fallback).is_err());
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        repaired,
        "fallback overwrote recovered content"
    );
    let (mut loaded, how) = store.load_settings();
    assert!(matches!(how, SettingsLoad::Loaded));
    assert_eq!(loaded.width, 333.0);
    loaded.width = 340.0;
    store.save_settings(&loaded).unwrap();
    assert_eq!(
        store.load_settings().0.width,
        340.0,
        "successful reload unblocks saves"
    );
}

#[test]
fn unknown_settings_keys_survive_save_settings() {
    let (tmp, store) = open();
    std::fs::write(
        tmp.path().join("settings.json"),
        r#"{"Width":333,"Later":[1]}"#,
    )
    .unwrap();
    let (settings, _) = store.load_settings();
    store.save_settings(&settings).unwrap();
    let raw: Value = serde_json::from_str(&read(&tmp, "settings.json")).unwrap();
    assert_eq!(raw["Later"], json!([1]));
}

#[test]
fn migration_validator_requires_settings_shape() {
    use std::fs;
    let root = tempfile::tempdir().unwrap();
    let legacy = root.path().join("LumaIsland");
    let next = root.path().join("NabyScape");
    fs::create_dir_all(&legacy).unwrap();
    // A Width that is not a number cannot deserialize to Settings, so the move is refused.
    Store::open(legacy.clone())
        .save_settings(&Settings::default())
        .unwrap();
    fs::write(legacy.join("settings.json"), r#"{"Width":"wide"}"#).unwrap();
    let store = Store::migrate(&legacy, &next);
    assert_eq!(store.dir(), legacy.as_path());
    assert!(store.warning().is_some());
    assert!(!next.exists());
}

// ---- reset / restore ---------------------------------------------------------------------

fn customized() -> Settings {
    Settings {
        width: 333.0,
        accent: "#112233".into(),
        settings_material: "Native Mica".into(),
        auto_start: true,
        clipboard_history: true,
        shortcuts: vec![Shortcut {
            name: "Keep".into(),
            target: "external".into(),
            app_id: None,
        }],
        ..Settings::default()
    }
}

#[test]
fn stored_reset_and_restore_preserve_capture_startup_shortcuts_and_personal_files() {
    let (tmp, store) = open();
    let mut settings = customized();
    store
        .save(
            "productivity",
            &ProductivityData {
                note: "keep".into(),
                ..ProductivityData::default()
            },
        )
        .unwrap();
    let before = read(&tmp, "productivity.json");

    reset::reset_stored(
        &store,
        &mut settings,
        ResetScope::Category(ResetCategory::Appearance),
    )
    .unwrap();
    assert_eq!(
        (settings.width, settings.accent.as_str()),
        (333.0, "#FFFFFF")
    );
    assert_eq!(settings.requested_backdrop(), "Solid");
    assert!(settings.auto_start && settings.clipboard_history);
    assert_eq!(settings.shortcuts[0].target, "external");
    assert_eq!(
        store.load_settings().0.accent,
        "#FFFFFF",
        "reset is persisted"
    );

    reset::restore_stored(&store, &mut settings).unwrap();
    assert_eq!(
        (settings.accent.as_str(), settings.width),
        ("#112233", 333.0)
    );
    assert_eq!(settings.settings_material, "Native Mica");
    assert_eq!(
        store.load_settings().0.accent,
        "#112233",
        "restore is persisted"
    );

    reset::reset_stored(&store, &mut settings, ResetScope::All).unwrap();
    assert_eq!(
        (settings.width, settings.accent.as_str()),
        (400.0, "#FFFFFF")
    );
    assert!(settings.auto_start && settings.clipboard_history);
    assert_eq!(settings.shortcuts[0].target, "external");
    assert_eq!(before, read(&tmp, "productivity.json"));

    settings.clipboard_history = false;
    settings.auto_start = false;
    reset::restore_stored(&store, &mut settings).unwrap();
    assert!(settings.width == 333.0 && !settings.clipboard_history && !settings.auto_start);
}

#[test]
fn restore_accepts_nullable_location_and_preserves_capture_consent() {
    let (tmp, store) = open();
    let mut settings = Settings {
        width: 333.0,
        weather_latitude: None,
        weather_longitude: None,
        clipboard_history: true,
        ui_language: "es".into(),
        auto_download_updates: false,
        ..Settings::default()
    };
    reset::reset_stored(&store, &mut settings, ResetScope::All).unwrap();
    assert!(settings.clipboard_history && settings.ui_language == "en");
    assert!(!settings.auto_download_updates);
    settings.clipboard_history = false;
    reset::restore_stored(&store, &mut settings).unwrap();
    assert!(!settings.clipboard_history && settings.width == 333.0);
    assert!(settings.weather_latitude.is_none() && settings.weather_longitude.is_none());
    assert_eq!(settings.ui_language, "es");
    assert!(!settings.auto_download_updates);

    let mut backup: Value = serde_json::from_str(&read(&tmp, "preference-backup.json")).unwrap();
    backup.as_object_mut().unwrap().remove("WeatherLatitude");
    std::fs::write(
        tmp.path().join("preference-backup.json"),
        backup.to_string(),
    )
    .unwrap();
    assert!(matches!(
        reset::restore_stored(&store, &mut settings),
        Err(PreferenceError::Invalid(_))
    ));
    assert_eq!(settings.width, 333.0);
}

#[test]
fn pre_material_backup_restores_the_solid_settings_surface() {
    let (tmp, store) = open();
    let mut old: Value = serde_json::to_value(Settings {
        width: 333.0,
        ..Settings::default()
    })
    .unwrap();
    old.as_object_mut().unwrap().remove("SettingsMaterial");
    std::fs::write(tmp.path().join("preference-backup.json"), old.to_string()).unwrap();
    let mut settings = Settings {
        settings_material: "Native Acrylic".into(),
        ..Settings::default()
    };
    reset::restore_stored(&store, &mut settings).unwrap();
    assert_eq!(settings.width, 333.0);
    assert_eq!(settings.requested_backdrop(), "Solid");
}

#[test]
fn backup_save_errors_abort_reset_and_invalid_restore_leaves_settings_intact() {
    let (tmp, store) = open();
    let mut settings = Settings {
        width: 333.0,
        ..Settings::default()
    };
    store.save_settings(&settings).unwrap();
    let before = read(&tmp, "settings.json");
    // A directory where the backup file belongs makes the backup write fail.
    std::fs::create_dir(tmp.path().join("preference-backup.json")).unwrap();
    assert!(reset::reset_stored(&store, &mut settings, ResetScope::All).is_err());
    assert_eq!(settings.width, 333.0);
    assert_eq!(before, read(&tmp, "settings.json"));

    std::fs::remove_dir(tmp.path().join("preference-backup.json")).unwrap();
    std::fs::write(
        tmp.path().join("preference-backup.json"),
        r#"{"Width":299}"#,
    )
    .unwrap();
    assert!(matches!(
        reset::restore_stored(&store, &mut settings),
        Err(PreferenceError::Invalid(_))
    ));
    assert_eq!(settings.width, 333.0);
    assert_eq!(before, read(&tmp, "settings.json"));
    // No backup at all is an I/O error, not a panic.
    std::fs::remove_file(tmp.path().join("preference-backup.json")).unwrap();
    assert!(matches!(
        reset::restore_stored(&store, &mut settings),
        Err(PreferenceError::Io(_))
    ));
}

// ---- welcome -----------------------------------------------------------------------------

#[test]
fn welcome_offers_new_profiles_and_remembers_skip() {
    let (tmp, store) = open();
    assert!(
        welcome::should_offer_stored(&store, false),
        "new profile receives setup"
    );
    let pending: Value = serde_json::from_str(&read(&tmp, "welcome.json")).unwrap();
    assert_eq!(pending, json!({"Pending": true, "Completed": false}));
    assert!(
        welcome::should_offer_stored(&store, true),
        "unfinished setup resumes"
    );
    welcome::complete(&store).unwrap();
    assert!(
        !welcome::should_offer_stored(&store, false),
        "completed setup stays closed"
    );
}

#[test]
fn welcome_preserves_existing_profiles() {
    let (tmp, store) = open();
    store
        .save_settings(&Settings {
            width: 333.0,
            ..Settings::default()
        })
        .unwrap();
    let before = read(&tmp, "settings.json");
    assert!(
        !welcome::should_offer_stored(&store, true),
        "upgrade shows no setup"
    );
    welcome::complete(&store).unwrap();
    assert_eq!(
        before,
        read(&tmp, "settings.json"),
        "setup marker changed preferences"
    );
}

// ---- productivity, calendar, weather, updates, assistant ---------------------------------

#[test]
fn productivity_service_opens_from_and_saves_to_the_store() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(tmp.path().to_path_buf()));
    store
        .save(
            "productivity",
            &ProductivityData {
                note: "kept".into(),
                ..ProductivityData::default()
            },
        )
        .unwrap();
    let mut service = ProductivityService::open(&store);
    assert_eq!(service.to_data().note, "kept");
    assert!(service.save());
    // Unreadable documents are protected, not replaced.
    let broken = tempfile::tempdir().unwrap();
    std::fs::write(broken.path().join("productivity.json"), "{nope").unwrap();
    let store = Arc::new(Store::open(broken.path().to_path_buf()));
    let mut service = ProductivityService::open(&store);
    assert_eq!(service.to_data(), ProductivityData::default());
    assert!(!service.save());
    assert_eq!(
        std::fs::read_to_string(broken.path().join("productivity.json")).unwrap(),
        "{nope"
    );
}

fn ts(text: &str) -> DateTime<chrono::FixedOffset> {
    DateTime::parse_from_rfc3339(text).unwrap()
}

#[test]
fn calendar_document_roundtrips_and_filters_invalid_entries() {
    let (tmp, store) = open();
    assert!(store.load_calendar().events().is_empty());
    let event = CalendarEvent {
        id: "a".repeat(32),
        title: "Stand-up".into(),
        start: ts("2026-10-29T09:00:00+07:00"),
        end: ts("2026-10-29T09:30:00+07:00"),
        meeting_url: String::new(),
    };
    store.save_calendar(std::slice::from_ref(&event)).unwrap();
    assert_eq!(store.load_calendar().events(), [event]);
    std::fs::write(
        tmp.path().join("calendar.json"),
        r#"[null,{"Id":"x","Title":"","Start":"2026-10-29T09:00:00Z","End":"2026-10-29T10:00:00Z","MeetingUrl":""}]"#,
    )
    .unwrap();
    assert!(
        store.load_calendar().events().is_empty(),
        "invalid entries are excluded"
    );
}

#[test]
fn weather_and_update_state_documents_roundtrip() {
    let (_tmp, store) = open();
    assert!(store.load_weather_cache().is_none());
    let snapshot = WeatherSnapshot {
        key: "Hanoi".into(),
        temperature: 21.5,
        ..WeatherSnapshot::default()
    };
    store.save_weather_cache(&snapshot).unwrap();
    assert_eq!(store.load_weather_cache(), Some(snapshot));

    assert_eq!(store.load_update_state(), UpdateState::default());
    let state = UpdateState {
        last_attempt: "2026-10-29T10:00:00Z".parse().unwrap(),
    };
    store.save_update_state(&state).unwrap();
    assert_eq!(store.load_update_state(), state);
}

#[test]
fn assistant_options_use_from_json_rules() {
    let (tmp, store) = open();
    assert_eq!(store.load_assistant_options(), Options::default());
    let text = r#"{"Enabled":null,"Instructions":null,"Providers":[{"Id":"0123456789abcdef0123456789abcdef"},{"Id":"0123456789abcdef0123456789abcdef"},{"Id":"bad id!"}],"FromTheFuture":1}"#;
    std::fs::write(tmp.path().join("assistant.json"), text).unwrap();
    let loaded = store.load_assistant_options();
    assert_eq!(loaded, Options::from_json(text.as_bytes()).unwrap());
    assert!(loaded.enabled, "null booleans default to true");
    assert_eq!(
        loaded.providers.len(),
        1,
        "duplicate and invalid ids are dropped"
    );
    store.save_assistant_options(&loaded).unwrap();
    let raw: Value = serde_json::from_str(&read(&tmp, "assistant.json")).unwrap();
    assert_eq!(raw["FromTheFuture"], json!(1));
}
