//! `WeatherSky` (the card background) and `WeatherArtwork` (the sun / moon / cloud / rain
//! illustration) drawn with vector paths, ported from `Features/Weather/Weather{Visuals,Artwork}.cs`.

use crate::pages::common::Ctl;
use naby_core::color::Rgba;
use naby_core::weather as wx;
use naby_render::icons::parse_path;
use naby_render::{Fill, Rect};
use tiny_skia::{FillRule, Path, PathBuilder, Transform};

fn oval(cx: f32, cy: f32, rx: f32, ry: f32) -> Option<Path> {
    tiny_skia::Rect::from_xywh(cx - rx, cy - ry, rx * 2.0, ry * 2.0)
        .and_then(PathBuilder::from_oval)
}

fn line(a: (f32, f32), b: (f32, f32)) -> Option<Path> {
    let mut pb = PathBuilder::new();
    pb.move_to(a.0, a.1);
    pb.line_to(b.0, b.1);
    pb.finish()
}

/// Border colour of the weather card.
pub fn card_border(day: bool) -> Rgba {
    if day {
        Rgba::rgb(0x3C, 0x52, 0x68)
    } else {
        Rgba::rgb(0x35, 0x43, 0x64)
    }
}

/// The card background: a vertical gradient, a soft light, stars at night and two hills.
pub fn draw_sky(c: &mut Ctl, rect: Rect, day: bool, code: i32) {
    if c.is_dry() || rect.w < 2.0 || rect.h < 2.0 {
        return;
    }
    let wet = wx::sky_is_wet(code);
    let (top, bottom) = match (day, wet) {
        (true, true) => (Rgba::rgb(28, 43, 58), Rgba::rgb(15, 24, 35)),
        (true, false) => (Rgba::rgb(36, 66, 88), Rgba::rgb(16, 29, 43)),
        (false, _) => (Rgba::rgb(20, 29, 55), Rgba::rgb(10, 16, 33)),
    };
    let (w, h) = (rect.w, rect.h);
    c.rrect_fill(
        rect,
        16.0,
        &Fill::Linear {
            from: (rect.x, rect.y),
            to: (rect.x, rect.bottom()),
            stops: vec![(0.0, top), (1.0, bottom)],
        },
    );
    c.clip_rounded(rect, 16.0);
    let light = if day {
        Rgba::rgb(255, 209, 131).with_alpha(if wet { 25 } else { 65 })
    } else {
        Rgba::rgb(131, 157, 234).with_alpha(35)
    };
    // An elliptical glow: a unit circle with a radial gradient, scaled.
    if let Some(unit) = oval(0.0, 0.0, 1.0, 1.0) {
        let fill = Fill::Radial {
            center: (0.0, 0.0),
            radius: 1.0,
            stops: vec![(0.0, light), (1.0, light.with_alpha(0))],
        };
        let t = Transform::from_row(
            w * 0.6,
            0.0,
            0.0,
            h * 1.1,
            rect.x + w * 0.88,
            rect.y + h * 0.18,
        );
        c.fill_path_t(&unit, &fill, FillRule::Winding, t);
    }
    if !day {
        let star = Rgba::rgb(0xC9, 0xD9, 0xEF).with_alpha(0x76);
        for (fx, fy) in [
            (0.56, 0.19),
            (0.76, 0.12),
            (0.92, 0.43),
            (0.61, 0.45),
            (0.84, 0.67),
        ] {
            c.circle(rect.x + w * fx, rect.y + h * fy, 0.9 * c.s, star);
        }
    }
    let mut far = PathBuilder::new();
    far.move_to(rect.x, rect.y + h * 0.86);
    far.cubic_to(
        rect.x + w * 0.25,
        rect.y + h * 0.60,
        rect.x + w * 0.48,
        rect.y + h * 0.96,
        rect.right(),
        rect.y + h * 0.72,
    );
    far.line_to(rect.right(), rect.bottom());
    far.line_to(rect.x, rect.bottom());
    far.close();
    if let Some(p) = far.finish() {
        c.fill_path(
            &p,
            &Fill::Solid(Rgba::rgb(0x3E, 0x73, 0x97).with_alpha(0x12)),
        );
    }
    let mut near = PathBuilder::new();
    near.move_to(rect.x, rect.y + h * 0.93);
    near.cubic_to(
        rect.x + w * 0.35,
        rect.y + h * 0.75,
        rect.x + w * 0.65,
        rect.y + h * 1.04,
        rect.right(),
        rect.y + h * 0.85,
    );
    near.line_to(rect.right(), rect.bottom());
    near.line_to(rect.x, rect.bottom());
    near.close();
    if let Some(p) = near.finish() {
        c.fill_path(
            &p,
            &Fill::Solid(Rgba::rgb(0x2B, 0x4A, 0x68).with_alpha(0x14)),
        );
    }
    c.unclip();
}

