use super::*;
use crate::icons::Icon;
use crate::snapshot;

pub(super) struct Harness {
    pub(super) canvas: Canvas,
    pub(super) fonts: Fonts,
    pub(super) theme: Theme,
    pub(super) mem: UiMemory,
    pub(super) area: Rect,
}

impl Harness {
    pub(super) fn new(w: u32, h: u32) -> Self {
        Harness::with_fonts(w, h, Fonts::empty())
    }
    pub(super) fn with_fonts(w: u32, h: u32, fonts: Fonts) -> Self {
        Harness {
            canvas: Canvas::new(w, h),
            fonts,
            theme: Theme::default(),
            mem: UiMemory::new(),
            area: Rect::new(0.0, 0.0, w as f32, h as f32),
        }
    }
    pub(super) fn frame(&mut self, input: &InputState, build: impl FnMut(&mut Ui)) {
        run_frame(
            &mut self.canvas,
            &self.fonts,
            &self.theme,
            input,
            &mut self.mem,
            self.area,
            Some(self.theme.background),
            build,
        );
    }
}

pub(super) fn click(x: f32, y: f32) -> InputState {
    InputState {
        pointer: Some((x, y)),
        pressed: true,
        released: true,
        ..InputState::default()
    }
}

pub(super) fn key(k: Key) -> InputState {
    InputState::key(KeyEvent::new(k))
}

#[test]
fn toggle_click_flips_value() {
    let mut h = Harness::new(300, 100);
    let mut on = false;
    h.frame(&InputState::default(), |ui| {
        ui.toggle(&mut on, "Enabled");
    });
    assert!(!on);
    h.frame(&click(150.0, 20.0), |ui| {
        let r = ui.toggle(&mut on, "Enabled");
        assert!(r.clicked && r.changed);
    });
    assert!(
        on,
        "click flips exactly once even though the frame repaints"
    );
    // A click elsewhere does nothing.
    h.frame(&click(150.0, 80.0), |ui| {
        ui.toggle(&mut on, "Enabled");
    });
    assert!(on);
    // Keyboard: Tab focuses, Space flips.
    h.frame(&key(Key::Tab), |ui| {
        ui.toggle(&mut on, "Enabled");
    });
    h.frame(&key(Key::Space), |ui| {
        ui.toggle(&mut on, "Enabled");
    });
    assert!(!on);
}

#[test]
fn disabled_widgets_ignore_input() {
    let mut h = Harness::new(300, 100);
    let mut on = false;
    h.frame(&click(150.0, 20.0), |ui| {
        ui.enabled(false, |ui| {
            ui.toggle(&mut on, "x");
        });
    });
    assert!(!on);
}

#[test]
fn checkbox_and_button_click() {
    let mut h = Harness::new(300, 200);
    let mut checked = false;
    let mut pressed = 0;
    h.frame(&click(20.0, 15.0), |ui| {
        ui.checkbox(&mut checked, "Remember");
        if ui.button("Go").clicked {
            pressed += 1;
        }
    });
    assert!(checked);
    // Button sits below the 30px checkbox + 8px spacing.
    h.frame(&click(100.0, 38.0 + 10.0), |ui| {
        ui.checkbox(&mut checked, "Remember");
        if ui.button("Go").clicked {
            pressed += 1;
        }
    });
    assert_eq!(pressed, 1);
}

