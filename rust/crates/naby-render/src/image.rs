//! Image decoding straight to display size, and a byte-bounded LRU of decoded thumbnails.
//!
//! PNG rows are streamed into the box downscaler, so the full-size bitmap is never resident.
//! JPEG is decoded in full by `zune-jpeg` (transient) and then downscaled; callers keep only
//! the display-sized [`Image`].

use std::collections::HashMap;
use std::io::Cursor;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Largest source image (in pixels) the decoder accepts, against decompression bombs.
const MAX_SOURCE_PIXELS: u64 = 100_000_000;

/// Premultiplied RGBA8 bitmap.
#[derive(Debug, Clone, PartialEq)]
pub struct Image {
    w: u32,
    h: u32,
    data: Vec<u8>,
}

impl Image {
    /// Builds an image from straight-alpha RGBA bytes (premultiplies them).
    pub fn from_rgba(w: u32, h: u32, mut rgba: Vec<u8>) -> Option<Image> {
        if w == 0 || h == 0 || rgba.len() != w as usize * h as usize * 4 {
            return None;
        }
        for p in rgba.chunks_exact_mut(4) {
            premultiply(p);
        }
        Some(Image { w, h, data: rgba })
    }

    pub fn width(&self) -> u32 {
        self.w
    }
    pub fn height(&self) -> u32 {
        self.h
    }
    /// Premultiplied RGBA bytes.
    pub fn data(&self) -> &[u8] {
        &self.data
    }
    pub fn byte_len(&self) -> usize {
        self.data.len()
    }
    /// Straight-alpha RGBA of one pixel.
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let o = (y * self.w + x) as usize * 4;
        let p = &self.data[o..o + 4];
        let a = p[3] as u32;
        if a == 0 {
            return [0; 4];
        }
        let up = |v: u8| ((v as u32 * 255 + a / 2) / a).min(255) as u8;
        [up(p[0]), up(p[1]), up(p[2]), p[3]]
    }

    /// Size that fits `src` inside `max_w` x `max_h` keeping the aspect ratio (never upscales).
    pub fn fit_size(src_w: u32, src_h: u32, max_w: u32, max_h: u32) -> (u32, u32) {
        if src_w == 0 || src_h == 0 {
            return (0, 0);
        }
        let (max_w, max_h) = (max_w.max(1), max_h.max(1));
        if src_w <= max_w && src_h <= max_h {
            return (src_w, src_h);
        }
        let scale = (max_w as f64 / src_w as f64).min(max_h as f64 / src_h as f64);
        (
            ((src_w as f64 * scale).round() as u32).max(1),
            ((src_h as f64 * scale).round() as u32).max(1),
        )
    }

    /// Decodes PNG or JPEG bytes, downscaling (box filter) to fit `max_w` x `max_h`.
    pub fn decode(bytes: &[u8], max_w: u32, max_h: u32) -> Option<Image> {
        if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
            decode_png(bytes, max_w, max_h)
        } else if bytes.starts_with(&[0xFF, 0xD8]) {
            decode_jpeg(bytes, max_w, max_h)
        } else {
            None
        }
    }

    /// Reads just the pixel size of a PNG or JPEG without decoding pixels.
    pub fn probe_size(bytes: &[u8]) -> Option<(u32, u32)> {
        if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
            let r = png::Decoder::new(Cursor::new(bytes)).read_info().ok()?;
            let i = r.info();
            Some((i.width, i.height))
        } else if bytes.starts_with(&[0xFF, 0xD8]) {
            let mut d = zune_jpeg::JpegDecoder::new(bytes);
            d.decode_headers().ok()?;
            let i = d.info()?;
            Some((i.width as u32, i.height as u32))
        } else {
            None
        }
    }
}

fn premultiply(p: &mut [u8]) {
    let a = p[3] as u32;
    if a != 255 {
        for c in &mut p[..3] {
            *c = ((*c as u32 * a + 127) / 255) as u8;
        }
    }
}

/// Streaming box downscaler accumulating premultiplied RGBA.
struct Downscaler {
    sw: u32,
    sh: u32,
    dw: u32,
    dh: u32,
    acc: Vec<u32>,
    cnt: Vec<u32>,
    col_map: Vec<u32>,
    col_count: Vec<u32>,
    row_count: Vec<u32>,
    y: u32,
}

