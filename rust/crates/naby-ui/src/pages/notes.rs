//! The `notes` page (C# `UtilityOverlay.BuildNotes`): one multiline editor bound to
//! `ProductivityService::set_note`; the disk write is debounced by 350 ms like `noteSave` and
//! flushed when the panel hides or the page is dropped.

use super::tools_common::{base_key, key_of, mark, tr, Shell};
use super::{Page, PageCtx};
use crate::host::ProductivityHandle;
use naby_core::command::Command;
use naby_render::{Canvas, InputState, Rect, TextEditOpts, Ui, Weight};

/// Delay between the last keystroke and the save (`noteSave.Interval`).
pub const SAVE_DELAY: f64 = 0.35;

pub fn build() -> Box<dyn Page> {
    Box::new(NotesPage {
        shell: Shell::new(),
        text: String::new(),
        loaded: false,
        save_at: None,
        handle: None,
    })
}

struct NotesPage {
    shell: Shell,
    text: String,
    /// The editor text was taken from the service at least once.
    loaded: bool,
    /// Host time at which the pending edit is written to disk.
    save_at: Option<f64>,
    handle: Option<ProductivityHandle>,
}

impl NotesPage {
    /// Writes the note to disk now (`FlushNotes`).
    fn flush(&mut self) {
        if self.save_at.take().is_none() {
            return;
        }
        if let Some(handle) = &self.handle {
            if let Ok(mut service) = handle.try_borrow_mut() {
                service.save();
            }
        }
    }
}

impl Drop for NotesPage {
    fn drop(&mut self) {
        self.flush();
    }
}

fn layout(ui: &mut Ui, ctx: &PageCtx, text: &mut String) -> bool {
    let th = ui.theme().clone();
    let heading = ui.text_style(19.0, Weight::Semibold, th.text);
    let hr = ui.allocate(None, ctx.fonts.line_height(&heading));
    ui.canvas().text(
        ctx.fonts,
        hr.x,
        hr.y,
        tr(ctx, "Notes"),
        &heading,
        Some(hr.w),
    );
    ui.gap(ctx.px(12.0));
    // 136 DIP editor: as many rows as fit (rows are the field's own line height).
    let body = ui.text_style(13.0, Weight::Regular, th.control_text);
    let lh = ctx.fonts.line_height(&body).max(1.0);
    let rows = (((ctx.px(136.0) - 18.0) / lh).round() as usize).max(3);
    let hint = tr(ctx, "Capture a thought\u{2026}").to_owned();
    let before = text.len();
    let r = ui.text_edit_with(
        text,
        &TextEditOpts::multiline(rows).hint(&hint).max_len(50_000),
    );
    mark("note-editor", r.rect);
    let changed = r.changed || text.len() != before;
    ui.gap(ctx.px(9.0));
    let meta = ui.text_style(10.0, Weight::Regular, th.text_dim);
    let row = ui.allocate(None, ctx.fonts.line_height(&meta));
    ui.canvas().text(
        ctx.fonts,
        row.x + ctx.px(2.0),
        row.y,
        tr(ctx, "Saved on this device"),
        &meta,
        Some(row.w),
    );
    ui.gap(ctx.px(2.0));
    changed
}

