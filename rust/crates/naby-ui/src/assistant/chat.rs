//! The chat page (`AssistantView`): header with the character, status and recovery controls, the
//! transcript, the composer with attachment chips, screen sharing and the footer.
//!
//! One call to [`ChatView::frame`] handles the input of a frame and paints it. Widgets from the
//! shared toolkit (buttons, the multiline composer) and custom regions (transcript links, card
//! actions, menus) both react to the same [`InputState`]; custom regions register hit regions
//! while painting and are resolved at the end of the frame.

use std::sync::Arc;

use naby_core::assistant::State;
use naby_core::color::Rgba;
use naby_core::command::Command;
use naby_core::settings::Settings;
use naby_render::{
    Canvas, Fonts, Icon, Image, InputState, Key, Rect, TextEditOpts, TextStyle, Theme, Ui,
    UiMemory, Weight,
};

use super::blocks::Ctx;
use super::orb::Orb;
use super::ports::ShareTarget;
use super::previews::{Preview, PreviewCache};
use super::service::AssistantService;
use super::session::{ChatModel, ChatSession, ShownFile};
use super::setup::SetupTab;
use super::style::{chat_theme, load_mono, Metrics, Palette};
use super::transcript::{draft_note, Action, Hits, Transcript};

/// Things only the platform host can do.
#[derive(Clone)]
pub enum HostRequest {
    /// Show the file picker; the host attaches the choice with `ChatSession::attach_files`.
    PickFiles,
    /// Put the attachment's image on the clipboard.
    CopyImage(Arc<ShownFile>),
    /// Ask where to save the attachment, then write it.
    SaveAttachment(Arc<ShownFile>),
}

impl std::fmt::Debug for HostRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            HostRequest::PickFiles => "PickFiles",
            HostRequest::CopyImage(_) => "CopyImage",
            HostRequest::SaveAttachment(_) => "SaveAttachment",
        })
    }
}

/// What a frame asks the host to do.
#[derive(Debug, Default)]
pub struct Out {
    pub commands: Vec<Command>,
    pub requests: Vec<HostRequest>,
    /// Switch the window to the setup pages.
    pub open_setup: Option<SetupTab>,
    /// Text a field copied or cut.
    pub clipboard: Option<String>,
}

/// Per-frame inputs from the window.
pub struct ChatEnv<'a> {
    pub settings: &'a Settings,
    pub theme: &'a Theme,
    pub metrics: Metrics,
    pub now: f64,
    /// The page area (below the caption bar), canvas pixels.
    pub area: Rect,
    pub visible: bool,
    /// The window is the foreground window; an inactive window keeps the character still.
    pub active: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Menu {
    Share,
    Options,
}

pub struct ChatView {
    session: Arc<ChatSession>,
    service: AssistantService,
    previews: PreviewCache,
    orb: Orb,
    mem: UiMemory,
    mono: Option<Fonts>,
    transcript: Transcript,
    input_text: String,
    // scrolling
    scroll: f32,
    at_bottom: bool,
    drag_grab: Option<f32>,
    scroll_shown_until: f64,
    last_content_h: f32,
    last_items: usize,
    // interaction
    press: Option<(Rect, Action)>,
    chip_hits: Hits,
    menu_anchor: Option<Rect>,
    menu: Option<Menu>,
    share_targets: Vec<ShareTarget>,
    menu_scroll: f32,
    lightbox: Option<Arc<ShownFile>>,
    preview_open: bool,
    draft_scroll: f32,
    composer_focused: bool,
    focus_request: bool,
    restored_serial: u64,
    // change tracking
    seen_revision: (u64, u64),
    seen_sizes: u64,
    next_wake: Option<f64>,
    last_now: f64,
    visible: bool,
    pub frames: u64,
}

impl ChatView {
    pub fn new(session: Arc<ChatSession>) -> Self {
        let service = session.service().clone();
        let previews = PreviewCache::new(session.media(), service.waker());
        Self {
            session,
            service,
            previews,
            orb: Orb::new(),
            mem: UiMemory::new(),
            mono: None,
            transcript: Transcript::default(),
            input_text: String::new(),
            scroll: 0.0,
            at_bottom: true,
            drag_grab: None,
            scroll_shown_until: 0.0,
            last_content_h: 0.0,
            last_items: 0,
            press: None,
            chip_hits: Vec::new(),
            menu_anchor: None,
            menu: None,
            share_targets: Vec::new(),
            menu_scroll: 0.0,
            lightbox: None,
            preview_open: false,
            draft_scroll: 0.0,
            composer_focused: false,
            focus_request: true,
            restored_serial: 0,
            seen_revision: (0, 0),
            seen_sizes: 0,
            next_wake: None,
            last_now: 0.0,
            visible: true,
            frames: 0,
        }
    }

    pub fn session(&self) -> &Arc<ChatSession> {
        &self.session
    }

    /// The composer text (tests, drafts).
    pub fn composer(&self) -> &str {
        &self.input_text
    }

    pub fn set_composer(&mut self, text: &str) {
        self.input_text = text.to_owned();
    }

    /// Asks for keyboard focus in the composer on the next frame.
    pub fn focus_composer(&mut self) {
        self.focus_request = true;
    }

    pub fn composer_focused(&self) -> bool {
        self.composer_focused
    }

    /// True when something changed since the last frame (session, service, previews).
    pub fn dirty(&self) -> bool {
        (self.session.revision(), self.service.revision()) != self.seen_revision
            || self.previews.size_version() != self.seen_sizes
    }

