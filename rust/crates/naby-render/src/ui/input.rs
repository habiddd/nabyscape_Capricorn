//! Per-frame input snapshot fed to the immediate-mode UI by the host window.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    Tab,
    Enter,
    Space,
    Escape,
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    /// A letter or digit key, lower-case (used with Ctrl for A/C/X/V shortcuts).
    Char(char),
}

/// A key-down (or auto-repeat) event with the modifiers held at the time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyEvent {
    pub key: Key,
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

impl KeyEvent {
    pub const fn new(key: Key) -> Self {
        KeyEvent {
            key,
            shift: false,
            ctrl: false,
            alt: false,
        }
    }
    pub const fn shift(mut self) -> Self {
        self.shift = true;
        self
    }
    pub const fn ctrl(mut self) -> Self {
        self.ctrl = true;
        self
    }
}

/// Everything the widgets read for one frame. The host fills it from window messages and
/// clears the edge fields (`pressed`, `released`, `wheel`, `keys`, `text`, `paste`) after each
/// frame.
#[derive(Debug, Clone, Default)]
pub struct InputState {
    /// Pointer position in canvas pixels, `None` when it is outside the window.
    pub pointer: Option<(f32, f32)>,
    /// Primary button currently held.
    pub down: bool,
    /// Primary button went down this frame.
    pub pressed: bool,
    /// Primary button went up this frame.
    pub released: bool,
    /// Wheel movement in pixels this frame; positive `y` scrolls content up (away from the user).
    pub wheel: (f32, f32),
    pub keys: Vec<KeyEvent>,
    /// Typed text (already IME/layout translated) this frame.
    pub text: String,
    /// Clipboard text, provided by the host only on frames where Ctrl+V was pressed.
    pub paste: Option<String>,
    /// Shift / Ctrl currently held (for shift-click selection).
    pub shift: bool,
    pub ctrl: bool,
    /// Seconds since an arbitrary start; drives caret blink, tooltips and scrollbar fades.
    pub time: f64,
}

impl InputState {
    pub fn at(x: f32, y: f32) -> Self {
        InputState {
            pointer: Some((x, y)),
            ..Self::default()
        }
    }

    /// Pointer moved to (x, y) with the button held down this frame.
    pub fn press(x: f32, y: f32) -> Self {
        InputState {
            pointer: Some((x, y)),
            down: true,
            pressed: true,
            ..Self::default()
        }
    }

    /// Pointer at (x, y), button released this frame.
    pub fn release(x: f32, y: f32) -> Self {
        InputState {
            pointer: Some((x, y)),
            released: true,
            ..Self::default()
        }
    }

    /// Pointer at (x, y) with the button held, no edge.
    pub fn drag(x: f32, y: f32) -> Self {
        InputState {
            pointer: Some((x, y)),
            down: true,
            ..Self::default()
        }
    }

    pub fn key(key: KeyEvent) -> Self {
        InputState {
            keys: vec![key],
            ..Self::default()
        }
    }

    pub fn typed(text: &str) -> Self {
        InputState {
            text: text.to_owned(),
            ..Self::default()
        }
    }

    /// A copy without one-shot events, used when a frame is repainted a second time.
    pub fn without_events(&self) -> InputState {
        InputState {
            pointer: self.pointer,
            down: self.down,
            shift: self.shift,
            ctrl: self.ctrl,
            time: self.time,
            ..InputState::default()
        }
    }

    pub fn has_key(&self, key: Key) -> bool {
        self.keys.iter().any(|k| k.key == key)
    }
}
