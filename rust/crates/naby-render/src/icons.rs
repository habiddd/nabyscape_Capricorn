//! Icons ported from `UI/Shared/Icons.cs` (Lucide outlines plus a few filled glyphs) and the
//! SVG path-data parser that turns them into tiny-skia paths.
//!
//! Outline icons live on a 24x24 canvas and are stroked (1.6 wide, round caps and joins in the
//! WPF build). Filled icons are stretched uniformly to fill the target square, like the WPF
//! `Path` with `Stretch=Uniform`.

use crate::canvas::{Canvas, Fill, Rect};
use naby_core::color::Rgba;
use std::sync::OnceLock;
use tiny_skia::{FillRule, Path, PathBuilder, Transform};

/// Stroke width the WPF build uses for outline icons (in the 24-unit canvas).
pub const DEFAULT_STROKE: f32 = 1.6;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconStyle {
    /// Stroked on a 24x24 canvas.
    Outline,
    /// Filled, stretched to the target square.
    Filled,
}

macro_rules! icons {
    ($($variant:ident, $name:literal, $style:ident, $data:literal;)*) => {
        /// Every icon in `Icons.cs`.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum Icon { $($variant,)* }

        impl Icon {
            /// All icons, in declaration order.
            pub const ALL: &'static [Icon] = &[$(Icon::$variant,)*];
            /// The C# icon name (`"nav-media"`, `"mic-off"`, ...).
            pub fn name(self) -> &'static str {
                match self { $(Icon::$variant => $name,)* }
            }
            pub fn style(self) -> IconStyle {
                match self { $(Icon::$variant => IconStyle::$style,)* }
            }
            /// Raw SVG path data as written in `Icons.cs`.
            pub fn data(self) -> &'static str {
                match self { $(Icon::$variant => $data,)* }
            }
        }
        const ICON_COUNT: usize = [$($name,)*].len();
    };
}

