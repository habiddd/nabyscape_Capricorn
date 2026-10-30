//! The page builders of `SettingsWindow.cs`: Appearance, Interactions (Controls), Content,
//! Launch, closed media display, Display and General. Sections are skipped by
//! `SettingsView::section` when the current category does not show them.

use super::widget_order::OrderScope;
use super::SettingsView;
use naby_core::command::{Command, FilePurpose, SettingsRequest};
use naby_core::settings::reset::{self, ResetCategory, ResetScope};
use naby_core::settings::{Settings, Shortcut};
use naby_render::{Fill, Icon, Rect, Ui, Weight};

const ISLAND_COLORS: [&str; 4] = ["#000000", "#101114", "#182028", "#25221E"];
const ACCENT_COLORS: [&str; 4] = ["#FFFFFF", "#E3D2AF", "#A8C6F2", "#B7D6BC"];
const KEYS: [&str; 26] = [
    "A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M", "N", "O", "P", "Q", "R", "S",
    "T", "U", "V", "W", "X", "Y", "Z",
];
const MODIFIERS: [&str; 3] = ["Ctrl+Alt", "Ctrl+Shift", "Alt+Shift"];

impl SettingsView {
    // ---- Appearance / Ambience / Motion -------------------------------------------------

    pub(super) fn appearance<'a>(&mut self, ui: &mut Ui<'a>) {
        self.section(ui, "Shape", |this, ui| this.shape_choices(ui));
        self.material_controls(ui);
        self.section(ui, "Color & material", |this, ui| {
            let (background, accent) = (this.s.background.clone(), this.s.accent.clone());
            this.color_control(ui, "Island color", &background, &ISLAND_COLORS, |s, v| {
                s.background = v
            });
            this.color_control(ui, "Accent color", &accent, &ACCENT_COLORS, |s, v| {
                s.accent = v
            });
            let opacity = this.s.opacity * 100.0;
            this.slider(ui, "Opacity", opacity, 65.0, 100.0, "%", 1.0, |s, v| {
                s.opacity = v / 100.0
            });
            let glass = this.s.glass_style;
            this.check(ui, "Black glass", glass, |s, v| s.glass_style = v);
            let album = this.s.album_accent;
            this.check(ui, "Color from album artwork", album, |s, v| {
                s.album_accent = v
            });
        });
        self.ambient_settings(ui);
        self.control_color_settings(ui);
        self.section(ui, "Surface", |this, ui| {
            let strength = this.s.glass_strength * 100.0;
            this.slider(
                ui,
                "Glass intensity",
                strength,
                0.0,
                100.0,
                "%",
                1.0,
                |s, v| s.glass_strength = v / 100.0,
            );
        });
        self.section(ui, "Text and navigation", |this, ui| {
            this.muted(
                ui,
                "Dots keep your panels easy to reach. Hover over a dot to see its name.",
                11.0,
            );
        });
        self.section(ui, "Typography", |this, ui| {
            let title = this.s.bold_media_title;
            this.check(ui, "Bold media titles", title, |s, v| s.bold_media_title = v);
            let headings = this.s.bold_headings;
            this.check(ui, "Bold panel and tool headings", headings, |s, v| s.bold_headings = v);
            this.muted(
                ui,
                "Use stronger titles without changing body text, timestamps or counters. Each option is independent; the adjacent preview shows both weights.",
                11.0,
            );
        });
        self.section(ui, "Motion", |this, ui| {
            let motion = this.s.interaction_motion.clone();
            this.choice(
                ui,
                "Interaction feedback",
                "Hover & click feedback",
                &[("Off", "Off"), ("Subtle", "Subtle"), ("Expressive", "Expressive")],
                &motion,
                |s, v| s.interaction_motion = v.to_owned(),
            );
            let ms = this.s.animation_ms;
            this.slider(ui, "Animation duration", ms, 0.0, 600.0, " ms", 10.0, |s, v| s.animation_ms = v);
            this.muted(
                ui,
                "Animations follow the Windows compositor. Actual FPS depends on your display and GPU load.",
                11.0,
            );
            let live = this.s.live_activity;
            this.check(ui, "Live activity · audio, progress & focus breathing", live, |s, v| s.live_activity = v);
            let reduce = this.s.reduce_motion;
            this.check(ui, "Reduce motion", reduce, |s, v| s.reduce_motion = v);
        });
        self.section(ui, "Animation performance", |this, ui| {
            this.muted(
                ui,
                "Auto follows your display. Save power reduces update frequency; Smoother motion allows more frequent frames.",
                11.0,
            );
            let current = this.s.performance.clone();
            this.combo_plain(
                ui,
                "Performance profile",
                &[("Auto (recommended)", "Auto"), ("Save power", "Hemat"), ("Smoother motion", "Maksimal")],
                &current,
                |s, v| s.performance = v.to_owned(),
            );
            this.advanced(ui, "Render statistics", |this, ui| {
                let text = this.host.frame_status.clone();
                let shown = if text.is_empty() {
                    this.tr("Render statistics appear while the island animates.").to_owned()
                } else {
                    text
                };
                this.muted_final(ui, &shown, 11.0);
            });
        });
    }

    /// `ShapeChoices`: the notch and pill silhouettes.
    fn shape_choices<'a>(&mut self, ui: &mut Ui<'a>) {
        let th = ui.theme().clone();
        let pal = self.pal(ui);
        let width = ui.available_width();
        let gap = 12.0;
        let w = ((width - gap) / 2.0).max(60.0);
        let h = 12.0 + 2.0 + 27.0 + 10.0 + 17.0 + 12.0;
        let (x0, y0) = ui.cursor_pos();
        let outer = ui.allocate(None, h);
        let _ = (x0, y0);
        let mut chosen: Option<&str> = None;
        for (i, (shape, label)) in [("Mac", "Notch"), ("iPhone", "Pill")].iter().enumerate() {
            let rect = Rect::new(outer.x + i as f32 * (w + gap), outer.y, w, h);
            let selected = self.s.preset == *shape;
            ui.push_id(*shape, |ui| {
                let resp = ui.hit_area(rect, true);
                self.record(&format!("shape:{shape}"), rect);
                self.hand |= resp.hovered;
                let (bg, border) = if pal.hc {
                    (pal.window, th.text)
                } else if selected {
                    (hexc("#2C2F35"), hexc("#B9BCC4"))
                } else {
                    (hexc("#1A1C20"), hexc("#35383F"))
                };
                let c = ui.canvas();
                c.fill_rrect(rect, 10.0, bg);
                if resp.hovered {
                    c.fill_rrect(rect, 10.0, th.hover_fill);
                }
                c.stroke_round_rect(rect, 10.0, border, 1.0);
                let contour =
                    naby_core::motion::Contour::create(110.0, 27.0, 12.0, *shape == "Mac", 0.0);
                c.fill_contour(
                    &contour,
                    rect.x + (rect.w - 110.0) / 2.0,
                    rect.y + 12.0 + 2.0,
                    1.0,
                    &Fill::Solid(naby_core::color::Rgba::BLACK),
                );
                c.stroke_contour(
                    &contour,
                    rect.x + (rect.w - 110.0) / 2.0,
                    rect.y + 12.0 + 2.0,
                    1.0,
                    &Fill::Solid(hexc("#2A2E36")),
                    0.75,
                );
                let st = ui.text_style(12.0, Weight::Regular, th.text);
                let text = self.tr(label).to_owned();
                let tw = ui.fonts().measure_width(&text, &st);
                ui.draw_text(
                    rect.x + (rect.w - tw) / 2.0,
                    rect.y + 12.0 + 2.0 + 27.0 + 10.0,
                    &text,
                    &st,
                    None,
                );
                ui.paint_focus_ring(&resp, 10.0);
                let tip = self
                    .tr(if *shape == "Mac" {
                        "Use notch shape"
                    } else {
                        "Use pill shape"
                    })
                    .to_owned();
                ui.tooltip(&resp, &tip);
                if resp.clicked {
                    chosen = Some(shape);
                }
            });
        }
        if let Some(shape) = chosen {
            // `Preset(name)`: a notch sits on the edge, a pill floats at least 8 px below it.
            self.s.preset = shape.to_owned();
            self.s.top = if shape == "Mac" {
                0.0
            } else {
                self.s.top.max(8.0)
            };
            self.configure_preview_for_setting("Shape");
            self.save();
            self.repaint = true;
        }
    }

    // ---- Controls / Visibility ------------------------------------------------------------

    pub(super) fn interactions<'a>(&mut self, ui: &mut Ui<'a>) {
        self.hover_controls(ui);
        self.interaction_feature_controls(ui);
        self.section(ui, "Automatic activity", |this, ui| {
            let smart = this.s.smart_activities;
            this.check(ui, "Preview new tracks & sessions ending soon", smart, |s, v| s.smart_activities = v);
            let browser = this.s.browser_media_previews;
            this.check(ui, "Automatic browser media previews", browser, |s, v| s.browser_media_previews = v);
            this.muted(
                ui,
                "Turn off to suppress automatic browser previews, including YouTube hover playback. Unknown Windows media identities are also suppressed. Manual island hover and playback controls still work.",
                11.0,
            );
            ui.gap(6.0);
            this.muted(ui, "Brief previews appear without taking focus or replacing an open panel.", 12.0);
        });
        self.section(ui, "Notch gestures", |this, ui| {
            this.muted(ui, "Mouse wheel", 12.0);
            let wheel = this.s.wheel_action.clone();
            this.combo_plain(
                ui,
                "Wheel action",
                &[
                    ("Volume", "volume"),
                    ("Switch tabs", "pages"),
                    ("Off", "none"),
                ],
                &wheel,
                |s, v| s.wheel_action = v.to_owned(),
            );
            this.muted(ui, "Middle click", 12.0);
            let middle = this.s.middle_action.clone();
            this.combo_plain(
                ui,
                "Middle-click action",
                &[
                    ("Play / pause", "toggle"),
                    ("Mute microphone", "microphone"),
                    ("Off", "none"),
                ],
                &middle,
                |s, v| s.middle_action = v.to_owned(),
            );
            let step = f64::from(this.s.volume_step);
            this.slider(ui, "Volume step", step, 1.0, 10.0, "%", 1.0, |s, v| {
                s.volume_step = v as i32
            });
        });
        self.section(ui, "Global hotkey", |this, ui| {
            let enabled = this.s.hotkey_enabled;
            this.check(ui, "Enable open / close hotkey", enabled, |s, v| s.hotkey_enabled = v);
            let modifiers = this.s.hotkey_modifiers.clone();
            let options: Vec<(&str, &str)> = MODIFIERS.iter().map(|m| (*m, *m)).collect();
            this.combo_plain(ui, "Hotkey modifiers", &options, &modifiers, |s, v| s.hotkey_modifiers = v.to_owned());
            let key = this.s.hotkey_key.clone();
            let keys: Vec<(&str, &str)> = KEYS.iter().map(|k| (*k, *k)).collect();
            this.combo_plain(ui, "Hotkey key", &keys, &key, |s, v| s.hotkey_key = v.to_owned());
            this.hotkey_status(ui);
            this.muted(
                ui,
                "Default: Ctrl+Alt+I. Use Ctrl+K for commands, Ctrl+Tab to switch tabs, and Esc to go back or close. Choose another hotkey if an app already uses it.",
                12.0,
            );
        });
        self.visibility_controls(ui);
    }

    /// The registration status lines of the three hotkeys plus conflicts between them
    /// (`hotkeyState`).
    pub(crate) fn hotkey_lines(&self) -> Vec<String> {
        let mut lines: Vec<String> = Vec::new();
        for (i, action) in ["Panel", "Visibility", "Tuck"].iter().enumerate() {
            let host = self.host.hotkey_status[i].trim();
            if !host.is_empty() {
                lines.push(host.to_owned());
            }
            let conflict = self.s.hotkey_conflict(action);
            if !conflict.is_empty() && !lines.iter().any(|l| l.contains(&conflict)) {
                lines.push(conflict);
            }
        }
        lines
    }

    fn hotkey_status<'a>(&self, ui: &mut Ui<'a>) {
        let lines = self.hotkey_lines();
        if lines.is_empty() {
            return;
        }
        ui.gap(4.0);
        let text = lines.join("\n");
        self.muted_final(ui, &text, 11.0);
        ui.gap(6.0);
    }

    // ---- Widgets & tools ------------------------------------------------------------------

    pub(super) fn content_settings<'a>(&mut self, ui: &mut Ui<'a>) {
        self.feature_resource_settings(ui);
        self.section(ui, "Enabled widgets & order", |this, ui| {
            this.muted(
                ui,
                "Drag the grip to arrange your widgets. Switch any widget on or off.",
                11.0,
            );
            ui.gap(6.0);
            this.widget_order(ui, OrderScope::Widgets);
        });
        if super::categories::category(&self.current)
            .sections
            .is_none()
            || self.current == "Clipboard"
        {
            self.muted(
                ui,
                "Each tool has its own panel above. Enable and reorder Notes, Tasks, Shelf, Clipboard, Calendar, Keep awake and Clean links alongside your other widgets. Saved content is preserved.",
                11.0,
            );
            ui.gap(10.0);
        }
        self.section(ui, "Focus sessions", |this, ui| {
            let focus = f64::from(this.s.focus_minutes);
            this.slider(
                ui,
                "Focus duration",
                focus,
                1.0,
                240.0,
                " min",
                1.0,
                |s, v| s.focus_minutes = v as i32,
            );
            let brk = f64::from(this.s.break_minutes);
            this.slider(
                ui,
                "Break duration",
                brk,
                1.0,
                120.0,
                " min",
                1.0,
                |s, v| s.break_minutes = v as i32,
            );
            this.muted(ui, "New durations apply to the next session.", 11.0);
        });
        self.section(ui, "Stopwatch", |this, ui| {
            this.stopwatch_view(ui);
            ui.gap(8.0);
            this.muted(
                ui,
                "Count elapsed time with Start, Pause, Resume and Reset. Sleep is counted, and the session survives restart. Tab to a button and press Space or Enter. Enable Stopwatch above to show it in Focus.",
                11.0,
            );
        });
        self.hover_page_settings(ui);
        self.section(ui, "Clipboard history", |this, ui| {
            let history = this.s.clipboard_history;
            this.check(ui, "Save text & image history locally", history, |s, v| s.clipboard_history = v);
            this.muted(
                ui,
                "Off by default. Capture starts only when enabled. Turning it off clears saved history and images. Pause and pin items in Tools → Clipboard.",
                12.0,
            );
        });
    }

    /// The compact stopwatch inside the Stopwatch section.
    fn stopwatch_view<'a>(&mut self, ui: &mut Ui<'a>) {
        use naby_core::session::Clock;
        let (display, label, running) = match &self.snaps {
            Some(s) => {
                let mut sw = s.session.stopwatch.clone();
                let clock = Clock::system();
                (sw.display(clock), sw.action_label(clock), sw.running)
            }
            None => ("00:00".to_owned(), "Start", false),
        };
        if running {
            ui.request_repaint_after(0.5);
        }
        self.text_final(ui, &display, 28.0, Weight::Semibold);
        ui.gap(6.0);
        if let Some(i) = self.wrap_buttons(ui, &[(label, true), ("Reset", true)]) {
            self.out.push(if i == 0 {
                Command::StopwatchToggle
            } else {
                Command::StopwatchReset
            });
        }
    }

    fn hover_page_settings<'a>(&mut self, ui: &mut Ui<'a>) {
        self.section(ui, "Hover pages", |this, ui| {
            let follows = this.s.hover_follows_widgets;
            this.check(ui, "Show all active widgets in hover", follows, |s, v| s.hover_follows_widgets = v);
            this.muted(
                ui,
                "Uses the same order as Your widgets. Turn this off to choose a separate hover order below.",
                11.0,
            );
            if !this.s.hover_follows_widgets {
                ui.gap(8.0);
                this.widget_order(ui, OrderScope::Hover);
            }
        });
    }

    // ---- Shortcuts ------------------------------------------------------------------------

    pub(super) fn launch_settings<'a>(&mut self, ui: &mut Ui<'a>) {
        self.section(ui, "Launch display", |this, ui| {
            this.style_choice(ui, "Launch appearance");
        });
        self.section(ui, "Shortcuts", |this, ui| this.shortcut_editor(ui));
    }

    /// `StyleChoice(label, selected, change)`.
    pub(super) fn style_choice<'a>(&mut self, ui: &mut Ui<'a>, label: &str) {
        let navigation = !label.starts_with("Launch");
        let current = if navigation {
            self.s.category_style.clone()
        } else {
            self.s.launch_style.clone()
        };
        let options: Vec<(&str, &str)> = if navigation {
            vec![
                ("Icons + labels", "IconsLabels"),
                ("Icons only", "Icons"),
                ("Text only", "Text"),
                ("Dots", "Dots"),
            ]
        } else {
            vec![
                ("Icons + labels", "IconsLabels"),
                ("Icons only", "Icons"),
                ("Text only", "Text"),
            ]
        };
        ui.gap(4.0);
        let th = ui.theme().clone();
        let avail = ui.available_width();
        let st = self.style(ui, 12.0, Weight::Regular, false);
        let title = self.tr(label).to_owned();
        let labels: Vec<String> = options.iter().map(|(l, _)| self.tr(l).to_owned()).collect();
        let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        let mut idx = options.iter().position(|(_, v)| *v == current).unwrap_or(0);
        let mut changed = false;
        ui.row_centered(34.0, |ui| {
            ui.spacing(0.0);
            let r = ui.allocate(Some((avail - 152.0).max(40.0)), 34.0);
            let lh = ui.line_height(&st);
            ui.draw_text(r.x, r.y + (r.h - lh) / 2.0, &title, &st, Some(r.w));
            ui.width(152.0, |ui| {
                let resp = ui.combo(&mut idx, &refs);
                self.hand |= resp.hovered;
                self.record(label, resp.rect);
                changed = resp.changed;
            });
        });
        let _ = th;
        if changed {
            let value = options[idx].1.to_owned();
            if navigation {
                self.s.category_style = value;
            } else {
                self.s.launch_style = value;
            }
            self.pv.mode = super::preview_motion::PreviewMode::Panel;
            self.pv.content = if navigation { "Navigation" } else { "Launch" }.to_owned();
            self.save();
        }
        ui.gap(10.0);
        self.muted(
            ui,
            if navigation {
                "Dots keep navigation minimal. Hover or focus a dot for its panel name; use Ctrl+Tab to switch. More panels scroll sideways."
            } else {
                "Choose how saved apps and shortcuts appear in Launch."
            },
            11.0,
        );
        ui.gap(14.0);
    }

    fn shortcut_editor<'a>(&mut self, ui: &mut Ui<'a>) {
        if self.button(ui, "Add installed app") {
            self.open_app_picker();
        }
        ui.gap(8.0);
        let th = ui.theme().clone();
        let count = self.s.shortcuts.len();
        let mut remove: Option<usize> = None;
        let mut swap: Option<(usize, usize)> = None;
        for i in 0..count {
            let name = self.s.shortcuts[i].name.clone();
            let target = self.s.shortcuts[i].target.clone();
            let row = ui.allocate(None, 46.0);
            let btn_w = 150.0;
            let st = ui.text_style(13.0, Weight::Regular, th.text);
            let ts = ui.text_style(10.0, Weight::Regular, th.text_dim);
            ui.draw_text(row.x, row.y + 2.0, &name, &st, Some(row.w - btn_w - 8.0));
            ui.draw_text(
                row.x,
                row.y + 2.0 + ui.line_height(&st),
                &target,
                &ts,
                Some(row.w - btn_w - 8.0),
            );
            let buttons = Rect::new(row.right() - btn_w, row.y + 4.0, btn_w, 34.0);
            ui.push_id(i, |ui| {
                ui.child(buttons, |ui| {
                    ui.row(|ui| {
                        ui.spacing(4.0);
                        ui.enabled(i > 0, |ui| {
                            let up = ui.button("↑");
                            self.hand |= up.hovered && i > 0;
                            self.record(&format!("up:{i}"), up.rect);
                            if up.clicked {
                                swap = Some((i, i - 1));
                            }
                        });
                        ui.enabled(i + 1 < count, |ui| {
                            let down = ui.button("↓");
                            self.hand |= down.hovered && i + 1 < count;
                            self.record(&format!("down:{i}"), down.rect);
                            if down.clicked {
                                swap = Some((i, i + 1));
                            }
                        });
                        let rm = ui.button(self.tr("Remove"));
                        self.hand |= rm.hovered;
                        self.record(&format!("remove:{i}"), rm.rect);
                        if rm.clicked {
                            remove = Some(i);
                        }
                    });
                });
            });
            ui.gap(8.0);
        }
        if let Some((a, b)) = swap {
            self.s.shortcuts.swap(a, b);
            self.save();
        }
        if let Some(i) = remove {
            self.s.shortcuts.remove(i);
            self.save();
        }
        self.muted(ui, "Shortcut name", 12.0);
        ui.gap(4.0);
        let (_, name) = self.field(ui, "shortcut-name", "e.g. Browser", false);
        self.shortcut_name = name;
        self.muted(ui, "Application, folder, or URL", 12.0);
        ui.gap(4.0);
        let (_, target) = self.field(
            ui,
            "shortcut-target",
            "Application path, folder, or https:// URL",
            false,
        );
        self.shortcut_target = target;
        if let Some(i) = self.wrap_buttons(ui, &[("Choose file", true), ("Choose folder", true)]) {
            self.out
                .push(Command::Settings(SettingsRequest::PickFile(if i == 0 {
                    FilePurpose::ShortcutFile
                } else {
                    FilePurpose::ShortcutFolder
                })));
        }
        if self.primary_button(ui, "Add shortcut") {
            self.add_shortcut();
        }
    }

    /// The "Add shortcut" button.
    pub(crate) fn add_shortcut(&mut self) {
        let name = self.shortcut_name.trim().to_owned();
        let target = self.shortcut_target.trim().to_owned();
        if name.is_empty() || target.is_empty() {
            self.set_feedback("Enter a name and destination first.", false);
            return;
        }
        if self.s.shortcuts.len() >= 40 {
            self.set_feedback("You can add up to 40 shortcuts.", false);
            return;
        }
        self.s.shortcuts.push(Shortcut {
            name,
            target,
            app_id: None,
        });
        self.fields.remove("shortcut-name");
        self.fields.remove("shortcut-target");
        self.shortcut_name.clear();
        self.shortcut_target.clear();
        self.save();
    }

    // ---- Music & sound --------------------------------------------------------------------

    pub(super) fn audio_settings<'a>(&mut self, ui: &mut Ui<'a>) {
        self.media_appearance_settings(ui);
        self.media_experience_settings(ui);
        self.audio_feedback_settings(ui);
        self.section(ui, "Volume mixer", |this, ui| {
            this.muted(
                ui,
                "In the compact media panel, choose the speaker button for volume and mute. The mixer icon opens Apps or Outputs; the chevron opens devices and microphone controls. Choose the media source button to switch playing apps. Tab to a button and press Enter or Space; Esc closes its popover.",
                12.0,
            );
            if this.button(ui, "Open volume mixer") {
                this.out.push(Command::NavigatePage("mixer".to_owned()));
            }
        });
        self.section(ui, "Live signal", |this, ui| {
            this.muted(
                ui,
                "Follows the selected app across its active outputs. Spectrum includes other sounds sharing those outputs; Amplitude history reads the selected app level only.",
                11.0,
            );
            ui.gap(4.0);
            this.audio_signal(ui);
            this.muted(
                ui,
                "Spectrum shows 25 real frequency bands from low to high. If output capture is unavailable, the waveform uses measured amplitude history. Sensitivity changes the visible range without adding artificial beats.",
                11.0,
            );
        });
        self.section(ui, "Audio behavior", |this, ui| {
            this.muted(
                ui,
                "Output selection changes the Windows default device. Microphone mute is available from the audio menu.",
                12.0,
            );
            if this.button(ui, "Windows sound settings") {
                this.out.push(Command::OpenUrl("ms-settings:sound".to_owned()));
            }
        });
    }

    /// `MediaAppearanceSettings`: the closed dock's media icon and content.
    fn media_appearance_settings<'a>(&mut self, ui: &mut Ui<'a>) {
        self.section(ui, "Closed media display", |this, ui| {
            let th = ui.theme().clone();
            let items: [(&str, &str, &str); 4] = [
                ("Waveform", "Waveform", "Follow the music"),
                ("Lyrics", "Live lyrics", "One line, in sync"),
                ("Title", "Song title", "Keep it simple"),
                ("None", "None", "Show idle or your timer"),
            ];
            ui.gap(8.0);
            let width = ui.available_width();
            let cell_w = ((width - 8.0) / 2.0).max(80.0);
            let mut picked: Option<&str> = None;
            let current = this.s.closed_media_content.clone();
            for (row_i, pair) in items.chunks(2).enumerate() {
                let rect = ui.allocate(None, 76.0);
                for (col, (id, label, hint)) in pair.iter().enumerate() {
                    let r = Rect::new(rect.x + col as f32 * (cell_w + 8.0), rect.y, cell_w, 76.0);
                    let selected = current == *id;
                    ui.push_id((row_i, col), |ui| {
                        let resp = ui.hit_area(r, true);
                        this.record(&format!("closed:{id}"), r);
                        this.wire(&resp, "Closed media content");
                        let fill = if selected {
                            naby_core::color::Rgba::parse_hex(&this.s.selection_fill).unwrap_or(th.selection_fill)
                        } else {
                            naby_core::color::Rgba::parse_hex(&this.s.control_fill).unwrap_or(th.control_fill)
                        };
                        let c = ui.canvas();
                        c.fill_rrect(r, th.radius, fill);
                        if resp.hovered {
                            c.fill_rrect(r, th.radius, th.hover_fill);
                        }
                        let fg = naby_render::Theme::contrast_text(fill);
                        let title = format!("{}{}", this.tr(label), if selected { " · Selected" } else { "" });
                        let st = ui.text_style(14.0, Weight::Regular, fg);
                        let hs = ui.text_style(12.0, Weight::Regular, th.text_dim);
                        ui.draw_text(r.x + 14.0, r.y + 14.0, &title, &st, Some(r.w - 28.0));
                        let h = ui.line_height(&st);
                        ui.draw_text(r.x + 14.0, r.y + 14.0 + h + 5.0, this.tr(hint), &hs, Some(r.w - 28.0));
                        ui.paint_focus_ring(&resp, th.radius);
                        if resp.clicked {
                            picked = Some(id);
                        }
                    });
                }
                ui.gap(8.0);
            }
            if let Some(id) = picked {
                this.s.closed_media_content = id.to_owned();
                this.pv.mode = super::preview_motion::PreviewMode::Closed;
                this.pv.content = "Music".to_owned();
                this.save();
            }
            this.muted(
                ui,
                "Live lyrics switches to a waveform when lyrics are unavailable or during marked instrumental breaks. Track details are sent to LRCLIB while visible. None shows idle or your timer without stopping music.",
                12.0,
            );
            ui.gap(10.0);
            this.muted(ui, "Media icon", 12.0);
            let icon = this.s.closed_media_icon.clone();
            this.combo_plain(
                ui,
                "Closed media icon",
                &[("Waveform", "Waveform"), ("Album artwork", "Artwork"), ("None", "None")],
                &icon,
                |s, v| s.closed_media_icon = v.to_owned(),
            );
            this.pv.mode = if this.pv.mode == super::preview_motion::PreviewMode::Panel && this.current == "Audio" {
                super::preview_motion::PreviewMode::Panel
            } else {
                this.pv.mode
            };
        });
    }

    // ---- Screen ---------------------------------------------------------------------------

    pub(super) fn display_settings<'a>(&mut self, ui: &mut Ui<'a>) {
        self.section(ui, "Display", |this, ui| {
            let mode = this.s.display_mode.clone();
            this.combo_plain(
                ui,
                "Display mode",
                &[("Primary", "Primary"), ("Selected", "Selected"), ("Follow cursor", "Follow")],
                &mode,
                |s, v| s.display_mode = v.to_owned(),
            );
            // The monitor chooser: "Primary display (automatic)" plus the connected displays.
            let mut labels: Vec<String> = vec![this.tr("Primary display (automatic)").to_owned()];
            labels.extend(this.host.monitors.iter().map(|m| m.label.clone()));
            let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
            let mut idx = this
                .host
                .monitors
                .iter()
                .position(|m| m.device == this.s.monitor)
                .map_or(0, |p| p + 1);
            ui.gap(4.0);
            let resp = ui.combo(&mut idx, &refs);
            this.record("Island display", resp.rect);
            this.wire(&resp, "Island display");
            let tip = this.tr(&super::help::setting_description("Island display")).to_owned();
            ui.tooltip(&resp, &tip);
            if resp.changed {
                if idx > 0 {
                    let device = this.host.monitors[idx - 1].device.clone();
                    this.s.monitor = device;
                    this.s.display_mode = "Selected".to_owned();
                } else {
                    this.s.monitor.clear();
                    this.s.display_mode = "Primary".to_owned();
                }
                this.save();
            }
            ui.gap(10.0);
            let follow = this.s.follow_monitor_animation;
            this.check(ui, "Slide smoothly between monitors", follow, |s, v| s.follow_monitor_animation = v);
            let dur = this.s.follow_monitor_duration;
            this.slider(ui, "Monitor slide duration", dur, 180.0, 700.0, "ms", 10.0, |s, v| s.follow_monitor_duration = v);
            let missing = !this.s.monitor.is_empty()
                && !this.host.monitors.iter().any(|m| m.device == this.s.monitor);
            let status = if missing {
                format!(
                    "{}: {} · {}",
                    this.tr("Saved display unavailable"),
                    this.s.monitor,
                    this.tr("using Primary fallback")
                )
            } else {
                this.tr("Selected display is saved; cursor moves never rewrite this preference.")
                    .to_owned()
            };
            this.text_final(ui, &status, 11.0, Weight::Regular);
            ui.gap(6.0);
            this.muted(
                ui,
                "Primary uses the main display. Selected uses the display below, falling back to Primary if disconnected. Follow cursor slides after 250 ms on another display; it waits during interaction and manual hide. Detached previews stay where placed. Example: move to a second display, pause, then continue working.",
                11.0,
            );
        });
    }

    // ---- General --------------------------------------------------------------------------

    pub(super) fn general<'a>(&mut self, ui: &mut Ui<'a>) {
        self.language_and_accessibility_settings(ui);
        self.updates_section(ui);
        self.diagnostics_section(ui);
        self.notification_settings(ui);
        self.section(ui, "Behavior", |this, ui| {
            if this.button(ui, "Open setup guide") {
                this.out.push(Command::Settings(SettingsRequest::ShowWelcome));
            }
            let auto = this.s.auto_start;
            if this.check(ui, "Start automatically when signing in to Windows", auto, |s, v| s.auto_start = v) {
                let want = this.s.auto_start;
                this.out.push(Command::Settings(SettingsRequest::SetAutoStart(want)));
            }
            this.muted(
                ui,
                "Launch NabyScape for this Windows user when you sign in. You can turn this off at any time.",
                11.0,
            );
            ui.gap(6.0);
            this.muted(
                ui,
                "NabyScape keeps running when you close Settings. Quit from the system tray.",
                12.0,
            );
        });
        self.section(ui, "Local storage", |this, ui| {
            this.muted(ui, "Preferences and timers stay on this device. No account required.", 12.0);
            if this.button(ui, "Open settings folder") {
                let dir = this.host.settings_dir.clone();
                this.out.push(Command::OpenPath(dir));
            }
            if this.button(ui, "Uninstall NabyScape…") {
                this.out.push(Command::Settings(SettingsRequest::OpenUninstaller));
            }
            this.muted(
                ui,
                "Uninstall can remove downloaded speech models, caches, history and saved connections too.",
                11.0,
            );
            if this.button(ui, "Quit NabyScape") {
                this.out.push(Command::Quit);
            }
        });
        self.section(ui, "Reset preferences", |this, ui| {
            let summary = reset::summary(ResetScope::All);
            this.text_final(ui, &summary, 11.0, Weight::Regular);
            ui.gap(6.0);
            this.muted(
                ui,
                "Choose a category to reset only its preferences; for example Appearance keeps your current layout.",
                11.0,
            );
            ui.gap(6.0);
            let cats: Vec<ResetCategory> = ResetCategory::ALL
                .iter()
                .copied()
                .filter(|c| !c.keys().is_empty())
                .collect();
            let titles: Vec<String> = cats
                .iter()
                .map(|c| super::categories::category_title(&this.lang, c.name()))
                .collect();
            let refs: Vec<&str> = titles.iter().map(String::as_str).collect();
            let mut idx = this.reset_choice.min(cats.len().saturating_sub(1));
            let resp = ui.combo(&mut idx, &refs);
            this.record("Preference reset category", resp.rect);
            this.hand |= resp.hovered;
            this.reset_choice = idx;
            match this.wrap_buttons(
                ui,
                &[
                    ("Reset this category", true),
                    ("Reset all preferences", true),
                    ("Restore last preference backup", true),
                ],
            ) {
                Some(0) => this.ask_reset(ResetScope::Category(cats[idx])),
                Some(1) => this.ask_reset(ResetScope::All),
                Some(_) => this.ask_restore(),
                None => {}
            }
        });
        self.section(ui, "About NabyScape", |this, ui| {
            let version = format!(
                "{} {} · Windows · {}",
                this.tr("Version"),
                this.host.app_version,
                this.host.platform
            );
            this.text_final(ui, &version, 14.0, Weight::Regular);
            ui.gap(6.0);
            this.muted(
                ui,
                "Media controls depend on support from your media app. Battery appears on devices with a battery. Transparency controls opacity; glass uses a dark gradient and reflections.",
                12.0,
            );
            ui.gap(6.0);
            this.muted(
                ui,
                "Focus sessions run in NabyScape and do not change Windows Do Not Disturb.",
                12.0,
            );
        });
    }

    #[allow(dead_code)]
    pub(crate) fn icon_for(name: &str) -> Option<Icon> {
        Icon::ALL.iter().copied().find(|i| i.name() == name)
    }

    #[allow(dead_code)]
    pub(crate) fn settings_ref(&self) -> &Settings {
        &self.s
    }
}

fn hexc(s: &str) -> naby_core::color::Rgba {
    naby_core::color::Rgba::parse_hex(s).unwrap_or(naby_core::color::Rgba::BLACK)
}
