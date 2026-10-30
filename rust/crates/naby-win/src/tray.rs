//! Notification-area icon, its context menu and balloon notifications (C# `NotifyIcon` and
//! `ContextMenuStrip` in `App.OnStartup`).

/// Tooltip of the icon.
pub const TOOLTIP: &str = "NabyScape";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrayCommand {
    OpenIsland,
    CommandBar,
    Settings,
    MoveResize,
    Assistant,
    ToggleWake,
    StopMicrophones,
    Tuck,
    HideShow,
    Quit,
}

impl TrayCommand {
    const ALL: [TrayCommand; 10] = [
        Self::OpenIsland,
        Self::CommandBar,
        Self::Settings,
        Self::MoveResize,
        Self::Assistant,
        Self::ToggleWake,
        Self::StopMicrophones,
        Self::Tuck,
        Self::HideShow,
        Self::Quit,
    ];

    /// Menu item id (never 0, which means "dismissed").
    pub fn id(self) -> u32 {
        100 + self as u32
    }

    pub fn from_id(id: u32) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.id() == id)
    }
}

/// State the wake-word item needs (C# `menu.Opening`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WakeMenu {
    /// The wake word is currently armed, so the item offers to pause it.
    pub armed: bool,
    /// The assistant and its wake word are switched on in Settings.
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuEntry {
    Item {
        command: TrayCommand,
        label: &'static str,
        enabled: bool,
    },
    Separator,
}

/// The tray menu exactly as `App.cs` builds it, in order.
pub fn menu_entries(wake: WakeMenu) -> Vec<MenuEntry> {
    let item = |command, label| MenuEntry::Item {
        command,
        label,
        enabled: true,
    };
    vec![
        item(TrayCommand::OpenIsland, "Open island"),
        item(TrayCommand::CommandBar, "Command bar"),
        item(TrayCommand::Settings, "Settings"),
        item(TrayCommand::MoveResize, "Move & resize island"),
        item(TrayCommand::Assistant, "AI companion"),
        MenuEntry::Item {
            command: TrayCommand::ToggleWake,
            label: if wake.armed {
                "Pause wake word"
            } else {
                "Resume wake word"
            },
            enabled: wake.enabled,
        },
        item(TrayCommand::StopMicrophones, "Stop all microphones"),
        item(TrayCommand::Tuck, "Tuck / Return to automatic"),
        item(TrayCommand::HideShow, "Hide / show"),
        MenuEntry::Separator,
        item(TrayCommand::Quit, "Quit"),
    ]
}

/// Win32 menu strings treat `&` as the mnemonic marker; double it to show a literal one.
pub fn escape_menu_label(label: &str) -> String {
    label.replace('&', "&&")
}

/// Tooltip while a microphone is live (C# `RefreshAssistantTray`).
pub fn tooltip_for(voice_connected: bool, wake_listening: bool, wake_armed: bool) -> &'static str {
    if voice_connected {
        "NabyScape \u{b7} Voice microphone on"
    } else if wake_listening {
        "NabyScape \u{b7} Wake microphone on"
    } else if wake_armed {
        "NabyScape \u{b7} Wake word preparing"
    } else {
        TOOLTIP
    }
}

#[cfg(windows)]
pub use os::Tray;

#[cfg(windows)]
mod os {
    use super::*;
    use crate::event::WM_APP_TRAY;
    use crate::util::{copy_into_fixed, wide};
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::Shell::{
        Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIIF_INFO,
        NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION, NOTIFYICONDATAW, NOTIFYICON_VERSION_4,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, GetSystemMetrics, LoadIconW,
        LoadImageW, PostMessageW, SetForegroundWindow, TrackPopupMenu, HICON, HMENU,
        IDI_APPLICATION, IMAGE_ICON, LR_DEFAULTCOLOR, MF_DISABLED, MF_GRAYED, MF_SEPARATOR,
        MF_STRING, SM_CXSMICON, SM_CYSMICON, TPM_BOTTOMALIGN, TPM_NONOTIFY, TPM_RETURNCMD,
        TPM_RIGHTBUTTON, WM_NULL,
    };

    const ICON_ID: u32 = 1;
    const TRAY_UID: u32 = 1;

    pub struct Tray {
        hwnd: HWND,
        icon: HICON,
        tooltip: String,
        added: bool,
    }

    impl Tray {
        /// Adds the icon. Fails only when the shell refuses (e.g. no taskbar yet); callers
        /// may call `readd` after `TaskbarCreated`.
        pub fn new(hwnd: HWND, tooltip: &str) -> Tray {
            let mut tray = Tray {
                hwnd,
                icon: load_icon(),
                tooltip: tooltip.to_string(),
                added: false,
            };
            tray.readd();
            tray
        }

        fn data(&self) -> NOTIFYICONDATAW {
            let mut data = NOTIFYICONDATAW {
                cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: self.hwnd,
                uID: TRAY_UID,
                ..Default::default()
            };
            data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            data
        }

        /// (Re-)adds the icon, after startup or an Explorer restart.
        pub fn readd(&mut self) {
            let mut data = self.data();
            data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
            data.uCallbackMessage = WM_APP_TRAY;
            data.hIcon = self.icon;
            copy_into_fixed(&mut data.szTip, &self.tooltip);
            // SAFETY: `data` is fully initialized and lives across the calls.
            unsafe {
                self.added = Shell_NotifyIconW(NIM_ADD, &data).as_bool();
                if self.added {
                    let _ = Shell_NotifyIconW(NIM_SETVERSION, &data);
                }
            }
        }