icons! {
    // ---- Lucide outline set (NavigationPaths) ----
    Search, "search", Outline, "M10.5,3 A7.5,7.5 0 1 1 10.49,3 M16,16 L21,21";
    Reset, "reset", Outline, "M4,10 A8,8 0 1 1 5,18 M4,4 V10 H10";
    Plus, "plus", Outline, "M5,12 H19 M12,5 V19";
    Weather, "weather", Outline, "M12,3 V1 M5,5 L3,3 M19,5 L21,3 M3,12 H1 M5,17 L3,19 M7,12 A5,5 0 1 1 17,12 M9,20 H19 A4,4 0 0 0 19,12 A5,5 0 0 0 9,13 A3.5,3.5 0 0 0 9,20";
    Folder, "folder", Outline, "M3,7 H10 L12,9 H21 V20 H3 Z M3,7 V4 H10 L12,6 H21 V9";
    Notes, "notes", Outline, "M14,2 H6 Q4,2 4,4 V20 Q4,22 6,22 H18 Q20,22 20,20 V8 Z M14,2 V8 H20 M8,13 H16 M8,17 H14";
    Clipboard, "clipboard", Outline, "M9,5 H5 V21 H19 V5 H15 M9,3 H15 V7 H9 Z";
    Coffee, "coffee", Outline, "M4,8 H17 V16 Q17,20 13,20 H8 Q4,20 4,16 Z M17,9 H20 Q24,13 17,15 M8,3 V5 M13,2 V5";
    Lyrics, "lyrics", Outline, "M4,5 H20 M4,10 H17 M4,15 H20 M4,20 H13";
    Chat, "chat", Outline, "M5,3 H19 Q21,3 21,5 V15 Q21,17 19,17 H9 L4,21 V17 Q2,17 2,15 V5 Q2,3 5,3 Z M7,8 H16 M7,12 H13";
    Mic, "mic", Outline, "M9,5 A3,3 0 0 1 15,5 V12 A3,3 0 0 1 9,12 Z M5,11 V12 A7,7 0 0 0 19,12 V11 M12,19 V22 M8,22 H16";
    MicOff, "mic-off", Outline, "M3,3 L21,21 M9,8 V12 Q9,15 12,15 M15,9 V5 A3,3 0 0 0 10,3 M5,11 V12 A7,7 0 0 0 17,17 M19,12 V11 M12,19 V22 M8,22 H16";
    Screen, "screen", Outline, "M4,4 H20 V17 H4 Z M8,21 H16 M12,17 V21";
    Send, "send", Outline, "M12,20 V4 M6,10 L12,4 18,10";
    Trash, "trash", Outline, "M3,6 H21 M9,6 V3 H15 V6 M6,6 L7,21 H17 L18,6 M10,10 V17 M14,10 V17";
    Stop, "stop", Outline, "M6,6 H18 V18 H6 Z";
    Back, "back", Outline, "M15,5 L8,12 15,19";
    Paperclip, "paperclip", Outline, "M21,11 L12,20 A6,6 0 0 1 4,12 L14,2 A4,4 0 0 1 20,8 L10,18 A2,2 0 0 1 7,15 L16,6";
    FileText, "file-text", Outline, "M14,2 H6 Q4,2 4,4 V20 Q4,22 6,22 H18 Q20,22 20,20 V8 Z M14,2 V8 H20 M8,13 H16 M8,17 H14";
    Image, "image", Outline, "M5,3 H19 Q21,3 21,5 V19 Q21,21 19,21 H5 Q3,21 3,19 V5 Q3,3 5,3 Z M3,16 L9,10 16,17 M14,15 L17,12 21,16 M14,7 A1,1 0 1 1 13.99,7";
    Link, "link", Outline, "M10,13 Q13,16 16,13 L20,9 Q23,6 20,3 Q17,0 14,3 L12,5 M14,11 Q11,8 8,11 L4,15 Q1,18 4,21 Q7,24 10,21 L12,19";
    X, "x", Outline, "M6,6 L18,18 M18,6 L6,18";
    Minus, "minus", Outline, "M5,12 H19";
    Volume, "volume", Outline, "M3,9 H7 L12,5 V19 L7,15 H3 Z M16,8 Q20,12 16,16 M19,5 Q25,12 19,19";
    Muted, "muted", Outline, "M3,9 H7 L12,5 V19 L7,15 H3 Z M17,9 L23,15 M23,9 L17,15";
    Calendar, "calendar", Outline, "M8,2 V6 M16,2 V6 M3,10 H21 M5,4 H19 Q21,4 21,6 V20 Q21,22 19,22 H5 Q3,22 3,20 V6 Q3,4 5,4 Z";
    NavMedia, "nav-media", Outline, "M12,3 A9,9 0 1 1 11.99,3 M10,8 L16,12 10,16 Z";
    NavFocus, "nav-focus", Outline, "M9,2 H15 M12,2 V5 M12,5 A8,8 0 1 1 11.99,5 M12,9 V13 L15,15 M18,6 L20,4";
    NavStatus, "nav-status", Outline, "M5,4 H19 Q21,4 21,6 V15 Q21,17 19,17 H5 Q3,17 3,15 V6 Q3,4 5,4 Z M8,21 H16 M12,17 V21 M7,12 L10,9 13,12 17,8";
    NavLaunch, "nav-launch", Outline, "M 4.0 3.0 H 9.0 Q 10.0 3.0 10.0 4.0 V 9.0 Q 10.0 10.0 9.0 10.0 H 4.0 Q 3.0 10.0 3.0 9.0 V 4.0 Q 3.0 3.0 4.0 3.0 Z M 15.0 3.0 H 20.0 Q 21.0 3.0 21.0 4.0 V 9.0 Q 21.0 10.0 20.0 10.0 H 15.0 Q 14.0 10.0 14.0 9.0 V 4.0 Q 14.0 3.0 15.0 3.0 Z M 15.0 14.0 H 20.0 Q 21.0 14.0 21.0 15.0 V 20.0 Q 21.0 21.0 20.0 21.0 H 15.0 Q 14.0 21.0 14.0 20.0 V 15.0 Q 14.0 14.0 15.0 14.0 Z M 4.0 14.0 H 9.0 Q 10.0 14.0 10.0 15.0 V 20.0 Q 10.0 21.0 9.0 21.0 H 4.0 Q 3.0 21.0 3.0 20.0 V 15.0 Q 3.0 14.0 4.0 14.0 Z";
    NavTools, "nav-tools", Outline, "M5,7 H19 Q21,7 21,9 V18 Q21,20 19,20 H5 Q3,20 3,18 V9 Q3,7 5,7 Z M8,7 V5 Q8,3 10,3 H14 Q16,3 16,5 V7 M3,12 Q12,17 21,12 M12,13 V16";
    NavSettings, "nav-settings", Outline, "M 14 17 H 5 M 19 7 h -9 M 20.0 17.0 A 3.0 3.0 0 1 1 14.0 17.0 A 3.0 3.0 0 1 1 20.0 17.0 M 10.0 7.0 A 3.0 3.0 0 1 1 4.0 7.0 A 3.0 3.0 0 1 1 10.0 7.0";
    Mixer, "mixer", Outline, "M 10 5 H 3 M 12 19 H 3 M 14 3 v 4 M 16 17 v 4 M 21 12 h -9 M 21 19 h -5 M 21 5 h -7 M 8 10 v 4 M 8 12 H 3";
    Settings, "settings", Outline, "M 14 17 H 5 M 19 7 h -9 M 20.0 17.0 A 3.0 3.0 0 1 1 14.0 17.0 A 3.0 3.0 0 1 1 20.0 17.0 M 10.0 7.0 A 3.0 3.0 0 1 1 4.0 7.0 A 3.0 3.0 0 1 1 10.0 7.0";
    // ---- Filled set (Paths) ----
    Copy, "copy", Filled, "M8,7 L21,7 21,22 8,22 Z M3,2 L16,2 16,5 6,5 6,17 3,17 Z";
    StopFilled, "stop-filled", Filled, "M6,6 H18 V18 H6 Z";
    Play, "play", Filled, "M7,4 L20,12 7,20 Z";
    Chevron, "chevron", Filled, "M4,7 L12,15 20,7 22,9 12,19 2,9 Z";
    Pause, "pause", Filled, "M6,4 L10,4 10,20 6,20 Z M14,4 L18,4 18,20 14,20 Z";
    Next, "next", Filled, "M3,5 L13,12 3,19 Z M13,5 L22,12 13,19 Z";
    Previous, "previous", Filled, "M21,5 L11,12 21,19 Z M11,5 L2,12 11,19 Z";
    Music, "music", Filled, "M10,4 L20,2 20,16 A3.5,3.5 0 1 1 18,13 L18,6 10,8 10,19 A3.5,3.5 0 1 1 8,16 L8,5 Z";
    Activity, "activity", Filled, "M3,9 Q3,8 4,8 Q5,8 5,9 L5,15 Q5,16 4,16 Q3,16 3,15 Z M8,5 Q8,4 9,4 Q10,4 10,5 L10,19 Q10,20 9,20 Q8,20 8,19 Z M13,7 Q13,6 14,6 Q15,6 15,7 L15,17 Q15,18 14,18 Q13,18 13,17 Z M18,10 Q18,9 19,9 Q20,9 20,10 L20,14 Q20,15 19,15 Q18,15 18,14 Z";
    Timer, "timer", Filled, "M9,1 L15,1 15,3 9,3 Z M12,5 A8,8 0 1 1 11.99,5 Z M11,8 L11,14 16,14 16,12 13,12 13,8 Z";
    Focus, "focus", Filled, "M16,2 A10,10 0 1 0 22,16 A9,9 0 0 1 16,2 Z";
    Clock, "clock", Filled, "M12,2 A10,10 0 1 0 12.01,2 Z M11,6 L11,13 17,16 18,14 13,11.5 13,6 Z";
    Chip, "chip", Filled, "M6,6 L18,6 18,18 6,18 Z M9,9 L9,15 15,15 15,9 Z M8,2 L10,2 10,6 8,6 Z M14,2 L16,2 16,6 14,6 Z M8,18 L10,18 10,22 8,22 Z M14,18 L16,18 16,22 14,22 Z M2,8 L6,8 6,10 2,10 Z M2,14 L6,14 6,16 2,16 Z M18,8 L22,8 22,10 18,10 Z M18,14 L22,14 22,16 18,16 Z";
    Arrow, "arrow", Filled, "M5,4 L20,4 20,19 17,19 17,9 6,20 4,18 15,7 5,7 Z";
    Battery, "battery", Filled, "M2,6 L19,6 Q21,6 21,8 L21,16 Q21,18 19,18 L2,18 Q0,18 0,16 L0,8 Q0,6 2,6 Z M3,8 L3,16 18,16 18,8 Z M22,9 L24,9 24,15 22,15 Z";
    // Filled variants whose names are shadowed by the outline set in C# (`Create` looks the
    // outline up first), kept so every path in Icons.cs is ported.
    SearchSolid, "search-solid", Filled, "M10,2 A8,8 0 1 0 10,18 A8,8 0 1 0 10,2 Z M16,16 L22,22 23,21 17,15 Z";
    MixerSolid, "mixer-solid", Filled, "M3,2 L5,2 5,8 3,8 Z M3,14 L5,14 5,22 3,22 Z M1,8 L7,8 7,14 1,14 Z M11,2 L13,2 13,13 11,13 Z M11,19 L13,19 13,22 11,22 Z M9,13 L15,13 15,19 9,19 Z M19,2 L21,2 21,6 19,6 Z M19,12 L21,12 21,22 19,22 Z M17,6 L23,6 23,12 17,12 Z";
    SettingsSolid, "settings-solid", Filled, "M12,2 L14,2 15,5 17,6 20,6 22,10 20,12 20,14 21,17 18,20 15,19 13,20 11,22 7,20 7,17 5,15 2,14 2,10 5,9 6,7 6,4 10,2 Z M12,8 A4,4 0 1 0 12,16 A4,4 0 1 0 12,8 Z";
    VolumeSolid, "volume-solid", Filled, "M3,9 L7,9 12,5 12,19 7,15 3,15 Z M16,7 Q22,12 16,17 L15,15 Q19,12 15,9 Z";
    MutedSolid, "muted-solid", Filled, "M3,9 L7,9 12,5 12,19 7,15 3,15 Z M16,8 L19,11 22,8 23,9 20,12 23,15 22,16 19,13 16,16 15,15 18,12 15,9 Z";
}

