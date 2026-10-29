//! Widget pages of the open panel (and their optional hover-peek faces).
//!
//! # Adding or replacing a page is a one-file change
//!
//! Every widget id has its own file in this directory exporting `pub fn build() -> Box<dyn Page>`;
//! [`build`] below already calls it. Today each file returns a [`TitlePage`] (the page title and a
//! hint). To implement a page, replace the body of that file's `build()` with your own struct that
//! implements [`Page`] — nothing outside the file changes. A new id needs one `mod` line and one
//! registry line here.
//!
//! # Contract
//!
//! * Pages are immediate mode: `frame` receives the frame's input and draws. Use
//!   `naby_render::Ui` with a `UiMemory` you keep in the page, or paint with the canvas directly.
//! * Units are **device pixels**. `ctx.scale` is device pixels per design unit (DPI scale times the
//!   panel scale), so `ctx.px(12.0)` is a 12 DIP gap. Fonts are `ctx.fonts`; for `Ui` pages build a
//!   theme with `ctx.theme` (its `text_scale` already includes `ctx.scale`).
//! * `height` returns the natural content height for a given width; the overlay sizes the panel
//!   from it (`PanelAutoHeight`) and scrolls when the page is taller than the viewport.
//! * Read data from `ctx.snaps`; push [`Command`]s for anything that touches the OS; mutate
//!   `ctx.snaps.productivity` / `ctx.snaps.calendar` directly (see `naby_core::command`).
//! * Return `true` from `frame` while something animates; otherwise return `false` and (if a
//!   countdown or clock needs repainting) report it through `next_frame`.
//! * Report services you need through `demand`; the host starts them only while you are shown.
//! * Drop buffers and caches in `on_hide`.

use crate::host::{Demand, Snapshots};
use naby_core::command::Command;
use naby_core::settings::{widget_display_name, Settings};
use naby_render::{Canvas, Fonts, InputState, Rect, TextStyle, Theme, Weight};

mod awake;
mod battery;
mod calendar;
mod clips;
mod clock;
mod files;
mod focus;
mod links;
mod lyrics;
mod media;
mod mixer;
mod notes;
mod shortcuts;
mod stopwatch;
mod system;
mod tasks;
mod timer;
mod tools;
mod voice;
mod volume;
mod weather;

/// What a page may use while it draws.
pub struct PageCtx<'a> {
    pub fonts: &'a Fonts,
    pub snaps: &'a Snapshots,
    pub theme: &'a Theme,
    /// Device pixels per design unit.
    pub scale: f32,
}

impl PageCtx<'_> {
    /// Design units to device pixels.
    pub fn px(&self, dip: f32) -> f32 {
        dip * self.scale
    }

    pub fn settings(&self) -> &Settings {
        &self.snaps.settings
    }

    /// A text style of `dip` design units in `weight` and `color` (scaled by the accessibility
    /// text scale too).
    pub fn style(&self, dip: f32, weight: Weight, color: naby_core::color::Rgba) -> TextStyle {
        TextStyle {
            size: self.px(dip) * self.theme.text_scale,
            weight,
            color,
        }
    }
}

/// One widget's page. See the module docs for the contract.
pub trait Page {
    /// The widget id (`"media"`, `"timer"`, ...).
    fn id(&self) -> &'static str;

    /// Natural height in device pixels for a content width in device pixels.
    fn height(&mut self, ctx: &PageCtx, _width: f32) -> f32 {
        ctx.px(64.0)
    }

    /// Handles `input` and draws into `rect` (device pixels). Pushes commands for the host.
    /// Returns true while the page animates and wants another frame right away.
    fn frame(
        &mut self,
        canvas: &mut Canvas,
        ctx: &PageCtx,
        rect: Rect,
        input: &InputState,
        out: &mut Vec<Command>,
    ) -> bool;

    /// Optional hover-peek face. `None` (the default) lets the overlay draw its built-in peek for
    /// this id; `Some(animating)` means the page drew itself into `rect`. `detailed` is false
    /// for the first 100 DIP stage.
    fn peek(
        &mut self,
        _canvas: &mut Canvas,
        _ctx: &PageCtx,
        _rect: Rect,
        _input: &InputState,
        _out: &mut Vec<Command>,
        _detailed: bool,
    ) -> Option<bool> {
        None
    }

