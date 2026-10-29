//! LRCLIB lookup with a bounded in-memory cache (`LyricsService`).
//!
//! The service is a plain state machine so it can be driven from any thread model:
//! [`LyricsService::begin`] returns the request to run on a worker, and
//! [`LyricsService::complete`] applies the outcome, dropping replies that were
//! cancelled or superseded. [`LyricsService::load`] does both with an injected
//! fetch closure (tests, or [`default_fetch`] in production).

use std::collections::HashSet;
use std::io::Read;
use std::time::Duration;

use super::lrc::{self, Lyrics};

const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const CACHE_LIMIT: usize = 20;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);

pub const STATUS_OFF: &str = "Lyrics are off.";
pub const STATUS_INCOMPATIBLE: &str =
    "Choose a music session with a title, artist and playback timeline.";
pub const STATUS_LOADING: &str = "Finding synced lyrics\u{2026}";
pub const STATUS_NONE: &str = "No synced lyrics for this track.";
pub const STATUS_SYNCED: &str = "Synced lyrics \u{b7} LRCLIB";
pub const STATUS_INSTRUMENTAL: &str = "Instrumental track";
pub const STATUS_TIMEOUT: &str = "Lyrics request timed out. Tap Retry.";
pub const STATUS_UNAVAILABLE: &str = "Lyrics unavailable. Tap Retry.";
pub const STATUS_LOCAL: &str = "Local LRC \u{b7} this session";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    /// Non-2xx response.
    Status(u16),
    Timeout,
    TooLarge,
    Network(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct LyricResult {
    pub lines: Lyrics,
    pub status: String,
}

impl LyricResult {
    fn empty(status: &str) -> Self {
        LyricResult {
            lines: Lyrics::default(),
            status: status.to_owned(),
        }
    }
}

/// Work for a worker thread: GET `url`, then hand the outcome to [`LyricsService::complete`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LyricsRequest {
    pub url: String,
    id: u64,
    key: String,
}

#[derive(Debug)]
pub struct LyricsService {
    cache: Vec<(String, LyricResult)>,
    key: String,
    pending: Option<u64>,
    next_id: u64,
    consumers: HashSet<u64>,
    disposed: bool,
    result: LyricResult,
    changed: bool,
}

impl Default for LyricsService {
    fn default() -> Self {
        Self::new()
    }
}

/// `Uri.EscapeDataString`: everything except RFC 3986 unreserved characters.
pub fn escape_data_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for &b in s.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn rounded(duration: f64) -> i64 {
    duration.round_ties_even() as i64
}

fn track_key(title: &str, artist: &str, duration: f64) -> String {
    format!("{title}\n{artist}\n{}", rounded(duration))
}

/// The exact LRCLIB request the C# service sends.
pub fn lookup_url(title: &str, artist: &str, duration: f64) -> String {
    let mut url = format!(
        "https://lrclib.net/api/get?track_name={}&artist_name={}",
        escape_data_string(title),
        escape_data_string(artist)
    );
    if (1.0..=3600.0).contains(&duration) {
        url.push_str(&format!("&duration={}", rounded(duration)));
    }
    url
}

impl LyricsService {
    pub fn new() -> Self {
        LyricsService {
            cache: Vec::new(),
            key: String::new(),
            pending: None,
            next_id: 1,
            consumers: HashSet::new(),
            disposed: false,
            result: LyricResult::empty(STATUS_OFF),
            changed: false,
        }
    }

    pub fn result(&self) -> &LyricResult {
        &self.result
    }

    /// True once after every change of [`result`](Self::result).
    pub fn take_changed(&mut self) -> bool {
        std::mem::take(&mut self.changed)
    }

    pub fn acquire(&mut self, owner: u64) {
        self.consumers.insert(owner);
    }

    /// The last consumer leaving cancels work and clears the cache.
    pub fn release(&mut self, owner: u64) {
        if !self.consumers.remove(&owner) || !self.consumers.is_empty() {
            return;
        }
        self.cancel();
        self.key.clear();
        self.cache.clear();
        self.set(LyricResult::empty(STATUS_OFF));
    }

