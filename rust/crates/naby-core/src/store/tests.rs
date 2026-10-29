use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::*;
use crate::session::SessionState;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
struct Prefs {
    width: i32,
    compact_layout: bool,
    panel_width: i32,
    shortcuts: Vec<String>,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            width: 400,
            compact_layout: true,
            panel_width: 0,
            shortcuts: vec![],
        }
    }
}

fn prefs(width: i32) -> Prefs {
    Prefs {
        width,
        ..Prefs::default()
    }
}

fn read(dir: &Path, file: &str) -> String {
    fs::read_to_string(dir.join(file)).unwrap()
}

fn open(dir: &Path) -> Store {
    Store::open(dir.to_path_buf())
}

#[test]
fn rejects_unsafe_names() {
    let tmp = tempfile::tempdir().unwrap();
    let store = open(tmp.path());
    let long = "a".repeat(65);
    for bad in [
        "CON", "con", "NuL", "COM1", "lpt9", "../x", "a.b", "a/b", "a\\b", "", "-a", "_a", "a:b",
        "a b", &long,
    ] {
        assert!(
            matches!(store.load::<Value>(bad), Err(StoreError::InvalidName)),
            "{bad}"
        );
        assert!(
            matches!(store.save(bad, &json!({})), Err(StoreError::InvalidName)),
            "{bad}"
        );
    }
    let ok = "a".repeat(64);
    for good in [
        "settings",
        "update-state",
        "a_b",
        "9lives",
        ok.as_str(),
        "COM10",
        "CONSOLE",
    ] {
        assert!(store.load::<Value>(good).is_ok(), "{good}");
    }
}

#[test]
fn missing_document_is_none_and_not_created() {
    let tmp = tempfile::tempdir().unwrap();
    let store = open(tmp.path());
    assert!(store.load::<Prefs>("settings").unwrap().is_none());
    assert!(!tmp.path().join("settings.json").exists());
    assert_eq!(
        store.document_state("settings"),
        Some(DocumentState::Missing)
    );
}

#[test]
fn save_then_load_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    let store = open(tmp.path());
    store.save("settings", &prefs(333)).unwrap();
    assert_eq!(store.load::<Prefs>("settings").unwrap().unwrap().width, 333);
    assert!(read(tmp.path(), "settings.json").contains("\"Width\": 333"));
    let schema: Value = serde_json::from_str(&read(tmp.path(), "settings.schema.json")).unwrap();
    assert_eq!(schema, json!({"SchemaVersion": 1}));
    // A second store instance (restart) sees the same data.
    let again = open(tmp.path());
    assert_eq!(again.load::<Prefs>("settings").unwrap().unwrap().width, 333);
    assert_eq!(again.health().current_documents, 1);
}

#[test]
fn second_save_keeps_last_good_backup() {
    let tmp = tempfile::tempdir().unwrap();
    let store = open(tmp.path());
    store.save("settings", &prefs(300)).unwrap();
    store.save("settings", &prefs(310)).unwrap();
    assert!(read(tmp.path(), "settings.json.last-good.bak").contains("300"));
    assert!(read(tmp.path(), "settings.json").contains("310"));
    let leftovers = fs::read_dir(tmp.path())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().contains(".tmp-"))
        .count();
    assert_eq!(leftovers, 0);
}

#[test]
fn unknown_fields_survive_save() {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(
        tmp.path().join("settings.json"),
        r#"{"Width":300,"FutureKey":{"a":1},"Shortcuts":["x"]}"#,
    )
    .unwrap();
    let store = open(tmp.path());
    let mut p: Prefs = store.load("settings").unwrap().unwrap();
    assert_eq!(p.width, 300);
    p.width = 301;
    p.shortcuts.clear(); // deliberate removal replaces the array
    store.save("settings", &p).unwrap();
    let saved: Value = serde_json::from_str(&read(tmp.path(), "settings.json")).unwrap();
    assert_eq!(saved["FutureKey"], json!({"a": 1}));
    assert_eq!(saved["Width"], json!(301));
    assert_eq!(saved["Shortcuts"], json!([]));
}

