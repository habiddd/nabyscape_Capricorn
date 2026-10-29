//! Ported from `Features/Activities/DownloadArrivals.cs` (the inference, not the watcher).
//!
//! Folder arrival inference, not a browser completion API: a file counts as a download once its
//! name looks final, it was not part of the baseline, its size stopped changing for two seconds
//! and it can be opened for reading. The host owns the `ReadDirectoryChangesW` watcher and the
//! 500 ms worker; it feeds events and file-system probes into [`DownloadInference`].

#[cfg(test)]
mod tests;

use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};

/// Most files tracked in the baseline; more fails closed.
pub const MAX_KNOWN: usize = 10_000;
/// Most candidates waiting for their size to settle.
pub const MAX_PENDING: usize = 128;
/// Observations of one candidate before it is dropped as never settling.
pub const MAX_ATTEMPTS: u32 = 240;
/// A size must stay unchanged this long.
pub const STABLE_SECONDS: i64 = 2;

const TOO_MANY: &str = "This folder has too many files to monitor safely (limit 10,000).";

/// `true` when `path` is not a temporary or hidden name (C# `FinalName`).
pub fn final_name(path: &str) -> bool {
    let name = path.rsplit(['\\', '/']).next().unwrap_or(path);
    let ext = name
        .rfind('.')
        .map(|i| name[i..].to_lowercase())
        .unwrap_or_default();
    !name.starts_with('.')
        && !name.starts_with('~')
        && !matches!(
            ext.as_str(),
            ".tmp" | ".temp" | ".part" | ".partial" | ".crdownload" | ".download" | ".opdownload"
        )
}

/// Length and last write time (milliseconds, UTC) of a file: the "unchanged" signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileSig {
    pub size: u64,
    pub modified_ms: i64,
}

/// The outcome of looking at one candidate file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Probe {
    /// Gone (deleted or renamed away).
    Missing,
    /// Exists and can be opened for reading.
    Ready(FileSig),
    /// Exists but cannot be opened yet (still being written, sharing violation).
    Locked(FileSig),
    /// Access denied: dropped without a notification.
    Denied,
}

/// File-system access injected by the host (and by tests).
pub trait DownloadFs {
    /// Up to `MAX_KNOWN + 1` file paths directly inside the folder.
    fn list(&mut self) -> Result<Vec<String>, String>;
    fn probe(&mut self, path: &str) -> Probe;
    fn folder_exists(&mut self) -> bool;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadError {
    TooManyFiles,
    /// A listing or probe failed; the message is shown to the user.
    Io(String),
    Disconnected,
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooManyFiles => f.write_str(TOO_MANY),
            Self::Io(m) => f.write_str(m),
            Self::Disconnected => f.write_str(
                "Download folder disconnected. Toggle monitoring off and on to restart.",
            ),
        }
    }
}

impl std::error::Error for DownloadError {}

fn fold(path: &str) -> String {
    path.to_uppercase()
}

fn trim_separator(folder: &str) -> &str {
    let t = folder.trim_end_matches(['\\', '/']);
    if t.is_empty() {
        folder
    } else {
        t
    }
}

#[derive(Debug, Clone)]
struct Pending {
    path: String,
    /// Last observed size, `-1` before the first observation.
    size: i64,
    stable: DateTime<Utc>,
    attempts: u32,
}

/// The arrival state machine.
#[derive(Debug, Default)]
pub struct DownloadInference {
    known: HashMap<String, (String, FileSig)>,
    pending: HashMap<String, Pending>,
    running: bool,
    reconcile: bool,
    folder: String,
}

impl DownloadInference {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn running(&self) -> bool {
        self.running
    }

    pub fn folder(&self) -> &str {
        &self.folder
    }

    /// Candidates waiting for their size to settle.
    pub fn candidates(&self) -> usize {
        self.pending.len()
    }

    /// Starts or stops monitoring (C# `Configure`). `baseline` is the folder's current files; it
    /// is recorded as already present. Re-configuring the same folder while running is a no-op.
    pub fn configure(
        &mut self,
        enabled: bool,
        folder: &str,
        baseline: impl IntoIterator<Item = (String, FileSig)>,
    ) -> Result<(), DownloadError> {
        if enabled && self.running && self.folder.eq_ignore_ascii_case(trim_separator(folder)) {
            return Ok(());
        }
        self.stop();
        if !enabled {
            return Ok(());
        }
        let mut known = HashMap::new();
        for (path, sig) in baseline.into_iter().take(MAX_KNOWN + 1) {
            // Overflow fails closed rather than announcing an untracked baseline.
            if known.len() == MAX_KNOWN {
                return Err(DownloadError::TooManyFiles);
            }
            known.insert(fold(&path), (path, sig));
        }
        self.known = known;
        self.folder = trim_separator(folder).to_owned();
        self.running = true;
        self.reconcile = true;
        Ok(())
    }

