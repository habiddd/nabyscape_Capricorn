//! Synced lyrics (`LyricsView.cs`): the previous, current and next line, with the current line
//! animated (Slide, Focus or Karaoke sweep), a status credit, Retry and a local `.lrc` import.
//! [`LyricsView`] is drawn inside the media page; the `lyrics` page id hosts the same view on
//! its own.

use super::common::{Ctl, Interaction, Presence};
use super::{Page, PageCtx};
use crate::host::{Demand, Snapshots};
use naby_core::color::Rgba;
use naby_core::command::Command;
use naby_core::media::lrc;
use naby_core::media::LyricsAnimation;
use naby_core::motion::{Ease, MotionConfig, Tween};
use naby_render::{Canvas, InputState, Rect, Weight};

pub(super) const RETRY: u32 = 80;
pub(super) const LOCAL: u32 = 81;

pub fn build() -> Box<dyn Page> {
    Box::new(LyricsPage::default())
}

/// Lyrics are shown when switched on and the track has the details a lookup needs.
pub fn visible(snaps: &Snapshots) -> bool {
    snaps.settings.lyrics_enabled && lrc::can_use(&snaps.media)
}

#[derive(Default)]
pub(super) struct LyricsView {
    key: Option<(i64, usize, LyricsAnimation)>,
    changed_at: f64,
    /// Host time of the next frame this view needs.
    pub next: Option<f64>,
}

impl LyricsView {
    pub fn reset(&mut self) {
        *self = LyricsView::default();
    }

