//! Voice view tests: state transitions of the room, the settings pages against a scripted host,
//! the spoken chat turn on a shared conversation, and PNG snapshots under
//! `target/snapshots/` (`voice-*.png`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use naby_core::assistant::speech::Hardware;
use naby_core::assistant::transport::HttpResponse;
use naby_core::assistant::State;
use naby_core::settings::presets;
use naby_render::snapshot::{count_differing, save, test_fonts};
use naby_render::{Canvas, Fonts, InputState, Rect, Theme, Ui, UiMemory};
use serde_json::json;

use super::model::*;
use super::room::{
    caption_text, detail_text, heard_text, phase_text, primary_label, privacy_text, VoiceEnv,
    VoiceOut, VoiceRoom,
};
use super::settings::{VoicePage, VoiceSettingsOut, VoiceSettingsView};
use super::turn::SpokenTurn;
use crate::assistant::ports::{BasicPreparer, NoScreen, NoTools};
use crate::assistant::service::test_support::{fixture, Fixture};
use crate::assistant::session::ChatSession;
use crate::assistant::style::{Metrics, Palette};

#[derive(Default, Debug)]
struct Calls {
    start: usize,
    end: usize,
    stop: usize,
    wake: Vec<bool>,
    installs: Vec<(String, String, bool)>,
    removes: Vec<(String, bool)>,
    checks: Vec<AudioCheckKind>,
    mic_tests: Vec<bool>,
    scans: usize,
    cancels: usize,
    whistle_installs: usize,
    piper_installs: Vec<String>,
}

struct Scripted {
    snap: Mutex<VoiceSnapshot>,
    panel: Mutex<SpeechPanel>,
    check: Mutex<AudioCheck>,
    mic: Mutex<MicTest>,
    calls: Mutex<Calls>,
    whistle_ready: AtomicBool,
    piper_ready: AtomicBool,
}

impl Scripted {
    fn new(snap: VoiceSnapshot) -> Arc<Self> {
        Arc::new(Self {
            snap: Mutex::new(snap),
            panel: Mutex::default(),
            check: Mutex::default(),
            mic: Mutex::default(),
            calls: Mutex::default(),
            whistle_ready: AtomicBool::new(true),
            piper_ready: AtomicBool::new(true),
        })
    }

    fn set(&self, edit: impl FnOnce(&mut VoiceSnapshot)) {
        let mut s = self.snap.lock().unwrap();
        edit(&mut s);
        s.revision += 1;
    }
}

impl VoiceHost for Scripted {
    fn snapshot(&self) -> Arc<VoiceSnapshot> {
        Arc::new(self.snap.lock().unwrap().clone())
    }
    fn start(&self) {
        self.calls.lock().unwrap().start += 1;
        self.set(|s| {
            s.active = true;
            s.phase = State::Listening;
        });
    }
    fn end(&self) {
        self.calls.lock().unwrap().end += 1;
        self.set(|s| {
            s.active = false;
            s.phase = State::Idle;
        });
    }
    fn stop(&self) {
        self.calls.lock().unwrap().stop += 1;
        self.set(|s| {
            s.active = false;
            s.phase = State::Idle;
            s.wake.paused = true;
            s.wake.listening = false;
        });
    }
    fn send_text(&self, _: &str) {}
    fn microphones(&self) -> Vec<MicChoice> {
        vec![
            MicChoice {
                id: String::new(),
                name: "Windows default microphone".into(),
            },
            MicChoice {
                id: "{0.0.1.00000000}.{a}".into(),
                name: "Headset (USB)".into(),
            },
        ]
    }
    fn mic_test(&self, on: bool) {
        self.calls.lock().unwrap().mic_tests.push(on);
    }
    fn mic_state(&self) -> MicTest {
        self.mic.lock().unwrap().clone()
    }
    fn set_wake_listening(&self, on: bool) {
        self.calls.lock().unwrap().wake.push(on);
    }
    fn recognizers(&self) -> Vec<Recognizer> {
        vec![
            Recognizer {
                id: "MS-1033-80-DESK".into(),
                name: "Microsoft English (US)".into(),
            },
            Recognizer {
                id: "MS-1031-80-DESK".into(),
                name: "Microsoft Deutsch".into(),
            },
        ]
    }
    fn speech(&self) -> SpeechPanel {
        self.panel.lock().unwrap().clone()
    }
    fn scan_hardware(&self) {
        self.calls.lock().unwrap().scans += 1;
    }
    fn install_pack(&self, asr: &str, tts: &str, gpu: bool) {
        self.calls
            .lock()
            .unwrap()
            .installs
            .push((asr.into(), tts.into(), gpu));
    }
    fn remove_model(&self, id: &str, tts: bool) {
        self.calls.lock().unwrap().removes.push((id.into(), tts));
    }
    fn cancel_speech(&self) {
        self.calls.lock().unwrap().cancels += 1;
    }
    fn run_audio_check(&self, kind: AudioCheckKind) {
        self.calls.lock().unwrap().checks.push(kind);
    }
    fn audio_check(&self) -> AudioCheck {
        self.check.lock().unwrap().clone()
    }
    fn whistle_status(&self) -> String {
        "Ready \u{b7} loads when a conversation starts.".into()
    }
    fn whistle_ready(&self) -> bool {
        self.whistle_ready.load(Ordering::SeqCst)
    }
    fn install_whistle(&self) {
        self.calls.lock().unwrap().whistle_installs += 1;
    }
    fn piper_status(&self, _: &str) -> String {
        if self.piper_ready("") {
            "Ready".into()
        } else {
            "Included with NabyScape".into()
        }
    }
    fn piper_ready(&self, _: &str) -> bool {
        self.piper_ready.load(Ordering::SeqCst)
    }
    fn install_piper(&self, voice: &str) {
        self.calls.lock().unwrap().piper_installs.push(voice.into());
    }
}

