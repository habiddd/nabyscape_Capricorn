//! Wake detection with Whistle, used when Whistle is the listening engine and is installed. The
//! microphone stays open and the existing energy detector cuts speech into segments; only a
//! segment (at most [`SEGMENT`] of audio) is transcribed, one at a time, so Whistle runs only when
//! it hears speech. Speech that arrives during a transcription is dropped, never queued, which
//! bounds the work under continuous audio such as music. Audio is never stored or uploaded and
//! transcripts are never logged.
//!
//! Same surface as [`super::wake::WakeListener`]. The confidence setting does not apply here.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use naby_core::assistant::voice::{wake_heard, wake_phrase, DEFAULT_WAKE_PHRASE};
use naby_core::assistant::Options;
use naby_ui::assistant::voice::engine;

use super::capture::Capture;
use super::pipeline::{Collected, Collector};
use super::whistle::{Transcriber, Whistle};

/// The longest segment sent to Whistle (the start of a longer one).
const SEGMENT: usize = 4 * 16_000;
/// Transcription errors in a row before listening stops with a failure.
const MAXIMUM_ERRORS: u32 = 3;

pub struct WhistleWake {
    capture: Option<Capture>,
    worker: Option<JoinHandle<()>>,
    live: Arc<AtomicBool>,
    shown: Vec<String>,
}

impl WhistleWake {
    /// Opens the microphone and Whistle (blocking: call from a worker thread). `detected` fires
    /// once; `failed` fires once if the microphone or Whistle stops working.
    pub fn start(
        whistle: &Whistle,
        options: &Options,
        detected: impl Fn() + Send + Sync + 'static,
        failed: impl Fn(String) + Send + Sync + 'static,
    ) -> Result<WhistleWake, String> {
        let phrase = wake_phrase(&options.wake_word.phrase).map_err(|e| e.message().to_owned())?;
        let local = &options.local_voice;
        let mut lease = whistle.acquire().map_err(|e| e.message())?;
        let keywords = engine::keywords(options);
        let live = Arc::new(AtomicBool::new(true));
        let busy = Arc::new(AtomicBool::new(false));
        let failed = Arc::new(failed);
        let (segments, inbox) = channel::<Vec<f32>>();

        let (flag, once, idle, report) = (
            live.clone(),
            AtomicBool::new(false),
            busy.clone(),
            failed.clone(),
        );
        let heard = phrase.clone();
        let worker = std::thread::Builder::new()
            .name("NabyScape wake whistle".into())
            .spawn(move || {
                let mut errors = 0;
                for audio in inbox {
                    match lease.transcribe(&audio, None, &keywords) {
                        Ok(t) => {
                            errors = 0;
                            if flag.load(Ordering::SeqCst)
                                && wake_heard(&heard, &t.text)
                                && !once.swap(true, Ordering::SeqCst)
                            {
                                flag.store(false, Ordering::SeqCst);
                                detected();
                            }
                        }
                        Err(_) => {
                            errors += 1;
                            if errors >= MAXIMUM_ERRORS && flag.swap(false, Ordering::SeqCst) {
                                report("Whistle stopped working while listening for the wake phrase. Resume wake listening or check Whistle in Models.".into());
                            }
                        }
                    }
                    idle.store(false, Ordering::SeqCst);
                }
            })
            .map_err(|e| e.to_string())?;

        let collector = Mutex::new(Collector::new(local.sensitivity, local.silence_ms));
        let (flag, idle) = (live.clone(), busy);
        let on_audio = move |chunk: &[f32]| {
            let mut collector = collector.lock().unwrap_or_else(|e| e.into_inner());
            if idle.load(Ordering::SeqCst) || !flag.load(Ordering::SeqCst) {
                collector.reset();
                return;
            }
            if let Collected::Utterance(mut audio) = collector.push(chunk) {
                audio.truncate(SEGMENT);
                idle.store(true, Ordering::SeqCst);
                collector.reset();
                let _ = segments.send(audio);
            }
        };
        let flag = live.clone();
        let capture = Capture::start(&options.microphone_id, 16_000, on_audio, move |m| {
            if flag.swap(false, Ordering::SeqCst) {
                failed(format!(
                    "The wake microphone stopped. {m} Check Windows sound settings, then resume wake listening."
                ));
            }
        });
        let capture = match capture {
            Ok(c) => c,
            Err(e) => {
                // The closure that owned the sender is gone, so the worker ends and frees Whistle.
                let _ = worker.join();
                return Err(e);
            }
        };
        let shown = if phrase.eq_ignore_ascii_case(DEFAULT_WAKE_PHRASE) {
            vec![DEFAULT_WAKE_PHRASE.to_owned(), "Oye Naby".to_owned()]
        } else {
            vec![phrase]
        };
        Ok(WhistleWake {
            capture: Some(capture),
            worker: Some(worker),
            live,
            shown,
        })
    }