impl Page for NotesPage {
    fn id(&self) -> &'static str {
        "notes"
    }

    fn height(&mut self, ctx: &PageCtx, width: f32) -> f32 {
        let key = key_of(&[base_key(ctx)]);
        let mut scratch = String::new();
        self.shell.measure(ctx, width, key, |ui| {
            layout(ui, ctx, &mut scratch);
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
        self.handle = Some(ctx.snaps.productivity.clone());
        // Follow the service when the note changed elsewhere (a detached editor, an import)
        // unless this editor is being typed in.
        let stored = ctx.snaps.productivity.borrow().note().to_owned();
        if (!self.loaded || stored != self.text) && !self.shell.wants_text_input() {
            self.text = stored;
            self.loaded = true;
        }
        let text = &mut self.text;
        let mut changed = false;
        let again = self.shell.run(canvas, ctx, rect, input, out, |ui| {
            changed = layout(ui, ctx, text);
        });
        if changed {
            ctx.snaps.productivity.borrow_mut().set_note(&self.text);
            self.save_at = Some(ctx.snaps.now + SAVE_DELAY);
        }
        if self.save_at.is_some_and(|t| ctx.snaps.now >= t) {
            self.flush();
        }
        again || changed
    }

    fn next_frame(&self, now: f64) -> Option<f64> {
        let blink = self.shell.next_frame();
        match (blink, self.save_at) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
        .map(|t| t.max(now))
    }

    fn captures_pointer(&self) -> bool {
        self.shell.captures_pointer()
    }

    fn on_hide(&mut self) {
        self.flush();
        self.shell.reset();
        self.loaded = false;
    }
}

#[cfg(test)]
mod tests {
    use crate::pages::tools_common::test_support::Rig;
    use naby_render::Key;

    #[test]
    fn typing_updates_the_service_and_saves_after_the_debounce() {
        let Some(mut rig) = Rig::new("notes") else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let saved = std::rc::Rc::new(std::cell::RefCell::new(Vec::<String>::new()));
        {
            let sink = saved.clone();
            let mut p = rig.snaps.productivity.borrow_mut();
            *p = naby_core::productivity::ProductivityService::new(dir.path(), Default::default());
            p.set_saver(Box::new(move |d| {
                sink.borrow_mut().push(d.note.clone());
                Ok(())
            }));
        }
        rig.paint();
        rig.click("note-editor");
        rig.type_text("hello");
        rig.press_key(Key::Enter);
        rig.type_text("world");
        assert_eq!(rig.snaps.productivity.borrow().note(), "hello\nworld");
        assert!(saved.borrow().is_empty(), "not saved yet");
        assert!(rig.page.next_frame(rig.snaps.now).is_some());
        rig.snaps.now += 0.4;
        rig.paint();
        assert_eq!(saved.borrow().as_slice(), ["hello\nworld"]);
        rig.save("page-notes-text");
    }

    #[test]
    fn hiding_the_panel_flushes_a_pending_save() {
        let Some(mut rig) = Rig::new("notes") else {
            return;
        };
        let saves = std::rc::Rc::new(std::cell::Cell::new(0));
        {
            let counter = saves.clone();
            rig.snaps
                .productivity
                .borrow_mut()
                .set_saver(Box::new(move |_| {
                    counter.set(counter.get() + 1);
                    Ok(())
                }));
        }
        rig.paint();
        rig.click("note-editor");
        rig.type_text("draft");
        assert_eq!(saves.get(), 0);
        rig.page.on_hide();
        assert_eq!(saves.get(), 1);
        assert!(rig.page.next_frame(0.0).is_none());
    }

    #[test]
    fn dropping_the_page_flushes_too() {
        let Some(mut rig) = Rig::new("notes") else {
            return;
        };
        let saves = std::rc::Rc::new(std::cell::Cell::new(0));
        let counter = saves.clone();
        rig.snaps
            .productivity
            .borrow_mut()
            .set_saver(Box::new(move |_| {
                counter.set(counter.get() + 1);
                Ok(())
            }));
        rig.paint();
        rig.click("note-editor");
        rig.type_text("x");
        // The overlay drops its pages on any settings change, without `on_hide`.
        rig.page = crate::pages::build("notes");
        assert_eq!(saves.get(), 1);
    }

    #[test]
    fn an_external_change_shows_up_when_not_editing() {
        let Some(mut rig) = Rig::new("notes") else {
            return;
        };
        rig.paint();
        rig.snaps
            .productivity
            .borrow_mut()
            .set_note("from elsewhere");
        rig.paint();
        rig.save("page-notes-external");
        rig.click("note-editor");
        rig.press_key(Key::End);
        rig.type_text("!");
        assert_eq!(rig.snaps.productivity.borrow().note(), "from elsewhere!");
    }

    #[test]
    fn the_empty_editor_shows_the_hint() {
        let Some(mut rig) = Rig::new("notes") else {
            return;
        };
        rig.paint();
        let r = rig.rect_of("note-editor").unwrap();
        assert!(rig.ink(r.inset(2.0)) > 20, "the hint is drawn");
        rig.save("page-notes-empty");
    }
}
