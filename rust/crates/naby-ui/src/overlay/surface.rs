//! The island surface: contour, glass material, edge light and the album ambient glow
//! (C# `IslandSurface.OnRender`, `AmbientPaint`).

use super::Overlay;
use crate::host::Snapshots;
use naby_core::color::Rgba;
use naby_core::motion::{AmbientLoop, AmbientMotion, BottomFade, ColorBand, Contour, FadeShape};
use naby_render::{Canvas, Fill, Rect};

/// Glass gradient of the default black material.
const GLASS: [Rgba; 3] = [
    Rgba::rgb(18, 19, 23),
    Rgba::rgb(8, 9, 12),
    Rgba::rgb(1, 2, 3),
];

/// `IslandSurface.Material`: the glass colors for a background color.
pub fn material(background: Rgba) -> [Rgba; 3] {
    if background == Rgba::BLACK {
        return GLASS;
    }
    let lift = |amount: u8| {
        Rgba::rgb(
            background.r.saturating_add(amount),
            background.g.saturating_add(amount),
            background.b.saturating_add(amount),
        )
    };
    [lift(18), lift(8), lift(1)]
}

/// Alpha of the fade mask at `t` (0..1), interpolating between the nine stops.
fn mask_alpha(stops: &[(f64, u8)], t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    for pair in stops.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if t <= b.0 {
            let span = (b.0 - a.0).max(1e-9);
            let f = (t - a.0) / span;
            return (f64::from(a.1) + (f64::from(b.1) - f64::from(a.1)) * f) / 255.0;
        }
    }
    stops.last().map_or(0.0, |s| f64::from(s.1) / 255.0)
}

fn scaled(c: Rgba, k: f64) -> Rgba {
    c.with_alpha((f64::from(c.a) * k.clamp(0.0, 1.0)).round() as u8)
}

impl Overlay {
    /// (Re)builds the contour for the current surface size.
    pub(crate) fn ensure_contour(&mut self) {
        let key = (
            (self.surf.0 * 100.0).round() as i64,
            (self.surf.1 * 100.0).round() as i64,
            (self.cfg.radius * 100.0).round() as i64,
            self.cfg.notch,
        );
        if self.contour.as_ref().is_none_or(|(k, _)| *k != key) {
            let c = Contour::create(
                self.surf.0,
                self.surf.1,
                self.cfg.radius,
                self.cfg.notch,
                0.0,
            );
            self.contour = Some((key, c));
        }
    }

    pub(crate) fn contour_ref(&self) -> Option<&Contour> {
        self.contour.as_ref().map(|(_, c)| c)
    }

    /// Whether the canvas-pixel point is on the (translated) contour.
    pub(crate) fn contour_contains(&self, origin: (f32, f32), p: (f32, f32)) -> bool {
        let s = f64::from(self.scale);
        self.contour_ref().is_some_and(|c| {
            c.contains(
                (f64::from(p.0) - f64::from(origin.0)) / s,
                (f64::from(p.1) - f64::from(origin.1)) / s,
            )
        })
    }

    /// Fills the island at `origin` (canvas pixels of the contour's top-left).
    pub(crate) fn paint_surface(&self, canvas: &mut Canvas, origin: (f32, f32), snaps: &Snapshots) {
        let Some(contour) = self.contour_ref() else {
            return;
        };
        let s = self.scale;
        let settings = &snaps.settings;
        let rect = Rect::new(
            origin.0,
            origin.1,
            self.surf.0 as f32 * s,
            self.surf.1 as f32 * s,
        );
        if self.cfg.theme.high_contrast {
            canvas.fill_contour(
                contour,
                origin.0,
                origin.1,
                s,
                &Fill::Solid(self.cfg.theme.background),
            );
            canvas.stroke_contour(
                contour,
                origin.0,
                origin.1,
                s,
                &Fill::Solid(self.cfg.theme.text),
                1.0,
            );
            return;
        }
        let quiet = self.tuck.is_tucked() || (!self.expanded && !self.peeking);
        let background = if quiet {
            Rgba::BLACK
        } else {
            Rgba::parse_hex(&settings.background).unwrap_or(Rgba::BLACK)
        };
        canvas.fill_contour(contour, origin.0, origin.1, s, &Fill::Solid(background));
        canvas.push_clip_contour(contour, origin.0, origin.1, s);
        if !quiet && settings.glass_style {
            let opacity = ((0.45 + 0.55 * self.lift) * settings.glass_strength) as f32;
            let mat = material(Rgba::parse_hex(&settings.background).unwrap_or(Rgba::BLACK));
            canvas.with_opacity(opacity, |c| {
                let fill = Fill::Linear {
                    from: (rect.x, rect.y),
                    to: (rect.x + 0.65 * rect.w, rect.y + rect.h),
                    stops: vec![(0.0, mat[0]), (0.46, mat[1]), (1.0, mat[2])],
                };
                c.fill_contour(contour, origin.0, origin.1, s, &fill);
                if self.tint.a > 0 {
                    let radial = Fill::Radial {
                        center: (rect.x + 0.1 * rect.w, rect.y + 0.35 * rect.h),
                        radius: rect.w * 0.6,
                        stops: vec![(0.0, self.tint), (1.0, self.tint.with_alpha(0))],
                    };
                    c.fill_round_rect(rect, 0.0, &radial);
                }
                let light = Fill::Radial {
                    center: (
                        rect.x + self.light.0 as f32 * rect.w,
                        rect.y + self.light.1 as f32 * rect.h,
                    ),
                    radius: rect.w * 0.85,
                    stops: vec![
                        (
                            0.0,
                            Rgba {
                                r: 232,
                                g: 239,
                                b: 255,
                                a: 35,
                            },
                        ),
                        (
                            1.0,
                            Rgba {
                                r: 232,
                                g: 239,
                                b: 255,
                                a: 0,
                            },
                        ),
                    ],
                };
                c.fill_round_rect(rect, 0.0, &light);
                let edge = Fill::Linear {
                    from: (rect.x, rect.y),
                    to: (rect.right(), rect.bottom()),
                    stops: vec![
                        (0.0, Rgba::WHITE.with_alpha(66)),
                        (0.4, Rgba::WHITE.with_alpha(9)),
                        (1.0, Rgba::WHITE.with_alpha(34)),
                    ],
                };
                c.stroke_contour(contour, origin.0, origin.1, s, &edge, 0.75);
            });
        }
        self.paint_ambient(canvas, rect);
        canvas.pop_clip();
    }

