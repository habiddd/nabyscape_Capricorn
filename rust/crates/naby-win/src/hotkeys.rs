//! The three configurable global hotkeys (open / close panel, hide / show, tuck).
//!
//! Parsing, conflict detection and the status strings are pure; `Hotkeys` (Windows only)
//! registers them on the host window with `RegisterHotKey`.

use naby_core::settings::Settings;

pub const MOD_ALT: u32 = 0x1;
pub const MOD_CONTROL: u32 = 0x2;
pub const MOD_SHIFT: u32 = 0x4;
pub const MOD_NOREPEAT: u32 = 0x4000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HotkeyAction {
    /// Open / close the panel (C# "Panel").
    Panel,
    Visibility,
    Tuck,
}

impl HotkeyAction {
    pub const ALL: [HotkeyAction; 3] = [Self::Panel, Self::Visibility, Self::Tuck];

    /// `RegisterHotKey` id, also the `WM_HOTKEY` wparam.
    pub fn id(self) -> i32 {
        match self {
            Self::Panel => 1,
            Self::Visibility => 2,
            Self::Tuck => 3,
        }
    }

    pub fn from_id(id: i32) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.id() == id)
    }

    /// The action name `Settings::hotkey_conflict` expects.
    pub fn conflict_name(self) -> &'static str {
        match self {
            Self::Panel => "Panel",
            Self::Visibility => "Visibility",
            Self::Tuck => "Tuck",
        }
    }

    /// Status text when another program already owns the combination.
    pub fn in_use_message(self) -> &'static str {
        match self {
            Self::Panel => "Open / close hotkey is in use. Choose another combination in Settings.",
            Self::Visibility => {
                "Hide / show hotkey is in use. Choose another combination in Settings."
            }
            Self::Tuck => "Tuck hotkey is in use. Choose another combination in Settings.",
        }
    }
}

/// A parsed shortcut: `RegisterHotKey` modifier flags (including `MOD_NOREPEAT`) and key code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HotkeySpec {
    pub modifiers: u32,
    pub vk: u32,
}

/// Modifier flags for the three combinations Settings allows. Anything else falls back to
/// Ctrl+Alt exactly like the C# `Modifiers` switch.
pub fn parse_modifiers(text: &str) -> u32 {
    match text {
        "Ctrl+Shift" => MOD_CONTROL | MOD_SHIFT,
        "Alt+Shift" => MOD_ALT | MOD_SHIFT,
        _ => MOD_CONTROL | MOD_ALT,
    }
}

/// Parses a settings pair such as `("Ctrl+Alt", "I")`. The key must be a single letter A-Z.
pub fn parse(modifiers: &str, key: &str) -> Option<HotkeySpec> {
    let mut chars = key.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else {
        return None;
    };
    if !c.is_ascii_uppercase() {
        return None;
    }
    Some(HotkeySpec {
        modifiers: parse_modifiers(modifiers) | MOD_NOREPEAT,
        vk: c as u32,
    })
}

/// What to do with one hotkey on (re)configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    Off,
    /// Disabled by a conflict or a bad key; the string is the status message.
    Blocked(String),
    Register(HotkeySpec),
}

fn fields(settings: &Settings, action: HotkeyAction) -> (bool, &str, &str) {
    match action {
        HotkeyAction::Panel => (
            settings.hotkey_enabled,
            &settings.hotkey_modifiers,
            &settings.hotkey_key,
        ),
        HotkeyAction::Visibility => (
            settings.visibility_hotkey_enabled,
            &settings.visibility_hotkey_modifiers,
            &settings.visibility_hotkey_key,
        ),
        HotkeyAction::Tuck => (
            settings.tuck_hotkey_enabled,
            &settings.tuck_hotkey_modifiers,
            &settings.tuck_hotkey_key,
        ),
    }
}

