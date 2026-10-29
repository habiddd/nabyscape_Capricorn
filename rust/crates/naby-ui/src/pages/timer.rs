//! The `timer` page, and the engine of the `focus` page (`CountdownView` in
//! `Features/Productivity/CountdownView.cs`): heading and status, progress ring with the time
//! left, Start / Pause / Resume and Reset, minute editors with steppers, and a hint line.

use super::common::{Ctl, EditOpts, Interaction, Presence};
use super::{Page, PageCtx};
use chrono::{DateTime, Utc};
use naby_core::command::Command;
use naby_core::motion::{Ease, MotionConfig, Tween};
use naby_core::productivity::timer::{format_seconds, parse_timer_minutes, Timer};
use naby_core::productivity::FocusConfig;
use naby_core::system::format_decimal;
use naby_render::{Canvas, Icon, InputState, Rect, Weight};

const PRIMARY: u32 = 1;
const RESET: u32 = 2;
const MINUS_A: u32 = 3;
const PLUS_A: u32 = 4;
const MINUS_B: u32 = 5;
const PLUS_B: u32 = 6;
const FIELD_A: u32 = 7;
const FIELD_B: u32 = 8;

const FOCUS_RING: naby_core::color::Rgba = naby_core::color::Rgba::rgb(0xDC, 0xC7, 0xAE);
const TRACK: naby_core::color::Rgba = naby_core::color::Rgba::rgb(0x34, 0x38, 0x40);
const FIELD_FILL: naby_core::color::Rgba = naby_core::color::Rgba::rgb(0x14, 0x16, 0x1A);
const FIELD_BORDER: naby_core::color::Rgba = naby_core::color::Rgba::rgb(0x30, 0x34, 0x3B);
const RESET_BORDER: naby_core::color::Rgba = naby_core::color::Rgba::rgb(0x3C, 0x40, 0x48);
const ERROR: naby_core::color::Rgba = naby_core::color::Rgba::rgb(0xF0, 0xB8, 0xA8);

pub fn build() -> Box<dyn Page> {
    Box::new(CountdownPage::new(false))
}

/// `.ToString("0.###")` / `"0.#"` of a minute count.
fn minutes_text(value: f64, decimals: usize) -> String {
    format_decimal(value, decimals)
}

fn digits_only(c: char) -> bool {
    c.is_ascii_digit()
}

fn decimal_chars(c: char) -> bool {
    c.is_ascii_digit() || c == '.' || c == ','
}

/// What the editors currently say.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Parsed {
    seconds: f64,
    work: i32,
    rest: i32,
}

pub struct CountdownPage {
    focus: bool,
    ui: Interaction,
    presence: Presence,
    st: Body,
}

struct Body {
    focus: bool,
    minutes: String,
    breaks: String,
    started: bool,
    known_work: i32,
    known_rest: i32,
    shown_run: u64,
    shown_duration: f64,
    ring: Tween<f64>,
    ring_target: f64,
    /// Host time of the next whole second of the countdown.
    next: Option<f64>,
}

impl CountdownPage {
    pub fn new(focus: bool) -> Self {
        CountdownPage {
            focus,
            ui: Interaction::default(),
            presence: Presence::new(),
            st: Body {
                focus,
                minutes: String::new(),
                breaks: String::new(),
                started: false,
                known_work: 0,
                known_rest: 0,
                shown_run: 0,
                shown_duration: -1.0,
                ring: Tween::instant(0.0),
                ring_target: 0.0,
                next: None,
            },
        }
    }
}

fn wall(ctx: &PageCtx) -> DateTime<Utc> {
    ctx.snaps.wall.with_timezone(&Utc)
}

fn countdown<'a>(ctx: &'a PageCtx, focus: bool) -> &'a Timer {
    if focus {
        &ctx.snaps.session.focus.countdown
    } else {
        &ctx.snaps.session.timer
    }
}

impl Body {
    fn parse(&self) -> Option<Parsed> {
        if self.focus {
            let cfg = FocusConfig::parse(&self.minutes, &self.breaks)?;
            Some(Parsed {
                seconds: f64::from(cfg.focus_minutes) * 60.0,
                work: cfg.focus_minutes as i32,
                rest: cfg.break_minutes as i32,
            })
        } else {
            let seconds = parse_timer_minutes(&self.minutes.replace(',', "."))?;
            Some(Parsed {
                seconds,
                work: 0,
                rest: 0,
            })
        }
    }

