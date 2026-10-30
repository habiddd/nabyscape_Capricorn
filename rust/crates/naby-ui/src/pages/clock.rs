//! The `clock` page: the time at 32 DIP and the date below it (`OverlayWindow.Update`'s
//! `clock` / `date` blocks), honouring 24 hour, seconds and show-date settings.

use super::common::{Ctl, Interaction};
use super::{Page, PageCtx};
use chrono::Timelike;
use naby_core::command::Command;
use naby_core::settings::Settings;
use naby_render::{Canvas, InputState, Rect, Weight};

pub fn build() -> Box<dyn Page> {
    Box::new(ClockPage::default())
}

/// `HH:mm`, `HH:mm:ss`, `h:mm tt` or `h:mm:ss tt` of the wall clock (en-US `tt`).
pub fn clock_text(settings: &Settings, wall: &chrono::DateTime<chrono::Local>) -> String {
    let format = match (settings.clock24_hour, settings.clock_seconds) {
        (true, false) => "%H:%M",
        (true, true) => "%H:%M:%S",
        (false, false) => "%-I:%M %p",
        (false, true) => "%-I:%M:%S %p",
    };
    wall.format(format).to_string()
}

/// `ddd, dd MMM` in en-US: "Thu, 29 Oct".
pub fn date_text(wall: &chrono::DateTime<chrono::Local>) -> String {
    wall.format("%a, %d %b").to_string()
}

#[derive(Default)]
struct ClockPage {
    ui: Interaction,
    /// What the strings were built from: the displayed second (or minute) and the settings.
    key: Option<(i64, bool, bool, bool)>,
    time: String,
    date: String,
    next: Option<f64>,
}

impl ClockPage {
    fn refresh(&mut self, ctx: &PageCtx) {
        let s = ctx.settings();
        let wall = &ctx.snaps.wall;
        let stamp = if s.clock_seconds {
            wall.timestamp()
        } else {
            wall.timestamp().div_euclid(60)
        };
        let key = (stamp, s.clock24_hour, s.clock_seconds, s.show_date);
        if self.key != Some(key) {
            self.key = Some(key);
            self.time = clock_text(s, wall);
            self.date = if s.show_date {
                date_text(wall)
            } else {
                String::new()
            };
        }
        // The next change: the next second, or the next minute.
        let frac = f64::from(wall.nanosecond() % 1_000_000_000) / 1e9;
        let until = if s.clock_seconds {
            1.0 - frac
        } else {
            60.0 - f64::from(wall.second()) - frac
        };
        self.next = Some(ctx.snaps.now + until.max(0.02) + 0.005);
    }

    fn layout(c: &mut Ctl, rect: Rect, time: &str, date: &str) -> f32 {
        let th = c.theme().clone();
        let mut y = rect.y;
        c.text(
            rect.x,
            y,
            time,
            32.0,
            Weight::Regular,
            th.text,
            Some(rect.w),
        );
        y += c.line_h(32.0, Weight::Regular);
        if !date.is_empty() {
            y += c.px(3.0);
            c.text(
                rect.x,
                y,
                date,
                11.0,
                Weight::Regular,
                th.text_dim,
                Some(rect.w),
            );
            // The WPF date has a 12 DIP bottom margin; the panel's 10 DIP page gap supplies most of it.
            y += c.line_h(11.0, Weight::Regular) + c.px(2.0);
        }
        y - rect.y
    }
}

impl Page for ClockPage {
    fn id(&self) -> &'static str {
        "clock"
    }

    fn height(&mut self, ctx: &PageCtx, width: f32) -> f32 {
        self.refresh(ctx);
        let input = InputState::default();
        let mut c = Ctl::measure(ctx, &input, &mut self.ui);
        Self::layout(
            &mut c,
            Rect::new(0.0, 0.0, width, 0.0),
            &self.time,
            &self.date,
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
        Self::layout(&mut c, rect, &self.time, &self.date);
        false
    }

    fn next_frame(&self, _now: f64) -> Option<f64> {
        self.next
    }

    fn on_hide(&mut self) {
        self.key = None;
        self.next = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pages::common::harness::Harness;
    use chrono::{Local, TimeZone};

    #[test]
    fn formats_follow_the_settings() {
        let wall = Local.with_ymd_and_hms(2026, 10, 29, 15, 5, 9).unwrap();
        let mut s = Settings {
            clock24_hour: true,
            clock_seconds: false,
            ..Settings::default()
        };
        assert_eq!(clock_text(&s, &wall), "15:05");
        s.clock_seconds = true;
        assert_eq!(clock_text(&s, &wall), "15:05:09");
        s.clock24_hour = false;
        assert_eq!(clock_text(&s, &wall), "3:05:09 PM");
        s.clock_seconds = false;
        assert_eq!(clock_text(&s, &wall), "3:05 PM");
        let morning = Local.with_ymd_and_hms(2026, 10, 29, 0, 7, 0).unwrap();
        assert_eq!(clock_text(&s, &morning), "12:07 AM");
        assert_eq!(date_text(&wall), "Thu, 29 Oct");
    }

    #[test]
    fn schedules_one_wake_per_minute_or_second_and_only_while_shown() {
        let Some(mut h) = Harness::new("clock") else {
            return;
        };
        let mut s = h.snaps.settings.arc().as_ref().clone();
        s.clock_seconds = false;
        h.snaps.settings.replace(s);
        h.idle();
        let at = h.page.next_frame(h.snaps.now).expect("minute tick");
        assert!(at - h.snaps.now > 30.0 && at - h.snaps.now <= 61.0);
        let mut s = h.snaps.settings.arc().as_ref().clone();
        s.clock_seconds = true;
        h.snaps.settings.replace(s);
        h.idle();
        let at = h.page.next_frame(h.snaps.now).expect("second tick");
        assert!(at - h.snaps.now <= 1.1);
    }

    #[test]
    fn the_date_can_be_hidden() {
        let Some(mut h) = Harness::new("clock") else {
            return;
        };
        let mut s = h.snaps.settings.arc().as_ref().clone();
        s.show_date = true;
        h.snaps.settings.replace(s.clone());
        let with = h.height();
        s.show_date = false;
        h.snaps.settings.replace(s);
        assert!(h.height() < with);
    }

    #[test]
    fn snapshot_clock() {
        let Some(mut h) = Harness::new("clock") else {
            return;
        };
        let mut s = h.snaps.settings.arc().as_ref().clone();
        s.clock24_hour = false;
        s.show_date = true;
        h.snaps.settings.replace(s);
        h.idle();
        assert!(h.ink() > 800);
        h.save("page-clock");
    }
}