#[test]
fn removed_map_entries_stay_removed() {
    let tmp = tempfile::tempdir().unwrap();
    let store = open(tmp.path());
    fs::write(
        tmp.path().join("activities.json"),
        r#"{"a":{"keep":1},"b":2}"#,
    )
    .unwrap();
    let mut map: std::collections::BTreeMap<String, Value> =
        store.load("activities").unwrap().unwrap();
    map.remove("b");
    store.save("activities", &map).unwrap();
    let saved: Value = serde_json::from_str(&read(tmp.path(), "activities.json")).unwrap();
    assert_eq!(saved, json!({"a": {"keep": 1}}));
}

#[test]
fn schema_zero_migration_is_explicit_idempotent_and_backed_up() {
    let tmp = tempfile::tempdir().unwrap();
    let original = r#"{"Width":301,"CompactLayout":false,"RendererMode":"Old","Extra":true}"#;
    fs::write(tmp.path().join("settings.json"), original).unwrap();
    let store = open(tmp.path());
    let loaded: Prefs = store.load("settings").unwrap().unwrap();
    assert_eq!(loaded.panel_width, 452);
    assert_eq!(
        store.document_state("settings"),
        Some(DocumentState::Legacy)
    );
    assert_eq!(store.health().legacy_documents, 1);
    // Loading never rewrites.
    assert_eq!(read(tmp.path(), "settings.json"), original);
    assert!(!tmp.path().join("settings.schema.json").exists());
    store.save("settings", &loaded).unwrap();
    assert_eq!(read(tmp.path(), "settings.json.migration-v0.bak"), original);
    let saved: Value = serde_json::from_str(&read(tmp.path(), "settings.json")).unwrap();
    assert_eq!(saved["PanelWidth"], json!(452));
    assert_eq!(saved["Extra"], json!(true));
    assert!(saved.get("RendererMode").is_none());
    // The migration backup is immutable.
    store.save("settings", &prefs(5)).unwrap();
    assert_eq!(read(tmp.path(), "settings.json.migration-v0.bak"), original);
    assert_eq!(
        store.document_state("settings"),
        Some(DocumentState::Current)
    );
}

#[test]
fn utf8_bom_is_accepted() {
    let tmp = tempfile::tempdir().unwrap();
    let mut bytes = vec![0xef, 0xbb, 0xbf];
    bytes.extend_from_slice(br#"{"Width":123}"#);
    fs::write(tmp.path().join("settings.json"), bytes).unwrap();
    assert_eq!(
        open(tmp.path())
            .load::<Prefs>("settings")
            .unwrap()
            .unwrap()
            .width,
        123
    );
}

#[test]
fn corrupt_document_is_not_overwritten() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("settings.json");
    fs::write(&path, "{broken").unwrap();
    let store = open(tmp.path());
    assert!(matches!(
        store.load::<Prefs>("settings"),
        Err(StoreError::Corrupt)
    ));
    assert_eq!(
        store.document_state("settings"),
        Some(DocumentState::RecoveryRequired)
    );
    assert!(store.warning().is_some());
    assert!(matches!(
        store.save("settings", &prefs(1)),
        Err(StoreError::RecoveryRequired)
    ));
    assert_eq!(fs::read_to_string(&path).unwrap(), "{broken");
    assert_eq!(read(tmp.path(), "settings.json.backup-original"), "{broken");
    assert_eq!(store.health().recovery_required_documents, 1);
}

