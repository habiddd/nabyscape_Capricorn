//! The Naby character: a round face with two eyes (`AssistantOrb.cs`) and its idle motion
//! (`AssistantIdleMotion.cs`).
//!
//! Everything is sampled from the clock: [`Orb::tick`] moves a handful of eased values toward
//! their targets and [`Orb::paint`] draws the pose for `now`. Nothing runs by itself, so a
//! hidden orb costs nothing; [`Orb::next_frame`] tells the host when (and whether) the next
//! frame is due.

use naby_core::assistant::State;
use naby_core::color::Rgba;
use naby_render::canvas::{circle_path, round_rect_path};
use naby_render::icons::parse_path;
use naby_render::{Canvas, Fill, Rect};
use tiny_skia::{FillRule, Path, Transform};

const BODY: Rgba = Rgba::rgb(0x23, 0x26, 0x2D);
const EYES: Rgba = Rgba::rgb(0xFA, 0xFB, 0xFC);
const EDGE: Rgba = Rgba::rgb(0x77, 0x7D, 0x88);
const ERROR_EDGE: Rgba = Rgba::rgb(0xD7, 0x96, 0x96);
const APPROVAL_EDGE: Rgba = Rgba::rgb(0xCD, 0xBB, 0x8E);

/// Frame interval while the face animates (live states, easing).
const LIVE_STEP: f64 = 1.0 / 30.0;
/// Frame interval of the idle breathing: slow enough to stay cheap, fine for ~5 s cycles.
const IDLE_STEP: f64 = 1.0 / 20.0;

/// `AssistantIdleMotion.Sine`: 0 -> 1 -> 0 over two half periods.
pub fn idle_sine(seconds: f64, half_period: f64) -> f64 {
    (1.0 - (std::f64::consts::PI * (seconds % (half_period * 2.0)) / half_period).cos()) / 2.0
}

/// Vertical float of the idle pose.
pub fn idle_float(seconds: f64, music: bool) -> f64 {
    -3.5 * idle_sine(seconds, if music { 0.65 } else { 2.1 })
}

/// The retained idle pose: breathing scale, sway angle (degrees) and float, about (50, 50).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IdlePose {
    pub scale: f64,
    pub angle: f64,
    pub float: f64,
}

pub fn idle_pose(seconds: f64, music: bool) -> IdlePose {
    IdlePose {
        scale: 1.0 + 0.035 * idle_sine(seconds, if music { 0.7 } else { 2.4 }),
        angle: (if music { -5.0 } else { -3.0 })
            * idle_sine(seconds, if music { 1.1 } else { 3.4 }),
        float: idle_float(seconds, music),
    }
}

/// A value that eases toward a target (`Motion.To`): after `duration` it is within ~5%.
#[derive(Debug, Clone, Copy)]
struct Ease {
    cur: f32,
    target: f32,
    tau: f32,
}

impl Ease {
    fn new(value: f32) -> Self {
        Self {
            cur: value,
            target: value,
            tau: 0.06,
        }
    }

    fn to(&mut self, target: f32, millis: f32) {
        self.target = target;
        self.tau = (millis / 1000.0 / 3.0).max(0.001);
    }

    fn snap(&mut self, value: f32) {
        self.cur = value;
        self.target = value;
    }

    fn step(&mut self, dt: f32) {
        self.cur += (self.target - self.cur) * (1.0 - (-dt / self.tau).exp());
        if (self.target - self.cur).abs() < 0.002 {
            self.cur = self.target;
        }
    }

    fn settled(&self) -> bool {
        self.cur == self.target
    }
}

/// `EyeScale`: how tall the eyes are for a state.
fn eye_scale(state: State, hovered: bool) -> f32 {
    (match state {
        State::Listening => 34.0,
        State::Thinking => 25.0,
        State::Error => 23.0,
        _ if hovered => 34.0,
        _ => 30.0,
    }) / 30.0
}

