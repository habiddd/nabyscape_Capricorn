//! Whistle, the built-in listening model: Cactus Compute `whistle.cact` run by the Cactus Needle
//! engine `libneedle.dll` (user decision 2026-10-29).
//!
//! * The engine DLL is bound dynamically (never at startup): `LoadLibraryW` on a full path inside
//!   the executable's own folder only, never the search path.
//! * Both files are checked against SHA-256 values pinned in `rust/whistle.lock`, which is
//!   embedded in the executable. A lock that still says `TO_BE_PINNED` refuses to load unless the
//!   build has debug assertions.
//! * One worker thread owns the model (it is not thread-safe). The thread starts when voice
//!   starts, and the model and DLL are unloaded 60 s after the last voice session ended. There is
//!   no always-on Whistle loop: wake listening uses Windows speech recognition.
//!
//! Everything except the Win32 binding in [`native`] is platform-free and tested on Linux.

use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Deserialize;
use sha2::{Digest, Sha256};

/// The lock file, embedded so a swapped file next to the exe cannot change what is trusted.
pub const EMBEDDED_LOCK: &str = include_str!("../../../../whistle.lock");
/// Placeholder in `whistle.lock` until the release process pins real hashes.
pub const UNPINNED: &str = "TO_BE_PINNED";
/// The model file and engine DLL names (beside `nabyscape.exe`).
pub const MODEL_FILE: &str = "whistle.cact";
pub const ENGINE_FILE: &str = "libneedle.dll";
/// Whistle's input rate.
pub const SAMPLE_RATE: u32 = 16_000;
/// Idle time after voice ended before the model and DLL are unloaded.
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
                "Whistle needs {f} next to nabyscape.exe. Reinstall NabyScape or choose another listening engine."
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
/// next to the executable and the lock is pinned (or this is a debug build).
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

/// [`availability_in`] for the running executable's folder.
pub fn availability() -> Result<(), Unavailable> {
    let lock = Lock::embedded()?;
    let folder = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(std::path::Path::to_path_buf))
        .ok_or_else(|| Unavailable::Missing(ENGINE_FILE.into()))?;
    availability_in(&folder, &lock, current_arch(), cfg!(debug_assertions))
}

