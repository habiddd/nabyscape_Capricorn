//! "Dock placement": which screen edge the dock sits on (C# `SettingsDockPlacement.cs`).

use super::SettingsView;
use naby_render::Ui;

impl SettingsView {
    pub(super) fn dock_placement_settings<'a>(&mut self, ui: &mut Ui<'a>) {
        self.section(ui, "Dock placement", |this, ui| {
            let edge = this.s.dock_edge.clone();
            this.choice(
                ui,
                "Dock side",
                "Dock position",
                &[
                    ("Top", "Top"),
                    ("Bottom", "Bottom"),
                    ("Left", "Left"),
                    ("Right", "Right"),
                    ("Floating", "Floating"),
                ],
                &edge,
                |s, v| s.dock_edge = v.to_owned(),
            );
            if this.s.dock_edge == "Floating" {
                let top = this.s.top;
                this.slider(ui, "Floating top offset", top, 0.0, 3000.0, "px", 1.0, |s, v| s.top = v);
            }
            let vertical = this.s.dock_vertical;
            this.slider(ui, "Vertical alignment", vertical, 0.0, 100.0, "%", 1.0, |s, v| s.dock_vertical = v);
            let inset = this.s.dock_inset;
            this.slider(ui, "Side / bottom inset", inset, 0.0, 200.0, "px", 1.0, |s, v| s.dock_inset = v);
            this.muted(
                ui,
                "Left and Right use vertical alignment. Top and Bottom use horizontal position below. Floating uses horizontal position and its top offset. The top notch stays attached to the screen edge. Panels stay upright; side and bottom docks use the available work area.",
                11.0,
            );
        });
    }
}
