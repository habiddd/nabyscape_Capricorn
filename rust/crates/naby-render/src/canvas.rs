//! CPU canvas over a tiny-skia pixmap: rectangles, paths, images, clips, blur and text blits.
//!
//! The pixel buffer is a single `Vec<u8>` that is reused across [`Canvas::resize`] calls, clip
//! masks come from a small pool, and nothing retains drawing commands (no scene graph).

use crate::image::Image;
use naby_core::color::Rgba;
use std::io;
use std::path::Path as FsPath;
use tiny_skia::{
    FillRule, FilterQuality, GradientStop, IntSize, LineCap, LineJoin, LinearGradient, Mask, Paint,
    Path, PathBuilder, Pattern, Pixmap, PixmapRef, Point, RadialGradient, Shader, SpreadMode,
    Stroke, Transform,
};

/// Axis-aligned rectangle in canvas pixels.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Rect { x, y, w, h }
    }
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    pub fn center(&self) -> (f32, f32) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
    pub fn is_empty(&self) -> bool {
        self.w <= 0.0 || self.h <= 0.0
    }
    /// Shrinks every side by `d` (grows when negative); never returns a negative size.
    pub fn inset(&self, d: f32) -> Rect {
        self.inset_ltrb(d, d, d, d)
    }
    pub fn inset_xy(&self, dx: f32, dy: f32) -> Rect {
        self.inset_ltrb(dx, dy, dx, dy)
    }
    pub fn inset_ltrb(&self, l: f32, t: f32, r: f32, b: f32) -> Rect {
        Rect::new(
            self.x + l,
            self.y + t,
            (self.w - l - r).max(0.0),
            (self.h - t - b).max(0.0),
        )
    }
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.w && y < self.y + self.h
    }
    pub fn contains_pt(&self, p: (f32, f32)) -> bool {
        self.contains(p.0, p.1)
    }
    pub fn translate(&self, dx: f32, dy: f32) -> Rect {
        Rect::new(self.x + dx, self.y + dy, self.w, self.h)
    }
    pub fn with_size(&self, w: f32, h: f32) -> Rect {
        Rect::new(self.x, self.y, w, h)
    }
    pub fn intersect(&self, o: &Rect) -> Option<Rect> {
        let l = self.x.max(o.x);
        let t = self.y.max(o.y);
        let r = self.right().min(o.right());
        let b = self.bottom().min(o.bottom());
        (r > l && b > t).then(|| Rect::new(l, t, r - l, b - t))
    }
    pub fn union(&self, o: &Rect) -> Rect {
        let l = self.x.min(o.x);
        let t = self.y.min(o.y);
        Rect::new(
            l,
            t,
            self.right().max(o.right()) - l,
            self.bottom().max(o.bottom()) - t,
        )
    }
    /// `(left of width w, remainder)`.
    pub fn split_left(&self, w: f32) -> (Rect, Rect) {
        let w = w.clamp(0.0, self.w);
        (
            Rect::new(self.x, self.y, w, self.h),
            Rect::new(self.x + w, self.y, self.w - w, self.h),
        )
    }
    /// `(remainder, right of width w)`.
    pub fn split_right(&self, w: f32) -> (Rect, Rect) {
        let w = w.clamp(0.0, self.w);
        (
            Rect::new(self.x, self.y, self.w - w, self.h),
            Rect::new(self.right() - w, self.y, w, self.h),
        )
    }
    /// `(top of height h, remainder)`.
    pub fn split_top(&self, h: f32) -> (Rect, Rect) {
        let h = h.clamp(0.0, self.h);
        (
            Rect::new(self.x, self.y, self.w, h),
            Rect::new(self.x, self.y + h, self.w, self.h - h),
        )
    }
    /// `(remainder, bottom of height h)`.
    pub fn split_bottom(&self, h: f32) -> (Rect, Rect) {
        let h = h.clamp(0.0, self.h);
        (
            Rect::new(self.x, self.y, self.w, self.h - h),
            Rect::new(self.x, self.bottom() - h, self.w, h),
        )
    }
    fn to_skia(self) -> Option<tiny_skia::Rect> {
        tiny_skia::Rect::from_xywh(self.x, self.y, self.w, self.h)
    }
}

