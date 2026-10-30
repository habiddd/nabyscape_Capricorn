//! The footer of the media page (`MediaControls.cs`): the media source button on the left, the
//! volume level button on the right, and the bounded popover below them (source list, volume
//! controls, or the output / microphone menu of `OpenAudioMenu`).

use crate::host::{MediaSource, Snapshots};
use crate::pages::common::Ctl;
use naby_core::color::Rgba;
use naby_core::command::Command;
use naby_core::media::source::{AUTO_ID, NONE_ID};
use naby_core::motion::{Ease, MotionConfig, Tween};
use naby_render::icons::parse_path;
use naby_render::{Fill, Icon, Rect, Weight};
use tiny_skia::Transform;

pub const SOURCE: u32 = 6;
pub const LEVEL: u32 = 7;
pub const MUTE: u32 = 10;
pub const SLIDER: u32 = 11;
pub const MIXER: u32 = 12;
pub const DEVICES: u32 = 13;
pub const MENU: u32 = 40;
pub const SRC: u32 = 60;

const OPEN_FILL: Rgba = Rgba::rgb(0x25, 0x27, 0x2B);
const POP_FILL: Rgba = Rgba::rgb(0x11, 0x12, 0x16);
const POP_BORDER: Rgba = Rgba::rgb(0x53, 0x56, 0x5D);
const MENU_FILL: Rgba = Rgba::rgb(0x19, 0x1A, 0x1E);
const MENU_BORDER: Rgba = Rgba::rgb(0x42, 0x43, 0x4B);
const SELECTED_ROW: Rgba = Rgba::rgb(0x24, 0x26, 0x2B);
const SPOTIFY_DISC: Rgba = Rgba::rgb(0xC5, 0xC8, 0xCD);
const SPOTIFY_WAVES: Rgba = Rgba::rgb(0x10, 0x13, 0x12);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pop {
    Source,
    Volume,
    Menu,
}

/// What the footer asks the page to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FooterEvent {
    None,
    OpenMixer,
}

#[derive(Default)]
pub struct Footer {
    pub open: Option<Pop>,
    opened_at: f64,
    /// The volume slider value shown while dragging and just after.
    vol_hold: Option<(f32, f64)>,
    slider_dragging: bool,
}

/// The sessions to pick from: None, Auto, then the running apps.
fn choices(snaps: &Snapshots) -> impl Iterator<Item = (&str, &str)> {
    [(NONE_ID, "None"), (AUTO_ID, "Auto")].into_iter().chain(
        snaps
            .sources
            .sessions
            .iter()
            .map(|s| (s.id.as_str(), s.name.as_str())),
    )
}

impl Footer {
    pub fn reset(&mut self) {
        *self = Footer::default();
    }

    pub fn close(&mut self) {
        self.open = None;
        self.slider_dragging = false;
    }

    /// The slider is held: the panel must not swipe.
    pub fn captures(&self) -> bool {
        self.slider_dragging
    }

    fn toggle(&mut self, pop: Pop, now: f64) {
        if self.open == Some(pop) {
            self.close();
        } else {
            self.open = Some(pop);
            self.opened_at = now;
        }
    }

    /// When the popover entrance (or the volume hold) needs another frame.
    pub fn next_frame(&self, now: f64) -> Option<f64> {
        let mut next = None;
        if self.open.is_some() && now - self.opened_at < 0.25 {
            next = Some(now + 1.0 / 60.0);
        }
        if let Some((_, until)) = self.vol_hold {
            if now < until {
                next = Some(next.map_or(until, |n: f64| n.min(until)));
            }
        }
        next
    }

