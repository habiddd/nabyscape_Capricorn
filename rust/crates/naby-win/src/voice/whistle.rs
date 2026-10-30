//! Whistle, the built-in listening model: Cactus Compute `whistle.cact` run by the Cactus
//! Needle engine `needle.exe` (user decision 2026-10-29). Cactus publishes no Windows DLL, only
//! `needle.exe` and a GCC static library, so each utterance runs `needle.exe --audio <wav>`.
//!
//! * The two files sit next to `nabyscape.exe` (Setup / Portable) or in
//!   `%LOCALAPPDATA%\NabyScape\local-speech\whistle`, where Settings > Models installs them.
//! * Both files are checked against SHA-256 values pinned in `rust/whistle.lock`, which is
//!   embedded in the executable. A lock that still says `TO_BE_PINNED` refuses to load unless the
//!   build has debug assertions.
//! * One worker thread owns the engine. The thread starts when voice starts and stops 60 s after
//!   the last voice session ended. There is no always-on Whistle loop: wake listening uses Windows
//!   speech recognition.
//!
//! Everything except the process launch in [`native`] is platform-free and tested on Linux.

use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Deserialize;
use sha2::{Digest, Sha256};

/// The lock file, embedded so a swapped file next to the exe cannot change what is trusted.
pub const EMBEDDED_LOCK: &str = include_str!("../../../../whistle.lock");
/// Placeholder in `whistle.lock` until the release process pins real hashes.
pub const UNPINNED: &str = "TO_BE_PINNED";
/// The model file and engine program names.
pub const MODEL_FILE: &str = "whistle.cact";
pub const ENGINE_FILE: &str = "needle.exe";
/// The folder under `local-speech` that Settings installs Whistle into.
pub const INSTALL_FOLDER: &str = "whistle";
/// Whistle's input rate.
pub const SAMPLE_RATE: u32 = 16_000;
/// Idle time after voice ended before the engine worker stops.
pub const IDLE_UNLOAD: Duration = Duration::from_secs(60);
/// Output budget per second of audio for the result buffer. The Python wrapper sizes the buffer
/// from the clip length; this is a generous constant (transcripts are a few hundred bytes a
/// second) with a floor so short clips with `words` JSON always fit.
pub const OUTPUT_BYTES_PER_SECOND: usize = 4096;
pub const OUTPUT_FLOOR: usize = 256 * 1024;
/// A hard cap on one utterance (the VAD cuts at 25 s already).
pub const MAXIMUM_SAMPLES: usize = SAMPLE_RATE as usize * 60;
/// Keyword list bounds.
pub const MAXIMUM_KEYWORDS: usize = 32;
pub const MAXIMUM_KEYWORD_CHARS: usize = 64;

/// Languages Whistle recognizes (auto-detected).
pub const LANGUAGES: [&str; 7] = ["en", "de", "fr", "es", "it", "nl", "pl"];

pub fn supports_language(code: &str) -> bool {
    let code = code.trim().to_ascii_lowercase();
    let base = code.split(['-', '_']).next().unwrap_or("");
    LANGUAGES.contains(&base)
}

// ----- lock file ---------------------------------------------------------------------------

/// One entry of `whistle.lock`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct LockEntry {
    pub name: String,
    /// `any`, `x64` or `arm64`.
    pub arch: String,
    pub url: String,
    /// Exact download size; 0 when unknown.
    #[serde(default)]
    pub bytes: u64,
    pub sha256: String,
}

impl LockEntry {
    pub fn pinned(&self) -> bool {
        self.sha256 != UNPINNED
            && self.sha256.len() == 64
            && self.sha256.bytes().all(|b| b.is_ascii_hexdigit())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lock {
    entries: Vec<LockEntry>,
}

/// Why Whistle cannot be used. `message` is what the voice views show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unavailable {
    Lock(String),
    UnsupportedArch(String),
    Missing(String),
    Unpinned,
    HashMismatch(String),
    Load(String),
}

impl Unavailable {
    pub fn message(&self) -> String {
        match self {
            Self::Lock(e) => format!("Whistle's lock file is invalid ({e}). Reinstall NabyScape."),
            Self::UnsupportedArch(a) => format!(
                "Whistle is not available for this processor ({a}). Choose another listening engine."
            ),
            Self::Missing(f) => format!(
                "Whistle is not installed ({f} is missing). Install it in Settings \u{2192} Voice \u{2192} Models, or choose another listening engine."
            ),
            Self::Unpinned => "Whistle's files are not verified in this build (hashes in whistle.lock are not pinned yet). Use an official release or choose another listening engine.".into(),
            Self::HashMismatch(f) => format!(
                "{f} does not match the checksum pinned in this version, so Whistle was not loaded. Reinstall NabyScape."
            ),
            Self::Load(e) => format!("Whistle could not be loaded: {e}"),
        }
    }
}

impl Lock {
    pub fn parse(text: &str) -> Result<Self, Unavailable> {
        let entries: Vec<LockEntry> =
            serde_json::from_str(text).map_err(|e| Unavailable::Lock(e.to_string()))?;
        for e in &entries {
            if e.name.is_empty()
                || e.name.contains(['/', '\\'])
                || !matches!(e.arch.as_str(), "any" | "x64" | "arm64")
                || !e.url.starts_with("https://")
                || e.sha256.is_empty()
            {
                return Err(Unavailable::Lock(format!("bad entry for {}", e.name)));
            }
        }
        Ok(Self { entries })
    }

    pub fn embedded() -> Result<Self, Unavailable> {
        Self::parse(EMBEDDED_LOCK)
    }

    pub fn entries(&self) -> &[LockEntry] {
        &self.entries
    }

