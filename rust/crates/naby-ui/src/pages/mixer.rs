//! The audio mixer (`MixerOverlay.cs`): a back button, the Apps / Outputs scope, and one row per
//! audio session or output with an icon, name, percentage, mute and a volume slider. The media
//! page swaps its body for [`MixerView`]; the `mixer` page id hosts the same view on its own.

use super::common::{Ctl, Interaction};
use super::{Page, PageCtx};
use crate::host::Demand;
use naby_core::color::Rgba;
use naby_core::command::Command;
use naby_core::media::{MixerItem, MixerScope, MixerTarget};
use naby_render::{Canvas, Icon, InputState, Rect, Weight};

pub(super) const BACK: u32 = 100;
pub(super) const APPS: u32 = 101;
pub(super) const OUTPUTS: u32 = 102;
/// Rows use `ROW + 4 * index` (mute) and `ROW + 4 * index + 1` (slider).
pub(super) const ROW: u32 = 200;

const ICON_FILL: Rgba = Rgba::rgb(0x27, 0x29, 0x2F);
const ICON_BORDER: Rgba = Rgba::rgb(0x41, 0x44, 0x4D);
const ICON_TEXT: Rgba = Rgba::rgb(0xDF, 0xE1, 0xE7);

pub fn build() -> Box<dyn Page> {
    Box::new(MixerPage::default())
}

/// The letters of the app tile (`MixerAppIcon.Monogram`).
pub fn monogram(name: &str) -> String {
    let words: Vec<&str> = name
        .split([' ', '-', '_'])
        .filter(|w| !w.is_empty())
        .collect();
    let mut letters: String = words
        .iter()
        .filter_map(|w| w.chars().find(|c| c.is_alphanumeric()))
        .take(2)
        .collect();
    if words.len() == 1 {
        letters = words[0]
            .chars()
            .filter(|c| c.is_alphanumeric())
            .take(2)
            .collect();
    }
    if letters.is_empty() {
        "\u{2022}".to_owned()
    } else {
        letters.to_uppercase()
    }
}

#[derive(Default)]
pub(super) struct MixerView {
    pub scope: MixerScope,
    /// The slider being dragged and its value.
    drag: Option<(MixerTarget, f32)>,
    /// A just-committed slider value, held until the host reports it back.
    held: Option<(MixerTarget, f32, f64)>,
    refresh: bool,
}

/// What the view asks the page to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MixerEvent {
    None,
    Back,
}

impl MixerView {
    /// The view was opened: read the devices.
    pub fn open(&mut self) {
        self.refresh = true;
        self.drag = None;
        self.held = None;
    }

    pub fn reset(&mut self) {
        *self = MixerView::default();
    }

    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    fn shown_volume(&self, item: &MixerItem, now: f64) -> f32 {
        if let Some((t, v)) = &self.drag {
            if *t == item.target {
                return *v;
            }
        }
        if let Some((t, v, until)) = &self.held {
            if *t == item.target && now < *until {
                return *v;
            }
        }
        item.volume as f32
    }

