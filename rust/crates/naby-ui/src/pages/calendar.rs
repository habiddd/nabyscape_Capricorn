//! The `calendar` page (C# `RoadmapOverlay.BuildCalendar`, `VisualCalendar`): a week strip or a
//! month grid, the agenda of the selected day with meeting links, and the local event editor.
//! Events live in the shared `CalendarHandle`, which validates and saves before it applies.

use super::tools_common::{
    base_key, button_in, button_width, ghost_icon, hash_str, key_of, mark, split_row, stroke_poly,
    text_button, tr, Look, Shell,
};
use super::{Page, PageCtx};
use chrono::{
    DateTime, Datelike, Duration, FixedOffset, Local, LocalResult, NaiveDate, NaiveDateTime,
    NaiveTime, TimeZone, Weekday,
};
use naby_core::color::Rgba;
use naby_core::command::Command;
use naby_core::notifications::NoticeKind;
use naby_core::productivity::calendar::{shift_month, week_of, CalendarEvent};
use naby_render::{Canvas, Fill, Icon, InputState, Rect, TextEditOpts, TextStyle, Ui, Weight};

pub fn build() -> Box<dyn Page> {
    Box::new(CalendarPage {
        shell: Shell::new(),
        st: State {
            ready: false,
            month: NaiveDate::MIN,
            day: NaiveDate::MIN,
            expanded: false,
            editing: false,
            form: Form::default(),
        },
    })
}

#[derive(Default, Clone)]
struct Form {
    title: String,
    start_date: String,
    start_time: String,
    end_date: String,
    end_time: String,
    url: String,
    status: String,
}

struct State {
    ready: bool,
    /// First day of the month shown in the grid.
    month: NaiveDate,
    /// The selected day.
    day: NaiveDate,
    /// The month grid is open instead of the week strip.
    expanded: bool,
    editing: bool,
    form: Form,
}

struct CalendarPage {
    shell: Shell,
    st: State,
}

/// What a frame asks the shared calendar to do.
enum Action {
    Remove(String),
    Save,
}

fn first_of(day: NaiveDate) -> NaiveDate {
    day.with_day(1).unwrap_or(day)
}

/// `EventTime`: a `yyyy-MM-dd` date and an `HH:mm` time in the local time zone.
pub fn event_time(date: &str, time: &str) -> Result<DateTime<FixedOffset>, String> {
    let bad = || "Use YYYY-MM-DD dates and HH:mm times.".to_owned();
    let shape = |s: &str, pattern: &str| {
        s.len() == pattern.len()
            && s.chars().zip(pattern.chars()).all(|(c, p)| match p {
                'd' => c.is_ascii_digit(),
                other => c == other,
            })
    };
    if !shape(date, "dddd-dd-dd") || !shape(time, "dd:dd") {
        return Err(bad());
    }
    let day = NaiveDate::parse_from_str(date, "%Y-%m-%d").map_err(|_| bad())?;
    let clock = NaiveTime::parse_from_str(time, "%H:%M").map_err(|_| bad())?;
    match Local.from_local_datetime(&NaiveDateTime::new(day, clock)) {
        LocalResult::Single(t) => Ok(t.fixed_offset()),
        _ => Err(
            "This time is skipped or repeated by daylight saving. Choose a different time."
                .to_owned(),
        ),
    }
}

/// Events shown for a day (`DayEvents`): they start on or before it and end after its start.
pub fn day_events(events: &[CalendarEvent], day: NaiveDate) -> Vec<CalendarEvent> {
    let midnight = day.and_hms_opt(0, 0, 0).unwrap_or_default();
    let mut found: Vec<CalendarEvent> = events
        .iter()
        .filter(|e| {
            e.start.with_timezone(&Local).date_naive() <= day
                && e.end.with_timezone(&Local).naive_local() > midnight
        })
        .cloned()
        .collect();
    found.sort_by_key(|e| e.start);
    found
}

fn muted(ui: &Ui, size: f32) -> TextStyle {
    ui.text_style(size, Weight::Regular, ui.theme().text_dim)
}

