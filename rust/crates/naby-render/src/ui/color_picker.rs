//! Color picker (`ColorPicker.cs`): collapsible saturation/brightness field, hue slider,
//! HEX / RGB inputs, preset and recent swatches.

use super::{capped, Key, Response, Ui};
use crate::canvas::{Fill, Rect};
use crate::icons::Icon;
use crate::text::Weight;
use crate::theme::{FONT_CAPTION, FONT_SMALL};
use naby_core::color::Rgba;

#[derive(Clone, Default)]
pub(crate) struct PickerState {
    pub open: bool,
    pub values_open: bool,
    pub h: f32,
    pub s: f32,
    pub v: f32,
    pub last: Option<Rgba>,
    pub hex: String,
    pub hex_focused: bool,
    pub rgb: [String; 3],
    pub rgb_focused: [bool; 3],
}

/// `#RRGGBB`, upper-case (the picker never writes alpha).
pub fn format_color(c: Rgba) -> String {
    format!("#{:02X}{:02X}{:02X}", c.r, c.g, c.b)
}

fn valid_hex(s: &str) -> bool {
    s.len() == 7 && s.starts_with('#') && Rgba::parse_hex(s).is_some()
}

/// Hue in degrees (0..360), saturation and value in 0..=1.
pub fn rgb_to_hsv(c: Rgba) -> (f32, f32, f32) {
    let (r, g, b) = (c.r as f32 / 255.0, c.g as f32 / 255.0, c.b as f32 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d == 0.0 {
        0.0
    } else if max == r {
        60.0 * (((g - b) / d) % 6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    (
        (h + 360.0) % 360.0,
        if max == 0.0 { 0.0 } else { d / max },
        max,
    )
}

pub fn hsv_to_rgb(hue: f32, saturation: f32, value: f32) -> Rgba {
    let h = (hue % 360.0 + 360.0) % 360.0;
    let s = saturation.clamp(0.0, 1.0);
    let v = value.clamp(0.0, 1.0);
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match h {
        h if h < 60.0 => (c, x, 0.0),
        h if h < 120.0 => (x, c, 0.0),
        h if h < 180.0 => (0.0, c, x),
        h if h < 240.0 => (0.0, x, c),
        h if h < 300.0 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let q = |v: f32| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    Rgba::rgb(q(r), q(g), q(b))
}

const RAINBOW: [(f32, Rgba); 7] = [
    (0.0, Rgba::rgb(255, 0, 0)),
    (0.167, Rgba::rgb(255, 255, 0)),
    (0.333, Rgba::rgb(0, 255, 0)),
    (0.5, Rgba::rgb(0, 255, 255)),
    (0.667, Rgba::rgb(0, 0, 255)),
    (0.833, Rgba::rgb(255, 0, 255)),
    (1.0, Rgba::rgb(255, 0, 0)),
];

impl<'a> Ui<'a> {
    /// Color picker for `value`. `presets` are always shown, `recent` colors (up to 8,
    /// without duplicates of presets) follow. `changed` is set while editing, `committed`
    /// when a pick is final (pointer released, swatch clicked, field left).
    pub fn color_picker(
        &mut self,
        value: &mut Rgba,
        presets: &[Rgba],
        recent: &[Rgba],
    ) -> Response {
        let pid = self.next_id();
        self.push_id(pid, |ui| ui.color_picker_inner(pid, value, presets, recent))
    }

    fn color_picker_inner(
        &mut self,
        pid: u64,
        value: &mut Rgba,
        presets: &[Rgba],
        recent: &[Rgba],
    ) -> Response {
        let th = self.theme;
        let mut st = self.mem.pickers.remove(&pid).unwrap_or_default();
        value.a = 255;
        if st.last != Some(*value) {
            let (h, s, v) = rgb_to_hsv(*value);
            if v > 0.0 {
                st.s = s;
                if s > 0.0 {
                    st.h = h;
                }
            } else if st.last.is_none() {
                st.s = 0.0;
            }
            st.v = v;
            st.last = Some(*value);
        }
        let mut changed = false;
        let mut committed = false;

        // ---- header button ----------------------------------------------------------------
        let style = self.text_style(FONT_SMALL, Weight::Regular, th.control_text);
        let lh = self.line_h(&style);
        let header = self.alloc(220.0, 44.0_f32.max(lh + 16.0), true);
        let hid = self.next_id();
        let hfocus = self.focusable(hid, header);
        let hit = self.interact(hid, header, true);
        if hit.clicked || (hfocus && (self.key_hit(Key::Enter) || self.key_hit(Key::Space))) {
            st.open = !st.open;
            self.mem.repaint = true;
        }
        if self.visible(header) {
            self.canvas.fill_rrect(header, th.radius, th.control_fill);
            if hit.hovered {
                self.canvas.fill_rrect(header, th.radius, th.hover_fill);
            }
            self.canvas
                .stroke_round_rect(header, th.radius, th.control_border, 1.0);
            let sample = Rect::new(
                header.x + 10.0,
                header.y + (header.h - 28.0) / 2.0,
                28.0,
                28.0,
            );
            self.canvas.fill_rrect(sample, 7.0, *value);
            self.canvas
                .stroke_round_rect(sample, 7.0, th.control_border, 1.0);
            let label = format!("Choose color \u{00B7} {}", format_color(*value));
            self.canvas.text(
                self.fonts,
                sample.right() + 10.0,
                header.y + (header.h - lh) / 2.0,
                &label,
                &style,
                Some((header.w - 80.0).max(0.0)),
            );
            let ic = Rect::new(
                header.right() - 25.0,
                header.y + (header.h - 9.0) / 2.0,
                9.0,
                9.0,
            );
            self.canvas.icon(Icon::Chevron, ic, th.text_dim, 1.6);
            if hfocus {
                self.focus_ring(header, th.radius);
            }
        }

        if st.open {
            // ---- saturation / value field ---------------------------------------------------
            let field = self.alloc(220.0, 136.0, true);
            let fid = self.next_id();
            let ffocus = self.focusable(fid, field);
            let fit = self.interact(fid, field, true);
            if fit.active && (fit.pressed || fit.pressed_now) {
                if let Some((px, py)) = self.input.pointer {
                    st.s = ((px - field.x) / field.w.max(1.0)).clamp(0.0, 1.0);
                    st.v = 1.0 - ((py - field.y) / field.h.max(1.0)).clamp(0.0, 1.0);
                    changed = true;
                }
            }
            if fit.active && self.input.released {
                committed = true;
            }
            if ffocus {
                for k in &self.input.keys {
                    let step = if k.shift { 0.05 } else { 0.01 };
                    match k.key {
                        Key::Left => st.s = (st.s - step).clamp(0.0, 1.0),
                        Key::Right => st.s = (st.s + step).clamp(0.0, 1.0),
                        Key::Up => st.v = (st.v + step).clamp(0.0, 1.0),
                        Key::Down => st.v = (st.v - step).clamp(0.0, 1.0),
                        _ => continue,
                    }
                    changed = true;
                    committed = true;
                }
            }
            if self.visible(field) {
                let base = hsv_to_rgb(st.h, 1.0, 1.0);
                self.canvas.fill_rrect(field, 8.0, base);
                self.canvas.push_clip_round_rect(field, 8.0);
                self.canvas.fill_round_rect(
                    field,
                    0.0,
                    &Fill::Linear {
                        from: (field.x, 0.0),
                        to: (field.right(), 0.0),
                        stops: vec![(0.0, Rgba::WHITE), (1.0, Rgba::WHITE.with_alpha(0))],
                    },
                );
                self.canvas.fill_round_rect(
                    field,
                    0.0,
                    &Fill::Linear {
                        from: (0.0, field.y),
                        to: (0.0, field.bottom()),
                        stops: vec![(0.0, Rgba::BLACK.with_alpha(0)), (1.0, Rgba::BLACK)],
                    },
                );
                self.canvas.pop_clip();
                let px = (st.s * field.w).clamp(6.0, (field.w - 6.0).max(6.0)) + field.x;
                let py = ((1.0 - st.v) * field.h).clamp(6.0, (field.h - 6.0).max(6.0)) + field.y;
                self.canvas.stroke_circle(px, py, 5.0, Rgba::BLACK, 4.0);
                self.canvas.stroke_circle(px, py, 5.0, Rgba::WHITE, 2.0);
                if ffocus && self.mem.focus_visible {
                    self.canvas
                        .stroke_round_rect(field.inset(1.0), 8.0, Rgba::WHITE, 1.0);
                }
            }

            // ---- hue slider -------------------------------------------------------------------
            self.gap(0.0);
            let hue_rect = self.alloc(220.0, 24.0, true);
            let uid = self.next_id();
            let ufocus = self.focusable(uid, hue_rect);
            let uit = self.interact(uid, hue_rect, true);
            let (tl, tw) = (hue_rect.x + 6.0, (hue_rect.w - 12.0).max(1.0));
            if uit.active && (uit.pressed || uit.pressed_now) {
                if let Some((px, _)) = self.input.pointer {
                    st.h = ((px - tl) / tw).clamp(0.0, 1.0) * 359.99;
                    changed = true;
                }
            }
            if uit.active && self.input.released {
                committed = true;
            }
            if ufocus {
                for k in &self.input.keys {
                    let step = if k.shift { 10.0 } else { 1.0 };
                    match k.key {
                        Key::Left | Key::Down => st.h = (st.h - step).clamp(0.0, 359.99),
                        Key::Right | Key::Up => st.h = (st.h + step).clamp(0.0, 359.99),
                        _ => continue,
                    }
                    changed = true;
                    committed = true;
                }
            }
            if self.visible(hue_rect) {
                let cy = hue_rect.y + hue_rect.h / 2.0;
                let bar = Rect::new(hue_rect.x, cy - 6.0, hue_rect.w, 12.0);
                self.canvas.fill_round_rect(
                    bar,
                    6.0,
                    &Fill::Linear {
                        from: (bar.x, 0.0),
                        to: (bar.right(), 0.0),
                        stops: RAINBOW.to_vec(),
                    },
                );
                let tx = tl + st.h / 359.99 * tw;
                let thumb = Rect::new(tx - 6.0, cy - 10.0, 12.0, 20.0);
                self.canvas
                    .fill_rrect(thumb, 6.0, Rgba::rgb(0x1B, 0x1D, 0x22));
                self.canvas.stroke_round_rect(thumb, 6.0, Rgba::WHITE, 2.0);
                if ufocus {
                    self.focus_ring(hue_rect, 8.0);
                }
            }

            if changed {
                *value = hsv_to_rgb(st.h, st.s, st.v);
                st.last = Some(*value);
            }

            // ---- HEX / RGB expander -----------------------------------------------------------
            let cap = self.text_style(FONT_CAPTION, Weight::Regular, th.text_dim);
            let elh = self.line_h(&style);
            let exp = self.alloc(220.0, elh + 8.0, true);
            let eid = self.next_id();
            let efocus = self.focusable(eid, exp);
            let eit = self.interact(eid, exp, true);
            if eit.clicked || (efocus && (self.key_hit(Key::Enter) || self.key_hit(Key::Space))) {
                st.values_open = !st.values_open;
                self.mem.repaint = true;
            }
            if self.visible(exp) {
                let (cx, cy) = (exp.x + 6.0, exp.y + exp.h / 2.0);
                let mut pb = tiny_skia::PathBuilder::new();
                if st.values_open {
                    pb.move_to(cx - 3.5, cy - 2.0);
                    pb.line_to(cx, cy + 2.0);
                    pb.line_to(cx + 3.5, cy - 2.0);
                } else {
                    pb.move_to(cx - 2.0, cy - 3.5);
                    pb.line_to(cx + 2.0, cy);
                    pb.line_to(cx - 2.0, cy + 3.5);
                }
                if let Some(p) = pb.finish() {
                    self.canvas.stroke_path(
                        &p,
                        &Fill::Solid(th.text_dim),
                        1.2,
                        tiny_skia::Transform::identity(),
                    );
                }
                let ds = crate::text::TextStyle {
                    color: th.text_dim,
                    ..style
                };
                self.canvas.text(
                    self.fonts,
                    exp.x + 22.0,
                    exp.y + (exp.h - elh) / 2.0,
                    "Color values \u{00B7} HEX / RGB",
                    &ds,
                    Some((exp.w - 24.0).max(0.0)),
                );
                if efocus {
                    self.focus_ring(exp, 6.0);
                }
            }
            if st.values_open {
                if !st.hex_focused {
                    st.hex = format_color(*value);
                }
                let opts = super::TextEditOpts::single()
                    .max_len(7)
                    .invalid(!valid_hex(&st.hex));
                let hr = self.text_edit_with(&mut st.hex, &opts);
                st.hex_focused = hr.focused;
                if hr.changed && valid_hex(&st.hex) {
                    if let Some(c) = Rgba::parse_hex(&st.hex) {
                        *value = c;
                        changed = true;
                    }
                }
                if hr.committed {
                    if !valid_hex(&st.hex) {
                        st.hex = format_color(*value);
                    }
                    committed = true;
                }
                let colw = ((self.available_width() - 12.0) / 3.0).max(30.0);
                let labels = ["Red", "Green", "Blue"];
                let chans = [value.r, value.g, value.b];
                let mut new_rgb: Option<(usize, u8)> = None;
                let mut rgb_committed = false;
                for (i, c) in chans.iter().enumerate() {
                    if !st.rgb_focused[i] {
                        st.rgb[i] = c.to_string();
                    }
                }
                self.row(|ui| {
                    ui.spacing(6.0);
                    for i in 0..3 {
                        ui.column_w(colw, |ui| {
                            ui.label_styled(labels[i], &cap);
                            let o = super::TextEditOpts::single().max_len(3);
                            let r = ui.text_edit_with(&mut st.rgb[i], &o);
                            st.rgb_focused[i] = r.focused;
                            if r.committed {
                                match st.rgb[i].parse::<u8>() {
                                    Ok(v) => new_rgb = Some((i, v)),
                                    Err(_) => st.rgb[i] = chans[i].to_string(),
                                }
                                rgb_committed = true;
                            }
                        });
                    }
                });
                if let Some((i, v)) = new_rgb {
                    match i {
                        0 => value.r = v,
                        1 => value.g = v,
                        _ => value.b = v,
                    }
                    changed = true;
                }
                committed |= rgb_committed;
            }
            if changed {
                let (h, s, v) = rgb_to_hsv(*value);
                if hsv_to_rgb(st.h, st.s, st.v) != *value {
                    if v > 0.0 {
                        st.s = s;
                        if s > 0.0 {
                            st.h = h;
                        }
                    }
                    st.v = v;
                }
                st.last = Some(*value);
            }
        }

        // ---- swatches ---------------------------------------------------------------------
        let mut picked: Option<Rgba> = None;
        self.swatch_grid(presets, &mut picked);
        let mut seen: Vec<Rgba> = Vec::new();
        for c in recent {
            if seen.len() >= 8 {
                break;
            }
            if !presets.contains(c) && !seen.contains(c) {
                seen.push(*c);
            }
        }
        self.swatch_grid(&seen, &mut picked);
        if let Some(c) = picked {
            *value = Rgba::rgb(c.r, c.g, c.b);
            changed = true;
            committed = true;
            let (h, s, v) = rgb_to_hsv(*value);
            if v > 0.0 {
                st.s = s;
                if s > 0.0 {
                    st.h = h;
                }
            }
            st.v = v;
            st.last = Some(*value);
        }

        *capped(&mut self.mem.pickers, pid) = st;
        let mut r = self.response(hid, header, &hit, hfocus);
        r.changed = changed;
        r.committed = committed;
        r
    }

    fn swatch_grid(&mut self, colors: &[Rgba], picked: &mut Option<Rgba>) {
        if colors.is_empty() {
            return;
        }
        let per_row = (((self.available_width() + 7.0) / 33.0).floor() as usize).max(1);
        self.column(|ui| {
            ui.spacing(5.0);
            for chunk in colors.chunks(per_row) {
                ui.row(|ui| {
                    ui.spacing(7.0);
                    for c in chunk {
                        if ui.color_swatch(*c).clicked {
                            *picked = Some(*c);
                        }
                    }
                });
            }
        });
    }
}

#[cfg(test)]
mod unit {
    use super::*;

    #[test]
    fn hsv_roundtrip_and_known_values() {
        assert_eq!(hsv_to_rgb(0.0, 1.0, 1.0), Rgba::rgb(255, 0, 0));
        assert_eq!(hsv_to_rgb(120.0, 1.0, 1.0), Rgba::rgb(0, 255, 0));
        assert_eq!(hsv_to_rgb(240.0, 1.0, 0.5), Rgba::rgb(0, 0, 128));
        assert_eq!(hsv_to_rgb(-60.0, 1.0, 1.0), Rgba::rgb(255, 0, 255));
        let (h, s, v) = rgb_to_hsv(Rgba::rgb(0x18, 0x20, 0x28));
        assert_eq!(hsv_to_rgb(h, s, v), Rgba::rgb(0x18, 0x20, 0x28));
        assert_eq!(rgb_to_hsv(Rgba::BLACK), (0.0, 0.0, 0.0));
        assert_eq!(format_color(Rgba::rgb(0xCB, 0x72, 0xD9)), "#CB72D9");
        for c in [Rgba::rgb(1, 2, 3), Rgba::rgb(250, 128, 5), Rgba::WHITE] {
            let (h, s, v) = rgb_to_hsv(c);
            assert_eq!(hsv_to_rgb(h, s, v), c);
        }
    }

    #[test]
    fn hex_validation() {
        assert!(valid_hex("#182028"));
        assert!(!valid_hex("182028"));
        assert!(!valid_hex("#18202"));
        assert!(!valid_hex("#18202G"));
    }
}
