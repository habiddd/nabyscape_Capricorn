//! The assistant's presence on the dock (`AssistantIsland`, `AssistantLauncher`,
//! `AssistantReplyNotice`): the character in the closed dock, the peek card with greeting and
//! actions, and the reply notice that shows a fresh answer for a reading time.
//!
//! These are self-contained pieces the overlay calls: [`IslandAssistant`] owns the three orbs,
//! the greeting texts and the [`ReplyNotice`] timing, and paints the peek card into any
//! rectangle. Nothing here owns a timer; [`ReplyNotice::next_wake`] tells the host when to look
//! again.

use std::sync::Arc;

use naby_core::assistant::markdown::spoken;
use naby_core::assistant::{Options, State};
use naby_core::color::Rgba;
use naby_render::{Canvas, Fonts, Icon, InputState, Rect, TextStyle, Theme, Weight};

use super::orb::Orb;
use super::session::{ChatItem, ChatModel, ChatRole};
use super::style::{Metrics, Palette};
use crate::host::AssistantSummary;

/// Reading time per character, clamped (`assistantReplyReadingMs`).
const MILLIS_PER_CHARACTER: f64 = 55.0;
/// A reply waits this long for the dock to be free (`assistantReplyQueuedUntil`).
const QUEUE_SECONDS: f64 = 60.0;
const MAXIMUM_PLAIN: usize = 560;

/// The dock's closed label (`ClosedVoiceLabel`).
pub fn closed_label(state: State, sharing: bool) -> &'static str {
    match state {
        State::Listening => "Listening",
        State::Thinking => "Thinking\u{2026}",
        State::Speaking => "Speaking",
        State::Connecting => "Connecting\u{2026}",
        State::Error => "Voice needs attention",
        _ if sharing => "Screen sharing",
        _ => "Naby voice",
    }
}

/// What the dock needs to know about the assistant. `reply` is the plain text of a reply that
/// is waiting for, or showing in, the dock's notice (see [`ReplyNotice::shown_text`]); it is not
/// the transcript's latest message.
pub fn summary_of(
    model: &ChatModel,
    voice_connected: bool,
    reply: Option<String>,
) -> AssistantSummary {
    let sharing = model.sharing.is_some();
    let busy = model.busy || model.importing;
    let state = if busy && model.state == State::Idle {
        State::Thinking
    } else {
        model.state
    };
    AssistantSummary {
        voice_connected,
        busy,
        sharing,
        label: closed_label(state, sharing).to_owned(),
        reply,
    }
}

/// The newest finished answer worth showing in the dock (`LatestAssistantReply`).
pub fn latest_reply(model: &ChatModel) -> Option<Arc<ChatItem>> {
    model
        .items
        .iter()
        .rev()
        .find(|item| {
            item.role == ChatRole::Assistant
                && !item.text.trim().is_empty()
                && !item.text.starts_with("Reviewed voice action outcome")
                && !item.text.starts_with("The reply was interrupted")
        })
        .cloned()
}

/// The reply as one plain line for the notice (`SetAssistantReplyContent`).
pub fn reply_plain_text(text: &str) -> String {
    let flat = spoken(text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if flat.is_empty() {
        return "Your reply is ready. Open chat to read it.".to_owned();
    }
    if flat.chars().count() > MAXIMUM_PLAIN {
        let head: String = flat.chars().take(MAXIMUM_PLAIN).collect();
        format!("{}\u{2026}", head.trim_end())
    } else {
        flat
    }
}

/// How long a reply stays up, in seconds.
pub fn reading_seconds(plain: &str) -> f64 {
    (plain.chars().count() as f64 * MILLIS_PER_CHARACTER).clamp(7000.0, 18000.0) / 1000.0
}

/// The face's greeting for a state.
pub fn greeting(state: State) -> &'static str {
    match state {
        State::Listening => "I'm listening.",
        State::Thinking => "Let me think\u{2026}",
        State::Speaking => "Let's talk.",
        State::Approval => "Your call.",
        State::Connecting => "Connecting\u{2026}",
        State::Error => "Let's check that.",
        State::Idle => "Hey, I'm here.",
    }
}