    /// Lays out the whole mixer in `rect.w`; returns its height.
    pub fn layout(&mut self, c: &mut Ctl, rect: Rect) -> (f32, MixerEvent) {
        let th = c.theme().clone();
        let s = c.s;
        let snaps = c.ctx.snaps;
        let mut event = MixerEvent::None;
        if self.refresh && !c.is_dry() {
            self.refresh = false;
            c.push(Command::MixerRefresh);
        }
        if self.held.as_ref().is_some_and(|h| c.now >= h.2) {
            self.held = None;
        }
        let mut y = rect.y;

        // ---- header ---------------------------------------------------------------------------
        let hw = if th.bold_headings {
            Weight::Semibold
        } else {
            Weight::Regular
        };
        let back_w = c.text_w("Back to media", 11.0, Weight::Regular) + 18.0 * s;
        let head_h = (28.0 * s).max(c.line_h(16.0, hw));
        let back = Rect::new(rect.x, y + (head_h - 28.0 * s) / 2.0, back_w, 28.0 * s);
        let b = c.text_button(
            BACK,
            back,
            "Back to media",
            11.0,
            Weight::Regular,
            th.text,
            None,
            true,
        );
        if b.clicked {
            event = MixerEvent::Back;
        }
        let hy = y + (head_h - c.line_h(16.0, hw)) / 2.0;
        c.text(
            back.right() + 12.0 * s,
            hy,
            "Audio mixer",
            16.0,
            hw,
            th.text,
            Some((rect.right() - back.right() - 12.0 * s).max(1.0)),
        );
        y += head_h + 12.0 * s;

        // ---- status --------------------------------------------------------------------------------
        let mixer = &*snaps.mixer;
        let status = mixer.errors.join("; ");
        if !status.is_empty() {
            y += 8.0 * s;
            y += c.paragraph(
                rect.x,
                y,
                rect.w,
                &status,
                11.0,
                Weight::Regular,
                th.text_dim,
            );
            y += 12.0 * s;
        }

        // ---- scope ------------------------------------------------------------------------------------
        let tab_w = (rect.w / 2.0 - 4.0 * s).max(1.0);
        for (i, (id, scope, label)) in [
            (APPS, MixerScope::Apps, "Apps"),
            (OUTPUTS, MixerScope::Outputs, "Outputs"),
        ]
        .into_iter()
        .enumerate()
        {
            let r = Rect::new(rect.x + i as f32 * rect.w / 2.0, y, tab_w, 28.0 * s);
            let selected = self.scope == scope;
            let (fill, color) = if selected {
                (Some(th.selection_fill), th.selection_text)
            } else {
                (None, th.text_dim)
            };
            let b = c.text_button(id, r, label, 11.0, Weight::Regular, color, fill, true);
            if b.clicked && !selected {
                self.scope = scope;
                self.drag = None;
                self.refresh = true;
            }
        }
        y += 28.0 * s + 12.0 * s;

        // ---- rows ---------------------------------------------------------------------------------------
        let items = match self.scope {
            MixerScope::Apps => &mixer.apps,
            MixerScope::Outputs => &mixer.outputs,
        };
        if items.is_empty() {
            let text = match self.scope {
                MixerScope::Apps => "No app audio sessions available.",
                MixerScope::Outputs => "No active audio outputs.",
            };
            y += c.paragraph(rect.x, y, rect.w, text, 12.0, Weight::Regular, th.text_dim);
        }
        for (i, item) in items.iter().enumerate() {
            y = self.row(c, rect, y, i, item);
        }
        (y - rect.y, event)
    }

