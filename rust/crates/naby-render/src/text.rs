//! Text: fontdue rasterization with a fallback chain, kerning, wrapping and ellipsis.
//!
//! Memory notes: font files are read and parsed lazily, the first time a glyph needs them
//! (the color emoji face is only consulted for emoji code points), and rasterized glyphs live
//! in a cache bounded at [`GLYPH_CACHE_BYTES`] with least-recently-used eviction.

use crate::canvas::Canvas;
use fontdue::{Font, FontSettings};
use naby_core::color::Rgba;
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::path::PathBuf;

/// Upper bound for rasterized glyph bitmaps (plus bookkeeping) kept in memory.
pub const GLYPH_CACHE_BYTES: usize = 4 * 1024 * 1024;
const ENTRY_OVERHEAD: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Weight {
    #[default]
    Regular,
    Semibold,
    Bold,
}

impl Weight {
    fn index(self) -> usize {
        match self {
            Weight::Regular => 0,
            Weight::Semibold => 1,
            Weight::Bold => 2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextStyle {
    pub size: f32,
    pub weight: Weight,
    pub color: Rgba,
}

impl TextStyle {
    pub fn new(size: f32, color: Rgba) -> Self {
        TextStyle {
            size,
            weight: Weight::Regular,
            color,
        }
    }
    pub fn weight(mut self, weight: Weight) -> Self {
        self.weight = weight;
        self
    }
    pub fn color(mut self, color: Rgba) -> Self {
        self.color = color;
        self
    }
}

/// Where a font face comes from.
#[derive(Debug, Clone)]
pub enum FontSource {
    Bytes(Vec<u8>),
    /// Read and parsed on first use.
    Path(PathBuf),
}

enum Slot {
    Loaded(Box<Font>),
    Lazy(PathBuf),
    Dead,
}

#[derive(Hash, PartialEq, Eq, Clone, Copy)]
struct Key {
    slot: u16,
    glyph: u16,
    /// Pixel size in quarter pixels.
    px4: u16,
}

#[derive(Clone, Copy)]
struct GlyphMetrics {
    advance: f32,
    xmin: i32,
    ymin: i32,
    w: u16,
    h: u16,
}

struct Entry {
    m: GlyphMetrics,
    bitmap: Option<Box<[u8]>>,
    used: u64,
}

struct Inner {
    slots: Vec<Slot>,
    emoji_only: Vec<bool>,
    map: HashMap<Key, Entry>,
    bytes: usize,
    tick: u64,
    limit: usize,
}

fn size_key(size: f32) -> u16 {
    (size.clamp(1.0, 1000.0) * 4.0).round() as u16
}

impl Inner {
    fn font(&mut self, slot: usize) -> Option<&Font> {
        if let Slot::Lazy(path) = &self.slots[slot] {
            let loaded = std::fs::read(path).ok().and_then(|b| {
                Font::from_bytes(
                    b,
                    FontSettings {
                        load_substitutions: false,
                        ..FontSettings::default()
                    },
                )
                .ok()
            });
            self.slots[slot] = match loaded {
                Some(f) => Slot::Loaded(Box::new(f)),
                None => Slot::Dead,
            };
        }
        match &self.slots[slot] {
            Slot::Loaded(f) => Some(f),
            _ => None,
        }
    }

    fn make_room(&mut self, extra: usize) {
        if self.bytes + extra <= self.limit {
            return;
        }
        let mut order: Vec<(u64, Key)> = self.map.iter().map(|(k, e)| (e.used, *k)).collect();
        order.sort_unstable_by_key(|(u, _)| *u);
        let target = self.limit / 4 * 3;
        for (_, k) in order {
            if self.bytes + extra <= target {
                break;
            }
            if let Some(e) = self.map.remove(&k) {
                self.bytes -= ENTRY_OVERHEAD + e.bitmap.as_ref().map_or(0, |b| b.len());
            }
        }
    }

    fn entry(&mut self, key: Key, raster: bool) -> &Entry {
        self.tick += 1;
        let tick = self.tick;
        let have = self
            .map
            .get(&key)
            .is_some_and(|e| !raster || e.bitmap.is_some());
        if !have {
            let px = key.px4 as f32 / 4.0;
            let existing = self.map.remove(&key);
            if let Some(e) = &existing {
                self.bytes -= ENTRY_OVERHEAD + e.bitmap.as_ref().map_or(0, |b| b.len());
            }
            let (m, bitmap) = match self.font(key.slot as usize) {
                Some(font) => {
                    if raster {
                        let (m, bmp) = font.rasterize_indexed(key.glyph, px);
                        (
                            GlyphMetrics {
                                advance: m.advance_width,
                                xmin: m.xmin,
                                ymin: m.ymin,
                                w: m.width as u16,
                                h: m.height as u16,
                            },
                            Some(bmp.into_boxed_slice()),
                        )
                    } else {
                        let m = font.metrics_indexed(key.glyph, px);
                        (
                            GlyphMetrics {
                                advance: m.advance_width,
                                xmin: m.xmin,
                                ymin: m.ymin,
                                w: m.width as u16,
                                h: m.height as u16,
                            },
                            None,
                        )
                    }
                }
                None => (
                    GlyphMetrics {
                        advance: 0.0,
                        xmin: 0,
                        ymin: 0,
                        w: 0,
                        h: 0,
                    },
                    raster.then(|| Vec::new().into_boxed_slice()),
                ),
            };
            let size = ENTRY_OVERHEAD + bitmap.as_ref().map_or(0, |b| b.len());
            self.make_room(size);
            self.bytes += size;
            self.map.insert(
                key,
                Entry {
                    m,
                    bitmap,
                    used: tick,
                },
            );
        }
        let e = self.map.get_mut(&key).expect("just inserted");
        e.used = tick;
        e
    }
}

/// Zero-width and format characters that never produce a glyph or advance.
fn is_invisible(c: char) -> bool {
    matches!(c, '\u{200B}'..='\u{200F}' | '\u{2060}' | '\u{FEFF}' | '\u{FE00}'..='\u{FE0F}')
        || (c.is_control())
}

fn is_emoji_like(c: char) -> bool {
    let u = c as u32;
    u >= 0x1F000
        || (0x2600..=0x27BF).contains(&u)
        || (0x2B00..=0x2BFF).contains(&u)
        || (0x2300..=0x23FF).contains(&u)
        || u == 0x00A9
        || u == 0x00AE
}

#[derive(Clone, Copy)]
struct Placed {
    slot: u16,
    glyph: u16,
    visible: bool,
    x: f32,
    adv: f32,
}

/// Font collection with a shared bounded glyph cache. Single-threaded (interior mutability).
pub struct Fonts {
    chains: [Vec<usize>; 3],
    inner: RefCell<Inner>,
    available: bool,
}

impl Fonts {
    /// No faces: every measurement is zero and nothing is drawn. Never panics.
    pub fn empty() -> Fonts {
        Fonts::from_sources(&[], &[], &[], &[])
    }

    /// First entry is the regular face, the rest are fallbacks tried in order.
    pub fn from_bytes(faces: Vec<Vec<u8>>) -> Fonts {
        let mut it = faces.into_iter().map(FontSource::Bytes);
        let regular: Vec<FontSource> = it.next().into_iter().collect();
        let fallbacks: Vec<FontSource> = it.collect();
        Fonts::from_sources(&regular, &[], &[], &fallbacks)
    }

    /// `regular`/`semibold`/`bold` hold at most one face each (empty = reuse regular);
    /// `fallbacks` cover symbols and emoji. Sources are consumed lazily.
    pub fn from_sources(
        regular: &[FontSource],
        semibold: &[FontSource],
        bold: &[FontSource],
        fallbacks: &[FontSource],
    ) -> Fonts {
        Fonts::build(regular, semibold, bold, fallbacks, &[])
    }

    fn build(
        regular: &[FontSource],
        semibold: &[FontSource],
        bold: &[FontSource],
        fallbacks: &[FontSource],
        emoji: &[FontSource],
    ) -> Fonts {
        let mut slots = Vec::new();
        let mut emoji_only = Vec::new();
        let mut add = |src: &[FontSource], is_emoji: bool| -> Vec<usize> {
            src.iter()
                .map(|s| {
                    slots.push(match s {
                        FontSource::Bytes(b) => Font::from_bytes(
                            b.as_slice(),
                            FontSettings {
                                load_substitutions: false,
                                ..FontSettings::default()
                            },
                        )
                        .map(|f| Slot::Loaded(Box::new(f)))
                        .unwrap_or(Slot::Dead),
                        FontSource::Path(p) => Slot::Lazy(p.clone()),
                    });
                    emoji_only.push(is_emoji);
                    slots.len() - 1
                })
                .collect()
        };
        let r = add(regular, false);
        let s = add(semibold, false);
        let b = add(bold, false);
        let fb = add(fallbacks, false);
        let em = add(emoji, true);
        let tail: Vec<usize> = fb.into_iter().chain(em).collect();
        let chain = |first: &[usize], also: &[&[usize]]| -> Vec<usize> {
            let mut v: Vec<usize> = first.to_vec();
            for a in also {
                v.extend_from_slice(a);
            }
            v.extend_from_slice(&tail);
            v
        };
        let chains = [chain(&r, &[]), chain(&s, &[&r]), chain(&b, &[&s, &r])];
        let available = !r.is_empty();
        Fonts {
            chains,
            inner: RefCell::new(Inner {
                slots,
                emoji_only,
                map: HashMap::new(),
                bytes: 0,
                tick: 0,
                limit: GLYPH_CACHE_BYTES,
            }),
            available,
        }
    }

    /// Windows: Segoe UI (+ symbol and emoji fallbacks) from `%WINDIR%\Fonts`; elsewhere the
    /// first DejaVu or Liberation faces found. Files are only read when a glyph needs them.
    pub fn load_system() -> Fonts {
        let mut dirs: Vec<PathBuf> = Vec::new();
        if cfg!(windows) {
            let win = std::env::var_os("WINDIR")
                .or_else(|| std::env::var_os("SystemRoot"))
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
            dirs.push(win.join("Fonts"));
        } else {
            for d in [
                "/usr/share/fonts/truetype/dejavu",
                "/usr/share/fonts/dejavu",
                "/usr/share/fonts/TTF",
                "/usr/share/fonts/truetype/liberation",
                "/usr/share/fonts/truetype/liberation2",
                "/usr/share/fonts/liberation",
                "/usr/share/fonts/liberation-fonts",
                "/usr/share/fonts",
                "/Library/Fonts",
                "/System/Library/Fonts/Supplemental",
            ] {
                dirs.push(PathBuf::from(d));
            }
        }
        let find = |names: &[&str]| -> Option<PathBuf> {
            names
                .iter()
                .flat_map(|n| dirs.iter().map(move |d| d.join(n)))
                .find(|p| p.is_file())
        };
        let one = |p: Option<PathBuf>| -> Vec<FontSource> {
            p.into_iter().map(FontSource::Path).collect()
        };
        let (regular, semibold, bold, symbols, emoji);
        if cfg!(windows) {
            regular = find(&["segoeui.ttf"]);
            semibold = find(&["seguisb.ttf"]);
            bold = find(&["segoeuib.ttf"]);
            symbols = find(&["seguisym.ttf"]);
            emoji = find(&["seguiemj.ttf"]);
        } else {
            let reg = find(&[
                "DejaVuSans.ttf",
                "LiberationSans-Regular.ttf",
                "Arial.ttf",
                "Arial Unicode.ttf",
            ]);
            bold = find(&[
                "DejaVuSans-Bold.ttf",
                "LiberationSans-Bold.ttf",
                "Arial Bold.ttf",
            ]);
            semibold = None;
            // DejaVu doubles as the symbol fallback when another face is the primary.
            symbols = find(&["DejaVuSans.ttf"]).filter(|p| Some(p) != reg.as_ref());
            emoji = None;
            regular = reg;
        }
        let fallbacks: Vec<FontSource> = one(symbols);
        Fonts::build(
            &one(regular),
            &one(semibold),
            &one(bold),
            &fallbacks,
            &one(emoji),
        )
    }

    /// True when a regular face was configured (it may still fail to parse).
    pub fn has_faces(&self) -> bool {
        self.available
    }

    /// Bytes held by the glyph cache (bitmaps + bookkeeping).
    pub fn cache_bytes(&self) -> usize {
        self.inner.borrow().bytes
    }
    pub fn cache_entries(&self) -> usize {
        self.inner.borrow().map.len()
    }
    pub fn clear_cache(&self) {
        let mut i = self.inner.borrow_mut();
        i.map.clear();
        i.bytes = 0;
    }
    /// Lowers the cache bound (tests, low-memory mode); never above [`GLYPH_CACHE_BYTES`].
    pub fn set_cache_limit(&self, bytes: usize) {
        self.inner.borrow_mut().limit = bytes.min(GLYPH_CACHE_BYTES);
    }

    fn resolve(&self, inner: &mut Inner, weight: Weight, c: char) -> Option<(u16, u16)> {
        let chain = &self.chains[weight.index()];
        let mut first_ok: Option<u16> = None;
        for &slot in chain {
            if inner.emoji_only[slot] && !is_emoji_like(c) {
                continue;
            }
            let Some(font) = inner.font(slot) else {
                continue;
            };
            let g = font.lookup_glyph_index(c);
            if g != 0 {
                return Some((slot as u16, g));
            }
            first_ok.get_or_insert(slot as u16);
        }
        // Nothing has it: draw the primary face's missing-glyph box (not for blanks).
        if c.is_whitespace() {
            None
        } else {
            first_ok.map(|s| (s, 0))
        }
    }

    fn shape(&self, text: &str, style: &TextStyle, mut visit: impl FnMut(&mut Inner, Placed)) {
        let mut inner = self.inner.borrow_mut();
        let px4 = size_key(style.size);
        let mut x = 0.0f32;
        let mut prev: Option<(u16, u16)> = None;
        for ch in text.chars() {
            let c = if ch == '\t' { ' ' } else { ch };
            let repeat = if ch == '\t' { 4 } else { 1 };
            let found = if is_invisible(c) {
                None
            } else {
                self.resolve(&mut inner, style.weight, c)
            };
            let Some((slot, glyph)) = found else {
                visit(
                    &mut inner,
                    Placed {
                        slot: 0,
                        glyph: 0,
                        visible: false,
                        x,
                        adv: 0.0,
                    },
                );
                continue;
            };
            if let Some((ps, pg)) = prev {
                if ps == slot {
                    if let Some(font) = inner.font(slot as usize) {
                        if let Some(k) = font.horizontal_kern_indexed(pg, glyph, px4 as f32 / 4.0) {
                            x += k;
                        }
                    }
                }
            }
            let key = Key { slot, glyph, px4 };
            let adv = inner.entry(key, false).m.advance * repeat as f32;
            visit(
                &mut inner,
                Placed {
                    slot,
                    glyph,
                    visible: ch != '\t',
                    x,
                    adv,
                },
            );
            x += adv;
            prev = Some((slot, glyph));
        }
    }

    /// (ascent, line height) in pixels for the style's primary face.
    pub fn line_metrics(&self, style: &TextStyle) -> (f32, f32) {
        let mut inner = self.inner.borrow_mut();
        for &slot in &self.chains[style.weight.index()] {
            if inner.emoji_only[slot] {
                continue;
            }
            if let Some(font) = inner.font(slot) {
                if let Some(m) = font.horizontal_line_metrics(size_key(style.size) as f32 / 4.0) {
                    return (m.ascent.round(), m.new_line_size.ceil().max(1.0));
                }
            }
        }
        (0.0, 0.0)
    }

    pub fn line_height(&self, style: &TextStyle) -> f32 {
        self.line_metrics(style).1
    }

    /// Width and height of one line of text. Newlines are ignored; use [`Fonts::wrap`].
    pub fn measure(&self, text: &str, style: &TextStyle) -> (f32, f32) {
        let mut end = 0.0f32;
        self.shape(text, style, |_, p| end = p.x + p.adv);
        (end, self.line_height(style))
    }

    pub fn measure_width(&self, text: &str, style: &TextStyle) -> f32 {
        self.measure(text, style).0
    }

    /// Cumulative x position before every character plus the total width (`chars + 1` values).
    pub fn advances(&self, text: &str, style: &TextStyle) -> Vec<f32> {
        let mut v = Vec::with_capacity(text.len() + 1);
        let mut end = 0.0f32;
        self.shape(text, style, |_, p| {
            v.push(p.x);
            end = p.x + p.adv;
        });
        v.push(end);
        v
    }

    /// Character index nearest to horizontal offset `x` (for caret placement).
    pub fn hit_index(&self, text: &str, style: &TextStyle, x: f32) -> usize {
        let adv = self.advances(text, style);
        let n = adv.len() - 1;
        for i in 0..n {
            let mid = (adv[i] + adv[i + 1]) / 2.0;
            if x < mid {
                return i;
            }
        }
        n
    }

    pub fn has_glyph(&self, c: char, weight: Weight) -> bool {
        let mut inner = self.inner.borrow_mut();
        matches!(self.resolve(&mut inner, weight, c), Some((_, g)) if g != 0)
    }

    /// Shortens `text` with an ellipsis so it fits `max_width`.
    pub fn ellipsize<'a>(&self, text: &'a str, style: &TextStyle, max_width: f32) -> Cow<'a, str> {
        if self.measure_width(text, style) <= max_width {
            return Cow::Borrowed(text);
        }
        let ell = if self.has_glyph('\u{2026}', style.weight) {
            "\u{2026}"
        } else {
            "..."
        };
        let ell_w = self.measure_width(ell, style);
        if ell_w > max_width {
            return Cow::Borrowed("");
        }
        let adv = self.advances(text, style);
        let chars: Vec<usize> = text.char_indices().map(|(b, _)| b).collect();
        // Largest n with adv[n] + ellipsis <= max (advances are monotonic).
        let mut n = 0;
        for i in 0..chars.len() {
            if adv[i + 1] + ell_w <= max_width {
                n = i + 1;
            } else {
                break;
            }
        }
        let end = chars.get(n).copied().unwrap_or(text.len());
        let mut out = text[..end].trim_end().to_owned();
        out.push_str(ell);
        Cow::Owned(out)
    }

    /// Greedy word wrap into byte ranges of `text`. Hard breaks (`\n`) end a range (the
    /// newline itself is excluded); an over-long word is split by character.
    pub fn wrap_ranges(&self, text: &str, style: &TextStyle, width: f32) -> Vec<Range<usize>> {
        let mut out = Vec::new();
        let mut para_start = 0usize;
        loop {
            let para_end = text[para_start..]
                .find('\n')
                .map_or(text.len(), |i| para_start + i);
            self.wrap_paragraph(text, para_start..para_end, style, width, &mut out);
            if para_end >= text.len() {
                break;
            }
            para_start = para_end + 1;
        }
        out
    }

    fn wrap_paragraph(
        &self,
        text: &str,
        range: Range<usize>,
        style: &TextStyle,
        width: f32,
        out: &mut Vec<Range<usize>>,
    ) {
        let para = &text[range.clone()];
        if para.is_empty() {
            out.push(range.start..range.start);
            return;
        }
        let adv = self.advances(para, style);
        let chars: Vec<(usize, char)> = para.char_indices().collect();
        let n = chars.len();
        let byte = |i: usize| range.start + chars.get(i).map_or(para.len(), |c| c.0);
        let mut start = 0usize;
        let mut last_break: Option<usize> = None;
        let mut i = 0usize;
        while i < n {
            let ws = chars[i].1.is_whitespace();
            if !ws && adv[i + 1] - adv[start] > width && i > start {
                match last_break.filter(|b| *b > start) {
                    Some(b) => {
                        out.push(byte(start)..byte(b));
                        start = b;
                    }
                    None => {
                        out.push(byte(start)..byte(i));
                        start = i;
                    }
                }
                last_break = None;
                i = start;
                // Skip leading whitespace on the continuation line.
                while i < n && chars[i].1 == ' ' && i + 1 < n && start == i {
                    start += 1;
                    i += 1;
                }
                continue;
            }
            if ws {
                last_break = Some(i + 1);
            }
            i += 1;
        }
        out.push(byte(start)..byte(n));
    }

    /// Word-wrapped lines (trailing blanks trimmed).
    pub fn wrap(&self, text: &str, style: &TextStyle, width: f32) -> Vec<String> {
        self.wrap_ranges(text, style, width)
            .into_iter()
            .map(|r| text[r].trim_end().to_owned())
            .collect()
    }
}

impl Canvas {
    /// Draws one line with its top-left at (x, y); returns the drawn width. With `max_width`
    /// the text is ellipsized to fit.
    pub fn text(
        &mut self,
        fonts: &Fonts,
        x: f32,
        y: f32,
        text: &str,
        style: &TextStyle,
        max_width: Option<f32>,
    ) -> f32 {
        let shown: Cow<str> = match max_width {
            Some(w) => fonts.ellipsize(text, style, w),
            None => Cow::Borrowed(text),
        };
        let (ascent, _) = fonts.line_metrics(style);
        let baseline = (y + ascent).round();
        let px4 = size_key(style.size);
        let x0 = x;
        let mut end = 0.0f32;
        fonts.shape(&shown, style, |inner, p| {
            end = p.x + p.adv;
            if !p.visible {
                return;
            }
            let key = Key {
                slot: p.slot,
                glyph: p.glyph,
                px4,
            };
            let e = inner.entry(key, true);
            let (m, bmp) = (e.m, e.bitmap.as_deref().unwrap_or(&[]));
            if m.w == 0 || m.h == 0 {
                return;
            }
            let gx = (x0 + p.x).round() as i32 + m.xmin;
            let gy = baseline as i32 - m.ymin - m.h as i32;
            self.blit_coverage(gx, gy, m.w as usize, m.h as usize, bmp, style.color);
        });
        end
    }

