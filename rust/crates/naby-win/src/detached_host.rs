//! Host of the detached preview windows (C# `DetachedPreviewManager` / `DetachedPreviewWindow`).
//!
//! Each preview is a small always-on-top layered window driven by a platform-free
//! [`DetachedView`](naby_ui::overlay::detached::DetachedView). The host owns the windows, applies
//! the view's move / resize / close requests, keeps the window inside its monitor's work area and
//! persists the geometry in a `detached-<id>` document of the store.
//!
//! The rectangle rules (DIP <-> pixel conversion, placing a window under the pointer, clamping)
//! are plain functions tested on every platform.

use naby_ui::overlay::detached::{clamp_geometry, DetachedGeometry, DEFAULT_SIZE, MIN_SIZE};

/// Store document of the geometry of the preview of `kind`.
pub fn document_name(kind: &str) -> String {
    format!("detached-{kind}")
}

/// A pixel rectangle `(x, y, width, height)`.
pub type PxRect = (i32, i32, u32, u32);

/// The stored DIP rectangle in pixels of a monitor scaled by `scale`.
pub fn geometry_to_px(g: &DetachedGeometry, scale: f64) -> PxRect {
    let (left, top, width, height) = g.sanitized();
    (
        (left * scale).round() as i32,
        (top * scale).round() as i32,
        (width * scale).round().max(1.0) as u32,
        (height * scale).round().max(1.0) as u32,
    )
}

/// The pixel rectangle as the DIP geometry to store.
pub fn px_to_geometry(rect: PxRect, scale: f64) -> DetachedGeometry {
    let scale = if scale > 0.0 { scale } else { 1.0 };
    DetachedGeometry {
        left: f64::from(rect.0) / scale,
        top: f64::from(rect.1) / scale,
        width: f64::from(rect.2) / scale,
        height: f64::from(rect.3) / scale,
    }
}

/// Keeps `rect` inside `work` (`DetachedPreviewGeometry.Clamp`, with the minimum size scaled).
pub fn clamp_px(rect: PxRect, work: (i32, i32, i32, i32), scale: f64) -> PxRect {
    let to_f = |r: PxRect| {
        (
            f64::from(r.0),
            f64::from(r.1),
            f64::from(r.2),
            f64::from(r.3),
        )
    };
    let work_f = (
        f64::from(work.0),
        f64::from(work.1),
        f64::from(work.2.max(1)),
        f64::from(work.3.max(1)),
    );
    // `clamp_geometry` works in the units of its arguments; scale the DIP minimum by passing the
    // candidate in pixels and the work area in pixels with a minimum already in pixels.
    let min = (MIN_SIZE.0 * scale, MIN_SIZE.1 * scale);
    let (x, y, mut w, mut h) = to_f(rect);
    w = w.max(min.0.min(work_f.2));
    h = h.max(min.1.min(work_f.3));
    let (x, y, w, h) = clamp_geometry((x, y, w, h), work_f);
    (
        x.round() as i32,
        y.round() as i32,
        w.round().max(1.0) as u32,
        h.round().max(1.0) as u32,
    )
}

/// `PlaceAt`: a window of `size` pixels with the pointer over its empty title area (48 and
/// 22 DIP from the top-left corner), clamped into `work`.
pub fn place_at(
    anchor: (i32, i32),
    size: (u32, u32),
    scale: f64,
    work: (i32, i32, i32, i32),
) -> PxRect {
    clamp_px(
        (
            anchor.0 - (48.0 * scale).round() as i32,
            anchor.1 - (22.0 * scale).round() as i32,
            size.0,
            size.1,
        ),
        work,
        scale,
    )
}

/// The default size in pixels.
pub fn default_size_px(scale: f64) -> (u32, u32) {
    (
        (DEFAULT_SIZE.0 * scale).round() as u32,
        (DEFAULT_SIZE.1 * scale).round() as u32,
    )
}

#[cfg(windows)]
pub use os::DetachedHost;

