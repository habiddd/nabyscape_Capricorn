//! The `files` page (C# `VisualShelf.BuildFileShelf`): the file shelf as a grid of previews.
//! Drop files on the dock to add them (the host posts `ShelfAddFiles`), drag tiles out with
//! `Command::DragFilesOut`, double-click to open, remove or reveal.

use super::tools_common::{
    base_key, button_width, check_mark, fmt, ghost_icon, hash_str, key_of, mark, text_button, tr,
    Look, Shell,
};
use super::{Page, PageCtx};
use crate::host::IconProvider;
use naby_core::color::Rgba;
use naby_core::command::Command;
use naby_core::productivity::shelf::MAX_SHELF;
use naby_render::{Canvas, Fill, Icon, InputState, Rect, TextStyle, Ui, Weight};
use std::collections::HashSet;
use std::rc::Rc;

const MIN_TILE: f32 = 150.0;
const TILE_GAP: f32 = 18.0;
const PREVIEW_H: f32 = 142.0;
/// Pointer travel before a press on a tile becomes a drag (`SystemParameters.MinimumDragDistance`).
const DRAG_DISTANCE: f32 = 4.0;
/// Two clicks on a tile within this many seconds open the file.
const DOUBLE_CLICK: f64 = 0.5;

pub fn build() -> Box<dyn Page> {
    Box::new(FilesPage {
        shell: Shell::new(),
        list: Shell::new(),
        st: State::default(),
        icons: None,
    })
}

#[derive(Default)]
struct State {
    /// Selected shelf paths (case-insensitive on Windows, so compared lower-cased).
    selection: HashSet<String>,
    press: Option<(String, (f32, f32))>,
    last_click: Option<(String, f64)>,
    /// A drag started: the release that follows is not a click.
    dragged: bool,
}

struct FilesPage {
    shell: Shell,
    list: Shell,
    st: State,
    icons: Option<Rc<dyn IconProvider>>,
}

#[derive(Clone)]
struct FileRow {
    path: String,
    name: String,
    subtitle: String,
    exists: bool,
    is_dir: bool,
}

#[derive(Debug, PartialEq)]
enum Action {
    Remove(String),
}

fn key_path(path: &str) -> String {
    path.to_lowercase()
}

fn rows_of(ctx: &PageCtx) -> Vec<FileRow> {
    ctx.snaps
        .productivity
        .borrow()
        .shelf()
        .iter()
        .map(|f| FileRow {
            path: f.path.clone(),
            name: f.name(),
            subtitle: f.subtitle(),
            exists: f.exists(),
            is_dir: f.is_dir(),
        })
        .collect()
}

/// Viewport budget: `Clamp((PanelMaxHeight - 160) / PanelScale, 84, 380)`.
fn budget(ctx: &PageCtx) -> f32 {
    let s = ctx.settings();
    (((s.panel_max_height - 160.0) / s.panel_scale.max(0.1)).clamp(84.0, 380.0)) as f32 * ctx.scale
}

fn muted(ui: &Ui, size: f32) -> TextStyle {
    ui.text_style(size, Weight::Regular, ui.theme().text_dim)
}