/// How a shape is painted.
#[derive(Debug, Clone, PartialEq)]
pub enum Fill {
    Solid(Rgba),
    Linear {
        from: (f32, f32),
        to: (f32, f32),
        stops: Vec<(f32, Rgba)>,
    },
    Radial {
        center: (f32, f32),
        radius: f32,
        stops: Vec<(f32, Rgba)>,
    },
}

impl From<Rgba> for Fill {
    fn from(c: Rgba) -> Self {
        Fill::Solid(c)
    }
}

fn sk_color(c: Rgba, alpha: f32) -> tiny_skia::Color {
    let a = (c.a as f32 / 255.0 * alpha).clamp(0.0, 1.0);
    tiny_skia::Color::from_rgba(
        c.r as f32 / 255.0,
        c.g as f32 / 255.0,
        c.b as f32 / 255.0,
        a,
    )
    .unwrap_or(tiny_skia::Color::TRANSPARENT)
}

fn gradient_stops(stops: &[(f32, Rgba)], alpha: f32) -> Vec<GradientStop> {
    stops
        .iter()
        .map(|(p, c)| GradientStop::new(p.clamp(0.0, 1.0), sk_color(*c, alpha)))
        .collect()
}

fn make_paint(fill: &Fill, alpha: f32) -> Paint<'static> {
    let mut paint = Paint {
        anti_alias: true,
        ..Paint::default()
    };
    let solid = |c: Rgba| Shader::SolidColor(sk_color(c, alpha));
    paint.shader = match fill {
        Fill::Solid(c) => solid(*c),
        Fill::Linear { from, to, stops } => match stops.as_slice() {
            [] => solid(Rgba::TRANSPARENT),
            [(_, c)] => solid(*c),
            _ => LinearGradient::new(
                Point::from_xy(from.0, from.1),
                Point::from_xy(to.0, to.1),
                gradient_stops(stops, alpha),
                SpreadMode::Pad,
                Transform::identity(),
            )
            .unwrap_or_else(|| solid(stops[0].1)),
        },
        Fill::Radial {
            center,
            radius,
            stops,
        } => match stops.as_slice() {
            [] => solid(Rgba::TRANSPARENT),
            [(_, c)] => solid(*c),
            _ => RadialGradient::new(
                Point::from_xy(center.0, center.1),
                Point::from_xy(center.0, center.1),
                radius.max(0.01),
                gradient_stops(stops, alpha),
                SpreadMode::Pad,
                Transform::identity(),
            )
            .unwrap_or_else(|| solid(stops[0].1)),
        },
    };
    paint
}

const KAPPA: f32 = 0.552_284_8;

/// Rounded rectangle outline; the radius is clamped to half the shorter side.
pub fn round_rect_path(r: Rect, radius: f32) -> Option<Path> {
    if r.w <= 0.0 || r.h <= 0.0 {
        return None;
    }
    let rad = radius.clamp(0.0, r.w.min(r.h) / 2.0);
    let mut pb = PathBuilder::new();
    if rad < 0.25 {
        pb.push_rect(r.to_skia()?);
        return pb.finish();
    }
    let (x, y, right, bottom) = (r.x, r.y, r.right(), r.bottom());
    let c = rad * KAPPA;
    pb.move_to(x + rad, y);
    pb.line_to(right - rad, y);
    pb.cubic_to(right - rad + c, y, right, y + rad - c, right, y + rad);
    pb.line_to(right, bottom - rad);
    pb.cubic_to(
        right,
        bottom - rad + c,
        right - rad + c,
        bottom,
        right - rad,
        bottom,
    );
    pb.line_to(x + rad, bottom);
    pb.cubic_to(x + rad - c, bottom, x, bottom - rad + c, x, bottom - rad);
    pb.line_to(x, y + rad);
    pb.cubic_to(x, y + rad - c, x + rad - c, y, x + rad, y);
    pb.close();
    pb.finish()
}

pub fn circle_path(cx: f32, cy: f32, r: f32) -> Option<Path> {
    PathBuilder::from_circle(cx, cy, r.max(0.01))
}

/// Pixel surface. Pixels are stored premultiplied (tiny-skia layout, RGBA byte order).
pub struct Canvas {
    pixmap: Pixmap,
    clips: Vec<Mask>,
    mask_pool: Vec<Mask>,
    alpha: f32,
    scratch: Vec<u8>,
    /// Logical-to-physical scale applied to every drawing call (1.0 = pixels).
    scale: f32,
}

