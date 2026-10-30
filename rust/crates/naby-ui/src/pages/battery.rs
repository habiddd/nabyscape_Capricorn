//! The `battery` page: "63% · Charging" / "63% · Battery" (`OverlayWindow.Update`). The WPF
//! card is collapsed on a desktop; here the line says why there is nothing to show instead.

use super::common::{Ctl, Interaction};
use super::{Page, PageCtx};
use naby_core::command::Command;
use naby_core::system::battery::BatterySnapshot;
use naby_render::{Canvas, InputState, Rect, Weight};

pub fn build() -> Box<dyn Page> {
    Box::new(BatteryPage::default())
}

/// The line shown for a snapshot: the C# status text with a battery, a plain statement without.
pub fn line(battery: &BatterySnapshot) -> (String, bool) {
    if battery.visible() {
        (battery.status_text(), true)
    } else if battery.available {
        ("Connected to power".to_owned(), false)
    } else {
        ("Battery status unavailable".to_owned(), false)
    }
}

#[derive(Default)]
struct BatteryPage {
    ui: Interaction,
    version: u64,
    text: String,
    primary: bool,
}

impl BatteryPage {
    fn refresh(&mut self, ctx: &PageCtx) {
        let v = ctx.snaps.battery.version();
        if v != self.version || self.text.is_empty() {
            self.version = v;
            let (text, primary) = line(&ctx.snaps.battery);
            self.text = text;
            self.primary = primary;
        }
    }

    fn layout(c: &mut Ctl, rect: Rect, text: &str, primary: bool) -> f32 {
        let th = c.theme().clone();
        let color = if primary { th.text } else { th.text_dim };
        let h = c.paragraph(rect.x, rect.y, rect.w, text, 13.0, Weight::Regular, color);
        h.max(c.line_h(13.0, Weight::Regular))
    }
}

impl Page for BatteryPage {
    fn id(&self) -> &'static str {
        "battery"
    }

    fn height(&mut self, ctx: &PageCtx, width: f32) -> f32 {
        self.refresh(ctx);
        let input = InputState::default();
        let mut c = Ctl::measure(ctx, &input, &mut self.ui);
        Self::layout(
            &mut c,
            Rect::new(0.0, 0.0, width, 0.0),
            &self.text,
            self.primary,
        )
    }

    fn frame(
        &mut self,
        canvas: &mut Canvas,
        ctx: &PageCtx,
        rect: Rect,
        input: &InputState,
        out: &mut Vec<Command>,
    ) -> bool {
        self.refresh(ctx);
        let mut c = Ctl::draw(canvas, ctx, input, out, &mut self.ui);
        Self::layout(&mut c, rect, &self.text, self.primary);
        false
    }

    fn on_hide(&mut self) {
        self.version = 0;
        self.text.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pages::common::harness::Harness;
    use naby_core::system::battery::RawPowerStatus;

    fn raw(ac: u8, flag: u8, percent: u8) -> BatterySnapshot {
        BatterySnapshot::from_raw(RawPowerStatus {
            ac_line_status: ac,
            battery_flag: flag,
            battery_life_percent: percent,
            system_status_flag: 0,
            battery_life_time: u32::MAX,
        })
    }

    #[test]
    fn lines_match_the_wpf_text() {
        assert_eq!(line(&raw(0, 1, 63)), ("63% · Battery".to_owned(), true));
        assert_eq!(line(&raw(1, 9, 80)), ("80% · Charging".to_owned(), true));
        assert_eq!(line(&raw(1, 128, 255)).0, "Connected to power");
        assert_eq!(
            line(&BatterySnapshot::UNAVAILABLE).0,
            "Battery status unavailable"
        );
    }

    #[test]
    fn every_state_paints_and_schedules_nothing() {
        let Some(mut h) = Harness::new("battery") else {
            return;
        };
        for (name, snap) in [
            ("page-battery", raw(0, 1, 63)),
            ("page-battery-charging", raw(1, 9, 80)),
            ("page-battery-desktop", raw(1, 128, 255)),
            ("page-battery-unavailable", BatterySnapshot::UNAVAILABLE),
        ] {
            h.snaps.battery.replace(snap);
            h.idle();
            assert!(h.ink() > 200, "{name} draws a line");
            h.save(name);
            assert_eq!(h.page.next_frame(h.snaps.now), None);
        }
    }
}
