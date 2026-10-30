//! Test helpers: PNG snapshots under `target/snapshots/` and a system font lookup that lets
//! font-dependent tests skip cleanly on machines without fonts.

use crate::canvas::{Canvas, Rect};
use crate::text::Fonts;
use naby_core::color::Rgba;
use std::path::PathBuf;

/// `<workspace>/target/snapshots`.
pub fn snapshot_dir() -> PathBuf {
    let base = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target"));
    base.join("snapshots")
}

/// Writes `canvas` to `target/snapshots/<name>.png`; failures are ignored (read-only checkouts).
pub fn save(name: &str, canvas: &Canvas) -> PathBuf {
    let path = snapshot_dir().join(format!("{name}.png"));
    let _ = canvas.save_png(&path);
    path
}

/// System fonts, or `None` (after printing a skip note) when none can be found.
pub fn test_fonts() -> Option<Fonts> {
    let f = Fonts::load_system();
    if f.has_faces() {
        Some(f)
    } else {
        println!("skip: no system font found");
        None
    }
}

/// Number of pixels in `region` whose alpha is above `min_alpha`.
pub fn count_ink(canvas: &Canvas, region: Rect, min_alpha: u8) -> usize {
    count_where(canvas, region, |p| p.a > min_alpha)
}

/// Number of pixels in `region` that differ from `background`.
pub fn count_differing(canvas: &Canvas, region: Rect, background: Rgba) -> usize {
    count_where(canvas, region, |p| p != background)
}

fn count_where(canvas: &Canvas, region: Rect, f: impl Fn(Rgba) -> bool) -> usize {
    let mut n = 0;
    let x1 = (region.right().ceil().max(0.0) as u32).min(canvas.width());
    let y1 = (region.bottom().ceil().max(0.0) as u32).min(canvas.height());
    for y in region.y.max(0.0) as u32..y1 {
        for x in region.x.max(0.0) as u32..x1 {
            if f(canvas.pixel(x, y)) {
                n += 1;
            }
        }
    }
    n
}
