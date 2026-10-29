//! Static schematic examples of the controls in a Settings section (C# `SettingsScene.cs`).
//! Each is drawn at 420 x 126 layout pixels and shrunk to fit narrower hosts.

use naby_core::color::Rgba;
use naby_core::motion::Contour;
use naby_core::settings::{policy, Settings, WIDGET_NAMES};
use naby_render::{Canvas, Fill, Fonts, Rect, TextStyle, Weight};

/// Every registered example (`SettingsScene.Sections`).
pub const SECTIONS: &[&str] = &[
    "Feature usage",
    "Your setup",
    "Ambient light",
    "Notification presentation",
    "Visualizer",
    "Lyrics",
    "Move & resize",
    "Completion sound",
    "AI companion",
    "Shape",
    "Settings material",
    "Color & material",
    "Surface",
    "Controls & outlines",
    "Typography",
    "Text and navigation",
    "Motion",
    "Animation performance",
    "Dock placement",
    "Display",
    "Closed island",
    "Open panel",
    "Hover preview",
    "Hover & swipe",
    "Enabled widgets & order",
    "Individual tools",
    "Hover pages",
    "Clipboard history",
    "Focus sessions",
    "Stopwatch",
    "Automatic tucking",
    "Manual tuck shortcut",
    "Panel navigation",
    "Drag to Island",
    "Detachable previews",
    "Closed cover gestures",
    "Automatic activity",
    "Notch gestures",
    "Global hotkey",
    "Hide or show the island",
    "Closed media display",
    "Waveform source",
    "Live signal",
    "Live feedback",
    "Volume mixer",
    "Audio behavior",
    "Header wheel",
    "Clock",
    "Monitor details",
    "Launch display",
    "Shortcuts",
    "Layout profile",
    "Notifications",
    "Downloads",
    "Updates",
    "Reset preferences",
    "Behavior",
    "Local storage",
    "Language & accessibility",
    "Private diagnostics",
    "About NabyScape",
];

/// Width of the drawing in layout pixels.
pub const WIDTH: f32 = 420.0;

/// One widget label tile (`SettingsScene.WidgetTile`).
#[derive(Debug, Clone, PartialEq)]
pub struct WidgetTile {
    pub bounds: Rect,
    pub label: String,
    pub label_lines: usize,
}

/// `WidgetTiles`: five tiles per row for the enabled widgets, each as tall as its wrapped label.
pub fn widget_tiles(settings: &Settings, fonts: &Fonts) -> Vec<WidgetTile> {
    let style = TextStyle::new(10.0, Rgba::WHITE);
    let names: Vec<String> = settings
        .widgets
        .iter()
        .filter(|w| w.enabled)
        .map(|w| {
            WIDGET_NAMES
                .iter()
                .find(|(id, _)| *id == w.id)
                .map_or(w.id.clone(), |(_, n)| (*n).to_owned())
        })
        .collect();
    let lh = fonts.line_height(&style).max(12.0);
    let lines: Vec<usize> = names
        .iter()
        .map(|n| fonts.wrap(n, &style, 60.0).len().max(1))
        .collect();
    let mut result = Vec::new();
    let mut y = 15.0;
    let mut row = 0;
    while row < names.len() {
        let end = (row + 5).min(names.len());
        let tallest = lines[row..end].iter().copied().max().unwrap_or(1) as f32 * lh;
        let height = (tallest + 12.0).max(28.0);
        for (column, i) in (row..end).enumerate() {
            result.push(WidgetTile {
                bounds: Rect::new(16.0 + column as f32 * 79.0, y, 72.0, height),
                label: names[i].clone(),
                label_lines: lines[i],
            });
        }
        y += height + 7.0;
        row += 5;
    }
    result
}

/// The height of an example in layout pixels (`MeasureOverride` before shrinking).
pub fn height_of(kind: &str, settings: &Settings, fonts: &Fonts) -> f32 {
    if kind == "Enabled widgets & order" {
        let bottom = widget_tiles(settings, fonts)
            .last()
            .map_or(15.0, |t| t.bounds.bottom());
        126.0_f32.max(bottom + 44.0)
    } else {
        126.0
    }
}

struct Sc<'c> {
    c: &'c mut Canvas,
    fonts: &'c Fonts,
    s: &'c Settings,
    white: Rgba,
    muted: Rgba,
    fill: Rgba,
    line: Rgba,
    edge: Rgba,
}

fn hexc(s: &str) -> Rgba {
    Rgba::parse_hex(s).unwrap_or(Rgba::BLACK)
}

