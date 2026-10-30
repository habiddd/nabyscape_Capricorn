//! Dominant cover colors (`CoverPalette.Extract`) from an already downscaled bitmap.

use crate::color::Rgba;

const HUE_BINS: usize = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CoverPalette {
    pub primary: Rgba,
    pub secondary: Rgba,
}

impl CoverPalette {
    /// No artwork: fully transparent.
    pub const NONE: CoverPalette = CoverPalette {
        primary: Rgba::TRANSPARENT,
        secondary: Rgba::TRANSPARENT,
    };

    /// True when a color was extracted (primary alpha > 0).
    pub fn has_color(&self) -> bool {
        self.primary.a > 0
    }
}

/// Extracts two accent colors from straight-alpha RGBA8 pixels (`w * h * 4` bytes,
/// typically a 32x32 thumbnail). Short or empty buffers give [`CoverPalette::NONE`].
pub fn extract(rgba: &[u8], w: usize, h: usize) -> CoverPalette {
    let Some(len) = w.checked_mul(h).and_then(|n| n.checked_mul(4)) else {
        return CoverPalette::NONE;
    };
    if len == 0 || rgba.len() < len {
        return CoverPalette::NONE;
    }
    let mut weight = [0f64; HUE_BINS];
    let mut red = [0f64; HUE_BINS];
    let mut green = [0f64; HUE_BINS];
    let mut blue = [0f64; HUE_BINS];
    for px in rgba[..len].chunks_exact(4) {
        let alpha = px[3] as f64 / 255.0;
        if alpha < 0.65 {
            continue;
        }
        let (r, g, b) = (px[0] as f64, px[1] as f64, px[2] as f64);
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let delta = max - min;
        if max < 28.0 || min > 238.0 || delta < 18.0 {
            continue;
        }
        let hue = if delta < 1.0 {
            0.0
        } else if max == r {
            ((g - b) / delta + 6.0) % 6.0
        } else if max == g {
            (b - r) / delta + 2.0
        } else {
            (r - g) / delta + 4.0
        };
        let bin = (hue * 4.0) as usize % HUE_BINS;
        let w = alpha * (delta / 255.0).powf(1.5) * (0.3 + max / 255.0);
        weight[bin] += w;
        red[bin] += r * w;
        green[bin] += g * w;
        blue[bin] += b * w;
    }
    let mut main = 0;
    for i in 1..HUE_BINS {
        if weight[i] > weight[main] {
            main = i;
        }
    }
    if weight[main] < 0.01 {
        return CoverPalette {
            primary: Rgba::rgb(96, 96, 96),
            secondary: Rgba::rgb(64, 64, 64),
        };
    }
    let pick = |i: usize| -> Rgba {
        let r = red[i] / weight[i];
        let g = green[i] / weight[i];
        let b = blue[i] / weight[i];
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let delta = max - min;
        if delta < 1.0 {
            return Rgba::rgb(r as u8, g as u8, b as u8);
        }
        let saturation = (delta / max * 1.15).clamp(0.72, 0.96);
        let value = max.clamp(190.0, 245.0);
        let channel = |c: f64| -> u8 {
            (value * (1.0 - saturation + saturation * (c - min) / delta))
                .round_ties_even()
                .clamp(0.0, 255.0) as u8
        };
        Rgba::rgb(channel(r), channel(g), channel(b))
    };
    let mut other = main;
    let mut best = 0.0;
    for (i, &w) in weight.iter().enumerate() {
        let d = main.abs_diff(i);
        let distance = d.min(HUE_BINS - d);
        let score = w * distance as f64;
        if distance >= 4 && score > best {
            other = i;
            best = score;
        }
    }
    CoverPalette {
        primary: pick(main),
        secondary: pick(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(r: u8, g: u8, b: u8, a: u8, n: usize) -> Vec<u8> {
        std::iter::repeat_n([r, g, b, a], n).flatten().collect()
    }

    #[test]
    fn palette_of_solid_red() {
        let p = extract(&solid(255, 0, 0, 255, 32 * 32), 32, 32);
        assert_eq!(p.primary, Rgba::rgb(245, 10, 10));
        assert_eq!(p.secondary, p.primary);
        assert!(p.has_color());
    }

    #[test]
    fn two_hues_give_two_colors() {
        let mut px = solid(255, 0, 0, 255, 600);
        px.extend(solid(0, 0, 255, 255, 400));
        let p = extract(&px, 1000, 1);
        assert_eq!(p.primary, Rgba::rgb(245, 10, 10));
        assert!(p.secondary.b > 200 && p.secondary.r < 30);
    }

    #[test]
    fn gray_transparent_or_empty_input_falls_back() {
        let gray = extract(&solid(128, 128, 128, 255, 100), 10, 10);
        assert_eq!(gray.primary, Rgba::rgb(96, 96, 96));
        assert_eq!(gray.secondary, Rgba::rgb(64, 64, 64));
        let faint = extract(&solid(255, 0, 0, 100, 100), 10, 10);
        assert_eq!(faint.primary, Rgba::rgb(96, 96, 96));
        assert_eq!(extract(&[], 0, 0), CoverPalette::NONE);
        assert_eq!(extract(&[1, 2, 3], 1, 1), CoverPalette::NONE);
        assert_eq!(extract(&[0; 8], usize::MAX, 2), CoverPalette::NONE);
        assert!(!CoverPalette::NONE.has_color());
    }

    #[test]
    fn near_black_and_near_white_are_ignored() {
        let mut px = solid(250, 250, 255, 255, 500);
        px.extend(solid(10, 0, 0, 255, 500));
        px.extend(solid(0, 255, 0, 255, 5));
        let p = extract(&px, 1005, 1);
        assert!(p.primary.g > 200 && p.primary.r < 40);
    }
}