impl Canvas {
    pub fn new(w: u32, h: u32) -> Canvas {
        let (w, h) = (w.clamp(1, 16384), h.clamp(1, 16384));
        Canvas {
            pixmap: Pixmap::new(w, h).expect("non-zero size"),
            clips: Vec::new(),
            mask_pool: Vec::new(),
            alpha: 1.0,
            scratch: Vec::new(),
            scale: 1.0,
        }
    }

    /// The logical-to-physical scale (see [`Canvas::set_scale`]).
    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// Makes every drawing call take logical coordinates that are multiplied by `scale`
    /// (shapes, strokes, clips, images, blur and text sizes). `bounds()` then reports the
    /// logical size. Used by the DPI-aware widget toolkit; 1.0 is the default.
    pub fn set_scale(&mut self, scale: f32) {
        self.scale = if scale.is_finite() {
            scale.clamp(0.5, 8.0)
        } else {
            1.0
        };
    }

    fn scale_transform(&self, t: Transform) -> Transform {
        if self.scale == 1.0 {
            t
        } else {
            t.post_scale(self.scale, self.scale)
        }
    }

    pub fn width(&self) -> u32 {
        self.pixmap.width()
    }
    pub fn height(&self) -> u32 {
        self.pixmap.height()
    }
    pub fn size(&self) -> (u32, u32) {
        (self.width(), self.height())
    }
    /// The drawable area in logical units (equals the pixel size at scale 1).
    pub fn bounds(&self) -> Rect {
        Rect::new(
            0.0,
            0.0,
            self.width() as f32 / self.scale,
            self.height() as f32 / self.scale,
        )
    }
    /// Bytes currently held by the pixel buffer, masks and scratch space.
    pub fn memory_bytes(&self) -> usize {
        self.pixmap.data().len()
            + (self.clips.len() + self.mask_pool.len()) * (self.width() * self.height()) as usize
            + self.scratch.capacity()
    }
    /// Premultiplied RGBA bytes, row-major.
    pub fn data(&self) -> &[u8] {
        self.pixmap.data()
    }

    /// Changes the size, reusing the existing allocation. Contents become transparent.
    pub fn resize(&mut self, w: u32, h: u32) {
        let (w, h) = (w.clamp(1, 16384), h.clamp(1, 16384));
        if (w, h) == self.size() {
            return;
        }
        let old = std::mem::replace(&mut self.pixmap, Pixmap::new(1, 1).expect("1x1"));
        let mut buf = old.take();
        let need = w as usize * h as usize * 4;
        buf.clear();
        if buf.capacity() > need.saturating_mul(2) {
            buf.shrink_to(need + need / 4);
        }
        buf.resize(need, 0);
        self.pixmap =
            Pixmap::from_vec(buf, IntSize::from_wh(w, h).expect("non-zero")).expect("size");
        self.clips.clear();
        self.mask_pool.clear();
    }

    /// Drops the pixel buffer, masks and scratch memory (hidden surface). Resize to reuse.
    pub fn release(&mut self) {
        self.pixmap = Pixmap::new(1, 1).expect("1x1");
        self.clips = Vec::new();
        self.mask_pool = Vec::new();
        self.scratch = Vec::new();
    }

    pub fn clear(&mut self, c: Rgba) {
        self.pixmap.fill(sk_color(c, 1.0));
    }

    pub fn alpha(&self) -> f32 {
        self.alpha
    }

    /// Sets the opacity multiplier directly (prefer [`Canvas::with_opacity`]).
    pub fn set_alpha(&mut self, alpha: f32) {
        self.alpha = alpha.clamp(0.0, 1.0);
    }

    /// Runs `f` with every paint operation multiplied by `opacity` (applied per primitive).
    pub fn with_opacity<R>(&mut self, opacity: f32, f: impl FnOnce(&mut Canvas) -> R) -> R {
        let old = self.alpha;
        self.alpha = old * opacity.clamp(0.0, 1.0);
        let r = f(self);
        self.alpha = old;
        r
    }

    // ---- clipping ---------------------------------------------------------------------

    pub fn push_clip_round_rect(&mut self, r: Rect, radius: f32) {
        self.push_clip_path(round_rect_path(r, radius).as_ref());
    }

