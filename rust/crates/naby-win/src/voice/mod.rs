//! Voice.
pub mod dsp;
pub mod install;
pub mod local;
pub mod pipeline;
pub mod playback;
pub mod realtime;
pub mod wake_logic;
pub mod wake_whistle;
pub mod whistle;

#[cfg(windows)]
pub mod capture;
#[cfg(windows)]
pub mod runtime;
#[cfg(windows)]
pub mod wake;
#[cfg(windows)]
pub mod worker;

#[cfg(windows)]
pub use runtime::{Request, VoiceBridge, VoiceRuntime};
