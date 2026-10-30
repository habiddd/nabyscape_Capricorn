//! The hover peek carousel (C# `PeekCarousel.cs`, `ExtraHoverViews.cs`, the peek half of
//! `CompactOverlay.cs`): page deck, dots, arrows, drag/wheel navigation and the built-in faces
//! of every hover page. A page may replace its face through `Page::peek`.

use super::util::{fade, style, text_centered, Hot};
use super::Overlay;
use crate::host::Snapshots;
use crate::pages::{self, PageCtx};
use naby_core::color::Rgba;
use naby_core::command::Command;
use naby_core::motion::{PeekLayout, PeekPolicy};
use naby_core::productivity::{active_countdown, ActiveCountdown};
use naby_core::session::format_seconds;
use naby_render::icons::DEFAULT_STROKE;
use naby_render::{Canvas, Fill, Fonts, Icon, InputState, Rect, Weight};

const PRIMARY: Rgba = Rgba::WHITE;
const BAR_TRACK: Rgba = Rgba::rgb(0x34, 0x36, 0x3D);

/// Peek content geometry in DIP relative to the (target-size) surface.
struct Geo {
    /// Safe content column width and top.
    w: f64,
    top: f64,
    viewport_h: f64,
    footer: bool,
}

impl Overlay {
    fn peek_geo(&self, snaps: &Snapshots) -> Geo {
        let area = self.area_dip(snaps);
        let size = naby_core::motion::IslandGeometry::hover_size(
            &snaps.settings,
            area,
            if self.peek.detailed {
                naby_core::motion::HoverStage::Detailed
            } else {
                naby_core::motion::HoverStage::Brief
            },
        );
        let safe = PeekLayout::safe_bounds(size.w, size.h, self.cfg.radius, self.cfg.notch);
        Geo {
            w: safe.w,
            top: safe.y,
            viewport_h: if self.peek.detailed {
                124.0
            } else {
                64.0f64.min(safe.h)
            },
            footer: self.peek.detailed,
        }
    }

    /// Carousel offset now, DIP.
    pub(crate) fn peek_offset_value(&self) -> f64 {
        self.peek.offset.value(self.now)
    }

    fn peek_width(&self, snaps: &Snapshots) -> f64 {
        self.peek_geo(snaps).w
    }

    pub(crate) fn current_peek_ids(&self, snaps: &Snapshots) -> Vec<String> {
        if self.peek.ids.is_empty() {
            self.peek_pages(snaps)
        } else {
            self.peek.ids.clone()
        }
    }

    // ---- navigation -------------------------------------------------------------------

    pub(crate) fn select_peek(&mut self, snaps: &Snapshots, id: &str) {
        if self.notice_showing || !self.peeking {
            return;
        }
        let ids = self.current_peek_ids(snaps);
        let Some(next) = ids.iter().position(|i| i == id) else {
            return;
        };
        self.peek.ids = ids.clone();
        self.promote_peek(snaps);
        let old = ids.iter().position(|i| *i == self.peek.id).unwrap_or(next);
        let width = self.peek_width(snaps);
        self.peek.id = id.to_owned();
        if old.abs_diff(next) > 1 || next == old {
            self.peek.offset = naby_core::motion::Tween::instant(0.0);
        } else {
            let start = (next as f64 - old as f64) * width;
            self.peek.offset = self.cfg.motion.to(start, 0.0, self.now, 300.0, 0.0);
        }
        self.dirty = true;
    }

    pub(crate) fn navigate_peek(&mut self, snaps: &Snapshots, direction: i32) {
        let ids = self.current_peek_ids(snaps);
        if let Some(id) = PeekPolicy::navigate(&ids, &self.peek.id, direction) {
            let id = id.to_owned();
            if id != self.peek.id {
                self.select_peek(snaps, &id);
            }
        }
    }

    /// Drag follows the pointer; past either end it resists.
    pub(crate) fn drag_peek(&mut self, snaps: &Snapshots, distance: f64) {
        let ids = self.current_peek_ids(snaps);
        let index = ids.iter().position(|i| *i == self.peek.id).unwrap_or(0);
        let offset = PeekPolicy::drag_offset(
            distance,
            index,
            ids.len(),
            self.peek_width(snaps),
            self.cfg.motion.enabled,
        );
        self.peek.offset = naby_core::motion::Tween::instant(offset);
        self.dirty = true;
    }