    /// When the next frame is due while nothing else happens (animation, caret, scrollbar).
    pub fn next_frame(&self, now: f64) -> Option<f64> {
        if !self.visible {
            return None;
        }
        let orb = self.orb.next_frame(now);
        match (orb, self.next_wake) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// The window hides or minimizes: drop pixel memory and stop animating.
    pub fn on_hide(&mut self) {
        self.visible = false;
        self.orb.set_visible(false);
        self.previews.release();
        self.transcript.release();
        self.mem.clear();
        self.mono = None;
        self.menu = None;
        self.lightbox = None;
        self.press = None;
        self.next_wake = None;
    }

    /// Stops what the session is doing (Escape, minimize, hide).
    pub fn stop_session(&self) {
        self.session.stop_session();
    }

    // ---- frame --------------------------------------------------------------------------

    pub fn frame(
        &mut self,
        canvas: &mut Canvas,
        fonts: &Fonts,
        raw: &InputState,
        env: &ChatEnv,
        out: &mut Out,
    ) {
        self.frames += 1;
        if !self.visible {
            self.visible = true;
        }
        let m = env.metrics;
        let now = raw.time;
        self.last_now = now;
        let theme = chat_theme(env.theme, &m, true);
        let pal = Palette::from_theme(&theme);
        let model = self.session.snapshot();
        self.seen_revision = (self.session.revision(), self.service.revision());
        self.seen_sizes = self.previews.size_version();
        if self.mono.is_none() {
            self.mono = Some(load_mono());
        }
        let mono = self.mono.take().unwrap_or_else(Fonts::empty);
        let cx = Ctx {
            fonts,
            mono: &mono,
            m,
            pal: &pal,
        };

        self.sync_service(&model);
        self.restore_failed_text(&model);
        self.orb_state(&model, env, now);

        // ---- input pre-processing -------------------------------------------------------
        let mut input = raw.clone();
        let mut send = false;
        let mut escape = false;
        if self.composer_focused {
            let shift_free = |k: &naby_render::KeyEvent| k.key == Key::Enter && !k.shift;
            if input.keys.iter().any(shift_free) {
                send = true;
                input.keys.retain(|k| !shift_free(k));
            }
        }
        if input.keys.iter().any(|k| k.key == Key::Escape) {
            escape = true;
            input.keys.retain(|k| k.key != Key::Escape);
        }
        if self.focus_request {
            input.keys.push(naby_render::KeyEvent::new(Key::Tab));
        }
        let overlay_open = self.menu.is_some() || self.lightbox.is_some();
        let ui_input = if overlay_open {
            // Widgets below a menu or the preview never see the pointer.
            InputState {
                pointer: None,
                pressed: false,
                released: false,
                down: false,
                wheel: (0.0, 0.0),
                ..input.clone()
            }
        } else {
            input.clone()
        };
        if escape {
            if self.lightbox.take().is_some() || self.menu.take().is_some() {
                // closed an overlay
            } else {
                self.session.stop_session();
            }
        }

        // ---- layout ---------------------------------------------------------------------
        let area = env.area;
        let pad_x = m.px(20.0);
        let inner = area.inset_ltrb(pad_x, m.px(12.0), pad_x, m.px(12.0));
        let mut mem = std::mem::take(&mut self.mem);
        let mut hits: Hits = Vec::new();
        {
            let mut ui = Ui::new(canvas, fonts, &theme, &ui_input, &mut mem, area);
            ui.canvas().fill_rect(area, pal.window);
            self.paint_page(&mut ui, &cx, &model, inner, &input, &mut hits, out, send);
            self.paint_overlays(&mut ui, &cx, &model, env, &input, &mut hits, out);
        }
        out.clipboard = out.clipboard.take().or_else(|| mem.take_clipboard());
        self.composer_focused_after(&mem);
        let wait = mem.repaint_after();
        let repaint = mem.take_repaint();
        self.mem = mem;
        self.mono = Some(mono);
        self.resolve_hits(&input, &hits, out);
        self.previews.trim();
        self.next_wake = wait
            .map(|secs| now + secs)
            .or(if repaint { Some(now) } else { None });
        if let Some(until) = (self.scroll_shown_until > now).then_some(self.scroll_shown_until) {
            self.next_wake = Some(self.next_wake.map_or(now + 0.05, |w| w.min(now + 0.05)));
            let _ = until;
        }
        if self.focus_request {
            self.focus_request = false;
        }
    }

    fn composer_focused_after(&mut self, mem: &UiMemory) {
        self.composer_focused = mem.wants_text_input();
    }

    fn sync_service(&mut self, model: &ChatModel) {
        let sharing = model.sharing.clone();
        self.service
            .update_capture(|capture| capture.sharing = sharing);
    }

    fn restore_failed_text(&mut self, model: &ChatModel) {
        if model.failed_serial > self.restored_serial {
            self.restored_serial = model.failed_serial;
            if self.input_text.is_empty() {
                if let Some(text) = &model.failed_text {
                    self.input_text = text.to_string();
                }
            }
        }
    }

    fn orb_state(&mut self, model: &ChatModel, env: &ChatEnv, now: f64) {
        self.orb
            .set_reduce_motion(env.settings.reduce_motion || !env.active);
        self.orb.set_visible(env.visible);
        let state = if model.busy && model.state == State::Idle {
            State::Thinking
        } else {
            model.state
        };
        self.orb.set_state(state, now);
        self.orb.tick(now);
    }

    // ---- page -----------------------------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    fn paint_page(
        &mut self,
        ui: &mut Ui<'_>,
        cx: &Ctx,
        model: &Arc<ChatModel>,
        inner: Rect,
        input: &InputState,
        hits: &mut Hits,
        out: &mut Out,
        send: bool,
    ) {
        let m = cx.m;
        let pal = cx.pal;
        let options = self.service.options();
        let provider = options.provider().cloned();
        let busy = model.busy || model.importing;

        // Bottom-up: footer, share row, composer; then header/status from the top.
        let footer_h = m.px(7.0) + m.px(1.0) + m.px(34.0) + m.px(7.0);
        let footer = Rect::new(inner.x, inner.bottom() - footer_h, inner.w, footer_h);
        let mut bottom = footer.y;

        let share_h = if model.sharing.is_some() {
            if self.preview_open && model.share_preview.is_some() {
                m.px(30.0) + m.px(138.0)
            } else {
                m.px(30.0)
            }
        } else {
            0.0
        };
        let share = Rect::new(inner.x, bottom - share_h, inner.w, share_h);
        bottom = share.y;

        // Composer geometry.
        let frame_w = inner.w;
        let editor_w = frame_w - m.px(11.0) - m.px(6.0);
        let edit_style = TextStyle::new(m.font(13.0), pal.text);
        let lh = cx.fonts.line_height(&edit_style).max(1.0);
        let content_w = (editor_w - m.px(22.0)).max(40.0);
        let lines = if self.input_text.is_empty() {
            1
        } else {
            cx.fonts
                .wrap_ranges(&self.input_text, &edit_style, content_w)
                .len()
        };
        let rows = lines.clamp(1, 3);
        let edit_h = rows as f32 * lh + 18.0;
        let chips = self.layout_chips(cx, model, provider.as_ref(), editor_w, busy);
        let chips_h = chips.height.min(m.px(106.0));
        let bar_h = m.px(32.0) + m.px(3.0);
        let composer_h = m.px(4.0) + chips_h + edit_h + bar_h + m.px(5.0);
        let composer = Rect::new(
            inner.x,
            bottom - composer_h - m.px(7.0),
            frame_w,
            composer_h,
        );
        bottom = composer.y - m.px(3.0);

        let header = Rect::new(inner.x, inner.y, inner.w, m.px(40.0));
        let mut top = header.bottom() + m.px(10.0);

        // Status block.
        let enabled = options.enabled;
        let configured = provider.is_some();
        let error = model.state == State::Error;
        let status_visible =
            !enabled || !configured || model.state != State::Idle || model.importing;
        let status_text = status_message(enabled, &model.status);
        let recovery = error || !enabled || !configured;
        let mut status_rect = None;
        if status_visible {
            let style = TextStyle::new(m.font(11.0), pal.dim);
            let mut lines = cx.fonts.wrap(&status_text, &style, inner.w - m.px(24.0));
            let overflow = lines.len() > 3;
            lines.truncate(3);
            if overflow {
                if let Some(last) = lines.last_mut() {
                    last.push('\u{2026}');
                }
            }
            let text_h = lines.len() as f32 * cx.fonts.line_height(&style);
            let buttons_h = if recovery {
                m.px(6.0) + m.px(34.0)
            } else {
                0.0
            };
            let h = m.px(9.0) * 2.0 + text_h + buttons_h;
            let rect = Rect::new(inner.x, top, inner.w, h);
            status_rect = Some((rect, lines, recovery, error));
            top = rect.bottom() + m.px(10.0);
        }

        // Transcript.
        let view = Rect::new(inner.x, top, inner.w, (bottom - top).max(m.px(44.0)));

        // ---- composer first (keyboard focus order) ---------------------------------------
        self.paint_composer(
            ui, cx, model, composer, editor_w, rows, edit_h, chips, busy, send, out,
        );

        // ---- transcript -------------------------------------------------------------------
        self.paint_transcript(ui, cx, model, view, input, hits, out);

        // ---- header -----------------------------------------------------------------------
        self.paint_header(ui, cx, model, header, input);

        // ---- status -----------------------------------------------------------------------
        if let Some((rect, lines, recovery, error)) = status_rect {
            let canvas = ui.canvas();
            canvas.fill_rrect(rect, m.px(10.0), pal.control_fill);
            let color = if error { pal.error } else { pal.dim };
            let style = TextStyle::new(m.font(11.0), color);
            let lh = cx.fonts.line_height(&style);
            for (i, line) in lines.iter().enumerate() {
                canvas.text(
                    cx.fonts,
                    rect.x + m.px(12.0),
                    rect.y + m.px(9.0) + i as f32 * lh,
                    line,
                    &style,
                    None,
                );
            }
            if recovery {
                let row = Rect::new(
                    rect.x + m.px(12.0),
                    rect.y + m.px(9.0) + lines.len() as f32 * lh + m.px(6.0),
                    rect.w - m.px(24.0),
                    m.px(34.0),
                );
                let mic = error && {
                    let s = status_text.to_ascii_lowercase();
                    s.contains("microphone") || s.contains("audio device")
                };
                ui.child(row, |ui| {
                    ui.spacing(m.px(6.0));
                    ui.row(|ui| {
                        ui.spacing(m.px(6.0));
                        if ui.button("Check connection").clicked {
                            out.open_setup = Some(SetupTab::Chat);
                        }
                        if error && ui.button("Voice settings").clicked {
                            out.commands.push(Command::OpenSettingsPage("ai".into()));
                        }
                        if mic && ui.button("Microphone settings").clicked {
                            out.commands
                                .push(Command::OpenPath("ms-settings:privacy-microphone".into()));
                        }
                    });
                });
            }
        }

        // ---- share row + footer -----------------------------------------------------------
        if model.sharing.is_some() {
            self.paint_share_row(ui, cx, model, share, input, hits);
        }
        self.paint_footer(ui, cx, footer, out);
    }