/// Draws a chevron pointing left or right (`Chevron` rotated by -90 / 90 degrees).
fn arrow(canvas: &mut Canvas, rect: Rect, forward: bool, color: Rgba, scale: f32) {
    let (cx, cy) = rect.center();
    let (dx, dy) = (2.5 * scale, 4.5 * scale);
    let d = if forward { -dx } else { dx };
    stroke_poly(
        canvas,
        &[(cx + d, cy - dy), (cx - d, cy), (cx + d, cy + dy)],
        1.6 * scale,
        color,
    );
}

fn toolbar(ui: &mut Ui, ctx: &PageCtx, st: &mut State, events_today: NaiveDate) {
    let th = ui.theme().clone();
    let label = st.month.format("%B %Y").to_string();
    let heading = ui.text_style(17.0, Weight::Regular, th.text);
    let lw = ctx.fonts.measure_width(&label, &heading);
    let (mut toggle, mut prev, mut next, mut today, mut add) = (false, false, false, false, false);
    let can_prev = shift_month(st.month.year(), st.month.month(), false).is_some();
    let can_next = shift_month(st.month.year(), st.month.month(), true).is_some();
    ui.header(
        |ui| {
            let h = ctx.px(30.0);
            let w = lw + ctx.px(8.0 + 8.0);
            let rect = ui.allocate(Some(w), h);
            mark("cal-month", rect);
            let r = ui.hit_area(rect, true);
            if ui.visible(rect) {
                ui.canvas().text(
                    ctx.fonts,
                    rect.x,
                    rect.y + (h - ctx.fonts.line_height(&heading)) / 2.0,
                    &label,
                    &heading,
                    None,
                );
                let s = ctx.px(8.0);
                ui.canvas().icon(
                    Icon::Chevron,
                    Rect::new(rect.x + lw + ctx.px(8.0), rect.y + (h - s) / 2.0, s, s),
                    th.text_dim,
                    naby_render::icons::DEFAULT_STROKE,
                );
                if r.focused {
                    ui.focus_outline(rect, th.radius);
                }
            }
            ui.tooltip(&r, tr(ctx, "Toggle calendar month"));
            toggle = r.clicked;
        },
        |ui| {
            ui.row(|ui| {
                ui.spacing(0.0);
                let size = (ctx.px(26.0), ctx.px(30.0));
                for (forward, enabled, flag) in
                    [(false, can_prev, &mut prev), (true, can_next, &mut next)]
                {
                    let rect = ui.allocate(Some(size.0 + ctx.px(4.0)), size.1);
                    let btn = Rect::new(rect.x, rect.y, size.0, size.1);
                    mark(if forward { "cal-next" } else { "cal-prev" }, btn);
                    let r = ui.enabled(enabled, |ui| ui.hit_area(btn, true));
                    if ui.visible(btn) {
                        if enabled && (r.hovered || r.pressed) {
                            ui.canvas().fill_rrect(btn, th.radius, th.hover_fill);
                        }
                        let alpha = ui.canvas().alpha();
                        if !enabled {
                            ui.canvas().set_alpha(alpha * 0.4);
                        }
                        arrow(ui.canvas(), btn, forward, th.text_dim, ctx.scale);
                        ui.canvas().set_alpha(alpha);
                    }
                    ui.tooltip(
                        &r,
                        tr(
                            ctx,
                            if forward {
                                "Next calendar month"
                            } else {
                                "Previous calendar month"
                            },
                        ),
                    );
                    *flag = enabled && r.clicked;
                }
                ui.gap(ctx.px(2.0));
                let r = text_button(
                    ui,
                    ctx,
                    tr(ctx, "Today"),
                    11.0,
                    30.0,
                    8.0,
                    Look::Ghost,
                    true,
                );
                mark("cal-today", r.rect);
                today = r.clicked;
                ui.gap(ctx.px(6.0));
                let r = text_button(
                    ui,
                    ctx,
                    tr(ctx, "Add event"),
                    11.0,
                    30.0,
                    10.0,
                    Look::Filled,
                    true,
                );
                mark("cal-add", r.rect);
                add = r.clicked;
            });
        },
    );
    if toggle {
        st.expanded = !st.expanded;
    }
    if prev || next {
        if let Some((y, m)) = shift_month(st.month.year(), st.month.month(), next) {
            if let Some(d) = NaiveDate::from_ymd_opt(y, m, 1) {
                st.month = d;
                st.expanded = true;
            }
        }
    }
    if today {
        st.day = events_today;
        st.month = first_of(events_today);
        st.expanded = false;
    }
    if add {
        st.editing = true;
        let d = st.day.format("%Y-%m-%d").to_string();
        st.form = Form {
            start_date: d.clone(),
            start_time: "09:00".into(),
            end_date: d,
            end_time: "10:00".into(),
            ..Form::default()
        };
    }
}