static PATHS: [OnceLock<Option<Path>>; ICON_COUNT] = [const { OnceLock::new() }; ICON_COUNT];

impl Icon {
    /// Looks an icon up by its C# name: outline icons first, then filled, unknown names map
    /// to [`Icon::Arrow`] (as `Icons.Create` does).
    pub fn from_name(name: &str) -> Icon {
        Icon::ALL
            .iter()
            .copied()
            .find(|i| i.name() == name)
            .unwrap_or(Icon::Arrow)
    }

    /// Parsed geometry (parsed once, then shared).
    pub fn path(self) -> Option<&'static Path> {
        PATHS[self as usize]
            .get_or_init(|| parse_path(self.data()))
            .as_ref()
    }
}

impl Canvas {
    /// Draws `icon` into `rect` with `color`. `stroke` is the outline width in 24-unit canvas
    /// space (use [`DEFAULT_STROKE`]); filled icons ignore it.
    pub fn icon(&mut self, icon: Icon, rect: Rect, color: Rgba, stroke: f32) {
        let Some(path) = icon.path() else { return };
        if rect.is_empty() {
            return;
        }
        let fill = Fill::Solid(color);
        match icon.style() {
            IconStyle::Outline => {
                let s = rect.w.min(rect.h) / 24.0;
                let t = Transform::from_row(
                    s,
                    0.0,
                    0.0,
                    s,
                    rect.x + (rect.w - 24.0 * s) / 2.0,
                    rect.y + (rect.h - 24.0 * s) / 2.0,
                );
                self.stroke_path(path, &fill, stroke, t);
            }
            IconStyle::Filled => {
                let b = path.bounds();
                let (bw, bh) = (b.width().max(0.001), b.height().max(0.001));
                let s = (rect.w / bw).min(rect.h / bh);
                let t = Transform::from_row(
                    s,
                    0.0,
                    0.0,
                    s,
                    rect.x + (rect.w - bw * s) / 2.0 - b.left() * s,
                    rect.y + (rect.h - bh * s) / 2.0 - b.top() * s,
                );
                self.fill_path_with(path, &fill, FillRule::EvenOdd, t);
            }
        }
    }
}

