//! Everything the Win32 plumbing reports to the application loop. Window procedures never run
//! application code: they translate a message into one of these and push it on the channel the
//! loop drains, so no handler can re-enter the application state.

use crate::hotkeys::HotkeyAction;
use crate::tray::TrayCommand;

/// `WM_APP + 1`: posted to the host window by every service's `Waker`.
pub const WM_APP_WAKE: u32 = 0x8001;
/// `WM_APP + 2`: tray icon callback.
pub const WM_APP_TRAY: u32 = 0x8002;

/// Extended additively by later host services.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostEvent {
    /// A background service published a snapshot (`WM_APP_WAKE`); pull and repaint what changed.
    Wake,
    Hotkey(HotkeyAction),
    Tray(TrayCommand),
    /// The tray icon was right-clicked: show its menu.
    TrayMenu,
    /// Explorer restarted; re-add the tray icon.
    TaskbarCreated,
    /// The tray icon was activated (double click): open Settings.
    TrayActivated,
    /// A second launch asked the running instance to open Settings.
    OpenSettings,
    /// `--exit` from another process, or the session is ending.
    Quit,
    /// Monitors, their work areas or the taskbar changed.
    DisplayChanged,
    /// The foreground window changed or moved: re-evaluate fullscreen / maximized hiding.
    ForegroundChanged,
    /// The dock window has fresh input (`LayeredWindow::take_input`).
    Input,
    /// The dock window moved to a monitor with another DPI.
    WindowDpi(u32),
    /// The dock window lost keyboard focus it had taken for text editing.
    TextFocusLost,
}