fn day_name(day: NaiveDate) -> &'static str {
    match day.weekday() {
        Weekday::Mon => "Mon",
        Weekday::Tue => "Tue",
        Weekday::Wed => "Wed",
        Weekday::Thu => "Thu",
        Weekday::Fri => "Fri",
        Weekday::Sat => "Sat",
        Weekday::Sun => "Sun",
    }
}

fn month_view(ui: &mut Ui, ctx: &PageCtx, st: &mut State, events: &[CalendarEvent]) {
    let th = ui.theme().clone();
    ui.gap(ctx.px(10.0));
    let names = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    let head_h = ctx.px(20.0);
    let row_h = ctx.px(24.0);
    let total = head_h + 6.0 * row_h;
    let rect = ui.allocate(None, total);
    let col_w = rect.w / 7.0;
    let small = ui.text_style(10.0, Weight::Regular, th.text_dim);
    for (i, n) in names.iter().enumerate() {
        let t = tr(ctx, n);
        let w = ctx.fonts.measure_width(t, &small);
        let lh = ctx.fonts.line_height(&small);
        ui.canvas().text(
            ctx.fonts,
            rect.x + i as f32 * col_w + (col_w - w) / 2.0,
            rect.y + (head_h - lh) / 2.0,
            t,
            &small,
            Some(col_w),
        );
    }
    // Days: six Monday-first weeks, leading and trailing days of the neighbours included.
    let offset = st.month.weekday().num_days_from_monday() as i64;
    for i in 0..42i64 {
        let Some(day) = st
            .month
            .checked_add_signed(Duration::days(i - offset))
            .filter(|d| d.year() >= 1)
        else {
            continue;
        };
        let (row, col) = ((i / 7) as f32, (i % 7) as f32);
        let cell = Rect::new(
            rect.x + col * col_w + ctx.px(1.0),
            rect.y + head_h + row * row_h + ctx.px(1.0),
            col_w - ctx.px(2.0),
            row_h - ctx.px(2.0),
        );
        let selected = day == st.day;
        let has_events = !day_events(events, day).is_empty();
        mark(&format!("cal-day-{}", day.format("%Y-%m-%d")), cell);
        let r = ui.hit_area(cell, true);
        if ui.visible(cell) {
            let bg = if selected {
                Some(Rgba::WHITE)
            } else if has_events {
                Some(Rgba::WHITE.with_alpha(0x2B))
            } else {
                None
            };
            if let Some(bg) = bg {
                ui.canvas().fill_rrect(cell, ctx.px(7.0), bg);
            }
            if r.hovered && !selected {
                ui.canvas().fill_rrect(cell, ctx.px(7.0), th.hover_fill);
            }
            let color = if selected {
                Rgba::BLACK
            } else if day.month() == st.month.month() {
                th.text
            } else {
                th.text_dim
            };
            let style = TextStyle { color, ..small };
            let t = day.day().to_string();
            let w = ctx.fonts.measure_width(&t, &style);
            let lh = ctx.fonts.line_height(&style);
            ui.canvas().text(
                ctx.fonts,
                cell.x + (cell.w - w) / 2.0,
                cell.y + (cell.h - lh) / 2.0,
                &t,
                &style,
                None,
            );
            if r.focused {
                ui.focus_outline(cell, ctx.px(7.0));
            }
        }
        ui.tooltip(&r, &day.format("%A, %B %-d, %Y").to_string());
        if r.clicked {
            st.day = day;
            st.month = first_of(day);
            st.expanded = false;
        }
    }
    ui.gap(ctx.px(10.0));
}