// ---- SVG path data ------------------------------------------------------------------------

struct Scanner<'a> {
    s: &'a [u8],
    i: usize,
}

impl Scanner<'_> {
    fn skip_sep(&mut self) {
        while self.i < self.s.len()
            && (self.s[self.i].is_ascii_whitespace() || self.s[self.i] == b',')
        {
            self.i += 1;
        }
    }
    fn eof(&mut self) -> bool {
        self.skip_sep();
        self.i >= self.s.len()
    }
    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }
    fn number(&mut self) -> Option<f32> {
        self.skip_sep();
        let start = self.i;
        if matches!(self.peek(), Some(b'+' | b'-')) {
            self.i += 1;
        }
        let mut digits = 0;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.i += 1;
            digits += 1;
        }
        if self.peek() == Some(b'.') {
            self.i += 1;
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.i += 1;
                digits += 1;
            }
        }
        if digits == 0 {
            self.i = start;
            return None;
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            let save = self.i;
            self.i += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.i += 1;
            }
            let ds = self.i;
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.i += 1;
            }
            if self.i == ds {
                self.i = save;
            }
        }
        std::str::from_utf8(&self.s[start..self.i])
            .ok()?
            .parse::<f32>()
            .ok()
            .filter(|v| v.is_finite())
    }
    /// Arc flag: a single `0` or `1`, which SVG allows without a separator after it.
    fn flag(&mut self) -> Option<bool> {
        self.skip_sep();
        match self.peek() {
            Some(b'0') => {
                self.i += 1;
                Some(false)
            }
            Some(b'1') => {
                self.i += 1;
                Some(true)
            }
            _ => None,
        }
    }
}

