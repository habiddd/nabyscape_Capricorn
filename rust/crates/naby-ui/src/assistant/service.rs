//! Assistant configuration shared by the chat window and the provider settings
//! (`AssistantRuntime` option handling plus `AssistantProviderSettings` operations).
//!
//! [`AssistantService`] is a cheap, cloneable, thread-safe handle. It owns the persisted
//! [`Options`], the in-memory connection check results and the background provider checks
//! (model list, connection check, image support check). Checks run on short-lived worker threads
//! with a cancellation flag; results are collected with [`AssistantService::take_results`] on the
//! UI thread after the [`Waker`] fired. Nothing runs while no check is active, and API keys are
//! only ever passed to a worker for the duration of one request.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use naby_core::assistant::persistence::{self, DraftIdentity, KeySource, Keys};
use naby_core::assistant::readiness::{
    assess_setup, capture_summary, setup_summary, Capture, ConnectionChecks, SetupReadiness,
    SetupState, SpeechService,
};
use naby_core::assistant::voice::default_realtime_url;
use naby_core::assistant::{providers, AssistantError, Options, Provider, Transport};
use naby_core::store::Store;

/// Called from any thread after state a view should pull changed.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// Builds the transport a worker uses (tests inject a scripted executor).
pub type TransportFactory = Arc<dyn Fn() -> Transport + Send + Sync>;

/// Everything the service needs from its host.
#[derive(Clone)]
pub struct ServiceConfig {
    pub store: Arc<Store>,
    pub keys: Arc<dyn Keys + Send + Sync>,
    pub transport: TransportFactory,
    pub waker: Waker,
}

impl ServiceConfig {
    /// Production transport over `ureq`.
    pub fn new(store: Arc<Store>, keys: Arc<dyn Keys + Send + Sync>, waker: Waker) -> Self {
        Self {
            store,
            keys,
            transport: Arc::new(Transport::ureq),
            waker,
        }
    }
}

/// Identifies one background provider check.
pub type OpId = u64;

/// What a finished background check found.
#[derive(Debug, Clone, PartialEq)]
pub enum OpResult {
    Models(Result<Vec<String>, String>),
    Connection(Result<(), String>),
    Image(Result<bool, String>),
}

/// What kind of check is running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpKind {
    Models,
    Connection,
    Image,
}

struct Running {
    kind: OpKind,
    cancel: Arc<AtomicBool>,
}

struct State {
    options: Options,
    checks: ConnectionChecks,
    ops: HashMap<OpId, Running>,
    results: Vec<(OpId, OpResult)>,
    next_op: OpId,
    status: String,
    capture: Capture,
    speech: SpeechService,
}

struct Inner {
    config: ServiceConfig,
    state: Mutex<State>,
    revision: AtomicU64,
}

/// Shared assistant configuration. Clone freely; all clones see the same state.
#[derive(Clone)]
pub struct AssistantService {
    inner: Arc<Inner>,
}

/// Which key a draft check may use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyChoice {
    /// A key typed into the editor (never stored by the service).
    Entered(String),
    /// The key saved for this provider id.
    Saved(String),
    None,
}

fn message(error: &AssistantError) -> String {
    error.message().to_owned()
}

