//! Assistant (Naby) protocol, provider adapters and conversation state.
//!
//! Ported from `Features/Assistant/*` of the C# sources; see
//! docs/superpowers/plans/2026-10-29-rust-port.md (Task 8).
//!
//! Everything here is platform-free: requests are built as plain data
//! ([`transport::HttpRequest`]), responses are parsed from bytes, and the only
//! blocking I/O is the [`transport::UreqExecutor`], which callers run on a worker
//! thread with a cancellation flag. The host owns the microphone, screen
//! capture, Windows Credential Manager and the realtime WebSocket.

pub mod attachments;
pub mod conversation;
pub mod hermes;
pub mod markdown;
pub mod media;
pub mod models;
pub mod persistence;
pub mod providers;
pub mod readiness;
pub mod speech;
pub mod sse;
pub mod storage;
pub mod tools;
pub mod transport;
pub mod voice;

use std::fmt;

pub use attachments::{Attachment, AttachmentSource};
pub use conversation::{Conversation, ConversationControl};
pub use models::{
    Call, LocalVoiceOptions, Message, Options, Protocol, Provider, Reply, Role, State, Tool,
    WakeOptions,
};
pub use sse::{SseParser, StreamEvent};
pub use transport::{HttpExecutor, HttpRequest, HttpResponse, Transport};

/// Failure categories, mirroring the exception types the C# code throws.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssistantError {
    /// Configuration or input rejected before any network use (`InvalidOperationException`).
    Invalid(String),
    /// Provider, network or storage failure with a user-safe message (`IOException`).
    Io(String),
    /// The request exceeded its deadline (`TimeoutException`).
    Timeout(String),
    /// The caller cancelled the work (`OperationCanceledException`).
    Cancelled,
}

impl AssistantError {
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid(message.into())
    }
    pub fn io(message: impl Into<String>) -> Self {
        Self::Io(message.into())
    }
    /// The user-facing text, without a category prefix.
    pub fn message(&self) -> &str {
        match self {
            Self::Invalid(m) | Self::Io(m) | Self::Timeout(m) => m,
            Self::Cancelled => "The operation was cancelled.",
        }
    }
}

impl fmt::Display for AssistantError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for AssistantError {}

pub type Result<T> = std::result::Result<T, AssistantError>;

/// UTF-16 length, matching C# `string.Length` for the character limits.
pub(crate) fn len16(text: &str) -> usize {
    if text.is_ascii() {
        text.len()
    } else {
        text.encode_utf16().count()
    }
}

/// Truncate to at most `max` UTF-16 units without splitting a scalar value.
pub(crate) fn take16(text: &str, max: usize) -> &str {
    if text.is_ascii() {
        return &text[..text.len().min(max)];
    }
    let mut units = 0;
    for (index, ch) in text.char_indices() {
        units += ch.len_utf16();
        if units > max {
            return &text[..index];
        }
    }
    text
}

/// A fresh 32-digit lowercase hex identity, like `Guid.NewGuid().ToString("N")`.
pub fn new_id() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut out = String::with_capacity(32);
    for round in 0..2u64 {
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
        hasher.write_u64(round);
        if let Ok(now) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
            hasher.write_u128(now.as_nanos());
        }
        out.push_str(&format!("{:016x}", hasher.finish()));
    }
    out
}

/// True for a 32-digit hex identity (`Guid.TryParseExact(id, "N")`).
pub fn is_id(value: &str) -> bool {
    value.len() == 32 && value.bytes().all(|b| b.is_ascii_hexdigit())
}