fn week_view(ui: &mut Ui, ctx: &PageCtx, st: &mut State, events: &[CalendarEvent]) {
    let th = ui.theme().clone();
    ui.gap(ctx.px(6.0));
    let name_style = ui.text_style(9.0, Weight::Regular, th.text_dim);
    let num_style = ui.text_style(15.0, Weight::Regular, th.text);
    let name_h = ctx.fonts.line_height(&name_style);
    let circle = ctx.px(30.0);
    let h = name_h + ctx.px(5.0) + circle + ctx.px(4.0) + ctx.px(3.0);
    let rect = ui.allocate(None, h);
    let col_w = rect.w / 7.0;
    for (i, day) in week_of(st.day).iter().enumerate() {
        let cell = Rect::new(rect.x + i as f32 * col_w, rect.y, col_w, h);
        mark(&format!("cal-day-{}", day.format("%Y-%m-%d")), cell);
        let r = ui.hit_area(cell, true);
        if ui.visible(cell) {
            let t = tr(ctx, day_name(*day));
            let w = ctx.fonts.measure_width(t, &name_style);
            ui.canvas().text(
                ctx.fonts,
                cell.x + (cell.w - w) / 2.0,
                cell.y,
                t,
                &name_style,
                None,
            );
            let (cx, cy) = (
                cell.x + cell.w / 2.0,
                cell.y + name_h + ctx.px(5.0) + circle / 2.0,
            );
            if *day == st.day {
                ui.canvas().fill_circle(
                    cx,
                    cy,
                    circle / 2.0,
                    &Fill::Solid(Rgba::rgb(0x34, 0x37, 0x45)),
                );
            } else if r.hovered {
                ui.canvas()
                    .fill_circle(cx, cy, circle / 2.0, &Fill::Solid(th.hover_fill));
            }
            let n = day.day().to_string();
            let nw = ctx.fonts.measure_width(&n, &num_style);
            ui.canvas().text(
                ctx.fonts,
                cx - nw / 2.0,
                cy - ctx.fonts.line_height(&num_style) / 2.0,
                &n,
                &num_style,
                None,
            );
            if !day_events(events, *day).is_empty() {
                ui.canvas().fill_circle(
                    cx,
                    cy + circle / 2.0 + ctx.px(4.0) + ctx.px(1.5),
                    ctx.px(1.5),
                    &Fill::Solid(Rgba::rgb(0x9A, 0xA7, 0xC2)),
                );
            }
            if r.focused {
                ui.focus_outline(cell, th.radius);
            }
        }
        ui.tooltip(&r, &day.format("%A, %B %-d, %Y").to_string());
        if r.clicked {
            st.day = *day;
            st.month = first_of(*day);
            st.expanded = false;
        }
    }
    ui.gap(ctx.px(18.0));
}

