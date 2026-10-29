//! Voice views and the platform-free voice rules (`AssistantVoiceView`, `AssistantVoiceOverlay`,
//! `AssistantVoiceWorkspace`, the engine choice).
//!
//! * [`room`]: the voice room and its compact overlay form.
//! * [`settings`]: the AI settings voice pages (Voice setup, Wake word, Models, Check audio),
//!   exposed to the Settings window as `naby_ui::assistant::VoiceSettingsView`.
//! * [`engine`]: which engine listens and speaks (Whistle by default).
//! * [`model`]: the snapshot and host trait that connect the views to the Windows voice runtime.
//! * [`turn`]: the chat half of a spoken turn on the shared conversation.

pub mod engine;
pub mod model;
pub mod room;
pub mod settings;
pub mod turn;

pub use engine::{Engines, Listening, Speaking};
pub use model::{
    AudioCheck, AudioCheckKind, InstalledModel, MicChoice, MicTest, NoVoice, Recognizer,
    SpeechPanel, VoiceHost, VoiceLine, VoiceSnapshot, WakeStatus,
};
pub use room::{VoiceEnv, VoiceOut, VoiceRoom};
pub use settings::{VoicePage, VoiceSettingsOut, VoiceSettingsView};
pub use turn::SpokenTurn;

#[cfg(test)]
mod tests;
