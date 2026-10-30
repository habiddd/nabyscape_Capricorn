//! The voice room (`AssistantVoiceView`, `AssistantVoiceWorkspace`) and its compact form on the
//! dock's overlay page (`AssistantVoiceOverlay`): the character, the stage ("Listening",
//! "Thinking", "Speaking"), live captions, the mic level, Start/End/Stop, the microphone picker
//! with a test, and the engine line ("Whistle · built in").
//!
//! The view never touches audio. It reads [`VoiceSnapshot`]s and asks the [`VoiceHost`] to act;
//! the microphone choice is saved through the [`AssistantService`].

use std::sync::Arc;

use naby_core::assistant::State;
use naby_core::command::Command;
use naby_render::{Rect, Ui, Weight};

use super::model::{MicChoice, MicTest, VoiceHost, VoiceSnapshot};
use crate::assistant::orb::Orb;
use crate::assistant::service::AssistantService;

/// Characters of the assistant caption shown on the stage (`answer[..217] + "…"`).
const CAPTION_LIMIT: usize = 220;
const HEARD_LIMIT: usize = 110;

/// Per-frame facts from the window.
#[derive(Debug, Clone, Copy)]
pub struct VoiceEnv {
    pub now: f64,
    pub reduce_motion: bool,
    pub visible: bool,
    pub active: bool,
}

/// What the room asks the window to do.
#[derive(Debug, Default)]
pub struct VoiceOut {
    pub commands: Vec<Command>,
    /// Back to the chat page.
    pub open_chat: bool,
    /// Open the AI setup, Voice tab.
    pub open_setup: bool,
}

/// The stage title for a snapshot (`Accessibility.Announce(phase, ...)`).
pub fn phase_text(s: &VoiceSnapshot) -> String {
    if s.wake.listening && !s.active {
        return format!("Waiting for \u{201c}{}\u{201d}", s.wake.phrase);
    }
    match s.phase {
        State::Listening => "Listening".into(),
        State::Speaking => "Speaking".into(),
        State::Thinking => {
            if s.status.is_empty() {
                "Thinking".into()
            } else {
                s.status.clone()
            }
        }
        State::Connecting => "Connecting".into(),
        State::Error => "Connection needs attention".into(),
        _ => "Ready to talk".into(),
    }
}

/// The main button label.
pub fn primary_label(s: &VoiceSnapshot) -> &'static str {
    if s.active {
        "End conversation"
    } else if !s.configured {
        "Set up voice"
    } else if s.phase == State::Error {
        "Try again"
    } else {
        "Start conversation"
    }
}

/// What the stage shows as the assistant's words: the live spoken line, else the last reply.
pub fn caption_text(s: &VoiceSnapshot) -> String {
    let last = s.lines.iter().rev().find(|l| !l.user);
    let text = last.map_or("Speak naturally. Naby will reply.", |l| l.text.as_str());
    clip(text, CAPTION_LIMIT)
}

/// The last thing the user said.
pub fn heard_text(s: &VoiceSnapshot) -> Option<String> {
    s.lines
        .iter()
        .rev()
        .find(|l| l.user)
        .map(|l| clip(&l.text, HEARD_LIMIT))
}

/// The error or timing line under the captions.
pub fn detail_text(s: &VoiceSnapshot) -> String {
    if !s.configured {
        return s.setup_hint.clone();
    }
    if !s.engine_ready && !s.active {
        return s.engine_problem.clone();
    }
    if matches!(s.phase, State::Error | State::Connecting) {
        return s.status.clone();
    }
    s.timing.clone().unwrap_or_default()
}

/// The privacy line (`privacy.Text`).
pub fn privacy_text(s: &VoiceSnapshot) -> String {
    if s.active {
        if s.wake.enabled {
            "End conversation returns to local wake listening. Stop all microphones from the tray to pause both.".into()
        } else {
            "End conversation turns off the microphone.".into()
        }
    } else if s.wake.enabled {
        s.wake.text.clone()
    } else {
        "Your microphone stays off until you start.".into()
    }
}