    pub fn shown(&self) -> &[String] {
        &self.shown
    }

    pub fn listening(&self) -> bool {
        self.live.load(Ordering::SeqCst)
    }

    /// Stops listening and releases the microphone and Whistle.
    pub fn stop(&mut self) {
        self.live.store(false, Ordering::SeqCst);
        if let Some(mut capture) = self.capture.take() {
            capture.stop();
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for WhistleWake {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::time::{Duration, Instant};

    fn whistle() -> Whistle {
        let folder = std::path::PathBuf::from(std::env::var("NABY_WHISTLE_DIR").unwrap());
        // `NABY_WHISTLE_DIR` is the needle.exe folder; the pip runtime sits beside it.
        Whistle::native(folder, || engine::WhistleRuntime::Exe)
    }

    /// Real engine, no microphone: each clip in `NABY_WAKE_CLIPS` (`;`-separated 16 kHz mono
    /// 16-bit WAVs) must be heard as the default wake phrase.
    #[test]
    #[ignore = "needs the Whistle files; set NABY_WHISTLE_DIR and NABY_WAKE_CLIPS"]
    fn whistle_hears_the_wake_clips() {
        let mut lease = whistle().acquire().unwrap();
        let keywords = engine::keywords(&Options::default());
        let mut missed = Vec::new();
        for clip in std::env::var("NABY_WAKE_CLIPS").unwrap().split(';') {
            let wav = std::fs::read(clip).unwrap();
            let pcm: Vec<f32> = wav[44..]
                .chunks_exact(2)
                .map(|c| f32::from(i16::from_le_bytes([c[0], c[1]])) / 32768.0)
                .collect();
            let started = Instant::now();
            let heard = lease.transcribe(&pcm, None, &keywords).unwrap();
            println!(
                "{clip}: {:?} in {:.2} s",
                heard.text,
                started.elapsed().as_secs_f64()
            );
            if !wake_heard(DEFAULT_WAKE_PHRASE, &heard.text) {
                missed.push(clip.to_owned());
            }
        }
        assert!(
            missed.is_empty(),
            "not heard as the wake phrase: {missed:?}"
        );
    }

    /// Real microphone: listens for `NABY_WAKE_SECS` (default 2) and reports detections; play a
    /// clip through the speakers meanwhile. Spoken detection through a loop may legitimately miss.
    #[test]
    #[ignore = "needs the Whistle files and a microphone"]
    fn whistle_listens_on_the_microphone() {
        let secs = std::env::var("NABY_WAKE_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(2);
        let hits = Arc::new(AtomicUsize::new(0));
        let failures = Arc::new(Mutex::new(Vec::new()));
        let (h, f) = (hits.clone(), failures.clone());
        let listener = WhistleWake::start(
            &whistle(),
            &Options::default(),
            move || {
                h.fetch_add(1, Ordering::SeqCst);
            },
            move |m| f.lock().unwrap().push(m),
        )
        .unwrap();
        std::thread::sleep(Duration::from_secs(secs));
        println!(
            "phrases: {:?} detections: {} listening: {} failures: {:?}",
            listener.shown(),
            hits.load(Ordering::SeqCst),
            listener.listening(),
            failures.lock().unwrap()
        );
        assert!(failures.lock().unwrap().is_empty());
    }
}
