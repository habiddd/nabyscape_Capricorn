//! The AI setup pages (`AssistantSetupOverview`, `AssistantProviderSettings` sections, the Hermes
//! setup): overview, chat connection, voice hints, Hermes and advanced options.
//!
//! [`SetupView`] bundles the [`ProvidersView`] with the other pages and is what the assistant
//! window shows for "AI setup"; the Settings window can embed the same view.

use naby_core::assistant::hermes;
use naby_core::assistant::models::MAX_INSTRUCTIONS;
use naby_core::assistant::readiness::{
    next_setup, overview_title, NextSetup, OVERVIEW_DESCRIPTION,
};
use naby_core::assistant::Provider;
use naby_core::command::Command;
use naby_render::{TextEditOpts, Ui};

use super::providers::{expander, field, pill, ProvidersOut, ProvidersView};
use super::service::{AssistantService, KeyChoice, OpId, OpResult};

/// The categories of the AI setup (`readiness::SETUP_SECTIONS`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SetupTab {
    Overview,
    Chat,
    Voice,
    Hermes,
    Advanced,
}

impl SetupTab {
    pub const ALL: [SetupTab; 5] = [
        SetupTab::Overview,
        SetupTab::Chat,
        SetupTab::Voice,
        SetupTab::Hermes,
        SetupTab::Advanced,
    ];

    /// The tab button text (`BuildSetupNavigation`).
    pub fn label(self) -> &'static str {
        match self {
            SetupTab::Overview => "Get started",
            SetupTab::Chat => "Chat",
            SetupTab::Voice => "Voice",
            SetupTab::Hermes => "Hermes",
            SetupTab::Advanced => "More",
        }
    }
}

/// What a frame asks the host to do.
#[derive(Debug, Default)]
pub struct SetupOut {
    pub commands: Vec<Command>,
    /// "Open chat" / "Continue setup" finished: show the chat page.
    pub open_chat: bool,
}

#[derive(Default)]
struct HermesPanel {
    seeded: bool,
    address: String,
    profile: String,
    key: String,
    agent: String,
    images: bool,
    agents: Vec<String>,
    pick: usize,
    op: Option<(OpId, Provider)>,
    result: String,
}

pub struct SetupView {
    service: AssistantService,
    providers: ProvidersView,
    tab: SetupTab,
    hermes: HermesPanel,
    instructions: String,
    instructions_seeded: bool,
    advanced_note: String,
    summary_open: bool,
    config_open: bool,
    executable: String,
}

impl SetupView {
    pub fn new(service: AssistantService) -> Self {
        let executable = std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        Self {
            providers: ProvidersView::new(service.clone()),
            service,
            tab: SetupTab::Overview,
            hermes: HermesPanel::default(),
            instructions: String::new(),
            instructions_seeded: false,
            advanced_note: String::new(),
            summary_open: false,
            config_open: false,
            executable,
        }
    }

    pub fn tab(&self) -> SetupTab {
        self.tab
    }

    pub fn open(&mut self, tab: SetupTab) {
        if self.tab != tab {
            self.providers.cancel_check();
        }
        self.tab = tab;
    }

    pub fn providers(&mut self) -> &mut ProvidersView {
        &mut self.providers
    }

    /// Where the MCP bridge command points (the running executable by default).
    pub fn set_executable(&mut self, path: &str) {
        self.executable = path.to_owned();
    }

    pub fn busy(&self) -> bool {
        self.providers.busy() || self.hermes.op.is_some()
    }

    /// Sets a Hermes form field (`address`, `profile`, `key`, `agent`).
    pub fn set_field(&mut self, field: &str, value: &str) {
        self.seed_hermes();
        match field {
            "address" => self.hermes.address = value.into(),
            "profile" => self.hermes.profile = value.into(),
            "key" => self.hermes.key = value.into(),
            "agent" => self.hermes.agent = value.into(),
            _ => {}
        }
    }