#[test]
fn corrupt_load_stays_protected_until_explicit_successful_reload() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("settings.json");
    fs::write(&path, "{broken").unwrap();
    let store = open(tmp.path());
    assert!(store.load::<Prefs>("settings").is_err());
    let repaired = serde_json::to_string(&prefs(333)).unwrap();
    fs::write(&path, &repaired).unwrap();
    // Externally repaired content is still not overwritten by the earlier fallback.
    assert!(matches!(
        store.save("settings", &prefs(400)),
        Err(StoreError::RecoveryRequired)
    ));
    assert_eq!(fs::read_to_string(&path).unwrap(), repaired);
    let mut loaded: Prefs = store.load("settings").unwrap().unwrap();
    assert_eq!(loaded.width, 333);
    loaded.width = 340;
    store.save("settings", &loaded).unwrap();
    assert_eq!(store.load::<Prefs>("settings").unwrap().unwrap().width, 340);
}

#[test]
fn wrong_shape_and_invalid_utf8_are_corrupt() {
    let tmp = tempfile::tempdir().unwrap();
    let store = open(tmp.path());
    let cases: [&[u8]; 6] = [
        b"[1,2]",
        b"\"text\"",
        b"null",
        b"",
        b"\xff\xfe{}",
        br#"{"Width":"wide"}"#,
    ];
    for (i, bytes) in cases.iter().enumerate() {
        let name = format!("doc{i}");
        fs::write(tmp.path().join(format!("{name}.json")), bytes).unwrap();
        assert!(
            matches!(store.load::<Prefs>(&name), Err(StoreError::Corrupt)),
            "{i}"
        );
    }
    // An unreadable existing payload also blocks a save that was not preceded by a load.
    let fresh = open(tmp.path());
    assert!(matches!(
        fresh.save("doc3", &prefs(1)),
        Err(StoreError::Corrupt)
    ));
    assert_eq!(read(tmp.path(), "doc3.json"), "");
}

#[test]
fn directory_in_payload_slot_is_corrupt() {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir(tmp.path().join("settings.json")).unwrap();
    assert!(matches!(
        open(tmp.path()).load::<Prefs>("settings"),
        Err(StoreError::Corrupt)
    ));
}

#[test]
fn future_schema_is_read_only() {
    let tmp = tempfile::tempdir().unwrap();
    let payload = tmp.path().join("settings.json");
    let schema = tmp.path().join("settings.schema.json");
    let original = r#"{"Width":333,"FutureContent":"preserve"}"#;
    let metadata = r#"{"SchemaVersion":999}"#;
    fs::write(&payload, original).unwrap();
    fs::write(&schema, metadata).unwrap();
    let store = open(tmp.path());
    assert!(matches!(
        store.load::<Prefs>("settings"),
        Err(StoreError::FutureSchema(999))
    ));
    assert!(matches!(
        store.save("settings", &prefs(400)),
        Err(StoreError::FutureSchema(999))
    ));
    assert_eq!(fs::read_to_string(&payload).unwrap(), original);
    assert_eq!(fs::read_to_string(&schema).unwrap(), metadata);
    assert_eq!(store.health().future_schema_documents, 1);
    assert!(store.warning().unwrap().contains("newer settings schema"));
    // A fresh store that never loaded is blocked on save too.
    let blind = open(tmp.path());
    assert!(matches!(
        blind.save("settings", &prefs(400)),
        Err(StoreError::FutureSchema(999))
    ));
    assert_eq!(fs::read_to_string(&payload).unwrap(), original);
}

#[test]
fn pending_future_schema_barrier_blocks_even_without_payload() {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(
        tmp.path().join("x.schema.json"),
        r#"{"SchemaVersion":1,"PendingSchemaVersion":2}"#,
    )
    .unwrap();
    let store = open(tmp.path());
    assert!(matches!(
        store.save("x", &prefs(1)),
        Err(StoreError::FutureSchema(2))
    ));
    assert!(!tmp.path().join("x.json").exists());
}

#[test]
fn malformed_sidecar_is_corrupt_and_left_alone() {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(tmp.path().join("x.json"), "{}").unwrap();
    for bad in [
        "{\"SchemaVersion\":-1}",
        "{}",
        "[1]",
        "{\"SchemaVersion\":1,\"PendingSchemaVersion\":0}",
        "nope",
    ] {
        fs::write(tmp.path().join("x.schema.json"), bad).unwrap();
        let store = open(tmp.path());
        assert!(
            matches!(store.load::<Value>("x"), Err(StoreError::Corrupt)),
            "{bad}"
        );
        assert_eq!(read(tmp.path(), "x.schema.json"), bad);
        assert!(!tmp.path().join("x.json.backup-original").exists(), "{bad}");
    }
}

