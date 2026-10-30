//! Control feedback targets (C# `Motion.Button`, `ControlFeedback`): the numbers views animate
//! towards on hover / press, and the notice entrance.

use super::easing::{Ease, Interaction, MotionConfig, Tween};

/// Scale and lift a button settles at, with the durations to get there.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ButtonFeedback {
    pub scale: f64,
    pub scale_ms: f64,
    /// Vertical translation in DIP (negative = up).
    pub lift: f64,
    pub lift_ms: f64,
}

/// C# `Motion.Button` state function.
pub fn button_feedback(
    cfg: &MotionConfig,
    control_enabled: bool,
    hover: bool,
    pressed: bool,
) -> ButtonFeedback {
    let active = cfg.interactive() && control_enabled;
    let expressive = cfg.interaction == Interaction::Expressive;
    let scale = if !active {
        1.0
    } else if pressed {
        if expressive {
            0.94
        } else {
            0.965
        }
    } else if hover {
        if expressive {
            1.035
        } else {
            1.018
        }
    } else {
        1.0
    };
    let lift = if active && hover && !pressed {
        if expressive {
            -1.5
        } else {
            -0.6
        }
    } else {
        0.0
    };
    ButtonFeedback {
        scale,
        scale_ms: if pressed { 100.0 } else { 220.0 },
        lift,
        lift_ms: 180.0,
    }
}

/// Slider thumb scale target and duration (ms): 1.16 on hover, 1.23 while dragging.
pub fn thumb_feedback(cfg: &MotionConfig, hover: bool, dragging: bool) -> (f64, f64) {
    let scale = if !cfg.interactive() {
        1.0
    } else if dragging {
        1.23
    } else if hover {
        1.16
    } else {
        1.0
    };
    (scale, 150.0)
}

/// Cover artwork scale target and duration (ms): 1.045 on hover, 0.98 while pressed.
pub fn artwork_feedback(cfg: &MotionConfig, hover: bool, pressed: bool) -> (f64, f64) {
    let scale = if !cfg.interactive() {
        1.0
    } else if pressed {
        0.98
    } else if hover {
        1.045
    } else {
        1.0
    };
    (scale, 280.0)
}

/// Toggle knob slide when a check box flips: starts offset by 12 DIP (plus any unfinished
/// offset) on the side it came from and eases to rest over 220 ms.
pub fn toggle_knob(
    cfg: &MotionConfig,
    now: f64,
    checked: bool,
    previous_offset: f64,
) -> Tween<f64> {
    if !cfg.interactive() {
        return Tween::instant(0.0);
    }
    let start = if checked { -12.0 } else { 12.0 } + previous_offset;
    cfg.to(start, 0.0, now, 220.0, 0.0)
}

/// Toggle ring pulse: scale 0.84 → 1 over 220 ms.
pub fn toggle_ring(cfg: &MotionConfig, now: f64) -> Tween<f64> {
    if !cfg.interactive() {
        return Tween::instant(1.0);
    }
    cfg.to(0.84, 1.0, now, 220.0, 0.0)
}

/// Entrance of a notice card.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NoticeEntry {
    pub opacity: Tween<f64>,
    pub offset_y: Tween<f64>,
    /// Uniform scale about the top-centre.
    pub scale: Tween<f64>,
}

impl NoticeEntry {
    pub fn next_frame(&self, now: f64) -> Option<f64> {
        [
            self.opacity.next_frame(now),
            self.offset_y.next_frame(now),
            self.scale.next_frame(now),
        ]
        .into_iter()
        .flatten()
        .reduce(f64::min)
    }
}