    /// Draws the view into `rect` (its height is fixed by the caller: 130 or 200 DIP).
    pub fn layout(&mut self, c: &mut Ctl, rect: Rect) {
        if c.is_dry() {
            return;
        }
        let th = c.theme().clone();
        let s = c.s;
        let snaps: &Snapshots = c.ctx.snaps;
        let settings = c.ctx.settings();
        let result = &*snaps.lyrics;
        let lines = &result.lines.lines;
        let style = LyricsAnimation::parse(&settings.lyrics_animation);
        let offset = if settings.lyrics_offset.is_finite() {
            settings.lyrics_offset.clamp(-10.0, 10.0)
        } else {
            0.0
        };
        let (elapsed, duration) = snaps.media_timeline();
        let at = elapsed + offset;
        let index = result.lines.index(at).map_or(-1i64, |i| i as i64);
        let key = (index, lines.as_ptr() as usize, style);
        if self.key != Some(key) {
            // The line shown when the view opens appears without the change animation.
            self.changed_at = if self.key.is_none() {
                f64::NEG_INFINITY
            } else {
                c.now
            };
            self.key = Some(key);
        }
        let motion = MotionConfig::from_settings(settings, true);
        let animate = motion.enabled && !settings.reduce_motion;
        let n = lines.len() as i64;
        let prev = if index > 0 {
            lines[index as usize - 1].text.as_str()
        } else {
            ""
        };
        let current = if index >= 0 {
            lines[index as usize].text.as_str()
        } else if n > 0 {
            "\u{266a}"
        } else {
            "Lyrics"
        };
        let next = if index + 1 < n {
            lines[(index + 1) as usize].text.as_str()
        } else {
            ""
        };

        // ---- rows ------------------------------------------------------------------------------
        let prev_h = c.line_h(11.0, Weight::Regular) + 5.0 * s;
        let next_h = 5.0 * s + c.line_h(12.0, Weight::Regular) + 8.0 * s;
        let footer_h = (22.0 * s).max(c.line_h(9.0, Weight::Regular));
        let line_h = (rect.h - prev_h - next_h - footer_h).max(42.0 * s);
        let line = Rect::new(rect.x, rect.y + prev_h, rect.w, line_h);
        c.faded(0.4, |c| {
            c.text(
                rect.x,
                rect.y,
                prev,
                11.0,
                Weight::Regular,
                th.text_dim,
                Some(rect.w),
            );
        });
        c.faded(0.6, |c| {
            c.text(
                rect.x,
                line.bottom() + 5.0 * s,
                next,
                12.0,
                Weight::Regular,
                th.text_dim,
                Some(rect.w),
            );
        });

        // ---- the current line ---------------------------------------------------------------
        let t = ((c.now - self.changed_at) / 0.3).clamp(0.0, 1.0);
        let changing = animate && t < 1.0;
        let (mut dy, mut alpha, mut size) = (0.0f32, 1.0f32, 18.0f32);
        if changing {
            match style {
                LyricsAnimation::Slide => {
                    let tw = Tween::new(10.0, 0.0, self.changed_at, 0.26, Ease::EXPO_OUT_5);
                    dy = tw.value(c.now) as f32 * s;
                }
                LyricsAnimation::Focus => {
                    let sc = Tween::new(0.97, 1.0, self.changed_at, 0.3, Ease::EXPO_OUT_5);
                    let fade = Tween::new(0.5, 1.0, self.changed_at, 0.26, Ease::CUBIC_OUT);
                    size = 18.0 * sc.value(c.now) as f32;
                    alpha = fade.value(c.now) as f32;
                }
                LyricsAnimation::Karaoke => {}
            }
        }
        let wrap_w = rect.w.max(1.0);
        let st = c.style(size, Weight::Semibold, Rgba::WHITE);
        let lh = c.line_h(size, Weight::Semibold);
        let mut wrapped = c.ctx.fonts.wrap(current, &st, wrap_w);
        if wrapped.is_empty() {
            wrapped.push(String::new());
        }
        if wrapped.len() > 2 {
            let rest = wrapped[1..].join(" ");
            wrapped.truncate(2);
            wrapped[1] = c.ctx.fonts.ellipsize(&rest, &st, wrap_w).into_owned();
        }
        let block = (lh * wrapped.len() as f32).min(52.0 * s);
        let top = line.y + (line.h - block) / 2.0 + dy;
        let karaoke = style == LyricsAnimation::Karaoke && index >= 0;
        c.clip(line);
        let base = if karaoke { th.text_dim } else { Rgba::WHITE };
        c.faded(alpha, |c| {
            for (i, l) in wrapped.iter().enumerate() {
                c.text(
                    rect.x,
                    top + i as f32 * lh,
                    l,
                    size,
                    Weight::Semibold,
                    base,
                    None,
                );
            }
        });
        if karaoke {
            let i = index as usize;
            let end = if i + 1 < lines.len() {
                lines[i + 1].at
            } else {
                duration
            };
            let progress =
                ((at - lines[i].at) / (end - lines[i].at).max(0.1)).clamp(0.0, 1.0) as f32;
            let width = line.w * if animate { progress } else { 1.0 };
            c.clip(Rect::new(line.x, line.y, width, line.h));
            for (i, l) in wrapped.iter().enumerate() {
                c.text(
                    rect.x,
                    top + i as f32 * lh,
                    l,
                    size,
                    Weight::Semibold,
                    Rgba::WHITE,
                    None,
                );
            }
            c.unclip();
        }
        c.unclip();

        // ---- footer: credit, Local LRC…, Retry ----------------------------------------------------
        let fy = rect.bottom() - footer_h;
        let mut right = rect.right();
        let retry_visible = lines.is_empty() && !result.status.starts_with("Finding");
        let mut buttons: Vec<(u32, &str)> = Vec::new();
        if retry_visible {
            buttons.push((RETRY, "Retry"));
        }
        buttons.push((LOCAL, "Local LRC\u{2026}"));
        for (id, label) in buttons {
            let w = c.text_w(label, 9.0, Weight::Regular) + 8.0 * s;
            let r = Rect::new(right - w, fy + (footer_h - 22.0 * s) / 2.0, w, 22.0 * s);
            let b = c.text_button(
                id,
                r,
                label,
                9.0,
                Weight::Regular,
                th.control_text,
                None,
                true,
            );
            if b.clicked {
                c.push(if id == RETRY {
                    Command::LyricsRetry
                } else {
                    Command::LyricsPickFile
                });
            }
            right = r.x - 6.0 * s;
        }
        let ly = fy + (footer_h - c.line_h(9.0, Weight::Regular)) / 2.0;
        c.text(
            rect.x,
            ly,
            &result.status,
            9.0,
            Weight::Regular,
            th.text_dim,
            Some((right - rect.x).max(1.0)),
        );

        // ---- what to wake for ---------------------------------------------------------------------
        self.next = None;
        if snaps.media.playing {
            let mut wake = f64::INFINITY;
            if karaoke {
                wake = wake.min(c.now + 0.05);
            } else if let Some(line) = lines.get((index + 1) as usize) {
                wake = wake.min(c.now + (line.at - at).max(0.02) + 0.01);
            }
            if wake.is_finite() {
                self.next = Some(wake);
            }
        }
        if changing {
            self.next = Some(
                self.next
                    .map_or(c.now + 1.0 / 60.0, |n| n.min(c.now + 1.0 / 60.0)),
            );
        }
    }
}

#[derive(Default)]
struct LyricsPage {
    ui: Interaction,
    presence: Presence,
    view: LyricsView,
}

