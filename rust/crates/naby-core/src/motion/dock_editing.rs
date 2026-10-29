//! Dock position editing maths (C# `UI/Island/DockEditing.cs`, `UI/Settings/DockPositionEditor.cs`)
//! and the layout-input change detector (C# `OverlaySettingsState`).
//!
//! Functions mutate a [`Settings`] value the way the C# handlers do; the host calls
//! `Settings::normalize` afterwards (C# `ChangeSettings`).

use super::follow::{monitor_at, Monitor};
use super::geometry::RectI;
use crate::settings::{DockEdge, Settings};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

// ---------------------------------------------------------------------------------------------
// Drag / nudge editing of the live island
// ---------------------------------------------------------------------------------------------

/// Snapshot of the layout inputs, restored when an edit is cancelled (C# `DockLayout`).
#[derive(Debug, Clone, PartialEq)]
pub struct DockLayout {
    pub horizontal: f64,
    pub top: f64,
    pub width: f64,
    pub height: f64,
    pub panel_width: f64,
    pub panel_height: f64,
    pub hover_width: f64,
    pub hover_height: f64,
    pub preset: String,
    pub monitor: String,
    pub mode: String,
    pub auto_height: bool,
    pub edge: String,
    pub vertical: f64,
    pub inset: f64,
}

impl DockLayout {
    pub fn from(s: &Settings) -> DockLayout {
        DockLayout {
            horizontal: s.horizontal,
            top: s.top,
            width: s.width,
            height: s.height,
            panel_width: s.panel_width,
            panel_height: s.panel_max_height,
            hover_width: s.hover_width,
            hover_height: s.hover_height,
            preset: s.preset.clone(),
            monitor: s.monitor.clone(),
            mode: s.display_mode.clone(),
            auto_height: s.panel_auto_height,
            edge: s.dock_edge.clone(),
            vertical: s.dock_vertical,
            inset: s.dock_inset,
        }
    }

    pub fn restore(&self, s: &mut Settings) {
        s.horizontal = self.horizontal;
        s.top = self.top;
        s.width = self.width;
        s.height = self.height;
        s.panel_width = self.panel_width;
        s.panel_max_height = self.panel_height;
        s.hover_width = self.hover_width;
        s.hover_height = self.hover_height;
        s.preset = self.preset.clone();
        s.monitor = self.monitor.clone();
        s.display_mode = self.mode.clone();
        s.panel_auto_height = self.auto_height;
        s.dock_edge = self.edge.clone();
        s.dock_vertical = self.vertical;
        s.dock_inset = self.inset;
    }
}

/// Which surface a resize handle edits.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ResizeTarget {
    /// Expanded panel; `painted_height` is its current height in DIP.
    Panel {
        painted_height: f64,
    },
    Hover,
    Closed,
}

/// Drag origin captured on `DragStarted`, physical pixels except `painted_height` (DIP).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DragStart {
    pub pointer: (f64, f64),
    /// Screen x of the island's centre.
    pub center_x: f64,
    /// Screen y of the island's top edge.
    pub top: f64,
    pub painted_height: f64,
}

/// Result of a move drag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MoveResult {
    /// The Mac notch applies (`Preset == Mac && DockEdge == Top`).
    pub notch: bool,
}

/// C# move-grip `DragDelta`: free-moves the island, picking the monitor under the pointer,
/// snapping to the horizontal centre within 0.7 % and to the top notch within 8 DIP.
pub fn move_drag(
    s: &mut Settings,
    original: &DockLayout,
    start: &DragStart,
    pointer: (f64, f64),
    monitors: &[Monitor],
) -> Option<MoveResult> {
    let (dx, dy) = (pointer.0 - start.pointer.0, pointer.1 - start.pointer.1);
    let screen = monitor_at(monitors, (start.center_x + dx) as i32, pointer.1 as i32)?;
    let scale = screen.scale;
    let compact = s.width * scale;
    let work = screen.work.to_f();
    s.monitor = screen.id.clone();
    s.display_mode = "Selected".to_string();
    s.horizontal = ((start.center_x + dx - work.x - compact / 2.0) / (work.w - compact).max(1.0)
        * 100.0)
        .clamp(0.0, 100.0);
    let top = ((start.top + dy - work.y) / scale)
        .clamp(0.0, (work.h / scale - start.painted_height).max(0.0));
    if (s.horizontal - 50.0).abs() < 0.7 {
        s.horizontal = 50.0;
    }
    let mac_top = original.preset == "Mac" && top <= 8.0;
    s.dock_edge = if mac_top { "Top" } else { "Floating" }.to_string();
    s.preset = if mac_top { "Mac" } else { "iPhone" }.to_string();
    s.top = if s.preset == "Mac" { 0.0 } else { top };
    Some(MoveResult {
        notch: s.preset == "Mac" && s.dock_edge() == DockEdge::Top,
    })
}