#[allow(clippy::too_many_arguments)]
fn tile(
    ui: &mut Ui,
    ctx: &PageCtx,
    st: &mut State,
    icons: &dyn IconProvider,
    f: &FileRow,
    actions: &mut Vec<Action>,
    out: &mut Vec<Command>,
    shelf: &[FileRow],
) {
    let th = ui.theme().clone();
    let name_style = ui.text_style(
        12.0,
        Weight::Regular,
        if f.exists { th.text } else { th.text_dim },
    );
    let sub_style = muted(ui, 10.0);
    let preview_h = ctx.px(PREVIEW_H);
    let total = preview_h
        + ctx.px(10.0)
        + ctx.fonts.line_height(&name_style)
        + ctx.px(4.0)
        + ctx.fonts.line_height(&sub_style);
    let r = ui.allocate(None, total);
    let preview = Rect::new(r.x, r.y, r.w, preview_h);
    let selected = st.selection.contains(&key_path(&f.path));
    // Controls (registered after the tile so they win presses).
    let remove_size = ctx.px(28.0);
    let remove_rect = Rect::new(
        preview.right() - ctx.px(6.0) - remove_size,
        preview.y + ctx.px(6.0),
        remove_size,
        remove_size,
    );
    let reveal_rect = Rect::new(
        remove_rect.x - ctx.px(4.0) - remove_size,
        remove_rect.y,
        remove_size,
        remove_size,
    );
    let check_rect = Rect::new(
        preview.x + ctx.px(6.0),
        preview.y + ctx.px(6.0),
        ctx.px(30.0),
        ctx.px(28.0),
    );
    mark(&format!("file-tile-{}", f.name), r);
    mark(&format!("file-check-{}", f.name), check_rect);
    mark(&format!("file-remove-{}", f.name), remove_rect);
    mark(&format!("file-reveal-{}", f.name), reveal_rect);
    let hit = ui.hit_area(r, true);
    if ui.visible(r) {
        // Preview frame.
        ui.canvas().push_clip_round_rect(preview, ctx.px(9.0));
        let thumb = (f.exists && ui.near_viewport(r))
            .then(|| {
                let px = naby_render::image::thumbnail_pixels(r.w, 1.0);
                icons.file_thumbnail(&f.path, px)
            })
            .flatten();
        match thumb {
            Some(img) => {
                let dst = super::tools_common::fit_rect(img.width(), img.height(), preview);
                ui.canvas().draw_image(&img, dst, 1.0, 0.0);
            }
            None => {
                let s = ctx.px(34.0);
                let (cx, cy) = preview.center();
                ui.canvas().icon(
                    if f.is_dir {
                        Icon::Folder
                    } else {
                        Icon::FileText
                    },
                    Rect::new(cx - s / 2.0, cy - s / 2.0, s, s),
                    th.text_dim,
                    naby_render::icons::DEFAULT_STROKE,
                );
            }
        }
        ui.canvas().pop_clip();
        if hit.hovered {
            ui.canvas()
                .fill_rrect(preview, ctx.px(9.0), Rgba::WHITE.with_alpha(0x0A));
        }
        let ny = preview.bottom() + ctx.px(10.0);
        ui.canvas()
            .text(ctx.fonts, r.x, ny, &f.name, &name_style, Some(r.w));
        ui.canvas().text(
            ctx.fonts,
            r.x,
            ny + ctx.fonts.line_height(&name_style) + ctx.px(4.0),
            &f.subtitle,
            &sub_style,
            Some(r.w),
        );
        if hit.focused {
            ui.focus_outline(preview, ctx.px(9.0));
        }
    }
    let tip = format!(
        "{}\n{}",
        f.path,
        tr(ctx, "Drag out \u{b7} double-click to open")
    );
    ui.tooltip(&hit, &tip);

    // Press / drag / double click on the tile body.
    let pointer = ui.input().pointer;
    let over_control = pointer.is_some_and(|p| {
        check_rect.contains_pt(p) || remove_rect.contains_pt(p) || reveal_rect.contains_pt(p)
    });
    if ui.input().pressed && hit.hovered && !over_control {
        st.press = pointer.map(|p| (f.path.clone(), p));
        st.dragged = false;
    }
    if ui.input().down {
        if let (Some((path, start)), Some(p)) = (st.press.clone(), pointer) {
            let moved = (p.0 - start.0).abs().max((p.1 - start.1).abs());
            if path == f.path && moved >= DRAG_DISTANCE * ctx.scale.min(2.0) && f.exists {
                st.press = None;
                st.dragged = true;
                let paths: Vec<String> = if selected {
                    shelf
                        .iter()
                        .filter(|x| x.exists && st.selection.contains(&key_path(&x.path)))
                        .map(|x| x.path.clone())
                        .collect()
                } else {
                    vec![f.path.clone()]
                };
                if !paths.is_empty() {
                    out.push(Command::DragFilesOut(paths));
                }
            }
        }
    } else {
        st.press = None;
    }
    if hit.clicked && !over_control {
        if std::mem::take(&mut st.dragged) {
            // The release of a drag.
        } else {
            let now = ctx.snaps.now;
            let double = st
                .last_click
                .as_ref()
                .is_some_and(|(p, t)| *p == f.path && now - *t <= DOUBLE_CLICK);
            if double {
                st.last_click = None;
                out.push(Command::OpenPath(f.path.clone()));
            } else {
                st.last_click = Some((f.path.clone(), now));
            }
        }
    }

    // Selection ring.
    if ui.visible(r) {
        let ring = ctx.px(19.0);
        let (cx, cy) = (check_rect.x + ring / 2.0, check_rect.y + check_rect.h / 2.0);
        if selected {
            ui.canvas()
                .fill_circle(cx, cy, ring / 2.0, &Fill::Solid(th.accent));
            check_mark(
                ui.canvas(),
                cx - ring / 2.0,
                cy - ring / 2.0,
                ctx.scale,
                th.accent_text,
            );
        } else if hit.hovered {
            ui.canvas()
                .stroke_circle(cx, cy, ring / 2.0 - 0.6, th.control_border, 1.2);
        }
    }
    let check = ui.enabled(f.exists, |ui| ui.hit_area(check_rect, true));
    if check.clicked {
        let k = key_path(&f.path);
        if !st.selection.remove(&k) {
            st.selection.insert(k);
        }
    }
    // Hover-revealed buttons.
    let (name_remove, name_reveal) = (
        format!("{} {}", tr(ctx, "Unstage"), f.name),
        tr(ctx, "Show in folder").to_owned(),
    );
    let (rm, rv) = ui.reveal_on_hover(r, false, |ui| {
        let fill = Some(Rgba::rgb(0x1A, 0x1D, 0x22).with_alpha(0xE6));
        let rv = ghost_icon(
            ui,
            reveal_rect,
            Icon::Folder,
            &name_reveal,
            ctx.px(13.0),
            th.text_dim,
            fill,
            ctx.px(8.0),
        );
        let rm = ghost_icon(
            ui,
            remove_rect,
            Icon::X,
            &name_remove,
            ctx.px(13.0),
            th.text_dim,
            fill,
            ctx.px(8.0),
        );
        (rm, rv)
    });
    if rm.clicked {
        actions.push(Action::Remove(f.path.clone()));
    }
    if rv.clicked {
        out.push(Command::RevealInFolder(f.path.clone()));
    }
}

