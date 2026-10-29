//! The `media` page (`BuildCompactPage("media")`, `MediaControls.cs`, `MediaLyricsLayout.cs`):
//! cover, title and artist, transport, the waveform, the seek bar and the footer (source picker
//! and volume). With lyrics on, the synced lyrics sit beside the controls when the panel is wide
//! and below them otherwise. The audio mixer replaces the body when opened.

pub mod footer;
#[cfg(test)]
mod tests;

use super::common::{ArtCache, Crossfade, Ctl, Interaction, Presence};
use super::lyrics::{self, LyricsView};
use super::mixer::{MixerEvent, MixerView};
use super::{Page, PageCtx};
use crate::host::Demand;
use crate::overlay::util::format_clock;
use crate::overlay::wave::Wave;
use footer::{Footer, FooterEvent};
use naby_core::color::Rgba;
use naby_core::command::Command;
use naby_core::media::lrc;
use naby_core::motion::MotionConfig;
use naby_render::{Canvas, Icon, InputState, Rect, Weight};

const PREV: u32 = 1;
const PLAY: u32 = 2;
const NEXT: u32 = 3;
const LYRICS_TOGGLE: u32 = 4;
const SEEK: u32 = 5;

const ASIDE_RULE: Rgba = Rgba::rgb(0x30, 0x31, 0x37);

pub fn build() -> Box<dyn Page> {
    Box::new(MediaPage::default())
}

/// Hover-driven horizontal scroll of a title that is wider than its box (`MarqueeLabel`).
#[derive(Default)]
struct Marquee {
    hover_since: Option<f64>,
}

impl Marquee {
    /// The leftward offset in device pixels for `overflow` px of hidden text at `now`.
    fn offset(&self, now: f64, overflow: f32, scale: f32) -> f32 {
        let Some(t0) = self.hover_since else {
            return 0.0;
        };
        let travel = f64::from(overflow / scale) / 28.0;
        let forward = 1.4 + travel;
        let t = (now - t0).rem_euclid(forward * 2.0);
        let u = if t > forward { forward * 2.0 - t } else { t };
        let off = if u < 0.65 {
            0.0
        } else if u < 0.65 + travel {
            (u - 0.65) / travel
        } else {
            1.0
        };
        -(off as f32) * overflow
    }
}

#[derive(Default)]
struct Seek {
    pending: Option<(f32, f64)>,
    hold: Option<(f32, f64)>,
    dragging: bool,
}

pub(super) struct Body {
    footer: Footer,
    mixer: MixerView,
    mixer_mode: bool,
    lyrics: LyricsView,
    art: ArtCache,
    title: Crossfade<String>,
    artist: Crossfade<String>,
    wave: Wave,
    wave_seq: u64,
    wave_at: f64,
    seek: Seek,
    marquee: [Marquee; 2],
    wide: bool,
    next: Option<f64>,
    marquee_wake: Option<f64>,
    synced: bool,
    shows_lyrics: bool,
}

impl Default for Body {
    fn default() -> Self {
        Body {
            footer: Footer::default(),
            mixer: MixerView::default(),
            mixer_mode: false,
            lyrics: LyricsView::default(),
            art: ArtCache::default(),
            title: Crossfade::new(String::new(), 0.22),
            artist: Crossfade::new(String::new(), 0.22),
            wave: Wave::new(true, false),
            wave_seq: 0,
            wave_at: 0.0,
            seek: Seek::default(),
            marquee: [Marquee::default(), Marquee::default()],
            wide: false,
            next: None,
            marquee_wake: None,
            synced: false,
            shows_lyrics: false,
        }
    }
}

#[derive(Default)]
struct MediaPage {
    ui: Interaction,
    presence: Presence,
    st: Body,
}