    /// Keeps the editors in step with the session (`CountdownView.Refresh`).
    fn sync(&mut self, ctx: &PageCtx) {
        let settings = ctx.settings();
        if !self.started {
            self.started = true;
            self.known_work = settings.focus_minutes;
            self.known_rest = settings.break_minutes;
            self.minutes = if self.focus {
                settings.focus_minutes.to_string()
            } else {
                "5".to_owned()
            };
            self.breaks = settings.break_minutes.to_string();
        }
        let state = countdown(ctx, self.focus);
        let now = wall(ctx);
        let active = state.remaining(now) > 0.0;
        if !self.focus
            && state.duration_seconds > 0.0
            && (self.shown_run != state.run_id || self.shown_duration != state.duration_seconds)
        {
            self.minutes = minutes_text(state.duration_seconds / 60.0, 3);
            self.shown_run = state.run_id;
            self.shown_duration = state.duration_seconds;
        }
        if self.focus && !active {
            if self.known_work != settings.focus_minutes
                && self.minutes == self.known_work.to_string()
            {
                self.minutes = settings.focus_minutes.to_string();
            }
            if self.known_rest != settings.break_minutes
                && self.breaks == self.known_rest.to_string()
            {
                self.breaks = settings.break_minutes.to_string();
            }
            self.known_work = settings.focus_minutes;
            self.known_rest = settings.break_minutes;
        }
    }

    /// Saves edited focus / break minutes when the session is idle (`CommitPreferences`).
    fn commit(&mut self, c: &mut Ctl) {
        if !self.focus {
            return;
        }
        let state = countdown(c.ctx, true);
        if state.remaining(wall(c.ctx)) > 0.0 {
            return;
        }
        let Some(p) = self.parse() else { return };
        let settings = c.ctx.settings();
        if settings.focus_minutes == p.work && settings.break_minutes == p.rest {
            return;
        }
        let mut next = settings.clone();
        next.focus_minutes = p.work;
        next.break_minutes = p.rest;
        self.known_work = p.work;
        self.known_rest = p.rest;
        c.push(Command::ApplySettings(Box::new(next)));
    }

    fn step(text: &mut String, direction: f64, minimum: f64, maximum: f64) {
        if let Ok(value) = text.trim().replace(',', ".").parse::<f64>() {
            if value.is_finite() {
                *text = minutes_text((value + direction).clamp(minimum, maximum), 3);
            }
        }
    }

    fn activate(&mut self, c: &mut Ctl) {
        let state = countdown(c.ctx, self.focus);
        let now = wall(c.ctx);
        if state.running() {
            c.push(if self.focus {
                Command::FocusToggle
            } else {
                Command::TimerPause
            });
        } else if state.remaining(now) > 0.0 {
            c.push(if self.focus {
                Command::FocusToggle
            } else {
                Command::TimerResume
            });
        } else if let Some(p) = self.parse() {
            self.commit(c);
            c.push(if self.focus {
                Command::FocusToggle
            } else {
                Command::TimerStart { seconds: p.seconds }
            });
        }
    }