fn content(
    ui: &mut Ui,
    ctx: &PageCtx,
    st: &mut State,
    icons: &dyn IconProvider,
    rows: &[FileRow],
    actions: &mut Vec<Action>,
    out: &mut Vec<Command>,
) {
    let th = ui.theme().clone();
    if rows.is_empty() {
        ui.gap(ctx.px(18.0));
        let s = ctx.px(34.0);
        let r = ui.allocate(None, s);
        if ui.visible(r) {
            let old = ui.canvas().alpha();
            ui.canvas().set_alpha(old * 0.6);
            ui.canvas().icon(
                Icon::Folder,
                Rect::new(r.x + (r.w - s) / 2.0, r.y, s, s),
                th.text_dim,
                naby_render::icons::DEFAULT_STROKE,
            );
            ui.canvas().set_alpha(old);
        }
        ui.gap(ctx.px(12.0));
        let style = ui.text_style(12.0, Weight::Regular, th.text_dim);
        let text = tr(ctx, "Drop files here or add files above.");
        let w = ctx.fonts.measure_width(text, &style);
        let lr = ui.allocate(None, ctx.fonts.line_height(&style));
        if ui.visible(lr) {
            ui.canvas().text(
                ctx.fonts,
                lr.x + ((lr.w - w) / 2.0).max(0.0),
                lr.y,
                text,
                &style,
                Some(lr.w),
            );
        }
        ui.gap(ctx.px(22.0));
        return;
    }
    let gap = ctx.px(TILE_GAP);
    ui.tile_flow(
        ctx.px(MIN_TILE),
        gap,
        rows.len(),
        |_| false,
        |ui, i| tile(ui, ctx, st, icons, &rows[i], actions, out, rows),
    );
}

