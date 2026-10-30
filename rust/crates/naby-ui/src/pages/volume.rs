//! The `volume` page. With the media widget on, volume lives in the media page's footer
//! (`BuildMediaControls`), so this page draws nothing and cancels the panel's 10 DIP page gap.
//! On its own it is the "Volume" heading with the level button and the same popover: slider,
//! mute, microphone mute, output list and the way into the mixer.

use super::common::{Ctl, Interaction};
use super::media::footer::{Footer, FooterEvent};
use super::mixer::{MixerEvent, MixerView};
use super::{Page, PageCtx};
use crate::host::Demand;
use naby_core::command::Command;
use naby_render::{Canvas, InputState, Rect, Weight};

pub fn build() -> Box<dyn Page> {
    Box::new(VolumePage::default())
}

/// The media page already carries the volume footer.
fn covered_by_media(ctx: &PageCtx) -> bool {
    ctx.snaps.widget_on("media") && ctx.snaps.widget_on("volume")
}

#[derive(Default)]
struct VolumePage {
    ui: Interaction,
    st: Body,
}

#[derive(Default)]
struct Body {
    footer: Footer,
    mixer: MixerView,
    mixer_mode: bool,
    next: Option<f64>,
}

impl Body {
    fn layout(&mut self, c: &mut Ctl, rect: Rect) -> f32 {
        let s = c.s;
        if self.mixer_mode {
            let (h, event) = self.mixer.layout(c, rect);
            if event == MixerEvent::Back {
                self.mixer_mode = false;
            }
            return h;
        }
        let th = c.theme().clone();
        let weight = if th.bold_headings {
            Weight::Semibold
        } else {
            Weight::Regular
        };
        let head = c.line_h(18.0, weight);
        c.text(
            rect.x,
            rect.y,
            "Volume",
            18.0,
            weight,
            th.text,
            Some(rect.w),
        );
        let top = head + 8.0 * s;
        // `content.MinHeight = 144`: the footer sits in a block at least that tall.
        let block = (144.0 * s - top).max(28.0 * s);
        let footer = Rect::new(rect.x, rect.y + top, rect.w, 0.0);
        let (h, event) = self.footer.layout(c, footer, false, true, block);
        if event == FooterEvent::OpenMixer {
            self.mixer_mode = true;
            self.mixer.open();
        }
        if !c.is_dry() {
            self.next = self.footer.next_frame(c.now);
        }
        top + h
    }
}

impl Page for VolumePage {
    fn id(&self) -> &'static str {
        "volume"
    }

    fn height(&mut self, ctx: &PageCtx, width: f32) -> f32 {
        if covered_by_media(ctx) {
            // Cancels the gap the panel puts between two pages of a group.
            return -ctx.px(10.0);
        }
        let input = InputState::default();
        let mut c = Ctl::measure(ctx, &input, &mut self.ui);
        self.st.layout(&mut c, Rect::new(0.0, 0.0, width, 0.0))
    }

    fn frame(
        &mut self,
        canvas: &mut Canvas,
        ctx: &PageCtx,
        rect: Rect,
        input: &InputState,
        out: &mut Vec<Command>,
    ) -> bool {
        if covered_by_media(ctx) {
            return false;
        }
        let mut c = Ctl::draw(canvas, ctx, input, out, &mut self.ui);
        self.st.layout(&mut c, rect);
        false
    }

    fn demand(&self) -> Demand {
        Demand {
            mixer: self.st.mixer_mode,
            ..Demand::default()
        }
    }

    fn next_frame(&self, _now: f64) -> Option<f64> {
        self.st.next
    }

    fn captures_pointer(&self) -> bool {
        self.ui.captures() || self.st.footer.captures() || self.st.mixer.dragging()
    }

    fn on_hide(&mut self) {
        self.ui.reset();
        self.st = Body::default();
    }
}
