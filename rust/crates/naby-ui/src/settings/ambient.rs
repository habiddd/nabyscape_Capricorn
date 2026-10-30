//! Ambient light and notification presentation (C# `SettingsAmbient.cs`).

use super::SettingsView;
use chrono::{Local, Utc};
use naby_core::command::Command;
use naby_core::notifications::NoticeKind;
use naby_core::settings::WIDGET_NAMES;
use naby_render::Ui;

const CUSTOM_COLORS: [&str; 4] = ["#7AA8DC", "#D9A46D", "#B4A0DC", "#73BFA6"];
const SECOND_COLORS: [&str; 4] = ["#CB72D9", "#50BBD7", "#E6AF62", "#76C18E"];

impl SettingsView {
    /// The widget ids ambient light can be limited to (every widget but the tools group, plus
    /// the AI companion) with their display names.
    fn ambient_widget_list() -> Vec<(&'static str, &'static str)> {
        WIDGET_NAMES
            .iter()
            .filter(|(id, _)| *id != "tools")
            .copied()
            .chain(std::iter::once(("ai", "AI companion")))
            .collect()
    }

    pub(super) fn ambient_settings<'a>(&mut self, ui: &mut Ui<'a>) {
        self.section(ui, "Ambient light", |this, ui| {
            let light = this.s.cover_ambient;
            this.check(ui, "Ambient light", light, |s, v| s.cover_ambient = v);
            let closed = this.s.closed_ambient;
            this.check(ui, "Show ambience on closed dock", closed, |s, v| s.closed_ambient = v);
            let scope = this.s.ambient_scope.clone();
            this.choice(
                ui,
                "Ambient scope",
                "Where the light appears",
                &[("All widgets", "All widgets"), ("Music only", "Music only"), ("Selected widgets", "Selected widgets")],
                &scope,
                |s, v| s.ambient_scope = v.to_owned(),
            );
            if this.s.ambient_scope == "Selected widgets" {
                this.expander(ui, "Choose widgets", "Choose widgets", |this, ui| {
                    for (id, name) in SettingsView::ambient_widget_list() {
                        let label = format!("{} ambient", this.tr(name));
                        let on = this.s.ambient_widgets.iter().any(|w| w == id);
                        this.check_label(ui, &label, on, |s, v| {
                            if v {
                                if !s.ambient_widgets.iter().any(|w| w == id) {
                                    s.ambient_widgets.push(id.to_owned());
                                }
                            } else {
                                s.ambient_widgets.retain(|w| w != id);
                            }
                        });
                    }
                });
                ui.gap(12.0);
            }
            let source = this.s.ambient_color_source.clone();
            this.choice(
                ui,
                "Ambient color source",
                "Color source",
                &[
                    ("Album cover", "Album cover"),
                    ("Accent color", "Accent color"),
                    ("Custom color", "Custom color"),
                    ("Custom blend", "Custom blend"),
                ],
                &source,
                |s, v| s.ambient_color_source = v.to_owned(),
            );
            if this.s.ambient_color_source.starts_with("Custom") {
                let custom = this.s.ambient_custom_color.clone();
                this.color_control(ui, "Ambient custom color", &custom, &CUSTOM_COLORS, |s, v| s.ambient_custom_color = v);
                if this.s.ambient_color_source == "Custom blend" {
                    let second = this.s.ambient_secondary_color.clone();
                    this.color_control(ui, "Second ambient color", &second, &SECOND_COLORS, |s, v| s.ambient_secondary_color = v);
                }
            }
            this.muted(
                ui,
                "Album color rises from the bottom and fades into the dark surface. Without artwork, your accent color is used.",
                11.0,
            );
            ui.gap(6.0);
            let strength = this.s.ambient_strength * 100.0;
            this.slider(ui, "Ambient intensity", strength, 0.0, 100.0, "%", 1.0, |s, v| s.ambient_strength = v / 100.0);
            let palette = this.s.ambient_palette.clone();
            this.choice(
                ui,
                "Cover palette",
                "Album colors",
                &[("Single color", "Single color"), ("Cover blend", "Cover blend")],
                &palette,
                |s, v| s.ambient_palette = v.to_owned(),
            );
            this.advanced(ui, "Fine-tune light", |this, ui| {
                let direction = this.s.ambient_direction.clone();
                this.choice_same(
                    ui,
                    "Light direction",
                    "Light direction",
                    &["Bottom", "Top", "Left", "Right", "Surround"],
                    &direction,
                    |s, v| s.ambient_direction = v.to_owned(),
                );
                let (balance, reach, soft, spread) = (
                    this.s.ambient_balance * 100.0,
                    this.s.ambient_height * 100.0,
                    this.s.ambient_softness * 100.0,
                    this.s.ambient_spread * 100.0,
                );
                this.slider(ui, "Color balance", balance, 0.0, 100.0, "%", 1.0, |s, v| s.ambient_balance = v / 100.0);
                this.slider(ui, "Light reach", reach, 25.0, 200.0, "%", 1.0, |s, v| s.ambient_height = v / 100.0);
                this.slider(ui, "Ambient softness", soft, 0.0, 100.0, "%", 1.0, |s, v| s.ambient_softness = v / 100.0);
                this.muted(
                    ui,
                    "Single color uses the cover's dominant hue. Cover blend mixes two album colors. Custom blend uses both chosen colors. Spread and height can extend light across the whole panel; closed-dock light has its own switch. Idle and tucked docks always stay black.",
                    11.0,
                );
                ui.gap(6.0);
                this.slider(ui, "Ambient spread", spread, 25.0, 250.0, "%", 1.0, |s, v| s.ambient_spread = v / 100.0);
            });
            let response = this.s.ambient_audio_response.clone();
            this.choice_same(
                ui,
                "Ambient audio response",
                "React to music",
                &["Off", "Soft", "Responsive", "Punchy"],
                &response,
                |s, v| s.ambient_audio_response = v.to_owned(),
            );
            let sens = this.s.ambient_sensitivity * 100.0;
            this.slider(ui, "Ambient sensitivity", sens, 50.0, 300.0, "%", 5.0, |s, v| s.ambient_sensitivity = v / 100.0);
            this.muted(
                ui,
                "Light follows the selected app's measured volume. Soft fades gently; Responsive follows transients; Punchy reacts faster. Sensitivity helps quiet tracks. Your microphone is never used.",
                11.0,
            );
            ui.gap(6.0);
            let motion = this.s.ambient_motion.clone();
            this.choice_same(
                ui,
                "Ambient movement",
                "Light movement",
                &["Still", "Breathe", "Drift"],
                &motion,
                |s, v| s.ambient_motion = v.to_owned(),
            );
            this.muted(
                ui,
                "Still keeps the glow in place. Breathe and Drift add gentle motion during playback. Audio response works with all three. Motion stops when paused, hidden, tucked, or Reduce motion is on; Live activity also controls audio response.",
                11.0,
            );
        });
    }

    /// A switch whose label is already final (the per-widget ambient list).
    pub(crate) fn check_label(
        &mut self,
        ui: &mut Ui,
        label: &str,
        value: bool,
        set: impl FnOnce(&mut naby_core::settings::Settings, bool),
    ) -> bool {
        self.check(ui, label, value, set)
    }

    pub(super) fn notification_presentation_settings<'a>(&mut self, ui: &mut Ui<'a>) {
        self.section(ui, "Notification presentation", |this, ui| {
            let anim = this.s.notification_animation.clone();
            this.choice(
                ui,
                "Notification animation",
                "Notification animation",
                &[("Lift", "Lift"), ("Fade", "Fade"), ("Soft scale", "Soft scale")],
                &anim,
                |s, v| s.notification_animation = v.to_owned(),
            );
            let secs = this.s.notification_seconds;
            this.slider(ui, "Notification lifetime", secs, 4.0, 30.0, " s", 1.0, |s, v| s.notification_seconds = v);
            this.muted(
                ui,
                "Compact indicators expire after this time. Open details and active interaction keep the current notification available.",
                11.0,
            );
            if this.button(ui, "Preview notch notification") {
                this.out.push(Command::Notify {
                    kind: NoticeKind::Failure,
                    title: this.tr("A little update, right here").to_owned(),
                    detail: this
                        .tr("This is a preview. Your files and running sessions stay as they are.")
                        .to_owned(),
                    files: None,
                });
                this.out.push(Command::NavigatePage("notifications".to_owned()));
            }
            let quiet = this.s.quiet_hours;
            this.check(ui, "Scheduled quiet hours", quiet, |s, v| s.quiet_hours = v);
            let hours: Vec<String> = (0..24).map(|h| format!("{h:02}:00")).collect();
            let options: Vec<(&str, &str)> = hours.iter().map(|h| (h.as_str(), h.as_str())).collect();
            this.muted(ui, "From · local time", 11.0);
            let start = hours[this.s.quiet_start.clamp(0, 23) as usize].clone();
            this.combo_plain(ui, "Quiet hours start", &options, &start, |s, v| {
                s.quiet_start = v[..2].parse().unwrap_or(0);
            });
            this.muted(ui, "Until", 11.0);
            let end = hours[this.s.quiet_end.clamp(0, 23) as usize].clone();
            this.combo_plain(ui, "Quiet hours end", &options, &end, |s, v| {
                s.quiet_end = v[..2].parse().unwrap_or(0);
            });
            this.muted(
                ui,
                "Quiet hours silence Naby notifications and completion sounds. A matching start and end disables the schedule. Windows notifications from other apps are unaffected.",
                11.0,
            );
            ui.gap(6.0);
            let snoozed = match this.s.notifications_snoozed_until {
                Some(until) if until.with_timezone(&Utc) > Utc::now() => format!(
                    "{} {}",
                    this.tr("Snoozed until"),
                    until.with_timezone(&Local).format("%H:%M")
                ),
                _ => this.tr("Notifications are not snoozed.").to_owned(),
            };
            this.muted_final(ui, &snoozed, 11.0);
            let labels = [
                ("Snooze 15 min", true),
                ("Snooze 30 min", true),
                ("Snooze 60 min", true),
                ("Resume notifications", true),
            ];
            if let Some(i) = this.wrap_buttons(ui, &labels) {
                if i < 3 {
                    let minutes = [15, 30, 60][i];
                    let until = Utc::now() + chrono::Duration::minutes(minutes);
                    this.s.notifications_snoozed_until = Some(until.fixed_offset());
                    this.out.push(Command::NoticeClearAll);
                } else {
                    this.s.notifications_snoozed_until = None;
                }
                this.save();
            }
            ui.gap(8.0);
            this.muted(
                ui,
                "Snooze ends automatically. It does not stop Timer or Focus. Resume removes the snooze; any scheduled quiet hours still apply.",
                11.0,
            );
        });
    }
}
