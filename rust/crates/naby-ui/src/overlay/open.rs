//! The open panel: page dots, settings/assistant buttons, widget pages grouped by category
//! (C# `BuildCompactBody`, `PageFor`, `SelectPage`, `Dimensions`).

use super::util::{style, Hot};
use super::Overlay;
use crate::host::Snapshots;
use crate::pages::{self, PageCtx};
use naby_core::color::Rgba;
use naby_core::command::Command;
use naby_core::motion::cycle_page;
use naby_core::settings::{is_tool_name, widget_display_name};
use naby_render::icons::DEFAULT_STROKE;
use naby_render::{Canvas, Fill, Fonts, Icon, InputState, Rect, Weight};

/// C# `PageFor`: which panel page (dot) a widget lives on.
pub fn page_key(widget: &str) -> String {
    if is_tool_name(widget) {
        return widget.to_owned();
    }
    match widget {
        "media" | "volume" => "media",
        "timer" | "focus" => "focus",
        "stopwatch" => "stopwatch",
        "voice" => "voice",
        "shortcuts" => "access",
        "tools" => "tools",
        _ => "status",
    }
    .to_owned()
}

/// C# `PageName`.
#[allow(dead_code)]
pub fn page_name(key: &str) -> String {
    if is_tool_name(key) {
        return widget_display_name(key).unwrap_or(key).to_owned();
    }
    match key {
        "media" => "Media",
        "focus" => "Focus",
        "stopwatch" => "Stopwatch",
        "voice" => "Naby voice",
        "status" => "Status",
        "tools" => "Tools",
        _ => "Launch",
    }
    .to_owned()
}

impl Overlay {
    /// Enabled widgets in settings order (plus the hover-opened one).
    pub(crate) fn panel_widget_ids(&self, snaps: &Snapshots) -> Vec<String> {
        let forced = self.panel.forced_widget.as_deref();
        snaps
            .settings
            .widgets
            .iter()
            .filter(|w| w.enabled || Some(w.id.as_str()) == forced)
            .map(|w| w.id.clone())
            .chain(
                forced
                    .filter(|f| !snaps.settings.widgets.iter().any(|w| w.id == *f))
                    .map(str::to_owned),
            )
            .collect()
    }

    /// Distinct page keys in dot order.
    pub(crate) fn page_keys(&self, snaps: &Snapshots) -> Vec<String> {
        let mut keys: Vec<String> = Vec::new();
        for id in self.panel_widget_ids(snaps) {
            let key = page_key(&id);
            if !keys.contains(&key) {
                keys.push(key);
            }
        }
        keys
    }

    /// Widget pages shown on the active dot.
    pub(crate) fn group_ids(&self, snaps: &Snapshots) -> Vec<String> {
        self.panel_widget_ids(snaps)
            .into_iter()
            .filter(|w| page_key(w) == self.panel.active)
            .collect()
    }

    pub(crate) fn ensure_panel_page(&mut self, snaps: &Snapshots) {
        let keys = self.page_keys(snaps);
        if !keys.contains(&self.panel.active) {
            if let Some(first) = keys.first() {
                self.panel.active.clone_from(first);
            }
        }
    }

    pub(crate) fn measure_panel_on_next_paint(&mut self) {
        self.panel.measure = true;
    }

    /// Opens the panel on the page of widget (or page) `id` (C# `OpenHoverPanel`).
    pub(crate) fn open_page(&mut self, snaps: &Snapshots, id: &str) {
        let widget = match id {
            "access" => "shortcuts",
            "status" => "clock",
            "mixer" | "lyrics" => "media",
            other => other,
        };
        let enabled = snaps.settings.widget_enabled(widget);
        self.panel.forced_widget = (!enabled).then(|| widget.to_owned());
        self.panel.active = page_key(widget);
        self.panel.scroll = 0.0;
        if self.expanded {
            self.panel.content_height = 0.0;
            self.measure_panel_on_next_paint();
            let ms = snaps.settings.animation_ms.min(300.0);
            self.retarget(snaps, ms);
            self.dirty = true;
        } else {
            self.reveal(snaps);
        }
    }

    pub(crate) fn cycle_panel_page(&mut self, snaps: &Snapshots, direction: i32) {
        let keys = self.page_keys(snaps);
        if let Some(next) = cycle_page(&keys, &self.panel.active, direction) {
            if next != self.panel.active {
                self.panel.active = next;
                self.panel.scroll = 0.0;
                self.page_enter();
                self.panel.memory.clear();
                self.panel.content_height = 0.0;
                self.measure_panel_on_next_paint();
                self.dirty = true;
            }
        }
    }

