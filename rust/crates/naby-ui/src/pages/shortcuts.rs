//! The `shortcuts` page (C# `LauncherPresentation.LaunchButton`, the "Quick launch" page, and
//! `AppPicker`): the saved shortcuts as icon and label buttons in the configured launch style,
//! plus an in-page installed-app picker whose catalog the host builds only while it is open.

use super::tools_common::{
    base_key, button_in, button_width, hash_str, key_of, mark, tr, Look, Shell,
};
use super::{Page, PageCtx};
use crate::host::{AppCatalog, AppEntry};
use naby_core::command::Command;
use naby_core::settings::Shortcut;
use naby_render::{Canvas, Icon, InputState, Key, Rect, TextEditOpts, TextStyle, Ui, Weight};
use std::sync::Arc;

/// `LauncherPresentation.ShowIcon` / `ShowLabel` for `Settings.LaunchStyle`.
pub fn show_icon(style: &str) -> bool {
    style != "Text"
}

pub fn show_label(style: &str) -> bool {
    style != "Icons"
}

/// `AppPicker.Search`: apps whose name contains the query (culture-insensitive, case-folded).
pub fn search<'a>(apps: &'a [AppEntry], query: &str) -> Vec<&'a AppEntry> {
    let q = query.trim().to_lowercase();
    apps.iter()
        .filter(|a| a.name.to_lowercase().contains(&q))
        .collect()
}

pub fn build() -> Box<dyn Page> {
    Box::new(ShortcutsPage {
        shell: Shell::new(),
        picker: Picker::default(),
        icons: None,
    })
}

#[derive(Default)]
struct Picker {
    open: bool,
    query: String,
    selected: usize,
    /// The query the selection was last reset for.
    reset_for: Option<String>,
    /// Row and time of the last click (a double click accepts the app).
    last_click: Option<(usize, f64)>,
}

struct ShortcutsPage {
    shell: Shell,
    picker: Picker,
    icons: Option<std::rc::Rc<dyn crate::host::IconProvider>>,
}

const BUTTON_H: f32 = 44.0;
const BUTTON_PAD: f32 = 9.0;
const BUTTON_GAP: f32 = 10.0;
const ICON: f32 = 24.0;
const LABEL_MAX: f32 = 115.0;

/// Pixel size to request for an icon of `dip` design units.
fn icon_px(ctx: &PageCtx, dip: f32) -> u32 {
    (((dip * ctx.scale) / 8.0).ceil() * 8.0).clamp(16.0, 256.0) as u32
}

struct Placed {
    index: usize,
    rect: Rect,
    label_w: f32,
    show_label: bool,
    icon: Option<Arc<naby_render::Image>>,
}

