//! Album-colour ambient light of the island (C# `Motion/IslandAmbient.cs`): the paint recipe
//! (colour band + bottom fade), the idle loop and the audio-reactive gain, as values.

use super::easing::{Ease, MotionConfig, Tween};
use crate::color::Rgba;
use crate::settings::AmbientLook;

/// Cover palette: the two colours the band blends between (C# `CoverPalette`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AmbientColors {
    pub primary: Rgba,
    pub secondary: Rgba,
}

/// `AmbientMotion` setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AmbientMotion {
    /// No loop.
    Still,
    /// Opacity pulses 0.72 ↔ 1 over 3.6 s.
    Breathe,
    /// The fade mask drifts ±0.035 over 5.5 s.
    Drift,
}

impl AmbientMotion {
    /// Anything but "Still" / "Breathe" drifts, like the C# `else` arm.
    pub fn parse(text: &str) -> AmbientMotion {
        match text {
            "Still" => AmbientMotion::Still,
            "Breathe" => AmbientMotion::Breathe,
            _ => AmbientMotion::Drift,
        }
    }
}

fn horizontal(look: &AmbientLook) -> bool {
    matches!(look.direction.as_str(), "Left" | "Right")
}

/// Horizontal two-colour gradient (C# `AmbientPaint.ColorBand`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColorBand {
    pub left: Rgba,
    pub right: Rgba,
    /// Gradient angle in degrees: 90 for left/right directions, else 0.
    pub angle: u16,
}

/// Opacity mask geometry (WPF relative coordinates).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FadeShape {
    Linear {
        from: (f64, f64),
        to: (f64, f64),
    },
    Radial {
        center: (f64, f64),
        rx: f64,
        ry: f64,
    },
}

/// Opacity mask that keeps the title region clear (C# `AmbientPaint.BottomFade`).
#[derive(Debug, Clone, PartialEq)]
pub struct BottomFade {
    pub shape: FadeShape,
    /// `(offset 0..1, alpha 0..255)`; nine stops.
    pub stops: Vec<(f64, u8)>,
}

fn mix(a: Rgba, b: Rgba, t: f64) -> Rgba {
    let c = |x: u8, y: u8| (f64::from(x) + (f64::from(y) - f64::from(x)) * t) as u8;
    Rgba {
        r: c(a.r, b.r),
        g: c(a.g, b.g),
        b: c(a.b, b.b),
        a: c(a.a, b.a),
    }
}

/// C# `AmbientPaint.ColorBand`.
pub fn color_band(palette: &AmbientColors, strength: f64, look: &AmbientLook) -> ColorBand {
    let alpha = (255.0 * strength.clamp(0.0, 1.0).sqrt()).round_ties_even() as u32;
    let tint = |c: Rgba| Rgba {
        a: (alpha * u32::from(c.a) / 255) as u8,
        ..c
    };
    let left = if look.balance <= 0.5 {
        palette.primary
    } else {
        mix(
            palette.primary,
            palette.secondary,
            (look.balance - 0.5) * 2.0,
        )
    };
    let right = if look.balance >= 0.5 {
        palette.secondary
    } else {
        mix(palette.primary, palette.secondary, look.balance * 2.0)
    };
    ColorBand {
        left: tint(left),
        right: tint(right),
        angle: if horizontal(look) { 90 } else { 0 },
    }
}

/// C# `AmbientPaint.BottomFade`.
pub fn bottom_fade(spread: f64, look: &AmbientLook) -> BottomFade {
    let start = 1.0 - (1.0 - (0.58 - spread.clamp(0.25, 2.5) * 0.30)) * look.height;
    let exponent = 3.6 - look.softness * 2.8;
    let stops = (0..=8)
        .map(|i| {
            let offset = f64::from(i) / 8.0;
            let mut t = ((offset - start) / (1.0 - start)).clamp(0.0, 1.0);
            if t.is_nan() {
                t = 0.0;
            }
            (offset, (255.0 * t.powf(exponent)).round_ties_even() as u8)
        })
        .collect();
    let shape = match look.direction.as_str() {
        "Surround" => FadeShape::Radial {
            center: (0.5, 0.5),
            rx: 0.55,
            ry: 0.65,
        },
        "Top" => FadeShape::Linear {
            from: (0.5, 1.0),
            to: (0.5, 0.0),
        },
        "Left" => FadeShape::Linear {
            from: (1.0, 0.5),
            to: (0.0, 0.5),
        },
        "Right" => FadeShape::Linear {
            from: (0.0, 0.5),
            to: (1.0, 0.5),
        },
        _ => FadeShape::Linear {
            from: (0.5, 0.0),
            to: (0.5, 1.0),
        },
    };
    BottomFade { shape, stops }
}