    /// The page entrance (`Motion.Enter(page, 4)`): fade in while sliding up 4 DIP.
    fn page_enter(&mut self) {
        let enter = self.cfg.motion.enter(4.0, 0.0, self.now);
        self.page_fade = enter.opacity;
        self.page_slide = enter.offset_y;
    }

    pub(crate) fn panel_overflow(&self, _snaps: &Snapshots) -> f64 {
        f64::from(self.panel.overflow)
    }

    pub(crate) fn scroll_panel(&mut self, wheel_y: f32, overflow: f32) {
        self.panel.scroll = (self.panel.scroll + wheel_y).clamp(0.0, overflow.max(0.0));
        self.dirty = true;
    }

    pub(crate) fn panel_wants_input(&self, input: &InputState) -> bool {
        self.pointer_inside
            || !input.keys.is_empty()
            || !input.text.is_empty()
            || input.paste.is_some()
            || input.released
    }

    pub(crate) fn panel_needs_seconds(&self, _snaps: &Snapshots) -> bool {
        false
    }

    pub(crate) fn activate_panel(&mut self, hot: Hot, snaps: &Snapshots) {
        match hot {
            Hot::PanelTab(i) => {
                let keys = self.page_keys(snaps);
                if let Some(key) = keys.get(i as usize) {
                    if *key != self.panel.active {
                        let key = key.clone();
                        self.panel.active = key;
                        self.panel.scroll = 0.0;
                        self.panel.content_height = 0.0;
                        self.panel.memory.clear();
                        self.page_enter();
                        self.measure_panel_on_next_paint();
                    }
                }
            }
            Hot::PanelSettings => self.push(Command::OpenSettings),
            Hot::PanelAssistant => self.push(Command::OpenAssistant),
            _ => {}
        }
    }

    // ---- painting ---------------------------------------------------------------------

