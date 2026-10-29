//! The live-preview rail (C# `SettingsCustomization.RenderSettingsPreview` and the rail built in
//! the `SettingsWindow` constructor): mode buttons, the stage with a miniature island drawn from
//! the current preferences, its size, the sample chooser and "Replay transition".

use super::categories::has_preview;
use super::kit::Pal;
use super::preview_motion::{PreviewMode, CONTENTS};
use super::SettingsView;
use chrono::Local;
use naby_core::color::Rgba;
use naby_core::motion::Contour;
use naby_core::settings::Settings;
use naby_render::{Canvas, Fill, Fonts, Icon, Rect, TextStyle, Theme, Ui, Weight};

/// Drawing context for one miniature island. Coordinates are island-local layout pixels;
/// `k` is the panel content scale (`Settings.PanelScale`).
pub(crate) struct Pv<'c> {
    pub c: &'c mut Canvas,
    pub fonts: &'c Fonts,
    pub th: Theme,
    pub s: &'c Settings,
    pub snaps: Option<&'c crate::host::Snapshots>,
    pub lang: &'c str,
    pub k: f32,
    pub now: f64,
}

impl Pv<'_> {
    pub fn t<'k>(&self, key: &'k str) -> &'k str {
        naby_core::i18n::t(self.lang, key)
    }

    pub fn accent(&self) -> Rgba {
        Rgba::parse_hex(&self.s.accent).unwrap_or(self.th.accent)
    }

    pub fn white(&self) -> Rgba {
        self.th.text
    }

    pub fn dim(&self) -> Rgba {
        self.th.text_dim
    }

    pub fn style(&self, size: f32, bold: bool, color: Rgba) -> TextStyle {
        TextStyle {
            size: size * self.k,
            weight: if bold {
                Weight::Semibold
            } else {
                Weight::Regular
            },
            color,
        }
    }

    /// Draws text at (x, y) (scaled by `k`); returns the line height in unscaled units.
    #[allow(clippy::too_many_arguments)]
    pub fn text(
        &mut self,
        x: f32,
        y: f32,
        text: &str,
        size: f32,
        bold: bool,
        color: Rgba,
        max_w: Option<f32>,
    ) -> f32 {
        let st = self.style(size, bold, color);
        self.c.text(self.fonts, x, y, text, &st, max_w);
        self.fonts.line_height(&st) / self.k.max(0.01)
    }

    pub fn line_h(&self, size: f32) -> f32 {
        let st = self.style(size, false, Rgba::WHITE);
        self.fonts.line_height(&st) / self.k.max(0.01)
    }

    pub fn rr(&mut self, r: Rect, radius: f32, color: Rgba) {
        self.c.fill_rrect(r, radius, color);
    }

    pub fn outline(&mut self, r: Rect, radius: f32, color: Rgba) {
        self.c.stroke_round_rect(r, radius, color, 1.0);
    }

    /// A 3 px progress bar (`ProgressBar` template).
    pub fn bar(&mut self, x: f32, y: f32, w: f32, fraction: f32) {
        let track = Rect::new(x, y, w, 3.0);
        self.c.fill_rrect(track, 1.5, self.th.progress_track);
        self.c.fill_rrect(
            Rect::new(x, y, (w * fraction.clamp(0.0, 1.0)).max(1.5), 3.0),
            1.5,
            self.th.progress_fill,
        );
    }

    pub fn icon(&mut self, icon: Icon, x: f32, y: f32, size: f32, color: Rgba) {
        self.c.icon(icon, Rect::new(x, y, size, size), color, 1.6);
    }

    /// A 34x20 switch.
    pub fn switch(&mut self, x: f32, y: f32, on: bool) {
        let track = Rect::new(x, y, 34.0, 20.0);
        self.c.fill_rrect(
            track,
            10.0,
            if on {
                self.th.checked_fill
            } else {
                self.th.track_fill
            },
        );
        let kx = if on {
            track.right() - 17.0
        } else {
            track.x + 3.0
        };
        self.c.fill_circle(
            kx + 7.0,
            track.y + 10.0,
            7.0,
            &Fill::Solid(if on {
                self.th.checked_text
            } else {
                self.th.switch_knob
            }),
        );
    }

    /// Decorative waveform bars (`ActivityVisual` at rest).
    pub fn wave(&mut self, x: f32, y: f32, w: f32, h: f32, color: Rgba) {
        let n = ((w / 3.6) as usize).clamp(4, 25);
        let step = w / n as f32;
        for i in 0..n {
            let a = (i as f32 * 0.37 + 1.0).sin();
            let bar_h = (3.0 + (h - 3.0) * a * a * (0.7 + 0.3 * (i as f32 * 0.13).cos())).max(2.0);
            self.c.fill_rrect(
                Rect::new(x + i as f32 * step, y + (h - bar_h) / 2.0, 1.6, bar_h),
                0.8,
                color,
            );
        }
    }

    pub fn clock_text(&self) -> String {
        let now = Local::now();
        if self.s.clock24_hour {
            now.format("%H:%M").to_string()
        } else {
            now.format("%-I:%M %p").to_string()
        }
    }

    pub fn media_title(&self) -> (String, String) {
        match self.snaps {
            Some(s) if s.media.available => (s.media.title.clone(), s.media.artist.clone()),
            _ => (
                self.t("Nothing playing").to_owned(),
                self.t("Windows media").to_owned(),
            ),
        }
    }
}