#[cfg(windows)]
mod os {
    use super::*;
    use crate::event::HostEvent;
    use crate::window::LayeredWindow;
    use naby_core::command::Command;
    use naby_core::store::Store;
    use naby_render::{Canvas, Fonts, Rgba};
    use naby_ui::host::{Demand, Signal, Snapshots, View};
    use naby_ui::overlay::detached::{can_detach, normalize_kind, DetachedRequest, DetachedView};
    use std::sync::mpsc::{channel, Receiver};
    use std::sync::Arc;

    struct Entry {
        kind: String,
        view: DetachedView,
        window: LayeredWindow,
        events: Receiver<HostEvent>,
        canvas: Canvas,
        origin: (i32, i32),
        size: (u32, u32),
        shown: bool,
        paint: bool,
    }

    impl Entry {
        fn scale(&self) -> f64 {
            f64::from(self.window.dpi()) / 96.0
        }

        fn rect(&self) -> PxRect {
            (self.origin.0, self.origin.1, self.size.0, self.size.1)
        }
    }

    /// Owns every open detached preview.
    pub struct DetachedHost {
        store: Arc<Store>,
        entries: Vec<Entry>,
        fonts: Option<Fonts>,
        hidden: bool,
    }

    fn work_of(snaps: &Snapshots) -> (i32, i32, i32, i32) {
        let w = snaps.display.work;
        (w.x, w.y, w.w, w.h)
    }

    impl DetachedHost {
        pub fn new(store: Arc<Store>) -> DetachedHost {
            DetachedHost {
                store,
                entries: Vec::new(),
                fonts: None,
                hidden: false,
            }
        }

        pub fn is_empty(&self) -> bool {
            self.entries.is_empty()
        }

        pub fn is_open(&self, kind: &str) -> bool {
            let kind = normalize_kind(kind);
            self.entries.iter().any(|e| e.kind == kind)
        }

        pub fn open_kinds(&self) -> Vec<String> {
            self.entries.iter().map(|e| e.kind.clone()).collect()
        }

        /// Opens (or raises) the preview of `kind` with the pointer at `at` (screen pixels).
        /// False when the page cannot be detached right now.
        pub fn open(&mut self, kind: &str, at: Option<(i32, i32)>, snaps: &Snapshots) -> bool {
            let kind = normalize_kind(kind);
            if !can_detach(&snaps.settings, &kind) {
                return false;
            }
            let work = work_of(snaps);
            let scale = f64::from(snaps.display.scale.max(0.5));
            if self.hidden {
                return true;
            }
            if let Some(i) = self.entries.iter().position(|e| e.kind == kind) {
                let e = &mut self.entries[i];
                if let Some(at) = at {
                    let rect = place_at(at, e.size, scale, work);
                    Self::move_entry(e, rect, work, scale);
                }
                e.paint = true;
                return true;
            }
            let stored = self
                .store
                .load::<DetachedGeometry>(&document_name(&kind))
                .ok()
                .flatten();
            let rect = match (&stored, at) {
                (Some(g), Some(at)) => {
                    let size = geometry_to_px(g, scale);
                    place_at(at, (size.2, size.3), scale, work)
                }
                (Some(g), None) => clamp_px(geometry_to_px(g, scale), work, scale),
                (None, at) => {
                    let anchor =
                        at.unwrap_or((work.0 + work.2 / 2, work.1 + (64.0 * scale).round() as i32));
                    place_at(anchor, default_size_px(scale), scale, work)
                }
            };
            let (tx, events) = channel();
            let Ok(window) = LayeredWindow::new(tx) else {
                return false;
            };
            let mut view = DetachedView::new(&kind, scale as f32, (rect.2, rect.3), snaps);
            view.set_origin((rect.0, rect.1));
            if self.fonts.is_none() {
                self.fonts = Some(Fonts::load_system());
            }
            self.entries.push(Entry {
                kind,
                view,
                window,
                events,
                canvas: Canvas::new(1, 1),
                origin: (rect.0, rect.1),
                size: (rect.2, rect.3),
                shown: false,
                paint: true,
            });
            true
        }

