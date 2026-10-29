//! NabyScape for Windows.
#![cfg_attr(windows, windows_subsystem = "windows")]

pub mod wake;

#[cfg(windows)]
fn main() {}

#[cfg(not(windows))]
fn main() {
    eprintln!("NabyScape runs on Windows 10 (19041) or later; build with --target x86_64-pc-windows-gnu or -msvc.");
    std::process::exit(1);
}
