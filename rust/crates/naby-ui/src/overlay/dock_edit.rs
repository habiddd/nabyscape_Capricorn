//! "Move & resize island" mode (C# `DockEditing.cs`): drag handles on the island, nudge keys,
//! and the Done / Cancel buttons the port adds so a mouse-only user can finish.
//!
//! The mode is on while `Settings::dock_unlocked` is set (the tray item flips it, as
//! `ToggleDockEditing` does). The overlay reports it with `Command::SetDockEditing` so the host
//! stops following and tucking, and applies every drag step with `Command::ApplySettings`
//! (the host keeps those in memory until the mode ends). The geometry maths is
//! `naby_core::motion::dock_editing`.

use super::util::{style, text_centered};
use super::Overlay;
use crate::host::{Signal, Snapshots};
use naby_core::color::Rgba;
use naby_core::command::Command;
use naby_core::motion::dock_editing::{
    move_drag, nudge_move, nudge_resize, resize_drag, DockLayout, DragStart, ResizeTarget,
};
use naby_core::motion::follow::Monitor;
use naby_core::settings::Settings;
use naby_render::{Canvas, Fonts, InputState, Key, Rect, Weight};

/// The grips of the editing mode, in Tab order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grip {
    Move,
    ResizeLeft,
    ResizeRight,
    Cancel,
    Done,
}

const ORDER: [Grip; 5] = [
    Grip::Move,
    Grip::ResizeLeft,
    Grip::ResizeRight,
    Grip::Cancel,
    Grip::Done,
];

/// Pointer travel before the mouse is moved with the handle (device pixels).
#[derive(Debug)]
struct Drag {
    grip: Grip,
    /// Layout when this drag began (`Esc` restores it; the maths work from it).
    original: DockLayout,
    start: DragStart,
    /// Screen position of the press.
    pointer: (f64, f64),
}

#[derive(Debug, Default)]
pub(crate) struct DockEdit {
    pub active: bool,
    /// The layout the mode started with (what Cancel restores).
    session: Option<DockLayout>,
    drag: Option<Drag>,
    focus: Option<Grip>,
    hover: Option<Grip>,
    pressed: Option<Grip>,
}

impl DockEdit {
    /// A drag handle is held.
    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }
}

impl Overlay {
    /// Whether the handles are drawn and reachable.
    fn dock_handles_visible(&self) -> bool {
        self.dock_edit.active
            && !self.user_hidden
            && !self.env_hidden
            && !self.tuck.is_tucked()
            && !self.notice_showing
    }

    /// Starts or ends the mode to match `Settings::dock_unlocked`.
    pub(crate) fn dock_edit_sync(&mut self, snaps: &Snapshots) {
        let want = snaps.settings.dock_unlocked;
        if want && !self.dock_edit.active {
            self.dock_edit = DockEdit {
                active: true,
                session: Some(DockLayout::from(&snaps.settings)),
                focus: Some(Grip::Move),
                ..DockEdit::default()
            };
            self.editing_dock = true;
            self.dirty = true;
            self.push(Command::SetDockEditing(true));
        } else if !want && self.dock_edit.active {
            self.dock_edit = DockEdit::default();
            self.editing_dock = false;
            self.dirty = true;
            self.push(Command::SetDockEditing(false));
        }
    }

    /// Hit rectangles in canvas pixels.
    fn grip_rects(&self, snaps: &Snapshots) -> [(Grip, Rect); 5] {
        let s = self.scale;
        let layout = self.layout(snaps);
        let (ox, oy) = layout.origin;
        let w = self.surf.0 as f32 * s;
        let h = self.surf.1 as f32 * s;
        let cx = ox + w / 2.0;
        let button = (58.0 * s, 22.0 * s);
        let top = oy + 7.0 * s;
        [
            (
                Grip::Move,
                Rect::new(cx - 21.0 * s, oy + 1.0 * s, 42.0 * s, 12.0 * s),
            ),
            (
                Grip::ResizeLeft,
                Rect::new(
                    ox + 2.0 * s,
                    (oy + h - 18.0 * s).max(oy),
                    16.0 * s,
                    16.0 * s,
                ),
            ),
            (
                Grip::ResizeRight,
                Rect::new(
                    ox + w - 18.0 * s,
                    (oy + h - 18.0 * s).max(oy),
                    16.0 * s,
                    16.0 * s,
                ),
            ),
            (
                Grip::Cancel,
                Rect::new(cx - 28.0 * s - button.0, top, button.0, button.1),
            ),
            (
                Grip::Done,
                Rect::new(cx + 28.0 * s, top, button.0, button.1),
            ),
        ]
    }