    pub fn hermes_result(&self) -> &str {
        &self.hermes.result
    }

    /// Collects a finished "Check server" (also done while painting).
    pub fn poll_hermes(&mut self) {
        self.hermes_poll();
    }

    /// Releases pending checks when the window closes.
    pub fn on_hide(&mut self) {
        self.providers.cancel_check();
        self.hermes.op = None;
        self.hermes.key.clear();
        self.service.cancel();
    }

    pub fn show(&mut self, ui: &mut Ui<'_>, out: &mut SetupOut) {
        ui.row(|ui| {
            ui.spacing(4.0);
            for tab in SetupTab::ALL {
                let clicked = if tab == self.tab {
                    ui.primary_button(tab.label()).clicked
                } else {
                    ui.button(tab.label()).clicked
                };
                if clicked {
                    self.open(tab);
                }
            }
        });
        ui.gap(8.0);
        match self.tab {
            SetupTab::Overview => self.show_overview(ui, out),
            SetupTab::Chat => {
                let mut inner = ProvidersOut::default();
                self.providers.show(ui, &mut inner);
                out.commands.append(&mut inner.commands);
                out.open_chat |= inner.open_chat;
            }
            SetupTab::Voice => self.show_voice(ui, out),
            SetupTab::Hermes => self.show_hermes(ui, out),
            SetupTab::Advanced => self.show_advanced(ui),
        }
    }

    // ---- overview -------------------------------------------------------------------------

    fn show_overview(&mut self, ui: &mut Ui<'_>, out: &mut SetupOut) {
        let options = self.service.options();
        let ready = self.service.setup();
        ui.heading_sized(overview_title(&ready), 21.0);
        ui.label_dim(OVERVIEW_DESCRIPTION);
        ui.gap(10.0);
        ui.heading_sized("Chat with Naby", 17.0);
        ui.label_dim("Ask questions, paste images and work with your files.");
        ui.label_dim(&format!("{}\n{}", ready.chat.title, ready.chat.detail));
        if pill(ui, "Set up chat", false) {
            self.open(SetupTab::Chat);
        }
        ui.gap(14.0);
        ui.heading_sized("Talk hands-free", 17.0);
        ui.label_dim("Choose a voice, check your microphone and set a wake word.");
        ui.label_dim(&ready.voice.title);
        if pill(ui, "Set up voice", false) {
            self.open(SetupTab::Voice);
        }
        ui.gap(14.0);
        ui.heading_sized("Use your Hermes agent", 17.0);
        ui.label_dim("Bring your agent's memory and tools into Naby.");
        if pill(ui, "Connect Hermes", false) {
            self.open(SetupTab::Hermes);
        }
        ui.gap(14.0);
        if expander(ui, "Your current setup", &mut self.summary_open) {
            let summary = self.service.setup_summary();
            ui.label_dim(&summary);
        }
        let next = next_setup(&options, &ready);
        if pill(ui, next.caption(), true) {
            match next {
                NextSetup::EnableCompanion => {
                    if let Err(error) = self.service.set_enabled(true) {
                        self.advanced_note = error;
                    }
                }
                NextSetup::SetUpChat => self.open(SetupTab::Chat),
                NextSetup::CheckChat => {
                    self.open(SetupTab::Chat);
                    self.providers.test_connection();
                }
                NextSetup::OpenChat => out.open_chat = true,
            }
        }
        if !self.advanced_note.is_empty() {
            let note = self.advanced_note.clone();
            ui.label_dim(&note);
        }
    }

    fn show_voice(&mut self, ui: &mut Ui<'_>, out: &mut SetupOut) {
        let ready = self.service.setup();
        ui.heading_sized("Talk hands-free", 20.0);
        ui.label_dim("Voice runs in its own room. The microphone starts only when you choose Start, and models load only while a conversation is active.");
        ui.label_dim(&format!("{}\n{}", ready.voice.title, ready.voice.detail));
        ui.row(|ui| {
            ui.spacing(6.0);
            if ui.primary_button("Open voice").clicked {
                out.commands.push(Command::OpenVoice);
            }
            if ui.button("Voice settings").clicked {
                out.commands.push(Command::OpenSettingsPage("ai".into()));
            }
        });
        ui.label_dim("You choose when to use the microphone or share your screen.");
    }