    /// Starts a lookup. Returns `None` when nothing needs fetching (disabled, incompatible,
    /// same track as before, cache hit).
    pub fn begin(
        &mut self,
        title: &str,
        artist: &str,
        duration: f64,
        enabled: bool,
        compatible: bool,
    ) -> Option<LyricsRequest> {
        if self.disposed {
            return None;
        }
        if !enabled || !compatible {
            self.cancel();
            self.key.clear();
            self.set(LyricResult::empty(if enabled {
                STATUS_INCOMPATIBLE
            } else {
                STATUS_OFF
            }));
            return None;
        }
        let next = track_key(title, artist, duration);
        if next == self.key {
            return None;
        }
        self.cancel();
        self.key = next.clone();
        if let Some((_, found)) = self.cache.iter().find(|(k, _)| *k == next) {
            let found = found.clone();
            self.set(found);
            return None;
        }
        self.set(LyricResult::empty(STATUS_LOADING));
        if title.trim().is_empty() || artist.trim().is_empty() {
            self.set(LyricResult::empty(STATUS_UNAVAILABLE));
            return None;
        }
        let id = self.next_id;
        self.next_id += 1;
        self.pending = Some(id);
        Some(LyricsRequest {
            url: lookup_url(title, artist, duration),
            id,
            key: next,
        })
    }

    /// Applies a fetch outcome. Replies for cancelled or superseded requests are ignored.
    pub fn complete(&mut self, request: &LyricsRequest, outcome: Result<String, FetchError>) {
        let (result, cacheable) = Self::interpret(outcome);
        let current = self.pending == Some(request.id) && !self.disposed && self.key == request.key;
        if self.pending == Some(request.id) {
            self.pending = None;
        }
        if !current {
            return;
        }
        if cacheable {
            self.cache_insert(&request.key, result.clone());
        }
        self.set(result);
    }

    /// Blocking convenience: [`begin`](Self::begin), run `fetch`, [`complete`](Self::complete).
    pub fn load<F>(
        &mut self,
        title: &str,
        artist: &str,
        duration: f64,
        enabled: bool,
        compatible: bool,
        fetch: F,
    ) where
        F: Fn(&str) -> Result<String, FetchError>,
    {
        if let Some(request) = self.begin(title, artist, duration, enabled, compatible) {
            let outcome = fetch(&request.url);
            self.complete(&request, outcome);
        }
    }

