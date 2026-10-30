//! The assistant window's content (`AssistantWindow`, `AssistantWindowChrome`): the caption bar
//! with its own minimize/maximize/close buttons, the chat page and the AI setup pages.
//!
//! The Windows host owns the top-level window, DWM styling and message loop; this view draws
//! everything inside it and tells the host where the caption and the resize borders are
//! ([`chrome_hit`]).

use std::sync::Arc;

use naby_core::command::Command;
use naby_core::settings::Settings;
use naby_render::{
    Canvas, Fonts, Icon, InputState, Key, Rect, TextStyle, Theme, Ui, UiMemory, Weight,
};

use super::chat::{ChatEnv, ChatView, HostRequest, Out};
use super::orb::Orb;
use super::service::AssistantService;
use super::session::ChatSession;
use super::setup::{SetupOut, SetupTab, SetupView};
use super::style::{chat_theme, Metrics, Palette};
use super::voice::{NoVoice, SpokenTurn, VoiceEnv, VoiceHost, VoiceOut, VoiceRoom};

/// Caption bar height in design units (`CaptionHeight=38`).
pub const CAPTION_HEIGHT: f32 = 38.0;
/// Resize border thickness in design units (`ResizeBorderThickness=6`).
pub const RESIZE_BORDER: f32 = 6.0;
/// Width of one caption button.
pub const BUTTON_WIDTH: f32 = 40.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Chat,
    Setup,
    /// The voice room (`AssistantVoiceView`).
    Voice,
}

/// What the pointer is over, for `WM_NCHITTEST`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chrome {
    Client,
    Caption,
    Left,
    Right,
    Top,
    Bottom,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

/// Classifies a point (window pixels) of a `w` x `h` window. The three caption buttons are
/// client area. A maximized window has no resize borders.
pub fn chrome_hit(x: f32, y: f32, w: f32, h: f32, scale: f32, maximized: bool) -> Chrome {
    if x < 0.0 || y < 0.0 || x >= w || y >= h {
        return Chrome::Client;
    }
    let border = RESIZE_BORDER * scale;
    if !maximized {
        let (l, r, t, b) = (x < border, x >= w - border, y < border, y >= h - border);
        match (l, r, t, b) {
            (true, _, true, _) => return Chrome::TopLeft,
            (_, true, true, _) => return Chrome::TopRight,
            (true, _, _, true) => return Chrome::BottomLeft,
            (_, true, _, true) => return Chrome::BottomRight,
            (true, ..) => return Chrome::Left,
            (_, true, ..) => return Chrome::Right,
            (_, _, true, _) => return Chrome::Top,
            (_, _, _, true) => return Chrome::Bottom,
            _ => {}
        }
    }
    if y < CAPTION_HEIGHT * scale && x < w - 3.0 * BUTTON_WIDTH * scale {
        return Chrome::Caption;
    }
    Chrome::Client
}

/// Window commands the caption buttons ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowCommand {
    Minimize,
    ToggleMaximize,
    Close,
}

#[derive(Debug, Default)]
pub struct WindowOut {
    pub commands: Vec<Command>,
    pub requests: Vec<HostRequest>,
    pub window: Vec<WindowCommand>,
    pub clipboard: Option<String>,
}

pub struct WindowEnv<'a> {
    pub settings: &'a Settings,
    pub theme: &'a Theme,
    /// Device pixels per design unit.
    pub scale: f32,
    pub maximized: bool,
    /// The window is shown (not minimized).
    pub visible: bool,
    /// The window is the foreground window (the idle character only moves then).
    pub active: bool,
}

pub struct AssistantWindowView {
    service: AssistantService,
    session: Arc<ChatSession>,
    chat: ChatView,
    setup: SetupView,
    mode: Mode,
    voice: VoiceRoom,
    mem: UiMemory,
    orb: Orb,
    seen_service: u64,
    next_wake: Option<f64>,
    pressed_button: Option<usize>,
}

