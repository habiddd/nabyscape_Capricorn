//! Clipboard history and smart classification (C# `ClipItem`, `SmartClipboard`, and the history
//! rules of `ProductivityService`). Listening to the OS clipboard and writing to it are host work.

use super::links::try_clean_link;
use super::notes::bound;
use super::{new_id, null_default};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const MAX_CLIP_TEXT: usize = 20_000;
pub const MAX_UNPINNED: usize = 30;
pub const MAX_PINNED: usize = 20;
pub const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;
pub const MAX_IMAGE_PIXELS: u64 = 25_000_000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct ClipItem {
    #[serde(deserialize_with = "null_default")]
    pub id: String,
    /// `"text"` or `"image"`.
    #[serde(deserialize_with = "null_default")]
    pub kind: String,
    #[serde(deserialize_with = "null_default")]
    pub text: String,
    #[serde(deserialize_with = "null_default")]
    pub image_path: String,
    pub pinned: bool,
    pub timestamp: DateTime<Utc>,
}

impl Default for ClipItem {
    fn default() -> Self {
        Self {
            id: new_id(),
            kind: "text".into(),
            text: String::new(),
            image_path: String::new(),
            pinned: false,
            timestamp: Utc::now(),
        }
    }
}

impl ClipItem {
    pub fn is_image(&self) -> bool {
        self.kind == "image"
    }