/// C# `SetAmbientAudio` clamp: gain stays within 0.32..1; non-finite input is the floor.
pub fn ambient_audio_gain(gain: f64) -> f64 {
    if gain.is_finite() {
        gain.clamp(0.32, 1.0)
    } else {
        0.32
    }
}

/// The endless auto-reversing sine loop (opacity for Breathe, mask offset for Drift).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AmbientLoop {
    pub motion: AmbientMotion,
    /// Drift axis: x for left/right directions, otherwise y.
    pub horizontal_axis: bool,
    pub from: f64,
    pub to: f64,
    pub period: f64,
    pub start: f64,
    pub fps: f64,
}

impl AmbientLoop {
    /// Current value between `from` and `to`.
    pub fn value(&self, now: f64) -> f64 {
        let cycle = ((now - self.start) / self.period).rem_euclid(2.0);
        let phase = if cycle <= 1.0 { cycle } else { 2.0 - cycle };
        self.from + (self.to - self.from) * Ease::SINE_IN_OUT.apply(phase)
    }

    /// A running loop always needs the next frame.
    pub fn next_frame(&self, now: f64) -> Option<f64> {
        Some(now + 1.0 / self.fps)
    }
}

#[derive(Debug, Clone, PartialEq)]
struct Key {
    colors: AmbientColors,
    strength: f64,
    spread: f64,
    motion: AmbientMotion,
    running: bool,
    frame_rate: u32,
    look: AmbientLook,
}

/// What changed in [`AmbientLayer::set`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AmbientChange {
    pub repaint: bool,
    pub unchanged: bool,
}

/// Ambient light state of one island surface.
#[derive(Debug, Clone)]
pub struct AmbientLayer {
    key: Option<Key>,
    band: Option<ColorBand>,
    fade: Option<BottomFade>,
    gain: Tween<f64>,
    visible: bool,
    clock: Option<AmbientLoop>,
}

impl Default for AmbientLayer {
    fn default() -> Self {
        AmbientLayer {
            key: None,
            band: None,
            fade: None,
            gain: Tween::instant(1.0),
            visible: false,
            clock: None,
        }
    }
}

