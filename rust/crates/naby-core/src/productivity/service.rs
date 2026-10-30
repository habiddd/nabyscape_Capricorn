//! `ProductivityService`: owns notes, tasks, the file shelf and the clipboard history, and the
//! on-disk image cache behind it (C# `ProductivityService` minus the Win32 clipboard listener).
//!
//! Persistence is injected: the host sets a saver that writes the `productivity` document through
//! the Store (`DOCUMENT_NAME`), and builds the service from the loaded `ProductivityData`.

use super::clipboard::{
    is_allowed_copy_variant, ClipItem, ClipboardHistory, MAX_IMAGE_BYTES, MAX_IMAGE_PIXELS,
};
use super::notes::{bound, MAX_NOTE};
use super::shelf::{canonical_path, ShelfItem};
use super::tasks::TaskItem;
use super::{list_skip_null, new_id, null_default, shelf, tasks};
use crate::store::Store;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;

/// Store document name of `ProductivityData`.
pub const DOCUMENT_NAME: &str = "productivity";
/// Sub-folder of the data directory holding clipboard images.
pub const CACHE_FOLDER: &str = "clipboard-images";

/// The persisted document. Unknown keys survive a load/save cycle.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct ProductivityData {
    #[serde(deserialize_with = "null_default")]
    pub note: String,
    #[serde(deserialize_with = "list_skip_null")]
    pub tasks: Vec<TaskItem>,
    #[serde(deserialize_with = "list_skip_null")]
    pub shelf: Vec<ShelfItem>,
    #[serde(deserialize_with = "list_skip_null")]
    pub clips: Vec<ClipItem>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

pub type Saver = Box<dyn FnMut(&ProductivityData) -> Result<(), String>>;