        pub fn set_tooltip(&mut self, text: &str) {
            if self.tooltip == text {
                return;
            }
            self.tooltip = text.to_string();
            if !self.added {
                return;
            }
            let mut data = self.data();
            data.uFlags = NIF_TIP | NIF_SHOWTIP;
            copy_into_fixed(&mut data.szTip, text);
            // SAFETY: `data` is fully initialized.
            let _ = unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) };
        }

        /// Shows a balloon / toast (C# `ShowBalloonTip(4500, title, detail, Info)`; Windows
        /// 10 chooses the duration itself).
        pub fn balloon(&self, title: &str, text: &str) {
            if !self.added {
                return;
            }
            let mut data = self.data();
            data.uFlags = NIF_INFO;
            data.dwInfoFlags = NIIF_INFO;
            copy_into_fixed(&mut data.szInfoTitle, title);
            copy_into_fixed(&mut data.szInfo, text);
            // SAFETY: `data` is fully initialized.
            let _ = unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) };
        }

        /// Shows the context menu at the pointer and returns the chosen command. Blocks in
        /// the menu's own message loop until the user chooses or dismisses.
        pub fn show_menu(&self, wake: WakeMenu) -> Option<TrayCommand> {
            // SAFETY: the menu is created, used and destroyed inside this call.
            unsafe {
                let menu: HMENU = CreatePopupMenu().ok()?;
                for entry in menu_entries(wake) {
                    match entry {
                        MenuEntry::Separator => {
                            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
                        }
                        MenuEntry::Item {
                            command,
                            label,
                            enabled,
                        } => {
                            let text = wide(&escape_menu_label(label));
                            let flags = if enabled {
                                MF_STRING
                            } else {
                                MF_STRING | MF_GRAYED | MF_DISABLED
                            };
                            let _ = AppendMenuW(
                                menu,
                                flags,
                                command.id() as usize,
                                PCWSTR(text.as_ptr()),
                            );
                        }
                    }
                }
                let mut at = POINT::default();
                let _ = GetCursorPos(&mut at);
                // Required so the menu closes when the user clicks elsewhere.
                let _ = SetForegroundWindow(self.hwnd);
                let chosen = TrackPopupMenu(
                    menu,
                    TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN,
                    at.x,
                    at.y,
                    None,
                    self.hwnd,
                    None,
                );
                let _ = PostMessageW(Some(self.hwnd), WM_NULL, WPARAM(0), LPARAM(0));
                let _ = DestroyMenu(menu);
                TrayCommand::from_id(chosen.0 as u32)
            }
        }

        pub fn remove(&mut self) {
            if self.added {
                let data = self.data();
                // SAFETY: `data` identifies the icon this struct added.
                let _ = unsafe { Shell_NotifyIconW(NIM_DELETE, &data) };
                self.added = false;
            }
        }
    }

    impl Drop for Tray {
        fn drop(&mut self) {
            self.remove();
        }
    }

    fn load_icon() -> HICON {
        // SAFETY: loads resource 1 (the app icon embedded by build.rs) from this module; the
        // fallback is a stock icon.
        unsafe {
            let size = (GetSystemMetrics(SM_CXSMICON), GetSystemMetrics(SM_CYSMICON));
            GetModuleHandleW(None)
                .ok()
                .and_then(|module| {
                    LoadImageW(
                        Some(module.into()),
                        PCWSTR(ICON_ID as usize as *const u16),
                        IMAGE_ICON,
                        size.0,
                        size.1,
                        LR_DEFAULTCOLOR,
                    )
                    .ok()
                })
                .map(|h| HICON(h.0))
                .or_else(|| LoadIconW(None, IDI_APPLICATION).ok())
                .unwrap_or_default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_matches_app_cs_order_and_labels() {
        let labels: Vec<_> = menu_entries(WakeMenu::default())
            .into_iter()
            .map(|e| match e {
                MenuEntry::Item { label, .. } => label,
                MenuEntry::Separator => "-",
            })
            .collect();
        assert_eq!(
            labels,
            [
                "Open island",
                "Command bar",
                "Settings",
                "Move & resize island",
                "AI companion",
                "Resume wake word",
                "Stop all microphones",
                "Tuck / Return to automatic",
                "Hide / show",
                "-",
                "Quit"
            ]
        );
    }

    #[test]
    fn wake_item_toggles_label_and_enabled_state() {
        let wake = |armed, enabled| {
            menu_entries(WakeMenu { armed, enabled })
                .into_iter()
                .find_map(|e| match e {
                    MenuEntry::Item {
                        command: TrayCommand::ToggleWake,
                        label,
                        enabled,
                    } => Some((label, enabled)),
                    _ => None,
                })
                .unwrap()
        };
        assert_eq!(wake(false, false), ("Resume wake word", false));
        assert_eq!(wake(true, true), ("Pause wake word", true));
    }

    #[test]
    fn ids_are_unique_nonzero_and_round_trip() {
        let mut seen = std::collections::HashSet::new();
        for c in TrayCommand::ALL {
            assert!(c.id() != 0 && seen.insert(c.id()));
            assert_eq!(TrayCommand::from_id(c.id()), Some(c));
        }
        assert_eq!(TrayCommand::from_id(0), None);
    }

    #[test]
    fn ampersands_are_escaped_for_win32_menus() {
        assert_eq!(
            escape_menu_label("Move & resize island"),
            "Move && resize island"
        );
        assert_eq!(escape_menu_label("Quit"), "Quit");
    }

    #[test]
    fn tooltip_prefers_the_live_microphone() {
        assert_eq!(
            tooltip_for(true, true, true),
            "NabyScape \u{b7} Voice microphone on"
        );
        assert_eq!(
            tooltip_for(false, true, true),
            "NabyScape \u{b7} Wake microphone on"
        );
        assert_eq!(
            tooltip_for(false, false, true),
            "NabyScape \u{b7} Wake word preparing"
        );
        assert_eq!(tooltip_for(false, false, false), "NabyScape");
    }
}