/// C# resize-grip `DragDelta`. `delta` is the pointer movement in physical pixels, `left`
/// whether the left handle is dragged (mirrors the horizontal growth), `scale` the monitor DPI.
pub fn resize_drag(
    s: &mut Settings,
    original: &DockLayout,
    target: ResizeTarget,
    delta: (f64, f64),
    left: bool,
    scale: f64,
) {
    let scale = if scale > 0.0 { scale } else { 1.0 };
    let dx = delta.0 / scale * if left { -2.0 } else { 2.0 };
    let dy = delta.1 / scale;
    match target {
        ResizeTarget::Panel { painted_height } => {
            s.panel_width = (original.panel_width + dx).clamp(300.0, 960.0);
            if dy.abs() > 2.0 {
                s.panel_auto_height = false;
                s.panel_max_height = (painted_height + dy).clamp(240.0, 1100.0);
            }
        }
        ResizeTarget::Hover => {
            s.hover_width = (original.hover_width + dx).clamp(300.0, 760.0);
            s.hover_height = (original.hover_height + dy).clamp(184.0, 440.0);
        }
        ResizeTarget::Closed => {
            s.width = (original.width + dx).clamp(180.0, 400.0);
            s.height = (original.height + dy).clamp(28.0, 72.0);
        }
    }
}

/// Arrow key on the move grip: 2 DIP steps (1 % for the percent-based axes).
pub fn nudge_move(s: &mut Settings, dx: f64, dy: f64) {
    match s.dock_edge() {
        DockEdge::Left | DockEdge::Right => {
            s.dock_vertical = (s.dock_vertical + dy / 2.0).clamp(0.0, 100.0);
            let toward = if s.dock_edge() == DockEdge::Left {
                dx
            } else {
                -dx
            };
            s.dock_inset = (s.dock_inset + toward).clamp(0.0, 200.0);
        }
        edge => {
            s.horizontal = (s.horizontal + dx / 2.0).clamp(0.0, 100.0);
            if edge == DockEdge::Bottom {
                s.dock_inset = (s.dock_inset - dy).clamp(0.0, 200.0);
            } else if s.preset != "Mac" || edge == DockEdge::Floating {
                s.top = (s.top + dy).max(0.0);
            }
        }
    }
}