/// Width and height from a PNG header, `None` when it is not a plausible PNG.
pub fn png_dimensions(header: &[u8]) -> Option<(u32, u32)> {
    const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    if header.len() < 33
        || header[..8] != SIGNATURE
        || header[8..12] != [0, 0, 0, 13]
        || &header[12..16] != b"IHDR"
    {
        return None;
    }
    let width = u32::from_be_bytes(header[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(header[20..24].try_into().ok()?);
    (width > 0 && height > 0 && u64::from(width) * u64::from(height) <= MAX_IMAGE_PIXELS)
        .then_some((width, height))
}

fn content_hash(bytes: &[u8]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

fn is_link(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
}

pub struct ProductivityService {
    note: String,
    tasks: Vec<TaskItem>,
    shelf: Vec<ShelfItem>,
    history: ClipboardHistory,
    extra: Map<String, Value>,
    cache: String,
    saver: Option<Saver>,
    last_error: Option<String>,
    revision: u64,
    disposed: bool,
    image_hashes: HashMap<String, u64>,
    validated: RefCell<HashMap<String, (u64, Option<SystemTime>)>>,
}

impl ProductivityService {
    /// `data_dir` is the Store directory; the image cache lives in `data_dir/clipboard-images`.
    pub fn new(data_dir: &Path, data: ProductivityData) -> Self {
        let cache =
            canonical_path(&data_dir.join(CACHE_FOLDER).to_string_lossy()).unwrap_or_default();
        let mut service = Self {
            note: bound(&data.note, MAX_NOTE),
            tasks: data.tasks,
            shelf: data.shelf,
            history: ClipboardHistory::from_items(data.clips),
            extra: data.extra,
            cache,
            saver: None,
            last_error: None,
            revision: 0,
            disposed: false,
            image_hashes: HashMap::new(),
            validated: RefCell::new(HashMap::new()),
        };
        service.normalize();
        service.purge_orphan_images();
        service
    }

    /// Loads the `productivity` document from `store` and persists every change back to it.
    /// An unreadable document starts empty; the store then refuses to overwrite it until a later
    /// load succeeds, so the failure shows up as `last_error` on the first save.
    pub fn open(store: &Arc<Store>) -> Self {
        let data = store
            .load::<ProductivityData>(DOCUMENT_NAME)
            .ok()
            .flatten()
            .unwrap_or_default();
        let mut service = Self::new(store.dir(), data);
        let target = Arc::clone(store);
        service.set_saver(Box::new(move |data| {
            target.save(DOCUMENT_NAME, data).map_err(|e| e.to_string())
        }));
        service
    }

    pub fn set_saver(&mut self, saver: Saver) {
        self.saver = Some(saver);
    }

    /// Snapshot of the normalized document.
    pub fn to_data(&self) -> ProductivityData {
        ProductivityData {
            note: self.note.clone(),
            tasks: self.tasks.clone(),
            shelf: self.shelf.clone(),
            clips: self.history.items().to_vec(),
            extra: self.extra.clone(),
        }
    }

    // ---- read access ---------------------------------------------------------------------

    pub fn note(&self) -> &str {
        &self.note
    }
    pub fn tasks(&self) -> &[TaskItem] {
        &self.tasks
    }
    pub fn shelf(&self) -> &[ShelfItem] {
        &self.shelf
    }
    pub fn clips(&self) -> &[ClipItem] {
        self.history.items()
    }
    pub fn clipboard_enabled(&self) -> bool {
        self.history.enabled()
    }
    pub fn clipboard_paused(&self) -> bool {
        self.history.paused()
    }
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }
    /// Incremented on every change (C# `Changed` event); views repaint when it moves.
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn cache_dir(&self) -> &Path {
        Path::new(&self.cache)
    }

    // ---- persistence ---------------------------------------------------------------------

    /// Normalizes and writes through the saver. False when disposed or the write failed (the
    /// reason is in `last_error`; in-memory state is kept).
    pub fn save(&mut self) -> bool {
        if self.disposed {
            return false;
        }
        self.normalize();
        let data = self.to_data();
        match self.saver.as_mut() {
            Some(saver) => match saver(&data) {
                Ok(()) => true,
                Err(message) => {
                    self.last_error = Some(message);
                    false
                }
            },
            None => true,
        }
    }

    fn commit(&mut self) {
        self.save();
        self.revision += 1;
    }

    fn report_error(&mut self, error: impl ToString) {
        self.last_error = Some(error.to_string());
    }

    // ---- notes ---------------------------------------------------------------------------

    /// Updates the note in memory only; the editor owns its debounce and calls `save`.
    pub fn set_note(&mut self, text: &str) -> bool {
        if self.disposed {
            return false;
        }
        let next = bound(text, MAX_NOTE);
        if next == self.note {
            return false;
        }
        self.note = next;
        self.revision += 1;
        true
    }

    // ---- tasks ---------------------------------------------------------------------------

    pub fn add_task(&mut self, text: &str) -> bool {
        if self.disposed || !tasks::add_task(&mut self.tasks, text) {
            return false;
        }
        self.commit();
        true
    }

    pub fn remove_task(&mut self, id: &str) {
        if !self.disposed && tasks::remove_task(&mut self.tasks, id) {
            self.commit();
        }
    }

    /// Checking a task saves without rebuilding the list (C# toggles `Done` then `Save`).
    pub fn set_task_done(&mut self, id: &str, done: bool) -> bool {
        if self.disposed {
            return false;
        }
        match self.tasks.iter_mut().find(|t| t.id == id) {
            Some(task) => {
                task.done = done;
                self.save()
            }
            None => false,
        }
    }

    // ---- shelf ---------------------------------------------------------------------------

    pub fn add_files<I, S>(&mut self, paths: I) -> usize
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        if self.disposed {
            return 0;
        }
        let added = shelf::add_files(&mut self.shelf, paths);
        if added > 0 {
            self.commit();
        }
        added
    }

    pub fn remove_file(&mut self, path: &str) {
        if !self.disposed && shelf::remove_file(&mut self.shelf, path) {
            self.commit();
        }
    }

    // ---- clipboard -----------------------------------------------------------------------

    /// Turns capture on or off. Disabling purges history and cached images. Returns true when
    /// the state changed, so the host attaches or detaches its clipboard listener.
    pub fn set_clipboard_enabled(&mut self, enabled: bool) -> bool {
        if self.disposed {
            return false;
        }
        let changed = self.history.set_enabled(enabled);
        if changed {
            if enabled {
                self.last_error = None;
                self.revision += 1;
            } else {
                self.after_history_change();
                self.purge_orphan_images();
                self.image_hashes.clear();
                self.commit();
            }
        }
        changed
    }

    pub fn set_clipboard_paused(&mut self, paused: bool) {
        if !self.disposed && self.history.set_paused(paused) {
            self.revision += 1;
        }
    }

    pub fn ingest_text(&mut self, text: Option<&str>) -> bool {
        if self.disposed {
            return false;
        }
        let before = self.history.items().len();
        let added = self.history.ingest_text(text, Utc::now());
        // A repeat also changed the order (touch) without adding.
        let touched = !added && text.is_some() && self.history.capturing() && before > 0;
        if added || touched {
            self.after_history_change();
            self.commit();
        }
        added
    }

    /// Stores a PNG copied by the user. Rejects empty, oversized, malformed or huge images and
    /// images identical to one already stored (which is moved to the front instead).
    pub fn ingest_image(&mut self, png: &[u8]) -> bool {
        if self.disposed
            || !self.history.capturing()
            || png.is_empty()
            || png.len() > MAX_IMAGE_BYTES
        {
            return false;
        }
        if png_dimensions(png).is_none() {
            return false;
        }
        let hash = content_hash(png);
        let candidates: Vec<(String, String)> = self
            .history
            .items()
            .iter()
            .filter(|c| c.is_image())
            .map(|c| (c.id.clone(), c.image_path.clone()))
            .collect();
        for (id, path) in candidates {
            let known = match self.image_hashes.get(&path).copied() {
                Some(h) => Some(h),
                None if self.valid_image_file(&path) => {
                    let h = std::fs::read(&path).ok().map(|b| content_hash(&b));
                    if let Some(h) = h {
                        self.image_hashes.insert(path.clone(), h);
                    }
                    h
                }
                None => None,
            };
            if known == Some(hash) && std::fs::read(&path).map(|b| b == png).unwrap_or(false) {
                self.history.touch(&id, Utc::now());
                self.commit();
                return false;
            }
        }
        let path = match self.write_image(png) {
            Ok(path) => path,
            Err(error) => {
                self.report_error(error);
                return false;
            }
        };
        self.image_hashes.insert(path.clone(), hash);
        self.history.add_image(path, Utc::now());
        self.after_history_change();
        self.commit();
        true
    }

    fn write_image(&self, png: &[u8]) -> std::io::Result<String> {
        let cache = Path::new(&self.cache);
        std::fs::create_dir_all(cache)?;
        if is_link(cache) {
            return Err(std::io::Error::other(
                "The clipboard cache folder cannot be a link.",
            ));
        }
        let path = cache.join(format!("{}.png", new_id()));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        if let Err(error) = file.write_all(png).and_then(|()| file.flush()) {
            drop(file);
            let _ = std::fs::remove_file(&path);
            return Err(error);
        }
        Ok(canonical_path(&path.to_string_lossy())
            .unwrap_or_else(|| path.to_string_lossy().into_owned()))
    }

    pub fn toggle_clip_pin(&mut self, id: &str) {
        if !self.disposed && self.history.toggle_pin(id) {
            self.after_history_change();
            self.commit();
        }
    }

    pub fn remove_clip(&mut self, id: &str) {
        if !self.disposed && self.history.remove(id) {
            self.after_history_change();
            self.commit();
        }
    }

    pub fn clear_clips(&mut self) {
        if self.disposed {
            return;
        }
        self.history.clear();
        self.after_history_change();
        self.image_hashes.clear();
        self.purge_orphan_images();
        self.commit();
    }

    /// The text to put on the clipboard for a text clip, optionally a derived variant (cleaned
    /// link, hex or rgb). `None` when capture is off, the clip is unknown, is an image, or the
    /// variant is not one of the allowed forms.
    pub fn text_for_copy(&self, id: &str, variant: Option<&str>) -> Option<String> {
        if self.disposed || !self.history.enabled() {
            return None;
        }
        let clip = self.history.find(id).filter(|c| !c.is_image())?;
        match variant {
            Some(v) if !is_allowed_copy_variant(clip, v) => None,
            Some(v) => Some(v.to_string()),
            None => Some(clip.text.clone()),
        }
    }

    /// PNG bytes of an image clip, only while capture is on and the cached file is valid.
    pub fn clip_image_bytes(&mut self, id: &str) -> Option<Vec<u8>> {
        if self.disposed || !self.history.enabled() {
            return None;
        }
        let path = self
            .history
            .find(id)
            .filter(|c| c.is_image())?
            .image_path
            .clone();
        if !self.valid_image_file(&path) {
            return None;
        }
        match std::fs::read(&path) {
            Ok(bytes) => Some(bytes),
            Err(error) => {
                self.report_error(error);
                None
            }
        }
    }

    /// Exports an image clip. The bytes are read before the destination is opened, so a failure
    /// cannot truncate an existing file.
    pub fn save_clip_image(&mut self, id: &str, destination: &Path) -> bool {
        let Some(bytes) = self.clip_image_bytes(id) else {
            return false;
        };
        match std::fs::write(destination, bytes) {
            Ok(()) => {
                self.last_error = None;
                true
            }
            Err(error) => {
                self.report_error(error);
                false
            }
        }
    }

    pub fn dispose(&mut self) {
        if self.disposed {
            return;
        }
        self.save();
        self.disposed = true;
    }

    // ---- internals -----------------------------------------------------------------------

    fn normalize(&mut self) {
        self.note = bound(&self.note, MAX_NOTE);
        tasks::normalize(&mut self.tasks);
        shelf::normalize(&mut self.shelf);
        let (cache, validated) = (&self.cache, &self.validated);
        self.history
            .normalize(canonical_path, |p| Self::valid_image(cache, validated, p));
    }

    /// Deletes images dropped from the history.
    fn after_history_change(&mut self) {
        for path in self.history.take_discarded() {
            self.delete_image(&path);
        }
    }

    fn confined_image(cache: &str, path: &str) -> bool {
        if path.is_empty() {
            return false;
        }
        let Some(full) = canonical_path(path) else {
            return false;
        };
        let full = Path::new(&full);
        let in_cache = full.parent().and_then(Path::to_str).map(str::to_lowercase)
            == Some(cache.to_lowercase());
        let png = full
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("png"));
        let guid = full
            .file_stem()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit()));
        in_cache && png && guid && !is_link(Path::new(cache))
    }

    fn valid_image(
        cache: &str,
        validated: &RefCell<HashMap<String, (u64, Option<SystemTime>)>>,
        path: &str,
    ) -> bool {
        if !Self::confined_image(cache, path) {
            return false;
        }
        let Ok(meta) = std::fs::symlink_metadata(path) else {
            return false;
        };
        if !meta.is_file() || meta.len() == 0 || meta.len() > MAX_IMAGE_BYTES as u64 {
            return false;
        }
        let version = (meta.len(), meta.modified().ok());
        if validated.borrow().get(path) == Some(&version) {
            return true;
        }
        let mut header = [0u8; 33];
        let ok = std::fs::File::open(path)
            .and_then(|mut f| std::io::Read::read_exact(&mut f, &mut header))
            .is_ok()
            && png_dimensions(&header).is_some();
        if ok {
            validated.borrow_mut().insert(path.to_string(), version);
        }
        ok
    }

    fn valid_image_file(&self, path: &str) -> bool {
        Self::valid_image(&self.cache, &self.validated, path)
    }

    fn delete_image(&mut self, path: &str) {
        if !Self::confined_image(&self.cache, path) {
            return;
        }
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => self.report_error(error),
        }
        self.image_hashes.remove(path);
        self.validated.borrow_mut().remove(path);
    }

    /// Removes cache PNGs no clip references (leftovers from crashes or disabled capture).
    fn purge_orphan_images(&mut self) {
        let cache = Path::new(&self.cache);
        if !cache.is_dir() || is_link(cache) {
            return;
        }
        let retained: HashSet<String> = self
            .history
            .items()
            .iter()
            .filter(|c| c.is_image())
            .map(|c| c.image_path.to_lowercase())
            .collect();
        let Ok(entries) = std::fs::read_dir(cache) else {
            return;
        };
        let orphans: Vec<String> = entries
            .flatten()
            .filter_map(|e| canonical_path(&e.path().to_string_lossy()))
            .filter(|p| p.to_lowercase().ends_with(".png") && !retained.contains(&p.to_lowercase()))
            .collect();
        for path in orphans {
            self.delete_image(&path);
        }
    }
}

