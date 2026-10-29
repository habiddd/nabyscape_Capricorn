//! Display-size previews of attachments (`AssistantPreviewCache`, `AssistantPreviewImage`).
//!
//! Images are decoded on two worker threads straight to the size they are shown at and kept in
//! a byte-bounded LRU ([`ThumbnailCache`]); the original attachment bytes never enter the cache.
//! Remote (provider-returned) images are downloaded with the current provider's credentials
//! through the [`MediaLoader`] on every miss. Layout never depends on decoded pixels: the natural
//! size is recorded once so a preview can be released and re-decoded without moving the
//! transcript.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use naby_core::assistant::attachments::bytes_from_data_url;
use naby_core::assistant::AttachmentSource;
use naby_render::{Image, ThumbnailCache};

use super::ports::MediaLoader;
use super::service::Waker;
use super::session::ShownFile;

/// Decoded previews stay under this many bytes.
pub const CACHE_BYTES: usize = 12 * 1024 * 1024;
/// A preview unused for this long is released.
pub const IDLE: Duration = Duration::from_secs(30);
const DECODERS: usize = 2;
/// Dimension limit of an image we are willing to preview (`Thumbnail` guard).
const MAXIMUM_PIXELS: u64 = 80_000_000;

/// What a preview request found.
#[derive(Clone)]
pub enum Preview {
    Ready(Arc<Image>),
    Loading,
    /// The image cannot be shown (unsupported codec, expired link, too large).
    Failed,
    /// The attachment has nothing to preview.
    None,
}

impl std::fmt::Debug for Preview {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Preview::Ready(_) => "Ready",
            Preview::Loading => "Loading",
            Preview::Failed => "Failed",
            Preview::None => "None",
        })
    }
}

struct Job {
    key: String,
    id: u64,
    file: Arc<ShownFile>,
    pixels: u32,
    generation: u64,
}

#[derive(Default)]
struct Inner {
    images: Option<ThumbnailCache>,
    natural: HashMap<u64, (u32, u32)>,
    pending: HashSet<String>,
    failed: HashSet<u64>,
}

/// Preview images of one chat window.
pub struct PreviewCache {
    inner: Arc<Mutex<Inner>>,
    loader: Arc<dyn MediaLoader>,
    waker: Waker,
    jobs: Mutex<Option<Sender<Job>>>,
    generation: Arc<AtomicU64>,
    /// Bumps whenever a natural size became known (layout must be recomputed).
    sizes: Arc<AtomicU64>,
    cancel: Arc<AtomicBool>,
}

/// Where the preview pixels come from.
fn source_of(file: &ShownFile) -> Option<Source> {
    let f = &file.file;
    if f.is_image() {
        return Some(Source::Whole);
    }
    f.frames.first().map(|frame| Source::Frame(frame.clone()))
}

enum Source {
    /// The attachment itself.
    Whole,
    /// A rendered page or sampled video frame (inline data URL).
    Frame(String),
}