    // ---- header ---------------------------------------------------------------------------

    fn paint_header(
        &mut self,
        ui: &mut Ui<'_>,
        cx: &Ctx,
        model: &Arc<ChatModel>,
        header: Rect,
        input: &InputState,
    ) {
        let m = cx.m;
        let pal = cx.pal;
        let orb_rect = Rect::new(header.x, header.y, m.px(40.0), m.px(40.0));
        match input.pointer {
            Some(p) => {
                let (cx0, cy0) = orb_rect.center();
                let area = ui.canvas().bounds();
                self.orb.look_at(
                    (p.0 - cx0) / (m.px(100.0)).max(area.w * 0.45),
                    (p.1 - cy0) / (m.px(90.0)).max(area.h * 0.45),
                );
            }
            None => self.orb.look_at(0.0, 0.0),
        }
        self.orb.paint(ui.canvas(), orb_rect, input.time);
        let title_x = orb_rect.right() + m.px(9.0);
        let title = TextStyle::new(m.font(18.0), pal.text).weight(if ui.theme().bold_headings {
            Weight::Semibold
        } else {
            Weight::Regular
        });
        let capture = self.service.capture_summary();
        let caption = TextStyle::new(m.font(10.0), pal.dim);
        let th = cx.fonts.line_height(&title);
        let ch = cx.fonts.line_height(&caption);
        let block = th + m.px(2.0) + ch;
        let ty = header.y + (header.h - block) / 2.0;
        let buttons_w = m.px(32.0) * 2.0 + m.px(6.0);
        ui.canvas()
            .text(cx.fonts, title_x, ty, "Naby", &title, None);
        ui.canvas().text(
            cx.fonts,
            title_x,
            ty + th + m.px(2.0),
            &capture,
            &caption,
            Some((header.right() - title_x - buttons_w - m.px(8.0)).max(40.0)),
        );
        let show_stop = model.busy || model.importing || model.sharing.is_some();
        let show_clear = !model.items.is_empty() || !model.draft.is_empty();
        let count = usize::from(show_stop) + usize::from(show_clear);
        if count == 0 {
            return;
        }
        let w = count as f32 * m.px(32.0) + (count - 1) as f32 * m.px(3.0);
        let rect = Rect::new(
            header.right() - w,
            header.y + (header.h - m.px(32.0)) / 2.0,
            w,
            m.px(32.0),
        );
        let mut stop = false;
        let mut clear = false;
        ui.child(rect, |ui| {
            ui.spacing(m.px(3.0));
            ui.row(|ui| {
                ui.spacing(m.px(3.0));
                if show_stop {
                    stop = ui
                        .icon_button_sized(
                            Icon::Stop,
                            "Stop session and all microphones",
                            m.px(32.0),
                            false,
                        )
                        .clicked;
                }
                if show_clear {
                    clear = ui
                        .icon_button_sized(Icon::Trash, "Clear conversation", m.px(32.0), false)
                        .clicked;
                }
            });
        });
        if stop {
            self.session.stop_session();
        }
        if clear {
            self.session.clear();
            self.scroll = 0.0;
            self.at_bottom = true;
        }
    }