fn event_row(
    ui: &mut Ui,
    ctx: &PageCtx,
    item: &CalendarEvent,
    actions: &mut Vec<Action>,
    out: &mut Vec<Command>,
) {
    let th = ui.theme().clone();
    let title_style = ui.text_style(12.0, Weight::Regular, th.text);
    let small = muted(ui, 10.0);
    let start = item.start.with_timezone(&Local);
    let end = item.end.with_timezone(&Local);
    let range = if start.date_naive() == end.date_naive() {
        format!("{} \u{2013} {}", start.format("%H:%M"), end.format("%H:%M"))
    } else {
        format!(
            "{} \u{2013} {}",
            start.format("%b %-d, %H:%M"),
            end.format("%b %-d, %H:%M")
        )
    };
    let time_w = ctx.px(54.0);
    let rail_w = ctx.px(14.0);
    let menu_w = ctx.px(30.0);
    let width = ui.available_width();
    let label_w = (width - time_w - rail_w - menu_w).max(ctx.px(40.0));
    let title_lines = ctx.fonts.wrap(&item.title, &title_style, label_w);
    let lh = ctx.fonts.line_height(&title_style);
    let range_h = ctx.fonts.line_height(&small);
    let meeting_h = if item.meeting_url.is_empty() {
        0.0
    } else {
        ctx.px(26.0)
    };
    let labels_h = title_lines.len() as f32 * lh + ctx.px(5.0) + range_h + meeting_h;
    let h = labels_h.max(ctx.px(35.0 + 6.0)).max(menu_w);
    let r = ui.allocate(None, h);
    ui.gap(ctx.px(14.0));
    if ui.visible(r) {
        ui.canvas().text(
            ctx.fonts,
            r.x,
            r.y + ctx.px(4.0),
            &start.format("%H:%M").to_string(),
            &small,
            Some(time_w),
        );
        let rail = Rect::new(
            r.x + time_w,
            r.y + ctx.px(3.0),
            ctx.px(2.0),
            (r.h - ctx.px(6.0)).max(ctx.px(35.0)),
        );
        ui.canvas()
            .fill_rrect(rail, ctx.px(1.0), Rgba::rgb(0x99, 0xA6, 0xC3));
        let x = r.x + time_w + rail_w;
        let mut y = r.y;
        for line in &title_lines {
            ui.canvas().text(ctx.fonts, x, y, line, &title_style, None);
            y += lh;
        }
        ui.canvas()
            .text(ctx.fonts, x, y + ctx.px(5.0), &range, &small, Some(label_w));
    }
    if !item.meeting_url.is_empty() {
        let mw = button_width(ctx, ui, tr(ctx, "Open meeting"), 10.0, 4.0, false);
        let rect = Rect::new(r.x + time_w + rail_w, r.bottom() - meeting_h, mw, meeting_h);
        mark(&format!("cal-meeting-{}", item.title), rect);
        let b = button_in(
            ui,
            ctx,
            rect,
            tr(ctx, "Open meeting"),
            10.0,
            Look::Ghost,
            true,
        );
        if b.clicked {
            out.push(Command::OpenUrl(item.meeting_url.clone()));
        }
    }
    // Event actions menu, revealed on hover.
    let menu_rect = Rect::new(r.right() - menu_w, r.y, menu_w, menu_w);
    mark(&format!("cal-menu-{}", item.title), menu_rect);
    let delete = format!("{} {}", tr(ctx, "Delete event"), item.title);
    let name = format!("{} \u{b7} {}", tr(ctx, "Event actions"), item.title);
    let picked = ui.reveal_on_hover(r, false, |ui| {
        ui.child(menu_rect, |ui| ui.more_menu(&name, &[delete.as_str()]))
    });
    if picked == Some(0) {
        actions.push(Action::Remove(item.id.clone()));
    }
}

fn agenda(
    ui: &mut Ui,
    ctx: &PageCtx,
    st: &State,
    events: &[CalendarEvent],
    actions: &mut Vec<Action>,
    out: &mut Vec<Command>,
) {
    let found = day_events(events, st.day);
    if found.is_empty() {
        let style = muted(ui, 12.0);
        ui.label_styled(tr(ctx, "No events for this day."), &style);
        return;
    }
    for item in &found {
        ui.push_id(item.id.as_str(), |ui| {
            event_row(ui, ctx, item, actions, out)
        });
    }
}

fn field_label(ui: &mut Ui, text: &str) {
    let style = muted(ui, 11.0);
    ui.label_styled(text, &style);
}

