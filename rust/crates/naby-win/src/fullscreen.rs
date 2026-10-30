//! Foreground fullscreen / maximized detection (C# `Native.IsExternalWindow`,
//! `FullscreenOn`, `ShouldHideOn`), used to hide the dock over games and videos.

use crate::monitors::RectI;

/// Window classes of the desktop shell; these never count as an application window.
pub fn is_shell_class(class: &str) -> bool {
    matches!(
        class,
        "WorkerW" | "Progman" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd"
    )
}

/// A window covers a monitor when its rectangle contains the monitor's bounds.
pub fn covers(window: RectI, monitor: RectI) -> bool {
    window.left <= monitor.left
        && window.top <= monitor.top
        && window.right >= monitor.right
        && window.bottom >= monitor.bottom
}

/// Facts about an external foreground window, gathered by the OS layer.
#[derive(Debug, Clone, Copy)]
pub struct WindowFacts {
    pub rect: RectI,
    pub zoomed: bool,
    /// The window is on the dock's monitor.
    pub on_dock_monitor: bool,
}

/// C# `ShouldHideOn`: hide when the window is on the dock's monitor and is maximized (and
/// `hide_maximized`) or covers the monitor (and `hide_fullscreen`).
pub fn should_hide(
    facts: &WindowFacts,
    monitor_bounds: RectI,
    hide_fullscreen: bool,
    hide_maximized: bool,
) -> bool {
    if !facts.on_dock_monitor {
        return false;
    }
    (hide_maximized && facts.zoomed) || (hide_fullscreen && covers(facts.rect, monitor_bounds))
}

#[cfg(windows)]
pub use os::*;