impl PreviewCache {
    pub fn new(loader: Arc<dyn MediaLoader>, waker: Waker) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                images: Some(ThumbnailCache::new(CACHE_BYTES)),
                ..Inner::default()
            })),
            loader,
            waker,
            jobs: Mutex::new(None),
            generation: Arc::new(AtomicU64::new(0)),
            sizes: Arc::new(AtomicU64::new(0)),
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Whether the attachment has anything to show.
    pub fn has_preview(file: &ShownFile) -> bool {
        source_of(file).is_some()
    }

    /// Moves when a natural size was learned.
    pub fn size_version(&self) -> u64 {
        self.sizes.load(Ordering::SeqCst)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The natural pixel size, when known. Inline images are probed on first use.
    pub fn natural(&self, file: &ShownFile) -> Option<(u32, u32)> {
        if let Some(size) = self.lock().natural.get(&file.id).copied() {
            return Some(size);
        }
        let bytes = match (&file.file.source, source_of(file)?) {
            (_, Source::Frame(frame)) => bytes_from_data_url(&frame).ok()?,
            (AttachmentSource::Inline(url), Source::Whole) => bytes_from_data_url(url).ok()?,
            _ => return None,
        };
        let size = Image::probe_size(&bytes)?;
        self.record_size(file.id, size);
        Some(size)
    }

    fn record_size(&self, id: u64, size: (u32, u32)) {
        if self.lock().natural.insert(id, size) != Some(size) {
            self.sizes.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// The preview of `file` decoded to fit `pixels` x `pixels`.
    pub fn get(&self, file: &Arc<ShownFile>, pixels: u32) -> Preview {
        if source_of(file).is_none() {
            return Preview::None;
        }
        let pixels = pixels.clamp(32, 1800);
        let key = format!("{}:{pixels}", file.id);
        {
            let mut inner = self.lock();
            if let Some(hit) = inner.images.as_mut().and_then(|c| c.get(&key)) {
                return Preview::Ready(hit);
            }
            if inner.failed.contains(&file.id) {
                return Preview::Failed;
            }
            if !inner.pending.insert(key.clone()) {
                return Preview::Loading;
            }
        }
        let job = Job {
            key: key.clone(),
            id: file.id,
            file: file.clone(),
            pixels,
            generation: self.generation.load(Ordering::SeqCst),
        };
        if self.submit(job).is_err() {
            let mut inner = self.lock();
            inner.pending.remove(&key);
            inner.failed.insert(file.id);
            return Preview::Failed;
        }
        Preview::Loading
    }

    fn submit(&self, job: Job) -> Result<(), ()> {
        let mut jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
        if jobs.is_none() {
            let (tx, rx) = channel::<Job>();
            let rx = Arc::new(Mutex::new(rx));
            for n in 0..DECODERS {
                let worker = Worker {
                    rx: rx.clone(),
                    inner: self.inner.clone(),
                    loader: self.loader.clone(),
                    waker: self.waker.clone(),
                    generation: self.generation.clone(),
                    sizes: self.sizes.clone(),
                    cancel: self.cancel.clone(),
                };
                std::thread::Builder::new()
                    .name(format!("NabyScape preview {n}"))
                    .spawn(move || worker.run())
                    .map_err(|_| ())?;
            }
            *jobs = Some(tx);
        }
        jobs.as_ref().ok_or(())?.send(job).map_err(|_| ())
    }

    /// Releases decoded previews not used for a while.
    pub fn trim(&self) {
        if let Some(cache) = self.lock().images.as_mut() {
            cache.trim_idle(IDLE);
        }
    }

    /// Drops every decoded preview (the window hides) but keeps the natural sizes, so the
    /// transcript keeps its extent.
    pub fn release(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        let mut inner = self.lock();
        if let Some(cache) = inner.images.as_mut() {
            cache.clear();
        }
        inner.pending.clear();
    }

    pub fn bytes(&self) -> usize {
        self.lock().images.as_ref().map_or(0, ThumbnailCache::bytes)
    }
}

impl Drop for PreviewCache {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::SeqCst);
        // Dropping the sender ends the decoder threads.
        self.jobs.lock().unwrap_or_else(|e| e.into_inner()).take();
    }
}

struct Worker {
    rx: Arc<Mutex<Receiver<Job>>>,
    inner: Arc<Mutex<Inner>>,
    loader: Arc<dyn MediaLoader>,
    waker: Waker,
    generation: Arc<AtomicU64>,
    sizes: Arc<AtomicU64>,
    cancel: Arc<AtomicBool>,
}

impl Worker {
    fn run(self) {
        loop {
            let job = {
                let rx = self.rx.lock().unwrap_or_else(|e| e.into_inner());
                rx.recv()
            };
            let Ok(job) = job else { return };
            if self.generation.load(Ordering::SeqCst) != job.generation
                || self.cancel.load(Ordering::SeqCst)
            {
                self.lock().pending.remove(&job.key);
                continue;
            }
            let decoded = self.decode(&job);
            let mut inner = self.lock();
            inner.pending.remove(&job.key);
            if self.generation.load(Ordering::SeqCst) == job.generation {
                match decoded {
                    Some((image, natural)) => {
                        if inner.natural.insert(job.id, natural) != Some(natural) {
                            self.sizes.fetch_add(1, Ordering::SeqCst);
                        }
                        if let Some(cache) = inner.images.as_mut() {
                            cache.insert(&job.key, Arc::new(image));
                        }
                    }
                    None => {
                        inner.failed.insert(job.id);
                    }
                }
            }
            drop(inner);
            (self.waker)();
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn decode(&self, job: &Job) -> Option<(Image, (u32, u32))> {
        let bytes = match source_of(&job.file)? {
            Source::Frame(frame) => bytes_from_data_url(&frame).ok()?,
            Source::Whole => self.loader.load(&job.file.file, &self.cancel).ok()?,
        };
        let natural = Image::probe_size(&bytes)?;
        if u64::from(natural.0) * u64::from(natural.1) > MAXIMUM_PIXELS
            || natural.0.max(natural.1) > natural.0.min(natural.1).saturating_mul(30)
        {
            return None;
        }
        let image = Image::decode(&bytes, job.pixels, job.pixels)?;
        Some((image, natural))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assistant::session::shown;
    use naby_core::assistant::attachments::data_url;
    use naby_core::assistant::Attachment;
    use std::sync::atomic::AtomicUsize;
    use std::time::Instant;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, w, h);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            let mut wr = enc.write_header().unwrap();
            let data: Vec<u8> = (0..w * h).flat_map(|_| [200u8, 40, 40, 255]).collect();
            wr.write_image_data(&data).unwrap();
        }
        out
    }

