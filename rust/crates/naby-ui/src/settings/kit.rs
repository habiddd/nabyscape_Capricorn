//! The Settings control kit: section cards, switches, slider rows with a number field, choices,
//! expanders and color pickers, drawn to match the WPF `SettingsWindow` helpers (`Section`,
//! `Check`, `AddSlider`, `PremiumChoice`, `ColorControl`, `Advanced`).

use super::help::setting_description;
use super::SettingsView;
use naby_core::color::Rgba;
use naby_core::settings::Settings;
use naby_render::ui::format_color;
use naby_render::{Fill, InputState, Rect, Response, TextEditOpts, TextStyle, Theme, Ui, Weight};

/// Window colors; the high-contrast palette collapses them onto the system colors.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Pal {
    pub window: Rgba,
    pub nav: Rgba,
    pub nav_border: Rgba,
    pub footer: Rgba,
    pub search_bg: Rgba,
    pub search_border: Rgba,
    pub section_bg: Rgba,
    pub section_border: Rgba,
    pub link_bg: Rgba,
    pub stage: Rgba,
    pub results_bg: Rgba,
    pub results_border: Rgba,
    pub hc: bool,
}

fn hex(s: &str) -> Rgba {
    Rgba::parse_hex(s).unwrap_or(Rgba::BLACK)
}

impl Pal {
    pub fn of(theme: &Theme) -> Pal {
        if theme.high_contrast {
            let w = theme.background;
            let t = theme.text;
            return Pal {
                window: w,
                nav: w,
                nav_border: t,
                footer: w,
                search_bg: w,
                search_border: t,
                section_bg: w,
                section_border: t,
                link_bg: w,
                stage: w,
                results_bg: w,
                results_border: t,
                hc: true,
            };
        }
        Pal {
            window: hex("#111214"),
            nav: hex("#141619"),
            nav_border: hex("#24262B"),
            footer: hex("#181B20"),
            search_bg: hex("#1A1D23"),
            search_border: hex("#30353F"),
            section_bg: hex("#17191D"),
            section_border: hex("#2A2E36"),
            link_bg: hex("#1B1E24"),
            stage: hex("#202226"),
            results_bg: hex("#181B21"),
            results_border: hex("#3C4350"),
            hc: false,
        }
    }
}

/// An input frame without pointer or key events (the window behind a modal dialog).
pub(crate) fn blind(input: &InputState) -> InputState {
    // `down` stays set so this frame's end does not drop the press the overlay captured.
    InputState {
        time: input.time,
        down: true,
        ..InputState::default()
    }
}

/// `value.ToString("0.##", InvariantCulture)`.
pub(crate) fn num(value: f64) -> String {
    let mut text = format!("{:.2}", if value.abs() < 0.005 { 0.0 } else { value });
    if text.contains('.') {
        while text.ends_with('0') {
            text.pop();
        }
        if text.ends_with('.') {
            text.pop();
        }
    }
    text
}

/// Draws a filled triangle (expander arrows) through the canvas path API.
pub(crate) fn triangle(ui: &mut Ui, points: [(f32, f32); 3], color: Rgba) {
    let mut pb = tiny_skia::PathBuilder::new();
    pb.move_to(points[0].0, points[0].1);
    pb.line_to(points[1].0, points[1].1);
    pb.line_to(points[2].0, points[2].1);
    pb.close();
    if let Some(path) = pb.finish() {
        ui.canvas().fill_path(&path, &Fill::Solid(color));
    }
}

impl SettingsView {
    // ---- text ---------------------------------------------------------------------------

    pub(crate) fn pal(&self, ui: &Ui) -> Pal {
        Pal::of(ui.theme())
    }

    pub(crate) fn style(&self, ui: &Ui, size: f32, weight: Weight, muted: bool) -> TextStyle {
        let th = ui.theme();
        ui.text_style(size, weight, if muted { th.text_dim } else { th.text })
    }

    /// `UI.Caption(text, size, Muted)`: translated, muted, wrapped.
    pub(crate) fn muted(&self, ui: &mut Ui, text: &str, size: f32) {
        let st = self.style(ui, size, Weight::Regular, true);
        ui.label_styled(self.tr(text), &st);
    }