#[allow(clippy::too_many_arguments)]
fn layout(
    ui: &mut Ui,
    ctx: &PageCtx,
    st: &mut State,
    list: &mut Shell,
    icons: &dyn IconProvider,
    rows: &[FileRow],
    actions: &mut Vec<Action>,
    out: &mut Vec<Command>,
) {
    let th = ui.theme().clone();
    st.selection
        .retain(|k| rows.iter().any(|f| key_path(&f.path) == *k));
    let count = rows.len();
    let heading = ui.text_style(
        18.0,
        if th.bold_headings {
            Weight::Semibold
        } else {
            Weight::Regular
        },
        th.text,
    );
    let count_style = muted(ui, 11.0);
    let count_text = format!("{count} / {MAX_SHELF}");
    let mut menu = None;
    let mut add = false;
    ui.header(
        |ui| {
            ui.row_centered(ctx.fonts.line_height(&heading), |ui| {
                ui.label_styled(tr(ctx, "File shelf"), &heading);
                ui.gap(ctx.px(12.0));
                ui.label_styled(&count_text, &count_style);
            });
        },
        |ui| {
            ui.row(|ui| {
                ui.spacing(ctx.px(4.0));
                let r = text_button(
                    ui,
                    ctx,
                    tr(ctx, "Add files\u{2026}"),
                    13.0,
                    30.0,
                    10.0,
                    Look::Filled,
                    count < MAX_SHELF,
                );
                mark("files-add", r.rect);
                add = r.clicked && count < MAX_SHELF;
                menu = ui.more_menu(
                    tr(ctx, "File shelf actions"),
                    &[tr(ctx, "Select all"), tr(ctx, "Clear selection")],
                );
            });
        },
    );
    if add {
        out.push(Command::PickShelfFiles);
    }
    match menu {
        Some(0) => {
            for f in rows.iter().filter(|f| f.exists) {
                st.selection.insert(key_path(&f.path));
            }
        }
        Some(1) => st.selection.clear(),
        _ => {}
    }
    // Gallery.
    let width = ui.available_width();
    let max = budget(ctx);
    let key = key_of(&[
        base_key(ctx),
        hash_str(
            &rows
                .iter()
                .map(|f| f.path.as_str())
                .collect::<Vec<_>>()
                .join("|"),
        ),
        rows.iter().filter(|f| f.exists).count() as u64,
    ]);
    let natural = list.measure(ctx, width, key, |ui| {
        let mut st = State::default();
        content(
            ui,
            ctx,
            &mut st,
            &super::super::host::NoIcons,
            rows,
            &mut Vec::new(),
            &mut Vec::new(),
        );
    });
    if natural <= max + 0.5 {
        content(ui, ctx, st, icons, rows, actions, out);
    } else {
        ui.scroll_area_h("files-gallery", max, |ui| {
            content(ui, ctx, st, icons, rows, actions, out);
        });
    }
    // Footer.
    ui.gap(ctx.px(18.0));
    let hint = muted(ui, 10.0);
    let selected_text = if st.selection.is_empty() {
        String::new()
    } else {
        fmt(ctx, "{0} selected", &[&st.selection.len()])
    };
    super::tools_common::split_row(
        ui,
        ctx,
        Some((tr(ctx, "\u{2197}  Drop files or drag out"), hint)),
        Some((selected_text.as_str(), hint)),
    );
    let _ = button_width;
}