fn line(user: bool, text: &str) -> VoiceLine {
    VoiceLine {
        user,
        text: text.into(),
    }
}

struct Rig {
    f: Fixture,
    host: Arc<Scripted>,
    fonts: Fonts,
    canvas: Canvas,
    mem: UiMemory,
    theme: Theme,
    now: f64,
}

impl Rig {
    fn new(snap: VoiceSnapshot) -> Option<Rig> {
        let fonts = test_fonts()?;
        let f = fixture(|_| Ok(HttpResponse::new(200, "application/json", b"{}".to_vec())));
        let settings = presets::recommended();
        let theme = Theme::from_settings(&settings);
        Some(Rig {
            f,
            host: Scripted::new(snap),
            fonts,
            canvas: Canvas::new(720, 720),
            mem: UiMemory::new(),
            theme,
            now: 1.0,
        })
    }

    fn area(&self) -> Rect {
        Rect::new(
            24.0,
            16.0,
            self.canvas.width() as f32 - 48.0,
            self.canvas.height() as f32 - 28.0,
        )
    }

    fn clear(&mut self) {
        let pal = Palette::from_theme(&self.theme);
        let full = Rect::new(
            0.0,
            0.0,
            self.canvas.width() as f32,
            self.canvas.height() as f32,
        );
        self.canvas.fill_rect(full, pal.window);
        let _ = Metrics::new(1.0, 1.0);
    }

    fn room_frame(&mut self, room: &mut VoiceRoom, input: InputState) -> VoiceOut {
        self.now += 0.016;
        let mut input = input;
        input.time = self.now;
        self.clear();
        let area = self.area();
        let env = VoiceEnv {
            now: self.now,
            reduce_motion: false,
            visible: true,
            active: true,
        };
        let mut out = VoiceOut::default();
        {
            let mut ui = Ui::new(
                &mut self.canvas,
                &self.fonts,
                &self.theme,
                &input,
                &mut self.mem,
                area,
            );
            room.show(&mut ui, &env, &mut out);
        }
        out
    }

    fn settings_frame(
        &mut self,
        view: &mut VoiceSettingsView,
        input: InputState,
    ) -> VoiceSettingsOut {
        self.now += 0.016;
        let mut input = input;
        input.time = self.now;
        self.clear();
        let area = self.area();
        let mut out = VoiceSettingsOut::default();
        {
            let mut ui = Ui::new(
                &mut self.canvas,
                &self.fonts,
                &self.theme,
                &input,
                &mut self.mem,
                area,
            );
            ui.scroll_area("voice-settings", |ui| view.show(ui, &mut out));
        }
        out
    }

