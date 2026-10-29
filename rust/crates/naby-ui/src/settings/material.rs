//! The Settings window material (C# `SettingsMaterial.cs`): the choice and its status line.
//! The DWM call that applies it belongs to the Windows host, which reads
//! [`SettingsView::backdrop_request`] and reports back through `HostInfo::backdrop`.

use super::SettingsView;
use naby_core::settings::SETTINGS_BACKDROP_CHOICES;
use naby_render::Ui;

impl SettingsView {
    /// The material the user asked for (`SettingsBackdrop.Requested`).
    pub fn backdrop_request(&self) -> &str {
        self.s.requested_backdrop()
    }

    /// `MaterialControls`: the "Settings material" card.
    pub(super) fn material_controls<'a>(&mut self, ui: &mut Ui<'a>) {
        self.section(ui, "Settings material", |this, ui| {
            let requested = this.s.requested_backdrop().to_owned();
            let options: Vec<(&str, &str)> = SETTINGS_BACKDROP_CHOICES.iter().map(|c| (*c, *c)).collect();
            this.combo_plain(ui, "Settings material", &options, &requested, |s, v| {
                s.settings_material = v.to_owned();
            });
            let status = match &this.host.backdrop {
                Some(result) => result.status(),
                None => format!("Requested: {}.", requested),
            };
            let line = format!(
                "{} {}",
                status,
                this.tr("Native materials apply to Settings only. The island preview keeps its existing material.")
            );
            this.muted_final(ui, &line, 11.0);
            ui.gap(6.0);
            this.muted(
                ui,
                "Mica is an opaque wallpaper/theme-based foundation. Acrylic is a translucent system blur. This illustration is schematic; see the Settings window itself for the active material. Island glass and its shaped corners remain unchanged.",
                11.0,
            );
        });
    }
}
