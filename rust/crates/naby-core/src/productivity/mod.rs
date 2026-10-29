//! Productivity models and policies ported from the C# `Features/Productivity`,
//! `Features/Activities`, `Features/Interaction` and `UI/Island/VisualCalendar` sources.
//!
//! Everything here is platform-free. Clipboard listening, the keep-awake syscall, drag and drop
//! and persistence are host concerns: the host feeds events in and persists the documents named by
//! the `DOCUMENT_NAME` constants (same PascalCase JSON the C# build writes).

pub mod activity;
pub mod awake;
pub mod calendar;
pub mod clipboard;
pub mod focus;
pub mod links;
pub mod notes;
pub mod service;
pub mod shelf;
pub mod stopwatch;
pub mod tasks;
pub mod timer;

pub use activity::{active_countdown, countdown_summary, ActiveCountdown};
pub use awake::{AwakeError, KeepAwakeState};
pub use calendar::{month_grid, week_of, Calendar, CalendarError, CalendarEvent};
pub use clipboard::{classify, smart_classify, ClipItem, ClipKind, ClipboardHistory, SmartClip};
pub use focus::{check_countdowns, CountdownReport, FocusConfig, FocusEvent, FocusSession};
pub use links::{clean_link, try_clean_link, LinkError};
pub use service::{ProductivityData, ProductivityService};
pub use shelf::ShelfItem;
pub use stopwatch::{Clock, Clocks, Stopwatch, StopwatchState};
pub use tasks::TaskItem;
pub use timer::{format_seconds, Countdown, Timer};

use serde::{Deserialize, Deserializer};
use std::sync::atomic::{AtomicU64, Ordering};

/// Generates a 32-hex-digit id, the shape of `Guid.NewGuid().ToString("N")`.
pub(crate) fn new_id() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut parts = [0u64; 2];
    for (i, part) in parts.iter_mut().enumerate() {
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_u64(n);
        hasher.write_u64(nanos);
        hasher.write_u64(i as u64);
        *part = hasher.finish();
    }
    format!("{:016x}{:016x}", parts[0], parts[1])
}

/// `null` becomes the default value (System.Text.Json writes null for unset reference fields).
pub(crate) fn null_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

/// A list that tolerates `null` elements and a `null` list, like the C# normalization does.
pub(crate) fn list_skip_null<'de, D, T>(d: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Option::<Vec<Option<T>>>::deserialize(d)?
        .unwrap_or_default()
        .into_iter()
        .flatten()
        .collect())
}
