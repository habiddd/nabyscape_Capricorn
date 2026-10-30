//! Hover timing policy (C# `OverlayWindow` hover / leave / detail timers in `CompactOverlay.cs`
//! and `HoverPagePolicy`). The four one-shot timers become deadlines the host polls.

use crate::settings::{policy, Settings};

/// Leave delay before an idle peek collapses (C# `leaveDelay`), seconds.
pub const LEAVE_DELAY: f64 = 0.300;

/// What the host should do when a timer fires.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoverAction {
    /// The pointer entered a tucked island: reveal it with a preview.
    RevealTucked,
    /// Show the activity peek (`PreviewActivity`).
    ShowPreview,
    /// Show the assistant peek (`PreviewAssistant`).
    ShowAssistantPreview,
    /// Collapse the peek / tucked reveal (`Collapse`).
    Collapse,
    /// Second hover stage: grow the strip to the detailed card (`PromotePeek`).
    PromoteDetail,
}

/// Live facts the decisions depend on.
#[derive(Debug, Clone, Copy, Default)]
pub struct HoverInputs {
    pub pointer_inside: bool,
    pub over_assistant_character: bool,
    pub expanded: bool,
    pub peeking: bool,
    pub tucked: bool,
    pub user_hidden: bool,
    pub tuck_requested: bool,
    /// Cover art press in progress.
    pub cover_pressed: bool,
    /// A peek drag / note edit / open menu (`PeekInteractionActive`).
    pub interaction_active: bool,
    pub file_drag_active: bool,
    pub assistant_reply_visible: bool,
}

/// Delays derived from settings (seconds).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HoverTimings {
    pub hover: f64,
    pub detail: f64,
    pub leave: f64,
}

impl HoverTimings {
    pub fn from_settings(s: &Settings) -> HoverTimings {
        HoverTimings {
            hover: f64::from(s.hover_delay_ms.max(0)) / 1000.0,
            detail: f64::from(s.hover_detail_delay_ms.max(0)) / 1000.0,
            leave: LEAVE_DELAY,
        }
    }
}

/// Deadline-based hover state machine.
#[derive(Debug, Clone, Default)]
pub struct HoverPolicy {
    hover: Option<f64>,
    leave: Option<f64>,
    detail: Option<f64>,
    assistant: Option<f64>,
}

impl HoverPolicy {
    pub fn new() -> Self {
        Self::default()
    }

    /// C# `HoverPagePolicy.Pages`: ids of the pages a hover peek can show.
    pub fn pages(s: &Settings) -> Vec<String> {
        policy::hover_pages(s)
    }

    /// C# `StopHoverTimers`.
    pub fn stop_all(&mut self) {
        *self = Self::default();
    }

    pub fn stop_leave(&mut self) {
        self.leave = None;
    }

    /// `root.MouseEnter`.
    pub fn pointer_enter(
        &mut self,
        s: &Settings,
        now: f64,
        i: &HoverInputs,
    ) -> Option<HoverAction> {
        if i.tucked {
            return Some(HoverAction::RevealTucked);
        }
        self.leave = None;
        if !i.cover_pressed
            && !i.expanded
            && !i.peeking
            && s.hover_peek
            && s.compact_layout
            && !i.user_hidden
            && !i.tuck_requested
        {
            self.hover = Some(now + HoverTimings::from_settings(s).hover);
        }
        None
    }

    /// `root.MouseLeave`.
    pub fn pointer_leave(&mut self, now: f64, i: &HoverInputs) {
        self.hover = None;
        if i.peeking || i.tuck_requested {
            self.leave = Some(now + LEAVE_DELAY);
        }
    }

    /// The pointer entered the assistant character button.
    pub fn assistant_enter(&mut self, s: &Settings, now: f64) {
        self.hover = None;
        self.assistant = Some(now + HoverTimings::from_settings(s).hover);
    }

    /// The pointer left (or pressed) the assistant character button.
    pub fn assistant_leave(&mut self) {
        self.assistant = None;
    }

    /// A peek just started: arm the second-stage timer unless it opened detailed.
    pub fn peek_started(&mut self, s: &Settings, now: f64, detailed: bool) {
        self.detail = None;
        if !detailed {
            self.detail = Some(now + HoverTimings::from_settings(s).detail);
        }
    }

    /// Start the leave timer (e.g. after a swipe when the pointer is already outside).
    pub fn start_leave(&mut self, now: f64) {
        self.leave = Some(now + LEAVE_DELAY);
    }

    /// The detail stage was reached another way (click, wheel, drag).
    pub fn cancel_detail(&mut self) {
        self.detail = None;
    }