/// The line under the greeting (`AssistantIslandChanged`).
pub fn caption(options: &Options, state: State, status: &str, wake: Option<&str>) -> String {
    if !options.enabled {
        "Set up your companion in Settings \u{2192} AI.".to_owned()
    } else if let Some(wake) = wake {
        wake.to_owned()
    } else if state == State::Idle {
        "Chat or talk, right from your island.".to_owned()
    } else {
        status.to_owned()
    }
}

/// Whether the character is shown in the closed dock (`AssistantCharacterAvailable`).
pub fn character_available(
    options: &Options,
    model: &ChatModel,
    voice_connected: bool,
    wake_listening: bool,
    reply_showing: bool,
) -> bool {
    options.island_character
        || voice_connected
        || wake_listening
        || model.sharing.is_some()
        || model.busy
        || reply_showing
}

// ---- the reply notice ---------------------------------------------------------------------

/// What the dock should do about the reply notice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplyEvent {
    None,
    /// Open the peek in reply mode with this plain text.
    Show(String),
    /// The shown reply changed (voice caption preview became the final reply).
    Update(String),
    /// Reading time is over: close the notice.
    Dismiss,
}

/// What blocks or holds a reply right now (all from the dock's state).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DockBusy {
    /// The dock is open, hidden, tucked, editing, showing feedback/notice, a drag is active or
    /// another peek is showing: a pending reply waits.
    pub blocked: bool,
    /// The pointer is over the notice, the assistant speaks or works: the notice stays.
    pub hold: bool,
}

#[derive(Debug, Default)]
pub struct ReplyNotice {
    last_seen: Option<u64>,
    pending: Option<String>,
    queued_until: f64,
    shown: Option<String>,
    until: f64,
    showing: bool,
}

impl ReplyNotice {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn showing(&self) -> bool {
        self.showing
    }

    pub fn shown_text(&self) -> Option<&str> {
        self.shown.as_deref()
    }

    /// A reply arrived or the assistant state changed.
    ///
    /// `eligible` is true while voice is connected or the island character is on (only then
    /// does a fresh reply pop up); `dock_open` means the panel is open, hidden or not visible.
    pub fn update(
        &mut self,
        now: f64,
        reply: Option<&ChatItem>,
        enabled: bool,
        eligible: bool,
        dock_open: bool,
        busy: DockBusy,
    ) -> ReplyEvent {
        let Some(reply) = reply.filter(|_| enabled) else {
            self.pending = None;
            return self.dismiss_if_showing();
        };
        let fresh = self.last_seen != Some(reply.id);
        self.last_seen = Some(reply.id);
        if dock_open {
            self.pending = None;
            return ReplyEvent::None;
        }
        if fresh && eligible {
            self.pending = Some(reply_plain_text(&reply.text));
            self.queued_until = now + QUEUE_SECONDS;
            return self.try_show(now, busy);
        }
        ReplyEvent::None
    }

    fn dismiss_if_showing(&mut self) -> ReplyEvent {
        if std::mem::take(&mut self.showing) {
            self.shown = None;
            ReplyEvent::Dismiss
        } else {
            ReplyEvent::None
        }
    }

    fn try_show(&mut self, now: f64, busy: DockBusy) -> ReplyEvent {
        if busy.blocked {
            return ReplyEvent::None;
        }
        let Some(plain) = self.pending.take() else {
            return ReplyEvent::None;
        };
        self.until = now + reading_seconds(&plain);
        self.shown = Some(plain.clone());
        let was_showing = std::mem::replace(&mut self.showing, true);
        if was_showing {
            ReplyEvent::Update(plain)
        } else {
            ReplyEvent::Show(plain)
        }
    }

    /// Call when the deadline from [`ReplyNotice::next_wake`] passed or the dock became free.
    pub fn tick(&mut self, now: f64, busy: DockBusy) -> ReplyEvent {
        if self.pending.is_some() && now >= self.queued_until {
            self.pending = None;
        }
        let shown = self.try_show(now, busy);
        if shown != ReplyEvent::None {
            return shown;
        }
        if self.showing {
            if busy.hold {
                if let Some(text) = &self.shown {
                    self.until = now + reading_seconds(text);
                }
            } else if now >= self.until {
                return self.dismiss_if_showing();
            }
        }
        ReplyEvent::None
    }

    /// The user closed the notice.
    pub fn dismissed(&mut self) {
        self.showing = false;
        self.shown = None;
        self.pending = None;
    }

