//! NabyScape Windows host: Win32 shell, windows, tray, hotkeys and system services.
//!
//! Everything that touches the OS is `#[cfg(windows)]`; the pure rules around it (hotkey
//! parsing, monitor selection and dock placement, shortcut validation, input folding, frame
//! scheduling) compile and are tested on every platform.

pub mod app;
pub mod awake;
pub mod clipboard;
pub mod credentials;
pub mod event;
pub mod fullscreen;
pub mod hotkeys;
pub mod instance;
pub mod monitors;
pub mod placeholder;
pub mod shell;
pub mod startup;
pub mod timer;
pub mod tray;
pub mod util;
pub mod wake;
pub mod window;
