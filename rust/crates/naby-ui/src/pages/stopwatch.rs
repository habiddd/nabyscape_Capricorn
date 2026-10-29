//! The `stopwatch` page (`StopwatchView` in `Features/Activities/StopwatchState.cs`): heading,
//! tabular digits, a Start / Pause / Resume button and Reset.

use super::common::{Ctl, Interaction, Presence};
use super::{Page, PageCtx};
use naby_core::command::Command;
use naby_core::session::{Clock, Stopwatch};
use naby_render::{Canvas, InputState, Rect, Weight};

const TOGGLE: u32 = 1;
const RESET: u32 = 2;

pub fn build() -> Box<dyn Page> {
    Box::new(StopwatchPage::default())
}

/// The elapsed time the way `StopwatchState.Display` prints it.
fn elapsed_text(stopwatch: &Stopwatch) -> (f64, String) {
    let mut sw = stopwatch.clone();
    let clock = Clock::system();
    let elapsed = sw.elapsed(clock);
    (elapsed, sw.display(clock))
}

fn action_label(stopwatch: &Stopwatch) -> &'static str {
    let mut sw = stopwatch.clone();
    sw.action_label(Clock::system())
}

/// The shared body of the page and its detached view: returns the height used.
pub(super) fn layout(c: &mut Ctl, rect: Rect, digits: &str, label: &str) -> f32 {
    let th = c.theme().clone();
    let weight = if th.bold_headings {
        Weight::Semibold
    } else {
        Weight::Regular
    };
    let mut y = rect.y;
    c.text(rect.x, y, "Stopwatch", 13.0, weight, th.text, Some(rect.w));
    y += c.line_h(13.0, weight) + c.px(10.0);
    c.text_tabular(rect.x, y, digits, 29.0, Weight::Regular, th.text);
    y += c.line_h(29.0, Weight::Regular) + c.px(8.0);
    // Two buttons, 11 DIP text, padding 9/4, min height 30, default margin 0,3,6,3.
    let h = c.px(30.0);
    let mut x = rect.x;
    y += c.px(3.0);
    for (id, text) in [(TOGGLE, label), (RESET, "Reset")] {
        let w = c.text_w(text, 11.0, Weight::Regular) + c.px(18.0);
        let r = Rect::new(x, y, w, h);
        let b = c.text_button(
            id,
            r,
            text,
            11.0,
            Weight::Regular,
            th.control_text,
            Some(th.control_fill),
            true,
        );
        if b.clicked {
            c.push(if id == TOGGLE {
                Command::StopwatchToggle
            } else {
                Command::StopwatchReset
            });
        }
        x += w + c.px(6.0);
    }
    y += h + c.px(3.0);
    y - rect.y
}

#[derive(Default)]
struct StopwatchPage {
    ui: Interaction,
    presence: Presence,
    secs: Option<u64>,
    text: String,
    running: bool,
    /// Host time of the next whole second of the elapsed time.
    next: Option<f64>,
}

impl StopwatchPage {
    fn refresh(&mut self, ctx: &PageCtx) -> &'static str {
        let sw = &ctx.snaps.session.stopwatch;
        let (elapsed, text) = elapsed_text(sw);
        self.running = sw.running;
        if self.secs != Some(elapsed.floor() as u64) || self.text.is_empty() {
            self.secs = Some(elapsed.floor() as u64);
            self.text = text;
        }
        self.next = self
            .running
            .then(|| ctx.snaps.now + (1.0 - elapsed.fract()).max(0.02) + 0.005);
        action_label(sw)
    }
}

impl Page for StopwatchPage {
    fn id(&self) -> &'static str {
        "stopwatch"
    }

    fn height(&mut self, ctx: &PageCtx, width: f32) -> f32 {
        let label = self.refresh(ctx);
        let input = InputState::default();
        let mut c = Ctl::measure(ctx, &input, &mut self.ui);
        layout(&mut c, Rect::new(0.0, 0.0, width, 0.0), &self.text, label)
    }

    fn frame(
        &mut self,
        canvas: &mut Canvas,
        ctx: &PageCtx,
        rect: Rect,
        input: &InputState,
        out: &mut Vec<Command>,
    ) -> bool {
        self.presence.mark(ctx.snaps.now);
        let label = self.refresh(ctx);
        let mut c = Ctl::draw(canvas, ctx, input, out, &mut self.ui);
        layout(&mut c, rect, &self.text, label);
        false
    }

    fn next_frame(&self, now: f64) -> Option<f64> {
        self.presence.observe(now);
        if !self.presence.active() {
            return None;
        }
        self.next
    }

    fn captures_pointer(&self) -> bool {
        self.ui.captures()
    }

    fn on_hide(&mut self) {
        self.presence.hide();
        self.ui.reset();
        self.secs = None;
        self.next = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pages::common::harness::Harness;
    use naby_core::session::SessionState;

    #[test]
    fn buttons_emit_toggle_and_reset() {
        let Some(mut h) = Harness::new("stopwatch") else {
            return;
        };
        h.idle();
        h.click_id(TOGGLE);
        h.click_id(RESET);
        assert_eq!(
            h.commands(),
            vec![Command::StopwatchToggle, Command::StopwatchReset]
        );
    }

    #[test]
    fn a_running_stopwatch_asks_for_a_frame_each_second_only_while_shown() {
        let Some(mut h) = Harness::new("stopwatch") else {
            return;
        };
        let mut session = SessionState::default();
        session.stopwatch.toggle(Clock::system());
        h.snaps.session.replace(session);
        assert_eq!(h.page.next_frame(h.snaps.now), None, "not drawn yet");
        h.idle();
        let at = h.page.next_frame(h.snaps.now).expect("ticks while running");
        assert!(at > h.snaps.now && at <= h.snaps.now + 1.1);
        // Not drawn for a while: the page goes quiet.
        assert_eq!(h.page.next_frame(h.snaps.now + 10.0), None);
        h.page.on_hide();
        assert_eq!(h.page.next_frame(h.snaps.now), None);
    }

    #[test]
    fn tab_and_enter_work_from_the_keyboard() {
        use naby_render::{Key, KeyEvent};
        let Some(mut h) = Harness::new("stopwatch") else {
            return;
        };
        h.idle();
        h.frame(InputState::key(KeyEvent::new(Key::Tab)));
        h.frame(InputState::key(KeyEvent::new(Key::Enter)));
        assert_eq!(h.commands(), vec![Command::StopwatchToggle]);
        h.frame(InputState::key(KeyEvent::new(Key::Tab)));
        h.frame(InputState::key(KeyEvent::new(Key::Space)));
        assert_eq!(h.commands(), vec![Command::StopwatchReset]);
    }

    #[test]
    fn a_stopped_stopwatch_schedules_nothing_and_says_start() {
        let Some(mut h) = Harness::new("stopwatch") else {
            return;
        };
        h.idle();
        assert_eq!(h.page.next_frame(h.snaps.now), None);
        assert_eq!(action_label(&h.snaps.session.stopwatch), "Start");
    }

    #[test]
    fn snapshot_stopwatch() {
        let Some(mut h) = Harness::new("stopwatch") else {
            return;
        };
        let mut session = SessionState::default();
        session.stopwatch.elapsed_seconds = 754.0;
        h.snaps.session.replace(session);
        h.idle();
        assert!(h.ink() > 500);
        h.save("page-stopwatch");
    }
}