/// Decides, before touching the OS, which hotkeys get registered (C# `ConfigureHotkey`).
pub fn plan(settings: &Settings) -> [Plan; 3] {
    HotkeyAction::ALL.map(|action| {
        let (enabled, modifiers, key) = fields(settings, action);
        if !enabled {
            return Plan::Off;
        }
        let conflict = settings.hotkey_conflict(action.conflict_name());
        if !conflict.is_empty() {
            return Plan::Blocked(conflict);
        }
        match parse(modifiers, key) {
            Some(spec) => Plan::Register(spec),
            None => Plan::Blocked("Choose a letter from A to Z for this hotkey.".to_string()),
        }
    })
}

/// The three status lines Settings shows (C# `HotkeyStatus`, `VisibilityHotkeyStatus`,
/// `TuckHotkeyStatus`); `errors` are the messages from the last configuration, in
/// `HotkeyAction::ALL` order, empty when fine.
pub fn status_lines(settings: &Settings, errors: &[String; 3]) -> [String; 3] {
    let line = |action: HotkeyAction, off: &str, label: &str| {
        let (enabled, modifiers, key) = fields(settings, action);
        let error = &errors[action as usize];
        if !enabled {
            off.to_string()
        } else if !error.is_empty() {
            error.clone()
        } else {
            format!("{label}{modifiers}+{key}")
        }
    };
    [
        line(HotkeyAction::Panel, "Hotkey off", "Active \u{b7} "),
        line(
            HotkeyAction::Visibility,
            "Visibility shortcut off",
            "Hide / show \u{b7} ",
        ),
        line(
            HotkeyAction::Tuck,
            "Tuck shortcut off",
            "Tuck / return to automatic \u{b7} ",
        ),
    ]
}

#[cfg(windows)]
pub use os::Hotkeys;