    fn row(&mut self, c: &mut Ctl, rect: Rect, y: f32, index: usize, item: &MixerItem) -> f32 {
        let th = c.theme().clone();
        let s = c.s;
        let apps = item.target.scope == MixerScope::Apps;
        let name_h = c.line_h(12.0, Weight::Regular);
        let out_h = if apps {
            c.line_h(10.0, Weight::Regular)
        } else {
            0.0
        };
        let head_h = name_h + out_h;
        let row0 = head_h.max(28.0 * s).max(20.0 * s);
        let row_h = (58.0 * s).max(row0 + 5.0 * s + 24.0 * s);
        // Icon.
        let icon = Rect::new(rect.x, y + (row0 - 20.0 * s) / 2.0, 20.0 * s, 20.0 * s);
        if apps {
            c.rrect(icon, 5.0, ICON_FILL);
            c.rrect_stroke(icon, 5.0, ICON_BORDER, 0.7);
            c.text_in(
                icon,
                &monogram(&item.name),
                9.0,
                Weight::Semibold,
                ICON_TEXT,
            );
        } else {
            c.icon(Icon::Volume, icon, th.text_dim);
        }
        // Name and output.
        let tx = rect.x + 32.0 * s;
        let mute_w = 30.0 * s;
        let pct_w = 42.0 * s;
        let text_w = (rect.w - 32.0 * s - pct_w - mute_w - 6.0 * s).max(1.0);
        let ty = y + (row0 - head_h) / 2.0;
        c.text(
            tx,
            ty,
            &item.name,
            12.0,
            Weight::Regular,
            th.text,
            Some(text_w),
        );
        if apps {
            c.text(
                tx,
                ty + name_h,
                &item.output_name,
                10.0,
                Weight::Regular,
                th.text_dim,
                Some(text_w),
            );
        }
        // Percentage.
        let value = self.shown_volume(item, c.now);
        let pct = format!("{}%", (value * 100.0).round() as i32);
        let py = y + (row0 - c.line_h(11.0, Weight::Regular)) / 2.0;
        c.text_right(
            rect.right() - mute_w,
            py,
            &pct,
            11.0,
            Weight::Regular,
            th.text_dim,
            Some(38.0 * s),
        );
        // Mute.
        let mute = Rect::new(
            rect.right() - mute_w,
            y + (row0 - 28.0 * s) / 2.0,
            mute_w,
            28.0 * s,
        );
        let id = ROW + 4 * index as u32;
        let icon = if item.muted {
            Icon::Muted
        } else {
            Icon::Volume
        };
        let b = c.icon_button(id, mute, icon, 16.0, None, Rgba::WHITE, true);
        if b.clicked {
            c.push(Command::MixerSet {
                target: item.target.clone(),
                level: None,
                muted: Some(!item.muted),
            });
            c.push(Command::MixerRefresh);
        }
        // Slider.
        let slider = Rect::new(
            rect.x + 32.0 * s,
            y + row0 + 5.0 * s,
            (rect.w - 40.0 * s).max(1.0),
            24.0 * s,
        );
        let r = c.slider(id + 1, slider, value, 0.01, true);
        if !c.is_dry() {
            if r.changed {
                self.drag = Some((item.target.clone(), r.value));
                self.held = None;
            }
            let was_dragging = matches!(&self.drag, Some((t, _)) if *t == item.target);
            // Released (or a key step): apply the level once, like `Commit` on mouse-up / key-up.
            if was_dragging && (r.committed || !r.dragging) {
                if let Some((target, v)) = self.drag.take() {
                    c.push(Command::MixerSet {
                        target: target.clone(),
                        level: Some(f64::from(v)),
                        muted: None,
                    });
                    self.held = Some((target, v, c.now + 1.0));
                }
            }
        }
        y + row_h + 14.0 * s
    }
}

#[derive(Default)]
struct MixerPage {
    ui: Interaction,
    view: MixerView,
    opened: bool,
}

