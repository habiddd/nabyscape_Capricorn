//! View-level tests: the chat page driven through a scripted provider, with PNG snapshots under
//! `target/snapshots/` (`assistant-*.png`).

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use naby_core::assistant::attachments::data_url;
use naby_core::assistant::transport::{HttpRequest, HttpResponse};
use naby_core::assistant::State;
use naby_core::settings::{presets, Settings};
use naby_render::snapshot::{count_differing, save, test_fonts};
use naby_render::{Canvas, Fonts, InputState, Key, KeyEvent, Rect, Theme};
use serde_json::json;

use super::chat::{ChatEnv, ChatView, Out};
use super::ports::{BasicPreparer, NoScreen, NoTools};
use super::service::test_support::{fixture, Fixture};
use super::session::{ChatModel, ChatSession};
use super::style::{Metrics, Palette};

pub(crate) fn red_png(w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().unwrap();
        let mut data = Vec::new();
        for y in 0..h {
            for x in 0..w {
                // A simple gradient so the picture is recognizable in a snapshot.
                data.extend_from_slice(&[
                    (60 + x * 150 / w.max(1)) as u8,
                    (80 + y * 120 / h.max(1)) as u8,
                    200,
                    255,
                ]);
            }
        }
        writer.write_image_data(&data).unwrap();
    }
    out
}

pub(crate) fn chat_reply(text: &str) -> HttpResponse {
    HttpResponse::new(
        200,
        "application/json",
        json!({"choices": [{"message": {"role": "assistant", "content": text}}]})
            .to_string()
            .into_bytes(),
    )
}

pub(crate) struct Rig {
    pub f: Fixture,
    pub session: Arc<ChatSession>,
    pub view: ChatView,
    pub fonts: Fonts,
    pub canvas: Canvas,
    pub settings: Settings,
    pub theme: Theme,
    pub now: f64,
    pub scale: f32,
}

impl Rig {
    pub fn new(
        configured: bool,
        handler: impl FnMut(&HttpRequest) -> naby_core::assistant::Result<HttpResponse> + Send + 'static,
    ) -> Option<Rig> {
        let fonts = test_fonts()?;
        let f = fixture(handler);
        if configured {
            let id = f.service.add_provider("Custom Chat Completions").unwrap();
            let mut p = f.service.provider().unwrap();
            p.name = "My provider".into();
            p.base_url = "https://provider.example/v1/".into();
            p.model = "chat-1".into();
            f.service.save_provider(&id, &p, "sk-test").unwrap();
        }
        let session = Arc::new(ChatSession::new(
            f.service.clone(),
            Arc::new(NoTools),
            Arc::new(NoScreen),
            Arc::new(BasicPreparer),
        ));
        let view = ChatView::new(session.clone());
        let settings = presets::recommended();
        let theme = Theme::from_settings(&settings);
        Some(Rig {
            f,
            session,
            view,
            fonts,
            canvas: Canvas::new(720, 700),
            settings,
            theme,
            now: 1.0,
            scale: 1.0,
        })
    }

    pub fn resize(&mut self, w: u32, h: u32, scale: f32) {
        self.canvas = Canvas::new(w, h);
        self.scale = scale;
    }

    pub fn frame(&mut self, mut input: InputState) -> Out {
        self.now += 0.016;
        input.time = self.now;
        let area = Rect::new(
            0.0,
            0.0,
            self.canvas.width() as f32,
            self.canvas.height() as f32,
        );
        let env = ChatEnv {
            settings: &self.settings,
            theme: &self.theme,
            metrics: Metrics::new(self.scale, 1.0),
            now: self.now,
            area,
            visible: true,
            active: true,
        };
        let mut out = Out::default();
        self.view
            .frame(&mut self.canvas, &self.fonts, &input, &env, &mut out);
        out
    }

    /// Paints a frame without input, repeating while the view asks for more.
    pub fn settle(&mut self) {
        for _ in 0..4 {
            self.frame(InputState::default());
        }
    }

