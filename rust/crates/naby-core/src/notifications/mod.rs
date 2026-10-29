//! Ported from `Application/InternalNotifications.cs`.
//!
//! The in-dock notice queue and delivery routing. Ephemeral and owned by the UI thread;
//! Windows toast delivery is returned to the host as data, so delivery errors can never feed
//! back into the queue.

#[cfg(test)]
mod tests;

use std::collections::HashSet;

use chrono::{DateTime, Duration, Utc};

use crate::settings::Settings;

/// Most notices kept in the dock queue.
pub const QUEUE_LIMIT: usize = 10;
/// Most files carried by one downloads notice.
pub const MAX_FILES: usize = 40;
/// Download notices posted closer together than this merge into one.
pub const DOWNLOAD_MERGE_SECONDS: i64 = 3;

/// Which preference gates a notice (C# `InternalNotice.Kind` strings).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeKind {
    Timer,
    Focus,
    Downloads,
    Updates,
    /// Failures and any unrecognized kind.
    Failure,
}

impl NoticeKind {
    /// Parses the C# kind string: `timer`, `focus`, `downloads`, `updates`, otherwise failure.
    pub fn parse(kind: &str) -> Self {
        match kind {
            "timer" => Self::Timer,
            "focus" => Self::Focus,
            "downloads" => Self::Downloads,
            "updates" => Self::Updates,
            _ => Self::Failure,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InternalNotice {
    pub kind: NoticeKind,
    pub title: String,
    pub detail: String,
    pub files: Option<Vec<String>>,
}

impl InternalNotice {
    pub fn new(kind: NoticeKind, title: &str, detail: &str) -> Self {
        Self {
            kind,
            title: title.to_owned(),
            detail: detail.to_owned(),
            files: None,
        }
    }

    pub fn with_files(kind: NoticeKind, title: &str, detail: &str, files: Vec<String>) -> Self {
        Self {
            files: Some(files),
            ..Self::new(kind, title, detail)
        }
    }
}

/// What a [`Notifications::post`] asks the host to do besides the dock queue.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Delivery {
    /// Notices to hand to Windows notifications, in order (a large arrival is split in chunks).
    pub windows: Vec<InternalNotice>,
    /// The dock queue changed and the overlay should repaint.
    pub changed: bool,
}

/// C# `InternalNotifications.Enabled`.
pub fn enabled(s: &Settings, kind: NoticeKind) -> bool {
    match kind {
        NoticeKind::Timer => s.notify_timer,
        NoticeKind::Focus => s.notify_focus,
        NoticeKind::Downloads => s.notify_downloads,
        NoticeKind::Updates => s.notify_updates,
        NoticeKind::Failure => s.notify_failures,
    }
}

/// C# `InternalNotifications.Route`: `(dock, windows)` for a delivery mode.
pub fn route(mode: &str, hidden: bool) -> (bool, bool) {
    match mode {
        "Windows" => (false, true),
        "Both" => (!hidden, true),
        "Off" => (false, false),
        _ => (!hidden, hidden),
    }
}

/// `StringComparer.OrdinalIgnoreCase`-style key.
fn fold(path: &str) -> String {
    path.to_uppercase()
}

/// Case-insensitive distinct, first occurrence wins, original order.
fn distinct<'a>(files: impl IntoIterator<Item = &'a String>) -> Vec<String> {
    let mut seen = HashSet::new();
    files
        .into_iter()
        .filter(|f| seen.insert(fold(f)))
        .cloned()
        .collect()
}

fn downloads_title(count: usize) -> String {
    if count == 1 {
        "Download ready".to_owned()
    } else {
        format!("{count} downloads ready")
    }
}

#[derive(Debug, Clone)]
struct Entry {
    notice: InternalNotice,
    at: DateTime<Utc>,
    displayed: bool,
}

#[derive(Debug, Default)]
pub struct Notifications {
    queue: Vec<Entry>,
    last_download_at: Option<DateTime<Utc>>,
}

impl Notifications {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn count(&self) -> usize {
        self.queue.len()
    }

    /// The notice currently shown (front of the queue).
    pub fn current(&self) -> Option<&InternalNotice> {
        self.queue.first().map(|e| &e.notice)
    }