    /// The entry for `name` valid on `arch` (`x64`/`arm64`); `any` entries match every arch.
    pub fn entry(&self, name: &str, arch: &str) -> Option<&LockEntry> {
        self.entries
            .iter()
            .find(|e| e.name == name && (e.arch == arch || e.arch == "any"))
    }

    /// Checks `bytes` against the pinned hash. Unpinned entries are refused unless
    /// `allow_unpinned` (debug builds).
    pub fn verify(
        &self,
        name: &str,
        arch: &str,
        bytes: &[u8],
        allow_unpinned: bool,
    ) -> Result<(), Unavailable> {
        let entry = self
            .entry(name, arch)
            .ok_or_else(|| Unavailable::UnsupportedArch(arch.to_owned()))?;
        if !entry.pinned() {
            return if allow_unpinned {
                Ok(())
            } else {
                Err(Unavailable::Unpinned)
            };
        }
        let digest = Sha256::digest(bytes);
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        if hex.eq_ignore_ascii_case(&entry.sha256) {
            Ok(())
        } else {
            Err(Unavailable::HashMismatch(name.to_owned()))
        }
    }

    /// Whether every file needed for `arch` is pinned.
    pub fn fully_pinned(&self, arch: &str) -> bool {
        [MODEL_FILE, ENGINE_FILE]
            .iter()
            .all(|n| self.entry(n, arch).is_some_and(LockEntry::pinned))
    }
}

/// `x64` or `arm64` for this process.
pub fn current_arch() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "x64"
    }
}

/// A cheap check (no hashing, nothing loaded) of whether Whistle can be used: both files sit
/// in `folder` and the lock is pinned (or this is a debug build).
pub fn availability_in(
    folder: &std::path::Path,
    lock: &Lock,
    arch: &str,
    allow_unpinned: bool,
) -> Result<(), Unavailable> {
    if lock.entry(ENGINE_FILE, arch).is_none() {
        return Err(Unavailable::UnsupportedArch(arch.to_owned()));
    }
    for name in [ENGINE_FILE, MODEL_FILE] {
        if !folder.join(name).is_file() {
            return Err(Unavailable::Missing(name.to_owned()));
        }
    }
    if !allow_unpinned && !lock.fully_pinned(arch) {
        return Err(Unavailable::Unpinned);
    }
    Ok(())
}

/// Where Whistle's files are used from: the executable's folder when the setup shipped them,
/// otherwise `install` (the Settings install folder, which may not exist yet).
pub fn locate(install: &std::path::Path) -> std::path::PathBuf {
    let bundled = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(std::path::Path::to_path_buf));
    bundled
        .filter(|dir| {
            [ENGINE_FILE, MODEL_FILE]
                .iter()
                .all(|n| dir.join(n).is_file())
        })
        .unwrap_or_else(|| install.to_path_buf())
}

/// [`availability_in`] for the folder [`locate`] picks.
pub fn availability(install: &std::path::Path) -> Result<(), Unavailable> {
    let lock = Lock::embedded()?;
    availability_in(
        &locate(install),
        &lock,
        current_arch(),
        cfg!(debug_assertions),
    )
}

/// The line the Models page and the voice room show for Whistle.
pub fn status_text(install: &std::path::Path) -> String {
    match availability(install) {
        Ok(()) => "Ready \u{b7} runs on this PC while a conversation is active.".into(),
        Err(e) => e.message(),
    }
}

// ----- results -----------------------------------------------------------------------------

/// A recognized word with optional timing in seconds.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Word {
    pub text: String,
    pub start: Option<f64>,
    pub end: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Transcript {
    pub text: String,
    pub language: Option<String>,
    pub words: Vec<Word>,
}

/// Anything that turns a finished utterance into text. [`WhistleLease`] implements it; the
/// pipeline tests use fakes so no engine is needed.
pub trait Transcriber {
    fn transcribe(
        &mut self,
        pcm: &[f32],
        language: Option<&str>,
        keywords: &[String],
    ) -> Result<Transcript, String>;
}

#[derive(Deserialize)]
struct RawWord {
    #[serde(alias = "word", alias = "text")]
    word: Option<String>,
    #[serde(alias = "start_time", alias = "start_s")]
    start: Option<f64>,
    #[serde(alias = "end_time", alias = "end_s")]
    end: Option<f64>,
}

#[derive(Deserialize)]
struct RawResult {
    #[serde(default)]
    text: String,
    language: Option<String>,
    #[serde(default)]
    words: Vec<serde_json::Value>,
    error: Option<String>,
}

fn word_of(value: &serde_json::Value) -> Option<Word> {
    match value {
        serde_json::Value::String(s) => Some(Word {
            text: s.clone(),
            ..Word::default()
        }),
        other => {
            let raw: RawWord = serde_json::from_value(other.clone()).ok()?;
            Some(Word {
                text: raw.word?,
                start: raw.start,
                end: raw.end,
            })
        }
    }
}

/// Parses `{text, language, ttft_ms, decode_tps[, words]}`.
pub fn parse_result(json: &str) -> Result<Transcript, String> {
    let json = json.trim_matches(char::from(0)).trim();
    let raw: RawResult = serde_json::from_str(json)
        .map_err(|_| "Whistle returned an unreadable result.".to_owned())?;
    if let Some(error) = raw.error.filter(|e| !e.is_empty()) {
        return Err(format!("Whistle: {error}"));
    }
    Ok(Transcript {
        text: raw.text.trim().to_owned(),
        language: raw.language.filter(|l| !l.is_empty()),
        words: raw.words.iter().filter_map(word_of).collect(),
    })
}

/// A streaming update: words already settled and words that may still change.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Partial {
    pub confirmed: String,
    pub pending: String,
}