    /// Fires at most one due timer; call again while it returns `Some` to drain simultaneous
    /// deadlines. The tick conditions are re-checked against `i` exactly like the C# handlers;
    /// a timer whose condition fails is consumed silently.
    pub fn poll(&mut self, now: f64, i: &HoverInputs) -> Option<HoverAction> {
        if due(&mut self.hover, now) {
            if i.over_assistant_character {
                return Some(HoverAction::ShowAssistantPreview);
            }
            if i.pointer_inside && !i.expanded && !i.user_hidden && !i.tuck_requested {
                return Some(HoverAction::ShowPreview);
            }
        }
        if due(&mut self.assistant, now)
            && i.over_assistant_character
            && !i.expanded
            && !i.user_hidden
        {
            return Some(HoverAction::ShowAssistantPreview);
        }
        if due(&mut self.leave, now)
            && (i.peeking || i.tuck_requested)
            && !i.assistant_reply_visible
            && !i.pointer_inside
            && !i.interaction_active
            && !i.file_drag_active
        {
            return Some(HoverAction::Collapse);
        }
        if due(&mut self.detail, now) {
            return Some(HoverAction::PromoteDetail);
        }
        None
    }

    /// Earliest pending deadline; `None` when no timer runs (the host can sleep).
    pub fn next_frame(&self, _now: f64) -> Option<f64> {
        [self.hover, self.leave, self.detail, self.assistant]
            .into_iter()
            .flatten()
            .reduce(f64::min)
    }

    pub fn pending(&self) -> bool {
        self.next_frame(0.0).is_some()
    }
}