#[cfg(windows)]
mod os {
    use super::*;
    use crate::event::HostEvent;
    use crate::monitors::{monitor_of_window, MonitorInfo};
    use crate::util::from_wide;
    use std::cell::{Cell, RefCell};
    use std::sync::mpsc::Sender;
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::System::Threading::GetCurrentProcessId;
    use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetClassNameW, GetDesktopWindow, GetForegroundWindow, GetShellWindow, GetWindowRect,
        GetWindowThreadProcessId, IsIconic, IsWindowVisible, IsZoomed,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        EVENT_OBJECT_LOCATIONCHANGE, EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_MINIMIZEEND,
        OBJID_WINDOW, WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS,
    };

    /// A visible, non-minimized window of another process that is not the desktop or taskbar.
    pub fn is_external_window(hwnd: HWND) -> bool {
        if hwnd.0.is_null() {
            return false;
        }
        // SAFETY: read-only queries on a window handle; stale handles just report false.
        unsafe {
            if hwnd == GetShellWindow()
                || hwnd == GetDesktopWindow()
                || !IsWindowVisible(hwnd).as_bool()
                || IsIconic(hwnd).as_bool()
            {
                return false;
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if pid == GetCurrentProcessId() {
                return false;
            }
            let mut class = [0u16; 128];
            let n = GetClassNameW(hwnd, &mut class);
            n > 0 && !is_shell_class(&from_wide(&class[..n as usize]))
        }
    }

    /// The foreground window if it belongs to another application.
    pub fn external_foreground() -> Option<HWND> {
        // SAFETY: no arguments.
        let hwnd = unsafe { GetForegroundWindow() };
        is_external_window(hwnd).then_some(hwnd)
    }

    fn window_rect(hwnd: HWND) -> Option<RectI> {
        let mut r = RECT::default();
        // SAFETY: out-pointer references a live local.
        unsafe { GetWindowRect(hwnd, &mut r) }.ok()?;
        Some(RectI::new(r.left, r.top, r.right, r.bottom))
    }

    /// Whether the foreground application covers `monitor` (C# `FullscreenOn`).
    pub fn fullscreen_on(monitor: &MonitorInfo) -> bool {
        external_foreground()
            .and_then(window_rect)
            .is_some_and(|r| covers(r, monitor.bounds))
    }

    /// Whether the dock should hide for `window` on `monitor` under the two settings.
    pub fn should_hide_on(
        monitor: &MonitorInfo,
        window: HWND,
        hide_fullscreen: bool,
        hide_maximized: bool,
    ) -> bool {
        if !(hide_fullscreen || hide_maximized) || !is_external_window(window) {
            return false;
        }
        let Some(rect) = window_rect(window) else {
            return false;
        };
        let facts = WindowFacts {
            rect,
            // SAFETY: read-only query.
            zoomed: unsafe { IsZoomed(window) }.as_bool(),
            on_dock_monitor: monitor_of_window(window) == monitor.handle,
        };
        should_hide(&facts, monitor.bounds, hide_fullscreen, hide_maximized)
    }

    thread_local! {
        static HOOK_TX: RefCell<Option<Sender<HostEvent>>> = const { RefCell::new(None) };
        static HOOK_PENDING: Cell<bool> = const { Cell::new(false) };
    }

    unsafe extern "system" fn hook_proc(
        _hook: HWINEVENTHOOK,
        event: u32,
        hwnd: HWND,
        id_object: i32,
        _id_child: i32,
        _thread: u32,
        _time: u32,
    ) {
        // Location changes are chatty; only the foreground window's own frame matters.
        if event == EVENT_OBJECT_LOCATIONCHANGE
            // SAFETY: plain query.
            && (id_object != OBJID_WINDOW.0 || hwnd != unsafe { GetForegroundWindow() })
        {
            return;
        }
        if HOOK_PENDING.with(|p| p.replace(true)) {
            return;
        }
        HOOK_TX.with(|tx| {
            if let Some(tx) = tx.borrow().as_ref() {
                let _ = tx.send(HostEvent::ForegroundChanged);
            }
        });
    }

    /// Event hooks that report foreground changes, minimizes and foreground resizes, so no
    /// polling timer is needed for fullscreen / maximized hiding. Must live on the UI thread
    /// (the hook callbacks arrive through its message loop).
    pub struct ForegroundHooks {
        hooks: Vec<HWINEVENTHOOK>,
    }

    impl ForegroundHooks {
        pub fn install(tx: Sender<HostEvent>) -> Self {
            HOOK_TX.with(|slot| *slot.borrow_mut() = Some(tx));
            HOOK_PENDING.with(|p| p.set(false));
            let ranges = [
                EVENT_SYSTEM_FOREGROUND,
                EVENT_SYSTEM_MINIMIZEEND,
                EVENT_OBJECT_LOCATIONCHANGE,
            ];
            let hooks = ranges
                .into_iter()
                // SAFETY: out-of-context hook; the callback is a plain function.
                .map(|e| unsafe {
                    SetWinEventHook(
                        e,
                        e,
                        None,
                        Some(hook_proc),
                        0,
                        0,
                        WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
                    )
                })
                .filter(|h| !h.is_invalid())
                .collect();
            Self { hooks }
        }

        /// Call after handling `HostEvent::ForegroundChanged` so the next change is reported.
        pub fn acknowledge() {
            HOOK_PENDING.with(|p| p.set(false));
        }
    }

    impl Drop for ForegroundHooks {
        fn drop(&mut self) {
            for hook in self.hooks.drain(..) {
                // SAFETY: unhooks a hook this struct installed.
                let _ = unsafe { UnhookWinEvent(hook) };
            }
            HOOK_TX.with(|slot| *slot.borrow_mut() = None);
        }
    }

    /// Convenience: evaluate the current foreground window.
    pub fn foreground_should_hide(
        monitor: &MonitorInfo,
        hide_fullscreen: bool,
        hide_maximized: bool,
    ) -> bool {
        external_foreground()
            .is_some_and(|w| should_hide_on(monitor, w, hide_fullscreen, hide_maximized))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const M: RectI = RectI::new(0, 0, 1920, 1080);

    #[test]
    fn shell_classes_are_not_applications() {
        for c in [
            "WorkerW",
            "Progman",
            "Shell_TrayWnd",
            "Shell_SecondaryTrayWnd",
        ] {
            assert!(is_shell_class(c));
        }
        assert!(!is_shell_class("Chrome_WidgetWin_1"));
    }

    #[test]
    fn covering_requires_all_four_edges() {
        assert!(covers(RectI::new(0, 0, 1920, 1080), M));
        assert!(covers(RectI::new(-8, -8, 1928, 1088), M));
        assert!(!covers(RectI::new(0, 0, 1920, 1040), M));
        assert!(!covers(RectI::new(10, 0, 1920, 1080), M));
    }

    #[test]
    fn hide_rules_follow_the_two_settings() {
        let fullscreen = WindowFacts {
            rect: M,
            zoomed: false,
            on_dock_monitor: true,
        };
        assert!(should_hide(&fullscreen, M, true, false));
        assert!(!should_hide(&fullscreen, M, false, true));
        let maximized = WindowFacts {
            rect: RectI::new(0, 0, 1920, 1040),
            zoomed: true,
            on_dock_monitor: true,
        };
        assert!(should_hide(&maximized, M, false, true));
        assert!(!should_hide(&maximized, M, true, false));
        let elsewhere = WindowFacts {
            on_dock_monitor: false,
            ..fullscreen
        };
        assert!(!should_hide(&elsewhere, M, true, true));
    }
}
