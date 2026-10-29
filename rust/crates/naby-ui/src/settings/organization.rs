//! Section organization and the visibility sample (C# `SettingsOrganization.cs`).
//!
//! In the WPF window this file wired previews to controls and moved section headings into a
//! header row. Here the wiring is `SettingsView::wire` / `configure_preview_for_*`
//! (`preview_motion.rs`) and headings are drawn by `SettingsView::section`; what remains is the
//! "tucked island under a maximized app" sample.

use super::preview::Pv;
use naby_core::color::Rgba;
use naby_render::{Fill, Rect};

/// `VisibilityExample`: a miniature screen with the island slid up by the tuck setting.
pub(super) fn visibility_example(pv: &mut Pv, a: Rect) {
    let s = pv.s;
    let white = pv.white();
    let dim = pv.dim();
    let scene = Rect::new(a.x, a.y, a.w, 108.0);
    pv.rr(scene, 0.0, Rgba::rgb(0x24, 0x27, 0x2D));
    pv.c.push_clip_rect(scene);
    let window = Rect::new(
        scene.x + 9.0,
        scene.y + 17.0,
        scene.w - 18.0,
        scene.h - 25.0,
    );
    pv.rr(window, 5.0, Rgba::rgb(0x17, 0x19, 0x1D));
    let label = pv.t("Your maximized app").to_owned();
    let st = pv.style(11.0, false, dim);
    let lw = pv.fonts.measure_width(&label, &st);
    pv.text(
        window.x + (window.w - lw) / 2.0,
        window.y + 4.0,
        &label,
        11.0,
        false,
        dim,
        None,
    );
    let tuck = s.always_tucked || s.hide_fullscreen || s.hide_maximized;
    let offset = 23.0 - s.tuck_edge as f32;
    let island_w = 110.0;
    let ix = scene.x + (scene.w - island_w) / 2.0;
    let iy = scene.y - if tuck { offset } else { 0.0 };
    pv.rr(Rect::new(ix, iy, island_w, 23.0), 9.0, Rgba::BLACK);
    pv.rr(Rect::new(ix, iy, island_w, 9.0), 0.0, Rgba::BLACK);
    let time = chrono::Local::now().format("%H:%M").to_string();
    let tw = pv.fonts.measure_width(&time, &pv.style(10.0, true, white));
    pv.text(
        ix + (island_w - tw) / 2.0,
        iy + 5.0,
        &time,
        10.0,
        true,
        white,
        None,
    );
    pv.c.pop_clip();
    let message = if tuck {
        format!(
            "{} {:.0} DIP {}",
            pv.t("Hover to reveal ·"),
            s.tuck_edge,
            pv.t("tucked edge. Manual hide removes it entirely.")
        )
    } else {
        pv.t("Auto-tuck is off. The island stays visible.")
            .to_owned()
    };
    let y = scene.bottom() + 12.0;
    let st = pv.style(12.0, false, white);
    let lines = pv.fonts.wrap(&message, &st, a.w);
    let lh = pv.fonts.line_height(&st);
    for (i, line) in lines.iter().enumerate() {
        pv.c.text(pv.fonts, a.x, y + i as f32 * lh, line, &st, None);
    }
    let detail = format!(
        "{}: {} · {}: {}",
        pv.t("Fullscreen"),
        pv.t(if s.hide_fullscreen { "tuck" } else { "visible" }),
        pv.t("Maximized"),
        pv.t(if s.hide_maximized { "tuck" } else { "visible" })
    );
    pv.text(
        a.x,
        y + lines.len() as f32 * lh + 4.0,
        &detail,
        11.0,
        false,
        dim,
        Some(a.w),
    );
    let _ = Fill::Solid(white);
}