impl SettingsView {
    /// The sample text shown for each "Preview content" entry.
    fn content_label(&self, name: &str) -> String {
        self.tr(name).to_owned()
    }

    /// The full right column (not narrow windows).
    pub(super) fn preview_rail<'a>(&mut self, ui: &mut Ui<'a>, rail: Rect, pal: &Pal) {
        let th = ui.theme().clone();
        // The "Live preview" expander header.
        let header = Rect::new(rail.x + 8.0, rail.y, rail.w - 8.0, 30.0);
        let hs = ui.text_style(12.0, Weight::Regular, th.text_dim);
        let title = self.tr("Live preview").to_owned();
        ui.draw_text(
            header.x + 20.0,
            header.y + 7.0,
            &title,
            &hs,
            Some(header.w - 24.0),
        );
        let body = Rect::new(
            rail.x + 8.0,
            rail.y + 30.0,
            rail.w - 8.0,
            (rail.h - 30.0).max(0.0),
        );
        ui.child(body, |ui| {
            ui.scroll_area("rail", |ui| {
                ui.spacing(0.0);
                ui.row(|ui| {
                    ui.spacing(0.0);
                    ui.gap(18.0);
                    ui.column_w((body.w - 36.0 - 14.0).max(80.0), |ui| {
                        ui.spacing(0.0);
                        ui.gap(18.0);
                        self.preview_column(ui, pal, false);
                        ui.gap(18.0);
                    });
                });
            });
        });
    }

    /// The narrow-window strip: a collapsible preview under the page header. Returns the
    /// height it used.
    pub(super) fn preview_strip<'a>(&mut self, ui: &mut Ui<'a>, area: Rect, pal: &Pal) -> f32 {
        let th = ui.theme().clone();
        let head = Rect::new(area.x, area.y, area.w, 30.0);
        let resp = ui.hit_area(head, true);
        self.record("Settings live preview", head);
        self.hand |= resp.hovered;
        if resp.clicked {
            self.pv.strip_open = !self.pv.strip_open;
            self.repaint = true;
        }
        let open = self.pv.strip_open;
        let (cx, cy) = (head.x + 8.0, head.y + head.h / 2.0);
        if open {
            super::kit::triangle(
                ui,
                [(cx - 4.0, cy - 2.5), (cx + 4.0, cy - 2.5), (cx, cy + 3.0)],
                th.text_dim,
            );
        } else {
            super::kit::triangle(
                ui,
                [(cx - 2.5, cy - 4.0), (cx + 3.0, cy), (cx - 2.5, cy + 4.0)],
                th.text_dim,
            );
        }
        let st = ui.text_style(12.0, Weight::Regular, th.text_dim);
        let title = self.tr("Live preview").to_owned();
        ui.draw_text(
            head.x + 24.0,
            head.y + 7.0,
            &title,
            &st,
            Some(head.w - 28.0),
        );
        ui.paint_focus_ring(&resp, 6.0);
        if !open {
            return 30.0;
        }
        let height = (area.h - 30.0 - 100.0).clamp(50.0, 210.0);
        let stage = Rect::new(area.x, head.bottom() + 4.0, area.w.min(520.0), height);
        self.draw_stage(ui, stage, pal);
        30.0 + 4.0 + height + 8.0
    }

    /// Everything in the rail below the header, laid out as a column of width
    /// `ui.available_width()`.
    fn preview_column<'a>(&mut self, ui: &mut Ui<'a>, pal: &Pal, _compact: bool) {
        let th = ui.theme().clone();
        let w = ui.available_width();
        // What the preview focuses on.
        ui.gap(0.0);
        let focus = if self.pv.focus.is_empty() {
            self.tr("Point to a setting to explore it").to_owned()
        } else {
            self.tr(&self.pv.focus.clone()).to_owned()
        };
        let st = self.style(ui, 11.0, Weight::Regular, true);
        ui.gap(7.0);
        ui.label_styled(&focus, &st);
        ui.gap(14.0);
        // Closed / Hover / Panel.
        let row = ui.allocate(None, 30.0);
        let seg = row.w / 3.0;
        for (i, mode) in PreviewMode::ALL.iter().enumerate() {
            let r = Rect::new(row.x + i as f32 * seg, row.y, seg - 4.0, row.h);
            let selected = self.pv.mode == *mode;
            ui.push_id(i, |ui| {
                let resp = ui.hit_area(r, true);
                self.record(&format!("Preview {}", mode.name()), r);
                self.hand |= resp.hovered;
                let c = ui.canvas();
                if selected {
                    c.fill_rrect(
                        r,
                        7.0,
                        Rgba::parse_hex("#34373E").unwrap_or(th.selection_fill),
                    );
                } else if resp.hovered {
                    c.fill_rrect(r, 7.0, th.hover_fill);
                }
                let st = ui.text_style(
                    10.0,
                    Weight::Regular,
                    if selected { th.text } else { th.text_dim },
                );
                let label = self.tr(mode.name()).to_owned();
                let tw = ui.fonts().measure_width(&label, &st);
                let lh = ui.line_height(&st);
                ui.draw_text(
                    r.x + (r.w - tw) / 2.0,
                    r.y + (r.h - lh) / 2.0,
                    &label,
                    &st,
                    Some(r.w - 6.0),
                );
                ui.paint_focus_ring(&resp, 7.0);
                if resp.clicked {
                    self.pv.mode = *mode;
                    self.repaint = true;
                }
            });
        }
        ui.gap(12.0);
        // The stage.
        let stage = ui.allocate(None, 238.0);
        self.draw_stage(ui, stage, pal);
        ui.gap(12.0);
        // Dimensions line.
        let ((tw, thh), detail) = self.preview_target();
        let dims = if detail {
            self.tr("Detail preview · your current appearance")
                .to_owned()
        } else {
            format!(
                "{:.0} × {:.0} px · {}",
                tw,
                thh,
                self.tr(if self.s.preset == "Mac" {
                    "Notch"
                } else {
                    "Pill"
                })
            )
        };
        self.muted_final(ui, &dims, 11.0);
        ui.gap(18.0);
        self.muted(ui, "Preview content", 11.0);
        ui.gap(7.0);
        let labels: Vec<String> = CONTENTS.iter().map(|c| self.content_label(c)).collect();
        let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        let mut idx = CONTENTS
            .iter()
            .position(|c| *c == self.pv.content)
            .unwrap_or(0);
        let resp = ui.combo(&mut idx, &refs);
        self.record("Preview content", resp.rect);
        self.hand |= resp.hovered;
        if resp.changed {
            self.pv.content = CONTENTS[idx].to_owned();
            self.repaint = true;
        }
        ui.gap(15.0);
        let hint = if self.current == "AI" {
            "Preview only. Apply connection edits with Save provider."
        } else {
            "Changes apply automatically. The preview uses your current shape, size and colors."
        };
        self.muted(ui, hint, 11.0);
        ui.gap(12.0);
        let label = self.tr("Replay transition").to_owned();
        let mut clicked = false;
        ui.row(|ui| {
            let r = ui.button(&label);
            self.hand |= r.hovered;
            self.record("Replay transition", r.rect);
            clicked = r.clicked;
        });
        if clicked {
            let step = (self.s.animation_ms + 180.0).max(500.0) / 1000.0;
            self.pv.start_replay(self.now, step);
            self.repaint = true;
        }
        let _ = w;
    }

    /// The dark stage with the miniature island fitted into it (Viewbox, down only).
    fn draw_stage<'a>(&mut self, ui: &mut Ui<'a>, stage: Rect, pal: &Pal) {
        let th = ui.theme().clone();
        self.pv.advance_replay(self.now);
        let ((tw, thh), detail) = self.preview_target();
        let animate = !self.s.reduce_motion && self.s.animation_ms > 0.0;
        let (w, h) = self
            .pv
            .size_at(self.now, (tw, thh), self.s.animation_ms, animate);
        {
            let c = ui.canvas();
            c.fill_rrect(stage, 10.0, pal.stage);
            if pal.hc {
                c.stroke_round_rect(stage, 10.0, th.text, 1.0);
            }
        }
        let margin_top = if self.s.preset == "Mac" {
            0.0
        } else {
            self.s.top as f32
        };
        let k = (stage.w / w.max(1.0))
            .min(stage.h / (h + margin_top).max(1.0))
            .min(1.0);
        let ox = stage.x + (stage.w - w * k) / 2.0;
        let oy = stage.y + margin_top * k;
        let snaps = self.snaps.clone();
        let (canvas, fonts) = ui.canvas_fonts();
        let base = canvas.scale();
        canvas.push_clip_round_rect(stage, 10.0);
        canvas.set_scale(base * k);
        let mut pv = Pv {
            c: canvas,
            fonts,
            th: th.clone(),
            s: &self.s,
            snaps: snaps.as_ref(),
            lang: &self.lang,
            k: 1.0,
            now: self.now,
        };
        draw_island(
            &mut pv,
            (ox / k, oy / k),
            (w, h),
            self.pv.mode,
            &self.pv.content,
            detail,
            &self.current,
        );
        pv.c.set_scale(base);
        pv.c.pop_clip();
    }
}