/// A keyframed bump: `start -> middle` (ease out, 42%) `-> end` (ease in-out).
fn bump(start: f32, middle: f32, end: f32, progress: f32) -> f32 {
    let p = progress.clamp(0.0, 1.0);
    let ease_out = |t: f32| 1.0 - (1.0 - t).powi(3);
    let ease_in_out = |t: f32| {
        if t < 0.5 {
            4.0 * t * t * t
        } else {
            1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
        }
    };
    if p < 0.42 {
        start + (middle - start) * ease_out(p / 0.42)
    } else {
        middle + (end - middle) * ease_in_out((p - 0.42) / 0.58)
    }
}

pub struct Orb {
    state: State,
    live: bool,
    music: bool,
    reduce_motion: bool,
    visible: bool,
    level: f32,
    display_level: f32,
    hovered: bool,
    pressed: bool,
    look_x: f32,
    gaze_x: Ease,
    gaze_y: Ease,
    lean: Ease,
    lift: Ease,
    squash_x: Ease,
    squash_y: Ease,
    expression: Ease,
    headphones: Ease,
    last: Option<f64>,
    /// Clock origin of the idle pose.
    origin: Option<f64>,
    /// Seconds spent in live (animated) states.
    phase: f32,
    blinks: u32,
    next_blink: Option<f64>,
    blink: Option<(f64, bool)>,
    bounce: Option<(f64, bool)>,
    glance: Option<(f64, f32)>,
    band: Option<Path>,
    /// Frames per second the host allows the face (0 = no cap beyond its own steps).
    frame_cap: u32,
}

impl Default for Orb {
    fn default() -> Self {
        Self::new()
    }
}

impl Orb {
    pub fn new() -> Self {
        Self {
            state: State::Idle,
            live: false,
            music: false,
            reduce_motion: false,
            visible: true,
            level: 0.0,
            display_level: 0.0,
            hovered: false,
            pressed: false,
            look_x: 0.0,
            gaze_x: Ease::new(0.0),
            gaze_y: Ease::new(0.0),
            lean: Ease::new(0.0),
            lift: Ease::new(0.0),
            squash_x: Ease::new(1.0),
            squash_y: Ease::new(1.0),
            expression: Ease::new(1.0),
            headphones: Ease::new(0.0),
            last: None,
            origin: None,
            phase: 0.0,
            blinks: 0,
            next_blink: None,
            blink: None,
            bounce: None,
            glance: None,
            band: parse_path("M12,50 L12,43 C12,-3 88,-3 88,43 L88,50"),
            frame_cap: 0,
        }
    }

    /// Caps the animation at `fps` frames per second (`FrameBudget::limit`; "Hemat" is 30).
    /// 0 removes the cap. The face never runs faster than its own 30 fps live step.
    pub fn set_frame_cap(&mut self, fps: u32) {
        self.frame_cap = fps;
    }

    /// The interval between animation frames given the face's own step and the cap.
    pub fn step(&self, own: f64) -> f64 {
        if self.frame_cap == 0 {
            own
        } else {
            own.max(1.0 / f64::from(self.frame_cap.clamp(1, 240)))
        }
    }

    // ---- inputs -------------------------------------------------------------------------

    pub fn state(&self) -> State {
        self.state
    }

    /// Changes the expression; a visible orb blinks and bounces like the original.
    pub fn set_state(&mut self, state: State, now: f64) {
        if self.state == state {
            return;
        }
        self.state = state;
        self.expression.to(eye_scale(state, self.hovered), 220.0);
        if self.animated() {
            self.blink = Some((now, state == State::Approval));
            self.bounce = Some((now, state == State::Error));
        } else {
            self.expression.snap(eye_scale(state, self.hovered));
        }
    }

    /// The face shows a live-connection dot (voice connected, wake listening).
    pub fn set_live(&mut self, live: bool) {
        self.live = live;
    }

    /// Headphones appear while music plays.
    pub fn set_music(&mut self, music: bool) {
        if self.music == music {
            return;
        }
        self.music = music;
        self.origin = None;
        let target = if music { 1.0 } else { 0.0 };
        if self.animated() {
            self.headphones.to(target, 260.0);
        } else {
            self.headphones.snap(target);
        }
    }