    fn grip_at(&self, snaps: &Snapshots, p: (f32, f32)) -> Option<Grip> {
        self.grip_rects(snaps)
            .into_iter()
            .rev()
            .find(|(_, r)| r.contains_pt(p))
            .map(|(g, _)| g)
    }

    /// The monitor the island is on, as `move_drag` wants it.
    fn edit_monitors(snaps: &Snapshots) -> Vec<Monitor> {
        let d = &snaps.display;
        vec![Monitor {
            id: d.id.clone(),
            bounds: d.bounds,
            work: d.work,
            primary: true,
            scale: f64::from(d.scale),
            refresh_hz: d.refresh_hz,
        }]
    }

    fn resize_target(&self) -> ResizeTarget {
        if self.expanded {
            ResizeTarget::Panel {
                painted_height: self.surf.1,
            }
        } else if self.peeking {
            ResizeTarget::Hover
        } else {
            ResizeTarget::Closed
        }
    }

    fn screen_point(&self, snaps: &Snapshots, p: (f32, f32)) -> (f64, f64) {
        let w = self.layout(snaps).window;
        (
            f64::from(w.0) + f64::from(p.0),
            f64::from(w.1) + f64::from(p.1),
        )
    }

    /// Applies edited settings when they differ from what is shown.
    fn push_edit(&mut self, snaps: &Snapshots, edited: Settings) {
        if edited != **snaps.settings.arc() {
            self.push(Command::ApplySettings(Box::new(edited)));
            self.dirty = true;
        }
    }

    fn edit_settings(snaps: &Snapshots) -> Settings {
        (**snaps.settings.arc()).clone()
    }

    /// Leaves the mode keeping the edited layout (Done, Enter).
    fn finish_dock_edit(&mut self, snaps: &Snapshots) {
        let mut s = Self::edit_settings(snaps);
        s.dock_unlocked = false;
        self.push(Command::ApplySettings(Box::new(s)));
        self.dock_edit.drag = None;
        self.dirty = true;
    }

    /// Leaves the mode restoring the layout it started with (Cancel, Esc).
    fn cancel_dock_edit(&mut self, snaps: &Snapshots) {
        let mut s = Self::edit_settings(snaps);
        if let Some(original) = &self.dock_edit.session {
            original.restore(&mut s);
        }
        s.dock_unlocked = false;
        self.push(Command::ApplySettings(Box::new(s)));
        self.dock_edit.drag = None;
        self.dirty = true;
    }

    fn begin_drag(&mut self, snaps: &Snapshots, grip: Grip, p: (f32, f32)) {
        if self.tuck.is_tucked() || self.animating() {
            return;
        }
        let s = f64::from(self.scale);
        let layout = self.layout(snaps);
        let w = self.surf.0 * s;
        self.dock_edit.drag = Some(Drag {
            grip,
            original: DockLayout::from(&snaps.settings),
            start: DragStart {
                pointer: self.screen_point(snaps, p),
                center_x: f64::from(layout.window.0) + f64::from(layout.origin.0) + w / 2.0,
                top: f64::from(layout.window.1) + f64::from(layout.origin.1),
                painted_height: self.surf.1,
            },
            pointer: self.screen_point(snaps, p),
        });
    }

    fn drag_to(&mut self, snaps: &Snapshots, p: (f32, f32)) {
        let Some(drag) = &self.dock_edit.drag else {
            return;
        };
        let pointer = self.screen_point(snaps, p);
        let mut s = Self::edit_settings(snaps);
        match drag.grip {
            Grip::Move => {
                let monitors = Self::edit_monitors(snaps);
                let _ = move_drag(&mut s, &drag.original, &drag.start, pointer, &monitors);
            }
            Grip::ResizeLeft | Grip::ResizeRight => {
                let delta = (pointer.0 - drag.pointer.0, pointer.1 - drag.pointer.1);
                resize_drag(
                    &mut s,
                    &drag.original,
                    self.resize_target(),
                    delta,
                    drag.grip == Grip::ResizeLeft,
                    f64::from(snaps.display.scale),
                );
            }
            _ => return,
        }
        self.push_edit(snaps, s);
    }

    fn nudge(&mut self, snaps: &Snapshots, dx: f64, dy: f64) {
        let mut s = Self::edit_settings(snaps);
        match self.dock_edit.focus.unwrap_or(Grip::Move) {
            Grip::Move => nudge_move(&mut s, dx, dy),
            Grip::ResizeLeft | Grip::ResizeRight => {
                nudge_resize(&mut s, self.resize_target(), dx, dy)
            }
            _ => return,
        }
        self.push_edit(snaps, s);
    }