    /// Footer buttons and the open popover starting at `rect.y`; returns the height used: the
    /// block that holds the buttons (`block_h`, at least 28 DIP), plus 6 DIP and the popover
    /// when one is open.
    pub fn layout(
        &mut self,
        c: &mut Ctl,
        rect: Rect,
        show_source: bool,
        show_volume: bool,
        block_h: f32,
    ) -> (f32, FooterEvent) {
        let th = c.theme().clone();
        let s = c.s;
        let snaps: &Snapshots = c.ctx.snaps;
        let mut event = FooterEvent::None;
        if !show_source && self.open == Some(Pop::Source) {
            self.close();
        }
        if !show_volume && matches!(self.open, Some(Pop::Volume | Pop::Menu)) {
            self.close();
        }
        let bh = 28.0 * s;
        let y = rect.y;

        if show_source {
            let selected = snaps.sources.selected.as_str();
            let name = choices(snaps)
                .find(|(id, _)| *id == selected)
                .map_or("Media", |(_, n)| n);
            let max_name = (48.0 * s).max(rect.w - 145.0 * s);
            let name_w = c.text_w(name, 11.0, Weight::Regular).min(max_name);
            let w = (5.0 + 16.0 + 6.0 + 7.0 + 8.0 + 5.0) * s + name_w;
            let r = Rect::new(rect.x, y, w, bh);
            let b = c.button(SOURCE, r, true);
            let open = self.open == Some(Pop::Source);
            c.button_chrome(r, 8.0, open.then_some(OPEN_FILL), true, &b);
            let cy = y + bh / 2.0;
            source_glyph(c, rect.x + 5.0 * s + 8.0 * s, cy, selected);
            let ty = y + (bh - c.line_h(11.0, Weight::Regular)) / 2.0;
            c.text(
                rect.x + 27.0 * s,
                ty,
                name,
                11.0,
                Weight::Regular,
                th.text,
                Some(max_name),
            );
            c.icon_at(
                Icon::Chevron,
                rect.x + 27.0 * s + name_w + 7.0 * s + 4.0 * s,
                cy,
                8.0,
                th.text_dim,
            );
            if b.clicked {
                self.toggle(Pop::Source, c.now);
            }
        }
        if show_volume {
            let audio = &*snaps.audio;
            let text = if audio.available {
                format!("{}%", (audio.volume * 100.0).round() as i32)
            } else {
                "\u{2014}".to_owned()
            };
            let tw = c.text_w(&text, 11.0, Weight::Regular);
            let w = (5.0 + 16.0 + 6.0 + 5.0) * s + tw;
            let r = Rect::new(rect.right() - w, y, w, bh);
            let b = c.button(LEVEL, r, true);
            let open = matches!(self.open, Some(Pop::Volume | Pop::Menu));
            c.button_chrome(r, 8.0, open.then_some(OPEN_FILL), true, &b);
            let cy = y + bh / 2.0;
            let icon = if audio.muted {
                Icon::Muted
            } else {
                Icon::Volume
            };
            c.icon_at(icon, r.x + 5.0 * s + 8.0 * s, cy, 16.0, Rgba::WHITE);
            let ty = y + (bh - c.line_h(11.0, Weight::Regular)) / 2.0;
            c.text(
                r.x + 27.0 * s,
                ty,
                &text,
                11.0,
                Weight::Regular,
                th.text_dim,
                None,
            );
            if b.clicked {
                self.toggle(Pop::Volume, c.now);
                if self.open.is_some() {
                    c.push(Command::RefreshAudio);
                }
            }
        }
        let block = block_h.max(bh);
        let mut height = block;

        if let Some(pop) = self.open {
            let width_dip = match pop {
                Pop::Source => 224.0,
                Pop::Volume => 240.0,
                Pop::Menu => 260.0,
            };
            let w = (width_dip * s).min(rect.w);
            let x = if pop == Pop::Source {
                rect.x
            } else {
                rect.right() - w
            };
            let pad = if pop == Pop::Menu { 5.0 * s } else { 8.0 * s };
            let inner_w = (w - 2.0 * pad).max(1.0);
            let top = y + block + 6.0 * s;
            // Measure first so the frame can be drawn behind the content.
            let content_h = c.dry(|c| {
                let mut ev = FooterEvent::None;
                self.content(c, pop, Rect::new(x + pad, top + pad, inner_w, 0.0), &mut ev)
            });
            let frame = Rect::new(x, top, w, content_h + 2.0 * pad);
            let motion = MotionConfig::from_settings(c.ctx.settings(), true);
            let (alpha, dy) = if motion.enabled {
                let fade = Tween::new(0.0, 1.0, self.opened_at, 0.22, Ease::CUBIC_OUT);
                let slide = Tween::new(5.0, 0.0, self.opened_at, 0.18, Ease::EXPO_OUT_5);
                (fade.value(c.now) as f32, slide.value(c.now) as f32 * s)
            } else {
                (1.0, 0.0)
            };
            let frame = frame.translate(0.0, dy);
            let (fill, border, radius) = if pop == Pop::Menu {
                (MENU_FILL, MENU_BORDER, 8.0)
            } else {
                (POP_FILL, POP_BORDER, 10.0)
            };
            let area = Rect::new(frame.x + pad, frame.y + pad, inner_w, 0.0);
            c.faded(alpha, |c| {
                c.rrect(frame, radius, fill);
                c.rrect_stroke(frame, radius, border, 0.7);
                self.content(c, pop, area, &mut event);
            });
            height += 6.0 * s + frame.h;
        }
        (height, event)
    }

