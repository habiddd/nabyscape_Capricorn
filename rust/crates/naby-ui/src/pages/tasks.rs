//! The `tasks` page (C# `VisualTasks.BuildVisualTasks`): a count, the task list with check
//! rings, hover-revealed remove buttons and the composer at the bottom.

use super::tools_common::{
    base_key, check_mark, fmt, ghost_icon, key_of, mark, split_row, strike, tr, Shell, BOX_BORDER,
    BOX_FILL,
};
use super::{Page, PageCtx};
use naby_core::command::Command;
use naby_core::notifications::NoticeKind;
use naby_render::{Canvas, Fill, Icon, InputState, Rect, TextEditOpts, TextStyle, Ui, Weight};

pub fn build() -> Box<dyn Page> {
    Box::new(TasksPage {
        shell: Shell::new(),
        draft: String::new(),
    })
}

/// What the frame decided; applied to the service after the `Ui` is done with it.
#[derive(Debug, PartialEq)]
pub(crate) enum TaskAction {
    Toggle(String, bool),
    Remove(String),
    Add,
}

#[derive(Clone)]
pub(crate) struct TaskRow {
    pub id: String,
    pub text: String,
    pub done: bool,
}

struct TasksPage {
    shell: Shell,
    draft: String,
}

fn rows_of(ctx: &PageCtx) -> Vec<TaskRow> {
    ctx.snaps
        .productivity
        .borrow()
        .tasks()
        .iter()
        .map(|t| TaskRow {
            id: t.id.clone(),
            text: t.text.clone(),
            done: t.done,
        })
        .collect()
}

/// Applies the frame's actions to the shared service; returns the commands to emit.
pub(crate) fn apply(
    ctx: &PageCtx,
    draft: &mut String,
    actions: Vec<TaskAction>,
    out: &mut Vec<Command>,
) {
    for action in actions {
        match action {
            TaskAction::Toggle(id, done) => {
                ctx.snaps.productivity.borrow_mut().set_task_done(&id, done);
            }
            TaskAction::Remove(id) => ctx.snaps.productivity.borrow_mut().remove_task(&id),
            TaskAction::Add => {
                if draft.trim().is_empty() {
                    continue;
                }
                if ctx.snaps.productivity.borrow_mut().add_task(draft) {
                    draft.clear();
                } else {
                    out.push(Command::Notify {
                        kind: NoticeKind::Failure,
                        title: "Task limit reached".to_owned(),
                        detail: "Remove a task before adding another. Your draft is still here."
                            .to_owned(),
                        files: None,
                    });
                }
            }
        }
    }
}

/// Maximum height of the list viewport: `Clamp((PanelMaxHeight - 180) / PanelScale, 72, 360)`.
fn list_max(ctx: &PageCtx) -> f32 {
    let s = ctx.settings();
    (((s.panel_max_height - 180.0) / s.panel_scale.max(0.1)).clamp(72.0, 360.0)) as f32 * ctx.scale
}

fn label_style(ui: &Ui, done: bool) -> TextStyle {
    let th = ui.theme();
    ui.text_style(
        12.0,
        Weight::Regular,
        if done { th.text_dim } else { th.text },
    )
}

fn row_height(ctx: &PageCtx, style: &TextStyle, text: &str, row_w: f32) -> (f32, usize) {
    let label_w = (row_w - ctx.px(66.0)).max(ctx.px(80.0));
    let lines = ctx.fonts.wrap(text, style, label_w).len().max(1);
    let lh = ctx.fonts.line_height(style);
    ((lines as f32 * lh + ctx.px(18.0)).max(ctx.px(46.0)), lines)
}

