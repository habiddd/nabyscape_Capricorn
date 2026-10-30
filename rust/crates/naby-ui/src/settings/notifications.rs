//! Notification delivery, per-type switches and download monitoring
//! (C# `SettingsNotifications.cs`).

use super::SettingsView;
use naby_core::command::{Command, FilePurpose, SettingsRequest};
use naby_render::Ui;

impl SettingsView {
    pub(super) fn notification_settings<'a>(&mut self, ui: &mut Ui<'a>) {
        self.completion_sound_settings(ui);
        self.section(ui, "Notifications", |this, ui| {
            let delivery = this.s.notification_delivery.clone();
            this.combo_plain(
                ui,
                "Notification delivery",
                &[("Notch", "Notch"), ("Windows", "Windows"), ("Both", "Both"), ("Off", "Off")],
                &delivery,
                |s, v| s.notification_delivery = v.to_owned(),
            );
            let (timer, focus, downloads, updates, failures) = (
                this.s.notify_timer,
                this.s.notify_focus,
                this.s.notify_downloads,
                this.s.notify_updates,
                this.s.notify_failures,
            );
            this.check(ui, "Timer finished", timer, |s, v| s.notify_timer = v);
            this.check(ui, "Focus finished", focus, |s, v| s.notify_focus = v);
            this.check(ui, "Downloads ready", downloads, |s, v| s.notify_downloads = v);
            this.check(ui, "Update available", updates, |s, v| s.notify_updates = v);
            this.check(ui, "Important app failures", failures, |s, v| s.notify_failures = v);
            this.muted(
                ui,
                "Notch shows a compact indicator. Hover or click for details. Manually hidden islands use Windows instead. Volume and brightness take priority; panels and editors stay undisturbed. Pending notices expire; no history is saved.",
                11.0,
            );
            if this.button(ui, "View pending notifications") {
                this.out.push(Command::NavigatePage("notifications".to_owned()));
            }
            ui.gap(6.0);
            this.muted(
                ui,
                "Keyboard: open the island, press Ctrl+K, choose Notifications, then Enter. Windows delivery follows your Windows notification preferences.",
                11.0,
            );
        });
        self.notification_presentation_settings(ui);
        self.section(ui, "Downloads", |this, ui| {
            let enabled = this.s.downloads_enabled;
            this.check(ui, "Infer completed arrivals in a download folder", enabled, |s, v| s.downloads_enabled = v);
            let folder = if this.s.downloads_folder.is_empty() {
                this.tr("Windows Downloads known folder (including relocated folders)").to_owned()
            } else {
                this.s.downloads_folder.clone()
            };
            this.muted_final(ui, &folder, 11.0);
            if let Some(i) = this.wrap_buttons(ui, &[("Choose folder…", true), ("Use Windows Downloads", true)]) {
                if i == 0 {
                    this.out.push(Command::Settings(SettingsRequest::PickFile(FilePurpose::DownloadsFolder)));
                } else {
                    this.s.downloads_folder.clear();
                    this.save();
                }
            }
            ui.gap(8.0);
            this.muted(
                ui,
                "Off by default. Watches this folder only, never subfolders. Existing files are ignored. A final name, readable file and stable size for two seconds suggest an arrival; this cannot always prove a browser finished downloading. Originals stay in place. Drag a burst of files together, Show in folder, or Add to shelf (40-item limit).",
                11.0,
            );
            ui.gap(6.0);
            this.muted(
                ui,
                "Monitoring stops when disabled. If a folder disconnects or becomes inaccessible, toggle monitoring off and on to restart. Folder paths are excluded from layout exports.",
                11.0,
            );
        });
    }
}
