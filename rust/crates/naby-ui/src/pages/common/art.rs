//! Cover art for the pages: decode once per artwork identity, crossfade to the next one.

use naby_render::Image;
use std::sync::Arc;

/// The decoded cover of the current track.
#[derive(Default)]
pub struct ArtCache {
    key: usize,
    px: u32,
    image: Option<Arc<Image>>,
    prev: Option<Arc<Image>>,
    since: f64,
}

/// Fade time of `SwapArtwork`.
pub const SWAP_SECS: f64 = 0.32;

impl ArtCache {
    /// The cover for `bytes`, decoded to at least `px` pixels square. A new identity starts a
    /// cross-fade (`animate`) from the previous cover.
    pub fn get(
        &mut self,
        bytes: Option<&Arc<[u8]>>,
        px: u32,
        now: f64,
        animate: bool,
    ) -> Option<Arc<Image>> {
        let key = bytes.map_or(0, |b| Arc::as_ptr(b) as *const u8 as usize);
        let px = px.div_ceil(32).max(2) * 32;
        if key != self.key || (self.image.is_some() && px > self.px) {
            let changed = key != self.key;
            let old = self.image.take();
            self.image = bytes.and_then(|b| Image::decode(b, px, px)).map(Arc::new);
            self.px = px;
            self.key = key;
            if changed {
                self.prev = if animate { old } else { None };
                self.since = now;
            }
        }
        self.image.clone()
    }

    /// The outgoing cover and its opacity while the swap runs.
    pub fn outgoing(&self, now: f64) -> Option<(&Arc<Image>, f32)> {
        let prev = self.prev.as_ref()?;
        let t = ((now - self.since) / SWAP_SECS).clamp(0.0, 1.0);
        (t < 1.0).then(|| (prev, (1.0 - t).powi(3) as f32))
    }

    /// Opacity of the current cover.
    pub fn alpha(&self, now: f64) -> f32 {
        if self.prev.is_none() {
            return 1.0;
        }
        let t = ((now - self.since) / SWAP_SECS).clamp(0.0, 1.0);
        (1.0 - (1.0 - t).powi(3)) as f32
    }

    pub fn animating(&self, now: f64) -> bool {
        self.prev.is_some() && now - self.since < SWAP_SECS
    }

    pub fn settle(&mut self, now: f64) {
        if self.prev.is_some() && now - self.since >= SWAP_SECS {
            self.prev = None;
        }
    }

    /// Frees the decoded images.
    pub fn clear(&mut self) {
        *self = ArtCache::default();
    }
}