#[test]
fn slider_drag_changes_value_by_step() {
    let mut h = Harness::new(300, 100);
    let mut v = 0.0f32;
    let mut committed = false;
    // Track spans x = 7 ..= 293 (14px thumb inset).
    h.frame(&InputState::press(7.0 + 0.5 * 286.0, 10.0), |ui| {
        ui.slider(&mut v, 0.0..=1.0, 0.25);
    });
    assert_eq!(v, 0.5);
    h.frame(&InputState::drag(7.0 + 0.8 * 286.0, 40.0), |ui| {
        let r = ui.slider(&mut v, 0.0..=1.0, 0.25);
        assert!(r.changed);
    });
    assert_eq!(v, 0.75);
    // Dragging past the right end clamps; the pointer may leave the widget vertically.
    h.frame(&InputState::drag(900.0, 90.0), |ui| {
        ui.slider(&mut v, 0.0..=1.0, 0.25);
    });
    assert_eq!(v, 1.0);
    h.frame(&InputState::release(900.0, 90.0), |ui| {
        committed = ui.slider(&mut v, 0.0..=1.0, 0.25).committed;
    });
    assert!(committed);
    assert_eq!(v, 1.0);
    // Once released, moving the pointer does not change the value.
    h.frame(&InputState::at(20.0, 10.0), |ui| {
        ui.slider(&mut v, 0.0..=1.0, 0.25);
    });
    assert_eq!(v, 1.0);
}

#[test]
fn slider_keyboard_steps_and_clamps() {
    let mut h = Harness::new(300, 100);
    let mut v = 5.0f32;
    h.frame(&key(Key::Tab), |ui| {
        ui.slider(&mut v, 0.0..=10.0, 1.0);
    });
    h.frame(&key(Key::Right), |ui| {
        ui.slider(&mut v, 0.0..=10.0, 1.0);
    });
    assert_eq!(v, 6.0);
    h.frame(&InputState::key(KeyEvent::new(Key::Right).shift()), |ui| {
        ui.slider(&mut v, 0.0..=10.0, 1.0);
    });
    assert_eq!(v, 10.0);
    h.frame(&key(Key::Home), |ui| {
        ui.slider(&mut v, 0.0..=10.0, 1.0);
    });
    assert_eq!(v, 0.0);
}

#[test]
fn tab_moves_focus() {
    let mut h = Harness::new(300, 300);
    let mut ids = Vec::new();
    let frame = |h: &mut Harness, input: &InputState| -> Vec<u64> {
        let mut got = Vec::new();
        h.frame(input, |ui| {
            got.clear();
            let mut v = 0.5;
            got.push(ui.button("One").id);
            got.push(ui.button("Two").id);
            got.push(ui.slider(&mut v, 0.0..=1.0, 0.0).id);
        });
        got
    };
    ids.extend(frame(&mut h, &InputState::default()));
    assert_eq!(h.mem.focused(), None);
    frame(&mut h, &key(Key::Tab));
    assert_eq!(h.mem.focused(), Some(ids[0]));
    frame(&mut h, &key(Key::Tab));
    assert_eq!(h.mem.focused(), Some(ids[1]));
    frame(&mut h, &key(Key::Tab));
    assert_eq!(h.mem.focused(), Some(ids[2]));
    // Wraps around at the end.
    frame(&mut h, &key(Key::Tab));
    assert_eq!(h.mem.focused(), Some(ids[0]));
    // Shift+Tab goes backwards and wraps to the last widget.
    let back = InputState::key(KeyEvent::new(Key::Tab).shift());
    frame(&mut h, &back);
    assert_eq!(h.mem.focused(), Some(ids[2]));
    frame(&mut h, &back);
    assert_eq!(h.mem.focused(), Some(ids[1]));
    // Enter activates the focused button.
    let mut clicked = false;
    h.frame(&key(Key::Enter), |ui| {
        clicked = ui.button("One").clicked;
        let two = ui.button("Two");
        assert!(two.clicked && two.focused);
    });
    assert!(!clicked);
    // Clicking empty space drops focus.
    frame(&mut h, &click(290.0, 290.0));
    assert_eq!(h.mem.focused(), None);
}

#[test]
fn push_id_keeps_state_with_the_item() {
    let mut h = Harness::new(200, 200);
    let mut ids = Vec::new();
    h.frame(&InputState::default(), |ui| {
        ids.clear();
        for i in [3, 7] {
            ui.push_id(i, |ui| ids.push(ui.button("x").id));
        }
    });
    let first = ids.clone();
    h.frame(&InputState::default(), |ui| {
        ids.clear();
        for i in [7, 3] {
            ui.push_id(i, |ui| ids.push(ui.button("x").id));
        }
    });
    assert_eq!(ids, vec![first[1], first[0]]);
}