    pub fn set_reduce_motion(&mut self, reduce: bool) {
        if self.reduce_motion != reduce {
            self.reduce_motion = reduce;
            if reduce {
                self.reset_interaction();
            }
        }
    }

    /// Shown or hidden; a hidden orb asks for no frames.
    pub fn set_visible(&mut self, visible: bool) {
        if self.visible != visible {
            self.visible = visible;
            self.last = None;
            self.next_blink = None;
            self.origin = None;
            if !visible {
                self.reset_interaction();
            }
        }
    }

    /// Voice level 0..=1 (drives the eyes while listening or speaking).
    pub fn set_level(&mut self, level: f32) {
        self.level = if level.is_finite() {
            level.clamp(0.0, 1.0)
        } else {
            0.0
        };
    }

    fn animated(&self) -> bool {
        self.visible && !self.reduce_motion
    }

    pub fn set_hover(&mut self, hovered: bool) {
        if self.hovered == hovered {
            return;
        }
        self.hovered = hovered;
        let motion = self.animated();
        let scale = if hovered { 1.06 } else { 1.0 };
        if !self.pressed {
            Self::go(&mut self.squash_x, scale, 180.0, motion);
            Self::go(&mut self.squash_y, scale, 180.0, motion);
        }
        Self::go(
            &mut self.lift,
            if hovered { -2.0 } else { 0.0 },
            180.0,
            motion,
        );
        let eyes = eye_scale(self.state, hovered);
        Self::go(&mut self.expression, eyes, 140.0, motion);
    }

    pub fn set_pressed(&mut self, pressed: bool, now: f64) {
        if self.pressed == pressed {
            return;
        }
        self.pressed = pressed;
        let motion = self.animated();
        if pressed {
            Self::go(&mut self.squash_x, 1.07, 90.0, motion);
            Self::go(&mut self.squash_y, 0.94, 90.0, motion);
            if motion {
                self.blink = Some((now, true));
            }
        } else if motion {
            self.bounce = Some((now, false));
            let hover = if self.hovered { 1.06 } else { 1.0 };
            self.squash_x.to(hover, 120.0);
            self.squash_y.to(hover, 120.0);
        } else {
            self.squash_x.snap(1.0);
            self.squash_y.snap(1.0);
        }
    }

    fn go(ease: &mut Ease, target: f32, millis: f32, motion: bool) {
        if motion {
            ease.to(target, millis);
        } else {
            ease.snap(target);
        }
    }

    /// Looks toward a point; `x` and `y` are -1..=1 relative to the face.
    pub fn look_at(&mut self, x: f32, y: f32) {
        let x = if x.is_finite() {
            x.clamp(-1.0, 1.0)
        } else {
            0.0
        };
        let y = if y.is_finite() {
            y.clamp(-1.0, 1.0)
        } else {
            0.0
        };
        self.look_x = x;
        let motion = self.animated();
        Self::go(&mut self.gaze_x, x * 6.0, 120.0, motion);
        Self::go(&mut self.gaze_y, y * 4.0, 120.0, motion);
        Self::go(&mut self.lean, x * 5.0, 180.0, motion);
    }

    fn reset_interaction(&mut self) {
        self.hovered = false;
        self.pressed = false;
        self.look_x = 0.0;
        self.blink = None;
        self.bounce = None;
        self.glance = None;
        for ease in [
            &mut self.gaze_x,
            &mut self.gaze_y,
            &mut self.lean,
            &mut self.lift,
        ] {
            ease.snap(0.0);
        }
        self.squash_x.snap(1.0);
        self.squash_y.snap(1.0);
        self.expression.snap(eye_scale(self.state, false));
        self.headphones.snap(if self.music { 1.0 } else { 0.0 });
    }

    // ---- time ---------------------------------------------------------------------------

    fn live_state(&self) -> bool {
        matches!(
            self.state,
            State::Connecting | State::Listening | State::Thinking | State::Speaking
        )
    }

    fn idle_motion(&self) -> bool {
        self.animated() && self.state == State::Idle && !self.hovered && !self.pressed
    }