impl AmbientLayer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether anything is painted (C# `AmbientVisible`).
    pub fn visible(&self) -> bool {
        self.band.is_some()
    }

    pub fn band(&self) -> Option<&ColorBand> {
        self.band.as_ref()
    }

    pub fn fade(&self) -> Option<&BottomFade> {
        self.fade.as_ref()
    }

    /// Primary colour (C# `AmbientColor`), transparent when unset.
    pub fn color(&self) -> Rgba {
        self.key
            .as_ref()
            .map_or(Rgba::TRANSPARENT, |k| k.colors.primary)
    }

    /// The idle loop is running (C# `AmbientAnimating`).
    pub fn animating(&self) -> bool {
        self.clock.is_some()
    }

    pub fn loop_state(&self) -> Option<&AmbientLoop> {
        self.clock.as_ref()
    }

    /// The host shows / hides the surface; the loop only runs while visible.
    pub fn set_surface_visible(&mut self, visible: bool, now: f64) {
        if self.visible != visible {
            self.visible = visible;
            self.configure_clock(now);
        }
    }

    /// C# `SetAmbient`. The paint is rebuilt only when colours, strength, spread or look change;
    /// a change of motion / running state only restarts the loop.
    #[allow(clippy::too_many_arguments)]
    pub fn set(
        &mut self,
        colors: AmbientColors,
        strength: f64,
        spread: f64,
        motion: &str,
        running: bool,
        look: Option<&AmbientLook>,
        cfg: &MotionConfig,
        now: f64,
    ) -> AmbientChange {
        let look = look.cloned().unwrap_or_default();
        let next = Key {
            colors,
            strength,
            spread,
            motion: AmbientMotion::parse(motion),
            running: cfg.enabled && running,
            frame_rate: cfg.frame_limit.clamp(30, 240),
            look,
        };
        if self.key.as_ref() == Some(&next) {
            return AmbientChange {
                unchanged: true,
                ..AmbientChange::default()
            };
        }
        let same_paint = self.key.as_ref().is_some_and(|old| {
            old.colors == next.colors
                && old.strength == next.strength
                && old.spread == next.spread
                && old.look == next.look
        });
        self.clock = None;
        if !same_paint {
            if strength > 0.0 && colors.primary.a > 0 {
                self.band = Some(color_band(&colors, strength, &next.look));
                self.fade = Some(bottom_fade(spread, &next.look));
            } else {
                self.band = None;
                self.fade = None;
            }
        }
        self.key = Some(next);
        self.configure_clock(now);
        AmbientChange {
            repaint: !same_paint,
            unchanged: false,
        }
    }

    fn configure_clock(&mut self, now: f64) {
        self.clock = None;
        let (Some(key), Some(_)) = (self.key.as_ref(), self.band.as_ref()) else {
            return;
        };
        if !self.visible || !key.running || key.motion == AmbientMotion::Still {
            return;
        }
        let breathe = key.motion == AmbientMotion::Breathe;
        self.clock = Some(AmbientLoop {
            motion: key.motion,
            horizontal_axis: horizontal(&key.look),
            from: if breathe { 0.72 } else { -0.035 },
            to: if breathe { 1.0 } else { 0.035 },
            period: if breathe { 3.6 } else { 5.5 },
            start: now,
            fps: f64::from(key.frame_rate),
        });
    }

    /// C# `SetAmbientAudio`: eases the layer opacity (55 ms) towards `gain`.
    pub fn set_audio(&mut self, gain: f64, now: f64, cfg: &MotionConfig) {
        let gain = ambient_audio_gain(gain);
        let current = self.gain.value(now);
        if (current - gain).abs() > 0.004 {
            self.gain = cfg.to(current, gain, now, 55.0, 0.0);
        }
    }

    /// C# `ResetAmbientAudio`.
    pub fn reset_audio(&mut self) {
        self.gain = Tween::instant(1.0);
    }

    /// Layer opacity from the audio response (C# `AmbientAudioGain`).
    pub fn audio_gain(&self, now: f64) -> f64 {
        self.gain.value(now)
    }

    /// Earliest time anything here needs a frame.
    pub fn next_frame(&self, now: f64) -> Option<f64> {
        let a = self.clock.and_then(|c| c.next_frame(now));
        let b = self.gain.next_frame(now);
        match (a, b) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn palette() -> AmbientColors {
        AmbientColors {
            primary: Rgba::rgb(200, 40, 20),
            secondary: Rgba::rgb(20, 40, 200),
        }
    }

    fn look(direction: &str, balance: f64) -> AmbientLook {
        AmbientLook {
            direction: direction.into(),
            height: 1.0,
            balance,
            softness: 0.5,
        }
    }

    #[test]
    fn color_band_alpha_balance_and_angle() {
        let band = color_band(&palette(), 0.25, &look("Bottom", 0.5));
        // alpha = round(255 * sqrt(.25)) = 128
        assert_eq!(
            band.left,
            Rgba {
                r: 200,
                g: 40,
                b: 20,
                a: 128
            }
        );
        assert_eq!(
            band.right,
            Rgba {
                r: 20,
                g: 40,
                b: 200,
                a: 128
            }
        );
        assert_eq!(band.angle, 0);
        let band = color_band(&palette(), 1.0, &look("Left", 1.0));
        assert_eq!(band.angle, 90);
        assert_eq!(
            band.left.r, 20,
            "balance 1 pulls the left colour fully to secondary"
        );
        assert_eq!(band.left.a, 255);
        let band = color_band(&palette(), 1.0, &look("Right", 0.0));
        assert_eq!(
            band.right.r, 200,
            "balance 0 pulls the right colour to primary"
        );
        // Strength is clamped and translucent colours scale the tint alpha.
        let half = AmbientColors {
            primary: Rgba {
                a: 128,
                ..palette().primary
            },
            secondary: palette().secondary,
        };
        let band = color_band(&half, 4.0, &look("Bottom", 0.5));
        assert_eq!(band.left.a, 128);
    }

    #[test]
    fn bottom_fade_stops_and_shape() {
        let fade = bottom_fade(1.0, &look("Bottom", 0.5));
        assert_eq!(fade.stops.len(), 9);
        assert_eq!(fade.stops[0], (0.0, 0));
        assert_eq!(fade.stops[8], (1.0, 255));
        assert!(fade.stops.windows(2).all(|w| w[0].1 <= w[1].1), "monotonic");
        // spread 1 → start = 1 - (1 - (0.58 - 0.30)) = 0.28; offset .25 is still clear
        assert_eq!(fade.stops[2].1, 0);
        assert!(fade.stops[3].1 > 0);
        assert_eq!(
            fade.shape,
            FadeShape::Linear {
                from: (0.5, 0.0),
                to: (0.5, 1.0)
            }
        );
        let surround = bottom_fade(1.0, &look("Surround", 0.5));
        assert!(
            matches!(surround.shape, FadeShape::Radial { rx, ry, .. } if rx == 0.55 && ry == 0.65)
        );
        let top = bottom_fade(1.0, &look("Top", 0.5));
        assert_eq!(
            top.shape,
            FadeShape::Linear {
                from: (0.5, 1.0),
                to: (0.5, 0.0)
            }
        );
        // A zero-height look degenerates without NaN artefacts.
        let flat = AmbientLook {
            height: 0.0,
            ..look("Bottom", 0.5)
        };
        let fade = bottom_fade(1.0, &flat);
        assert!(fade.stops.iter().all(|(_, a)| *a == 0));
        // Softness changes the curve.
        let soft = AmbientLook {
            softness: 1.0,
            ..look("Bottom", 0.5)
        };
        assert!(
            bottom_fade(1.0, &soft).stops[4].1 > bottom_fade(1.0, &look("Bottom", 0.0)).stops[4].1
        );
    }

    #[test]
    fn audio_gain_clamps() {
        assert_eq!(ambient_audio_gain(0.0), 0.32);
        assert_eq!(ambient_audio_gain(5.0), 1.0);
        assert_eq!(ambient_audio_gain(0.5), 0.5);
        assert_eq!(ambient_audio_gain(f64::NAN), 0.32);
        assert_eq!(ambient_audio_gain(f64::INFINITY), 0.32);
    }

    #[test]
    fn loop_shapes() {
        let l = AmbientLoop {
            motion: AmbientMotion::Breathe,
            horizontal_axis: false,
            from: 0.72,
            to: 1.0,
            period: 3.6,
            start: 0.0,
            fps: 60.0,
        };
        assert!((l.value(0.0) - 0.72).abs() < 1e-12);
        assert!((l.value(3.6) - 1.0).abs() < 1e-12);
        assert!((l.value(7.2) - 0.72).abs() < 1e-12, "auto-reverses");
        assert!((l.value(1.8) - 0.86).abs() < 1e-12, "sine in-out midpoint");
        assert!(l.next_frame(1.0).unwrap() > 1.0);
    }

    fn running_cfg() -> MotionConfig {
        MotionConfig::default()
    }

    #[test]
    fn layer_builds_paint_and_runs_the_right_loop() {
        let cfg = running_cfg();
        let mut layer = AmbientLayer::new();
        layer.set_surface_visible(true, 0.0);
        let l = look("Bottom", 0.5);
        let change = layer.set(palette(), 0.6, 1.0, "Breathe", true, Some(&l), &cfg, 1.0);
        assert!(change.repaint && layer.visible());
        let lp = layer.loop_state().unwrap();
        assert_eq!((lp.from, lp.to, lp.period, lp.start), (0.72, 1.0, 3.6, 1.0));
        assert!(layer.animating());
        // Identical input is a no-op and keeps the loop phase.
        let again = layer.set(palette(), 0.6, 1.0, "Breathe", true, Some(&l), &cfg, 2.0);
        assert!(again.unchanged);
        assert_eq!(layer.loop_state().unwrap().start, 1.0);
        // Drift on a side direction animates the x axis.
        let side = look("Left", 0.5);
        let c = layer.set(palette(), 0.6, 1.0, "Drift", true, Some(&side), &cfg, 3.0);
        assert!(c.repaint, "look changed");
        let lp = layer.loop_state().unwrap();
        assert!(lp.horizontal_axis && lp.period == 5.5 && lp.to == 0.035);
        // Only the motion changing restarts the loop without repainting.
        let c = layer.set(palette(), 0.6, 1.0, "Still", true, Some(&side), &cfg, 4.0);
        assert!(!c.repaint && !c.unchanged);
        assert!(!layer.animating());
    }

    #[test]
    fn layer_stays_still_when_hidden_paused_or_reduced() {
        let cfg = running_cfg();
        let mut layer = AmbientLayer::new();
        layer.set(palette(), 0.6, 1.0, "Drift", true, None, &cfg, 0.0);
        assert!(!layer.animating(), "surface not visible yet");
        layer.set_surface_visible(true, 0.0);
        assert!(layer.animating());
        layer.set_surface_visible(false, 1.0);
        assert!(!layer.animating());
        layer.set_surface_visible(true, 2.0);
        layer.set(palette(), 0.6, 1.0, "Drift", false, None, &cfg, 2.0);
        assert!(!layer.animating(), "music paused");
        let reduced = MotionConfig {
            enabled: false,
            ..cfg
        };
        layer.set(palette(), 0.6, 1.0, "Drift", true, None, &reduced, 3.0);
        assert!(!layer.animating(), "reduce motion");
        assert_eq!(layer.next_frame(3.0), None, "nothing asks for a frame");
    }

    #[test]
    fn layer_clears_paint_for_zero_strength_or_transparent_colour() {
        let cfg = running_cfg();
        let mut layer = AmbientLayer::new();
        layer.set(palette(), 0.0, 1.0, "Drift", true, None, &cfg, 0.0);
        assert!(!layer.visible() && layer.fade().is_none());
        let clear = AmbientColors {
            primary: Rgba::TRANSPARENT,
            secondary: Rgba::TRANSPARENT,
        };
        layer.set(clear, 1.0, 1.0, "Drift", true, None, &cfg, 0.0);
        assert!(!layer.visible());
        layer.set(palette(), 1.0, 1.0, "Drift", true, None, &cfg, 0.0);
        assert!(layer.visible() && layer.band().is_some());
        assert_eq!(layer.color(), palette().primary);
        layer.set(palette(), 0.0, 1.0, "Drift", true, None, &cfg, 0.0);
        assert!(!layer.visible());
    }

    #[test]
    fn audio_response_tween() {
        let cfg = running_cfg();
        let mut layer = AmbientLayer::new();
        assert_eq!(layer.audio_gain(0.0), 1.0);
        layer.set_audio(0.5, 0.0, &cfg);
        assert!(layer.next_frame(0.0).is_some());
        let mid = layer.audio_gain(0.03);
        assert!(mid < 1.0 && mid > 0.5);
        assert_eq!(layer.audio_gain(0.1), 0.5);
        assert_eq!(layer.next_frame(0.1), None);
        // Sub-threshold changes are ignored.
        layer.set_audio(0.502, 0.2, &cfg);
        assert_eq!(layer.audio_gain(0.3), 0.5);
        layer.set_audio(f64::NAN, 1.0, &cfg);
        assert_eq!(layer.audio_gain(2.0), 0.32);
        layer.reset_audio();
        assert_eq!(layer.audio_gain(2.0), 1.0);
    }
}