    // ---- transcript -----------------------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    fn paint_transcript(
        &mut self,
        ui: &mut Ui<'_>,
        cx: &Ctx,
        model: &Arc<ChatModel>,
        view: Rect,
        input: &InputState,
        hits: &mut Hits,
        out: &mut Out,
    ) {
        let m = cx.m;
        let pal = cx.pal;
        let gutter = m.px(14.0);
        let content_w = (view.w - gutter).max(m.px(120.0));
        self.transcript
            .prepare(cx, model, &self.previews, content_w);
        let content_h = self.transcript.content_h + m.px(10.0);
        let max_scroll = (content_h - view.h).max(0.0);
        let pointer_in = input.pointer.is_some_and(|p| view.contains_pt(p));
        let now = input.time;
        let manual =
            pointer_in && input.wheel.1 != 0.0 && self.lightbox.is_none() && self.menu.is_none();
        if manual {
            self.scroll -= input.wheel.1;
            self.scroll_shown_until = now + 0.75 + 0.28;
        }
        // Dragging the scrollbar.
        let track = Rect::new(
            view.right() - m.px(12.0),
            view.y + m.px(8.0),
            m.px(12.0),
            (view.h - m.px(16.0)).max(1.0),
        );
        let thumb_h = if content_h > 0.0 {
            (track.h * view.h / content_h).clamp(m.px(24.0).min(track.h), track.h)
        } else {
            track.h
        };
        let travel = (track.h - thumb_h).max(1.0);
        let thumb_y = track.y
            + travel
                * if max_scroll > 0.0 {
                    self.scroll.clamp(0.0, max_scroll) / max_scroll
                } else {
                    0.0
                };
        let gutter_rect = Rect::new(view.right() - gutter, view.y, gutter, view.h);
        if max_scroll > 0.0 && self.lightbox.is_none() && self.menu.is_none() {
            if let Some((px, py)) = input.pointer {
                if input.pressed && gutter_rect.contains(px, py) {
                    self.drag_grab = Some(if py >= thumb_y && py <= thumb_y + thumb_h {
                        py - thumb_y
                    } else {
                        thumb_h / 2.0
                    });
                }
                if let Some(grab) = self.drag_grab {
                    if input.down {
                        self.scroll = (py - track.y - grab) / travel * max_scroll;
                        self.scroll_shown_until = now + 0.75 + 0.28;
                    }
                }
            }
        }
        if !input.down {
            self.drag_grab = None;
        }
        // Stick to the newest message while the user is at the bottom.
        let grew = content_h != self.last_content_h || model.items.len() != self.last_items;
        if grew && self.at_bottom && !manual && self.drag_grab.is_none() {
            self.scroll = max_scroll;
        }
        self.last_content_h = content_h;
        self.last_items = model.items.len();
        self.scroll = self.scroll.clamp(0.0, max_scroll);
        if manual || self.drag_grab.is_some() || grew {
            self.at_bottom = max_scroll - self.scroll < m.px(30.0);
        }

        if model.items.is_empty() && model.live.is_none() {
            self.paint_welcome(ui, cx, model, view, out);
        } else {
            let canvas = ui.canvas();
            let hovered = self.transcript.paint(
                cx,
                canvas,
                view,
                self.scroll,
                model,
                &self.previews,
                input,
                hits,
            );
            let _ = hovered;
        }

        // Overlay scrollbar.
        if max_scroll > 0.0 {
            let canvas = ui.canvas();
            let alpha = if self.drag_grab.is_some() {
                0.65
            } else {
                let since = self.scroll_shown_until - now;
                if since > 0.28 {
                    0.65
                } else if since > 0.0 {
                    0.65 * (since / 0.28) as f32
                } else {
                    0.0
                }
            };
            if alpha > 0.0 {
                let thumb_y = track.y + travel * (self.scroll / max_scroll);
                let thumb = Rect::new(track.x + m.px(4.0), thumb_y, m.px(4.0), thumb_h);
                canvas.with_opacity(alpha, |c| c.fill_rrect(thumb, m.px(2.0), pal.dim));
            }
        }
    }

    fn paint_welcome(
        &mut self,
        ui: &mut Ui<'_>,
        cx: &Ctx,
        model: &Arc<ChatModel>,
        view: Rect,
        out: &mut Out,
    ) {
        let m = cx.m;
        let options = self.service.options();
        let setup = self.service.setup();
        let description = if !options.enabled {
            "Enable your companion in AI setup to begin."
        } else if !setup.chat.configured {
            "Connect your provider or Hermes to start chatting."
        } else {
            "Ask a question, attach a file, or switch to voice."
        };
        let _ = model;
        let rect = Rect::new(
            view.x + m.px(2.0),
            view.y + m.px(18.0),
            view.w - m.px(4.0),
            view.h - m.px(18.0),
        );
        ui.child(rect, |ui| {
            ui.spacing(m.px(7.0));
            ui.heading_sized("What\u{2019}s on your mind?", 18.0);
            ui.label_dim(description);
            ui.gap(m.px(7.0));
            ui.row(|ui| {
                ui.spacing(m.px(6.0));
                if ui.button("AI setup").clicked {
                    out.open_setup = Some(SetupTab::Overview);
                }
                if ui.button("Voice mode").clicked {
                    out.commands.push(Command::OpenVoice);
                }
            });
        });
    }

    // ---- composer -------------------------------------------------------------------------

    fn layout_chips(
        &self,
        cx: &Ctx,
        model: &ChatModel,
        provider: Option<&naby_core::assistant::Provider>,
        width: f32,
        busy: bool,
    ) -> Chips {
        let m = cx.m;
        if busy || model.draft.is_empty() {
            return Chips::default();
        }
        let chip_w = (m.px(240.0)).min((width - m.px(32.0)).max(m.px(100.0)));
        let note_style = TextStyle::new(m.font(10.0), cx.pal.dim);
        let mut items = Vec::new();
        let (mut x, mut y, mut row_h) = (0.0f32, 0.0f32, 0.0f32);
        for shown in &model.draft {
            let note = draft_note(shown, provider);
            let mut lines = cx.fonts.wrap(
                &note,
                &note_style,
                chip_w - m.px(20.0) - m.px(48.0) - m.px(30.0),
            );
            lines.truncate(6);
            let text_h = cx
                .fonts
                .line_height(&TextStyle::new(m.font(11.0), cx.pal.text))
                + m.px(4.0)
                + lines.len() as f32 * cx.fonts.line_height(&note_style);
            let h = m.px(20.0) + text_h.max(m.px(40.0));
            if x > 0.0 && x + chip_w > width {
                x = 0.0;
                y += row_h + m.px(8.0);
                row_h = 0.0;
            }
            items.push(Chip {
                shown: shown.clone(),
                rect: Rect::new(x, y, chip_w, h),
                lines,
            });
            x += chip_w + m.px(6.0);
            row_h = row_h.max(h);
        }
        Chips {
            height: y + row_h + m.px(8.0),
            items,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_composer(
        &mut self,
        ui: &mut Ui<'_>,
        cx: &Ctx,
        model: &Arc<ChatModel>,
        composer: Rect,
        editor_w: f32,
        rows: usize,
        edit_h: f32,
        chips: Chips,
        busy: bool,
        send: bool,
        out: &mut Out,
    ) {
        let m = cx.m;
        let pal = cx.pal;
        let enabled = self.service.enabled();
        ui.canvas()
            .fill_rrect(composer, m.px(12.0), pal.control_fill);
        ui.canvas()
            .stroke_round_rect(composer, m.px(12.0), pal.control_border, m.s.max(1.0));
        let x = composer.x + m.px(11.0);
        let mut y = composer.y + m.px(4.0);
        // Editor (first focusable widget).
        let chips_h = chips.height.min(m.px(106.0));
        let edit_rect = Rect::new(x, y + chips_h, editor_w, edit_h);
        let can_type = enabled;
        let mut changed = false;
        ui.child(edit_rect, |ui| {
            ui.enabled(can_type, |ui| {
                let opts = TextEditOpts::multiline(rows).hint("Message Naby\u{2026}");
                changed = ui.text_edit_with(&mut self.input_text, &opts).changed;
            });
        });
        let _ = changed;
        // Draft chips above the editor.
        if chips_h > 0.0 {
            let clip = Rect::new(x, y, editor_w, chips_h);
            let max_scroll = (chips.height - chips_h).max(0.0);
            let pointer_in = ui.input().pointer.is_some_and(|p| clip.contains_pt(p));
            if pointer_in && ui.input().wheel.1 != 0.0 {
                self.draft_scroll -= ui.input().wheel.1;
            }
            self.draft_scroll = self.draft_scroll.clamp(0.0, max_scroll);
            ui.canvas().push_clip_rect(clip);
            for chip in &chips.items {
                let rect = chip.rect.translate(clip.x, clip.y - self.draft_scroll);
                self.paint_chip(ui, cx, chip, rect, clip, out);
            }
            ui.canvas().pop_clip();
        }
        y += chips_h + edit_h + m.px(3.0);
        // Button bar.
        let bar = Rect::new(x, y, composer.right() - x - m.px(6.0), m.px(32.0));
        let sharing = model.sharing.is_some();
        let can_attach = enabled && !busy;
        let can_share = enabled && (!model.busy || sharing);
        let has_input = !self.input_text.trim().is_empty() || !model.draft.is_empty();
        let can_send = enabled && !busy && has_input;
        let (mut attach, mut share, mut do_send) = (false, false, false);
        ui.child(bar, |ui| {
            ui.row(|ui| {
                ui.spacing(m.px(3.0));
                ui.enabled(can_attach, |ui| {
                    attach = ui
                        .icon_button_sized(Icon::Paperclip, "Attach images, videos, PDFs and documents \u{b7} or drop files / paste an image", m.px(32.0), false)
                        .clicked;
                });
                ui.enabled(can_share, |ui| {
                    let tip = match &model.sharing {
                        Some(name) => format!("Stop sharing {name}"),
                        None => "Share a window or display".to_owned(),
                    };
                    share = ui.icon_button_sized(Icon::Screen, &tip, m.px(32.0), sharing).clicked;
                });
            });
        });
        let send_rect = Rect::new(bar.right() - m.px(32.0), bar.y, m.px(32.0), m.px(32.0));
        ui.child(send_rect, |ui| {
            ui.enabled(can_send, |ui| {
                do_send = ui
                    .icon_button_sized(Icon::Send, "Send", m.px(32.0), true)
                    .clicked;
            });
        });
        if attach {
            out.requests.push(HostRequest::PickFiles);
        }
        if share {
            if sharing {
                self.session.stop_sharing();
            } else {
                self.share_targets = self.session.share_targets();
                self.menu_scroll = 0.0;
                self.menu = Some(Menu::Share);
                self.menu_anchor =
                    Some(Rect::new(bar.x + m.px(35.0), bar.y, m.px(32.0), m.px(32.0)));
            }
        }
        if (send || do_send) && can_send {
            self.submit();
        }
    }

    fn submit(&mut self) {
        let text = self.input_text.trim().to_owned();
        if self.session.send(&text) {
            self.input_text.clear();
            self.at_bottom = true;
        }
    }

    fn paint_chip(
        &mut self,
        ui: &mut Ui<'_>,
        cx: &Ctx,
        chip: &Chip,
        rect: Rect,
        clip: Rect,
        out: &mut Out,
    ) {
        let _ = out;
        let m = cx.m;
        let pal = cx.pal;
        let canvas = ui.canvas();
        if rect.bottom() < clip.y || rect.y > clip.bottom() {
            return;
        }
        canvas.fill_rrect(rect, m.px(10.0), pal.card);
        canvas.stroke_round_rect(rect, m.px(10.0), pal.card_border, m.s.max(1.0));
        let pad = m.px(10.0);
        let thumb = Rect::new(rect.x + pad, rect.y + pad, m.px(40.0), m.px(40.0));
        let mut drew = false;
        if PreviewCache::has_preview(&chip.shown) {
            if let Preview::Ready(image) = self.previews.get(&chip.shown, 96) {
                draw_cover(canvas, &image, thumb, m.px(6.0));
                drew = true;
            }
        }
        if !drew {
            let icon = if chip.shown.file.is_image() {
                Icon::Image
            } else {
                Icon::FileText
            };
            canvas.icon(icon, thumb.inset(m.px(12.0)), pal.dim, 1.6);
        }
        let text_x = thumb.right() + m.px(8.0);
        let name = TextStyle::new(m.font(11.0), pal.text);
        canvas.text(
            cx.fonts,
            text_x,
            rect.y + pad,
            &chip.shown.file.name,
            &name,
            Some(rect.right() - text_x - pad - m.px(26.0)),
        );
        let note = TextStyle::new(m.font(10.0), pal.dim);
        let lh = cx.fonts.line_height(&note);
        let mut y = rect.y + pad + cx.fonts.line_height(&name) + m.px(4.0);
        for line in &chip.lines {
            canvas.text(
                cx.fonts,
                text_x,
                y,
                line,
                &note,
                Some(rect.right() - text_x - pad),
            );
            y += lh;
        }
        // Remove button.
        let x_rect = Rect::new(
            rect.right() - m.px(30.0),
            rect.y + m.px(4.0),
            m.px(26.0),
            m.px(26.0),
        );
        let over = ui
            .input()
            .pointer
            .is_some_and(|p| x_rect.contains_pt(p) && clip.contains_pt(p));
        let canvas = ui.canvas();
        if over {
            canvas.fill_rrect(x_rect, m.px(8.0), pal.selection_fill);
        }
        canvas.icon(
            Icon::X,
            x_rect.inset(m.px(6.5)),
            if over { pal.selection_text } else { pal.dim },
            1.6,
        );
        self.chip_hits
            .push((x_rect, Action::RemoveDraft(chip.shown.id)));
    }

    // ---- share row, footer ---------------------------------------------------------------

    fn paint_share_row(
        &mut self,
        ui: &mut Ui<'_>,
        cx: &Ctx,
        model: &Arc<ChatModel>,
        rect: Rect,
        input: &InputState,
        hits: &mut Hits,
    ) {
        let m = cx.m;
        let pal = cx.pal;
        let name = model.sharing.clone().unwrap_or_default();
        let header = Rect::new(rect.x, rect.y, rect.w, m.px(28.0));
        let over = input.pointer.is_some_and(|p| header.contains_pt(p)) && self.menu.is_none();
        let canvas = ui.canvas();
        if over {
            canvas.fill_rrect(header, m.px(8.0), pal.control_fill);
        }
        let style = TextStyle::new(m.font(11.0), pal.dim);
        let chevron = if self.preview_open {
            "\u{25BE}"
        } else {
            "\u{25B8}"
        };
        let label = format!("{chevron}  Sharing \u{b7} {name}");
        canvas.text(
            cx.fonts,
            header.x + m.px(8.0),
            header.y + (header.h - cx.fonts.line_height(&style)) / 2.0,
            &label,
            &style,
            Some(header.w - m.px(16.0)),
        );
        hits.push((header, Action::TogglePreview));
        if self.preview_open {
            if let Some(image) = &model.share_preview {
                let box_rect = Rect::new(rect.x, header.bottom() + m.px(4.0), rect.w, m.px(130.0));
                let (w, h) = Image::fit_size(
                    image.width(),
                    image.height(),
                    box_rect.w as u32,
                    box_rect.h as u32,
                );
                canvas.draw_image(
                    image,
                    Rect::new(box_rect.x, box_rect.y, w as f32, h as f32),
                    1.0,
                    m.px(6.0),
                );
            }
        }
    }

    fn paint_footer(&mut self, ui: &mut Ui<'_>, cx: &Ctx, footer: Rect, out: &mut Out) {
        let m = cx.m;
        let pal = cx.pal;
        ui.canvas().fill_rect(
            Rect::new(footer.x, footer.y + m.px(7.0), footer.w, m.s.max(1.0)),
            Rgba::rgb(0x24, 0x28, 0x2E),
        );
        let row = Rect::new(
            footer.x,
            footer.y + m.px(7.0) + m.px(1.0) + m.px(7.0),
            footer.w,
            m.px(34.0),
        );
        ui.child(row, |ui| {
            ui.row(|ui| {
                ui.spacing(m.px(4.0));
                // Chat is the current mode; Voice is its own room.
                ui.primary_button("Chat");
                if ui.button("Voice").clicked {
                    out.commands.push(Command::OpenVoice);
                }
            });
        });
        // Options button on the right.
        let label = "Options";
        let style = TextStyle::new(m.font(13.0), pal.text);
        let w = cx.fonts.measure_width(label, &style) + m.px(28.0);
        let rect = Rect::new(row.right() - w, row.y, w, row.h);
        let mut open = false;
        ui.child(rect, |ui| {
            open = ui.button(label).clicked;
        });
        if open {
            self.menu = Some(Menu::Options);
            self.menu_anchor = Some(rect);
        }
    }

    // ---- overlays (menus, preview) -------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    fn paint_overlays(
        &mut self,
        ui: &mut Ui<'_>,
        cx: &Ctx,
        model: &Arc<ChatModel>,
        env: &ChatEnv,
        input: &InputState,
        hits: &mut Hits,
        out: &mut Out,
    ) {
        let _ = (model, out);
        let m = cx.m;
        let pal = cx.pal;
        let area = env.area;
        // Chip hit regions registered while painting the composer.
        hits.append(&mut self.chip_hits);
        if let Some(shown) = self.lightbox.clone() {
            let canvas = ui.canvas();
            canvas.fill_rect(area, Rgba::BLACK.with_alpha(0xD9));
            hits.push((area, Action::CloseLightbox));
            let stage = area.inset(m.px(24.0));
            let pixels = (stage.w.max(stage.h).min(1280.0)) as u32;
            match self.previews.get(&shown, pixels) {
                Preview::Ready(image) => {
                    let (w, h) = Image::fit_size(
                        image.width(),
                        image.height(),
                        stage.w as u32,
                        stage.h as u32,
                    );
                    let r = Rect::new(
                        stage.x + (stage.w - w as f32) / 2.0,
                        stage.y + (stage.h - h as f32) / 2.0,
                        w as f32,
                        h as f32,
                    );
                    canvas.draw_image(&image, r, 1.0, m.px(8.0));
                }
                Preview::Loading => {
                    let style = TextStyle::new(m.font(12.0), pal.dim);
                    canvas.text(
                        cx.fonts,
                        stage.x + m.px(8.0),
                        stage.y + m.px(8.0),
                        "Loading\u{2026}",
                        &style,
                        None,
                    );
                }
                _ => {
                    let style = TextStyle::new(m.font(12.0), pal.dim);
                    canvas.text(
                        cx.fonts,
                        stage.x + m.px(8.0),
                        stage.y + m.px(8.0),
                        "Could not open the image preview. Try Save instead.",
                        &style,
                        None,
                    );
                }
            }
            let close = Rect::new(
                area.right() - m.px(48.0),
                area.y + m.px(12.0),
                m.px(32.0),
                m.px(32.0),
            );
            canvas.fill_rrect(close, m.px(16.0), pal.control_fill);
            canvas.icon(Icon::X, close.inset(m.px(8.0)), pal.text, 1.6);
            return;
        }
        let Some(menu) = self.menu else { return };
        hits.push((area, Action::CloseMenus));
        let anchor = self.menu_anchor.unwrap_or(area);
        match menu {
            Menu::Share => self.paint_share_menu(ui, cx, anchor, area, input, hits),
            Menu::Options => self.paint_options_menu(ui, cx, anchor, area, input, hits),
        }
    }

    fn popup_rect(
        &self,
        cx: &Ctx,
        anchor: Rect,
        area: Rect,
        w: f32,
        h: f32,
        align_right: bool,
    ) -> Rect {
        let m = cx.m;
        let x = if align_right {
            anchor.right() - w
        } else {
            anchor.x
        };
        let x = x.clamp(
            area.x + m.px(8.0),
            (area.right() - w - m.px(8.0)).max(area.x),
        );
        let y = (anchor.y - h - m.px(6.0)).max(area.y + m.px(8.0));
        Rect::new(x, y, w, h)
    }

    fn paint_share_menu(
        &mut self,
        ui: &mut Ui<'_>,
        cx: &Ctx,
        anchor: Rect,
        area: Rect,
        input: &InputState,
        hits: &mut Hits,
    ) {
        let m = cx.m;
        let pal = cx.pal;
        let row_h = m.px(32.0);
        let title_h = m.px(30.0);
        let visible_rows = self.share_targets.len().clamp(1, 7);
        let w = m.px(340.0).min(area.w - m.px(16.0));
        let h = title_h + visible_rows as f32 * row_h + m.px(8.0);
        let rect = self.popup_rect(cx, anchor, area, w, h, false);
        let max_scroll = (self.share_targets.len().saturating_sub(visible_rows)) as f32 * row_h;
        if input.pointer.is_some_and(|p| rect.contains_pt(p)) && input.wheel.1 != 0.0 {
            self.menu_scroll = (self.menu_scroll - input.wheel.1).clamp(0.0, max_scroll);
        }
        let canvas = ui.canvas();
        canvas.fill_rrect(rect, m.px(10.0), pal.control_fill);
        canvas.stroke_round_rect(rect, m.px(10.0), pal.control_border, m.s.max(1.0));
        hits.push((rect, Action::Noop));
        let head = TextStyle::new(m.font(11.0), pal.dim);
        canvas.text(
            cx.fonts,
            rect.x + m.px(12.0),
            rect.y + (title_h - cx.fonts.line_height(&head)) / 2.0 + m.px(2.0),
            "Share a window or display",
            &head,
            None,
        );
        let list = Rect::new(
            rect.x + m.px(4.0),
            rect.y + title_h,
            rect.w - m.px(8.0),
            visible_rows as f32 * row_h,
        );
        canvas.push_clip_rect(list);
        let style = TextStyle::new(m.font(12.0), pal.text);
        if self.share_targets.is_empty() {
            canvas.text(
                cx.fonts,
                list.x + m.px(8.0),
                list.y + (row_h - cx.fonts.line_height(&style)) / 2.0,
                "No windows or displays found",
                &TextStyle::new(m.font(12.0), pal.dim),
                None,
            );
        }
        for (i, target) in self.share_targets.iter().enumerate() {
            let r = Rect::new(
                list.x,
                list.y + i as f32 * row_h - self.menu_scroll,
                list.w,
                row_h,
            );
            if r.bottom() < list.y || r.y > list.bottom() {
                continue;
            }
            let over = input
                .pointer
                .is_some_and(|p| r.contains_pt(p) && list.contains_pt(p));
            if over {
                canvas.fill_rrect(r, m.px(7.0), pal.selection_fill);
            }
            let color = if over { pal.selection_text } else { pal.text };
            let icon = if target.display {
                Icon::Screen
            } else {
                Icon::NavStatus
            };
            canvas.icon(
                icon,
                Rect::new(
                    r.x + m.px(8.0),
                    r.y + (r.h - m.px(15.0)) / 2.0,
                    m.px(15.0),
                    m.px(15.0),
                ),
                color,
                1.6,
            );
            canvas.text(
                cx.fonts,
                r.x + m.px(32.0),
                r.y + (r.h - cx.fonts.line_height(&style)) / 2.0,
                &target.name,
                &TextStyle { color, ..style },
                Some(r.w - m.px(40.0)),
            );
            hits.push((
                r.intersect(&list).unwrap_or(r),
                Action::PickShare(target.id.clone()),
            ));
        }
        canvas.pop_clip();
    }

    fn paint_options_menu(
        &mut self,
        ui: &mut Ui<'_>,
        cx: &Ctx,
        anchor: Rect,
        area: Rect,
        input: &InputState,
        hits: &mut Hits,
    ) {
        let m = cx.m;
        let pal = cx.pal;
        let caption = self.options_caption();
        let items: [(&str, &str, Icon, Action); 3] = [
            (
                "AI settings",
                "Providers & API keys",
                Icon::Settings,
                Action::OpenSetup(SetupTab::Chat),
            ),
            (
                "Voice settings",
                "Models & microphone",
                Icon::Mic,
                Action::OpenSettingsPage("ai"),
            ),
            (
                "Connect Hermes",
                "Connect or manage your own agent",
                Icon::Link,
                Action::OpenSetup(SetupTab::Hermes),
            ),
        ];
        let row_h = m.px(34.0);
        let w = m.px(260.0).min(area.w - m.px(16.0));
        let caption_style = TextStyle::new(m.font(11.0), pal.dim);
        let caption_lines = cx.fonts.wrap(&caption, &caption_style, w - m.px(28.0));
        let caption_h =
            caption_lines.len().min(3) as f32 * cx.fonts.line_height(&caption_style) + m.px(14.0);
        let h = caption_h + m.px(8.0) + items.len() as f32 * row_h + m.px(6.0);
        let rect = self.popup_rect(cx, anchor, area, w, h, true);
        let canvas = ui.canvas();
        canvas.fill_rrect(rect, m.px(10.0), pal.control_fill);
        canvas.stroke_round_rect(rect, m.px(10.0), pal.control_border, m.s.max(1.0));
        hits.push((rect, Action::Noop));
        for (i, line) in caption_lines.iter().take(3).enumerate() {
            canvas.text(
                cx.fonts,
                rect.x + m.px(14.0),
                rect.y + m.px(8.0) + i as f32 * cx.fonts.line_height(&caption_style),
                line,
                &caption_style,
                None,
            );
        }
        canvas.fill_rect(
            Rect::new(
                rect.x + m.px(8.0),
                rect.y + caption_h,
                rect.w - m.px(16.0),
                m.s.max(1.0),
            ),
            pal.rule,
        );
        let style = TextStyle::new(m.font(12.0), pal.text);
        for (i, (title, _hint, icon, action)) in items.into_iter().enumerate() {
            let r = Rect::new(
                rect.x + m.px(4.0),
                rect.y + caption_h + m.px(6.0) + i as f32 * row_h,
                rect.w - m.px(8.0),
                row_h,
            );
            let over = input.pointer.is_some_and(|p| r.contains_pt(p));
            if over {
                canvas.fill_rrect(r, m.px(7.0), pal.selection_fill);
            }
            let color = if over { pal.selection_text } else { pal.text };
            canvas.icon(
                icon,
                Rect::new(
                    r.x + m.px(10.0),
                    r.y + (r.h - m.px(15.0)) / 2.0,
                    m.px(15.0),
                    m.px(15.0),
                ),
                color,
                1.6,
            );
            canvas.text(
                cx.fonts,
                r.x + m.px(36.0),
                r.y + (r.h - cx.fonts.line_height(&style)) / 2.0,
                title,
                &TextStyle { color, ..style },
                None,
            );
            hits.push((r, action));
        }
    }

    fn options_caption(&self) -> String {
        match self.service.provider() {
            None => "Choose a provider to get started".to_owned(),
            Some(p) => format!(
                "Selected: {}{}",
                p.name,
                if p.model.is_empty() {
                    String::new()
                } else {
                    format!(" \u{b7} {}", p.model)
                }
            ),
        }
    }

    // ---- hit resolution -------------------------------------------------------------------

    fn resolve_hits(&mut self, input: &InputState, hits: &Hits, out: &mut Out) {
        let Some(pointer) = input.pointer else {
            if input.released {
                self.press = None;
            }
            return;
        };
        let top = hits.iter().rev().find(|(r, _)| r.contains_pt(pointer));
        if input.pressed {
            self.press = top.map(|(r, a)| (*r, a.clone()));
        }
        if input.released {
            if let (Some((rect, action)), Some((_, now))) = (self.press.take(), top) {
                if rect.contains_pt(pointer) && same_target(&action, now) {
                    self.perform(action, out);
                }
            }
        }
    }

    fn perform(&mut self, action: Action, out: &mut Out) {
        match action {
            Action::OpenUrl(url) => out.commands.push(Command::OpenUrl(url)),
            Action::CopyText(text) => out.commands.push(Command::CopyText(text)),
            Action::OpenPreview(shown) => self.lightbox = Some(shown),
            Action::CopyImage(shown) => out.requests.push(HostRequest::CopyImage(shown)),
            Action::SaveFile(shown) => out.requests.push(HostRequest::SaveAttachment(shown)),
            Action::RemoveDraft(id) => self.session.remove_attachment(id),
            Action::CloseLightbox => self.lightbox = None,
            Action::CloseMenus => self.menu = None,
            Action::TogglePreview => self.preview_open = !self.preview_open,
            Action::PickShare(id) => {
                self.menu = None;
                self.session.start_sharing(&id);
            }
            Action::OpenSetup(tab) => {
                self.menu = None;
                out.open_setup = Some(tab);
            }
            Action::OpenSettingsPage(page) => {
                self.menu = None;
                out.commands
                    .push(Command::OpenSettingsPage(page.to_owned()));
            }
            Action::Noop => {}
        }
    }
}

