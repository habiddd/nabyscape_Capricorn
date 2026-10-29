//! "Your setup": the readiness rows on the Start page (C# `SettingsFeatureStatus.cs`). The
//! state lines come from `naby_core::readiness`; this file lays them out and routes each row's
//! button.

use super::{AiTarget, SettingsView};
use naby_core::command::Command;
use naby_core::readiness::{self, FeatureStatus, LyricsInput, MusicInput};
use naby_render::{Rect, Ui, Weight};

/// What a row's button does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RowAction {
    Music,
    Ai,
    Voice,
    Lyrics,
    Weather,
}

impl SettingsView {
    /// The five rows, computed from the preferences and the live snapshots.
    pub(crate) fn feature_rows(&self) -> [(&'static str, FeatureStatus); 5] {
        let (media_available, lyrics_status, has_lines, cached) = match &self.snaps {
            Some(s) => (
                s.media.available,
                s.lyrics.status.clone(),
                !s.lyrics.lines.is_empty(),
                s.weather.snapshot.clone(),
            ),
            None => (false, String::new(), false, None),
        };
        let music = readiness::music(&MusicInput::from_settings(&self.s, media_available));
        let ai = readiness::ai_chat(&self.host.ai);
        let voice = readiness::voice(&self.host.voice);
        let lyrics = readiness::lyrics(&LyricsInput::from_settings(
            &self.s,
            media_available,
            self.host.lyrics_can_time,
            has_lines,
            &lyrics_status,
        ));
        let weather = readiness::weather(&self.s, cached.as_ref());
        [
            ("Music", music),
            ("AI chat", ai),
            ("Voice", voice),
            ("Lyrics", lyrics),
            ("Weather", weather),
        ]
    }

    /// `HomeFeatureStatuses`: the "Your setup" card and the quick-setup button.
    pub(super) fn home_feature_statuses<'a>(&mut self, ui: &mut Ui<'a>) {
        let rows = self.feature_rows();
        let actions = [
            RowAction::Music,
            RowAction::Ai,
            RowAction::Voice,
            RowAction::Lyrics,
            RowAction::Weather,
        ];
        let mut action: Option<RowAction> = None;
        self.section(ui, "Your setup", |this, ui| {
            let th = ui.theme().clone();
            for (i, ((title, status), what)) in rows.iter().zip(actions).enumerate() {
                let state_style = ui.text_style(14.0, Weight::Semibold, th.text);
                let detail_style = ui.text_style(12.0, Weight::Regular, th.text_dim);
                let avail = ui.available_width();
                let btn_w = 116.0_f32.max(
                    ui.fonts().measure_width(
                        this.tr(status.action),
                        &ui.text_style(13.0, Weight::Regular, th.text),
                    ) + 28.0,
                );
                let text_w = (avail - btn_w - 16.0).max(60.0);
                let heading = format!("{} · {}", this.tr(title), this.tr(status.state));
                let detail = this.tr(status.detail).to_owned();
                let sl = ui.line_height(&state_style);
                let dl = ui.line_height(&detail_style);
                let state_lines = ui.fonts().wrap(&heading, &state_style, text_w);
                let detail_lines = ui.fonts().wrap(&detail, &detail_style, text_w);
                let h = (state_lines.len() as f32 * sl + 5.0 + detail_lines.len() as f32 * dl)
                    .max(34.0);
                let rect = ui.allocate(None, h);
                let mut y = rect.y;
                for line in &state_lines {
                    ui.draw_text(rect.x, y, line, &state_style, None);
                    y += sl;
                }
                y += 5.0;
                for line in &detail_lines {
                    ui.draw_text(rect.x, y, line, &detail_style, None);
                    y += dl;
                }
                let button =
                    Rect::new(rect.right() - btn_w, rect.y + (h - 34.0) / 2.0, btn_w, 34.0);
                let label = this.tr(status.action).to_owned();
                ui.push_id(i, |ui| {
                    ui.child(button, |ui| {
                        let resp = ui.button(&label);
                        this.hand |= resp.hovered;
                        this.record(&format!("setup:{title}"), resp.rect);
                        if resp.clicked {
                            action = Some(what);
                        }
                    });
                });
                ui.gap(16.0);
            }
        });
        // "Run quick setup again" sits right under the card.
        if self.button(ui, "Run quick setup again") {
            self.out.push(Command::Settings(
                naby_core::command::SettingsRequest::ShowWelcome,
            ));
        }
        ui.gap(10.0);
        if let Some(a) = action {
            self.run_row_action(a);
        }
    }

    fn run_row_action(&mut self, action: RowAction) {
        match action {
            RowAction::Music => {
                if !self.s.media_integration_enabled {
                    self.edit(|s| s.media_integration_enabled = true);
                    self.flush();
                } else if self.s.widget_enabled("media") {
                    self.navigate("Audio");
                } else {
                    self.navigate("Content");
                }
            }
            RowAction::Ai => {
                let ai = self.host.ai;
                if ai.enabled && ai.chat_verified {
                    self.out.push(Command::OpenAssistant);
                } else if !ai.enabled {
                    self.open_ai(AiTarget::Overview);
                } else {
                    self.open_ai(AiTarget::Chat);
                }
            }
            RowAction::Voice => {
                let ai = self.host.ai;
                let voice = self.host.voice;
                if !ai.enabled {
                    self.open_ai(AiTarget::Overview);
                } else if voice.voice_configured && !voice.voice_connected && !voice.speech_busy {
                    if !voice.local_voice_enabled {
                        self.out.push(Command::OpenVoice);
                    } else {
                        self.open_ai(AiTarget::VoiceChecks);
                    }
                } else {
                    self.open_ai(AiTarget::Voice);
                }
            }
            RowAction::Lyrics => self.navigate("Audio"),
            RowAction::Weather => self.navigate("Weather"),
        }
    }
}
