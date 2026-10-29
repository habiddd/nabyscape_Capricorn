//! Platform-independent NabyScape logic: preferences, persistence and feature models.
//!
//! Nothing in this crate touches Win32, so it is unit-tested on any host.

pub mod assistant;
pub mod color;
pub mod command;
pub mod downloads;
pub mod http;
pub mod i18n;
pub mod media;
pub mod motion;
pub mod notifications;
pub mod paths;
pub mod productivity;
pub mod readiness;
pub mod session;
pub mod settings;
pub mod store;
pub mod system;
pub mod updates;
pub mod weather;