impl Partial {
    pub fn text(&self) -> String {
        let (a, b) = (self.confirmed.trim(), self.pending.trim());
        match (a.is_empty(), b.is_empty()) {
            (true, _) => b.to_owned(),
            (_, true) => a.to_owned(),
            _ => format!("{a} {b}"),
        }
    }
}

fn joined(value: Option<&serde_json::Value>) -> String {
    match value {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Array(items)) => items
            .iter()
            .filter_map(word_of)
            .map(|w| w.text)
            .collect::<Vec<_>>()
            .join(" "),
        _ => String::new(),
    }
}

/// Parses the JSON of `needle_stream_transcribe_process` / `_stop` (confirmed / pending words,
/// given as strings or word arrays).
pub fn parse_partial(json: &str) -> Result<Partial, String> {
    let json = json.trim_matches(char::from(0)).trim();
    let value: serde_json::Value = serde_json::from_str(json)
        .map_err(|_| "Whistle returned an unreadable result.".to_owned())?;
    if let Some(error) = value
        .get("error")
        .and_then(|e| e.as_str())
        .filter(|e| !e.is_empty())
    {
        return Err(format!("Whistle: {error}"));
    }
    let mut partial = Partial {
        confirmed: joined(
            value
                .get("confirmed")
                .or_else(|| value.get("confirmed_words")),
        ),
        pending: joined(value.get("pending").or_else(|| value.get("pending_words"))),
    };
    if partial.confirmed.is_empty() && partial.pending.is_empty() {
        partial.confirmed = value
            .get("text")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_owned();
    }
    Ok(partial)
}

/// Result buffer size for `count` samples (`count / 16000 * OUTPUT_BYTES_PER_SECOND`, floored).
pub fn output_buffer_len(count: usize) -> usize {
    let seconds = count.div_ceil(SAMPLE_RATE as usize);
    (seconds * OUTPUT_BYTES_PER_SECOND).max(OUTPUT_FLOOR)
}

/// Biasing words for the recognizer: the wake phrase, its words, and the app's names; deduped,
/// bounded and free of control characters. Returned newline-joinable.
pub fn keyword_list(wake_phrase: &str, extra: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut add = |s: &str| {
        let clean: String = s
            .chars()
            .filter(|c| !c.is_control())
            .take(MAXIMUM_KEYWORD_CHARS)
            .collect();
        let clean = clean.trim().to_owned();
        if clean.len() >= 2
            && out.len() < MAXIMUM_KEYWORDS
            && !out.iter().any(|k| k.eq_ignore_ascii_case(&clean))
        {
            out.push(clean);
        }
    };
    add(wake_phrase);
    for word in wake_phrase.split_whitespace() {
        add(word);
    }
    for name in ["Naby", "NabyScape"] {
        add(name);
    }
    for e in extra {
        add(e);
    }
    out
}

/// The `keywords` argument: `\n`-separated, or `None` for no biasing.
pub fn keywords_arg(keywords: &[String]) -> Option<String> {
    let list: Vec<&str> = keywords
        .iter()
        .map(|k| k.trim())
        .filter(|k| !k.is_empty() && !k.contains(['\n', '\0']))
        .take(MAXIMUM_KEYWORDS)
        .collect();
    (!list.is_empty()).then(|| list.join("\n"))
}

// ----- worker lifecycle ----------------------------------------------------------------------

/// The native engine behind the worker (a fake in tests).
pub trait Backend: Send {
    /// Verifies the engine and model. Called once on the worker thread.
    fn load(&mut self) -> Result<(), Unavailable>;
    fn transcribe(
        &mut self,
        pcm: &[f32],
        language: Option<&str>,
        keywords: Option<&str>,
        words: bool,
    ) -> Result<String, String>;
    fn stream_process(
        &mut self,
        pcm: &[f32],
        language: Option<&str>,
        keywords: Option<&str>,
    ) -> Result<String, String>;
    fn stream_stop(&mut self) -> Result<String, String>;
    /// Releases the engine.
    fn unload(&mut self);
}

pub type BackendFactory = Arc<dyn Fn() -> Box<dyn Backend> + Send + Sync>;

enum Job {
    Transcribe {
        pcm: Vec<f32>,
        language: Option<String>,
        keywords: Option<String>,
        words: bool,
        reply: Sender<Result<String, String>>,
    },
    StreamProcess {
        pcm: Vec<f32>,
        language: Option<String>,
        keywords: Option<String>,
        reply: Sender<Result<String, String>>,
    },
    StreamStop {
        reply: Sender<Result<String, String>>,
    },
    /// Wake the worker so it recomputes its idle deadline.
    Recheck,
}

#[derive(Default)]
struct Shared {
    leases: usize,
    running: bool,
    tx: Option<Sender<Job>>,
    loads: usize,
}

/// Owns the (lazily started) worker thread. Cheap to clone.
#[derive(Clone)]
pub struct Whistle {
    factory: BackendFactory,
    idle: Duration,
    shared: Arc<Mutex<Shared>>,
}

impl Whistle {
    pub fn new(factory: BackendFactory, idle: Duration) -> Self {
        Self {
            factory,
            idle,
            shared: Arc::default(),
        }
    }

    /// The production engine (Windows: `needle.exe`; elsewhere: always unavailable). `install`
    /// is the Settings install folder (see [`locate`]).
    pub fn native(install: std::path::PathBuf) -> Self {
        Self::new(
            Arc::new(move || native::backend(locate(&install))),
            IDLE_UNLOAD,
        )
    }

    /// Whether the worker (and so the model) is currently loaded.
    pub fn loaded(&self) -> bool {
        self.lock().running
    }