impl Body {
    /// Absorbs the snapshots (no drawing).
    fn sync(&mut self, ctx: &PageCtx) {
        let snaps = ctx.snaps;
        let motion = MotionConfig::from_settings(ctx.settings(), true);
        // The first track after the page opens appears without a cross-fade.
        let animate = motion.enabled && self.synced;
        self.synced = true;
        let m = &snaps.media;
        self.title.set(m.title.clone(), snaps.now, animate);
        self.artist.set(m.artist.clone(), snaps.now, animate);
        self.title.settle(snaps.now);
        self.artist.settle(snaps.now);
        let px = (88.0 * ctx.scale).ceil() as u32;
        self.art.get(m.artwork.as_ref(), px, snaps.now, animate);
        self.art.settle(snaps.now);
        self.shows_lyrics = lyrics::visible(snaps);
        self.wide = self.shows_lyrics;
    }

    /// Advances the waveform like the dock's own bars.
    fn sample_wave(&mut self, ctx: &PageCtx) {
        let snaps = ctx.snaps;
        let s = ctx.settings();
        let motion = MotionConfig::from_settings(s, true);
        let playing = snaps.media.playing && !snaps.levels.meter_muted;
        let animate = playing && s.live_activity && !s.reduce_motion && motion.enabled;
        let dt = (snaps.now - self.wave_at).clamp(0.001, 0.5);
        self.wave_at = snaps.now;
        if !animate {
            if self.wave.moving() {
                self.wave.rest(0.0, false, false);
            }
            return;
        }
        let spectrum = (s.waveform_source == "Spectrum" && snaps.levels.spectrum_ready)
            .then_some(&snaps.levels.bands);
        let advance = snaps.levels.sequence != self.wave_seq;
        self.wave_seq = snaps.levels.sequence;
        self.wave.sample(
            f64::from(snaps.levels.peak),
            dt,
            0.0,
            false,
            false,
            true,
            snaps.now,
            advance,
            s.waveform_idle_motion,
            spectrum,
            s.waveform_sensitivity,
        );
    }

    fn layout(&mut self, c: &mut Ctl, rect: Rect) -> f32 {
        if self.mixer_mode {
            let (h, event) = self.mixer.layout(c, rect);
            if event == MixerEvent::Back {
                self.mixer_mode = false;
            }
            return h;
        }
        let s = c.s;
        let snaps = c.ctx.snaps;
        let show_volume = snaps.widget_on("volume");
        let side = self.shows_lyrics && rect.w / s >= 500.0;
        let controls_w = if side { rect.w - 280.0 * s } else { rect.w };
        let controls = Rect::new(rect.x, rect.y, controls_w, 0.0);
        let controls_h = self.controls(c, controls, show_volume);
        if !self.shows_lyrics {
            return controls_h;
        }
        if side {
            let height = 200.0 * s;
            let total = controls_h.max(height);
            let ax = rect.right() - 256.0 * s;
            c.fill(Rect::new(ax, rect.y, s.max(1.0), total), ASIDE_RULE);
            let view = Rect::new(
                ax + 21.0 * s,
                rect.y + (total - height) / 2.0,
                235.0 * s,
                height,
            );
            self.lyrics.layout(c, view);
            total
        } else {
            let height = 130.0 * s;
            let view = Rect::new(
                rect.x,
                rect.y + controls_h + 8.0 * s + 6.0 * s,
                rect.w,
                height,
            );
            self.lyrics.layout(c, view);
            controls_h + 8.0 * s + 6.0 * s + height + 12.0 * s
        }
    }

