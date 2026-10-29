//! Setup, provider settings and window tests (scripted provider, PNG snapshots).

use std::sync::Arc;
use std::time::{Duration, Instant};

use naby_core::assistant::persistence::{DraftChoice, Keys};
use naby_core::assistant::providers::PRESETS;
use naby_core::assistant::transport::{HttpRequest, HttpResponse};
use naby_core::settings::{presets, Settings};
use naby_render::snapshot::{count_differing, save, test_fonts};
use naby_render::{Canvas, Fonts, InputState, Key, KeyEvent, Rect, Theme};
use serde_json::json;

use super::ports::{BasicPreparer, NoScreen, NoTools};
use super::providers::{ProvidersOut, ProvidersView};
use super::service::test_support::{fixture, Fixture};
use super::session::ChatSession;
use super::setup::{SetupTab, SetupView};
use super::style::Palette;
use super::tests::chat_reply;
use super::window::{AssistantWindowView, Mode, WindowCommand, WindowEnv, WindowOut};

fn models_reply() -> HttpResponse {
    HttpResponse::new(
        200,
        "application/json",
        json!({"data": [{"id": "gpt-small"}, {"id": "gpt-large"}]})
            .to_string()
            .into_bytes(),
    )
}

type Reply = naby_core::assistant::Result<HttpResponse>;

struct ProviderRig {
    f: Fixture,
    view: ProvidersView,
}

impl ProviderRig {
    fn new(handler: impl FnMut(&HttpRequest) -> Reply + Send + 'static) -> Self {
        let f = fixture(handler);
        let view = ProvidersView::new(f.service.clone());
        Self { f, view }
    }

    fn preset(name: &str) -> usize {
        PRESETS.iter().position(|p| *p == name).unwrap()
    }

    /// Adds and saves a Custom Chat Completions provider named `name`.
    fn saved(&mut self, name: &str) -> String {
        self.view
            .request_add(Self::preset("Custom Chat Completions"));
        self.view.set_field("name", name);
        self.view
            .set_field("base_url", "https://provider.example/v1/");
        self.view.set_field("model", "chat-1");
        self.view.set_field("key", "sk-secret-key");
        assert!(self.view.save(), "{}", self.view.feedback());
        self.view.edited_id().unwrap().to_owned()
    }

