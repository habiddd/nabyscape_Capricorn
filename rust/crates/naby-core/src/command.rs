//! Commands emitted by views and executed by the platform host.
//!
//! Views never call Win32 directly; they push `Command`s and the host executes them against its
//! services. Productivity data (notes, tasks, shelf, clipboard history) and the local calendar are
//! the exception: views mutate the shared `ProductivityService` / `CalendarHandle` handles they
//! find in the snapshots, because those mutations are pure (the services persist themselves) and
//! need synchronous results (validation messages). Everything that touches the OS, a worker thread
//! or another window is a `Command`.
//!
//! Later tasks extend this enum by adding variants only.

use crate::media::{MixerScope, MixerTarget};
use crate::notifications::NoticeKind;
use crate::settings::Settings;

/// Which media session the dock follows (`MediaService.Select*`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaSource {
    /// "None": no source, no transport commands are issued.
    None,
    /// "Auto": follow whichever session is playing.
    Auto,
    /// A fixed app, by its app user model id.
    App(String),
}

/// A user intent that needs the platform (media keys, audio endpoints, shell).
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    // ---- windows / shell ----------------------------------------------------------------
    OpenSettings,
    /// Open Settings at a page id (`"home"`, `"widgets"`, ...; unknown ids open the home page).
    OpenSettingsPage(String),
    Quit,
    /// Open the assistant window (chat).
    OpenAssistant,
    /// Open the voice room.
    OpenVoice,
    /// Show the dock's panel at a widget or page id (assistant navigation tool, command bar).
    NavigatePage(String),
    /// Open a hover page in its own floating preview window (`app.Previews.OpenAt`).
    DetachPreview {
        page: String,
        at: (i32, i32),
        dragging: bool,
    },

    // ---- dock ---------------------------------------------------------------------------
    /// Toggle the manual tuck latch (tuck hotkey, command bar, tray).
    ToggleTuck,
    /// Hide or show the whole dock (visibility hotkey, tray).
    ToggleVisibility,
    /// Persist and apply new preferences (dock move/resize commit, widget toggles, lyrics switch).
    /// The host normalizes, saves `settings.json` and republishes the snapshot.
    ApplySettings(Box<Settings>),
    /// Dock editing mode (drag handles) started or ended; the host stops follow/tuck while on.
    SetDockEditing(bool),

    // ---- media --------------------------------------------------------------------------
    MediaToggle,
    MediaPlay,
    MediaPause,
    MediaNext,
    MediaPrevious,
    /// Seek to a fraction (0..=1) of the track.
    MediaSeek(f64),
    MediaSelectSource(MediaSource),

    // ---- audio --------------------------------------------------------------------------
    /// Master volume scalar 0..=1.
    SetVolume(f64),
    ToggleMute,
    ToggleMicMute,
    /// Switch the default output endpoint by id.
    SetOutput(String),
    /// Re-read volume, devices and microphone now (menu opened, popover shown).
    RefreshAudio,
    MixerRefresh,
    MixerSet {
        target: MixerTarget,
        level: Option<f64>,
        muted: Option<bool>,
    },
    MixerAdjust {
        scope: MixerScope,
        delta: f64,
    },

    // ---- timer / focus / stopwatch ------------------------------------------------------
    TimerStart {
        seconds: f64,
    },
    /// Pause when running, resume when paused, otherwise start the default five minutes.
    TimerToggle,
    TimerPause,
    TimerResume,
    TimerReset,
    /// Add time to the running or paused timer (starts one when idle).
    TimerExtend {
        seconds: f64,
    },
    /// Pause when running, resume when paused, otherwise start a work phase.
    FocusToggle,
    FocusReset,
    FocusExtend {
        seconds: f64,
    },
    StopwatchToggle,
    StopwatchReset,

    // ---- notifications ------------------------------------------------------------------
    /// Dismiss the notice on screen.
    NoticeDismiss,
    /// Dismiss it and show the next queued notice.
    NoticeNext,
    NoticeClearAll,
    /// A view reports something the user should see (routing, quiet hours and kind gates apply).
    Notify {
        kind: NoticeKind,
        title: String,
        detail: String,
        files: Option<Vec<String>>,
    },

    // ---- files, clipboard, links --------------------------------------------------------
    OpenPath(String),
    OpenUrl(String),
    RevealInFolder(String),
    /// Put text on the clipboard.
    CopyText(String),
    /// Put a cached clipboard-history image back on the clipboard.
    CopyClipImage(String),
    /// Stage files on the shelf (also what the drag-drop service posts).
    ShelfAddFiles(Vec<String>),

    // ---- launcher -----------------------------------------------------------------------
    /// Launch a configured shortcut (`ShortcutService.Open`): a path, URL, or packaged app id.
    LaunchShortcut {
        target: String,
        app_id: Option<String>,
    },

    // ---- keep awake ---------------------------------------------------------------------
    AwakeStart {
        minutes: Option<u32>,
        keep_display_on: bool,
    },
    AwakeStop,

    // ---- lyrics, weather ----------------------------------------------------------------
    /// Fetch the lyrics of the current track again.
    LyricsRetry,
    /// Load a local `.lrc` file for the current track (this session only).
    LyricsImport {
        path: String,
    },
    /// Refresh the weather now (`force` skips the 20 minute cache).
    WeatherRefresh {
        force: bool,
    },
}