    /// The controls column: returns its height.
    fn controls(&mut self, c: &mut Ctl, rect: Rect, show_volume: bool) -> f32 {
        let th = c.theme().clone();
        let s = c.s;
        let snaps = c.ctx.snaps;
        let settings = c.ctx.settings();
        let m = &snaps.media;
        let now = c.now;
        let narrow = rect.w < 310.0 * s;
        let mut y = rect.y;

        // ---- cover, title, artist --------------------------------------------------------------
        let size = if narrow { 64.0 } else { 88.0 } * s;
        let gap = if narrow { 14.0 } else { 20.0 } * s;
        let radius = if narrow { 11.0 } else { 15.0 };
        let title_dip = if narrow { 15.0 } else { 18.0 };
        let weight = if settings.bold_media_title {
            Weight::Semibold
        } else {
            Weight::Regular
        };
        let title_h = c.line_h(title_dip, weight);
        let artist_h = c.line_h(12.0, Weight::Regular) + 4.0 * s;
        let row_h = size.max(title_h + artist_h);
        if !c.is_dry() {
            let cover = Rect::new(rect.x, y + (row_h - size) / 2.0, size, size);
            let px = (88.0 * s).ceil() as u32;
            let image = self.art.get(m.artwork.as_ref(), px, now, false);
            if let Some((old, a)) = self.art.outgoing(now) {
                c.cover(old, cover, radius, a);
            }
            if let Some(img) = image {
                c.cover(&img, cover, radius, self.art.alpha(now));
            }
        }
        let tx = rect.x + size + gap;
        let tw = (rect.w - size - gap).max(1.0);
        let ty = y + (row_h - title_h - artist_h) / 2.0;
        self.label(
            c,
            0,
            Rect::new(tx, ty, tw, title_h),
            title_dip,
            weight,
            th.text,
            true,
        );
        self.label(
            c,
            1,
            Rect::new(tx, ty + title_h + 4.0 * s, tw, artist_h - 4.0 * s),
            12.0,
            Weight::Regular,
            th.text_dim,
            false,
        );
        y += row_h + 6.0 * s;

        // ---- transport -----------------------------------------------------------------------------------
        let play_size = if narrow { 38.0 } else { 46.0 } * s;
        let side_size = if narrow { 28.0 } else { 38.0 } * s;
        let btn_gap = if narrow { 2.0 } else { 8.0 } * s;
        let bar_h = play_size.max(28.0 * s);
        let total_w = side_size * 2.0 + play_size + btn_gap * 2.0;
        let bx = rect.x + (rect.w - total_w) / 2.0;
        let center = y + bar_h / 2.0;
        let prev = Rect::new(bx, center - side_size / 2.0, side_size, side_size);
        let play = Rect::new(
            prev.right() + btn_gap,
            center - play_size / 2.0,
            play_size,
            play_size,
        );
        let next = Rect::new(
            play.right() + btn_gap,
            center - side_size / 2.0,
            side_size,
            side_size,
        );
        let b = c.icon_button(
            PREV,
            prev,
            Icon::Previous,
            20.0,
            None,
            Rgba::WHITE,
            m.can_previous,
        );
        if b.clicked {
            c.push(Command::MediaPrevious);
        }
        let icon = if m.playing { Icon::Pause } else { Icon::Play };
        let b = c.icon_button(
            PLAY,
            play,
            icon,
            22.0,
            Some(th.accent),
            Rgba::BLACK,
            m.can_toggle,
        );
        if b.clicked {
            c.push(Command::MediaToggle);
        }
        let b = c.icon_button(NEXT, next, Icon::Next, 20.0, None, Rgba::WHITE, m.can_next);
        if b.clicked {
            c.push(Command::MediaNext);
        }
        if lrc::can_use(m) {
            let r = Rect::new(
                rect.right() - 28.0 * s,
                center - 14.0 * s,
                28.0 * s,
                28.0 * s,
            );
            let on = settings.lyrics_enabled;
            let b = c.icon_button(
                LYRICS_TOGGLE,
                r,
                Icon::Lyrics,
                20.0,
                on.then_some(th.selection_fill),
                Rgba::WHITE,
                true,
            );
            if b.clicked {
                let mut next = settings.clone();
                next.lyrics_enabled = !next.lyrics_enabled;
                c.push(Command::ApplySettings(Box::new(next)));
            }
        }
        y += bar_h;

        // ---- waveform ---------------------------------------------------------------------------------------
        if settings.live_activity {
            y += 3.0 * s;
            let w = if narrow { 112.0 } else { 164.0 } * s;
            let r = Rect::new(rect.x + (rect.w - w) / 2.0, y, w, 22.0 * s);
            c.wave(&self.wave, r, th.text);
            y += 22.0 * s + 8.0 * s;
        }

        // ---- timeline ------------------------------------------------------------------------------------------
        let (elapsed, duration) = snaps.media_timeline();
        let can_seek = duration > 0.0 && m.can_seek;
        let timeline = if duration > 0.0 {
            (elapsed / duration) as f32
        } else {
            0.0
        };
        let hold = self
            .seek
            .hold
            .filter(|(_, until)| now < *until)
            .map(|(v, _)| v);
        let shown = if self.seek.dragging {
            self.seek.pending.map_or(timeline, |p| p.0)
        } else {
            hold.unwrap_or(timeline)
        };
        let long = duration >= 3600.0;
        let label_h = c.line_h(10.0, Weight::Regular);
        let shown_elapsed = if hold.is_some() || self.seek.dragging {
            f64::from(shown) * duration
        } else {
            elapsed
        };
        c.text(
            rect.x,
            y,
            &format_clock(shown_elapsed, long),
            10.0,
            Weight::Regular,
            th.text_dim,
            None,
        );
        c.text_right(
            rect.right(),
            y,
            &format_clock(duration, long),
            10.0,
            Weight::Regular,
            th.text_dim,
            None,
        );
        y += label_h;
        let slider = Rect::new(rect.x, y, rect.w, 24.0 * s);
        let r = c.slider(SEEK, slider, shown, 0.01, can_seek);
        if !c.is_dry() {
            self.seek.dragging = r.dragging;
            if r.changed {
                self.seek.pending = Some((r.value, now));
                self.seek.hold = Some((r.value, now + 0.9));
            }
            if let Some((value, at)) = self.seek.pending {
                if r.committed || now - at >= 0.14 {
                    c.push(Command::MediaSeek(f64::from(value)));
                    self.seek.pending = None;
                    self.seek.hold = Some((value, now + 0.9));
                }
            }
        }
        y += 24.0 * s + 10.0 * s;

        // ---- footer --------------------------------------------------------------------------------------------------
        y += 8.0 * s;
        let (h, event) = self.footer.layout(
            c,
            Rect::new(rect.x, y, rect.w, 0.0),
            true,
            show_volume,
            28.0 * s,
        );
        y += h;
        if event == FooterEvent::OpenMixer {
            self.mixer_mode = true;
            self.mixer.open();
        }

        // ---- when to wake -----------------------------------------------------------------------------------------------
        if !c.is_dry() {
            self.schedule(c, elapsed, duration);
        }
        y - rect.y
    }

