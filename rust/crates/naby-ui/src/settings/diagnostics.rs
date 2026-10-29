//! The private diagnostics section (C# `SettingsDiagnostics.cs`): a read-only preview of the
//! report with copy and export. Nothing is uploaded and no file is written until Export.

use super::SettingsView;
use naby_core::command::{Command, FilePurpose, SettingsRequest};
use naby_render::{Ui, Weight};

impl SettingsView {
    pub(super) fn diagnostics_section<'a>(&mut self, ui: &mut Ui<'a>) {
        self.section(ui, "Private diagnostics", |this, ui| {
            this.muted(
                ui,
                "Includes app and runtime versions, architecture, coarse storage and service status, and up to 64 controlled diagnostic events. No usernames, paths, endpoints, models, device names, keys, conversations, screenshots, audio or saved profile content are included.",
                11.0,
            );
            ui.gap(6.0);
            this.muted(
                ui,
                "The existing local errors.log may contain private details and is never included in this report.",
                11.0,
            );
            ui.gap(8.0);
            // The report preview: a read-only box (180-320 px; longer reports are cut off and
            // can be read in full through Copy or Export).
            let th = ui.theme().clone();
            let report = this.host.diagnostics_report.clone();
            let st = ui.text_style(12.0, Weight::Regular, th.control_text);
            let lh = ui.line_height(&st);
            let width = ui.available_width() - 20.0;
            let mut lines: Vec<String> = Vec::new();
            for raw in report.lines() {
                lines.extend(ui.fonts().wrap(raw, &st, width));
            }
            let want = lines.len() as f32 * lh + 20.0;
            let height = want.clamp(180.0, 320.0);
            let box_rect = ui.allocate(None, height);
            {
                let c = ui.canvas();
                c.fill_rrect(box_rect, th.radius, th.control_fill);
                c.stroke_round_rect(box_rect, th.radius, th.control_border, 1.0);
            }
            let fit = (((height - 20.0) / lh).floor() as usize).max(1);
            for (i, line) in lines.iter().take(fit).enumerate() {
                let text = if i + 1 == fit && lines.len() > fit {
                    format!("{}…", line.trim_end())
                } else {
                    line.clone()
                };
                ui.draw_text(box_rect.x + 10.0, box_rect.y + 10.0 + i as f32 * lh, &text, &st, Some(width));
            }
            this.record("Private diagnostics report preview", box_rect);
            if this.button(ui, "Refresh diagnostics preview") {
                this.out.push(Command::Settings(SettingsRequest::RefreshDiagnostics));
                this.diag_note = Some("Diagnostics preview refreshed.".to_owned());
            }
            if this.button(ui, "Copy diagnostics report") {
                this.out.push(Command::CopyText(report));
                this.diag_note = Some("Diagnostics report copied.".to_owned());
            }
            if this.button(ui, "Export diagnostics report") {
                this.out
                    .push(Command::Settings(SettingsRequest::PickFile(FilePurpose::DiagnosticsExport)));
            }
            ui.gap(6.0);
            let note = this
                .diag_note
                .clone()
                .unwrap_or_else(|| "Review the preview before copying or exporting. Nothing is uploaded automatically.".to_owned());
            let shown = this.tr(&note).to_owned();
            this.muted_final(ui, &shown, 11.0);
            ui.gap(8.0);
            this.text(ui, "Storage recovery", 13.0, Weight::Regular);
            ui.gap(4.0);
            this.muted(
                ui,
                "When a document is unreadable or from a newer version, it stays protected from automatic saves. Temporary defaults may be shown. Quit NabyScape and preserve the original JSON file and its matching .schema.json sidecar before recovery. Use a compatible app version, or restore a known-good matching document and schema backup. For a legacy backup without a sidecar, preserve and move aside the current sidecar too. Keep the original and migration backup until recovery is verified. Reset preferences does not unlock protected documents.",
                11.0,
            );
        });
    }
}
