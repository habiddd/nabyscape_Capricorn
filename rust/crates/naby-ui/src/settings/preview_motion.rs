//! What the live preview shows and how it moves (C# `SettingsPreviewMotion.cs`): which sample
//! (`Closed`, `Hover`, `Panel` and a content kind) follows the control under the pointer, the
//! eased resize between states and "Replay transition".

use super::SettingsView;
use naby_core::settings::Settings;

/// The three states of the island the preview can show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PreviewMode {
    #[default]
    Closed,
    Hover,
    Panel,
}

impl PreviewMode {
    pub const ALL: [PreviewMode; 3] = [PreviewMode::Closed, PreviewMode::Hover, PreviewMode::Panel];

    pub fn name(self) -> &'static str {
        match self {
            PreviewMode::Closed => "Closed",
            PreviewMode::Hover => "Hover",
            PreviewMode::Panel => "Panel",
        }
    }
}

/// The "Preview content" choices, in the order of the combo box.
pub const CONTENTS: [&str; 23] = [
    "Music",
    "Timer",
    "Focus",
    "Stopwatch",
    "Calendar",
    "Note",
    "Shelf",
    "Tools",
    "System",
    "Launch",
    "Navigation",
    "Volume feedback",
    "Drop zone",
    "Detached preview",
    "Cover gestures",
    "Smart clipboard",
    "Control colors",
    "Typography",
    "Panel navigation",
    "Visibility",
    "Tasks",
    "Startup",
    "AI",
];

#[derive(Debug, Clone, Copy)]
struct SizeAnim {
    from: (f32, f32),
    to: (f32, f32),
    start: f64,
    duration: f64,
}

#[derive(Debug, Clone, Copy)]
struct Replay {
    start: f64,
    previous: PreviewMode,
    step: f64,
}

/// Preview selection and animation state.
#[derive(Debug, Default)]
pub(crate) struct PreviewState {
    pub mode: PreviewMode,
    pub content: String,
    /// The line under "Live preview" (`previewFocus`).
    pub focus: String,
    /// Bumped by every saved edit so cached samples refresh.
    pub revision: u64,
    /// The narrow-window preview strip is expanded.
    pub strip_open: bool,
    size: Option<SizeAnim>,
    target: Option<(f32, f32)>,
    replay: Option<Replay>,
}

impl PreviewState {
    /// `Navigate` resets the sample for the new category.
    pub fn on_navigate(&mut self, id: &str, _same: bool) {
        self.focus = format!("{id} overview");
        let (content, mode) = match id {
            "Launch" => ("Launch", PreviewMode::Panel),
            "Content" => ("Navigation", PreviewMode::Panel),
            "Controls" => ("Music", PreviewMode::Hover),
            "System" => ("System", PreviewMode::Panel),
            "Audio" => ("Music", PreviewMode::Panel),
            "AI" => ("AI", PreviewMode::Panel),
            "Application" => ("Startup", PreviewMode::Panel),
            _ => ("Music", PreviewMode::Closed),
        };
        self.content = content.to_owned();
        self.mode = mode;
        self.revision += 1;
    }

    /// The size to draw at `now` for the wanted `target`, easing from the previous one.
    pub fn size_at(
        &mut self,
        now: f64,
        target: (f32, f32),
        duration_ms: f64,
        animate: bool,
    ) -> (f32, f32) {
        let current = self.sample(now);
        match self.target {
            None => {
                self.target = Some(target);
                self.size = None;
                return target;
            }
            Some(t) if t == target => return current,
            Some(_) => {}
        }
        self.target = Some(target);
        let moved = (current.0 - target.0).abs() > 0.5 || (current.1 - target.1).abs() > 0.5;
        if animate && duration_ms > 0.0 && moved {
            self.size = Some(SizeAnim {
                from: current,
                to: target,
                start: now,
                duration: duration_ms.clamp(80.0, 600.0) / 1000.0,
            });
            self.sample(now)
        } else {
            self.size = None;
            target
        }
    }