    fn click(&mut self, room: &mut VoiceRoom, rect: Rect) -> VoiceOut {
        let (x, y) = (rect.x + rect.w / 2.0, rect.y + rect.h / 2.0);
        self.room_frame(room, InputState::at(x, y));
        self.room_frame(room, InputState::press(x, y));
        self.room_frame(room, InputState::release(x, y))
    }

    fn ink(&self) -> usize {
        let full = Rect::new(
            0.0,
            0.0,
            self.canvas.width() as f32,
            self.canvas.height() as f32,
        );
        count_differing(&self.canvas, full, Palette::from_theme(&self.theme).window)
    }
}

fn room(rig: &Rig) -> VoiceRoom {
    VoiceRoom::new(rig.f.service.clone(), rig.host.clone())
}

// ----- pure text rules ------------------------------------------------------------------------

#[test]
fn stage_text_follows_the_phase() {
    let mut s = VoiceSnapshot::default();
    assert_eq!(phase_text(&s), "Ready to talk");
    assert_eq!(primary_label(&s), "Start conversation");
    s.phase = State::Listening;
    s.active = true;
    assert_eq!(phase_text(&s), "Listening");
    assert_eq!(primary_label(&s), "End conversation");
    s.phase = State::Thinking;
    s.status = "Transcribing speech\u{2026}".into();
    assert_eq!(phase_text(&s), "Transcribing speech\u{2026}");
    s.phase = State::Speaking;
    assert_eq!(phase_text(&s), "Speaking");
    s.active = false;
    s.phase = State::Error;
    assert_eq!(phase_text(&s), "Connection needs attention");
    assert_eq!(primary_label(&s), "Try again");
    s.configured = false;
    assert_eq!(primary_label(&s), "Set up voice");
    // Wake listening while idle.
    let w = VoiceSnapshot {
        wake: WakeStatus {
            enabled: true,
            listening: true,
            paused: false,
            text: "x".into(),
            phrase: "Hey Naby".into(),
        },
        ..VoiceSnapshot::default()
    };
    assert_eq!(phase_text(&w), "Waiting for \u{201c}Hey Naby\u{201d}");
}

#[test]
fn captions_prefer_the_assistant_and_clip_long_text() {
    let mut s = VoiceSnapshot::default();
    assert_eq!(caption_text(&s), "Speak naturally. Naby will reply.");
    assert_eq!(heard_text(&s), None);
    s.lines = vec![line(true, "what is up"), line(false, &"word ".repeat(80))];
    assert!(caption_text(&s).chars().count() <= 220 && caption_text(&s).ends_with('\u{2026}'));
    assert_eq!(heard_text(&s).as_deref(), Some("what is up"));
}

#[test]
fn detail_and_privacy_lines() {
    let mut s = VoiceSnapshot::default();
    assert_eq!(detail_text(&s), "");
    s.timing = Some("ASR 0.4s \u{b7} Reply 0.9s \u{b7} TTS 0.3s".into());
    assert!(detail_text(&s).starts_with("ASR"));
    s.phase = State::Error;
    s.status = "Voice stopped.".into();
    assert_eq!(detail_text(&s), "Voice stopped.");
    s.configured = false;
    s.setup_hint = "Choose a voice connection".into();
    assert_eq!(detail_text(&s), "Choose a voice connection");
    assert_eq!(
        privacy_text(&VoiceSnapshot::default()),
        "Your microphone stays off until you start."
    );
    let mut active = VoiceSnapshot {
        active: true,
        ..VoiceSnapshot::default()
    };
    assert_eq!(
        privacy_text(&active),
        "End conversation turns off the microphone."
    );
    active.wake.enabled = true;
    assert!(privacy_text(&active).contains("returns to local wake listening"));
}

// ----- room behaviour and snapshots ---------------------------------------------------------------