/// C# `ControlFeedback.Notice`: `mode` is `NotificationAnimation` ("Lift", "Fade", otherwise
/// the scale-in default).
pub fn notice_entry(cfg: &MotionConfig, mode: &str, now: f64) -> NoticeEntry {
    let rest = NoticeEntry {
        opacity: Tween::instant(1.0),
        offset_y: Tween::instant(0.0),
        scale: Tween::instant(1.0),
    };
    if !cfg.enabled {
        return rest;
    }
    match mode {
        "Lift" => {
            let enter = cfg.enter(9.0, 0.0, now);
            NoticeEntry {
                opacity: enter.opacity,
                offset_y: enter.offset_y,
                ..rest
            }
        }
        "Fade" => NoticeEntry {
            opacity: Tween::new(0.0, 1.0, now, 0.26, Ease::CUBIC_OUT),
            ..rest
        },
        _ => NoticeEntry {
            opacity: Tween::new(0.0, 1.0, now, 0.24, Ease::CUBIC_OUT),
            scale: cfg.to(0.965, 1.0, now, 350.0, 0.0),
            ..rest
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(i: Interaction) -> MotionConfig {
        MotionConfig {
            interaction: i,
            ..MotionConfig::default()
        }
    }

    #[test]
    fn button_states_subtle_and_expressive() {
        let subtle = cfg(Interaction::Subtle);
        assert_eq!(button_feedback(&subtle, true, false, false).scale, 1.0);
        let hover = button_feedback(&subtle, true, true, false);
        assert_eq!(
            (hover.scale, hover.lift, hover.scale_ms),
            (1.018, -0.6, 220.0)
        );
        let press = button_feedback(&subtle, true, true, true);
        assert_eq!(
            (press.scale, press.lift, press.scale_ms),
            (0.965, 0.0, 100.0)
        );
        let expressive = cfg(Interaction::Expressive);
        assert_eq!(button_feedback(&expressive, true, true, false).scale, 1.035);
        assert_eq!(button_feedback(&expressive, true, true, false).lift, -1.5);
        assert_eq!(button_feedback(&expressive, true, false, true).scale, 0.94);
    }

    #[test]
    fn feedback_is_flat_when_off_disabled_or_reduced() {
        let flat = ButtonFeedback {
            scale: 1.0,
            scale_ms: 220.0,
            lift: 0.0,
            lift_ms: 180.0,
        };
        assert_eq!(
            button_feedback(&cfg(Interaction::Off), true, true, false),
            flat
        );
        assert_eq!(
            button_feedback(&cfg(Interaction::Subtle), false, true, false),
            flat
        );
        let reduced = MotionConfig {
            enabled: false,
            ..MotionConfig::default()
        };
        assert_eq!(button_feedback(&reduced, true, true, false), flat);
        assert_eq!(thumb_feedback(&reduced, true, true).0, 1.0);
        assert_eq!(artwork_feedback(&reduced, true, true).0, 1.0);
    }

    #[test]
    fn thumb_and_artwork_targets() {
        let c = cfg(Interaction::Subtle);
        assert_eq!(thumb_feedback(&c, false, false), (1.0, 150.0));
        assert_eq!(thumb_feedback(&c, true, false), (1.16, 150.0));
        assert_eq!(thumb_feedback(&c, true, true), (1.23, 150.0));
        assert_eq!(thumb_feedback(&c, false, true), (1.23, 150.0));
        assert_eq!(artwork_feedback(&c, true, false), (1.045, 280.0));
        assert_eq!(artwork_feedback(&c, true, true), (0.98, 280.0));
        assert_eq!(artwork_feedback(&c, false, false), (1.0, 280.0));
    }

    #[test]
    fn toggle_pulse() {
        let c = cfg(Interaction::Subtle);
        let knob = toggle_knob(&c, 0.0, true, 0.0);
        assert_eq!(knob.value(0.0), -12.0);
        assert_eq!(knob.value(1.0), 0.0);
        assert_eq!(toggle_knob(&c, 0.0, false, 3.0).value(0.0), 15.0);
        let ring = toggle_ring(&c, 0.0);
        assert_eq!(ring.value(0.0), 0.84);
        assert_eq!(ring.value(1.0), 1.0);
        let off = cfg(Interaction::Off);
        assert_eq!(toggle_knob(&off, 0.0, true, 0.0).next_frame(0.0), None);
        assert_eq!(toggle_ring(&off, 0.0).value(0.0), 1.0);
    }

    #[test]
    fn notice_modes() {
        let c = MotionConfig::default();
        let lift = notice_entry(&c, "Lift", 0.0);
        assert_eq!(lift.offset_y.value(0.0), 9.0);
        assert_eq!(lift.opacity.value(0.0), 0.0);
        let fade = notice_entry(&c, "Fade", 0.0);
        assert_eq!(
            (fade.offset_y.value(0.0), fade.scale.value(0.0)),
            (0.0, 1.0)
        );
        assert_eq!(fade.opacity.value(0.26), 1.0);
        let scale = notice_entry(&c, "Scale", 0.0);
        assert_eq!(scale.scale.value(0.0), 0.965);
        assert_eq!(scale.opacity.value(0.24), 1.0);
        assert!(scale.next_frame(0.1).is_some());
        assert_eq!(scale.next_frame(1.0), None);
        let reduced = MotionConfig {
            enabled: false,
            ..c
        };
        let rest = notice_entry(&reduced, "Lift", 0.0);
        assert_eq!(rest.opacity.value(0.0), 1.0);
        assert_eq!(rest.next_frame(0.0), None);
    }
}
