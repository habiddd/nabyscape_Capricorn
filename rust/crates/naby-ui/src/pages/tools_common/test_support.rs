//! Page test rig: builds a page, paints it like the overlay does (measure, then frame into a
//! rectangle) and feeds pointer / key / text input one frame at a time.

use super::{clear_marks, marked};
use crate::host::Snapshots;
use crate::pages::{build, Page, PageCtx};
use naby_core::color::Rgba;
use naby_core::command::Command;
use naby_core::settings::Settings;
use naby_render::{snapshot, Canvas, Fonts, InputState, Key, KeyEvent, Rect, Theme};

/// Icons and previews that exist for every path: flat color images of fixed sizes.
#[derive(Default)]
pub struct FakeIcons {
    pub cleared: std::cell::Cell<u32>,
    pub requests: std::cell::Cell<u32>,
}

impl crate::host::IconProvider for FakeIcons {
    fn shortcut_icon(
        &self,
        shortcut: &naby_core::settings::Shortcut,
        pixels: u32,
    ) -> Option<std::sync::Arc<naby_render::Image>> {
        self.requests.set(self.requests.get() + 1);
        let shade = (shortcut.name.len() as u8).wrapping_mul(37);
        let px: Vec<u8> = (0..pixels * pixels)
            .flat_map(|_| [60 + shade % 150, 120, 220 - shade % 100, 255])
            .collect();
        naby_render::Image::from_rgba(pixels, pixels, px).map(std::sync::Arc::new)
    }
    fn file_thumbnail(
        &self,
        path: &str,
        pixels: u32,
    ) -> Option<std::sync::Arc<naby_render::Image>> {
        self.requests.set(self.requests.get() + 1);
        let (w, h) = (pixels, pixels * 3 / 4);
        let shade = path.len() as u8;
        let px: Vec<u8> = (0..w * h)
            .flat_map(|i| {
                let x = (i % w) as u8;
                [shade.wrapping_mul(9), x.wrapping_mul(3), 140, 255]
            })
            .collect();
        naby_render::Image::from_rgba(w, h, px).map(std::sync::Arc::new)
    }
    fn clear(&self) {
        self.cleared.set(self.cleared.get() + 1);
    }
}

pub struct Rig {
    pub snaps: Snapshots,
    pub fonts: Fonts,
    pub theme: Theme,
    pub scale: f32,
    pub page: Box<dyn Page>,
    pub canvas: Canvas,
    pub out: Vec<Command>,
    /// Content width in device pixels.
    pub width: f32,
    /// Height the page reported on the last paint.
    pub height: f32,
    pub pad: f32,
}

impl Rig {
    /// `None` (test skipped) when the machine has no fonts.
    pub fn new(id: &str) -> Option<Rig> {
        let fonts = snapshot::test_fonts()?;
        let settings = Settings::default();
        let theme = Theme::from_settings(&settings);
        let snaps = Snapshots::new(settings, 10.0);
        Some(Rig {
            snaps,
            fonts,
            theme,
            scale: 1.0,
            page: build(id),
            canvas: Canvas::new(1, 1),
            out: Vec::new(),
            width: 320.0,
            height: 0.0,
            pad: 16.0,
        })
    }

    pub fn ctx(&self) -> PageCtx<'_> {
        PageCtx {
            fonts: &self.fonts,
            snaps: &self.snaps,
            theme: &self.theme,
            scale: self.scale,
        }
    }

    /// Lets pages grow tall instead of scrolling their lists (`PanelMaxHeight`).
    pub fn tall(&mut self) {
        let mut s = (**self.snaps.settings.arc()).clone();
        s.panel_max_height = 1100.0;
        self.snaps.settings.replace(s);
    }

    /// Paints one frame with `input` (time follows the snapshot clock). Returns whether the
    /// page asked for another frame.
    pub fn frame(&mut self, input: &InputState) -> bool {
        clear_marks();
        let ctx = PageCtx {
            fonts: &self.fonts,
            snaps: &self.snaps,
            theme: &self.theme,
            scale: self.scale,
        };
        let h = self.page.height(&ctx, self.width);
        self.height = h;
        let w = (self.width + 2.0 * self.pad).ceil() as u32;
        let ch = (h + 2.0 * self.pad).ceil() as u32;
        self.canvas.resize(w, ch);
        self.canvas.clear(self.theme.background);
        let rect = Rect::new(self.pad, self.pad, self.width, h);
        let again = self
            .page
            .frame(&mut self.canvas, &ctx, rect, input, &mut self.out);
        // The overlay repaints once more when the page asked to (heights, focus).
        again
    }

    /// Paints the hover face into a 340 x 130 area; `None` when the page has no face of its own.
    pub fn peek(&mut self, detailed: bool) -> Option<bool> {
        let ctx = PageCtx {
            fonts: &self.fonts,
            snaps: &self.snaps,
            theme: &self.theme,
            scale: self.scale,
        };
        self.canvas.resize(380, 160);
        self.canvas.clear(self.theme.background);
        let rect = Rect::new(20.0, 20.0, 340.0, 120.0);
        self.page.peek(
            &mut self.canvas,
            &ctx,
            rect,
            &InputState::default(),
            &mut self.out,
            detailed,
        )
    }

    /// Paints until the page settles (at most 4 frames) with no input events.
    pub fn paint(&mut self) {
        let quiet = InputState {
            time: self.snaps.now,
            ..InputState::default()
        };
        for _ in 0..4 {
            if !self.frame(&quiet) {
                break;
            }
        }
    }

    /// Pointer position in canvas pixels of the center of the control marked `name`.
    pub fn center_of(&self, name: &str) -> (f32, f32) {
        marked(name)
            .unwrap_or_else(|| panic!("no control marked {name}"))
            .center()
    }

    pub fn rect_of(&self, name: &str) -> Option<Rect> {
        marked(name)
    }

    pub fn has(&self, name: &str) -> bool {
        marked(name).is_some()
    }

    pub fn click_at(&mut self, p: (f32, f32)) {
        self.frame(&InputState::at(p.0, p.1));
        self.frame(&InputState::press(p.0, p.1));
        self.frame(&InputState::release(p.0, p.1));
        self.paint();
    }

    /// Clicks the control marked `name` (found in the last frame).
    pub fn click(&mut self, name: &str) {
        let p = self.center_of(name);
        self.click_at(p);
    }

    pub fn hover(&mut self, name: &str) {
        let p = self.center_of(name);
        self.frame(&InputState::at(p.0, p.1));
    }

    pub fn type_text(&mut self, text: &str) {
        self.frame(&InputState::typed(text));
        self.paint();
    }

    pub fn press_key(&mut self, key: Key) {
        self.frame(&InputState::key(KeyEvent::new(key)));
        self.paint();
    }

    pub fn commands(&mut self) -> Vec<Command> {
        std::mem::take(&mut self.out)
    }

    pub fn save(&self, name: &str) {
        snapshot::save(name, &self.canvas);
    }

    pub fn ink(&self, region: Rect) -> usize {
        snapshot::count_differing(&self.canvas, region, Rgba::rgb(0, 0, 0))
    }
}