    /// The album glow band masked by the fade (`DrawAmbient`).
    fn paint_ambient(&self, canvas: &mut Canvas, rect: Rect) {
        let (Some(band), Some(fade)) = (self.ambient.band(), self.ambient.fade()) else {
            return;
        };
        let gain = self.ambient.audio_gain(self.now);
        let (opacity, offset) = match self.ambient.loop_state() {
            Some(l) => loop_effect(l, self.now),
            None => (1.0, 0.0),
        };
        paint_band(canvas, rect, band, fade, gain * opacity, offset);
    }
}

fn loop_effect(l: &AmbientLoop, now: f64) -> (f64, f64) {
    let v = l.value(now);
    match l.motion {
        AmbientMotion::Breathe => (v, 0.0),
        AmbientMotion::Drift => (1.0, v),
        AmbientMotion::Still => (1.0, 0.0),
    }
}

/// Draws the color band through the fade mask. Linear masks are painted as 48 strips along the
/// fade axis, each with the band gradient scaled by the mask alpha at its centre.
pub fn paint_band(
    canvas: &mut Canvas,
    rect: Rect,
    band: &ColorBand,
    fade: &BottomFade,
    gain: f64,
    offset: f64,
) {
    if rect.is_empty() || gain <= 0.0 {
        return;
    }
    match fade.shape {
        FadeShape::Radial { .. } => {
            let mid = band.left.lerp(band.right, 0.5);
            let stops = fade
                .stops
                .iter()
                .map(|(o, a)| (*o as f32, scaled(mid, f64::from(*a) / 255.0 * gain)))
                .collect();
            let fill = Fill::Radial {
                center: rect.center(),
                radius: rect.w.max(rect.h) * 0.6,
                stops,
            };
            canvas.fill_round_rect(rect, 0.0, &fill);
        }
        FadeShape::Linear { from, to } => {
            let vertical = (to.1 - from.1).abs() > (to.0 - from.0).abs();
            let reversed = if vertical {
                to.1 < from.1
            } else {
                to.0 < from.0
            };
            const STRIPS: usize = 48;
            let extent = if vertical { rect.h } else { rect.w };
            let origin = if vertical { rect.y } else { rect.x };
            for i in 0..STRIPS {
                let p0 = i as f64 / STRIPS as f64;
                let p1 = (i + 1) as f64 / STRIPS as f64;
                let centre = (p0 + p1) / 2.0;
                let t = if reversed { 1.0 - centre } else { centre };
                let alpha = mask_alpha(&fade.stops, t - offset) * gain;
                if alpha <= 0.002 {
                    continue;
                }
                let a = origin + (p0 as f32 * extent).round();
                let b = origin + (p1 as f32 * extent).round();
                let strip = if vertical {
                    Rect::new(rect.x, a, rect.w, b - a)
                } else {
                    Rect::new(a, rect.y, b - a, rect.h)
                };
                if strip.is_empty() {
                    continue;
                }
                let (c0, c1) = (scaled(band.left, alpha), scaled(band.right, alpha));
                let (gf, gt) = if band.angle == 0 {
                    ((rect.x, 0.0), (rect.right(), 0.0))
                } else {
                    ((0.0, rect.y), (0.0, rect.bottom()))
                };
                let fill = Fill::Linear {
                    from: gf,
                    to: gt,
                    stops: vec![(0.0, c0), (1.0, c1)],
                };
                canvas.fill_round_rect(strip, 0.0, &fill);
            }
        }
    }
}
