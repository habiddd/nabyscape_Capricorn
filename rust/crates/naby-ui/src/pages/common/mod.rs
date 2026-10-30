//! Shared building blocks of the media and status pages: a scaled immediate-mode control set
//! (`Ctl`), a text field, cross-fades, cover art.

pub mod art;
pub mod ctl;
pub mod edit;
pub mod fade;
#[cfg(test)]
pub mod harness;
#[cfg(test)]
pub mod overlay_rig;
#[cfg(test)]
mod panel_tests;

pub use art::ArtCache;
pub use ctl::{Ctl, Interaction};
pub use edit::EditOpts;
pub use fade::Crossfade;