    /// Stops monitoring and drops all state.
    pub fn stop(&mut self) {
        self.running = false;
        self.known.clear();
        self.pending.clear();
        self.reconcile = false;
        self.folder.clear();
    }

    fn candidate(&mut self, path: &str, now: DateTime<Utc>) {
        if !final_name(path) || !self.running {
            return;
        }
        let key = fold(path);
        if self.known.contains_key(&key) || self.pending.contains_key(&key) {
            return;
        }
        if self.pending.len() < MAX_PENDING {
            self.pending.insert(
                key,
                Pending {
                    path: path.to_owned(),
                    size: -1,
                    stable: now,
                    attempts: 0,
                },
            );
        } else {
            self.reconcile = true;
        }
    }

    /// A watcher Created or Changed event.
    pub fn on_changed(&mut self, path: &str, now: DateTime<Utc>) {
        self.candidate(path, now);
    }

    pub fn on_deleted(&mut self, path: &str) {
        if !self.running {
            return;
        }
        let key = fold(path);
        self.pending.remove(&key);
        self.known.remove(&key);
    }

    pub fn on_renamed(&mut self, old: &str, new: &str, now: DateTime<Utc>) {
        if !self.running {
            return;
        }
        let old_key = fold(old);
        self.pending.remove(&old_key);
        // A completed final file renamed by the user is not another arrival.
        if final_name(old) {
            if let Some((_, sig)) = self.known.remove(&old_key) {
                self.known.insert(fold(new), (new.to_owned(), sig));
                return;
            }
        }
        self.known.remove(&old_key);
        self.candidate(new, now);
    }

    /// Ask for a full folder scan on the next tick (watcher buffer overflow, wake from sleep).
    pub fn request_reconcile(&mut self) {
        if self.running {
            self.reconcile = true;
        }
    }

    fn fail(&mut self, error: DownloadError) -> DownloadError {
        self.stop();
        error
    }

    /// One worker iteration (every 500 ms while monitoring): reconcile if requested, examine
    /// every candidate and return the paths that have arrived, sorted case-insensitively.
    /// On `Err` the monitor has stopped and cleared its state; the host reports the message.
    pub fn tick(
        &mut self,
        now: DateTime<Utc>,
        fs: &mut dyn DownloadFs,
    ) -> Result<Vec<String>, DownloadError> {
        if !self.running {
            return Ok(Vec::new());
        }
        if std::mem::take(&mut self.reconcile) {
            let files = match fs.list() {
                Ok(files) => files,
                Err(message) => return Err(self.fail(DownloadError::Io(message))),
            };
            if files.len() > MAX_KNOWN {
                return Err(self.fail(DownloadError::Io(
                    "Download folder exceeds the 10,000-file monitoring limit.".to_owned(),
                )));
            }
            let present: std::collections::HashSet<String> =
                files.iter().map(|f| fold(f)).collect();
            self.known.retain(|key, _| present.contains(key));
            for path in &files {
                self.candidate(path, now);
            }
        }
        let mut batch = Vec::new();
        let keys: Vec<String> = self.pending.keys().cloned().collect();
        for key in keys {
            let Some(item) = self.pending.get(&key).cloned() else {
                continue;
            };
            let probe = fs.probe(&item.path);
            self.examine(&key, item, probe, now, &mut batch);
        }
        batch.sort_by_key(|p| fold(p));
        if !fs.folder_exists() {
            return Err(self.fail(DownloadError::Disconnected));
        }
        Ok(batch)
    }

    fn examine(
        &mut self,
        key: &str,
        item: Pending,
        probe: Probe,
        now: DateTime<Utc>,
        batch: &mut Vec<String>,
    ) {
        let (sig, locked) = match probe {
            Probe::Missing | Probe::Denied => {
                self.pending.remove(key);
                return;
            }
            Probe::Ready(sig) => (sig, false),
            Probe::Locked(sig) => (sig, true),
        };
        if self.known.get(key).is_some_and(|(_, old)| *old == sig) {
            self.pending.remove(key);
            return;
        }
        if item.attempts >= MAX_ATTEMPTS {
            self.pending.remove(key);
            return;
        }
        if item.size != sig.size as i64 {
            self.pending.insert(
                key.to_owned(),
                Pending {
                    size: sig.size as i64,
                    stable: now,
                    attempts: item.attempts + 1,
                    ..item
                },
            );
            return;
        }
        self.pending.insert(
            key.to_owned(),
            Pending {
                attempts: item.attempts + 1,
                ..item.clone()
            },
        );
        if now - item.stable < Duration::seconds(STABLE_SECONDS) || locked {
            return;
        }
        if self.known.len() >= MAX_KNOWN {
            self.pending.remove(key);
            return;
        }
        self.known.insert(key.to_owned(), (item.path.clone(), sig));
        self.pending.remove(key);
        batch.push(item.path);
    }
}
