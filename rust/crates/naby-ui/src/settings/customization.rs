//! Layout, hover, clock and system pages plus the layout profile import/export
//! (C# `SettingsCustomization.cs`).

use super::SettingsView;
use naby_core::command::{Command, FilePurpose, SettingsRequest};
use naby_core::settings::profile;
use naby_render::Ui;

impl SettingsView {
    // ---- Position & size --------------------------------------------------------------------

    pub(super) fn layout_settings<'a>(&mut self, ui: &mut Ui<'a>) {
        self.display_settings(ui);
        self.dock_placement_settings(ui);
        self.dock_editing_settings(ui);
        self.section(ui, "Closed island", |this, ui| {
            let (w, h, r, x) = (
                this.s.width,
                this.s.height,
                this.s.radius,
                this.s.horizontal,
            );
            this.slider(ui, "Closed width", w, 180.0, 400.0, "px", 1.0, |s, v| {
                s.width = v
            });
            this.slider(ui, "Closed height", h, 28.0, 72.0, "px", 1.0, |s, v| {
                s.height = v
            });
            this.slider(ui, "Corner radius", r, 0.0, 40.0, "px", 1.0, |s, v| {
                s.radius = v
            });
            this.slider(
                ui,
                "Horizontal position",
                x,
                0.0,
                100.0,
                "%",
                1.0,
                |s, v| s.horizontal = v,
            );
            if this.s.preset != "Mac" || this.s.dock_edge == "Floating" {
                let top = this.s.top;
                let max = (this.host.work_area_height - 72.0).max(150.0);
                this.slider(ui, "Top offset", top, 0.0, max, "px", 1.0, |s, v| s.top = v);
            }
        });
        self.section(ui, "Open panel", |this, ui| {
            let compact = this.s.compact_layout;
            this.check(ui, "Compact panel with tabs", compact, |s, v| s.compact_layout = v);
            let auto = this.s.panel_auto_height;
            this.check(ui, "Fit panel height to content", auto, |s, v| s.panel_auto_height = v);
            this.muted(
                ui,
                "Dragging a lower panel corner sets a manual height. With Fit height off, the height limit becomes the fixed panel height; content scrolls inside it.",
                11.0,
            );
            this.advanced(ui, "Panel dimensions and spacing", |this, ui| {
                let (w, h, pad, scale, gap) = (
                    this.s.panel_width,
                    this.s.panel_max_height,
                    this.s.panel_padding,
                    this.s.panel_scale * 100.0,
                    this.s.section_gap,
                );
                this.slider(ui, "Panel width", w, 300.0, 960.0, "px", 1.0, |s, v| s.panel_width = v);
                this.slider(ui, "Maximum panel height", h, 240.0, 1100.0, "px", 1.0, |s, v| s.panel_max_height = v);
                this.slider(ui, "Content inset", pad, 16.0, 40.0, "px", 1.0, |s, v| s.panel_padding = v);
                this.slider(ui, "Content size", scale, 85.0, 125.0, "%", 1.0, |s, v| s.panel_scale = v / 100.0);
                this.slider(ui, "Section spacing", gap, 8.0, 28.0, "px", 1.0, |s, v| s.section_gap = v);
            });
        });
        self.section(ui, "Hover preview", |this, ui| {
            let (w, h) = (this.s.hover_width, this.s.hover_height);
            this.slider(ui, "Preview width", w, 300.0, 760.0, "px", 1.0, |s, v| {
                s.hover_width = v
            });
            this.slider(ui, "Preview height", h, 184.0, 440.0, "px", 1.0, |s, v| {
                s.hover_height = v
            });
        });
        self.section(ui, "Layout profile", |this, ui| {
            if let Some(i) = this.wrap_buttons(ui, &[("Export layout", true), ("Import layout", true)]) {
                this.out.push(Command::Settings(SettingsRequest::PickFile(if i == 0 {
                    FilePurpose::ExportLayout
                } else {
                    FilePurpose::ImportLayout
                })));
            }
            ui.gap(8.0);
            this.muted(
                ui,
                "Includes appearance, layout, widget order and gestures. Personal notes, shortcuts and startup settings are excluded.",
                11.0,
            );
        });
    }

    // ---- Clock & system ---------------------------------------------------------------------

    pub(super) fn system_settings<'a>(&mut self, ui: &mut Ui<'a>) {
        self.section(ui, "Monitor details", |this, ui| {
            let (n, st, pr) = (this.s.monitor_network, this.s.monitor_storage, this.s.monitor_processes);
            this.check(ui, "Network transfer rates", n, |s, v| s.monitor_network = v);
            this.check(ui, "Storage capacity", st, |s, v| s.monitor_storage = v);
            this.check(ui, "Processes by memory", pr, |s, v| s.monitor_processes = v);
            this.muted(
                ui,
                "Overview shows CPU, memory and uptime. Switch tabs for network, storage or processes. Sampling stops when Status is hidden.",
                11.0,
            );
        });
        self.section(ui, "Clock", |this, ui| {
            let (h24, sec, date) = (this.s.clock24_hour, this.s.clock_seconds, this.s.show_date);
            this.check(ui, "24-hour clock", h24, |s, v| s.clock24_hour = v);
            this.check(ui, "Show seconds in Status", sec, |s, v| {
                s.clock_seconds = v
            });
            this.check(ui, "Show date", date, |s, v| s.show_date = v);
        });
    }

    // ---- Hover & swipe ----------------------------------------------------------------------

    pub(super) fn hover_controls<'a>(&mut self, ui: &mut Ui<'a>) {
        self.section(ui, "Hover & swipe", |this, ui| {
            this.muted(
                ui,
                "Scroll anywhere on a preview to browse. Click to open its panel.",
                11.0,
            );
            ui.gap(6.0);
            let (peek, two) = (this.s.hover_peek, this.s.two_stage_hover);
            this.check(ui, "Preview on hover", peek, |s, v| s.hover_peek = v);
            this.check(ui, "Two-stage reveal", two, |s, v| s.two_stage_hover = v);
            this.advanced(ui, "Hover timing", |this, ui| {
                let (d, dd, sw) = (
                    f64::from(this.s.hover_delay_ms),
                    f64::from(this.s.hover_detail_delay_ms),
                    this.s.swipe_distance,
                );
                this.slider(ui, "Hover delay", d, 80.0, 900.0, "ms", 10.0, |s, v| {
                    s.hover_delay_ms = v as i32
                });
                this.slider(ui, "Detail delay", dd, 200.0, 1200.0, "ms", 10.0, |s, v| {
                    s.hover_detail_delay_ms = v as i32
                });
                this.slider(ui, "Swipe distance", sw, 24.0, 100.0, "px", 1.0, |s, v| {
                    s.swipe_distance = v
                });
            });
        });
    }

    // ---- file dialog results ------------------------------------------------------------------

    /// Applies the answer of a file dialog (`file_picked`).
    pub(crate) fn handle_file(&mut self, purpose: FilePurpose, path: &str) {
        match purpose {
            FilePurpose::ExportLayout => match std::fs::write(path, profile::export(&self.s)) {
                Ok(()) => self.set_feedback("Layout exported", false),
                Err(e) => {
                    self.log_error(&e);
                    self.set_feedback("Could not export layout", true);
                }
            },
            FilePurpose::ImportLayout => self.import_layout(path),
            FilePurpose::CompletionSound => {
                self.s.countdown_sound_file = path.to_owned();
                self.s.countdown_sound = "Custom".to_owned();
                self.save();
            }
            FilePurpose::LyricsFile => {
                let name = std::path::Path::new(path)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.out.push(Command::LyricsImport {
                    path: path.to_owned(),
                });
                self.lyrics_note = Some(format!(
                    "{} {}",
                    self.tr("Loaded timed lyrics for this session:"),
                    name
                ));
                self.repaint = true;
            }
            FilePurpose::DownloadsFolder => {
                self.s.downloads_folder = path.to_owned();
                self.save();
            }
            FilePurpose::ShortcutFile | FilePurpose::ShortcutFolder => {
                self.shortcut_target = path.to_owned();
                self.fields
                    .insert("shortcut-target".to_owned(), (path.to_owned(), false));
                if self.shortcut_name.trim().is_empty() {
                    let p = std::path::Path::new(path);
                    let name = if purpose == FilePurpose::ShortcutFile {
                        p.file_stem()
                    } else {
                        p.file_name()
                    }
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                    self.shortcut_name = name.clone();
                    self.fields
                        .insert("shortcut-name".to_owned(), (name, false));
                }
                self.repaint = true;
            }
            FilePurpose::DiagnosticsExport => {
                let text = self.host.diagnostics_report.clone();
                match std::fs::write(path, text.as_bytes()) {
                    Ok(()) => self.diag_note = Some("Diagnostics report exported.".to_owned()),
                    Err(e) => {
                        self.log_error(&e);
                        self.diag_note =
                            Some("Could not export the report. Try another location.".to_owned());
                    }
                }
                self.repaint = true;
            }
        }
    }

    fn import_layout(&mut self, path: &str) {
        let result = (|| -> Result<(), String> {
            let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
            if meta.len() > 262_144 {
                return Err("Layout file is too large.".to_owned());
            }
            let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
            profile::import_into(&text, &mut self.s).map_err(|e| e.to_string())
        })();
        match result {
            Ok(()) => {
                self.save();
                self.set_feedback("Layout imported", false);
            }
            Err(message) => {
                self.log_error(&message);
                let text = format!("{} {}", self.tr("Could not import layout."), message);
                self.set_feedback(&text, true);
            }
        }
    }

    /// Records a failure in the app log through the store.
    pub(crate) fn log_error(&self, error: &dyn std::fmt::Display) {
        if let Some(store) = &self.store {
            store.log(error);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use naby_core::settings::Settings;

    #[test]
    fn exported_layout_imports_back_and_oversized_files_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("layout.json");
        let mut view = SettingsView::new(Settings::default(), None);
        view.s.width = 333.0;
        view.handle_file(FilePurpose::ExportLayout, path.to_str().unwrap());
        assert_eq!(view.feedback_text(), "Layout exported");

        let mut other = SettingsView::new(Settings::default(), None);
        other.handle_file(FilePurpose::ImportLayout, path.to_str().unwrap());
        assert_eq!(other.s.width, 333.0);
        assert_eq!(other.feedback_text(), "Layout imported");
        assert!(
            other.save_at.is_some(),
            "an import is saved through the debounce"
        );

        let big = dir.path().join("big.json");
        std::fs::write(&big, vec![b' '; 300_000]).unwrap();
        other.handle_file(FilePurpose::ImportLayout, big.to_str().unwrap());
        assert!(other.feedback_text().contains("too large"));
        assert!(other.feedback_is_error());
    }

    #[test]
    fn picked_shortcut_files_fill_the_form() {
        let mut view = SettingsView::new(Settings::default(), None);
        view.handle_file(FilePurpose::ShortcutFile, "C:/Apps/Editor.exe");
        assert_eq!(view.shortcut_target, "C:/Apps/Editor.exe");
        assert_eq!(view.shortcut_name, "Editor");
        view.shortcut_name = "Mine".into();
        view.handle_file(FilePurpose::ShortcutFolder, "C:/Work/Docs");
        assert_eq!(view.shortcut_name, "Mine", "an existing name is kept");
        view.add_shortcut();
        assert_eq!(view.s.shortcuts.last().unwrap().name, "Mine");
        assert_eq!(view.s.shortcuts.last().unwrap().target, "C:/Work/Docs");
    }
}