/// `IslandSurface.Material`: the glass colors for a background color.
fn material(background: Rgba) -> [Rgba; 3] {
    if background == Rgba::BLACK {
        return [
            Rgba::rgb(18, 19, 23),
            Rgba::rgb(8, 9, 12),
            Rgba::rgb(1, 2, 3),
        ];
    }
    let lift = |a: u8| {
        Rgba::rgb(
            background.r.saturating_add(a),
            background.g.saturating_add(a),
            background.b.saturating_add(a),
        )
    };
    [lift(18), lift(8), lift(1)]
}

/// Paints the island body and its sample content at `origin` (canvas units).
pub(crate) fn draw_island(
    pv: &mut Pv,
    origin: (f32, f32),
    size: (f32, f32),
    mode: PreviewMode,
    content: &str,
    _detail: bool,
    category: &str,
) {
    let s = pv.s;
    let (w, h) = size;
    let closed = mode == PreviewMode::Closed;
    let hover = mode == PreviewMode::Hover;
    let notch = s.preset == "Mac";
    let contour = Contour::create(f64::from(w), f64::from(h), s.radius, notch, 0.0);
    let background = if pv.th.high_contrast {
        pv.th.background
    } else if closed {
        Rgba::BLACK
    } else {
        Rgba::parse_hex(&s.background).unwrap_or(Rgba::BLACK)
    };
    let opacity = if pv.th.high_contrast {
        1.0
    } else {
        s.opacity as f32
    };
    let glass = s.glass_style && !pv.th.high_contrast;
    let strength = s.glass_strength as f32;
    let (ox, oy) = origin;
    pv.c.with_opacity(opacity, |c| {
        c.fill_contour(&contour, ox, oy, 1.0, &Fill::Solid(background));
    });
    pv.c.push_clip_contour(&contour, ox, oy, 1.0);
    if glass {
        let material = material(Rgba::parse_hex(&s.background).unwrap_or(Rgba::BLACK));
        let alpha = (if closed { 0.45 } else { 1.0 }) * strength;
        let rect = Rect::new(ox, oy, w, h);
        pv.c.with_opacity(alpha * opacity, |c| {
            let fill = Fill::Linear {
                from: (rect.x, rect.y),
                to: (rect.x + 0.65 * rect.w, rect.y + rect.h),
                stops: vec![(0.0, material[0]), (0.46, material[1]), (1.0, material[2])],
            };
            c.fill_contour(&contour, ox, oy, 1.0, &fill);
        });
    }
    draw_ambient(pv, Rect::new(ox, oy, w, h), closed, content);
    // Content.
    let panel_scale = s.panel_scale as f32;
    let (inset_x, inset_top) = if closed {
        (18.0, 0.0)
    } else if hover {
        (32.0, 15.0)
    } else {
        (s.panel_padding as f32, 15.0)
    };
    let mut y = oy + inset_top;
    let x = ox + inset_x;
    let width = w - 2.0 * inset_x;
    if closed || hover {
        let header_h = if closed { h } else { 23.0 };
        draw_header(pv, Rect::new(x, y, width, header_h), closed, content);
        y += header_h;
    }
    if !closed {
        // The detail is a `LayoutTransform` by PanelScale: draw it in scaled space.
        let kk = panel_scale.max(0.1);
        let top = if hover { 12.0 } else { s.section_gap as f32 };
        let base = pv.c.scale();
        pv.c.set_scale(base * kk);
        let area = Rect::new(x / kk, (y + top) / kk, width / kk, 1000.0);
        draw_detail(pv, area, content, hover, category);
        pv.c.set_scale(base);
    }
    pv.c.pop_clip();
}

