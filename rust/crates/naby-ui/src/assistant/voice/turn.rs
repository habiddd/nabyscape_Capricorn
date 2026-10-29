//! The chat half of a spoken turn (`LocalAssistantVoice` reply callback, `AssistantRuntime`
//! voice transcript handling): the recognized text goes to the selected chat provider on the
//! window's shared conversation with spoken-reply instructions, the reply streams back as text for
//! the sentence splitter, and realtime transcripts and voice action outcomes are recorded in the
//! same history so Chat and Voice share one conversation.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use naby_core::assistant::conversation::{
    realtime_instructions, spoken_instructions, Hooks, SendRequest,
};
use naby_core::assistant::{AssistantError, Call, Message, Role, State};

use crate::assistant::session::ChatSession;

/// A voice view of one [`ChatSession`].
#[derive(Clone)]
pub struct SpokenTurn {
    session: Arc<ChatSession>,
}

impl SpokenTurn {
    pub fn new(session: Arc<ChatSession>) -> Self {
        Self { session }
    }

    pub fn session(&self) -> &Arc<ChatSession> {
        &self.session
    }

    /// The widget tools offered to a voice session (empty when navigation is off).
    pub fn tools(&self) -> Vec<naby_core::assistant::Tool> {
        self.session.tool_runner().tools()
    }

    /// Runs one validated tool call and returns the JSON text for the model.
    pub fn execute_tool(&self, call: &Call) -> String {
        self.session.tool_runner().execute(call)
    }

    /// A fresh frame of the shared window or display as an image data URL, when sharing.
    pub fn shared_frame(&self, cancel: &AtomicBool) -> Option<String> {
        let screen = self.session.screen_source();
        screen.sharing()?;
        screen.capture(cancel).ok().map(|f| f.data_url)
    }

    /// The name of the shared source.
    pub fn sharing(&self) -> Option<String> {
        self.session.screen_source().sharing()
    }

    /// Instructions for a realtime session (`StartVoice`).
    pub fn realtime_instructions(&self) -> String {
        realtime_instructions(&self.session.service().options().instructions)
    }

    /// The last 80 spoken-relevant history messages used to seed a realtime session.
    pub fn history(&self) -> Vec<Message> {
        self.session.with_conversation(|c| {
            let all: Vec<Message> = c
                .messages()
                .iter()
                .filter(|m| {
                    matches!(m.role, Role::User | Role::Assistant) && !m.text.trim().is_empty()
                })
                .cloned()
                .collect();
            let skip = all.len().saturating_sub(80);
            all.into_iter().skip(skip).collect()
        })
    }

    /// Records one finished line from a realtime session (ignored while a chat turn runs).
    pub fn record_line(&self, user: bool, text: &str) {
        self.session
            .with_conversation(|c| c.record(if user { Role::User } else { Role::Assistant }, text));
        self.session.mirror_conversation();
    }

    /// Records the outcome of a voice-triggered action as bounded plain text.
    pub fn record_outcome(&self, generation: u64, call: &Call, result: &str) {
        self.session
            .with_conversation(|c| c.record_voice_outcome(generation, call, result));
        self.session.mirror_conversation();
    }

    /// The conversation epoch (changes when chat is cleared).
    pub fn epoch(&self) -> u64 {
        self.session.conversation_control().epoch()
    }

    /// Stops a running reply (Stop, End conversation).
    pub fn stop(&self) {
        self.session.conversation_control().stop();
    }

    /// Reflects the voice phase in the chat model (busy blocks chat sends while talking).
    pub fn set_phase(&self, phase: State, status: &str, busy: bool) {
        self.session.voice_edit(|m| {
            m.state = phase;
            m.status = status.to_owned();
            m.busy = busy;
            if !busy {
                m.live = None;
            }
        });
    }

    /// Sends `text` and feeds the streamed reply to `on_text` (return false to stop). Blocking:
    /// call from a worker thread. The conversation records the user line and the reply.
    pub fn reply(
        &self,
        text: &str,
        on_text: &mut dyn FnMut(&str) -> bool,
        cancel: &Arc<AtomicBool>,
    ) -> Result<(), String> {
        let result = self.turn(text, on_text, cancel);
        self.session.mirror_conversation();
        match result {
            Ok(()) => {
                if let Some(provider) = self.session.service().provider() {
                    self.session.service().record_check(
                        &provider,
                        true,
                        &format!(
                            "{} · {} completed a chat reply.",
                            provider.name, provider.model
                        ),
                    );
                }
                Ok(())
            }
            Err(AssistantError::Cancelled) => Ok(()),
            Err(e) => Err(e.message().to_owned()),
        }
    }

    fn turn(
        &self,
        text: &str,
        on_text: &mut dyn FnMut(&str) -> bool,
        cancel: &Arc<AtomicBool>,
    ) -> Result<(), AssistantError> {
        let service = self.session.service();
        let mut provider = service
            .provider()
            .ok_or_else(|| AssistantError::invalid("Add and select a provider first."))?;
        let options = service.options();
        let runner = self.session.tool_runner();
        let tools = if provider.is_hermes() {
            Vec::new()
        } else {
            runner.tools()
        };
        provider.tools = !tools.is_empty();
        let key = service.key_for(&provider.id);
        let screen = self.session.screen_source();
        let capture = |cancel: &AtomicBool| -> Result<Option<String>, AssistantError> {
            if screen.sharing().is_none() {
                return Ok(None);
            }
            screen
                .capture(cancel)
                .map(|f| Some(f.data_url))
                .map_err(AssistantError::io)
        };
        let image = if provider.vision {
            capture(cancel)?
        } else {
            None
        };
        let instructions = spoken_instructions(&options.instructions, &options.local_voice);
        let transport = service.transport();
        let mut execute = |call: &Call, _: &AtomicBool| -> naby_core::assistant::Result<String> {
            Ok(runner.execute(call))
        };
        let mut refresh = |cancel: &AtomicBool| -> naby_core::assistant::Result<Option<String>> {
            capture(cancel)
        };
        let mut sink = |delta: &str| -> naby_core::assistant::Result<()> {
            if cancel.load(Ordering::SeqCst) || !on_text(delta) {
                return Err(AssistantError::Cancelled);
            }
            Ok(())
        };
        let mut hooks = Hooks {
            execute: &mut execute,
            refresh_screen: if provider.vision {
                Some(&mut refresh)
            } else {
                None
            },
            voice_text: Some(&mut sink),
            voice_progress: None,
        };
        let mut request = SendRequest::new(&provider, &instructions, text);
        request.key = key.as_deref();
        request.image = image;
        request.tools = &tools;
        request.cancel = Some(cancel.clone());
        self.session
            .with_conversation(|c| c.send(&transport, request, &mut hooks))
    }
}