    // ---- hermes ---------------------------------------------------------------------------

    fn seed_hermes(&mut self) {
        if self.hermes.seeded {
            return;
        }
        self.hermes.seeded = true;
        let options = self.service.options();
        let saved = options.providers.iter().find(|p| p.is_hermes());
        self.hermes.address =
            saved.map_or(hermes::DEFAULT_ADDRESS.to_owned(), |p| p.base_url.clone());
        self.hermes.agent = saved.map_or("hermes-agent".to_owned(), |p| p.model.clone());
        self.hermes.images = saved.is_some_and(|p| p.vision);
        self.hermes.result =
            "Not connected yet. Check server reads metadata only; it does not start an agent task."
                .to_owned();
    }

    fn hermes_endpoint(&self) -> Result<String, String> {
        hermes::base_url(&self.hermes.address, &self.hermes.profile)
            .map_err(|e| e.message().to_owned())
    }

    /// Reads the server's agent list (`Check`). The key stays in the field until connected.
    pub fn hermes_check(&mut self) {
        if self.hermes.op.is_some() {
            return;
        }
        self.seed_hermes();
        let endpoint = match self.hermes_endpoint() {
            Ok(e) => e,
            Err(error) => {
                self.hermes.result = format!("Could not connect: {error}");
                return;
            }
        };
        let options = self.service.options();
        let existing = hermes::existing(&options.providers, &endpoint);
        let key = if !self.hermes.key.is_empty() {
            KeyChoice::Entered(self.hermes.key.clone())
        } else if let Some(old) = existing {
            KeyChoice::Saved(old.id.clone())
        } else {
            KeyChoice::None
        };
        if key == KeyChoice::None {
            self.hermes.result =
                "Could not connect: Enter the API_SERVER_KEY from this Hermes profile.".into();
            return;
        }
        let mut candidate = match naby_core::assistant::providers::create("Hermes Agent") {
            Ok(c) => c,
            Err(error) => {
                self.hermes.result = format!("Could not connect: {error}");
                return;
            }
        };
        candidate.base_url = endpoint;
        self.hermes.result = "Checking Hermes address, key and advertised agents\u{2026}".into();
        match self.service.begin_load_models(candidate.clone(), &key) {
            Ok(id) => self.hermes.op = Some((id, candidate)),
            Err(error) => self.hermes.result = format!("Could not connect: {error}"),
        }
    }

    fn hermes_poll(&mut self) {
        let Some((id, _)) = self.hermes.op.clone() else {
            return;
        };
        let Some(result) = self.service.take_result(id) else {
            return;
        };
        self.hermes.op = None;
        match result {
            OpResult::Models(Ok(models)) => {
                self.hermes.agent = hermes::choose_agent(&self.hermes.agent, &models);
                self.hermes.pick = models
                    .iter()
                    .position(|m| *m == self.hermes.agent)
                    .map_or(0, |i| i + 1);
                self.hermes.result = format!(
                    "Server available \u{b7} {} agent(s). Choose one, then Connect & use Hermes. No chat or tool task was started.",
                    models.len()
                );
                self.hermes.agents = models;
            }
            OpResult::Models(Err(error)) => {
                self.hermes.result = format!("Could not connect: {error}");
            }
            _ => {}
        }
    }

