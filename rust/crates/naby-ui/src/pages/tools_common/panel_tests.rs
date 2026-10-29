//! The tool pages inside the real overlay panel: pointer, keyboard and text input travel
//! through the dock the way the host feeds them, and closing the panel frees what it held.

use super::{clear_marks, marked};
use crate::host::Signal;
use crate::overlay::tests::{quiet_settings, Rig};
use crate::OverlayState;
use naby_core::command::Command;
use naby_core::settings::{Settings, WidgetSetting};
use naby_render::{snapshot, InputState, Key, KeyEvent, Rgba};

fn tool_settings() -> Settings {
    let mut s = quiet_settings();
    s.hover_peek = false;
    for id in [
        "tasks", "notes", "clips", "calendar", "awake", "links", "files",
    ] {
        s.widgets.push(WidgetSetting::new(id, true));
    }
    s
}

/// The dock open on tool page `id` (a font-less machine skips the test).
fn open_on(id: &'static str) -> Option<Rig> {
    snapshot::test_fonts()?;
    let mut r = Rig::new(tool_settings());
    r.snaps.display.replace(crate::host::DisplayInfo {
        work: naby_core::motion::RectI::new(0, 0, 1920, 1040),
        ..Default::default()
    });
    r.signal(Signal::ShowPage(id));
    r.settle();
    r.paint();
    r.settle();
    r.paint();
    assert_eq!(r.state(), OverlayState::Open);
    assert_eq!(r.overlay.active_page(), id);
    Some(r)
}

/// Pointer position of the last painted control `name`.
fn at(name: &str) -> (f32, f32) {
    marked(name)
        .unwrap_or_else(|| panic!("no control marked {name}"))
        .center()
}

fn click(r: &mut Rig, p: (f32, f32)) {
    r.point(Some(p));
    r.input(InputState::press(p.0, p.1));
    r.paint();
    r.input(InputState::release(p.0, p.1));
    r.paint();
}

fn type_text(r: &mut Rig, text: &str) {
    r.input(InputState::typed(text));
    r.paint();
}

fn press(r: &mut Rig, key: Key) {
    r.input(InputState::key(KeyEvent::new(key)));
    r.paint();
}

fn fresh_marks(r: &mut Rig) {
    clear_marks();
    r.paint();
}

#[test]
fn tasks_are_added_checked_and_removed_through_the_panel() {
    let Some(mut r) = open_on("tasks") else {
        return;
    };
    fresh_marks(&mut r);
    click(&mut r, at("task-draft"));
    type_text(&mut r, "Water the plants");
    press(&mut r, Key::Enter);
    assert_eq!(r.snaps.productivity.borrow().tasks().len(), 1);
    fresh_marks(&mut r);
    click(&mut r, at("task-toggle-Water the plants"));
    assert!(r.snaps.productivity.borrow().tasks()[0].done);
    fresh_marks(&mut r);
    r.point(Some(at("task-toggle-Water the plants")));
    r.paint();
    click(&mut r, at("task-remove-Water the plants"));
    assert!(r.snaps.productivity.borrow().tasks().is_empty());
    // The panel is drawn: snapshot of the whole dock.
    fresh_marks(&mut r);
    snapshot::save("panel-tasks", &r.canvas);
    assert!(r.canvas.width() > 100);
}

#[test]
fn typing_in_notes_saves_when_the_panel_closes() {
    let Some(mut r) = open_on("notes") else {
        return;
    };
    let saves = std::rc::Rc::new(std::cell::Cell::new(0));
    {
        let counter = saves.clone();
        r.snaps
            .productivity
            .borrow_mut()
            .set_saver(Box::new(move |_| {
                counter.set(counter.get() + 1);
                Ok(())
            }));
    }
    fresh_marks(&mut r);
    click(&mut r, at("note-editor"));
    type_text(&mut r, "remember the milk");
    assert_eq!(r.snaps.productivity.borrow().note(), "remember the milk");
    assert_eq!(saves.get(), 0, "the save waits for the debounce");
    snapshot::save("panel-notes", &r.canvas);
    // Closing the panel (Escape) flushes the pending save.
    r.input(InputState::key(KeyEvent::new(Key::Escape)));
    r.settle();
    r.paint();
    assert_ne!(r.state(), OverlayState::Open);
    assert_eq!(saves.get(), 1);
}

#[test]
fn the_awake_page_starts_a_session_from_the_panel() {
    let Some(mut r) = open_on("awake") else {
        return;
    };
    fresh_marks(&mut r);
    click(&mut r, at("awake-start"));
    let sent = r.commands();
    assert!(
        sent.contains(&Command::AwakeStart {
            minutes: Some(30),
            keep_display_on: false
        }),
        "{sent:?}"
    );
}

#[test]
fn the_calendar_and_clips_pages_paint_inside_the_panel() {
    for id in ["calendar", "clips", "files", "links"] {
        let Some(mut r) = open_on(id) else {
            return;
        };
        fresh_marks(&mut r);
        let (w, h) = (r.canvas.width(), r.canvas.height());
        assert!(w > 100 && h > 100, "{id}");
        let ink = snapshot::count_differing(
            &r.canvas,
            naby_render::Rect::new(0.0, 0.0, w as f32, h as f32),
            Rgba::TRANSPARENT,
        );
        assert!(ink > 2000, "{id} drew into the panel");
        snapshot::save(&format!("panel-{id}"), &r.canvas);
    }
}

#[test]
fn a_text_field_stops_the_page_swipe() {
    let Some(mut r) = open_on("links") else {
        return;
    };
    fresh_marks(&mut r);
    click(&mut r, at("link-input"));
    assert!(
        r.overlay.panel.pages.values().any(|p| p.captures_pointer()),
        "the focused field holds the pointer"
    );
}

/// A page with nothing animating asks for no further frames and no timer (the dock must stay
/// idle); typing in a field only schedules the caret blink.
#[test]
fn idle_pages_schedule_nothing() {
    use super::test_support::Rig as PageRig;
    for id in [
        "tasks",
        "notes",
        "clips",
        "calendar",
        "awake",
        "links",
        "files",
        "shortcuts",
    ] {
        let Some(mut rig) = PageRig::new(id) else {
            return;
        };
        {
            let mut p = rig.snaps.productivity.borrow_mut();
            p.add_task("A task that is long enough to wrap onto a second line in a narrow panel");
            p.set_clipboard_enabled(true);
            p.ingest_text(Some("clip one"));
            p.ingest_text(Some("#FF8800"));
        }
        rig.paint();
        let quiet = InputState {
            time: rig.snaps.now,
            ..InputState::default()
        };
        assert!(!rig.frame(&quiet), "{id} keeps asking for frames");
        assert_eq!(
            rig.page.next_frame(rig.snaps.now),
            None,
            "{id} keeps a timer"
        );
    }
}