#[test]
fn pending_equal_to_current_is_finalized_by_next_save() {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(tmp.path().join("x.json"), "{\"Width\":5}").unwrap();
    fs::write(
        tmp.path().join("x.schema.json"),
        r#"{"SchemaVersion":0,"PendingSchemaVersion":1}"#,
    )
    .unwrap();
    let store = open(tmp.path());
    let p: Prefs = store.load("x").unwrap().unwrap();
    store.save("x", &p).unwrap();
    let schema: Value = serde_json::from_str(&read(tmp.path(), "x.schema.json")).unwrap();
    assert_eq!(schema, json!({"SchemaVersion": 1}));
}

#[test]
fn oversized_document_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("big.json");
    let f = fs::File::create(&path).unwrap();
    f.set_len(MAX_DOCUMENT_BYTES as u64 + 1).unwrap();
    drop(f);
    let store = open(tmp.path());
    assert!(matches!(
        store.load::<Value>("big"),
        Err(StoreError::TooLarge)
    ));
    assert_eq!(
        fs::metadata(&path).unwrap().len(),
        MAX_DOCUMENT_BYTES as u64 + 1
    );
    assert!(matches!(
        store.save("big", &json!({})),
        Err(StoreError::RecoveryRequired)
    ));
}

#[test]
fn oversized_save_is_rejected_without_touching_files() {
    let tmp = tempfile::tempdir().unwrap();
    let store = open(tmp.path());
    store.save("notes", &json!({"Note": "keep"})).unwrap();
    let before = read(tmp.path(), "notes.json");
    let huge = json!({"Note": "x".repeat(MAX_DOCUMENT_BYTES + 1)});
    assert!(matches!(
        store.save("notes", &huge),
        Err(StoreError::TooLarge)
    ));
    assert_eq!(read(tmp.path(), "notes.json"), before);
    // A failed save does not latch; a retry with valid data succeeds.
    store.save("notes", &json!({"Note": "next"})).unwrap();
    assert!(read(tmp.path(), "notes.json").contains("next"));
}

#[test]
fn state_names_are_case_insensitive() {
    let tmp = tempfile::tempdir().unwrap();
    let store = open(tmp.path());
    store.save("Settings", &prefs(1)).unwrap();
    store.load::<Prefs>("settings").unwrap();
    assert_eq!(store.health().observed_documents, 1);
}

#[test]
fn log_appends_to_errors_log() {
    let tmp = tempfile::tempdir().unwrap();
    let store = open(tmp.path());
    store.log(&"first");
    store.log(&"second");
    let text = read(tmp.path(), "errors.log");
    assert_eq!(text.lines().count(), 2);
    assert!(text.contains("second"));
}

// ---- migration (PersistenceTests.cs) ----

fn legacy_and_next(root: &Path) -> (PathBuf, PathBuf) {
    (root.join("LumaIsland"), root.join("NabyScape"))
}

