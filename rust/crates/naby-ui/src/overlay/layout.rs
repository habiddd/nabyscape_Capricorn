//! Window placement, scheduling (`next_frame`), service demand and the top-level paint order.

use super::util::Hot;
use super::Overlay;
use crate::host::{Demand, Placement, Snapshots};
use naby_core::media::policy::ClosedMediaContent;
use naby_core::motion::{place_host, IslandGeometry, RectI};
use naby_render::{Canvas, Fonts, Rect};

/// Where everything sits, in physical pixels.
pub(crate) struct Layout {
    /// Canvas (host window) size.
    pub size: (u32, u32),
    /// Window top-left on the screen.
    pub window: (i32, i32),
    /// Top-left of the island contour inside the canvas (tuck slide included).
    pub origin: (f32, f32),
}

impl Overlay {
    pub(crate) fn hidden(&self) -> bool {
        self.user_hidden || self.env_hidden
    }

    pub(crate) fn layout(&self, snaps: &Snapshots) -> Layout {
        if self.hidden() {
            return Layout {
                size: (0, 0),
                window: (0, 0),
                origin: (0.0, 0.0),
            };
        }
        let s = snaps.settings.clone();
        let scale = f64::from(self.scale);
        let area = self.area(snaps);
        let area_dip = self.area_dip(snaps);
        let content = (self.panel.content_height > 0.0).then_some(self.panel.content_height);
        let widest_dip = self.surf.0.max(self.target_size.0).max(s.width);
        let host_w = IslandGeometry::host_width_px(&s, area_dip, scale, content)
            .max((widest_dip * scale).ceil() as i32)
            .max(1);
        let host_h = ((self.host_h.max(self.surf.1) * scale).ceil() as i32).max(1);
        let paint_w = (self.surf.0 * scale).ceil() as i32;
        let paint_h = (self.surf.1 * scale).ceil() as i32;
        let window = place_host(
            &s,
            RectI::from_f(area),
            scale,
            host_w,
            host_h.min(paint_h.max(1)),
            self.tuck_requested(&s),
            paint_w,
        );
        let (tx, ty) = self.tuck.offset();
        let ox = (host_w as f32 - self.surf.0 as f32 * self.scale) / 2.0 + tx as f32 * self.scale;
        let oy = ty as f32 * self.scale;
        Layout {
            size: (host_w as u32, host_h as u32),
            window,
            origin: (ox, oy),
        }
    }

    /// Canvas position of the contour's top-left (what hit testing uses).
    pub(crate) fn surface_origin(&self, snaps: &Snapshots) -> (f32, f32) {
        self.layout(snaps).origin
    }

    pub(crate) fn host_placement(&self, snaps: &Snapshots) -> Placement {
        let l = self.layout(snaps);
        Placement {
            origin: l.window,
            size: l.size,
        }
    }

    // ---- scheduling -------------------------------------------------------------------

    pub(crate) fn next_wake(&self, now: f64) -> Option<f64> {
        if self.hidden() {
            return None;
        }
        let mut next: Option<f64> = None;
        let mut add = |t: Option<f64>| {
            if let Some(t) = t {
                next = Some(next.map_or(t, |n| n.min(t)));
            }
        };
        if self.dirty {
            add(Some(now));
        }
        add(self.motion.next_frame(now));
        add(self.tuck.next_frame(now));
        add(self.compact_fade.next_frame(now));
        add(self.peek_fade.next_frame(now));
        add(self.panel_fade.next_frame(now));
        add(self.panel_slide.next_frame(now));
        add(self.page_fade.next_frame(now));
        add(self.page_slide.next_frame(now));
        add(self.peek.offset.next_frame(now));
        add(self.hover.next_frame(now));
        add(self.feedback.next_frame(now));
        add(self.notice.entry.and_then(|e| e.next_frame(now)));
        if self.peek.automatic && self.peeking {
            add(Some(self.activity_until));
        }
        if self.shown_ambient {
            add(self.ambient.next_frame(now));
        }
        // Waveform / countdown ring while music or a countdown animates.
        if self.live_work {
            add(Some(
                now + 1.0 / f64::from(self.cfg.motion.frame_limit.clamp(15, 240)),
            ));
        }
        // Once-a-second countdown text, once-a-minute clock.
        if self.countdown_visible || (self.expanded && self.panel_ticks) {
            add(Some(now + (1.0 - self.wall_frac).max(0.02)));
        } else if self.clock_visible {
            add(Some(
                now + (60.0 - f64::from(self.wall_second) - self.wall_frac).max(0.02),
            ));
        }
        if self.lyric_anim_until > now {
            add(Some(now + 1.0 / 60.0));
        }
        if self.lyrics_ticking {
            add(Some(now + 0.1));
        }
        if snaps_notice_pending(self) {
            add(Some(self.notice_deadline.max(now)));
        }
        if self.expanded {
            for page in self.panel.pages.values() {
                add(page.next_frame(now));
            }
        }
        if self.pending_input_frame() {
            add(Some(now));
        }
        next
    }

