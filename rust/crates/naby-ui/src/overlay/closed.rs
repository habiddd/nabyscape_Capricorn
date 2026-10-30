//! The closed pill: what it shows and how it is drawn (C# `OverlayWindow` header grid,
//! `IdleDock.cs`, `ClosedLyrics.cs`, `ClosedMediaIcon.cs`, the compact clock, awake mark and the
//! feedback pill), plus the glow/waveform sampling that runs while music plays.

use super::util::{fade, style, until_boundary};
use super::Overlay;
use crate::host::Snapshots;
use chrono::Timelike;
use naby_core::color::Rgba;
use naby_core::media::lrc::can_use;
use naby_core::media::policy::{
    closed_media_active, closed_media_view, ClosedDockContext, ClosedMediaContent, ClosedMediaIcon,
};
use naby_core::media::{
    AmbientConfig, ClosedLine, ClosedLyrics, ClosedLyricsConfig, LyricsAnimation,
};
use naby_core::motion::Ease;
use naby_core::productivity::{active_countdown, countdown_summary, ActiveCountdown};
use naby_core::settings::Settings;
use naby_render::{Canvas, Fonts, Icon, Image, Rect, Weight};
use std::sync::Arc;

/// What the pill shows right now, computed from snapshots.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ClosedPlan {
    pub idle: bool,
    pub cover: bool,
    pub waveform: bool,
    pub leading: bool,
    pub countdown: Option<ActiveCountdown>,
    /// Summary text for every mode except lyrics (computed at paint time with the fonts).
    pub text: String,
    pub lyrics: bool,
    pub bold: bool,
    pub clock: Option<String>,
    pub awake: bool,
    pub feedback: bool,
    pub notice: bool,
    pub date: bool,
}

impl Overlay {
    /// Recomputes the plan and the scheduling flags. True when the visible plan changed.
    pub(crate) fn refresh_closed(&mut self, snaps: &Snapshots) -> bool {
        let s: &Settings = &snaps.settings;
        let now = snaps.now;
        let wall_utc = snaps.wall.with_timezone(&chrono::Utc);
        self.wall_frac = f64::from(snaps.wall.nanosecond() % 1_000_000_000) / 1e9;
        self.wall_second = snaps.wall.second();
        let content = ClosedMediaContent::parse(&s.closed_media_content);
        let icon = ClosedMediaIcon::parse(&s.closed_media_icon);
        let m = &snaps.media;
        let media_active =
            closed_media_active(content, s.widget_enabled("media"), m.available, m.playing);
        let feedback =
            self.feedback.visible(now) && !self.expanded && !self.peeking && !self.tuck.is_tucked();
        let countdown = active_countdown(
            s.widget_enabled("focus"),
            s.widget_enabled("timer"),
            &snaps.session.focus,
            &snaps.session.timer,
            wall_utc,
        );
        let voice = snaps.assistant.closed_activity();
        let notice = snaps.notices.current.is_some();
        let ctx = ClosedDockContext {
            visible: !self.hidden(),
            user_hidden: self.user_hidden,
            tucked: self.tuck.is_tucked(),
            tuck_requested: self.tuck_requested(s),
            expanded: self.expanded,
            peeking: self.peeking,
            feedback_visible: feedback,
            notification_active: notice,
            voice_activity: voice,
            countdown_active: countdown.is_some(),
        };
        let usable = can_use(m);
        let cfg = ClosedLyricsConfig {
            offset_seconds: s.lyrics_offset,
            animation: LyricsAnimation::parse(&s.lyrics_animation),
        };
        let elapsed = snaps.media_timeline().0;
        let lyric_line = ClosedLyrics::index(&cfg, &snaps.lyrics.lines, usable, elapsed);
        self.wants_closed_lyrics =
            naby_core::media::policy::wants_closed_lyrics(content, media_active, &ctx);
        let view = closed_media_view(
            content,
            icon,
            media_active,
            m.artwork.is_some(),
            lyric_line.is_some(),
            countdown.is_some(),
            &ctx,
        );
        let idle = !feedback
            && !notice
            && !voice
            && !media_active
            && countdown.is_none()
            && !self.tuck_requested(s)
            && !self.editing_dock;
        let text = if feedback {
            self.feedback.text.clone()
        } else if let Some(n) = snaps.notices.current.as_ref() {
            format!("\u{25CF} {}", n.title)
        } else if let Some(c) = countdown {
            countdown_summary(c, &snaps.session.focus, &snaps.session.timer, wall_utc)
        } else if voice {
            snaps.assistant.label.clone()
        } else {
            match (media_active, content) {
                (true, ClosedMediaContent::Title) => m.title.clone(),
                _ => String::new(),
            }
        };
        let lyrics = !feedback
            && !notice
            && countdown.is_none()
            && !voice
            && media_active
            && content == ClosedMediaContent::Lyrics
            && lyric_line.is_some();
        let media_summary =
            !feedback && countdown.is_none() && s.widget_enabled("media") && m.available;
        let clock = (!feedback && s.widget_enabled("clock")).then(|| clock_text(snaps, false));
        let plan = ClosedPlan {
            idle,
            cover: view.cover && !idle,
            waveform: view.waveform && !idle,
            leading: view.activity && !idle,
            countdown,
            text,
            lyrics,
            bold: media_summary && s.bold_media_title,
            clock: clock.clone(),
            awake: !feedback && snaps.awake.active,
            feedback,
            notice,
            date: idle && s.show_date && clock.is_some(),
        };
        self.closed_wave_visible = plan.waveform;
        self.closed_icon_wave = plan.leading && countdown.is_none();
        let visible_closed = !self.expanded && !self.peeking && !self.hidden();
        self.countdown_visible = visible_closed && countdown.is_some() && !self.tuck.is_tucked();
        self.clock_visible = visible_closed && clock.is_some() && !self.tuck.is_tucked();
        self.notice_pending = snaps.notices.current.is_some();
        self.lyrics_ticking = visible_closed && plan.lyrics && m.playing;
        self.panel_ticks = self.expanded && self.panel_needs_seconds(snaps);
        // Waveform / ring sampling.
        let moved = self.sample_waves(snaps, &plan);
        let changed = plan != self.plan;
        self.plan = plan;
        changed || moved
    }

