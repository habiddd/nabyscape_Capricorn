//! "Feature usage": switch whole features off to save resources
//! (C# `SettingsFeatureResources.cs`).

use super::nav::Reveal;
use super::search::SearchItem;
use super::SettingsView;
use naby_render::{Ui, Weight};

/// The widgets "Use essentials" keeps.
const ESSENTIALS: [&str; 4] = ["media", "clock", "timer", "focus"];

impl SettingsView {
    pub(super) fn feature_resource_settings<'a>(&mut self, ui: &mut Ui<'a>) {
        self.section(ui, "Feature usage", |this, ui| {
            let enabled = this.s.widgets.iter().filter(|w| w.enabled).count();
            let summary = format!("{} {}", enabled, this.tr("widgets enabled · choose only what you use"));
            this.text_final(ui, &summary, 14.0, Weight::Regular);
            ui.gap(2.0);
            match this.wrap_buttons(ui, &[("Use essentials", true), ("Customize", true)]) {
                Some(0) => {
                    for w in &mut this.s.widgets {
                        w.enabled = ESSENTIALS.contains(&w.id.as_str());
                    }
                    for v in &mut this.s.hover_views {
                        v.enabled = ESSENTIALS.contains(&v.id.as_str());
                    }
                    this.s.hover_follows_widgets = true;
                    this.save();
                    this.navigate("Content");
                    this.set_feedback(
                        "Essentials selected · music, clock, timer and focus. Saved content and running sessions are preserved.",
                        false,
                    );
                }
                Some(_) => {
                    this.reveal = Some(Reveal {
                        item: SearchItem {
                            title: "Your widgets".to_owned(),
                            category: "Content",
                            section: "Enabled widgets & order",
                            control: "",
                            keywords: String::new(),
                        },
                        tries: 0,
                    });
                    this.repaint = true;
                }
                None => {}
            }
            ui.gap(8.0);
            this.muted(
                ui,
                "Essentials shows music, clock, timer and focus. Other widgets stay off until you enable them. Animation quality stays the same.",
                12.0,
            );
            let integration = this.s.media_integration_enabled;
            this.check(ui, "Connect to music apps", integration, |s, v| s.media_integration_enabled = v);
            this.muted(
                ui,
                "Off disconnects Windows media sessions and releases track artwork. It leaves playback in your music app untouched.",
                12.0,
            );
            let downloads = this.s.downloads_enabled;
            this.check(ui, "Watch for completed downloads", downloads, |s, v| s.downloads_enabled = v);
            match this.wrap_buttons(ui, &[("Clipboard capture", true), ("Voice & wake word", true)]) {
                Some(0) => this.navigate("Clipboard"),
                Some(_) => this.navigate("AI"),
                None => {}
            }
            ui.gap(8.0);
            this.muted(
                ui,
                "Clipboard capture and voice listening have separate switches. Turning a widget off preserves your notes, files and timers. Hidden weather and system pages stop their work; audio capture is released when no visual needs it.",
                12.0,
            );
        });
    }
}