    /// When to call [`ReplyNotice::tick`] again (`None`: nothing is waiting or showing).
    pub fn next_wake(&self) -> Option<f64> {
        let showing = self.showing.then_some(self.until);
        let pending = self.pending.is_some().then_some(self.queued_until);
        match (showing, pending) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }
}

// ---- the character and its peek card ------------------------------------------------------

/// What the peek card's buttons did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeekAction {
    OpenChat,
    Talk,
    EndVoice,
    Dismiss,
}

/// What the peek card shows.
#[derive(Debug, Clone)]
pub struct PeekContent {
    pub state: State,
    pub caption: String,
    /// Reply mode: the plain reply text under a small "Naby" label.
    pub reply: Option<String>,
    pub voice_connected: bool,
    pub busy: bool,
    pub speaking: bool,
}

/// Where and how the peek card is painted.
#[derive(Clone, Copy)]
pub struct PeekPaint<'a> {
    pub fonts: &'a Fonts,
    pub theme: &'a Theme,
    pub metrics: Metrics,
    pub rect: Rect,
    pub input: &'a InputState,
    pub now: f64,
}

pub struct IslandAssistant {
    pub closed: Orb,
    pub peek: Orb,
    pub launcher: Orb,
    pub notice: ReplyNotice,
}

impl Default for IslandAssistant {
    fn default() -> Self {
        Self::new()
    }
}

impl IslandAssistant {
    pub fn new() -> Self {
        Self {
            closed: Orb::new(),
            peek: Orb::new(),
            launcher: Orb::new(),
            notice: ReplyNotice::new(),
        }
    }

    /// Applies the assistant's visuals to every orb that is shown (`RefreshAssistantVisuals`).
    #[allow(clippy::too_many_arguments)]
    pub fn update_visuals(
        &mut self,
        now: f64,
        state: State,
        level: f32,
        live: bool,
        music: bool,
        reduce_motion: bool,
        visible: (bool, bool, bool),
    ) {
        for (orb, shown) in [
            (&mut self.closed, visible.0),
            (&mut self.peek, visible.1),
            (&mut self.launcher, visible.2),
        ] {
            orb.set_visible(shown);
            if !shown {
                continue;
            }
            orb.set_reduce_motion(reduce_motion);
            orb.set_state(state, now);
            orb.set_level(level);
            orb.set_live(live);
            orb.set_music(music);
            orb.tick(now);
        }
    }

    /// The pointer position relative to the shown orbs (`TrackAssistantPointerAt`).
    pub fn look_at_screen(&mut self, pointer: (f32, f32), closed: Rect, peek: Rect) {
        for (orb, rect) in [(&mut self.closed, closed), (&mut self.peek, peek)] {
            let (cx, cy) = rect.center();
            orb.look_at((pointer.0 - cx) / 360.0, (pointer.1 - cy) / 280.0);
        }
    }

    /// The earliest time an orb or the reply notice wants a frame.
    pub fn next_frame(&self, now: f64) -> Option<f64> {
        [
            self.closed.next_frame(now),
            self.peek.next_frame(now),
            self.launcher.next_frame(now),
            self.notice.next_wake(),
        ]
        .into_iter()
        .flatten()
        .reduce(f64::min)
    }

