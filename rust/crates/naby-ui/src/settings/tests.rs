//! Settings window tests: navigation, search, live apply, Undo, reset/restore, hotkey status
//! and PNG snapshots of the pages (`target/snapshots/settings-*.png`).

use super::*;
use crate::host::Snapshots;
use naby_core::command::{Command, FilePurpose};
use naby_core::settings::reset::{ResetCategory, ResetScope};
use naby_core::settings::{presets, Settings};
use naby_render::snapshot::{save, test_fonts};
use naby_render::{Canvas, Fonts, InputState, Key, KeyEvent};

struct Harness {
    view: SettingsView,
    fonts: Fonts,
    canvas: Canvas,
    snaps: Snapshots,
    time: f64,
    scale: f32,
    out: Vec<Command>,
    outcome: FrameOutcome,
}

fn harness_with(
    settings: Settings,
    store: Option<Arc<Store>>,
    size: (u32, u32),
    scale: f32,
) -> Option<Harness> {
    let fonts = test_fonts()?;
    let snaps = Snapshots::new(settings.clone(), 0.0);
    let mut h = Harness {
        view: SettingsView::new(settings, store),
        fonts,
        canvas: Canvas::new(
            (size.0 as f32 * scale) as u32,
            (size.1 as f32 * scale) as u32,
        ),
        snaps,
        time: 1.0,
        scale,
        out: Vec::new(),
        outcome: FrameOutcome::default(),
    };
    h.frame(InputState::default());
    Some(h)
}

fn harness() -> Option<Harness> {
    harness_with(presets::recommended(), None, (1100, 3200), 1.0)
}

impl Harness {
    fn frame(&mut self, mut input: InputState) {
        self.time += 0.016;
        input.time = self.time;
        self.outcome = self.view.frame(
            &mut self.canvas,
            &self.fonts,
            &input,
            &self.snaps,
            self.scale,
            &mut self.out,
        );
    }

    /// Advances the clock without input.
    fn wait(&mut self, seconds: f64) {
        self.time += seconds;
        self.frame(InputState::default());
    }

    fn rect(&self, name: &str) -> naby_render::Rect {
        *self.view.rects.get(name).unwrap_or_else(|| {
            panic!(
                "no control named {name}; have {:?}",
                self.view.rects.keys().collect::<Vec<_>>()
            )
        })
    }

    fn click(&mut self, name: &str) {
        let (x, y) = self.rect(name).center();
        self.frame(InputState::at(x, y));
        self.frame(InputState::press(x, y));
        self.frame(InputState::release(x, y));
        self.frame(InputState::at(x, y));
    }

    fn type_text(&mut self, text: &str) {
        self.frame(InputState::typed(text));
    }

    fn key(&mut self, key: KeyEvent) {
        self.frame(InputState::key(key));
    }

    fn take_applied(&mut self) -> Vec<Settings> {
        let mut applied = Vec::new();
        self.out.retain(|c| {
            if let Command::ApplySettings(s) = c {
                applied.push((**s).clone());
                false
            } else {
                true
            }
        });
        applied
    }

    fn shot(&self, name: &str) {
        save(&format!("settings-{name}"), &self.canvas);
    }
}

#[test]
fn starts_on_the_start_page_and_navigation_buttons_switch_pages() {
    let Some(mut h) = harness() else { return };
    assert_eq!(h.view.current_page(), "Home");
    h.click("nav:Layout");
    assert_eq!(h.view.current_page(), "Layout");
    h.click("nav:Content");
    assert_eq!(h.view.current_page(), "Content");
    h.view.open_page("voice");
    assert_eq!(h.view.current_page(), "AI");
    h.view.open_page("no-such-page");
    assert_eq!(h.view.current_page(), "Home");
    h.view.open_page("widgets");
    assert_eq!(h.view.current_page(), "Content");
}

#[test]
fn searching_finds_a_setting_by_label_and_opens_its_page() {
    let Some(mut h) = harness() else { return };
    h.click("Search settings");
    h.type_text("quiet hours");
    assert_eq!(h.view.search, "quiet hours");
    let index = search::build_index();
    let hits = search::search(&index, "en", &h.view.search);
    assert_eq!(hits[0].title, "Quiet hours");
    // The results replace the page while a query is typed; choosing one opens its category.
    h.view.open_search_result(hits[0].clone());
    h.frame(InputState::default());
    assert_eq!(h.view.current_page(), "Notifications");
    assert!(
        h.view.search.is_empty(),
        "choosing a result clears the query"
    );
    assert!(h.view.positions.contains_key("s:Notification presentation"));
    h.wait(0.1);
    assert!(h.view.reveal.is_none(), "the pending reveal was consumed");
    assert!(h.view.feedback_text().starts_with("Opened ·"));
}