    /// Advances to `now`. Returns true while another frame is needed right away.
    pub fn tick(&mut self, now: f64) -> bool {
        if !self.visible {
            self.last = None;
            return false;
        }
        let dt = self.last.map_or(0.0, |l| (now - l).clamp(0.0, 0.05)) as f32;
        self.last = Some(now);
        if self.reduce_motion {
            return false;
        }
        if self.origin.is_none() {
            self.origin = Some(now);
        }
        if self.live_state() {
            self.phase += dt;
        }
        self.display_level += (self.level - self.display_level) * (1.0 - (-dt * 16.0).exp());
        for ease in [
            &mut self.gaze_x,
            &mut self.gaze_y,
            &mut self.lean,
            &mut self.lift,
            &mut self.squash_x,
            &mut self.squash_y,
            &mut self.expression,
            &mut self.headphones,
        ] {
            ease.step(dt);
        }
        // The blink "timer": 2.2 s plus a rotating 0..1.65 s, with an occasional glance.
        let due = *self.next_blink.get_or_insert(now + 1.6);
        if now >= due {
            self.blink = Some((now, false));
            self.blinks += 1;
            self.next_blink = Some(now + 2.2 + f64::from(self.blinks % 4) * 0.55);
            if self.state == State::Idle
                && !self.hovered
                && self.look_x.abs() < 0.05
                && self.blinks.is_multiple_of(2)
            {
                let direction = if self.blinks.is_multiple_of(4) {
                    1.0
                } else {
                    -1.0
                };
                self.glance = Some((now + 0.13, direction));
            }
        }
        if self.blink.is_some_and(|(at, _)| now - at > 0.3) {
            self.blink = None;
        }
        if self.bounce.is_some_and(|(at, _)| now - at > 0.35) {
            self.bounce = None;
        }
        if self.glance.is_some_and(|(at, _)| now - at > 1.0) {
            self.glance = None;
        }
        self.frames_needed()
    }

    fn frames_needed(&self) -> bool {
        self.live_state()
            || self.idle_motion()
            || self.blink.is_some()
            || self.bounce.is_some()
            || self.glance.is_some()
            || ![
                &self.gaze_x,
                &self.gaze_y,
                &self.lean,
                &self.lift,
                &self.squash_x,
                &self.squash_y,
                &self.expression,
                &self.headphones,
            ]
            .iter()
            .all(|e| e.settled())
    }

    /// When the host should tick and paint next; `None` when the face is static or hidden.
    pub fn next_frame(&self, now: f64) -> Option<f64> {
        if !self.animated() {
            return None;
        }
        if self.live_state()
            || self.blink.is_some()
            || self.bounce.is_some()
            || self.glance.is_some()
        {
            return Some(now + self.step(LIVE_STEP));
        }
        if self.frames_needed() {
            let step = if self.idle_motion() {
                IDLE_STEP
            } else {
                LIVE_STEP
            };
            return Some(now + self.step(step));
        }
        self.next_blink.map(|t| t.max(now))
    }

    /// Whether the face is moving by itself (breathing or a live state).
    pub fn animating(&self) -> bool {
        self.animated() && (self.idle_motion() || self.live_state())
    }

    // ---- painting -----------------------------------------------------------------------

