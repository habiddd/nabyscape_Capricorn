//! One chat window's conversation (`AssistantRuntime` chat half): the history, the draft
//! attachments, screen sharing and the worker thread that runs a turn.
//!
//! The session is created when the window opens and dropped when it closes, which releases the
//! conversation memory (only the persisted options live on in the service). The UI thread reads
//! immutable [`ChatModel`] snapshots; workers publish by replacing the snapshot and calling the
//! waker, so nothing polls.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, RecvTimeoutError};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Duration;

use naby_core::assistant::attachments::{
    bytes_from_data_url, check_draft_add, Attachment, MAXIMUM_FILES,
};
use naby_core::assistant::conversation::{chat_instructions, Hooks, SendRequest};
use naby_core::assistant::media::check_attachments;
use naby_core::assistant::{AssistantError, Call, Conversation, Message, Role, State};
use naby_render::Image;

use super::ports::{
    AttachmentPreparer, MediaLoader, ScreenSource, ServiceMedia, ShareTarget, ToolRunner,
};
use super::service::{AssistantService, Waker};
use super::widget_tools::WidgetTools;

/// The whole turn (provider rounds and actions) may take this long.
const TURN_DEADLINE: Duration = Duration::from_secs(300);
const PREVIEW_EDGE: u32 = 360;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

/// An attachment as the chat shows it. `id` is unique for the life of the process, so caches can
/// key on it.
#[derive(Debug)]
pub struct ShownFile {
    pub id: u64,
    pub file: Attachment,
}