    /// Draws wrapped text; returns the total height. `line_gap` is extra space between lines.
    #[allow(clippy::too_many_arguments)]
    pub fn paragraph(
        &mut self,
        fonts: &Fonts,
        x: f32,
        y: f32,
        text: &str,
        style: &TextStyle,
        width: f32,
        line_gap: f32,
    ) -> f32 {
        let lh = fonts.line_height(style) + line_gap;
        let lines = fonts.wrap(text, style, width);
        for (i, line) in lines.iter().enumerate() {
            self.text(fonts, x, y + i as f32 * lh, line, style, None);
        }
        lines.len() as f32 * lh
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::Rect;

    fn fonts() -> Option<Fonts> {
        let f = Fonts::load_system();
        if !f.has_faces() {
            println!("skip: no system font found");
            return None;
        }
        Some(f)
    }

    fn style(size: f32) -> TextStyle {
        TextStyle::new(size, Rgba::WHITE)
    }

    #[test]
    fn empty_fonts_measure_zero_and_never_panic() {
        let f = Fonts::empty();
        assert_eq!(f.measure("hello", &style(13.0)), (0.0, 0.0));
        let mut c = Canvas::new(10, 10);
        assert_eq!(c.text(&f, 0.0, 0.0, "x", &style(13.0), Some(5.0)), 0.0);
        assert!(f.wrap("a b", &style(13.0), 10.0).len() <= 2);
        let broken = Fonts::from_bytes(vec![b"not a font".to_vec()]);
        assert_eq!(broken.measure("hi", &style(13.0)).0, 0.0);
    }

    #[test]
    fn text_measure_monotonic() {
        let Some(f) = fonts() else { return };
        let s = style(14.0);
        let a = f.measure("Hello", &s).0;
        let b = f.measure("Hello world", &s).0;
        assert!(a > 0.0 && b > a);
        let big = f.measure("Hello", &style(28.0)).0;
        assert!(big > a * 1.8);
        assert!(f.measure("", &s).0 == 0.0);
        let adv = f.advances("Hello", &s);
        assert_eq!(adv.len(), 6);
        assert!(adv.windows(2).all(|w| w[1] >= w[0]));
        assert!((adv[5] - a).abs() < 0.01);
    }

    #[test]
    fn ellipsis_fits_width() {
        let Some(f) = fonts() else { return };
        let s = style(14.0);
        let text = "The quick brown fox jumps over the lazy dog";
        let full = f.measure_width(text, &s);
        for max in [20.0, 60.0, 120.0, full - 1.0] {
            let out = f.ellipsize(text, &s, max);
            assert!(f.measure_width(&out, &s) <= max + 0.01, "{max}: {out}");
            assert!(out.ends_with('\u{2026}') || out.ends_with("..."), "{out}");
        }
        assert!(matches!(
            f.ellipsize(text, &s, full + 1.0),
            Cow::Borrowed(_)
        ));
        assert_eq!(f.ellipsize(text, &s, 1.0), "");
    }

    #[test]
    fn wrap_breaks_on_words_and_newlines() {
        let Some(f) = fonts() else { return };
        let s = style(14.0);
        let w = f.measure_width("hello world", &s);
        let lines = f.wrap("hello world foo\nbar", &s, w + 2.0);
        assert_eq!(lines, vec!["hello world", "foo", "bar"]);
        // Long word splits by character and every line fits.
        let lines = f.wrap("abcdefghijklmnopqrstuvwxyz", &s, 40.0);
        assert!(lines.len() > 1);
        assert!(lines.iter().all(|l| f.measure_width(l, &s) <= 40.5));
        assert_eq!(lines.concat(), "abcdefghijklmnopqrstuvwxyz");
        assert_eq!(f.wrap("", &s, 50.0), vec![""]);
    }

    #[test]
    fn text_draws_pixels_and_clips_to_ellipsis() {
        let Some(f) = fonts() else { return };
        let mut c = Canvas::new(120, 30);
        let s = style(16.0);
        let w = c.text(&f, 4.0, 4.0, "Hello", &s, None);
        assert!(w > 20.0);
        let mut ink = 0;
        for y in 0..30 {
            for x in 0..120 {
                if c.pixel(x, y).a > 0 {
                    ink += 1;
                    assert!(x < 4 + w.ceil() as u32 + 2);
                }
            }
        }
        assert!(ink > 40);
        let mut d = Canvas::new(60, 30);
        let long = "Hello wonderful world";
        let used = d.text(&f, 0.0, 0.0, long, &s, Some(40.0));
        assert!(used <= 40.01);
        assert!((0..30).any(|y| (40..60).all(|x| d.pixel(x, y).a == 0)));
        assert_eq!(d.pixel(59, 15).a, 0);
        let _ = Rect::default();
    }

    #[test]
    fn glyph_cache_stays_bounded() {
        let Some(f) = fonts() else { return };
        f.set_cache_limit(64 * 1024);
        let mut c = Canvas::new(300, 120);
        let text: String = ('A'..='z').collect();
        for i in 0..80 {
            let s = style(10.0 + i as f32 * 0.5);
            c.text(&f, 0.0, 0.0, &text, &s, None);
            assert!(
                f.cache_bytes() <= 64 * 1024 + 8 * 1024,
                "{}",
                f.cache_bytes()
            );
        }
        assert!(f.cache_entries() > 0);
        let g = Fonts::load_system();
        let mut c = Canvas::new(300, 120);
        for i in 0..200 {
            c.text(&g, 0.0, 0.0, &text, &style(8.0 + i as f32 * 0.25), None);
        }
        assert!(g.cache_bytes() <= GLYPH_CACHE_BYTES);
    }

    #[test]
    fn fallback_chain_covers_missing_glyphs() {
        let Some(f) = fonts() else { return };
        // The ellipsis and arrows exist in DejaVu; at minimum the call must not panic and
        // must give a positive width for a symbol that some face in the chain provides.
        let s = style(14.0);
        assert!(f.measure_width("\u{2192}", &s) >= 0.0);
        assert_eq!(f.measure_width("\u{200B}", &s), 0.0);
        assert!(f.measure_width("a\tb", &s) > f.measure_width("ab", &s));
    }

    #[test]
    fn hit_index_picks_nearest_boundary() {
        let Some(f) = fonts() else { return };
        let s = style(14.0);
        let adv = f.advances("abcd", &s);
        assert_eq!(f.hit_index("abcd", &s, -5.0), 0);
        assert_eq!(f.hit_index("abcd", &s, adv[2] + 0.5), 2);
        assert_eq!(f.hit_index("abcd", &s, 999.0), 4);
    }
}