    /// Draws the face centered in the square that fits `rect`.
    pub fn paint(&self, canvas: &mut Canvas, rect: Rect, now: f64) {
        let size = rect.w.min(rect.h);
        if size <= 0.0 {
            return;
        }
        let moving = self.animated() && self.live_state();
        let t = if moving { self.phase } else { 0.0 };
        let energy = if moving && matches!(self.state, State::Listening | State::Speaking) {
            self.display_level
        } else {
            0.0
        };
        let bob = if moving {
            (t * if self.state == State::Thinking {
                3.0
            } else {
                2.0
            })
            .sin()
                * 1.2
        } else {
            0.0
        };
        let viewport = Transform::from_translate(
            rect.x + (rect.w - size) / 2.0,
            rect.y + (rect.h - size) / 2.0,
        )
        .pre_scale(size / 100.0, size / 100.0);
        let about = |cx: f32, cy: f32, t: Transform| {
            Transform::from_translate(cx, cy)
                .pre_concat(t)
                .pre_translate(-cx, -cy)
        };
        let idle = if self.idle_motion() {
            let seconds = now - self.origin.unwrap_or(now);
            let pose = idle_pose(seconds, self.music);
            about(50.0, 50.0, Transform::from_rotate(pose.angle as f32))
                .pre_concat(about(
                    50.0,
                    50.0,
                    Transform::from_scale(pose.scale as f32, pose.scale as f32),
                ))
                .post_concat(Transform::from_translate(0.0, pose.float as f32))
        } else {
            Transform::identity()
        };
        // Bounce on a state change or after a press; shake on error.
        let (bounce, shake) = match self.bounce {
            Some((at, shake)) => {
                let p = ((now - at) / if shake { 0.3 } else { 0.28 }) as f32;
                let target = if self.hovered { 1.06 } else { 1.0 };
                (
                    bump(1.0, 1.085, target, p),
                    if shake {
                        bump(0.0, 7.0, self.look_x * 5.0, p)
                    } else {
                        0.0
                    },
                )
            }
            None => (1.0, 0.0),
        };
        let squash_x = self.squash_x.cur * bounce;
        let squash_y = self.squash_y.cur * bounce;
        let thinking_tilt = if self.state == State::Thinking && moving {
            (t * 2.3).sin() * 5.0
        } else {
            0.0
        };
        let (glance_x, glance_y, glance_lean) = match self.glance {
            Some((at, direction)) if now >= at => {
                let p = ((now - at) / 0.85) as f32;
                (
                    bump(0.0, 4.0 * direction, 0.0, p),
                    bump(0.0, -1.5, 0.0, p),
                    bump(0.0, 3.0 * direction, 0.0, ((now - at) / 0.95) as f32),
                )
            }
            _ => (0.0, 0.0, 0.0),
        };
        let lean = self.lean.cur + shake + glance_lean;
        let body_t = viewport
            .pre_concat(idle)
            .pre_translate(0.0, self.lift.cur)
            .pre_concat(about(50.0, 50.0, Transform::from_rotate(lean)))
            .pre_concat(about(50.0, 50.0, Transform::from_scale(squash_x, squash_y)))
            .pre_concat(about(50.0, 50.0, Transform::from_rotate(thinking_tilt)));

        let radius = 43.0 * (1.0 + energy * 0.018);
        let edge = match self.state {
            State::Error => ERROR_EDGE,
            State::Approval => APPROVAL_EDGE,
            _ => EDGE,
        };
        let stroke_width = (65.0 / size).max(0.85);
        if let Some(body) = circle_path(50.0, 50.0 + bob, radius) {
            canvas.fill_path_with(&body, &Fill::Solid(BODY), FillRule::Winding, body_t);
            canvas.stroke_path(&body, &Fill::Solid(edge), stroke_width, body_t);
        }
        let reveal = self.headphones.cur;
        if reveal > 0.01 {
            let phones =
                body_t.pre_concat(about(50.0, 50.0, Transform::from_scale(reveal, reveal)));
            if let Some(band) = &self.band {
                canvas.stroke_path(band, &Fill::Solid(EYES), 5.0, phones);
            }
            for x in [5.0, 81.0] {
                if let Some(cup) = round_rect_path(Rect::new(x, 43.0, 14.0, 27.0), 6.0) {
                    canvas.fill_path_with(&cup, &Fill::Solid(edge), FillRule::Winding, phones);
                    canvas.stroke_path(&cup, &Fill::Solid(EYES), 1.5, phones);
                }
            }
        }
        // Eyes: tilt -> lid -> expression -> gaze, all inside the body transform.
        let gaze =
            Transform::from_translate(self.gaze_x.cur + glance_x, self.gaze_y.cur + glance_y);
        let searching = if self.state == State::Thinking && moving {
            (t * 2.3).sin() * 4.0
        } else {
            0.0
        };
        let (left_lid, right_lid) = self.lids(now);
        for i in 0..2 {
            let x = if i == 0 { 35.0 } else { 65.0 };
            let y = 41.0
                + bob
                + if self.state == State::Approval {
                    if i == 0 {
                        -3.0
                    } else {
                        3.0
                    }
                } else {
                    0.0
                };
            let height = 30.0
                + if self.state == State::Speaking && moving {
                    (t * 11.0 + i as f32 * 0.9).sin() * energy * 8.0 + energy * 5.0
                } else {
                    0.0
                };
            let tilt = if self.state == State::Error {
                if i == 0 {
                    -18.0
                } else {
                    18.0
                }
            } else {
                -8.0 + if self.state == State::Thinking {
                    searching
                } else {
                    0.0
                }
            };
            let lid = if i == 0 { left_lid } else { right_lid };
            let eye_t = body_t
                .pre_concat(gaze)
                .pre_concat(about(
                    50.0,
                    41.0,
                    Transform::from_scale(1.0, self.expression.cur),
                ))
                .pre_concat(about(x, 41.0, Transform::from_scale(1.0, lid)))
                .pre_concat(about(x, y, Transform::from_rotate(tilt)));
            if let Some(eye) = round_rect_path(
                Rect::new(x - 8.0 + searching, y - height / 2.0, 16.0, height),
                8.0,
            ) {
                canvas.fill_path_with(&eye, &Fill::Solid(EYES), FillRule::Winding, eye_t);
            }
        }
        if self.live {
            let dot_t = viewport;
            if let Some(dot) = circle_path(83.0, 83.0, 6.5) {
                canvas.fill_path_with(&dot, &Fill::Solid(EYES), FillRule::Winding, dot_t);
                canvas.stroke_path(&dot, &Fill::Solid(BODY), (110.0 / size).max(2.0), dot_t);
            }
        }
    }