impl Page for LyricsPage {
    fn id(&self) -> &'static str {
        "lyrics"
    }

    fn height(&mut self, ctx: &PageCtx, _width: f32) -> f32 {
        ctx.px(130.0 + 18.0)
    }

    fn frame(
        &mut self,
        canvas: &mut Canvas,
        ctx: &PageCtx,
        rect: Rect,
        input: &InputState,
        out: &mut Vec<Command>,
    ) -> bool {
        self.presence.mark(ctx.snaps.now);
        let mut c = Ctl::draw(canvas, ctx, input, out, &mut self.ui);
        let view = Rect::new(rect.x, rect.y + c.px(6.0), rect.w, c.px(130.0));
        self.view.layout(&mut c, view);
        false
    }

    fn demand(&self) -> Demand {
        Demand {
            lyrics: self.presence.active(),
            ..Demand::default()
        }
    }

    fn next_frame(&self, now: f64) -> Option<f64> {
        self.presence.observe(now);
        if self.presence.active() {
            self.view.next
        } else {
            None
        }
    }

    fn captures_pointer(&self) -> bool {
        self.ui.captures()
    }

    fn on_hide(&mut self) {
        self.presence.hide();
        self.ui.reset();
        self.view.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pages::common::harness::Harness;
    use naby_core::media::{LyricResult, MediaSnapshot};
    use std::time::Duration;

    fn lyrics() -> LyricResult {
        LyricResult {
            lines: lrc::parse(
                "[00:01.00]First light on the water\n[00:05.00]A long line that keeps going and going past the edge of the narrow column\n[00:09.00]Third\n[00:12.00]Fourth",
            ),
            status: "Synced lyrics \u{b7} LRCLIB".into(),
        }
    }

    fn playing(h: &mut Harness, position: u64) {
        h.snaps.media.set(MediaSnapshot {
            available: true,
            title: "Song".into(),
            artist: "Artist".into(),
            source_id: "Spotify.exe".into(),
            playing: true,
            position: Duration::from_secs(position),
            duration: Duration::from_secs(200),
            ..Default::default()
        });
        h.snaps.media_epoch = h.snaps.now;
        h.snaps.lyrics.replace(lyrics());
        let mut s = h.snaps.settings.arc().as_ref().clone();
        s.lyrics_enabled = true;
        h.snaps.settings.replace(s);
        h.refresh_theme();
    }

    #[test]
    fn visibility_needs_the_setting_and_track_details() {
        let Some(mut h) = Harness::new("lyrics") else {
            return;
        };
        assert!(!visible(&h.snaps));
        playing(&mut h, 3);
        assert!(visible(&h.snaps));
        let mut s = h.snaps.settings.arc().as_ref().clone();
        s.lyrics_enabled = false;
        h.snaps.settings.replace(s);
        assert!(!visible(&h.snaps));
    }

    #[test]
    fn wakes_at_the_next_line_and_stays_quiet_when_paused_or_hidden() {
        let Some(mut h) = Harness::new("lyrics") else {
            return;
        };
        playing(&mut h, 3);
        h.idle();
        let wake = h.page.next_frame(h.snaps.now).expect("next line pending");
        // The next line starts at 5 s: two seconds away.
        assert!(wake - h.snaps.now > 1.5, "{}", wake - h.snaps.now);
        let mut m = h.snaps.media.arc().as_ref().clone();
        m.playing = false;
        h.snaps.media.set(m);
        h.advance(1.0);
        h.idle();
        // Only the line change animation may still be running right after a change.
        h.advance(1.0);
        h.idle();
        assert_eq!(h.page.next_frame(h.snaps.now), None);
        assert_eq!(h.page.next_frame(h.snaps.now + 20.0), None);
    }

    #[test]
    fn karaoke_ticks_fast_and_asks_for_lyrics_while_shown() {
        let Some(mut h) = Harness::new("lyrics") else {
            return;
        };
        playing(&mut h, 3);
        let mut s = h.snaps.settings.arc().as_ref().clone();
        s.lyrics_animation = "Karaoke".into();
        h.snaps.settings.replace(s);
        h.idle();
        let wake = h.page.next_frame(h.snaps.now).unwrap();
        assert!(wake - h.snaps.now <= 0.06);
        assert!(h.page.demand().lyrics);
        h.page.on_hide();
        assert!(!h.page.demand().lyrics);
    }

    #[test]
    fn retry_and_local_import_emit_their_commands() {
        let Some(mut h) = Harness::new("lyrics") else {
            return;
        };
        playing(&mut h, 3);
        h.snaps.lyrics.replace(LyricResult {
            lines: Default::default(),
            status: "No synced lyrics for this track.".into(),
        });
        h.idle();
        h.click_id(RETRY);
        h.click_id(LOCAL);
        assert_eq!(
            h.commands(),
            vec![Command::LyricsRetry, Command::LyricsPickFile]
        );
        // While finding, only the import stays.
        h.snaps.lyrics.replace(LyricResult {
            lines: Default::default(),
            status: "Finding synced lyrics\u{2026}".into(),
        });
        h.idle();
        assert!(!h.has_control(RETRY));
        assert!(h.has_control(LOCAL));
    }

    #[test]
    fn snapshots_of_each_animation() {
        for style in ["Slide", "Focus", "Karaoke"] {
            let Some(mut h) = Harness::new("lyrics") else {
                return;
            };
            playing(&mut h, 6);
            let mut s = h.snaps.settings.arc().as_ref().clone();
            s.lyrics_animation = style.into();
            h.snaps.settings.replace(s);
            h.idle();
            h.advance(1.0);
            h.idle();
            assert!(h.ink() > 800, "{style}");
            h.save(&format!("page-lyrics-{}", style.to_lowercase()));
        }
        let Some(mut h) = Harness::new("lyrics") else {
            return;
        };
        playing(&mut h, 0);
        h.snaps.lyrics.replace(LyricResult {
            lines: Default::default(),
            status: "Lyrics unavailable. Tap Retry.".into(),
        });
        h.idle();
        h.save("page-lyrics-unavailable");
    }
}