#[test]
fn combo_opens_and_picks() {
    let mut h = Harness::new(300, 300);
    let items = ["Alpha", "Beta", "Gamma"];
    let mut sel = 0usize;
    h.frame(&click(50.0, 10.0), |ui| {
        ui.combo(&mut sel, &items);
    });
    assert!(h.mem.popup.is_some());
    // The list is open: it blocks widgets underneath it.
    let mut under_clicked = false;
    // Popup top = 34 + 2, inner top = 40, item height = line_h(≈16) + 16 = 32; item 2 centered at 40+64+16.
    let mut changed = false;
    h.frame(&click(50.0, 120.0), |ui| {
        changed |= ui.combo(&mut sel, &items).changed;
        under_clicked |= ui.button("Under").clicked;
    });
    assert!(changed);
    assert_eq!(sel, 2);
    assert!(h.mem.popup.is_none());
    assert!(!under_clicked);
    // Keyboard: Tab, Up changes without opening.
    h.frame(&key(Key::Tab), |ui| {
        ui.combo(&mut sel, &items);
    });
    h.frame(&key(Key::Up), |ui| {
        ui.combo(&mut sel, &items);
    });
    assert_eq!(sel, 1);
    // Click outside closes an open list.
    h.frame(&click(50.0, 10.0), |ui| {
        ui.combo(&mut sel, &items);
    });
    assert!(h.mem.popup.is_some());
    h.frame(&click(250.0, 280.0), |ui| {
        ui.combo(&mut sel, &items);
    });
    assert!(h.mem.popup.is_none());
    assert_eq!(sel, 1);
}

#[test]
fn segmented_selects_by_position_and_arrows() {
    let mut h = Harness::new(300, 100);
    let mut sel = 0usize;
    let items = ["A", "B", "C", "D"];
    // inner width 294 -> each 73.5; third segment spans 3 + 147 ..
    h.frame(&click(3.0 + 73.5 * 2.5, 15.0), |ui| {
        let r = ui.segmented(&items, &mut sel);
        assert!(r.changed);
    });
    assert_eq!(sel, 2);
    h.frame(&key(Key::Right), |ui| {
        ui.segmented(&items, &mut sel);
    });
    assert_eq!(sel, 3);
    h.frame(&key(Key::Right), |ui| {
        ui.segmented(&items, &mut sel);
    });
    assert_eq!(sel, 3);
}

#[test]
fn text_edit_types_deletes_selects_and_clips() {
    let mut h = Harness::new(300, 100);
    let mut text = String::new();
    // Focus by clicking into the field.
    h.frame(&click(30.0, 15.0), |ui| {
        ui.text_edit(&mut text, false);
    });
    assert!(h.mem.wants_text_input() || h.mem.focused().is_some());
    h.frame(&InputState::typed("héllo"), |ui| {
        let r = ui.text_edit(&mut text, false);
        assert!(r.changed);
    });
    assert_eq!(text, "héllo");
    h.frame(&key(Key::Backspace), |ui| {
        ui.text_edit(&mut text, false);
    });
    assert_eq!(text, "héll");
    h.frame(&InputState::key(KeyEvent::new(Key::Left)), |ui| {
        ui.text_edit(&mut text, false);
    });
    h.frame(&key(Key::Delete), |ui| {
        ui.text_edit(&mut text, false);
    });
    assert_eq!(text, "hél");
    // Ctrl+A, then typing replaces everything.
    h.frame(
        &InputState::key(KeyEvent::new(Key::Char('a')).ctrl()),
        |ui| {
            ui.text_edit(&mut text, false);
        },
    );
    h.frame(
        &InputState::key(KeyEvent::new(Key::Char('c')).ctrl()),
        |ui| {
            ui.text_edit(&mut text, false);
        },
    );
    assert_eq!(h.mem.take_clipboard().as_deref(), Some("hél"));
    h.frame(&InputState::typed("x"), |ui| {
        ui.text_edit(&mut text, false);
    });
    assert_eq!(text, "x");
    // Paste (newlines stripped in single-line fields).
    let paste = InputState {
        keys: vec![KeyEvent::new(Key::Char('v')).ctrl()],
        paste: Some("a\nb".into()),
        ..InputState::default()
    };
    h.frame(&paste, |ui| {
        ui.text_edit(&mut text, false);
    });
    assert_eq!(text, "xab");
    // Enter submits a single-line field.
    let mut submitted = false;
    h.frame(&key(Key::Enter), |ui| {
        submitted = ui.text_edit(&mut text, false).submitted;
    });
    assert!(submitted);
    // Clicking away ends text input.
    h.frame(&click(250.0, 90.0), |ui| {
        ui.text_edit(&mut text, false);
    });
    assert!(!h.mem.wants_text_input());
}