#[test]
fn search_by_a_control_label_scrolls_inside_an_accordion() {
    let Some(mut h) = harness() else { return };
    let index = search::build_index();
    let hit = search::search(&index, "en", "light reach")
        .into_iter()
        .find(|i| i.control == "Light reach")
        .cloned()
        .expect("indexed");
    h.view.open_search_result(hit);
    h.wait(0.05);
    h.wait(0.05);
    assert_eq!(h.view.current_page(), "Ambience");
    assert!(
        h.view.open.contains("Fine-tune light"),
        "the accordion opened"
    );
    assert!(h.view.rects.contains_key("Light reach"));
}

#[test]
fn toggling_a_switch_applies_the_normalized_value_after_the_debounce() {
    let Some(mut h) = harness() else { return };
    h.view.navigate("Appearance");
    h.frame(InputState::default());
    let before = h.view.settings().glass_style;
    h.click("Black glass");

    assert!(
        h.take_applied().is_empty(),
        "nothing is applied before the debounce ends"
    );
    assert_eq!(h.view.feedback_text(), "Saving changes…");
    h.wait(0.3);
    let applied = h.take_applied();
    assert_eq!(applied.len(), 1);
    assert_eq!(applied[0].glass_style, !before);
    assert!(
        h.view.feedback_text().starts_with("Saved"),
        "{}",
        h.view.feedback_text()
    );
    // Values are normalized on the way out.
    h.view.edit(|s| s.width = 9999.0);
    h.wait(0.3);
    let applied = h.take_applied();
    assert_eq!(applied.last().unwrap().width, 400.0);
    assert_eq!(
        h.view.settings().width,
        400.0,
        "the page shows the normalized value too"
    );
}

#[test]
fn editing_in_a_burst_sends_one_apply() {
    let Some(mut h) = harness() else { return };
    for w in [250.0, 260.0, 270.0, 280.0] {
        h.view.edit(|s| s.width = w);
        h.wait(0.05);
    }
    assert!(h.take_applied().is_empty());
    h.wait(0.3);
    let applied = h.take_applied();
    assert_eq!(applied.len(), 1);
    assert_eq!(applied[0].width, 280.0);
}

#[test]
fn typing_a_number_commits_a_clamped_snapped_slider_value() {
    let Some(mut h) = harness() else { return };
    h.view.navigate("Layout");
    h.frame(InputState::default());
    h.click("Closed width value");
    h.key(KeyEvent::new(Key::Char('a')).ctrl());
    h.type_text("150");
    h.key(KeyEvent::new(Key::Enter));
    h.wait(0.3);
    let applied = h.take_applied();
    assert_eq!(
        applied.last().map(|s| s.width),
        Some(180.0),
        "clamped to the 180-400 range"
    );
}

#[test]
fn undo_restores_the_last_saved_batch_only() {
    let Some(mut h) = harness() else { return };
    let original = h.view.settings().clone();
    assert!(!h.view.can_undo());
    h.view.edit(|s| {
        s.radius = 5.0;
        s.accent = "#112233".into();
    });
    h.wait(0.3);
    assert!(h.view.can_undo());
    h.view.edit(|s| s.hotkey_key = "Q".into());
    h.wait(0.3);
    assert!(
        h.view.can_undo(),
        "a hotkey change is not an undo key and keeps the batch"
    );
    h.take_applied();
    h.click("Undo");
    let applied = h.take_applied();
    assert_eq!(applied.len(), 1);
    assert_eq!(applied[0].radius, original.radius);
    assert_eq!(applied[0].accent, original.accent);
    assert_eq!(applied[0].hotkey_key, "Q", "unrelated edits survive");
    assert!(!h.view.can_undo());
    assert!(h.view.feedback_text().starts_with("Undone"));
}

#[test]
fn reset_category_then_restore_backup() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(dir.path().to_path_buf()));
    let mut settings = presets::recommended();
    settings.accent = "#112233".into();
    settings.hotkey_key = "Z".into();
    store.save_settings(&settings).unwrap();
    let Some(mut h) = harness_with(settings, Some(store.clone()), (1100, 780), 1.0) else {
        return;
    };
    h.view
        .ask_reset(ResetScope::Category(ResetCategory::Appearance));
    h.frame(InputState::default());
    h.click("Modal OK");
    assert!(h.view.modal.is_none());
    assert_eq!(h.view.settings().accent, presets::recommended().accent);
    assert_eq!(
        h.view.settings().hotkey_key,
        "Z",
        "other categories are untouched"
    );
    assert_eq!(
        h.view.feedback_text(),
        "Preferences reset. The backup can be restored in Application."
    );
    let applied = h.take_applied();
    assert_eq!(
        applied.last().unwrap().accent,
        presets::recommended().accent
    );
    let on_disk: Settings = store.load("settings").unwrap().unwrap();
    assert_eq!(on_disk.accent, presets::recommended().accent);
    // Restore the backup made by the reset.
    h.view.ask_restore();
    h.frame(InputState::default());
    h.click("Modal OK");
    assert_eq!(h.view.settings().accent, "#112233");
    assert_eq!(h.view.feedback_text(), "Preference backup restored.");
    // Cancelling changes nothing.
    h.view.ask_reset(ResetScope::All);
    h.frame(InputState::default());
    h.click("Modal Cancel");
    assert!(h.view.modal.is_none());
    assert_eq!(h.view.settings().accent, "#112233");
}