        fn move_entry(e: &mut Entry, rect: PxRect, _work: (i32, i32, i32, i32), _scale: f64) {
            e.origin = (rect.0, rect.1);
            e.size = (rect.2, rect.3);
            e.view.set_origin(e.origin);
            e.view.set_size(e.size);
            e.window.move_to(e.origin.0, e.origin.1);
        }

        fn save(&self, e: &Entry) {
            let geometry = px_to_geometry(e.rect(), e.scale());
            if let Err(error) = self.store.save(&document_name(&e.kind), &geometry) {
                self.store.log(&error);
            }
        }

        /// New preferences: windows whose page can no longer be detached close.
        pub fn apply_settings(&mut self, snaps: &Snapshots) {
            let gone: Vec<usize> = self
                .entries
                .iter()
                .enumerate()
                .filter(|(_, e)| !can_detach(&snaps.settings, &e.kind))
                .map(|(i, _)| i)
                .collect();
            for i in gone.into_iter().rev() {
                self.close(i);
            }
            for e in &mut self.entries {
                e.paint = true;
            }
        }

        fn close(&mut self, index: usize) {
            let e = self.entries.remove(index);
            self.save(&e);
            if self.entries.is_empty() {
                self.fonts = None;
            }
        }

        pub fn close_all(&mut self) {
            while !self.entries.is_empty() {
                self.close(self.entries.len() - 1);
            }
        }

        /// The dock was hidden or shown by the user (`SetHidden`).
        pub fn set_hidden(&mut self, hidden: bool) {
            if hidden == self.hidden {
                return;
            }
            self.hidden = hidden;
            for e in &mut self.entries {
                e.view.set_visible(!hidden);
                if hidden {
                    e.window.release();
                    e.canvas.release();
                    e.shown = false;
                } else {
                    e.paint = true;
                }
            }
        }

        /// Services the open previews need.
        pub fn demand(&self, snaps: &Snapshots) -> Demand {
            self.entries
                .iter()
                .fold(Demand::default(), |d, e| d.merge(e.view.demand(snaps)))
        }

        /// When a preview next needs a frame (host clock seconds).
        pub fn next_frame(&self, now: f64) -> Option<f64> {
            self.entries
                .iter()
                .filter_map(|e| {
                    if e.paint {
                        Some(now)
                    } else {
                        e.view.next_frame(now)
                    }
                })
                .reduce(f64::min)
        }