    /// How many times the model was loaded in this process (diagnostics and tests).
    pub fn loads(&self) -> usize {
        self.lock().loads
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Starts the worker if needed, loads the model, and returns a lease. Blocking (the load
    /// takes a moment): call from a worker thread, never the UI thread.
    pub fn acquire(&self) -> Result<WhistleLease, Unavailable> {
        let start = {
            let mut shared = self.lock();
            if shared.running {
                shared.leases += 1;
                None
            } else {
                let (tx, rx) = channel::<Job>();
                shared.tx = Some(tx);
                shared.running = true;
                shared.leases = 1;
                Some(rx)
            }
        };
        if let Some(rx) = start {
            let (ready_tx, ready_rx) = channel::<Result<(), Unavailable>>();
            let factory = self.factory.clone();
            let shared = self.shared.clone();
            let idle = self.idle;
            let spawned = std::thread::Builder::new()
                .name("NabyScape whistle".into())
                .spawn(move || worker(factory(), rx, ready_tx, shared, idle));
            let outcome = match spawned {
                Ok(_) => ready_rx
                    .recv()
                    .unwrap_or_else(|_| Err(Unavailable::Load("the worker stopped".into()))),
                Err(e) => Err(Unavailable::Load(e.to_string())),
            };
            if let Err(error) = outcome {
                let mut shared = self.lock();
                shared.running = false;
                shared.leases = 0;
                shared.tx = None;
                return Err(error);
            }
        }
        let tx = self
            .lock()
            .tx
            .clone()
            .ok_or_else(|| Unavailable::Load("stopped".into()))?;
        Ok(WhistleLease {
            whistle: self.clone(),
            tx,
            language: None,
            word_timestamps: false,
        })
    }
}

fn worker(
    mut backend: Box<dyn Backend>,
    rx: Receiver<Job>,
    ready: Sender<Result<(), Unavailable>>,
    shared: Arc<Mutex<Shared>>,
    idle: Duration,
) {
    match backend.load() {
        Ok(()) => {
            shared.lock().unwrap_or_else(|e| e.into_inner()).loads += 1;
            let _ = ready.send(Ok(()));
        }
        Err(e) => {
            backend.unload();
            let _ = ready.send(Err(e));
            return;
        }
    }
    loop {
        let leases = shared.lock().unwrap_or_else(|e| e.into_inner()).leases;
        let job = if leases > 0 {
            rx.recv().map_err(|_| RecvTimeoutError::Disconnected)
        } else {
            rx.recv_timeout(idle)
        };
        match job {
            Ok(Job::Transcribe {
                pcm,
                language,
                keywords,
                words,
                reply,
            }) => {
                let _ = reply.send(backend.transcribe(
                    &pcm,
                    language.as_deref(),
                    keywords.as_deref(),
                    words,
                ));
            }
            Ok(Job::StreamProcess {
                pcm,
                language,
                keywords,
                reply,
            }) => {
                let _ = reply.send(backend.stream_process(
                    &pcm,
                    language.as_deref(),
                    keywords.as_deref(),
                ));
            }
            Ok(Job::StreamStop { reply }) => {
                let _ = reply.send(backend.stream_stop());
            }
            Ok(Job::Recheck) => {}
            Err(RecvTimeoutError::Timeout) | Err(RecvTimeoutError::Disconnected) => {
                let mut guard = shared.lock().unwrap_or_else(|e| e.into_inner());
                if guard.leases == 0 {
                    guard.running = false;
                    guard.tx = None;
                    drop(guard);
                    backend.unload();
                    return;
                }
            }
        }
    }
}

/// A voice session's use of the model. Dropping it starts the idle countdown (when it was the
/// last one).
pub struct WhistleLease {
    whistle: Whistle,
    tx: Sender<Job>,
    language: Option<String>,
    word_timestamps: bool,
}

impl WhistleLease {
    /// Pins a language instead of auto-detecting.
    pub fn with_language(mut self, language: Option<String>) -> Self {
        self.language = language;
        self
    }

    pub fn with_word_timestamps(mut self, on: bool) -> Self {
        self.word_timestamps = on;
        self
    }

    fn call(
        &self,
        make: impl FnOnce(Sender<Result<String, String>>) -> Job,
    ) -> Result<String, String> {
        let (reply_tx, reply_rx) = channel();
        self.tx
            .send(make(reply_tx))
            .map_err(|_| "Whistle stopped. Start voice again.".to_owned())?;
        reply_rx
            .recv()
            .map_err(|_| "Whistle stopped. Start voice again.".to_owned())?
    }

    /// Feeds streaming audio and returns the live partial text.
    pub fn stream(&self, pcm: &[f32], keywords: &[String]) -> Result<Partial, String> {
        let language = self.language.clone();
        let keywords = keywords_arg(keywords);
        let pcm = pcm.to_vec();
        let json = self.call(|reply| Job::StreamProcess {
            pcm,
            language,
            keywords,
            reply,
        })?;
        parse_partial(&json)
    }

    /// Ends the stream and returns the final settled text.
    pub fn stream_end(&self) -> Result<Partial, String> {
        let json = self.call(|reply| Job::StreamStop { reply })?;
        parse_partial(&json)
    }
}

impl Transcriber for WhistleLease {
    fn transcribe(
        &mut self,
        pcm: &[f32],
        language: Option<&str>,
        keywords: &[String],
    ) -> Result<Transcript, String> {
        if pcm.is_empty() {
            return Ok(Transcript::default());
        }
        let pcm = &pcm[..pcm.len().min(MAXIMUM_SAMPLES)];
        let language = language
            .map(str::to_owned)
            .or_else(|| self.language.clone());
        let keywords = keywords_arg(keywords);
        let words = self.word_timestamps;
        let owned = pcm.to_vec();
        let json = self.call(|reply| Job::Transcribe {
            pcm: owned,
            language,
            keywords,
            words,
            reply,
        })?;
        parse_result(&json)
    }
}

impl Drop for WhistleLease {
    fn drop(&mut self) {
        let mut shared = self.whistle.lock();
        shared.leases = shared.leases.saturating_sub(1);
        if shared.leases == 0 {
            // Wake the worker so it starts the idle countdown.
            let _ = self.tx.send(Job::Recheck);
        }
    }
}

// ----- native binding -------------------------------------------------------------------------

#[cfg(not(windows))]
mod native {
    use super::*;

    struct Absent;

    impl Backend for Absent {
        fn load(&mut self) -> Result<(), Unavailable> {
            Err(Unavailable::Load("Whistle runs on Windows only.".into()))
        }
        fn transcribe(
            &mut self,
            _: &[f32],
            _: Option<&str>,
            _: Option<&str>,
            _: bool,
        ) -> Result<String, String> {
            Err("Whistle is unavailable.".into())
        }
        fn stream_process(
            &mut self,
            _: &[f32],
            _: Option<&str>,
            _: Option<&str>,
        ) -> Result<String, String> {
            Err("Whistle is unavailable.".into())
        }
        fn stream_stop(&mut self) -> Result<String, String> {
            Err("Whistle is unavailable.".into())
        }
        fn unload(&mut self) {}
    }

    pub fn backend(_: std::path::PathBuf) -> Box<dyn Backend> {
        Box::new(Absent)
    }
}

/// A 16 kHz mono 16-bit PCM RIFF WAV of `pcm` (samples in [-1, 1], clamped), for `needle.exe`.
pub fn wav_bytes(pcm: &[f32]) -> Vec<u8> {
    let data = (pcm.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    out.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    for s in pcm {
        out.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes());
    }
    out
}

/// The result JSON in `needle.exe`'s output: its last line that is a JSON object.
pub fn last_json_line(stdout: &str) -> Option<&str> {
    stdout
        .lines()
        .map(str::trim)
        .rfind(|l| l.starts_with('{') && l.ends_with('}'))
}

#[cfg(windows)]
mod native {
    use super::*;
    use std::os::windows::process::CommandExt;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Instant;

    /// `needle.exe` is a console program; never flash a console window.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    /// An utterance is at most 60 s of audio; a stuck engine is stopped after this.
    const TIMEOUT: Duration = Duration::from_secs(90);

    pub struct Native {
        folder: PathBuf,
        ready: bool,
    }

    pub fn backend(folder: PathBuf) -> Box<dyn Backend> {
        Box::new(Native {
            folder,
            ready: false,
        })
    }

    fn verify(lock: &Lock, folder: &Path, name: &str, arch: &str) -> Result<(), Unavailable> {
        let bytes =
            std::fs::read(folder.join(name)).map_err(|_| Unavailable::Missing(name.to_owned()))?;
        lock.verify(name, arch, &bytes, cfg!(debug_assertions))
    }

    /// A temp file path unique to this process and call.
    fn scratch(extension: &str) -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "nabyscape-whistle-{}-{}.{extension}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }

    /// Deletes a call's temp files however the call ends.
    struct Scratch(Vec<PathBuf>);

    impl Drop for Scratch {
        fn drop(&mut self) {
            for path in &self.0 {
                let _ = std::fs::remove_file(path);
            }
        }
    }

    fn io(what: &str) -> impl Fn(std::io::Error) -> String + '_ {
        move |e| format!("Whistle could not {what}: {e}")
    }

    impl Backend for Native {
        fn load(&mut self) -> Result<(), Unavailable> {
            let lock = Lock::embedded()?;
            let arch = current_arch();
            // Hash both files before anything is executed.
            // shortcut: not re-hashed per utterance, so a writer in the user's own profile could
            // swap needle.exe while voice is active; re-verify per call if the folder is shared.
            verify(&lock, &self.folder, ENGINE_FILE, arch)?;
            verify(&lock, &self.folder, MODEL_FILE, arch)?;
            self.ready = true;
            Ok(())
        }

        fn transcribe(
            &mut self,
            pcm: &[f32],
            language: Option<&str>,
            keywords: Option<&str>,
            words: bool,
        ) -> Result<String, String> {
            if !self.ready {
                return Err("Whistle is not loaded.".into());
            }
            let (wav, out, err) = (scratch("wav"), scratch("out"), scratch("err"));
            let mut files = Scratch(vec![wav.clone(), out.clone(), err.clone()]);
            std::fs::write(&wav, wav_bytes(pcm)).map_err(io("write the audio"))?;
            let mut command = Command::new(self.folder.join(ENGINE_FILE));
            command
                .arg("--model")
                .arg(self.folder.join(MODEL_FILE))
                .arg("--audio")
                .arg(&wav);
            let language = language
                .map(|l| l.trim().to_ascii_lowercase())
                .map(|l| l.split(['-', '_']).next().unwrap_or("").to_owned())
                .filter(|l| supports_language(l));
            if let Some(language) = &language {
                command.args(["--audio-language", language]);
            }
            if let Some(keywords) = keywords {
                let list = scratch("txt");
                files.0.push(list.clone());
                std::fs::write(&list, keywords).map_err(io("write the keywords"))?;
                command.arg("--audio-keywords").arg(&list);
            }
            if words {
                command.arg("--audio-word-timestamps");
            }
            let stdout = std::fs::File::create(&out).map_err(io("start"))?;
            let stderr = std::fs::File::create(&err).map_err(io("start"))?;
            let mut child = command
                .current_dir(&self.folder)
                .stdin(Stdio::null())
                .stdout(stdout)
                .stderr(stderr)
                .creation_flags(CREATE_NO_WINDOW)
                .spawn()
                .map_err(io("start"))?;
            let started = Instant::now();
            let status = loop {
                match child.try_wait().map_err(io("run"))? {
                    Some(status) => break status,
                    None if started.elapsed() > TIMEOUT => {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err("Whistle took too long and was stopped.".into());
                    }
                    None => std::thread::sleep(Duration::from_millis(10)),
                }
            };
            let text = std::fs::read_to_string(&out).unwrap_or_default();
            if !status.success() {
                let detail = std::fs::read_to_string(&err).unwrap_or_default();
                let detail = detail.lines().map(str::trim).rfind(|l| !l.is_empty());
                return Err(match detail {
                    Some(d) => format!("Whistle transcription failed: {d}"),
                    None => format!("Whistle transcription failed ({status})."),
                });
            }
            last_json_line(&text)
                .map(str::to_owned)
                .ok_or_else(|| "Whistle returned no result.".into())
        }

        fn stream_process(
            &mut self,
            _: &[f32],
            _: Option<&str>,
            _: Option<&str>,
        ) -> Result<String, String> {
            Err("Live transcription is not available with needle.exe.".into())
        }

        fn stream_stop(&mut self) -> Result<String, String> {
            Err("Live transcription is not available with needle.exe.".into())
        }

        fn unload(&mut self) {
            self.ready = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real engine: `NABY_WHISTLE_DIR` holds the pinned `needle.exe` and `whistle.cact`, and
    /// `NABY_WHISTLE_WAV` a 16 kHz mono 16-bit clip of speech.
    #[cfg(windows)]
    #[test]
    #[ignore = "needs the Whistle files; set NABY_WHISTLE_DIR and NABY_WHISTLE_WAV"]
    fn the_real_engine_transcribes_a_clip() {
        let folder = std::path::PathBuf::from(std::env::var("NABY_WHISTLE_DIR").unwrap());
        let wav = std::fs::read(std::env::var("NABY_WHISTLE_WAV").unwrap()).unwrap();
        let pcm: Vec<f32> = wav[44..]
            .chunks_exact(2)
            .map(|c| f32::from(i16::from_le_bytes([c[0], c[1]])) / 32768.0)
            .collect();
        assert_eq!(
            availability_in(&folder, &Lock::embedded().unwrap(), "x64", false),
            Ok(())
        );
        let whistle = Whistle::new(
            Arc::new(move || native::backend(folder.clone())),
            Duration::from_secs(1),
        );
        let mut lease = whistle.acquire().unwrap();
        let keywords = keyword_list("Hey Naby", &[]);
        let heard = lease.transcribe(&pcm, Some("es-ES"), &keywords).unwrap();
        println!("{heard:?}");
        assert_eq!(heard.language.as_deref(), Some("es"));
        assert!(
            heard.text.to_lowercase().contains("temporizador"),
            "{}",
            heard.text
        );
    }

    #[test]
    fn wav_bytes_is_a_16_khz_mono_pcm_riff_with_clamped_samples() {
        let wav = wav_bytes(&[0.0, 1.0, -2.0]);
        assert_eq!(wav.len(), 44 + 6);
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(wav[4..8].try_into().unwrap()), 36 + 6);
        assert_eq!(&wav[8..16], b"WAVEfmt ");
        assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 16_000);
        assert_eq!(&wav[36..40], b"data");
        let samples: Vec<i16> = wav[44..]
            .chunks(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect();
        assert_eq!(samples, [0, 32767, -32767]);
    }

    #[test]
    fn the_result_is_the_last_json_line_of_the_output() {
        let out = "loading whistle.cact
{\"text\":\"a\"}
{\"text\":\"Hola\",\"language\":\"es\"}
";
        assert_eq!(
            last_json_line(out),
            Some("{\"text\":\"Hola\",\"language\":\"es\"}")
        );
        assert_eq!(
            parse_result(last_json_line(out).unwrap()).unwrap().text,
            "Hola"
        );
        assert_eq!(last_json_line("usage: needle"), None);
    }
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn hex(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    fn lock_with(model_hash: &str, dll_hash: &str) -> Lock {
        Lock::parse(&format!(
            r#"[
            {{"name":"whistle.cact","arch":"any","url":"https://huggingface.co/a/b","sha256":"{model_hash}"}},
            {{"name":"needle.exe","arch":"x64","url":"https://huggingface.co/c/d","sha256":"{dll_hash}"}},
            {{"name":"needle.exe","arch":"arm64","url":"https://huggingface.co/e/f","sha256":"TO_BE_PINNED"}}]"#
        ))
        .unwrap()
    }

    #[test]
    fn the_embedded_lock_parses_and_lists_every_file() {
        let lock = Lock::embedded().unwrap();
        assert_eq!(lock.entries().len(), 3);
        let model = lock.entry(MODEL_FILE, "x64").unwrap();
        assert_eq!(model.arch, "any");
        assert!(model
            .url
            .starts_with("https://huggingface.co/Cactus-Compute/whistle/resolve/"));
        assert!(lock.entries().iter().all(LockEntry::pinned));
        assert!(lock
            .entry(ENGINE_FILE, "x64")
            .unwrap()
            .url
            .contains("windows-x86_64"));
        assert!(lock
            .entry(ENGINE_FILE, "arm64")
            .unwrap()
            .url
            .contains("windows-arm64"));
        assert!(lock.entry("other.dll", "x64").is_none());
    }

    #[test]
    fn an_unpinned_lock_refuses_unless_debug() {
        let lock = lock_with(UNPINNED, UNPINNED);
        assert!(!lock.fully_pinned("x64"));
        assert_eq!(
            lock.verify(MODEL_FILE, "x64", b"anything", false),
            Err(Unavailable::Unpinned)
        );
        assert_eq!(lock.verify(MODEL_FILE, "x64", b"anything", true), Ok(()));
        assert!(Unavailable::Unpinned.message().contains("not verified"));
        // The repository lock either ships unpinned (refused above) or fully pinned.
        let embedded = Lock::embedded().unwrap();
        if !embedded.fully_pinned("x64") {
            assert_eq!(
                embedded.verify(MODEL_FILE, "x64", b"x", false),
                Err(Unavailable::Unpinned)
            );
        }
    }

    #[test]
    fn pinned_hashes_are_enforced() {
        let model = b"model bytes";
        let lock = lock_with(&hex(model), &"0".repeat(64));
        assert_eq!(lock.verify(MODEL_FILE, "x64", model, false), Ok(()));
        assert_eq!(
            lock.verify(MODEL_FILE, "x64", b"tampered", true),
            Err(Unavailable::HashMismatch(MODEL_FILE.into()))
        );
        // A pinned entry is enforced even where unpinned ones are tolerated.
        assert_eq!(
            lock.verify(ENGINE_FILE, "x64", b"x", true),
            Err(Unavailable::HashMismatch(ENGINE_FILE.into()))
        );
        assert!(lock
            .entry(ENGINE_FILE, "arm64")
            .is_some_and(|e| !e.pinned()));
    }

    #[test]
    fn malformed_locks_are_rejected() {
        assert!(Lock::parse("not json").is_err());
        assert!(
            Lock::parse(r#"[{"name":"../x","arch":"any","url":"https://a","sha256":"x"}]"#)
                .is_err()
        );
        assert!(
            Lock::parse(r#"[{"name":"x","arch":"mips","url":"https://a","sha256":"x"}]"#).is_err()
        );
        assert!(
            Lock::parse(r#"[{"name":"x","arch":"any","url":"http://a","sha256":"x"}]"#).is_err()
        );
    }

    #[test]
    fn result_json_parses_with_and_without_words() {
        let t = parse_result(
            r#"{"text":" Hello there ","language":"en","ttft_ms":12.5,"decode_tps":88.0}"#,
        )
        .unwrap();
        assert_eq!(t.text, "Hello there");
        assert_eq!(t.language.as_deref(), Some("en"));
        assert!(t.words.is_empty());

        let t = parse_result(
            r#"{"text":"hi naby","language":"de","words":[{"word":"hi","start":0.0,"end":0.4},{"word":"naby","start":0.5,"end":0.9},"extra"]}"#,
        )
        .unwrap();
        assert_eq!(t.words.len(), 3);
        assert_eq!(t.words[1].text, "naby");
        assert_eq!(t.words[1].start, Some(0.5));
        assert_eq!(t.words[2].start, None);

        assert!(parse_result("garbage").is_err());
        assert_eq!(
            parse_result(r#"{"error":"bad model"}"#).unwrap_err(),
            "Whistle: bad model"
        );
        // Trailing NULs from the C buffer are tolerated.
        assert_eq!(parse_result("{\"text\":\"a\"}\0\0\0").unwrap().text, "a");
    }

    #[test]
    fn stream_json_parses_confirmed_and_pending() {
        let p = parse_partial(r#"{"confirmed":"turn on","pending":"the lig"}"#).unwrap();
        assert_eq!(p.text(), "turn on the lig");
        let p =
            parse_partial(r#"{"confirmed":[{"word":"open"},"the"],"pending":[{"word":"door"}]}"#)
                .unwrap();
        assert_eq!(p.confirmed, "open the");
        assert_eq!(p.text(), "open the door");
        assert_eq!(
            parse_partial(r#"{"text":"final"}"#).unwrap().text(),
            "final"
        );
        assert!(parse_partial("{").is_err());
    }

    #[test]
    fn output_buffer_scales_with_the_clip_and_has_a_floor() {
        assert_eq!(output_buffer_len(0), OUTPUT_FLOOR);
        assert_eq!(output_buffer_len(16_000 * 3), OUTPUT_FLOOR);
        let long = 16_000 * 120;
        assert_eq!(output_buffer_len(long), 120 * OUTPUT_BYTES_PER_SECOND);
        assert!(output_buffer_len(long) >= OUTPUT_FLOOR);
    }

    #[test]
    fn keywords_include_the_wake_phrase_and_app_names_once() {
        let k = keyword_list("Hey Naby", &["Naby", "Spotify"]);
        assert_eq!(k[0], "Hey Naby");
        assert!(k.contains(&"Naby".to_owned()) && k.contains(&"Spotify".to_owned()));
        assert!(k.contains(&"NabyScape".to_owned()));
        assert_eq!(
            k.iter().filter(|w| w.eq_ignore_ascii_case("naby")).count(),
            1
        );
        assert_eq!(
            keywords_arg(&["a b".into(), "".into(), "c\nd".into(), "ok".into()]).unwrap(),
            "a b\nok"
        );
        assert_eq!(keywords_arg(&[]), None);
    }

    #[test]
    fn language_support() {
        for l in ["en", "de", "fr", "es", "it", "nl", "pl", "en-US", "DE"] {
            assert!(supports_language(l), "{l}");
        }
        for l in ["id", "ja", "pt", "", "zh"] {
            assert!(!supports_language(l), "{l}");
        }
    }

    #[test]
    fn availability_needs_both_files_and_a_pinned_lock() {
        let dir = tempfile::tempdir().unwrap();
        let lock = lock_with(&"a".repeat(64), &"b".repeat(64));
        assert_eq!(
            availability_in(dir.path(), &lock, "x64", false),
            Err(Unavailable::Missing(ENGINE_FILE.into()))
        );
        std::fs::write(dir.path().join(ENGINE_FILE), b"x").unwrap();
        assert_eq!(
            availability_in(dir.path(), &lock, "x64", false),
            Err(Unavailable::Missing(MODEL_FILE.into()))
        );
        std::fs::write(dir.path().join(MODEL_FILE), b"x").unwrap();
        assert_eq!(availability_in(dir.path(), &lock, "x64", false), Ok(()));
        // arm64 is unpinned in this lock.
        assert_eq!(
            availability_in(dir.path(), &lock, "arm64", false),
            Err(Unavailable::Unpinned)
        );
        assert_eq!(availability_in(dir.path(), &lock, "arm64", true), Ok(()));
        assert!(matches!(
            availability_in(dir.path(), &lock, "riscv", true),
            Err(Unavailable::UnsupportedArch(_))
        ));
    }

    // ----- lifecycle with a fake engine ------------------------------------------------------

    struct Fake {
        loads: Arc<AtomicUsize>,
        unloads: Arc<AtomicUsize>,
        fail: bool,
    }

    impl Backend for Fake {
        fn load(&mut self) -> Result<(), Unavailable> {
            if self.fail {
                return Err(Unavailable::Missing(MODEL_FILE.into()));
            }
            self.loads.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn transcribe(
            &mut self,
            pcm: &[f32],
            language: Option<&str>,
            keywords: Option<&str>,
            _: bool,
        ) -> Result<String, String> {
            Ok(format!(
                r#"{{"text":"{} samples","language":"{}","kw":"{}"}}"#,
                pcm.len(),
                language.unwrap_or("auto"),
                keywords.unwrap_or("").replace('\n', "|")
            ))
        }
        fn stream_process(
            &mut self,
            pcm: &[f32],
            _: Option<&str>,
            _: Option<&str>,
        ) -> Result<String, String> {
            Ok(format!(r#"{{"confirmed":"{}","pending":"x"}}"#, pcm.len()))
        }
        fn stream_stop(&mut self) -> Result<String, String> {
            Ok(r#"{"confirmed":"done","pending":""}"#.into())
        }
        fn unload(&mut self) {
            self.unloads.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn fake(fail: bool, idle_ms: u64) -> (Whistle, Arc<AtomicUsize>, Arc<AtomicUsize>) {
        let loads = Arc::new(AtomicUsize::new(0));
        let unloads = Arc::new(AtomicUsize::new(0));
        let (l, u) = (loads.clone(), unloads.clone());
        let factory: BackendFactory = Arc::new(move || {
            Box::new(Fake {
                loads: l.clone(),
                unloads: u.clone(),
                fail,
            })
        });
        (
            Whistle::new(factory, Duration::from_millis(idle_ms)),
            loads,
            unloads,
        )
    }

    fn wait_for(mut done: impl FnMut() -> bool) {
        for _ in 0..400 {
            if done() {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("timed out");
    }

    #[test]
    fn a_lease_transcribes_and_the_model_unloads_after_the_idle_time() {
        let (whistle, loads, unloads) = fake(false, 60);
        assert!(!whistle.loaded());
        let mut lease = whistle.acquire().unwrap();
        assert!(whistle.loaded());
        let t = lease
            .transcribe(&[0.0; 160], Some("en"), &["Hey Naby".into(), "Naby".into()])
            .unwrap();
        assert_eq!(t.text, "160 samples");
        assert_eq!(t.language.as_deref(), Some("en"));
        let p = lease.stream(&[0.0; 10], &[]).unwrap();
        assert_eq!(p.text(), "10 x");
        assert_eq!(lease.stream_end().unwrap().text(), "done");
        // While a lease is held the model stays, however long it idles.
        std::thread::sleep(Duration::from_millis(150));
        assert!(whistle.loaded() && unloads.load(Ordering::SeqCst) == 0);
        drop(lease);
        wait_for(|| !whistle.loaded());
        wait_for(|| unloads.load(Ordering::SeqCst) == 1);
        assert_eq!(loads.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn reacquiring_inside_the_idle_window_reuses_the_loaded_model() {
        let (whistle, loads, _) = fake(false, 500);
        drop(whistle.acquire().unwrap());
        let mut again = whistle.acquire().unwrap();
        again.transcribe(&[0.0; 16], None, &[]).unwrap();
        assert_eq!(loads.load(Ordering::SeqCst), 1);
        assert_eq!(whistle.loads(), 1);
    }

    #[test]
    fn a_new_session_after_unloading_loads_again() {
        let (whistle, loads, unloads) = fake(false, 20);
        drop(whistle.acquire().unwrap());
        wait_for(|| unloads.load(Ordering::SeqCst) == 1);
        let mut lease = whistle.acquire().unwrap();
        lease.transcribe(&[0.0; 16], None, &[]).unwrap();
        assert_eq!(loads.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_failed_load_reports_the_reason_and_leaves_nothing_running() {
        let (whistle, _, unloads) = fake(true, 20);
        let error = whistle.acquire().err().unwrap();
        assert_eq!(error, Unavailable::Missing(MODEL_FILE.into()));
        assert!(error.message().contains("whistle.cact"));
        assert!(!whistle.loaded());
        wait_for(|| unloads.load(Ordering::SeqCst) == 1);
    }

    #[test]
    fn empty_audio_is_not_sent_to_the_engine() {
        let (whistle, _, _) = fake(false, 50);
        let mut lease = whistle.acquire().unwrap();
        assert_eq!(
            lease.transcribe(&[], None, &[]).unwrap(),
            Transcript::default()
        );
    }
}