fn clip(text: &str, limit: usize) -> String {
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > limit {
        let mut out: String = flat.chars().take(limit - 3).collect();
        out.push('\u{2026}');
        out
    } else {
        flat
    }
}

pub struct VoiceRoom {
    host: Arc<dyn VoiceHost>,
    service: AssistantService,
    orb: Orb,
    compact: bool,
    mics: Vec<MicChoice>,
    mic_pick: usize,
    mics_loaded: bool,
    seen_revision: u64,
    seen_service: u64,
    next_wake: Option<f64>,
    detail_open: bool,
    note: String,
    /// Where the main and Stop buttons were drawn last frame (hit tests).
    pub(crate) primary_rect: Rect,
    pub(crate) stop_rect: Rect,
}

impl VoiceRoom {
    pub fn new(service: AssistantService, host: Arc<dyn VoiceHost>) -> Self {
        Self {
            host,
            service,
            orb: Orb::new(),
            compact: false,
            mics: Vec::new(),
            mic_pick: 0,
            mics_loaded: false,
            seen_revision: u64::MAX,
            seen_service: 0,
            next_wake: None,
            detail_open: false,
            note: String::new(),
            primary_rect: Rect::new(0.0, 0.0, 0.0, 0.0),
            stop_rect: Rect::new(0.0, 0.0, 0.0, 0.0),
        }
    }

    /// The overlay page form: smaller stage, no microphone picker.
    pub fn compact(mut self) -> Self {
        self.compact = true;
        self
    }

    pub fn host(&self) -> &Arc<dyn VoiceHost> {
        &self.host
    }

    /// Something changed that needs a frame.
    pub fn dirty(&self) -> bool {
        self.host.snapshot().revision != self.seen_revision
            || self.seen_service != self.service.revision()
            || self.host.mic_state().running
    }

    pub fn next_frame(&self, now: f64) -> Option<f64> {
        [self.orb.next_frame(now), self.next_wake]
            .into_iter()
            .flatten()
            .reduce(f64::min)
    }

    /// The window was hidden: forget pixel-adjacent state and stop the microphone test.
    pub fn on_hide(&mut self) {
        self.orb.set_visible(false);
        self.next_wake = None;
        self.host.mic_test(false);
        self.mics.clear();
        self.mics_loaded = false;
    }

    fn load_mics(&mut self) {
        self.mics = self.host.microphones();
        let saved = self.service.options().microphone_id;
        self.mic_pick = self
            .mics
            .iter()
            .position(|m| m.id == saved)
            .or_else(|| {
                // Legacy `<guid>|<name>` ids resolve by the host's own rules; show the default.
                (!saved.is_empty()).then_some(0)
            })
            .unwrap_or(0);
        self.mics_loaded = true;
    }