    /// Final (already formatted) muted text.
    pub(crate) fn muted_final(&self, ui: &mut Ui, text: &str, size: f32) {
        let st = self.style(ui, size, Weight::Regular, true);
        ui.label_styled(text, &st);
    }

    /// Primary text, translated.
    pub(crate) fn text(&self, ui: &mut Ui, text: &str, size: f32, weight: Weight) {
        let st = self.style(ui, size, weight, false);
        ui.label_styled(self.tr(text), &st);
    }

    /// Primary text that is already final.
    pub(crate) fn text_final(&self, ui: &mut Ui, text: &str, size: f32, weight: Weight) {
        let st = self.style(ui, size, weight, false);
        ui.label_styled(text, &st);
    }

    /// Triggers the preview focus for the control `name` once per hover/focus change.
    pub(crate) fn wire(&mut self, resp: &Response, name: &str) {
        if resp.hovered {
            self.hand = true;
        }
        if (resp.hovered || resp.focused) && self.wired != name {
            self.wired = name.to_owned();
            self.configure_preview_for_setting(name);
            self.repaint = true;
        }
    }

    /// Remembers where a control or section sits so a search result can scroll to it.
    pub(crate) fn register(&mut self, key: &str, y: f32) {
        self.positions
            .insert(key.to_owned(), y - self.content_origin);
    }

    #[cfg(test)]
    pub(crate) fn record(&mut self, key: &str, rect: Rect) {
        self.rects.insert(key.to_owned(), rect);
    }

    #[cfg(not(test))]
    pub(crate) fn record(&mut self, _key: &str, _rect: Rect) {}

    // ---- sections -----------------------------------------------------------------------

    /// `Section(title, ...)`: a card with a 16 px semibold heading. Sections the current
    /// category does not show are skipped without running their body.
    pub(crate) fn section<'a>(
        &mut self,
        ui: &mut Ui<'a>,
        title: &str,
        body: impl FnOnce(&mut Self, &mut Ui<'a>),
    ) {
        if !super::categories::shows_section(&self.current, title) {
            return;
        }
        let pal = self.pal(ui);
        let prev = self.section_h.get(title).copied().unwrap_or(0.0);
        let outer = ui.allocate(None, prev);
        self.register(&format!("s:{title}"), outer.y);
        self.last_section = title.to_owned();
        if prev > 0.0 && ui.visible(outer) {
            let c = ui.canvas();
            c.fill_rrect(outer, 12.0, pal.section_bg);
            c.stroke_round_rect(outer, 12.0, pal.section_border, 1.0);
        }
        let inner = Rect::new(
            outer.x + 16.0,
            outer.y + 16.0,
            (outer.w - 32.0).max(0.0),
            100_000.0,
        );
        let mut end_y = inner.y;
        let heading = super::categories::friendly_section(title);
        ui.child(inner, |ui| {
            ui.spacing(0.0);
            let st = self.style(ui, 16.0, Weight::Semibold, false);
            ui.label_styled(self.tr(heading), &st);
            ui.gap(12.0);
            body(self, ui);
            end_y = ui.cursor_pos().1;
        });
        let h = end_y - outer.y + 16.0;
        if (h - prev).abs() > 0.5 {
            self.section_h.insert(title.to_owned(), h);
            self.repaint = true;
        }
        ui.gap(15.0);
    }

    /// `Advanced(title, content)` / the "More details" drawer: a header that shows or hides
    /// `body`.
    pub(crate) fn expander<'a>(
        &mut self,
        ui: &mut Ui<'a>,
        key: &str,
        header: &str,
        body: impl FnOnce(&mut Self, &mut Ui<'a>),
    ) {
        let open = self.open.contains(key);
        ui.gap(8.0);
        self.register(&format!("x:{key}"), ui.cursor_pos().1);
        let rect = ui.allocate(None, 30.0);
        let resp = ui.hit_area(rect, true);
        self.record(key, rect);
        self.wire(&resp, key);
        let th = ui.theme().clone();
        if resp.clicked {
            if open {
                self.open.remove(key);
            } else {
                self.open.insert(key.to_owned());
            }
            self.repaint = true;
        }
        if resp.hovered {
            ui.canvas().fill_rrect(rect, 8.0, th.hover_fill);
        }
        let (cx, cy) = (rect.x + 12.0, rect.y + rect.h / 2.0);
        if open {
            triangle(
                ui,
                [(cx - 4.0, cy - 2.5), (cx + 4.0, cy - 2.5), (cx, cy + 3.0)],
                th.text,
            );
        } else {
            triangle(
                ui,
                [(cx - 2.5, cy - 4.0), (cx + 3.0, cy), (cx - 2.5, cy + 4.0)],
                th.text,
            );
        }
        let st = ui.text_style(12.0, Weight::Regular, th.text);
        let lh = ui.line_height(&st);
        ui.draw_text(
            rect.x + 28.0,
            rect.y + (rect.h - lh) / 2.0,
            header,
            &st,
            Some(rect.w - 36.0),
        );
        ui.paint_focus_ring(&resp, 8.0);
        if open {
            ui.gap(4.0);
            body(self, ui);
        }
    }