    /// Saves the connection and selects it (`Connect & use Hermes`). Returns success.
    pub fn hermes_connect(&mut self) -> bool {
        self.seed_hermes();
        if self.providers.has_draft() {
            self.hermes.result = "Save or discard your connection changes in Chat first.".into();
            return false;
        }
        let endpoint = match self.hermes_endpoint() {
            Ok(e) => e,
            Err(error) => {
                self.hermes.result = format!("Could not connect: {error}");
                return false;
            }
        };
        let mut candidate = match hermes::connection(
            &endpoint,
            &self.hermes.agent,
            &self.hermes.profile,
            self.hermes.images,
        ) {
            Ok(c) => c,
            Err(error) => {
                self.hermes.result = format!("Could not connect: {error}");
                return false;
            }
        };
        let options = self.service.options();
        let saved = match hermes::existing(&options.providers, &endpoint).cloned() {
            Some(old) => {
                candidate.id = old.id.clone();
                self.service
                    .save_provider(&old.id, &candidate, self.hermes.key.trim())
            }
            None => self
                .service
                .add_connected_provider(candidate, self.hermes.key.trim()),
        };
        match saved {
            Ok(id) => {
                self.hermes.key.clear();
                self.providers.reload(Some(&id));
                self.hermes.result =
                    "Hermes connected and selected for chat. Open chat to try it.".into();
                true
            }
            Err(error) => {
                self.hermes.result = format!("Could not connect: {error}");
                false
            }
        }
    }

    fn show_hermes(&mut self, ui: &mut Ui<'_>, out: &mut SetupOut) {
        self.seed_hermes();
        self.hermes_poll();
        let busy = self.hermes.op.is_some();
        ui.heading_sized("Your Hermes, inside Naby", 16.0);
        ui.label_dim("Naby handles chat, voice and the window/display you choose to share. Your Hermes agent keeps its own skills, memory, tools and permissions.");
        ui.section_label("1 \u{b7} Enable API server in Hermes");
        ui.label_dim("Open Hermes \u{2192} API server, enable it, and set an API_SERVER_KEY for your profile. Start or restart the gateway with hermes gateway. Your model/provider must already be configured in Hermes.");
        ui.row(|ui| {
            ui.spacing(6.0);
            if ui.button("Copy gateway setup template").clicked {
                out.commands
                    .push(Command::CopyText(hermes::GATEWAY_TEMPLATE.to_owned()));
            }
            if ui.button("Open Hermes API setup guide").clicked {
                out.commands
                    .push(Command::OpenUrl(hermes::SETUP_GUIDE_URL.to_owned()));
            }
        });
        ui.enabled(!busy, |ui| {
            field(ui, "2 \u{b7} Paste your Hermes API address", &mut self.hermes.address, 2048);
            ui.label_dim("Same PC: http://127.0.0.1:8642/v1/. Remote server: use its public HTTPS address, or forward its port to localhost. Naby does not install or start Hermes for you.");
            field(ui, "Profile \u{b7} optional", &mut self.hermes.profile, 64);
            ui.section_label("API_SERVER_KEY \u{b7} from Hermes, not the underlying LLM provider");
            ui.text_edit_with(&mut self.hermes.key, &TextEditOpts::password().max_len(2000));
            ui.label_dim("An existing saved key is kept only for the exact same Hermes address/profile. Keys stay in Windows Credential Manager; they are not put in chat or copied by the setup template.");
            field(ui, "3 \u{b7} Check the server and choose its advertised agent", &mut self.hermes.agent, 160);
        });
        if !self.hermes.agents.is_empty() {
            let mut labels: Vec<&str> = vec!["Choose an advertised agent\u{2026}"];
            labels.extend(self.hermes.agents.iter().map(String::as_str));
            let before = self.hermes.pick;
            ui.combo(&mut self.hermes.pick, &labels);
            if self.hermes.pick != before && self.hermes.pick > 0 {
                self.hermes.agent = self.hermes.agents[self.hermes.pick - 1].clone();
            }
        }
        ui.checkbox(
            &mut self.hermes.images,
            "Allow the window/display images I choose to share",
        );
        ui.label_dim("Image understanding depends on the model configured in Hermes. The agent/model list is not a list of all underlying LLMs; change the agent\u{2019}s LLM in Hermes.");
        let result = self.hermes.result.clone();
        ui.label_dim(&result);
        ui.row(|ui| {
            ui.spacing(6.0);
            ui.enabled(!busy, |ui| {
                if ui.button("Check server").clicked {
                    self.hermes_check();
                }
                if ui.primary_button("Connect & use Hermes").clicked {
                    if self.hermes.agents.is_empty() {
                        self.hermes_check();
                    } else {
                        self.hermes_connect();
                    }
                }
            });
        });
        ui.gap(8.0);
        if expander(
            ui,
            "Let Hermes open Naby widget pages (MCP)",
            &mut self.config_open,
        ) {
            ui.label_dim("Add this to your Hermes config so the agent can list and open widget pages. It only changes which page is visible; it never starts timers, recordings or computer actions. Start NabyScape on this Windows account first.");
            let exe = self.executable.clone();
            ui.row(|ui| {
                ui.spacing(6.0);
                if ui.button("Copy config.yaml entry").clicked {
                    out.commands.push(Command::CopyText(hermes::mcp_yaml(&exe)));
                }
                if ui.button("Copy MCP JSON").clicked {
                    out.commands.push(Command::CopyText(hermes::mcp_json(&exe)));
                }
                if ui.button("Copy for WSL").clicked {
                    match hermes::wsl_path(&exe) {
                        Ok(path) => out
                            .commands
                            .push(Command::CopyText(hermes::mcp_yaml(&path))),
                        Err(error) => self.hermes.result = error.message().to_owned(),
                    }
                }
            });
        }
    }