    /// The whole page: returns its height.
    fn layout(&mut self, c: &mut Ctl, rect: Rect) -> f32 {
        let th = c.theme().clone();
        let state = countdown(c.ctx, self.focus);
        let now = wall(c.ctx);
        let remaining = state.remaining(now);
        let active = remaining > 0.0;
        let completed = !active && state.duration_seconds > 0.0;
        let parsed = self.parse();
        let valid = parsed.is_some();
        let on_break = self.focus && c.ctx.snaps.session.focus.is_break();
        let mut y = rect.y + c.px(4.0);

        // ---- header: heading left, status right -------------------------------------
        let heading = c.tr(if self.focus { "Focus" } else { "Timer" });
        let hw = if th.bold_headings {
            Weight::Semibold
        } else {
            Weight::Regular
        };
        let status_key = if active {
            if state.running() {
                if self.focus {
                    if on_break {
                        "Break"
                    } else {
                        "Focusing"
                    }
                } else {
                    "Running"
                }
            } else {
                "Paused"
            }
        } else if completed {
            "Complete"
        } else {
            "Ready"
        };
        let status = c.tr(status_key);
        let row_h = c.line_h(13.0, hw).max(c.line_h(10.0, Weight::Regular));
        let hy = y + (row_h - c.line_h(13.0, hw)) / 2.0;
        let sy = y + (row_h - c.line_h(10.0, Weight::Regular)) / 2.0;
        let sw = c.text_w(status, 10.0, Weight::Regular);
        c.text(
            rect.x,
            hy,
            heading,
            13.0,
            hw,
            th.text,
            Some((rect.w - sw - c.px(8.0)).max(1.0)),
        );
        c.text_right(
            rect.right(),
            sy,
            status,
            10.0,
            Weight::Regular,
            th.text_dim,
            None,
        );
        y += row_h + c.px(9.0);

        // ---- instrument ---------------------------------------------------------------
        y += c.px(4.0);
        let ring_h = c.px(128.0);
        let ring = Rect::new(rect.x, y, rect.w, ring_h);
        let digits = if active || completed {
            format_seconds(remaining)
        } else {
            format_seconds(parsed.map_or(0.0, |p| p.seconds))
        };
        let duration_text = if active || completed {
            format!("of {} min", minutes_text(state.duration_seconds / 60.0, 1))
        } else if self.focus {
            format!("{} \u{b7} {} min", c.tr("Focus"), self.minutes)
        } else {
            format!("of {} min", self.minutes)
        };
        let target = if active || completed {
            state.progress(now)
        } else {
            0.0
        };
        if !c.is_dry() {
            self.update_ring(c, target);
            let shown = self.ring.value(c.now).clamp(0.0, 1.0);
            let (cx, cy) = ring.center();
            let radius = (ring.w.min(ring.h) / 2.0 - c.px(3.0)).max(2.0);
            let color = if self.focus && !on_break {
                FOCUS_RING
            } else {
                naby_core::color::Rgba::WHITE
            };
            c.circle_stroke(cx, cy, radius, TRACK, c.px(2.3));
            if shown > 0.9999 {
                c.circle_stroke(cx, cy, radius, color, c.px(2.3));
            } else if shown > 0.0001 {
                c.arc(
                    (cx, cy),
                    radius,
                    shown as f32 * std::f32::consts::TAU,
                    color,
                    c.px(2.3),
                );
            }
            // The time left, centered in the ring.
            let dh = c.line_h(29.0, Weight::Regular);
            let lh = c.line_h(10.0, Weight::Regular);
            let block = dh + c.px(4.0) + lh;
            let top = cy - block / 2.0;
            let dw = c.text_tabular_w(&digits, 29.0, Weight::Regular);
            c.text_tabular(cx - dw / 2.0, top, &digits, 29.0, Weight::Regular, th.text);
            let tw = c.text_w(&duration_text, 10.0, Weight::Regular);
            c.text(
                cx - tw / 2.0,
                top + dh + c.px(4.0),
                &duration_text,
                10.0,
                Weight::Regular,
                th.text_dim,
                Some(ring.w - c.px(16.0)),
            );
        }
        y += ring_h + c.px(14.0);

        // ---- primary and reset ----------------------------------------------------------
        let size = c.px(36.0);
        let total = size * 2.0 + c.px(12.0);
        let ax = rect.x + (rect.w - total) / 2.0;
        let primary = Rect::new(ax, y, size, size);
        let reset = Rect::new(ax + size + c.px(12.0), y, size, size);
        let can_start = active || valid;
        let icon = if state.running() {
            Icon::Pause
        } else {
            Icon::Play
        };
        let b = c.icon_button(
            PRIMARY,
            primary,
            icon,
            17.0,
            Some(th.accent),
            naby_core::color::Rgba::BLACK,
            can_start,
        );
        if b.clicked {
            self.activate(c);
        }
        let can_reset = active || completed;
        let r = c.button(RESET, reset, can_reset);
        c.button_chrome(reset, 18.0, None, can_reset, &r);
        let a = if can_reset { 1.0 } else { 0.4 };
        c.faded(a, |c| {
            c.rrect_stroke(reset, 18.0, RESET_BORDER, 1.0);
            let (cx, cy) = reset.center();
            c.icon_at(Icon::Reset, cx, cy, 17.0, th.text_dim);
        });
        if r.clicked {
            c.push(if self.focus {
                Command::FocusReset
            } else {
                Command::TimerReset
            });
        }
        y += size + c.px(18.0);

        // ---- editors ----------------------------------------------------------------------
        let editable = !active;
        let two = self.focus;
        let gap = c.px(8.0);
        let col_w = if two { (rect.w - gap) / 2.0 } else { rect.w };
        let label_h = c.line_h(10.0, Weight::Regular);
        let box_h = c.px(32.0 + 1.4);
        let mut committed = false;
        for i in 0..(if two { 2 } else { 1 }) {
            let x = rect.x + i as f32 * (col_w + gap);
            let (label, minus, plus, field, maximum, minimum) = match (self.focus, i) {
                (false, _) => ("Duration · min", MINUS_A, PLUS_A, FIELD_A, 1440.0, 0.1),
                (true, 0) => ("Focus · min", MINUS_A, PLUS_A, FIELD_A, 240.0, 1.0),
                _ => ("Break · min", MINUS_B, PLUS_B, FIELD_B, 120.0, 1.0),
            };
            c.text(
                x,
                y,
                c.tr(label),
                10.0,
                Weight::Regular,
                th.text_dim,
                Some(col_w),
            );
            let by = y + label_h + c.px(7.0);
            let frame = Rect::new(x, by, col_w, box_h);
            c.faded(if editable { 1.0 } else { 0.45 }, |c| {
                c.rrect(frame, 8.0, FIELD_FILL);
                c.rrect_stroke(frame, 8.0, FIELD_BORDER, 0.7);
            });
            let inner = frame.inset(c.px(0.7));
            let bw = c.px(28.0);
            let minus_r = Rect::new(inner.x, inner.y + (inner.h - bw) / 2.0, bw, bw);
            let plus_r = Rect::new(inner.right() - bw, minus_r.y, bw, bw);
            let text_r = Rect::new(
                inner.x + bw,
                inner.y,
                (inner.w - 2.0 * bw).max(1.0),
                inner.h,
            );
            for (id, rr, ic, dir) in [
                (minus, minus_r, Icon::Minus, -1.0),
                (plus, plus_r, Icon::Plus, 1.0),
            ] {
                let b = c.icon_button(id, rr, ic, 12.0, None, th.text_dim, editable);
                if b.clicked {
                    let target = if i == 0 {
                        &mut self.minutes
                    } else {
                        &mut self.breaks
                    };
                    Self::step(target, dir, minimum, maximum);
                }
            }
            let opts = EditOpts {
                dip: 12.0,
                max_chars: 8,
                centered: true,
                accept: if self.focus {
                    digits_only
                } else {
                    decimal_chars
                },
            };
            let text = if i == 0 {
                &mut self.minutes
            } else {
                &mut self.breaks
            };
            let out = c.faded(if editable { 1.0 } else { 0.45 }, |c| {
                c.line_edit(field, text_r, text, "", &opts, editable)
            });
            if out.left || out.submitted {
                committed = true;
            }
        }
        y += label_h + c.px(7.0) + box_h;
        if committed {
            self.commit(c);
        }

        // ---- hint ----------------------------------------------------------------------------
        y += c.px(10.0);
        let (hint, hint_color) = if !active && !valid {
            (
                c.tr(if self.focus {
                    "Use 1–240 focus and 1–120 break minutes."
                } else {
                    "Enter 0.1–1440 minutes."
                })
                .to_owned(),
                ERROR,
            )
        } else if completed {
            (c.tr("Time is up.").to_owned(), th.text_dim)
        } else if self.focus {
            (
                naby_core::i18n::format(c.lang(), "{0} min break after focus.", &[&self.breaks]),
                th.text_dim,
            )
        } else {
            (String::new(), th.text_dim)
        };
        let hint_h = if hint.is_empty() {
            0.0
        } else {
            c.paragraph(rect.x, y, rect.w, &hint, 10.0, Weight::Regular, hint_color)
        };
        y += hint_h.max(c.px(20.0));
        y += c.px(4.0);

        // ---- schedule ---------------------------------------------------------------------------
        if !c.is_dry() {
            self.next = state
                .running()
                .then(|| c.now + remaining.fract().max(0.0).max(0.02) + 0.005);
            if self.next.is_some() && remaining.fract() == 0.0 {
                self.next = Some(c.now + 1.0);
            }
        }
        y - rect.y
    }

