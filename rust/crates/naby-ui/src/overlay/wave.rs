//! `ActivityVisual` (C# `Features/Activities/LiveActivity.cs`): the waveform bars and the
//! countdown ring of the closed dock. `Wave::sample` is a line-for-line port of
//! `ActivityVisual.Sample`; `paint` of `OnRender`.

use naby_core::color::Rgba;
use naby_core::media::BANDS;
use naby_render::{Canvas, Rect};

const FOCUS: Rgba = Rgba::rgb(0x30, 0xD1, 0x58);
const TIMER: Rgba = Rgba::rgb(0xFF, 0x9F, 0x0A);
const TRACK: Rgba = Rgba::rgb(0x35, 0x35, 0x37);

#[derive(Debug, Clone)]
pub struct Wave {
    history: [f64; BANDS],
    levels: [f64; BANDS],
    envelope: f64,
    progress: f64,
    pulse: f64,
    level_floor: f64,
    level_ceiling: f64,
    countdown: bool,
    focus: bool,
    /// Only bars, never the countdown ring.
    pub wave_only: bool,
    /// The thin look of the closed-dock title waveform.
    pub slender: bool,
}

impl Wave {
    pub fn new(wave_only: bool, slender: bool) -> Wave {
        Wave {
            history: [0.0; BANDS],
            levels: [0.0; BANDS],
            envelope: 0.0,
            progress: 0.0,
            pulse: 1.0,
            level_floor: 0.0,
            level_ceiling: 0.15,
            countdown: false,
            focus: false,
            wave_only,
            slender,
        }
    }

    pub fn envelope(&self) -> f64 {
        self.envelope
    }

    pub fn level(&self, index: usize) -> f64 {
        self.levels[index.min(BANDS - 1)]
    }

    /// True while any bar or the envelope is still above rest.
    pub fn moving(&self) -> bool {
        self.envelope > 0.0005 || self.levels.iter().any(|l| *l > 0.0005)
    }

    /// Bar height for band `index` in a box `height` tall (`WaveBarHeight`).
    pub fn bar_height(&self, index: usize, height: f64) -> f64 {
        let baseline = height.min(if self.slender { 1.4 } else { 1.8 });
        let level = if self.slender {
            self.levels[index].powf(0.65)
        } else {
            self.levels[index]
        };
        baseline + (height - if self.slender { baseline } else { 2.0 }).max(0.0) * level
    }