    // ---- advanced -------------------------------------------------------------------------

    fn show_advanced(&mut self, ui: &mut Ui<'_>) {
        let options = self.service.options();
        if !self.instructions_seeded {
            self.instructions = options.instructions.clone();
            self.instructions_seeded = true;
        }
        ui.heading_sized("More options", 20.0);
        let enable = if options.enabled {
            "AI companion on"
        } else {
            "Enable AI companion"
        };
        if pill(ui, enable, false) {
            self.advanced_note = match self.service.set_enabled(!options.enabled) {
                Ok(()) => format!(
                    "AI companion saved \u{b7} companion {}.",
                    if options.enabled { "Off" } else { "On" }
                ),
                Err(error) => format!("Not saved \u{b7} {error}"),
            };
        }
        let character = if options.island_character {
            "Idle character visible"
        } else {
            "Idle character hidden"
        };
        if pill(ui, character, false) {
            self.advanced_note = match self.service.set_island_character(!options.island_character)
            {
                Ok(()) => format!(
                    "Idle character saved \u{b7} {}.",
                    if options.island_character {
                        "Off"
                    } else {
                        "On"
                    }
                ),
                Err(_) => "Could not save the island character preference.".into(),
            };
        }
        let mut navigation = options.widget_navigation;
        if ui
            .toggle(&mut navigation, "Let Naby open widget pages")
            .changed
        {
            self.advanced_note = match self.service.set_widget_navigation(navigation) {
                Ok(()) => format!(
                    "Widget navigation saved \u{b7} {}.",
                    if navigation { "On" } else { "Off" }
                ),
                Err(error) => format!("Not saved \u{b7} {error}"),
            };
        }
        ui.label_dim("Naby can list and open enabled widget pages when you ask. It cannot change widget contents or control the computer.");
        ui.section_label("Assistant instructions");
        ui.text_edit_with(
            &mut self.instructions,
            &TextEditOpts::multiline(5).max_len(MAX_INSTRUCTIONS),
        );
        if pill(ui, "Save instructions", false) {
            self.advanced_note = match self.service.set_instructions(&self.instructions) {
                Ok(()) => "Assistant instructions saved.".into(),
                Err(error) => format!("Not saved \u{b7} {error}"),
            };
        }
        if !self.advanced_note.is_empty() {
            let note = self.advanced_note.clone();
            ui.label_dim(&note);
        }
    }
}