/// The closed-island / hover header: icon or wave, title, clock.
fn draw_header(pv: &mut Pv, r: Rect, closed: bool, content: &str) {
    let s = pv.s;
    let white = pv.white();
    let bold_head = s.bold_headings;
    let time = pv.clock_text();
    let tw = {
        let st = pv.style(11.0, true, white);
        pv.fonts.measure_width(&time, &st)
    };
    let lh = pv.line_h(11.0);
    let cy = r.y + (r.h - lh) / 2.0;
    pv.text(r.right() - tw, cy, &time, 11.0, true, white, None);
    let mut x = r.x;
    let right_limit = r.right() - tw - 10.0;
    if closed && content == "Volume feedback" {
        let vol = pv.snaps.map_or(0.5, |s| s.audio.volume);
        let label = format!("{} {:.0}%", pv.t("Volume"), vol * 100.0);
        pv.text(x, cy, &label, 11.0, false, white, Some(right_limit - x));
        let lw = {
            let st = pv.style(11.0, false, white);
            pv.fonts.measure_width(&label, &st)
        };
        pv.bar(x + lw + 10.0, r.y + r.h / 2.0 - 1.5, 64.0, vol);
        return;
    }
    if closed && content == "AI" {
        pv.c.fill_circle(
            x + 11.0,
            r.y + r.h / 2.0,
            11.0,
            &Fill::Solid(Rgba::rgb(0x23, 0x26, 0x2D)),
        );
        pv.c.stroke_circle(
            x + 11.0,
            r.y + r.h / 2.0,
            10.5,
            Rgba::rgb(0x77, 0x7D, 0x88),
            1.0,
        );
        pv.rr(
            Rect::new(x + 7.0, r.y + r.h / 2.0 - 4.0, 3.0, 8.0),
            1.5,
            white,
        );
        pv.rr(
            Rect::new(x + 13.0, r.y + r.h / 2.0 - 4.0, 3.0, 8.0),
            1.5,
            white,
        );
        x += 30.0;
        let label = pv.t("Ask Naby").to_owned();
        pv.text(x, cy, &label, 11.0, bold_head, white, Some(right_limit - x));
        return;
    }
    let (title, _) = pv.media_title();
    let music = closed && content == "Music";
    let available = pv.snaps.is_some_and(|sn| sn.media.available);
    if music && s.closed_media_icon == "Artwork" {
        pv.rr(
            Rect::new(x, r.y + (r.h - 20.0) / 2.0, 20.0, 20.0),
            5.0,
            Rgba::rgb(0x73, 0x7D, 0x8C),
        );
        x += 28.0;
    } else if music
        && s.live_activity
        && s.closed_media_icon == "Waveform"
        && s.closed_media_content == "Title"
    {
        pv.wave(x, r.y + (r.h - 14.0) / 2.0, 60.0, 14.0, white);
        x += 68.0;
    }
    let label = if closed && content == "Music" {
        match s.closed_media_content.as_str() {
            "None" => String::new(),
            "Waveform" | "Lyrics" => {
                // The waveform (or the lyric line when one is available).
                pv.wave(
                    x,
                    r.y + (r.h - 12.0) / 2.0,
                    (right_limit - x).clamp(20.0, 72.0),
                    12.0,
                    white,
                );
                return;
            }
            _ => {
                if available {
                    title
                } else {
                    String::new()
                }
            }
        }
    } else {
        pv.t(content).to_owned()
    };
    pv.text(
        x,
        cy,
        &label,
        11.0,
        bold_head,
        white,
        Some((right_limit - x).max(10.0)),
    );
}

