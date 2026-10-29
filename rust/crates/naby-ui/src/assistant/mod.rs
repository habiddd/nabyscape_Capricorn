//! The assistant (Naby): chat window, orb, provider settings and island pieces.
pub mod blocks;
pub mod chat;
pub mod island;
pub mod orb;
pub mod ports;
pub mod previews;
pub mod providers;
pub mod service;
pub mod session;
pub mod setup;
pub mod style;
pub mod transcript;
pub mod voice;
pub mod widget_tools;
pub mod window;

pub use voice::VoiceSettingsView;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_cost;
#[cfg(test)]
mod tests_setup;
#[cfg(test)]
mod tests_stream;