    /// `Advanced · {title}` drawer with a translated title.
    pub(crate) fn advanced<'a>(
        &mut self,
        ui: &mut Ui<'a>,
        title: &str,
        body: impl FnOnce(&mut Self, &mut Ui<'a>),
    ) {
        let header = format!("{} · {}", self.tr("Advanced"), self.tr(title));
        self.expander(ui, title, &header, body);
    }

    // ---- switch -------------------------------------------------------------------------

    /// `Check(name, value, change)`: the label with its On/Off state, the explanation and a
    /// switch at the right. Returns true when it was toggled this frame.
    pub(crate) fn check(
        &mut self,
        ui: &mut Ui,
        name: &str,
        value: bool,
        set: impl FnOnce(&mut Settings, bool),
    ) -> bool {
        let state = if value { "On" } else { "Off" };
        let label = format!("{} · {}", self.tr(name), self.tr(state));
        self.switch_row(ui, name, &label, value, set)
    }

    /// A switch whose label has no On/Off suffix (a plain `CheckBox`).
    pub(crate) fn check_plain(
        &mut self,
        ui: &mut Ui,
        name: &str,
        value: bool,
        set: impl FnOnce(&mut Settings, bool),
    ) -> bool {
        let label = self.tr(name).to_owned();
        self.switch_row(ui, name, &label, value, set)
    }

    /// The switch row shared by [`Self::check`] and [`Self::check_plain`].
    fn switch_row(
        &mut self,
        ui: &mut Ui,
        name: &str,
        label: &str,
        value: bool,
        set: impl FnOnce(&mut Settings, bool),
    ) -> bool {
        let help = setting_description(name);
        let th = ui.theme().clone();
        let avail = ui.available_width();
        let text_w = (avail - 58.0).max(40.0);
        let st = self.style(ui, 12.0, Weight::Regular, false);
        let hst = self.style(ui, 11.0, Weight::Regular, true);
        let lh = ui.line_height(&st);
        let hlh = ui.line_height(&hst);
        let label_lines = ui.fonts().wrap(label, &st, text_w);
        let help_text = self.tr(&help).to_owned();
        let help_lines = if help.is_empty() {
            Vec::new()
        } else {
            ui.fonts().wrap(&help_text, &hst, text_w)
        };
        let content_h = label_lines.len() as f32 * lh
            + if help_lines.is_empty() {
                0.0
            } else {
                4.0 + help_lines.len() as f32 * hlh
            };
        let h = (content_h + 14.0).max(40.0);
        self.register(&format!("c:{name}"), ui.cursor_pos().1);
        let rect = ui.allocate(None, h);
        let resp = ui.hit_area(rect, true);
        self.record(name, rect);
        self.wire(&resp, name);
        if !help.is_empty() {
            ui.tooltip(&resp, &help_text);
        }
        let mut toggled = false;
        if resp.clicked {
            set(&mut self.s, !value);
            self.configure_preview_for_setting(name);
            self.save();
            toggled = true;
        }
        if ui.visible(rect) {
            let mut y = rect.y + (h - content_h) / 2.0;
            for line in &label_lines {
                ui.draw_text(rect.x, y, line, &st, None);
                y += lh;
            }
            if !help_lines.is_empty() {
                y += 4.0;
                for line in &help_lines {
                    ui.draw_text(rect.x, y, line, &hst, None);
                    y += hlh;
                }
            }
            let track = Rect::new(
                rect.right() - 40.0,
                rect.y + (rect.h - 20.0) / 2.0,
                34.0,
                20.0,
            );
            let c = ui.canvas();
            c.fill_rrect(
                track,
                10.0,
                if value {
                    th.checked_fill
                } else {
                    th.track_fill
                },
            );
            let kx = if value {
                track.right() - 17.0
            } else {
                track.x + 3.0
            };
            c.fill_circle(
                kx + 7.0,
                track.y + 10.0,
                7.0,
                &Fill::Solid(if value {
                    th.checked_text
                } else {
                    th.switch_knob
                }),
            );
            if resp.hovered {
                c.fill_rrect(track, 10.0, th.hover_fill);
            }
            if th.high_contrast {
                c.stroke_round_rect(track, 10.0, th.text, 1.0);
            }
            ui.paint_focus_ring(&resp, 8.0);
        }
        toggled
    }