        /// Handles input events, applies requests, ticks the views and presents what changed.
        /// Commands the pages produced land in `out`. True when anything was presented.
        pub fn pump(&mut self, snaps: &Snapshots, out: &mut Vec<Command>) -> bool {
            if self.entries.is_empty() {
                return false;
            }
            let mut presented = false;
            let work = work_of(snaps);
            let mut closed = Vec::new();
            for i in 0..self.entries.len() {
                // Window events.
                let mut input = None;
                {
                    let e = &mut self.entries[i];
                    while let Ok(event) = e.events.try_recv() {
                        match event {
                            HostEvent::Input => input = Some(e.window.take_input()),
                            HostEvent::WindowDpi(dpi) => {
                                e.view.set_scale(dpi as f32 / 96.0);
                                e.paint = true;
                            }
                            HostEvent::TextFocusLost => {
                                e.view.signal(Signal::FocusLost, snaps, out);
                            }
                            _ => {}
                        }
                    }
                    if let Some(mut input) = input {
                        input.time = snaps.now;
                        e.view.input(&input, snaps, out);
                    }
                }
                // Requests (move, resize, settle, close).
                let requests = self.entries[i].view.take_requests();
                for request in requests {
                    let scale = self.entries[i].scale();
                    match request {
                        DetachedRequest::Move { dx, dy } => {
                            let e = &mut self.entries[i];
                            let rect = clamp_px(
                                (e.origin.0 + dx, e.origin.1 + dy, e.size.0, e.size.1),
                                work,
                                scale,
                            );
                            Self::move_entry(e, rect, work, scale);
                        }
                        DetachedRequest::Resize { width, height } => {
                            let e = &mut self.entries[i];
                            let rect =
                                clamp_px((e.origin.0, e.origin.1, width, height), work, scale);
                            Self::move_entry(e, rect, work, scale);
                            e.paint = true;
                        }
                        DetachedRequest::Settled => self.save(&self.entries[i]),
                        DetachedRequest::Close => closed.push(i),
                    }
                }
                let e = &mut self.entries[i];
                let dirty = e.view.tick(snaps, out) || e.paint;
                e.view.drain(out);
                if dirty && !self.hidden {
                    if let Some(fonts) = &self.fonts {
                        let size = e.view.size(snaps);
                        if size.0 > 0 {
                            e.canvas.resize(size.0, size.1);
                            e.canvas.clear(Rgba::TRANSPARENT);
                            e.view.paint(&mut e.canvas, fonts, snaps);
                            e.view.drain(out);
                            e.window.set_opacity(snaps.settings.opacity as f32);
                            e.window.present(&e.canvas, e.origin.0, e.origin.1);
                            e.window.set_text_input(e.view.wants_focus());
                            e.shown = true;
                            presented = true;
                        }
                    }
                    e.paint = false;
                }
            }
            for i in closed.into_iter().rev() {
                if i < self.entries.len() {
                    self.close(i);
                }
            }
            presented
        }

        /// True while a drag or resize of a preview is in progress.
        pub fn interaction_active(&self) -> bool {
            self.entries.iter().any(|e| e.view.interaction_active())
        }
    }

    impl Drop for DetachedHost {
        fn drop(&mut self) {
            self.close_all();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn documents_are_named_like_the_csharp_store_keys() {
        assert_eq!(document_name("media"), "detached-media");
    }

    #[test]
    fn dip_geometry_converts_to_pixels_and_back() {
        let g = DetachedGeometry {
            left: 100.0,
            top: 50.0,
            width: 350.0,
            height: 330.0,
        };
        assert_eq!(geometry_to_px(&g, 1.5), (150, 75, 525, 495));
        assert_eq!(px_to_geometry((150, 75, 525, 495), 1.5), g);
    }

    #[test]
    fn placing_puts_the_pointer_over_the_title_and_stays_on_screen() {
        let work = (0, 0, 1920, 1040);
        assert_eq!(
            place_at((960, 60), (350, 330), 1.0, work),
            (912, 38, 350, 330)
        );
        // Near the bottom-right corner the window is pushed back inside.
        assert_eq!(
            place_at((1900, 1030), (350, 330), 1.0, work),
            (1570, 710, 350, 330)
        );
        // Off the top-left it is pulled in.
        assert_eq!(place_at((0, 0), (350, 330), 1.0, work), (0, 0, 350, 330));
    }

    #[test]
    fn clamping_enforces_the_scaled_minimum_and_the_work_area() {
        let work = (0, 0, 1920, 1080);
        assert_eq!(clamp_px((10, 10, 100, 50), work, 1.0), (10, 10, 280, 220));
        assert_eq!(clamp_px((10, 10, 100, 50), work, 2.0), (10, 10, 560, 440));
        assert_eq!(clamp_px((0, 0, 9000, 9000), work, 1.0), (0, 0, 1920, 1080));
        // A secondary monitor with a negative origin.
        let left = (-1920, 0, 1920, 1080);
        assert_eq!(
            clamp_px((-2000, 5, 350, 330), left, 1.0),
            (-1920, 5, 350, 330)
        );
    }

    #[test]
    fn the_default_size_scales_with_the_monitor() {
        assert_eq!(default_size_px(1.0), (350, 330));
        assert_eq!(default_size_px(1.25), (438, 413));
    }
}