impl AssistantService {
    pub fn new(config: ServiceConfig) -> Self {
        let options = config.store.load_assistant_options();
        Self {
            inner: Arc::new(Inner {
                config,
                state: Mutex::new(State {
                    options,
                    checks: ConnectionChecks::new(),
                    ops: HashMap::new(),
                    results: Vec::new(),
                    next_op: 1,
                    status: "Configure a provider to begin. Nothing is captured in the background."
                        .to_owned(),
                    capture: Capture::default(),
                    speech: SpeechService::default(),
                }),
                revision: AtomicU64::new(1),
            }),
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.inner.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn changed(&self) {
        self.inner.revision.fetch_add(1, Ordering::SeqCst);
        (self.inner.config.waker)();
    }

    /// Moves whenever options, checks or results changed.
    pub fn revision(&self) -> u64 {
        self.inner.revision.load(Ordering::SeqCst)
    }

    pub fn waker(&self) -> Waker {
        self.inner.config.waker.clone()
    }

    pub fn transport(&self) -> Transport {
        (self.inner.config.transport)()
    }

    pub fn keys(&self) -> Arc<dyn Keys + Send + Sync> {
        self.inner.config.keys.clone()
    }

    pub fn store(&self) -> Arc<Store> {
        self.inner.config.store.clone()
    }

    // ---- options ------------------------------------------------------------------------

    pub fn options(&self) -> Options {
        self.state().options.clone()
    }

    /// The selected chat provider.
    pub fn provider(&self) -> Option<Provider> {
        self.state().options.provider().cloned()
    }

    pub fn enabled(&self) -> bool {
        self.state().options.enabled
    }

    pub fn status(&self) -> String {
        self.state().status.clone()
    }

    pub fn set_status(&self, text: &str) {
        self.state().status = text.to_owned();
        self.changed();
    }

    fn persist(&self, options: &Options) -> Result<(), String> {
        self.inner
            .config
            .store
            .save_assistant_options(options)
            .map_err(|e| {
                format!("Could not save AI preferences. Check the local settings folder. ({e})")
            })
    }

    /// Applies `edit` to a copy of the options, saves it, and adopts it only when saving worked.
    fn update(&self, edit: impl FnOnce(&mut Options) -> Result<(), String>) -> Result<(), String> {
        let mut next = self.options();
        edit(&mut next)?;
        self.persist(&next)?;
        self.state().options = next;
        self.changed();
        Ok(())
    }

    pub fn set_enabled(&self, enabled: bool) -> Result<(), String> {
        self.update(|o| {
            o.enabled = enabled;
            Ok(())
        })?;
        self.set_status(if enabled {
            "Ready · choose Chat or start voice."
        } else {
            "AI companion is off."
        });
        Ok(())
    }

    pub fn set_island_character(&self, visible: bool) -> Result<(), String> {
        self.update(|o| {
            o.island_character = visible;
            Ok(())
        })
    }

    pub fn set_widget_navigation(&self, on: bool) -> Result<(), String> {
        self.update(|o| {
            o.widget_navigation = on;
            Ok(())
        })
    }

    pub fn set_instructions(&self, text: &str) -> Result<(), String> {
        let text = text.to_owned();
        self.update(move |o| {
            o.instructions = text;
            Ok(())
        })
    }

    pub fn select_provider(&self, id: &str) -> Result<(), String> {
        let id = id.to_owned();
        self.update(|o| {
            if !o.providers.iter().any(|p| p.id == id) {
                return Err("Choose a saved provider.".into());
            }
            o.selected_provider = id;
            Ok(())
        })?;
        self.set_status("New conversation · provider changed.");
        Ok(())
    }

    pub fn select_voice_provider(&self, id: &str) -> Result<(), String> {
        let id = id.to_owned();
        self.update(|o| {
            o.selected_voice_provider = id;
            Ok(())
        })
    }

    /// Adds a provider from a preset name (`AddProvider`); returns its id.
    pub fn add_provider(&self, preset: &str) -> Result<String, String> {
        let mut provider = providers::create(preset).map_err(|e| message(&e))?;
        if matches!(provider.name.as_str(), "OpenAI" | "Google Gemini") {
            let openai = provider.name == "OpenAI";
            provider.realtime_protocol = if openai {
                "OpenAIRealtime"
            } else {
                "GeminiLive"
            }
            .into();
            provider.realtime_url = default_realtime_url(&provider.realtime_protocol).into();
            provider.voice = if openai { "marin" } else { "Kore" }.into();
        }
        let id = provider.id.clone();
        self.update(|o| {
            if o.providers.len() >= naby_core::assistant::models::MAX_PROVIDERS {
                return Err("Keep up to 12 providers. Remove one before adding another.".into());
            }
            o.providers.push(provider);
            if o.selected_provider.is_empty() {
                o.selected_provider = id.clone();
            }
            Ok(())
        })?;
        Ok(id)
    }

    /// Saves an edited provider. A changed server or a new key moves to a fresh credential
    /// identity (`save_provider`). Returns the saved provider's id.
    pub fn save_provider(
        &self,
        original_id: &str,
        candidate: &Provider,
        replacement_key: &str,
    ) -> Result<String, String> {
        let current = self.options();
        let keys = self.inner.config.keys.clone();
        let store = self.inner.config.store.clone();
        let next = persistence::save_provider(
            &current,
            original_id,
            candidate,
            replacement_key,
            &*keys,
            &mut |o| {
                store.save_assistant_options(o).map_err(|e| {
                    AssistantError::io(format!(
                        "Could not save AI preferences. Check the local settings folder. ({e})"
                    ))
                })
            },
        )
        .map_err(|e| message(&e))?;
        let id = next.selected_provider.clone();
        {
            let mut state = self.state();
            state.options = next;
            state.checks.forget(original_id);
            state.status = "Provider saved on this device.".into();
        }
        self.changed();
        Ok(id)
    }

    /// Removes a provider and its key (`RemoveProvider`).
    pub fn remove_provider(&self, id: &str) -> Result<(), String> {
        let id = id.to_owned();
        self.keys_remove(&id)?;
        self.update(|o| {
            o.providers.retain(|p| p.id != id);
            if o.selected_provider == id {
                o.selected_provider = o
                    .providers
                    .first()
                    .map(|p| p.id.clone())
                    .unwrap_or_default();
            }
            if o.selected_voice_provider == id {
                o.selected_voice_provider.clear();
            }
            Ok(())
        })?;
        self.state().checks.forget(&id);
        Ok(())
    }

    fn keys_remove(&self, id: &str) -> Result<(), String> {
        self.inner.config.keys.remove(id).map_err(|e| message(&e))
    }

    /// Deletes only the stored key (`ForgetKey`).
    pub fn forget_key(&self, id: &str) -> Result<(), String> {
        self.keys_remove(id)?;
        self.state().checks.forget(id);
        self.changed();
        Ok(())
    }

    /// Whether a key is stored for `id`. Keys are never read back for display.
    pub fn key_exists(&self, id: &str) -> Result<bool, String> {
        self.inner.config.keys.exists(id).map_err(|e| message(&e))
    }

    /// The key to send with a request for `provider` (chat).
    pub fn key_for(&self, id: &str) -> Option<String> {
        self.inner.config.keys.read(id).ok().flatten()
    }

    // ---- readiness ----------------------------------------------------------------------

    pub fn record_check(&self, provider: &Provider, success: bool, detail: &str) {
        self.state().checks.record(provider, success, detail);
        self.changed();
    }

    pub fn forget_check(&self, id: &str) {
        self.state().checks.forget(id);
    }

    /// Chat readiness of a saved provider (`ChatSetupFor`).
    pub fn chat_setup_for(&self, provider: &Provider) -> SetupState {
        self.state().checks.chat_setup(Some(provider))
    }

    pub fn setup(&self) -> SetupReadiness {
        let state = self.state();
        assess_setup(&state.options, &state.checks, &state.speech)
    }

    pub fn set_capture(&self, capture: Capture) {
        let mut state = self.state();
        if state.capture != capture {
            state.capture = capture;
            drop(state);
            self.changed();
        }
    }

    /// Edits the capture state (screen sharing, wake, microphone); notifies only on a change.
    pub fn update_capture(&self, edit: impl FnOnce(&mut Capture)) {
        let mut state = self.state();
        let mut next = state.capture.clone();
        edit(&mut next);
        if state.capture != next {
            state.capture = next;
            drop(state);
            self.changed();
        }
    }

    /// Saves a change to the options (the voice settings pages use it). The edit runs on a copy
    /// and is adopted only when it succeeds and saving works.
    pub fn edit_options(
        &self,
        edit: impl FnOnce(&mut Options) -> Result<(), String>,
    ) -> Result<(), String> {
        self.update(edit)
    }

    /// The host reports speech component work (downloads, removal) for the readiness summary.
    pub fn set_speech(&self, speech: SpeechService) {
        let mut state = self.state();
        if state.speech != speech {
            state.speech = speech;
            drop(state);
            self.changed();
        }
    }

    pub fn capture_summary(&self) -> String {
        capture_summary(&self.state().capture)
    }

    pub fn setup_summary(&self) -> String {
        let state = self.state();
        setup_summary(&state.options, &state.capture)
    }

    // ---- background checks --------------------------------------------------------------

    /// Resolves the key a draft check may use (`DraftKey`): a typed key, or the saved key only
    /// when the destination did not change.
    pub fn key_for_draft(
        &self,
        entered: &str,
        saved: Option<&Provider>,
        candidate: &Provider,
    ) -> KeyChoice {
        match persistence::draft_key_source(entered, saved, candidate) {
            KeySource::Entered => KeyChoice::Entered(entered.to_owned()),
            KeySource::Saved(id) => KeyChoice::Saved(id),
            KeySource::None => KeyChoice::None,
        }
    }

    fn resolve_key(&self, choice: &KeyChoice) -> Option<String> {
        match choice {
            KeyChoice::Entered(key) => Some(key.clone()),
            KeyChoice::Saved(id) => self.inner.config.keys.read(id).ok().flatten(),
            KeyChoice::None => None,
        }
    }

    pub fn busy(&self) -> bool {
        !self.state().ops.is_empty()
    }

    pub fn running(&self) -> Option<OpKind> {
        self.state().ops.values().next().map(|r| r.kind)
    }

    fn begin(
        &self,
        kind: OpKind,
        timeout: Duration,
        work: impl FnOnce(Transport, &AtomicBool) -> OpResult + Send + 'static,
    ) -> Result<OpId, String> {
        let cancel = Arc::new(AtomicBool::new(false));
        let id = {
            let mut state = self.state();
            if !state.ops.is_empty() {
                return Err("Finish or cancel the current check first.".into());
            }
            let id = state.next_op;
            state.next_op += 1;
            state.ops.insert(
                id,
                Running {
                    kind,
                    cancel: cancel.clone(),
                },
            );
            id
        };
        let transport = (self.inner.config.transport)()
            .with_timeout(timeout)
            .unwrap_or_else(|_| (self.inner.config.transport)());
        let me = self.clone();
        let spawned = std::thread::Builder::new()
            .name("NabyScape provider check".into())
            .spawn(move || {
                let result = work(transport, &cancel);
                let mut state = me.state();
                // A cancelled check reports nothing: the editor already moved on.
                if state.ops.remove(&id).is_some() && !cancel.load(Ordering::SeqCst) {
                    state.results.push((id, result));
                }
                drop(state);
                me.changed();
            });
        if spawned.is_err() {
            self.state().ops.remove(&id);
            return Err("The check could not start.".into());
        }
        self.changed();
        Ok(id)
    }

    /// Loads the model IDs a provider advertises (metadata only).
    pub fn begin_load_models(&self, candidate: Provider, key: &KeyChoice) -> Result<OpId, String> {
        let key = self.resolve_key(key);
        self.begin(
            OpKind::Models,
            Duration::from_secs(30),
            move |transport, cancel| {
                OpResult::Models(
                    transport
                        .list_models(&candidate, key.as_deref(), cancel)
                        .map_err(|e| message(&e)),
                )
            },
        )
    }

    /// Sends one short chat message that proves the key, address and model work.
    pub fn begin_check_connection(
        &self,
        candidate: Provider,
        key: &KeyChoice,
    ) -> Result<OpId, String> {
        let key = self.resolve_key(key);
        self.begin(
            OpKind::Connection,
            Duration::from_secs(45),
            move |transport, cancel| {
                OpResult::Connection(
                    transport
                        .check_connection(&candidate, key.as_deref(), cancel)
                        .map_err(|e| message(&e)),
                )
            },
        )
    }

    /// Sends a generated red square to the saved provider.
    pub fn begin_check_image(&self, saved: Provider) -> Result<OpId, String> {
        let key = self.key_for(&saved.id);
        self.begin(
            OpKind::Image,
            Duration::from_secs(45),
            move |transport, cancel| {
                OpResult::Image(
                    transport
                        .check_image_support(&saved, key.as_deref(), cancel)
                        .map_err(|e| message(&e)),
                )
            },
        )
    }

    /// Cancels the running check; its result is dropped.
    pub fn cancel(&self) {
        let mut state = self.state();
        for running in state.ops.values() {
            running.cancel.store(true, Ordering::SeqCst);
        }
        state.ops.clear();
        state.results.clear();
        drop(state);
        self.changed();
    }

    /// The result of one check, if it finished (several views can run checks side by side).
    pub fn take_result(&self, id: OpId) -> Option<OpResult> {
        let mut state = self.state();
        let index = state.results.iter().position(|(op, _)| *op == id)?;
        Some(state.results.remove(index).1)
    }

    /// Adds a fully built provider (Hermes setup), stores its key and selects it. The key is
    /// removed again when the options cannot be saved.
    pub fn add_connected_provider(&self, provider: Provider, key: &str) -> Result<String, String> {
        let id = provider.id.clone();
        naby_core::assistant::hermes::check_room(&self.options().providers)
            .map_err(|e| message(&e))?;
        let stored = if key.is_empty() {
            None
        } else {
            Some(persistence::prepare_key_for_storage(key).map_err(|e| message(&e))?)
        };
        if let Some(key) = &stored {
            self.inner
                .config
                .keys
                .write(&id, key)
                .map_err(|e| message(&e))?;
        }
        let result = self.update(|o| {
            o.providers.push(provider);
            o.selected_provider = id.clone();
            Ok(())
        });
        if let Err(error) = result {
            let _ = self.inner.config.keys.remove(&id);
            return Err(error);
        }
        self.set_status("New conversation \u{b7} provider changed.");
        Ok(id)
    }

    /// Finished checks since the last call.
    pub fn take_results(&self) -> Vec<(OpId, OpResult)> {
        std::mem::take(&mut self.state().results)
    }

    /// The draft guard's identity of a provider (what the editor can change).
    pub fn identity_of(provider: &Provider) -> DraftIdentity {
        DraftIdentity::of(provider)
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use naby_core::assistant::persistence::MemoryKeys;
    use naby_core::assistant::transport::{HttpExecutor, HttpRequest, HttpResponse};
    use naby_core::assistant::Result as AResult;
    use std::sync::atomic::AtomicUsize;

    pub type Handler = Box<dyn FnMut(&HttpRequest) -> AResult<HttpResponse> + Send>;

    /// A scripted HTTP executor that records every request it sees.
    pub struct Script {
        pub handler: Mutex<Handler>,
        pub seen: Arc<Mutex<Vec<HttpRequest>>>,
        pub calls: Arc<AtomicUsize>,
    }

    impl HttpExecutor for Script {
        fn execute(
            &self,
            request: &HttpRequest,
            _: Duration,
            _: &AtomicBool,
        ) -> AResult<HttpResponse> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.seen.lock().unwrap().push(request.clone());
            (self.handler.lock().unwrap())(request)
        }
    }

    pub struct Fixture {
        pub service: AssistantService,
        pub seen: Arc<Mutex<Vec<HttpRequest>>>,
        pub wakes: Arc<AtomicUsize>,
        #[allow(dead_code)]
        pub dir: tempfile::TempDir,
        pub keys: Arc<MemoryKeys>,
    }

    /// A service over a temp store, in-memory keys and a scripted provider.
    pub fn fixture(
        handler: impl FnMut(&HttpRequest) -> AResult<HttpResponse> + Send + 'static,
    ) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(dir.path().to_path_buf()));
        let seen: Arc<Mutex<Vec<HttpRequest>>> = Arc::default();
        let calls = Arc::new(AtomicUsize::new(0));
        let handler: Arc<Mutex<Handler>> = Arc::new(Mutex::new(Box::new(handler)));
        let (seen2, calls2) = (seen.clone(), calls.clone());
        let transport: TransportFactory = Arc::new(move || {
            let handler = handler.clone();
            Transport::new(Box::new(Script {
                handler: Mutex::new(Box::new(move |r| (handler.lock().unwrap())(r))),
                seen: seen2.clone(),
                calls: calls2.clone(),
            }))
        });
        let wakes = Arc::new(AtomicUsize::new(0));
        let w = wakes.clone();
        let keys = Arc::new(MemoryKeys::new());
        let service = AssistantService::new(ServiceConfig {
            store,
            keys: keys.clone(),
            transport,
            waker: Arc::new(move || {
                w.fetch_add(1, Ordering::SeqCst);
            }),
        });
        Fixture {
            service,
            seen,
            wakes,
            dir,
            keys,
        }
    }