    fn content(&mut self, c: &mut Ctl, pop: Pop, area: Rect, event: &mut FooterEvent) -> f32 {
        match pop {
            Pop::Source => self.source_list(c, area),
            Pop::Volume => self.volume_controls(c, area, event),
            Pop::Menu => self.audio_menu(c, area, event),
        }
    }

    // ---- the source list ---------------------------------------------------------------------

    fn source_list(&mut self, c: &mut Ctl, area: Rect) -> f32 {
        let th = c.theme().clone();
        let s = c.s;
        let snaps: &Snapshots = c.ctx.snaps;
        let mut y = area.y;
        c.text(
            area.x + 3.0 * s,
            y,
            "Media source",
            11.0,
            Weight::Regular,
            th.text_dim,
            Some(area.w),
        );
        y += c.line_h(11.0, Weight::Regular) + 8.0 * s;
        if snaps.sources.sessions.is_empty() {
            y += 6.0 * s;
            y += c.paragraph(
                area.x + 3.0 * s,
                y,
                area.w - 6.0 * s,
                "No active media sessions.\nPlay something in Spotify or your browser.",
                11.0,
                Weight::Regular,
                th.text_dim,
            );
            y += 6.0 * s;
        }
        let selected = snaps.sources.selected.as_str();
        for (i, (id, name)) in choices(snaps).enumerate() {
            let h = c
                .line_h(14.0, Weight::Regular)
                .max(c.line_h(12.0, Weight::Regular))
                .max(16.0 * s)
                + 12.0 * s;
            let r = Rect::new(area.x, y, area.w, h);
            let on = id == selected;
            let b = c.button(SRC + i as u32, r, true);
            c.button_chrome(r, 8.0, on.then_some(SELECTED_ROW), true, &b);
            source_glyph(c, r.x + 6.0 * s + 8.0 * s, y + h / 2.0, id);
            let nx = r.x + 6.0 * s + 16.0 * s + 10.0 * s;
            let ty = y + (h - c.line_h(12.0, Weight::Regular)) / 2.0;
            c.text(
                nx,
                ty,
                name,
                12.0,
                Weight::Regular,
                th.text,
                Some((r.right() - 6.0 * s - 22.0 * s - nx).max(1.0)),
            );
            if on {
                let cy = y + (h - c.line_h(14.0, Weight::Regular)) / 2.0;
                c.text_right(
                    r.right() - 6.0 * s,
                    cy,
                    "\u{2713}",
                    14.0,
                    Weight::Regular,
                    th.text,
                    None,
                );
            }
            if b.clicked {
                c.push(Command::MediaSelectSource(match id {
                    NONE_ID => MediaSource::None,
                    AUTO_ID => MediaSource::Auto,
                    other => MediaSource::App(other.to_owned()),
                }));
                self.close();
            }
            y += h;
        }
        y - area.y
    }

