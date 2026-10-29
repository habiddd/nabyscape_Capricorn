#![cfg_attr(test, allow(clippy::field_reassign_with_default))]
//! Platform-independent NabyScape views: overlay dock, settings, welcome and assistant.
//!
//! Views consume snapshots from `naby_core`, paint with `naby_render` and emit
//! `naby_core::command::Command`s; the Windows host owns every OS handle.
//!
//! * [`host`]: [`host::Snapshots`], the [`host::View`] trait and the host-facing types.
//! * [`overlay`]: the dock ([`overlay::Overlay`]).
//! * [`pages`]: widget pages of the open panel; one file per widget id.
//! * [`settings`]: the Settings window content ([`settings::SettingsView`]).
//! * [`welcome`]: the first-run setup guide ([`welcome::WelcomeView`]).

pub mod host;
pub mod overlay;
pub mod pages;
pub mod settings;
pub mod welcome;

pub use host::{Demand, Signal, Snapshots, View};
pub use overlay::{Overlay, OverlayState};