    /// Clips to an arbitrary path (`None` clips everything away). Pair with [`Canvas::pop_clip`].
    pub fn push_clip_path(&mut self, path: Option<&Path>) {
        let (w, h) = self.size();
        let mut mask = self
            .mask_pool
            .pop()
            .or_else(|| Mask::new(w, h))
            .expect("mask");
        let t = self.scale_transform(Transform::identity());
        match path {
            Some(path) => {
                if let Some(prev) = self.clips.last() {
                    mask.data_mut().copy_from_slice(prev.data());
                    mask.intersect_path(path, FillRule::Winding, true, t);
                } else {
                    mask.clear();
                    mask.fill_path(path, FillRule::Winding, true, t);
                }
            }
            None => mask.clear(),
        }
        self.clips.push(mask);
    }

    pub fn push_clip_rect(&mut self, r: Rect) {
        self.push_clip_round_rect(r, 0.0);
    }

    pub fn pop_clip(&mut self) {
        if let Some(m) = self.clips.pop() {
            if self.mask_pool.len() < 4 {
                self.mask_pool.push(m);
            }
        }
    }

    pub fn clip_depth(&self) -> usize {
        self.clips.len()
    }

    // ---- shapes -----------------------------------------------------------------------

    pub fn fill_rect(&mut self, r: Rect, c: Rgba) {
        self.fill_round_rect(r, 0.0, &Fill::Solid(c));
    }

    pub fn fill_rrect(&mut self, r: Rect, radius: f32, c: Rgba) {
        self.fill_round_rect(r, radius, &Fill::Solid(c));
    }

    pub fn fill_round_rect(&mut self, r: Rect, radius: f32, fill: &Fill) {
        if let Some(path) = round_rect_path(r, radius) {
            self.fill_path(&path, fill);
        }
    }

    /// Border drawn inside `r` (like a WPF `BorderThickness`).
    pub fn stroke_round_rect(&mut self, r: Rect, radius: f32, c: Rgba, width: f32) {
        let half = width / 2.0;
        let inner = r.inset(half);
        if let Some(path) = round_rect_path(inner, (radius - half).max(0.0)) {
            self.stroke_path(&path, &Fill::Solid(c), width, Transform::identity());
        }
    }

    pub fn fill_circle(&mut self, cx: f32, cy: f32, r: f32, fill: &Fill) {
        if let Some(p) = circle_path(cx, cy, r) {
            self.fill_path(&p, fill);
        }
    }

    pub fn stroke_circle(&mut self, cx: f32, cy: f32, r: f32, c: Rgba, width: f32) {
        if let Some(p) = circle_path(cx, cy, r) {
            self.stroke_path(&p, &Fill::Solid(c), width, Transform::identity());
        }
    }

    pub fn fill_path(&mut self, path: &Path, fill: &Fill) {
        self.fill_path_with(path, fill, FillRule::Winding, Transform::identity());
    }

    pub fn fill_path_with(&mut self, path: &Path, fill: &Fill, rule: FillRule, t: Transform) {
        let paint = make_paint(fill, self.alpha);
        let t = self.scale_transform(t);
        self.pixmap
            .fill_path(path, &paint, rule, t, self.clips.last());
    }

    /// Round-capped, round-joined stroke of `width` (in the transformed space).
    pub fn stroke_path(&mut self, path: &Path, fill: &Fill, width: f32, t: Transform) {
        let paint = make_paint(fill, self.alpha);
        let stroke = Stroke {
            width,
            line_cap: LineCap::Round,
            line_join: LineJoin::Round,
            ..Stroke::default()
        };
        let t = self.scale_transform(t);
        self.pixmap
            .stroke_path(path, &paint, &stroke, t, self.clips.last());
    }

    /// Draws `img` stretched into `dst` with rounded corners.
    pub fn draw_image(&mut self, img: &Image, dst: Rect, opacity: f32, radius: f32) {
        let Some(pm) = PixmapRef::from_bytes(img.data(), img.width(), img.height()) else {
            return;
        };
        if dst.is_empty() {
            return;
        }
        let k = self.scale;
        let dst = Rect::new(dst.x * k, dst.y * k, dst.w * k, dst.h * k);
        let radius = radius * k;
        let t = Transform::from_row(
            dst.w / img.width() as f32,
            0.0,
            0.0,
            dst.h / img.height() as f32,
            dst.x,
            dst.y,
        );
        let exact =
            (dst.w - img.width() as f32).abs() < 0.5 && (dst.h - img.height() as f32).abs() < 0.5;
        let quality = if exact {
            FilterQuality::Nearest
        } else {
            FilterQuality::Bilinear
        };
        let paint = Paint {
            shader: Pattern::new(
                pm,
                SpreadMode::Pad,
                quality,
                (opacity * self.alpha).clamp(0.0, 1.0),
                t,
            ),
            anti_alias: true,
            ..Paint::default()
        };
        if let Some(path) = round_rect_path(dst, radius) {
            self.pixmap.fill_path(
                &path,
                &paint,
                FillRule::Winding,
                Transform::identity(),
                self.clips.last(),
            );
        }
    }