    /// One line of text with the 220 ms swap cross-fade and the hover marquee.
    #[allow(clippy::too_many_arguments)]
    fn label(
        &mut self,
        c: &mut Ctl,
        slot: usize,
        r: Rect,
        dip: f32,
        weight: Weight,
        color: Rgba,
        is_title: bool,
    ) {
        if c.is_dry() {
            return;
        }
        let fade = if is_title { &self.title } else { &self.artist };
        let now = c.now;
        let text = fade.current().clone();
        let full = c.text_w(&text, dip, weight);
        let overflow = full - r.w;
        let motion = MotionConfig::from_settings(c.ctx.settings(), true).enabled;
        let hovered = c.pointer_in(r);
        let marquee = &mut self.marquee[slot];
        if hovered && motion && overflow >= 2.0 * c.s {
            marquee.hover_since.get_or_insert(now);
        } else {
            marquee.hover_since = None;
        }
        let running = marquee.hover_since.is_some();
        let off = marquee.offset(now, overflow.max(0.0), c.s);
        let a = fade.alpha(now);
        let outgoing = fade.outgoing(now).map(|(t, a)| (t.clone(), a));
        c.clip(r);
        if let Some((old, oa)) = outgoing {
            c.faded(oa, |c| {
                c.text(r.x, r.y, &old, dip, weight, color, Some(r.w));
            });
        }
        c.faded(a, |c| {
            if running {
                c.text(r.x + off, r.y, &text, dip, weight, color, None);
            } else {
                c.text(r.x, r.y, &text, dip, weight, color, Some(r.w));
            }
        });
        c.unclip();
        if running {
            self.marquee_wake = Some(now + 1.0 / 30.0);
        }
    }