    fn activate_grip(&mut self, snaps: &Snapshots, grip: Grip) {
        match grip {
            Grip::Done => self.finish_dock_edit(snaps),
            Grip::Cancel => self.cancel_dock_edit(snaps),
            _ => {}
        }
    }

    /// Keyboard of the mode. True when a key was consumed.
    fn dock_edit_keys(&mut self, input: &InputState, snaps: &Snapshots) -> bool {
        let mut used = false;
        for k in &input.keys {
            match k.key {
                Key::Escape => {
                    if let Some(drag) = self.dock_edit.drag.take() {
                        let mut s = Self::edit_settings(snaps);
                        drag.original.restore(&mut s);
                        self.push_edit(snaps, s);
                    } else {
                        self.cancel_dock_edit(snaps);
                    }
                }
                Key::Enter => match self.dock_edit.focus {
                    Some(Grip::Cancel) => self.cancel_dock_edit(snaps),
                    _ => self.finish_dock_edit(snaps),
                },
                Key::Space => {
                    if let Some(g) = self.dock_edit.focus {
                        self.activate_grip(snaps, g);
                    }
                }
                Key::Tab => {
                    let at = ORDER
                        .iter()
                        .position(|g| Some(*g) == self.dock_edit.focus)
                        .unwrap_or(0);
                    let next = if k.shift {
                        (at + ORDER.len() - 1) % ORDER.len()
                    } else {
                        (at + 1) % ORDER.len()
                    };
                    self.dock_edit.focus = Some(ORDER[next]);
                }
                Key::Left => self.nudge(snaps, -2.0, 0.0),
                Key::Right => self.nudge(snaps, 2.0, 0.0),
                Key::Up => self.nudge(snaps, 0.0, -2.0),
                Key::Down => self.nudge(snaps, 0.0, 2.0),
                _ => continue,
            }
            used = true;
        }
        used
    }

    /// Input of the mode, before the dock's normal handling. True = the input was used (the
    /// dock must not also react) and the surface needs a repaint.
    pub(crate) fn dock_edit_input(
        &mut self,
        input: &InputState,
        snaps: &Snapshots,
    ) -> (bool, bool) {
        if !self.dock_edit.active || !self.dock_handles_visible() {
            return (false, false);
        }
        let mut changed = false;
        let over = input.pointer.and_then(|p| self.grip_at(snaps, p));
        if over != self.dock_edit.hover {
            self.dock_edit.hover = over;
            changed = true;
        }
        let mut consumed = false;
        if self.dock_edit_keys(input, snaps) {
            changed = true;
            consumed = true;
        }
        if let Some(p) = input.pointer {
            if input.pressed {
                if let Some(grip) = over {
                    self.dock_edit.focus = Some(grip);
                    match grip {
                        Grip::Move | Grip::ResizeLeft | Grip::ResizeRight => {
                            self.begin_drag(snaps, grip, p)
                        }
                        _ => self.dock_edit.pressed = Some(grip),
                    }
                    changed = true;
                    consumed = true;
                }
            } else if input.down && self.dock_edit.dragging() {
                self.drag_to(snaps, p);
                changed = true;
                consumed = true;
            }
        }
        if input.released {
            if self.dock_edit.drag.take().is_some() {
                changed = true;
                consumed = true;
            }
            if let Some(grip) = self.dock_edit.pressed.take() {
                if over == Some(grip) {
                    self.activate_grip(snaps, grip);
                }
                changed = true;
                consumed = true;
            }
        }
        if changed {
            self.dirty = true;
        }
        (consumed, changed)
    }

    /// Escape as a host signal ends the mode like the Cancel button.
    pub(crate) fn dock_edit_signal(&mut self, signal: Signal, snaps: &Snapshots) -> bool {
        if self.dock_edit.active && signal == Signal::Escape {
            self.cancel_dock_edit(snaps);
            return true;
        }
        false
    }