fn launch_buttons(ui: &mut Ui, ctx: &PageCtx, out: &mut Vec<Command>, shortcuts: &[Shortcut]) {
    let th = ui.theme().clone();
    let s = ctx.settings();
    let style = s.launch_style.as_str();
    let label_style = ui.text_style(11.0, Weight::Regular, th.control_text);
    let lh = ctx.fonts.line_height(&label_style);
    let width = ui.available_width();
    let pad = ctx.px(BUTTON_PAD);
    let gap = ctx.px(BUTTON_GAP);
    let h = ctx.px(BUTTON_H).max(lh + 2.0 * pad);
    // Icons first: the label of an icons-only button shows while no icon exists.
    let mut placed: Vec<Placed> = Vec::new();
    let (mut x, mut y) = (0.0f32, 0.0f32);
    for (i, sc) in shortcuts.iter().enumerate() {
        let icon = show_icon(style)
            .then(|| ctx.snaps.icons.shortcut_icon(sc, icon_px(ctx, ICON)))
            .flatten();
        let label = show_label(style) || (show_icon(style) && icon.is_none());
        let label_w = if label {
            ctx.fonts
                .measure_width(&sc.name, &label_style)
                .min(ctx.px(LABEL_MAX))
        } else {
            0.0
        };
        let icon_w = if show_icon(style) && icon.is_some() {
            ctx.px(ICON) + if label { ctx.px(7.0) } else { 0.0 }
        } else {
            0.0
        };
        let w = 2.0 * pad + icon_w + label_w;
        if x > 0.0 && x + w > width {
            x = 0.0;
            y += h + gap;
        }
        placed.push(Placed {
            index: i,
            rect: Rect::new(x, y, w, h),
            label_w,
            show_label: label,
            icon,
        });
        x += w + gap;
    }
    let total = placed.last().map_or(0.0, |p| p.rect.bottom() + gap);
    let area = ui.allocate(None, total);
    for p in placed {
        let sc = &shortcuts[p.index];
        let rect = p.rect.translate(area.x, area.y);
        mark(&format!("shortcut-{}", sc.name), rect);
        let r = ui.hit_area(rect, true);
        if ui.visible(rect) {
            if r.hovered || r.pressed {
                ui.canvas().fill_rrect(rect, th.radius, th.hover_fill);
            }
            let mut cx = rect.x + pad;
            if let Some(img) = &p.icon {
                let side = ctx.px(ICON);
                ui.canvas().draw_image(
                    img,
                    Rect::new(cx, rect.y + (rect.h - side) / 2.0, side, side),
                    1.0,
                    0.0,
                );
                cx += side + if p.show_label { ctx.px(7.0) } else { 0.0 };
            }
            if p.show_label {
                ui.canvas().text(
                    ctx.fonts,
                    cx,
                    rect.y + (rect.h - lh) / 2.0,
                    &sc.name,
                    &label_style,
                    Some(p.label_w + 1.0),
                );
            }
            if r.focused {
                ui.focus_outline(rect, th.radius);
            }
        }
        ui.tooltip(&r, &format!("{}\n{}", sc.name, sc.target));
        if r.clicked {
            out.push(Command::LaunchShortcut {
                target: sc.target.clone(),
                app_id: sc.app_id.clone(),
            });
            out.push(Command::CollapseDock);
        }
    }
}

fn shortcuts_view(ui: &mut Ui, ctx: &PageCtx, picker: &mut Picker, out: &mut Vec<Command>) {
    let th = ui.theme().clone();
    ui.gap(ctx.px(2.0));
    let shortcuts = ctx.snaps.settings.shortcuts.clone();
    if shortcuts.is_empty() {
        let st = ui.text_style(12.0, Weight::Regular, th.text_dim);
        ui.label_styled(tr(ctx, "Add shortcuts in Settings."), &st);
        ui.gap(ctx.px(8.0));
    } else {
        launch_buttons(ui, ctx, out, &shortcuts);
    }
    ui.gap(ctx.px(4.0));
    let w = button_width(
        ctx,
        ui,
        tr(ctx, "Add installed app\u{2026}"),
        11.0,
        10.0,
        false,
    );
    let rect = ui.allocate(Some(w), ctx.px(30.0));
    mark("shortcuts-add", rect);
    let r = button_in(
        ui,
        ctx,
        rect,
        tr(ctx, "Add installed app\u{2026}"),
        11.0,
        Look::Ghost,
        true,
    );
    if r.clicked {
        *picker = Picker {
            open: true,
            ..Picker::default()
        };
        out.push(Command::OpenAppPicker);
    }
}

fn state_text(ctx: &PageCtx, catalog: &AppCatalog, shown: usize) -> String {
    match catalog {
        AppCatalog::Closed | AppCatalog::Loading => {
            tr(ctx, "Loading installed apps\u{2026}").to_owned()
        }
        AppCatalog::Failed(_) => tr(
            ctx,
            "Installed apps could not be loaded. Close and try again, or add a shortcut manually.",
        )
        .to_owned(),
        AppCatalog::Ready(apps) if shown == 0 => if apps.is_empty() {
            tr(
                ctx,
                "No installed apps were found. You can add a shortcut manually in Settings.",
            )
        } else {
            tr(ctx, "No installed apps match your search.")
        }
        .to_owned(),
        AppCatalog::Ready(_) => super::tools_common::fmt(ctx, "{0} applications", &[&shown]),
    }
}