/// The illustration for a WMO code in a 132x110 design canvas scaled into `rect`. `drift` is
/// the idle vertical offset in design units.
pub fn draw_artwork(c: &mut Ctl, rect: Rect, code: i32, day: bool, drift: f32) {
    if c.is_dry() || rect.w < 2.0 || rect.h < 2.0 {
        return;
    }
    let (sx, sy) = (rect.w / 132.0, rect.h / 110.0);
    let p = |x: f32, y: f32| (rect.x + x * sx, rect.y + (y + drift) * sy);
    let layers = wx::artwork(code, day);
    let sky = if day {
        Rgba::rgb(0xFF, 0xD8, 0x88)
    } else {
        Rgba::rgb(0xD3, 0xDD, 0xFB)
    };
    if day && layers.sky_body {
        let (cx, cy) = p(76.0, 42.0);
        if let Some(unit) = oval(0.0, 0.0, 1.0, 1.0) {
            let glow = Rgba::rgb(255, 190, 90).with_alpha(65);
            let fill = Fill::Radial {
                center: (0.0, 0.0),
                radius: 1.0,
                stops: vec![(0.0, glow), (1.0, glow.with_alpha(0))],
            };
            c.fill_path_t(
                &unit,
                &fill,
                FillRule::Winding,
                Transform::from_row(48.0 * sx, 0.0, 0.0, 42.0 * sy, cx, cy),
            );
        }
        let (dx, dy) = p(76.0, 39.0);
        if let Some(disc) = oval(dx, dy, 22.0 * sx, 22.0 * sy) {
            let fill = Fill::Linear {
                from: (dx, dy - 22.0 * sy),
                to: (dx, dy + 22.0 * sy),
                stops: vec![
                    (0.0, Rgba::rgb(255, 236, 171)),
                    (1.0, Rgba::rgb(237, 171, 73)),
                ],
            };
            c.fill_path(&disc, &fill);
        }
        if layers.sun_rays {
            let ray = Rgba::rgb(0xEB, 0xC5, 0x7D).with_alpha(0xAA);
            for i in 0..8 {
                let a = i as f32 * std::f32::consts::FRAC_PI_4;
                let from = p(76.0 + a.cos() * 29.0, 39.0 + a.sin() * 29.0);
                let to = p(76.0 + a.cos() * 34.0, 39.0 + a.sin() * 34.0);
                if let Some(path) = line(from, to) {
                    c.stroke_path(&path, &Fill::Solid(ray), 1.5 * sx);
                }
            }
        }
    } else if !day && layers.sky_body {
        // A crescent: the big disc minus an offset disc.
        let (ax, ay) = p(79.0, 35.0);
        let (bx, by) = p(89.0, 26.0);
        if let (Some(a), Some(b)) = (
            oval(ax, ay, 23.0 * sx, 23.0 * sy),
            oval(bx, by, 22.0 * sx, 22.0 * sy),
        ) {
            c.clip_path(Some(&a));
            let mut pb = PathBuilder::new();
            if let Some(r) =
                tiny_skia::Rect::from_xywh(ax - 24.0 * sx, ay - 24.0 * sy, 48.0 * sx, 48.0 * sy)
            {
                pb.push_rect(r);
            }
            pb.push_path(&b);
            if let Some(cut) = pb.finish() {
                c.fill_path_t(
                    &cut,
                    &Fill::Solid(sky),
                    FillRule::EvenOdd,
                    Transform::identity(),
                );
            }
            c.unclip();
        }
        for (x, y) in [(34.0, 20.0), (111.0, 58.0), (40.0, 50.0)] {
            if let Some(h) = line(p(x - 2.0, y), p(x + 2.0, y)) {
                c.stroke_path(&h, &Fill::Solid(sky), sx.max(0.5));
            }
            if let Some(v) = line(p(x, y - 2.0), p(x, y + 2.0)) {
                c.stroke_path(&v, &Fill::Solid(sky), sx.max(0.5));
            }
        }
    }
    if layers.cloud {
        let (top, bottom) = if day {
            (Rgba::rgb(238, 244, 250), Rgba::rgb(153, 178, 201))
        } else {
            (Rgba::rgb(167, 185, 214), Rgba::rgb(89, 116, 157))
        };
        let (_, y0) = p(0.0, 29.0);
        let (_, y1) = p(0.0, 82.0);
        let fill = Fill::Linear {
            from: (rect.x, y0),
            to: (rect.x, y1),
            stops: vec![(0.0, top), (1.0, bottom)],
        };
        for (cx, cy, rx, ry) in [
            (53.0, 58.0, 21.0, 19.0),
            (75.0, 53.0, 25.0, 24.0),
            (97.0, 65.0, 20.0, 17.0),
        ] {
            let (x, y) = p(cx, cy);
            if let Some(e) = oval(x, y, rx * sx, ry * sy) {
                c.fill_path(&e, &fill);
            }
        }
        let (rx, ry) = p(34.0, 57.0);
        let body = Rect::new(rx, ry, 64.0 * sx, 25.0 * sy);
        if let Some(r) = naby_render::canvas::round_rect_path(body, 10.0 * sx) {
            c.fill_path(&r, &fill);
        }
    }
    if layers.rain {
        let drop = Fill::Solid(Rgba::rgb(0x78, 0xBD, 0xF8));
        for i in 0..4 {
            let k = i as f32 * 16.0;
            if let Some(path) = line(p(45.0 + k, 88.0), p(41.0 + k, 97.0)) {
                c.stroke_path(&path, &drop, 2.0 * sx);
            }
        }
    }
    if layers.snow {
        for i in 0..4 {
            let (x, y) = p(43.0 + i as f32 * 18.0, 92.0 + (i % 2) as f32 * 5.0);
            c.circle(x, y, 1.8 * sx, Rgba::WHITE);
        }
    }
    if layers.thunder {
        if let Some(bolt) = parse_path("M74,79 L65,96 H75 L70,108 L87,89 H76 L82,79 Z") {
            let t = Transform::from_row(sx, 0.0, 0.0, sy, rect.x, rect.y + drift * sy);
            c.fill_path_t(
                &bolt,
                &Fill::Solid(Rgba::rgb(0xFF, 0xE2, 0xA2)),
                FillRule::Winding,
                t,
            );
        }
    }
    if layers.fog {
        let fog = Fill::Solid(Rgba::rgb(0xA5, 0xB5, 0xC8));
        for (a, b) in [((30.0, 90.0), (105.0, 90.0)), ((42.0, 97.0), (115.0, 97.0))] {
            if let Some(path) = line(p(a.0, a.1), p(b.0, b.1)) {
                c.stroke_path(&path, &fog, 2.0 * sx);
            }
        }
    }
}

/// The idle drift of the big artwork in design units (`DoubleAnimation(-2, 2, 4.5 s)` with
/// auto-reverse and a sine ease).
pub fn drift(now: f64) -> f32 {
    (-2.0 * (std::f64::consts::TAU * now / 9.0).cos()) as f32
}