impl AssistantWindowView {
    pub fn new(service: AssistantService, session: Arc<ChatSession>) -> Self {
        Self {
            chat: ChatView::new(session.clone()),
            setup: SetupView::new(service.clone()),
            voice: VoiceRoom::new(service.clone(), Arc::new(NoVoice)),
            service,
            session,
            mode: Mode::Chat,
            mem: UiMemory::new(),
            orb: Orb::new(),
            seen_service: 0,
            next_wake: None,
            pressed_button: None,
        }
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn chat(&mut self) -> &mut ChatView {
        &mut self.chat
    }

    pub fn setup(&mut self) -> &mut SetupView {
        &mut self.setup
    }

    pub fn session(&self) -> &Arc<ChatSession> {
        &self.session
    }

    pub fn open_setup(&mut self, tab: SetupTab) {
        self.setup.open(tab);
        self.mode = Mode::Setup;
        self.mem.clear();
    }

    /// Connects the voice room to the host's voice runtime.
    pub fn set_voice(&mut self, host: Arc<dyn VoiceHost>) {
        self.voice = VoiceRoom::new(self.service.clone(), host);
    }

    /// The chat as the voice runtime sees it (shared conversation).
    pub fn spoken_turn(&self) -> SpokenTurn {
        SpokenTurn::new(self.session.clone())
    }

    pub fn open_voice(&mut self) {
        self.mode = Mode::Voice;
        self.mem.clear();
    }

    pub fn open_chat(&mut self) {
        self.mode = Mode::Chat;
        self.chat.focus_composer();
        self.mem.clear();
    }

    /// Something changed that needs a frame (a worker published, a check finished).
    pub fn dirty(&self) -> bool {
        self.chat.dirty()
            || self.seen_service != self.service.revision()
            || (self.mode == Mode::Voice && self.voice.dirty())
    }

    pub fn next_frame(&self, now: f64) -> Option<f64> {
        let chat = self.chat.next_frame(now);
        let own = match self.mode {
            Mode::Setup => self.orb.next_frame(now),
            Mode::Voice => self.voice.next_frame(now),
            Mode::Chat => None,
        };
        [chat, own, self.next_wake]
            .into_iter()
            .flatten()
            .reduce(f64::min)
    }

    /// The window was minimized or hidden: free pixel memory and stop running checks.
    pub fn on_hide(&mut self) {
        self.chat.on_hide();
        self.setup.on_hide();
        self.voice.on_hide();
        self.orb.set_visible(false);
        self.mem.clear();
        self.next_wake = None;
    }

    pub fn shutdown(&mut self) {
        self.on_hide();
        self.session.shutdown();
    }

    // ---- frame ----------------------------------------------------------------------------

    pub fn frame(
        &mut self,
        canvas: &mut Canvas,
        fonts: &Fonts,
        raw: &InputState,
        env: &WindowEnv,
        out: &mut WindowOut,
    ) {
        let m = Metrics::new(env.scale, 1.0);
        let m_text = Metrics::new(
            env.scale,
            naby_core::i18n::Accessibility::from_settings(env.settings).text_scale as f32,
        );
        let size = Rect::new(0.0, 0.0, canvas.width() as f32, canvas.height() as f32);
        let caption_h = m.px(CAPTION_HEIGHT);
        let page = Rect::new(0.0, caption_h, size.w, (size.h - caption_h).max(0.0));
        self.seen_service = self.service.revision();
        let theme = chat_theme(env.theme, &m_text, true);
        let pal = Palette::from_theme(&theme);

        // The caption bar first: its buttons also take the pointer.
        let mut input = raw.clone();
        let button =
            self.caption_buttons(canvas, fonts, &pal, &m, size, caption_h, &mut input, out);
        let _ = button;

        match self.mode {
            Mode::Chat => {
                let mut chat_out = Out::default();
                let chat_env = ChatEnv {
                    settings: env.settings,
                    theme: env.theme,
                    metrics: m_text,
                    now: raw.time,
                    area: page,
                    visible: env.visible,
                    active: env.active,
                };
                self.chat
                    .frame(canvas, fonts, &input, &chat_env, &mut chat_out);
                out.commands.append(&mut chat_out.commands);
                out.requests.append(&mut chat_out.requests);
                out.clipboard = out.clipboard.take().or(chat_out.clipboard.take());
                if let Some(tab) = chat_out.open_setup {
                    self.open_setup(tab);
                }
            }
            Mode::Setup => {
                self.setup_frame(canvas, fonts, &input, env, &theme, &pal, &m_text, page, out)
            }
            Mode::Voice => {
                self.voice_frame(canvas, fonts, &input, env, &theme, &pal, &m_text, page, out)
            }
        }
        // `Command::OpenVoice` from the chat or setup pages is this window's own Voice tab.
        if let Some(i) = out.commands.iter().position(|c| *c == Command::OpenVoice) {
            out.commands.remove(i);
            self.open_voice();
        }
        // The title is drawn last so the page never covers it.
        self.paint_title(canvas, fonts, &pal, &m_text, caption_h);
    }

    #[allow(clippy::too_many_arguments)]
    fn caption_buttons(
        &mut self,
        canvas: &mut Canvas,
        fonts: &Fonts,
        pal: &Palette,
        m: &Metrics,
        size: Rect,
        caption_h: f32,
        input: &mut InputState,
        out: &mut WindowOut,
    ) -> Option<usize> {
        let _ = fonts;
        canvas.fill_rect(Rect::new(0.0, 0.0, size.w, caption_h), pal.caption);
        let bw = m.px(BUTTON_WIDTH);
        let specs = [
            (Icon::Minus, WindowCommand::Minimize),
            (Icon::NavStatus, WindowCommand::ToggleMaximize),
            (Icon::X, WindowCommand::Close),
        ];
        let mut hovered = None;
        for (i, (icon, command)) in specs.iter().enumerate() {
            let x = size.w - bw * (3 - i) as f32;
            let r = Rect::new(x, 0.0, bw, caption_h);
            let over = input.pointer.is_some_and(|p| r.contains_pt(p));
            if over {
                hovered = Some(i);
            }
            if over {
                let color = if matches!(command, WindowCommand::Close) {
                    naby_core::color::Rgba::rgb(0xC4, 0x2B, 0x1C)
                } else {
                    pal.control_fill
                };
                canvas.fill_rect(r, color);
            }
            let glyph = Rect::new(
                r.x + (r.w - m.px(12.0)) / 2.0,
                (r.h - m.px(12.0)) / 2.0,
                m.px(12.0),
                m.px(12.0),
            );
            match command {
                WindowCommand::ToggleMaximize => {
                    // A plain square (the original used a box glyph).
                    canvas.stroke_round_rect(
                        glyph.inset(m.px(0.5)),
                        m.px(1.5),
                        pal.text,
                        m.s.max(1.0) * 1.2,
                    );
                }
                _ => canvas.icon(*icon, glyph, pal.text, 2.0),
            }
            if input.pressed && over {
                self.pressed_button = Some(i);
            }
            if input.released && over && self.pressed_button == Some(i) {
                out.window.push(*command);
            }
        }
        if input.released || !input.down {
            self.pressed_button = None;
        }
        // The caption keeps its clicks.
        if hovered.is_some() || input.pointer.is_some_and(|p| p.1 < caption_h) {
            input.pressed = false;
            input.released = false;
            if hovered.is_some() {
                input.pointer = None;
            }
        }
        hovered
    }

    fn paint_title(
        &self,
        canvas: &mut Canvas,
        fonts: &Fonts,
        pal: &Palette,
        m: &Metrics,
        caption_h: f32,
    ) {
        let title = match self.mode {
            Mode::Chat => "NabyScape \u{b7} AI companion",
            Mode::Setup => "NabyScape \u{b7} AI setup",
            Mode::Voice => "NabyScape \u{b7} Naby voice",
        };
        let style = TextStyle::new(m.font(12.0), pal.text).weight(Weight::Semibold);
        let y = (caption_h - fonts.line_height(&style)) / 2.0;
        canvas.text(fonts, m.px(15.0), y, title, &style, Some(m.px(420.0)));
    }

    #[allow(clippy::too_many_arguments)]
    fn voice_frame(
        &mut self,
        canvas: &mut Canvas,
        fonts: &Fonts,
        raw: &InputState,
        env: &WindowEnv,
        theme: &Theme,
        pal: &Palette,
        m: &Metrics,
        page: Rect,
        out: &mut WindowOut,
    ) {
        let now = raw.time;
        canvas.fill_rect(page, pal.window);
        let inner = page.inset_ltrb(m.px(24.0), m.px(12.0), m.px(24.0), m.px(12.0));
        let mut voice_out = VoiceOut::default();
        let mut mem = std::mem::take(&mut self.mem);
        {
            let mut ui = Ui::new(canvas, fonts, theme, raw, &mut mem, page);
            let voice_env = VoiceEnv {
                now,
                reduce_motion: env.settings.reduce_motion,
                visible: env.visible,
                active: env.active,
            };
            ui.child(inner, |ui| {
                ui.spacing(m.px(6.0));
                ui.scroll_area("voice", |ui| {
                    self.voice.show(ui, &voice_env, &mut voice_out)
                });
            });
        }
        let wait = mem.repaint_after();
        if let Some(text) = mem.take_clipboard() {
            out.clipboard = Some(text);
        }
        let repaint = mem.take_repaint();
        self.mem = mem;
        self.next_wake = wait.map(|s| now + s).or(repaint.then_some(now));
        out.commands.append(&mut voice_out.commands);
        if voice_out.open_setup {
            self.open_setup(SetupTab::Voice);
        } else if voice_out.open_chat {
            self.open_chat();
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn setup_frame(
        &mut self,
        canvas: &mut Canvas,
        fonts: &Fonts,
        raw: &InputState,
        env: &WindowEnv,
        theme: &Theme,
        pal: &Palette,
        m: &Metrics,
        page: Rect,
        out: &mut WindowOut,
    ) {
        let now = raw.time;
        let mut input = raw.clone();
        let escape = input.keys.iter().any(|k| k.key == Key::Escape);
        input.keys.retain(|k| k.key != Key::Escape);
        canvas.fill_rect(page, pal.window);
        self.orb
            .set_reduce_motion(env.settings.reduce_motion || !env.active);
        self.orb.set_visible(env.visible);
        self.orb.tick(now);
        let inner = page.inset_ltrb(m.px(24.0), m.px(16.0), m.px(24.0), m.px(12.0));
        // Header: character and title.
        let orb_rect = Rect::new(inner.x, inner.y, m.px(54.0), m.px(54.0));
        self.orb.paint(canvas, orb_rect, now);
        let title = TextStyle::new(m.font(22.0), pal.text);
        let sub = TextStyle::new(m.font(12.0), pal.dim);
        let tx = orb_rect.right() + m.px(12.0);
        canvas.text(
            fonts,
            tx,
            orb_rect.y + m.px(6.0),
            "Your AI, your provider",
            &title,
            Some(inner.right() - tx),
        );
        canvas.text(
            fonts,
            tx,
            orb_rect.y + m.px(6.0) + fonts.line_height(&title) + m.px(2.0),
            "Connect an API. Choose how you chat, speak and share context.",
            &sub,
            Some(inner.right() - tx),
        );
        let top = orb_rect.bottom() + m.px(10.0);
        let mut setup_out = SetupOut::default();
        let mut mem = std::mem::take(&mut self.mem);
        {
            let mut ui = Ui::new(canvas, fonts, theme, &input, &mut mem, page);
            let back = Rect::new(
                inner.right() - m.px(150.0),
                inner.y + m.px(8.0),
                m.px(150.0),
                m.px(34.0),
            );
            let mut go_back = false;
            ui.child(back, |ui| {
                go_back = ui.button("\u{2190} Back to chat").clicked;
            });
            let body = Rect::new(inner.x, top, inner.w, (inner.bottom() - top).max(0.0));
            ui.child(body, |ui| {
                ui.spacing(m.px(8.0));
                ui.scroll_area("setup", |ui| {
                    self.setup.show(ui, &mut setup_out);
                });
            });
            if go_back || escape {
                setup_out.open_chat = true;
            }
        }
        let wait = mem.repaint_after();
        if let Some(text) = mem.take_clipboard() {
            out.clipboard = Some(text);
        }
        let repaint = mem.take_repaint();
        self.mem = mem;
        self.next_wake = wait.map(|s| now + s).or(repaint.then_some(now));
        out.commands.append(&mut setup_out.commands);
        if setup_out.open_chat {
            self.open_chat();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chrome_regions_follow_the_original_window_chrome() {
        let (w, h) = (720.0, 740.0);
        assert_eq!(chrome_hit(300.0, 20.0, w, h, 1.0, false), Chrome::Caption);
        assert_eq!(chrome_hit(300.0, 100.0, w, h, 1.0, false), Chrome::Client);
        // The caption buttons are client area.
        assert_eq!(chrome_hit(w - 20.0, 20.0, w, h, 1.0, false), Chrome::Client);
        assert_eq!(
            chrome_hit(w - 125.0, 20.0, w, h, 1.0, false),
            Chrome::Caption
        );
        assert_eq!(chrome_hit(2.0, 300.0, w, h, 1.0, false), Chrome::Left);
        assert_eq!(chrome_hit(w - 2.0, 300.0, w, h, 1.0, false), Chrome::Right);
        assert_eq!(chrome_hit(300.0, 2.0, w, h, 1.0, false), Chrome::Top);
        assert_eq!(chrome_hit(300.0, h - 2.0, w, h, 1.0, false), Chrome::Bottom);
        assert_eq!(chrome_hit(2.0, 2.0, w, h, 1.0, false), Chrome::TopLeft);
        assert_eq!(
            chrome_hit(w - 2.0, h - 2.0, w, h, 1.0, false),
            Chrome::BottomRight
        );
        // Maximized: no borders, caption reaches the top edge.
        assert_eq!(chrome_hit(2.0, 2.0, w, h, 1.0, true), Chrome::Caption);
        // High DPI scales the regions.
        assert_eq!(chrome_hit(300.0, 50.0, w, h, 2.0, false), Chrome::Caption);
        assert_eq!(chrome_hit(300.0, 80.0, w, h, 2.0, false), Chrome::Client);
    }
}