    pub fn wait(&mut self, done: impl Fn(&ChatModel) -> bool) -> Arc<ChatModel> {
        let start = Instant::now();
        loop {
            self.frame(InputState::default());
            let model = self.session.snapshot();
            if done(&model) {
                self.settle();
                return model;
            }
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "timed out: {}",
                model.status
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    pub fn type_text(&mut self, text: &str) {
        self.frame(InputState::typed(text));
    }

    pub fn press_enter(&mut self) {
        self.frame(InputState::key(KeyEvent::new(Key::Enter)));
    }

    pub fn ink(&self) -> usize {
        let area = Rect::new(
            0.0,
            0.0,
            self.canvas.width() as f32,
            self.canvas.height() as f32,
        );
        count_differing(&self.canvas, area, Palette::from_theme(&self.theme).window)
    }
}

fn rich_reply() -> String {
    let image = data_url("image/png", &red_png(320, 180));
    format!(
        "## Quick answer\n\nHere is a **small example** with `inline code` and a [link](https://example.com/docs).\n\n- first point\n- second point that is long enough to wrap around when the window is narrow\n\n```rust\nfn main() {{\n    println!(\"hello from Naby\");\n}}\n```\n\n![chart]({image})"
    )
}

#[test]
fn composer_send_emits_the_request_and_streams_into_the_transcript() {
    let reply = rich_reply();
    let Some(mut rig) = Rig::new(true, move |request| {
        let body = request.json().unwrap();
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.last().unwrap()["content"], "Show me a code sample");
        Ok(chat_reply(&reply))
    }) else {
        return;
    };
    rig.settle();
    assert!(
        rig.view.composer_focused(),
        "the composer takes focus when the window opens"
    );
    rig.type_text("Show me a code sample");
    assert_eq!(rig.view.composer(), "Show me a code sample");
    rig.press_enter();
    assert_eq!(rig.view.composer(), "", "sending clears the composer");
    let model = rig.wait(|m| !m.busy && m.items.len() == 2);
    assert_eq!(model.items[0].text, "Show me a code sample");
    assert_eq!(
        model.items[1].attachments.len(),
        1,
        "the markdown image became a card"
    );
    assert_eq!(rig.f.seen.lock().unwrap().len(), 1);
    save("assistant-chat", &rig.canvas);
    assert!(rig.ink() > 20_000);
    // The reply contributed link, code and card hit regions: a click on a link emits OpenUrl.
}

#[test]
fn shift_enter_is_a_newline_and_enter_on_empty_does_nothing() {
    let Some(mut rig) = Rig::new(true, |_| Ok(chat_reply("ok"))) else {
        return;
    };
    rig.settle();
    rig.type_text("line one");
    rig.frame(InputState::key(KeyEvent::new(Key::Enter).shift()));
    rig.type_text("line two");
    assert_eq!(rig.view.composer(), "line one\nline two");
    assert!(rig.f.seen.lock().unwrap().is_empty());
    rig.view.set_composer("");
    rig.press_enter();
    assert!(rig.f.seen.lock().unwrap().is_empty());
}

#[test]
fn empty_state_without_a_provider_shows_the_welcome_and_recovery_controls() {
    let Some(mut rig) = Rig::new(false, |_| Ok(chat_reply("ok"))) else {
        return;
    };
    rig.settle();
    save("assistant-empty", &rig.canvas);
    assert!(rig.ink() > 8_000);
    // Typing and pressing Enter cannot send without a provider; the session reports it.
    rig.type_text("hello");
    rig.press_enter();
    let model = rig.wait(|m| !m.busy && m.state == State::Error);
    assert!(model.status.contains("provider"), "{}", model.status);
    assert_eq!(
        rig.view.composer(),
        "hello",
        "the text comes back after a failed send"
    );
}

#[test]
fn escape_stops_the_running_turn() {
    let Some(mut rig) = Rig::new(true, |_| {
        std::thread::sleep(Duration::from_millis(200));
        Ok(chat_reply("late"))
    }) else {
        return;
    };
    rig.settle();
    rig.type_text("slow one");
    rig.press_enter();
    rig.wait(|m| m.busy);
    rig.frame(InputState::key(KeyEvent::new(Key::Escape)));
    let model = rig.session.snapshot();
    assert!(!model.busy);
    assert!(model.status.starts_with("Session stopped"));
    std::thread::sleep(Duration::from_millis(300));
    assert!(rig.session.snapshot().items.is_empty());
}

#[test]
fn attachment_chips_appear_in_the_composer_with_their_notes() {
    let Some(mut rig) = Rig::new(true, |_| Ok(chat_reply("ok"))) else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let note = dir.path().join("meeting-notes.txt");
    std::fs::write(&note, "agenda").unwrap();
    let doc = dir.path().join("data.json");
    std::fs::write(&doc, "{}").unwrap();
    rig.settle();
    let empty_ink = rig.ink();
    rig.session
        .attach_files(vec![note.display().to_string(), doc.display().to_string()]);
    let model = rig.wait(|m| !m.importing && m.draft.len() == 2);
    assert_eq!(model.draft[0].file.name, "meeting-notes.txt");
    assert!(rig.ink() > empty_ink, "two chips add ink");
    save("assistant-chips", &rig.canvas);
    // Removing a chip through the model removes it from the composer.
    let first = model.draft[0].id;
    rig.session.remove_attachment(first);
    rig.settle();
    assert_eq!(rig.session.snapshot().draft.len(), 1);
}

#[test]
fn the_view_repaints_at_high_dpi_and_in_a_narrow_window() {
    let reply = rich_reply();
    let Some(mut rig) = Rig::new(true, move |_| Ok(chat_reply(&reply))) else {
        return;
    };
    rig.resize(420, 640, 1.5);
    rig.settle();
    rig.type_text("hi");
    rig.press_enter();
    rig.wait(|m| !m.busy && m.items.len() == 2);
    save("assistant-chat-narrow", &rig.canvas);
    assert!(rig.ink() > 10_000);
}

#[test]
fn hiding_the_window_releases_pixels_and_stops_the_clock() {
    let Some(mut rig) = Rig::new(true, |_| Ok(chat_reply("ok"))) else {
        return;
    };
    rig.settle();
    assert!(
        rig.view.next_frame(rig.now).is_some(),
        "the idle character animates while visible"
    );
    rig.view.on_hide();
    assert_eq!(rig.view.next_frame(rig.now), None);
    rig.settle();
    assert!(rig.f.wakes.load(Ordering::SeqCst) < 10_000);
}
