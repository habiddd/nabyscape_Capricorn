//! Provider settings (`AssistantProviderSettings`): list, add, edit and remove chat providers,
//! load model IDs, check the connection and image support, with the unsaved-draft guard.
//!
//! [`ProvidersView`] is self-contained and meant to be embedded in any [`Ui`] column (the
//! assistant's setup pages, the Settings window's AI category): construct it once with an
//! [`AssistantService`] handle and call [`ProvidersView::show`] every frame inside the caller's
//! scroll area. It never displays or reads back an API key: the key field only holds what the
//! user is typing, and is cleared on every provider switch and after saving.

use naby_core::assistant::persistence::{
    apply_draft, guard_switch, has_provider_draft, image_replies_supported, DraftAction,
    DraftChoice, DraftIdentity,
};
use naby_core::assistant::providers::PRESETS;
use naby_core::assistant::voice::{default_realtime_url, REALTIME_PROTOCOLS};
use naby_core::assistant::{Protocol, Provider};
use naby_core::command::Command;
use naby_render::{TextEditOpts, Ui};

use super::service::{AssistantService, KeyChoice, OpId, OpKind, OpResult};

/// What a frame of the view asks the host to do.
#[derive(Debug, Default)]
pub struct ProvidersOut {
    pub commands: Vec<Command>,
    /// The user pressed "Open chat".
    pub open_chat: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pending {
    /// Edit another saved provider (index into the saved list).
    Edit(usize),
    /// Use another provider for chat.
    UseForChat(usize),
    Add,
    Remove,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Feedback {
    Info,
    Error,
    Success,
}

#[derive(Debug, Clone, Default)]
struct Draft {
    name: String,
    base_url: String,
    model: String,
    protocol: usize,
    vision: bool,
    image_output: bool,
    realtime_url: String,
    realtime_model: String,
    realtime_protocol: usize,
    voice: String,
    key: String,
}

fn protocol_index(value: &str) -> usize {
    Protocol::ALL
        .iter()
        .position(|p| p.as_str() == value)
        .unwrap_or(0)
}

fn realtime_index(value: &str) -> usize {
    REALTIME_PROTOCOLS
        .iter()
        .position(|p| *p == value)
        .unwrap_or(0)
}

impl Draft {
    fn of(provider: &Provider) -> Self {
        Self {
            name: provider.name.clone(),
            base_url: provider.base_url.clone(),
            model: provider.model.clone(),
            protocol: protocol_index(&provider.protocol),
            vision: provider.vision,
            image_output: provider.image_output,
            realtime_url: provider.realtime_url.clone(),
            realtime_model: provider.realtime_model.clone(),
            realtime_protocol: realtime_index(&provider.realtime_protocol),
            voice: provider.voice.clone(),
            key: String::new(),
        }
    }

    fn identity(&self) -> DraftIdentity {
        DraftIdentity {
            name: self.name.clone(),
            base_url: self.base_url.clone(),
            model: self.model.clone(),
            protocol: Protocol::ALL[self.protocol.min(Protocol::ALL.len() - 1)]
                .as_str()
                .to_owned(),
            vision: self.vision,
            image_output: self.image_output,
            realtime_url: self.realtime_url.clone(),
            realtime_model: self.realtime_model.clone(),
            realtime_protocol: REALTIME_PROTOCOLS[self.realtime_protocol.min(1)].to_owned(),
            voice: self.voice.clone(),
        }
    }
}

pub struct ProvidersView {
    service: AssistantService,
    /// Id of the provider the editor shows.
    edited: Option<String>,
    draft: Draft,
    preset: usize,
    models: Vec<String>,
    model_pick: usize,
    op: Option<(OpId, OpKind, Provider, bool)>,
    pending: Option<Pending>,
    feedback: (String, Feedback),
    key_note: String,
    image_note: String,
    manage_open: bool,
    editor_open: bool,
    advanced_open: bool,
    images_open: bool,
    voice_open: bool,
    remove_open: bool,
    pending_after_models: bool,
    /// Signature of the provider list the editor was loaded from.
    revision: u64,
}

impl ProvidersView {
    pub fn new(service: AssistantService) -> Self {
        let mut view = Self {
            service,
            edited: None,
            draft: Draft::default(),
            preset: 0,
            models: Vec::new(),
            model_pick: 0,
            op: None,
            pending: None,
            feedback: (String::new(), Feedback::Info),
            key_note: String::new(),
            image_note:
                "Image support: not tested. Turning Image input on does not verify model support."
                    .to_owned(),
            manage_open: false,
            editor_open: false,
            advanced_open: false,
            images_open: false,
            voice_open: false,
            remove_open: false,
            pending_after_models: false,
            revision: 0,
        };
        view.reload(None);
        view
    }

    pub fn service(&self) -> &AssistantService {
        &self.service
    }

    // ---- state ----------------------------------------------------------------------------

    fn saved(&self) -> Option<Provider> {
        let id = self.edited.as_deref()?;
        self.service
            .options()
            .providers
            .into_iter()
            .find(|p| p.id == id)
    }

    /// Reloads the editor from the saved provider (`Reload`/`LoadSelected`). The key field is
    /// always cleared.
    pub fn reload(&mut self, selected: Option<&str>) {
        let options = self.service.options();
        let id = selected
            .map(str::to_owned)
            .filter(|id| options.providers.iter().any(|p| &p.id == id))
            .or_else(|| options.provider().map(|p| p.id.clone()))
            .or_else(|| options.providers.first().map(|p| p.id.clone()));
        self.edited = id;
        self.models.clear();
        self.model_pick = 0;
        self.image_note = "Image support: not tested for this selected connection.".to_owned();
        match self.saved() {
            Some(saved) => {
                self.draft = Draft::of(&saved);
                self.key_note = match self.service.key_exists(&saved.id) {
                    Ok(true) => {
                        "A key is saved for this Windows user. Leave blank to keep it.".into()
                    }
                    Ok(false) => "No key saved.".into(),
                    Err(_) => "Windows credential storage is unavailable.".into(),
                };
                self.editor_open = saved.base_url.is_empty()
                    || (saved.model.is_empty() && saved.realtime_model.is_empty());
                self.feedback = (String::new(), Feedback::Info);
            }
            None => {
                self.draft = Draft::default();
                self.key_note.clear();
                self.feedback = (
                    "Choose a preset and Add provider to get started.".to_owned(),
                    Feedback::Info,
                );
            }
        }
        self.revision = self.options_signature();
    }

    /// Opens every section of the editor (tests, screenshots, "Edit connection").
    pub fn open_all_sections(&mut self) {
        self.manage_open = true;
        self.editor_open = true;
        self.advanced_open = true;
        self.images_open = true;
        self.voice_open = true;
        self.remove_open = true;
    }

    pub fn edited_id(&self) -> Option<&str> {
        self.edited.as_deref()
    }

    /// The loaded model IDs.
    pub fn models(&self) -> &[String] {
        &self.models
    }

    pub fn feedback(&self) -> &str {
        &self.feedback.0
    }

    pub fn feedback_is_error(&self) -> bool {
        self.feedback.1 == Feedback::Error
    }

    pub fn image_note(&self) -> &str {
        &self.image_note
    }

    pub fn busy(&self) -> bool {
        self.op.is_some()
    }

    /// Unsaved connection changes (`HasProviderDraft`).
    pub fn has_draft(&self) -> bool {
        match self.saved() {
            Some(saved) => has_provider_draft(
                &saved,
                &self.draft.identity(),
                !self.draft.key.is_empty(),
                false,
            ),
            None => false,
        }
    }

    /// A guarded switch is waiting for the user's choice.
    pub fn confirm_pending(&self) -> bool {
        self.pending.is_some()
    }

    pub fn set_field(&mut self, field: &str, value: &str) {
        let d = &mut self.draft;
        match field {
            "name" => d.name = value.into(),
            "base_url" => d.base_url = value.into(),
            "model" => d.model = value.into(),
            "key" => d.key = value.into(),
            "realtime_url" => d.realtime_url = value.into(),
            "realtime_model" => d.realtime_model = value.into(),
            "voice" => d.voice = value.into(),
            _ => {}
        }
    }

    /// How many characters the key field holds (the key itself is never exposed).
    pub fn key_len(&self) -> usize {
        self.draft.key.len()
    }

    pub fn set_vision(&mut self, on: bool) {
        self.draft.vision = on;
    }

    fn say(&mut self, text: impl Into<String>, kind: Feedback) {
        self.feedback = (text.into(), kind);
    }

    // ---- guarded switching ----------------------------------------------------------------

    fn guard(&mut self, action: Pending) {
        if self.op.is_some() {
            return;
        }
        if self.has_draft() {
            self.pending = Some(action);
        } else {
            self.run(action);
        }
    }

    /// The user's answer to "Keep your connection changes?".
    pub fn confirm(&mut self, choice: DraftChoice) {
        let Some(action) = self.pending.take() else {
            return;
        };
        match guard_switch(true, Some(choice)) {
            DraftAction::Stay => {}
            DraftAction::Proceed => self.run(action),
            DraftAction::SaveThenProceed => {
                if self.save() {
                    self.run(action);
                }
            }
        }
    }

    fn run(&mut self, action: Pending) {
        match action {
            Pending::Edit(index) => {
                let id = self
                    .service
                    .options()
                    .providers
                    .get(index)
                    .map(|p| p.id.clone());
                self.reload(id.as_deref());
            }
            Pending::UseForChat(index) => {
                let id = self
                    .service
                    .options()
                    .providers
                    .get(index)
                    .map(|p| p.id.clone());
                if let Some(id) = id {
                    match self.service.select_provider(&id) {
                        Ok(()) => {
                            let name = self.service.provider().map(|p| p.name).unwrap_or_default();
                            self.reload(Some(&id));
                            self.say(
                                format!("Chat provider saved \u{b7} {name}."),
                                Feedback::Info,
                            );
                        }
                        Err(_) => self.say("Could not save the chat provider.", Feedback::Error),
                    }
                }
            }
            Pending::Add => self.add_now(),
            Pending::Remove => self.remove_now(),
        }
    }

    // ---- actions --------------------------------------------------------------------------

    /// Adds the preset at `index` of [`PRESETS`], after the draft guard.
    pub fn request_add(&mut self, preset: usize) {
        self.preset = preset.min(PRESETS.len() - 1);
        self.guard(Pending::Add);
    }

    fn add_now(&mut self) {
        if self.service.options().providers.len() >= naby_core::assistant::models::MAX_PROVIDERS {
            self.say(
                "Keep up to 12 providers. Remove one before adding another.",
                Feedback::Error,
            );
            return;
        }
        match self.service.add_provider(PRESETS[self.preset]) {
            Ok(id) => {
                self.reload(Some(&id));
                self.editor_open = true;
                self.say(
                    format!(
                        "{} added. Enter its address, model and key, then Save provider.",
                        PRESETS[self.preset]
                    ),
                    Feedback::Info,
                );
            }
            Err(error) => self.say(format!("Not saved \u{b7} {error}"), Feedback::Error),
        }
    }

    pub fn request_remove(&mut self) {
        self.guard(Pending::Remove);
    }

    fn remove_now(&mut self) {
        let Some(saved) = self.saved() else { return };
        match self.service.remove_provider(&saved.id) {
            Ok(()) => {
                self.reload(None);
                self.say(format!("{} removed.", saved.name), Feedback::Info);
            }
            Err(error) => self.say(format!("Not saved \u{b7} {error}"), Feedback::Error),
        }
    }

    pub fn request_edit(&mut self, index: usize) {
        if self.service.options().providers.get(index).map(|p| &p.id) != self.edited.as_ref() {
            self.guard(Pending::Edit(index));
        }
    }

    pub fn request_use_for_chat(&mut self, index: usize) {
        let options = self.service.options();
        if options.providers.get(index).map(|p| &p.id) != Some(&options.selected_provider) {
            self.guard(Pending::UseForChat(index));
        }
    }

    pub fn forget_key(&mut self) {
        let Some(saved) = self.saved() else { return };
        match self.service.forget_key(&saved.id) {
            Ok(()) => {
                self.draft.key.clear();
                self.key_note = "Key removed from Windows Credential Manager.".into();
            }
            Err(error) => self.say(format!("Not saved \u{b7} {error}"), Feedback::Error),
        }
    }

    /// Validates the editor over the saved provider (`ReadFields`).
    fn read_fields(&self, allow_empty_model: bool) -> Result<Provider, String> {
        let saved = self.saved().ok_or("Add a provider first.")?;
        apply_draft(&saved, &self.draft.identity(), allow_empty_model)
            .map_err(|e| e.message().to_owned())
    }

    /// Saves the editor (`SaveProvider`). Returns whether it was saved.
    pub fn save(&mut self) -> bool {
        if self.op.is_some() {
            self.say(
                "Finish or cancel the current check before saving.",
                Feedback::Error,
            );
            return false;
        }
        if self.draft.model.trim().is_empty() && self.draft.realtime_model.trim().is_empty() {
            self.load_models_for_choice();
            return false;
        }
        let candidate = match self.read_fields(false) {
            Ok(c) => c,
            Err(error) => {
                self.say(format!("Not saved \u{b7} {error}"), Feedback::Error);
                return false;
            }
        };
        let Some(original) = self.saved() else {
            return false;
        };
        match self
            .service
            .save_provider(&original.id, &candidate, self.draft.key.trim())
        {
            Ok(id) => {
                self.draft.key.clear();
                self.reload(Some(&id));
                let images = if candidate.is_hermes() {
                    "image replies managed in Hermes"
                } else if candidate.image_output {
                    "image replies On"
                } else {
                    "image replies Off"
                };
                self.say(
                    format!(
                        "Provider saved \u{b7} image input {} \u{b7} {images}. Test connection to check API access.",
                        if candidate.vision { "On" } else { "Off" }
                    ),
                    Feedback::Info,
                );
                true
            }
            Err(error) => {
                self.say(format!("Not saved \u{b7} {error}"), Feedback::Error);
                false
            }
        }
    }

    fn key_choice(&self, candidate: &Provider) -> KeyChoice {
        let saved = self.saved();
        self.service
            .key_for_draft(self.draft.key.trim(), saved.as_ref(), candidate)
    }

    fn start(
        &mut self,
        kind: OpKind,
        result: Result<OpId, String>,
        candidate: Provider,
        saved_connection: bool,
    ) {
        match result {
            Ok(id) => self.op = Some((id, kind, candidate, saved_connection)),
            Err(error) => self.say(error, Feedback::Error),
        }
    }

    /// Loads the advertised model IDs (`LoadModels`).
    pub fn load_models(&mut self) {
        if self.op.is_some() {
            return;
        }
        let candidate = match self.read_fields(true) {
            Ok(c) if c.base_url.is_empty() => {
                self.say("Enter the API base URL first.", Feedback::Error);
                return;
            }
            Ok(c) => c,
            Err(error) => {
                self.say(
                    format!("Could not load models. {error} You can paste the model ID manually."),
                    Feedback::Error,
                );
                return;
            }
        };
        let key = self.key_choice(&candidate);
        self.say(
            "Loading model IDs from the provider\u{2026}",
            Feedback::Info,
        );
        let started = self.service.begin_load_models(candidate.clone(), &key);
        self.start(OpKind::Models, started, candidate, false);
    }

    /// `EnsureModel` when saving or testing without a model.
    fn load_models_for_choice(&mut self) {
        self.pending_after_models = false;
        self.load_models();
        self.pending_after_models = self.op.is_some();
    }

    /// Checks API access and the chosen model (`TestConnection`).
    pub fn test_connection(&mut self) {
        if self.op.is_some() {
            return;
        }
        if self.draft.model.trim().is_empty() {
            self.load_models_for_choice();
            return;
        }
        let candidate = match self.read_fields(false) {
            Ok(c) => c,
            Err(error) => {
                self.say(format!("Connection failed \u{b7} {error}"), Feedback::Error);
                return;
            }
        };
        if candidate.model.is_empty() {
            self.say(
                "Choose a chat model before testing. Realtime voice has separate settings.",
                Feedback::Error,
            );
            return;
        }
        let saved_connection = self.saved().is_some_and(|s| {
            self.draft.key.is_empty() && s.connection_identity() == candidate.connection_identity()
        });
        let key = self.key_choice(&candidate);
        self.say(
            "Checking API access and the selected chat model\u{2026}",
            Feedback::Info,
        );
        let started = self.service.begin_check_connection(candidate.clone(), &key);
        self.start(OpKind::Connection, started, candidate, saved_connection);
    }

    /// Sends the generated color sample to the saved provider (`TestImageSupport`).
    pub fn test_image(&mut self) {
        if self.op.is_some() {
            return;
        }
        if self.has_draft() {
            self.say(
                "Save provider before testing its image support.",
                Feedback::Error,
            );
            return;
        }
        let Some(saved) = self.saved().filter(|s| s.vision) else {
            self.say(
                "Enable Image input and Save provider first.",
                Feedback::Error,
            );
            return;
        };
        self.image_note = "Checking image understanding\u{2026}".into();
        let started = self.service.begin_check_image(saved.clone());
        self.start(OpKind::Image, started, saved, true);
    }

    pub fn cancel_check(&mut self) {
        if self.op.take().is_some() {
            self.service.cancel();
            self.say("Check cancelled. No retry was sent.", Feedback::Info);
        }
    }

    /// Collects finished background checks. Call every frame (cheap).
    pub fn poll(&mut self) {
        if let Some((op, kind, candidate, saved_connection)) = self.op.clone() {
            let Some(result) = self.service.take_result(op) else {
                return self.sync_options();
            };
            self.op = None;
            match (kind, result) {
                (OpKind::Models, OpResult::Models(Ok(models))) => {
                    let typed = self.draft.model.clone();
                    self.model_pick = models.iter().position(|m| *m == typed).map_or(0, |i| i + 1);
                    let count = models.len();
                    self.models = models;
                    if std::mem::take(&mut self.pending_after_models) && typed.trim().is_empty() {
                        self.say(
                            "Models loaded. Choose a model from the list, then Save provider or Test connection.",
                            Feedback::Info,
                        );
                    } else {
                        self.say(
                            format!("{count} models loaded. Paste an ID or choose one; capabilities depend on the model."),
                            Feedback::Success,
                        );
                    }
                }
                (OpKind::Models, OpResult::Models(Err(error))) => {
                    self.pending_after_models = false;
                    self.say(
                        format!(
                            "Could not load models. {error} You can paste the model ID manually."
                        ),
                        Feedback::Error,
                    );
                }
                (OpKind::Connection, OpResult::Connection(Ok(()))) => {
                    if saved_connection {
                        self.service.record_check(
                            &candidate,
                            true,
                            &format!(
                                "{} \u{b7} {} returned a chat response.",
                                candidate.name, candidate.model
                            ),
                        );
                    }
                    self.say(
                        format!(
                            "Connected \u{b7} {} returned a chat response. {} Voice and tools were not tested.",
                            candidate.model,
                            if saved_connection {
                                "Saved connection checked."
                            } else {
                                "These are draft settings. Save the provider, then check its saved connection."
                            }
                        ),
                        Feedback::Success,
                    );
                }
                (OpKind::Connection, OpResult::Connection(Err(error))) => {
                    if saved_connection {
                        self.service.record_check(&candidate, false, &error);
                    }
                    self.say(format!("Connection failed \u{b7} {error}"), Feedback::Error);
                }
                (OpKind::Image, OpResult::Image(Ok(recognized))) => {
                    self.image_note = if recognized {
                        "Image support checked \u{b7} the model identified the red sample. Image replies and other formats were not tested.".into()
                    } else {
                        "Image support not confirmed \u{b7} the provider replied but did not identify the sample. Check the model's vision support.".into()
                    };
                    let note = self.image_note.clone();
                    self.say(
                        note,
                        if recognized {
                            Feedback::Success
                        } else {
                            Feedback::Error
                        },
                    );
                }
                (OpKind::Image, OpResult::Image(Err(error))) => {
                    self.image_note = format!("Image check failed \u{b7} {error}");
                    let note = self.image_note.clone();
                    self.say(note, Feedback::Error);
                }
                _ => {}
            }
        }
        self.sync_options();
    }

    /// A provider was added, removed or edited elsewhere (another window): follow it unless the
    /// editor holds unsaved changes.
    fn sync_options(&mut self) {
        if self.revision != self.options_signature() && self.op.is_none() && !self.has_draft() {
            let id = self.edited.clone();
            self.reload(id.as_deref());
        }
    }

    fn options_signature(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let options = self.service.options();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        format!("{:?}|{}", options.providers, options.selected_provider).hash(&mut hasher);
        hasher.finish()
    }

    // ---- painting -------------------------------------------------------------------------

    /// Draws the whole editor into the caller's column. Handles input through `ui`.
    pub fn show(&mut self, ui: &mut Ui<'_>, out: &mut ProvidersOut) {
        self.poll();
        let options = self.service.options();
        let names: Vec<String> = options.providers.iter().map(|p| p.name.clone()).collect();
        let name_refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let selected_chat = options
            .providers
            .iter()
            .position(|p| p.id == options.selected_provider)
            .unwrap_or(0);
        let edited_index = self
            .edited
            .as_ref()
            .and_then(|id| options.providers.iter().position(|p| &p.id == id))
            .unwrap_or(0);
        let busy = self.op.is_some();

        ui.heading_sized("Your chat connection", 20.0);
        ui.label_dim("Choose who answers your messages. Your connection stays saved on this PC.");

        if self.pending.is_some() {
            self.show_confirm(ui);
        }

        // ---- use for chat ---------------------------------------------------------------
        ui.enabled(
            !busy && !options.providers.is_empty() && self.pending.is_none(),
            |ui| {
                ui.section_label("Use for chat");
                let mut pick = selected_chat;
                if !name_refs.is_empty()
                    && ui.combo(&mut pick, &name_refs).changed
                    && pick != selected_chat
                {
                    self.request_use_for_chat(pick);
                }
            },
        );

        // ---- add or manage -------------------------------------------------------------
        if expander(ui, "Add or manage connections", &mut self.manage_open) {
            ui.label_dim("Add another provider, or choose which saved connection to edit.");
            ui.enabled(!busy && self.pending.is_none(), |ui| {
                ui.row(|ui| {
                    ui.spacing(8.0);
                    ui.width(240.0, |ui| {
                        ui.combo(&mut self.preset, &PRESETS);
                    });
                    if ui.button("Add provider").clicked {
                        self.guard(Pending::Add);
                    }
                });
                if !name_refs.is_empty() {
                    ui.section_label("Saved connection to edit");
                    let mut pick = edited_index;
                    if ui.combo(&mut pick, &name_refs).changed && pick != edited_index {
                        self.request_edit(pick);
                    }
                }
            });
        }

        // ---- status of the saved connection -----------------------------------------------
        if let Some(saved) = self.saved() {
            let setup = self.service.chat_setup_for(&saved);
            let (text, _) = if self.has_draft() {
                (
                    "Not saved yet \u{b7} choose Save provider to apply your connection changes. Test connection checks this draft only.".to_owned(),
                    false,
                )
            } else {
                (format!("{}\n{}", setup.title, setup.detail), setup.verified)
            };
            ui.label_dim(&text);
            ui.row(|ui| {
                ui.spacing(6.0);
                if ui.button("Edit connection").clicked {
                    self.editor_open = true;
                }
                ui.enabled(!busy, |ui| {
                    if ui.button("Check connection").clicked {
                        self.test_connection();
                    }
                });
                if ui.primary_button("Open chat").clicked {
                    out.open_chat = true;
                }
            });
        }

        // ---- the editor ----------------------------------------------------------------
        if self.saved().is_some() && expander(ui, "Edit connection details", &mut self.editor_open)
        {
            self.show_editor(ui, out, busy);
        }

        if !self.feedback.0.is_empty() {
            let text = self.feedback.0.clone();
            match self.feedback.1 {
                Feedback::Error => status_line(ui, &text, ui.theme().invalid),
                Feedback::Success => {
                    status_line(ui, &text, naby_core::color::Rgba::rgb(0xA9, 0xCD, 0xBD))
                }
                Feedback::Info => {
                    ui.label_dim(&text);
                }
            }
        }
        ui.label_dim("You choose when to use the microphone or share your screen. Hermes manages its own tools and permissions.");
    }

    fn show_confirm(&mut self, ui: &mut Ui<'_>) {
        ui.card("Keep your connection changes?", |ui| {
            ui.label_dim("You changed this provider's connection details but did not save them.");
            ui.row(|ui| {
                ui.spacing(6.0);
                if ui.primary_button("Save and continue").clicked {
                    self.confirm(DraftChoice::Save);
                }
                if ui.button("Discard").clicked {
                    self.confirm(DraftChoice::Discard);
                }
                if ui.button("Keep editing").clicked {
                    self.confirm(DraftChoice::KeepEditing);
                }
            });
        });
    }

    fn show_editor(&mut self, ui: &mut Ui<'_>, out: &mut ProvidersOut, busy: bool) {
        let _ = out;
        let hermes = self.saved().is_some_and(|s| s.is_hermes());
        ui.enabled(self.pending.is_none(), |ui| {
            ui.row(|ui| {
                ui.spacing(6.0);
                ui.enabled(!busy, |ui| {
                    if ui.primary_button("Save provider").clicked {
                        self.save();
                    }
                    if ui.button("Test connection").clicked {
                        self.test_connection();
                    }
                });
                if busy && ui.button("Cancel check").clicked {
                    self.cancel_check();
                }
            });
            ui.enabled(!busy, |ui| {
                if expander(ui, "Advanced \u{b7} API protocol", &mut self.advanced_open) {
                    field(ui, "Connection name", &mut self.draft.name, 60);
                    ui.section_label("Chat API protocol");
                    let labels: Vec<&str> = Protocol::ALL.iter().map(|p| p.as_str()).collect();
                    ui.combo(&mut self.draft.protocol, &labels);
                }
                field(ui, "API base URL", &mut self.draft.base_url, 2048);
                field(ui, "Model \u{b7} choose one or paste its name", &mut self.draft.model, 200);
            });
            ui.row(|ui| {
                ui.spacing(6.0);
                ui.enabled(!busy, |ui| {
                    if pill(ui, "Load model list", false) {
                        self.load_models();
                    }
                });
            });
            if !self.models.is_empty() {
                let mut labels: Vec<&str> = vec!["Choose a loaded model\u{2026}"];
                labels.extend(self.models.iter().map(String::as_str));
                let before = self.model_pick;
                ui.enabled(!busy, |ui| {
                    ui.combo(&mut self.model_pick, &labels);
                });
                if self.model_pick != before && self.model_pick > 0 {
                    self.draft.model = self.models[self.model_pick - 1].clone();
                }
            }
            ui.label_dim("Checking the connection sends one short message and may use provider credits. Your chat history, microphone and screen are not sent.");

            ui.enabled(!busy, |ui| {
                ui.section_label("API key");
                ui.text_edit_with(&mut self.draft.key, &TextEditOpts::password().hint("Paste a new key to replace the saved one").max_len(2000));
            });
            ui.label_dim(&self.key_note.clone());
            if hermes {
                ui.label_dim("Connect your own Hermes API server. Its tools, skills and memory stay in Hermes. Naby handles chat, voice and the sharing source you select. Use Connect Hermes for the setup guide.");
            }
            ui.label_dim("Your key is stored securely in Windows. Leave it blank to keep the saved key.");

            if expander(ui, "Images & files", &mut self.images_open) {
                self.show_images(ui, busy, hermes);
            }
            if expander(ui, "Advanced \u{b7} live voice connection", &mut self.voice_open) {
                self.show_voice(ui, busy);
            }
            if expander(ui, "Remove connection or key", &mut self.remove_open) {
                ui.row(|ui| {
                    ui.spacing(6.0);
                    ui.enabled(!busy, |ui| {
                        if ui.button("Forget key").clicked {
                            self.forget_key();
                        }
                        if ui.button("Remove provider + key").clicked {
                            self.request_remove();
                        }
                    });
                });
            }
        });
    }

    fn show_images(&mut self, ui: &mut Ui<'_>, busy: bool, hermes: bool) {
        let saved = self.saved();
        let anthropic = self.draft.protocol == protocol_index("Anthropic");
        let candidate_supported = !hermes && !anthropic;
        let label = format!(
            "Image input \u{b7} {}",
            if self.draft.vision { "On" } else { "Off" }
        );
        ui.enabled(!busy, |ui| {
            ui.checkbox(&mut self.draft.vision, &label);
        });
        let label = if hermes {
            "Image replies \u{b7} managed in Hermes".to_owned()
        } else if !candidate_supported {
            "Image replies \u{b7} unavailable for this protocol".to_owned()
        } else {
            format!(
                "Image replies \u{b7} {}",
                if self.draft.image_output { "On" } else { "Off" }
            )
        };
        let mut output = self.draft.image_output && candidate_supported;
        ui.enabled(!busy && candidate_supported, |ui| {
            ui.checkbox(&mut output, &label);
        });
        if candidate_supported {
            self.draft.image_output = output;
        }
        let pending = saved.as_ref().is_some_and(|s| {
            self.draft.vision != s.vision
                || (candidate_supported && self.draft.image_output != s.image_output)
        });
        let note = if pending {
            "Not saved yet \u{b7} choose Save provider to apply these image preferences.".to_owned()
        } else {
            format!(
                "Saved image input: {}. Your selected model must support images.",
                if saved.as_ref().is_some_and(|s| s.vision) {
                    "On"
                } else {
                    "Off"
                }
            )
        };
        ui.label_dim(&note);
        ui.label_dim("Enable image input if this model can understand attached images, PDF pages, video frames and shared windows. Agent tools are managed in Hermes; Naby does not execute local actions.");
        ui.label_dim("Enable image replies only for compatible image models: Responses image generation, Gemini image models or Chat Completions endpoints with image output. Hermes uses its own configured image tools. Image links returned in chat can always be displayed.");
        if let Some(saved) = &saved {
            let _ = image_replies_supported(saved);
        }
        ui.enabled(!busy, |ui| {
            if pill(ui, "Test image support", false) {
                self.test_image();
            }
        });
        let note = self.image_note.clone();
        ui.label_dim(&note);
        ui.label_dim("Sends a generated color sample to the saved provider. No personal image is sent. This may use provider credits; Hermes may run an agent task. Image replies are a separate capability.");
    }

    fn show_voice(&mut self, ui: &mut Ui<'_>, busy: bool) {
        ui.enabled(!busy, |ui| {
            ui.section_label("Voice protocol");
            let labels: Vec<&str> = REALTIME_PROTOCOLS.to_vec();
            ui.combo(&mut self.draft.realtime_protocol, &labels);
            field(
                ui,
                "Realtime WebSocket URL",
                &mut self.draft.realtime_url,
                2048,
            );
            if pill(ui, "Use protocol URL", false) {
                self.draft.realtime_url =
                    default_realtime_url(REALTIME_PROTOCOLS[self.draft.realtime_protocol.min(1)])
                        .to_owned();
            }
            field(ui, "Realtime model ID", &mut self.draft.realtime_model, 200);
            field(ui, "Provider voice name", &mut self.draft.voice, 60);
        });
        ui.label_dim("OpenAI Realtime and Gemini Live use different protocols. Model access and charges depend on your provider. No microphone or screen is opened by saving.");
    }
}

/// A collapsible section header; returns whether the body should be drawn.
pub(crate) fn expander(ui: &mut Ui<'_>, title: &str, open: &mut bool) -> bool {
    let label = format!("{} {title}", if *open { "\u{25BE}" } else { "\u{25B8}" });
    if ui.button(&label).clicked {
        *open = !*open;
    }
    *open
}

/// A button at its natural width (a bare `Ui::button` fills the column).
pub(crate) fn pill(ui: &mut Ui<'_>, text: &str, primary: bool) -> bool {
    ui.row(|ui| {
        if primary {
            ui.primary_button(text).clicked
        } else {
            ui.button(text).clicked
        }
    })
}

/// A labeled single-line text field.
pub(crate) fn field(ui: &mut Ui<'_>, label: &str, text: &mut String, max_len: usize) {
    ui.section_label(label);
    ui.text_edit_with(text, &TextEditOpts::single().max_len(max_len));
}

fn status_line(ui: &mut Ui<'_>, text: &str, color: naby_core::color::Rgba) {
    let style = ui.text_style(11.0, naby_render::Weight::Regular, color);
    ui.label_styled(text, &style);
}