    fn wait_idle(&mut self) {
        let start = Instant::now();
        while self.view.busy() {
            self.view.poll();
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "check never finished"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

#[test]
fn providers_can_be_added_edited_selected_and_removed() {
    let mut rig = ProviderRig::new(|_| Ok(chat_reply("OK")));
    assert!(rig.view.feedback().contains("Add provider"));
    let first = rig.saved("Work");
    let second = rig.saved("Home");
    assert_ne!(first, second);
    let options = rig.f.service.options();
    assert_eq!(options.providers.len(), 2);
    assert_eq!(
        options.selected_provider, second,
        "saving selects the provider"
    );
    assert!(rig.view.feedback().starts_with("Provider saved"));

    // The saved key is stored, and never shown or read back by the view.
    assert_eq!(
        rig.f.keys.read(&second).unwrap().as_deref(),
        Some("sk-secret-key")
    );
    assert_eq!(rig.view.key_len(), 0);
    assert!(!rig.view.feedback().contains("sk-secret"));

    // Switch the chat provider and the edited one.
    rig.view.request_use_for_chat(0);
    assert_eq!(rig.f.service.options().selected_provider, first);
    rig.view.request_edit(1);
    assert_eq!(rig.view.edited_id(), Some(second.as_str()));

    rig.view.request_remove();
    assert_eq!(rig.f.service.options().providers.len(), 1);
    assert_eq!(
        rig.f.keys.read(&second).unwrap(),
        None,
        "the key is removed with the provider"
    );
    assert_eq!(rig.view.edited_id(), Some(first.as_str()));
    rig.view.request_remove();
    assert!(rig.f.service.options().providers.is_empty());
    assert_eq!(rig.view.edited_id(), None);
}

#[test]
fn unsaved_connection_changes_guard_provider_switches() {
    let mut rig = ProviderRig::new(|_| Ok(chat_reply("OK")));
    let first = rig.saved("Work");
    let second = rig.saved("Home");
    rig.view.request_edit(0);
    assert_eq!(rig.view.edited_id(), Some(first.as_str()));
    assert!(!rig.view.has_draft());

    // Editing makes a draft; switching asks first.
    rig.view.set_field("model", "chat-2");
    assert!(rig.view.has_draft());
    rig.view.request_edit(1);
    assert!(rig.view.confirm_pending());
    assert_eq!(rig.view.edited_id(), Some(first.as_str()));

    rig.view.confirm(DraftChoice::KeepEditing);
    assert!(!rig.view.confirm_pending());
    assert_eq!(rig.view.edited_id(), Some(first.as_str()));
    assert!(rig.view.has_draft());

    // Discard moves on and forgets the change.
    rig.view.request_edit(1);
    rig.view.confirm(DraftChoice::Discard);
    assert_eq!(rig.view.edited_id(), Some(second.as_str()));
    rig.view.request_edit(0);
    assert_eq!(
        rig.f.service.options().providers[0].model,
        "chat-1",
        "the discarded edit was not saved"
    );

    // Save and continue persists first, then switches.
    rig.view.set_field("model", "chat-3");
    assert_eq!(rig.f.service.options().selected_provider, second);
    rig.view.request_use_for_chat(0);
    assert!(rig.view.confirm_pending());
    rig.view.confirm(DraftChoice::Save);
    assert_eq!(rig.f.service.options().providers[0].model, "chat-3");
    assert_eq!(rig.f.service.options().selected_provider, first);

    // A new key alone is also a draft; adding and removing are guarded as well.
    rig.view.request_edit(0);
    rig.view.set_field("key", "another");
    assert!(rig.view.has_draft());
    rig.view.request_add(0);
    assert!(rig.view.confirm_pending());
    rig.view.confirm(DraftChoice::KeepEditing);
    rig.view.request_remove();
    assert!(rig.view.confirm_pending());
    assert_eq!(rig.f.service.options().providers.len(), 2);
}

#[test]
fn connection_check_results_are_shown_and_recorded() {
    let mut rig = ProviderRig::new(|_| Ok(chat_reply("OK")));
    rig.saved("Work");
    rig.view.test_connection();
    assert!(rig.view.busy());
    rig.wait_idle();
    assert!(
        rig.view
            .feedback()
            .starts_with("Connected \u{b7} chat-1 returned a chat response."),
        "{}",
        rig.view.feedback()
    );
    assert!(rig.view.feedback().contains("Saved connection checked."));
    assert!(!rig.view.feedback_is_error());
    assert!(rig.f.service.setup().chat.verified);
    // The saved key was used even though the editor's key field was empty.
    assert_eq!(
        rig.f
            .seen
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .header("authorization"),
        Some("Bearer sk-secret-key")
    );

    // A draft check does not mark the saved connection as verified.
    rig.view.set_field("model", "chat-9");
    rig.view.test_connection();
    rig.wait_idle();
    assert!(
        rig.view.feedback().contains("These are draft settings."),
        "{}",
        rig.view.feedback()
    );
}

#[test]
fn failed_checks_show_the_failure_and_cancel_drops_the_result() {
    let mut rig =
        ProviderRig::new(|_| Ok(HttpResponse::new(401, "application/json", b"{}".to_vec())));
    rig.saved("Work");
    rig.view.test_connection();
    rig.wait_idle();
    assert!(
        rig.view.feedback().starts_with("Connection failed \u{b7} "),
        "{}",
        rig.view.feedback()
    );
    assert!(rig.view.feedback_is_error());
    assert!(!rig.f.service.setup().chat.verified);

    let mut slow = ProviderRig::new(|_| {
        std::thread::sleep(Duration::from_millis(150));
        Ok(chat_reply("OK"))
    });
    slow.saved("Work");
    slow.view.test_connection();
    slow.view.cancel_check();
    assert!(!slow.view.busy());
    assert_eq!(slow.view.feedback(), "Check cancelled. No retry was sent.");
    std::thread::sleep(Duration::from_millis(300));
    slow.view.poll();
    assert_eq!(slow.view.feedback(), "Check cancelled. No retry was sent.");
}

#[test]
fn model_list_loads_and_a_pasted_model_id_wins() {
    let mut rig = ProviderRig::new(|_| Ok(models_reply()));
    rig.saved("Work");
    rig.view.set_field("model", "my-custom-model");
    rig.view.load_models();
    rig.wait_idle();
    assert_eq!(rig.view.models(), ["gpt-large", "gpt-small"]);
    assert!(rig.view.feedback().starts_with("2 models loaded."));
    // The pasted ID stays; the list does not overwrite it.
    assert!(rig.view.save());
    assert_eq!(rig.f.service.provider().unwrap().model, "my-custom-model");

    // Saving with no model loads the list first and asks the user to choose.
    rig.view.set_field("model", "");
    assert!(!rig.view.save());
    rig.wait_idle();
    assert!(
        rig.view
            .feedback()
            .starts_with("Models loaded. Choose a model"),
        "{}",
        rig.view.feedback()
    );
}

#[test]
fn image_support_check_needs_a_saved_vision_provider() {
    let mut rig = ProviderRig::new(|_| Ok(chat_reply("Red")));
    rig.saved("Work");
    rig.view.test_image();
    assert_eq!(
        rig.view.feedback(),
        "Enable Image input and Save provider first."
    );
    rig.view.set_vision(true);
    rig.view.test_image();
    assert_eq!(
        rig.view.feedback(),
        "Save provider before testing its image support."
    );
    assert!(rig.view.save());
    rig.view.test_image();
    rig.wait_idle();
    assert!(
        rig.view.image_note().starts_with("Image support checked"),
        "{}",
        rig.view.image_note()
    );
    let body = rig
        .f
        .seen
        .lock()
        .unwrap()
        .last()
        .unwrap()
        .json()
        .unwrap()
        .to_string();
    assert!(
        body.contains("data:image/png;base64,"),
        "the generated sample is what was sent"
    );
}

#[test]
fn hermes_connect_checks_the_server_then_saves_and_selects() {
    let f = fixture(|_| Ok(models_reply()));
    let mut setup = SetupView::new(f.service.clone());
    setup.open(SetupTab::Hermes);
    setup.set_field("address", "http://127.0.0.1:8642/v1/");
    setup.set_field("agent", "hermes-agent");
    setup.set_field("key", "hermes-key");
    setup.hermes_check();
    let start = Instant::now();
    while setup.busy() {
        setup.poll_hermes();
        assert!(start.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        setup
            .hermes_result()
            .starts_with("Server available \u{b7} 2 agent(s)."),
        "{}",
        setup.hermes_result()
    );
    setup.set_field("agent", "gpt-large");
    assert!(setup.hermes_connect(), "{}", setup.hermes_result());
    let options = f.service.options();
    let hermes = options.providers.iter().find(|p| p.is_hermes()).unwrap();
    assert_eq!(hermes.model, "gpt-large");
    assert_eq!(options.selected_provider, hermes.id);
    assert_eq!(
        f.keys.read(&hermes.id).unwrap().as_deref(),
        Some("hermes-key")
    );
}

struct WinRig {
    f: Fixture,
    view: AssistantWindowView,
    fonts: Fonts,
    canvas: Canvas,
    settings: Settings,
    theme: Theme,
    now: f64,
    scale: f32,
}

impl WinRig {
    fn new(
        configured: bool,
        handler: impl FnMut(&HttpRequest) -> Reply + Send + 'static,
    ) -> Option<WinRig> {
        let fonts = test_fonts()?;
        let f = fixture(handler);
        if configured {
            let id = f.service.add_provider("Custom Chat Completions").unwrap();
            let mut p = f.service.provider().unwrap();
            p.name = "My provider".into();
            p.base_url = "https://provider.example/v1/".into();
            p.model = "chat-1".into();
            f.service.save_provider(&id, &p, "sk-test").unwrap();
        }
        let session = Arc::new(ChatSession::new(
            f.service.clone(),
            Arc::new(NoTools),
            Arc::new(NoScreen),
            Arc::new(BasicPreparer),
        ));
        let view = AssistantWindowView::new(f.service.clone(), session);
        let settings = presets::recommended();
        let theme = Theme::from_settings(&settings);
        Some(WinRig {
            f,
            view,
            fonts,
            canvas: Canvas::new(720, 740),
            settings,
            theme,
            now: 1.0,
            scale: 1.0,
        })
    }

    fn frame(&mut self, mut input: InputState) -> WindowOut {
        self.now += 0.016;
        input.time = self.now;
        let env = WindowEnv {
            settings: &self.settings,
            theme: &self.theme,
            scale: self.scale,
            maximized: false,
            visible: true,
            active: true,
        };
        let mut out = WindowOut::default();
        self.view
            .frame(&mut self.canvas, &self.fonts, &input, &env, &mut out);
        out
    }

    fn settle(&mut self) {
        for _ in 0..4 {
            self.frame(InputState::default());
        }
    }
}

#[test]
fn setup_overview_and_providers_snapshots() {
    let Some(mut rig) = WinRig::new(true, |_| Ok(chat_reply("OK"))) else {
        return;
    };
    rig.settle();
    assert_eq!(rig.view.mode(), Mode::Chat);
    rig.view.open_setup(SetupTab::Overview);
    rig.settle();
    assert_eq!(rig.view.mode(), Mode::Setup);
    save("assistant-setup-overview", &rig.canvas);
    rig.view.open_setup(SetupTab::Chat);
    rig.settle();
    save("assistant-providers", &rig.canvas);
    rig.view.open_setup(SetupTab::Hermes);
    rig.settle();
    save("assistant-hermes", &rig.canvas);
    rig.view.open_setup(SetupTab::Advanced);
    rig.settle();
    save("assistant-advanced", &rig.canvas);
    rig.view.open_setup(SetupTab::Voice);
    rig.settle();
    // Escape goes back to the chat.
    rig.frame(InputState::key(KeyEvent::new(Key::Escape)));
    assert_eq!(rig.view.mode(), Mode::Chat);
}

#[test]
fn the_providers_editor_paints_with_a_draft_pending_and_a_loaded_list() {
    let Some(fonts) = test_fonts() else { return };
    let mut rig = ProviderRig::new(|_| Ok(models_reply()));
    rig.saved("Work");
    rig.saved("Home");
    rig.view.set_field("model", "typed-model");
    rig.view.load_models();
    rig.wait_idle();
    rig.view.request_edit(0);
    assert!(rig.view.confirm_pending());
    rig.view.open_all_sections();
    let theme = Theme::default();
    let mut canvas = Canvas::new(640, 1900);
    let mut mem = naby_render::UiMemory::new();
    let window = Palette::from_theme(&theme).window;
    for _ in 0..3 {
        canvas.clear(window);
        let input = InputState {
            time: 1.0,
            ..InputState::default()
        };
        let mut ui = naby_render::Ui::new(
            &mut canvas,
            &fonts,
            &theme,
            &input,
            &mut mem,
            Rect::new(16.0, 16.0, 608.0, 1868.0),
        );
        let mut out = ProvidersOut::default();
        rig.view.show(&mut ui, &mut out);
    }
    save("assistant-providers-draft", &canvas);
    assert!(count_differing(&canvas, Rect::new(0.0, 0.0, 640.0, 1900.0), window) > 5000);
}

#[test]
fn caption_buttons_ask_the_host_to_close_minimize_and_maximize() {
    let Some(mut rig) = WinRig::new(true, |_| Ok(chat_reply("OK"))) else {
        return;
    };
    rig.settle();
    let click = |rig: &mut WinRig, x: f32| {
        rig.frame(InputState::press(x, 19.0));
        rig.frame(InputState::release(x, 19.0))
    };
    assert_eq!(
        click(&mut rig, 720.0 - 20.0).window,
        vec![WindowCommand::Close]
    );
    assert_eq!(
        click(&mut rig, 720.0 - 60.0).window,
        vec![WindowCommand::ToggleMaximize]
    );
    assert_eq!(
        click(&mut rig, 720.0 - 100.0).window,
        vec![WindowCommand::Minimize]
    );
    save("assistant-window", &rig.canvas);
    let _ = &rig.f;
}
