//! Countdown completion sounds (`CountdownAudio.cs`).
//!
//! The built-in bell/chime are synthesized (see `tone.rs`) into an in-memory WAV, scaled by
//! the configured volume, and played with `PlaySound` on a short-lived thread, so the sound
//! follows the default output but not the Windows notification volume. Nothing is kept
//! between sounds: the WAV image (about 88 KB) is freed when playback ends.
#![allow(dead_code)] // consumed by the app shell, which is wired separately

use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use windows::core::PCWSTR;
use windows::Win32::Media::Audio::{PlaySoundW, SND_MEMORY, SND_NODEFAULT, SND_PURGE, SND_SYNC};

use crate::tone::{self, CountdownSound, Plan};
use crate::wake::Waker;

const PLAY_FAILED: &str = "Could not play the completion sound on the current output.";

/// The three countdown settings, as stored in `Settings`.
#[derive(Debug, Clone, PartialEq)]
pub struct CountdownSoundConfig {
    /// "Off", "Bell", "Chime" or "Custom".
    pub sound: String,
    /// 0..=1.
    pub volume: f64,
    /// WAV path used by "Custom".
    pub custom_file: String,
}

pub struct SoundService {
    wake: Waker,
    current: Mutex<Option<JoinHandle<()>>>,
    last_error: Arc<Mutex<Option<String>>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl SoundService {
    pub fn start(wake: Waker) -> Self {
        SoundService {
            wake,
            current: Mutex::new(None),
            last_error: Arc::new(Mutex::new(None)),
        }
    }

    /// Failure of the last `play`, with the built-in fallback noted for broken custom files.
    pub fn last_error(&self) -> Option<String> {
        lock(&self.last_error).clone()
    }

    /// Stops any running sound and plays the configured one (nothing for "Off" or volume 0).
    pub fn play(&self, config: &CountdownSoundConfig) {
        self.stop();
        *lock(&self.last_error) = None;
        let plan = tone::plan(&config.sound, config.volume, &config.custom_file);
        if plan == Plan::Silent {
            return;
        }
        let volume = config.volume.clamp(0.0, 1.0) as f32;
        let error = Arc::clone(&self.last_error);
        let wake = Arc::clone(&self.wake);
        let spawned = std::thread::Builder::new()
            .name("naby-sound".into())
            .spawn(move || {
                let (wav, note) = match plan {
                    Plan::Tone(kind) => (tone::tone_wav(kind, volume), None),
                    Plan::Custom(path) => match load_custom(&path, volume) {
                        Ok(wav) => (wav, None),
                        Err(e) => (
                            tone::tone_wav(CountdownSound::Chime, volume),
                            Some(format!("{e} Used the built-in chime instead.")),
                        ),
                    },
                    Plan::Silent => return,
                };
                if let Some(note) = note {
                    *lock(&error) = Some(note);
                    wake();
                }
                if !play_blocking(&wav) {
                    let mut e = lock(&error);
                    if e.is_none() {
                        *e = Some(PLAY_FAILED.to_owned());
                        drop(e);
                        wake();
                    }
                }
            });
        if let Ok(handle) = spawned {
            *lock(&self.current) = Some(handle);
        }
    }

    /// Cuts the current sound off.
    pub fn stop(&self) {
        let handle = lock(&self.current).take();
        if let Some(handle) = handle {
            // SAFETY: SND_PURGE with a null name stops waveform sounds started by this process.
            unsafe {
                let _ = PlaySoundW(PCWSTR::null(), None, SND_PURGE);
            }
            let _ = handle.join();
        }
    }
}

impl Drop for SoundService {
    fn drop(&mut self) {
        self.stop();
    }
}

fn load_custom(path: &str, volume: f32) -> Result<Vec<u8>, String> {
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    tone::custom_file_acceptable(path, meta.len()).map_err(|e| e.to_string())?;
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    tone::scale_wav(&bytes, volume).map_err(|e| e.to_string())
}

fn play_blocking(wav: &[u8]) -> bool {
    // SAFETY: SND_MEMORY treats the pointer as a WAV image; SND_SYNC keeps the call (and
    // therefore the borrow of `wav`) alive until playback has finished or was purged.
    unsafe {
        PlaySoundW(
            PCWSTR(wav.as_ptr().cast()),
            None,
            SND_MEMORY | SND_SYNC | SND_NODEFAULT,
        )
        .as_bool()
    }
}