#[test]
fn password_field_hides_and_blocks_copy() {
    let mut h = Harness::new(300, 100);
    let mut text = "secret".to_owned();
    let opts = TextEditOpts::password();
    h.frame(&click(30.0, 15.0), |ui| {
        ui.text_edit_with(&mut text, &opts);
    });
    h.frame(
        &InputState::key(KeyEvent::new(Key::Char('a')).ctrl()),
        |ui| {
            ui.text_edit_with(&mut text, &opts);
        },
    );
    h.frame(
        &InputState::key(KeyEvent::new(Key::Char('c')).ctrl()),
        |ui| {
            ui.text_edit_with(&mut text, &opts);
        },
    );
    assert!(h.mem.take_clipboard().is_none());
}

#[test]
fn multiline_edit_enter_inserts_newline_and_moves_vertically() {
    let Some(fonts) = snapshot::test_fonts() else {
        return;
    };
    let mut h = Harness::with_fonts(300, 160, fonts);
    let mut text = String::from("one\ntwo");
    let opts = TextEditOpts::multiline(3);
    h.frame(&click(250.0, 14.0), |ui| {
        ui.text_edit_with(&mut text, &opts);
    });
    // Caret lands at the end of the first line; End then Enter splits there.
    h.frame(&key(Key::Enter), |ui| {
        let r = ui.text_edit_with(&mut text, &opts);
        assert!(r.changed && !r.submitted);
    });
    assert_eq!(text, "one\n\ntwo");
    h.frame(&key(Key::Down), |ui| {
        ui.text_edit_with(&mut text, &opts);
    });
    h.frame(&InputState::typed("X"), |ui| {
        ui.text_edit_with(&mut text, &opts);
    });
    assert_eq!(text, "one\n\nXtwo");
    h.frame(&key(Key::Up), |ui| {
        ui.text_edit_with(&mut text, &opts);
    });
    h.frame(&InputState::typed("Y"), |ui| {
        ui.text_edit_with(&mut text, &opts);
    });
    assert_eq!(text, "one\nY\nXtwo");
}