    pub(crate) fn paint_panel(
        &mut self,
        canvas: &mut Canvas,
        fonts: &Fonts,
        snaps: &Snapshots,
        rect: Rect,
        alpha: f32,
    ) {
        let s = self.scale;
        let settings = &snaps.settings;
        let ps = settings.panel_scale as f32;
        let cs = s * ps;
        let frame_pad = if settings.compact_layout { 44.0 } else { 26.0 };
        let target = self.target_size;
        let pad = settings.panel_padding as f32;
        let view_w = (target.0 as f32 - 2.0 * pad) * s;
        let top = if frame_pad == 44.0 { 20.0 } else { 10.0 };
        let view = Rect::new(
            rect.x + (rect.w - view_w) / 2.0,
            rect.y + top * s + self.panel_slide.value(self.now) as f32 * s,
            view_w.max(1.0),
            ((target.1 as f32 - frame_pad) * s).max(1.0),
        );
        self.hot.add(Hot::PanelBody, view);
        let keys = self.page_keys(snaps);
        let dim = self.cfg.theme.text_dim;
        let k = self.cfg.theme.text_scale;
        let theme_text = self.cfg.theme.text;
        // Navigation row.
        let nav_h = 28.0 * cs;
        let nav = Rect::new(view.x, view.y, view.w, nav_h);
        let content_top = nav.bottom() + settings.section_gap as f32 * cs;
        let last_input = self.panel.last_input.clone();
        let mut input = self
            .panel
            .pending_input
            .take()
            .unwrap_or_else(|| last_input.without_events());
        // The nav strip never forwards presses to pages.
        let nav_hit = input.pointer.is_some_and(|p| nav.contains_pt(p));
        if nav_hit {
            input.pointer = None;
        }
        canvas.with_opacity(alpha, |canvas| {
            // Assistant launcher and settings buttons.
            let left = Rect::new(nav.x, nav.y, 28.0 * cs, nav_h);
            let right = Rect::new(nav.right() - 28.0 * cs, nav.y, 28.0 * cs, nav_h);
            self.button_light(canvas, left, Hot::PanelAssistant);
            canvas.icon(Icon::Chat, left.inset(6.0 * cs), dim, DEFAULT_STROKE);
            self.button_light(canvas, right, Hot::PanelSettings);
            canvas.icon(Icon::Settings, right.inset(6.0 * cs), dim, DEFAULT_STROKE);
            // Dots, centered.
            let cell_w = 18.0 * cs;
            let strip_w = cell_w * keys.len() as f32;
            let sx = nav.x + (nav.w - strip_w) / 2.0;
            for (i, key) in keys.iter().enumerate() {
                let cell = Rect::new(
                    sx + i as f32 * cell_w,
                    nav.y + (nav_h - 30.0 * cs) / 2.0,
                    cell_w,
                    30.0 * cs,
                );
                self.hot.add(Hot::PanelTab(i as u8), cell);
                let selected = *key == self.panel.active;
                let w = if selected { 13.0 } else { 5.0 } * s * ps;
                let dot = Rect::new(
                    cell.x + (cell.w - w) / 2.0,
                    cell.y + (cell.h - 5.0 * cs) / 2.0,
                    w,
                    5.0 * cs,
                );
                let color = if selected {
                    Rgba::WHITE
                } else {
                    dim.with_alpha(128)
                };
                canvas.fill_rrect(dot, 2.5 * cs, color);
            }
        });
        // Page group.
        let group = self.group_ids(snaps);
        if group.is_empty() {
            let st = style(12.0 * cs * k, Weight::Regular, dim);
            canvas.text(
                fonts,
                view.x,
                content_top,
                "Enable widgets in Settings.",
                &st,
                Some(view.w),
            );
            return;
        }
        let measure_wide = group
            .iter()
            .any(|g| self.panel.pages.get(g).is_some_and(|p| p.wants_wide()));
        // Build pages lazily and measure.
        let mut out = Vec::new();
        let mut heights: Vec<f32> = Vec::with_capacity(group.len());
        for id in &group {
            let page = self
                .panel
                .pages
                .entry(id.clone())
                .or_insert_with(|| pages::build(id));
            let ctx = PageCtx {
                fonts,
                snaps,
                theme: &self.cfg.theme,
                scale: cs,
            };
            heights.push(page.height(&ctx, view.w));
        }
        let side_by_side = self.panel.active == "focus" && group.len() > 1 && view.w / cs >= 310.0;
        let gap = if side_by_side {
            22.0 * cs
        } else if self.panel.active == "focus" {
            16.0 * cs
        } else {
            10.0 * cs
        };
        let content_h = if side_by_side {
            heights.iter().copied().fold(0.0, f32::max)
        } else {
            heights.iter().sum::<f32>() + gap * (heights.len().saturating_sub(1)) as f32
        };
        let viewport = Rect::new(
            view.x,
            content_top,
            view.w,
            (view.bottom() - content_top).max(1.0),
        );
        let total = content_top - view.y + content_h;
        self.panel.overflow = (total - view.h).max(0.0);
        let body_dip = (nav_h + settings.section_gap as f32 * cs + content_h) / cs;
        let wide = measure_wide;
        if self.panel.measure
            || (f64::from(body_dip) - self.panel.content_height).abs() > 1.0
            || wide != self.panel.wide
        {
            self.panel.measure = false;
            self.panel.wide = wide;
            self.panel.content_height = f64::from(body_dip);
            self.retarget_pending = true;
        }
        self.panel.scroll = self.panel.scroll.clamp(0.0, self.panel.overflow);
        canvas.push_clip_rect(Rect::new(
            view.x - 2.0 * s,
            content_top - 2.0 * s,
            view.w + 4.0 * s,
            view.bottom() - content_top + 2.0 * s,
        ));
        let page_alpha = self.page_fade.value(self.now) as f32;
        let mut y = content_top - self.panel.scroll + self.page_slide.value(self.now) as f32 * cs;
        let n = group.len();
        let col_w = if side_by_side {
            (view.w - gap * (n as f32 - 1.0)) / n as f32
        } else {
            view.w
        };
        let mut animating = false;
        for (i, id) in group.iter().enumerate() {
            let (x, w, h) = if side_by_side {
                (view.x + i as f32 * (col_w + gap), col_w, heights[i])
            } else {
                (view.x, view.w, heights[i])
            };
            let page_rect = Rect::new(x, y, w, h);
            let ctx = PageCtx {
                fonts,
                snaps,
                theme: &self.cfg.theme,
                scale: cs,
            };
            if let Some(page) = self.panel.pages.get_mut(id) {
                let visible = page_rect.intersect(&viewport).is_some();
                if visible {
                    animating |= canvas.with_opacity(page_alpha, |c| {
                        page.frame(c, &ctx, page_rect, &input, &mut out)
                    });
                }
            }
            if !side_by_side {
                y += h + gap;
            }
        }
        canvas.pop_clip();
        self.outbox.append(&mut out);
        if animating {
            self.dirty = true;
        }
        // Scroll hint: a thin bar while the page overflows.
        if self.panel.overflow > 1.0 {
            let track = Rect::new(view.right() + 4.0 * s, viewport.y, 2.0 * s, viewport.h);
            let thumb_h = (track.h * viewport.h / (viewport.h + self.panel.overflow)).max(16.0 * s);
            let pos = self.panel.scroll / self.panel.overflow.max(1.0);
            let thumb = Rect::new(
                track.x,
                track.y + (track.h - thumb_h) * pos,
                track.w,
                thumb_h,
            );
            canvas.fill_round_rect(thumb, 1.0 * s, &Fill::Solid(theme_text.with_alpha(60)));
        }
    }
}

#[allow(dead_code)]
fn unused(_: &pages::TitlePage) {}