/// Arrow key on a resize grip.
pub fn nudge_resize(s: &mut Settings, target: ResizeTarget, dx: f64, dy: f64) {
    match target {
        ResizeTarget::Panel { painted_height } => {
            s.panel_width += dx;
            if dy != 0.0 {
                s.panel_auto_height = false;
                s.panel_max_height = painted_height + dy;
            }
        }
        ResizeTarget::Hover => {
            s.hover_width += dx;
            s.hover_height += dy;
        }
        ResizeTarget::Closed => {
            s.width += dx;
            s.height += dy;
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Settings page position editor
// ---------------------------------------------------------------------------------------------

/// Height of the editor's screen map in DIP.
pub const MAP_HEIGHT: f64 = 132.0;

/// Pure model of `DockPositionEditor`: sliders, labels and the screen map.
#[derive(Debug, Clone, PartialEq)]
pub struct DockPositionModel {
    pub horizontal: f64,
    pub vertical_min: f64,
    pub vertical_max: f64,
    pub vertical_step: f64,
    pub vertical_value: f64,
    pub vertical_label: &'static str,
    pub vertical_enabled: bool,
    pub left: f64,
    pub top: f64,
    pub alignment: String,
    pub centered: bool,
    pub coordinates: String,
}

impl DockPositionModel {
    /// C# `Sync`. `work_dip` is the target monitor's work area in DIP, `scale` its DPI scale.
    pub fn sync(s: &Settings, work_dip: (f64, f64), scale: f64) -> DockPositionModel {
        let (work_w, height) = work_dip;
        let edge = s.dock_edge();
        let side = matches!(edge, DockEdge::Left | DockEdge::Right);
        let bottom = edge == DockEdge::Bottom;
        let (vertical_max, vertical_step) = if side {
            (100.0, 0.1)
        } else if bottom {
            (200.0, 1.0)
        } else {
            (3000f64.min((height - s.height).max(0.0)), 1.0)
        };
        let value = if side {
            s.dock_vertical
        } else if bottom {
            s.dock_inset
        } else {
            s.top
        };
        let mut left = (work_w - s.width) * s.horizontal / 100.0;
        let top = if side {
            (height - s.height) * s.dock_vertical / 100.0
        } else if bottom {
            height - s.height - s.dock_inset
        } else {
            s.top
        };
        if side {
            left = if edge == DockEdge::Left {
                s.dock_inset
            } else {
                work_w - s.width - s.dock_inset
            };
        }
        let off = if side {
            s.dock_vertical - 50.0
        } else {
            s.horizontal - 50.0
        };
        let centered = off.abs() < 0.05;
        let alignment = if centered {
            "Centered on this screen".to_string()
        } else {
            let word = match (side, off < 0.0) {
                (true, true) => "above",
                (true, false) => "below",
                (false, true) => "left of",
                (false, false) => "right of",
            };
            format!("{}% {word} center", format_trimmed(off.abs()))
        };
        DockPositionModel {
            horizontal: s.horizontal,
            vertical_min: 0.0,
            vertical_max,
            vertical_step,
            vertical_value: value,
            vertical_label: if side {
                "Vertical \u{b7} %"
            } else if bottom {
                "Bottom inset \u{b7} px"
            } else {
                "Top offset \u{b7} px"
            },
            vertical_enabled: !(s.preset == "Mac" && edge == DockEdge::Top),
            left,
            top,
            alignment,
            centered,
            coordinates: format!(
                "Left {:.0} px \u{b7} Top {:.0} px \u{b7} {:.0}% display scaling",
                left,
                top,
                scale * 100.0
            ),
        }
    }

    /// Horizontal text box / slider text (`0.0`).
    pub fn horizontal_text(&self) -> String {
        format!("{:.1}", self.horizontal)
    }

    /// Vertical text box text (`0.#`).
    pub fn vertical_text(&self) -> String {
        format_trimmed(self.vertical_value)
    }

    /// Vertical slider change: writes the axis that applies to the current edge.
    pub fn set_vertical(s: &mut Settings, value: f64) {
        match s.dock_edge() {
            DockEdge::Left | DockEdge::Right => s.dock_vertical = value,
            DockEdge::Bottom => s.dock_inset = value,
            _ => s.top = value,
        }
    }

    /// "Center" button.
    pub fn center(s: &mut Settings) {
        s.horizontal = 50.0;
        s.dock_vertical = 50.0;
    }

    /// Text-box commit: invariant-culture float, clamped to the slider range.
    pub fn commit_text(text: &str, min: f64, max: f64) -> Option<f64> {
        text.trim()
            .parse::<f64>()
            .ok()
            .filter(|v| v.is_finite())
            .map(|v| v.clamp(min, max))
    }

    /// C# `Drag`: pointer position on the map (DIP) → settings, snapping to centre guides
    /// within 1 %.
    pub fn map_drag(
        s: &mut Settings,
        point: (f64, f64),
        map_width: f64,
        map_height: f64,
        vertical_max: f64,
    ) {
        let w = (map_width - 32.0).max(1.0);
        let h = map_height - 24.0;
        let mut px = ((point.0 - 45.0) / (w - 58.0).max(1.0) * 100.0).clamp(0.0, 100.0);
        let mut py = ((point.1 - 19.5) / (h - 15.0).max(1.0) * 100.0).clamp(0.0, 100.0);
        if (px - 50.0).abs() < 1.0 {
            px = 50.0;
        }
        if (py - 50.0).abs() < 1.0 {
            py = 50.0;
        }
        let edge = s.dock_edge();
        if matches!(edge, DockEdge::Left | DockEdge::Right) {
            s.dock_vertical = py;
        } else {
            s.horizontal = px;
        }
        if edge == DockEdge::Floating || edge == DockEdge::Top && s.preset != "Mac" {
            s.top = (py / 100.0 * vertical_max).round_ties_even();
        }
    }

    /// C# `Draw`: top-left of the 58 x 15 dock marker inside the map, `None` while the map
    /// is too narrow to draw.
    pub fn marker(s: &Settings, vertical_max: f64, map_width: f64) -> Option<(f64, f64)> {
        if map_width <= 32.0 {
            return None;
        }
        let w = map_width - 32.0;
        let h = MAP_HEIGHT - 24.0;
        let edge = s.dock_edge();
        let mut dx = s.horizontal / 100.0;
        let dy = match edge {
            DockEdge::Bottom => 1.0,
            DockEdge::Left | DockEdge::Right => s.dock_vertical / 100.0,
            DockEdge::Floating => s.top / vertical_max.max(1.0),
            DockEdge::Top if s.preset != "Mac" => s.top / vertical_max.max(1.0),
            DockEdge::Top => 0.0,
        };
        if edge == DockEdge::Left {
            dx = 0.0;
        }
        if edge == DockEdge::Right {
            dx = 1.0;
        }
        Some((16.0 + dx * (w - 58.0), 12.0 + dy * (h - 15.0)))
    }
}

/// .NET `ToString("0.#")` (one optional decimal, no trailing zero).
fn format_trimmed(v: f64) -> String {
    let rounded = (v * 10.0).round() / 10.0;
    if (rounded - rounded.trunc()).abs() < 1e-9 {
        format!("{}", rounded.trunc() as i64)
    } else {
        format!("{rounded:.1}")
    }
}

// ---------------------------------------------------------------------------------------------
// Overlay settings change detection
// ---------------------------------------------------------------------------------------------

/// Hashes of the layout inputs only, never personal data (C# `OverlaySettingsState`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverlaySettingsState {
    pub content: u64,
    pub reflow: u64,
    pub geometry: u64,
    pub hotkeys: u64,
}

fn bits(v: f64) -> u64 {
    v.to_bits()
}

impl OverlaySettingsState {
    pub fn capture(s: &Settings) -> OverlaySettingsState {
        let mut content = DefaultHasher::new();
        (
            s.compact_layout,
            &s.category_style,
            &s.launch_style,
            s.live_activity,
            s.monitor_network,
            s.monitor_storage,
            s.monitor_processes,
        )
            .hash(&mut content);
        for w in &s.widgets {
            (&w.id, w.enabled).hash(&mut content);
        }
        for t in &s.tools {
            (&t.id, t.enabled).hash(&mut content);
        }
        for sc in &s.shortcuts {
            (&sc.name, &sc.target, &sc.app_id).hash(&mut content);
        }
        let mut reflow = DefaultHasher::new();
        (
            bits(s.panel_width),
            bits(s.panel_padding),
            bits(s.panel_scale),
            bits(s.section_gap),
        )
            .hash(&mut reflow);
        let mut geometry = DefaultHasher::new();
        (
            bits(s.width),
            bits(s.height),
            bits(s.radius),
            &s.preset,
            bits(s.horizontal),
            bits(s.top),
            &s.monitor,
        )
            .hash(&mut geometry);
        (
            bits(s.panel_max_height),
            bits(s.hover_width),
            bits(s.hover_height),
            &s.display_mode,
            s.panel_auto_height,
            &s.dock_edge,
            bits(s.dock_vertical),
            bits(s.dock_inset),
        )
            .hash(&mut geometry);
        let mut hotkeys = DefaultHasher::new();
        (
            s.tuck_hotkey_enabled,
            &s.tuck_hotkey_modifiers,
            &s.tuck_hotkey_key,
            s.hotkey_enabled,
            &s.hotkey_modifiers,
            &s.hotkey_key,
            s.visibility_hotkey_enabled,
            &s.visibility_hotkey_modifiers,
            &s.visibility_hotkey_key,
        )
            .hash(&mut hotkeys);
        OverlaySettingsState {
            content: content.finish(),
            reflow: reflow.finish(),
            geometry: geometry.finish(),
            hotkeys: hotkeys.finish(),
        }
    }

    /// Content changed: the body must be rebuilt rather than updated incrementally.
    pub fn needs_rebuild(&self, previous: &OverlaySettingsState) -> bool {
        self.content != previous.content
    }

    pub fn geometry_changed(&self, previous: &OverlaySettingsState) -> bool {
        self.geometry != previous.geometry || self.reflow != previous.reflow
    }

    pub fn hotkeys_changed(&self, previous: &OverlaySettingsState) -> bool {
        self.hotkeys != previous.hotkeys
    }
}

/// Work-area rectangle in DIP for the editor and geometry helpers.
pub fn work_dip(work: RectI, scale: f64) -> (f64, f64) {
    (f64::from(work.w) / scale, f64::from(work.h) / scale)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{Shortcut, WidgetSetting};

    fn monitors() -> Vec<Monitor> {
        vec![
            Monitor {
                id: "A".into(),
                bounds: RectI::new(0, 0, 1920, 1080),
                work: RectI::new(0, 0, 1920, 1040),
                primary: true,
                scale: 1.0,
                refresh_hz: 60,
            },
            Monitor {
                id: "B".into(),
                bounds: RectI::new(1920, 0, 2880, 1620),
                work: RectI::new(1920, 0, 2880, 1580),
                primary: false,
                scale: 1.5,
                refresh_hz: 120,
            },
        ]
    }

    fn start(s: &Settings) -> DragStart {
        let _ = s;
        DragStart {
            pointer: (960.0, 20.0),
            center_x: 960.0,
            top: 8.0,
            painted_height: 44.0,
        }
    }

    #[test]
    fn dock_layout_restores_everything_on_cancel() {
        let mut s = Settings::default();
        let before = DockLayout::from(&s);
        let original = s.clone();
        s.horizontal = 3.0;
        s.top = 400.0;
        s.width = 222.0;
        s.preset = "iPhone".into();
        s.dock_edge = "Left".into();
        s.display_mode = "Selected".into();
        s.monitor = "X".into();
        s.panel_auto_height = !original.panel_auto_height;
        before.restore(&mut s);
        assert_eq!(s, original);
    }

    #[test]
    fn move_drag_snaps_center_and_picks_monitor() {
        let mut s = Settings {
            preset: "Mac".into(),
            ..Settings::default()
        };
        let original = DockLayout::from(&s);
        let st = start(&s);
        // Drag straight down 200 px on monitor A: floating iPhone preset at top = 208.
        let r = move_drag(&mut s, &original, &st, (960.0, 220.0), &monitors()).unwrap();
        assert!(!r.notch);
        assert_eq!(
            (s.dock_edge.as_str(), s.preset.as_str(), s.monitor.as_str()),
            ("Floating", "iPhone", "A")
        );
        assert_eq!(s.display_mode, "Selected");
        assert_eq!(s.top, 208.0);
        assert_eq!(s.horizontal, 50.0, "snaps to centre");
        // Dropping back within 8 DIP of the top re-docks the notch for a Mac preset.
        let r = move_drag(&mut s, &original, &st, (960.0, 18.0), &monitors()).unwrap();
        assert!(r.notch);
        assert_eq!(
            (s.dock_edge.as_str(), s.preset.as_str(), s.top),
            ("Top", "Mac", 0.0)
        );
        // Dragging onto the 150 % monitor uses its work area and scale.
        let r = move_drag(&mut s, &original, &st, (3000.0, 300.0), &monitors()).unwrap();
        assert!(!r.notch);
        assert_eq!(s.monitor, "B");
        let compact = s.width * 1.5;
        let expected = ((960.0 + 2040.0 - 1920.0 - compact / 2.0) / (2880.0 - compact) * 100.0)
            .clamp(0.0, 100.0);
        assert!((s.horizontal - expected).abs() < 1e-9);
        // Non-Mac presets never dock the notch.
        let plain = Settings::default();
        let mut s2 = Settings {
            preset: "iPhone".into(),
            ..plain
        };
        let orig2 = DockLayout::from(&s2);
        move_drag(&mut s2, &orig2, &st, (960.0, 24.0), &monitors());
        assert_eq!(
            (s2.dock_edge.as_str(), s2.preset.as_str()),
            ("Floating", "iPhone")
        );
        // Clamps to the work area.
        move_drag(&mut s2, &orig2, &st, (960.0, 1500.0), &monitors());
        assert!(s2.top <= 1040.0 - 44.0 + 1e-9);
        move_drag(&mut s2, &orig2, &st, (-9000.0, 100.0), &monitors());
        assert_eq!(s2.horizontal, 0.0);
        assert!(move_drag(&mut s2, &orig2, &st, (0.0, 0.0), &[]).is_none());
    }

    #[test]
    fn resize_drag_clamps_per_surface() {
        let mut s = Settings::default();
        let o = DockLayout::from(&s);
        resize_drag(&mut s, &o, ResizeTarget::Closed, (10.0, 5.0), false, 1.0);
        assert_eq!((s.width, s.height), (o.width + 20.0, o.height + 5.0));
        resize_drag(&mut s, &o, ResizeTarget::Closed, (10.0, 5.0), true, 1.0);
        assert_eq!(s.width, o.width - 20.0, "left handle mirrors");
        resize_drag(
            &mut s,
            &o,
            ResizeTarget::Closed,
            (9999.0, 9999.0),
            false,
            1.0,
        );
        assert_eq!((s.width, s.height), (400.0, 72.0));
        resize_drag(
            &mut s,
            &o,
            ResizeTarget::Closed,
            (-9999.0, -9999.0),
            false,
            1.0,
        );
        assert_eq!((s.width, s.height), (180.0, 28.0));
        resize_drag(
            &mut s,
            &o,
            ResizeTarget::Hover,
            (9999.0, 9999.0),
            false,
            1.0,
        );
        assert_eq!((s.hover_width, s.hover_height), (760.0, 440.0));
        resize_drag(
            &mut s,
            &o,
            ResizeTarget::Hover,
            (-9999.0, -9999.0),
            false,
            1.0,
        );
        assert_eq!((s.hover_width, s.hover_height), (300.0, 184.0));
        // Panel: small vertical wobble keeps auto height; a real change turns it off.
        let mut s = Settings {
            panel_auto_height: true,
            ..Settings::default()
        };
        resize_drag(
            &mut s,
            &o,
            ResizeTarget::Panel {
                painted_height: 400.0,
            },
            (0.0, 2.0),
            false,
            1.0,
        );
        assert!(s.panel_auto_height);
        resize_drag(
            &mut s,
            &o,
            ResizeTarget::Panel {
                painted_height: 400.0,
            },
            (20.0, 50.0),
            false,
            1.0,
        );
        assert!(!s.panel_auto_height);
        assert_eq!(s.panel_max_height, 450.0);
        assert_eq!(s.panel_width, o.panel_width + 40.0);
        resize_drag(
            &mut s,
            &o,
            ResizeTarget::Panel {
                painted_height: 400.0,
            },
            (9999.0, 9999.0),
            false,
            1.0,
        );
        assert_eq!((s.panel_width, s.panel_max_height), (960.0, 1100.0));
        // DPI scales the pointer delta.
        let mut s = Settings::default();
        resize_drag(&mut s, &o, ResizeTarget::Closed, (30.0, 0.0), false, 1.5);
        assert_eq!(s.width, o.width + 40.0);
    }

    #[test]
    fn arrow_keys_nudge_by_edge() {
        let mut s = Settings {
            dock_edge: "Left".into(),
            dock_vertical: 50.0,
            dock_inset: 8.0,
            ..Settings::default()
        };
        nudge_move(&mut s, 2.0, 2.0);
        assert_eq!((s.dock_vertical, s.dock_inset), (51.0, 10.0));
        s.dock_edge = "Right".into();
        nudge_move(&mut s, 2.0, 0.0);
        assert_eq!(s.dock_inset, 8.0);
        s.dock_inset = 1.0;
        nudge_move(&mut s, 2.0, 0.0);
        assert_eq!(s.dock_inset, 0.0, "clamped");
        let mut s = Settings {
            dock_edge: "Bottom".into(),
            horizontal: 50.0,
            dock_inset: 8.0,
            ..Settings::default()
        };
        nudge_move(&mut s, -2.0, 2.0);
        assert_eq!((s.horizontal, s.dock_inset), (49.0, 6.0));
        let mut s = Settings {
            dock_edge: "Floating".into(),
            preset: "Mac".into(),
            top: 10.0,
            ..Settings::default()
        };
        nudge_move(&mut s, 0.0, 2.0);
        assert_eq!(s.top, 12.0);
        s.dock_edge = "Top".into();
        nudge_move(&mut s, 0.0, 2.0);
        assert_eq!(s.top, 12.0, "the Mac notch ignores vertical nudges");
        s.preset = "iPhone".into();
        nudge_move(&mut s, 0.0, -50.0);
        assert_eq!(s.top, 0.0, "never negative");
        let mut s = Settings::default();
        let (w, h) = (s.width, s.height);
        nudge_resize(&mut s, ResizeTarget::Closed, 2.0, -2.0);
        assert_eq!((s.width, s.height), (w + 2.0, h - 2.0));
        nudge_resize(&mut s, ResizeTarget::Hover, 2.0, 2.0);
        let mut p = Settings {
            panel_auto_height: true,
            ..Settings::default()
        };
        nudge_resize(
            &mut p,
            ResizeTarget::Panel {
                painted_height: 300.0,
            },
            0.0,
            0.0,
        );
        assert!(p.panel_auto_height);
        nudge_resize(
            &mut p,
            ResizeTarget::Panel {
                painted_height: 300.0,
            },
            2.0,
            2.0,
        );
        assert!(!p.panel_auto_height);
        assert_eq!(p.panel_max_height, 302.0);
    }

    #[test]
    fn position_model_top_floating() {
        let s = Settings {
            dock_edge: "Floating".into(),
            horizontal: 25.0,
            top: 100.0,
            width: 300.0,
            height: 44.0,
            ..Settings::default()
        };
        let m = DockPositionModel::sync(&s, (1920.0, 1040.0), 1.0);
        assert_eq!(m.vertical_label, "Top offset \u{b7} px");
        assert_eq!(
            (m.vertical_min, m.vertical_max, m.vertical_step),
            (0.0, 996.0, 1.0)
        );
        assert_eq!(m.vertical_value, 100.0);
        assert_eq!(m.left, (1920.0 - 300.0) * 0.25);
        assert_eq!(m.top, 100.0);
        assert_eq!(m.alignment, "25% left of center");
        assert!(!m.centered && m.vertical_enabled);
        assert_eq!(
            m.coordinates,
            "Left 405 px \u{b7} Top 100 px \u{b7} 100% display scaling"
        );
        assert_eq!(m.horizontal_text(), "25.0");
        // Mac notch disables the vertical control.
        let mac = Settings {
            preset: "Mac".into(),
            dock_edge: "Top".into(),
            ..s.clone()
        };
        assert!(!DockPositionModel::sync(&mac, (1920.0, 1040.0), 1.0).vertical_enabled);
        // Large work areas cap the offset range at 3000.
        let tall = DockPositionModel::sync(&s, (1920.0, 9000.0), 1.0);
        assert_eq!(tall.vertical_max, 3000.0);
    }

    #[test]
    fn position_model_sides_bottom_and_centered() {
        let s = Settings {
            dock_edge: "Left".into(),
            dock_vertical: 20.0,
            dock_inset: 12.0,
            height: 44.0,
            ..Settings::default()
        };
        let m = DockPositionModel::sync(&s, (1920.0, 1040.0), 2.0);
        assert_eq!(
            (m.vertical_max, m.vertical_step, m.vertical_value),
            (100.0, 0.1, 20.0)
        );
        assert_eq!(m.vertical_label, "Vertical \u{b7} %");
        assert_eq!(m.left, 12.0);
        assert_eq!(m.alignment, "30% above center");
        assert!(m.coordinates.ends_with("200% display scaling"));
        let right = Settings {
            dock_edge: "Right".into(),
            dock_vertical: 80.0,
            ..s.clone()
        };
        let m = DockPositionModel::sync(&right, (1920.0, 1040.0), 1.0);
        assert_eq!(m.left, 1920.0 - right.width - 12.0);
        assert_eq!(m.alignment, "30% below center");
        let bottom = Settings {
            dock_edge: "Bottom".into(),
            horizontal: 50.0,
            dock_inset: 20.0,
            ..Settings::default()
        };
        let m = DockPositionModel::sync(&bottom, (1920.0, 1040.0), 1.0);
        assert_eq!((m.vertical_max, m.vertical_value), (200.0, 20.0));
        assert_eq!(m.top, 1040.0 - bottom.height - 20.0);
        assert_eq!(m.alignment, "Centered on this screen");
        assert!(m.centered);
        assert_eq!(m.vertical_label, "Bottom inset \u{b7} px");
        let fine = Settings {
            horizontal: 50.04,
            dock_edge: "Floating".into(),
            ..Settings::default()
        };
        assert!(DockPositionModel::sync(&fine, (1920.0, 1040.0), 1.0).centered);
        let near = Settings {
            horizontal: 50.5,
            dock_edge: "Floating".into(),
            ..Settings::default()
        };
        assert_eq!(
            DockPositionModel::sync(&near, (1920.0, 1040.0), 1.0).alignment,
            "0.5% right of center"
        );
    }

    #[test]
    fn position_inputs_write_the_right_axis() {
        let mut s = Settings {
            dock_edge: "Left".into(),
            ..Settings::default()
        };
        DockPositionModel::set_vertical(&mut s, 33.0);
        assert_eq!(s.dock_vertical, 33.0);
        s.dock_edge = "Bottom".into();
        DockPositionModel::set_vertical(&mut s, 44.0);
        assert_eq!(s.dock_inset, 44.0);
        s.dock_edge = "Top".into();
        DockPositionModel::set_vertical(&mut s, 55.0);
        assert_eq!(s.top, 55.0);
        DockPositionModel::center(&mut s);
        assert_eq!((s.horizontal, s.dock_vertical), (50.0, 50.0));
        assert_eq!(
            DockPositionModel::commit_text(" 12.5 ", 0.0, 100.0),
            Some(12.5)
        );
        assert_eq!(
            DockPositionModel::commit_text("500", 0.0, 100.0),
            Some(100.0)
        );
        assert_eq!(DockPositionModel::commit_text("abc", 0.0, 100.0), None);
        assert_eq!(DockPositionModel::commit_text("NaN", 0.0, 100.0), None);
        let m = DockPositionModel {
            vertical_value: 12.0,
            ..DockPositionModel::sync(&s, (1920.0, 1040.0), 1.0)
        };
        assert_eq!(m.vertical_text(), "12");
        let m = DockPositionModel {
            vertical_value: 12.34,
            ..m
        };
        assert_eq!(m.vertical_text(), "12.3");
    }

    #[test]
    fn map_drag_snaps_and_maps_axes() {
        let mut s = Settings {
            dock_edge: "Floating".into(),
            ..Settings::default()
        };
        // Map 400 wide: usable w = 368; px = (x - 45) / (368 - 58) * 100.
        DockPositionModel::map_drag(
            &mut s,
            (45.0 + 155.0, 19.5 + 0.5 * 93.0),
            400.0,
            MAP_HEIGHT,
            1000.0,
        );
        assert_eq!(s.horizontal, 50.0);
        assert_eq!(
            s.top, 500.0,
            "top follows the vertical fraction of the range"
        );
        DockPositionModel::map_drag(&mut s, (-100.0, -100.0), 400.0, MAP_HEIGHT, 1000.0);
        assert_eq!((s.horizontal, s.top), (0.0, 0.0));
        DockPositionModel::map_drag(&mut s, (9999.0, 9999.0), 400.0, MAP_HEIGHT, 1000.0);
        assert_eq!((s.horizontal, s.top), (100.0, 1000.0));
        let mut side = Settings {
            dock_edge: "Left".into(),
            horizontal: 7.0,
            ..Settings::default()
        };
        DockPositionModel::map_drag(
            &mut side,
            (200.0, 19.5 + 0.2 * 93.0),
            400.0,
            MAP_HEIGHT,
            100.0,
        );
        assert!((side.dock_vertical - 20.0).abs() < 1e-9);
        assert_eq!(side.horizontal, 7.0, "side docks only move vertically");
        let mut mac = Settings {
            dock_edge: "Top".into(),
            preset: "Mac".into(),
            top: 0.0,
            ..Settings::default()
        };
        DockPositionModel::map_drag(&mut mac, (200.0, 100.0), 400.0, MAP_HEIGHT, 1000.0);
        assert_eq!(mac.top, 0.0, "Mac notch keeps top 0");
    }

    #[test]
    fn marker_position() {
        let s = Settings {
            dock_edge: "Floating".into(),
            horizontal: 100.0,
            top: 500.0,
            ..Settings::default()
        };
        let (x, y) = DockPositionModel::marker(&s, 1000.0, 400.0).unwrap();
        assert_eq!(x, 16.0 + (368.0 - 58.0));
        assert_eq!(y, 12.0 + 0.5 * (108.0 - 15.0));
        let left = Settings {
            dock_edge: "Left".into(),
            horizontal: 100.0,
            dock_vertical: 0.0,
            ..Settings::default()
        };
        assert_eq!(
            DockPositionModel::marker(&left, 100.0, 400.0),
            Some((16.0, 12.0))
        );
        let right = Settings {
            dock_edge: "Right".into(),
            ..left.clone()
        };
        assert_eq!(
            DockPositionModel::marker(&right, 100.0, 400.0).unwrap().0,
            16.0 + 310.0
        );
        let bottom = Settings {
            dock_edge: "Bottom".into(),
            horizontal: 0.0,
            ..Settings::default()
        };
        assert_eq!(
            DockPositionModel::marker(&bottom, 200.0, 400.0).unwrap().1,
            12.0 + 93.0
        );
        let mac = Settings {
            dock_edge: "Top".into(),
            preset: "Mac".into(),
            ..Settings::default()
        };
        assert_eq!(
            DockPositionModel::marker(&mac, 1000.0, 400.0).unwrap().1,
            12.0
        );
        assert_eq!(DockPositionModel::marker(&s, 1000.0, 32.0), None);
    }

    #[test]
    fn settings_state_separates_content_reflow_geometry_hotkeys() {
        let base = Settings::default();
        let a = OverlaySettingsState::capture(&base);
        assert_eq!(a, OverlaySettingsState::capture(&base.clone()));
        let check = |edit: &dyn Fn(&mut Settings), content: bool, geometry: bool, hotkeys: bool| {
            let mut s = base.clone();
            edit(&mut s);
            let b = OverlaySettingsState::capture(&s);
            assert_eq!(b.needs_rebuild(&a), content, "content");
            assert_eq!(b.geometry_changed(&a), geometry, "geometry");
            assert_eq!(b.hotkeys_changed(&a), hotkeys, "hotkeys");
        };
        check(&|s| s.width += 1.0, false, true, false);
        check(&|s| s.dock_edge = "Left".into(), false, true, false);
        check(&|s| s.panel_scale += 0.1, false, true, false);
        check(
            &|s| s.compact_layout = !s.compact_layout,
            true,
            false,
            false,
        );
        check(
            &|s| s.widgets.push(WidgetSetting::new("extra", true)),
            true,
            false,
            false,
        );
        check(
            &|s| s.shortcuts.push(Shortcut::default()),
            true,
            false,
            false,
        );
        check(
            &|s| s.hotkey_enabled = !s.hotkey_enabled,
            false,
            false,
            true,
        );
        check(&|s| s.tuck_hotkey_key = "Q".into(), false, false, true);
        // Unrelated preferences change nothing.
        check(&|s| s.opacity = 0.5, false, false, false);
        check(&|s| s.animation_ms = 123.0, false, false, false);
    }

    #[test]
    fn work_dip_divides_by_scale() {
        assert_eq!(
            work_dip(RectI::new(0, 0, 3000, 1500), 1.5),
            (2000.0, 1000.0)
        );
    }
}
