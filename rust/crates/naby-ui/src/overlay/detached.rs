//! Detached previews (C# `DetachedPreviews.cs`, `DetachedPreviewViews.cs`): a hover page shown in
//! its own small floating window that stays on screen after the dock collapses.
//!
//! [`DetachedView`] is the platform-free half: a [`View`] that draws a title bar, the page of
//! the widget (the same [`Page`] the panel uses) and an "Open <name>" button, and turns dragging
//! the title or the corner into [`DetachedRequest`]s. The Windows host owns the layered windows,
//! applies those requests and persists the geometry in `detached-<id>` documents.

use super::util::{style, text_centered};
use crate::host::{Demand, Placement, Signal, Snapshots, View};
use crate::pages::{self, Page, PageCtx};
use naby_core::command::Command;
use naby_core::settings::{policy, Settings};
use naby_render::icons::DEFAULT_STROKE;
use naby_render::{Canvas, Fonts, Icon, InputState, Rect, Theme, Weight};
use serde::{Deserialize, Serialize};

/// Smallest window (DIP): `MinWidth` / `MinHeight` of the WPF window.
pub const MIN_SIZE: (f64, f64) = (280.0, 220.0);
/// Size of a window opened for the first time (DIP).
pub const DEFAULT_SIZE: (f64, f64) = (350.0, 330.0);

/// The persisted window rectangle in the `detached-<id>` document (`DetachedPreviewGeometry`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct DetachedGeometry {
    pub left: f64,
    pub top: f64,
    pub width: f64,
    pub height: f64,
}

impl Default for DetachedGeometry {
    fn default() -> Self {
        DetachedGeometry {
            left: 80.0,
            top: 80.0,
            width: DEFAULT_SIZE.0,
            height: DEFAULT_SIZE.1,
        }
    }
}

impl DetachedGeometry {
    /// `SafeRect`: non-finite or non-positive values fall back to the defaults.
    pub fn sanitized(&self) -> (f64, f64, f64, f64) {
        let d = DetachedGeometry::default();
        let finite = |v: f64, fallback: f64| if v.is_finite() { v } else { fallback };
        let size = |v: f64, fallback: f64| {
            if v.is_finite() && v > 0.0 {
                v
            } else {
                fallback
            }
        };
        (
            finite(self.left, d.left),
            finite(self.top, d.top),
            size(self.width, d.width),
            size(self.height, d.height),
        )
    }
}

/// `DetachedPreviewGeometry.Clamp`: keeps a window of at least 280 x 220 (or the work area when
/// smaller) fully inside `work`. Both rectangles are `(x, y, width, height)`.
pub fn clamp_geometry(
    candidate: (f64, f64, f64, f64),
    work: (f64, f64, f64, f64),
) -> (f64, f64, f64, f64) {
    let (cx, cy, cw, ch) = candidate;
    let (wx, wy, ww, wh) = work;
    let width = (if cw.is_finite() { cw } else { DEFAULT_SIZE.0 }).clamp(MIN_SIZE.0.min(ww), ww);
    let height = (if ch.is_finite() { ch } else { DEFAULT_SIZE.1 }).clamp(MIN_SIZE.1.min(wh), wh);
    let left = if cx.is_finite() { cx } else { wx + 24.0 };
    let top = if cy.is_finite() { cy } else { wy + 24.0 };
    (
        left.clamp(wx, (wx + ww - width).max(wx)),
        top.clamp(wy, (wy + wh - height).max(wy)),
        width,
        height,
    )
}

/// `NormalizeKind`: the ids a caller may use for a previewable page.
pub fn normalize_kind(kind: &str) -> String {
    match kind.to_lowercase().as_str() {
        "music" => "media".to_owned(),
        "shelf" => "files".to_owned(),
        "note" => "notes".to_owned(),
        other => other.to_owned(),
    }
}

/// `NameFor`: the window title of a page.
pub fn display_name(kind: &str) -> String {
    match kind {
        "media" => "Music".to_owned(),
        "files" => "Shelf".to_owned(),
        "notes" => "Notes".to_owned(),
        other => {
            let mut chars = other.chars();
            chars
                .next()
                .map(|c| c.to_uppercase().collect::<String>() + chars.as_str())
                .unwrap_or_default()
        }
    }
}

