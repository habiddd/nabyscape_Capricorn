//! Welcome flow tests and step snapshots (`target/snapshots/welcome-*.png`).

use super::*;
use naby_core::settings::presets;
use naby_render::snapshot::{save, test_fonts};
use naby_render::InputState;

struct H {
    view: WelcomeView,
    fonts: Fonts,
    canvas: Canvas,
    out: Vec<Command>,
    time: f64,
    last: WelcomeOutcome,
}

fn harness(store: Option<Arc<Store>>) -> Option<H> {
    let fonts = test_fonts()?;
    let mut h = H {
        view: WelcomeView::new(presets::recommended(), store),
        fonts,
        canvas: Canvas::new(580, 720),
        out: Vec::new(),
        time: 1.0,
        last: WelcomeOutcome::default(),
    };
    h.frame(InputState::default());
    Some(h)
}

impl H {
    fn frame(&mut self, mut input: InputState) {
        self.time += 0.016;
        input.time = self.time;
        self.last = self
            .view
            .frame(&mut self.canvas, &self.fonts, &input, 1.0, &mut self.out);
    }

    fn click(&mut self, name: &str) {
        let (x, y) = self.view.rects[name].center();
        self.frame(InputState::at(x, y));
        self.frame(InputState::press(x, y));
        self.frame(InputState::release(x, y));
        self.frame(InputState::at(x, y));
    }
}

#[test]
fn productivity_preset_flow_ends_with_only_those_widgets_and_the_chosen_edge() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(dir.path().to_path_buf()));
    let Some(mut h) = harness(Some(store.clone())) else {
        return;
    };
    h.click("purpose:Productivity");
    assert_eq!(h.view.purpose(), WelcomePreset::Productivity);
    h.click("Next");
    assert_eq!(h.view.step(), 1);
    h.frame(InputState::default());
    h.click("edge:Bottom");
    h.click("widget:notes");
    h.click("Next");
    assert_eq!(h.view.step(), 2);
    h.click("Next");
    assert_eq!(h.last.done, Some(WelcomeDone::Finished { setup_ai: false }));
    let applied = h
        .out
        .iter()
        .find_map(|c| match c {
            Command::ApplySettings(s) => Some((**s).clone()),
            _ => None,
        })
        .expect("settings applied");
    let on: Vec<&str> = applied
        .widgets
        .iter()
        .filter(|w| w.enabled)
        .map(|w| w.id.as_str())
        .collect();
    assert_eq!(
        on,
        vec!["timer", "focus", "clock", "tasks", "calendar"],
        "notes was switched off in step 2"
    );
    assert_eq!(applied.dock_edge, "Bottom");
    assert!(applied.hover_follows_widgets);
    // Presets never switch on capture or downloads.
    assert_eq!(
        applied.clipboard_history,
        presets::recommended().clipboard_history
    );
    assert_eq!(
        applied.downloads_enabled,
        presets::recommended().downloads_enabled
    );
    let saved: Settings = store.load("settings").unwrap().unwrap();
    assert_eq!(saved.dock_edge, "Bottom");
    assert!(
        !welcome::should_offer_stored(&store, false),
        "setup is marked complete"
    );
}

#[test]
fn ai_preset_continues_to_ai_setup_and_skip_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(dir.path().to_path_buf()));
    let Some(mut h) = harness(Some(store.clone())) else {
        return;
    };
    h.click("purpose:AI");
    h.click("Next");
    h.click("Next");
    h.click("Next");
    assert_eq!(h.last.done, Some(WelcomeDone::Finished { setup_ai: true }));
    let Some(mut s) = harness(Some(Arc::new(Store::open(
        tempfile::tempdir().unwrap().keep(),
    )))) else {
        return;
    };
    s.click("Skip setup");
    assert_eq!(s.last.done, Some(WelcomeDone::Skipped));
    assert!(s.out.is_empty());
}

#[test]
fn custom_keeps_the_current_widgets_and_back_returns() {
    let Some(mut h) = harness(None) else { return };
    let before = h.view.enabled_widgets();
    h.click("purpose:Productivity");
    assert_ne!(h.view.enabled_widgets(), before);
    h.click("purpose:Custom");
    assert_eq!(h.view.enabled_widgets(), before);
    h.click("Next");
    h.click("Back");
    assert_eq!(h.view.step(), 0);
}

#[test]
fn a_failed_write_reports_and_keeps_the_guide_open() {
    let dir = tempfile::tempdir().unwrap();
    let blocker = dir.path().join("file");
    std::fs::write(&blocker, "x").unwrap();
    let store = Arc::new(Store::open(blocker.join("sub")));
    let Some(mut h) = harness(Some(store)) else {
        return;
    };
    h.view.finish(false);
    assert!(h.view.done.is_none());
    assert!(h.view.error.contains("previous settings are unchanged"));
}

#[test]
fn snapshots_of_the_three_steps() {
    let Some(mut h) = harness(None) else { return };
    save("welcome-1", &h.canvas);
    h.click("purpose:Music");
    h.frame(InputState::default());
    save("welcome-1-music", &h.canvas);
    h.click("Next");
    h.frame(InputState::default());
    save("welcome-2", &h.canvas);
    h.click("Next");
    h.frame(InputState::default());
    save("welcome-3", &h.canvas);
    assert_eq!(h.view.step(), 2);
}