impl Drop for ProductivityService {
    fn drop(&mut self) {
        self.dispose();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;

    /// 1x1 PNG, enough for the header checks (the cache never decodes pixels).
    fn png(seed: u8) -> Vec<u8> {
        let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13];
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&1u32.to_be_bytes());
        bytes.extend_from_slice(&1u32.to_be_bytes());
        bytes.extend_from_slice(&[8, 6, 0, 0, 0, 0, 0, 0, 0, seed]);
        bytes
    }

    fn open(root: &Path) -> ProductivityService {
        let file = root.join("productivity.json");
        let data = std::fs::read_to_string(&file)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        let mut service = ProductivityService::new(root, data);
        service.set_saver(Box::new(move |d| {
            std::fs::write(&file, serde_json::to_string_pretty(d).unwrap())
                .map_err(|e| e.to_string())
        }));
        service
    }

    fn dir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }
    fn cache_files(root: &Path) -> usize {
        std::fs::read_dir(root.join(CACHE_FOLDER))
            .map(|d| d.count())
            .unwrap_or(0)
    }

    #[test]
    fn clipboard_is_disabled_by_default_and_ingest_cannot_read_it() {
        let root = dir();
        let mut s = open(root.path());
        assert!(
            !s.clipboard_enabled() && !s.ingest_text(Some("private")) && !s.ingest_image(&png(1))
        );
        assert!(s.clips().is_empty() && s.text_for_copy("x", None).is_none());
    }

    #[test]
    fn notes_tasks_and_completion_survive_restart() {
        let root = dir();
        {
            let mut s = open(root.path());
            s.set_note(&"n".repeat(51_000));
            assert_eq!(s.note().len(), 50_000);
            assert!(!s.add_task(" ") && s.add_task("  tugas  "));
            let id = s.tasks()[0].id.clone();
            assert!(s.set_task_done(&id, true));
        }
        let mut restored = open(root.path());
        assert_eq!(restored.note().len(), 50_000);
        assert_eq!(restored.tasks().len(), 1);
        assert_eq!(restored.tasks()[0].text, "tugas");
        assert!(restored.tasks()[0].done);
        let id = restored.tasks()[0].id.clone();
        restored.remove_task(&id);
        assert!(restored.tasks().is_empty());
    }

    #[test]
    fn tasks_roundtrip_store() {
        let root = dir();
        let mut s = open(root.path());
        s.add_task("one");
        s.add_task("two");
        let raw = std::fs::read_to_string(root.path().join("productivity.json")).unwrap();
        let value: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(value["Tasks"][1]["Text"], "two");
        assert_eq!(value["Tasks"][0]["Done"], false);
        assert!(value["Tasks"][0]["Id"].as_str().unwrap().len() == 32);
        let reloaded: ProductivityData = serde_json::from_str(&raw).unwrap();
        assert_eq!(reloaded.tasks, s.tasks());
        assert_eq!(DOCUMENT_NAME, "productivity");
    }

    #[test]
    fn shelf_survives_restart_without_deleting_broken_references() {
        let root = dir();
        let path = root.path().join("gone.txt").to_string_lossy().into_owned();
        {
            let mut s = open(root.path());
            assert_eq!(s.add_files([&path]), 1);
        }
        let s = open(root.path());
        assert_eq!(s.shelf()[0].path, path);
        assert!(!s.shelf()[0].exists());
    }

    #[test]
    fn unknown_document_keys_are_preserved() {
        let root = dir();
        std::fs::write(
            root.path().join("productivity.json"),
            r#"{"Note":"hi","Future":{"a":[1,2]},"Tasks":null,"Shelf":[null],"Clips":[null,{"Text":"x"}]}"#,
        )
        .unwrap();
        let mut s = open(root.path());
        assert_eq!(s.note(), "hi");
        assert!(s.tasks().is_empty() && s.shelf().is_empty());
        assert_eq!(s.clips().len(), 1);
        s.add_task("t");
        let raw: Value = serde_json::from_str(
            &std::fs::read_to_string(root.path().join("productivity.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(raw["Future"]["a"][1], 2);
    }

    #[test]
    fn image_deduplication_removal_and_size_caps() {
        let root = dir();
        let mut s = open(root.path());
        s.set_clipboard_enabled(true);
        let image = png(1);
        assert!(s.ingest_image(&image) && !s.ingest_image(&image) && s.clips().len() == 1);
        assert!(s.ingest_image(&png(2)));
        assert!(
            !s.ingest_image(&vec![0u8; MAX_IMAGE_BYTES + 1])
                && !s.ingest_image(&[1, 2, 3])
                && !s.ingest_image(&[])
        );
        assert_eq!(s.clips().len(), 2);
        // Re-copying the first image moved it to the front.
        assert!(!s.ingest_image(&image));
        let first = s.clips()[0].clone();
        assert!(Path::new(&first.image_path).exists());
        s.remove_clip(&first.id);
        assert!(!Path::new(&first.image_path).exists());
        assert_eq!(s.clips().len(), 1);
        assert_eq!(s.clip_image_bytes(&first.id), None);
    }

    #[test]
    fn clear_purges_pinned_images_and_leaves_source_files_alone() {
        let root = dir();
        let source = root.path().join("source.png");
        std::fs::write(&source, png(3)).unwrap();
        let mut s = open(root.path());
        s.set_clipboard_enabled(true);
        s.ingest_image(&std::fs::read(&source).unwrap());
        let image = s.clips()[0].clone();
        s.toggle_clip_pin(&image.id);
        s.ingest_text(Some("clear"));
        s.clear_clips();
        assert!(s.clips().is_empty() && !Path::new(&image.image_path).exists() && source.exists());
    }

    #[test]
    fn evicting_an_unpinned_image_removes_its_cached_png() {
        let root = dir();
        let mut s = open(root.path());
        s.set_clipboard_enabled(true);
        s.ingest_image(&png(4));
        let image = s.clips()[0].clone();
        for i in 0..30 {
            s.ingest_text(Some(&format!("newer {i}")));
        }
        assert_eq!(s.clips().len(), 30);
        assert!(!s.clips().iter().any(|c| c.id == image.id));
        assert!(!Path::new(&image.image_path).exists());
    }

    #[test]
    fn clipboard_off_clears_history_and_cache_without_altering_notes() {
        let root = dir();
        {
            let mut s = open(root.path());
            s.set_note("keep");
            s.set_clipboard_enabled(true);
            s.ingest_image(&png(5));
            s.ingest_text(Some("secret"));
            s.set_clipboard_paused(true);
            assert!(s.set_clipboard_enabled(false));
            assert!(!s.clipboard_enabled() && !s.clipboard_paused() && s.clips().is_empty());
            assert!(!s.ingest_text(Some("ignored")));
            s.save();
        }
        let s = open(root.path());
        assert_eq!(s.note(), "keep");
        assert!(s.clips().is_empty());
        assert_eq!(cache_files(root.path()), 0);
    }

    #[test]
    fn restart_loads_history_without_enabling_capture() {
        let root = dir();
        {
            let mut s = open(root.path());
            s.set_clipboard_enabled(true);
            s.ingest_text(Some("persist"));
            let id = s.clips()[0].id.clone();
            s.toggle_clip_pin(&id);
            s.ingest_image(&png(6));
        }
        let mut s = open(root.path());
        assert!(!s.clipboard_enabled());
        assert_eq!(s.clips().len(), 2);
        assert_eq!(s.clips().iter().filter(|c| c.pinned).count(), 1);
        assert!(!s.ingest_text(Some("no capture")));
        s.set_clipboard_enabled(false);
        assert!(s.clips().is_empty());
        assert_eq!(cache_files(root.path()), 0);
    }

    #[test]
    fn orphan_cache_files_are_purged_but_unrelated_names_kept() {
        let root = dir();
        let cache = root.path().join(CACHE_FOLDER);
        std::fs::create_dir_all(&cache).unwrap();
        let orphan = cache.join(format!("{}.png", new_id()));
        std::fs::write(&orphan, png(7)).unwrap();
        let unrelated = cache.join("unrelated.png");
        std::fs::write(&unrelated, png(7)).unwrap();
        let _s = open(root.path());
        assert!(!orphan.exists() && unrelated.exists());
    }

    #[test]
    fn corrupt_cached_images_are_discarded_on_restart() {
        let root = dir();
        let path;
        {
            let mut s = open(root.path());
            s.set_clipboard_enabled(true);
            s.ingest_image(&png(8));
            path = s.clips()[0].image_path.clone();
        }
        std::fs::write(&path, "not a PNG").unwrap();
        let s = open(root.path());
        assert!(s.clips().is_empty());
        assert!(!Path::new(&path).exists());
    }

    #[test]
    fn corrupt_collections_normalize_and_cannot_delete_outside_cache() {
        let root = dir();
        let outside = root.path().join("outside.png");
        std::fs::write(&outside, png(9)).unwrap();
        let outside_s = outside.to_string_lossy().replace('\\', "\\\\");
        let json = format!(
            r#"{{"Note":null,
              "Tasks":[null,{{"Id":"same","Text":"first"}},{{"Id":"same","Text":"second"}},{{"Text":""}}],
              "Shelf":[null,{{"Path":"\u0000"}},{{"Path":"{outside_s}"}},{{"Path":"{outside_s}"}}],
              "Clips":[null,{{"Kind":"image","ImagePath":"{outside_s}"}},{{"Kind":"invalid"}},{{"Text":"okay"}},{{"Text":"okay"}}]}}"#
        );
        let data: ProductivityData = serde_json::from_str(&json).unwrap();
        let mut s = ProductivityService::new(root.path(), data);
        assert_eq!(s.note(), "");
        assert_eq!(s.tasks().len(), 2);
        assert_ne!(s.tasks()[0].id, s.tasks()[1].id);
        assert_eq!(s.shelf().len(), 1);
        assert_eq!(s.clips().len(), 1);
        s.set_clipboard_enabled(true);
        s.history.items_mut().push(ClipItem {
            kind: "image".into(),
            image_path: outside_s.replace("\\\\", "\\"),
            ..Default::default()
        });
        s.clear_clips();
        assert!(outside.exists());
    }

    #[test]
    fn persistence_failures_are_reported_and_note_typing_stays_in_memory() {
        let root = dir();
        let mut s = ProductivityService::new(root.path(), ProductivityData::default());
        let fail = Rc::new(std::cell::Cell::new(false));
        let f = fail.clone();
        s.set_saver(Box::new(move |_| {
            if f.get() {
                Err("disk full".into())
            } else {
                Ok(())
            }
        }));
        assert!(s.save());
        fail.set(true);
        let before = s.revision();
        assert!(s.set_note("editable despite disk error"));
        assert!(!s.set_note("editable despite disk error"));
        assert_eq!(s.revision(), before + 1);
        assert!(s.last_error().is_none());
        assert!(!s.save());
        assert_eq!(s.last_error(), Some("disk full"));
        assert_eq!(s.note(), "editable despite disk error");
    }

    #[test]
    fn copy_text_and_image_export_policies() {
        let root = dir();
        let mut s = open(root.path());
        s.set_clipboard_enabled(true);
        s.ingest_text(Some("#abc"));
        let id = s.clips()[0].id.clone();
        assert_eq!(s.text_for_copy(&id, None).as_deref(), Some("#abc"));
        assert_eq!(
            s.text_for_copy(&id, Some("#AABBCC")).as_deref(),
            Some("#AABBCC")
        );
        assert_eq!(s.text_for_copy(&id, Some("evil")), None);

        s.ingest_image(&png(10));
        let image = s.clips()[0].clone();
        assert_eq!(s.text_for_copy(&image.id, None), None);
        let output = root.path().join("saved.png");
        assert!(s.save_clip_image(&image.id, &output) && output.exists());
        assert!(!s.save_clip_image(&image.id, root.path()) && s.last_error().is_some());
        s.remove_clip(&image.id);
        assert!(!s.save_clip_image(&image.id, &root.path().join("removed.png")));

        let stale = s.clips()[0].id.clone();
        s.set_clipboard_enabled(false);
        assert_eq!(s.text_for_copy(&stale, None), None);
    }

    #[test]
    fn repeated_text_moves_to_front_and_counts_as_change() {
        let root = dir();
        let mut s = open(root.path());
        s.set_clipboard_enabled(true);
        s.ingest_text(Some("a"));
        s.ingest_text(Some("b"));
        let before = s.revision();
        assert!(!s.ingest_text(Some("a")));
        assert_eq!(s.clips()[0].text, "a");
        assert!(s.revision() > before);
    }

    #[test]
    fn png_header_checks() {
        assert_eq!(png_dimensions(&png(0)), Some((1, 1)));
        assert_eq!(png_dimensions(&[0u8; 40]), None);
        let mut huge = png(0);
        huge[16..20].copy_from_slice(&6000u32.to_be_bytes());
        huge[20..24].copy_from_slice(&6000u32.to_be_bytes());
        assert_eq!(png_dimensions(&huge), None);
    }
}