fn due(timer: &mut Option<f64>, now: f64) -> bool {
    match *timer {
        Some(deadline) if now >= deadline => {
            *timer = None;
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> Settings {
        Settings {
            hover_peek: true,
            compact_layout: true,
            hover_delay_ms: 180,
            hover_detail_delay_ms: 550,
            ..Settings::default()
        }
    }

    fn inside() -> HoverInputs {
        HoverInputs {
            pointer_inside: true,
            ..HoverInputs::default()
        }
    }

    #[test]
    fn hover_delay_respected() {
        let s = settings();
        let mut h = HoverPolicy::new();
        assert_eq!(h.pointer_enter(&s, 10.0, &inside()), None);
        assert_eq!(h.next_frame(10.0), Some(10.18));
        assert_eq!(h.poll(10.179, &inside()), None);
        assert_eq!(h.poll(10.18, &inside()), Some(HoverAction::ShowPreview));
        assert_eq!(h.poll(10.5, &inside()), None, "one-shot");
        assert_eq!(h.next_frame(10.5), None);
        // The configured delay is used.
        let slow = Settings {
            hover_delay_ms: 600,
            ..s
        };
        let mut h = HoverPolicy::new();
        h.pointer_enter(&slow, 0.0, &inside());
        assert_eq!(h.poll(0.59, &inside()), None);
        assert_eq!(h.poll(0.6, &inside()), Some(HoverAction::ShowPreview));
    }

    #[test]
    fn leaving_before_the_delay_cancels_the_preview() {
        let s = settings();
        let mut h = HoverPolicy::new();
        h.pointer_enter(&s, 0.0, &inside());
        h.pointer_leave(0.1, &HoverInputs::default());
        assert_eq!(h.poll(1.0, &inside()), None);
        assert!(!h.pending());
    }

    #[test]
    fn hover_never_starts_when_disallowed() {
        let s = settings();
        for blocked in [
            HoverInputs {
                expanded: true,
                ..inside()
            },
            HoverInputs {
                peeking: true,
                ..inside()
            },
            HoverInputs {
                user_hidden: true,
                ..inside()
            },
            HoverInputs {
                tuck_requested: true,
                ..inside()
            },
            HoverInputs {
                cover_pressed: true,
                ..inside()
            },
        ] {
            let mut h = HoverPolicy::new();
            h.pointer_enter(&s, 0.0, &blocked);
            assert!(h.hover.is_none(), "{blocked:?}");
        }
        let off = Settings {
            hover_peek: false,
            ..s.clone()
        };
        let mut h = HoverPolicy::new();
        h.pointer_enter(&off, 0.0, &inside());
        assert!(!h.pending());
        let classic = Settings {
            compact_layout: false,
            ..s
        };
        h.pointer_enter(&classic, 0.0, &inside());
        assert!(!h.pending());
    }

    #[test]
    fn tick_rechecks_the_pointer() {
        let s = settings();
        let mut h = HoverPolicy::new();
        h.pointer_enter(&s, 0.0, &inside());
        // Pointer left without a MouseLeave being delivered: no preview.
        assert_eq!(h.poll(0.2, &HoverInputs::default()), None);
    }

    #[test]
    fn tucked_entry_reveals_immediately() {
        let s = settings();
        let mut h = HoverPolicy::new();
        let tucked = HoverInputs {
            tucked: true,
            tuck_requested: true,
            ..inside()
        };
        assert_eq!(
            h.pointer_enter(&s, 0.0, &tucked),
            Some(HoverAction::RevealTucked)
        );
        // Leaving a tuck reveal collapses after 300 ms unless something holds it open.
        h.pointer_leave(
            1.0,
            &HoverInputs {
                tuck_requested: true,
                ..HoverInputs::default()
            },
        );
        let leaving = HoverInputs {
            tuck_requested: true,
            ..HoverInputs::default()
        };
        assert_eq!(h.poll(1.299, &leaving), None);
        assert_eq!(h.poll(1.3, &leaving), Some(HoverAction::Collapse));
    }

    #[test]
    fn leave_delay_collapses_only_when_idle() {
        let mut h = HoverPolicy::new();
        let peeking = HoverInputs {
            peeking: true,
            ..HoverInputs::default()
        };
        for held in [
            HoverInputs {
                pointer_inside: true,
                ..peeking
            },
            HoverInputs {
                interaction_active: true,
                ..peeking
            },
            HoverInputs {
                file_drag_active: true,
                ..peeking
            },
            HoverInputs {
                assistant_reply_visible: true,
                ..peeking
            },
        ] {
            h.pointer_leave(0.0, &peeking);
            assert_eq!(h.poll(0.3, &held), None, "{held:?}");
        }
        h.pointer_leave(0.0, &peeking);
        assert_eq!(h.poll(0.3, &peeking), Some(HoverAction::Collapse));
        // Re-entering cancels a pending leave.
        let s = settings();
        h.pointer_leave(5.0, &peeking);
        h.pointer_enter(
            &s,
            5.1,
            &HoverInputs {
                peeking: true,
                ..inside()
            },
        );
        assert_eq!(h.poll(6.0, &peeking), None);
    }

    #[test]
    fn two_stage_detail_timer() {
        let s = settings();
        let mut h = HoverPolicy::new();
        h.peek_started(&s, 2.0, false);
        assert_eq!(h.next_frame(2.0), Some(2.55));
        assert_eq!(h.poll(2.54, &inside()), None);
        assert_eq!(h.poll(2.55, &inside()), Some(HoverAction::PromoteDetail));
        h.peek_started(&s, 3.0, true);
        assert!(!h.pending(), "a detailed start needs no second stage");
        h.peek_started(&s, 3.0, false);
        h.cancel_detail();
        assert!(!h.pending());
    }

    #[test]
    fn assistant_character_hover() {
        let s = settings();
        let mut h = HoverPolicy::new();
        h.pointer_enter(&s, 0.0, &inside());
        h.assistant_enter(&s, 0.05);
        assert!(
            h.hover.is_none(),
            "character hover replaces the activity hover"
        );
        let on_character = HoverInputs {
            over_assistant_character: true,
            ..inside()
        };
        assert_eq!(h.poll(0.2, &on_character), None);
        assert_eq!(
            h.poll(0.23, &on_character),
            Some(HoverAction::ShowAssistantPreview)
        );
        h.assistant_enter(&s, 1.0);
        h.assistant_leave();
        assert_eq!(h.poll(2.0, &on_character), None);
        // The activity timer defers to the character when the pointer rests on it.
        h.pointer_enter(&s, 3.0, &inside());
        assert_eq!(
            h.poll(3.2, &on_character),
            Some(HoverAction::ShowAssistantPreview)
        );
    }

    #[test]
    fn stop_all_clears_everything() {
        let s = settings();
        let mut h = HoverPolicy::new();
        h.pointer_enter(&s, 0.0, &inside());
        h.peek_started(&s, 0.0, false);
        h.start_leave(0.0);
        assert!(h.pending());
        h.stop_all();
        assert!(!h.pending());
        h.start_leave(0.0);
        h.stop_leave();
        assert!(!h.pending());
    }

    #[test]
    fn hover_pages_follow_settings() {
        let s = Settings {
            hover_follows_widgets: false,
            ..Settings::default()
        };
        let pages = HoverPolicy::pages(&s);
        assert_eq!(
            pages,
            crate::settings::policy::hover_pages(&s),
            "delegates to the settings policy"
        );
        assert!(pages
            .iter()
            .all(|p| s.hover_views.iter().any(|v| &v.id == p && v.enabled)));
    }

    #[test]
    fn timings_clamp_negative_delays() {
        let s = Settings {
            hover_delay_ms: -5,
            hover_detail_delay_ms: -1,
            ..Settings::default()
        };
        let t = HoverTimings::from_settings(&s);
        assert_eq!((t.hover, t.detail, t.leave), (0.0, 0.0, 0.3));
    }
}