    pub fn show(&mut self, ui: &mut Ui<'_>, env: &VoiceEnv, out: &mut VoiceOut) {
        let snap = self.host.snapshot();
        self.seen_revision = snap.revision;
        self.seen_service = self.service.revision();
        if !self.mics_loaded && !self.compact {
            self.load_mics();
        }
        let now = env.now;
        self.orb.set_reduce_motion(env.reduce_motion || !env.active);
        self.orb.set_visible(env.visible);
        let orb_state = if snap.wake.listening && !snap.active {
            State::Listening
        } else {
            snap.phase
        };
        if self.orb.state() != orb_state {
            self.orb.set_state(orb_state, now);
        }
        self.orb.set_live(snap.active || snap.wake.listening);
        self.orb.set_level(match snap.phase {
            State::Speaking => snap.output_level,
            _ => snap.mic_level,
        });
        self.orb.tick(now);

        if !self.compact {
            ui.row(|ui| {
                if ui.button("\u{2190} Chat").clicked {
                    out.open_chat = true;
                }
                if ui.button("Voice settings").clicked {
                    out.commands.push(Command::OpenSettingsPage("ai".into()));
                }
            });
        }
        ui.heading_sized("Naby voice", if self.compact { 16.0 } else { 18.0 });

        // The character.
        let size = if self.compact { 96.0 } else { 150.0 };
        let avail = ui.available_width();
        let stage = ui.allocate(None, size);
        let orb_rect = Rect::new(
            stage.x + (avail - size).max(0.0) / 2.0,
            stage.y,
            size.min(avail),
            size,
        );
        self.orb.paint(ui.canvas(), orb_rect, now);

        let dim = ui.theme().text_dim;
        let text = ui.theme().text;
        let phase_style = ui.text_style(12.0, Weight::Regular, dim);
        centered(ui, &phase_text(&snap), &phase_style, 1);
        let caption_style = ui.text_style(
            if self.compact { 14.0 } else { 17.0 },
            Weight::Regular,
            text,
        );
        centered(ui, &caption_text(&snap), &caption_style, 3);
        let heard_style = ui.text_style(11.0, Weight::Regular, dim);
        if let Some(heard) = heard_text(&snap) {
            centered(ui, &heard, &heard_style, 2);
        }
        if !snap.partial.is_empty() {
            let partial = format!("{}\u{2026}", clip(&snap.partial, HEARD_LIMIT));
            centered(ui, &partial, &heard_style, 2);
        }
        let detail = detail_text(&snap);
        if !detail.is_empty() {
            let style = ui.text_style(10.0, Weight::Regular, dim);
            centered(ui, &detail, &style, 2);
        }
        ui.gap(4.0);

        // Controls.
        let primary = primary_label(&snap);
        let stop_label = "Stop";
        let widths = ui
            .fonts()
            .measure_width(primary, &ui.text_style(13.0, Weight::Regular, text))
            + 40.0
            + if snap.active || snap.wake.listening {
                ui.fonts()
                    .measure_width(stop_label, &ui.text_style(13.0, Weight::Regular, text))
                    + 40.0
            } else {
                0.0
            };
        let pad = ((ui.available_width() - widths) / 2.0).max(0.0);
        let mut press_primary = false;
        let mut press_stop = false;
        let mut primary_rect = Rect::new(0.0, 0.0, 0.0, 0.0);
        let mut stop_rect = Rect::new(0.0, 0.0, 0.0, 0.0);
        ui.row(|ui| {
            ui.spacing(6.0);
            ui.allocate(Some(pad), 1.0);
            let p = ui.primary_button(primary);
            press_primary = p.clicked;
            primary_rect = p.rect;
            if snap.active || snap.wake.listening {
                let s = ui.button(stop_label);
                press_stop = s.clicked;
                stop_rect = s.rect;
            }
        });
        self.primary_rect = primary_rect;
        self.stop_rect = stop_rect;
        if press_primary {
            if snap.active {
                self.host.end();
            } else if !snap.configured {
                out.open_setup = true;
                out.commands.push(Command::OpenSettingsPage("ai".into()));
            } else {
                self.host.start();
            }
        }
        if press_stop {
            self.host.stop();
        }
        ui.gap(4.0);
        let privacy_style = ui.text_style(10.0, Weight::Regular, dim);
        centered(ui, &privacy_text(&snap), &privacy_style, 3);

        if self.compact {
            return;
        }
        ui.separator();
        self.details(ui, &snap, out);
        let wait = ui.memory().repaint_after();
        self.next_wake = wait.map(|s| now + s).or(self.orb.next_frame(now));
    }