    /// Draws the handles over the island.
    pub(crate) fn paint_dock_edit(
        &mut self,
        canvas: &mut Canvas,
        fonts: &Fonts,
        snaps: &Snapshots,
    ) {
        if !self.dock_handles_visible() {
            return;
        }
        let s = self.scale;
        let k = self.cfg.theme.text_scale;
        let theme = self.cfg.theme.clone();
        let grey = Rgba::rgb(0x98, 0x9D, 0xA7);
        for (grip, r) in self.grip_rects(snaps) {
            let hot = self.dock_edit.hover == Some(grip)
                || self.dock_edit.drag.as_ref().is_some_and(|d| d.grip == grip);
            let focused = self.dock_edit.focus == Some(grip);
            match grip {
                Grip::Move | Grip::ResizeLeft | Grip::ResizeRight => {
                    let alpha = if hot { 1.0 } else { 0.7 };
                    let c = grey.with_alpha((255.0 * alpha) as u8);
                    let (cx, cy) = r.center();
                    let bar = if grip == Grip::Move {
                        Rect::new(cx - 13.0 * s, cy - 1.5 * s, 26.0 * s, 3.0 * s)
                    } else {
                        Rect::new(cx - 3.0 * s, cy - 3.0 * s, 6.0 * s, 6.0 * s)
                    };
                    canvas.fill_rrect(bar, 2.0 * s, c);
                    if hot || focused {
                        canvas.stroke_round_rect(
                            r.inset(1.0 * s),
                            4.0 * s,
                            theme.text.with_alpha(if focused { 160 } else { 70 }),
                            1.0,
                        );
                    }
                }
                Grip::Cancel | Grip::Done => {
                    let done = grip == Grip::Done;
                    let fill = if done {
                        theme.accent
                    } else {
                        Rgba::BLACK.with_alpha(150)
                    };
                    canvas.fill_rrect(r, 8.0 * s, fill);
                    canvas.stroke_round_rect(r, 8.0 * s, theme.control_border, 1.0);
                    if hot {
                        canvas.fill_rrect(r, 8.0 * s, theme.hover_fill);
                    }
                    if focused {
                        canvas.stroke_round_rect(r.inset(1.0 * s), 7.0 * s, theme.text, 1.5);
                    }
                    let color = if done { theme.accent_text } else { theme.text };
                    let st = style(
                        11.0 * s * k,
                        if done {
                            Weight::Semibold
                        } else {
                            Weight::Regular
                        },
                        color,
                    );
                    text_centered(canvas, fonts, r, if done { "Done" } else { "Cancel" }, &st);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{quiet_settings, Rig};
    use super::*;
    use crate::host::View;
    use naby_core::settings::Settings;
    use naby_render::{snapshot, KeyEvent};

    fn editing_settings() -> Settings {
        Settings {
            preset: "iPhone".into(),
            dock_edge: "Floating".into(),
            top: 20.0,
            horizontal: 50.0,
            width: 240.0,
            height: 44.0,
            dock_unlocked: true,
            ..quiet_settings()
        }
    }

    /// A rig whose host applies `ApplySettings` like `OverlayHost` does.
    struct Edit {
        rig: Rig,
        sent: Vec<Command>,
    }

    impl Edit {
        fn new() -> Edit {
            let mut e = Edit {
                rig: Rig::new(editing_settings()),
                sent: Vec::new(),
            };
            e.input(InputState::default());
            e
        }

        fn host(&mut self) {
            for command in self.rig.commands() {
                if let Command::ApplySettings(s) = &command {
                    self.rig.snaps.settings.replace((**s).clone());
                }
                self.sent.push(command);
            }
            self.rig.overlay.tick(&self.rig.snaps, &mut self.rig.out);
        }

        fn input(&mut self, input: InputState) {
            self.rig.input(input);
            self.host();
        }

        fn settings(&self) -> Settings {
            (**self.rig.snaps.settings.arc()).clone()
        }

        fn grip(&self, grip: Grip) -> (f32, f32) {
            self.rig
                .overlay
                .grip_rects(&self.rig.snaps)
                .into_iter()
                .find(|(g, _)| *g == grip)
                .map(|(_, r)| r.center())
                .unwrap()
        }

        fn drag(&mut self, grip: Grip, dx: f32, dy: f32) {
            let (x, y) = self.grip(grip);
            self.input(InputState::at(x, y));
            self.input(InputState::press(x, y));
            self.input(InputState::drag(x + dx, y + dy));
            self.input(InputState::release(x + dx, y + dy));
        }

        fn key(&mut self, key: Key) {
            self.input(InputState::key(KeyEvent::new(key)));
        }
    }

    #[test]
    fn unlocking_starts_the_mode_and_locking_ends_it() {
        let mut e = Edit::new();
        assert!(e.sent.contains(&Command::SetDockEditing(true)));
        assert!(e.rig.overlay.interaction_active());
        let mut s = e.settings();
        s.dock_unlocked = false;
        e.rig.snaps.settings.replace(s);
        e.input(InputState::default());
        assert!(e.sent.contains(&Command::SetDockEditing(false)));
        assert!(!e.rig.overlay.interaction_active());
    }

    #[test]
    fn dragging_the_move_grip_moves_the_island() {
        let mut e = Edit::new();
        let before = e.settings();
        e.drag(Grip::Move, 60.0, 30.0);
        let after = e.settings();
        assert!(after.horizontal > before.horizontal, "{}", after.horizontal);
        assert!(after.top > before.top);
        assert_eq!(after.display_mode, "Selected");
        assert!(after.dock_unlocked, "still editing after a drag");
        // The press did not reach the dock: it did not open.
        assert_eq!(e.rig.state(), crate::OverlayState::Closed);
    }

    #[test]
    fn dragging_a_resize_grip_changes_the_island_size() {
        let mut e = Edit::new();
        let before = e.settings();
        e.drag(Grip::ResizeRight, 20.0, 6.0);
        let after = e.settings();
        assert_eq!(after.width, (before.width + 40.0).min(400.0));
        assert_eq!(after.height, before.height + 6.0);
        // The left grip mirrors the horizontal growth.
        e.drag(Grip::ResizeLeft, -10.0, 0.0);
        assert_eq!(e.settings().width, after.width + 20.0);
    }

    #[test]
    fn arrow_keys_nudge_the_focused_grip() {
        let mut e = Edit::new();
        let before = e.settings();
        e.key(Key::Right);
        assert_eq!(e.settings().horizontal, before.horizontal + 1.0);
        e.key(Key::Down);
        assert_eq!(e.settings().top, before.top + 2.0);
        // Tab to the left resize grip: arrows resize.
        e.key(Key::Tab);
        e.key(Key::Right);
        assert_eq!(e.settings().width, before.width + 2.0);
        e.key(Key::Down);
        assert_eq!(e.settings().height, before.height + 2.0);
    }

    #[test]
    fn done_keeps_the_layout_and_unlocks() {
        let mut e = Edit::new();
        e.drag(Grip::Move, 40.0, 0.0);
        let moved = e.settings().horizontal;
        let (x, y) = e.grip(Grip::Done);
        e.input(InputState::at(x, y));
        e.input(InputState::press(x, y));
        e.input(InputState::release(x, y));
        let s = e.settings();
        assert!(!s.dock_unlocked);
        assert_eq!(s.horizontal, moved);
        e.input(InputState::default());
        assert!(e.sent.contains(&Command::SetDockEditing(false)));
    }

    #[test]
    fn cancel_and_escape_restore_the_starting_layout() {
        for escape in [false, true] {
            let mut e = Edit::new();
            let before = e.settings();
            e.drag(Grip::Move, 80.0, 40.0);
            e.drag(Grip::ResizeRight, 30.0, 10.0);
            assert_ne!(e.settings().width, before.width);
            if escape {
                e.key(Key::Escape);
            } else {
                let (x, y) = e.grip(Grip::Cancel);
                e.input(InputState::at(x, y));
                e.input(InputState::press(x, y));
                e.input(InputState::release(x, y));
            }
            let after = e.settings();
            assert!(!after.dock_unlocked);
            assert_eq!(after.horizontal, before.horizontal);
            assert_eq!(after.top, before.top);
            assert_eq!(after.width, before.width);
            assert_eq!(after.height, before.height);
        }
    }

    #[test]
    fn escape_during_a_drag_restores_only_that_drag() {
        let mut e = Edit::new();
        e.drag(Grip::ResizeRight, 20.0, 0.0);
        let first = e.settings();
        let (x, y) = e.grip(Grip::ResizeRight);
        e.input(InputState::at(x, y));
        e.input(InputState::press(x, y));
        e.input(InputState::drag(x + 30.0, y));
        assert_ne!(e.settings().width, first.width);
        e.key(Key::Escape);
        assert_eq!(e.settings().width, first.width);
        assert!(e.settings().dock_unlocked, "the mode goes on");
    }

    #[test]
    fn a_tucked_dock_has_no_handles() {
        let mut e = Edit::new();
        e.rig.signal(Signal::ToggleTuck);
        e.rig.settle();
        assert!(e.rig.overlay.is_tucked());
        let (x, y) = e.grip(Grip::Move);
        e.input(InputState::press(x, y));
        e.input(InputState::drag(x + 50.0, y));
        assert_eq!(e.settings().horizontal, 50.0);
    }

    #[test]
    fn snapshot_of_the_handles() {
        let Some(mut e) = snapshot::test_fonts().map(|_| Edit::new()) else {
            return;
        };
        e.rig.overlay.dock_edit.hover = Some(Grip::Move);
        e.rig.paint();
        snapshot::save("dock-edit-closed", &e.rig.canvas);
        let (w, h) = (e.rig.canvas.width(), e.rig.canvas.height());
        assert!(w > 100 && h > 20);
    }
}