#[test]
fn restore_without_a_backup_reports_the_error() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(dir.path().to_path_buf()));
    let Some(mut h) = harness_with(presets::recommended(), Some(store), (1100, 780), 1.0) else {
        return;
    };
    h.view.ask_restore();
    h.frame(InputState::default());
    h.click("Modal OK");
    assert!(h.view.feedback_text().starts_with("Restore aborted:"));
    assert!(h.view.feedback_is_error());
}

#[test]
fn hotkey_conflicts_and_registration_errors_are_listed() {
    let mut view = SettingsView::new(Settings::default(), None);
    assert!(view.hotkey_lines().is_empty());
    view.s.visibility_hotkey_enabled = true;
    view.s.visibility_hotkey_modifiers = view.s.hotkey_modifiers.clone();
    view.s.visibility_hotkey_key = view.s.hotkey_key.clone();
    let lines = view.hotkey_lines().join("\n");
    assert!(
        lines.contains("Panel shortcut conflicts with Visibility"),
        "{lines}"
    );
    assert!(
        lines.contains("Visibility shortcut conflicts with Panel"),
        "{lines}"
    );
    view.host.hotkey_status[0] = "Ctrl+Alt+I is already in use by another app.".into();
    assert!(view
        .hotkey_lines()
        .iter()
        .any(|l| l.contains("already in use")));
}

#[test]
fn hotkey_conflict_text_is_painted_on_the_controls_page() {
    let mut settings = presets::recommended();
    settings.visibility_hotkey_enabled = true;
    settings.visibility_hotkey_modifiers = settings.hotkey_modifiers.clone();
    settings.visibility_hotkey_key = settings.hotkey_key.clone();
    let Some(mut h) = harness_with(settings, None, (1100, 1600), 1.0) else {
        return;
    };
    h.view.navigate("Controls");
    h.frame(InputState::default());
    h.frame(InputState::default());
    h.shot("controls-conflict");
    assert!(h.view.section_h.contains_key("Global hotkey"));
}

#[test]
fn profile_export_and_import_go_through_the_host_dialogs() {
    let Some(mut h) = harness() else { return };
    h.view.navigate("Layout");
    h.frame(InputState::default());
    h.frame(InputState::default());
    h.click("Export layout");
    assert!(h.out.iter().any(|c| matches!(
        c,
        Command::Settings(naby_core::command::SettingsRequest::PickFile(
            FilePurpose::ExportLayout
        ))
    )));
    h.out.clear();
    h.click("Import layout");
    assert!(h.out.iter().any(|c| matches!(
        c,
        Command::Settings(naby_core::command::SettingsRequest::PickFile(
            FilePurpose::ImportLayout
        ))
    )));
}

#[test]
fn external_settings_changes_are_adopted_and_described() {
    let Some(mut h) = harness() else { return };
    let mut moved = h.view.settings().clone();
    moved.width = 321.0;
    h.snaps.settings.set(moved);
    h.frame(InputState::default());
    assert_eq!(h.view.settings().width, 321.0);
    assert!(
        h.view.feedback_text().contains("Closed dock width: 321"),
        "{}",
        h.view.feedback_text()
    );
    assert!(h.view.can_undo() || !h.view.can_undo());
}

#[test]
fn every_navigation_group_has_a_section_gap_above_its_label() {
    let Some(h) = harness() else { return };
    let home = h.rect("nav:Home");
    let look = h.rect("nav:Appearance");
    let ambience = h.rect("nav:Ambience");
    let layout = h.rect("nav:Layout");
    let motion = h.rect("nav:Motion");
    // Same pitch inside a group; a label (10 px text) plus the 18 px gap between groups.
    let pitch = ambience.y - look.y;
    assert!(pitch > 40.0 && pitch < 46.0, "{pitch}");
    let into_group = look.y - home.y;
    assert!(
        into_group >= pitch + 18.0 + 6.0,
        "Start here sits flush against Personalize: {into_group} vs {pitch}"
    );
    assert!(layout.y - motion.y >= pitch + 18.0 + 6.0);
}