/// `DetachedPreviewManager.CanOpen`.
pub fn can_detach(settings: &Settings, kind: &str) -> bool {
    let kind = normalize_kind(kind);
    if !settings.detachable_previews {
        return false;
    }
    if matches!(kind.as_str(), "calendar" | "notes" | "files")
        && !policy::hover_available(settings, &kind)
    {
        return false;
    }
    matches!(
        kind.as_str(),
        "stopwatch" | "media" | "timer" | "focus" | "calendar" | "notes" | "files" | "system"
    ) && (settings
        .hover_views
        .iter()
        .any(|v| v.id == kind && v.enabled)
        || settings.widgets.iter().any(|w| w.id == kind && w.enabled)
        || (kind == "media"
            && settings
                .widgets
                .iter()
                .any(|w| w.id == "volume" && w.enabled)))
}

/// What a detached view asks of its window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetachedRequest {
    /// Move the window by this many pixels (the title bar is being dragged).
    Move { dx: i32, dy: i32 },
    /// Resize to this client size in pixels (the corner or an edge is being dragged); the host
    /// clamps it and reports the result with [`DetachedView::set_size`].
    Resize { width: u32, height: u32 },
    /// A drag ended: persist the geometry.
    Settled,
    /// The close button was pressed.
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    Title,
    Close,
    Open,
    Corner,
    EdgeRight,
    EdgeBottom,
}

struct Grab {
    part: Part,
    press: (f32, f32),
    size: (u32, u32),
}

/// One floating preview window's content.
pub struct DetachedView {
    kind: String,
    page: Box<dyn Page>,
    scale: f32,
    size: (u32, u32),
    origin: (i32, i32),
    visible: bool,
    scroll: f32,
    overflow: f32,
    theme: Theme,
    settings_version: u64,
    revision: u64,
    hits: Vec<(Part, Rect)>,
    pending_input: Option<InputState>,
    last_input: InputState,
    grab: Option<Grab>,
    pressed: Option<Part>,
    hover: Option<Part>,
    requests: Vec<DetachedRequest>,
    outbox: Vec<Command>,
    dirty: bool,
    animating: bool,
}

impl DetachedView {
    /// A view of the page `kind` (any spelling `normalize_kind` knows), `size` in pixels.
    pub fn new(kind: &str, scale: f32, size: (u32, u32), snaps: &Snapshots) -> DetachedView {
        let kind = normalize_kind(kind);
        DetachedView {
            page: pages::build(&kind),
            kind,
            scale: scale.clamp(0.5, 8.0),
            size,
            origin: (0, 0),
            visible: true,
            scroll: 0.0,
            overflow: 0.0,
            theme: Theme::from_settings(&snaps.settings),
            settings_version: snaps.settings.version(),
            revision: snaps.revision(),
            hits: Vec::new(),
            pending_input: None,
            last_input: InputState::default(),
            grab: None,
            pressed: None,
            hover: None,
            requests: Vec::new(),
            outbox: Vec::new(),
            dirty: true,
            animating: false,
        }
    }

    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// The window title (`Title = "NabyScape · " + name`).
    pub fn title(&self) -> String {
        format!("NabyScape \u{b7} {}", display_name(&self.kind))
    }

    /// The host moved the window to `origin` (screen pixels).
    pub fn set_origin(&mut self, origin: (i32, i32)) {
        self.origin = origin;
    }

    /// The host's final client size after clamping (pixels).
    pub fn set_size(&mut self, size: (u32, u32)) {
        if self.size != size {
            self.size = size;
            self.dirty = true;
        }
    }

    /// The window moved to a monitor with another DPI scale.
    pub fn set_scale(&mut self, scale: f32) {
        let scale = scale.clamp(0.5, 8.0);
        if (self.scale - scale).abs() > f32::EPSILON {
            self.scale = scale;
            self.dirty = true;
        }
    }

    /// The window shows (true) or is hidden / minimized: hidden views drop their caches.
    pub fn set_visible(&mut self, visible: bool) {
        if self.visible != visible {
            self.visible = visible;
            if !visible {
                self.page.on_hide();
                self.pending_input = None;
                self.grab = None;
            }
            self.dirty = true;
        }
    }