#[test]
fn scroll_area_wheel_clamps_and_reports_content() {
    let mut h = Harness::new(200, 100);
    let build = |ui: &mut Ui| {
        ui.scroll_area_h("list", 100.0, |ui| {
            for i in 0..10 {
                ui.push_id(i, |ui| {
                    ui.button("row");
                });
            }
        });
    };
    h.frame(&InputState::default(), build);
    let sid = {
        let (k, st) = h.mem.scrolls.iter().next().unwrap();
        assert!(
            st.content_h > 400.0 && st.content_h < 430.0,
            "{}",
            st.content_h
        );
        *k
    };
    let wheel = |dy: f32| InputState {
        pointer: Some((50.0, 50.0)),
        wheel: (0.0, dy),
        time: 1.0,
        ..InputState::default()
    };
    h.frame(&wheel(-60.0), build);
    assert_eq!(h.mem.scrolls[&sid].offset, 60.0);
    h.frame(&wheel(-10_000.0), build);
    let max = h.mem.scrolls[&sid].content_h - 100.0;
    assert_eq!(h.mem.scrolls[&sid].offset, max);
    h.frame(&wheel(10_000.0), build);
    assert_eq!(h.mem.scrolls[&sid].offset, 0.0);
    // The scrollbar fades: a repaint is requested right after scrolling, none once idle.
    h.frame(&wheel(-30.0), build);
    assert!(h.mem.repaint_after().is_some());
    let idle = InputState {
        time: 5.0,
        ..InputState::default()
    };
    h.frame(&idle, build);
    assert!(h.mem.repaint_after().is_none());
}

#[test]
fn tab_into_scrolled_content_scrolls_it_into_view() {
    let mut h = Harness::new(200, 100);
    let build = |ui: &mut Ui| {
        ui.scroll_area_h("list", 100.0, |ui| {
            for i in 0..10 {
                ui.push_id(i, |ui| {
                    ui.button("row");
                });
            }
        });
    };
    h.frame(&InputState::default(), build);
    for _ in 0..4 {
        h.frame(&key(Key::Tab), build);
    }
    let st = h.mem.scrolls.values().next().unwrap();
    assert!(st.offset > 0.0, "fourth button is below the 100px viewport");
}

#[test]
fn card_grows_to_fit_after_repaint() {
    let Some(fonts) = snapshot::test_fonts() else {
        return;
    };
    let mut h = Harness::with_fonts(300, 300, fonts);
    h.frame(&InputState::default(), |ui| {
        ui.card("Title", |ui| {
            ui.label("Body");
            ui.button("Go");
        });
    });
    // After the settle pass the card background reaches its content.
    let c = h.theme.control_fill;
    assert_eq!(h.canvas.pixel(150, 60), c);
    assert!(!h.mem.take_repaint());
}

#[test]
fn color_picker_field_and_hex_edit() {
    let mut h = Harness::new(300, 600);
    let mut value = Rgba::rgb(255, 0, 0);
    let presets = [Rgba::rgb(10, 20, 30), Rgba::rgb(200, 100, 50)];
    // Open via the header.
    h.frame(&click(100.0, 20.0), |ui| {
        ui.color_picker(&mut value, &presets, &[]);
    });
    // Header 44 + spacing 8: the field starts at y = 52 and is 136 tall.
    let mut r = Response::default();
    h.frame(&InputState::press(150.0, 52.0 + 68.0), |ui| {
        r = ui.color_picker(&mut value, &presets, &[]);
    });
    assert!(r.changed);
    assert_eq!(value, hsv_to_rgb(0.0, 0.5, 0.5));
    assert_eq!(value, Rgba::rgb(128, 64, 64));
    h.frame(&InputState::release(150.0, 120.0), |ui| {
        r = ui.color_picker(&mut value, &presets, &[]);
    });
    assert!(r.committed);
    // Presets sit below the hue slider (24) and expander row; clicking the first applies it.
    // Locate it by scanning the response of a swatch instead of hard-coding the position.
    let mut swatch_rect = None;
    h.frame(&InputState::default(), |ui| {
        ui.color_picker(&mut value, &presets, &[]);
        swatch_rect = Some(ui.cursor_pos());
    });
    let (_, bottom) = swatch_rect.unwrap();
    // Presets are the last block; their row is 26 px above the cursor (minus nothing else).
    h.frame(&click(13.0, bottom - 13.0), |ui| {
        let r = ui.color_picker(&mut value, &presets, &[]);
        assert!(r.committed);
    });
    assert_eq!(value, presets[0]);
}

