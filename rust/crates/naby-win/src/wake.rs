//! How background services tell the UI thread that fresh data is ready.

use std::sync::Arc;

/// Called from any thread after a service stored a new snapshot.
/// The app shell implements it with `PostMessageW(hwnd, WM_APP_WAKE, ..)`;
/// the UI thread then pulls snapshots it cares about. Calls coalesce, so
/// services may call it as often as data changes.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// A waker that does nothing (tests, services created before the window exists).
pub fn noop() -> Waker {
    Arc::new(|| {})
}