    /// Paints the peek card into `paint.rect` and returns the button the pointer released on.
    pub fn paint_peek(
        &self,
        canvas: &mut Canvas,
        paint: &PeekPaint,
        content: &PeekContent,
    ) -> Option<PeekAction> {
        let PeekPaint {
            fonts,
            theme,
            metrics,
            rect,
            input,
            now,
        } = *paint;
        let m = metrics;
        let pal = Palette::from_theme(theme);
        let reply = content.reply.is_some();
        let orb_size = m.px(if reply { 64.0 } else { 72.0 });
        let orb_rect = Rect::new(rect.x + m.px(12.0), rect.y + m.px(10.0), orb_size, orb_size);
        self.peek.paint(canvas, orb_rect, now);
        let tx = orb_rect.right() + m.px(12.0);
        let tw = (rect.right() - tx - m.px(12.0)).max(m.px(40.0));
        let (title, title_style) = if reply {
            (
                if content.speaking {
                    "Naby \u{b7} Speaking"
                } else {
                    "Naby \u{b7} Reply"
                },
                TextStyle::new(m.font(11.0), pal.dim),
            )
        } else {
            (
                greeting(content.state),
                TextStyle::new(m.font(17.0), pal.text).weight(Weight::Semibold),
            )
        };
        let mut y = rect.y + m.px(14.0);
        canvas.text(fonts, tx, y, title, &title_style, Some(tw));
        y += fonts.line_height(&title_style) + m.px(4.0);
        let body = content.reply.as_deref().unwrap_or(&content.caption);
        let body_style = if reply {
            TextStyle::new(m.font(13.0), pal.text)
        } else {
            TextStyle::new(m.font(11.0), pal.dim)
        };
        let line_h = fonts.line_height(&body_style) * if reply { 1.15 } else { 1.0 };
        let max_lines = if reply { 5 } else { 3 };
        let mut lines = fonts.wrap(body, &body_style, tw);
        if lines.len() > max_lines {
            lines.truncate(max_lines);
            if let Some(last) = lines.last_mut() {
                *last = fonts
                    .ellipsize(&format!("{last}\u{2026}"), &body_style, tw)
                    .into_owned();
            }
        }
        for line in &lines {
            canvas.text(fonts, tx, y, line, &body_style, None);
            y += line_h;
        }

        // Buttons.
        let button_h = m.px(28.0);
        let row_y = rect.bottom() - button_h - m.px(10.0);
        let mut action = None;
        let mut button =
            |canvas: &mut Canvas, r: Rect, label: &str, primary: bool, what: PeekAction| {
                let over = input.pointer.is_some_and(|p| r.contains_pt(p));
                let fill = if primary {
                    pal.accent
                } else {
                    pal.control_fill
                };
                canvas.fill_rrect(r, m.px(8.0), fill);
                if over {
                    canvas.fill_rrect(r, m.px(8.0), Rgba::WHITE.with_alpha(0x18));
                }
                let color = if primary { pal.accent_text } else { pal.text };
                let style = TextStyle::new(m.font(12.0), color);
                let w = fonts.measure_width(label, &style);
                canvas.text(
                    fonts,
                    r.x + (r.w - w) / 2.0,
                    r.y + (r.h - fonts.line_height(&style)) / 2.0,
                    label,
                    &style,
                    None,
                );
                if over && input.released {
                    action = Some(what);
                }
            };
        let style = TextStyle::new(m.font(12.0), pal.text);
        let pill = |label: &str| fonts.measure_width(label, &style) + m.px(24.0);
        if reply {
            let w = pill("Open chat");
            button(
                canvas,
                Rect::new(tx, row_y, w, button_h),
                "Open chat",
                false,
                PeekAction::OpenChat,
            );
            let close = Rect::new(
                rect.right() - m.px(12.0) - button_h,
                row_y,
                button_h,
                button_h,
            );
            let over = input.pointer.is_some_and(|p| close.contains_pt(p));
            if over {
                canvas.fill_rrect(close, m.px(8.0), pal.control_fill);
            }
            canvas.icon(Icon::X, close.inset(m.px(8.0)), pal.dim, 1.6);
            if over && input.released {
                action = Some(PeekAction::Dismiss);
            }
        } else {
            let chat = pill("Chat here");
            button(
                canvas,
                Rect::new(tx, row_y, chat, button_h),
                "Chat here",
                true,
                PeekAction::OpenChat,
            );
            let (label, what) = if content.voice_connected {
                ("End voice", PeekAction::EndVoice)
            } else {
                ("Talk", PeekAction::Talk)
            };
            let talk = Rect::new(tx + chat + m.px(6.0), row_y, pill(label), button_h);
            if content.busy && !content.voice_connected {
                canvas.with_opacity(0.4, |c| c.fill_rrect(talk, m.px(8.0), pal.control_fill));
                let dim = TextStyle::new(m.font(12.0), pal.dim);
                let w = fonts.measure_width(label, &dim);
                canvas.text(
                    fonts,
                    talk.x + (talk.w - w) / 2.0,
                    talk.y + (talk.h - fonts.line_height(&dim)) / 2.0,
                    label,
                    &dim,
                    None,
                );
            } else {
                button(canvas, talk, label, false, what);
            }
        }
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assistant::session::shown;
    use naby_core::assistant::Attachment;
    use naby_render::snapshot::{count_differing, save, test_fonts};

    fn item(id: u64, text: &str) -> ChatItem {
        ChatItem {
            id,
            role: ChatRole::Assistant,
            text: text.to_owned(),
            attachments: vec![shown(
                Attachment::from_text("a.txt", "x".into(), "").unwrap(),
            )],
        }
    }

    #[test]
    fn labels_follow_the_dock_rules() {
        assert_eq!(closed_label(State::Thinking, false), "Thinking\u{2026}");
        assert_eq!(closed_label(State::Error, false), "Voice needs attention");
        assert_eq!(closed_label(State::Idle, true), "Screen sharing");
        assert_eq!(closed_label(State::Idle, false), "Naby voice");
        assert_eq!(greeting(State::Idle), "Hey, I'm here.");
        assert_eq!(greeting(State::Approval), "Your call.");
    }

    #[test]
    fn summary_reports_busy_sharing_and_the_reply_in_the_notice() {
        let mut model = ChatModel::default();
        let idle = summary_of(&model, false, None);
        assert!(!idle.closed_activity() && idle.reply.is_none());
        model.busy = true;
        model.state = State::Idle;
        model.items.push(Arc::new(item(1, "Hello there")));
        let busy = summary_of(&model, false, Some("Hello there".into()));
        assert!(busy.closed_activity());
        assert_eq!(busy.label, "Thinking\u{2026}");
        assert_eq!(busy.reply.as_deref(), Some("Hello there"));
        model.busy = false;
        model.sharing = Some("Main display".into());
        let sharing = summary_of(&model, false, None);
        assert!(sharing.closed_activity());
        assert_eq!(sharing.label, "Screen sharing");
        // Internal outcome notes are never a reply.
        model.items.push(Arc::new(item(
            2,
            "The reply was interrupted before completion.",
        )));
        assert_eq!(latest_reply(&model).unwrap().id, 1);
    }

    #[test]
    fn reply_text_is_flattened_bounded_and_timed() {
        assert_eq!(
            reply_plain_text("# Title\n\n**bold** `code`\n```rust\nfn x() {}\n```\nend"),
            "Title bold code end"
        );
        assert_eq!(
            reply_plain_text("```\nonly code\n```"),
            "Your reply is ready. Open chat to read it."
        );
        let long = "word ".repeat(300);
        let plain = reply_plain_text(&long);
        assert!(plain.chars().count() <= 561 && plain.ends_with('\u{2026}'));
        assert_eq!(reading_seconds("short"), 7.0);
        assert_eq!(reading_seconds(&"x".repeat(1000)), 18.0);
        assert!((reading_seconds(&"x".repeat(200)) - 11.0).abs() < 1e-9);
    }

    #[test]
    fn a_fresh_reply_pops_up_waits_when_blocked_and_closes_after_reading() {
        let mut notice = ReplyNotice::new();
        let reply = item(7, "A short answer.");
        let free = DockBusy::default();
        // Only eligible replies pop up; the same reply is not fresh twice.
        assert_eq!(
            notice.update(0.0, Some(&reply), true, false, false, free),
            ReplyEvent::None
        );
        let reply = item(8, "A short answer.");
        assert_eq!(
            notice.update(1.0, Some(&reply), true, true, false, free),
            ReplyEvent::Show("A short answer.".into())
        );
        assert!(notice.showing());
        assert_eq!(notice.next_wake(), Some(8.0));
        // The pointer on the notice extends its stay; leaving lets it end.
        assert_eq!(
            notice.tick(
                8.0,
                DockBusy {
                    blocked: false,
                    hold: true
                }
            ),
            ReplyEvent::None
        );
        assert_eq!(notice.next_wake(), Some(15.0));
        assert_eq!(notice.tick(14.9, free), ReplyEvent::None);
        assert_eq!(notice.tick(15.0, free), ReplyEvent::Dismiss);
        assert!(!notice.showing() && notice.next_wake().is_none());

        // A blocked dock queues the reply for up to a minute.
        let next = item(9, "Another one.");
        let blocked = DockBusy {
            blocked: true,
            hold: false,
        };
        assert_eq!(
            notice.update(20.0, Some(&next), true, true, false, blocked),
            ReplyEvent::None
        );
        assert_eq!(notice.next_wake(), Some(80.0));
        assert_eq!(notice.tick(30.0, blocked), ReplyEvent::None);
        assert_eq!(
            notice.tick(31.0, free),
            ReplyEvent::Show("Another one.".into())
        );
        // Too late: the queue expires.
        let late = item(10, "Missed.");
        notice.update(100.0, Some(&late), true, true, false, blocked);
        // The earlier notice ended meanwhile and the expired queue entry is dropped.
        assert_eq!(notice.tick(161.0, free), ReplyEvent::Dismiss);
        assert!(notice.next_wake().is_none());
        // The open panel drops pending replies; disabling closes the notice.
        let again = item(11, "Open panel.");
        assert_eq!(
            notice.update(200.0, Some(&again), true, true, true, free),
            ReplyEvent::None
        );
        assert!(notice.next_wake().is_none());
        let shown_one = item(12, "Shown.");
        notice.update(300.0, Some(&shown_one), true, true, false, free);
        assert!(notice.showing());
        assert_eq!(
            notice.update(301.0, Some(&shown_one), false, true, false, free),
            ReplyEvent::Dismiss
        );
    }

    #[test]
    fn the_peek_card_paints_both_modes_and_reports_button_releases() {
        let Some(fonts) = test_fonts() else { return };
        let theme = Theme::default();
        let mut island = IslandAssistant::new();
        island.update_visuals(
            1.0,
            State::Idle,
            0.0,
            false,
            false,
            true,
            (false, true, false),
        );
        let mut canvas = Canvas::new(380, 330);
        canvas.clear(Rgba::rgb(0x10, 0x11, 0x13));
        let normal = PeekContent {
            state: State::Idle,
            caption: "Chat or talk, right from your island.".into(),
            reply: None,
            voice_connected: false,
            busy: false,
            speaking: false,
        };
        let idle_input = InputState::default();
        let card = Rect::new(10.0, 10.0, 360.0, 130.0);
        fn paint<'a>(
            fonts: &'a Fonts,
            theme: &'a Theme,
            rect: Rect,
            input: &'a InputState,
        ) -> PeekPaint<'a> {
            PeekPaint {
                fonts,
                theme,
                metrics: Metrics::new(1.0, 1.0),
                rect,
                input,
                now: 1.0,
            }
        }
        assert!(island
            .paint_peek(
                &mut canvas,
                &paint(&fonts, &theme, card, &idle_input),
                &normal
            )
            .is_none());
        let reply = PeekContent {
            reply: Some("The weather looks clear this afternoon, so a walk would be nice. Take a jacket for the evening.".into()),
            ..normal.clone()
        };
        let reply_card = Rect::new(10.0, 160.0, 360.0, 150.0);
        assert!(island
            .paint_peek(
                &mut canvas,
                &paint(&fonts, &theme, reply_card, &idle_input),
                &reply
            )
            .is_none());
        save("assistant-peek", &canvas);
        assert!(
            count_differing(
                &canvas,
                Rect::new(0.0, 0.0, 380.0, 330.0),
                Rgba::rgb(0x10, 0x11, 0x13)
            ) > 3000
        );