    // ---- the volume popover ---------------------------------------------------------------------

    fn volume_controls(&mut self, c: &mut Ctl, area: Rect, event: &mut FooterEvent) -> f32 {
        let th = c.theme().clone();
        let s = c.s;
        let snaps: &Snapshots = c.ctx.snaps;
        let audio = &*snaps.audio;
        let mut y = area.y;
        // Header: "Volume", the percentage and the devices button.
        let head_h = 22.0 * s;
        let devices = Rect::new(area.right() - 22.0 * s, y, 22.0 * s, head_h);
        let b = c.icon_button(
            DEVICES,
            devices,
            Icon::Chevron,
            14.0,
            None,
            Rgba::WHITE,
            true,
        );
        if b.clicked {
            self.open = Some(Pop::Menu);
            self.opened_at = c.now;
            c.push(Command::RefreshAudio);
        }
        let shown = self.shown_volume(c.now, audio.volume);
        let pct = if audio.available {
            format!("{}%", (shown * 100.0).round() as i32)
        } else {
            "\u{2014}".to_owned()
        };
        let pw = c.text_w(&pct, 11.0, Weight::Regular);
        let py = y + (head_h - c.line_h(11.0, Weight::Regular)) / 2.0;
        c.text(
            devices.x - 6.0 * s - pw,
            py,
            &pct,
            11.0,
            Weight::Regular,
            th.text_dim,
            None,
        );
        let ty = y + (head_h - c.line_h(12.0, Weight::Regular)) / 2.0;
        c.text(
            area.x,
            ty,
            "Volume",
            12.0,
            Weight::Regular,
            th.text,
            Some(area.w),
        );
        y += head_h + 7.0 * s;
        // Row: mute, slider, mixer.
        let row_h = 28.0 * s;
        let mute = Rect::new(area.x, y, 28.0 * s, row_h);
        let icon = if audio.muted {
            Icon::Muted
        } else {
            Icon::Volume
        };
        let b = c.icon_button(MUTE, mute, icon, 18.0, None, Rgba::WHITE, true);
        if b.clicked {
            c.push(Command::ToggleMute);
            c.push(Command::RefreshAudio);
        }
        let mixer = Rect::new(area.right() - 28.0 * s, y, 28.0 * s, row_h);
        let b = c.icon_button(MIXER, mixer, Icon::Mixer, 20.0, None, Rgba::WHITE, true);
        if b.clicked {
            c.push(Command::MixerRefresh);
            *event = FooterEvent::OpenMixer;
            self.close();
            return 0.0;
        }
        let sx = mute.right() + 8.0 * s;
        let slider = Rect::new(
            sx,
            y + (row_h - 22.0 * s) / 2.0,
            (mixer.x - 7.0 * s - 5.0 * s - sx).max(1.0),
            22.0 * s,
        );
        let r = c.slider(SLIDER, slider, shown, 0.01, audio.available);
        if !c.is_dry() {
            self.slider_dragging = r.dragging;
        }
        if r.changed {
            self.vol_hold = Some((r.value, c.now + 0.6));
            c.push(Command::SetVolume(f64::from(r.value)));
        }
        y += row_h;
        y - area.y
    }

    fn shown_volume(&self, now: f64, actual: f32) -> f32 {
        match self.vol_hold {
            Some((v, until)) if now < until || self.slider_dragging => v,
            _ => actual,
        }
    }

    // ---- the audio menu ---------------------------------------------------------------------------