pub fn shown(file: Attachment) -> Arc<ShownFile> {
    Arc::new(ShownFile {
        id: next_id(),
        file,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatRole {
    User,
    Assistant,
}

/// One bubble of the transcript.
#[derive(Debug)]
pub struct ChatItem {
    pub id: u64,
    pub role: ChatRole,
    pub text: String,
    pub attachments: Vec<Arc<ShownFile>>,
}

/// Everything the chat view reads. Cloning is cheap (`Arc`s inside).
#[derive(Debug, Clone)]
pub struct ChatModel {
    pub items: Vec<Arc<ChatItem>>,
    /// Reply text streaming in right now.
    pub live: Option<String>,
    pub state: State,
    pub status: String,
    /// A turn is running.
    pub busy: bool,
    /// Attachments are being prepared.
    pub importing: bool,
    pub draft: Vec<Arc<ShownFile>>,
    /// Name of the shared window or display.
    pub sharing: Option<String>,
    pub share_preview: Option<Arc<Image>>,
    /// The last send went to a Hermes agent that may have started work.
    pub agent_work: bool,
    /// Bumps on every conversation reset.
    pub epoch: u64,
    /// Counts sends; with `failed_serial` it tells the composer to restore the text.
    pub send_serial: u64,
    pub failed_serial: u64,
    pub failed_text: Option<Arc<str>>,
}

impl Default for ChatModel {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            live: None,
            state: State::Idle,
            status: "Configure a provider to begin. Nothing is captured in the background.".into(),
            busy: false,
            importing: false,
            draft: Vec::new(),
            sharing: None,
            share_preview: None,
            agent_work: false,
            epoch: 0,
            send_serial: 0,
            failed_serial: 0,
            failed_text: None,
        }
    }
}

fn visible_role(message: &Message) -> Option<ChatRole> {
    let role = match message.role {
        Role::User => ChatRole::User,
        Role::Assistant => ChatRole::Assistant,
        _ => return None,
    };
    if message.text.trim().is_empty() && message.attachments.is_empty() {
        return None;
    }
    if message.text.starts_with("Updated shared screen after") {
        return None;
    }
    Some(role)
}

fn item_from(message: &Message, role: ChatRole) -> ChatItem {
    ChatItem {
        id: next_id(),
        role,
        text: message.text.clone(),
        attachments: message.attachments.iter().cloned().map(shown).collect(),
    }
}

/// Rebuilds the transcript from the conversation, keeping unchanged items (so their layout and
/// image caches stay valid).
fn mirror(messages: &[Message], old: &[Arc<ChatItem>]) -> Vec<Arc<ChatItem>> {
    let mut out: Vec<Arc<ChatItem>> = Vec::new();
    for message in messages {
        let Some(role) = visible_role(message) else {
            continue;
        };
        let reuse = old.get(out.len()).filter(|item| {
            item.role == role
                && item.text == message.text
                && item.attachments.len() == message.attachments.len()
                && item
                    .attachments
                    .iter()
                    .zip(&message.attachments)
                    .all(|(shown, file)| shown.file == *file)
        });
        out.push(match reuse {
            Some(item) => item.clone(),
            None => Arc::new(item_from(message, role)),
        });
    }
    out
}

struct Shared {
    model: Mutex<Arc<ChatModel>>,
    revision: AtomicU64,
    waker: Waker,
    conversation: Mutex<Conversation>,
    control: naby_core::assistant::ConversationControl,
    /// Bumped by stop, clear and close: results of older workers are ignored.
    generation: AtomicU64,
}

impl Shared {
    fn edit(&self, f: impl FnOnce(&mut ChatModel)) {
        {
            let mut guard = self.model.lock().unwrap_or_else(|e| e.into_inner());
            f(Arc::make_mut(&mut guard));
        }
        self.revision.fetch_add(1, Ordering::SeqCst);
        (self.waker)();
    }

    fn model(&self) -> Arc<ChatModel> {
        self.model.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn conversation(&self) -> MutexGuard<'_, Conversation> {
        self.conversation.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// The chat of one window.
pub struct ChatSession {
    shared: Arc<Shared>,
    service: AssistantService,
    tools: Arc<dyn ToolRunner>,
    screen: Arc<dyn ScreenSource>,
    preparer: Arc<dyn AttachmentPreparer>,
    media: Arc<dyn MediaLoader>,
}

impl ChatSession {
    pub fn new(
        service: AssistantService,
        tools: Arc<dyn ToolRunner>,
        screen: Arc<dyn ScreenSource>,
        preparer: Arc<dyn AttachmentPreparer>,
    ) -> Self {
        let waker = service.waker();
        let mut conversation = Conversation::new();
        let control = conversation.control();
        let shared = Arc::new_cyclic(|weak: &Weak<Shared>| {
            let weak = weak.clone();
            conversation.set_on_added(move |message| {
                let Some(shared) = weak.upgrade() else { return };
                let Some(role) = visible_role(message) else {
                    return;
                };
                let item = Arc::new(item_from(message, role));
                shared.edit(|m| {
                    m.items.push(item);
                    if role == ChatRole::Assistant {
                        m.live = None;
                    }
                });
            });
            Shared {
                model: Mutex::new(Arc::new(ChatModel::default())),
                revision: AtomicU64::new(1),
                waker,
                conversation: Mutex::new(conversation),
                control,
                generation: AtomicU64::new(0),
            }
        });
        let media: Arc<dyn MediaLoader> = Arc::new(ServiceMedia::new(service.clone()));
        let session = Self {
            shared,
            service,
            tools,
            screen,
            preparer,
            media,
        };
        let status = session.service.status();
        session.shared.edit(|m| m.status = status);
        session
    }

    /// Convenience: the chat with the widget tools already shared with the host.
    pub fn with_widget_tools(
        service: AssistantService,
        tools: WidgetTools,
        screen: Arc<dyn ScreenSource>,
        preparer: Arc<dyn AttachmentPreparer>,
    ) -> Self {
        Self::new(service, Arc::new(tools), screen, preparer)
    }

    pub fn service(&self) -> &AssistantService {
        &self.service
    }

    pub fn snapshot(&self) -> Arc<ChatModel> {
        self.shared.model()
    }

    /// Moves whenever the snapshot changed.
    pub fn revision(&self) -> u64 {
        self.shared.revision.load(Ordering::SeqCst)
    }

    pub fn media(&self) -> Arc<dyn MediaLoader> {
        self.media.clone()
    }

    pub fn busy(&self) -> bool {
        let model = self.shared.model();
        model.busy || model.importing
    }

    // ---- sending ------------------------------------------------------------------------

    /// Starts a turn (`Send`). False when nothing was started: busy, disabled, or empty.
    pub fn send(&self, text: &str) -> bool {
        let model = self.shared.model();
        let text = text.trim().to_owned();
        if model.busy || model.importing {
            return false;
        }
        if !self.service.enabled() {
            self.shared.edit(|m| {
                m.status = "Enable AI companion in provider settings.".into();
            });
            return false;
        }
        let files: Vec<Attachment> = model.draft.iter().map(|d| d.file.clone()).collect();
        if text.is_empty() && files.is_empty() {
            return false;
        }
        let text = if text.is_empty() {
            "Please review the attached files.".to_owned()
        } else {
            text
        };
        let provider = self.service.provider();
        let agent_work = provider.as_ref().is_some_and(|p| p.is_hermes());
        let generation = self.shared.generation.load(Ordering::SeqCst);
        let serial = model.send_serial + 1;
        self.shared.edit(|m| {
            m.busy = true;
            m.state = State::Thinking;
            m.status = "Thinking…".into();
            m.live = None;
            m.agent_work = agent_work;
            m.send_serial = serial;
            m.failed_text = None;
        });
        let worker = Worker {
            shared: self.shared.clone(),
            service: self.service.clone(),
            tools: self.tools.clone(),
            screen: self.screen.clone(),
            generation,
            serial,
            text,
            files,
        };
        let spawned = std::thread::Builder::new()
            .name("NabyScape chat".into())
            .spawn(move || worker.run());
        if spawned.is_err() {
            self.shared.edit(|m| {
                m.busy = false;
                m.state = State::Error;
                m.status = "The reply could not start.".into();
            });
            return false;
        }
        true
    }

    /// Stops the running turn, imports and screen sharing (`StopSession`, chat part).
    pub fn stop_session(&self) {
        self.shared.generation.fetch_add(1, Ordering::SeqCst);
        self.shared.control.stop();
        self.screen.stop();
        self.shared.edit(|m| {
            m.busy = false;
            m.importing = false;
            m.state = State::Idle;
            m.live = None;
            m.sharing = None;
            m.share_preview = None;
            m.status = "Session stopped · microphones, wake word and screen sharing off.".into();
        });
    }

    /// Drops the history and the draft (`Clear`).
    pub fn clear(&self) {
        self.shared.generation.fetch_add(1, Ordering::SeqCst);
        self.screen.stop();
        self.shared.control.clear();
        if let Ok(mut conversation) = self.shared.conversation.try_lock() {
            conversation.clear();
        }
        let epoch = self.shared.control.epoch();
        self.shared.edit(|m| {
            m.items.clear();
            m.draft.clear();
            m.live = None;
            m.busy = false;
            m.importing = false;
            m.state = State::Idle;
            m.sharing = None;
            m.share_preview = None;
            m.epoch = epoch;
            m.status = "Conversation cleared from memory.".into();
        });
    }

    /// Ends the session before the window is destroyed.
    pub fn shutdown(&self) {
        self.shared.generation.fetch_add(1, Ordering::SeqCst);
        self.shared.control.clear();
        self.screen.stop();
    }

    // ---- attachments ---------------------------------------------------------------------

    /// Prepares files on a worker and adds them to the draft (`AttachFiles`).
    pub fn attach_files(&self, paths: Vec<String>) {
        self.import(
            "Preparing attachments…",
            "Could not attach file: ",
            move |preparer| {
                paths
                    .iter()
                    .take(MAXIMUM_FILES + 1)
                    .map(|p| preparer.prepare_file(Path::new(p)))
                    .collect()
            },
        );
    }

    /// Prepares a clipboard image (encoded PNG) and adds it to the draft.
    pub fn attach_image(&self, bytes: Vec<u8>) {
        self.import(
            "Preparing clipboard image…",
            "Could not paste image: ",
            move |preparer| preparer.prepare_image(&bytes).map(|a| vec![a]),
        );
    }

    fn import(
        &self,
        working: &str,
        failure: &'static str,
        prepare: impl FnOnce(&dyn AttachmentPreparer) -> Result<Vec<Attachment>, String>
            + Send
            + 'static,
    ) {
        let model = self.shared.model();
        if model.busy || model.importing {
            self.shared.edit(|m| {
                m.status = "Wait for the current operation before attaching.".into();
            });
            return;
        }
        let generation = self.shared.generation.load(Ordering::SeqCst);
        let working = working.to_owned();
        self.shared.edit(|m| {
            m.importing = true;
            m.status = working;
        });
        let shared = self.shared.clone();
        let preparer = self.preparer.clone();
        let spawned = std::thread::Builder::new()
            .name("NabyScape attachments".into())
            .spawn(move || {
                let result = prepare(&*preparer);
                if shared.generation.load(Ordering::SeqCst) != generation {
                    return;
                }
                let existing: Vec<Attachment> = shared
                    .model()
                    .draft
                    .iter()
                    .map(|d| d.file.clone())
                    .collect();
                let checked = result.and_then(|files| {
                    let mut all = existing;
                    for file in &files {
                        check_draft_add(&all, file).map_err(|e| e.message().to_owned())?;
                        all.push(file.clone());
                    }
                    Ok(files)
                });
                shared.edit(|m| {
                    m.importing = false;
                    match checked {
                        Ok(files) => {
                            m.draft.extend(files.into_iter().map(shown));
                            m.status =
                                "Attachments ready. They are sent only when you press Send.".into();
                            if m.state == State::Error {
                                m.state = State::Idle;
                            }
                        }
                        Err(error) => {
                            m.state = State::Error;
                            m.status = format!("{failure}{error}");
                        }
                    }
                });
            });
        if spawned.is_err() {
            self.shared.edit(|m| m.importing = false);
        }
    }

    pub fn remove_attachment(&self, id: u64) {
        let model = self.shared.model();
        if model.busy || model.importing {
            return;
        }
        self.shared.edit(|m| m.draft.retain(|d| d.id != id));
    }

    // ---- screen sharing ------------------------------------------------------------------

    pub fn share_targets(&self) -> Vec<ShareTarget> {
        self.screen.targets()
    }

    pub fn sharing(&self) -> bool {
        self.screen.sharing().is_some()
    }

    /// Starts sharing a window or display and captures a first frame for the preview.
    pub fn start_sharing(&self, id: &str) {
        let fail = |text: &str| {
            let text = text.to_owned();
            self.shared.edit(|m| {
                m.state = State::Error;
                m.status = text;
            });
        };
        if !self.service.enabled() {
            return fail("Enable AI companion first.");
        }
        if !self.service.provider().is_some_and(|p| p.vision) {
            return fail(
                "Choose a model that supports images, then enable Image input in AI settings.",
            );
        }
        match self.screen.select(id) {
            Err(error) => fail(&error),
            Ok(name) => {
                self.shared.edit(|m| {
                    m.sharing = Some(name.clone());
                    m.status = format!("Sharing: {name}");
                    if m.state == State::Error {
                        m.state = State::Idle;
                    }
                });
                let shared = self.shared.clone();
                let screen = self.screen.clone();
                let generation = shared.generation.load(Ordering::SeqCst);
                let _ = std::thread::Builder::new()
                    .name("NabyScape share preview".into())
                    .spawn(move || {
                        let cancel = AtomicBool::new(false);
                        let frame = capture_frame(&*screen, &shared, &cancel, generation);
                        if let Err(error) = frame {
                            if shared.generation.load(Ordering::SeqCst) == generation {
                                screen.stop();
                                shared.edit(|m| {
                                    m.sharing = None;
                                    m.share_preview = None;
                                    m.state = State::Error;
                                    m.status = error;
                                });
                            }
                        }
                    });
            }
        }
    }

    pub fn stop_sharing(&self) {
        self.screen.stop();
        self.shared.edit(|m| {
            m.sharing = None;
            m.share_preview = None;
            m.status = "Screen sharing stopped.".into();
        });
    }
}

impl Drop for ChatSession {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Captures one frame, publishes the preview and returns the data URL with its size.
fn capture_frame(
    screen: &dyn ScreenSource,
    shared: &Shared,
    cancel: &AtomicBool,
    generation: u64,
) -> Result<(String, u32, u32), String> {
    let frame = screen.capture(cancel)?;
    if shared.generation.load(Ordering::SeqCst) == generation {
        let preview = bytes_from_data_url(&frame.data_url)
            .ok()
            .and_then(|bytes| Image::decode(&bytes, PREVIEW_EDGE, PREVIEW_EDGE))
            .map(Arc::new);
        shared.edit(|m| m.share_preview = preview);
    }
    Ok((frame.data_url, frame.width, frame.height))
}

struct Worker {
    shared: Arc<Shared>,
    service: AssistantService,
    tools: Arc<dyn ToolRunner>,
    screen: Arc<dyn ScreenSource>,
    generation: u64,
    serial: u64,
    text: String,
    files: Vec<Attachment>,
}

impl Worker {
    fn current(&self) -> bool {
        self.shared.generation.load(Ordering::SeqCst) == self.generation
    }

    fn run(self) {
        let cancel = Arc::new(AtomicBool::new(false));
        // Overall deadline: wakes the moment the turn is done, otherwise cancels after 5 min.
        let (done, waiting) = channel::<()>();
        let watchdog_cancel = cancel.clone();
        let _ = std::thread::Builder::new()
            .name("NabyScape chat deadline".into())
            .spawn(move || {
                if let Err(RecvTimeoutError::Timeout) = waiting.recv_timeout(TURN_DEADLINE) {
                    watchdog_cancel.store(true, Ordering::SeqCst);
                }
            });
        let outcome = self.turn(&cancel);
        drop(done);

        // Keep the transcript honest: a cancelled or failed turn rolled its messages back.
        if self.shared.control.epoch() == self.shared.model().epoch {
            let old = self.shared.model().items.clone();
            let items = {
                let conversation = self.shared.conversation();
                mirror(conversation.messages(), &old)
            };
            self.shared.edit(|m| m.items = items);
        }
        if !self.current() {
            return;
        }
        let provider = self.service.provider();
        let agent = self.shared.model().agent_work;
        let hermes_note = " Hermes may have started work; check its session before retrying.";
        match outcome {
            Ok(()) => {
                if let Some(provider) = provider {
                    self.service.record_check(
                        &provider,
                        true,
                        &format!("{} · {} completed a chat reply.", provider.name, provider.model),
                    );
                }
                self.shared.edit(|m| {
                    m.busy = false;
                    m.live = None;
                    m.draft.clear();
                    m.state = State::Idle;
                    m.status = "Ready".into();
                });
            }
            Err(AssistantError::Cancelled) => self.shared.edit(|m| {
                m.busy = false;
                m.live = None;
                m.state = State::Idle;
                m.status = if agent {
                    "Request stopped. Hermes may still be working; check its session before retrying."
                        .into()
                } else {
                    "Request stopped.".into()
                };
            }),
            Err(error) => {
                let text: Arc<str> = Arc::from(self.text.as_str());
                let serial = self.serial;
                self.shared.edit(|m| {
                    m.busy = false;
                    m.live = None;
                    m.state = State::Error;
                    m.status = format!("{}{}", error.message(), if agent { hermes_note } else { "" });
                    m.failed_serial = serial;
                    m.failed_text = Some(text);
                });
            }
        }
    }

    fn turn(&self, cancel: &Arc<AtomicBool>) -> Result<(), AssistantError> {
        let mut provider = self
            .service
            .provider()
            .ok_or_else(|| AssistantError::invalid("Add and select a provider first."))?;
        let options = self.service.options();
        let tools = if provider.is_hermes() {
            Vec::new()
        } else {
            self.tools.tools()
        };
        provider.tools = !tools.is_empty();
        let key = self.service.key_for(&provider.id);
        check_attachments(&provider, &self.files)?;
        let generation = self.generation;
        let capture = |cancel: &AtomicBool| -> Result<Option<(String, u32, u32)>, AssistantError> {
            if self.screen.sharing().is_none() {
                return Ok(None);
            }
            capture_frame(&*self.screen, &self.shared, cancel, generation)
                .map(Some)
                .map_err(AssistantError::io)
        };
        let first = if provider.vision {
            capture(cancel)?
        } else {
            None
        };
        let frame = first.as_ref().map(|(_, w, h)| (*w, *h));
        let instructions = chat_instructions(&options.instructions, frame);
        let transport = self.service.transport();

        let tools_runner = self.tools.clone();
        let mut execute = |call: &Call, _: &AtomicBool| -> naby_core::assistant::Result<String> {
            Ok(tools_runner.execute(call))
        };
        let mut refresh = |cancel: &AtomicBool| -> naby_core::assistant::Result<Option<String>> {
            Ok(capture(cancel)?.map(|(url, _, _)| url))
        };
        let shared = self.shared.clone();
        let mut stream = |delta: &str| -> naby_core::assistant::Result<()> {
            if shared.generation.load(Ordering::SeqCst) != generation {
                return Err(AssistantError::Cancelled);
            }
            shared.edit(|m| m.live.get_or_insert_with(String::new).push_str(delta));
            Ok(())
        };
        let mut hooks = Hooks {
            execute: &mut execute,
            refresh_screen: if provider.vision {
                Some(&mut refresh)
            } else {
                None
            },
            voice_text: Some(&mut stream),
            voice_progress: None,
        };
        let mut request = SendRequest::new(&provider, &instructions, &self.text);
        request.key = key.as_deref();
        request.image = first.map(|(url, _, _)| url);
        request.tools = &tools;
        request.attachments = self.files.clone();
        request.cancel = Some(cancel.clone());
        let mut conversation = self.shared.conversation();
        conversation.send(&transport, request, &mut hooks)
    }
}

#[cfg(test)]
mod tests {
    use super::super::ports::{BasicPreparer, NoScreen, NoTools, ShareFrame};
    use super::super::service::test_support::{fixture, Fixture};
    use super::*;
    use naby_core::assistant::transport::HttpResponse;
    use serde_json::json;
    use std::sync::atomic::AtomicUsize;
    use std::time::Instant;

    fn reply(text: &str) -> HttpResponse {
        HttpResponse::new(
            200,
            "application/json",
            json!({"choices": [{"message": {"role": "assistant", "content": text}}]})
                .to_string()
                .into_bytes(),
        )
    }

    fn sse(deltas: &[&str]) -> HttpResponse {
        let mut body = String::new();
        for d in deltas {
            body.push_str(&format!(
                "data: {}\n\n",
                json!({"choices": [{"delta": {"content": d}}]})
            ));
        }
        body.push_str(&format!(
            "data: {}\n\n",
            json!({"choices": [{"delta": {}, "finish_reason": "stop"}]})
        ));
        body.push_str("data: [DONE]\n\n");
        HttpResponse::new(200, "text/event-stream", body.into_bytes())
    }

    fn wait_until(session: &ChatSession, done: impl Fn(&ChatModel) -> bool) -> Arc<ChatModel> {
        let start = Instant::now();
        loop {
            let model = session.snapshot();
            if done(&model) {
                return model;
            }
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "timed out: {:?}",
                model.status
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn session_with(f: &Fixture, screen: Arc<dyn ScreenSource>) -> ChatSession {
        let id = f.service.add_provider("Custom Chat Completions").unwrap();
        let mut p = f.service.provider().unwrap();
        p.base_url = "https://provider.example/v1/".into();
        p.model = "chat-1".into();
        f.service.save_provider(&id, &p, "sk-chat").unwrap();
        ChatSession::new(
            f.service.clone(),
            Arc::new(NoTools),
            screen,
            Arc::new(BasicPreparer),
        )
    }

    #[test]
    fn composer_send_emits_the_request_and_shows_the_reply() {
        let f = fixture(|r| {
            let body = r.json().unwrap();
            assert_eq!(body["model"], "chat-1");
            Ok(reply("Hello from the fake provider."))
        });
        let session = session_with(&f, Arc::new(NoScreen));
        assert!(session.send("  Hi Naby  "));
        let model = wait_until(&session, |m| !m.busy && m.items.len() == 2);
        assert_eq!(model.items[0].role, ChatRole::User);
        assert_eq!(model.items[0].text, "Hi Naby");
        assert_eq!(model.items[1].text, "Hello from the fake provider.");
        assert_eq!(model.status, "Ready");
        assert_eq!(model.state, State::Idle);
        let seen = f.seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].header("authorization"), Some("Bearer sk-chat"));
        let body = seen[0].json().unwrap();
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.last().unwrap()["content"], "Hi Naby");
        assert_eq!(messages[0]["role"], "system");
        drop(seen);
        // A completed reply marks the saved connection as checked.
        assert!(f.service.setup().chat.verified);
    }

    #[test]
    fn streaming_deltas_update_the_live_reply_before_it_is_final() {
        let gate = Arc::new(Mutex::new(()));
        let held = gate.lock().unwrap();
        let g = gate.clone();
        let f = fixture(move |_| {
            let _gate = g.lock().unwrap();
            Ok(sse(&["Hel", "lo ", "there"]))
        });
        let session = session_with(&f, Arc::new(NoScreen));
        assert!(session.send("stream please"));
        let model = wait_until(&session, |m| m.busy && m.items.len() == 1);
        assert_eq!(model.state, State::Thinking);
        assert_eq!(model.status, "Thinking…");
        drop(held);
        let model = wait_until(&session, |m| !m.busy && m.items.len() == 2);
        assert_eq!(model.items[1].text, "Hello there");
        assert_eq!(model.live, None);
        // The request asked for a stream.
        assert_eq!(f.seen.lock().unwrap()[0].json().unwrap()["stream"], true);
    }

    #[test]
    fn stop_cancels_the_turn_and_rolls_the_transcript_back() {
        let (started_tx, started_rx) = std::sync::mpsc::channel::<()>();
        let started_tx = Mutex::new(started_tx);
        let f = fixture(move |_| {
            let _ = started_tx.lock().unwrap().send(());
            std::thread::sleep(Duration::from_millis(250));
            Ok(reply("too late"))
        });
        let session = session_with(&f, Arc::new(NoScreen));
        assert!(session.send("long question"));
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        session.stop_session();
        let model = session.snapshot();
        assert!(!model.busy);
        assert!(model.status.starts_with("Session stopped"));
        let model = wait_until(&session, |m| m.items.is_empty());
        assert!(model.items.is_empty());
        std::thread::sleep(Duration::from_millis(400));
        let model = session.snapshot();
        assert!(model.items.is_empty(), "a late reply must not appear");
        assert!(!model.busy);
        // The session works again afterwards.
        assert!(session.send("again"));
    }

    #[test]
    fn provider_errors_restore_the_text_and_show_the_failure() {
        let f = fixture(|_| Ok(HttpResponse::new(401, "application/json", b"{}".to_vec())));
        let session = session_with(&f, Arc::new(NoScreen));
        assert!(session.send("secret question"));
        let model = wait_until(&session, |m| !m.busy && m.state == State::Error);
        assert!(model.items.is_empty());
        assert_eq!(model.failed_serial, model.send_serial);
        assert_eq!(model.failed_text.as_deref(), Some("secret question"));
        assert!(!model.status.is_empty());
    }

    #[test]
    fn clear_forgets_the_history_and_draft() {
        let f = fixture(|_| Ok(reply("ok")));
        let session = session_with(&f, Arc::new(NoScreen));
        session.send("one");
        wait_until(&session, |m| !m.busy && m.items.len() == 2);
        session.clear();
        let model = session.snapshot();
        assert!(model.items.is_empty() && model.draft.is_empty());
        assert_eq!(model.status, "Conversation cleared from memory.");
        session.send("two");
        let model = wait_until(&session, |m| !m.busy && m.items.len() == 2);
        // The provider only saw the new message after the reset.
        let seen = f.seen.lock().unwrap();
        let body = seen.last().unwrap().json().unwrap();
        let roles: Vec<&str> = body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["role"].as_str().unwrap())
            .collect();
        assert_eq!(roles, vec!["system", "user"]);
        assert_eq!(model.items[0].text, "two");
    }

    #[test]
    fn attachment_chips_come_from_files_and_are_sent_once() {
        let f = fixture(|_| Ok(reply("Read it.")));
        let session = session_with(&f, Arc::new(NoScreen));
        let dir = tempfile::tempdir().unwrap();
        let note = dir.path().join("notes.txt");
        std::fs::write(&note, "alpha beta").unwrap();
        let bad = dir.path().join("tool.exe");
        std::fs::write(&bad, "MZ").unwrap();
        session.attach_files(vec![note.display().to_string()]);
        let model = wait_until(&session, |m| !m.importing && m.draft.len() == 1);
        assert_eq!(model.draft[0].file.name, "notes.txt");
        assert!(model.status.contains("sent only when you press Send"));
        session.attach_files(vec![bad.display().to_string()]);
        let model = wait_until(&session, |m| !m.importing && m.state == State::Error);
        assert!(model.status.starts_with("Could not attach file:"));
        assert_eq!(model.draft.len(), 1);

        // Send with no text uses the review prompt; the draft clears after success.
        assert!(session.send(""));
        let model = wait_until(&session, |m| !m.busy && m.items.len() == 2);
        assert_eq!(model.items[0].text, "Please review the attached files.");
        assert_eq!(model.items[0].attachments.len(), 1);
        assert!(model.draft.is_empty());
        let body = f.seen.lock().unwrap()[0].json().unwrap().to_string();
        assert!(body.contains("alpha beta"));

        // Removing a chip works while idle.
        session.attach_files(vec![note.display().to_string()]);
        let model = wait_until(&session, |m| !m.importing && m.draft.len() == 1);
        let id = model.draft[0].id;
        session.remove_attachment(id);
        assert!(session.snapshot().draft.is_empty());
    }

    struct FakeScreen {
        sharing: Mutex<Option<String>>,
        captures: AtomicUsize,
    }

    impl ScreenSource for FakeScreen {
        fn targets(&self) -> Vec<ShareTarget> {
            vec![ShareTarget {
                id: "d:1".into(),
                name: "Main display".into(),
                display: true,
            }]
        }
        fn select(&self, id: &str) -> Result<String, String> {
            *self.sharing.lock().unwrap() = Some(format!("shared {id}"));
            Ok(format!("shared {id}"))
        }
        fn stop(&self) {
            *self.sharing.lock().unwrap() = None;
        }
        fn sharing(&self) -> Option<String> {
            self.sharing.lock().unwrap().clone()
        }
        fn capture(&self, _: &AtomicBool) -> Result<ShareFrame, String> {
            self.captures.fetch_add(1, Ordering::SeqCst);
            let mut png = Vec::new();
            {
                let mut enc = png::Encoder::new(&mut png, 2, 2);
                enc.set_color(png::ColorType::Rgba);
                enc.set_depth(png::BitDepth::Eight);
                enc.write_header()
                    .unwrap()
                    .write_image_data(&[255u8; 16])
                    .unwrap();
            }
            Ok(ShareFrame {
                data_url: naby_core::assistant::attachments::data_url("image/png", &png),
                width: 2,
                height: 2,
            })
        }
    }

    #[test]
    fn sharing_needs_image_input_and_sends_a_frame_with_the_message() {
        let f = fixture(|_| Ok(reply("I see a white square.")));
        let screen = Arc::new(FakeScreen {
            sharing: Mutex::new(None),
            captures: AtomicUsize::new(0),
        });
        let session = session_with(&f, screen.clone());
        session.start_sharing("d:1");
        let model = session.snapshot();
        assert_eq!(model.state, State::Error);
        assert!(model.status.contains("Image input"));
        assert!(model.sharing.is_none());

        let mut provider = f.service.provider().unwrap();
        provider.vision = true;
        let id = provider.id.clone();
        f.service.save_provider(&id, &provider, "").unwrap();
        session.start_sharing("d:1");
        let model = wait_until(&session, |m| {
            m.sharing.is_some() && m.share_preview.is_some()
        });
        assert_eq!(model.sharing.as_deref(), Some("shared d:1"));
        assert!(session.sharing());

        assert!(session.send("what is on screen?"));
        wait_until(&session, |m| !m.busy && m.items.len() == 2);
        let body = f.seen.lock().unwrap()[0].json().unwrap().to_string();
        assert!(body.contains("data:image/png;base64,"));
        assert!(body.contains("Shared image: 2 x 2 pixels."));
        session.stop_sharing();
        assert!(!session.sharing() && session.snapshot().sharing.is_none());
    }

    #[test]
    fn sending_while_disabled_or_empty_does_nothing() {
        let f = fixture(|_| Ok(reply("ok")));
        let session = session_with(&f, Arc::new(NoScreen));
        assert!(!session.send("   "));
        f.service.set_enabled(false).unwrap();
        assert!(!session.send("hello"));
        assert!(session.snapshot().status.contains("Enable AI companion"));
        assert!(f.seen.lock().unwrap().is_empty());
    }

    #[test]
    fn mirror_keeps_unchanged_items_and_hides_internal_messages() {
        let mut shot = Message::user("Updated shared screen after the reviewed action.");
        shot.image_data_url = Some("data:image/png;base64,AA==".into());
        let messages = vec![
            Message::user("one"),
            Message::assistant("two"),
            Message::tool_result("x", "result"),
            shot,
        ];
        let first = mirror(&messages, &[]);
        assert_eq!(first.len(), 2);
        let again = mirror(&messages, &first);
        assert!(Arc::ptr_eq(&first[0], &again[0]) && Arc::ptr_eq(&first[1], &again[1]));
        let changed = mirror(&messages[..1], &first);
        assert_eq!(changed.len(), 1);
    }
}
