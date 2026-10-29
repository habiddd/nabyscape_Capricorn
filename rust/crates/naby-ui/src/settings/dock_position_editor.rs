//! The screen map that places the dock by dragging (C# `DockPositionEditor.cs`): a miniature
//! screen with a dock marker, the position sliders with number fields, and a readout of the
//! resulting pixel position.

use super::kit::num;
use super::SettingsView;
use naby_core::color::Rgba;
use naby_core::settings::Settings;
use naby_render::{Fill, Rect, TextEditOpts, Ui, Weight};

/// The numbers the editor derives from the preferences and the screen (`Sync`).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DockReadout {
    pub side: bool,
    pub bottom: bool,
    pub vertical_max: f64,
    pub vertical_step: f64,
    pub vertical_value: f64,
    pub vertical_label: &'static str,
    pub vertical_enabled: bool,
    pub left: f64,
    pub top: f64,
    pub offset: f64,
}

impl DockReadout {
    pub fn compute(s: &Settings, work_w: f64, height: f64) -> DockReadout {
        let side = matches!(s.dock_edge.as_str(), "Left" | "Right");
        let bottom = s.dock_edge == "Bottom";
        let vertical_max = if side {
            100.0
        } else if bottom {
            200.0
        } else {
            (height - s.height).clamp(0.0, 3000.0)
        };
        let vertical_value = if side {
            s.dock_vertical
        } else if bottom {
            s.dock_inset
        } else {
            s.top
        };
        let mut left = (work_w - s.width) * s.horizontal / 100.0;
        let top = if side {
            (height - s.height) * s.dock_vertical / 100.0
        } else if bottom {
            height - s.height - s.dock_inset
        } else {
            s.top
        };
        if side {
            left = if s.dock_edge == "Left" {
                s.dock_inset
            } else {
                work_w - s.width - s.dock_inset
            };
        }
        let offset = if side {
            s.dock_vertical - 50.0
        } else {
            s.horizontal - 50.0
        };
        DockReadout {
            side,
            bottom,
            vertical_max,
            vertical_step: if side { 0.1 } else { 1.0 },
            vertical_value,
            vertical_label: if side {
                "Vertical · %"
            } else if bottom {
                "Bottom inset · px"
            } else {
                "Top offset · px"
            },
            vertical_enabled: !(s.preset == "Mac" && s.dock_edge == "Top"),
            left,
            top,
            offset,
        }
    }

    /// "Centered on this screen" / "12% left of center".
    pub fn alignment(&self) -> (String, bool) {
        if self.offset.abs() < 0.05 {
            return ("Centered on this screen".to_owned(), true);
        }
        let how = if self.side {
            if self.offset < 0.0 {
                "above"
            } else {
                "below"
            }
        } else if self.offset < 0.0 {
            "left of"
        } else {
            "right of"
        };
        (
            format!("{}% {} center", trim_num(self.offset.abs()), how),
            false,
        )
    }
}

/// `ToString("0.#")`.
fn trim_num(v: f64) -> String {
    let mut text = format!("{v:.1}");
    if text.ends_with(".0") {
        text.truncate(text.len() - 2);
    }
    text
}

