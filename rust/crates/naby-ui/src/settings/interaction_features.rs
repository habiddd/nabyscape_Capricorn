//! Tucking, panel swipe, drag-to-island, detachable previews and cover gestures
//! (C# `SettingsInteractionFeatures.cs`), plus the samples that show them.

use super::preview::Pv;
use super::SettingsView;
use naby_core::color::Rgba;
use naby_render::{Fill, Icon, Rect, Ui};

impl SettingsView {
    pub(super) fn interaction_feature_controls<'a>(&mut self, ui: &mut Ui<'a>) {
        self.section(ui, "Automatic tucking", |this, ui| {
            let always = this.s.always_tucked;
            this.check(ui, "Always tucked", always, |s, v| s.always_tucked = v);
            let edge = this.s.tuck_edge;
            this.slider(ui, "Tucked edge height", edge, 0.0, 12.0, " px", 1.0, |s, v| s.tuck_edge = v);
            this.muted(
                ui,
                "0 hides the dock completely. Use the tuck shortcut or hover the screen edge to reveal it.",
                11.0,
            );
            ui.gap(6.0);
            let (max, full) = (this.s.hide_maximized, this.s.hide_fullscreen);
            this.check(ui, "Tuck above maximized apps", max, |s, v| s.hide_maximized = v);
            this.check(ui, "Tuck above fullscreen apps", full, |s, v| s.hide_fullscreen = v);
            this.muted(
                ui,
                "Leaves a small edge at the top when Chrome or another app fills this display. Move the pointer to that edge to reveal the preview. Manual hide still hides the whole island.",
                11.0,
            );
        });
        self.section(ui, "Panel navigation", |this, ui| {
            let swipe = this.s.panel_swipe;
            this.check(ui, "Swipe between open panels", swipe, |s, v| s.panel_swipe = v);
            this.muted(
                ui,
                "Swipe empty space or use Shift+wheel to switch panels. Long lists keep vertical scrolling. Keyboard: Ctrl+Tab.",
                11.0,
            );
        });
        self.section(ui, "Drag to Island", |this, ui| {
            let reveal = this.s.drop_reveal;
            this.check(ui, "Reveal shelf when dragging files", reveal, |s, v| s.drop_reveal = v);
            this.muted(
                ui,
                "Drag files onto the island or its tucked edge. Drop to stage references; select multiple shelf items to drag them out together. Manual hiding stays hidden.",
                11.0,
            );
        });
        self.section(ui, "Detachable previews", |this, ui| {
            let detach = this.s.detachable_previews;
            this.check(ui, "Allow detachable previews", detach, |s, v| s.detachable_previews = v);
            this.muted(
                ui,
                "Drag the grip to place a mini window, or choose Detach. Supports Music, Timer, Focus, Calendar, Notes, Shelf and System. Closing it keeps music and sessions running.",
                11.0,
            );
        });
        self.section(ui, "Closed cover gestures", |this, ui| {
            let cover = this.s.cover_gestures;
            this.check(ui, "Control playback from album cover", cover, |s, v| s.cover_gestures = v);
            this.muted(
                ui,
                "Choose Album artwork in Audio. Click to play/pause; hold 500 ms to choose a session. Keyboard: Ctrl+Space for playback, Ctrl+M for sessions.",
                11.0,
            );
        });
    }
}

/// `FeaturePreview(kind)` for "Drop zone", "Detached preview" and "Cover gestures"
/// (the other kinds live in `control_appearance`).
pub(super) fn feature_preview(pv: &mut Pv, a: Rect, kind: &str) {
    let white = pv.white();
    let dim = pv.dim();
    let s = pv.s;
    let inactive = match kind {
        "Drop zone" if !s.drop_reveal => Some("Drop reveal is off. Open the shelf to add files."),
        "Detached preview" if !s.detachable_previews => Some("Detachable previews are off."),
        "Cover gestures" if !s.cover_gestures => {
            Some("Cover gestures are off. Click to open the panel.")
        }
        "Cover gestures" if s.closed_media_icon != "Artwork" => {
            Some("Choose Artwork in Appearance to use cover gestures.")
        }
        _ => None,
    };
    if let Some(message) = inactive {
        let message = pv.t(message).to_owned();
        pv.text(a.x, a.y, &message, 12.0, false, dim, Some(a.w));
        return;
    }
    match kind {
        "Drop zone" => {
            let phase = pv.now as f32 * 2.0;
            let mut x = a.x + (a.w - 3.0 * 44.0) / 2.0;
            for i in 0..3 {
                let dy = if s.reduce_motion {
                    0.0
                } else {
                    (phase + i as f32).sin() * 3.0
                };
                let icon = if i == 1 { Icon::Music } else { Icon::NavTools };
                pv.icon(icon, x + 9.0, a.y + 4.0 + dy, 26.0, white);
                x += 44.0;
            }
            let note = pv.t("3 files · drop to stage").to_owned();
            pv.text(a.x, a.y + 44.0, &note, 12.0, false, white, Some(a.w));
        }
        "Detached preview" => detached_example(pv, a),
        _ => {
            // Cover gestures: the album cover button and its state line.
            let fill = Rgba::parse_hex(&s.control_fill).unwrap_or(pv.th.control_fill);
            pv.rr(Rect::new(a.x, a.y, 44.0, 44.0), pv.th.radius, fill);
            pv.icon(
                Icon::Music,
                a.x + 10.0,
                a.y + 10.0,
                24.0,
                naby_render::Theme::contrast_text(fill),
            );
            let note = pv.t("Click cover to play").to_owned();
            pv.text(a.x, a.y + 52.0, &note, 12.0, false, white, Some(a.w));
        }
    }
}

/// `DetachedExample`: a mini window with a header, close button and note field.
fn detached_example(pv: &mut Pv, a: Rect) {
    let white = pv.white();
    let dim = pv.dim();
    let s = pv.s;
    let frame = Rect::new(a.x, a.y, a.w, 150.0);
    let border = Rgba::parse_hex(&s.control_border).unwrap_or(pv.th.control_border);
    let background = Rgba::parse_hex(&s.background).unwrap_or(Rgba::BLACK);
    pv.rr(frame, 14.0, background);
    pv.outline(frame, 14.0, border);
    pv.text(
        frame.x + 12.0,
        frame.y + 12.0,
        "Notes",
        13.0,
        s.bold_headings,
        white,
        None,
    );
    pv.c.stroke_circle(frame.right() - 23.0, frame.y + 20.0, 8.0, dim, 1.0);
    pv.icon(Icon::X, frame.right() - 31.0, frame.y + 12.0, 16.0, dim);
    let note = Rect::new(frame.x + 12.0, frame.y + 40.0, frame.w - 24.0, 70.0);
    let fill = Rgba::parse_hex(&s.control_fill).unwrap_or(pv.th.control_fill);
    pv.rr(note, pv.th.radius, fill);
    pv.outline(note, pv.th.radius, border);
    let text = pv.t("A little space for your next idea.").to_owned();
    pv.text(
        note.x + 10.0,
        note.y + 8.0,
        &text,
        13.0,
        false,
        naby_render::Theme::contrast_text(fill),
        Some(note.w - 20.0),
    );
    let hint = pv
        .t("Drag the example header · close button dims this example")
        .to_owned();
    pv.text(
        a.x,
        frame.bottom() + 8.0,
        &hint,
        10.0,
        false,
        dim,
        Some(a.w),
    );
    let _ = Fill::Solid(white);
}