    fn sample(&self, now: f64) -> (f32, f32) {
        match (self.size, self.target) {
            (Some(a), _) => {
                let t = ((now - a.start) / a.duration).clamp(0.0, 1.0);
                let e = 1.0 - (1.0 - t).powi(3);
                let k = e as f32;
                (
                    a.from.0 + (a.to.0 - a.from.0) * k,
                    a.from.1 + (a.to.1 - a.from.1) * k,
                )
            }
            (None, Some(t)) => t,
            (None, None) => (0.0, 0.0),
        }
    }

    /// Seconds until the next preview frame while something moves.
    pub fn animation_wait(&self, now: f64) -> Option<f64> {
        let mut wait: Option<f64> = None;
        if let Some(a) = self.size {
            if now < a.start + a.duration {
                wait = Some(0.016);
            }
        }
        if let Some(r) = self.replay {
            let next = r.start + (r.step * (((now - r.start) / r.step).floor() + 1.0));
            let w = (next - now).max(0.016);
            wait = Some(wait.map_or(w, |x| x.min(w)));
        }
        wait
    }

    /// Starts "Replay transition": Closed, Hover and Panel for `step` seconds each.
    pub fn start_replay(&mut self, now: f64, step: f64) {
        if self.replay.is_some() {
            return;
        }
        self.replay = Some(Replay {
            start: now,
            previous: self.mode,
            step: step.max(0.5),
        });
        self.mode = PreviewMode::Closed;
    }

    /// Advances a running replay; true while it still runs.
    pub fn advance_replay(&mut self, now: f64) -> bool {
        let Some(r) = self.replay else {
            return false;
        };
        let index = ((now - r.start) / r.step).floor() as usize;
        if index >= 3 {
            self.mode = r.previous;
            self.replay = None;
            return false;
        }
        self.mode = PreviewMode::ALL[index];
        true
    }
}

fn has(name: &str, part: &str) -> bool {
    name.to_lowercase().contains(&part.to_lowercase())
}

impl SettingsView {
    fn set_preview(&mut self, mode: PreviewMode, content: Option<&str>) {
        self.pv.mode = mode;
        if let Some(c) = content {
            self.pv.content = c.to_owned();
        }
    }