fn same_target(pressed: &Action, released: &Action) -> bool {
    std::mem::discriminant(pressed) == std::mem::discriminant(released)
}

#[derive(Default)]
struct Chips {
    height: f32,
    items: Vec<Chip>,
}

struct Chip {
    shown: Arc<ShownFile>,
    rect: Rect,
    lines: Vec<String>,
}

/// Draws `image` scaled to cover `rect` (center crop via clip).
fn draw_cover(canvas: &mut Canvas, image: &Image, rect: Rect, radius: f32) {
    let (iw, ih) = (image.width() as f32, image.height() as f32);
    let scale = (rect.w / iw).max(rect.h / ih);
    let (w, h) = (iw * scale, ih * scale);
    canvas.push_clip_round_rect(rect, radius);
    canvas.draw_image(
        image,
        Rect::new(
            rect.x + (rect.w - w) / 2.0,
            rect.y + (rect.h - h) / 2.0,
            w,
            h,
        ),
        1.0,
        0.0,
    );
    canvas.pop_clip();
}

/// The status text with the technical hints rewritten for the user (`Refresh`).
pub fn status_message(enabled: bool, status: &str) -> String {
    if !enabled {
        return "Enable your companion in Settings \u{2192} AI.".to_owned();
    }
    status
        .replace(
            "Configure a realtime endpoint and model in Providers.",
            "Set up voice in Settings \u{2192} AI.",
        )
        .replace("in provider settings", "in Settings \u{2192} AI")
}