impl Page for FilesPage {
    fn id(&self) -> &'static str {
        "files"
    }

    fn height(&mut self, ctx: &PageCtx, width: f32) -> f32 {
        let rows = rows_of(ctx);
        let key = key_of(&[
            base_key(ctx),
            self.st.selection.len() as u64,
            hash_str(
                &rows
                    .iter()
                    .map(|f| f.path.as_str())
                    .collect::<Vec<_>>()
                    .join("|"),
            ),
        ]);
        let (st, list) = (&mut self.st, &mut self.list);
        let saved = (st.press.clone(), st.last_click.clone(), st.dragged);
        let h = self.shell.measure(ctx, width, key, |ui| {
            layout(
                ui,
                ctx,
                st,
                list,
                &super::super::host::NoIcons,
                &rows,
                &mut Vec::new(),
                &mut Vec::new(),
            );
        });
        (self.st.press, self.st.last_click, self.st.dragged) = saved;
        h
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
        self.icons = Some(ctx.snaps.icons.clone());
        let icons = ctx.snaps.icons.clone();
        let mut actions = Vec::new();
        let mut commands = Vec::new();
        let (st, list) = (&mut self.st, &mut self.list);
        let again = self.shell.run(canvas, ctx, rect, input, out, |ui| {
            layout(
                ui,
                ctx,
                st,
                list,
                icons.as_ref(),
                &rows,
                &mut actions,
                &mut commands,
            );
        });
        let acted = !actions.is_empty() || !commands.is_empty();
        for action in actions {
            match action {
                Action::Remove(path) => ctx.snaps.productivity.borrow_mut().remove_file(&path),
            }
        }
        out.append(&mut commands);
        again || acted
    }

    fn next_frame(&self, now: f64) -> Option<f64> {
        self.shell.next_frame().map(|t| t.max(now))
    }

    fn captures_pointer(&self) -> bool {
        self.shell.captures_pointer()
    }

    fn on_hide(&mut self) {
        self.shell.reset();
        self.list.reset();
        self.st = State::default();
        if let Some(icons) = self.icons.take() {
            icons.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pages::tools_common::test_support::{FakeIcons, Rig};
    use naby_render::InputState;

    fn with_files(rig: &mut Rig, dir: &std::path::Path, names: &[&str]) -> Vec<String> {
        let mut paths = Vec::new();
        for n in names {
            let p = dir.join(n);
            std::fs::write(&p, b"data").unwrap();
            paths.push(p.to_string_lossy().into_owned());
        }
        rig.snaps.productivity.borrow_mut().add_files(&paths);
        paths
    }

    #[test]
    fn the_empty_shelf_shows_the_drop_hint_and_add_button() {
        let Some(mut rig) = Rig::new("files") else {
            return;
        };
        rig.paint();
        assert!(rig.has("files-add"));
        rig.click("files-add");
        assert_eq!(rig.commands(), vec![Command::PickShelfFiles]);
        rig.save("page-files-empty");
    }

    #[test]
    fn tiles_show_thumbnails_and_open_on_double_click() {
        let Some(mut rig) = Rig::new("files") else {
            return;
        };
        rig.snaps.icons = Rc::new(FakeIcons::default());
        let dir = tempfile::tempdir().unwrap();
        let paths = with_files(&mut rig, dir.path(), &["a.png", "notes.txt", "gone.bin"]);
        std::fs::remove_file(&paths[2]).unwrap();
        rig.paint();
        rig.save("page-files-grid");
        rig.click("file-tile-a.png");
        assert!(
            rig.commands().is_empty(),
            "one click only selects the pointer target"
        );
        rig.snaps.now += 0.2;
        rig.click("file-tile-a.png");
        assert_eq!(rig.commands(), vec![Command::OpenPath(paths[0].clone())]);
        // A slow second click is not a double click.
        rig.snaps.now += 3.0;
        rig.click("file-tile-a.png");
        rig.snaps.now += 3.0;
        rig.click("file-tile-a.png");
        assert!(rig.commands().is_empty());
    }

    #[test]
    fn selection_drag_out_remove_and_reveal() {
        let Some(mut rig) = Rig::new("files") else {
            return;
        };
        rig.snaps.icons = Rc::new(FakeIcons::default());
        let dir = tempfile::tempdir().unwrap();
        let paths = with_files(&mut rig, dir.path(), &["a.txt", "b.txt"]);
        rig.paint();
        rig.click("file-check-a.txt");
        rig.click("file-check-b.txt");
        rig.paint();
        rig.save("page-files-selected");
        // Dragging a selected tile takes the whole selection.
        let (x, y) = rig.center_of("file-tile-a.txt");
        rig.frame(&InputState::at(x, y));
        rig.frame(&InputState::press(x, y));
        rig.frame(&InputState::drag(x + 12.0, y + 6.0));
        assert_eq!(
            rig.commands(),
            vec![Command::DragFilesOut(vec![
                paths[0].clone(),
                paths[1].clone()
            ])]
        );
        rig.frame(&InputState::release(x + 12.0, y + 6.0));
        // An unselected tile drags alone.
        rig.paint();
        rig.click("file-check-b.txt");
        let (x, y) = rig.center_of("file-tile-b.txt");
        rig.frame(&InputState::at(x, y));
        rig.frame(&InputState::press(x, y));
        rig.frame(&InputState::drag(x, y + 20.0));
        assert_eq!(
            rig.commands(),
            vec![Command::DragFilesOut(vec![paths[1].clone()])]
        );
        rig.frame(&InputState::release(x, y + 20.0));
        rig.paint();
        // Reveal and remove buttons appear on hover.
        rig.hover("file-tile-b.txt");
        rig.click("file-reveal-b.txt");
        assert_eq!(
            rig.commands(),
            vec![Command::RevealInFolder(paths[1].clone())]
        );
        rig.hover("file-tile-b.txt");
        rig.click("file-remove-b.txt");
        let left: Vec<String> = rig
            .snaps
            .productivity
            .borrow()
            .shelf()
            .iter()
            .map(|f| f.name())
            .collect();
        assert_eq!(left, ["a.txt"]);
    }

    #[test]
    fn hiding_the_panel_clears_the_preview_cache() {
        let Some(mut rig) = Rig::new("files") else {
            return;
        };
        let icons = Rc::new(FakeIcons::default());
        rig.snaps.icons = icons.clone();
        let dir = tempfile::tempdir().unwrap();
        with_files(&mut rig, dir.path(), &["a.txt"]);
        rig.paint();
        assert!(icons.requests.get() > 0);
        rig.page.on_hide();
        assert_eq!(icons.cleared.get(), 1);
    }

    #[test]
    fn a_full_shelf_disables_adding() {
        let Some(mut rig) = Rig::new("files") else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let names: Vec<String> = (0..40).map(|i| format!("f{i}.txt")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        with_files(&mut rig, dir.path(), &refs);
        rig.paint();
        rig.click("files-add");
        assert!(rig.commands().is_empty());
        rig.save("page-files-full");
    }
}