    pub(crate) fn finish_peek_swipe(&mut self, snaps: &Snapshots, offset: f64) {
        let ids = self.current_peek_ids(snaps);
        if ids.is_empty() {
            return;
        }
        let index = ids.iter().position(|i| *i == self.peek.id).unwrap_or(0);
        let next =
            PeekPolicy::swipe_target(index, ids.len(), offset, snaps.settings.swipe_distance);
        self.peek.id = ids[next].clone();
        let width = self.peek_width(snaps);
        let from = offset + (next as f64 - index as f64) * width;
        self.peek.offset = self.cfg.motion.to(from, 0.0, self.now, 300.0, 0.0);
        if !self.pointer_inside && !self.interacting() {
            self.hover.start_leave(self.now);
        }
        self.dirty = true;
    }

    pub(crate) fn open_peek_page(&mut self, snaps: &Snapshots) {
        let id = self.peek.id.clone();
        if !naby_core::settings::policy::hover_available(&snaps.settings, &id) {
            return;
        }
        self.open_page(snaps, &id);
    }

    pub(crate) fn drag_peek_volume(&mut self, x: f32, snaps: &Snapshots) -> bool {
        let Some(r) = self.hot.rect(Hot::PeekVolume) else {
            return false;
        };
        if !snaps.audio.available || r.w <= 1.0 {
            return false;
        }
        let v = f64::from(((x - r.x) / r.w).clamp(0.0, 1.0));
        self.push(Command::SetVolume(v));
        true
    }

    /// A press and release on a control of the peek, the notice or the panel.
    pub(crate) fn activate_control(&mut self, hot: Hot, snaps: &Snapshots) {
        let id = self.peek.id.clone();
        match hot {
            Hot::PeekPlay => self.push(Command::MediaToggle),
            Hot::PeekPrev => self.push(Command::MediaPrevious),
            Hot::PeekNext => self.push(Command::MediaNext),
            Hot::PeekMute => {
                self.push(Command::ToggleMute);
                self.push(Command::RefreshAudio);
            }
            Hot::PeekOutput => {
                self.push(Command::RefreshAudio);
                self.open_page(snaps, "media");
            }
            Hot::PeekMixer => {
                self.push(Command::MixerRefresh);
                self.open_page(snaps, "media");
            }
            Hot::PeekToggle => self.push(match id.as_str() {
                "timer" => Command::TimerToggle,
                "stopwatch" => Command::StopwatchToggle,
                _ => Command::FocusToggle,
            }),
            Hot::PeekExtend => self.push(if id == "timer" {
                Command::TimerExtend { seconds: 300.0 }
            } else {
                Command::FocusExtend { seconds: 300.0 }
            }),
            Hot::PeekReset => self.push(match id.as_str() {
                "timer" => Command::TimerReset,
                "stopwatch" => Command::StopwatchReset,
                _ => Command::FocusReset,
            }),
            Hot::PeekEdit | Hot::PeekOpen => self.open_peek_page(snaps),
            Hot::PeekDot(i) => {
                let ids = self.current_peek_ids(snaps);
                if let Some(id) = ids.get(i as usize).cloned() {
                    self.select_peek(snaps, &id);
                }
            }
            Hot::PeekArrow(d) => self.navigate_peek(snaps, i32::from(d)),
            Hot::PeekDetach => {
                let at = self.last_paint_origin;
                let layout = self.layout(snaps);
                self.push(Command::DetachPreview {
                    page: id,
                    at: (layout.window.0 + at.0 as i32, layout.window.1 + at.1 as i32),
                    dragging: false,
                });
                self.collapse(snaps);
            }
            Hot::PeekExtra(i) => {
                if let Some(sc) = snaps.settings.shortcuts.get(i as usize) {
                    self.push(Command::LaunchShortcut {
                        target: sc.target.clone(),
                        app_id: sc.app_id.clone(),
                    });
                }
            }
            Hot::NoticeDismiss
            | Hot::NoticeNext
            | Hot::NoticeClear
            | Hot::NoticeFolder
            | Hot::NoticeShelf
            | Hot::NoticeSettings => self.activate_notice(hot, snaps),
            Hot::PanelTab(_) | Hot::PanelSettings | Hot::PanelAssistant => {
                self.activate_panel(hot, snaps)
            }
            _ => {}
        }
        self.dirty = true;
    }

    // ---- painting ---------------------------------------------------------------------