    /// Requests made since the last call (window moves, resizes, close).
    pub fn take_requests(&mut self) -> Vec<DetachedRequest> {
        std::mem::take(&mut self.requests)
    }

    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// Scale-aware layout rectangles in canvas pixels.
    fn frame_rects(&self) -> FrameRects {
        let s = self.scale;
        let (w, h) = (self.size.0 as f32, self.size.1 as f32);
        let outer = 10.0 * s;
        let header = Rect::new(
            outer + 8.0 * s,
            outer + 4.0 * s,
            w - 2.0 * outer - 10.0 * s,
            28.0 * s,
        );
        let close = Rect::new(header.right() - 28.0 * s, header.y, 28.0 * s, 28.0 * s);
        let content = Rect::new(
            outer + 6.0 * s,
            header.bottom() + 10.0 * s,
            (w - 2.0 * (outer + 6.0 * s)).max(1.0),
            (h - header.bottom() - 10.0 * s - outer - 6.0 * s).max(1.0),
        );
        FrameRects {
            header,
            close,
            content,
        }
    }
}

struct FrameRects {
    header: Rect,
    close: Rect,
    content: Rect,
}

impl View for DetachedView {
    fn input(&mut self, input: &InputState, snaps: &Snapshots, _out: &mut Vec<Command>) -> bool {
        let _ = snaps;
        let mut changed = false;
        self.last_input = input.without_events();
        let pointer = input.pointer;
        let hit = pointer.and_then(|p| {
            self.hits
                .iter()
                .rev()
                .find(|(_, r)| r.contains_pt(p))
                .map(|(part, _)| *part)
        });
        if hit != self.hover {
            self.hover = hit;
            changed = true;
        }
        if input.pressed {
            if let (Some(part), Some(p)) = (hit, pointer) {
                match part {
                    Part::Title | Part::Corner | Part::EdgeRight | Part::EdgeBottom => {
                        self.grab = Some(Grab {
                            part,
                            press: p,
                            size: self.size,
                        });
                    }
                    Part::Close | Part::Open => self.pressed = Some(part),
                }
                changed = true;
            }
        }
        if let (Some(grab), Some(p), true) = (&self.grab, pointer, input.down) {
            let (dx, dy) = (p.0 - grab.press.0, p.1 - grab.press.1);
            match grab.part {
                Part::Title => {
                    if dx.abs() >= 1.0 || dy.abs() >= 1.0 {
                        self.requests.push(DetachedRequest::Move {
                            dx: dx.round() as i32,
                            dy: dy.round() as i32,
                        });
                    }
                }
                part => {
                    let w = if part == Part::EdgeBottom { 0.0 } else { dx };
                    let h = if part == Part::EdgeRight { 0.0 } else { dy };
                    let width = (grab.size.0 as f32 + w).max(1.0) as u32;
                    let height = (grab.size.1 as f32 + h).max(1.0) as u32;
                    if (width, height) != self.size {
                        self.requests
                            .push(DetachedRequest::Resize { width, height });
                    }
                }
            }
            changed = true;
        }
        if input.released {
            if self.grab.take().is_some() {
                self.requests.push(DetachedRequest::Settled);
                changed = true;
            }
            if let Some(part) = self.pressed.take() {
                if hit == Some(part) {
                    match part {
                        Part::Close => self.requests.push(DetachedRequest::Close),
                        Part::Open => self.outbox.push(Command::NavigatePage(self.kind.clone())),
                        _ => {}
                    }
                }
                changed = true;
            }
        }
        // The wheel scrolls a page taller than the window.
        let (_, wy) = input.wheel;
        if wy != 0.0 && self.overflow > 0.0 && !self.page.captures_pointer() {
            let over = pointer.is_some_and(|p| self.frame_rects().content.contains_pt(p));
            if over {
                self.scroll = (self.scroll - wy).clamp(0.0, self.overflow);
                changed = true;
            }
        }
        // Everything else belongs to the page; a press on the chrome never reaches it.
        let content = self.frame_rects().content;
        let inside = pointer.is_some_and(|p| content.contains_pt(p));
        if inside
            || self.page.captures_pointer()
            || !input.keys.is_empty()
            || !input.text.is_empty()
            || input.paste.is_some()
            || input.released
        {
            let mut forwarded = input.clone();
            if !inside && !self.page.captures_pointer() {
                forwarded.pointer = None;
            }
            self.pending_input = Some(forwarded);
            changed = true;
        }
        if changed {
            self.dirty = true;
        }
        changed
    }