    /// Advances the closed-dock bars and the glow level. True while something moved.
    fn sample_waves(&mut self, snaps: &Snapshots, plan: &ClosedPlan) -> bool {
        let s = &snaps.settings;
        let now = snaps.now;
        let dt = (now - self.closed.last_sample).clamp(0.001, 0.5);
        let playing = snaps.media.playing && !snaps.levels.meter_muted;
        let animate_audio =
            playing && s.live_activity && !s.reduce_motion && self.cfg.motion.enabled;
        let visible = !self.hidden() && !self.tuck.is_tucked();
        let peak = if animate_audio && visible {
            f64::from(snaps.levels.peak)
        } else {
            0.0
        };
        let spectrum =
            (animate_audio && s.waveform_source == "Spectrum" && snaps.levels.spectrum_ready)
                .then_some(&snaps.levels.bands);
        let advance = snaps.levels.sequence != self.closed.last_sequence;
        let decorative = s.waveform_idle_motion && animate_audio;
        let seconds = now;
        let wall = snaps.wall.with_timezone(&chrono::Utc);
        let mut moving = false;
        let ring = plan.countdown.map(|c| match c {
            ActiveCountdown::Focus => (
                snaps.session.focus.countdown.progress(wall),
                true,
                snaps.session.focus.running(),
            ),
            ActiveCountdown::Timer => (
                snaps.session.timer.progress(wall),
                false,
                snaps.session.timer.running(),
            ),
        });
        if visible && !self.expanded && !self.peeking {
            if plan.waveform {
                self.closed.wave.sample(
                    peak,
                    dt,
                    0.0,
                    false,
                    false,
                    animate_audio,
                    seconds,
                    advance,
                    decorative,
                    spectrum,
                    s.waveform_sensitivity,
                );
                moving |= animate_audio;
            }
            if plan.leading {
                match ring {
                    Some((fraction, focus, running)) => {
                        self.closed.leading.sample(
                            0.0, dt, fraction, true, focus, running, seconds, advance, false, None,
                            1.0,
                        );
                        moving |= running && focus;
                    }
                    None => {
                        self.closed.leading.sample(
                            peak,
                            dt,
                            0.0,
                            false,
                            false,
                            animate_audio,
                            seconds,
                            advance,
                            decorative,
                            spectrum,
                            s.waveform_sensitivity,
                        );
                        moving |= animate_audio;
                    }
                }
            }
        } else {
            if self.closed.wave.moving() {
                self.closed.wave.rest(0.0, false, false);
            }
            if self.closed.leading.moving() {
                self.closed.leading.rest(0.0, false, false);
            }
        }
        self.closed.last_sequence = snaps.levels.sequence;
        self.closed.last_sample = now;
        // Peek and panel waves share the sampling cadence.
        let wave_wanted = animate_audio && visible && (self.peeking || self.expanded);
        if wave_wanted {
            self.closed.peek_wave.sample(
                peak,
                dt,
                0.0,
                false,
                false,
                true,
                seconds,
                advance,
                decorative,
                spectrum,
                s.waveform_sensitivity,
            );
            moving = true;
        } else if self.closed.peek_wave.moving() {
            self.closed.peek_wave.rest(0.0, false, false);
        }
        self.live_work = moving;
        moving
    }