    /// Box blur applied three times (a close Gaussian approximation) to `region`.
    /// Ignores clips and opacity; used for frosted panels behind content.
    pub fn blur(&mut self, region: Rect, radius: f32) {
        let k = self.scale;
        let region = Rect::new(region.x * k, region.y * k, region.w * k, region.h * k);
        let radius = radius * k;
        let each = (radius / 3.0)
            .round()
            .max(if radius >= 1.0 { 1.0 } else { 0.0 }) as usize;
        if each == 0 {
            return;
        }
        let x0 = region.x.floor().max(0.0) as usize;
        let y0 = region.y.floor().max(0.0) as usize;
        let x1 = (region.right().ceil().max(0.0) as usize).min(self.width() as usize);
        let y1 = (region.bottom().ceil().max(0.0) as usize).min(self.height() as usize);
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let stride = self.width() as usize * 4;
        let (w, h) = (x1 - x0, y1 - y0);
        let line = w.max(h) * 4;
        let mut scratch = std::mem::take(&mut self.scratch);
        scratch.clear();
        scratch.resize(line * 2, 0);
        let data = self.pixmap.data_mut();
        for _ in 0..3 {
            let (src, dst) = scratch.split_at_mut(line);
            for y in y0..y1 {
                let row = &mut data[y * stride + x0 * 4..y * stride + x1 * 4];
                src[..w * 4].copy_from_slice(row);
                blur_line(&src[..w * 4], row, each);
            }
            for x in x0..x1 {
                for (i, y) in (y0..y1).enumerate() {
                    let o = y * stride + x * 4;
                    src[i * 4..i * 4 + 4].copy_from_slice(&data[o..o + 4]);
                }
                blur_line(&src[..h * 4], &mut dst[..h * 4], each);
                for (i, y) in (y0..y1).enumerate() {
                    let o = y * stride + x * 4;
                    data[o..o + 4].copy_from_slice(&dst[i * 4..i * 4 + 4]);
                }
            }
        }
        self.scratch = scratch;
    }

    // ---- coverage blit (used by text) ---------------------------------------------------

    /// Composites an 8-bit coverage bitmap tinted with `c` at integer position (x, y).
    pub fn blit_coverage(&mut self, x: i32, y: i32, w: usize, h: usize, cov: &[u8], c: Rgba) {
        let cw = self.width() as i32;
        let ch = self.height() as i32;
        let base = (c.a as f32 * self.alpha).round() as u32;
        if base == 0 || cov.len() < w * h {
            return;
        }
        let mask = self.clips.last().map(|m| m.data());
        let data = self.pixmap.data_mut();
        for row in 0..h as i32 {
            let py = y + row;
            if py < 0 || py >= ch {
                continue;
            }
            for col in 0..w as i32 {
                let px = x + col;
                if px < 0 || px >= cw {
                    continue;
                }
                let cv = cov[row as usize * w + col as usize] as u32;
                if cv == 0 {
                    continue;
                }
                let mut sa = (cv * base + 127) / 255;
                if let Some(m) = mask {
                    sa = (sa * m[(py * cw + px) as usize] as u32 + 127) / 255;
                }
                if sa == 0 {
                    continue;
                }
                let o = (py * cw + px) as usize * 4;
                let inv = 255 - sa;
                let d = &mut data[o..o + 4];
                d[0] = ((c.r as u32 * sa + 127) / 255 + (d[0] as u32 * inv + 127) / 255).min(255)
                    as u8;
                d[1] = ((c.g as u32 * sa + 127) / 255 + (d[1] as u32 * inv + 127) / 255).min(255)
                    as u8;
                d[2] = ((c.b as u32 * sa + 127) / 255 + (d[2] as u32 * inv + 127) / 255).min(255)
                    as u8;
                d[3] = (sa + (d[3] as u32 * inv + 127) / 255).min(255) as u8;
            }
        }
    }