pub(crate) fn layout(
    ui: &mut Ui,
    ctx: &PageCtx,
    draft: &mut String,
    rows: &[TaskRow],
    actions: &mut Vec<TaskAction>,
) {
    let th = ui.theme().clone();
    let done = rows.iter().filter(|t| t.done).count();
    let open = rows.len() - done;
    // Heading with the open count on the right.
    let heading = ui.text_style(
        18.0,
        if th.bold_headings {
            Weight::Semibold
        } else {
            Weight::Regular
        },
        th.text,
    );
    let count_style = ui.text_style(11.0, Weight::Regular, th.text_dim);
    let count = if rows.is_empty() {
        String::new()
    } else {
        fmt(ctx, "{0} open", &[&open])
    };
    split_row(
        ui,
        ctx,
        Some((tr(ctx, "Tasks"), heading)),
        Some((count.as_str(), count_style)),
    );
    ui.gap(ctx.px(16.0));

    if rows.is_empty() {
        let st = ui.text_style(12.0, Weight::Regular, th.text_dim);
        let lh = ctx.fonts.line_height(&st);
        let r = ui.allocate(None, lh + ctx.px(16.0 + 18.0));
        ui.canvas().text(
            ctx.fonts,
            r.x + ctx.px(4.0),
            r.y + ctx.px(16.0),
            tr(ctx, "Add your first task below."),
            &st,
            Some(r.w),
        );
    } else {
        let width = ui.available_width();
        let heights: Vec<f32> = rows
            .iter()
            .map(|t| row_height(ctx, &label_style(ui, t.done), &t.text, width).0)
            .collect();
        let total: f32 = heights.iter().sum();
        let viewport = total.min(list_max(ctx));
        if total > viewport + 0.5 {
            ui.scroll_area_h("tasks-list", viewport, |ui| {
                for (t, h) in rows.iter().zip(&heights) {
                    task_row(ui, ctx, t, *h, actions);
                }
            });
        } else {
            for (t, h) in rows.iter().zip(&heights) {
                task_row(ui, ctx, t, *h, actions);
            }
        }
    }

    // Composer.
    ui.gap(ctx.px(14.0));
    let box_h = ctx.px(38.0);
    let boxr = ui.allocate(None, box_h);
    if ui.visible(boxr) {
        ui.canvas().fill_rrect(boxr, ctx.px(9.0), BOX_FILL);
        ui.canvas()
            .stroke_round_rect(boxr, ctx.px(9.0), BOX_BORDER, 1.0);
    }
    let btn = ctx.px(34.0);
    let add_rect = Rect::new(
        boxr.right() - btn - ctx.px(2.0),
        boxr.y + (boxr.h - btn) / 2.0,
        btn,
        btn,
    );
    let field = Rect::new(
        boxr.x + ctx.px(2.0),
        boxr.y,
        (boxr.w - btn - ctx.px(6.0)).max(0.0),
        boxr.h,
    );
    mark("task-draft", field);
    mark("task-add", add_rect);
    let hint = tr(ctx, "Add a task\u{2026}").to_owned();
    let mut submit = false;
    ui.child(field, |ui| {
        let r = ui.text_edit_with(
            draft,
            &TextEditOpts::single().hint(&hint).max_len(240).underline(),
        );
        submit |= r.submitted;
    });
    let can_add = !draft.trim().is_empty();
    let clicked = ui.enabled(can_add, |ui| {
        ghost_icon(
            ui,
            add_rect,
            Icon::Plus,
            tr(ctx, "Add task"),
            ctx.px(15.0),
            th.text_dim,
            None,
            ctx.px(8.0),
        )
        .clicked
    });
    if can_add && (clicked || submit) {
        actions.push(TaskAction::Add);
    }
}

fn task_row(ui: &mut Ui, ctx: &PageCtx, t: &TaskRow, height: f32, actions: &mut Vec<TaskAction>) {
    let th = ui.theme().clone();
    let r = ui.allocate(None, height);
    let remove_size = ctx.px(28.0);
    let remove_rect = Rect::new(
        r.right() - ctx.px(4.0) - remove_size,
        r.y + (r.h - remove_size) / 2.0,
        remove_size,
        remove_size,
    );
    let toggle_rect = Rect::new(r.x, r.y, (remove_rect.x - ctx.px(8.0) - r.x).max(0.0), r.h);
    mark(&format!("task-toggle-{}", t.text), toggle_rect);
    mark(&format!("task-remove-{}", t.text), remove_rect);
    ui.push_id(t.id.as_str(), |ui| {
        let hit = ui.hit_area(toggle_rect, true);
        let row_hover = hit.hovered || ui.pointer_over(r);
        if ui.visible(r) {
            if row_hover {
                ui.canvas()
                    .fill_rrect(r, ctx.px(8.0), naby_render::Rgba::WHITE.with_alpha(0x0B));
            }
            // Ring and tick.
            let ring = ctx.px(19.0);
            let rx = r.x + ctx.px(4.0);
            let ry = r.y + (r.h - ring) / 2.0;
            let (cx, cy) = (rx + ring / 2.0, ry + ring / 2.0);
            if t.done {
                ui.canvas()
                    .fill_circle(cx, cy, ring / 2.0, &Fill::Solid(th.accent));
                check_mark(ui.canvas(), rx, ry, ctx.scale, th.accent_text);
            } else {
                ui.canvas().stroke_circle(
                    cx,
                    cy,
                    ring / 2.0 - 0.6,
                    if hit.focused {
                        th.control_text
                    } else {
                        th.control_border
                    },
                    if hit.focused { 2.0 } else { 1.2 },
                );
            }
            // Label.
            let style = label_style(ui, t.done);
            let label_w = (r.w - ctx.px(66.0)).max(ctx.px(80.0));
            let lines = ctx.fonts.wrap(&t.text, &style, label_w);
            let lh = ctx.fonts.line_height(&style);
            let block = lines.len().max(1) as f32 * lh;
            let x = r.x + ctx.px(4.0 + 30.0);
            let mut y = r.y + (r.h - block) / 2.0;
            for line in &lines {
                ui.canvas().text(ctx.fonts, x, y, line, &style, None);
                if t.done {
                    let w = ctx.fonts.measure_width(line, &style);
                    strike(ui.canvas(), x, y, w, lh, style.color);
                }
                y += lh;
            }
        }
        if hit.clicked {
            actions.push(TaskAction::Toggle(t.id.clone(), !t.done));
        }
        let name = format!("{} {}", tr(ctx, "Remove task"), t.text);
        let remove = ui.reveal_on_hover(r, false, |ui| {
            ghost_icon(
                ui,
                remove_rect,
                Icon::X,
                &name,
                ctx.px(13.0),
                th.text_dim,
                None,
                ctx.px(8.0),
            )
        });
        if remove.clicked {
            actions.push(TaskAction::Remove(t.id.clone()));
        }
    });
}