    fn signal(&mut self, signal: Signal, _snaps: &Snapshots, _out: &mut Vec<Command>) -> bool {
        match signal {
            Signal::FocusLost => {
                self.pending_input = None;
                false
            }
            _ => false,
        }
    }

    fn tick(&mut self, snaps: &Snapshots, _out: &mut Vec<Command>) -> bool {
        if snaps.settings.version() != self.settings_version {
            self.settings_version = snaps.settings.version();
            self.theme = Theme::from_settings(&snaps.settings);
            self.dirty = true;
        }
        if snaps.revision() != self.revision {
            self.revision = snaps.revision();
            self.dirty = true;
        }
        if self.animating {
            self.dirty = true;
        }
        let was = self.dirty;
        if !self.dirty && self.visible {
            if let Some(at) = self.page.next_frame(snaps.now) {
                if at <= snaps.now {
                    self.dirty = true;
                }
            }
        }
        was || self.dirty
    }

    fn paint(&mut self, canvas: &mut Canvas, fonts: &Fonts, snaps: &Snapshots) {
        self.dirty = false;
        if !self.visible {
            return;
        }
        let s = self.scale;
        let k = self.theme.text_scale;
        let theme = self.theme.clone();
        let (w, h) = (self.size.0 as f32, self.size.1 as f32);
        // Surface.
        let body = Rect::new(0.0, 0.0, w, h);
        canvas.fill_rrect(body, 16.0 * s, theme.background);
        canvas.stroke_round_rect(body.inset(0.5), 16.0 * s, theme.control_border, 1.0);
        self.hits.clear();
        let rects = self.frame_rects();
        // Title bar.
        self.hits.push((Part::Title, rects.header));
        let title = style(13.0 * s * k, Weight::Semibold, theme.text);
        canvas.text(
            fonts,
            rects.header.x,
            rects.header.y + (rects.header.h - fonts.line_height(&title)) / 2.0,
            &display_name(&self.kind),
            &title,
            Some(rects.close.x - rects.header.x - 8.0 * s),
        );
        self.hits.push((Part::Close, rects.close));
        if self.hover == Some(Part::Close) || self.pressed == Some(Part::Close) {
            canvas.fill_rrect(rects.close, 8.0 * s, theme.hover_fill);
        }
        canvas.icon(
            Icon::X,
            rects.close.inset(7.5 * s),
            theme.text,
            DEFAULT_STROKE,
        );
        // Page, scrolled when taller than the window.
        let ctx = PageCtx {
            fonts,
            snaps,
            theme: &theme,
            scale: s,
        };
        let content = rects.content;
        let page_h = self.page.height(&ctx, content.w);
        let button_h = 34.0 * s;
        let total = page_h + 12.0 * s + button_h;
        self.overflow = (total - content.h).max(0.0);
        self.scroll = self.scroll.clamp(0.0, self.overflow);
        let input = self
            .pending_input
            .take()
            .unwrap_or_else(|| self.last_input.without_events());
        let page_rect = Rect::new(content.x, content.y - self.scroll, content.w, page_h);
        canvas.push_clip_rect(content);
        let mut out = Vec::new();
        self.animating = self.page.frame(canvas, &ctx, page_rect, &input, &mut out);
        self.outbox.append(&mut out);
        // "Open <name>".
        let open = Rect::new(
            content.x,
            page_rect.bottom() + 12.0 * s,
            content.w,
            button_h,
        );
        if open.intersect(&content).is_some() {
            let label = format!("Open {}", display_name(&self.kind));
            canvas.fill_rrect(open, theme.radius, theme.control_fill);
            if self.hover == Some(Part::Open) || self.pressed == Some(Part::Open) {
                canvas.fill_rrect(open, theme.radius, theme.hover_fill);
            }
            let st = style(13.0 * s * k, Weight::Regular, theme.control_text);
            text_centered(canvas, fonts, open, &label, &st);
            if let Some(visible) = open.intersect(&content) {
                self.hits.push((Part::Open, visible));
            }
        }
        canvas.pop_clip();
        if self.overflow > 0.5 {
            let track = Rect::new(content.right() - 3.0 * s, content.y, 2.0 * s, content.h);
            let thumb_h = (track.h * content.h / total).max(16.0 * s);
            let at = track.y + (track.h - thumb_h) * (self.scroll / self.overflow);
            canvas.fill_rrect(
                Rect::new(track.x, at, track.w, thumb_h),
                1.0 * s,
                theme.text.with_alpha(60),
            );
        }
        // Resize handles: the right and bottom edges and the corner grip.
        let edge = 6.0 * s;
        self.hits.push((
            Part::EdgeRight,
            Rect::new(
                w - edge,
                rects.header.bottom(),
                edge,
                (h - rects.header.bottom() - edge).max(1.0),
            ),
        ));
        self.hits.push((
            Part::EdgeBottom,
            Rect::new(edge, h - edge, (w - 2.0 * edge).max(1.0), edge),
        ));
        let corner = Rect::new(w - 18.0 * s, h - 18.0 * s, 18.0 * s, 18.0 * s);
        self.hits.push((Part::Corner, corner));
        let grip = theme
            .text_dim
            .with_alpha(if self.hover == Some(Part::Corner) {
                220
            } else {
                110
            });
        for i in 0..3 {
            let o = (4.0 + 3.5 * i as f32) * s;
            canvas.fill_rrect(
                Rect::new(
                    corner.right() - o - 1.5 * s,
                    corner.bottom() - 4.0 * s,
                    1.5 * s,
                    1.5 * s,
                ),
                0.75 * s,
                grip,
            );
            canvas.fill_rrect(
                Rect::new(
                    corner.right() - 4.0 * s,
                    corner.bottom() - o - 1.5 * s,
                    1.5 * s,
                    1.5 * s,
                ),
                0.75 * s,
                grip,
            );
        }
    }