    /// One-line label used by lists.
    pub fn label(&self) -> String {
        if self.is_image() {
            "Image".into()
        } else {
            self.text.replace(['\r', '\n'], " ")
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipKind {
    Text,
    Link,
    Image,
    Color,
}

/// Presentation derived from a clip; the stored text and kind are never changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmartClip {
    pub kind: ClipKind,
    pub hex: String,
    pub rgb: String,
    pub cleaned_link: String,
}

impl SmartClip {
    fn plain(kind: ClipKind) -> Self {
        Self {
            kind,
            hex: String::new(),
            rgb: String::new(),
            cleaned_link: String::new(),
        }
    }
}

fn parse_hex_color(text: &str) -> Option<(u8, u8, u8)> {
    let digits = text.strip_prefix('#')?;
    if !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let full: String = match digits.len() {
        3 => digits.chars().flat_map(|c| [c, c]).collect(),
        6 => digits.to_string(),
        _ => return None,
    };
    let channel = |i: usize| u8::from_str_radix(&full[i..i + 2], 16).ok();
    Some((channel(0)?, channel(2)?, channel(4)?))
}

fn parse_rgb_color(text: &str) -> Option<(u8, u8, u8)> {
    let head = text.get(..4)?;
    if !head.eq_ignore_ascii_case("rgb(") {
        return None;
    }
    let inner = text[4..].strip_suffix(')')?;
    let mut channels = [0u8; 3];
    let mut parts = inner.split(',');
    for channel in channels.iter_mut() {
        let part = parts.next()?.trim_matches(char::is_whitespace);
        if part.is_empty() || part.len() > 3 || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        *channel = u8::try_from(part.parse::<u16>().ok()?).ok()?;
    }
    parts
        .next()
        .is_none()
        .then_some((channels[0], channels[1], channels[2]))
}

/// C# `SmartClipboard.Classify` for a clip.
pub fn smart_classify(item: &ClipItem) -> SmartClip {
    if item.is_image() {
        return SmartClip::plain(ClipKind::Image);
    }
    let text = item.text.as_str();
    if let Some((r, g, b)) = parse_hex_color(text).or_else(|| parse_rgb_color(text)) {
        return SmartClip {
            kind: ClipKind::Color,
            hex: format!("#{r:02X}{g:02X}{b:02X}"),
            rgb: format!("rgb({r}, {g}, {b})"),
            cleaned_link: String::new(),
        };
    }
    match try_clean_link(text) {
        Ok(cleaned) => SmartClip {
            cleaned_link: cleaned,
            ..SmartClip::plain(ClipKind::Link)
        },
        Err(_) => SmartClip::plain(ClipKind::Text),
    }
}

/// Kind of a plain clipboard string: color (whole-text `#abc`, `#aabbcc`, `rgb(r, g, b)`),
/// link (valid HTTP/HTTPS URL) or text.
pub fn classify(text: &str) -> ClipKind {
    smart_classify(&ClipItem {
        text: text.to_string(),
        ..Default::default()
    })
    .kind
}

/// Whether `variant` may be copied instead of the clip's own text: the original, the cleaned
/// link of a link, or the hex/rgb form of a color. Nothing else can be exported.
pub fn is_allowed_copy_variant(item: &ClipItem, variant: &str) -> bool {
    if variant == item.text {
        return true;
    }
    let smart = smart_classify(item);
    (smart.kind == ClipKind::Link && variant == smart.cleaned_link)
        || (smart.kind == ClipKind::Color && (variant == smart.hex || variant == smart.rgb))
}

/// Clip list rules: opt-in capture, pause, dedupe, pinned/unpinned caps and purge on disable.
/// Image bytes live on disk; paths that stop being referenced are queued for deletion in
/// `take_discarded` so the owner (which knows the cache directory) can remove the files.
#[derive(Debug, Clone, Default)]
pub struct ClipboardHistory {
    clips: Vec<ClipItem>,
    enabled: bool,
    paused: bool,
    configured: bool,
    discarded: Vec<String>,
}

impl ClipboardHistory {
    pub fn from_items(clips: Vec<ClipItem>) -> Self {
        Self {
            clips,
            ..Default::default()
        }
    }

    pub fn items(&self) -> &[ClipItem] {
        &self.clips
    }
    pub fn items_mut(&mut self) -> &mut Vec<ClipItem> {
        &mut self.clips
    }
    pub fn enabled(&self) -> bool {
        self.enabled
    }
    pub fn paused(&self) -> bool {
        self.paused
    }
    pub fn capturing(&self) -> bool {
        self.enabled && !self.paused
    }
    pub fn find(&self, id: &str) -> Option<&ClipItem> {
        self.clips.iter().find(|c| c.id == id)
    }

    /// Image files no longer referenced by the history.
    pub fn take_discarded(&mut self) -> Vec<String> {
        std::mem::take(&mut self.discarded)
    }

    fn discard(&mut self, clip: &ClipItem) {
        if clip.is_image() && !clip.image_path.is_empty() {
            self.discarded.push(clip.image_path.clone());
        }
    }

    /// Returns true when the state changed (listener must attach or detach).
    pub fn set_enabled(&mut self, enabled: bool) -> bool {
        if !enabled {
            if self.configured && !self.enabled && !self.paused && self.clips.is_empty() {
                return false;
            }
            self.enabled = false;
            self.paused = false;
            self.clear();
            self.configured = true;
            return true;
        }
        if self.enabled {
            return false;
        }
        self.enabled = true;
        true
    }

    pub fn set_paused(&mut self, paused: bool) -> bool {
        if self.paused == paused {
            return false;
        }
        self.paused = paused;
        true
    }

    /// Records copied text. A repeat moves the existing clip to the front and returns false.
    pub fn ingest_text(&mut self, text: Option<&str>, now: DateTime<Utc>) -> bool {
        let Some(text) = text else { return false };
        if !self.capturing() || text.trim().is_empty() {
            return false;
        }
        let value = bound(text, MAX_CLIP_TEXT);
        if let Some(existing) = self
            .clips
            .iter()
            .find(|c| !c.is_image() && c.text == value)
            .map(|c| c.id.clone())
        {
            self.touch(&existing, now);
            return false;
        }
        self.clips.insert(
            0,
            ClipItem {
                text: value,
                timestamp: now,
                ..Default::default()
            },
        );
        self.trim();
        true
    }

    /// Records an image already written to `path`. The caller deduplicates by content first.
    pub fn add_image(&mut self, path: String, now: DateTime<Utc>) {
        self.clips.insert(
            0,
            ClipItem {
                kind: "image".into(),
                image_path: path,
                timestamp: now,
                ..Default::default()
            },
        );
        self.trim();
    }

    pub fn touch(&mut self, id: &str, now: DateTime<Utc>) {
        if let Some(index) = self.clips.iter().position(|c| c.id == id) {
            let mut clip = self.clips.remove(index);
            clip.timestamp = now;
            self.clips.insert(0, clip);
        }
    }

    /// Pins or unpins; pinning is refused at 20 pinned clips. Returns true when changed.
    pub fn toggle_pin(&mut self, id: &str) -> bool {
        let pinned = self.clips.iter().filter(|c| c.pinned).count();
        let Some(clip) = self.clips.iter_mut().find(|c| c.id == id) else {
            return false;
        };
        if !clip.pinned && pinned >= MAX_PINNED {
            return false;
        }
        clip.pinned = !clip.pinned;
        self.trim();
        true
    }

    pub fn remove(&mut self, id: &str) -> bool {
        let Some(index) = self.clips.iter().position(|c| c.id == id) else {
            return false;
        };
        let clip = self.clips.remove(index);
        self.discard(&clip);
        true
    }

    pub fn clear(&mut self) {
        for clip in std::mem::take(&mut self.clips) {
            self.discard(&clip);
        }
    }

    /// Keeps the newest 20 pinned and 30 unpinned clips.
    pub fn trim(&mut self) {
        let (mut pinned, mut regular) = (0, 0);
        let mut kept = Vec::with_capacity(self.clips.len());
        for clip in std::mem::take(&mut self.clips) {
            let keep = if clip.pinned {
                pinned += 1;
                pinned <= MAX_PINNED
            } else {
                regular += 1;
                regular <= MAX_UNPINNED
            };
            if keep {
                kept.push(clip);
            } else {
                self.discard(&clip);
            }
        }
        self.clips = kept;
    }

    /// Cleans a stored list: invalid or duplicate clips are dropped, newest first, ids unique.
    /// `valid_image` decides whether an image clip's file is usable. Nothing is deleted here;
    /// orphaned files are purged by the owner.
    pub fn normalize(
        &mut self,
        canonical: impl Fn(&str) -> Option<String>,
        valid_image: impl Fn(&str) -> bool,
    ) {
        let mut clips: Vec<ClipItem> = std::mem::take(&mut self.clips)
            .into_iter()
            .filter(|c| match c.kind.as_str() {
                "text" => !c.text.trim().is_empty(),
                "image" => valid_image(&c.image_path),
                _ => false,
            })
            .collect();
        clips.sort_by_key(|c| std::cmp::Reverse(c.timestamp));
        let mut ids = HashSet::new();
        let mut texts = HashSet::new();
        let mut images = HashSet::new();
        for mut clip in clips {
            if clip.is_image() {
                clip.text.clear();
                match canonical(&clip.image_path) {
                    Some(path) => clip.image_path = path,
                    None => continue,
                }
                if !images.insert(clip.image_path.to_lowercase()) {
                    continue;
                }
            } else {
                clip.text = bound(&clip.text, MAX_CLIP_TEXT);
                clip.image_path.clear();
                if !texts.insert(clip.text.clone()) {
                    continue;
                }
            }
            if clip.id.trim().is_empty() || !ids.insert(clip.id.clone()) {
                clip.id = new_id();
                ids.insert(clip.id.clone());
            }
            self.clips.push(clip);
        }
        self.trim();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        Utc::now()
    }
    fn on() -> ClipboardHistory {
        let mut h = ClipboardHistory::default();
        h.set_enabled(true);
        h
    }
    fn text(h: &ClipboardHistory, i: usize) -> &str {
        &h.items()[i].text
    }

    #[test]
    fn whole_hex_values_normalize_without_altering_source() {
        let clip = ClipItem {
            text: "#aBc".into(),
            ..Default::default()
        };
        let r = smart_classify(&clip);
        assert_eq!(
            (r.kind, r.hex.as_str(), r.rgb.as_str()),
            (ClipKind::Color, "#AABBCC", "rgb(170, 187, 204)")
        );
        assert_eq!((clip.kind.as_str(), clip.text.as_str()), ("text", "#aBc"));
        assert_eq!(
            smart_classify(&ClipItem {
                text: "#0012ff".into(),
                ..Default::default()
            })
            .hex,
            "#0012FF"
        );
    }

    #[test]
    fn rgb_accepts_only_complete_integer_channels_within_bounds() {
        let hex = |t: &str| {
            smart_classify(&ClipItem {
                text: t.into(),
                ..Default::default()
            })
            .hex
        };
        assert_eq!(hex("rgb(0, 255, 16)"), "#00FF10");
        assert_eq!(classify("RGB(255,0,0)"), ClipKind::Color);
        assert_eq!(classify("rgb( 1 ,2 , 3 )"), ClipKind::Color);
        for value in [
            "rgb(256,0,0)",
            "rgb(-1,0,0)",
            "rgb(1.5,2,3)",
            "rgba(1,2,3,1)",
            "rgb(1%,2%,3%)",
            "rgb(1,2,3)\n",
            " rgb(1,2,3)",
            "rgb(1,2,3) text",
            "#fff\n",
            " #fff",
            "#ffff",
            "#12345678",
            "#ggg",
            "color: #fff",
            "rgb(1,2)",
            "rgb(1,2,3,4)",
            "rgb(0001,2,3)",
            "rgb(,2,3)",
        ] {
            assert_eq!(classify(value), ClipKind::Text, "{value:?}");
        }
    }

    #[test]
    fn links_use_validation_and_expose_a_clean_variant() {
        let original = "https://example.com/a?q=keep&utm_source=test#anchor";
        let clip = ClipItem {
            text: original.into(),
            ..Default::default()
        };
        let r = smart_classify(&clip);
        assert_eq!(r.kind, ClipKind::Link);
        assert_eq!(r.cleaned_link, "https://example.com/a?q=keep#anchor");
        assert_eq!(clip.text, original);
        for value in [
            "file:///C:/private.txt",
            "javascript:alert(1)",
            "ftp://example.com",
            "https://user:password@example.com",
            "example.com",
            "read https://example.com",
            "https://example.com/\nsecret",
        ] {
            assert_eq!(classify(value), ClipKind::Text, "{value:?}");
        }
        let signed = "https://example.com/?sig=secret&utm_source=keep";
        assert_eq!(
            smart_classify(&ClipItem {
                text: signed.into(),
                ..Default::default()
            })
            .cleaned_link,
            signed
        );
        let image = ClipItem {
            kind: "image".into(),
            text: "#fff".into(),
            ..Default::default()
        };
        assert_eq!(smart_classify(&image).kind, ClipKind::Image);
    }

    #[test]
    fn copy_variants_are_limited_to_derived_forms() {
        let color = ClipItem {
            text: "#abc".into(),
            ..Default::default()
        };
        assert!(is_allowed_copy_variant(&color, "#abc"));
        assert!(is_allowed_copy_variant(&color, "#AABBCC"));
        assert!(is_allowed_copy_variant(&color, "rgb(170, 187, 204)"));
        assert!(!is_allowed_copy_variant(&color, "anything else"));
        let link = ClipItem {
            text: "https://e.com/?utm_a=1".into(),
            ..Default::default()
        };
        assert!(is_allowed_copy_variant(&link, "https://e.com/"));
        assert!(!is_allowed_copy_variant(&link, "https://evil.com/"));
    }

    #[test]
    fn clipboard_is_disabled_by_default_and_ingest_is_ignored() {
        let mut h = ClipboardHistory::default();
        assert!(!h.enabled() && !h.ingest_text(Some("private"), now()) && h.items().is_empty());
    }

    #[test]
    fn deduplication_empty_exclusion_and_text_cap() {
        let mut h = on();
        assert!(!h.ingest_text(None, now()) && !h.ingest_text(Some("\r\n "), now()));
        assert!(
            h.ingest_text(Some("first"), now())
                && h.ingest_text(Some("second"), now())
                && !h.ingest_text(Some("first"), now())
        );
        assert_eq!(h.items().len(), 2);
        assert_eq!(text(&h, 0), "first");
        assert!(h.ingest_text(Some(&"a".repeat(21_000)), now()));
        assert_eq!(text(&h, 0).len(), 20_000);
    }

    #[test]
    fn pause_stops_capture() {
        let mut h = on();
        h.set_paused(true);
        assert!(!h.ingest_text(Some("paused"), now()) && h.items().is_empty());
        h.set_paused(false);
        assert!(h.ingest_text(Some("resumed"), now()));
    }

    #[test]
    fn separate_pinned_and_unpinned_limits_retain_newest_entries() {
        let mut h = on();
        for i in 0..20 {
            h.ingest_text(Some(&format!("pin{i}")), now());
            let id = h.items()[0].id.clone();
            assert!(h.toggle_pin(&id));
        }
        for i in 0..40 {
            h.ingest_text(Some(&format!("item{i}")), now());
        }
        let pinned = |h: &ClipboardHistory| h.items().iter().filter(|c| c.pinned).count();
        assert_eq!((h.items().len(), pinned(&h)), (50, 20));
        assert!(!h.items().iter().any(|c| c.text == "item0"));
        assert!(h.items().iter().any(|c| c.text == "pin0"));
        // A 21st pin is refused.
        let id = h.items().iter().find(|c| !c.pinned).unwrap().id.clone();
        assert!(!h.toggle_pin(&id));
        assert_eq!(pinned(&h), 20);
        // Unpinning pushes the oldest unpinned out of the 30 cap.
        let id = h.items().iter().find(|c| c.pinned).unwrap().id.clone();
        assert!(h.toggle_pin(&id));
        assert_eq!((h.items().len(), pinned(&h)), (49, 19));
    }

    #[test]
    fn clipboard_off_clears_history() {
        let mut h = on();
        h.ingest_text(Some("secret"), now());
        h.add_image("/cache/a.png".into(), now());
        h.set_paused(true);
        assert!(h.set_enabled(false));
        assert!(!h.enabled() && !h.paused() && h.items().is_empty());
        assert_eq!(h.take_discarded(), vec!["/cache/a.png".to_string()]);
        assert!(!h.ingest_text(Some("ignored"), now()));
        // Already off and empty: nothing more to do.
        assert!(!h.set_enabled(false));
    }

    #[test]
    fn disabled_cleanup_still_works_for_restored_history() {
        let mut h = ClipboardHistory::from_items(vec![ClipItem {
            text: "persist".into(),
            ..Default::default()
        }]);
        assert!(!h.enabled() && !h.ingest_text(Some("no capture"), now()));
        assert!(h.set_enabled(false));
        assert!(h.items().is_empty());
    }

    #[test]
    fn evicting_an_unpinned_image_queues_its_file() {
        let mut h = on();
        h.add_image("/cache/old.png".into(), now());
        let id = h.items()[0].id.clone();
        for i in 0..30 {
            h.ingest_text(Some(&format!("newer {i}")), now());
        }
        assert_eq!(h.items().len(), 30);
        assert!(h.find(&id).is_none());
        assert_eq!(h.take_discarded(), vec!["/cache/old.png".to_string()]);
    }

    #[test]
    fn remove_queues_image_and_touch_moves_to_front() {
        let mut h = on();
        h.add_image("/cache/a.png".into(), now());
        let image = h.items()[0].id.clone();
        h.ingest_text(Some("a"), now());
        h.touch(&image, now());
        assert_eq!(h.items()[0].id, image);
        assert!(h.remove(&image));
        assert!(!h.remove(&image));
        assert_eq!(h.take_discarded().len(), 1);
    }

    #[test]
    fn normalize_cleans_stored_clips() {
        let t = |s: &str| ClipItem {
            text: s.into(),
            ..Default::default()
        };
        let mut h = ClipboardHistory::from_items(vec![
            ClipItem {
                kind: "image".into(),
                image_path: "/outside.png".into(),
                ..Default::default()
            },
            ClipItem {
                kind: "invalid".into(),
                ..Default::default()
            },
            t("okay"),
            t("okay"),
            t("   "),
            ClipItem {
                id: String::new(),
                ..t("other")
            },
        ]);
        h.normalize(|p| Some(p.to_string()), |_| false);
        let mut texts: Vec<_> = h.items().iter().map(|c| c.text.as_str()).collect();
        texts.sort_unstable();
        assert_eq!(texts, ["okay", "other"]);
        assert!(h.items().iter().all(|c| c.id.len() == 32));
    }
}