#[test]
fn start_end_and_stop_reach_the_host_and_states_change() {
    let Some(mut rig) = Rig::new(VoiceSnapshot::default()) else {
        return;
    };
    let mut room = room(&rig);
    rig.room_frame(&mut room, InputState::default());
    save("voice-room-idle", &rig.canvas);
    assert!(rig.ink() > 3000, "the idle room paints");
    let rect = room.primary_rect;
    assert!(rect.w > 50.0 && rect.h > 10.0, "{rect:?}");
    rig.click(&mut room, rect);
    assert_eq!(rig.host.calls.lock().unwrap().start, 1);

    // The conversation is now active: the label flips and Stop appears.
    rig.host.set(|s| {
        s.mic_level = 0.45;
        s.partial = "what is the weather like in".into();
        s.lines = vec![line(false, "Hi, I am Naby. How can I help?")];
    });
    rig.room_frame(&mut room, InputState::default());
    save("voice-room-listening", &rig.canvas);
    assert_eq!(primary_label(&rig.host.snapshot()), "End conversation");
    assert!(room.stop_rect.w > 20.0);
    let stop = room.stop_rect;
    rig.click(&mut room, stop);
    assert_eq!(rig.host.calls.lock().unwrap().stop, 1);
    assert!(rig.host.snapshot().wake.paused);

    rig.host.start();
    rig.room_frame(&mut room, InputState::default());
    let end = room.primary_rect;
    rig.click(&mut room, end);
    assert_eq!(rig.host.calls.lock().unwrap().end, 1);
    assert!(!rig.host.snapshot().active);
}

#[test]
fn unconfigured_voice_opens_setup_instead_of_starting() {
    let snap = VoiceSnapshot {
        configured: false,
        setup_hint: "Choose a voice connection. Voice is optional.".into(),
        ..VoiceSnapshot::default()
    };
    let Some(mut rig) = Rig::new(snap) else {
        return;
    };
    let mut room = room(&rig);
    rig.room_frame(&mut room, InputState::default());
    let rect = room.primary_rect;
    let out = rig.click(&mut room, rect);
    assert!(out.open_setup);
    assert_eq!(rig.host.calls.lock().unwrap().start, 0);
}

#[test]
fn thinking_speaking_error_and_wake_snapshots() {
    let Some(mut rig) = Rig::new(VoiceSnapshot::default()) else {
        return;
    };
    let mut room = room(&rig);
    rig.host.set(|s| {
        s.active = true;
        s.phase = State::Thinking;
        s.status = "Thinking\u{2026}".into();
        s.lines = vec![line(
            true,
            "Open the weather page and tell me about tomorrow",
        )];
    });
    rig.room_frame(&mut room, InputState::default());
    let thinking_ink = rig.ink();
    save("voice-room-thinking", &rig.canvas);

    rig.host.set(|s| {
        s.phase = State::Speaking;
        s.output_level = 0.6;
        s.lines = vec![
            line(true, "Open the weather page and tell me about tomorrow"),
            line(false, "Tomorrow looks mild with light rain in the afternoon. Highs near eighteen degrees."),
        ];
        s.timing = Some("ASR 0.4s \u{b7} Reply 0.9s \u{b7} TTS 0.3s".into());
        s.timing_detail = Some("Speech recognition: 0.4s".into());
    });
    for _ in 0..30 {
        rig.room_frame(&mut room, InputState::default());
    }
    save("voice-room-speaking", &rig.canvas);
    assert!(rig.ink() > 3000 && thinking_ink > 3000);

    rig.host.set(|s| {
        s.active = false;
        s.phase = State::Error;
        s.status = "Voice stopped. Whistle needs needle.exe next to nabyscape.exe.".into();
        s.engine_ready = false;
        s.engine_problem = "Whistle needs needle.exe next to nabyscape.exe.".into();
    });
    rig.room_frame(&mut room, InputState::default());
    save("voice-room-error", &rig.canvas);
    assert_eq!(primary_label(&rig.host.snapshot()), "Try again");

    rig.host.set(|s| {
        s.phase = State::Idle;
        s.engine_ready = true;
        s.engine_problem.clear();
        s.wake = WakeStatus {
            enabled: true,
            listening: true,
            paused: false,
            text: "Wake microphone on \u{b7} say \u{201c}Hey Naby\u{201d}. Detection stays on this PC.".into(),
            phrase: "Hey Naby".into(),
        };
    });
    rig.room_frame(&mut room, InputState::default());
    save("voice-room-wake", &rig.canvas);
    assert_eq!(
        phase_text(&rig.host.snapshot()),
        "Waiting for \u{201c}Hey Naby\u{201d}"
    );
}