/// Parses SVG path data (`M L H V C S Q T A Z`, absolute and relative; implicit repeats;
/// compact arc flags). Arcs become cubic Béziers. Returns `None` for malformed data or an
/// empty path.
pub fn parse_path(data: &str) -> Option<Path> {
    let mut sc = Scanner {
        s: data.as_bytes(),
        i: 0,
    };
    let mut pb = PathBuilder::new();
    let mut cur = (0.0f32, 0.0f32);
    let mut start = cur;
    let mut last_cubic: Option<(f32, f32)> = None;
    let mut last_quad: Option<(f32, f32)> = None;
    let mut need_move = false;
    let mut cmd: Option<u8> = None;
    let mut first = true;

    while !sc.eof() {
        let c = sc.peek()?;
        if c.is_ascii_alphabetic() {
            sc.i += 1;
            cmd = Some(c);
            if matches!(c, b'Z' | b'z') {
                pb.close();
                cur = start;
                need_move = true;
                last_cubic = None;
                last_quad = None;
                cmd = Some(c);
                continue;
            }
        } else if matches!(cmd, None | Some(b'Z' | b'z')) {
            return None;
        }
        let c = cmd?;
        if first && !matches!(c, b'M' | b'm') {
            return None;
        }
        let rel = c.is_ascii_lowercase();
        let (ox, oy) = if rel { cur } else { (0.0, 0.0) };
        match c.to_ascii_uppercase() {
            b'M' => {
                let p = (sc.number()? + ox, sc.number()? + oy);
                pb.move_to(p.0, p.1);
                cur = p;
                start = p;
                need_move = false;
                first = false;
                cmd = Some(if rel { b'l' } else { b'L' });
                last_cubic = None;
                last_quad = None;
                continue;
            }
            b'L' | b'H' | b'V' | b'C' | b'S' | b'Q' | b'T' | b'A' => {}
            _ => return None,
        }
        if need_move {
            pb.move_to(cur.0, cur.1);
            need_move = false;
        }
        match c.to_ascii_uppercase() {
            b'L' => {
                let p = (sc.number()? + ox, sc.number()? + oy);
                pb.line_to(p.0, p.1);
                cur = p;
                last_cubic = None;
                last_quad = None;
            }
            b'H' => {
                let x = sc.number()? + ox;
                pb.line_to(x, cur.1);
                cur.0 = x;
                last_cubic = None;
                last_quad = None;
            }
            b'V' => {
                let y = sc.number()? + oy;
                pb.line_to(cur.0, y);
                cur.1 = y;
                last_cubic = None;
                last_quad = None;
            }
            b'C' => {
                let c1 = (sc.number()? + ox, sc.number()? + oy);
                let c2 = (sc.number()? + ox, sc.number()? + oy);
                let p = (sc.number()? + ox, sc.number()? + oy);
                pb.cubic_to(c1.0, c1.1, c2.0, c2.1, p.0, p.1);
                last_cubic = Some(c2);
                last_quad = None;
                cur = p;
            }
            b'S' => {
                let c1 = match last_cubic {
                    Some(l) => (2.0 * cur.0 - l.0, 2.0 * cur.1 - l.1),
                    None => cur,
                };
                let c2 = (sc.number()? + ox, sc.number()? + oy);
                let p = (sc.number()? + ox, sc.number()? + oy);
                pb.cubic_to(c1.0, c1.1, c2.0, c2.1, p.0, p.1);
                last_cubic = Some(c2);
                last_quad = None;
                cur = p;
            }
            b'Q' => {
                let c1 = (sc.number()? + ox, sc.number()? + oy);
                let p = (sc.number()? + ox, sc.number()? + oy);
                pb.quad_to(c1.0, c1.1, p.0, p.1);
                last_quad = Some(c1);
                last_cubic = None;
                cur = p;
            }
            b'T' => {
                let c1 = match last_quad {
                    Some(l) => (2.0 * cur.0 - l.0, 2.0 * cur.1 - l.1),
                    None => cur,
                };
                let p = (sc.number()? + ox, sc.number()? + oy);
                pb.quad_to(c1.0, c1.1, p.0, p.1);
                last_quad = Some(c1);
                last_cubic = None;
                cur = p;
            }
            b'A' => {
                let rx = sc.number()?;
                let ry = sc.number()?;
                let rot = sc.number()?;
                let large = sc.flag()?;
                let sweep = sc.flag()?;
                let p = (sc.number()? + ox, sc.number()? + oy);
                arc_to(&mut pb, cur, rx, ry, rot, large, sweep, p);
                cur = p;
                last_cubic = None;
                last_quad = None;
            }
            _ => return None,
        }
    }
    pb.finish()
}

