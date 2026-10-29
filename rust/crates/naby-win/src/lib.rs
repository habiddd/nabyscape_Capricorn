//! NabyScape Windows host: Win32 shell, windows, tray, hotkeys and system services.
//!
//! Everything that touches the OS is `#[cfg(windows)]`; the pure rules around it (hotkey
//! parsing, monitor selection and dock placement, shortcut validation, input folding, frame
//! scheduling) compile and are tested on every platform.

pub mod app;
pub mod assistant_host;
pub mod awake;
pub mod clipboard;
pub mod credentials;
pub mod detached_host;
pub mod dialogs;
pub mod event;
pub mod fullscreen;
pub mod hotkeys;
pub mod instance;
pub mod monitors;
pub mod office_text;
pub mod outside_click;
pub mod overlay_host;
pub mod shell;
pub mod startup;
pub mod text_clipboard;
pub mod timer;
pub mod tray;
pub mod util;
pub mod voice;
pub mod wake;
pub mod widget_bridge;
pub mod window;

// Media and audio services. Pure helpers build everywhere; OS services are Windows-only.
#[cfg(windows)]
pub mod audio;
pub mod audio_logic;
#[cfg(windows)]
pub mod brightness;
#[cfg(windows)]
pub mod devices;
#[cfg(windows)]
pub mod loopback;
#[cfg(windows)]
pub mod media;
pub mod media_logic;
pub mod microphones;
#[cfg(windows)]
pub mod mixer;
#[cfg(windows)]
pub mod sounds;
pub mod tone;

// System, clipboard history, downloads, launcher, drag-drop and capture services.
pub mod attachment_prep;
pub mod battery;
pub mod capture;
pub mod downloads;
pub mod dragdrop;
pub mod launcher;
#[cfg(windows)]
pub mod svc_util;
pub mod system;