fn editor(ui: &mut Ui, ctx: &PageCtx, st: &mut State, actions: &mut Vec<Action>) {
    let th = ui.theme().clone();
    let heading = ui.text_style(
        18.0,
        if th.bold_headings {
            Weight::Semibold
        } else {
            Weight::Regular
        },
        th.text,
    );
    ui.label_styled(tr(ctx, "New local event"), &heading);
    ui.gap(ctx.px(8.0));
    field_label(
        ui,
        tr(
            ctx,
            "Saved on this device. Times use your Windows time zone.",
        ),
    );
    ui.gap(ctx.px(12.0));
    field_label(ui, tr(ctx, "Title"));
    ui.gap(ctx.px(4.0));
    let r = ui.text_edit_with(&mut st.form.title, &TextEditOpts::single().max_len(120));
    mark("cal-title", r.rect);
    ui.gap(ctx.px(8.0));
    let date_row = |ui: &mut Ui, label: &str, date: &mut String, time: &mut String, key: &str| {
        field_label(ui, &format!("{} \u{b7} YYYY-MM-DD / HH:mm", tr(ctx, label)));
        ui.gap(ctx.px(4.0));
        let lh = ctx
            .fonts
            .line_height(&ui.text_style(13.0, Weight::Regular, Rgba::WHITE));
        let h = 34.0f32.max(lh + 18.0);
        let row = ui.allocate(None, h);
        let time_w = ctx.px(83.0);
        let left = Rect::new(row.x, row.y, (row.w - time_w - ctx.px(10.0)).max(40.0), h);
        let right = Rect::new(row.right() - time_w, row.y, time_w, h);
        ui.child(left, |ui| {
            let r = ui.text_edit_with(date, &TextEditOpts::single());
            mark(&format!("cal-{key}-date"), r.rect);
        });
        ui.child(right, |ui| {
            let r = ui.text_edit_with(time, &TextEditOpts::single());
            mark(&format!("cal-{key}-time"), r.rect);
        });
        ui.gap(ctx.px(8.0));
    };
    let f = &mut st.form;
    date_row(ui, "Start", &mut f.start_date, &mut f.start_time, "start");
    date_row(ui, "End", &mut f.end_date, &mut f.end_time, "end");
    field_label(ui, tr(ctx, "Meeting URL \u{b7} optional http(s)"));
    ui.gap(ctx.px(4.0));
    let r = ui.text_edit_with(&mut st.form.url, &TextEditOpts::single().max_len(2048));
    mark("cal-url", r.rect);
    ui.gap(ctx.px(6.0));
    let status = muted(ui, 11.0);
    if st.form.status.is_empty() {
        ui.allocate(None, ctx.fonts.line_height(&status));
    } else {
        ui.label_styled(&st.form.status, &status);
    }
    ui.gap(ctx.px(8.0));
    ui.row(|ui| {
        ui.spacing(ctx.px(8.0));
        let save = ui.primary_button(tr(ctx, "Save event"));
        mark("cal-save", save.rect);
        if save.clicked {
            actions.push(Action::Save);
        }
        let cancel = ui.button(tr(ctx, "Cancel"));
        mark("cal-cancel", cancel.rect);
        if cancel.clicked {
            st.editing = false;
        }
    });
}

fn layout(
    ui: &mut Ui,
    ctx: &PageCtx,
    st: &mut State,
    events: &[CalendarEvent],
    actions: &mut Vec<Action>,
    out: &mut Vec<Command>,
) {
    let today = ctx.snaps.wall.date_naive();
    if !st.ready {
        st.ready = true;
        st.day = today;
        st.month = first_of(today);
    }
    if st.editing {
        editor(ui, ctx, st, actions);
        return;
    }
    toolbar(ui, ctx, st, today);
    if st.expanded {
        month_view(ui, ctx, st, events);
    } else {
        week_view(ui, ctx, st, events);
    }
    agenda(ui, ctx, st, events, actions, out);
    let _ = (split_row, ghost_icon);
}

/// Applies the frame's actions to the calendar; commands report failures like `MutateUtility`.
fn apply(ctx: &PageCtx, st: &mut State, actions: Vec<Action>, out: &mut Vec<Command>) {
    for action in actions {
        match action {
            Action::Remove(id) => {
                if let Err(e) = ctx.snaps.calendar.remove(&id) {
                    out.push(Command::Notify {
                        kind: NoticeKind::Failure,
                        title: "Could not complete the action".to_owned(),
                        detail: e.to_string(),
                        files: None,
                    });
                }
            }
            Action::Save => {
                let f = st.form.clone();
                let result = event_time(&f.start_date, &f.start_time)
                    .and_then(|s| event_time(&f.end_date, &f.end_time).map(|e| (s, e)))
                    .and_then(|(s, e)| {
                        ctx.snaps
                            .calendar
                            .add(&f.title, s, e, &f.url)
                            .map(|_| s)
                            .map_err(|e| e.to_string())
                    });
                match result {
                    Ok(start) => {
                        st.editing = false;
                        st.day = start.with_timezone(&Local).date_naive();
                        st.month = first_of(st.day);
                    }
                    Err(message) => st.form.status = message,
                }
            }
        }
    }
}

