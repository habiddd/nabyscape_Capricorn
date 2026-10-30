//! Visibility shortcuts and live feedback switches (C# `SettingsControls.cs`; the shared
//! `StyleChoice` drop-down lives with the pages that use it in `pages.rs`).

use super::SettingsView;
use naby_render::{Ui, Weight};

const KEYS: [&str; 26] = [
    "A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M", "N", "O", "P", "Q", "R", "S",
    "T", "U", "V", "W", "X", "Y", "Z",
];
const MODIFIERS: [&str; 3] = ["Ctrl+Alt", "Ctrl+Shift", "Alt+Shift"];

impl SettingsView {
    /// `VisibilityControls`: the hide/show and manual tuck shortcuts.
    pub(super) fn visibility_controls<'a>(&mut self, ui: &mut Ui<'a>) {
        let mods: Vec<(&str, &str)> = MODIFIERS.iter().map(|m| (*m, *m)).collect();
        let keys: Vec<(&str, &str)> = KEYS.iter().map(|k| (*k, *k)).collect();
        self.section(ui, "Hide or show the island", |this, ui| {
            let on = this.s.visibility_hotkey_enabled;
            this.check(ui, "Enable visibility shortcut", on, |s, v| s.visibility_hotkey_enabled = v);
            let (m, k) = (
                this.s.visibility_hotkey_modifiers.clone(),
                this.s.visibility_hotkey_key.clone(),
            );
            this.combo_plain(ui, "Visibility hotkey modifiers", &mods, &m, |s, v| s.visibility_hotkey_modifiers = v.to_owned());
            this.combo_plain(ui, "Visibility hotkey key", &keys, &k, |s, v| s.visibility_hotkey_key = v.to_owned());
            this.muted(
                ui,
                "Default: Ctrl+Alt+H. Shows the closed island even over the current maximized or fullscreen app. Switching apps restores automatic hiding.",
                11.0,
            );
        });
        self.section(ui, "Manual tuck shortcut", |this, ui| {
            let on = this.s.tuck_hotkey_enabled;
            this.check(ui, "Enable tuck shortcut", on, |s, v| s.tuck_hotkey_enabled = v);
            let (m, k) = (
                this.s.tuck_hotkey_modifiers.clone(),
                this.s.tuck_hotkey_key.clone(),
            );
            this.combo_plain(ui, "Tuck hotkey modifiers", &mods, &m, |s, v| s.tuck_hotkey_modifiers = v.to_owned());
            this.combo_plain(ui, "Tuck hotkey key", &keys, &k, |s, v| s.tuck_hotkey_key = v.to_owned());
            this.muted(
                ui,
                "Default: Ctrl+Alt+T. First press tucks to the edge; second returns to automatic rules. Manual hide stays hidden. Choose distinct combinations for panel, visibility and tuck; registration errors appear above.",
                11.0,
            );
        });
    }

    /// `AudioFeedbackSettings`: the "Live feedback" card.
    pub(super) fn audio_feedback_settings<'a>(&mut self, ui: &mut Ui<'a>) {
        self.section(ui, "Live feedback", |this, ui| {
            let live = this.s.live_activity;
            this.check(ui, "Live activity · audio, progress & focus breathing", live, |s, v| s.live_activity = v);
            let reactive = this.s.reactive_feedback;
            this.check(ui, "Feedback for volume & audio device changes", reactive, |s, v| s.reactive_feedback = v);
            let idle = this.s.waveform_idle_motion;
            this.check(ui, "Gentle waveform motion during quiet playback", idle, |s, v| s.waveform_idle_motion = v);
            this.muted(
                ui,
                "Real audio levels drive the bars. Quiet playback can use decorative motion when no level is available.",
                11.0,
            );
            ui.gap(6.0);
            let volume = this.s.volume_feedback;
            this.check(ui, "Closed-island volume indicator", volume, |s, v| s.volume_feedback = v);
            let brightness = this.s.brightness_feedback;
            this.check(ui, "Closed-island brightness indicator", brightness, |s, v| s.brightness_feedback = v);
            let status = this.host.brightness_status.clone();
            if !status.is_empty() {
                this.text_final(ui, &status, 11.0, Weight::Regular);
                ui.gap(6.0);
            }
            this.muted(
                ui,
                "Brightness indication requires Windows display-brightness events. Unsupported displays cannot report changes; no brightness is adjusted.",
                11.0,
            );
        });
    }
}