impl SettingsView {
    /// `DockEditingSettings`: the "Move & resize" card.
    pub(super) fn dock_editing_settings<'a>(&mut self, ui: &mut Ui<'a>) {
        self.section(ui, "Move & resize", |this, ui| {
            this.dock_position_editor(ui);
            let unlocked = this.s.dock_unlocked;
            this.check(ui, "Show island move and resize handles", unlocked, |s, v| s.dock_unlocked = v);
            this.muted(
                ui,
                "Drag the top grip to reposition the island. Drag either lower corner to resize the current state: closed, hover or open panel. Position and each size are saved separately. Hold Alt while the island is focused to reveal handles temporarily; Esc cancels a drag. Dragging down detaches a notch into a pill; a drag that begins as a notch can snap back to the top edge.",
                11.0,
            );
        });
    }

    fn dock_position_editor<'a>(&mut self, ui: &mut Ui<'a>) {
        let th = ui.theme().clone();
        let pal = self.pal(ui);
        let read = DockReadout::compute(
            &self.s,
            self.host.work_area_width,
            self.host.work_area_height,
        );
        // ---- the map ----------------------------------------------------------------------
        let map = ui.allocate(None, 132.0);
        let resp = ui.hit_area(map, false);
        self.record("Dock map", map);
        self.hand |= resp.hovered;
        let tip = self.tr("Drag the dock marker. Center guides snap within 1%. Values are Windows layout pixels (DIPs).").to_owned();
        ui.tooltip(&resp, &tip);
        let bg = if pal.hc {
            pal.window
        } else {
            Rgba::parse_hex("#10151D").unwrap_or(Rgba::BLACK)
        };
        let border = if pal.hc {
            th.text
        } else {
            Rgba::parse_hex("#343C48").unwrap_or(th.control_border)
        };
        let line_c = if pal.hc {
            th.text
        } else {
            Rgba::parse_hex("#415064").unwrap_or(th.text_dim)
        };
        {
            let c = ui.canvas();
            c.fill_rrect(map, 10.0, bg);
            c.stroke_round_rect(map, 10.0, border, 1.0);
        }
        let w = map.w - 32.0;
        let h = map.h - 24.0;
        if map.w > 32.0 {
            ui.canvas().push_clip_round_rect(map, 10.0);
            // Dashed center guides.
            let mut y = map.y + 8.0;
            while y < map.y + h + 16.0 {
                ui.canvas()
                    .fill_rect(Rect::new(map.x + 16.0 + w / 2.0, y, 0.7, 3.0), line_c);
                y += 7.0;
            }
            let mut x = map.x + 12.0;
            while x < map.x + w + 20.0 {
                ui.canvas()
                    .fill_rect(Rect::new(x, map.y + 12.0 + h / 2.0, 3.0, 0.7), line_c);
                x += 7.0;
            }
            let s = &self.s;
            let mut dx = s.horizontal / 100.0;
            let dy = if s.dock_edge == "Bottom" {
                1.0
            } else if read.side {
                s.dock_vertical / 100.0
            } else if s.dock_edge == "Floating" || s.preset != "Mac" {
                s.top / read.vertical_max.max(1.0)
            } else {
                0.0
            };
            if s.dock_edge == "Left" {
                dx = 0.0;
            }
            if s.dock_edge == "Right" {
                dx = 1.0;
            }
            let marker = Rect::new(
                map.x + 16.0 + (dx as f32) * (w - 58.0),
                map.y + 12.0 + (dy as f32) * (h - 15.0),
                58.0,
                15.0,
            );
            let c = ui.canvas();
            c.fill_rrect(marker, 6.0, Rgba::BLACK);
            c.stroke_round_rect(
                marker,
                6.0,
                Rgba::parse_hex("#CAD9EA").unwrap_or(th.text),
                1.0,
            );
            c.pop_clip();
        }
        // Drag: the press and every move while it is held.
        if resp.pressed {
            if let Some((px, py)) = ui.input().pointer {
                let mut x = (((px - map.x) - 45.0) / (w - 58.0).max(1.0) * 100.0).clamp(0.0, 100.0);
                let mut y = (((py - map.y) - 19.5) / (h - 15.0).max(1.0) * 100.0).clamp(0.0, 100.0);
                if (x - 50.0).abs() < 1.0 {
                    x = 50.0;
                }
                if (y - 50.0).abs() < 1.0 {
                    y = 50.0;
                }
                let (x, y) = (f64::from(x), f64::from(y));
                if read.side {
                    self.s.dock_vertical = y;
                } else {
                    self.s.horizontal = x;
                }
                if self.s.dock_edge == "Floating"
                    || (self.s.dock_edge == "Top" && self.s.preset != "Mac")
                {
                    self.s.top = (y / 100.0 * read.vertical_max).round();
                }
                self.configure_preview_for_setting("Horizontal position");
                self.save();
                self.repaint = true;
            }
        }
        ui.gap(10.0);
        // ---- readout ----------------------------------------------------------------------
        let (text, centered) = read.alignment();
        let align = if centered {
            self.tr(&text).to_owned()
        } else {
            let (num_part, rest) = text.split_once("% ").unwrap_or((&text, ""));
            let mut it = rest.rsplitn(2, ' ');
            let center = it.next().unwrap_or("center");
            let how = it.next().unwrap_or("");
            format!("{}% {} {}", num_part, self.tr(how), self.tr(center))
        };
        let color = if centered {
            Rgba::parse_hex("#A4CEC0").unwrap_or(th.text)
        } else {
            th.text_dim
        };
        let st = ui.text_style(
            11.0,
            Weight::Regular,
            if centered { color } else { th.text },
        );
        let btn = self.tr("Center").to_owned();
        let avail = ui.available_width();
        let mut center_clicked = false;
        ui.row_centered(34.0, |ui| {
            ui.spacing(0.0);
            let r = ui.allocate(Some((avail - 90.0).max(40.0)), 34.0);
            let lh = ui.line_height(&st);
            ui.draw_text(r.x, r.y + (r.h - lh) / 2.0, &align, &st, Some(r.w));
            let r = ui.button(&btn);
            self.hand |= r.hovered;
            self.record("Center", r.rect);
            center_clicked = r.clicked;
        });
        if center_clicked {
            self.s.horizontal = 50.0;
            self.s.dock_vertical = 50.0;
            self.save();
        }
        ui.gap(3.0);
        let coords = format!(
            "{} {} px · {} {} px · {}% {}",
            self.tr("Left"),
            format_args!("{:.0}", read.left),
            self.tr("Top"),
            format_args!("{:.0}", read.top),
            format_args!("{:.0}", self.host.display_scale_percent),
            self.tr("display scaling")
        );
        self.muted_final(ui, &coords, 11.0);
        ui.gap(12.0);
        // ---- the two position fields ---------------------------------------------------------
        let horizontal = self.s.horizontal;
        if let Some(v) = self.position_field(
            ui,
            "Horizontal position",
            "Horizontal · %",
            horizontal,
            0.0,
            100.0,
            0.1,
            true,
        ) {
            self.s.horizontal = v;
            self.save();
        }
        let label = read.vertical_label;
        let value = read.vertical_value;
        let enabled = read.vertical_enabled;
        let mut changed: Option<f64> = None;
        ui.enabled(enabled, |ui| {
            changed = self.position_field(
                ui,
                "Vertical dock position",
                label,
                value,
                0.0,
                read.vertical_max,
                read.vertical_step,
                false,
            );
        });
        if let Some(v) = changed {
            if read.side {
                self.s.dock_vertical = v;
            } else if read.bottom {
                self.s.dock_inset = v;
            } else {
                self.s.top = v;
            }
            self.save();
        }
    }

    /// One label + number field + slider; returns the new value when it changed.
    #[allow(clippy::too_many_arguments)]
    fn position_field<'a>(
        &mut self,
        ui: &mut Ui<'a>,
        name: &str,
        label: &str,
        value: f64,
        min: f64,
        max: f64,
        step: f64,
        one_decimal: bool,
    ) -> Option<f64> {
        let key = format!("pos:{name}");
        let (mut buf, had_focus) = self
            .fields
            .get(&key)
            .cloned()
            .unwrap_or_else(|| (String::new(), false));
        let shown = if one_decimal {
            format!("{value:.1}")
        } else {
            num(value)
        };
        if !had_focus {
            buf = shown;
        }
        let st = self.style(ui, 11.0, Weight::Regular, true);
        let text = self.tr(label).to_owned();
        let avail = ui.available_width();
        let mut resp = naby_render::Response::default();
        ui.row_centered(34.0, |ui| {
            ui.spacing(0.0);
            let r = ui.allocate(Some((avail - 90.0).max(40.0)), 34.0);
            let lh = ui.line_height(&st);
            ui.draw_text(r.x, r.y + (r.h - lh) / 2.0, &text, &st, Some(r.w));
            ui.width(82.0, |ui| {
                resp = ui.text_edit_with(&mut buf, &TextEditOpts::single());
            });
        });
        self.record(&format!("{name} value"), resp.rect);
        let mut result: Option<f64> = None;
        if resp.submitted || (had_focus && !resp.focused) {
            if let Ok(n) = buf.trim().parse::<f64>() {
                if n.is_finite() {
                    result = Some(n.clamp(min, max));
                }
            }
            if result.is_none() {
                buf = String::new();
            }
        }
        ui.gap(8.0);
        let mut v = result.unwrap_or(value) as f32;
        let before = v;
        let r = ui.slider(
            &mut v,
            min as f32..=max.max(min + 0.001) as f32,
            step as f32,
        );
        self.record(name, r.rect);
        self.hand |= r.hovered;
        if r.changed || (v - before).abs() > f32::EPSILON {
            result = Some(f64::from(v));
        }
        self.fields.insert(key, (buf, resp.focused));
        ui.gap(14.0);
        result.filter(|n| (n - value).abs() > 1e-9)
    }
}