    /// Eyelid scale of both eyes at `now` (the right eye lags by 18 ms and a wink skips it).
    fn lids(&self, now: f64) -> (f32, f32) {
        let Some((at, wink)) = self.blink else {
            return (1.0, 1.0);
        };
        let lid = |delay: f64| {
            let p = ((now - at - delay) / 0.24) as f32;
            if !(0.0..=1.0).contains(&p) {
                1.0
            } else {
                bump(1.0, 0.06, 1.0, p)
            }
        };
        (lid(0.0), if wink { 1.0 } else { lid(0.018) })
    }

    /// The eyes' current gaze offset (diagnostics, tests).
    pub fn gaze(&self) -> (f32, f32) {
        (self.gaze_x.cur, self.gaze_y.cur)
    }

    pub fn pressed(&self) -> bool {
        self.pressed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use naby_render::snapshot::count_differing;

    fn painted(orb: &Orb, now: f64) -> Canvas {
        let mut canvas = Canvas::new(120, 120);
        canvas.clear(Rgba::rgb(0, 0, 0));
        orb.paint(&mut canvas, Rect::new(10.0, 10.0, 100.0, 100.0), now);
        canvas
    }

    #[test]
    fn idle_pose_breathes_floats_and_sways_on_the_original_periods() {
        let rest = idle_pose(0.0, false);
        assert_eq!((rest.scale, rest.angle, rest.float), (1.0, 0.0, 0.0));
        // Half a scale period (2.4 s) is the peak; the float peaks at 2.1 s.
        let peak = idle_pose(2.4, false);
        assert!((peak.scale - 1.035).abs() < 1e-9);
        assert!((idle_float(2.1, false) + 3.5).abs() < 1e-9);
        assert!((idle_pose(3.4, false).angle + 3.0).abs() < 1e-9);
        // Music plays the faster, larger variant.
        assert!((idle_pose(1.1, true).angle + 5.0).abs() < 1e-9);
        assert!((idle_float(0.65, true) + 3.5).abs() < 1e-9);
        // Periodic.
        assert!((idle_pose(4.8, false).scale - 1.0).abs() < 1e-9);
    }

    #[test]
    fn the_face_paints_circle_and_eyes() {
        let mut orb = Orb::new();
        orb.set_reduce_motion(true);
        let canvas = painted(&orb, 0.0);
        let background = Rgba::rgb(0, 0, 0);
        // Body pixels around the center, bright eye pixels left and right of it.
        assert_ne!(canvas.pixel(60, 90), background);
        let eye = canvas.pixel(10 + 35, 10 + 41);
        assert!(eye.r > 0xE0 && eye.g > 0xE0, "left eye {eye:?}");
        let eye = canvas.pixel(10 + 65, 10 + 41);
        assert!(eye.r > 0xE0, "right eye {eye:?}");
        // The corner stays untouched.
        assert_eq!(canvas.pixel(11, 11), background);
        assert!(count_differing(&canvas, Rect::new(0.0, 0.0, 120.0, 120.0), background) > 5000);
    }

    #[test]
    fn expressions_change_the_picture() {
        let mut idle = Orb::new();
        idle.set_reduce_motion(true);
        let mut error = Orb::new();
        error.set_reduce_motion(true);
        error.set_state(State::Error, 0.0);
        let a = painted(&idle, 0.0);
        let b = painted(&error, 0.0);
        assert_ne!(a.data(), b.data());
        // Live dot only when live.
        let mut live = Orb::new();
        live.set_reduce_motion(true);
        live.set_live(true);
        assert_ne!(painted(&live, 0.0).data(), a.data());
    }

    #[test]
    fn idle_motion_is_sampled_from_time_and_stops_when_hidden_or_reduced() {
        let mut orb = Orb::new();
        orb.tick(0.0);
        let first = painted(&orb, 0.0);
        orb.tick(1.0);
        let later = painted(&orb, 2.1);
        assert_ne!(first.data(), later.data(), "the pose moves with the clock");
        assert!(orb.animating());
        let next = orb.next_frame(2.1).unwrap();
        assert!(next > 2.1 && next < 2.2);

        orb.set_visible(false);
        assert_eq!(orb.next_frame(3.0), None);
        assert!(!orb.tick(3.0));
        orb.set_visible(true);
        assert!(orb.next_frame(4.0).is_some());

        orb.set_reduce_motion(true);
        assert_eq!(orb.next_frame(5.0), None);
        let still_a = painted(&orb, 5.0);
        let still_b = painted(&orb, 9.0);
        assert_eq!(still_a.data(), still_b.data());
    }

    #[test]
    fn hover_interrupts_the_idle_loop_and_settles_without_more_frames() {
        let mut orb = Orb::new();
        orb.tick(0.0);
        orb.set_hover(true);
        let mut now = 0.0;
        for _ in 0..60 {
            now += 1.0 / 30.0;
            orb.tick(now);
        }
        assert!(!orb.animating());
        // Settled: the next frame is only the next blink, not a render loop.
        let next = orb.next_frame(now).unwrap();
        assert!(next - now > 0.05 || orb.blink.is_some());
    }

    #[test]
    fn looking_moves_the_gaze_within_bounds() {
        let mut orb = Orb::new();
        orb.look_at(5.0, -5.0);
        let mut now = 0.0;
        for _ in 0..30 {
            now += 0.016;
            orb.tick(now);
        }
        let (x, y) = orb.gaze();
        assert!((x - 6.0).abs() < 0.1 && (y + 4.0).abs() < 0.1, "{x} {y}");
        orb.look_at(f32::NAN, 0.0);
        assert_eq!(orb.look_x, 0.0);
    }

    #[test]
    fn listening_state_animates_with_the_voice_level() {
        let mut orb = Orb::new();
        orb.tick(0.0);
        orb.set_state(State::Listening, 0.0);
        orb.set_level(1.0);
        let mut now = 0.0;
        for _ in 0..40 {
            now += 1.0 / 30.0;
            orb.tick(now);
        }
        assert!(orb.animating());
        assert_eq!(orb.next_frame(now), Some(now + LIVE_STEP));
        let loud = painted(&orb, now);
        orb.set_level(0.0);
        for _ in 0..60 {
            now += 1.0 / 30.0;
            orb.tick(now);
        }
        assert_ne!(painted(&orb, now).data(), loud.data());
    }
}