    fn audio_menu(&mut self, c: &mut Ctl, area: Rect, event: &mut FooterEvent) -> f32 {
        let th = c.theme().clone();
        let s = c.s;
        let snaps: &Snapshots = c.ctx.snaps;
        let audio = &*snaps.audio;
        let mut y = area.y;
        let row_h = c.line_h(13.0, Weight::Regular) + 16.0 * s;
        let mut index = 0u32;
        let mut item = |this: &mut Footer,
                        c: &mut Ctl,
                        y: &mut f32,
                        label: &str,
                        checked: bool,
                        enabled: bool|
         -> bool {
            let r = Rect::new(area.x, *y, area.w, row_h);
            let b = c.button(MENU + index, r, enabled);
            index += 1;
            if b.hovered && enabled {
                c.rrect(r, 7.0, th.selection_fill);
            }
            let color = if b.hovered && enabled {
                th.selection_text
            } else {
                Rgba::WHITE
            };
            let alpha = if enabled { 1.0 } else { 0.4 };
            c.faded(alpha, |c| {
                if checked {
                    let (cx, cy) = (r.x + 10.0 * s + 6.0 * s, *y + row_h / 2.0);
                    c.stroke_polyline(
                        &[
                            (cx - 5.5 * s, cy),
                            (cx - 1.5 * s, cy + 4.0 * s),
                            (cx + 6.0 * s, cy - 4.0 * s),
                        ],
                        color,
                        1.5 * s,
                    );
                }
                let ty = *y + (row_h - c.line_h(13.0, Weight::Regular)) / 2.0;
                c.text(
                    r.x + 10.0 * s + 18.0 * s,
                    ty,
                    label,
                    13.0,
                    Weight::Regular,
                    color,
                    Some(r.w - 38.0 * s),
                );
            });
            let _ = this;
            *y += row_h;
            b.clicked
        };
        if item(self, c, &mut y, "Volume mixer", false, true) {
            c.push(Command::MixerRefresh);
            *event = FooterEvent::OpenMixer;
            self.close();
            return 0.0;
        }
        separator(c, area, &mut y);
        if audio.outputs.is_empty() {
            item(self, c, &mut y, "No active output devices", false, false);
        }
        for out in &audio.outputs {
            if item(
                self,
                c,
                &mut y,
                &out.name,
                out.id == audio.default_output_id,
                true,
            ) {
                c.push(Command::SetOutput(out.id.clone()));
                c.push(Command::RefreshAudio);
                self.close();
                return 0.0;
            }
        }
        separator(c, area, &mut y);
        let mic = format!(
            "{} microphone \u{b7} {}",
            if audio.mic_muted { "Unmute" } else { "Mute" },
            audio.mic_name
        );
        if item(self, c, &mut y, &mic, false, audio.mic_available) {
            c.push(Command::ToggleMicMute);
            c.push(Command::RefreshAudio);
        }
        y - area.y
    }
}

fn separator(c: &mut Ctl, area: Rect, y: &mut f32) {
    let s = c.s;
    let line = c.theme().separator;
    c.fill(
        Rect::new(area.x + 4.0 * s, *y + 4.0 * s, area.w - 8.0 * s, s.max(1.0)),
        line,
    );
    *y += 9.0 * s;
}

/// `SourceGlyph`: a drawn Spotify mark for Spotify, a music note otherwise (16 DIP).
fn source_glyph(c: &mut Ctl, cx: f32, cy: f32, id: &str) {
    let s = c.s;
    if id.to_ascii_lowercase().contains("spotify") {
        c.circle(cx, cy, 8.0 * s, SPOTIFY_DISC);
        if let Some(waves) = parse_path("M4,6 Q9,4 14,7 M5,9 Q9,7.5 13,10 M6,12 Q9,11 12,13") {
            let k = 16.0 / 18.0 * s;
            let t = Transform::from_row(k, 0.0, 0.0, k, cx - 8.0 * s, cy - 8.0 * s);
            c.stroke_path_t(&waves, &Fill::Solid(SPOTIFY_WAVES), 1.3, t);
        }
    } else {
        let muted = c.theme().text_dim;
        c.icon_at(Icon::Music, cx, cy, 16.0, muted);
    }
}