#[allow(dead_code)]
fn unused(_: Fill) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readout_matches_the_dock_placement_rules() {
        let mut s = Settings::default();
        s.horizontal = 30.0;
        let r = DockReadout::compute(&s, 1920.0, 1040.0);
        assert!(!r.side && !r.bottom);
        assert_eq!(r.vertical_label, "Top offset · px");
        assert!(!r.vertical_enabled, "a top notch has no vertical offset");
        let (text, centered) = r.alignment();
        assert!(!centered && text.ends_with("center"));

        let mut left = Settings::default();
        left.dock_edge = "Left".into();
        left.dock_inset = 12.0;
        left.dock_vertical = 50.0;
        let r = DockReadout::compute(&left, 1920.0, 1040.0);
        assert!(r.side);
        assert_eq!(r.left, 12.0);
        assert_eq!(r.vertical_max, 100.0);
        assert_eq!(r.alignment(), ("Centered on this screen".to_owned(), true));

        let mut bottom = Settings::default();
        bottom.dock_edge = "Bottom".into();
        bottom.dock_inset = 20.0;
        let r = DockReadout::compute(&bottom, 1920.0, 1040.0);
        assert_eq!(r.vertical_max, 200.0);
        assert_eq!(r.top, 1040.0 - bottom.height - 20.0);
    }
}