    /// Cover tint, glow and the audio-reactive glow gain (C# `UpdateAmbientMotion`).
    pub(crate) fn refresh_ambient(&mut self, snaps: &Snapshots) {
        let s = &snaps.settings;
        let now = snaps.now;
        // Cover palette follows the artwork identity.
        let art_id = snaps
            .media
            .artwork
            .as_ref()
            .map_or(0, |a| Arc::as_ptr(a) as *const u8 as usize);
        if art_id != self.palette_key {
            self.palette_key = art_id;
            self.palette = snaps
                .media
                .artwork
                .as_ref()
                .and_then(|bytes| Image::decode(bytes, 32, 32))
                .map_or(naby_core::media::CoverPalette::NONE, |img| {
                    naby_core::media::palette::extract(
                        img.data(),
                        img.width() as usize,
                        img.height() as usize,
                    )
                });
        }
        let cfg = AmbientConfig {
            cover_ambient: s.cover_ambient,
            closed_ambient: s.closed_ambient,
            strength: s.ambient_strength,
            scope: naby_core::media::ambient::AmbientScope::parse(&s.ambient_scope),
            widgets: s.ambient_widgets.clone(),
            color_source: naby_core::media::ambient::AmbientColorSource::parse(
                &s.ambient_color_source,
            ),
            single_color: s.ambient_palette == "Single color",
            custom_color: s.ambient_custom_color.clone(),
            secondary_color: s.ambient_secondary_color.clone(),
            accent: s.accent.clone(),
        };
        let includes = |id: &str| cfg.includes(id, false);
        let tucked = self.tuck.is_tucked();
        let hidden = self.hidden();
        let mut allowed = !self.notice_showing
            && if self.expanded {
                self.panel_widget_ids(snaps).iter().any(|w| {
                    (!s.compact_layout || super::open::page_key(w) == self.panel.active)
                        && includes(w)
                })
            } else if self.peeking {
                includes(&self.peek.id)
            } else {
                includes("closed")
            };
        allowed &= (self.expanded || self.peeking || (s.closed_ambient && !self.plan.idle))
            && !tucked
            && !hidden;
        let quiet = tucked || (!self.expanded && !self.peeking);
        let moving = snaps.media.playing && s.live_activity && !tucked && !hidden;
        self.tint = if !quiet && s.album_accent && self.palette.primary.a > 0 {
            self.palette.primary.with_alpha(24)
        } else {
            Rgba::TRANSPARENT
        };
        let colors = cfg.colors_for(self.palette);
        let look = s.ambient_look();
        let change = self.ambient.set(
            naby_core::motion::AmbientColors {
                primary: colors.primary,
                secondary: colors.secondary,
            },
            if allowed { s.ambient_strength } else { 0.0 },
            s.ambient_spread,
            &s.ambient_motion,
            moving && allowed,
            Some(&look),
            &self.cfg.motion,
            now,
        );
        if change.repaint {
            self.dirty = true;
        }
        self.ambient.set_surface_visible(!hidden && !tucked, now);
        self.shown_ambient = self.ambient.visible() && !hidden;
        // Audio response.
        let response = naby_core::media::ambient::AmbientResponse::parse(&s.ambient_audio_response);
        if allowed && moving && response != naby_core::media::ambient::AmbientResponse::Off {
            let dt = (now - self.closed.last_ambient).clamp(0.001, 0.25);
            let peak = f64::from(snaps.levels.peak);
            let gain = self
                .ambient_env
                .sample(peak, dt, response, s.ambient_sensitivity);
            if snaps.levels.sequence != self.closed.last_ambient_sequence || peak == 0.0 {
                self.ambient.set_audio(gain, now, &self.cfg.motion);
            }
            self.closed.last_ambient_sequence = snaps.levels.sequence;
        } else {
            self.ambient_env.reset();
            self.ambient.reset_audio();
        }
        self.closed.last_ambient = now;
    }