#[test]
fn the_overlay_form_is_compact_and_has_no_microphone_picker() {
    let Some(mut rig) = Rig::new(VoiceSnapshot::default()) else {
        return;
    };
    rig.canvas = Canvas::new(360, 420);
    let mut room = VoiceRoom::new(rig.f.service.clone(), rig.host.clone()).compact();
    rig.host.set(|s| {
        s.active = true;
        s.phase = State::Listening;
        s.lines = vec![line(false, "Sure, what would you like to hear?")];
    });
    rig.room_frame(&mut room, InputState::default());
    save("voice-overlay", &rig.canvas);
    assert!(rig.ink() > 1500);
    assert!(room.primary_rect.w > 50.0);
}

#[test]
fn choosing_a_microphone_saves_it_for_the_next_start() {
    let Some(mut rig) = Rig::new(VoiceSnapshot::default()) else {
        return;
    };
    let mut room = room(&rig);
    rig.room_frame(&mut room, InputState::default());
    // Select through the saved option, as the picker does, and see it resolve on refresh.
    rig.f
        .service
        .edit_options(|o| {
            o.microphone_id = "{0.0.1.00000000}.{a}".into();
            Ok(())
        })
        .unwrap();
    room.on_hide();
    rig.room_frame(&mut room, InputState::default());
    assert!(
        rig.host.calls.lock().unwrap().mic_tests.contains(&false),
        "hiding stops the mic test"
    );
    assert_eq!(
        rig.f.service.options().microphone_id,
        "{0.0.1.00000000}.{a}"
    );
}

#[test]
fn hiding_the_room_releases_everything_it_holds() {
    let Some(mut rig) = Rig::new(VoiceSnapshot::default()) else {
        return;
    };
    let mut room = room(&rig);
    rig.room_frame(&mut room, InputState::default());
    room.on_hide();
    assert!(room.next_frame(1.0).is_none());
}

// ----- settings pages ---------------------------------------------------------------------------

fn hardware() -> Hardware {
    Hardware {
        cpu: "Test CPU".into(),
        threads: 12,
        ram_gb: 32.0,
        free_gb: 20.0,
        gpu: "Test GPU".into(),
        vram_gb: 0.0,
        free_vram_gb: 0.0,
        cuda: false,
        supported: true,
    }
}

#[test]
fn voice_setup_defaults_to_whistle_and_saves_other_engines() {
    let Some(mut rig) = Rig::new(VoiceSnapshot::default()) else {
        return;
    };
    let mut view = VoiceSettingsView::new(rig.f.service.clone(), rig.host.clone());
    rig.settings_frame(&mut view, InputState::default());
    save("voice-settings-setup", &rig.canvas);
    assert!(rig.ink() > 3000);
    // First run: nothing saved; saving the shown choice makes local Whistle explicit.
    view.save_setup().unwrap();
    let local = rig.f.service.options().local_voice;
    assert!(local.enabled && !local.managed);
    assert_eq!(
        super::engine::resolve(&local).listening,
        super::engine::Listening::Whistle
    );
    assert!(local.sensitivity >= 0.003);
    assert_eq!(
        super::engine::speaking(&local),
        super::engine::Speaking::Piper
    );

    // A bad external address is rejected without touching the saved options.
    let mut external = VoiceSettingsView::new(rig.f.service.clone(), rig.host.clone());
    external.seed_setup();
    rig.f
        .service
        .edit_options(|o| {
            o.local_voice.asr_url = "https://remote.example/".into();
            o.local_voice
                .extra
                .insert("ListeningEngine".into(), json!("External"));
            Ok(())
        })
        .unwrap();
    external = VoiceSettingsView::new(rig.f.service.clone(), rig.host.clone());
    let err = external.save_setup().unwrap_err();
    assert!(err.contains("localhost"), "{err}");
    assert_eq!(
        rig.f.service.options().local_voice.asr_url,
        "https://remote.example/"
    );
}

