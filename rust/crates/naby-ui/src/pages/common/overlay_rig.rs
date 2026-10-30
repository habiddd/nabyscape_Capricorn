//! Test rig that drives the real overlay to show a page open in the panel or as a hover face.

use super::harness::settings;
use crate::host::{Signal, Snapshots, View};
use crate::overlay::Overlay;
use chrono::{Duration, Local, TimeZone};
use naby_core::color::Rgba;
use naby_core::command::Command;
use naby_core::settings::Settings;
use naby_render::{snapshot, Canvas, Fonts, InputState};

pub struct OverlayRig {
    pub snaps: Snapshots,
    pub overlay: Overlay,
    pub out: Vec<Command>,
    pub fonts: Fonts,
    pub canvas: Canvas,
}

impl OverlayRig {
    pub fn new() -> Option<OverlayRig> {
        Self::with(settings())
    }

    pub fn with(settings: Settings) -> Option<OverlayRig> {
        let fonts = Fonts::load_system();
        if !fonts.has_faces() {
            println!("skip: no system font found");
            return None;
        }
        let mut snaps = Snapshots::new(settings, 10.0);
        snaps.wall = Local.with_ymd_and_hms(2026, 10, 29, 12, 0, 0).unwrap();
        let overlay = Overlay::new(&snaps);
        Some(OverlayRig {
            snaps,
            overlay,
            out: Vec::new(),
            fonts,
            canvas: Canvas::new(1, 1),
        })
    }

    pub fn advance(&mut self, seconds: f64) {
        for _ in 0..(seconds / 0.01).round() as usize {
            self.snaps.now += 0.01;
            self.snaps.wall += Duration::milliseconds(10);
            self.overlay.tick(&self.snaps, &mut self.out);
        }
    }

    pub fn paint(&mut self) {
        let (w, h) = self.overlay.size(&self.snaps);
        self.canvas.resize(w.max(1), h.max(1));
        self.canvas.clear(Rgba::rgb(0x3D, 0x41, 0x4A));
        self.overlay
            .paint(&mut self.canvas, &self.fonts, &self.snaps);
        self.overlay.drain(&mut self.out);
    }

    /// Opens the panel on the page of widget `id` and lets it settle.
    pub fn open_page(&mut self, id: &'static str) {
        self.overlay
            .signal(Signal::Panel, &self.snaps, &mut self.out);
        self.advance(1.5);
        self.paint();
        self.overlay
            .signal(Signal::ShowPage(id), &self.snaps, &mut self.out);
        self.advance(1.5);
        self.paint();
        self.advance(0.5);
        self.paint();
    }

    /// Hovers the closed dock until the detailed peek is up, then pages to `id`.
    pub fn hover_page(&mut self, id: &str) {
        self.paint();
        let (ox, oy) = self.overlay.surface_origin(&self.snaps);
        let s = self.overlay.scale;
        let (sw, sh) = self.overlay.surf;
        let p = (ox + sw as f32 * s / 2.0, oy + sh as f32 * s / 2.0);
        let mut input = InputState::at(p.0, p.1);
        input.time = self.snaps.now;
        self.overlay.input(&input, &self.snaps, &mut self.out);
        self.advance(1.0);
        self.advance(1.5);
        self.paint();
        self.overlay.select_peek(&self.snaps, id);
        self.advance(1.0);
        self.paint();
    }

    pub fn save(&self, name: &str) {
        snapshot::save(name, &self.canvas);
    }
}