impl Page for CalendarPage {
    fn id(&self) -> &'static str {
        "calendar"
    }

    fn height(&mut self, ctx: &PageCtx, width: f32) -> f32 {
        let events = ctx.snaps.calendar.events();
        let key = key_of(&[
            base_key(ctx),
            self.st.expanded as u64,
            self.st.editing as u64,
            self.st.day.num_days_from_ce() as u64,
            self.st.month.num_days_from_ce() as u64,
            hash_str(&self.st.form.status),
            hash_str(&ctx.snaps.wall.date_naive().to_string()),
        ]);
        let st = &mut self.st;
        // Without input a measure pass changes nothing but the first-time initialization.
        self.shell.measure(ctx, width, key, |ui| {
            layout(ui, ctx, st, &events, &mut Vec::new(), &mut Vec::new());
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
        let events = ctx.snaps.calendar.events();
        let mut actions = Vec::new();
        let mut commands = Vec::new();
        let st = &mut self.st;
        let again = self.shell.run(canvas, ctx, rect, input, out, |ui| {
            layout(ui, ctx, st, &events, &mut actions, &mut commands);
        });
        let acted = !actions.is_empty() || !commands.is_empty();
        out.append(&mut commands);
        apply(ctx, &mut self.st, actions, out);
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
        self.st.editing = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pages::tools_common::test_support::Rig;
    use naby_render::Key;

    fn at(date: &str, time: &str) -> DateTime<FixedOffset> {
        event_time(date, time).unwrap()
    }

    fn rig_on(date: NaiveDate) -> Option<Rig> {
        let mut rig = Rig::new("calendar")?;
        rig.snaps.wall = Local
            .from_local_datetime(&date.and_hms_opt(12, 0, 0).unwrap())
            .unwrap();
        Some(rig)
    }

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn event_times_are_strict() {
        assert!(event_time("2026-10-29", "09:30").is_ok());
        for (date, time) in [
            ("2026-1-29", "09:30"),
            ("2026-10-29", "9:30"),
            ("29/10/2026", "09:30"),
            ("2026-13-01", "09:30"),
            ("2026-10-29", "25:00"),
            ("2026-10-29", ""),
        ] {
            assert_eq!(
                event_time(date, time).unwrap_err(),
                "Use YYYY-MM-DD dates and HH:mm times.",
                "{date} {time}"
            );
        }
    }

    #[test]
    fn day_events_include_multi_day_events_and_sort_by_start() {
        let events = vec![
            CalendarEvent {
                id: "b".into(),
                title: "Late".into(),
                start: at("2026-10-29", "15:00"),
                end: at("2026-10-29", "16:00"),
                meeting_url: String::new(),
            },
            CalendarEvent {
                id: "a".into(),
                title: "Trip".into(),
                start: at("2026-10-28", "09:00"),
                end: at("2026-10-30", "09:00"),
                meeting_url: String::new(),
            },
        ];
        let titles = |day| -> Vec<String> {
            day_events(&events, day)
                .into_iter()
                .map(|e| e.title)
                .collect()
        };
        assert_eq!(titles(d(2026, 10, 29)), ["Trip", "Late"]);
        assert_eq!(titles(d(2026, 10, 30)), ["Trip"]);
        assert!(titles(d(2026, 10, 31)).is_empty());
        assert!(titles(d(2026, 10, 27)).is_empty());
    }

    #[test]
    fn the_week_strip_selects_days_and_the_month_grid_navigates() {
        let Some(mut rig) = rig_on(d(2026, 10, 29)) else {
            return;
        };
        rig.paint();
        rig.save("page-calendar-week");
        assert!(rig.has("cal-day-2026-10-26") && rig.has("cal-day-2026-11-01"));
        rig.click("cal-day-2026-10-27");
        rig.paint();
        assert!(rig.has("cal-day-2026-10-27"));
        // Open the month grid and go forward a month.
        rig.click("cal-month");
        rig.paint();
        rig.save("page-calendar-month");
        assert!(rig.has("cal-day-2026-10-01"));
        rig.click("cal-next");
        rig.paint();
        assert!(rig.has("cal-day-2026-11-15"), "November is shown");
        rig.click("cal-day-2026-11-15");
        rig.paint();
        assert!(!rig.has("cal-day-2026-11-30"), "back on the week strip");
        rig.click("cal-today");
        rig.paint();
        assert!(rig.has("cal-day-2026-10-29"));
    }

    #[test]
    fn adding_an_event_validates_saves_and_shows_it() {
        let Some(mut rig) = rig_on(d(2026, 10, 29)) else {
            return;
        };
        rig.paint();
        rig.click("cal-add");
        rig.paint();
        rig.save("page-calendar-editor");
        // An empty title is rejected with the calendar's message.
        rig.click("cal-save");
        rig.paint();
        assert!(rig.snaps.calendar.events().is_empty());
        rig.save("page-calendar-error");
        rig.click("cal-title");
        rig.type_text("Standup");
        rig.click("cal-url");
        rig.type_text("https://meet.example.com/room");
        rig.click("cal-save");
        let events = rig.snaps.calendar.events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].title, "Standup");
        assert_eq!(events[0].meeting_url, "https://meet.example.com/room");
        rig.paint();
        assert!(rig.has("cal-meeting-Standup"));
        rig.save("page-calendar-event");
        rig.click("cal-meeting-Standup");
        assert_eq!(
            rig.commands(),
            vec![Command::OpenUrl("https://meet.example.com/room".into())]
        );
    }

    fn replace_field(rig: &mut Rig, name: &str, text: &str) {
        rig.click(name);
        rig.frame(&InputState::key(naby_render::KeyEvent {
            ctrl: true,
            ..naby_render::KeyEvent::new(Key::Char('a'))
        }));
        rig.type_text(text);
    }

    #[test]
    fn bad_times_and_ranges_report_inline() {
        let Some(mut rig) = rig_on(d(2026, 10, 29)) else {
            return;
        };
        rig.paint();
        rig.click("cal-add");
        rig.paint();
        rig.click("cal-title");
        rig.type_text("Bad");
        // End before start.
        replace_field(&mut rig, "cal-end-time", "08:00");
        rig.click("cal-save");
        rig.paint();
        assert!(rig.snaps.calendar.events().is_empty());
        assert!(rig.has("cal-save"), "the editor stays open");
        // Wrong time shape.
        replace_field(&mut rig, "cal-end-time", "9am");
        rig.click("cal-save");
        rig.paint();
        assert!(rig.snaps.calendar.events().is_empty());
        // A good range saves.
        replace_field(&mut rig, "cal-end-time", "11:30");
        rig.click("cal-save");
        assert_eq!(rig.snaps.calendar.events().len(), 1);
    }

    #[test]
    fn deleting_an_event_removes_it_from_the_calendar() {
        let Some(mut rig) = rig_on(d(2026, 10, 29)) else {
            return;
        };
        rig.snaps
            .calendar
            .add(
                "Dentist",
                at("2026-10-29", "10:00"),
                at("2026-10-29", "11:00"),
                "",
            )
            .unwrap();
        rig.paint();
        rig.hover("cal-menu-Dentist");
        rig.click("cal-menu-Dentist");
        rig.press_key(Key::Enter);
        assert!(rig.snaps.calendar.events().is_empty());
        rig.paint();
        rig.save("page-calendar-empty-day");
    }

    #[test]
    fn cancel_leaves_the_editor_without_saving() {
        let Some(mut rig) = rig_on(d(2026, 10, 29)) else {
            return;
        };
        rig.paint();
        rig.click("cal-add");
        rig.paint();
        rig.click("cal-cancel");
        rig.paint();
        assert!(rig.has("cal-add") && rig.snaps.calendar.events().is_empty());
    }
}