#[cfg(windows)]
mod os {
    use super::*;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS,
    };

    /// Registers the configured hotkeys on one window and remembers the failure messages.
    #[derive(Default)]
    pub struct Hotkeys {
        hwnd: Option<HWND>,
        registered: [bool; 3],
        errors: [String; 3],
    }

    impl Hotkeys {
        pub fn new() -> Self {
            Self::default()
        }

        /// Unregisters everything, then registers what `settings` asks for. A key another
        /// program owns yields the C# "... is in use" message instead of an error.
        pub fn configure(&mut self, hwnd: HWND, settings: &Settings) {
            self.unregister_all();
            self.hwnd = Some(hwnd);
            for (action, plan) in HotkeyAction::ALL.into_iter().zip(plan(settings)) {
                let slot = action as usize;
                match plan {
                    Plan::Off => {}
                    Plan::Blocked(message) => self.errors[slot] = message,
                    Plan::Register(spec) => {
                        // SAFETY: plain Win32 call on a window owned by this thread.
                        let ok = unsafe {
                            RegisterHotKey(
                                Some(hwnd),
                                action.id(),
                                HOT_KEY_MODIFIERS(spec.modifiers),
                                spec.vk,
                            )
                        }
                        .is_ok();
                        self.registered[slot] = ok;
                        if !ok {
                            self.errors[slot] = action.in_use_message().to_string();
                        }
                    }
                }
            }
        }

        pub fn unregister_all(&mut self) {
            if let Some(hwnd) = self.hwnd {
                for action in HotkeyAction::ALL {
                    if self.registered[action as usize] {
                        // SAFETY: unregisters an id this window registered.
                        let _ = unsafe { UnregisterHotKey(Some(hwnd), action.id()) };
                    }
                }
            }
            self.registered = [false; 3];
            self.errors = Default::default();
        }

        pub fn errors(&self) -> &[String; 3] {
            &self.errors
        }

        pub fn status_lines(&self, settings: &Settings) -> [String; 3] {
            status_lines(settings, &self.errors)
        }
    }

    impl Drop for Hotkeys {
        fn drop(&mut self) {
            self.unregister_all();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use naby_core::settings::presets;

    #[test]
    fn parses_the_three_modifier_pairs_and_letters() {
        let spec = parse("Ctrl+Alt", "I").unwrap();
        assert_eq!(spec.modifiers, MOD_CONTROL | MOD_ALT | MOD_NOREPEAT);
        assert_eq!(spec.vk, 'I' as u32);
        assert_eq!(parse("Ctrl+Shift", "Z").unwrap().modifiers & 0xF, 6);
        assert_eq!(parse("Alt+Shift", "A").unwrap().modifiers & 0xF, 5);
        // Unknown combinations fall back to Ctrl+Alt (C# `Modifiers`).
        assert_eq!(parse("Win+Q", "Q").unwrap().modifiers & 0xF, 3);
    }

    #[test]
    fn rejects_anything_but_one_capital_letter() {
        for bad in ["", "i", "II", "1", "\u{e9}", " "] {
            assert!(parse("Ctrl+Alt", bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn default_settings_register_three_distinct_hotkeys() {
        let settings = Settings::default();
        let plans = plan(&settings);
        let specs: Vec<_> = plans
            .iter()
            .map(|p| match p {
                Plan::Register(s) => (s.modifiers, s.vk),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(specs[0].1, 'I' as u32);
        assert_eq!(specs[1].1, 'H' as u32);
        assert_eq!(specs[2].1, 'T' as u32);
    }

    #[test]
    fn identical_enabled_shortcuts_block_both_with_the_c_sharp_text() {
        let mut settings = Settings::default();
        settings.visibility_hotkey_key = settings.hotkey_key.clone();
        let plans = plan(&settings);
        assert_eq!(
            plans[0],
            Plan::Blocked(
                "Panel shortcut conflicts with Visibility. Choose a different key or modifiers."
                    .to_string()
            )
        );
        assert!(
            matches!(&plans[1], Plan::Blocked(m) if m.starts_with("Visibility shortcut conflicts with Panel"))
        );
        assert!(matches!(plans[2], Plan::Register(_)));
    }

    #[test]
    fn disabled_hotkeys_are_off_and_do_not_conflict() {
        let mut settings = Settings::default();
        settings.visibility_hotkey_key = settings.hotkey_key.clone();
        settings.visibility_hotkey_enabled = false;
        let plans = plan(&settings);
        assert!(matches!(plans[0], Plan::Register(_)));
        assert_eq!(plans[1], Plan::Off);
    }

    #[test]
    fn status_lines_match_app_cs() {
        let settings = presets::recommended();
        let none: [String; 3] = Default::default();
        let lines = status_lines(&settings, &none);
        assert_eq!(
            lines[0],
            format!(
                "Active \u{b7} {}+{}",
                settings.hotkey_modifiers, settings.hotkey_key
            )
        );
        assert!(lines[1].starts_with("Hide / show \u{b7} "));
        assert!(lines[2].starts_with("Tuck / return to automatic \u{b7} "));
        let errors = [String::new(), "boom".to_string(), String::new()];
        assert_eq!(status_lines(&settings, &errors)[1], "boom");
        let mut off = settings.clone();
        off.hotkey_enabled = false;
        off.visibility_hotkey_enabled = false;
        off.tuck_hotkey_enabled = false;
        let lines = status_lines(&off, &errors);
        assert_eq!(
            lines,
            ["Hotkey off", "Visibility shortcut off", "Tuck shortcut off"]
        );
    }

    #[test]
    fn in_use_messages_match_the_c_sharp_strings() {
        assert_eq!(
            HotkeyAction::Tuck.in_use_message(),
            "Tuck hotkey is in use. Choose another combination in Settings."
        );
        assert!(HotkeyAction::Panel
            .in_use_message()
            .starts_with("Open / close hotkey is in use"));
    }

    #[test]
    fn ids_round_trip() {
        for a in HotkeyAction::ALL {
            assert_eq!(HotkeyAction::from_id(a.id()), Some(a));
        }
        assert_eq!(HotkeyAction::from_id(0), None);
    }
}
