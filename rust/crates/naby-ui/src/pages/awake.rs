//! The `awake` page (C# `RoadmapOverlay.BuildAwake`): keep the computer awake for a chosen time.
//! State comes from `Snapshots::awake`; start and stop are `Command`s.

use super::tools_common::{base_key, heading_weight, key_of, mark, tr, Shell};
use super::{Page, PageCtx};
use crate::host::AwakeInfo;
use naby_core::command::Command;
use naby_render::{Canvas, InputState, Rect, Ui, Weight};

/// The duration choices, in the order of the combo box.
pub const DURATIONS: [(&str, Option<u32>); 4] = [
    ("30 minutes", Some(30)),
    ("1 hour", Some(60)),
    ("2 hours", Some(120)),
    ("Until NabyScape quits", None),
];

pub fn build() -> Box<dyn Page> {
    Box::new(AwakePage {
        shell: Shell::new(),
        duration: 0,
        display_on: false,
        refresh_at: None,
    })
}

struct AwakePage {
    shell: Shell,
    duration: usize,
    display_on: bool,
    /// While a timed session runs, when the remaining minutes are read again.
    refresh_at: Option<f64>,
}

/// Seconds between refreshes of the remaining time of a timed session.
const REFRESH: f64 = 30.0;

/// `UpdateRoadmapStatus`: the status text for a state.
pub fn status_text(ctx: &PageCtx, info: &AwakeInfo) -> String {
    if !info.active {
        return tr(ctx, "Off \u{b7} Windows power settings apply").to_owned();
    }
    let detail = match info.minutes_left {
        Some(m) => format!("{m} min remaining"),
        None => tr(ctx, "Until NabyScape quits").to_owned(),
    };
    let display = if info.keep_display_on {
        tr(ctx, "Display stays on")
    } else {
        tr(ctx, "Display can turn off")
    };
    format!("{} \u{b7} {detail}\n{display}", tr(ctx, "Awake"))
}

fn layout(
    ui: &mut Ui,
    ctx: &PageCtx,
    info: &AwakeInfo,
    duration: &mut usize,
    display_on: &mut bool,
    out: &mut Vec<Command>,
) {
    let th = ui.theme().clone();
    let heading = ui.text_style(18.0, heading_weight(ctx), th.text);
    ui.label_styled(tr(ctx, "Keep awake"), &heading);
    ui.gap(ctx.px(8.0));
    let hint = ui.text_style(12.0, Weight::Regular, th.text_dim);
    ui.label_styled(
        tr(
            ctx,
            "Temporarily prevent automatic sleep. Your normal power settings return when this stops or NabyScape quits.",
        ),
        &hint,
    );
    ui.gap(ctx.px(14.0));
    let status = ui.text_style(14.0, Weight::Regular, th.text);
    for line in status_text(ctx, info).lines() {
        ui.label_styled(line, &status);
    }
    ui.gap(ctx.px(16.0));
    if info.active {
        let r = ui.primary_button(tr(ctx, "Stop keep awake"));
        mark("awake-stop", r.rect);
        if r.clicked {
            out.push(Command::AwakeStop);
        }
        return;
    }
    let labels: Vec<&str> = DURATIONS.iter().map(|(l, _)| tr(ctx, l)).collect();
    let r = ui.combo(duration, &labels);
    mark("awake-duration", r.rect);
    ui.gap(ctx.px(10.0));
    let r = ui.checkbox(display_on, tr(ctx, "Keep the display on too"));
    mark("awake-display", r.rect);
    ui.gap(ctx.px(12.0));
    let r = ui.primary_button(tr(ctx, "Start keep awake"));
    mark("awake-start", r.rect);
    if r.clicked {
        out.push(Command::AwakeStart {
            minutes: DURATIONS[(*duration).min(DURATIONS.len() - 1)].1,
            keep_display_on: *display_on,
        });
    }
    ui.gap(ctx.px(8.0));
    let note = ui.text_style(11.0, Weight::Regular, th.text_dim);
    ui.label_styled(
        tr(
            ctx,
            "Explicit sleep, locking, and closing the lid remain under Windows control.",
        ),
        &note,
    );
}