impl Sc<'_> {
    fn boxed(&mut self, x: f32, y: f32, w: f32, h: f32, brush: Option<Rgba>, r: f32) {
        let rect = Rect::new(x, y, w, h);
        self.c.fill_rrect(rect, r, brush.unwrap_or(self.fill));
        if w >= 28.0 && h >= 22.0 {
            self.c.stroke_round_rect(rect, r, self.edge, 0.75);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn text(
        &mut self,
        value: &str,
        x: f32,
        y: f32,
        size: f32,
        bold: bool,
        color: Option<Rgba>,
        max: f32,
    ) {
        let st = TextStyle {
            size,
            weight: if bold {
                Weight::Semibold
            } else {
                Weight::Regular
            },
            color: color.unwrap_or(self.white),
        };
        let max_w = max.min(405.0 - x).max(10.0);
        self.c.text(self.fonts, x, y, value, &st, Some(max_w));
    }

    fn t(&mut self, value: &str, x: f32, y: f32, size: f32, bold: bool) {
        self.text(value, x, y, size, bold, None, 400.0);
    }

    fn line(&mut self, x: f32, y: f32, xx: f32, yy: f32) {
        let mut pb = tiny_skia::PathBuilder::new();
        pb.move_to(x, y);
        pb.line_to(xx, yy);
        if let Some(path) = pb.finish() {
            self.c.stroke_path(
                &path,
                &Fill::Solid(self.line),
                1.0,
                tiny_skia::Transform::identity(),
            );
        }
    }

    fn toggle(&mut self, x: f32, y: f32, on: bool) {
        let (white, fill, muted) = (self.white, self.fill, self.muted);
        self.boxed(x, y, 30.0, 16.0, Some(if on { white } else { fill }), 8.0);
        self.c.fill_circle(
            x + if on { 22.0 } else { 8.0 },
            y + 8.0,
            5.0,
            &Fill::Solid(if on { fill } else { muted }),
        );
    }

    fn bar(&mut self, x: f32, y: f32, w: f32, value: f32) {
        let (fill, white) = (self.fill, self.white);
        self.boxed(x, y, w, 3.0, Some(fill), 1.0);
        self.boxed(
            x,
            y,
            (w * value.clamp(0.0, 1.0)).max(1.0),
            3.0,
            Some(white),
            1.0,
        );
    }

    fn keys(&mut self, key: &str, x: f32, y: f32, w: f32) {
        self.boxed(x, y, w, 30.0, None, 8.0);
        self.t(key, x + 10.0, y + 7.0, 11.0, false);
    }

    fn silhouette(&mut self, x: f32, y: f32, w: f32, h: f32, notch: bool) {
        let contour = Contour::create(f64::from(w), f64::from(h), self.s.radius, notch, 0.0);
        let background = hexc(&self.s.background);
        self.c
            .fill_contour(&contour, x, y, 1.0, &Fill::Solid(background));
        self.c
            .stroke_contour(&contour, x, y, 1.0, &Fill::Solid(self.edge), 0.75);
        if self.s.glass_style {
            let alpha = (self.s.glass_strength * 0.5) as f32;
            self.c.with_opacity(alpha, |c| {
                let fill = Fill::Linear {
                    from: (x, y),
                    to: (x + 0.65 * w, y + h),
                    stops: vec![
                        (0.0, Rgba::rgb(18, 19, 23)),
                        (0.46, Rgba::rgb(8, 9, 12)),
                        (1.0, Rgba::rgb(1, 2, 3)),
                    ],
                };
                c.fill_contour(&contour, x, y, 1.0, &fill);
            });
        }
    }

    fn island(&mut self, x: f32, y: f32, w: f32, h: f32) {
        let notch = self.s.preset == "Mac";
        self.silhouette(x, y, w, h, notch);
        self.t("Music", x + 20.0, y + 9.0, 11.0, true);
        self.t("12:45", x + w - 54.0, y + 9.0, 11.0, true);
    }

    fn wave(&mut self, x: f32, y: f32) {
        let white = self.white;
        for i in 0..25 {
            let a = (i as f32 * 0.37 + 1.0).sin();
            let h = 4.0 + 15.0 * a * a * (0.7 + 0.3 * (i as f32 * 0.13).cos());
            self.boxed(x + i as f32 * 3.6, y - h / 2.0, 1.5, h, Some(white), 1.0);
        }
    }

    fn cover(&mut self, x: f32, y: f32, size: f32) {
        self.boxed(x, y, size, size, Some(hexc("#737D8C")), 5.0);
        self.c
            .stroke_circle(x + size / 2.0, y + size / 2.0, size * 0.27, self.white, 1.0);
    }
}

/// Draws example `kind` with its top-left at (`x`, `y`) and scaled to fit `width`.
pub fn paint(
    canvas: &mut Canvas,
    fonts: &Fonts,
    kind: &str,
    settings: &Settings,
    x: f32,
    y: f32,
    width: f32,
) {
    let k = (width / WIDTH).min(1.0);
    let base = canvas.scale();
    canvas.set_scale(base * k);
    paint_unscaled(canvas, fonts, kind, settings, x / k, y / k);
    canvas.set_scale(base);
}

fn paint_unscaled(
    canvas: &mut Canvas,
    fonts: &Fonts,
    kind: &str,
    settings: &Settings,
    ox: f32,
    oy: f32,
) {
    let s = settings;
    let mut sc = Sc {
        c: canvas,
        fonts,
        s,
        white: hexc("#F2F3F5"),
        muted: hexc("#ADB3BD"),
        fill: hexc("#24272D"),
        line: hexc("#626874"),
        edge: Rgba::WHITE.with_alpha(0x16),
    };
    let tiles = if kind == "Enabled widgets & order" {
        widget_tiles(s, fonts)
    } else {
        Vec::new()
    };
    let widget_bottom = tiles.last().map_or(15.0, |t| t.bounds.bottom());
    let shift = Shift { dx: ox, dy: oy };
    sc.run(kind, &tiles, widget_bottom, shift);
}

/// A translation applied to every coordinate of an example.
#[derive(Clone, Copy)]
struct Shift {
    dx: f32,
    dy: f32,
}

impl Sc<'_> {
    fn run(&mut self, kind: &str, tiles: &[WidgetTile], widget_bottom: f32, sh: Shift) {
        // Re-route the primitive helpers through the shift by wrapping them.
        let mut w = Shifted { sc: self, sh };
        w.draw(kind, tiles, widget_bottom);
    }
}

struct Shifted<'a, 'c> {
    sc: &'a mut Sc<'c>,
    sh: Shift,
}

impl Shifted<'_, '_> {
    fn boxed(&mut self, x: f32, y: f32, w: f32, h: f32, brush: Option<Rgba>, r: f32) {
        self.sc
            .boxed(x + self.sh.dx, y + self.sh.dy, w, h, brush, r);
    }
    fn t(&mut self, v: &str, x: f32, y: f32, size: f32, bold: bool) {
        self.sc.text(
            v,
            x + self.sh.dx,
            y + self.sh.dy,
            size,
            bold,
            None,
            400.0 - (x - (x + self.sh.dx)).abs() * 0.0,
        );
    }
    #[allow(clippy::too_many_arguments)]
    fn text(
        &mut self,
        v: &str,
        x: f32,
        y: f32,
        size: f32,
        bold: bool,
        color: Option<Rgba>,
        max: f32,
    ) {
        self.sc
            .text(v, x + self.sh.dx, y + self.sh.dy, size, bold, color, max);
    }
    fn line(&mut self, x: f32, y: f32, xx: f32, yy: f32) {
        let (dx, dy) = (self.sh.dx, self.sh.dy);
        self.sc.line(x + dx, y + dy, xx + dx, yy + dy);
    }
    fn toggle(&mut self, x: f32, y: f32, on: bool) {
        self.sc.toggle(x + self.sh.dx, y + self.sh.dy, on);
    }
    fn bar(&mut self, x: f32, y: f32, w: f32, v: f32) {
        self.sc.bar(x + self.sh.dx, y + self.sh.dy, w, v);
    }
    fn keys(&mut self, key: &str, x: f32, y: f32, w: f32) {
        self.sc.keys(key, x + self.sh.dx, y + self.sh.dy, w);
    }
    fn silhouette(&mut self, x: f32, y: f32, w: f32, h: f32, notch: bool) {
        self.sc
            .silhouette(x + self.sh.dx, y + self.sh.dy, w, h, notch);
    }
    fn island(&mut self, x: f32, y: f32, w: f32, h: f32) {
        self.sc.island(x + self.sh.dx, y + self.sh.dy, w, h);
    }
    fn wave(&mut self, x: f32, y: f32) {
        self.sc.wave(x + self.sh.dx, y + self.sh.dy);
    }
    fn cover(&mut self, x: f32, y: f32, size: f32) {
        self.sc.cover(x + self.sh.dx, y + self.sh.dy, size);
    }
    fn note(&mut self, m: &str) {
        let muted = self.sc.muted;
        self.text(m, 14.0, 99.0, 11.0, false, Some(muted), 400.0);
    }

