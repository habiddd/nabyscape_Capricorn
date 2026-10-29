//! Visualizer, lyrics, live signal and completion sound (C# `SettingsMediaExperience.cs`).

use super::SettingsView;
use naby_core::color::Rgba;
use naby_core::command::{Command, FilePurpose, SettingsRequest};
use naby_render::{Rect, Ui, Weight};

/// The three lyric animation presets: (label, value).
const PRESETS: [(&str, &str); 3] = [
    ("Slide · flowing lines", "Slide"),
    ("Focus · soft emphasis", "Focus"),
    ("Karaoke · line sweep", "Karaoke"),
];

/// Seconds of the looping sample lyric and its three lines (`LyricsAnimationExample`).
const SAMPLE_SECONDS: f64 = 12.0;
const SAMPLE_LINES: [(f64, &str); 3] = [
    (0.0, "A little closer to the music"),
    (4.0, "One line, right on time"),
    (8.0, "Let the next moment arrive"),
];

impl SettingsView {
    pub(super) fn media_experience_settings<'a>(&mut self, ui: &mut Ui<'a>) {
        self.section(ui, "Visualizer", |this, ui| {
            let source = this.s.waveform_source.clone();
            this.combo_plain(
                ui,
                "Visualizer input",
                &[("Frequency spectrum", "Spectrum"), ("Amplitude history", "Envelope")],
                &source,
                |s, v| s.waveform_source = v.to_owned(),
            );
            let sens = this.s.waveform_sensitivity;
            this.slider(ui, "Visualizer sensitivity", sens, 0.5, 3.0, "×", 0.05, |s, v| s.waveform_sensitivity = v);
            this.muted(
                ui,
                "Spectrum separates real bass, midrange and treble from the selected media app's active output. Other apps sharing that output can influence the bands. Muting the selected app silences the display. Output samples stay in memory; microphone capture is never used. Amplitude history remains the fallback for unsupported output formats.",
                11.0,
            );
        });
        self.section(ui, "Lyrics", |this, ui| {
            this.lyrics_animation_example(ui);
            let on = this.s.lyrics_enabled;
            this.check(ui, "Show synced lyrics in the media panel", on, |s, v| s.lyrics_enabled = v);
            this.muted(
                ui,
                "Off by default. When visible, song title, artist and duration are sent to LRCLIB to find timed lyrics. No music-service login or API key is needed. Availability varies by track; lyrics are cached only in memory.",
                11.0,
            );
            ui.gap(6.0);
            let anim = this.s.lyrics_animation.clone();
            this.combo_plain(ui, "Lyrics animation preset", &PRESETS, &anim, |s, v| s.lyrics_animation = v.to_owned());
            let offset = this.s.lyrics_offset;
            this.slider(ui, "Lyrics timing offset", offset, -10.0, 10.0, " s", 0.1, |s, v| s.lyrics_offset = v);
            if this.button(ui, "Load local timed lyrics…") {
                this.out.push(Command::Settings(SettingsRequest::PickFile(FilePurpose::LyricsFile)));
            }
            ui.gap(4.0);
            let note = this.lyrics_note.clone();
            match note {
                Some(text) => this.muted_final(ui, &text, 11.0),
                None => this.muted(
                    ui,
                    "Local LRC files can supply timings when the service is unavailable. Imports last for this app session.",
                    11.0,
                ),
            }
            ui.gap(6.0);
            this.muted(
                ui,
                "The lyrics icon beside playback controls toggles this preference. Slide follows each new line; Focus softly emphasizes it; Karaoke sweeps across the line using line timing. Reduce Motion keeps the text static. Tab to the lyrics button and press Space to show or hide.",
                11.0,
            );
        });
    }

    /// The animated sample line card above the lyrics switch.
    fn lyrics_animation_example<'a>(&mut self, ui: &mut Ui<'a>) {
        let th = ui.theme().clone();
        let rect = ui.allocate(None, 124.0);
        ui.gap(14.0);
        let fixed = self.s.reduce_motion;
        let t = if fixed {
            5.5
        } else {
            self.now % SAMPLE_SECONDS
        };
        if !fixed && ui.visible(rect) {
            ui.request_repaint_after(0.1);
        }
        let c = ui.canvas();
        c.fill_rrect(rect, th.radius, Rgba::rgb(0x1C, 0x1F, 0x24));
        c.push_clip_round_rect(rect, th.radius);
        let active = SAMPLE_LINES
            .iter()
            .rposition(|(start, _)| t >= *start)
            .unwrap_or(0);
        let start = SAMPLE_LINES[active].0;
        let end = SAMPLE_LINES.get(active + 1).map_or(SAMPLE_SECONDS, |l| l.0);
        let progress = ((t - start) / (end - start)).clamp(0.0, 1.0) as f32;
        let mode = self.s.lyrics_animation.clone();
        let line_h = 34.0;
        // The active line sits in the middle; `Slide` eases the whole stack as the line changes.
        let slide = if mode == "Slide" && !fixed {
            (1.0 - (progress * 6.0).min(1.0)) * 12.0
        } else {
            0.0
        };
        for (i, (_, text)) in SAMPLE_LINES.iter().enumerate() {
            let offset = i as f32 - active as f32;
            let y = rect.y + (rect.h - line_h) / 2.0 + offset * line_h + slide;
            let is_active = i == active;
            let size = if is_active { 18.0 } else { 13.0 };
            let weight = if is_active {
                Weight::Semibold
            } else {
                Weight::Regular
            };
            let color = if is_active {
                th.text
            } else {
                th.text_dim
                    .with_alpha(if mode == "Focus" { 120 } else { 190 })
            };
            let st = ui.text_style(size, weight, color);
            let tw = ui.fonts().measure_width(text, &st);
            let x = rect.x + (rect.w - tw.min(rect.w - 24.0)) / 2.0;
            ui.draw_text(
                x,
                y + (line_h - ui.line_height(&st)) / 2.0,
                text,
                &st,
                Some(rect.w - 24.0),
            );
            if is_active && mode == "Karaoke" {
                // The sweep: the part of the line already sung is repainted brighter.
                let sung = ui.canvas_fonts();
                let _ = sung;
                let partial = tw.min(rect.w - 24.0) * progress;
                let (canvas, fonts) = ui.canvas_fonts();
                canvas.push_clip_rect(Rect::new(x, y, partial, line_h));
                let hot = naby_render::TextStyle {
                    color: th.accent,
                    ..st
                };
                canvas.text(
                    fonts,
                    x,
                    y + (line_h - fonts.line_height(&hot)) / 2.0,
                    text,
                    &hot,
                    Some(rect.w - 24.0),
                );
                canvas.pop_clip();
            }
        }
        ui.canvas().pop_clip();
        let label = self
            .tr("Animation preview · original sample text")
            .to_owned();
        let st = ui.text_style(10.0, Weight::Regular, th.text_dim);
        ui.draw_text(
            rect.x + 10.0,
            rect.bottom() - 16.0,
            &label,
            &st,
            Some(rect.w - 20.0),
        );
    }

    /// The "Live signal" waveform readout under the volume mixer card.
    pub(super) fn audio_signal<'a>(&mut self, ui: &mut Ui<'a>) {
        let th = ui.theme().clone();
        let (playing, peak, muted, ready) = match &self.snaps {
            Some(s) => (
                s.media.playing,
                s.levels.peak,
                s.levels.meter_muted,
                s.levels.spectrum_ready,
            ),
            None => (false, 0.0, false, false),
        };
        let animate = playing && self.s.live_activity && !self.s.reduce_motion && !muted;
        let rect = ui.allocate(None, 28.0);
        let bars = Rect::new(rect.x, rect.y + 6.0, 96.0, 16.0);
        let bands: Vec<f32> = self
            .snaps
            .as_ref()
            .map_or_else(Vec::new, |s| s.levels.bands.to_vec());
        {
            let c = ui.canvas();
            let n = 25;
            for i in 0..n {
                let level = if animate && ready {
                    bands.get(i).copied().unwrap_or(0.0)
                } else if animate {
                    peak * (0.4
                        + 0.6 * ((i as f32 * 0.37 + self.now as f32 * 3.0).sin() * 0.5 + 0.5))
                } else {
                    0.0
                };
                let h = (2.0 + 14.0 * level.clamp(0.0, 1.0)).min(bars.h);
                c.fill_rrect(
                    Rect::new(
                        bars.x + i as f32 * 3.84,
                        bars.y + (bars.h - h) / 2.0,
                        2.0,
                        h,
                    ),
                    1.0,
                    th.text,
                );
            }
        }
        if animate {
            ui.request_repaint_after(0.05);
        }
        let state = if !self.s.live_activity {
            "Live activity is off · enable it in Audio".to_owned()
        } else if self.s.reduce_motion {
            "Reduce Motion is on · the waveform stays still".to_owned()
        } else if !playing {
            "Playback paused · quiet baseline".to_owned()
        } else if muted {
            "Selected app or output is muted".to_owned()
        } else if peak <= 0.0 && !ready {
            "No matching audio meter · optional gentle fallback".to_owned()
        } else {
            format!("{} · {:.0}% peak", self.tr("Live meter"), peak * 100.0)
        };
        let shown = self.tr(&state).to_owned();
        self.muted_final(ui, &shown, 11.0);
        ui.gap(8.0);
    }

    pub(super) fn completion_sound_settings<'a>(&mut self, ui: &mut Ui<'a>) {
        self.section(ui, "Completion sound", |this, ui| {
            let sound = this.s.countdown_sound.clone();
            this.combo_plain(
                ui,
                "Timer and Focus completion sound",
                &[("Chime", "Chime"), ("Bell", "Bell"), ("Custom", "Custom"), ("Off", "Off")],
                &sound,
                |s, v| s.countdown_sound = v.to_owned(),
            );
            let volume = this.s.countdown_sound_volume * 100.0;
            this.slider(ui, "Completion sound volume", volume, 0.0, 100.0, "%", 1.0, |s, v| s.countdown_sound_volume = v / 100.0);
            let file = if this.s.countdown_sound_file.is_empty() {
                this.tr("No custom sound chosen").to_owned()
            } else {
                std::path::Path::new(&this.s.countdown_sound_file)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            };
            this.muted_final(ui, &file, 11.0);
            if this.button(ui, "Choose WAV…") {
                this.out.push(Command::Settings(SettingsRequest::PickFile(FilePurpose::CompletionSound)));
            }
            if this.button(ui, "Preview sound") {
                this.out.push(Command::Settings(SettingsRequest::PreviewCompletionSound));
                this.sound_note = Some(
                    "Played through your Windows output. Sound does not depend on Windows notifications.".to_owned(),
                );
            }
            if let Some(note) = this.sound_note.clone() {
                ui.gap(4.0);
                let text = this.tr(&note).to_owned();
                this.muted_final(ui, &text, 11.0);
            }
            ui.gap(6.0);
            this.muted(
                ui,
                "Timer and Focus play this sound directly when their per-type notification toggle is on. It uses the current Windows output; Off silences it. Custom WAV files must be under 10 MB and at most 15 seconds. File paths are kept locally and excluded from layout exports.",
                11.0,
            );
        });
    }
}