fn picker_view(
    ui: &mut Ui,
    ctx: &PageCtx,
    picker: &mut Picker,
    out: &mut Vec<Command>,
    keys: &[Key],
) {
    let th = ui.theme().clone();
    let heading = ui.text_style(20.0, Weight::Regular, th.text);
    ui.label_styled(tr(ctx, "Installed apps"), &heading);
    ui.gap(ctx.px(14.0));
    let hint = tr(ctx, "Search installed apps").to_owned();
    let r = ui.text_edit_with(&mut picker.query, &TextEditOpts::single().hint(&hint));
    mark("picker-search", r.rect);
    ui.tooltip(&r, tr(ctx, "Search by application name"));
    ui.gap(ctx.px(12.0));
    let catalog = (**ctx.snaps.catalog.arc()).clone();
    let apps: Vec<AppEntry> = match &catalog {
        AppCatalog::Ready(apps) => search(apps, &picker.query).into_iter().cloned().collect(),
        _ => Vec::new(),
    };
    // The first match is selected again whenever the filter changed.
    if picker.reset_for.as_deref() != Some(picker.query.as_str()) {
        picker.reset_for = Some(picker.query.clone());
        picker.selected = 0;
    }
    picker.selected = picker.selected.min(apps.len().saturating_sub(1));
    for key in keys {
        match key {
            Key::Down => picker.selected = (picker.selected + 1).min(apps.len().saturating_sub(1)),
            Key::Up => picker.selected = picker.selected.saturating_sub(1),
            _ => {}
        }
    }
    let state = ui.text_style(12.0, Weight::Regular, th.text_dim);
    ui.label_styled(&state_text(ctx, &catalog, apps.len()), &state);
    ui.gap(ctx.px(8.0));
    // The list.
    let row_h = ctx.px(ICON + 14.0);
    let view_h = (apps.len() as f32 * row_h).min(ctx.px(280.0)).max(0.0);
    let mut accept: Option<AppEntry> = None;
    if !apps.is_empty() {
        let name_style = ui.text_style(13.0, Weight::Regular, th.text);
        let nlh = ctx.fonts.line_height(&name_style);
        let body = |ui: &mut Ui, accept: &mut Option<AppEntry>, picker: &mut Picker| {
            for (i, app) in apps.iter().enumerate() {
                let rect = ui.allocate(None, row_h);
                if !ui.near_viewport(rect) {
                    continue;
                }
                ui.push_id(i, |ui| {
                    mark(&format!("picker-app-{}", app.name), rect);
                    let hit = ui.hit_area(rect, false);
                    if ui.visible(rect) {
                        if picker.selected == i {
                            ui.canvas().fill_rrect(rect, ctx.px(6.0), th.selection_fill);
                        } else if hit.hovered {
                            ui.canvas().fill_rrect(rect, ctx.px(6.0), th.hover_fill);
                        }
                        let side = ctx.px(ICON);
                        let sc = app.to_shortcut();
                        if let Some(img) = ctx.snaps.icons.shortcut_icon(&sc, icon_px(ctx, ICON)) {
                            ui.canvas().draw_image(
                                &img,
                                Rect::new(
                                    rect.x + ctx.px(4.0),
                                    rect.y + (rect.h - side) / 2.0,
                                    side,
                                    side,
                                ),
                                1.0,
                                0.0,
                            );
                        }
                        let color = if picker.selected == i {
                            th.selection_text
                        } else {
                            th.text
                        };
                        ui.canvas().text(
                            ctx.fonts,
                            rect.x + ctx.px(4.0 + ICON + 12.0),
                            rect.y + (rect.h - nlh) / 2.0,
                            &app.name,
                            &TextStyle {
                                color,
                                ..name_style
                            },
                            Some(rect.w - ctx.px(ICON + 24.0)),
                        );
                    }
                    if hit.clicked {
                        let now = ctx.snaps.now;
                        let double = picker
                            .last_click
                            .is_some_and(|(j, t)| j == i && now - t < 0.5);
                        picker.last_click = Some((i, now));
                        picker.selected = i;
                        if double {
                            *accept = Some(app.clone());
                        }
                    }
                });
            }
        };
        if apps.len() as f32 * row_h > view_h + 0.5 {
            ui.scroll_area_h("picker-apps", view_h, |ui| body(ui, &mut accept, picker));
        } else {
            body(ui, &mut accept, picker);
        }
    }
    ui.gap(ctx.px(12.0));
    let selected = apps.get(picker.selected).cloned();
    if keys.contains(&Key::Enter) && !r.submitted {
        accept = accept.or_else(|| selected.clone());
    }
    if r.submitted {
        accept = accept.or_else(|| selected.clone());
    }
    // Buttons, right aligned.
    let cancel_w = button_width(ctx, ui, tr(ctx, "Cancel"), 13.0, 14.0, false);
    let add_w = button_width(ctx, ui, tr(ctx, "Add selected app"), 13.0, 14.0, true);
    let h = ctx.px(34.0);
    let row = ui.allocate(None, h);
    let add_rect = Rect::new(row.right() - add_w, row.y, add_w, h);
    let cancel_rect = Rect::new(add_rect.x - ctx.px(8.0) - cancel_w, row.y, cancel_w, h);
    mark("picker-cancel", cancel_rect);
    mark("picker-add", add_rect);
    let c = button_in(
        ui,
        ctx,
        cancel_rect,
        tr(ctx, "Cancel"),
        13.0,
        Look::Filled,
        true,
    );
    let a = button_in(
        ui,
        ctx,
        add_rect,
        tr(ctx, "Add selected app"),
        13.0,
        Look::Primary,
        selected.is_some(),
    );
    if a.clicked && selected.is_some() {
        accept = accept.or(selected);
    }
    if let Some(app) = accept {
        let mut s = (**ctx.snaps.settings.arc()).clone();
        let dup = s
            .shortcuts
            .iter()
            .any(|x| x.target == app.target && x.app_id == app.app_id);
        if !dup {
            s.shortcuts.push(app.to_shortcut());
            out.push(Command::ApplySettings(Box::new(s)));
        }
        *picker = Picker::default();
        out.push(Command::CloseAppPicker);
    } else if c.clicked {
        *picker = Picker::default();
        out.push(Command::CloseAppPicker);
    }
}