    fn details(&mut self, ui: &mut Ui<'_>, snap: &VoiceSnapshot, _out: &mut VoiceOut) {
        ui.row(|ui| {
            ui.label_dim("Listening");
            ui.label(&snap.engine);
        });
        ui.row(|ui| {
            ui.label_dim("Speaking");
            ui.label(&snap.speaker);
        });
        if !snap.engine_ready && !snap.engine_problem.is_empty() {
            let invalid = ui.theme().invalid;
            let style = ui.text_style(11.0, Weight::Regular, invalid);
            ui.label_styled(&snap.engine_problem, &style);
        }

        // Microphone picker and test.
        ui.section_label("Microphone");
        let names: Vec<String> = self.mics.iter().map(|m| m.name.clone()).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let mut pick = self.mic_pick;
        if !refs.is_empty() {
            let resp = ui.enabled(!snap.active, |ui| ui.combo(&mut pick, &refs));
            if resp.changed && pick != self.mic_pick {
                self.mic_pick = pick;
                let id = self
                    .mics
                    .get(pick)
                    .map(|m| m.id.clone())
                    .unwrap_or_default();
                self.note = match self.service.edit_options(|o| {
                    o.microphone_id = id;
                    Ok(())
                }) {
                    Ok(()) => "Microphone saved. It is used the next time voice starts.".into(),
                    Err(e) => e,
                };
            }
        }
        let test = self.host.mic_state();
        ui.row(|ui| {
            ui.spacing(6.0);
            let label = if test.running {
                "Stop test"
            } else {
                "Test microphone"
            };
            if ui.enabled(!snap.active, |ui| ui.button(label)).clicked {
                self.host.mic_test(!test.running);
            }
            if ui.button("Refresh").clicked {
                self.load_mics();
            }
        });
        mic_test_line(ui, &test);
        if !self.note.is_empty() {
            let note = self.note.clone();
            ui.label_dim(&note);
        }

        // Wake word.
        if snap.wake.enabled || !snap.wake.text.is_empty() {
            ui.section_label("Wake word");
            ui.label_dim(&snap.wake.text);
            if snap.wake.enabled {
                let label = if snap.wake.paused {
                    "Resume wake word"
                } else {
                    "Pause wake word"
                };
                let resume = snap.wake.paused;
                let busy = snap.active;
                if ui.enabled(!busy, |ui| ui.button(label)).clicked {
                    self.host.set_wake_listening(resume);
                }
            }
        }

        // Timing, with the explanation behind a toggle.
        if let Some(detail) = &snap.timing_detail {
            let label = format!(
                "{} Turn timing",
                if self.detail_open {
                    "\u{25BE}"
                } else {
                    "\u{25B8}"
                }
            );
            if ui.button(&label).clicked {
                self.detail_open = !self.detail_open;
            }
            if self.detail_open {
                ui.label_dim(detail);
            }
        }
        if let Some(name) = &snap.sharing {
            let line = format!("Sharing {name} \u{b7} stop sharing in chat.");
            ui.label_dim(&line);
        }
    }
}

/// The microphone test result line with a live level bar.
pub fn mic_test_line(ui: &mut Ui<'_>, test: &MicTest) {
    if test.running {
        ui.progress_h(test.level.clamp(0.0, 1.0), 6.0);
        ui.label_dim(&format!(
            "Microphone on \u{b7} input level {}% \u{b7} testing for four seconds\u{2026}",
            (test.level.clamp(0.0, 1.0) * 100.0).round() as i32
        ));
    } else if !test.result.is_empty() {
        ui.label_dim(&test.result);
    }
}

/// Wraps `text` and draws each line centered, at most `max_lines` lines.
pub fn centered(ui: &mut Ui<'_>, text: &str, style: &naby_render::TextStyle, max_lines: usize) {
    let avail = ui.available_width();
    let mut lines = ui.fonts().wrap(text, style, avail);
    if lines.len() > max_lines {
        lines.truncate(max_lines);
        if let Some(last) = lines.last_mut() {
            if !last.ends_with('\u{2026}') {
                last.push('\u{2026}');
            }
        }
    }
    for line in lines {
        let w = ui.fonts().measure_width(&line, style).min(avail);
        let pad = ((avail - w) / 2.0).max(0.0);
        ui.row(|ui| {
            ui.spacing(0.0);
            ui.allocate(Some(pad), 1.0);
            ui.label_styled(&line, style);
        });
    }
}