    /// Waits until the service has a finished check (tests only; bounded).
    pub fn wait_result(service: &AssistantService) -> (OpId, OpResult) {
        for _ in 0..400 {
            if let Some(first) = service.take_results().into_iter().next() {
                return first;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("the provider check did not finish");
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;
    use naby_core::assistant::transport::HttpResponse;
    use serde_json::json;

    fn chat_reply(text: &str) -> HttpResponse {
        HttpResponse::new(
            200,
            "application/json",
            json!({"choices": [{"message": {"role": "assistant", "content": text}}]})
                .to_string()
                .into_bytes(),
        )
    }

    fn saved_provider(f: &Fixture) -> Provider {
        let id = f.service.add_provider("Custom Chat Completions").unwrap();
        let mut p = f
            .service
            .options()
            .providers
            .into_iter()
            .find(|p| p.id == id)
            .unwrap();
        p.base_url = "https://provider.example/v1/".into();
        p.model = "chat-1".into();
        f.service.save_provider(&id, &p, "sk-test").unwrap();
        f.service.provider().unwrap()
    }

    #[test]
    fn add_edit_select_and_remove_providers_persist() {
        let f = fixture(|_| Ok(chat_reply("OK")));
        let first = f.service.add_provider("OpenAI").unwrap();
        let second = f.service.add_provider("Ollama").unwrap();
        let options = f.service.options();
        assert_eq!(options.providers.len(), 2);
        assert_eq!(options.selected_provider, first);
        let openai = &options.providers[0];
        assert_eq!(openai.realtime_protocol, "OpenAIRealtime");
        assert_eq!(openai.voice, "marin");

        f.service.select_provider(&second).unwrap();
        assert_eq!(f.service.provider().unwrap().name, "Ollama");
        assert!(f.service.select_provider("nope").is_err());

        // Edit: same server keeps its identity and takes the new key.
        let mut edited = f.service.options().providers[1].clone();
        edited.model = "llama".into();
        let id = f.service.save_provider(&second, &edited, "").unwrap();
        assert_eq!(id, second);
        assert_eq!(f.service.provider().unwrap().model, "llama");

        // A new key rotates the credential identity and the old key is gone.
        f.keys.write(&second, "old").unwrap();
        let rotated = f
            .service
            .save_provider(&second, &edited, "new-key")
            .unwrap();
        assert_ne!(rotated, second);
        assert_eq!(f.keys.read(&rotated).unwrap().as_deref(), Some("new-key"));
        assert_eq!(f.keys.read(&second).unwrap(), None);

        // Persisted on disk: a fresh service sees both providers.
        let reread = f.service.store().load_assistant_options();
        assert_eq!(reread.providers.len(), 2);

        f.service.remove_provider(&rotated).unwrap();
        assert_eq!(f.service.options().providers.len(), 1);
        assert_eq!(f.service.options().selected_provider, first);
        assert_eq!(f.keys.read(&rotated).unwrap(), None);
    }

    #[test]
    fn twelve_providers_is_the_limit() {
        let f = fixture(|_| Ok(chat_reply("OK")));
        for _ in 0..12 {
            f.service.add_provider("Ollama").unwrap();
        }
        let error = f.service.add_provider("Ollama").unwrap_err();
        assert!(error.contains("12 providers"), "{error}");
        assert_eq!(f.service.options().providers.len(), 12);
    }

    #[test]
    fn load_models_returns_sorted_ids_from_the_scripted_provider() {
        let f = fixture(|_| {
            Ok(HttpResponse::new(
                200,
                "application/json",
                json!({"data": [{"id": "b-model"}, {"id": "a-model"}]})
                    .to_string()
                    .into_bytes(),
            ))
        });
        let provider = saved_provider(&f);
        f.service
            .begin_load_models(provider, &KeyChoice::None)
            .unwrap();
        let (_, result) = wait_result(&f.service);
        match result {
            OpResult::Models(Ok(ids)) => assert_eq!(ids, vec!["a-model", "b-model"]),
            other => panic!("{other:?}"),
        }
        assert!(!f.service.busy());
        assert!(f.wakes.load(Ordering::SeqCst) > 0);
    }

    #[test]
    fn connection_check_reports_success_and_failure_text() {
        let f = fixture(|_| Ok(chat_reply("OK")));
        let provider = saved_provider(&f);
        let key = f.service.key_for_draft("", Some(&provider), &provider);
        assert_eq!(key, KeyChoice::Saved(provider.id.clone()));
        f.service
            .begin_check_connection(provider.clone(), &key)
            .unwrap();
        let (_, result) = wait_result(&f.service);
        assert_eq!(result, OpResult::Connection(Ok(())));
        // The key used came from the credential store, not the editor.
        let seen = f.seen.lock().unwrap();
        assert_eq!(seen[0].header("authorization"), Some("Bearer sk-test"));
        drop(seen);

        let failing = fixture(|_| Ok(HttpResponse::new(401, "application/json", b"{}".to_vec())));
        let provider = saved_provider(&failing);
        failing
            .service
            .begin_check_connection(provider, &KeyChoice::Entered("bad".into()))
            .unwrap();
        match wait_result(&failing.service).1 {
            OpResult::Connection(Err(text)) => assert!(!text.is_empty()),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn only_one_check_runs_at_a_time_and_cancel_drops_the_result() {
        let f = fixture(|_| {
            std::thread::sleep(Duration::from_millis(150));
            Ok(chat_reply("OK"))
        });
        let provider = saved_provider(&f);
        f.service
            .begin_check_connection(provider.clone(), &KeyChoice::None)
            .unwrap();
        assert!(f
            .service
            .begin_check_connection(provider, &KeyChoice::None)
            .is_err());
        f.service.cancel();
        assert!(!f.service.busy());
        std::thread::sleep(Duration::from_millis(300));
        assert!(f.service.take_results().is_empty());
    }

    #[test]
    fn image_check_recognizes_red() {
        let f = fixture(|_| Ok(chat_reply("Red.")));
        let mut provider = saved_provider(&f);
        provider.vision = true;
        let id = provider.id.clone();
        f.service.save_provider(&id, &provider, "").unwrap();
        let saved = f.service.provider().unwrap();
        f.service.begin_check_image(saved).unwrap();
        assert_eq!(wait_result(&f.service).1, OpResult::Image(Ok(true)));
    }

    #[test]
    fn readiness_follows_recorded_checks() {
        let f = fixture(|_| Ok(chat_reply("OK")));
        let provider = saved_provider(&f);
        assert_eq!(
            f.service.chat_setup_for(&provider).title,
            "Saved · not tested"
        );
        f.service
            .record_check(&provider, true, "Mine returned a chat response.");
        assert!(f.service.chat_setup_for(&provider).verified);
        assert!(f.service.setup().chat.verified);
    }
}