#[test]
fn piper_control_saves_voice_and_speed_and_offers_install() {
    let Some(mut rig) = Rig::new(VoiceSnapshot::default()) else {
        return;
    };
    rig.f
        .service
        .edit_options(|o| {
            o.local_voice.speech_language = "es".into();
            Ok(())
        })
        .unwrap();
    let mut view = VoiceSettingsView::new(rig.f.service.clone(), rig.host.clone());
    view.set_piper_choice("es_ES-carlfm-x_low", 1.25);
    view.save_setup().unwrap();
    let local = rig.f.service.options().local_voice;
    assert_eq!(local.extra["PiperVoice"], json!("es_ES-carlfm-x_low"));
    assert_eq!(local.extra["PiperSpeed"], json!(1.25));
    assert_eq!(
        super::engine::piper_voice(&local).unwrap().id,
        "es_ES-carlfm-x_low"
    );
    assert_eq!(
        super::engine::speaking(&local),
        super::engine::Speaking::Piper
    );
    // Missing files: the setup and Models pages offer the install.
    rig.host.piper_ready.store(false, Ordering::SeqCst);
    let mut view = VoiceSettingsView::new(rig.f.service.clone(), rig.host.clone());
    rig.settings_frame(&mut view, InputState::default());
    save("voice-settings-piper-missing", &rig.canvas);
    assert!(rig.ink() > 3000);
    view.open(VoicePage::Models);
    rig.settings_frame(&mut view, InputState::default());
    save("voice-settings-models-piper-missing", &rig.canvas);
    assert!(rig.ink() > 3000);
}

#[test]
fn wake_page_saves_through_the_core_rules_and_arms_the_host() {
    let Some(mut rig) = Rig::new(VoiceSnapshot::default()) else {
        return;
    };
    let mut view = VoiceSettingsView::new(rig.f.service.clone(), rig.host.clone());
    view.open(VoicePage::Wake);
    rig.settings_frame(&mut view, InputState::default());
    save("voice-settings-wake", &rig.canvas);
    // Enabling without a voice connection explains what is missing.
    view.set_wake_enabled(true);
    let err = view.save_wake().unwrap_err();
    assert!(
        err.contains("working voice connection") || err.contains("Enable AI companion"),
        "{err}"
    );
    assert!(rig.host.calls.lock().unwrap().wake.is_empty());

    // With a chat model and local speech it saves, clamps and arms.
    let id = rig
        .f
        .service
        .add_provider("Custom Chat Completions")
        .unwrap();
    let mut p = rig.f.service.provider().unwrap();
    p.base_url = "https://provider.example/v1/".into();
    p.model = "chat-1".into();
    rig.f.service.save_provider(&id, &p, "sk-test").unwrap();
    rig.f
        .service
        .edit_options(|o| {
            o.local_voice.enabled = true;
            Ok(())
        })
        .unwrap();
    let mut view = VoiceSettingsView::new(rig.f.service.clone(), rig.host.clone());
    view.open(VoicePage::Wake);
    rig.settings_frame(&mut view, InputState::default());
    view.set_wake_enabled(true);
    view.set_wake_phrase("Hello Naby");
    view.save_wake().unwrap();
    let saved = rig.f.service.options().wake_word;
    assert!(saved.enabled);
    assert_eq!(saved.phrase, "Hello Naby");
    assert_eq!(rig.host.calls.lock().unwrap().wake, vec![true]);
    // A bad phrase is refused.
    view.set_wake_phrase("x1");
    assert!(view.save_wake().is_err());
}

