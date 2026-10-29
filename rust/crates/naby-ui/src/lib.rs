//! Platform-independent NabyScape views: overlay dock, settings, welcome and assistant.
//!
//! Views consume snapshots from `naby_core`, paint with `naby_render` and emit
//! `naby_core::command::Command`s; the Windows host owns every OS handle.
