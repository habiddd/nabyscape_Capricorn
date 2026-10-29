//! The Updates section (C# `SettingsUpdates.cs`). The update service lives in the host; this
//! file shows its status and forwards the buttons as `SettingsRequest::Update`.

use super::{InstallKind, Modal, ModalAction, SettingsView};
use naby_core::command::{Command, SettingsRequest, UpdateAction};
use naby_render::{Ui, Weight};

impl SettingsView {
    pub(super) fn updates_section<'a>(&mut self, ui: &mut Ui<'a>) {
        self.section(ui, "Updates", |this, ui| {
            let info = this.host.update.clone();
            let installed = format!("{} {} · {}", this.tr("Installed:"), info.installed_version, info.platform);
            this.text_final(ui, &installed, 13.0, Weight::Regular);
            ui.gap(6.0);
            this.muted(ui, "Check for a release, download it, then choose when to install and restart.", 12.0);
            let auto = this.s.auto_check_updates;
            this.check_plain(ui, "Check automatically once per day", auto, |s, v| s.auto_check_updates = v);
            let download = this.s.auto_download_updates;
            this.check_plain(ui, "Download updates automatically", download, |s, v| s.auto_download_updates = v);
            this.muted(
                ui,
                "Automatic downloads are optional and may use a metered connection. They follow automatic startup checks; installation always waits for you.",
                11.0,
            );
            ui.gap(6.0);
            let available = info
                .available
                .clone()
                .unwrap_or_else(|| this.tr("None checked").to_owned());
            let status = format!("{}\n{} {}", this.tr(&info.status), this.tr("Available:"), available);
            this.muted_final(ui, &status, 12.0);
            let automatic = info.install_kind == InstallKind::Automatic;
            let mut request: Option<UpdateAction> = None;
            if this.button(ui, "Check for updates") {
                request = Some(UpdateAction::Check);
            }
            if this.button(ui, "Release notes") {
                request = Some(UpdateAction::ReleaseNotes);
            }
            if this.button_enabled(ui, "Download update", !info.busy && info.can_download) {
                request = Some(UpdateAction::Download);
            }
            if this.button(ui, "Cancel update") {
                request = Some(UpdateAction::Cancel);
            }
            if this.button_enabled(ui, "Install and restart", !info.busy && info.can_install && automatic) {
                request = Some(UpdateAction::Install);
            }
            ui.gap(8.0);
            let footnote = match info.install_kind {
                InstallKind::Automatic => "Before installation, the updater saves a local recovery copy of the app and JSON profiles. If installation or initial startup fails, it restores that copy.",
                InstallKind::Legacy => "Legacy installation: quit NabyScape and run the latest official setup, then open NabyScape.exe. Automatic installation and recovery require the current executable name.",
                InstallKind::Portable => "Portable copy: use Release notes to download the portable ZIP, quit, then replace your portable files manually. Automatic installation is unavailable here.",
            };
            this.muted(ui, footnote, 11.0);
            if !info.recovery.is_empty() {
                ui.gap(6.0);
                let text = this.tr(&info.recovery).to_owned();
                this.muted_final(ui, &text, 11.0);
            }
            if this.button_enabled(ui, "Restore previous version", !info.busy && info.can_restore && automatic) {
                this.modal = Some(Modal {
                    title: "Restore previous version".to_owned(),
                    body: "Restore the previous app and its saved settings? Changes to settings and JSON profile data since that backup will be replaced. Local media files are kept.".to_owned(),
                    confirm: "Yes".to_owned(),
                    action: ModalAction::RestorePreviousVersion,
                });
            }
            ui.gap(8.0);
            this.muted(
                ui,
                "Downloads use official GitHub releases. SHA-256 checks integrity; unsigned checksums do not establish publisher identity. Windows security checks remain enabled.",
                11.0,
            );
            if let Some(action) = request {
                this.out.push(Command::Settings(SettingsRequest::Update(action)));
            }
        });
    }
}