    pub(crate) fn paint_peek(
        &mut self,
        canvas: &mut Canvas,
        fonts: &Fonts,
        snaps: &Snapshots,
        rect: Rect,
        alpha: f32,
    ) {
        let ids = self.current_peek_ids(snaps);
        if ids.is_empty() {
            return;
        }
        let s = self.scale;
        let geo = self.peek_geo(snaps);
        let index = ids.iter().position(|i| *i == self.peek.id).unwrap_or(0);
        let cx = rect.x + rect.w / 2.0;
        let viewport = Rect::new(
            cx - geo.w as f32 * s / 2.0,
            rect.y + geo.top as f32 * s,
            geo.w as f32 * s,
            geo.viewport_h as f32 * s,
        );
        self.hot.add(Hot::PeekViewport, viewport);
        let offset = self.peek_offset_value();
        let pending = self.panel.pending_input.take();
        canvas.with_opacity(alpha, |canvas| {
            canvas.push_clip_rect(viewport);
            for (i, id) in ids.iter().enumerate() {
                let x = (i as f64 - index as f64) * geo.w + offset;
                if x.abs() >= geo.w - 0.1 {
                    continue;
                }
                let page = viewport.translate(x as f32 * s, 0.0);
                let active = i == index && offset.abs() < 0.5;
                self.paint_peek_page(canvas, fonts, snaps, id, page, active, pending.as_ref());
            }
            canvas.pop_clip();
        });
        if geo.footer {
            let footer = Rect::new(viewport.x, viewport.bottom(), viewport.w, 24.0 * s);
            canvas.with_opacity(alpha, |canvas| {
                self.paint_footer(canvas, fonts, snaps, footer, &ids, index);
            });
        }
    }

    fn paint_footer(
        &mut self,
        canvas: &mut Canvas,
        fonts: &Fonts,
        snaps: &Snapshots,
        footer: Rect,
        ids: &[String],
        index: usize,
    ) {
        let s = self.scale;
        let text_scale = self.cfg.theme.text_scale;
        let dim = self.cfg.theme.text_dim;
        let wide = footer.w >= 270.0 * s;
        if wide {
            let st = style(10.0 * s * text_scale, Weight::Regular, dim);
            let name = peek_name(&self.peek.id);
            let h = fonts.line_height(&st);
            canvas.text(
                fonts,
                footer.x,
                footer.y + (footer.h - h) / 2.0,
                &name,
                &st,
                Some(50.0 * s),
            );
        }
        // Dots.
        let detach = snaps.settings.detachable_previews
            && matches!(
                self.peek.id.as_str(),
                "media"
                    | "timer"
                    | "focus"
                    | "stopwatch"
                    | "calendar"
                    | "notes"
                    | "files"
                    | "system"
            );
        let name_w = if wide { 50.0 } else { 0.0 };
        let arrows_w = 48.0;
        let detach_w = if detach { 74.0 } else { 0.0 };
        let fixed = (name_w + arrows_w + detach_w) as f32 * s;
        let cell =
            (14.0f32 * s).min(((footer.w - fixed - 0.0) / ids.len().max(1) as f32).max(7.0 * s));
        let strip = cell * ids.len() as f32;
        let strip_x = footer.x + name_w as f32 * s + ((footer.w - fixed) - strip) / 2.0;
        for (i, _) in ids.iter().enumerate() {
            let r = Rect::new(strip_x + i as f32 * cell, footer.y, cell, footer.h);
            self.hot.add(Hot::PeekDot(i as u8), r);
            let active = i == index;
            let (w, a) = PeekPolicy::dot(active);
            let dot = Rect::new(
                r.x + (r.w - w as f32 * s.min(cell / 14.0 * s).max(0.0)) / 2.0,
                r.y + (r.h - 4.0 * s) / 2.0,
                w as f32 * s.min(cell / 14.0 * s),
                4.0 * s,
            );
            canvas.fill_rrect(dot, 2.0 * s, PRIMARY.with_alpha((a * 255.0) as u8));
        }
        // Arrows.
        let ax = footer.right() - (detach_w as f32 + arrows_w as f32) * s;
        for (k, dir) in [(0, -1i8), (1, 1i8)] {
            let r = Rect::new(ax + k as f32 * 24.0 * s, footer.y, 24.0 * s, footer.h);
            let enabled = if dir < 0 {
                index > 0
            } else {
                index + 1 < ids.len()
            };
            let color = Rgba::rgb(0xBF, 0xC2, 0xCA).with_alpha(if enabled { 255 } else { 64 });
            let (cx, cy) = r.center();
            let d = 2.4 * s;
            let (x0, x1) = if dir < 0 {
                (cx + d, cx - d)
            } else {
                (cx - d, cx + d)
            };
            let pb = [(x0, cy - 4.0 * s), (x1, cy), (x0, cy + 4.0 * s)];
            stroke_polyline(canvas, &pb, color, 1.3 * s);
            if enabled {
                self.button_light(canvas, r, Hot::PeekArrow(dir));
            }
        }
        if detach {
            let r = Rect::new(
                footer.right() - detach_w as f32 * s + 22.0 * s,
                footer.y,
                52.0 * s,
                footer.h,
            );
            let st = style(10.0 * s * text_scale, Weight::Regular, dim);
            text_centered(canvas, fonts, r, "Detach", &st);
            self.button_light(canvas, r, Hot::PeekDetach);
        }
    }