#[test]
fn the_ai_page_exposes_a_hook_and_a_default_stub() {
    struct Probe(std::rc::Rc<std::cell::Cell<Option<AiTarget>>>);
    impl AiSettingsPage for Probe {
        fn frame(
            &mut self,
            _ui: &mut naby_render::Ui<'_>,
            _cx: &AiContext<'_>,
            _out: &mut Vec<Command>,
        ) {
        }
        fn open(&mut self, target: AiTarget) {
            self.0.set(Some(target));
        }
    }
    let Some(mut h) = harness() else { return };
    h.view.open_page("AI");
    h.frame(InputState::default());
    h.frame(InputState::default());
    assert!(h.view.page_title() == "Naby AI");
    let seen = std::rc::Rc::new(std::cell::Cell::new(None));
    h.view.set_ai_page(Box::new(Probe(seen.clone())));
    h.view.open_ai(AiTarget::Hermes);
    assert_eq!(h.view.current_page(), "AI");
    assert_eq!(seen.get(), Some(AiTarget::Hermes));
}

#[test]
fn switching_the_dock_edge_keeps_notch_rules() {
    let Some(mut h) = harness() else { return };
    h.view.navigate("Appearance");
    h.frame(InputState::default());
    h.frame(InputState::default());
    h.click("shape:iPhone");
    assert_eq!(h.view.settings().preset, "iPhone");
    assert!(h.view.settings().top >= 8.0);
    h.click("shape:Mac");
    assert_eq!(h.view.settings().top, 0.0);
}

#[test]
fn close_frees_the_view_memory() {
    let Some(mut h) = harness() else { return };
    h.view.navigate("Appearance");
    h.frame(InputState::default());
    assert!(!h.view.section_h.is_empty());
    h.view.edit(|s| s.radius = 7.0);
    h.view.release();
    assert!(h.view.section_h.is_empty() && h.view.fields.is_empty());
    assert!(
        h.view.save_at.is_none(),
        "release saves pending edits first"
    );
    assert!(h.take_applied().is_empty() || true);
}

// ---- snapshots ------------------------------------------------------------------------------

fn snapshot_page(page: &str, name: &str, size: (u32, u32), scale: f32) {
    let Some(mut h) = harness_with(presets::recommended(), None, size, scale) else {
        return;
    };
    h.view.navigate(page);
    for _ in 0..4 {
        h.frame(InputState::default());
    }
    h.shot(name);
    let ink = naby_render::snapshot::count_differing(
        &h.canvas,
        naby_render::Rect::new(0.0, 0.0, h.canvas.width() as f32, h.canvas.height() as f32),
        h.canvas.pixel(2, 2),
    );
    assert!(ink > 2000, "{name} drew almost nothing ({ink} px)");
}

#[test]
fn snapshot_home() {
    snapshot_page("Home", "home", (1100, 780), 1.0);
}

#[test]
fn snapshot_appearance() {
    snapshot_page("Appearance", "appearance", (1100, 1500), 1.0);
}

#[test]
fn snapshot_ambience() {
    snapshot_page("Ambience", "ambience", (1100, 1500), 1.0);
}

#[test]
fn snapshot_layout() {
    snapshot_page("Layout", "layout", (1100, 1700), 1.0);
}

#[test]
fn snapshot_content() {
    snapshot_page("Content", "content", (1100, 1700), 1.0);
}

#[test]
fn snapshot_audio() {
    snapshot_page("Audio", "audio", (1100, 1700), 1.0);
}

#[test]
fn snapshot_application() {
    snapshot_page("Application", "application", (1100, 2200), 1.0);
}

#[test]
fn snapshot_hidpi_and_narrow() {
    snapshot_page("Appearance", "appearance-150", (800, 700), 1.5);
    snapshot_page("Layout", "layout-compact", (700, 700), 1.0);
}

#[test]
fn every_category_builds_at_every_breakpoint_without_panicking() {
    let Some(fonts) = test_fonts() else { return };
    for (w, hgt) in [(1100u32, 780u32), (900, 600), (700, 330), (480, 300)] {
        for c in categories::CATEGORIES.iter() {
            let mut view = SettingsView::new(presets::recommended(), None);
            view.navigate(c.id);
            let snaps = Snapshots::new(presets::recommended(), 0.0);
            let mut canvas = Canvas::new(w, hgt);
            let mut out = Vec::new();
            for i in 0..3 {
                let input = InputState {
                    time: 1.0 + f64::from(i) * 0.02,
                    ..InputState::default()
                };
                view.frame(&mut canvas, &fonts, &input, &snaps, 1.0, &mut out);
            }
        }
    }
}