    fn pending_input_frame(&self) -> bool {
        self.panel.pending_input.is_some()
    }

    /// Services needed right now.
    pub(crate) fn current_demand(&self, snaps: &Snapshots) -> Demand {
        let s = &snaps.settings;
        let mut demand = Demand::default();
        if self.hidden() || self.tuck.is_tucked() {
            return demand;
        }
        let playing = snaps.media.playing && s.live_activity && s.widget_enabled("media");
        let wave_shown = self.closed_wave_visible
            || self.closed_icon_wave
            || (self.expanded && playing)
            || (self.peeking && self.peek.id == "media" && playing);
        if playing && wave_shown && !s.reduce_motion {
            if s.waveform_source == "Spectrum" {
                demand.spectrum = true;
            } else {
                demand.peak = true;
            }
        }
        let ambient_reactive = self.ambient.visible()
            && playing
            && naby_core::media::AmbientConfig::default().strength >= 0.0
            && s.ambient_audio_response != "Off";
        if ambient_reactive {
            demand.peak = true;
        }
        if ClosedMediaContent::parse(&s.closed_media_content) == ClosedMediaContent::Lyrics
            && self.wants_closed_lyrics
        {
            demand.lyrics = true;
        }
        if self.expanded {
            for page in self.panel.pages.values() {
                demand = demand.merge(page.demand());
            }
        }
        if self.peeking && self.peek.id == "system" {
            demand.system = true;
        }
        if self.peeking && self.peek.id == "weather" {
            demand.weather = true;
        }
        demand
    }

    // ---- painting ---------------------------------------------------------------------

    pub(crate) fn paint_all(&mut self, canvas: &mut Canvas, fonts: &Fonts, snaps: &Snapshots) {
        if self.hidden() {
            return;
        }
        self.ensure_contour();
        let layout = self.layout(snaps);
        let origin = layout.origin;
        self.last_paint_origin = origin;
        self.hot.clear();
        self.paint_surface(canvas, origin, snaps);
        let s = self.scale;
        let rect = Rect::new(
            origin.0,
            origin.1,
            self.surf.0 as f32 * s,
            self.surf.1 as f32 * s,
        );
        // Content is limited to the (possibly tucked) contour and, on side docks, the monitor.
        if let Some(contour) = self.contour_ref() {
            let contour = contour.clone();
            canvas.push_clip_contour(&contour, origin.0, origin.1, s);
        } else {
            canvas.push_clip_rect(rect);
        }
        let now = self.now;
        let closed_alpha = if self.expanded {
            0.0
        } else {
            self.compact_fade.value(now) as f32
        };
        if closed_alpha > 0.001 && !self.notice_showing {
            self.paint_closed(canvas, fonts, snaps, rect, closed_alpha);
        }
        let peek_alpha = self.peek_fade.value(now) as f32;
        if peek_alpha > 0.001
            && !self.notice_showing
            && (self.peeking || !self.peek_fade.finished(now))
        {
            self.paint_peek(canvas, fonts, snaps, rect, peek_alpha);
        }
        if self.notice_showing {
            self.paint_notice(canvas, fonts, snaps, rect);
        }
        let panel_alpha = self.panel_fade.value(now) as f32;
        if (self.expanded || panel_alpha > 0.001) && panel_alpha > 0.001 {
            self.paint_panel(canvas, fonts, snaps, rect, panel_alpha);
        }
        canvas.pop_clip();
        if !self.expanded && !self.peeking && !self.hot_has_surface() {
            self.hot.add(Hot::Surface, rect);
        }
        self.paint_notice_edge(canvas, snaps, rect);
    }

    fn hot_has_surface(&self) -> bool {
        self.hot.rect(Hot::Surface).is_some()
    }
}

fn snaps_notice_pending(o: &Overlay) -> bool {
    o.notice_pending && !o.notice_dismiss_sent
}
