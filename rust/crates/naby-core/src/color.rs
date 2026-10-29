//! sRGB colors as stored in preferences (`#RRGGBB` / `#AARRGGBB`).

/// Straight (non-premultiplied) 8-bit sRGB color.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const BLACK: Rgba = Rgba::rgb(0, 0, 0);
    pub const WHITE: Rgba = Rgba::rgb(255, 255, 255);
    pub const TRANSPARENT: Rgba = Rgba {
        r: 0,
        g: 0,
        b: 0,
        a: 0,
    };

    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Rgba { r, g, b, a: 255 }
    }

    pub const fn with_alpha(self, a: u8) -> Self {
        Rgba { a, ..self }
    }

    /// Parses `#RGB`, `#RRGGBB` or `#AARRGGBB` (WPF order); the `#` is optional.
    pub fn parse_hex(text: &str) -> Option<Rgba> {
        let h = text.trim().trim_start_matches('#');
        let byte = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok();
        match h.len() {
            3 => {
                let n = |i: usize| {
                    u8::from_str_radix(h.get(i..i + 1)?, 16)
                        .ok()
                        .map(|v| v * 17)
                };
                Some(Rgba::rgb(n(0)?, n(1)?, n(2)?))
            }
            6 => Some(Rgba::rgb(byte(0)?, byte(2)?, byte(4)?)),
            8 => Some(Rgba {
                a: byte(0)?,
                r: byte(2)?,
                g: byte(4)?,
                b: byte(6)?,
            }),
            _ => None,
        }
    }

    /// `#RRGGBB` when opaque, `#AARRGGBB` otherwise.
    pub fn to_hex(self) -> String {
        if self.a == 255 {
            format!("#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
        } else {
            format!("#{:02X}{:02X}{:02X}{:02X}", self.a, self.r, self.g, self.b)
        }
    }

    /// Linear interpolation in sRGB space, `t` clamped to 0..=1.
    pub fn lerp(self, other: Rgba, t: f32) -> Rgba {
        let t = t.clamp(0.0, 1.0);
        let m = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
        Rgba {
            r: m(self.r, other.r),
            g: m(self.g, other.g),
            b: m(self.b, other.b),
            a: m(self.a, other.a),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_wpf_hex_forms() {
        assert_eq!(
            Rgba::parse_hex("#16171B"),
            Some(Rgba::rgb(0x16, 0x17, 0x1B))
        );
        assert_eq!(
            Rgba::parse_hex("80FFFFFF"),
            Some(Rgba::WHITE.with_alpha(0x80))
        );
        assert_eq!(Rgba::parse_hex("#fff"), Some(Rgba::WHITE));
        assert_eq!(Rgba::parse_hex("#12345"), None);
        assert_eq!(Rgba::parse_hex("#GG0000"), None);
        assert_eq!(Rgba::rgb(0xCB, 0x72, 0xD9).to_hex(), "#CB72D9");
    }
}