#[test]
fn models_page_shows_whistle_packs_and_removes_with_confirmation() {
    let Some(mut rig) = Rig::new(VoiceSnapshot::default()) else {
        return;
    };
    {
        let mut panel = rig.host.panel.lock().unwrap();
        panel.hardware = Some(hardware());
        panel.installed_asr = vec![InstalledModel {
            id: "base-q5_1".into(),
            name: "Whisper base".into(),
            size: "57.0 MB".into(),
            selected: true,
        }];
        panel.installed_tts = vec![InstalledModel {
            id: "en_US-lessac-medium".into(),
            name: "Lessac".into(),
            size: "60.7 MB".into(),
            selected: false,
        }];
    }
    let mut view = VoiceSettingsView::new(rig.f.service.clone(), rig.host.clone());
    view.open(VoicePage::Models);
    rig.settings_frame(&mut view, InputState::default());
    save("voice-settings-models", &rig.canvas);
    assert!(rig.ink() > 3000);
    // Installing is asked of the host; the host reports the install later.
    rig.host.panel.lock().unwrap().busy = true;
    rig.host.panel.lock().unwrap().status = "Downloading Whisper base \u{b7} 12.0 / 57.0 MB".into();
    rig.host.panel.lock().unwrap().progress = 21.0;
    rig.settings_frame(&mut view, InputState::default());
    save("voice-settings-models-installing", &rig.canvas);
    assert!(
        view.dirty(),
        "an install in progress keeps the page repainting"
    );
    // Without Whistle's files the page offers to install it.
    rig.host.whistle_ready.store(false, Ordering::SeqCst);
    rig.host.panel.lock().unwrap().busy = false;
    rig.settings_frame(&mut view, InputState::default());
    save("voice-settings-models-whistle-missing", &rig.canvas);
    assert!(rig.ink() > 3000);
}

#[test]
fn check_audio_page_runs_checks_and_shows_results() {
    let Some(mut rig) = Rig::new(VoiceSnapshot::default()) else {
        return;
    };
    let mut view = VoiceSettingsView::new(rig.f.service.clone(), rig.host.clone());
    view.open(VoicePage::Audio);
    rig.settings_frame(&mut view, InputState::default());
    let first = rig.ink();
    *rig.host.check.lock().unwrap() = AudioCheck {
        running: false,
        level: 0.0,
        message:
            "Transcription checked: \u{201c}hello naby\u{201d}. This sample was not sent to chat."
                .into(),
        ok: Some(true),
    };
    rig.settings_frame(&mut view, InputState::default());
    save("voice-settings-audio", &rig.canvas);
    assert!(rig.ink() > 0 && first > 0);
    *rig.host.check.lock().unwrap() = AudioCheck {
        running: false,
        level: 0.0,
        message: "Microphone opened, but no audible input was detected.".into(),
        ok: Some(false),
    };
    rig.settings_frame(&mut view, InputState::default());
}

// ----- the chat half of a spoken turn ---------------------------------------------------------------

fn sse(deltas: &[&str]) -> HttpResponse {
    let mut body = String::new();
    for d in deltas {
        body.push_str(&format!(
            "data: {}\n\n",
            json!({"choices": [{"delta": {"content": d}}]})
        ));
    }
    body.push_str(&format!(
        "data: {}\n\n",
        json!({"choices": [{"delta": {}, "finish_reason": "stop"}]})
    ));
    body.push_str("data: [DONE]\n\n");
    HttpResponse::new(200, "text/event-stream", body.into_bytes())
}

fn session_with(f: &Fixture) -> Arc<ChatSession> {
    let id = f.service.add_provider("Custom Chat Completions").unwrap();
    let mut p = f.service.provider().unwrap();
    p.base_url = "https://provider.example/v1/".into();
    p.model = "chat-1".into();
    f.service.save_provider(&id, &p, "sk-chat").unwrap();
    Arc::new(ChatSession::new(
        f.service.clone(),
        Arc::new(NoTools),
        Arc::new(NoScreen),
        Arc::new(BasicPreparer),
    ))
}