/// Appends an SVG elliptical arc (endpoint form) as cubic segments of at most 90 degrees.
#[allow(clippy::too_many_arguments)]
pub fn arc_to(
    pb: &mut PathBuilder,
    from: (f32, f32),
    rx: f32,
    ry: f32,
    rot_deg: f32,
    large: bool,
    sweep: bool,
    to: (f32, f32),
) {
    let (x1, y1) = (from.0 as f64, from.1 as f64);
    let (x2, y2) = (to.0 as f64, to.1 as f64);
    if x1 == x2 && y1 == y2 {
        return;
    }
    let (mut rx, mut ry) = ((rx as f64).abs(), (ry as f64).abs());
    if rx == 0.0 || ry == 0.0 {
        pb.line_to(to.0, to.1);
        return;
    }
    let phi = (rot_deg as f64).to_radians();
    let (sin, cos) = phi.sin_cos();
    let dx2 = (x1 - x2) / 2.0;
    let dy2 = (y1 - y2) / 2.0;
    let x1p = cos * dx2 + sin * dy2;
    let y1p = -sin * dx2 + cos * dy2;
    let lambda = x1p * x1p / (rx * rx) + y1p * y1p / (ry * ry);
    if lambda > 1.0 {
        let s = lambda.sqrt();
        rx *= s;
        ry *= s;
    }
    let (rx2, ry2) = (rx * rx, ry * ry);
    let num = rx2 * ry2 - rx2 * y1p * y1p - ry2 * x1p * x1p;
    let den = rx2 * y1p * y1p + ry2 * x1p * x1p;
    let sign = if large == sweep { -1.0 } else { 1.0 };
    let coef = if den == 0.0 {
        0.0
    } else {
        sign * (num / den).max(0.0).sqrt()
    };
    let cxp = coef * rx * y1p / ry;
    let cyp = -coef * ry * x1p / rx;
    let cx = cos * cxp - sin * cyp + (x1 + x2) / 2.0;
    let cy = sin * cxp + cos * cyp + (y1 + y2) / 2.0;
    let angle = |ux: f64, uy: f64, vx: f64, vy: f64| -> f64 {
        let dot = ux * vx + uy * vy;
        let len = (ux * ux + uy * uy).sqrt() * (vx * vx + vy * vy).sqrt();
        let mut a = (dot / len).clamp(-1.0, 1.0).acos();
        if ux * vy - uy * vx < 0.0 {
            a = -a;
        }
        a
    };
    let ux = (x1p - cxp) / rx;
    let uy = (y1p - cyp) / ry;
    let vx = (-x1p - cxp) / rx;
    let vy = (-y1p - cyp) / ry;
    let theta1 = angle(1.0, 0.0, ux, uy);
    let mut dtheta = angle(ux, uy, vx, vy) % (2.0 * std::f64::consts::PI);
    if !sweep && dtheta > 0.0 {
        dtheta -= 2.0 * std::f64::consts::PI;
    } else if sweep && dtheta < 0.0 {
        dtheta += 2.0 * std::f64::consts::PI;
    }
    let n = ((dtheta.abs() / (std::f64::consts::PI / 2.0)) - 1e-9)
        .ceil()
        .max(1.0) as usize;
    let delta = dtheta / n as f64;
    let t = 4.0 / 3.0 * (delta / 4.0).tan();
    let map = |px: f64, py: f64| -> (f32, f32) {
        (
            (cx + rx * px * cos - ry * py * sin) as f32,
            (cy + rx * px * sin + ry * py * cos) as f32,
        )
    };
    for i in 0..n {
        let a1 = theta1 + i as f64 * delta;
        let a2 = a1 + delta;
        let (s1, c1) = a1.sin_cos();
        let (s2, c2) = a2.sin_cos();
        let cp1 = map(c1 - t * s1, s1 + t * c1);
        let cp2 = map(c2 + t * s2, s2 - t * c2);
        let end = if i + 1 == n { to } else { map(c2, s2) };
        pb.cubic_to(cp1.0, cp1.1, cp2.0, cp2.1, end.0, end.1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_parses() {
        assert_eq!(
            Icon::ALL.len(),
            54,
            "34 outline + 15 filled + 5 shadowed filled"
        );
        for icon in Icon::ALL {
            let p = icon
                .path()
                .unwrap_or_else(|| panic!("{} failed to parse", icon.name()));
            let b = p.bounds();
            assert!(b.width() > 0.0 || b.height() > 0.0, "{}", icon.name());
            // Everything fits the 24 canvas with a little room (Lucide link/battery reach edges).
            assert!(
                b.left() >= -0.5 && b.right() <= 25.5,
                "{} {:?}",
                icon.name(),
                b
            );
            assert!(
                b.top() >= -0.5 && b.bottom() <= 25.5,
                "{} {:?}",
                icon.name(),
                b
            );
        }
    }

    #[test]
    fn names_are_unique_and_lookup_matches_csharp() {
        let mut names: Vec<_> = Icon::ALL.iter().map(|i| i.name()).collect();
        names.sort_unstable();
        let n = names.len();
        names.dedup();
        assert_eq!(names.len(), n);
        assert_eq!(Icon::from_name("search"), Icon::Search);
        assert_eq!(Icon::from_name("search").style(), IconStyle::Outline);
        assert_eq!(Icon::from_name("play"), Icon::Play);
        assert_eq!(Icon::from_name("no-such-icon"), Icon::Arrow);
    }

    #[test]
    fn relative_and_implicit_commands() {
        let p = parse_path("m 1 1 h 4 v 4 h -4 z").unwrap();
        let b = p.bounds();
        assert_eq!(
            (b.left(), b.top(), b.right(), b.bottom()),
            (1.0, 1.0, 5.0, 5.0)
        );
        // Implicit lineto after M.
        let p = parse_path("M0,0 10,0 10,10").unwrap();
        assert_eq!(p.bounds().right(), 10.0);
        assert_eq!(p.bounds().bottom(), 10.0);
        // Number glued by signs and decimals: "1.5.5" is two numbers.
        let p = parse_path("M1.5.5L-2-3").unwrap();
        assert_eq!(p.bounds().left(), -2.0);
    }

    #[test]
    fn smooth_curves_reflect_control_points() {
        let p = parse_path("M0,0 C0,10 10,10 10,0 S20,-10 20,0 Q25,5 30,0 T40,0").unwrap();
        assert!(p.bounds().right() >= 40.0);
        assert!(parse_path("M0 0 S5 5 10 0 T20 0").is_some());
    }

    #[test]
    fn arcs_become_cubics_on_the_circle() {
        let p = parse_path("M0,0 A5,5 0 0 1 10,0").unwrap();
        let b = p.bounds();
        assert!((b.right() - 10.0).abs() < 0.01);
        // Sweep 1 from (0,0) to (10,0) bulges upward in screen space (y negative).
        assert!((b.top() + 5.0).abs() < 0.1, "{b:?}");
        assert!(b.bottom() <= 0.01);
        // Large-arc full circle through nearly coincident endpoints.
        let p = parse_path("M12,2 A10,10 0 1 0 12.01,2 Z").unwrap();
        let b = p.bounds();
        assert!(
            (b.width() - 20.0).abs() < 0.2 && (b.height() - 20.0).abs() < 0.2,
            "{b:?}"
        );
        // Compact flags "a1 1 0 011 1".
        assert!(parse_path("M0,0 a1 1 0 011 1").is_some());
        // Radii too small are scaled up to reach the endpoint.
        let p = parse_path("M0,0 A1,1 0 0 1 10,0").unwrap();
        assert!((p.bounds().right() - 10.0).abs() < 0.01);
    }

    #[test]
    fn malformed_data_is_rejected() {
        assert!(parse_path("").is_none());
        assert!(parse_path("L5,5").is_none());
        assert!(parse_path("M1").is_none());
        assert!(parse_path("M0,0 L1,x").is_none());
        assert!(parse_path("M0,0 Z 5").is_none());
        assert!(parse_path("M0,0 X1,1").is_none());
        assert!(parse_path("M0,0 A5,5 0 2 1 10,0").is_none());
    }

    #[test]
    fn closing_then_drawing_starts_new_subpath_at_start() {
        let p = parse_path("M2,2 L8,2 L8,8 Z L2,9").unwrap();
        assert_eq!(p.bounds().bottom(), 9.0);
    }

    #[test]
    fn icon_draws_inside_rect_both_styles() {
        let mut c = Canvas::new(40, 20);
        c.icon(
            Icon::Play,
            Rect::new(0.0, 0.0, 20.0, 20.0),
            Rgba::WHITE,
            DEFAULT_STROKE,
        );
        c.icon(
            Icon::Plus,
            Rect::new(20.0, 0.0, 20.0, 20.0),
            Rgba::WHITE,
            DEFAULT_STROKE,
        );
        assert!(c.pixel(8, 10).a > 200);
        assert!(c.pixel(30, 10).a > 100);
        assert_eq!(c.pixel(1, 1).a, 0);
        assert_eq!(c.pixel(21, 1).a, 0);
    }
}