    /// `ConfigurePreviewForSetting(name)`: the sample that shows the effect of a control.
    pub(crate) fn configure_preview_for_setting(&mut self, name: &str) {
        use PreviewMode::*;
        self.pv.focus = name.to_owned();
        let current = self.current.clone();
        if name == "Show ambience on closed dock" {
            self.set_preview(Closed, Some("Music"));
        } else if let Some(tool) = name.strip_suffix(" tool") {
            let content = match tool {
                "File shelf" => "Shelf",
                "Notes" => "Note",
                "Clipboard" => "Smart clipboard",
                "Tasks" => "Tasks",
                "Calendar" => "Calendar",
                _ => "Tools",
            };
            self.set_preview(Hover, Some(content));
        } else if current == "Ambience" {
            self.set_preview(Panel, Some("Music"));
        } else if current == "AI" {
            self.set_preview(Panel, Some("AI"));
        } else if current == "Application" {
            self.set_preview(Panel, Some("Startup"));
        } else if has(name, "Tuck") {
            self.set_preview(Panel, Some("Visibility"));
        } else if name == "Swipe between open panels" {
            self.set_preview(Panel, Some("Panel navigation"));
        } else if name == "Reveal shelf when dragging files" {
            self.set_preview(Hover, Some("Drop zone"));
        } else if name == "Allow detachable previews" {
            self.set_preview(Hover, Some("Detached preview"));
        } else if name == "Control playback from album cover" {
            self.set_preview(Hover, Some("Cover gestures"));
        } else if has(name, "media title") || has(name, "heading") {
            self.set_preview(Panel, Some("Typography"));
        } else if has(name, "volume indicator") || name == "Volume step" {
            self.set_preview(Closed, Some("Volume feedback"));
        } else if has(name, "motion") || has(name, "activity") || name == "Animation duration" {
            self.set_preview(Hover, Some("Music"));
        } else if has(name, "hover") || name == "Two-stage reveal" || name == "Swipe distance" {
            self.pv.mode = Hover;
        } else if has(name, "Focus duration") || has(name, "Break duration") {
            self.set_preview(Panel, Some("Focus"));
        } else if has(name, "Closed media") || has(name, "Closed island") {
            self.set_preview(Closed, Some("Music"));
        } else if has(name, "color") || has(name, "outline") || name == "Control surfaces" {
            self.set_preview(Panel, Some("Control colors"));
        } else if has(name, "clipboard") || has(name, "history") {
            self.set_preview(Panel, Some("Smart clipboard"));
        } else if has(name, "shortcut") || name == "Shortcuts" {
            self.set_preview(Panel, Some("Launch"));
        } else if has(name, "Category") || name == "Text and navigation" {
            self.set_preview(Panel, Some("Navigation"));
        } else if has(name, "Launch") {
            self.set_preview(Panel, Some("Launch"));
        } else if has(name, "Panel") || has(name, "Content") {
            self.pv.mode = Panel;
        } else if has(name, "Preview") {
            self.pv.mode = Hover;
        } else if has(name, "width")
            || has(name, "height")
            || has(name, "position")
            || has(name, "offset")
            || matches!(
                name,
                "Shape" | "Corner radius" | "Display" | "Island display"
            )
        {
            self.pv.mode = Closed;
        } else if current == "System" {
            self.set_preview(Panel, Some("System"));
        } else if current == "Audio" {
            self.set_preview(Panel, Some("Music"));
        }
    }

    /// The extra mode switches `AddSlider` applies after a slider changed.
    pub(crate) fn slider_preview_mode(&mut self, label: &str) {
        if matches!(
            label,
            "Panel width"
                | "Maximum panel height"
                | "Content inset"
                | "Content size"
                | "Section spacing"
        ) {
            self.pv.mode = PreviewMode::Panel;
        } else if label.starts_with("Preview") {
            self.pv.mode = PreviewMode::Hover;
        } else if label.starts_with("Closed") || label == "Corner radius" {
            self.pv.mode = PreviewMode::Closed;
        }
    }

    /// `ConfigurePreviewForSection(title)`: what a section's sample shows.
    pub(crate) fn configure_preview_for_section(&mut self, title: &str) {
        use PreviewMode::*;
        self.pv.focus = self.tr(title).to_owned();
        let (mode, content) = match title {
            "Enabled widgets & order" | "Text and navigation" => (Panel, "Navigation"),
            "Hover pages" | "Hover & swipe" | "Hover preview" => (Hover, "Music"),
            "AI companion" => (Panel, "AI"),
            "Individual tools" => (Panel, "Tools"),
            "Clipboard history" => (Panel, "Smart clipboard"),
            "Focus sessions" => (Panel, "Focus"),
            "Stopwatch" => (Panel, "Stopwatch"),
            "Automatic tucking" | "Hide or show the island" => (Panel, "Visibility"),
            "Drag to Island" => (Hover, "Drop zone"),
            "Detachable previews" => (Hover, "Detached preview"),
            "Closed cover gestures" => (Hover, "Cover gestures"),
            "Launch display" | "Shortcuts" => (Panel, "Launch"),
            "Clock" | "Monitor details" => (Panel, "System"),
            "Controls & outlines" => (Panel, "Control colors"),
            "Typography" => (Panel, "Typography"),
            "Panel navigation" => (Panel, "Panel navigation"),
            "Live signal" | "Waveform source" => (Panel, "Music"),
            "Live feedback" => (Closed, "Volume feedback"),
            "Open panel" => (Panel, "Music"),
            _ => (Closed, "Music"),
        };
        self.pv.mode = mode;
        self.pv.content = content.to_owned();
        self.pv.revision += 1;
    }