/// Ambient glow behind the content (`ambientPreview`), simplified to a soft gradient.
fn draw_ambient(pv: &mut Pv, rect: Rect, closed: bool, content: &str) {
    let s = pv.s;
    if !s.cover_ambient || s.ambient_strength <= 0.0 || (closed && !s.closed_ambient) {
        return;
    }
    let widget = if closed {
        "closed"
    } else {
        match content {
            "Music" => "media",
            "Timer" => "timer",
            "Focus" => "focus",
            "Stopwatch" => "stopwatch",
            "System" => "system",
            "Note" => "notes",
            "Tasks" => "tasks",
            "Calendar" => "calendar",
            "Shelf" => "files",
            "Smart clipboard" => "clips",
            "AI" => "ai",
            "Launch" => "shortcuts",
            _ => "preview",
        }
    };
    let allowed = match s.ambient_scope.as_str() {
        "Music only" => widget == "media" || widget == "closed",
        "Selected widgets" => s.ambient_widgets.iter().any(|w| w == widget),
        _ => true,
    };
    if !allowed {
        return;
    }
    let accent = Rgba::parse_hex(&s.accent).unwrap_or(Rgba::WHITE);
    let color = match s.ambient_color_source.as_str() {
        "Custom color" | "Custom blend" => {
            Rgba::parse_hex(&s.ambient_custom_color).unwrap_or(accent)
        }
        _ => accent,
    };
    let alpha = (s.ambient_strength * 0.55).clamp(0.0, 1.0) as f32;
    let reach = (s.ambient_height as f32).clamp(0.25, 2.0);
    let strong = Rgba {
        a: (255.0 * alpha) as u8,
        ..color
    };
    let clear = Rgba { a: 0, ..color };
    let (from, to, band) = match s.ambient_direction.as_str() {
        "Top" => ((0.0, rect.y), (0.0, rect.y + rect.h * 0.5 * reach), rect),
        "Left" => ((rect.x, 0.0), (rect.x + rect.w * 0.4 * reach, 0.0), rect),
        "Right" => (
            (rect.right(), 0.0),
            (rect.right() - rect.w * 0.4 * reach, 0.0),
            rect,
        ),
        _ => (
            (0.0, rect.bottom()),
            (0.0, rect.bottom() - rect.h * 0.5 * reach),
            rect,
        ),
    };
    let soft = s.ambient_softness as f32;
    let fill = Fill::Linear {
        from,
        to,
        stops: vec![
            (0.0, strong),
            (
                0.35 + 0.3 * soft,
                strong.with_alpha((255.0 * alpha * 0.5) as u8),
            ),
            (1.0, clear),
        ],
    };
    pv.c.fill_round_rect(band, 0.0, &fill);
}