    /// When the page next needs a frame while shown (`None` when static).
    fn next_frame(&self, _now: f64) -> Option<f64> {
        None
    }

    /// Services this page needs while shown.
    fn demand(&self) -> Demand {
        Demand::default()
    }

    /// A widget of this page holds the pointer (pressed button, dragged slider, focused text
    /// field): the panel must not start a page swipe or treat the wheel as navigation.
    fn captures_pointer(&self) -> bool {
        false
    }

    /// The page wants the wide panel (synced lyrics beside the controls).
    fn wants_wide(&self) -> bool {
        false
    }

    /// The panel closed or hid: free caches and decoded images.
    fn on_hide(&mut self) {}
}

/// Creates the page for a widget id. Unknown ids get a title page.
pub fn build(id: &str) -> Box<dyn Page> {
    match id {
        "media" => media::build(),
        "lyrics" => lyrics::build(),
        "volume" => volume::build(),
        "mixer" => mixer::build(),
        "timer" => timer::build(),
        "focus" => focus::build(),
        "stopwatch" => stopwatch::build(),
        "clock" => clock::build(),
        "weather" => weather::build(),
        "system" => system::build(),
        "battery" => battery::build(),
        "shortcuts" => shortcuts::build(),
        "files" => files::build(),
        "notes" => notes::build(),
        "tasks" => tasks::build(),
        "clips" => clips::build(),
        "calendar" => calendar::build(),
        "awake" => awake::build(),
        "links" => links::build(),
        "voice" => voice::build(),
        "tools" => tools::build(),
        other => Box::new(TitlePage::new(static_id(other))),
    }
}

/// Every id [`build`] knows, in registry order.
pub const PAGE_IDS: &[&str] = &[
    "media",
    "lyrics",
    "volume",
    "mixer",
    "timer",
    "focus",
    "stopwatch",
    "clock",
    "weather",
    "system",
    "battery",
    "shortcuts",
    "files",
    "notes",
    "tasks",
    "clips",
    "calendar",
    "awake",
    "links",
    "voice",
    "tools",
];

fn static_id(id: &str) -> &'static str {
    PAGE_IDS
        .iter()
        .copied()
        .find(|k| *k == id)
        .unwrap_or("page")
}

/// Display name of a page / widget id (`Settings.Names`, with "Music" for the media peek).
pub fn page_title(id: &str) -> String {
    match id {
        "lyrics" => "Lyrics".to_owned(),
        "mixer" => "Mixer".to_owned(),
        other => widget_display_name(other).unwrap_or(other).to_owned(),
    }
}

/// The built-in placeholder: the page title and a hint that the page is not implemented yet.
/// Later tasks replace it per file.
pub struct TitlePage {
    id: &'static str,
}

impl TitlePage {
    pub fn new(id: &'static str) -> Self {
        TitlePage { id }
    }
}

impl Page for TitlePage {
    fn id(&self) -> &'static str {
        self.id
    }

    fn height(&mut self, ctx: &PageCtx, _width: f32) -> f32 {
        ctx.px(72.0)
    }

    fn frame(
        &mut self,
        canvas: &mut Canvas,
        ctx: &PageCtx,
        rect: Rect,
        _input: &InputState,
        _out: &mut Vec<Command>,
    ) -> bool {
        let title = page_title(self.id);
        let heading = ctx.style(18.0, Weight::Semibold, ctx.theme.text);
        canvas.text(ctx.fonts, rect.x, rect.y, &title, &heading, Some(rect.w));
        let hint = ctx.style(12.0, Weight::Regular, ctx.theme.text_dim);
        canvas.text(
            ctx.fonts,
            rect.x,
            rect.y + ctx.px(28.0),
            "Coming to this build soon.",
            &hint,
            Some(rect.w),
        );
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_registered_id_builds_a_page_with_that_id() {
        for id in PAGE_IDS {
            assert_eq!(build(id).id(), *id);
        }
        assert_eq!(build("unknown").id(), "page");
    }
}