    /// The decoded cover at about `px` pixels (cached per artwork identity).
    pub(crate) fn artwork(&mut self, snaps: &Snapshots) -> Option<Arc<Image>> {
        let bytes = snaps.media.artwork.as_ref()?;
        let id = Arc::as_ptr(bytes) as *const u8 as usize;
        if let Some((cached, image)) = &self.art_image {
            if *cached == id {
                return Some(image.clone());
            }
        }
        let key = format!("art:{id:x}");
        let image = self
            .art
            .get_or_insert_with(&key, || Image::decode(bytes, 256, 256))?;
        self.art_image = Some((id, image.clone()));
        Some(image)
    }

    // ---- painting -----------------------------------------------------------------------

    pub(crate) fn paint_closed(
        &mut self,
        canvas: &mut Canvas,
        fonts: &Fonts,
        snaps: &Snapshots,
        rect: Rect,
        alpha: f32,
    ) {
        let s = self.scale;
        let settings = &snaps.settings;
        let theme_text = self.cfg.theme.text;
        let text_scale = self.cfg.theme.text_scale;
        let plan = std::mem::take(&mut self.plan);
        let closed_w = settings.width.min(self.surf.0.max(settings.width)) as f32;
        let content_w = (closed_w - if self.cfg.notch { 32.0 } else { 12.0 }).max(120.0);
        let row = Rect::new(
            rect.x + (rect.w - content_w * s) / 2.0,
            rect.y,
            content_w * s,
            settings.height as f32 * s,
        );
        let pad = 14.0 * s;
        let inner = Rect::new(row.x + pad, row.y, (row.w - 2.0 * pad).max(1.0), row.h);
        let weight_clock = if plan.idle {
            Weight::Semibold
        } else {
            Weight::Bold
        };
        let clock_size = (if plan.idle { 11.0 } else { 12.0 }) * s * text_scale;
        let clock_style = style(clock_size, weight_clock, fade(theme_text, alpha));
        let summary_size = 12.0 * s * text_scale;
        canvas.with_opacity(alpha, |canvas| {
            let mut left = inner.x;
            let mut right = inner.right();
            // Right cluster: clock, feedback level, awake mark.
            if let Some(clock) = plan.clock.as_deref() {
                let w = fonts.measure_width(clock, &clock_style);
                if plan.idle {
                    let r = Rect::new(inner.x, inner.y, w, inner.h);
                    super::util::text_left(canvas, fonts, r.x, r, clock, &clock_style, w + 2.0);
                    left += w + 10.0 * s;
                } else {
                    right -= w;
                    let r = Rect::new(right, inner.y, w, inner.h);
                    super::util::text_left(canvas, fonts, right, r, clock, &clock_style, w + 2.0);
                    right -= 12.0 * s;
                }
            }
            if plan.feedback {
                let level = Rect::new(
                    right - 34.0 * s,
                    inner.y + (inner.h - 3.0 * s) / 2.0,
                    34.0 * s,
                    3.0 * s,
                );
                canvas.fill_rrect(level, 1.5 * s, self.cfg.theme.progress_track);
                let fill = Rect::new(
                    level.x,
                    level.y,
                    level.w * self.feedback.level(snaps.now) as f32,
                    level.h,
                );
                if fill.w > 0.0 {
                    canvas.fill_rrect(fill, 1.5 * s, self.cfg.theme.progress_fill);
                }
                right -= 41.0 * s;
            }
            if plan.awake {
                let dot = Rgba::rgb(0xB4, 0xF2, 0x72);
                let w = 9.0 * s;
                canvas.fill_circle(
                    right - w / 2.0 - 8.0 * s,
                    inner.y + inner.h / 2.0,
                    2.4 * s,
                    &naby_render::Fill::Solid(dot),
                );
                right -= w + 8.0 * s;
            }
            // Leading icon / cover.
            let lead = Rect::new(
                left,
                inner.y + (inner.h - 18.0 * s) / 2.0,
                18.0 * s,
                18.0 * s,
            );
            if plan.feedback {
                let icon = Rect::new(
                    left,
                    inner.y + (inner.h - 16.0 * s) / 2.0,
                    16.0 * s,
                    16.0 * s,
                );
                canvas.icon(
                    self.feedback.icon(),
                    icon,
                    theme_text,
                    naby_render::icons::DEFAULT_STROKE,
                );
                left += 27.0 * s;
            } else if plan.cover {
                let cover = Rect::new(
                    left,
                    inner.y + (inner.h - 20.0 * s) / 2.0,
                    20.0 * s,
                    20.0 * s,
                );
                if let Some(image) = self.artwork(snaps) {
                    canvas.draw_image(&image, cover, 1.0, 5.0 * s);
                } else {
                    canvas.fill_rrect(cover, 5.0 * s, Rgba::WHITE.with_alpha(0x16));
                    canvas.icon(Icon::Activity, cover.inset(4.0 * s), theme_text, 1.5);
                }
                left += 29.0 * s;
            } else if plan.leading {
                self.closed.leading.paint(canvas, lead, s, theme_text);
                left += 27.0 * s;
            }
            let middle = Rect::new(left, inner.y, (right - left).max(0.0), inner.h);
            if plan.waveform {
                let w = (104.0 * s).min(middle.w);
                let r = Rect::new(
                    middle.x + (middle.w - w) / 2.0,
                    inner.y + (inner.h - 22.0 * s) / 2.0,
                    w,
                    22.0 * s,
                );
                self.closed.wave.paint(canvas, r, s, theme_text);
            } else if plan.idle {
                if plan.date {
                    self.paint_idle_date(
                        canvas,
                        fonts,
                        snaps,
                        inner,
                        plan.clock.as_deref(),
                        s * text_scale,
                    );
                }
            } else {
                self.paint_summary(
                    canvas,
                    fonts,
                    snaps,
                    &plan,
                    middle,
                    summary_size,
                    theme_text,
                );
            }
        });
        self.plan = plan;
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_summary(
        &mut self,
        canvas: &mut Canvas,
        fonts: &Fonts,
        snaps: &Snapshots,
        plan: &ClosedPlan,
        middle: Rect,
        size: f32,
        color: Rgba,
    ) {
        if middle.w < 4.0 {
            return;
        }
        let weight = if plan.bold {
            Weight::Semibold
        } else {
            Weight::Regular
        };
        let st = style(size, weight, color);
        let mut text = plan.text.clone();
        let mut centered = false;
        if plan.lyrics {
            let s = &snaps.settings;
            let cfg = ClosedLyricsConfig {
                offset_seconds: s.lyrics_offset,
                animation: LyricsAnimation::parse(&s.lyrics_animation),
            };
            let (elapsed, duration) = snaps.media_timeline();
            let style_key = u64::from(size.to_bits()) ^ ((weight as u64) << 40);
            let measure = |t: &str| fonts.measure_width(t, &st);
            let line = self.closed.lyric.line(
                &cfg,
                &snaps.lyrics.lines,
                can_use(&snaps.media),
                elapsed,
                duration,
                middle.w / self.scale,
                style_key,
                &|t| measure(t) / self.scale,
            );
            if let ClosedLine::Text(t) = line {
                text = t;
            }
            centered = true;
        }
        if text != self.closed.text {
            self.closed.previous_text = std::mem::replace(&mut self.closed.text, text.clone());
            self.closed.text_changed_at = snaps.now;
            if plan.lyrics && self.cfg.motion.enabled && !snaps.settings.reduce_motion {
                self.lyric_anim_until = snaps.now + 0.34;
            }
        }
        let animate = plan.lyrics
            && self.cfg.motion.enabled
            && !snaps.settings.reduce_motion
            && snaps.now < self.closed.text_changed_at + 0.34
            && !self.closed.previous_text.is_empty();
        let draw = |canvas: &mut Canvas, t: &str, dy: f32, opacity: f32| {
            let st = style(size, weight, fade(color, opacity));
            let shown = fonts.ellipsize(t, &st, middle.w);
            let (w, h) = fonts.measure(&shown, &st);
            let x = if centered {
                middle.x + (middle.w - w) / 2.0
            } else {
                middle.x
            };
            canvas.text(
                fonts,
                x,
                middle.y + (middle.h - h) / 2.0 + dy,
                &shown,
                &st,
                None,
            );
        };
        if animate {
            let t = ((snaps.now - self.closed.text_changed_at) / 0.34).clamp(0.0, 1.0);
            let e = Ease::CUBIC_OUT.apply(t) as f32;
            let old = self.closed.previous_text.clone();
            draw(canvas, &old, -2.0 * e * self.scale, 1.0 - e);
            draw(canvas, &text, 2.0 * (1.0 - e) * self.scale, 0.18 + 0.82 * e);
        } else {
            draw(canvas, &text, 0.0, 1.0);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_idle_date(
        &self,
        canvas: &mut Canvas,
        fonts: &Fonts,
        snaps: &Snapshots,
        inner: Rect,
        clock: Option<&str>,
        k: f32,
    ) {
        // Equal clearance on both sides keeps the date on the dock's centre.
        let clock_w = clock.map_or(0.0, |c| {
            fonts.measure_width(c, &style(11.0 * k, Weight::Semibold, Rgba::WHITE))
        });
        let room = inner.w - 2.0 * (60.0 * self.scale).max(clock_w + 10.0 * self.scale);
        if room < 80.0 * self.scale {
            return;
        }
        let date = snaps.wall.date_naive();
        let wd = date.format("%a").to_string().to_uppercase();
        let day = date.format("%d").to_string();
        let month = date.format("%b").to_string().to_uppercase();
        let parts = [
            (wd, 9.0, Weight::Regular, Rgba::rgb(0x9D, 0xA3, 0xAD)),
            (
                "  \u{b7}  ".to_owned(),
                9.0,
                Weight::Regular,
                Rgba::rgb(0x62, 0x68, 0x73),
            ),
            (day, 12.0, Weight::Semibold, Rgba::rgb(0xE5, 0xE8, 0xED)),
            (
                format!(" {month}"),
                10.0,
                Weight::Regular,
                Rgba::rgb(0xB7, 0xBD, 0xC7),
            ),
        ];
        let total: f32 = parts
            .iter()
            .map(|(t, sz, w, c)| fonts.measure_width(t, &style(sz * k, *w, *c)))
            .sum();
        let mut x = inner.x + (inner.w - total) / 2.0;
        for (t, sz, w, c) in &parts {
            let st = style(sz * k, *w, *c);
            let h = fonts.line_height(&st);
            let width = fonts.measure_width(t, &st);
            canvas.text(fonts, x, inner.y + (inner.h - h) / 2.0, t, &st, None);
            x += width;
        }
    }
}

/// `HH:mm` / `h:mm tt` (and with seconds when `seconds`).
pub fn clock_text(snaps: &Snapshots, seconds: bool) -> String {
    let s = &snaps.settings;
    let fmt = match (s.clock24_hour, seconds || s.clock_seconds && seconds) {
        (true, false) => "%H:%M",
        (true, true) => "%H:%M:%S",
        (false, false) => "%-I:%M %p",
        (false, true) => "%-I:%M:%S %p",
    };
    snaps.wall.format(fmt).to_string()
}

#[allow(dead_code)]
pub(crate) fn next_minute(wall_second: u32, frac: f64) -> f64 {
    until_boundary(f64::from(wall_second) + frac, 60.0)
}