    fn draw(&mut self, kind: &str, tiles: &[WidgetTile], widget_bottom: f32) {
        let s = self.sc.s;
        let (white, muted, fill) = (self.sc.white, self.sc.muted, self.sc.fill);
        let black = Rgba::BLACK;
        let bg_height = if kind == "Enabled widgets & order" {
            124.0_f32.max(widget_bottom + 42.0)
        } else {
            124.0
        };
        self.boxed(0.0, 0.0, 420.0, bg_height, Some(hexc("#191C21")), 10.0);
        let preset_notch = s.preset == "Mac";
        match kind {
            "Feature usage" => {
                self.t("Choose what runs", 24.0, 18.0, 18.0, true);
                self.t(
                    &format!(
                        "{} enabled widgets",
                        s.widgets.iter().filter(|w| w.enabled).count()
                    ),
                    24.0,
                    51.0,
                    12.0,
                    false,
                );
                self.note("Essentials or Custom · background switches stay explicit");
            }
            "Your setup" => {
                self.t("Your next step", 24.0, 18.0, 18.0, true);
                self.t(
                    "Music · AI · Voice · Lyrics · Weather",
                    24.0,
                    51.0,
                    12.0,
                    false,
                );
                self.note("Saved settings and verified connections have distinct statuses");
            }
            "AI companion" => {
                let (dx, dy) = (self.sh.dx, self.sh.dy);
                self.sc
                    .c
                    .fill_circle(53.0 + dx, 51.0 + dy, 24.0, &Fill::Solid(hexc("#23262D")));
                self.sc
                    .c
                    .stroke_circle(53.0 + dx, 51.0 + dy, 23.5, hexc("#777D88"), 1.0);
                self.boxed(42.0, 37.0, 7.0, 16.0, Some(white), 3.5);
                self.boxed(57.0, 37.0, 7.0, 16.0, Some(white), 3.5);
                self.t("Connect your AI", 94.0, 24.0, 15.0, true);
                self.t("Provider → model → chat / voice", 94.0, 51.0, 12.0, false);
                self.keys("Local key", 298.0, 23.0, 95.0);
                self.note(
                    "Optional · keys in Windows Credential Manager · session capture controls",
                );
            }
            "Shape" => {
                self.silhouette(28.0, 22.0, 160.0, 36.0, true);
                self.silhouette(228.0, 22.0, 160.0, 36.0, false);
                self.t("Notch · attached to edge", 30.0, 69.0, 11.0, false);
                self.t("Pill · floating", 230.0, 69.0, 11.0, false);
                self.note(&format!(
                    "Current: {}",
                    if preset_notch { "Notch" } else { "Pill" }
                ));
            }
            "Settings material" => {
                self.boxed(22.0, 18.0, 168.0, 65.0, Some(hexc("#17191D")), 8.0);
                self.t("Settings window", 34.0, 31.0, 13.0, true);
                self.t(s.requested_backdrop(), 34.0, 55.0, 11.0, false);
                self.t("Island stays independent", 210.0, 32.0, 11.0, false);
                self.note("Schematic only - actual status appears below");
            }
            "Color & material" => {
                let background = hexc(&s.background);
                let ink = naby_render::Theme::contrast_text(background);
                self.boxed(22.0, 19.0, 216.0, 68.0, Some(background), 8.0);
                self.boxed(36.0, 33.0, 34.0, 34.0, Some(hexc(&s.accent)), 8.0);
                self.text("Song title", 82.0, 34.0, 14.0, true, Some(ink), 400.0);
                self.text(
                    if s.glass_style {
                        "Black glass"
                    } else {
                        "Solid surface"
                    },
                    82.0,
                    57.0,
                    11.0,
                    false,
                    Some(ink),
                    400.0,
                );
                self.t(
                    &format!("Opacity {:.0}%", s.opacity * 100.0),
                    256.0,
                    28.0,
                    12.0,
                    false,
                );
                self.t(
                    &format!("Glass {:.0}%", s.glass_strength * 100.0),
                    256.0,
                    52.0,
                    12.0,
                    false,
                );
                self.note("Your island color and material");
            }
            "Surface" => {
                self.boxed(24.0, 18.0, 176.0, 61.0, Some(black), 8.0);
                let glass = Rgba::lerp(hexc(&s.background), hexc("#3E424A"), 0.5)
                    .with_alpha((255.0 * (0.25 + s.glass_strength * 0.6)) as u8);
                self.boxed(224.0, 18.0, 176.0, 61.0, Some(glass), 8.0);
                self.t("Solid", 39.0, 38.0, 14.0, true);
                self.t("Glass", 240.0, 38.0, 14.0, true);
                self.note(&format!(
                    "Glass intensity {:.0}% · black glass {}",
                    s.glass_strength * 100.0,
                    if s.glass_style { "on" } else { "off" }
                ));
            }
            "Controls & outlines" => {
                let cf = hexc(&s.control_fill);
                let sf = hexc(&s.selection_fill);
                self.boxed(18.0, 24.0, 112.0, 34.0, Some(cf), 8.0);
                self.text(
                    "Button",
                    36.0,
                    33.0,
                    12.0,
                    false,
                    Some(naby_render::Theme::contrast_text(cf)),
                    400.0,
                );
                let (dx, dy) = (self.sh.dx, self.sh.dy);
                self.sc.c.stroke_round_rect(
                    Rect::new(146.0 + dx, 24.0 + dy, 122.0, 34.0),
                    6.0,
                    hexc(&s.control_border),
                    1.0,
                );
                self.t("Input field", 158.0, 33.0, 12.0, false);
                self.boxed(284.0, 24.0, 116.0, 34.0, Some(sf), 8.0);
                self.text(
                    "Selected tab",
                    296.0,
                    33.0,
                    12.0,
                    false,
                    Some(naby_render::Theme::contrast_text(sf)),
                    400.0,
                );
                self.note("Button fill · input outline · selected state");
            }
            "Typography" => {
                self.t("Song title", 24.0, 23.0, 21.0, s.bold_media_title);
                self.text("Artist name", 24.0, 54.0, 12.0, false, Some(muted), 400.0);
                self.t("Panel heading", 232.0, 27.0, 16.0, s.bold_headings);
                self.text(
                    "Body text stays regular",
                    232.0,
                    56.0,
                    11.0,
                    false,
                    Some(muted),
                    400.0,
                );
                self.note("Current title weights; body text stays the same");
            }
            "Text and navigation" => {
                for (name, x) in [
                    ("Media", 22.0),
                    ("Focus", 112.0),
                    ("Status", 202.0),
                    ("Tools", 292.0),
                ] {
                    self.boxed(x, 22.0, 76.0, 46.0, None, 8.0);
                    let (dx, dy) = (self.sh.dx, self.sh.dy);
                    if s.category_style == "Dots" {
                        self.sc
                            .c
                            .fill_circle(x + 38.0 + dx, 44.0 + dy, 3.0, &Fill::Solid(white));
                        continue;
                    }
                    if s.category_style != "Text" {
                        self.sc
                            .c
                            .stroke_circle(x + 38.0 + dx, 36.0 + dy, 6.0, white, 1.4);
                    }
                    if s.category_style != "Icons" {
                        self.t(
                            name,
                            x + 14.0,
                            if s.category_style == "Text" {
                                38.0
                            } else {
                                49.0
                            },
                            11.0,
                            false,
                        );
                    }
                }
                self.note(&format!(
                    "Navigation · {}",
                    s.category_style.replace("IconsLabels", "Icons + labels")
                ));
            }
            "Motion" => {
                self.boxed(23.0, 28.0, 68.0, 22.0, Some(black), 8.0);
                self.t("Music", 34.0, 33.0, 9.0, false);
                self.line(108.0, 40.0, 176.0, 40.0);
                self.boxed(193.0, 16.0, 135.0, 63.0, Some(black), 8.0);
                self.cover(203.0, 25.0, 22.0);
                self.t("Song title", 234.0, 28.0, 10.0, true);
                self.bar(204.0, 65.0, 112.0, 0.4);
                self.t(
                    &if s.reduce_motion {
                        "Instant state change".to_owned()
                    } else {
                        format!("{:.0} ms transition", s.animation_ms)
                    },
                    23.0,
                    78.0,
                    11.0,
                    false,
                );
                self.text(
                    "Closed → open",
                    193.0,
                    87.0,
                    11.0,
                    false,
                    Some(muted),
                    400.0,
                );
            }
            "Animation performance" => {
                self.t("Eco", 24.0, 16.0, 12.0, true);
                self.t("Auto", 24.0, 42.0, 12.0, true);
                self.t("Maximum", 24.0, 68.0, 12.0, true);
                for row in 0..3 {
                    for i in 0..(row + 1) * 8 {
                        self.boxed(
                            120.0 + i as f32 * 10.0,
                            20.0 + row as f32 * 26.0,
                            3.0,
                            12.0,
                            Some(white),
                            1.0,
                        );
                    }
                }
                self.note("Illustrative update cadence · your display sets the visible limit");
            }
            "Dock placement" => {
                self.boxed(110.0, 12.0, 205.0, 76.0, Some(hexc("#14171D")), 7.0);
                let (dx, dy) = (self.sh.dx, self.sh.dy);
                self.sc.c.stroke_round_rect(
                    Rect::new(110.0 + dx, 12.0 + dy, 205.0, 76.0),
                    7.0,
                    self.sc.line,
                    1.0,
                );
                let marker = match s.dock_edge.as_str() {
                    "Left" => Rect::new(111.0, 36.0, 10.0, 30.0),
                    "Right" => Rect::new(304.0, 36.0, 10.0, 30.0),
                    "Bottom" => Rect::new(182.0, 77.0, 62.0, 9.0),
                    "Floating" => Rect::new(182.0, 45.0, 62.0, 12.0),
                    _ => Rect::new(182.0, 14.0, 62.0, 9.0),
                };
                self.sc.c.fill_rrect(marker.translate(dx, dy), 4.0, white);
                self.note(&format!(
                    "{} · {:.0} px inset · panels remain upright",
                    s.dock_edge, s.dock_inset
                ));
            }
            "Display" => {
                let (dx, dy) = (self.sh.dx, self.sh.dy);
                let line = self.sc.line;
                self.sc.c.stroke_round_rect(
                    Rect::new(24.0 + dx, 17.0 + dy, 166.0, 67.0),
                    5.0,
                    line,
                    1.0,
                );
                self.sc.c.stroke_round_rect(
                    Rect::new(220.0 + dx, 17.0 + dy, 166.0, 67.0),
                    5.0,
                    line,
                    1.0,
                );
                let marker_x = if matches!(s.display_mode.as_str(), "Follow" | "Selected") {
                    266.0
                } else {
                    70.0
                };
                self.boxed(marker_x, 18.0, 76.0, 12.0, Some(white), 3.0);
                if s.display_mode == "Follow" {
                    self.line(318.0, 39.0, 332.0, 48.0);
                    self.line(318.0, 39.0, 321.0, 54.0);
                    self.t("250 ms", 242.0, 39.0, 10.0, false);
                }
                self.t("Primary display", 50.0, 66.0, 11.0, false);
                self.t("Other display", 250.0, 66.0, 11.0, false);
                self.note(&match s.display_mode.as_str() {
                    "Follow" => {
                        "Follow cursor · move after 250 ms · waits during interaction".to_owned()
                    }
                    "Selected" => format!("Selected display: {}", s.monitor),
                    _ => "Primary display".to_owned(),
                });
            }
            "Closed island" => {
                self.island(90.0, 18.0, 240.0, 34.0);
                self.line(90.0, 69.0, 330.0, 69.0);
                self.line(90.0, 63.0, 90.0, 75.0);
                self.line(330.0, 63.0, 330.0, 75.0);
                self.t(
                    &format!(
                        "{:.0} × {:.0} px · radius {:.0} px",
                        s.width, s.height, s.radius
                    ),
                    115.0,
                    77.0,
                    11.0,
                    false,
                );
                self.note(&format!(
                    "Horizontal position {:.0}% · top offset {:.0} px",
                    s.horizontal, s.top
                ));
            }
            "Open panel" => {
                self.boxed(24.0, 12.0, 210.0, 76.0, Some(black), 8.0);
                self.boxed(37.0, 23.0, 35.0, 14.0, None, 8.0);
                self.t("Media", 86.0, 25.0, 10.0, false);
                self.t("Song title", 42.0, 48.0, 13.0, true);
                self.bar(42.0, 73.0, 172.0, 0.4);
                self.t(
                    &format!("{:.0} px wide", s.panel_width),
                    253.0,
                    24.0,
                    12.0,
                    false,
                );
                self.t(
                    &format!("Inset {:.0} px", s.panel_padding),
                    253.0,
                    47.0,
                    11.0,
                    false,
                );
                self.t(
                    &format!("Text {:.0}%", s.panel_scale * 100.0),
                    253.0,
                    67.0,
                    11.0,
                    false,
                );
                self.note(&format!(
                    "Maximum height {:.0} px · spacing {:.0} px",
                    s.panel_max_height, s.section_gap
                ));
            }
            "Hover preview" | "Hover & swipe" => {
                self.island(20.0, 18.0, 126.0, 27.0);
                self.line(159.0, 37.0, 199.0, 37.0);
                self.boxed(212.0, 12.0, 186.0, 73.0, Some(black), 8.0);
                self.t("Song title", 226.0, 24.0, 13.0, true);
                self.bar(226.0, 61.0, 156.0, 0.35);
                self.t("Hover", 45.0, 63.0, 11.0, false);
                self.note(&if kind == "Hover & swipe" {
                    format!(
                        "Preview after {} ms · detail after {} ms",
                        s.hover_delay_ms, s.hover_detail_delay_ms
                    )
                } else {
                    format!("Preview {:.0} × {:.0} px", s.hover_width, s.hover_height)
                });
            }
            "Enabled widgets & order" => {
                for tile in tiles {
                    let b = tile.bounds;
                    self.boxed(b.x, b.y, b.w, b.h, None, 8.0);
                    self.text(&tile.label, b.x + 6.0, b.y + 6.0, 10.0, false, None, 60.0);
                }
                self.text(
                    "Visible panels, in your chosen order",
                    14.0,
                    99.0_f32.max(widget_bottom + 12.0),
                    11.0,
                    false,
                    Some(muted),
                    400.0,
                );
            }
            "Individual tools" => {
                for (ti, item) in s.tools.iter().take(7).enumerate() {
                    let x = 16.0 + (ti % 4) as f32 * 100.0;
                    let y = 10.0 + (ti / 4) as f32 * 40.0;
                    let name = naby_core::settings::TOOL_NAMES
                        .iter()
                        .find(|(id, _)| *id == item.id)
                        .map_or(item.id.as_str(), |(_, n)| *n);
                    self.text(
                        name,
                        x,
                        y,
                        10.0,
                        false,
                        Some(if item.enabled { white } else { muted }),
                        92.0,
                    );
                    self.toggle(x, y + 18.0, item.enabled);
                }
                self.note("Disabled tools disappear; their saved data stays");
            }
            "Hover pages" => {
                let available = policy::hover_pages(s);
                for (i, id) in available.iter().take(4).enumerate() {
                    let x = 18.0 + i as f32 * 99.0;
                    self.boxed(x, 12.0, 88.0, 72.0, Some(black), 8.0);
                    let title = match id.as_str() {
                        "media" => "Music".to_owned(),
                        "files" => "Shelf".to_owned(),
                        "notes" => "Notes".to_owned(),
                        "clips" => "Clipboard".to_owned(),
                        other => title_case(other),
                    };
                    self.text(&title, x + 8.0, 20.0, 10.0, true, None, 72.0);
                    match id.as_str() {
                        "media" => {
                            self.cover(x + 8.0, 41.0, 22.0);
                            self.bar(x + 36.0, 44.0, 42.0, 0.7);
                            self.bar(x + 36.0, 53.0, 32.0, 0.5);
                            self.bar(x + 8.0, 72.0, 71.0, 0.35);
                        }
                        "timer" => {
                            self.t("05:00", x + 10.0, 40.0, 18.0, true);
                            self.bar(x + 10.0, 69.0, 67.0, 0.45);
                        }
                        "focus" => {
                            let (dx, dy) = (self.sh.dx, self.sh.dy);
                            self.sc
                                .c
                                .stroke_circle(x + 21.0 + dx, 53.0 + dy, 10.0, muted, 2.0);
                            self.text("25:00", x + 37.0, 47.0, 11.0, true, None, 45.0);
                            self.t("Work session", x + 9.0, 69.0, 9.0, false);
                        }
                        "calendar" => {
                            self.t("16", x + 11.0, 39.0, 21.0, true);
                            self.t("OCT", x + 43.0, 48.0, 10.0, false);
                            self.bar(x + 10.0, 73.0, 66.0, 0.6);
                        }
                        "tasks" => {
                            for j in 0..3 {
                                self.boxed(x + 10.0, 42.0 + j as f32 * 12.0, 6.0, 6.0, None, 1.0);
                                self.bar(x + 23.0, 45.0 + j as f32 * 12.0, 50.0, 0.7);
                            }
                        }
                        "files" => {
                            for j in 0..3 {
                                self.boxed(
                                    x + 11.0 + j as f32 * 11.0,
                                    49.0 - j as f32 * 4.0,
                                    28.0,
                                    25.0,
                                    Some(hexc(if j == 2 { "#737D8C" } else { "#353B45" })),
                                    3.0,
                                );
                            }
                        }
                        "system" => {
                            self.t("CPU   RAM", x + 9.0, 42.0, 9.0, false);
                            self.bar(x + 9.0, 59.0, 69.0, 0.3);
                            self.bar(x + 9.0, 70.0, 69.0, 0.6);
                        }
                        _ => {
                            self.bar(x + 10.0, 45.0, 65.0, 0.9);
                            self.bar(x + 10.0, 56.0, 65.0, 0.6);
                            self.bar(x + 10.0, 67.0, 65.0, 0.75);
                        }
                    }
                }
                self.note(&format!(
                    "{} available pages · first {} shown in this example",
                    available.len(),
                    available.len().min(4)
                ));
            }
            "Clipboard history" => {
                self.t("Text", 22.0, 20.0, 12.0, true);
                self.text(
                    "Copied thought",
                    22.0,
                    42.0,
                    11.0,
                    false,
                    Some(muted),
                    400.0,
                );
                self.t("Link", 158.0, 20.0, 12.0, true);
                self.text("example.com", 158.0, 42.0, 11.0, false, Some(muted), 400.0);
                self.boxed(296.0, 18.0, 40.0, 40.0, None, 8.0);
                self.t("Image", 346.0, 30.0, 11.0, false);
                self.toggle(22.0, 74.0, s.clipboard_history);
                self.t(
                    if s.clipboard_history {
                        "History enabled"
                    } else {
                        "History off"
                    },
                    64.0,
                    74.0,
                    11.0,
                    false,
                );
                self.note("Example items · turning history off deletes saved items");
            }
            "Stopwatch" => {
                self.t("Stopwatch", 28.0, 13.0, 12.0, true);
                self.t("00:00", 28.0, 33.0, 28.0, true);
                self.keys("Start", 205.0, 29.0, 78.0);
                self.keys("Reset", 292.0, 29.0, 78.0);
                self.note("Start → Pause → Resume · Reset clears elapsed time");
            }
            "Focus sessions" => {
                self.t(&format!("{}:00", s.focus_minutes), 28.0, 20.0, 28.0, true);
                self.text("Focus", 30.0, 59.0, 12.0, false, Some(muted), 400.0);
                self.line(170.0, 25.0, 170.0, 77.0);
                self.t(&format!("{}:00", s.break_minutes), 224.0, 20.0, 28.0, true);
                self.text("Break", 226.0, 59.0, 12.0, false, Some(muted), 400.0);
                self.note("Used when the next session starts");
            }
            "Automatic tucking" => {
                self.boxed(20.0, 18.0, 112.0, 35.0, Some(black), 8.0);
                self.t("Visible", 45.0, 66.0, 11.0, false);
                self.boxed(154.0, 18.0, 112.0, s.tuck_edge as f32, Some(black), 2.0);
                self.t(
                    &format!("Tucked · {:.0} DIP", s.tuck_edge),
                    164.0,
                    66.0,
                    11.0,
                    false,
                );
                let (dx, dy) = (self.sh.dx, self.sh.dy);
                self.sc.c.stroke_round_rect(
                    Rect::new(288.0 + dx, 18.0 + dy, 112.0, 35.0),
                    8.0,
                    self.sc.line,
                    1.0,
                );
                self.t("Hidden", 320.0, 66.0, 11.0, false);
                self.note(if s.always_tucked {
                    "Always tucked · hover the edge for a preview"
                } else {
                    "Automatic rules · manual hide removes the whole island"
                });
            }
            "Manual tuck shortcut" => {
                self.keys(&s.tuck_hotkey_modifiers, 24.0, 24.0, 130.0);
                self.keys(&s.tuck_hotkey_key, 166.0, 24.0, 40.0);
                self.t("Tuck / automatic", 232.0, 30.0, 13.0, true);
                self.note("First press tucks · second returns to automatic rules");
            }
            "Panel navigation" => {
                self.boxed(20.0, 15.0, 146.0, 68.0, Some(black), 8.0);
                self.t("Media", 35.0, 28.0, 14.0, true);
                self.line(182.0, 48.0, 234.0, 48.0);
                self.boxed(252.0, 15.0, 146.0, 68.0, Some(black), 8.0);
                self.t("Status", 268.0, 28.0, 14.0, true);
                self.note("Swipe empty space · Shift + wheel · Ctrl + Tab");
            }
            "Drag to Island" => {
                for i in 0..3 {
                    self.boxed(
                        28.0 + i as f32 * 12.0,
                        34.0 - i as f32 * 8.0,
                        42.0,
                        44.0,
                        Some(hexc(if i == 2 { "#737D8C" } else { "#3C424C" })),
                        4.0,
                    );
                }
                self.line(110.0, 48.0, 198.0, 48.0);
                self.boxed(218.0, 19.0, 180.0, 64.0, Some(black), 8.0);
                self.t("Drop files here", 239.0, 32.0, 14.0, true);
                self.t("References · originals stay", 230.0, 59.0, 11.0, false);
                self.note(if s.drop_reveal {
                    "Dragging reveals the drop area"
                } else {
                    "Drop reveal disabled"
                });
            }
            "Detachable previews" => {
                self.boxed(18.0, 15.0, 151.0, 68.0, Some(black), 8.0);
                self.bar(79.0, 24.0, 30.0, 1.0);
                self.t("Island preview", 33.0, 49.0, 12.0, false);
                self.line(185.0, 48.0, 224.0, 61.0);
                self.boxed(244.0, 31.0, 151.0, 57.0, Some(black), 8.0);
                self.t("Mini window", 260.0, 49.0, 12.0, true);
                self.note("Drag the grip to position · or use Detach");
            }
            "Closed cover gestures" => {
                self.cover(22.0, 22.0, 38.0);
                self.t("Click", 80.0, 21.0, 13.0, true);
                self.t("Play / pause", 80.0, 43.0, 11.0, false);
                self.t("Hold 500 ms", 240.0, 21.0, 13.0, true);
                self.t("Choose media session", 240.0, 43.0, 11.0, false);
                self.note(if s.cover_gestures {
                    "Enabled when the closed icon uses album artwork"
                } else {
                    "Cover gestures disabled"
                });
            }
            "Automatic activity" => {
                self.t("New song / session ending", 24.0, 20.0, 14.0, true);
                self.island(26.0, 51.0, 225.0, 30.0);
                self.t("Brief preview", 274.0, 57.0, 11.0, false);
                self.note(if s.smart_activities {
                    "Appears without taking keyboard focus"
                } else {
                    "Automatic previews disabled"
                });
            }
            "Notch gestures" => {
                self.t("Mouse wheel", 24.0, 22.0, 13.0, true);
                self.t(
                    match s.wheel_action.as_str() {
                        "volume" => "Volume",
                        "pages" => "Switch tabs",
                        _ => "Off",
                    },
                    24.0,
                    49.0,
                    14.0,
                    false,
                );
                self.t("Middle click", 232.0, 22.0, 13.0, true);
                self.t(
                    match s.middle_action.as_str() {
                        "toggle" => "Play / pause",
                        "microphone" => "Mute microphone",
                        _ => "Off",
                    },
                    232.0,
                    49.0,
                    14.0,
                    false,
                );
                self.note("Hover previews always use the wheel to browse");
            }
            "Global hotkey" | "Hide or show the island" => {
                let hide = kind == "Hide or show the island";
                let (m, k) = if hide {
                    (&s.visibility_hotkey_modifiers, &s.visibility_hotkey_key)
                } else {
                    (&s.hotkey_modifiers, &s.hotkey_key)
                };
                self.keys(m, 24.0, 24.0, 130.0);
                self.keys(k, 166.0, 24.0, 40.0);
                self.t(
                    if hide {
                        "Hide / show"
                    } else {
                        "Open / close panel"
                    },
                    232.0,
                    30.0,
                    13.0,
                    true,
                );
                self.note("Works while another app is focused");
            }
            "Closed media display" => {
                self.silhouette(32.0, 22.0, 352.0, 40.0, preset_notch);
                if s.closed_media_icon == "Artwork" {
                    self.cover(46.0, 31.0, 22.0);
                } else {
                    self.wave(46.0, 42.0);
                }
                if s.closed_media_content == "Waveform" {
                    self.wave(158.0, 42.0);
                } else {
                    self.t("Song title", 160.0, 34.0, 13.0, true);
                }
                self.t("12:45", 334.0, 36.0, 11.0, true);
                self.note("Current closed media layout · example title and artwork");
            }
            "Visualizer" | "Waveform source" | "Live signal" => {
                let spectrum = s.waveform_source == "Spectrum";
                self.t(
                    if spectrum {
                        "Bass → mids → treble"
                    } else {
                        "Amplitude history"
                    },
                    22.0,
                    18.0,
                    13.0,
                    true,
                );
                self.wave(125.0, 62.0);
                self.t(
                    &format!("{}× sensitivity", super::kit::num(s.waveform_sensitivity)),
                    270.0,
                    38.0,
                    12.0,
                    false,
                );
                self.note(if spectrum {
                    "Real frequency bands · selected media output mix"
                } else {
                    "Measured selected-app amplitude · fast attack, smooth release"
                });
            }
            "Lyrics" => {
                self.text(
                    "A little closer to the music",
                    24.0,
                    17.0,
                    11.0,
                    false,
                    Some(muted),
                    400.0,
                );
                self.t("One line, right on time", 24.0, 42.0, 22.0, true);
                self.text(
                    "Slide     Focus     Karaoke",
                    24.0,
                    77.0,
                    11.0,
                    false,
                    Some(muted),
                    400.0,
                );
                self.toggle(355.0, 20.0, s.lyrics_enabled);
                self.note(&format!(
                    "{} · Synced lyrics via LRCLIB · no key required",
                    s.lyrics_animation
                ));
            }
            "Move & resize" => {
                self.island(95.0, 22.0, 220.0, 36.0);
                self.line(88.0, 18.0, 66.0, 18.0);
                self.line(66.0, 18.0, 66.0, 83.0);
                self.line(66.0, 83.0, 88.0, 83.0);
                self.t("↔", 180.0, 70.0, 20.0, false);
                self.toggle(352.0, 28.0, s.dock_unlocked);
                self.note("Drag the top grip to move · lower corners resize each state");
            }
            "Completion sound" => {
                self.t(
                    if s.countdown_sound == "Off" {
                        "Silent"
                    } else {
                        "♪"
                    },
                    25.0,
                    18.0,
                    28.0,
                    true,
                );
                self.wave(119.0, 50.0);
                self.t(&s.countdown_sound, 264.0, 25.0, 16.0, true);
                self.text(
                    &format!("{:.0}%", s.countdown_sound_volume * 100.0),
                    264.0,
                    53.0,
                    12.0,
                    false,
                    Some(muted),
                    400.0,
                );
                self.note("Timer & Focus · audible without Windows notifications");
            }
            "Live feedback" => {
                self.island(24.0, 23.0, 208.0, 34.0);
                self.bar(54.0, 47.0, 146.0, 0.66);
                self.t("Volume / brightness", 252.0, 27.0, 12.0, false);
                self.text(
                    "Temporary feedback",
                    252.0,
                    50.0,
                    11.0,
                    false,
                    Some(muted),
                    400.0,
                );
                self.note(&format!(
                    "Volume {} · brightness {} · quiet motion {}",
                    on_off(s.volume_feedback),
                    on_off(s.brightness_feedback),
                    on_off(s.waveform_idle_motion)
                ));
            }
            "Volume mixer" | "Audio behavior" => {
                self.t("Apps", 24.0, 18.0, 13.0, true);
                self.t("Output", 246.0, 18.0, 13.0, true);
                self.t("Music app", 24.0, 46.0, 11.0, false);
                self.bar(96.0, 55.0, 114.0, 0.7);
                self.t("Windows default", 246.0, 46.0, 11.0, false);
                self.text(
                    "Mute or set volume",
                    24.0,
                    75.0,
                    11.0,
                    false,
                    Some(muted),
                    400.0,
                );
                self.note("Illustrative controls · open the mixer to adjust real audio");
            }
            "Header wheel" => {
                self.t("One wheel step", 24.0, 22.0, 15.0, true);
                self.t(&format!("+{}%", s.volume_step), 264.0, 20.0, 26.0, true);
                self.bar(24.0, 73.0, 365.0, 0.6);
                self.note("Volume change per notch when wheel action is Volume");
            }
            "Clock" => {
                let time = if s.clock24_hour {
                    format!("14:35{}", if s.clock_seconds { ":08" } else { "" })
                } else {
                    format!("2:35{} PM", if s.clock_seconds { ":08" } else { "" })
                };
                self.t(&time, 26.0, 18.0, 30.0, true);
                self.text(
                    if s.show_date {
                        "Friday, 16 October"
                    } else {
                        "Date hidden"
                    },
                    27.0,
                    60.0,
                    12.0,
                    false,
                    Some(muted),
                    400.0,
                );
                self.note("Format example · seconds appear in Status");
            }
            "Monitor details" => {
                self.t("CPU · Memory", 24.0, 20.0, 14.0, true);
                self.text(
                    "Always in Overview",
                    24.0,
                    47.0,
                    11.0,
                    false,
                    Some(muted),
                    400.0,
                );
                self.t(
                    &format!("Network   {}", if s.monitor_network { "On" } else { "Off" }),
                    244.0,
                    17.0,
                    11.0,
                    false,
                );
                self.t(
                    &format!(
                        "Storage     {}",
                        if s.monitor_storage { "On" } else { "Off" }
                    ),
                    244.0,
                    39.0,
                    11.0,
                    false,
                );
                self.t(
                    &format!(
                        "Processes  {}",
                        if s.monitor_processes { "On" } else { "Off" }
                    ),
                    244.0,
                    61.0,
                    11.0,
                    false,
                );
                self.note("Additional details are available in their Status tabs");
            }
            "Launch display" => {
                self.t("Launch", 24.0, 18.0, 16.0, true);
                self.t(
                    &format!(
                        "Current style: {}",
                        s.launch_style.replace("IconsLabels", "Icons + labels")
                    ),
                    24.0,
                    50.0,
                    13.0,
                    false,
                );
                self.note("Saved apps and shortcuts use this display style");
            }
            "Shortcuts" => {
                self.t("Launch", 24.0, 18.0, 15.0, true);
                self.text(
                    &format!("{} saved shortcuts", s.shortcuts.len()),
                    250.0,
                    22.0,
                    11.0,
                    false,
                    Some(muted),
                    400.0,
                );
                for (si, item) in s.shortcuts.iter().take(3).enumerate() {
                    self.boxed(24.0 + si as f32 * 128.0, 49.0, 116.0, 29.0, None, 8.0);
                    self.text(
                        &item.name,
                        32.0 + si as f32 * 128.0,
                        57.0,
                        11.0,
                        false,
                        None,
                        100.0,
                    );
                }
                if s.shortcuts.is_empty() {
                    self.t(
                        "Add an app, folder or website below",
                        24.0,
                        56.0,
                        12.0,
                        false,
                    );
                }
                self.note("Only your saved shortcuts appear in Launch");
            }
            "Layout profile" => {
                self.t("Appearance + layout", 24.0, 22.0, 16.0, true);
                self.t("Content order + gestures", 24.0, 50.0, 12.0, false);
                self.keys("Export / Import", 250.0, 27.0, 143.0);
                self.note("Excludes notes, clipboard items, app targets and startup");
            }
            "Ambient light" => {
                self.boxed(36.0, 12.0, 348.0, 82.0, Some(black), 16.0);
                if s.cover_ambient {
                    let accent = match s.ambient_color_source.as_str() {
                        "Custom color" | "Custom blend" => hexc(&s.ambient_custom_color),
                        _ => hexc(&s.accent),
                    };
                    let (dx, dy) = (self.sh.dx, self.sh.dy);
                    self.sc
                        .c
                        .push_clip_round_rect(Rect::new(36.0 + dx, 12.0 + dy, 348.0, 82.0), 16.0);
                    let strong = accent.with_alpha((255.0 * s.ambient_strength * 0.8) as u8);
                    let fill = Fill::Linear {
                        from: (0.0, 94.0 + dy),
                        to: (0.0, 12.0 + dy),
                        stops: vec![
                            (0.0, strong),
                            (0.5, strong.with_alpha(strong.a / 3)),
                            (1.0, accent.with_alpha(0)),
                        ],
                    };
                    self.sc.c.fill_round_rect(
                        Rect::new(36.0 + dx, 12.0 + dy, 348.0, 82.0),
                        0.0,
                        &fill,
                    );
                    self.sc.c.pop_clip();
                }
                self.cover(51.0, 28.0, 36.0);
                self.t(&s.ambient_scope, 102.0, 30.0, 15.0, true);
                self.t(
                    &format!(
                        "{} · {}%",
                        s.ambient_audio_response,
                        (s.ambient_strength * 100.0) as i32
                    ),
                    102.0,
                    56.0,
                    11.0,
                    false,
                );
                self.note(&format!(
                    "{} · {} · audio-reactive",
                    s.ambient_color_source, s.ambient_motion
                ));
            }
            "Notification presentation" => {
                self.boxed(46.0, 10.0, 328.0, 77.0, Some(black), 16.0);
                self.t("Focus complete", 68.0, 25.0, 16.0, true);
                self.text(
                    "Time for a short break",
                    68.0,
                    52.0,
                    11.0,
                    false,
                    Some(muted),
                    400.0,
                );
                self.text("2 queued", 286.0, 28.0, 10.0, false, Some(muted), 400.0);
                self.bar(68.0, 73.0, 280.0, 0.65);
                self.note(&format!(
                    "{} · {:.0} sec · {}",
                    s.notification_animation,
                    s.notification_seconds,
                    if s.quiet_hours {
                        "quiet hours on"
                    } else {
                        "quiet hours off"
                    }
                ));
            }
            "Notifications" => {
                self.boxed(36.0, 18.0, 215.0, 34.0, Some(black), 17.0);
                self.t("Timer finished", 53.0, 29.0, 11.0, true);
                self.keys(&s.notification_delivery, 274.0, 20.0, 110.0);
                self.note("Notch / Windows / Both / Off · hidden island uses Windows");
            }
            "Downloads" => {
                self.boxed(24.0, 18.0, 42.0, 47.0, None, 8.0);
                self.t("↓", 36.0, 25.0, 21.0, false);
                self.t("Report.pdf", 83.0, 22.0, 14.0, true);
                self.text(
                    "2 arrivals · drag originals",
                    83.0,
                    49.0,
                    11.0,
                    false,
                    Some(muted),
                    400.0,
                );
                self.toggle(350.0, 25.0, s.downloads_enabled);
                self.note("Example only · folder arrivals inferred after stable readable size");
            }
            "Updates" => {
                self.t(
                    &format!("Installed {}", env!("CARGO_PKG_VERSION")),
                    22.0,
                    15.0,
                    13.0,
                    true,
                );
                self.t("Example: newer stable", 225.0, 15.0, 12.0, true);
                self.bar(24.0, 55.0, 365.0, 0.6);
                self.t(
                    "Check > download 60% > verify > install",
                    24.0,
                    70.0,
                    12.0,
                    false,
                );
                self.note(&format!(
                    "Automatic checks {} - no automatic installation",
                    if s.auto_check_updates { "on" } else { "off" }
                ));
            }
            "Reset preferences" => {
                self.keys("Your preferences", 20.0, 24.0, 128.0);
                self.line(158.0, 39.0, 196.0, 39.0);
                self.keys("Backup saved", 208.0, 24.0, 120.0);
                self.t("Reset selected category", 22.0, 71.0, 12.0, true);
                self.note("Notes, Tasks, startup and clipboard capture stay intact");
            }
            "Behavior" => {
                self.t("Windows sign-in", 24.0, 23.0, 15.0, true);
                self.line(163.0, 41.0, 217.0, 41.0);
                self.t("NabyScape", 240.0, 23.0, 15.0, true);
                self.toggle(244.0, 60.0, s.auto_start);
                self.text(
                    if s.auto_start {
                        "Starts automatically"
                    } else {
                        "Start manually"
                    },
                    24.0,
                    68.0,
                    12.0,
                    false,
                    Some(muted),
                    400.0,
                );
            }
            "Local storage" => {
                self.t("Saved on this PC", 24.0, 21.0, 17.0, true);
                self.t("Preferences · Notes · Tasks", 24.0, 51.0, 12.0, false);
                self.text(
                    "No account required",
                    24.0,
                    77.0,
                    11.0,
                    false,
                    Some(muted),
                    400.0,
                );
            }
            "About NabyScape" => {
                self.t("NabyScape", 24.0, 20.0, 23.0, true);
                self.text(
                    "Windows 11 · x64",
                    24.0,
                    60.0,
                    12.0,
                    false,
                    Some(muted),
                    400.0,
                );
                self.t(
                    &format!("Version {}", env!("CARGO_PKG_VERSION")),
                    250.0,
                    31.0,
                    12.0,
                    false,
                );
            }
            "Language & accessibility" => {
                self.t("Language", 24.0, 18.0, 12.0, true);
                let name = naby_core::i18n::languages()
                    .iter()
                    .find(|l| l.code == s.ui_language)
                    .map_or("English", |l| l.name);
                self.keys(name, 210.0, 12.0, 176.0);
                self.t("Text size", 24.0, 55.0, 12.0, true);
                self.t(
                    &format!("{:.0}%", s.accessibility_text_scale * 100.0),
                    218.0,
                    55.0,
                    12.0,
                    false,
                );
                self.toggle(350.0, 55.0, s.accessibility_high_contrast);
                self.note("Language · text size · contrast · Windows motion preferences");
            }
            "Private diagnostics" => {
                self.t("Review before sharing", 24.0, 18.0, 16.0, true);
                self.text(
                    "App version · service status · controlled events",
                    24.0,
                    48.0,
                    11.0,
                    false,
                    Some(muted),
                    400.0,
                );
                self.keys("Copy", 24.0, 73.0, 95.0);
                self.keys("Export", 133.0, 73.0, 95.0);
                self.note("No automatic upload · conversations and credentials excluded");
            }
            other => {
                self.t(other, 24.0, 40.0, 14.0, true);
            }
        }
        let _ = fill;
    }
}