    /// Loads a local LRC for this session.
    pub fn import(
        &mut self,
        title: &str,
        artist: &str,
        duration: f64,
        lrc_text: &str,
    ) -> Result<(), &'static str> {
        const BAD: &str = "Choose an LRC file with [mm:ss] line timestamps.";
        let lines = lrc::try_parse(lrc_text).map_err(|_| BAD)?;
        if lines.is_empty() {
            return Err(BAD);
        }
        self.cancel();
        self.key = track_key(title, artist, duration);
        let result = LyricResult {
            lines,
            status: STATUS_LOCAL.to_owned(),
        };
        let key = self.key.clone();
        self.cache_insert(&key, result.clone());
        self.set(result);
        Ok(())
    }

    /// Forgets the current track's result so the next `begin` fetches again.
    pub fn retry(&mut self) {
        let key = std::mem::take(&mut self.key);
        self.cache.retain(|(k, _)| *k != key);
    }

    pub fn cancel(&mut self) {
        if self.pending.take().is_some() {
            self.key.clear();
        }
    }

    pub fn dispose(&mut self) {
        self.disposed = true;
        self.cancel();
        self.cache.clear();
    }

    fn set(&mut self, result: LyricResult) {
        if self.result != result {
            self.result = result;
            self.changed = true;
        }
    }

    /// Mirrors `Dictionary` semantics: evict the oldest entry at the limit, then upsert.
    fn cache_insert(&mut self, key: &str, result: LyricResult) {
        if self.cache.len() >= CACHE_LIMIT {
            self.cache.remove(0);
        }
        match self.cache.iter_mut().find(|(k, _)| k == key) {
            Some(slot) => slot.1 = result,
            None => self.cache.push((key.to_owned(), result)),
        }
    }

    /// Outcome to displayed result, plus whether it may be cached.
    fn interpret(outcome: Result<String, FetchError>) -> (LyricResult, bool) {
        let body = match outcome {
            Ok(body) => body,
            Err(FetchError::Status(404)) => return (LyricResult::empty(STATUS_NONE), true),
            Err(FetchError::Timeout) => return (LyricResult::empty(STATUS_TIMEOUT), false),
            Err(_) => return (LyricResult::empty(STATUS_UNAVAILABLE), false),
        };
        let unavailable = (LyricResult::empty(STATUS_UNAVAILABLE), false);
        if body.len() > MAX_RESPONSE_BYTES {
            return unavailable;
        }
        let Ok(json) = serde_json::from_str::<serde_json::Value>(&body) else {
            return unavailable;
        };
        if !json.is_object() {
            return unavailable;
        }
        let instrumental = json.get("instrumental").and_then(|v| v.as_bool()) == Some(true);
        let lines = match json.get("syncedLyrics").and_then(|v| v.as_str()) {
            None => Lyrics::default(),
            Some(text) => match lrc::try_parse(text) {
                Ok(lines) => lines,
                Err(_) => return unavailable,
            },
        };
        let status = if !lines.is_empty() {
            STATUS_SYNCED
        } else if instrumental {
            STATUS_INSTRUMENTAL
        } else {
            STATUS_NONE
        };
        (
            LyricResult {
                lines,
                status: status.to_owned(),
            },
            true,
        )
    }
}

/// Lyrics agent: the shared agent settings but with redirects disabled, as the C#
/// client (`AllowAutoRedirect = false`). A 3xx therefore resolves to unavailable.
fn agent() -> &'static ureq::Agent {
    static AGENT: std::sync::OnceLock<ureq::Agent> = std::sync::OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::AgentBuilder::new()
            .user_agent(crate::http::USER_AGENT)
            .timeout_connect(Duration::from_secs(10))
            .redirects(0)
            .build()
    })
}

/// Production fetch: 8 s overall timeout, no redirects, 1 MiB cap.
pub fn default_fetch(url: &str) -> Result<String, FetchError> {
    let response = agent().get(url).timeout(REQUEST_TIMEOUT).call();
    let response = match response {
        Ok(r) => r,
        Err(ureq::Error::Status(code, _)) => return Err(FetchError::Status(code)),
        Err(ureq::Error::Transport(t)) => return Err(transport_error(&t)),
    };
    if !(200..300).contains(&response.status()) {
        return Err(FetchError::Status(response.status()));
    }
    if response
        .header("Content-Length")
        .and_then(|v| v.trim().parse::<usize>().ok())
        .is_some_and(|n| n > MAX_RESPONSE_BYTES)
    {
        return Err(FetchError::TooLarge);
    }
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(MAX_RESPONSE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| io_error(&e))?;
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err(FetchError::TooLarge);
    }
    String::from_utf8(bytes).map_err(|_| FetchError::Network("response is not UTF-8".to_owned()))
}

fn io_error(e: &std::io::Error) -> FetchError {
    if matches!(
        e.kind(),
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
    ) {
        FetchError::Timeout
    } else {
        FetchError::Network(e.to_string())
    }
}