impl Downscaler {
    fn new(sw: u32, sh: u32, dw: u32, dh: u32) -> Self {
        let col_map: Vec<u32> = (0..sw)
            .map(|x| ((x as u64 * dw as u64) / sw as u64) as u32)
            .collect();
        let mut col_count = vec![0u32; dw as usize];
        for &d in &col_map {
            col_count[d as usize] += 1;
        }
        let mut row_count = vec![0u32; dh as usize];
        for y in 0..sh {
            row_count[((y as u64 * dh as u64) / sh as u64) as usize] += 1;
        }
        Downscaler {
            sw,
            sh,
            dw,
            dh,
            acc: vec![0; dw as usize * dh as usize * 4],
            cnt: Vec::new(),
            col_map,
            col_count,
            row_count,
            y: 0,
        }
    }

    /// Adds one source row of straight RGBA8 pixels.
    fn push_row(&mut self, row: &[[u8; 4]]) {
        if self.y >= self.sh {
            return;
        }
        let dy = ((self.y as u64 * self.dh as u64) / self.sh as u64) as usize;
        let base = dy * self.dw as usize * 4;
        for (x, px) in row.iter().take(self.sw as usize).enumerate() {
            let a = px[3] as u32;
            let d = base + self.col_map[x] as usize * 4;
            self.acc[d] += (px[0] as u32 * a + 127) / 255;
            self.acc[d + 1] += (px[1] as u32 * a + 127) / 255;
            self.acc[d + 2] += (px[2] as u32 * a + 127) / 255;
            self.acc[d + 3] += a;
        }
        self.y += 1;
    }

    fn finish(mut self) -> Image {
        self.cnt.clear();
        let mut data = Vec::with_capacity(self.acc.len());
        for dy in 0..self.dh as usize {
            for dx in 0..self.dw as usize {
                let n = (self.col_count[dx] * self.row_count[dy]).max(1);
                let o = (dy * self.dw as usize + dx) * 4;
                for c in 0..4 {
                    data.push(((self.acc[o + c] + n / 2) / n).min(255) as u8);
                }
            }
        }
        Image {
            w: self.dw,
            h: self.dh,
            data,
        }
    }
}

fn decode_png(bytes: &[u8], max_w: u32, max_h: u32) -> Option<Image> {
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let (sw, sh) = (reader.info().width, reader.info().height);
    if sw == 0 || sh == 0 || (sw as u64 * sh as u64) > MAX_SOURCE_PIXELS {
        return None;
    }
    let (dw, dh) = Image::fit_size(sw, sh, max_w, max_h);
    let mut ds = Downscaler::new(sw, sh, dw, dh);
    let (color, _) = reader.output_color_type();
    let mut row_px: Vec<[u8; 4]> = vec![[0; 4]; sw as usize];
    while let Some(row) = reader.next_row().ok()? {
        let data = row.data();
        match color {
            png::ColorType::Rgba => {
                for (o, c) in row_px.iter_mut().zip(data.chunks_exact(4)) {
                    *o = [c[0], c[1], c[2], c[3]];
                }
            }
            png::ColorType::Rgb => {
                for (o, c) in row_px.iter_mut().zip(data.chunks_exact(3)) {
                    *o = [c[0], c[1], c[2], 255];
                }
            }
            png::ColorType::GrayscaleAlpha => {
                for (o, c) in row_px.iter_mut().zip(data.chunks_exact(2)) {
                    *o = [c[0], c[0], c[0], c[1]];
                }
            }
            png::ColorType::Grayscale => {
                for (o, c) in row_px.iter_mut().zip(data.iter()) {
                    *o = [*c, *c, *c, 255];
                }
            }
            png::ColorType::Indexed => return None,
        }
        ds.push_row(&row_px);
    }
    if ds.y != sh {
        return None;
    }
    Some(ds.finish())
}