    /// Advances the bars by `dt` seconds. Mirrors `ActivityVisual.Sample` argument for argument.
    #[allow(clippy::too_many_arguments)]
    pub fn sample(
        &mut self,
        peak: f64,
        dt: f64,
        fraction: f64,
        session: bool,
        focused: bool,
        animate: bool,
        seconds: f64,
        advance_history: bool,
        decorative: bool,
        spectrum: Option<&[f32; BANDS]>,
        sensitivity: f64,
    ) {
        let peak = if peak.is_finite() {
            peak.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let dt = dt.clamp(0.001, 0.5);
        let mut target = 0.0;
        if animate && peak > 0.002 {
            let k = if peak < self.level_floor { 0.045 } else { 2.4 };
            self.level_floor = peak + (self.level_floor - peak) * (-dt / k).exp();
            let k = if peak > self.level_ceiling {
                0.018
            } else {
                1.2
            };
            self.level_ceiling = peak + (self.level_ceiling - peak) * (-dt / k).exp();
            let dynamics = ((peak - self.level_floor)
                / (self.level_ceiling - self.level_floor).max(0.04))
            .clamp(0.0, 1.0);
            target = 0.22 * peak.sqrt() + 0.78 * dynamics;
        }
        if !animate {
            self.level_floor = 0.0;
            self.level_ceiling = 0.15;
        }
        if animate && decorative && peak < 0.002 {
            target =
                target.max(0.055 + 0.035 * (seconds * 3.1).sin() + 0.02 * (seconds * 5.7).sin());
        }
        self.envelope = if animate {
            let k = if target > self.envelope { 0.018 } else { 0.09 };
            target + (self.envelope - target) * (-dt / k).exp()
        } else {
            0.0
        };
        if !animate {
            self.history = [0.0; BANDS];
        }
        if self.envelope < 0.001 {
            self.envelope = 0.0;
        }
        if advance_history {
            for i in (1..BANDS).rev() {
                self.history[i] = self.history[i - 1];
            }
            self.history[0] = self.envelope;
        }
        for i in 0..BANDS {
            let delay = (i * 5 + i / 7) % 9;
            let recent = (self.history[delay]
                + (self.envelope - self.history[0]) * (-(delay as f64) * 0.12).exp())
            .clamp(0.0, 1.0);
            let transient = (self.history[delay] - self.history[(delay + 3).min(24)]).abs();
            let gain = 0.62 + 0.38 * ((i * 11 + 3) % 17) as f64 / 16.0;
            let target_bar = match spectrum {
                Some(bands) => (f64::from(bands[i]).powf(1.5) * sensitivity).clamp(0.0, 1.0),
                None => ((recent * gain + transient * (0.45 + 0.55 * (i % 3) as f64 / 2.0))
                    * sensitivity)
                    .clamp(0.0, 1.0),
            };
            let old = self.levels[i];
            let response = if target_bar > old {
                0.009 + 0.001 * (i % 3) as f64
            } else {
                0.065 + 0.007 * (i % 4) as f64
            };
            self.levels[i] = if animate {
                target_bar + (old - target_bar) * (-dt / response).exp()
            } else {
                0.0
            };
            if self.levels[i] < 0.0001 {
                self.levels[i] = 0.0;
            }
        }
        self.progress = fraction;
        self.countdown = session;
        self.focus = focused;
        self.pulse = if animate && session && focused {
            0.76 + 0.24 * (seconds * std::f64::consts::PI / 1.9).sin()
        } else {
            1.0
        };
    }

    /// Resets to rest (stopped media, hidden dock).
    pub fn rest(&mut self, fraction: f64, session: bool, focused: bool) {
        self.sample(
            0.0, 1.0, fraction, session, focused, false, 0.0, true, false, None, 1.0,
        );
    }

    /// Paints into `rect` (device pixels). `scale` converts the design-unit constants.
    pub fn paint(&self, canvas: &mut Canvas, rect: Rect, scale: f32, color: Rgba) {
        let (w, h) = (rect.w, rect.h);
        if w < 2.0 || h < 2.0 {
            return;
        }
        if self.countdown && !self.wave_only {
            let c = (rect.x + w / 2.0, rect.y + h / 2.0);
            let r = w.min(h) / 2.0 - 1.5 * scale;
            canvas.stroke_circle(c.0, c.1, r, TRACK, 1.8 * scale);
            let brush = if self.focus { FOCUS } else { TIMER };
            let dot = crate::overlay::util::fade(brush, self.pulse as f32);
            canvas.fill_circle(c.0, c.1, 2.2 * scale, &naby_render::Fill::Solid(dot));
            let sweep = self.progress.clamp(0.0, 0.99999) as f32 * std::f32::consts::TAU;
            if sweep > 0.001 {
                canvas.stroke_arc(
                    c,
                    r,
                    0.0,
                    sweep,
                    crate::overlay::util::fade(brush, self.pulse as f32),
                    1.8 * scale,
                );
            }
            return;
        }
        let w_dip = f64::from(w / scale);
        let h_dip = f64::from(h / scale);
        let count = if self.wave_only {
            ((w_dip / 3.0) as usize).clamp(5, 25)
        } else {
            5
        };
        let mut gap = if self.wave_only { 1.5 } else { 1.4 };
        let mut bw = ((w_dip - gap * (count as f64 - 1.0)) / count as f64).max(0.4);
        if self.slender {
            bw = ((w_dip - (count as f64 - 1.0) * 2.25) / count as f64).clamp(0.4, 2.0);
            gap = ((w_dip - count as f64 * bw) / (count as f64 - 1.0).max(1.0)).clamp(0.8, 2.25);
        }
        let start = if self.slender {
            (w_dip - (count as f64 * bw + (count as f64 - 1.0) * gap)) / 2.0
        } else {
            0.0
        };
        for i in 0..count {
            let index = (i as f64 * 24.0 / (count as f64 - 1.0).max(1.0)).round() as usize;
            let height = self.bar_height(index, h_dip);
            let x = start + i as f64 * (bw + gap);
            let bar = Rect::new(
                rect.x + x as f32 * scale,
                rect.y + ((h_dip - height) / 2.0) as f32 * scale,
                bw as f32 * scale,
                height as f32 * scale,
            );
            canvas.fill_rrect(bar, bar.w / 2.0, color);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_rests_and_music_lifts_the_bars() {
        let mut w = Wave::new(true, true);
        for _ in 0..30 {
            w.sample(
                0.0, 0.033, 0.0, false, false, true, 0.0, true, false, None, 1.0,
            );
        }
        assert!(!w.moving());
        for i in 0..60 {
            w.sample(
                0.6,
                0.033,
                0.0,
                false,
                false,
                true,
                i as f64 * 0.033,
                true,
                false,
                None,
                1.35,
            );
        }
        assert!(w.envelope() > 0.2);
        assert!(w.bar_height(5, 22.0) > w.bar_height(5, 22.0) * 0.0 + 1.4);
        w.rest(0.0, false, false);
        assert!(!w.moving());
    }

    #[test]
    fn measured_spectrum_drives_the_bars_directly() {
        let mut w = Wave::new(true, false);
        let mut bands = [0.0f32; BANDS];
        bands[3] = 1.0;
        for _ in 0..40 {
            w.sample(
                0.5,
                0.033,
                0.0,
                false,
                false,
                true,
                0.0,
                true,
                false,
                Some(&bands),
                1.0,
            );
        }
        assert!(w.level(3) > 0.9);
        assert!(w.level(10) < 0.01);
    }

    #[test]
    fn decorative_idle_motion_keeps_a_floor() {
        let mut w = Wave::new(true, false);
        for i in 0..20 {
            w.sample(
                0.0,
                0.033,
                0.0,
                false,
                false,
                true,
                i as f64 * 0.033,
                true,
                true,
                None,
                1.0,
            );
        }
        assert!(w.moving());
    }
}
