//! "Add installed app": a searchable list drawn over the Settings window. It stands in for the
//! WPF `AppPicker` dialog; the host fills [`HostInfo::installed_apps`] after
//! `SettingsRequest::PickInstalledApp`.

use super::kit::Pal;
use super::SettingsView;
use naby_core::command::{Command, SettingsRequest};
use naby_core::settings::Shortcut;
use naby_render::{Key, Rect, TextEditOpts, Ui, Weight};

/// An installed app the user can turn into a shortcut.
#[derive(Debug, Clone, PartialEq)]
pub struct AppChoice {
    pub name: String,
    pub target: String,
    pub app_id: Option<String>,
}

/// Open picker state.
#[derive(Debug, Default, Clone)]
pub(crate) struct Picker {
    pub query: String,
    pub selected: usize,
}

/// `AppPicker.Search`: case-insensitive name match.
pub fn filter<'a>(apps: &'a [AppChoice], query: &str) -> Vec<&'a AppChoice> {
    let q = query.trim().to_lowercase();
    apps.iter()
        .filter(|a| a.name.to_lowercase().contains(&q))
        .collect()
}

impl SettingsView {
    /// Opens the picker and asks the host for the app list.
    pub(crate) fn open_app_picker(&mut self) {
        self.picker = Some(Picker::default());
        self.out
            .push(Command::Settings(SettingsRequest::PickInstalledApp));
        self.repaint = true;
    }

    /// Adds the chosen app (`Accept`).
    pub(crate) fn add_installed_app(&mut self, app: &AppChoice) {
        if self.s.shortcuts.len() >= 40 {
            self.set_feedback("You can add up to 40 shortcuts.", false);
            return;
        }
        self.s.shortcuts.push(Shortcut {
            name: app.name.clone(),
            target: app.target.clone(),
            app_id: app.app_id.clone(),
        });
        self.save();
    }

    pub(super) fn picker_ui<'a>(&mut self, ui: &mut Ui<'a>, area: Rect, pal: &Pal) {
        let Some(mut picker) = self.picker.take() else {
            return;
        };
        let th = ui.theme().clone();
        ui.canvas()
            .fill_rect(area, naby_core::color::Rgba::BLACK.with_alpha(150));
        let w = (area.w - 40.0).clamp(280.0, 480.0);
        let h = (area.h - 40.0).clamp(300.0, 570.0);
        let card = Rect::new(
            area.x + (area.w - w) / 2.0,
            area.y + (area.h - h) / 2.0,
            w,
            h,
        );
        ui.canvas().fill_rrect(card, 12.0, pal.section_bg);
        ui.canvas()
            .stroke_round_rect(card, 12.0, pal.section_border, 1.0);
        let inner = card.inset(20.0);
        let apps = self.host.installed_apps.clone();
        let mut close = false;
        let mut accept: Option<AppChoice> = None;
        ui.child(inner, |ui| {
            ui.spacing(0.0);
            self.text(ui, "Installed apps", 20.0, Weight::Regular);
            ui.gap(14.0);
            let hint = self.tr("Search by application name").to_owned();
            let resp = ui.text_edit_with(&mut picker.query, &TextEditOpts::single().hint(&hint));
            self.record("Search installed apps", resp.rect);
            if !self.picker_focused {
                ui.focus_id(resp.id);
                self.picker_focused = true;
            }
            ui.gap(12.0);
            let list: Vec<&AppChoice> = match &apps {
                Some(a) => filter(a, &picker.query),
                None => Vec::new(),
            };
            let state = match &apps {
                None => "Loading installed apps…".to_owned(),
                Some(a) if a.is_empty() => {
                    "No installed apps were found. You can add a shortcut manually in Settings."
                        .to_owned()
                }
                Some(_) if list.is_empty() => "No installed apps match your search.".to_owned(),
                Some(_) => format!("{} {}", list.len(), self.tr("applications")),
            };
            let shown = self.tr(&state).to_owned();
            self.muted_final(ui, &shown, 12.0);
            ui.gap(8.0);
            picker.selected = picker.selected.min(list.len().saturating_sub(1));
            let list_h = (inner.h - 20.0 - 14.0 - 34.0 - 12.0 - 20.0 - 8.0 - 34.0 - 12.0).max(80.0);
            let region = ui.allocate(None, list_h);
            ui.child(region, |ui| {
                ui.scroll_area("apps", |ui| {
                    ui.spacing(0.0);
                    for (i, app) in list.iter().enumerate() {
                        let row = ui.allocate(None, 36.0);
                        ui.push_id(i, |ui| {
                            let r = ui.hit_area(row, true);
                            self.hand |= r.hovered;
                            self.record(&format!("app:{}", app.name), row);
                            if i == picker.selected {
                                ui.canvas().fill_rrect(row, 8.0, th.selection_fill);
                            } else if r.hovered {
                                ui.canvas().fill_rrect(row, 8.0, th.hover_fill);
                            }
                            let st = ui.text_style(
                                13.0,
                                Weight::Regular,
                                if i == picker.selected {
                                    th.selection_text
                                } else {
                                    th.text
                                },
                            );
                            let lh = ui.line_height(&st);
                            ui.draw_text(
                                row.x + 12.0,
                                row.y + (row.h - lh) / 2.0,
                                &app.name,
                                &st,
                                Some(row.w - 24.0),
                            );
                            ui.paint_focus_ring(&r, 8.0);
                            if r.pressed {
                                picker.selected = i;
                            }
                            if r.clicked {
                                picker.selected = i;
                            }
                            if r.clicked && ui.input().pressed {
                                accept = Some((*app).clone());
                            }
                        });
                    }
                });
            });
            ui.gap(12.0);
            ui.row(|ui| {
                ui.spacing(8.0);
                let cancel = ui.button(self.tr("Cancel"));
                self.record("Picker Cancel", cancel.rect);
                close |= cancel.clicked;
                let ok = ui.enabled(!list.is_empty(), |ui| {
                    ui.primary_button(self.tr("Add selected app"))
                });
                self.record("Picker Add", ok.rect);
                if ok.clicked && !list.is_empty() {
                    accept = Some(list[picker.selected].clone());
                }
            });
            for k in ui.input().keys.clone() {
                match k.key {
                    Key::Escape => close = true,
                    Key::Down => {
                        picker.selected = (picker.selected + 1).min(list.len().saturating_sub(1))
                    }
                    Key::Up => picker.selected = picker.selected.saturating_sub(1),
                    Key::Enter if !list.is_empty() => accept = Some(list[picker.selected].clone()),
                    _ => {}
                }
            }
        });
        if let Some(app) = accept {
            self.add_installed_app(&app);
            self.picker_focused = false;
        } else if close {
            self.picker_focused = false;
        } else {
            self.picker = Some(picker);
        }
        self.repaint = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_matches_names_ignoring_case() {
        let apps = vec![
            AppChoice {
                name: "Notepad".into(),
                target: "notepad.exe".into(),
                app_id: None,
            },
            AppChoice {
                name: "Calculator".into(),
                target: "calc".into(),
                app_id: Some("Microsoft.Calc".into()),
            },
        ];
        assert_eq!(filter(&apps, " NOTE ").len(), 1);
        assert_eq!(filter(&apps, "").len(), 2);
        assert!(filter(&apps, "zz").is_empty());
    }
}