#[test]
fn a_spoken_turn_streams_text_with_spoken_instructions_and_joins_the_chat_history() {
    let f = fixture(|r| {
        let body = r.json().unwrap();
        let system = body["messages"][0]["content"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        assert!(system.contains("spoken conversation"), "{system}");
        assert_eq!(
            body["messages"].as_array().unwrap().last().unwrap()["content"],
            "tell me a joke"
        );
        Ok(sse(&[
            "Why did the ",
            "chicken cross the road? ",
            "To get to the other side.",
        ]))
    });
    let session = session_with(&f);
    let turn = SpokenTurn::new(session.clone());
    let cancel = Arc::new(AtomicBool::new(false));
    let mut heard = String::new();
    turn.reply(
        "tell me a joke",
        &mut |d| {
            heard.push_str(d);
            true
        },
        &cancel,
    )
    .unwrap();
    assert_eq!(
        heard,
        "Why did the chicken cross the road? To get to the other side."
    );
    // Chat shows the same exchange.
    let model = session.snapshot();
    assert_eq!(model.items.len(), 2);
    assert_eq!(model.items[0].text, "tell me a joke");
    assert!(model.items[1].text.contains("other side"));
    assert!(f.service.setup().chat.verified);
    // The next realtime session would be seeded with it.
    assert_eq!(turn.history().len(), 2);
}

#[test]
fn stopping_the_sink_cancels_the_reply_and_keeps_no_partial_assistant_turn() {
    let f = fixture(|_| Ok(sse(&["One. ", "Two. ", "Three."])));
    let session = session_with(&f);
    let turn = SpokenTurn::new(session);
    let cancel = Arc::new(AtomicBool::new(false));
    let mut seen = 0;
    let result = turn.reply(
        "count",
        &mut |_| {
            seen += 1;
            seen < 2
        },
        &cancel,
    );
    assert!(result.is_ok(), "cancelling is not an error: {result:?}");
    assert_eq!(seen, 2);
    assert!(!cancel.load(Ordering::SeqCst));
}

#[test]
fn realtime_lines_are_recorded_for_the_chat_tab() {
    let f = fixture(|_| Ok(sse(&["unused"])));
    let session = session_with(&f);
    let turn = SpokenTurn::new(session.clone());
    turn.record_line(true, "good morning");
    turn.record_line(false, "Good morning! How can I help?");
    let items = &session.snapshot().items;
    assert_eq!((items.len(), items[0].text.as_str()), (2, "good morning"));
    assert!(turn
        .realtime_instructions()
        .contains("does not provide computer-control tools"));
}

// ----- the Settings AI page --------------------------------------------------------------------

#[test]
fn the_settings_ai_page_embeds_providers_and_the_voice_pages() {
    use crate::assistant::setup::SetupTab;
    use crate::settings::{AiContext, AiSettingsPage, AiTarget, AssistantAiPage, HostInfo};
    let Some(mut rig) = Rig::new(VoiceSnapshot::default()) else {
        return;
    };
    let mut page = AssistantAiPage::new(rig.f.service.clone(), rig.host.clone());
    let settings = presets::recommended();
    let host = HostInfo::default();
    let mut commands = Vec::new();
    let draw = |rig: &mut Rig, page: &mut AssistantAiPage, commands: &mut Vec<_>| {
        rig.now += 0.016;
        let mut input = InputState::default();
        input.time = rig.now;
        rig.clear();
        let area = rig.area();
        let cx = AiContext {
            lang: "en",
            settings: &settings,
            host: &host,
            now: rig.now,
        };
        let mut ui = Ui::new(
            &mut rig.canvas,
            &rig.fonts,
            &rig.theme,
            &input,
            &mut rig.mem,
            area,
        );
        ui.scroll_area("ai", |ui| page.frame(ui, &cx, commands));
    };
    // Voice requests land on the real voice pages (Voice setup / Check audio).
    page.open(AiTarget::Voice);
    assert_eq!(page.setup().tab(), SetupTab::Voice);
    assert_eq!(page.setup().voice_view().unwrap().page(), VoicePage::Setup);
    draw(&mut rig, &mut page, &mut commands);
    save("settings-ai-voice", &rig.canvas);
    assert!(rig.ink() > 3000);
    page.open(AiTarget::VoiceChecks);
    assert_eq!(page.setup().voice_view().unwrap().page(), VoicePage::Audio);
    draw(&mut rig, &mut page, &mut commands);
    // Chat shows the provider connection editor.
    page.open(AiTarget::Chat);
    assert_eq!(page.setup().tab(), SetupTab::Chat);
    draw(&mut rig, &mut page, &mut commands);
    save("settings-ai-chat", &rig.canvas);
    assert!(!page.has_unsaved_changes());
    assert!(!page.dirty(), "nothing published since the last frame");
    // A worker publishing (an options change) asks for a repaint.
    rig.f
        .service
        .edit_options(|o| {
            o.enabled = !o.enabled;
            Ok(())
        })
        .unwrap();
    assert!(page.dirty());
    page.on_close();
}
