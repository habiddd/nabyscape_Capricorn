//! Timer, focus and stopwatch state persisted in the `session` document.
//!
//! The models live in [`crate::productivity`] ([`Timer`], [`FocusSession`], [`Stopwatch`]); this
//! module only joins them into the C# `SessionState`. Deadlines are stored as absolute UTC
//! instants so a running countdown survives a restart or sleep: the remaining time is always
//! recomputed from the wall clock.

use serde::{Deserialize, Serialize};

pub use crate::productivity::timer::{format_seconds, Countdown};
pub use crate::productivity::{Clock, Clocks, FocusSession, Stopwatch, StopwatchState};

use crate::store::{Store, StoreError};

/// Name of the session document.
pub const SESSION_DOCUMENT: &str = "session";

/// The `session` document. JSON keys: `Stopwatch`, `Timer`, `Focus`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase", default)]
pub struct SessionState {
    pub stopwatch: Stopwatch,
    pub timer: Countdown,
    /// Persists exactly like a `Countdown` (the wrapper is transparent).
    pub focus: FocusSession,
}

impl Store {
    /// Loads the session, or a fresh one when none was saved yet.
    pub fn load_session(&self) -> Result<SessionState, StoreError> {
        self.load_or_default(SESSION_DOCUMENT)
    }

    /// Checkpoints the stopwatch against the system clocks (so a running one carries a fresh
    /// anchor) and saves the session.
    pub fn save_session(&self, session: &mut SessionState) -> Result<(), StoreError> {
        self.save_session_at(session, Clock::system())
    }

    /// [`Store::save_session`] with explicit clock readings.
    pub fn save_session_at(
        &self,
        session: &mut SessionState,
        clock: Clock,
    ) -> Result<(), StoreError> {
        session.stopwatch.checkpoint(clock);
        self.save(SESSION_DOCUMENT, session)
    }
}

#[cfg(test)]
mod tests;