    // ---- slider row ---------------------------------------------------------------------

    /// `AddSlider(label, value, min, max, change, unit, step)`: the label, a number field and
    /// the unit on one row, the slider below and the explanation under it.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn slider(
        &mut self,
        ui: &mut Ui,
        label: &str,
        value: f64,
        min: f64,
        max: f64,
        unit: &str,
        step: f64,
        set: impl FnOnce(&mut Settings, f64),
    ) {
        let help = setting_description(label);
        let unit_text = unit.trim().to_owned();
        let st = self.style(ui, 12.0, Weight::Regular, false);
        let ust = self.style(ui, 10.0, Weight::Regular, true);
        let unit_w = if unit_text.is_empty() {
            0.0
        } else {
            ui.fonts().measure_width(&unit_text, &ust) + 7.0
        };
        let avail = ui.available_width();
        let label_w = (avail - 72.0 - unit_w - 4.0).max(40.0);
        ui.gap(2.0);
        let key = format!("n:{label}");
        let (mut buf, had_focus) = self
            .fields
            .get(&key)
            .cloned()
            .unwrap_or_else(|| (num(value), false));
        if !had_focus {
            buf = num(value);
        }
        let mut number_resp = Response::default();
        self.register(&format!("c:{label}"), ui.cursor_pos().1);
        let title = self.tr(label).to_owned();
        ui.row_centered(34.0, |ui| {
            ui.spacing(0.0);
            let r = ui.allocate(Some(label_w), 34.0);
            let lh = ui.line_height(&st);
            ui.draw_text(r.x, r.y + (r.h - lh) / 2.0, &title, &st, Some(r.w));
            ui.width(72.0, |ui| {
                number_resp = ui.text_edit_with(&mut buf, &TextEditOpts::single());
            });
            if !unit_text.is_empty() {
                ui.gap(7.0);
                ui.label_styled(&unit_text, &ust);
            }
        });
        self.record(&format!("{label} value"), number_resp.rect);
        let tip = format!("{} to {} {}", num(min), num(max), unit_text);
        ui.tooltip(&number_resp, tip.trim());
        let mut new_value: Option<f64> = None;
        let commit = number_resp.submitted || (had_focus && !number_resp.focused);
        if commit {
            match buf.trim().parse::<f64>() {
                Ok(n) if n.is_finite() => {
                    let snapped = if step > 0.0 {
                        (n / step).round() * step
                    } else {
                        n
                    };
                    new_value = Some(snapped.clamp(min, max));
                }
                _ => {
                    let msg = range_message(self, min, max, &unit_text);
                    self.set_feedback(&msg, false);
                    buf = num(value);
                }
            }
        }
        ui.gap(6.0);
        let mut v = new_value.unwrap_or(value) as f32;
        let before = v;
        let resp = ui.slider(&mut v, min as f32..=max as f32, step as f32);
        self.record(label, resp.rect);
        if resp.changed || (v - before).abs() > f32::EPSILON {
            new_value = Some(f64::from(v));
        }
        self.wire(&resp, label);
        self.wire(&number_resp, label);
        match new_value {
            Some(n) if (n - value).abs() > 1e-9 => {
                set(&mut self.s, n);
                self.configure_preview_for_setting(label);
                self.slider_preview_mode(label);
                buf = num(n);
                self.save();
            }
            _ => {}
        }
        self.fields.insert(key, (buf, number_resp.focused));
        if help.is_empty() {
            ui.gap(14.0);
        } else {
            ui.gap(8.0);
            let translated = self.tr(&help).to_owned();
            self.muted_final(ui, &translated, 11.0);
            ui.gap(16.0);
        }
    }

    // ---- choices ------------------------------------------------------------------------

    /// A drop-down with no caption (a plain `ComboBox`). `options` are (English label, value).
    pub(crate) fn combo_plain(
        &mut self,
        ui: &mut Ui,
        name: &str,
        options: &[(&str, &str)],
        current: &str,
        set: impl FnOnce(&mut Settings, &str),
    ) {
        let labels: Vec<String> = options.iter().map(|(l, _)| self.tr(l).to_owned()).collect();
        let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        let mut idx = options.iter().position(|(_, v)| *v == current).unwrap_or(0);
        self.register(&format!("c:{name}"), ui.cursor_pos().1);
        ui.gap(4.0);
        let resp = ui.combo(&mut idx, &refs);
        self.record(name, resp.rect);
        self.wire(&resp, name);
        let help = setting_description(name);
        if !help.is_empty() {
            let tip = self.tr(&help).to_owned();
            ui.tooltip(&resp, &tip);
        }
        if resp.changed {
            if let Some((_, value)) = options.get(idx) {
                set(&mut self.s, value);
                self.configure_preview_for_setting(name);
                self.save();
            }
        }
        ui.gap(10.0);
    }

    /// `PremiumChoice(name, value, values, change)`: a muted caption over a drop-down.
    pub(crate) fn choice(
        &mut self,
        ui: &mut Ui,
        name: &str,
        caption: &str,
        options: &[(&str, &str)],
        current: &str,
        set: impl FnOnce(&mut Settings, &str),
    ) {
        ui.gap(4.0);
        self.muted(ui, caption, 11.0);
        self.combo_plain(ui, name, options, current, set);
    }

    /// Options whose label equals their value.
    pub(crate) fn choice_same(
        &mut self,
        ui: &mut Ui,
        name: &str,
        caption: &str,
        values: &[&str],
        current: &str,
        set: impl FnOnce(&mut Settings, &str),
    ) {
        let options: Vec<(&str, &str)> = values.iter().map(|v| (*v, *v)).collect();
        self.choice(ui, name, caption, &options, current, set);
    }

    // ---- colors -------------------------------------------------------------------------

    /// `ColorControl(parent, name, value, change, palette)`.
    pub(crate) fn color_control(
        &mut self,
        ui: &mut Ui,
        name: &str,
        value: &str,
        presets: &[&str],
        set: impl FnOnce(&mut Settings, String),
    ) {
        self.muted(ui, name, 12.0);
        ui.gap(8.0);
        let mut color = Rgba::parse_hex(value).unwrap_or(Rgba::BLACK);
        let presets: Vec<Rgba> = presets.iter().filter_map(|p| Rgba::parse_hex(p)).collect();
        let recent: Vec<Rgba> = self
            .s
            .recent_colors
            .iter()
            .filter_map(|c| Rgba::parse_hex(c))
            .collect();
        self.register(&format!("c:{name}"), ui.cursor_pos().1);
        let resp = ui.color_picker(&mut color, &presets, &recent);
        self.record(name, resp.rect);
        self.wire(&resp, name);
        let hex = format_color(color);
        if resp.changed || resp.committed {
            if !hex.eq_ignore_ascii_case(value) {
                set(&mut self.s, hex.clone());
            }
            if resp.committed {
                self.s
                    .recent_colors
                    .retain(|c| !c.eq_ignore_ascii_case(&hex));
                self.s.recent_colors.insert(0, hex);
                self.s.recent_colors.truncate(8);
            }
            self.configure_preview_for_setting(name);
            self.save();
        }
        ui.gap(16.0);
    }

    // ---- buttons ------------------------------------------------------------------------

    /// A caption button at its natural width (`UI.CaptionButton` inside a section).
    pub(crate) fn button(&mut self, ui: &mut Ui, text: &str) -> bool {
        ui.gap(8.0);
        let label = self.tr(text).to_owned();
        let mut clicked = false;
        let mut hot = false;
        let mut rect = Rect::default();
        ui.row(|ui| {
            let r = ui.button(&label);
            hot = r.hovered;
            clicked = r.clicked;
            rect = r.rect;
        });
        self.hand |= hot;
        self.record(text, rect);
        clicked
    }

    /// A caption button that can be disabled (dimmed, no interaction).
    pub(crate) fn button_enabled(&mut self, ui: &mut Ui, text: &str, enabled: bool) -> bool {
        ui.gap(8.0);
        let label = self.tr(text).to_owned();
        let mut clicked = false;
        let mut hot = false;
        let mut rect = Rect::default();
        ui.enabled(enabled, |ui| {
            ui.row(|ui| {
                let r = ui.button(&label);
                hot = r.hovered;
                clicked = r.clicked;
                rect = r.rect;
            });
        });
        self.hand |= hot && enabled;
        self.record(text, rect);
        clicked
    }

    pub(crate) fn primary_button(&mut self, ui: &mut Ui, text: &str) -> bool {
        ui.gap(8.0);
        let label = self.tr(text).to_owned();
        let mut clicked = false;
        let mut hot = false;
        let mut rect = Rect::default();
        ui.row(|ui| {
            let r = ui.primary_button(&label);
            hot = r.hovered;
            clicked = r.clicked;
            rect = r.rect;
        });
        self.hand |= hot;
        self.record(text, rect);
        clicked
    }

    /// Buttons wrapping over rows (`WrapPanel`); `(label, enabled)`; returns the index clicked.
    pub(crate) fn wrap_buttons(&mut self, ui: &mut Ui, labels: &[(&str, bool)]) -> Option<usize> {
        ui.gap(8.0);
        let width = ui.available_width();
        let mut clicked = None;
        let mut rows: Vec<Vec<usize>> = Vec::new();
        let mut row: Vec<usize> = Vec::new();
        let mut used = 0.0;
        let st = self.style(ui, 13.0, Weight::Regular, false);
        for (i, (label, _)) in labels.iter().enumerate() {
            let w = ui.fonts().measure_width(self.tr(label), &st) + 28.0 + 8.0;
            if !row.is_empty() && used + w > width {
                rows.push(std::mem::take(&mut row));
                used = 0.0;
            }
            used += w;
            row.push(i);
        }
        if !row.is_empty() {
            rows.push(row);
        }
        let mut hot = false;
        let mut rects: Vec<(usize, Rect)> = Vec::new();
        for (ri, r) in rows.iter().enumerate() {
            if ri > 0 {
                ui.gap(8.0);
            }
            ui.row(|ui| {
                ui.spacing(8.0);
                for &i in r {
                    let (label, enabled) = labels[i];
                    let text = self.tr(label).to_owned();
                    ui.push_id(i, |ui| {
                        ui.enabled(enabled, |ui| {
                            let resp = ui.button(&text);
                            hot |= resp.hovered && enabled;
                            rects.push((i, resp.rect));
                            if resp.clicked {
                                clicked = Some(i);
                            }
                        });
                    });
                }
            });
        }
        self.hand |= hot;
        for (i, rect) in rects {
            self.record(labels[i].0, rect);
        }
        clicked
    }

    /// A single-line text field with its buffer kept in the view (`UI.Input`).
    pub(crate) fn field(
        &mut self,
        ui: &mut Ui,
        key: &str,
        hint: &str,
        invalid: bool,
    ) -> (Response, String) {
        let mut buf = self
            .fields
            .get(key)
            .map(|(t, _)| t.clone())
            .unwrap_or_default();
        let hint = self.tr(hint).to_owned();
        let resp = ui.text_edit_with(
            &mut buf,
            &TextEditOpts::single().hint(&hint).invalid(invalid),
        );
        self.record(key, resp.rect);
        self.fields
            .insert(key.to_owned(), (buf.clone(), resp.focused));
        (resp, buf)
    }
}

fn range_message(view: &SettingsView, min: f64, max: f64, unit: &str) -> String {
    naby_core::i18n::format(
        &view.lang,
        "Enter a number from {0} to {1} {2}.",
        &[&num(min), &num(max), &unit],
    )
}