    // ---- output -----------------------------------------------------------------------

    /// Straight-alpha color of one pixel (test and snapshot helper).
    pub fn pixel(&self, x: u32, y: u32) -> Rgba {
        if x >= self.width() || y >= self.height() {
            return Rgba::TRANSPARENT;
        }
        let o = (y * self.width() + x) as usize * 4;
        let p = &self.pixmap.data()[o..o + 4];
        demultiply([p[0], p[1], p[2], p[3]])
    }

    /// Raw premultiplied RGBA of one pixel.
    pub fn pixel_premultiplied(&self, x: u32, y: u32) -> [u8; 4] {
        let o = (y * self.width() + x) as usize * 4;
        let p = &self.pixmap.data()[o..o + 4];
        [p[0], p[1], p[2], p[3]]
    }

    /// Fills `out` with BGRA premultiplied bytes, ready for a 32-bit top-down DIB passed to
    /// `UpdateLayeredWindow`. `out` is reused (no allocation when it is already large enough).
    pub fn write_bgra_premultiplied(&self, out: &mut Vec<u8>) {
        let src = self.pixmap.data();
        out.clear();
        out.reserve(src.len());
        out.extend(src.chunks_exact(4).flat_map(|p| [p[2], p[1], p[0], p[3]]));
    }

    /// Writes the canvas as a straight-alpha RGBA PNG.
    pub fn save_png(&self, path: &FsPath) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let file = io::BufWriter::new(std::fs::File::create(path)?);
        let mut enc = png::Encoder::new(file, self.width(), self.height());
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().map_err(io::Error::other)?;
        let mut straight = Vec::with_capacity(self.pixmap.data().len());
        for p in self.pixmap.data().chunks_exact(4) {
            let c = demultiply([p[0], p[1], p[2], p[3]]);
            straight.extend_from_slice(&[c.r, c.g, c.b, c.a]);
        }
        writer.write_image_data(&straight).map_err(io::Error::other)
    }
}

fn demultiply(p: [u8; 4]) -> Rgba {
    let a = p[3] as u32;
    if a == 0 {
        return Rgba::TRANSPARENT;
    }
    let up = |v: u8| ((v as u32 * 255 + a / 2) / a).min(255) as u8;
    Rgba {
        r: up(p[0]),
        g: up(p[1]),
        b: up(p[2]),
        a: p[3],
    }
}