fn decode_jpeg(bytes: &[u8], max_w: u32, max_h: u32) -> Option<Image> {
    use zune_jpeg::zune_core::colorspace::ColorSpace;
    use zune_jpeg::zune_core::options::DecoderOptions;
    let options = DecoderOptions::default()
        .jpeg_set_out_colorspace(ColorSpace::RGB)
        .set_max_width(16_384)
        .set_max_height(16_384);
    let mut decoder = zune_jpeg::JpegDecoder::new_with_options(bytes, options);
    decoder.decode_headers().ok()?;
    let info = decoder.info()?;
    let (sw, sh) = (info.width as u32, info.height as u32);
    if sw == 0 || sh == 0 || (sw as u64 * sh as u64) > MAX_SOURCE_PIXELS {
        return None;
    }
    let pixels = decoder.decode().ok()?;
    if pixels.len() < sw as usize * sh as usize * 3 {
        return None;
    }
    let (dw, dh) = Image::fit_size(sw, sh, max_w, max_h);
    let mut ds = Downscaler::new(sw, sh, dw, dh);
    let mut row_px: Vec<[u8; 4]> = vec![[0, 0, 0, 255]; sw as usize];
    for row in pixels.chunks_exact(sw as usize * 3).take(sh as usize) {
        for (o, c) in row_px.iter_mut().zip(row.chunks_exact(3)) {
            *o = [c[0], c[1], c[2], 255];
        }
        ds.push_row(&row_px);
    }
    Some(ds.finish())
}

/// Pixel size to decode a tile at (`ViewportThumbnails`): the tile width in device pixels,
/// rounded up to a multiple of 32 and clamped to 64..=768.
pub fn thumbnail_pixels(tile_width: f32, dpi_scale: f32) -> u32 {
    ((tile_width.max(48.0) * dpi_scale / 32.0).ceil() * 32.0).clamp(64.0, 768.0) as u32
}

struct CacheEntry {
    image: Arc<Image>,
    bytes: usize,
    seq: u64,
    used_at: Instant,
}

/// LRU of display thumbnails bounded by total pixel bytes and entry count (64, like the WPF
/// build). Images larger than the whole budget are returned but never cached.
pub struct ThumbnailCache {
    entries: HashMap<String, CacheEntry>,
    budget: usize,
    bytes: usize,
    seq: u64,
}

impl ThumbnailCache {
    const MAX_ENTRIES: usize = 64;