    fn schedule(&mut self, c: &Ctl, elapsed: f64, duration: f64) {
        let now = c.now;
        let snaps = c.ctx.snaps;
        let mut wake: Vec<f64> = Vec::new();
        if let Some(n) = self.marquee_wake.take() {
            wake.push(n);
        }
        if snaps.media.playing && duration > 0.0 && !self.mixer_mode {
            wake.push(now + (1.0 - elapsed.fract()).max(0.02) + 0.005);
        }
        if let Some((_, at)) = self.seek.pending {
            wake.push(at + 0.14);
        }
        if let Some((_, until)) = self.seek.hold {
            if now < until {
                wake.push(until + 0.01);
            }
        }
        if self.art.animating(now) || self.title.animating(now) || self.artist.animating(now) {
            wake.push(now + 1.0 / 60.0);
        }
        if let Some(n) = self.lyrics.next {
            if self.shows_lyrics && !self.mixer_mode {
                wake.push(n);
            }
        }
        if let Some(n) = self.footer.next_frame(now) {
            wake.push(n);
        }
        self.next = wake.into_iter().reduce(f64::min);
    }
}

impl Page for MediaPage {
    fn id(&self) -> &'static str {
        "media"
    }

    fn height(&mut self, ctx: &PageCtx, width: f32) -> f32 {
        self.st.sync(ctx);
        let input = InputState::default();
        let mut c = Ctl::measure(ctx, &input, &mut self.ui);
        self.st.layout(&mut c, Rect::new(0.0, 0.0, width, 0.0))
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
        self.st.sync(ctx);
        self.st.sample_wave(ctx);
        let mut c = Ctl::draw(canvas, ctx, input, out, &mut self.ui);
        self.st.layout(&mut c, rect);
        false
    }

    fn demand(&self) -> Demand {
        let on = self.presence.active();
        Demand {
            lyrics: on && self.st.shows_lyrics && !self.st.mixer_mode,
            mixer: on && self.st.mixer_mode,
            ..Demand::default()
        }
    }

    fn next_frame(&self, now: f64) -> Option<f64> {
        self.presence.observe(now);
        if !self.presence.active() {
            return None;
        }
        let blink = self.ui.next_frame(now);
        [self.st.next, blink].into_iter().flatten().reduce(f64::min)
    }

    fn captures_pointer(&self) -> bool {
        self.ui.captures() || self.st.footer.captures() || self.st.mixer.dragging()
    }

    fn wants_wide(&self) -> bool {
        self.st.wide
    }

    fn on_hide(&mut self) {
        self.presence.hide();
        self.ui.reset();
        self.st.footer.reset();
        self.st.mixer.reset();
        self.st.mixer_mode = false;
        self.st.lyrics.reset();
        self.st.art.clear();
        self.st.seek = Seek::default();
        self.st.next = None;
        self.st.marquee_wake = None;
        self.st.synced = false;
        self.st.title = Crossfade::new(String::new(), 0.22);
        self.st.artist = Crossfade::new(String::new(), 0.22);
        self.st.wave.rest(0.0, false, false);
        self.st.marquee = [Marquee::default(), Marquee::default()];
    }
}