    /// Freezes the front notice: later arrivals queue behind it instead of merging into it.
    pub fn mark_displayed(&mut self) {
        if let Some(e) = self.queue.first_mut() {
            e.displayed = true;
        }
    }

    /// C# `Post`. Quiet hours and disabled kinds suppress everything; otherwise the notice is
    /// routed to the dock queue, Windows, both or neither according to
    /// `settings.notification_delivery`.
    pub fn post(
        &mut self,
        settings: &Settings,
        notice: InternalNotice,
        hidden: bool,
        now: DateTime<Utc>,
    ) -> Delivery {
        let mut delivery = Delivery::default();
        self.post_into(settings, notice, hidden, now, &mut delivery);
        delivery
    }

    fn post_into(
        &mut self,
        settings: &Settings,
        notice: InternalNotice,
        hidden: bool,
        now: DateTime<Utc>,
        delivery: &mut Delivery,
    ) {
        if !enabled(settings, notice.kind) || settings.quiet(now) {
            return;
        }
        // A watcher iteration can deliver many arrivals. Bound each payload before either
        // delivery route, so every original stays reachable after dismissal.
        if notice.kind == NoticeKind::Downloads {
            if let Some(files) = notice.files.as_ref().filter(|f| f.len() > MAX_FILES) {
                for chunk in distinct(files).chunks(MAX_FILES) {
                    let chunk_notice = InternalNotice::with_files(
                        NoticeKind::Downloads,
                        &downloads_title(chunk.len()),
                        &notice.detail,
                        chunk.to_vec(),
                    );
                    self.post_into(settings, chunk_notice, hidden, now, delivery);
                }
                return;
            }
        }
        let (dock, windows) = route(&settings.notification_delivery, hidden);
        if windows {
            delivery.windows.push(notice.clone());
        }
        if !dock {
            return;
        }
        let merged = if notice.kind == NoticeKind::Downloads {
            self.merge_target(&notice, now)
        } else {
            None
        };
        match merged {
            Some(files) => {
                let last = self.queue.len() - 1;
                self.queue[last] = Entry {
                    notice: InternalNotice::with_files(
                        NoticeKind::Downloads,
                        &downloads_title(files.len()),
                        &notice.detail,
                        files,
                    ),
                    at: now,
                    displayed: false,
                };
            }
            None => {
                if self.queue.len() == QUEUE_LIMIT {
                    self.queue.remove(QUEUE_LIMIT - 1);
                }
                self.queue.push(Entry {
                    notice: notice.clone(),
                    at: now,
                    displayed: false,
                });
            }
        }
        if notice.kind == NoticeKind::Downloads {
            self.last_download_at = Some(now);
        }
        delivery.changed = true;
    }

    /// The merged file list when `notice` should fold into the last queued downloads notice.
    fn merge_target(&self, notice: &InternalNotice, now: DateTime<Utc>) -> Option<Vec<String>> {
        let last = self.queue.last()?;
        let recent = self
            .last_download_at
            .is_some_and(|t| now - t < Duration::seconds(DOWNLOAD_MERGE_SECONDS));
        if last.notice.kind != NoticeKind::Downloads || last.displayed || !recent {
            return None;
        }
        let empty = Vec::new();
        let merged = distinct(
            last.notice
                .files
                .as_ref()
                .unwrap_or(&empty)
                .iter()
                .chain(notice.files.as_ref().unwrap_or(&empty).iter()),
        );
        (merged.len() <= MAX_FILES).then_some(merged)
    }

    /// C# `Tick`: while the overlay is busy the front notice's clock restarts; otherwise it
    /// expires after `seconds` (the `NotificationSeconds` preference, 12 by default).
    /// Returns true when a notice was removed.
    pub fn tick(&mut self, now: DateTime<Utc>, busy: bool, seconds: f64) -> bool {
        let Some(front) = self.queue.first_mut() else {
            return false;
        };
        if busy {
            front.at = now;
            return false;
        }
        let limit = Duration::milliseconds((seconds.max(0.0) * 1000.0) as i64);
        if now - front.at > limit {
            self.queue.remove(0);
            return true;
        }
        false
    }

    /// Removes the front notice. Returns true if there was one.
    pub fn dismiss(&mut self) -> bool {
        if self.queue.is_empty() {
            return false;
        }
        self.queue.remove(0);
        true
    }

    pub fn clear(&mut self) {
        self.queue.clear();
    }
}