    pub fn new(max_bytes: usize) -> Self {
        ThumbnailCache {
            entries: HashMap::new(),
            budget: max_bytes,
            bytes: 0,
            seq: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn get(&mut self, key: &str) -> Option<Arc<Image>> {
        self.seq += 1;
        let seq = self.seq;
        let e = self.entries.get_mut(key)?;
        e.seq = seq;
        e.used_at = Instant::now();
        Some(e.image.clone())
    }

    pub fn get_or_insert_with(
        &mut self,
        key: &str,
        make: impl FnOnce() -> Option<Image>,
    ) -> Option<Arc<Image>> {
        if let Some(hit) = self.get(key) {
            return Some(hit);
        }
        let image = Arc::new(make()?);
        self.insert(key, image.clone());
        Some(image)
    }

    pub fn insert(&mut self, key: &str, image: Arc<Image>) {
        if self.entries.contains_key(key) {
            self.get(key);
            return;
        }
        let size = image.byte_len();
        if size > self.budget {
            return;
        }
        while self.entries.len() >= Self::MAX_ENTRIES || self.bytes + size > self.budget {
            if !self.remove_oldest() {
                break;
            }
        }
        self.seq += 1;
        self.bytes += size;
        self.entries.insert(
            key.to_owned(),
            CacheEntry {
                image,
                bytes: size,
                seq: self.seq,
                used_at: Instant::now(),
            },
        );
    }

    fn remove_oldest(&mut self) -> bool {
        let Some(key) = self
            .entries
            .iter()
            .min_by_key(|(_, e)| e.seq)
            .map(|(k, _)| k.clone())
        else {
            return false;
        };
        if let Some(e) = self.entries.remove(&key) {
            self.bytes -= e.bytes;
        }
        true
    }

    /// Drops entries not used for `idle`. Only cache references expire; images handed out
    /// earlier stay valid.
    pub fn trim_idle(&mut self, idle: Duration) {
        let now = Instant::now();
        let stale: Vec<String> = self
            .entries
            .iter()
            .filter(|(_, e)| now.duration_since(e.used_at) >= idle)
            .map(|(k, _)| k.clone())
            .collect();
        for k in stale {
            if let Some(e) = self.entries.remove(&k) {
                self.bytes -= e.bytes;
            }
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn make_png(w: u32, h: u32, color: [u8; 4]) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, w, h);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            let mut wr = enc.write_header().unwrap();
            let mut data = Vec::new();
            for _ in 0..w * h {
                data.extend_from_slice(&color);
            }
            wr.write_image_data(&data).unwrap();
        }
        out
    }

    #[test]
    fn jpeg_png_decode_downscales() {
        let bytes = make_png(200, 100, [10, 200, 30, 255]);
        let img = Image::decode(&bytes, 50, 50).unwrap();
        assert_eq!((img.width(), img.height()), (50, 25));
        assert_eq!(img.pixel(10, 10), [10, 200, 30, 255]);
        assert_eq!(Image::probe_size(&bytes), Some((200, 100)));
        // Never upscales.
        let small = Image::decode(&bytes, 1000, 1000).unwrap();
        assert_eq!((small.width(), small.height()), (200, 100));
        // Garbage is rejected.
        assert!(Image::decode(b"not an image", 10, 10).is_none());
        assert!(Image::decode(&bytes[..40], 10, 10).is_none());
    }

    #[test]
    fn downscale_averages_halves() {
        // Left half black, right half white, 4x1 -> 2x1... fits 2 wide.
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, 4, 1);
            enc.set_color(png::ColorType::Rgb);
            enc.set_depth(png::BitDepth::Eight);
            let mut wr = enc.write_header().unwrap();
            wr.write_image_data(&[0, 0, 0, 0, 0, 0, 255, 255, 255, 255, 255, 255])
                .unwrap();
        }
        let img = Image::decode(&out, 1, 1).unwrap();
        assert_eq!((img.width(), img.height()), (1, 1));
        let p = img.pixel(0, 0);
        assert!((126..=129).contains(&p[0]), "{p:?}");
    }

    #[test]
    fn jpeg_decodes() {
        // 8x8 solid-ish JPEG produced by a reference encoder.
        const JPEG: &[u8] = include_bytes!("test_8x8.jpg");
        let img = Image::decode(JPEG, 4, 4).unwrap();
        assert_eq!((img.width(), img.height()), (4, 4));
        let p = img.pixel(1, 1);
        assert!(p[0] > 200 && p[1] < 60 && p[2] < 60, "{p:?}");
        assert_eq!(Image::probe_size(JPEG), Some((8, 8)));
    }

    #[test]
    fn cache_evicts_by_bytes() {
        let img = |n: u32| Arc::new(Image::from_rgba(n, 1, vec![255; n as usize * 4]).unwrap());
        let mut c = ThumbnailCache::new(100);
        c.insert("a", img(10)); // 40 bytes
        c.insert("b", img(10));
        assert_eq!((c.len(), c.bytes()), (2, 80));
        c.get("a"); // b is now oldest
        c.insert("c", img(10));
        assert!(c.get("b").is_none());
        assert!(c.get("a").is_some() && c.get("c").is_some());
        assert!(c.bytes() <= 100);
        // Too big for the whole budget: returned, not cached.
        let big = c.get_or_insert_with("big", || Image::from_rgba(100, 1, vec![0; 400]));
        assert!(big.is_some());
        assert!(c.get("big").is_none());
        // Producer is not called on a hit.
        let hit = c.get_or_insert_with("a", || panic!("should be cached"));
        assert!(hit.is_some());
        c.trim_idle(Duration::ZERO);
        assert!(c.is_empty() && c.bytes() == 0);
    }

    #[test]
    fn thumbnail_pixels_follow_tile_width() {
        assert_eq!(thumbnail_pixels(10.0, 1.0), 64);
        assert_eq!(thumbnail_pixels(150.0, 1.0), 160);
        assert_eq!(thumbnail_pixels(150.0, 1.5), 256);
        assert_eq!(thumbnail_pixels(5000.0, 2.0), 768);
    }

    #[test]
    fn cache_caps_entry_count() {
        let mut c = ThumbnailCache::new(1 << 20);
        for i in 0..100 {
            c.insert(
                &i.to_string(),
                Arc::new(Image::from_rgba(1, 1, vec![0; 4]).unwrap()),
            );
        }
        assert_eq!(c.len(), 64);
    }
}