    /// Registers a hot rect and draws the hover/press light of the C# `ToolRowTemplate`.
    pub(crate) fn button_light(&mut self, canvas: &mut Canvas, r: Rect, hot: Hot) {
        self.hot.add(hot, r);
        let hovered = self.pointer.is_some_and(|p| r.contains_pt(p));
        let pressed = self.pressed == Some(hot);
        if pressed {
            canvas.fill_rrect(r, 8.0 * self.scale, Rgba::WHITE.with_alpha(40));
        } else if hovered {
            canvas.fill_rrect(r, 8.0 * self.scale, Rgba::WHITE.with_alpha(19));
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn icon_button(
        &mut self,
        canvas: &mut Canvas,
        r: Rect,
        icon: Icon,
        hot: Hot,
        enabled: bool,
        color: Rgba,
        active: bool,
    ) {
        let s = self.scale;
        let c = if enabled { color } else { fade(color, 0.3) };
        let size = 14.0 * s;
        let (cx, cy) = r.center();
        let ir = Rect::new(cx - size / 2.0, cy - size / 2.0, size, size);
        if enabled && active {
            self.button_light(canvas, r, hot);
        }
        canvas.icon(icon, ir, c, DEFAULT_STROKE);
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_peek_page(
        &mut self,
        canvas: &mut Canvas,
        fonts: &Fonts,
        snaps: &Snapshots,
        id: &str,
        rect: Rect,
        active: bool,
        input: Option<&InputState>,
    ) {
        if active {
            if !self.panel.shown.iter().any(|s| s == id) {
                self.panel.shown.push(id.to_owned());
            }
            // A page can draw its own face.
            let page = self
                .panel
                .pages
                .entry(id.to_owned())
                .or_insert_with(|| pages::build(id));
            let ctx = PageCtx {
                fonts,
                snaps,
                theme: &self.cfg.theme,
                scale: self.scale,
            };
            let empty = InputState::default();
            let mut out = Vec::new();
            let drawn = page.peek(
                canvas,
                &ctx,
                rect,
                input.unwrap_or(&empty),
                &mut out,
                self.peek.detailed,
            );
            self.outbox.append(&mut out);
            if drawn.is_some() {
                return;
            }
        }
        match id {
            "media" => self.peek_media(canvas, fonts, snaps, rect, active),
            "timer" | "focus" | "stopwatch" => {
                self.peek_clock(canvas, fonts, snaps, id, rect, active)
            }
            _ => self.peek_simple(canvas, fonts, snaps, id, rect, active),
        }
    }

    // ---- media face -------------------------------------------------------------------

    fn peek_media(
        &mut self,
        canvas: &mut Canvas,
        fonts: &Fonts,
        snaps: &Snapshots,
        r: Rect,
        active: bool,
    ) {
        let s = self.scale;
        let k = self.cfg.theme.text_scale;
        let dim = self.cfg.theme.text_dim;
        let theme_text = self.cfg.theme.text;
        let m = &snaps.media;
        let media = m.available;
        let wall = snaps.wall.with_timezone(&chrono::Utc);
        let settings = &snaps.settings;
        let countdown = active_countdown(
            settings.widget_enabled("focus"),
            settings.widget_enabled("timer"),
            &snaps.session.focus,
            &snaps.session.timer,
            wall,
        );
        // Cover.
        let cover = Rect::new(r.x, r.y, 56.0 * s, 56.0 * s);
        canvas.fill_rrect(cover, 14.0 * s, Rgba::WHITE.with_alpha(0x16));
        let art = if media { self.artwork(snaps) } else { None };
        match art {
            Some(image) => canvas.draw_image(&image, cover, 1.0, 14.0 * s),
            None => canvas.icon(
                Icon::Activity,
                cover.inset(14.0 * s),
                theme_text,
                DEFAULT_STROKE,
            ),
        }
        // Title and subtitle.
        let (title, subtitle) = if media {
            (m.title.clone(), m.artist.clone())
        } else if let Some(c) = countdown {
            let label = match c {
                ActiveCountdown::Focus => format!(
                    "Focus \u{b7} {}",
                    snaps.session.focus.countdown.display(wall)
                ),
                ActiveCountdown::Timer => {
                    format!("Timer \u{b7} {}", snaps.session.timer.display(wall))
                }
            };
            (
                label,
                "Session running \u{b7} click for controls".to_owned(),
            )
        } else {
            (
                snaps.wall.format("%H:%M").to_string(),
                snaps.wall.format("%A, %-d %B").to_string(),
            )
        };
        let weight = if media && settings.bold_media_title {
            Weight::Semibold
        } else {
            Weight::Regular
        };
        let play_w = if media { 38.0 + 16.0 } else { 0.0 } * s;
        let tx = r.x + 72.0 * s;
        let tw = (r.w - 72.0 * s - play_w).max(10.0);
        let tst = style(15.0 * s * k, weight, theme_text);
        let sst = style(12.0 * s * k, Weight::Regular, dim);
        let th = fonts.line_height(&tst);
        let sh = fonts.line_height(&sst);
        let block = th + 6.0 * s + sh;
        let ty = r.y + (56.0 * s - block) / 2.0;
        canvas.text(fonts, tx, ty, &title, &tst, Some(tw));
        canvas.text(fonts, tx, ty + th + 6.0 * s, &subtitle, &sst, Some(tw));
        if media {
            let pr = Rect::new(r.right() - 38.0 * s, r.y + 9.0 * s, 38.0 * s, 38.0 * s);
            let enabled = m.can_toggle;
            self.hot.add(Hot::PeekPlay, pr);
            let hovered = self.pointer.is_some_and(|p| pr.contains_pt(p));
            let base = if enabled { PRIMARY } else { fade(PRIMARY, 0.4) };
            let col = if hovered && enabled {
                base.lerp(Rgba::rgb(0xE6, 0xE7, 0xEB), 0.5)
            } else {
                base
            };
            canvas.fill_circle(pr.center().0, pr.center().1, 19.0 * s, &Fill::Solid(col));
            let icon = if m.playing { Icon::Pause } else { Icon::Play };
            canvas.icon(icon, pr.inset(10.0 * s), Rgba::BLACK, DEFAULT_STROKE);
        }
        if !self.peek.detailed {
            return;
        }
        // Progress.
        let (elapsed, duration) = if media {
            snaps.media_timeline()
        } else if let Some(c) = countdown {
            let (total, left) = match c {
                ActiveCountdown::Focus => (
                    snaps.session.focus.countdown.duration_seconds,
                    snaps.session.focus.countdown.remaining(wall),
                ),
                ActiveCountdown::Timer => (
                    snaps.session.timer.duration_seconds,
                    snaps.session.timer.remaining(wall),
                ),
            };
            (total - left, total)
        } else {
            (0.0, 0.0)
        };
        let bar = Rect::new(r.x, r.y + 68.0 * s, r.w, 3.0 * s);
        if duration > 0.0 {
            canvas.fill_rrect(bar, 1.5 * s, BAR_TRACK);
            let f = (elapsed / duration).clamp(0.0, 1.0) as f32;
            if f > 0.0 {
                canvas.fill_rrect(Rect::new(bar.x, bar.y, bar.w * f, bar.h), 1.5 * s, PRIMARY);
            }
        }
        let long = duration >= 3600.0;
        let (left_text, right_text) = if duration > 0.0 {
            (
                super::util::format_clock(elapsed, long),
                super::util::format_clock(duration, long),
            )
        } else if media {
            ("Windows media controls".to_owned(), String::new())
        } else {
            ("No active session".to_owned(), "Open \u{2192}".to_owned())
        };
        let tst = style(11.0 * s * k, Weight::Regular, dim);
        let row = Rect::new(r.x, r.y + 80.0 * s, r.w, 18.0 * s);
        let h = fonts.line_height(&tst);
        canvas.text(fonts, row.x, row.y, &left_text, &tst, Some(row.w * 0.4));
        let rw = fonts.measure_width(&right_text, &tst);
        canvas.text(fonts, row.right() - rw, row.y, &right_text, &tst, None);
        let _ = h;
        if self.peek.id == "media" && media && settings.live_activity {
            let w = 58.0 * s;
            let wr = Rect::new(r.x + (r.w - w) / 2.0, row.y - 1.0 * s, w, 16.0 * s);
            self.closed.peek_wave.paint(canvas, wr, s, theme_text);
        }
        // Controls row.
        let crow = Rect::new(r.x, r.y + 92.0 * s, r.w, 32.0 * s);
        let narrow = r.w < 260.0 * s;
        let mut x = crow.x;
        if !narrow {
            self.icon_button(
                canvas,
                Rect::new(x, crow.y, 34.0 * s, crow.h),
                Icon::Previous,
                Hot::PeekPrev,
                m.can_previous,
                theme_text,
                active,
            );
            x += 34.0 * s;
            self.icon_button(
                canvas,
                Rect::new(x, crow.y, 34.0 * s, crow.h),
                Icon::Next,
                Hot::PeekNext,
                m.can_next,
                theme_text,
                active,
            );
            x += 34.0 * s;
        }
        let audio = &snaps.audio;
        let mute_icon = if audio.muted {
            Icon::Muted
        } else {
            Icon::Volume
        };
        self.icon_button(
            canvas,
            Rect::new(x, crow.y, 34.0 * s, crow.h),
            mute_icon,
            Hot::PeekMute,
            audio.available,
            theme_text,
            active,
        );
        x += 34.0 * s;
        let right = crow.right() - 56.0 * s;
        let slider = Rect::new(
            x + 7.0 * s,
            crow.y,
            (right - x - 16.0 * s).max(10.0),
            crow.h,
        );
        self.paint_slider(
            canvas,
            slider,
            f32::from(audio.available as u8) * audio.volume,
            audio.available,
            Hot::PeekVolume,
        );
        self.icon_button(
            canvas,
            Rect::new(right, crow.y, 28.0 * s, crow.h),
            Icon::Chevron,
            Hot::PeekOutput,
            audio.available,
            theme_text,
            active,
        );
        self.icon_button(
            canvas,
            Rect::new(right + 28.0 * s, crow.y, 28.0 * s, crow.h),
            Icon::Mixer,
            Hot::PeekMixer,
            true,
            theme_text,
            active,
        );
    }

    fn paint_slider(&mut self, canvas: &mut Canvas, r: Rect, value: f32, enabled: bool, hot: Hot) {
        let s = self.scale;
        let track = Rect::new(r.x, r.y + (r.h - 3.0 * s) / 2.0, r.w, 3.0 * s);
        canvas.with_opacity(if enabled { 1.0 } else { 0.4 }, |canvas| {
            canvas.fill_rrect(track, 1.5 * s, self.cfg.theme.track_fill);
            let fill = Rect::new(track.x, track.y, track.w * value.clamp(0.0, 1.0), track.h);
            if fill.w > 0.0 {
                canvas.fill_rrect(fill, 1.5 * s, self.cfg.theme.progress_fill);
            }
            let hovered = self.pointer.is_some_and(|p| r.contains_pt(p));
            let radius = 6.0
                * s
                * if hovered || self.peek.slider_drag {
                    1.16
                } else {
                    1.0
                };
            canvas.fill_circle(
                track.x + track.w * value.clamp(0.0, 1.0),
                track.y + track.h / 2.0,
                radius,
                &Fill::Solid(self.cfg.theme.switch_knob),
            );
        });
        if enabled {
            self.hot
                .add(hot, Rect::new(r.x - 6.0 * s, r.y, r.w + 12.0 * s, r.h));
        }
    }

    // ---- timer / focus / stopwatch face ----------------------------------------------

    fn peek_clock(
        &mut self,
        canvas: &mut Canvas,
        fonts: &Fonts,
        snaps: &Snapshots,
        id: &str,
        r: Rect,
        active: bool,
    ) {
        let s = self.scale;
        let k = self.cfg.theme.text_scale;
        let dim = self.cfg.theme.text_dim;
        let text = self.cfg.theme.text;
        let wall = snaps.wall.with_timezone(&chrono::Utc);
        let detailed = self.peek.detailed;
        let (label, time, fraction, running) = match id {
            "timer" => {
                let t = &snaps.session.timer;
                let left = t.remaining(wall);
                let mut label = "Timer".to_owned();
                if left > 0.0 && !t.running() {
                    label.push_str(" \u{b7} paused");
                }
                (
                    label,
                    if left > 0.0 {
                        t.display(wall)
                    } else {
                        format_seconds(300.0)
                    },
                    if left > 0.0 { t.progress(wall) } else { 0.0 },
                    t.running(),
                )
            }
            "focus" => {
                let f = &snaps.session.focus;
                let left = f.countdown.remaining(wall);
                let mut label = if f.is_break() { "Break" } else { "Focus" }.to_owned();
                if left > 0.0 && !f.running() {
                    label.push_str(" \u{b7} paused");
                }
                (
                    label,
                    if left > 0.0 {
                        f.countdown.display(wall)
                    } else {
                        format_seconds(f64::from(snaps.settings.focus_minutes) * 60.0)
                    },
                    if left > 0.0 {
                        f.countdown.progress(wall)
                    } else {
                        0.0
                    },
                    f.running(),
                )
            }
            _ => {
                let mut sw = snaps.session.stopwatch.clone();
                let clock = naby_core::session::Clock::system();
                let running = sw.running;
                ("Stopwatch".to_owned(), sw.display(clock), 0.0, running)
            }
        };
        let lst = style(11.0 * s * k, Weight::Regular, dim);
        canvas.text(fonts, r.x, r.y + 3.0 * s, &label, &lst, Some(r.w));
        let tst = style(32.0 * s * k, Weight::Semibold, text);
        canvas.text(fonts, r.x, r.y + 20.0 * s, &time, &tst, Some(r.w));
        if !detailed {
            return;
        }
        if id != "stopwatch" {
            let bar = Rect::new(r.x, r.y + 77.0 * s, r.w, 2.0 * s);
            canvas.fill_rrect(bar, 1.0 * s, Rgba::WHITE.with_alpha(0x35));
            if fraction > 0.0 {
                canvas.fill_rrect(
                    Rect::new(bar.x, bar.y, bar.w * fraction as f32, bar.h),
                    1.0 * s,
                    Rgba::rgb(0xDC, 0xDA, 0xD5),
                );
            }
        }
        let row = Rect::new(r.x, r.y + 88.0 * s, r.w, 32.0 * s);
        let bst = style(11.0 * s * k, Weight::Regular, text);
        let mst = style(11.0 * s * k, Weight::Regular, dim);
        let icon = if running { Icon::Pause } else { Icon::Play };
        self.icon_button(
            canvas,
            Rect::new(row.x, row.y, 28.0 * s, row.h),
            icon,
            Hot::PeekToggle,
            true,
            text,
            active,
        );
        let mut x = row.x + 36.0 * s;
        let mut text_button = |this: &mut Overlay,
                               canvas: &mut Canvas,
                               label: &str,
                               hot: Hot,
                               st: &naby_render::TextStyle,
                               gap: f32| {
            let w = fonts.measure_width(label, st) + 14.0 * s;
            let b = Rect::new(x + gap, row.y + 3.0 * s, w, row.h - 6.0 * s);
            if active {
                this.button_light(canvas, b, hot);
            }
            text_centered(canvas, fonts, b, label, st);
            x += w + gap;
        };
        if id != "stopwatch" {
            text_button(self, canvas, "+5 min", Hot::PeekExtend, &bst, 0.0);
        }
        text_button(self, canvas, "Reset", Hot::PeekReset, &mst, 6.0 * s);
        let edit_w = fonts.measure_width("Edit", &mst) + 14.0 * s;
        let eb = Rect::new(
            row.right() - edit_w,
            row.y + 3.0 * s,
            edit_w,
            row.h - 6.0 * s,
        );
        if active {
            self.button_light(canvas, eb, Hot::PeekEdit);
        }
        text_centered(canvas, fonts, eb, "Edit", &mst);
    }

    // ---- everything else --------------------------------------------------------------

    fn peek_simple(
        &mut self,
        canvas: &mut Canvas,
        fonts: &Fonts,
        snaps: &Snapshots,
        id: &str,
        r: Rect,
        active: bool,
    ) {
        let s = self.scale;
        let k = self.cfg.theme.text_scale;
        let dim = self.cfg.theme.text_dim;
        let text = self.cfg.theme.text;
        let name = peek_name(id);
        let hst = style(13.0 * s * k, Weight::Semibold, text);
        let mut y = r.y;
        // The weather glance leads with the temperature instead of a heading.
        let mut detail = String::new();
        let mut big = false;
        match id {
            "clock" => {
                detail = super::closed::clock_text(snaps, false);
                big = true;
            }
            "battery" => detail = snaps.battery.hover_text(),
            "volume" => {
                detail = if snaps.audio.available {
                    format!(
                        "{:.0}% \u{b7} {}",
                        snaps.audio.volume * 100.0,
                        if snaps.audio.muted {
                            "Muted"
                        } else {
                            "Output volume"
                        }
                    )
                } else {
                    "No output device".to_owned()
                }
            }
            "system" => {
                let cpu = snaps
                    .system
                    .cpu
                    .map_or("\u{2014}".to_owned(), |c| format!("{c:.0}%"));
                let ram = snaps
                    .system
                    .latest
                    .as_ref()
                    .map_or("\u{2014}".to_owned(), |m| m.memory_text());
                detail = format!("CPU  {cpu}\nRAM  {ram}");
            }
            "shortcuts" => detail = "Your favorite apps, one click away".to_owned(),
            "awake" => detail = "Keep your computer awake while you work".to_owned(),
            "links" => detail = "Remove tracking parameters from a link".to_owned(),
            "voice" => {
                detail = if snaps.assistant.voice_connected {
                    "Voice session active".to_owned()
                } else {
                    "A dedicated space to talk with Naby".to_owned()
                }
            }
            "weather" => {
                detail = match snaps.weather.snapshot.as_ref() {
                    Some(w) => format!(
                        "{} \u{b7} H {} L {}",
                        naby_core::weather::condition(w.code),
                        naby_core::weather::degrees(w.high),
                        naby_core::weather::degrees(w.low)
                    ),
                    None => "Choose a city in Weather".to_owned(),
                };
            }
            "calendar" => {
                let now = snaps.wall.with_timezone(&chrono::Utc);
                let next = snaps
                    .calendar
                    .events()
                    .into_iter()
                    .filter(|e| e.end.with_timezone(&chrono::Utc) > now)
                    .min_by_key(|e| e.start);
                detail = match next {
                    Some(e) => format!(
                        "{}\n{}",
                        e.title,
                        e.start
                            .with_timezone(&chrono::Local)
                            .format("%a, %-d %b \u{b7} %H:%M")
                    ),
                    None => "No upcoming events".to_owned(),
                };
            }
            "notes" => {
                let p = snaps.productivity.borrow();
                let note = p.note().trim().to_owned();
                detail = if note.is_empty() {
                    "Quick note".to_owned()
                } else {
                    note.lines().take(2).collect::<Vec<_>>().join("\n")
                };
            }
            "files" => {
                let p = snaps.productivity.borrow();
                detail = match p.shelf().last() {
                    Some(f) => format!("{} staged\n{}", p.shelf().len(), f.name()),
                    None => "Drop files onto the notch".to_owned(),
                };
            }
            "tasks" => {
                let p = snaps.productivity.borrow();
                let open = p.tasks().iter().filter(|t| !t.done).count();
                detail = format!("{open} open");
                if let Some(t) = p.tasks().iter().find(|t| !t.done) {
                    detail.push_str(&format!("\n{}", t.text));
                }
            }
            "clips" => {
                let p = snaps.productivity.borrow();
                detail = match p.clips().first() {
                    Some(c) => c.label(),
                    None => "Clipboard history is empty".to_owned(),
                };
            }
            _ => {}
        }
        if id == "weather" {
            let temp = snaps
                .weather
                .snapshot
                .as_ref()
                .map_or("\u{2014}\u{b0}".to_owned(), |w| {
                    naby_core::weather::degrees(w.temperature)
                });
            let tst = style(34.0 * s * k, Weight::Regular, text);
            canvas.text(fonts, r.x, y, &temp, &tst, Some(r.w));
            y += fonts.line_height(&tst);
        } else {
            canvas.text(fonts, r.x, y, &name, &hst, Some(r.w));
            y += fonts.line_height(&hst) + 9.0 * s;
        }
        let size = if big { 30.0 } else { 12.0 };
        let dst = style(size * s * k, Weight::Regular, dim);
        let line_h = fonts.line_height(&dst);
        for line in detail.lines().take(3) {
            canvas.text(fonts, r.x, y, line, &dst, Some(r.w));
            y += line_h + 1.0 * s;
        }
        if id == "volume" {
            let sl = Rect::new(r.x, y + 2.0 * s, r.w, 26.0 * s);
            self.paint_slider(
                canvas,
                sl,
                snaps.audio.volume,
                snaps.audio.available && active,
                Hot::PeekVolume,
            );
            y += 30.0 * s;
        }
        if id == "shortcuts" && self.peek.detailed {
            let mut x = r.x;
            for (i, sc) in snaps.settings.shortcuts.iter().take(4).enumerate() {
                let st = style(11.0 * s * k, Weight::Regular, text);
                let w = fonts.measure_width(&sc.name, &st) + 14.0 * s;
                if x + w > r.right() {
                    break;
                }
                let b = Rect::new(x, y, w, 22.0 * s);
                if active {
                    self.button_light(canvas, b, Hot::PeekExtra(i as u8));
                }
                text_centered(canvas, fonts, b, &sc.name, &st);
                x += w + 4.0 * s;
            }
            y += 24.0 * s;
        }
        if self.peek.detailed {
            let label = format!("Open {name}");
            let ost = style(10.0 * s * k, Weight::Regular, dim);
            let w = fonts.measure_width(&label, &ost) + 14.0 * s;
            let b = Rect::new(r.x, (y + 2.0 * s).min(r.bottom() - 22.0 * s), w, 22.0 * s);
            if active {
                self.button_light(canvas, b, Hot::PeekOpen);
            }
            text_centered(canvas, fonts, b, &label, &ost);
        }
    }
}

/// Display name of a hover page (`PeekName`): "Music" for media.
pub fn peek_name(id: &str) -> String {
    if id == "media" {
        "Music".to_owned()
    } else {
        pages::page_title(id)
    }
}

fn stroke_polyline(canvas: &mut Canvas, points: &[(f32, f32)], color: Rgba, width: f32) {
    // Short two-segment strokes are drawn as thin rotated rounded rects via arcs of lines.
    for pair in points.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let len = (dx * dx + dy * dy).sqrt().max(0.001);
        let steps = (len / (width * 0.5)).ceil().max(1.0) as usize;
        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            canvas.fill_circle(a.0 + dx * t, a.1 + dy * t, width / 2.0, &Fill::Solid(color));
        }
    }
}