    /// The size (in layout pixels) the preview island wants for the current sample, and whether
    /// it is one of the fixed-size "detail" examples (`RenderSettingsPreview`).
    pub(crate) fn preview_target(&self) -> ((f32, f32), bool) {
        let s: &Settings = &self.s;
        let mode = self.pv.mode;
        let closed = mode == PreviewMode::Closed;
        let hover = mode == PreviewMode::Hover;
        let (mut w, mut h) = if closed {
            (s.width, s.height)
        } else if hover {
            (s.hover_width.min(s.panel_width), s.hover_height)
        } else {
            (s.panel_width, s.panel_max_height)
        };
        let content = self.pv.content.as_str();
        let detail = !closed
            && ((!hover && self.current != "Layout")
                || matches!(
                    content,
                    "Control colors"
                        | "Typography"
                        | "Panel navigation"
                        | "Drop zone"
                        | "Detached preview"
                        | "Cover gestures"
                        | "Smart clipboard"
                        | "Shelf"
                        | "Visibility"
                ));
        if detail {
            w = 300.0;
            h = match content {
                "Control colors" => 330.0,
                "Smart clipboard" => 320.0,
                "Detached preview" => 260.0,
                "Typography" => 220.0,
                "Shelf" => 260.0,
                "Music" => 250.0,
                "System" => 210.0,
                "Navigation" => 160.0,
                _ => 230.0,
            };
        }
        ((w as f32, h as f32), detail)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_picks_the_default_sample_per_category() {
        let mut pv = PreviewState::default();
        pv.on_navigate("Audio", false);
        assert_eq!(
            (pv.mode, pv.content.as_str()),
            (PreviewMode::Panel, "Music")
        );
        pv.on_navigate("Controls", false);
        assert_eq!(
            (pv.mode, pv.content.as_str()),
            (PreviewMode::Hover, "Music")
        );
        pv.on_navigate("Layout", false);
        assert_eq!(
            (pv.mode, pv.content.as_str()),
            (PreviewMode::Closed, "Music")
        );
        assert_eq!(pv.focus, "Layout overview");
    }

    #[test]
    fn size_eases_out_and_settles() {
        let mut pv = PreviewState::default();
        assert_eq!(pv.size_at(0.0, (228.0, 32.0), 420.0, true), (228.0, 32.0));
        let mid = pv.size_at(1.0, (400.0, 200.0), 420.0, true);
        assert_eq!(mid, (228.0, 32.0), "starts from the previous size");
        let later = pv.size_at(1.2, (400.0, 200.0), 420.0, true);
        assert!(later.0 > 228.0 && later.0 < 400.0);
        assert_eq!(pv.size_at(2.0, (400.0, 200.0), 420.0, true), (400.0, 200.0));
        assert!(pv.animation_wait(2.0).is_none());
        // Reduced motion jumps.
        assert_eq!(
            pv.size_at(3.0, (300.0, 100.0), 420.0, false),
            (300.0, 100.0)
        );
    }

    #[test]
    fn replay_walks_the_three_states_and_restores() {
        let mut pv = PreviewState::default();
        pv.mode = PreviewMode::Panel;
        pv.start_replay(10.0, 0.6);
        assert!(pv.advance_replay(10.1));
        assert_eq!(pv.mode, PreviewMode::Closed);
        assert!(pv.advance_replay(10.7));
        assert_eq!(pv.mode, PreviewMode::Hover);
        assert!(pv.advance_replay(11.3));
        assert_eq!(pv.mode, PreviewMode::Panel);
        assert!(!pv.advance_replay(12.0));
        assert_eq!(pv.mode, PreviewMode::Panel, "restored");
    }
}
