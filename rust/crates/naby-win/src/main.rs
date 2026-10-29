//! NabyScape for Windows.
#![cfg_attr(windows, windows_subsystem = "windows")]

pub mod wake;

// Media and audio services (Task 11). Pure helpers build everywhere; OS services are Windows-only.
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

#[cfg(windows)]
fn main() {}

#[cfg(not(windows))]
fn main() {
    eprintln!("NabyScape runs on Windows 10 (19041) or later; build with --target x86_64-pc-windows-gnu or -msvc.");
    std::process::exit(1);
}