    /// Moves the drawn ring value toward `target` (220 ms ease-out for jumps, snapping tiny
    /// once-a-second steps).
    fn update_ring(&mut self, c: &Ctl, target: f64) {
        if (target - self.ring_target).abs() < 1e-9 {
            return;
        }
        let from = self.ring.value(c.now);
        let motion = MotionConfig::from_settings(c.ctx.settings(), true);
        self.ring_target = target;
        self.ring = if motion.enabled && (target - from).abs() > 0.02 {
            Tween::new(from, target, c.now, 0.22, Ease::CUBIC_OUT).with_fps(60.0)
        } else {
            Tween::instant(target)
        };
    }
}

impl Page for CountdownPage {
    fn id(&self) -> &'static str {
        if self.focus {
            "focus"
        } else {
            "timer"
        }
    }

    fn height(&mut self, ctx: &PageCtx, width: f32) -> f32 {
        self.st.sync(ctx);
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
        self.presence.mark(ctx.snaps.now);
        self.st.sync(ctx);
        let mut c = Ctl::draw(canvas, ctx, input, out, &mut self.ui);
        self.st.layout(&mut c, rect);
        let now = ctx.snaps.now;
        !self.st.ring.finished(now)
    }

    fn next_frame(&self, now: f64) -> Option<f64> {
        self.presence.observe(now);
        if !self.presence.active() {
            return None;
        }
        let blink = self.ui.next_frame(now);
        let ring = self.st.ring.next_frame(now);
        [self.st.next, blink, ring]
            .into_iter()
            .flatten()
            .reduce(f64::min)
    }

    fn captures_pointer(&self) -> bool {
        self.ui.captures()
    }

    fn on_hide(&mut self) {
        self.presence.hide();
        self.ui.reset();
        self.st.started = false;
        self.st.next = None;
        self.st.shown_run = 0;
        self.st.shown_duration = -1.0;
        self.st.ring = Tween::instant(0.0);
        self.st.ring_target = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pages::common::harness::Harness;
    use chrono::Duration;
    use naby_core::session::SessionState;

    fn run_timer(h: &mut Harness, seconds: f64, paused: bool) {
        let mut session = SessionState::default();
        let now = wall_of(h);
        session.timer.start(seconds, now, "", false);
        if paused {
            session.timer.pause(now);
        }
        h.snaps.session.replace(session);
    }

    fn wall_of(h: &Harness) -> DateTime<Utc> {
        h.snaps.wall.with_timezone(&Utc)
    }

    #[test]
    fn start_sends_the_edited_duration() {
        let Some(mut h) = Harness::new("timer") else {
            return;
        };
        h.idle();
        h.click_id(PRIMARY);
        assert_eq!(h.commands(), vec![Command::TimerStart { seconds: 300.0 }]);
    }

    #[test]
    fn steppers_edit_the_minutes_and_start_uses_them() {
        let Some(mut h) = Harness::new("timer") else {
            return;
        };
        h.idle();
        h.click_id(PLUS_A);
        h.click_id(PLUS_A);
        h.click_id(MINUS_A);
        h.click_id(PRIMARY);
        assert_eq!(h.commands(), vec![Command::TimerStart { seconds: 360.0 }]);
    }

    #[test]
    fn typed_minutes_start_a_custom_timer_and_invalid_text_blocks_start() {
        let Some(mut h) = Harness::new("timer") else {
            return;
        };
        h.idle();
        h.click_id(FIELD_A);
        h.frame(InputState::typed("12.5"));
        h.idle();
        h.click_id(PRIMARY);
        assert_eq!(h.commands(), vec![Command::TimerStart { seconds: 750.0 }]);
        // Out of range: the primary button is disabled and the hint says why.
        h.click_id(FIELD_A);
        h.frame(InputState::typed("9999"));
        h.idle();
        h.click_id(PRIMARY);
        assert!(h.commands().is_empty());
    }

    #[test]
    fn a_running_timer_pauses_and_a_paused_one_resumes() {
        let Some(mut h) = Harness::new("timer") else {
            return;
        };
        run_timer(&mut h, 120.0, false);
        h.idle();
        h.click_id(PRIMARY);
        assert_eq!(h.commands(), vec![Command::TimerPause]);
        run_timer(&mut h, 120.0, true);
        h.idle();
        h.click_id(PRIMARY);
        assert_eq!(h.commands(), vec![Command::TimerResume]);
        h.click_id(RESET);
        assert_eq!(h.commands(), vec![Command::TimerReset]);
    }

    #[test]
    fn the_minutes_field_edits_like_a_text_box() {
        use naby_render::{Key, KeyEvent};
        let Some(mut h) = Harness::new("timer") else {
            return;
        };
        h.idle();
        // Focusing selects everything; typing replaces it.
        h.click_id(FIELD_A);
        assert!(
            h.page.captures_pointer(),
            "a focused field holds the panel still"
        );
        h.frame(InputState::typed("7"));
        h.frame(InputState::typed("5"));
        h.frame(InputState::key(KeyEvent::new(Key::Backspace)));
        h.frame(InputState::typed("0"));
        h.idle();
        h.click_id(PRIMARY);
        assert_eq!(h.commands(), vec![Command::TimerStart { seconds: 4200.0 }]);
        // Letters are refused; an emptied field is invalid.
        h.click_id(FIELD_A);
        h.frame(InputState::typed("abc"));
        h.frame(InputState::key(KeyEvent::new(Key::Backspace)));
        h.idle();
        h.click_id(PRIMARY);
        assert!(h.commands().is_empty());
        // Pasting works and Ctrl+A selects.
        h.click_id(FIELD_A);
        h.frame(InputState {
            paste: Some("15".into()),
            ..InputState::default()
        });
        h.idle();
        h.click_id(PRIMARY);
        assert_eq!(h.commands(), vec![Command::TimerStart { seconds: 900.0 }]);
    }

    #[test]
    fn reset_is_disabled_while_idle() {
        let Some(mut h) = Harness::new("timer") else {
            return;
        };
        h.idle();
        h.click_id(RESET);
        assert!(h.commands().is_empty());
    }

    #[test]
    fn schedules_once_a_second_only_while_running_and_shown() {
        let Some(mut h) = Harness::new("timer") else {
            return;
        };
        h.idle();
        assert_eq!(h.page.next_frame(h.snaps.now), None);
        run_timer(&mut h, 120.0, false);
        h.idle();
        let at = h.page.next_frame(h.snaps.now).expect("ticking");
        assert!(at > h.snaps.now && at <= h.snaps.now + 1.1);
        run_timer(&mut h, 120.0, true);
        h.idle();
        assert_eq!(h.page.next_frame(h.snaps.now), None, "paused is static");
        run_timer(&mut h, 120.0, false);
        h.idle();
        assert_eq!(
            h.page.next_frame(h.snaps.now + 30.0),
            None,
            "hidden pages go quiet"
        );
    }

    #[test]
    fn focus_start_saves_edited_preferences_first() {
        let Some(mut h) = Harness::new("focus") else {
            return;
        };
        h.idle();
        h.click_id(FIELD_A);
        h.frame(InputState::typed("40"));
        h.idle();
        h.click_id(PRIMARY);
        let commands = h.commands();
        assert!(
            matches!(&commands[0], Command::ApplySettings(s) if s.focus_minutes == 40 && s.break_minutes == 5)
        );
        assert_eq!(commands.last(), Some(&Command::FocusToggle));
    }

    #[test]
    fn focus_without_edits_just_toggles() {
        let Some(mut h) = Harness::new("focus") else {
            return;
        };
        h.idle();
        h.click_id(PRIMARY);
        assert_eq!(h.commands(), vec![Command::FocusToggle]);
        h.click_id(MINUS_B);
        h.click_id(RESET);
        assert!(h.commands().is_empty(), "reset needs an active session");
    }

    #[test]
    fn editors_are_locked_while_a_countdown_runs() {
        let Some(mut h) = Harness::new("timer") else {
            return;
        };
        run_timer(&mut h, 120.0, false);
        h.idle();
        assert!(!h.is_enabled(PLUS_A));
    }

    #[test]
    fn snapshots_of_each_state() {
        for (id, name) in [("timer", "page-timer"), ("focus", "page-focus")] {
            let Some(mut h) = Harness::new(id) else {
                return;
            };
            h.idle();
            assert!(h.ink() > 2000);
            h.save(&format!("{name}-idle"));
            let mut session = SessionState::default();
            let now = wall_of(&h);
            if id == "timer" {
                session
                    .timer
                    .start(600.0, now - Duration::seconds(222), "", false);
            } else {
                session
                    .focus
                    .countdown
                    .start(1500.0, now - Duration::seconds(400), "work", false);
            }
            h.snaps.session.replace(session);
            h.idle();
            h.advance(1.0);
            h.idle();
            h.save(&format!("{name}-running"));
        }
    }
}
