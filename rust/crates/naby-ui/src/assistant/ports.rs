//! What the assistant needs from the host, as small thread-safe traits. The Windows host
//! implements them over screen capture, Office text extraction and the widget command queue;
//! tests implement them with fakes.

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use naby_core::assistant::attachments::bytes_from_data_url;
use naby_core::assistant::{Attachment, AttachmentSource, Call, Tool};
use naby_core::command::Command;

use super::service::{AssistantService, Waker};

/// The widget tools offered to the model and how a call runs.
pub trait ToolRunner: Send + Sync {
    /// Tool definitions for the next request (empty when navigation is off).
    fn tools(&self) -> Vec<Tool>;
    /// Runs one validated-by-the-runner call and returns the JSON text for the model.
    fn execute(&self, call: &Call) -> String;
}

/// A tool runner that offers nothing.
pub struct NoTools;

impl ToolRunner for NoTools {
    fn tools(&self) -> Vec<Tool> {
        Vec::new()
    }
    fn execute(&self, _: &Call) -> String {
        r#"{"ok":false,"error":"This action is unavailable in the current session."}"#.into()
    }
}

/// One window or display that can be shared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShareTarget {
    pub id: String,
    pub name: String,
    pub display: bool,
}

/// One encoded frame of the shared source.
#[derive(Debug, Clone)]
pub struct ShareFrame {
    pub data_url: String,
    pub width: u32,
    pub height: u32,
}

/// Screen sharing source (the picker list and frame capture).
pub trait ScreenSource: Send + Sync {
    fn targets(&self) -> Vec<ShareTarget>;
    /// Starts sharing `id`; returns its display name.
    fn select(&self, id: &str) -> Result<String, String>;
    fn stop(&self);
    fn sharing(&self) -> Option<String>;
    /// Whether the shared source still exists.
    fn alive(&self) -> bool {
        true
    }
    /// Captures one frame of the shared source (blocking).
    fn capture(&self, cancel: &AtomicBool) -> Result<ShareFrame, String>;
}

/// No capture available (no sources, nothing shared).
pub struct NoScreen;

impl ScreenSource for NoScreen {
    fn targets(&self) -> Vec<ShareTarget> {
        Vec::new()
    }
    fn select(&self, _: &str) -> Result<String, String> {
        Err("Screen sharing is not available.".into())
    }
    fn stop(&self) {}
    fn sharing(&self) -> Option<String> {
        None
    }
    fn capture(&self, _: &AtomicBool) -> Result<ShareFrame, String> {
        Err("Choose a window or display to share.".into())
    }
}

/// Reads and converts attachments (runs on a worker thread).
pub trait AttachmentPreparer: Send + Sync {
    fn prepare_file(&self, path: &Path) -> Result<Attachment, String>;
    /// A clipboard image (encoded PNG bytes).
    fn prepare_image(&self, bytes: &[u8]) -> Result<Attachment, String>;
}

/// The platform-free rules of `Attachment::from_file`; images stay as they are when already
/// PNG/JPEG within the picture limit.
pub struct BasicPreparer;

impl AttachmentPreparer for BasicPreparer {
    fn prepare_file(&self, path: &Path) -> Result<Attachment, String> {
        Attachment::from_file(path).map_err(|e| e.message().to_owned())
    }

    fn prepare_image(&self, bytes: &[u8]) -> Result<Attachment, String> {
        Attachment::inline_image("Pasted image.png", "image/png", bytes, "Pasted image")
            .map_err(|e| e.message().to_owned())
    }
}

/// Reads the bytes behind an attachment (inline data, a file, or a provider link).
pub trait MediaLoader: Send + Sync {
    fn load(&self, file: &Attachment, cancel: &AtomicBool) -> Result<Vec<u8>, String>;
}

/// Media loading through the service: provider-returned links are fetched with the current
/// provider's credentials when the origin matches (`AssistantRuntime.Media`).
pub struct ServiceMedia {
    service: AssistantService,
}

impl ServiceMedia {
    pub fn new(service: AssistantService) -> Self {
        Self { service }
    }
}

impl MediaLoader for ServiceMedia {
    fn load(&self, file: &Attachment, cancel: &AtomicBool) -> Result<Vec<u8>, String> {
        match &file.source {
            AttachmentSource::Inline(source) => {
                bytes_from_data_url(source).map_err(|e| e.message().to_owned())
            }
            AttachmentSource::Path { .. } => file.bytes().map_err(|e| e.message().to_owned()),
            AttachmentSource::Remote(_) => {
                let options = self.service.options();
                let provider = options
                    .providers
                    .iter()
                    .find(|p| p.id == file.provider_id && p.base_url == file.origin);
                let key = provider.and_then(|p| self.service.key_for(&p.id));
                self.service
                    .transport()
                    .download_media(file, provider, key.as_deref(), cancel)
                    .map_err(|e| e.message().to_owned())
            }
            AttachmentSource::None => Err("There is nothing to load.".into()),
        }
    }
}

/// Commands a worker thread wants the UI thread to run (widget navigation). The host drains the
/// queue when the waker fires.
#[derive(Clone)]
pub struct CommandQueue {
    queue: Arc<Mutex<Vec<Command>>>,
    waker: Waker,
}

impl CommandQueue {
    pub fn new(waker: Waker) -> Self {
        Self {
            queue: Arc::default(),
            waker,
        }
    }

    pub fn push(&self, command: Command) {
        self.queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(command);
        (self.waker)();
    }

    pub fn drain(&self) -> Vec<Command> {
        std::mem::take(&mut *self.queue.lock().unwrap_or_else(|e| e.into_inner()))
    }
}