fn on_off(v: bool) -> &'static str {
    if v {
        "on"
    } else {
        "off"
    }
}

fn title_case(id: &str) -> String {
    let mut chars = id.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().collect::<String>() + chars.as_str())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use naby_render::snapshot::test_fonts;

    #[test]
    fn registered_examples_are_unique_and_include_the_late_additions() {
        let mut seen = std::collections::HashSet::new();
        assert!(SECTIONS.iter().all(|s| seen.insert(*s)));
        assert!(SECTIONS.contains(&"Updates") && SECTIONS.contains(&"Reset preferences"));
    }

    #[test]
    fn every_example_renders_without_a_running_app() {
        let Some(fonts) = test_fonts() else { return };
        for kind in SECTIONS {
            let mut canvas = Canvas::new(420, 200);
            paint(
                &mut canvas,
                &fonts,
                kind,
                &Settings::default(),
                0.0,
                0.0,
                420.0,
            );
            let ink =
                (0..canvas.height()).any(|y| (0..canvas.width()).any(|x| canvas.pixel(x, y).a > 0));
            assert!(ink, "{kind} drew nothing");
        }
    }

    #[test]
    fn widget_labels_and_caption_have_separate_measured_tile_space() {
        let Some(fonts) = test_fonts() else { return };
        let mut settings = Settings::default();
        for w in &mut settings.widgets {
            w.enabled = true;
        }
        let tiles = widget_tiles(&settings, &fonts);
        assert_eq!(tiles.len(), settings.widgets.len());
        let style = TextStyle::new(10.0, Rgba::WHITE);
        let lh = fonts.line_height(&style);
        for tile in &tiles {
            assert!(tile.bounds.h >= tile.label_lines as f32 * lh + 12.0 - 0.01);
            assert!(fonts.measure_width(&tile.label, &style) <= tile.bounds.w * 4.0);
        }
        assert!(
            tiles.iter().any(|t| t.bounds.h > 28.0),
            "a two-line label makes a taller tile"
        );
        let bottom = tiles.last().unwrap().bounds.bottom();
        assert!(height_of("Enabled widgets & order", &settings, &fonts) >= bottom + 44.0);
    }
}