/// The panel / hover body for a sample kind.
fn draw_detail(pv: &mut Pv, a: Rect, content: &str, hover: bool, category: &str) {
    let _ = category;
    let white = pv.white();
    let dim = pv.dim();
    let s = pv.s;
    match content {
        "AI" => {
            let cx = a.x + 32.0;
            pv.c.fill_circle(
                cx,
                a.y + 32.0,
                32.0,
                &Fill::Solid(Rgba::rgb(0x23, 0x26, 0x2D)),
            );
            pv.c.stroke_circle(cx, a.y + 32.0, 31.5, Rgba::rgb(0x77, 0x7D, 0x88), 1.0);
            pv.rr(Rect::new(cx - 12.0, a.y + 22.0, 8.0, 20.0), 4.0, white);
            pv.rr(Rect::new(cx + 4.0, a.y + 22.0, 8.0, 20.0), 4.0, white);
            let mut y = a.y + 72.0;
            y += pv.text(a.x, y, "Ask Naby", 17.0, s.bold_headings, white, Some(a.w));
            let sub = if hover {
                "Chat here · Talk"
            } else {
                "Chat and voice in your island"
            };
            let sub = pv.t(sub).to_owned();
            y += pv.text(a.x, y + 4.0, &sub, 12.0, false, dim, Some(a.w)) + 4.0;
            let note = pv
                .t("Mic and screen start only when you choose.")
                .to_owned();
            pv.text(a.x, y + 4.0, &note, 11.0, false, dim, Some(a.w));
        }
        "Stopwatch" => {
            pv.text(a.x, a.y, "00:00", 28.0, true, white, None);
            let label = pv.t("Start").to_owned();
            let rect = Rect::new(a.x, a.y + 44.0, 78.0, 30.0);
            pv.rr(rect, 8.0, pv.th.control_fill);
            pv.text(
                a.x + 20.0,
                a.y + 50.0,
                &label,
                11.0,
                false,
                pv.th.control_text,
                None,
            );
            let reset = pv.t("Reset").to_owned();
            let rect = Rect::new(a.x + 86.0, a.y + 44.0, 78.0, 30.0);
            pv.rr(rect, 8.0, pv.th.control_fill);
            pv.text(
                a.x + 106.0,
                a.y + 50.0,
                &reset,
                11.0,
                false,
                pv.th.control_text,
                None,
            );
        }
        "Visibility" => super::organization::visibility_example(pv, a),
        "Drop zone" | "Detached preview" | "Cover gestures" => {
            super::interaction_features::feature_preview(pv, a, content)
        }
        "Smart clipboard" => super::control_appearance::clipboard_example(pv, a),
        "Control colors" | "Typography" | "Panel navigation" => {
            super::control_appearance::control_example(pv, a, content)
        }
        "Music" => {
            let (title, artist) = pv.media_title();
            let mut y = a.y;
            y += pv.text(a.x, y, &title, 15.0, s.bold_media_title, white, Some(a.w));
            y += 5.0;
            y += pv.text(a.x, y, &artist, 11.0, false, dim, Some(a.w)) + 14.0;
            let accent = pv.accent();
            let mid = a.x + a.w / 2.0;
            pv.icon(
                Icon::Previous,
                mid - 15.0 - 8.0 - 30.0,
                y + 4.0,
                16.0,
                accent,
            );
            pv.icon(Icon::Play, mid - 12.5, y, 25.0, accent);
            pv.icon(Icon::Next, mid + 15.0 + 8.0 + 6.0, y + 4.0, 16.0, accent);
            y += 25.0 + 14.0;
            let frac = pv.snaps.map_or(0.0, |sn| {
                let (e, d) = sn.media_timeline();
                if d > 0.0 {
                    (e / d) as f32
                } else {
                    0.0
                }
            });
            pv.bar(a.x, y, a.w, frac);
            y += 3.0 + 8.0;
            pv.wave(a.x, y, 60.0, 14.0, white);
        }
        "Timer" | "Focus" => {
            let digits = if content == "Focus" {
                format!("{:02}:00", s.focus_minutes)
            } else {
                "05:00".to_owned()
            };
            let h = pv.text(a.x, a.y, &digits, 32.0, false, white, None);
            let hint = format!("{}     +5 min", pv.t("Start"));
            pv.text(a.x, a.y + h + 18.0, &hint, 11.0, false, dim, None);
        }
        "Tasks" => {
            let mut y = a.y;
            y += pv.text(a.x, y, "Tasks", 16.0, s.bold_headings, white, None);
            let tasks: Vec<(bool, String)> = pv
                .snaps
                .map(|sn| {
                    sn.productivity
                        .borrow()
                        .tasks()
                        .iter()
                        .take(3)
                        .map(|t| (t.done, t.text.clone()))
                        .collect()
                })
                .unwrap_or_default();
            if tasks.is_empty() {
                let note = pv
                    .t("Your tasks appear here. Add them in Tools → Tasks.")
                    .to_owned();
                pv.text(a.x, y + 6.0, &note, 12.0, false, dim, Some(a.w));
            }
            for (done, text) in tasks {
                y += 6.0;
                let line = format!("{}  {}", if done { "✓" } else { "○" }, text);
                y += pv.text(a.x, y, &line, 12.0, false, white, Some(a.w));
            }
        }
        "Calendar" => {
            let day = Local::now().format("%a, %-d %b").to_string();
            let h = pv.text(a.x, a.y, &day, 11.0, false, dim, None);
            let next = pv
                .snaps
                .and_then(|sn| {
                    let now = chrono::Utc::now();
                    sn.calendar
                        .events()
                        .into_iter()
                        .filter(|e| e.end.with_timezone(&chrono::Utc) > now)
                        .min_by_key(|e| e.start)
                        .map(|e| e.title)
                })
                .unwrap_or_else(|| pv.t("No upcoming events").to_owned());
            pv.text(a.x, a.y + h + 12.0, &next, 15.0, false, white, Some(a.w));
        }
        "Note" => {
            let h = pv.text(a.x, a.y, pv.t("Quick note"), 11.0, false, dim, None);
            let note = pv
                .snaps
                .map(|sn| {
                    sn.productivity
                        .borrow()
                        .note()
                        .chars()
                        .take(90)
                        .collect::<String>()
                })
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| pv.t("Capture a thought…").to_owned());
            let r = Rect::new(a.x, a.y + h + 10.0, a.w, 64.0);
            pv.rr(r, 8.0, Rgba::rgb(0x24, 0x26, 0x2B));
            let st = pv.style(12.0, false, white);
            let lines = pv.fonts.wrap(&note, &st, r.w - 20.0);
            let lh = pv.fonts.line_height(&st);
            for (i, line) in lines.iter().take(3).enumerate() {
                pv.c.text(
                    pv.fonts,
                    r.x + 10.0,
                    r.y + 10.0 + i as f32 * lh,
                    line,
                    &st,
                    None,
                );
            }
        }
        "Shelf" => {
            let mut y = a.y;
            y += pv.text(
                a.x,
                y,
                pv.t("File shelf"),
                16.0,
                s.bold_headings,
                white,
                None,
            );
            y += pv.text(
                a.x,
                y + 2.0,
                pv.t("Select files, then drag them out together"),
                11.0,
                false,
                dim,
                Some(a.w),
            );
            for file in ["Design notes.pdf", "Wallpaper.png"] {
                y += 10.0;
                pv.c.stroke_circle(a.x + 9.0, y + 12.0, 8.6, pv.th.control_border, 1.2);
                pv.icon(Icon::FileText, a.x + 34.0, y, 24.0, white);
                pv.text(
                    a.x + 68.0,
                    y + 5.0,
                    file,
                    12.0,
                    false,
                    white,
                    Some(a.w - 70.0),
                );
                y += 26.0;
            }
            pv.text(
                a.x,
                y + 8.0,
                pv.t("Example files · nothing is added to your shelf"),
                10.0,
                false,
                dim,
                Some(a.w),
            );
        }
        "Tools" => {
            let mut y = a.y;
            for tool in [
                "File shelf",
                "Notes",
                "Clipboard",
                "Calendar",
                "Keep awake",
                "Clean links",
            ] {
                pv.text(a.x, y + 6.0, pv.t(tool), 12.0, false, white, Some(a.w));
                y += 30.0;
            }
        }
        "Launch" | "Navigation" => {
            let style = if content == "Launch" {
                s.launch_style.clone()
            } else {
                s.category_style.clone()
            };
            let samples: Vec<(String, Icon)> = if content == "Launch" {
                s.shortcuts
                    .iter()
                    .take(4)
                    .map(|x| (x.name.clone(), Icon::Arrow))
                    .collect()
            } else {
                vec![
                    ("Media".to_owned(), Icon::NavMedia),
                    ("Focus".to_owned(), Icon::NavFocus),
                    ("Status".to_owned(), Icon::NavStatus),
                    ("Launch".to_owned(), Icon::NavLaunch),
                    ("Tools".to_owned(), Icon::NavTools),
                ]
            };
            let accent = pv.accent();
            let (mut x, mut y) = (a.x, a.y);
            let mut row_h: f32 = 0.0;
            for (name, icon) in samples {
                let label = pv.t(&name).to_owned();
                let lw = {
                    let st = pv.style(11.0, s.bold_headings, white);
                    pv.fonts.measure_width(&label, &st).min(96.0)
                };
                let mut item_w = 0.0;
                if style != "Text" {
                    item_w += 16.0;
                }
                if style != "Icons" {
                    item_w += lw + if style == "Text" { 0.0 } else { 7.0 };
                }
                if x + item_w > a.right() && x > a.x {
                    x = a.x;
                    y += row_h + 14.0;
                    row_h = 0.0;
                }
                let mut cx = x;
                if style != "Text" {
                    pv.icon(icon, cx, y, 16.0, accent);
                    cx += 16.0;
                }
                if style != "Icons" {
                    cx += if style == "Text" { 0.0 } else { 7.0 };
                    pv.text(cx, y, &label, 11.0, s.bold_headings, white, Some(96.0));
                }
                x += item_w + 12.0;
                row_h = row_h.max(16.0);
            }
        }
        "Startup" => {
            let mut y = a.y;
            y += pv.text(
                a.x,
                y,
                pv.t("Start with Windows"),
                16.0,
                s.bold_headings,
                white,
                None,
            );
            y += 8.0;
            pv.c.stroke_circle(a.x + 9.0, y + 11.0, 8.6, pv.th.control_border, 1.2);
            if s.auto_start {
                pv.c.fill_circle(a.x + 9.0, y + 11.0, 9.0, &Fill::Solid(pv.th.checked_fill));
            }
            pv.text(
                a.x + 26.0,
                y + 4.0,
                pv.t(if s.auto_start { "Enabled" } else { "Off" }),
                13.0,
                false,
                white,
                None,
            );
            y += 30.0;
            pv.text(
                a.x,
                y,
                pv.t("Closing Settings keeps NabyScape running. Quit from the tray."),
                11.0,
                false,
                dim,
                Some(a.w),
            );
        }
        "Volume feedback" => {
            let vol = pv.snaps.map_or(0.5, |sn| sn.audio.volume);
            pv.icon(Icon::Volume, a.x, a.y, 24.0, white);
            pv.text(
                a.x,
                a.y + 30.0,
                &format!("{:.0}%", vol * 100.0),
                24.0,
                false,
                white,
                None,
            );
            pv.bar(a.x, a.y + 70.0, a.w, vol);
        }
        _ => {
            // System.
            let (cpu, used, total) = pv
                .snaps
                .map(|sn| {
                    let cpu = sn.system.cpu.unwrap_or(0.0);
                    let (used, total) = sn
                        .system
                        .latest
                        .as_ref()
                        .and_then(|m| {
                            Some((m.total? as f64 - m.available? as f64, m.total? as f64))
                        })
                        .unwrap_or((0.0, 0.0));
                    (cpu, used / 1.0e9, total / 1.0e9)
                })
                .unwrap_or((0.0, 0.0, 0.0));
            let h = pv.text(
                a.x,
                a.y,
                &format!("CPU   {cpu:.0}%"),
                14.0,
                false,
                white,
                None,
            );
            pv.bar(a.x, a.y + h + 10.0, a.w, (cpu / 100.0) as f32);
            pv.text(
                a.x,
                a.y + h + 10.0 + 3.0 + 18.0,
                &format!("Memory   {used:.1} / {total:.1} GB"),
                12.0,
                false,
                white,
                Some(a.w),
            );
        }
    }
}

/// Whether the category has a rail (re-exported for the layout).
#[allow(dead_code)]
pub(crate) fn category_has_rail(id: &str) -> bool {
    has_preview(id)
}
