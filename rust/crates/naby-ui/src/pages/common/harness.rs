//! Test rig: hosts one page on a black "island" canvas, feeds it pointer input and collects
//! the commands it emits. Pages are painted at their natural height for a content width.

use super::ctl::HITS;
use crate::host::Snapshots;
use crate::pages::{self, Page, PageCtx};
use chrono::{Local, TimeZone};
use naby_core::color::Rgba;
use naby_core::command::Command;
use naby_core::settings::{Settings, WidgetSetting};
use naby_render::{snapshot, Canvas, Fonts, InputState, Rect, Theme};

pub struct Harness {
    pub snaps: Snapshots,
    pub fonts: Fonts,
    pub theme: Theme,
    pub scale: f32,
    pub canvas: Canvas,
    pub page: Box<dyn Page>,
    pub out: Vec<Command>,
    /// Content width in design units.
    pub width: f32,
    pub rect: Rect,
    pad: f32,
}

/// Settings that enable the widgets the pages under test belong to.
pub fn settings() -> Settings {
    Settings {
        widgets: [
            "media",
            "volume",
            "timer",
            "focus",
            "stopwatch",
            "clock",
            "system",
            "battery",
            "weather",
        ]
        .iter()
        .map(|id| WidgetSetting::new(id, true))
        .collect(),
        ..Settings::default()
    }
}

impl Harness {
    /// A rig for page `id`, or `None` (with a note) when the machine has no fonts.
    pub fn new(id: &str) -> Option<Harness> {
        Self::with(id, settings())
    }

    pub fn with(id: &str, settings: Settings) -> Option<Harness> {
        let fonts = Fonts::load_system();
        if !fonts.has_faces() {
            println!("skip: no system font found");
            return None;
        }
        let mut snaps = Snapshots::new(settings, 10.0);
        snaps.wall = Local.with_ymd_and_hms(2026, 10, 29, 12, 0, 0).unwrap();
        let theme = Theme::from_settings(&snaps.settings);
        Some(Harness {
            snaps,
            fonts,
            theme,
            scale: 1.0,
            canvas: Canvas::new(1, 1),
            page: pages::build(id),
            out: Vec::new(),
            width: 340.0,
            rect: Rect::default(),
            pad: 16.0,
        })
    }

    pub fn set_scale(&mut self, scale: f32) {
        self.scale = scale;
    }

    /// Re-reads the theme after changing settings.
    pub fn refresh_theme(&mut self) {
        self.theme = Theme::from_settings(&self.snaps.settings);
    }

    pub fn ctx(&self) -> PageCtx<'_> {
        PageCtx {
            fonts: &self.fonts,
            snaps: &self.snaps,
            theme: &self.theme,
            scale: self.scale,
        }
    }

    /// Natural height in device pixels at the current width.
    pub fn height(&mut self) -> f32 {
        let w = self.width * self.scale;
        let ctx = PageCtx {
            fonts: &self.fonts,
            snaps: &self.snaps,
            theme: &self.theme,
            scale: self.scale,
        };
        self.page.height(&ctx, w)
    }

    /// Lays out and paints one frame; true while the page asks for another right away.
    pub fn frame(&mut self, mut input: InputState) -> bool {
        input.time = self.snaps.now;
        let pad = self.pad * self.scale;
        let w = self.width * self.scale;
        let h = self.height().max(1.0);
        self.canvas
            .resize((w + 2.0 * pad).ceil() as u32, (h + 2.0 * pad).ceil() as u32);
        self.canvas.clear(Rgba::BLACK);
        self.rect = Rect::new(pad, pad, w, h);
        let ctx = PageCtx {
            fonts: &self.fonts,
            snaps: &self.snaps,
            theme: &self.theme,
            scale: self.scale,
        };
        self.page
            .frame(&mut self.canvas, &ctx, self.rect, &input, &mut self.out)
    }

    /// A frame with no input.
    pub fn idle(&mut self) -> bool {
        self.frame(InputState::default())
    }

    /// Advances the clocks by `seconds` (host and wall).
    pub fn advance(&mut self, seconds: f64) {
        self.snaps.now += seconds;
        self.snaps.wall += chrono::Duration::milliseconds((seconds * 1000.0) as i64);
    }

    /// Center of the control `id` drawn by the last frame.
    pub fn center_of(&self, id: u32) -> (f32, f32) {
        self.rect_of(id).center()
    }

    pub fn rect_of(&self, id: u32) -> Rect {
        HITS.with(|h| {
            h.borrow()
                .iter()
                .find(|(i, _, _)| *i == id)
                .map(|(_, r, _)| *r)
                .unwrap_or_else(|| panic!("control {id} was not drawn"))
        })
    }

    /// The control `id` was drawn and accepts input.
    pub fn is_enabled(&self, id: u32) -> bool {
        HITS.with(|h| h.borrow().iter().any(|(i, _, on)| *i == id && *on))
    }

    pub fn has_control(&self, id: u32) -> bool {
        HITS.with(|h| h.borrow().iter().any(|(i, _, _)| *i == id))
    }

    /// Press and release on (`x`, `y`) in three frames.
    pub fn click(&mut self, p: (f32, f32)) {
        self.frame(InputState::at(p.0, p.1));
        self.frame(InputState::press(p.0, p.1));
        self.frame(InputState::release(p.0, p.1));
        // The pointer stays; redraw so state that follows the release settles.
        self.frame(InputState::at(p.0, p.1));
    }

    /// Clicks the control `id` of the last frame.
    pub fn click_id(&mut self, id: u32) {
        let p = self.center_of(id);
        self.click(p);
    }

    /// Presses on `id` and drags to horizontal fraction `to` of its width, then releases.
    pub fn drag_id(&mut self, id: u32, to: f32) {
        let r = self.rect_of(id);
        let y = r.y + r.h / 2.0;
        let (x0, x1) = (
            r.x + r.w * 0.1,
            r.x + 7.0 * self.scale + (r.w - 14.0 * self.scale) * to,
        );
        self.frame(InputState::at(x0, y));
        self.frame(InputState::press(x0, y));
        self.frame(InputState::drag(x1, y));
        self.frame(InputState::release(x1, y));
        self.frame(InputState::at(x1, y));
    }

    pub fn commands(&mut self) -> Vec<Command> {
        std::mem::take(&mut self.out)
    }

    /// Writes the last frame to `target/snapshots/<name>.png`.
    pub fn save(&self, name: &str) {
        snapshot::save(name, &self.canvas);
    }

    /// Number of pixels that are not the island background.
    pub fn ink(&self) -> usize {
        snapshot::count_differing(&self.canvas, self.canvas.bounds(), Rgba::BLACK)
    }
}

/// A 96 px two-color cover encoded as PNG.
pub fn cover_png() -> std::sync::Arc<[u8]> {
    let mut c = Canvas::new(96, 96);
    c.clear(Rgba::rgb(0x2B, 0x4C, 0x7E));
    c.fill_circle(
        30.0,
        30.0,
        40.0,
        &naby_render::Fill::Solid(Rgba::rgb(0xE0, 0x5A, 0x47)),
    );
    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path =
        std::env::temp_dir().join(format!("naby-ui-page-cover-{}-{n}.png", std::process::id()));
    c.save_png(&path).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let _ = std::fs::remove_file(path);
    std::sync::Arc::from(bytes)
}