/// The line the Models page and the voice room show for Whistle.
pub fn status_text() -> String {
    match availability() {
        Ok(()) => {
            "Ready \u{b7} loads when a conversation starts and unloads a minute after it ends."
                .into()
        }
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
/// pipeline tests use fakes so no DLL is needed.
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
    /// Loads the DLL and model. Called once on the worker thread.
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
    /// Unloads the model and the DLL.
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

    /// The production engine (Windows: `libneedle.dll`; elsewhere: always unavailable).
    pub fn native() -> Self {
        Self::new(Arc::new(|| native::backend()), IDLE_UNLOAD)
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

    pub fn backend() -> Box<dyn Backend> {
        Box::new(Absent)
    }
}

#[cfg(windows)]
mod native {
    use super::*;
    use std::ffi::{c_char, c_int, CStr, CString};
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Path, PathBuf};
    use windows::core::{PCSTR, PCWSTR};
    use windows::Win32::Foundation::{FreeLibrary, HMODULE};
    use windows::Win32::System::LibraryLoader::{
        GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR,
        LOAD_LIBRARY_SEARCH_SYSTEM32,
    };

    type Load = unsafe extern "C" fn(*const c_char, u64) -> c_int;
    type Transcribe = unsafe extern "C" fn(
        *const f32,
        c_int,
        *const c_char,
        *const c_char,
        c_int,
        *mut c_char,
        c_int,
    ) -> c_int;
    type StreamProcess = unsafe extern "C" fn(
        *const f32,
        c_int,
        *const c_char,
        *const c_char,
        *mut c_char,
        c_int,
    ) -> c_int;
    type StreamStop = unsafe extern "C" fn(*mut c_char, c_int) -> c_int;
    type LastError = unsafe extern "C" fn() -> *const c_char;

    struct Api {
        load: Load,
        transcribe: Transcribe,
        stream_process: StreamProcess,
        stream_stop: StreamStop,
        last_error: LastError,
    }

    pub struct Native {
        module: Option<HMODULE>,
        api: Option<Api>,
        /// The `.cact` bytes stay alive while the engine may reference them.
        model: Vec<u8>,
        folder: Option<PathBuf>,
    }

    // SAFETY: the module handle and function pointers are only used on the one worker thread
    // that owns the backend; the type moves there once, before `load`.
    unsafe impl Send for Native {}

    pub fn backend() -> Box<dyn Backend> {
        Box::new(Native {
            module: None,
            api: None,
            model: Vec::new(),
            folder: None,
        })
    }

    /// The only place Whistle files are looked for: the folder of the running executable.
    fn exe_folder() -> Option<PathBuf> {
        std::env::current_exe()
            .ok()?
            .parent()
            .map(Path::to_path_buf)
    }

    fn read_verified(
        lock: &Lock,
        folder: &Path,
        name: &str,
        arch: &str,
    ) -> Result<Vec<u8>, Unavailable> {
        let path = folder.join(name);
        let bytes = std::fs::read(&path).map_err(|_| Unavailable::Missing(name.to_owned()))?;
        lock.verify(name, arch, &bytes, cfg!(debug_assertions))?;
        Ok(bytes)
    }

    impl Native {
        fn error(&self) -> String {
            let Some(api) = &self.api else {
                return String::new();
            };
            // SAFETY: `needle_last_error` returns NULL or a NUL-terminated string owned by the
            // engine that stays valid until the next call on this thread.
            unsafe {
                let p = (api.last_error)();
                if p.is_null() {
                    String::new()
                } else {
                    CStr::from_ptr(p).to_string_lossy().into_owned()
                }
            }
        }

        fn run(
            &mut self,
            what: &str,
            size: usize,
            call: impl FnOnce(&Api, *mut c_char, c_int) -> c_int,
        ) -> Result<String, String> {
            let mut out = vec![0u8; size];
            let code = {
                let api = self.api.as_ref().ok_or("Whistle is not loaded.")?;
                call(api, out.as_mut_ptr().cast(), out.len() as c_int)
            };
            if code < 0 {
                let detail = self.error();
                return Err(if detail.is_empty() {
                    format!("Whistle {what} failed ({code}).")
                } else {
                    format!("Whistle {what} failed: {detail}")
                });
            }
            let end = out.iter().position(|b| *b == 0).unwrap_or(out.len());
            Ok(String::from_utf8_lossy(&out[..end]).into_owned())
        }
    }

    fn proc<T>(module: HMODULE, name: &'static [u8]) -> Result<T, Unavailable> {
        debug_assert_eq!(std::mem::size_of::<T>(), std::mem::size_of::<usize>());
        // SAFETY: `name` is NUL terminated; the address is only transmuted to the documented
        // C signature of the export it names.
        unsafe {
            let address = GetProcAddress(module, PCSTR(name.as_ptr()));
            match address {
                Some(f) => Ok(std::mem::transmute_copy::<_, T>(&f)),
                None => Err(Unavailable::Load(format!(
                    "{} is missing from the engine DLL.",
                    String::from_utf8_lossy(&name[..name.len() - 1])
                ))),
            }
        }
    }

    impl Backend for Native {
        fn load(&mut self) -> Result<(), Unavailable> {
            let lock = Lock::embedded()?;
            let arch = current_arch();
            let folder = exe_folder().ok_or_else(|| Unavailable::Missing(ENGINE_FILE.into()))?;
            // Hash both files before anything is executed or parsed.
            let _dll = read_verified(&lock, &folder, ENGINE_FILE, arch)?;
            let model = read_verified(&lock, &folder, MODEL_FILE, arch)?;
            drop(_dll);
            let path = folder.join(ENGINE_FILE);
            let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            // SAFETY: `wide` is a NUL-terminated absolute path in the exe folder. The flags make
            // dependencies resolve from that folder and System32 only, never the search path.
            let module = unsafe {
                LoadLibraryExW(
                    PCWSTR(wide.as_ptr()),
                    None,
                    LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32,
                )
            }
            .map_err(|e| Unavailable::Load(e.message()))?;
            self.module = Some(module);
            self.folder = Some(folder);
            let api = Api {
                load: proc(module, b"needle_load\0")?,
                transcribe: proc(module, b"needle_transcribe\0")?,
                stream_process: proc(module, b"needle_stream_transcribe_process\0")?,
                stream_stop: proc(module, b"needle_stream_transcribe_stop\0")?,
                last_error: proc(module, b"needle_last_error\0")?,
            };
            self.api = Some(api);
            self.model = model;
            let code = {
                let api = self.api.as_ref().expect("api just set");
                // SAFETY: the pointer and length describe `self.model`, which outlives the engine.
                unsafe { (api.load)(self.model.as_ptr().cast(), self.model.len() as u64) }
            };
            if code < 0 {
                let detail = self.error();
                return Err(Unavailable::Load(if detail.is_empty() {
                    format!("the model was rejected ({code})")
                } else {
                    detail
                }));
            }
            Ok(())
        }

        fn transcribe(
            &mut self,
            pcm: &[f32],
            language: Option<&str>,
            keywords: Option<&str>,
            words: bool,
        ) -> Result<String, String> {
            let language = language
                .map(CString::new)
                .transpose()
                .map_err(|_| "Bad language.")?;
            let keywords = keywords
                .map(CString::new)
                .transpose()
                .map_err(|_| "Bad keywords.")?;
            self.run(
                "transcription",
                output_buffer_len(pcm.len()),
                |api, out, len| {
                    // SAFETY: all pointers are valid for the stated lengths for the call duration.
                    unsafe {
                        (api.transcribe)(
                            pcm.as_ptr(),
                            pcm.len() as c_int,
                            language.as_ref().map_or(std::ptr::null(), |c| c.as_ptr()),
                            keywords.as_ref().map_or(std::ptr::null(), |c| c.as_ptr()),
                            c_int::from(words),
                            out,
                            len,
                        )
                    }
                },
            )
        }

        fn stream_process(
            &mut self,
            pcm: &[f32],
            language: Option<&str>,
            keywords: Option<&str>,
        ) -> Result<String, String> {
            let language = language
                .map(CString::new)
                .transpose()
                .map_err(|_| "Bad language.")?;
            let keywords = keywords
                .map(CString::new)
                .transpose()
                .map_err(|_| "Bad keywords.")?;
            self.run(
                "streaming",
                output_buffer_len(pcm.len()),
                |api, out, len| {
                    // SAFETY: as for `transcribe`.
                    unsafe {
                        (api.stream_process)(
                            pcm.as_ptr(),
                            pcm.len() as c_int,
                            language.as_ref().map_or(std::ptr::null(), |c| c.as_ptr()),
                            keywords.as_ref().map_or(std::ptr::null(), |c| c.as_ptr()),
                            out,
                            len,
                        )
                    }
                },
            )
        }

        fn stream_stop(&mut self) -> Result<String, String> {
            self.run("streaming", OUTPUT_FLOOR, |api, out, len| {
                // SAFETY: `out` is valid for `len` bytes.
                unsafe { (api.stream_stop)(out, len) }
            })
        }

        fn unload(&mut self) {
            self.api = None;
            self.model = Vec::new();
            if let Some(module) = self.module.take() {
                // SAFETY: the module handle came from LoadLibraryExW and no function pointer
                // from it is used after `api` was cleared.
                unsafe {
                    let _ = FreeLibrary(module);
                }
            }
        }
    }

    impl Drop for Native {
        fn drop(&mut self) {
            self.unload();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
            {{"name":"libneedle.dll","arch":"x64","url":"https://huggingface.co/c/d","sha256":"{dll_hash}"}},
            {{"name":"libneedle.dll","arch":"arm64","url":"https://huggingface.co/e/f","sha256":"TO_BE_PINNED"}}]"#
        ))
        .unwrap()
    }

    #[test]
    fn the_embedded_lock_parses_and_lists_every_file() {
        let lock = Lock::embedded().unwrap();
        assert_eq!(lock.entries().len(), 3);
        let model = lock.entry(MODEL_FILE, "x64").unwrap();
        assert_eq!(model.arch, "any");
        assert_eq!(
            model.url,
            "https://huggingface.co/Cactus-Compute/whistle/resolve/main/whistle.cact"
        );
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
