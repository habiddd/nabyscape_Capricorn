//! The "Naby AI" category hook.
//!
//! The assistant tasks (providers, voice setup, wake word, models, audio checks) fill the
//! category by implementing [`AiSettingsPage`] and installing it with
//! `SettingsView::set_ai_page`. Until then [`DefaultAiPage`] shows a short explanation and
//! buttons that open the assistant window or the voice room, so the category is never empty.
//!
//! Entry points that jump into the page (`OpenAiSettings`, `OpenAiHermesSettings`,
//! `OpenAiVoiceSettings`, the Start page's "Open chat"/"Install models" buttons) call
//! [`AiSettingsPage::open`] with an [`AiTarget`].

use super::HostInfo;
use naby_core::command::Command;
use naby_core::settings::Settings;
use naby_render::{Ui, Weight};

/// Where inside the AI settings a request wants to land.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiTarget {
    /// The setup overview (`OpenSetupOverview`).
    Overview,
    /// The chat provider fields (`OpenChatSetup`).
    Chat,
    /// The Hermes agent setup (`OpenHermesSetup`).
    Hermes,
    /// Voice and wake word setup (`OpenVoiceSetup`).
    Voice,
    /// The audio checks inside the voice setup (`OpenVoiceChecks`).
    VoiceChecks,
}

/// What the page may read while it draws.
pub struct AiContext<'a> {
    /// The interface language code (`en`, `es`, `id`).
    pub lang: &'a str,
    pub settings: &'a Settings,
    pub host: &'a HostInfo,
    /// The settings window time (seconds), for animation.
    pub now: f64,
}

impl AiContext<'_> {
    /// Translates an application caption.
    pub fn t<'k>(&self, key: &'k str) -> &'k str {
        naby_core::i18n::t(self.lang, key)
    }
}

/// The content of the "Naby AI" category. Immediate mode: `frame` is called once per paint
/// pass inside a scrolling card; widgets come from `ui`, side effects are `out` commands.
pub trait AiSettingsPage {
    /// Draws the page in the available width.
    fn frame(&mut self, ui: &mut Ui<'_>, cx: &AiContext<'_>, out: &mut Vec<Command>);

    /// A request to show a particular part of the page.
    fn open(&mut self, _target: AiTarget) {}

    /// Unsaved provider edits exist (`HasProviderDraft`); the window asks before closing.
    fn has_unsaved_changes(&self) -> bool {
        false
    }

    /// The settings window closed: clear secrets and caches.
    fn on_close(&mut self) {}
}

/// The stand-in page until the assistant settings are installed.
#[derive(Default)]
pub struct DefaultAiPage {
    target: Option<AiTarget>,
}

impl AiSettingsPage for DefaultAiPage {
    fn frame(&mut self, ui: &mut Ui<'_>, cx: &AiContext<'_>, out: &mut Vec<Command>) {
        let th = ui.theme().clone();
        let muted = ui.text_style(11.0, Weight::Regular, th.text_dim);
        let body = ui.text_style(12.0, Weight::Regular, th.text);
        ui.label_styled(
            cx.t("Connect an AI provider and manage companion preferences."),
            &body,
        );
        ui.gap(8.0);
        let note = match self.target {
            Some(AiTarget::Voice | AiTarget::VoiceChecks) => {
                "Voice and wake word setup opens from the voice room."
            }
            Some(AiTarget::Hermes) => "Hermes connections are configured in the assistant window.",
            _ => "Chat, voice and wake word settings are managed in the assistant window.",
        };
        ui.label_styled(cx.t(note), &muted);
        ui.gap(8.0);
        ui.row(|ui| {
            ui.spacing(8.0);
            if ui.button(cx.t("Open Naby AI")).clicked {
                out.push(Command::OpenAssistant);
            }
            if ui.button(cx.t("Open voice")).clicked {
                out.push(Command::OpenVoice);
            }
        });
    }

    fn open(&mut self, target: AiTarget) {
        self.target = Some(target);
    }
}

impl super::SettingsView {
    /// The "Naby AI" category (`AiSettings()`).
    pub(super) fn ai_settings<'a>(&mut self, ui: &mut Ui<'a>) {
        self.section(ui, "AI companion", |this, ui| {
            let cx = AiContext {
                lang: &this.lang,
                settings: &this.s,
                host: &this.host,
                now: this.now,
            };
            this.ai_page.frame(ui, &cx, &mut this.out);
        });
    }
}
