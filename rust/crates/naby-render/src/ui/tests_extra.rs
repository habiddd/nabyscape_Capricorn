use super::tests::{click, Harness};
use super::*;
use crate::snapshot;

#[test]
fn more_menu_opens_and_returns_pick() {
    let mut h = Harness::new(300, 300);
    let labels = ["Rename", "Delete"];
    let mut picked = None;
    // 30x30 button at the left edge.
    h.frame(&click(15.0, 15.0), |ui| {
        ui.more_menu("More", &labels);
    });
    assert!(h.mem.popup.is_some());
    // Popup is right-aligned to the button (clamped into the canvas); rows are 32 px tall.
    h.frame(&click(30.0, 32.0 + 6.0 + 48.0), |ui| {
        picked = picked.or(ui.more_menu("More", &labels));
    });
    assert_eq!(picked, Some(1));
    assert!(h.mem.popup.is_none());
}

#[test]
fn header_stacks_when_narrow_and_aligns_actions_right_when_wide() {
    let mut wide = Harness::new(500, 100);
    let mut narrow = Harness::new(300, 200);
    let mut clicked_wide = false;
    let mut clicked_narrow = false;
    let run = |h: &mut Harness, input: &InputState, flag: &mut bool| {
        h.frame(input, |ui| {
            ui.header(
                |ui| {
                    ui.heading("Title");
                },
                |ui| {
                    *flag |= ui.button("Act").clicked;
                },
            );
        });
    };
    run(&mut wide, &InputState::default(), &mut clicked_wide);
    // Button width = measure("Act") (0 without fonts) + 28; right edge at 500.
    run(&mut wide, &click(490.0, 10.0), &mut clicked_wide);
    assert!(clicked_wide, "actions sit at the right edge");
    run(&mut narrow, &InputState::default(), &mut clicked_narrow);
    // Heading line, 12 px gap, then the action button below it, left aligned.
    run(&mut narrow, &click(10.0, 45.0), &mut clicked_narrow);
    assert!(clicked_narrow);
}

#[test]
fn tile_flow_uses_two_columns_and_full_rows() {
    let mut h = Harness::new(340, 400);
    let mut rects: Vec<Rect> = Vec::new();
    h.frame(&InputState::default(), |ui| {
        rects.clear();
        ui.tile_flow(
            150.0,
            16.0,
            5,
            |i| i == 2,
            |ui, _| {
                rects.push(ui.button("t").rect);
            },
        );
    });
    assert_eq!(rects.len(), 5);
    // 340 wide: two columns of 162.
    assert_eq!(rects[0].w, 162.0);
    assert_eq!(rects[1].x, 162.0 + 16.0);
    assert_eq!(rects[0].y, rects[1].y);
    // Full-width tile starts its own row.
    assert_eq!(rects[2].w, 340.0);
    assert!(rects[2].y > rects[1].y);
    assert_eq!(rects[3].y, rects[4].y);
}

#[test]
fn near_viewport_extends_half_a_viewport_each_way() {
    let mut h = Harness::new(200, 100);
    let mut results = Vec::new();
    h.frame(&InputState::default(), |ui| {
        results.clear();
        ui.scroll_area_h("s", 100.0, |ui| {
            results.push(ui.near_viewport(Rect::new(0.0, 0.0, 10.0, 10.0)));
            results.push(ui.near_viewport(Rect::new(0.0, 120.0, 10.0, 10.0)));
            results.push(ui.near_viewport(Rect::new(0.0, 180.0, 10.0, 10.0)));
            results.push(ui.near_viewport(Rect::new(0.0, -80.0, 10.0, 10.0)));
            results.push(ui.near_viewport(Rect::new(0.0, -40.0, 10.0, 10.0)));
        });
    });
    assert_eq!(results, vec![true, true, false, false, true]);
}

#[test]
fn reveal_dims_until_hovered() {
    let mut h = Harness::new(100, 60);
    let owner = Rect::new(0.0, 0.0, 100.0, 40.0);
    let draw = |ui: &mut Ui| {
        ui.reveal_on_hover(owner, false, |ui| {
            ui.canvas()
                .fill_rect(Rect::new(0.0, 0.0, 10.0, 10.0), Rgba::WHITE);
        });
    };
    h.frame(&InputState::default(), draw);
    let dim = h.canvas.pixel(5, 5);
    h.frame(&InputState::at(50.0, 20.0), draw);
    let bright = h.canvas.pixel(5, 5);
    assert!(dim.r < 60, "{dim:?}");
    assert_eq!(bright, Rgba::WHITE);
}

#[test]
fn progress_ring_arc_is_drawn_clockwise_from_top() {
    let mut h = Harness::new(60, 60);
    h.frame(&InputState::default(), |ui| {
        ui.progress_ring(0.25, 40.0);
    });
    // Quarter turn: top (12 o'clock) and right (3 o'clock) are white, left is the dim track.
    let white = |x, y| h.canvas.pixel(x, y).r > 200;
    assert!(white(20, 3));
    assert!(white(37, 20));
    assert!(!white(3, 20));
    assert!(!white(20, 37));
}

#[test]
fn overlay_snapshot_shows_open_popup_over_later_widgets() {
    let Some(fonts) = snapshot::test_fonts() else {
        return;
    };
    let mut h = Harness::with_fonts(380, 700, fonts);
    let mut color = Rgba::rgb(0x58, 0xA6, 0xFF);
    let mut sel = 0usize;
    let mut text = String::from("caret here");
    let presets = [Rgba::rgb(0x18, 0x20, 0x28), Rgba::rgb(0xCB, 0x72, 0xD9)];
    let recent = [Rgba::rgb(1, 2, 3), Rgba::rgb(0x18, 0x20, 0x28)];
    let items = ["Alpha", "Beta", "Gamma", "Delta"];
    let mut combo_rect = Rect::default();
    macro_rules! ui_frame {
        ($input:expr) => {
            h.frame($input, |ui| {
                ui.color_picker(&mut color, &presets, &recent);
                ui.text_edit(&mut text, false);
                combo_rect = ui.combo(&mut sel, &items).rect;
                ui.label("Under the popup");
                ui.button("Button below");
            });
        };
    }
    // Open the picker, then expand its HEX / RGB values (header 44, field 136, hue 24, 8 gaps).
    ui_frame!(&click(100.0, 20.0));
    ui_frame!(&click(60.0, 44.0 + 8.0 + 136.0 + 8.0 + 24.0 + 8.0 + 10.0));
    ui_frame!(&InputState::default());
    let (cx, cy) = combo_rect.center();
    ui_frame!(&click(cx, cy));
    ui_frame!(&InputState::default());
    assert!(h.mem.popup.is_some());
    snapshot::save("ui_overlays", &h.canvas);
    // The popup list paints over the label and button that sit below the combo.
    let below = Rect::new(combo_rect.x, combo_rect.bottom() + 4.0, combo_rect.w, 60.0);
    let fill = h.theme.control_fill;
    assert!(snapshot::count_differing(&h.canvas, below, h.theme.background) > 3000);
    assert_eq!(
        h.canvas
            .pixel((below.x + 3.0) as u32, (below.y + 3.0) as u32),
        fill
    );
}