#[test]
fn legacy_lumaisland_folder_migrates_once() {
    let tmp = tempfile::tempdir().unwrap();
    let (legacy, next) = legacy_and_next(tmp.path());
    let old = Store::open(legacy.clone());
    let images = legacy.join("clipboard-images");
    fs::create_dir_all(&images).unwrap();
    let image = images.join("test.png");
    fs::write(&image, [1, 2, 3]).unwrap();
    let outside = tmp.path().join("outside.png");
    old.save("settings", &json!({"Width": 311, "AutoStart": true, "Shortcuts": [{"Name": "External", "Target": image}]})).unwrap();
    old.save(
        "productivity",
        &json!({"Note": "private", "Tasks": [{"Text": "keep"}], "Shelf": [{"Path": image}],
            "Clips": [{"Kind": "image", "ImagePath": image}, {"Kind": "image", "ImagePath": outside}]}),
    )
    .unwrap();
    old.save("activities", &json!({"Activities": [{"Name": "Keep"}]}))
        .unwrap();

    let migrated = Store::migrate(&legacy, &next);
    assert_eq!(migrated.dir(), next);
    assert!(legacy.exists() && image.exists());
    assert!(migrated.warning().unwrap().contains("moved to NabyScape"));
    let settings: Value = migrated.load("settings").unwrap().unwrap();
    assert_eq!(settings["Width"], json!(311));
    let data: Value = migrated.load("productivity").unwrap().unwrap();
    let rebased = next.join("clipboard-images").join("test.png");
    assert_eq!(data["Clips"][0]["ImagePath"], json!(rebased));
    assert_eq!(data["Clips"][1]["ImagePath"], json!(outside));
    assert_eq!(data["Shelf"][0]["Path"], json!(image));
    assert_eq!(settings["Shortcuts"][0]["Target"], json!(image));
    assert_eq!(data["Note"], json!("private"));
    assert_eq!(
        read(&next, "activities.json"),
        read(&legacy, "activities.json")
    );
    assert!(rebased.exists());

    // Second run: destination exists, so nothing is merged or moved again.
    let again = Store::migrate(&legacy, &next);
    assert_eq!(again.dir(), next);
    assert!(again.warning().unwrap().contains("Both"));
}

#[test]
fn migration_ignores_interrupted_staging_and_never_merges_existing_destination() {
    let tmp = tempfile::tempdir().unwrap();
    let (legacy, next) = (tmp.path().join("legacy"), tmp.path().join("next"));
    Store::open(legacy.clone())
        .save("settings", &json!({"Width": 301}))
        .unwrap();
    let stale = tmp.path().join("next.migration-interrupted");
    fs::create_dir_all(&stale).unwrap();
    fs::write(stale.join("settings.json"), "{broken").unwrap();
    let migrated = Store::migrate(&legacy, &next);
    assert_eq!(
        migrated.load::<Value>("settings").unwrap().unwrap()["Width"],
        json!(301)
    );
    Store::open(next.clone())
        .save("settings", &json!({"Width": 322}))
        .unwrap();
    let existing = Store::migrate(&legacy, &next);
    assert_eq!(
        existing.load::<Value>("settings").unwrap().unwrap()["Width"],
        json!(322)
    );
    assert!(existing.warning().unwrap().contains("Both"));
}