        // "Chat here" sits at the left of the button row; releasing on it reports the action.
        let click = InputState::release(130.0, card.bottom() - 24.0);
        assert_eq!(
            island.paint_peek(&mut canvas, &paint(&fonts, &theme, card, &click), &normal),
            Some(PeekAction::OpenChat)
        );
        // A reply's dismiss button is at the right.
        let close = InputState::release(reply_card.right() - 26.0, reply_card.bottom() - 24.0);
        assert_eq!(
            island.paint_peek(
                &mut canvas,
                &paint(&fonts, &theme, reply_card, &close),
                &reply
            ),
            Some(PeekAction::Dismiss)
        );
    }

    #[test]
    fn orbs_only_run_while_shown() {
        let mut island = IslandAssistant::new();
        island.update_visuals(
            1.0,
            State::Idle,
            0.0,
            false,
            false,
            false,
            (false, false, false),
        );
        assert_eq!(island.next_frame(1.0), None);
        island.update_visuals(
            1.1,
            State::Idle,
            0.0,
            false,
            false,
            false,
            (true, false, false),
        );
        assert!(island.next_frame(1.1).is_some());
        island.update_visuals(
            1.2,
            State::Idle,
            0.0,
            false,
            false,
            true,
            (true, true, true),
        );
        assert_eq!(
            island.next_frame(1.2),
            None,
            "reduced motion stops every orb"
        );
    }
}