/// One box-blur pass over a line of RGBA pixels (window `2r+1`, edges clamped).
fn blur_line(src: &[u8], dst: &mut [u8], r: usize) {
    let n = src.len() / 4;
    if n == 0 {
        return;
    }
    let win = (2 * r + 1) as u32;
    let at = |i: isize| -> usize { i.clamp(0, n as isize - 1) as usize * 4 };
    let mut sum = [0u32; 4];
    for k in -(r as isize)..=(r as isize) {
        let o = at(k);
        for c in 0..4 {
            sum[c] += src[o + c] as u32;
        }
    }
    for i in 0..n {
        for c in 0..4 {
            dst[i * 4 + c] = ((sum[c] + win / 2) / win) as u8;
        }
        let add = at(i as isize + r as isize + 1);
        let sub = at(i as isize - r as isize);
        for c in 0..4 {
            sum[c] = sum[c] + src[add + c] as u32 - src[sub + c] as u32;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_rect_paints_inside_only() {
        let mut c = Canvas::new(40, 40);
        c.clear(Rgba::TRANSPARENT);
        c.fill_rrect(Rect::new(10.0, 10.0, 20.0, 20.0), 8.0, Rgba::rgb(255, 0, 0));
        assert_eq!(c.pixel(20, 20), Rgba::rgb(255, 0, 0));
        assert_eq!(c.pixel(5, 5).a, 0);
        assert_eq!(c.pixel(35, 20).a, 0);
        // The corner pixel of a radius-8 rounded rect is outside the curve.
        assert_eq!(c.pixel(10, 10).a, 0);
        assert!(c.pixel(20, 10).a > 200);
    }

    #[test]
    fn bgra_conversion_is_premultiplied() {
        let mut c = Canvas::new(2, 2);
        c.clear(Rgba {
            r: 255,
            g: 0,
            b: 0,
            a: 128,
        });
        let mut out = Vec::new();
        c.write_bgra_premultiplied(&mut out);
        assert_eq!(out.len(), 16);
        assert_eq!(&out[0..4], &[0, 0, 128, 128]);
    }

    #[test]
    fn resize_reuses_allocation_and_clears() {
        let mut c = Canvas::new(100, 100);
        c.clear(Rgba::WHITE);
        let ptr = c.data().as_ptr();
        c.resize(80, 90);
        assert_eq!(c.size(), (80, 90));
        assert_eq!(c.data().as_ptr(), ptr);
        assert_eq!(c.pixel(0, 0).a, 0);
        c.release();
        assert_eq!(c.size(), (1, 1));
        c.resize(10, 10);
        assert_eq!(c.data().len(), 400);
    }

    #[test]
    fn clip_limits_painting_and_nests() {
        let mut c = Canvas::new(40, 40);
        c.push_clip_rect(Rect::new(0.0, 0.0, 20.0, 40.0));
        c.push_clip_rect(Rect::new(0.0, 0.0, 40.0, 20.0));
        c.fill_rect(Rect::new(0.0, 0.0, 40.0, 40.0), Rgba::WHITE);
        c.pop_clip();
        c.pop_clip();
        assert_eq!(c.pixel(10, 10), Rgba::WHITE);
        assert_eq!(c.pixel(30, 10).a, 0);
        assert_eq!(c.pixel(10, 30).a, 0);
        assert_eq!(c.clip_depth(), 0);
    }

    #[test]
    fn opacity_scales_alpha() {
        let mut c = Canvas::new(4, 4);
        c.with_opacity(0.5, |c| {
            c.fill_rect(Rect::new(0.0, 0.0, 4.0, 4.0), Rgba::WHITE)
        });
        let a = c.pixel(1, 1).a;
        assert!((126..=129).contains(&a), "{a}");
        assert_eq!(c.alpha(), 1.0);
    }

    #[test]
    fn gradient_fill_varies() {
        let mut c = Canvas::new(100, 4);
        c.fill_round_rect(
            Rect::new(0.0, 0.0, 100.0, 4.0),
            0.0,
            &Fill::Linear {
                from: (0.0, 0.0),
                to: (100.0, 0.0),
                stops: vec![(0.0, Rgba::BLACK), (1.0, Rgba::WHITE)],
            },
        );
        assert!(c.pixel(90, 1).r > c.pixel(10, 1).r + 100);
    }

    #[test]
    fn blur_spreads_and_preserves_uniform() {
        let mut c = Canvas::new(30, 30);
        c.fill_rect(Rect::new(14.0, 14.0, 2.0, 2.0), Rgba::WHITE);
        let before = c.pixel(10, 15).a;
        c.blur(Rect::new(0.0, 0.0, 30.0, 30.0), 6.0);
        assert!(c.pixel(10, 15).a > before);
        assert!(c.pixel(15, 15).a < 255);
        let mut u = Canvas::new(10, 10);
        u.clear(Rgba::rgb(10, 20, 30));
        u.blur(Rect::new(0.0, 0.0, 10.0, 10.0), 6.0);
        assert_eq!(u.pixel(5, 5), Rgba::rgb(10, 20, 30));
    }

    #[test]
    fn coverage_blit_respects_clip() {
        let mut c = Canvas::new(8, 8);
        c.push_clip_rect(Rect::new(0.0, 0.0, 4.0, 8.0));
        c.blit_coverage(2, 2, 4, 1, &[255; 4], Rgba::WHITE);
        c.pop_clip();
        assert_eq!(c.pixel(3, 2), Rgba::WHITE);
        assert_eq!(c.pixel(4, 2).a, 0);
    }

    #[test]
    fn rect_helpers() {
        let r = Rect::new(0.0, 0.0, 100.0, 50.0);
        let (a, b) = r.split_left(30.0);
        assert_eq!((a.w, b.x, b.w), (30.0, 30.0, 70.0));
        assert_eq!(r.inset(60.0).w, 0.0);
        assert!(r.contains(0.0, 0.0) && !r.contains(100.0, 10.0));
        assert_eq!(
            r.intersect(&Rect::new(90.0, 40.0, 50.0, 50.0)).unwrap().w,
            10.0
        );
    }
}