#[test]
fn migration_invalid_json_or_blocked_destination_falls_back_with_warning() {
    let tmp = tempfile::tempdir().unwrap();
    let (legacy, next) = (tmp.path().join("legacy"), tmp.path().join("next"));
    Store::open(legacy.clone())
        .save("settings", &json!({"Width": 301}))
        .unwrap();

    fs::write(legacy.join("broken.json"), "{broken").unwrap();
    let failed = Store::migrate(&legacy, &next);
    assert_eq!(failed.dir(), legacy);
    assert!(failed.warning().is_some() && !next.exists());
    assert_eq!(
        failed.load::<Value>("settings").unwrap().unwrap()["Width"],
        json!(301)
    );
    fs::remove_file(legacy.join("broken.json")).unwrap();

    fs::write(legacy.join("productivity.json"), r#"{"Tasks":"invalid"}"#).unwrap();
    let failed = Store::migrate(&legacy, &next);
    assert_eq!(failed.dir(), legacy);
    assert!(failed.warning().is_some() && !next.exists());
    fs::remove_file(legacy.join("productivity.json")).unwrap();

    fs::write(legacy.join("session.json"), r#"{"Timer":"nope"}"#).unwrap();
    assert_eq!(Store::migrate(&legacy, &next).dir(), legacy);
    fs::remove_file(legacy.join("session.json")).unwrap();

    // Blocked destination: a file sits where the new folder must go.
    fs::write(&next, "blocked").unwrap();
    let failed = Store::migrate(&legacy, &next);
    assert_eq!(failed.dir(), legacy);
    assert!(failed.warning().unwrap().contains("could not complete"));
    assert_eq!(
        failed.load::<Value>("settings").unwrap().unwrap()["Width"],
        json!(301)
    );
    // No staging folders are left behind.
    let staged = fs::read_dir(tmp.path())
        .unwrap()
        .filter_map(Result::ok)
        .any(|e| e.file_name().to_string_lossy().contains(".migration-"));
    assert!(!staged);
}

#[test]
fn migration_refuses_missing_clipboard_image_and_readonly_files() {
    let tmp = tempfile::tempdir().unwrap();
    let (legacy, next) = (tmp.path().join("legacy"), tmp.path().join("next"));
    let old = Store::open(legacy.clone());
    let missing = legacy.join("clipboard-images").join("gone.png");
    old.save(
        "productivity",
        &json!({"Clips": [{"Kind": "image", "ImagePath": missing}]}),
    )
    .unwrap();
    assert_eq!(Store::migrate(&legacy, &next).dir(), legacy);
    assert!(!next.exists());

    old.save("productivity", &json!({"Note": "ok"})).unwrap();
    old.save("settings", &json!({"Width": 1})).unwrap();
    let settings = legacy.join("settings.json");
    let mut perms = fs::metadata(&settings).unwrap().permissions();
    perms.set_readonly(true);
    fs::set_permissions(&settings, perms).unwrap();
    assert_eq!(Store::migrate(&legacy, &next).dir(), legacy);
    assert!(!next.exists());
    let mut perms = fs::metadata(&settings).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(false);
    fs::set_permissions(&settings, perms).unwrap();
}

#[test]
fn explicit_open_does_not_migrate_sibling_legacy_data() {
    let tmp = tempfile::tempdir().unwrap();
    let (legacy, next) = legacy_and_next(tmp.path());
    Store::open(legacy)
        .save("settings", &json!({"Width": 301}))
        .unwrap();
    let isolated = Store::open(next.clone());
    assert!(isolated.load::<Value>("settings").unwrap().is_none());
    assert!(!next.join("settings.json").exists());
    assert!(isolated.warning().is_none());
}

#[test]
fn migrate_with_extra_validator_can_reject() {
    let tmp = tempfile::tempdir().unwrap();
    let (legacy, next) = legacy_and_next(tmp.path());
    Store::open(legacy.clone())
        .save("settings", &json!({"Width": "wide"}))
        .unwrap();
    let validator = MigrationValidator {
        document: "settings",
        check: |v| v["Width"].is_number(),
    };
    let store = Store::migrate_with(&legacy, &next, &[validator]);
    assert_eq!(store.dir(), legacy);
    assert!(!next.exists());
}

// ---- cross-build compatibility ----

#[test]
fn reads_a_session_written_by_csharp_with_offset_timestamps() {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(
        tmp.path().join("session.json"),
        r#"{
  "Stopwatch": {"ElapsedSeconds": 12.5, "UtcAnchor": "2026-10-29T10:00:00.1234567+00:00", "Running": true},
  "Timer": {"Deadline": "2026-10-29T12:30:00+02:00", "RemainingSeconds": 1500, "DurationSeconds": 1500, "Phase": ""},
  "Focus": {"Deadline": null, "RemainingSeconds": 0, "DurationSeconds": 0, "Phase": ""}
}"#,
    )
    .unwrap();
    fs::write(
        tmp.path().join("session.schema.json"),
        "{\n  \"SchemaVersion\": 1\n}",
    )
    .unwrap();
    let s: SessionState = open(tmp.path()).load_session().unwrap();
    assert_eq!(
        s.timer.deadline.unwrap().to_rfc3339(),
        "2026-10-29T10:30:00+00:00"
    );
    assert!(s.stopwatch.running && s.timer.running() && !s.focus.running());
}
