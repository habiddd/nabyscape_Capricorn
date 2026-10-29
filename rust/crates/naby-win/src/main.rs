//! NabyScape for Windows.
#![cfg_attr(windows, windows_subsystem = "windows")]

pub mod wake;

// Host services (task 12). Each keeps its pure helpers outside `cfg(windows)` for Linux tests.
pub mod battery;
pub mod capture;
pub mod clipboard;
pub mod downloads;
pub mod dragdrop;
pub mod launcher;
#[cfg(windows)]
pub mod svc_util;
pub mod system;

#[cfg(windows)]
fn main() {}

#[cfg(not(windows))]
fn main() {
    eprintln!("NabyScape runs on Windows 10 (19041) or later; build with --target x86_64-pc-windows-gnu or -msvc.");
    std::process::exit(1);
}