impl Page for AwakePage {
    fn id(&self) -> &'static str {
        "awake"
    }

    fn height(&mut self, ctx: &PageCtx, width: f32) -> f32 {
        let info = *ctx.snaps.awake;
        let key = key_of(&[
            base_key(ctx),
            info.active as u64,
            info.keep_display_on as u64,
            u64::from(info.minutes_left.unwrap_or(u32::MAX)),
        ]);
        let (mut d, mut on) = (self.duration, self.display_on);
        self.shell.measure(ctx, width, key, |ui| {
            layout(ui, ctx, &info, &mut d, &mut on, &mut Vec::new());
        })
    }

    fn frame(
        &mut self,
        canvas: &mut Canvas,
        ctx: &PageCtx,
        rect: Rect,
        input: &InputState,
        out: &mut Vec<Command>,
    ) -> bool {
        let info = *ctx.snaps.awake;
        let (d, on) = (&mut self.duration, &mut self.display_on);
        let mut commands = Vec::new();
        let again = self.shell.run(canvas, ctx, rect, input, out, |ui| {
            layout(ui, ctx, &info, d, on, &mut commands);
        });
        let acted = !commands.is_empty();
        out.append(&mut commands);
        self.refresh_at =
            (info.active && info.minutes_left.is_some()).then_some(ctx.snaps.now + REFRESH);
        again || acted
    }

    /// While a timed session runs the remaining minutes count down.
    fn next_frame(&self, now: f64) -> Option<f64> {
        match (self.shell.next_frame(), self.refresh_at) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
        .map(|t| t.max(now))
    }

    fn captures_pointer(&self) -> bool {
        self.shell.captures_pointer()
    }

    fn on_hide(&mut self) {
        self.shell.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pages::tools_common::test_support::Rig;

    #[test]
    fn start_sends_the_chosen_duration_and_display_flag() {
        let Some(mut rig) = Rig::new("awake") else {
            return;
        };
        rig.paint();
        rig.save("page-awake-off");
        rig.click("awake-display");
        rig.click("awake-start");
        assert_eq!(
            rig.commands(),
            vec![Command::AwakeStart {
                minutes: Some(30),
                keep_display_on: true
            }]
        );
    }

    #[test]
    fn picking_until_quit_sends_no_duration() {
        let Some(mut rig) = Rig::new("awake") else {
            return;
        };
        rig.paint();
        // Open the combo and choose the last entry with the keyboard.
        rig.click("awake-duration");
        for _ in 0..3 {
            rig.press_key(naby_render::Key::Down);
        }
        rig.press_key(naby_render::Key::Enter);
        rig.click("awake-start");
        let sent = rig.commands();
        assert!(
            sent.contains(&Command::AwakeStart {
                minutes: None,
                keep_display_on: false
            }),
            "{sent:?}"
        );
    }

    #[test]
    fn an_active_session_shows_remaining_time_and_stops() {
        let Some(mut rig) = Rig::new("awake") else {
            return;
        };
        rig.snaps.awake.replace(AwakeInfo {
            active: true,
            keep_display_on: true,
            minutes_left: Some(42),
        });
        rig.paint();
        assert!(!rig.has("awake-start") && rig.has("awake-stop"));
        rig.save("page-awake-on");
        rig.click("awake-stop");
        assert_eq!(rig.commands(), vec![Command::AwakeStop]);
    }

    #[test]
    fn status_texts_match_the_roadmap_overlay() {
        let Some(rig) = Rig::new("awake") else {
            return;
        };
        let ctx = rig.ctx();
        assert_eq!(
            status_text(&ctx, &AwakeInfo::default()),
            "Off \u{b7} Windows power settings apply"
        );
        assert_eq!(
            status_text(
                &ctx,
                &AwakeInfo {
                    active: true,
                    keep_display_on: false,
                    minutes_left: None
                }
            ),
            "Awake \u{b7} Until NabyScape quits\nDisplay can turn off"
        );
    }
}