impl Page for MixerPage {
    fn id(&self) -> &'static str {
        "mixer"
    }

    fn height(&mut self, ctx: &PageCtx, width: f32) -> f32 {
        let input = InputState::default();
        let mut c = Ctl::measure(ctx, &input, &mut self.ui);
        self.view.layout(&mut c, Rect::new(0.0, 0.0, width, 0.0)).0
    }

    fn frame(
        &mut self,
        canvas: &mut Canvas,
        ctx: &PageCtx,
        rect: Rect,
        input: &InputState,
        out: &mut Vec<Command>,
    ) -> bool {
        if !self.opened {
            self.opened = true;
            self.view.open();
        }
        let mut c = Ctl::draw(canvas, ctx, input, out, &mut self.ui);
        self.view.layout(&mut c, rect);
        false
    }

    fn demand(&self) -> Demand {
        Demand {
            mixer: true,
            ..Demand::default()
        }
    }

    fn next_frame(&self, _now: f64) -> Option<f64> {
        None
    }

    fn captures_pointer(&self) -> bool {
        self.ui.captures() || self.view.dragging()
    }

    fn on_hide(&mut self) {
        self.ui.reset();
        self.view.reset();
        self.opened = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pages::common::harness::Harness;
    use naby_core::media::MixerSnapshot;

    fn item(app: bool, name: &str, output: &str, volume: f64, muted: bool) -> MixerItem {
        MixerItem {
            target: if app {
                MixerTarget::app("out-1", format!("s-{name}"))
            } else {
                MixerTarget::output(format!("out-{name}"))
            },
            name: name.into(),
            output_name: output.into(),
            volume,
            muted,
            process_path: None,
        }
    }

    fn feed(h: &mut Harness) {
        h.snaps.mixer.replace(MixerSnapshot {
            apps: vec![
                item(true, "Spotify", "Speakers", 0.65, false),
                item(true, "Google Chrome", "Speakers", 0.3, true),
                item(true, "discord-ptb", "Headset", 1.0, false),
            ],
            outputs: vec![
                item(false, "Speakers", "", 0.8, false),
                item(false, "Headset", "", 0.4, true),
            ],
            errors: vec![],
        });
    }

    #[test]
    fn monograms_follow_the_words_of_the_name() {
        assert_eq!(monogram("Spotify"), "SP");
        assert_eq!(monogram("Google Chrome"), "GC");
        assert_eq!(monogram("discord-ptb"), "DP");
        assert_eq!(monogram("  "), "\u{2022}");
        assert_eq!(monogram("7zip"), "7Z");
    }

    #[test]
    fn opening_reads_the_devices_and_the_page_asks_for_snapshots() {
        let Some(mut h) = Harness::new("mixer") else {
            return;
        };
        h.idle();
        assert!(h.page.demand().mixer);
        assert_eq!(h.commands(), vec![Command::MixerRefresh]);
        h.idle();
        assert!(h.commands().is_empty(), "read once");
        h.click_id(OUTPUTS);
        assert_eq!(h.commands(), vec![Command::MixerRefresh]);
    }

    #[test]
    fn mute_and_slider_write_through_commands() {
        let Some(mut h) = Harness::new("mixer") else {
            return;
        };
        feed(&mut h);
        h.idle();
        h.commands();
        h.click_id(ROW);
        let cmds = h.commands();
        assert_eq!(
            cmds[0],
            Command::MixerSet {
                target: MixerTarget::app("out-1", "s-Spotify"),
                level: None,
                muted: Some(true),
            }
        );
        h.drag_id(ROW + 1, 0.5);
        let cmds = h.commands();
        match &cmds[..] {
            [Command::MixerSet {
                target,
                level: Some(v),
                muted: None,
            }] => {
                assert_eq!(*target, MixerTarget::app("out-1", "s-Spotify"));
                assert!((*v - 0.5).abs() < 0.03, "{v}");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn the_back_button_reports_back() {
        let Some(mut h) = Harness::new("mixer") else {
            return;
        };
        h.idle();
        let mut ui = Interaction::default();
        let mut view = MixerView::default();
        let mut canvas = Canvas::new(400, 300);
        let ctx = h.ctx();
        let mut out = Vec::new();
        let input = InputState::default();
        {
            let mut c = Ctl::draw(&mut canvas, &ctx, &input, &mut out, &mut ui);
            view.layout(&mut c, Rect::new(10.0, 10.0, 340.0, 0.0));
        }
        let p = h.center_of(BACK);
        let press = InputState::press(p.0, p.1);
        let release = InputState::release(p.0, p.1);
        let mut events = Vec::new();
        for input in [InputState::at(p.0, p.1), press, release] {
            let mut c = Ctl::draw(&mut canvas, &ctx, &input, &mut out, &mut ui);
            events.push(view.layout(&mut c, Rect::new(10.0, 10.0, 340.0, 0.0)).1);
        }
        assert_eq!(events.last(), Some(&MixerEvent::Back));
    }

    #[test]
    fn empty_scopes_say_so_and_snapshots_paint() {
        let Some(mut h) = Harness::new("mixer") else {
            return;
        };
        h.idle();
        assert!(h.ink() > 500);
        h.save("page-mixer-empty");
        feed(&mut h);
        h.idle();
        h.save("page-mixer-apps");
        h.click_id(OUTPUTS);
        h.idle();
        h.save("page-mixer-outputs");
        h.snaps.mixer.replace(MixerSnapshot {
            errors: vec!["Audio service unavailable".into(), "Device busy".into()],
            ..MixerSnapshot::default()
        });
        h.idle();
        h.save("page-mixer-errors");
    }
}