    struct Loader(AtomicUsize);

    impl MediaLoader for Loader {
        fn load(&self, file: &Attachment, _: &AtomicBool) -> Result<Vec<u8>, String> {
            self.0.fetch_add(1, Ordering::SeqCst);
            match &file.source {
                AttachmentSource::Inline(url) => {
                    bytes_from_data_url(url).map_err(|e| e.to_string())
                }
                _ => Err("no data".into()),
            }
        }
    }

    fn cache() -> (PreviewCache, Arc<Loader>, Arc<AtomicUsize>) {
        let loader = Arc::new(Loader(AtomicUsize::new(0)));
        let wakes = Arc::new(AtomicUsize::new(0));
        let w = wakes.clone();
        let cache = PreviewCache::new(
            loader.clone(),
            Arc::new(move || {
                w.fetch_add(1, Ordering::SeqCst);
            }),
        );
        (cache, loader, wakes)
    }

    fn picture(w: u32, h: u32) -> Arc<ShownFile> {
        shown(Attachment::inline(
            "shot.png",
            "image/png",
            data_url("image/png", &png(w, h)),
            "Image",
        ))
    }

    fn wait(cache: &PreviewCache, file: &Arc<ShownFile>, pixels: u32) -> Preview {
        let start = Instant::now();
        loop {
            match cache.get(file, pixels) {
                Preview::Loading => {
                    assert!(start.elapsed() < Duration::from_secs(5));
                    std::thread::sleep(Duration::from_millis(5));
                }
                other => return other,
            }
        }
    }

    #[test]
    fn images_decode_to_display_size_and_are_cached() {
        let (cache, loader, wakes) = cache();
        let file = picture(400, 200);
        assert!(matches!(cache.get(&file, 128), Preview::Loading));
        let Preview::Ready(image) = wait(&cache, &file, 128) else {
            panic!("not ready");
        };
        assert_eq!((image.width(), image.height()), (128, 64));
        assert_eq!(cache.natural(&file), Some((400, 200)));
        assert!(wakes.load(Ordering::SeqCst) > 0);
        assert!(cache.bytes() >= 128 * 64 * 4);
        // A second request is served from the cache without reading the attachment again.
        let reads = loader.0.load(Ordering::SeqCst);
        assert!(matches!(cache.get(&file, 128), Preview::Ready(_)));
        assert_eq!(loader.0.load(Ordering::SeqCst), reads);
    }

    #[test]
    fn natural_size_of_inline_images_is_known_before_decoding() {
        let (cache, loader, _) = cache();
        let file = picture(30, 10);
        assert_eq!(cache.natural(&file), Some((30, 10)));
        assert_eq!(loader.0.load(Ordering::SeqCst), 0);
        assert!(cache.size_version() > 0);
    }

    #[test]
    fn release_drops_pixels_but_keeps_the_layout_size() {
        let (cache, _, _) = cache();
        let file = picture(64, 64);
        assert!(matches!(wait(&cache, &file, 64), Preview::Ready(_)));
        cache.release();
        assert_eq!(cache.bytes(), 0);
        assert_eq!(cache.natural(&file), Some((64, 64)));
        assert!(matches!(wait(&cache, &file, 64), Preview::Ready(_)));
    }

    #[test]
    fn undecodable_and_text_attachments_do_not_preview() {
        let (cache, _, _) = cache();
        let broken = shown(Attachment::inline(
            "bad.png",
            "image/png",
            data_url("image/png", b"not an image"),
            "Image",
        ));
        assert!(matches!(wait(&cache, &broken, 64), Preview::Failed));
        let note = shown(Attachment::from_text("a.txt", "hi".into(), "Document").unwrap());
        assert!(matches!(cache.get(&note, 64), Preview::None));
        assert!(!PreviewCache::has_preview(&note));
        // A PDF with rendered pages previews its first page.
        let pdf = shown(
            Attachment::inline(
                "a.pdf",
                "application/pdf",
                data_url("application/pdf", b"%PDF"),
                "PDF",
            )
            .with_frames(vec![data_url("image/png", &png(20, 30))], ""),
        );
        assert!(PreviewCache::has_preview(&pdf));
        assert!(matches!(wait(&cache, &pdf, 64), Preview::Ready(_)));
    }

    #[test]
    fn oversized_dimensions_are_refused() {
        let (cache, _, _) = cache();
        let strip = picture(2000, 20);
        assert!(matches!(wait(&cache, &strip, 128), Preview::Failed));
    }
}