fn layout(ui: &mut Ui, ctx: &PageCtx, picker: &mut Picker, out: &mut Vec<Command>, keys: &[Key]) {
    if picker.open {
        picker_view(ui, ctx, picker, out, keys);
    } else {
        shortcuts_view(ui, ctx, picker, out);
    }
}

impl Page for ShortcutsPage {
    fn id(&self) -> &'static str {
        "shortcuts"
    }

    fn height(&mut self, ctx: &PageCtx, width: f32) -> f32 {
        let s = ctx.settings();
        let key = key_of(&[
            base_key(ctx),
            self.picker.open as u64,
            hash_str(&self.picker.query),
            hash_str(&format!("{:?}", s.shortcuts)),
            hash_str(&s.launch_style),
        ]);
        let mut picker = Picker {
            open: self.picker.open,
            query: self.picker.query.clone(),
            selected: self.picker.selected,
            reset_for: self.picker.reset_for.clone(),
            last_click: None,
        };
        self.shell.measure(ctx, width, key, |ui| {
            layout(ui, ctx, &mut picker, &mut Vec::new(), &[]);
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
        self.icons = Some(ctx.snaps.icons.clone());
        let keys: Vec<Key> = input.keys.iter().map(|k| k.key).collect();
        let mut commands = Vec::new();
        let was_open = self.picker.open;
        let picker = &mut self.picker;
        let again = self.shell.run(canvas, ctx, rect, input, out, |ui| {
            layout(ui, ctx, picker, &mut commands, &keys);
        });
        let acted = !commands.is_empty() || was_open != self.picker.open;
        out.append(&mut commands);
        again || acted
    }

    fn demand(&self) -> crate::host::Demand {
        crate::host::Demand {
            catalog: self.picker.open,
            ..Default::default()
        }
    }

    fn next_frame(&self, now: f64) -> Option<f64> {
        self.shell.next_frame().map(|t| t.max(now))
    }

    fn captures_pointer(&self) -> bool {
        self.shell.captures_pointer()
    }

    fn on_hide(&mut self) {
        self.shell.reset();
        self.picker = Picker::default();
        if let Some(icons) = self.icons.take() {
            icons.clear();
        }
    }
}

#[allow(dead_code)]
fn unused(_: Icon) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pages::tools_common::test_support::{FakeIcons, Rig};
    use std::rc::Rc;

    fn app(name: &str) -> AppEntry {
        AppEntry {
            name: name.into(),
            target: format!("C:\\Apps\\{name}.lnk"),
            app_id: None,
        }
    }

    fn with_shortcuts(rig: &mut Rig) {
        let mut s = (**rig.snaps.settings.arc()).clone();
        s.shortcuts = vec![
            Shortcut {
                name: "Notepad".into(),
                target: "notepad.exe".into(),
                app_id: None,
            },
            Shortcut {
                name: "Calculator".into(),
                target: "shell:AppsFolder\\calc".into(),
                app_id: Some("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App".into()),
            },
            Shortcut {
                name: "Settings".into(),
                target: "ms-settings:".into(),
                app_id: None,
            },
        ];
        rig.snaps.settings.replace(s);
    }

    #[test]
    fn styles_follow_launcher_presentation() {
        assert!(show_icon("IconsLabels") && show_label("IconsLabels"));
        assert!(show_icon("Icons") && !show_label("Icons"));
        assert!(!show_icon("Text") && show_label("Text"));
    }

    #[test]
    fn picker_search_matches_names_case_insensitively() {
        let apps = vec![app("Calculator"), app("Notepad"), app("Paint")];
        let names =
            |q: &str| -> Vec<String> { search(&apps, q).iter().map(|a| a.name.clone()).collect() };
        assert_eq!(names("pad"), ["Notepad"]);
        assert_eq!(names("  CALC "), ["Calculator"]);
        assert_eq!(names("").len(), 3);
        assert!(names("zzz").is_empty());
    }

    #[test]
    fn an_empty_list_points_to_settings() {
        let Some(mut rig) = Rig::new("shortcuts") else {
            return;
        };
        rig.paint();
        assert!(rig.has("shortcuts-add"));
        rig.save("page-shortcuts-empty");
    }

    #[test]
    fn clicking_a_shortcut_launches_it_and_collapses_the_dock() {
        let Some(mut rig) = Rig::new("shortcuts") else {
            return;
        };
        with_shortcuts(&mut rig);
        rig.snaps.icons = Rc::new(FakeIcons::default());
        rig.paint();
        rig.save("page-shortcuts-icons-labels");
        rig.click("shortcut-Calculator");
        assert_eq!(
            rig.commands(),
            vec![
                Command::LaunchShortcut {
                    target: "shell:AppsFolder\\calc".into(),
                    app_id: Some("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App".into()),
                },
                Command::CollapseDock
            ]
        );
    }

    #[test]
    fn launch_styles_change_what_is_drawn() {
        let Some(mut rig) = Rig::new("shortcuts") else {
            return;
        };
        with_shortcuts(&mut rig);
        rig.snaps.icons = Rc::new(FakeIcons::default());
        let widths: Vec<f32> = ["IconsLabels", "Icons", "Text"]
            .iter()
            .map(|style| {
                let mut s = (**rig.snaps.settings.arc()).clone();
                s.launch_style = (*style).to_owned();
                rig.snaps.settings.replace(s);
                rig.paint();
                rig.save(&format!("page-shortcuts-{}", style.to_lowercase()));
                rig.rect_of("shortcut-Notepad").unwrap().w
            })
            .collect();
        assert!(widths[1] < widths[0], "icons only is narrower: {widths:?}");
        assert!(widths[2] < widths[0], "text only is narrower: {widths:?}");
    }

    #[test]
    fn without_icons_the_icons_style_falls_back_to_the_label() {
        let Some(mut rig) = Rig::new("shortcuts") else {
            return;
        };
        with_shortcuts(&mut rig);
        let mut s = (**rig.snaps.settings.arc()).clone();
        s.launch_style = "Icons".into();
        rig.snaps.settings.replace(s);
        rig.paint();
        assert!(rig.rect_of("shortcut-Notepad").unwrap().w > 40.0);
    }

    #[test]
    fn the_picker_opens_filters_and_adds_a_shortcut() {
        let Some(mut rig) = Rig::new("shortcuts") else {
            return;
        };
        rig.snaps.icons = Rc::new(FakeIcons::default());
        rig.paint();
        assert!(!rig.page.demand().catalog);
        rig.click("shortcuts-add");
        assert_eq!(rig.commands(), vec![Command::OpenAppPicker]);
        assert!(rig.page.demand().catalog, "the host builds the catalog now");
        rig.snaps.catalog.replace(AppCatalog::Loading);
        rig.paint();
        rig.save("page-shortcuts-picker-loading");
        rig.snaps.catalog.replace(AppCatalog::Ready(Arc::new(vec![
            app("Calculator"),
            app("Notepad"),
            app("Paint"),
        ])));
        rig.paint();
        rig.save("page-shortcuts-picker");
        rig.click("picker-search");
        rig.type_text("pa");
        rig.paint();
        assert!(rig.has("picker-app-Paint") && rig.has("picker-app-Notepad"));
        assert!(!rig.has("picker-app-Calculator"));
        rig.click("picker-app-Paint");
        rig.click("picker-add");
        let out = rig.commands();
        assert_eq!(out.len(), 2, "{out:?}");
        let Command::ApplySettings(settings) = &out[0] else {
            panic!("{out:?}");
        };
        assert_eq!(settings.shortcuts.last().unwrap().name, "Paint");
        assert_eq!(out[1], Command::CloseAppPicker);
        assert!(!rig.page.demand().catalog);
    }

    #[test]
    fn the_picker_reports_failures_and_cancel_closes_it() {
        let Some(mut rig) = Rig::new("shortcuts") else {
            return;
        };
        rig.paint();
        rig.click("shortcuts-add");
        rig.commands();
        rig.snaps.catalog.replace(AppCatalog::Failed("boom".into()));
        rig.paint();
        rig.save("page-shortcuts-picker-failed");
        rig.click("picker-cancel");
        assert_eq!(rig.commands(), vec![Command::CloseAppPicker]);
        assert!(rig.has("shortcuts-add"));
    }

    #[test]
    fn hiding_the_panel_closes_the_picker_and_clears_icons() {
        let Some(mut rig) = Rig::new("shortcuts") else {
            return;
        };
        let icons = Rc::new(FakeIcons::default());
        rig.snaps.icons = icons.clone();
        rig.paint();
        rig.click("shortcuts-add");
        rig.page.on_hide();
        assert!(!rig.page.demand().catalog);
        assert_eq!(icons.cleared.get(), 1);
    }
}