#[test]
fn progress_and_separator_paint() {
    let mut h = Harness::new(100, 60);
    h.frame(&InputState::default(), |ui| {
        ui.progress(0.5);
        ui.separator();
        ui.progress_ring(0.75, 24.0);
    });
    let th = &h.theme;
    assert_eq!(h.canvas.pixel(10, 2), th.progress_fill);
    assert_eq!(h.canvas.pixel(90, 2), th.progress_track);
    // Separator line at y = 4 + 8 + 5.
    assert_eq!(h.canvas.pixel(50, 17), th.separator);
}

#[test]
fn tooltip_waits_then_shows_and_requests_timer() {
    let mut h = Harness::with_fonts(
        300,
        200,
        snapshot::test_fonts().unwrap_or_else(Fonts::empty),
    );
    let at = |t: f64| InputState {
        pointer: Some((20.0, 20.0)),
        time: t,
        ..InputState::default()
    };
    let build = |ui: &mut Ui| {
        let r = ui.icon_button(Icon::Plus, "Add item");
        let _ = r;
    };
    h.frame(&at(0.0), build);
    assert!(h.mem.repaint_after().is_some(), "timer armed for the delay");
    h.frame(&at(0.6), build);
    assert!(
        h.mem.repaint_after().is_none(),
        "tooltip is shown, nothing animates"
    );
}

#[test]
fn memory_clear_resets_everything() {
    let mut h = Harness::new(100, 100);
    h.frame(&key(Key::Tab), |ui| {
        ui.button("a");
    });
    assert!(h.mem.focused().is_some());
    h.mem.clear();
    assert!(h.mem.focused().is_none());
}

#[test]
fn smoke_snapshot_renders_visible_widgets() {
    let Some(fonts) = snapshot::test_fonts() else {
        return;
    };
    let mut h = Harness::with_fonts(420, 760, fonts);
    let mut on = true;
    let mut off = false;
    let mut v = 0.4f32;
    let mut sel = 1usize;
    let mut seg = 0usize;
    let mut text = String::from("Hello widgets");
    let mut color = Rgba::rgb(0xCB, 0x72, 0xD9);
    let presets = [Rgba::rgb(0x18, 0x20, 0x28), Rgba::rgb(0xCB, 0x72, 0xD9)];
    // Pre-open the picker and focus the text box so the snapshot shows both.
    h.frame(&click(100.0, 700.0), |_| {});
    let mut build = |ui: &mut Ui| {
        ui.heading("NabyScape");
        ui.label_dim("Render toolkit smoke test");
        ui.card("Controls", |ui| {
            ui.toggle(&mut on, "Switch");
            ui.checkbox(&mut off, "Checkbox");
            ui.slider(&mut v, 0.0..=1.0, 0.1);
            ui.segmented(&["One", "Two", "Three"], &mut seg);
            ui.combo(&mut sel, &["Alpha", "Beta", "Gamma"]);
            ui.text_edit(&mut text, false);
            ui.row_centered(42.0, |ui| {
                ui.primary_button("Save");
                ui.button("Cancel");
                ui.icon_button(Icon::Settings, "Settings");
                ui.icon_button(Icon::Play, "Play");
            });
            ui.progress(0.6);
        });
        ui.color_picker(&mut color, &presets, &[]);
    };
    h.frame(&InputState::default(), &mut build);
    let path = snapshot::save("ui_smoke", &h.canvas);
    let bg = h.theme.background;
    // Heading text, card body, buttons and the picker header all drew something.
    assert!(snapshot::count_differing(&h.canvas, Rect::new(0.0, 0.0, 200.0, 30.0), bg) > 100);
    assert!(snapshot::count_differing(&h.canvas, Rect::new(0.0, 60.0, 420.0, 400.0), bg) > 5000);
    assert!(snapshot::count_differing(&h.canvas, Rect::new(0.0, 420.0, 420.0, 60.0), bg) > 500);
    assert!(path.to_string_lossy().contains("ui_smoke"));
}