fn transport_error(t: &ureq::Transport) -> FetchError {
    let mut source: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(t);
    while let Some(err) = source {
        if let Some(io) = err.downcast_ref::<std::io::Error>() {
            if let FetchError::Timeout = io_error(io) {
                return FetchError::Timeout;
            }
        }
        source = err.source();
    }
    FetchError::Network(t.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    const BODY: &str = r#"{"syncedLyrics":"[00:01.00]Original fixture line\n[00:03.00]Another sample line","instrumental":false}"#;

    fn ok(_: &str) -> Result<String, FetchError> {
        Ok(BODY.to_owned())
    }

    #[test]
    fn url_matches_csharp() {
        assert_eq!(
            lookup_url("Sample title", "Sample artist", 180.4),
            "https://lrclib.net/api/get?track_name=Sample%20title&artist_name=Sample%20artist&duration=180"
        );
        assert_eq!(
            lookup_url("a&b", "é", 0.5),
            "https://lrclib.net/api/get?track_name=a%26b&artist_name=%C3%A9"
        );
        assert!(lookup_url("a", "b", 3601.0).ends_with("artist_name=b"));
        // Banker's rounding like Math.Round.
        assert!(lookup_url("a", "b", 2.5).ends_with("&duration=2"));
        assert!(lookup_url("a", "b", f64::NAN).ends_with("artist_name=b"));
        assert_eq!(
            escape_data_string("A-z_0.9~!'()*"),
            "A-z_0.9~%21%27%28%29%2A"
        );
    }

    #[test]
    fn requires_consent_and_metadata_then_caches() {
        let calls = Cell::new(0);
        let seen = RefCell::new(String::new());
        let fetch = |url: &str| {
            calls.set(calls.get() + 1);
            *seen.borrow_mut() = url.to_owned();
            ok(url)
        };
        let mut s = LyricsService::new();
        s.load("Sample title", "Sample artist", 180.0, false, true, fetch);
        assert_eq!(s.result().status, STATUS_OFF);
        s.load("Sample title", "Sample artist", 180.0, true, false, fetch);
        assert_eq!(s.result().status, STATUS_INCOMPATIBLE);
        assert_eq!(calls.get(), 0);
        s.load("Sample title", "Sample artist", 180.0, true, true, fetch);
        s.load("Sample title", "Sample artist", 180.0, true, true, fetch);
        assert_eq!(calls.get(), 1);
        assert_eq!(s.result().lines.len(), 2);
        assert_eq!(s.result().status, STATUS_SYNCED);
        assert!(seen.borrow().starts_with("https://lrclib.net/api/get?"));
        assert!(!seen.borrow().contains("key"));
        s.retry();
        s.load("Sample title", "Sample artist", 180.0, true, true, fetch);
        assert_eq!(calls.get(), 2);
        s.import(
            "Sample title",
            "Sample artist",
            180.0,
            "[00:01.0]Original offline sample",
        )
        .unwrap();
        s.load("Sample title", "Sample artist", 180.0, true, true, fetch);
        assert_eq!(calls.get(), 2);
        assert_eq!(s.result().lines.lines[0].text, "Original offline sample");
        assert_eq!(s.result().status, STATUS_LOCAL);
        assert!(s.import("t", "a", 1.0, "no stamps").is_err());
    }

    #[test]
    fn shared_consumers_retain_until_last_release() {
        let calls = Cell::new(0);
        let fetch = |u: &str| {
            calls.set(calls.get() + 1);
            ok(u)
        };
        let mut s = LyricsService::new();
        let (dock, panel) = (1, 2);
        s.acquire(dock);
        s.acquire(panel);
        s.load("Track", "Artist", 180.0, true, true, fetch);
        s.release(dock);
        assert_eq!(s.result().lines.len(), 2);
        s.release(panel);
        assert!(s.result().lines.is_empty());
        s.acquire(dock);
        s.load("Track", "Artist", 180.0, true, true, fetch);
        assert_eq!(calls.get(), 2);
        assert!(s.take_changed());
        assert!(!s.take_changed());
    }

    #[test]
    fn lyrics_lookup_http_404_is_unavailable() {
        let mut s = LyricsService::new();
        s.load("T", "A", 100.0, true, true, |_| {
            Err(FetchError::Status(404))
        });
        assert!(s.result().lines.is_empty());
        assert_eq!(s.result().status, STATUS_NONE);
        // 404 is cached: no second request.
        let calls = Cell::new(0);
        s.load("T", "A", 100.0, true, true, |_| {
            calls.set(calls.get() + 1);
            ok("")
        });
        assert_eq!(calls.get(), 0);
    }

    #[test]
    fn errors_resolve_to_unavailable_and_are_not_cached() {
        let cases: Vec<(Result<String, FetchError>, &str)> = vec![
            (Err(FetchError::Status(500)), STATUS_UNAVAILABLE),
            (Err(FetchError::Status(301)), STATUS_UNAVAILABLE),
            (
                Err(FetchError::Network("offline".into())),
                STATUS_UNAVAILABLE,
            ),
            (Err(FetchError::TooLarge), STATUS_UNAVAILABLE),
            (Err(FetchError::Timeout), STATUS_TIMEOUT),
            (Ok("not json".into()), STATUS_UNAVAILABLE),
            (Ok("[1,2]".into()), STATUS_UNAVAILABLE),
            (Ok("".into()), STATUS_UNAVAILABLE),
            (Ok("{\"syncedLyrics\":".into()), STATUS_UNAVAILABLE),
            (Ok("x".repeat(MAX_RESPONSE_BYTES + 1)), STATUS_UNAVAILABLE),
            (Ok("{\"syncedLyrics\":5}".into()), STATUS_NONE),
            (
                Ok("{\"syncedLyrics\":null,\"instrumental\":true}".into()),
                STATUS_INSTRUMENTAL,
            ),
            (Ok("{\"syncedLyrics\":\"plain text\"}".into()), STATUS_NONE),
        ];
        for (outcome, status) in cases {
            let mut s = LyricsService::new();
            let o = outcome.clone();
            s.load("T", "A", 100.0, true, true, move |_| o.clone());
            assert_eq!(s.result().status, status, "{outcome:?}");
            assert!(s.result().lines.is_empty());
            if status == STATUS_UNAVAILABLE || status == STATUS_TIMEOUT {
                s.retry();
                let calls = Cell::new(0);
                s.load("T", "A", 100.0, true, true, |u| {
                    calls.set(calls.get() + 1);
                    ok(u)
                });
                assert_eq!(calls.get(), 1);
            }
        }
    }

    #[test]
    fn cancelled_reply_cannot_overwrite_new_track() {
        let mut s = LyricsService::new();
        let old = s.begin("Old sample", "Fixture", 180.0, true, true).unwrap();
        let new = s.begin("New sample", "Fixture", 180.0, true, true).unwrap();
        s.complete(&new, Err(FetchError::Status(503)));
        assert!(s.result().status.to_lowercase().contains("unavailable"));
        s.complete(
            &old,
            Ok("{\"syncedLyrics\":\"[00:01.0]Old sample line\"}".to_owned()),
        );
        assert!(s.result().lines.is_empty());
        assert!(s.result().status.to_lowercase().contains("unavailable"));
    }

    #[test]
    fn disabling_cancels_pending_and_dispose_stops_everything() {
        let mut s = LyricsService::new();
        let r = s.begin("A", "B", 100.0, true, true).unwrap();
        assert!(s.begin("A", "B", 100.0, false, true).is_none());
        s.complete(&r, Ok(BODY.to_owned()));
        assert_eq!(s.result().status, STATUS_OFF);
        let r = s.begin("A", "B", 100.0, true, true).unwrap();
        s.dispose();
        s.complete(&r, Ok(BODY.to_owned()));
        assert!(s.begin("A", "B", 100.0, true, true).is_none());
        assert_ne!(s.result().status, STATUS_SYNCED);
    }

    #[test]
    fn cache_is_bounded() {
        let mut s = LyricsService::new();
        for i in 0..30 {
            s.load(&format!("t{i}"), "a", 100.0, true, true, ok);
        }
        assert!(s.cache.len() <= CACHE_LIMIT);
    }
}