impl Page for TasksPage {
    fn id(&self) -> &'static str {
        "tasks"
    }

    fn height(&mut self, ctx: &PageCtx, width: f32) -> f32 {
        let rows = rows_of(ctx);
        let key = key_of(&[base_key(ctx), self.draft.is_empty() as u64]);
        let draft = &mut self.draft;
        self.shell.measure(ctx, width, key, |ui| {
            layout(ui, ctx, draft, &rows, &mut Vec::new());
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
        let rows = rows_of(ctx);
        let mut actions = Vec::new();
        let draft = &mut self.draft;
        let again = self.shell.run(canvas, ctx, rect, input, out, |ui| {
            layout(ui, ctx, draft, &rows, &mut actions);
        });
        let acted = !actions.is_empty();
        apply(ctx, &mut self.draft, actions, out);
        again || acted
    }

    /// The hover face (`HoverTools.BuildToolPeek`): the heading, the first two tasks (only the
    /// first before the detailed stage) and an "All tasks" hint. Presses fall through to the
    /// peek, which opens the page.
    fn peek(
        &mut self,
        canvas: &mut Canvas,
        ctx: &PageCtx,
        rect: Rect,
        _input: &InputState,
        _out: &mut Vec<Command>,
        detailed: bool,
    ) -> Option<bool> {
        let th = ctx.theme;
        let weight = super::tools_common::heading_weight(ctx);
        let heading = ctx.style(13.0, weight, th.text);
        canvas.text(
            ctx.fonts,
            rect.x,
            rect.y,
            tr(ctx, "Tasks"),
            &heading,
            Some(rect.w),
        );
        let mut y = rect.y + ctx.fonts.line_height(&heading) + ctx.px(6.0);
        let rows = rows_of(ctx);
        if rows.is_empty() {
            let st = ctx.style(11.0, Weight::Regular, th.text_dim);
            canvas.text(
                ctx.fonts,
                rect.x,
                y,
                tr(ctx, "No tasks yet. Add one in Tools \u{2192} Tasks."),
                &st,
                Some(rect.w),
            );
            y += ctx.fonts.line_height(&st) + ctx.px(6.0);
        }
        for (i, t) in rows.iter().take(2).enumerate() {
            if i > 0 && !detailed {
                break;
            }
            let row_h = ctx.px(28.0);
            super::tools_common::check_ring(
                canvas,
                rect.x,
                y + (row_h - ctx.px(19.0)) / 2.0,
                t.done,
                th,
                ctx.scale,
            );
            let st = ctx.style(
                12.0,
                Weight::Regular,
                if t.done { th.text_dim } else { th.text },
            );
            let lh = ctx.fonts.line_height(&st);
            let x = rect.x + ctx.px(30.0);
            let w = (rect.right() - x).max(0.0);
            canvas.text(ctx.fonts, x, y + (row_h - lh) / 2.0, &t.text, &st, Some(w));
            if t.done {
                let shown = ctx.fonts.measure_width(&t.text, &st).min(w);
                strike(canvas, x, y + (row_h - lh) / 2.0, shown, lh, st.color);
            }
            y += row_h;
        }
        super::tools_common::peek_button(
            canvas,
            ctx,
            rect.x,
            y + ctx.px(5.0),
            tr(ctx, "All tasks"),
        );
        Some(false)
    }

    fn next_frame(&self, _now: f64) -> Option<f64> {
        self.shell.next_frame()
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
    use naby_render::Key;

    fn titles(rig: &Rig) -> Vec<(String, bool)> {
        rig.snaps
            .productivity
            .borrow()
            .tasks()
            .iter()
            .map(|t| (t.text.clone(), t.done))
            .collect()
    }

    #[test]
    fn empty_page_shows_the_prompt_and_snapshots() {
        let Some(mut rig) = Rig::new("tasks") else {
            return;
        };
        rig.paint();
        assert!(rig.height > 60.0);
        assert!(rig.has("task-draft") && rig.has("task-add"));
        rig.save("page-tasks-empty");
    }

    #[test]
    fn typing_and_pressing_enter_adds_a_task() {
        let Some(mut rig) = Rig::new("tasks") else {
            return;
        };
        rig.paint();
        rig.click("task-draft");
        rig.type_text("Buy milk");
        rig.press_key(Key::Enter);
        assert_eq!(titles(&rig), vec![("Buy milk".to_owned(), false)]);
        rig.paint();
        assert!(rig.has("task-toggle-Buy milk"));
        rig.save("page-tasks-one");
    }

    #[test]
    fn blank_drafts_are_ignored_and_the_add_button_adds() {
        let Some(mut rig) = Rig::new("tasks") else {
            return;
        };
        rig.paint();
        rig.click("task-draft");
        rig.type_text("   ");
        rig.press_key(Key::Enter);
        assert!(titles(&rig).is_empty());
        rig.type_text("Write tests");
        rig.click("task-add");
        assert_eq!(titles(&rig), vec![("Write tests".to_owned(), false)]);
    }

    #[test]
    fn checking_and_removing_update_the_service() {
        let Some(mut rig) = Rig::new("tasks") else {
            return;
        };
        {
            let mut p = rig.snaps.productivity.borrow_mut();
            p.add_task("One");
            p.add_task("Two");
        }
        rig.paint();
        rig.click("task-toggle-One");
        assert_eq!(
            titles(&rig),
            vec![("One".to_owned(), true), ("Two".to_owned(), false)]
        );
        rig.paint();
        rig.save("page-tasks-done");
        rig.click("task-toggle-One");
        assert!(!titles(&rig)[0].1, "toggling again unchecks");
        rig.hover("task-toggle-Two");
        rig.click("task-remove-Two");
        assert_eq!(titles(&rig), vec![("One".to_owned(), false)]);
    }

    #[test]
    fn a_full_list_keeps_the_draft_and_notifies() {
        let Some(mut rig) = Rig::new("tasks") else {
            return;
        };
        for i in 0..100 {
            rig.snaps
                .productivity
                .borrow_mut()
                .add_task(&format!("task {i}"));
        }
        rig.paint();
        rig.click("task-draft");
        rig.type_text("one more");
        rig.press_key(Key::Enter);
        let out = rig.commands();
        assert!(out.iter().any(|c| matches!(
            c,
            Command::Notify { title, .. } if title == "Task limit reached"
        )));
        assert_eq!(rig.snaps.productivity.borrow().tasks().len(), 100);
        rig.save("page-tasks-scroll");
    }

    #[test]
    fn the_hover_face_lists_two_tasks_when_detailed_and_draws_ink() {
        let Some(mut rig) = Rig::new("tasks") else {
            return;
        };
        assert_eq!(rig.peek(false), Some(false));
        let empty_ink = rig.ink(Rect::new(0.0, 0.0, 380.0, 160.0));
        rig.save("page-tasks-peek-empty");
        {
            let mut p = rig.snaps.productivity.borrow_mut();
            p.add_task("First task");
            p.add_task("Second task");
        }
        let id = rig.snaps.productivity.borrow().tasks()[0].id.clone();
        rig.snaps.productivity.borrow_mut().set_task_done(&id, true);
        rig.peek(false);
        let brief = rig.ink(Rect::new(0.0, 0.0, 380.0, 160.0));
        rig.save("page-tasks-peek-brief");
        rig.peek(true);
        let detailed = rig.ink(Rect::new(0.0, 0.0, 380.0, 160.0));
        rig.save("page-tasks-peek-detailed");
        assert!(brief > empty_ink / 2);
        assert!(
            detailed > brief,
            "the second row appears in the detailed stage"
        );
    }
}
