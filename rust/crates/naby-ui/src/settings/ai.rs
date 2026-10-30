//! The "Naby AI" category hook.
//!
//! The category is filled by an [`AiSettingsPage`] installed with `SettingsView::set_ai_page`.
//! [`AssistantAiPage`] is the real one: the assistant's setup tabs (Get started, Chat, Voice,
//! Hermes, More) with the provider connection editor and, under Voice, the Voice setup / Wake
//! word / Models / Check audio pages. [`DefaultAiPage`] shows a short explanation and buttons
//! that open the assistant window or the voice room until the host installs the real page.
//!
//! Entry points that jump into the page (`OpenAiSettings`, `OpenAiHermesSettings`,
//! `OpenAiVoiceSettings`, the Start page's "Open chat"/"Install models" buttons) call
//! [`AiSettingsPage::open`] with an [`AiTarget`].

use super::HostInfo;
use crate::assistant::service::AssistantService;
use crate::assistant::setup::{SetupOut, SetupTab, SetupView};
use crate::assistant::voice::{VoiceHost, VoicePage};
use naby_core::command::Command;
use naby_core::settings::Settings;
use naby_render::{Ui, Weight};
use std::sync::Arc;

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

    /// A worker published something the page shows (check result, install progress); the host
    /// repaints the window. Cheap, called from the app's wake handler.
    fn dirty(&self) -> bool {
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

/// The real "Naby AI" page: the same [`SetupView`] the assistant window shows for "AI setup",
/// over the same [`AssistantService`] and voice runtime, so edits in either window are seen by
/// the other. Created when the settings window opens, dropped with it.
pub struct AssistantAiPage {
    service: AssistantService,
    setup: SetupView,
    seen: u64,
}

impl AssistantAiPage {
    pub fn new(service: AssistantService, voice: Arc<dyn VoiceHost>) -> Self {
        let mut setup = SetupView::new(service.clone());
        setup.set_voice_host(voice);
        Self {
            service,
            setup,
            seen: 0,
        }
    }

    /// The setup tabs (tests, hosts).
    pub fn setup(&mut self) -> &mut SetupView {
        &mut self.setup
    }
}

impl AiSettingsPage for AssistantAiPage {
    fn frame(&mut self, ui: &mut Ui<'_>, _cx: &AiContext<'_>, out: &mut Vec<Command>) {
        self.seen = self.service.revision();
        let mut inner = SetupOut::default();
        self.setup.show(ui, &mut inner);
        out.append(&mut inner.commands);
        if inner.open_chat {
            out.push(Command::OpenAssistant);
        }
    }

    fn open(&mut self, target: AiTarget) {
        match target {
            AiTarget::Overview => self.setup.open(SetupTab::Overview),
            AiTarget::Chat => self.setup.open(SetupTab::Chat),
            AiTarget::Hermes => self.setup.open(SetupTab::Hermes),
            AiTarget::Voice => self.setup.open_voice(VoicePage::Setup),
            AiTarget::VoiceChecks => self.setup.open_voice(VoicePage::Audio),
        }
    }

    fn has_unsaved_changes(&self) -> bool {
        self.setup.has_provider_draft()
    }

    fn dirty(&self) -> bool {
        self.seen != self.service.revision() || self.setup.voice_dirty() || self.setup.busy()
    }

    fn on_close(&mut self) {
        self.setup.on_hide();
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