    fn size(&self, _snaps: &Snapshots) -> (u32, u32) {
        if self.visible {
            self.size
        } else {
            (0, 0)
        }
    }

    fn placement(&self, _snaps: &Snapshots) -> Placement {
        Placement {
            origin: self.origin,
            size: self.size,
        }
    }

    fn next_frame(&self, now: f64) -> Option<f64> {
        if !self.visible {
            return None;
        }
        let mut next = self.page.next_frame(now);
        if self.dirty || self.animating {
            next = Some(next.map_or(now, |n: f64| n.min(now)));
        }
        next
    }

    fn demand(&self, _snaps: &Snapshots) -> Demand {
        if self.visible {
            self.page.demand()
        } else {
            Demand::default()
        }
    }

    fn wants_focus(&self) -> bool {
        self.visible && self.page.captures_pointer() && self.grab.is_none()
    }

    fn interaction_active(&self) -> bool {
        self.grab.is_some() || self.pressed.is_some()
    }

    fn drain(&mut self, out: &mut Vec<Command>) {
        out.append(&mut self.outbox);
    }

    fn on_hide(&mut self) {
        self.set_visible(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use naby_core::settings::WidgetSetting;
    use naby_render::snapshot;

    fn settings() -> Settings {
        Settings {
            widgets: vec![
                WidgetSetting::new("media", true),
                WidgetSetting::new("timer", true),
                WidgetSetting::new("notes", true),
                WidgetSetting::new("tasks", true),
                WidgetSetting::new("calendar", true),
                WidgetSetting::new("files", true),
            ],
            detachable_previews: true,
            ..Settings::default()
        }
    }

    #[test]
    fn kinds_and_names_match_the_csharp_manager() {
        assert_eq!(normalize_kind("Music"), "media");
        assert_eq!(normalize_kind("shelf"), "files");
        assert_eq!(normalize_kind("NOTE"), "notes");
        assert_eq!(normalize_kind("timer"), "timer");
        assert_eq!(display_name("media"), "Music");
        assert_eq!(display_name("files"), "Shelf");
        assert_eq!(display_name("timer"), "Timer");
    }

    #[test]
    fn only_enabled_previewable_pages_can_detach() {
        let mut s = settings();
        assert!(can_detach(&s, "media"));
        assert!(can_detach(&s, "Music"));
        assert!(can_detach(&s, "notes"));
        assert!(!can_detach(&s, "weather"), "not a previewable page");
        assert!(!can_detach(&s, "stopwatch"), "not enabled");
        s.detachable_previews = false;
        assert!(!can_detach(&s, "media"));
        let mut s = settings();
        s.widgets.retain(|w| w.id != "media");
        s.widgets.push(WidgetSetting::new("volume", true));
        assert!(
            can_detach(&s, "media"),
            "the volume widget enables the music page"
        );
    }

    #[test]
    fn geometry_is_clamped_into_the_work_area() {
        let work = (0.0, 0.0, 1920.0, 1040.0);
        assert_eq!(
            clamp_geometry((-50.0, 2000.0, 350.0, 330.0), work),
            (0.0, 710.0, 350.0, 330.0)
        );
        assert_eq!(
            clamp_geometry((10.0, 10.0, 100.0, 50.0), work),
            (10.0, 10.0, 280.0, 220.0),
            "minimum size"
        );
        assert_eq!(
            clamp_geometry((f64::NAN, f64::NAN, f64::NAN, f64::NAN), work),
            (24.0, 24.0, 350.0, 330.0)
        );
        assert_eq!(
            clamp_geometry((0.0, 0.0, 5000.0, 5000.0), work),
            (0.0, 0.0, 1920.0, 1040.0)
        );
        let small = (0.0, 0.0, 200.0, 150.0);
        assert_eq!(
            clamp_geometry((0.0, 0.0, 350.0, 330.0), small),
            (0.0, 0.0, 200.0, 150.0)
        );
    }

    #[test]
    fn stored_geometry_uses_the_csharp_keys_and_repairs_bad_values() {
        let json = serde_json::to_string(&DetachedGeometry::default()).unwrap();
        assert_eq!(
            json,
            r#"{"Left":80.0,"Top":80.0,"Width":350.0,"Height":330.0}"#
        );
        let bad: DetachedGeometry = serde_json::from_str(r#"{"Left":5,"Width":-4}"#).unwrap();
        assert_eq!(bad.sanitized(), (5.0, 80.0, 350.0, 330.0));
    }

    struct Rig {
        view: DetachedView,
        snaps: Snapshots,
        fonts: Fonts,
        canvas: Canvas,
        out: Vec<Command>,
    }

    impl Rig {
        fn new(kind: &str) -> Option<Rig> {
            let fonts = snapshot::test_fonts()?;
            let snaps = Snapshots::new(settings(), 10.0);
            let view = DetachedView::new(kind, 1.0, (350, 330), &snaps);
            Some(Rig {
                view,
                snaps,
                fonts,
                canvas: Canvas::new(1, 1),
                out: Vec::new(),
            })
        }

        fn paint(&mut self) {
            let (w, h) = self.view.size(&self.snaps);
            self.canvas.resize(w, h);
            self.canvas.clear(naby_core::color::Rgba::TRANSPARENT);
            self.view.paint(&mut self.canvas, &self.fonts, &self.snaps);
            self.view.drain(&mut self.out);
        }

        fn input(&mut self, input: InputState) -> Vec<DetachedRequest> {
            let mut input = input;
            input.time = self.snaps.now;
            self.view.input(&input, &self.snaps, &mut self.out);
            let requests = self.view.take_requests();
            self.paint();
            requests
        }

        fn click(&mut self, p: (f32, f32)) -> Vec<DetachedRequest> {
            self.input(InputState::at(p.0, p.1));
            self.input(InputState::press(p.0, p.1));
            self.input(InputState::release(p.0, p.1))
        }
    }

    #[test]
    fn the_close_button_requests_closing() {
        let Some(mut r) = Rig::new("notes") else {
            return;
        };
        r.paint();
        r.canvas
            .save_png(&snapshot::snapshot_dir().join("detached-notes.png"))
            .ok();
        let close = r.view.frame_rects().close.center();
        assert_eq!(r.click(close), vec![DetachedRequest::Close]);
    }

    #[test]
    fn dragging_the_title_moves_and_the_corner_resizes() {
        let Some(mut r) = Rig::new("notes") else {
            return;
        };
        r.paint();
        let title = (40.0, r.view.frame_rects().header.center().1);
        r.input(InputState::at(title.0, title.1));
        r.input(InputState::press(title.0, title.1));
        let moved = r.input(InputState::drag(title.0 + 30.0, title.1 + 12.0));
        assert_eq!(moved, vec![DetachedRequest::Move { dx: 30, dy: 12 }]);
        let settled = r.input(InputState::release(title.0 + 30.0, title.1 + 12.0));
        assert_eq!(settled, vec![DetachedRequest::Settled]);
        assert!(!r.view.interaction_active());
        // The corner grip.
        let corner = (348.0, 328.0);
        r.input(InputState::at(corner.0, corner.1));
        r.input(InputState::press(corner.0, corner.1));
        let resized = r.input(InputState::drag(corner.0 + 50.0, corner.1 + 20.0));
        assert_eq!(
            resized,
            vec![DetachedRequest::Resize {
                width: 400,
                height: 350
            }]
        );
        assert!(r.view.interaction_active());
        r.view.set_size((400, 350));
        r.input(InputState::release(corner.0 + 50.0, corner.1 + 20.0));
        assert_eq!(r.view.size(&r.snaps), (400, 350));
    }

    #[test]
    fn the_open_button_navigates_the_dock_to_the_page() {
        let Some(mut r) = Rig::new("calendar") else {
            return;
        };
        r.paint();
        let open = r
            .view
            .hits
            .iter()
            .find(|(p, _)| *p == Part::Open)
            .map(|(_, rect)| rect.center())
            .expect("open button");
        r.click(open);
        assert!(r.out.contains(&Command::NavigatePage("calendar".into())));
    }

    #[test]
    fn page_widgets_work_inside_the_window_and_hiding_frees_the_page() {
        let Some(mut r) = Rig::new("tasks") else {
            return;
        };
        r.snaps.productivity.borrow_mut().add_task("Water plants");
        r.paint();
        r.canvas
            .save_png(&snapshot::snapshot_dir().join("detached-tasks.png"))
            .ok();
        // Click the first task's check ring (inside the content area).
        let content = r.view.frame_rects().content;
        let ring = (content.x + 14.0, content.y + 60.0);
        r.click(ring);
        let done = r.snaps.productivity.borrow().tasks()[0].done;
        assert!(done, "the tasks page toggled inside the detached window");
        r.view.set_visible(false);
        assert_eq!(r.view.size(&r.snaps), (0, 0));
        assert!(r.view.next_frame(10.0).is_none());
        r.view.set_visible(true);
        assert!(r.view.size(&r.snaps).0 > 0);
    }

    #[test]
    fn a_title_reflects_the_page() {
        let Some(r) = Rig::new("music") else {
            return;
        };
        assert_eq!(r.view.kind(), "media");
        assert_eq!(r.view.title(), "NabyScape \u{b7} Music");
    }

    #[test]
    fn the_hover_footer_emits_the_detach_command_for_the_shown_page() {
        use super::super::tests::{quiet_settings, Rig as DockRig};
        use super::super::util::Hot;
        let mut r = DockRig::new(quiet_settings());
        r.playing_media("Blue Hour", "The Hours");
        r.paint();
        r.hover_center();
        r.advance(1.0);
        r.settle();
        r.paint();
        let button = r
            .overlay
            .hot
            .rect(Hot::PeekDetach)
            .expect("the detach button shows while detachable previews are on");
        r.click(button.center());
        let sent = r.commands();
        let detach = sent
            .iter()
            .find_map(|c| match c {
                Command::DetachPreview { page, at, dragging } => {
                    Some((page.clone(), *at, *dragging))
                }
                _ => None,
            })
            .expect("DetachPreview was sent");
        assert_eq!(detach.0, "media");
        assert!(!detach.2);
        // With the preference off the button is not offered.
        let mut off = quiet_settings();
        off.detachable_previews = false;
        let mut r = DockRig::new(off);
        r.playing_media("Blue Hour", "The Hours");
        r.paint();
        r.hover_center();
        r.advance(1.0);
        r.settle();
        r.paint();
        assert!(r.overlay.hot.rect(Hot::PeekDetach).is_none());
    }
}
