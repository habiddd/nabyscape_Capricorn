//! When the wake word listens (`AssistantWakeRuntime`), as a pure state machine.
//!
//! Hands-free semantics from the README: the wake microphone runs only while wake listening is
//! enabled (AI on and wake word on) and no voice conversation is active. **End conversation**
//! resumes wake listening after a short cooldown; an explicit **Stop** pauses it until the user
//! resumes. A failure also pauses it (no retry loop that keeps opening the microphone).

use std::time::Duration;

/// Delay before wake listening re-arms after End conversation, so the old voice device closes.
pub const RESUME_COOLDOWN: Duration = Duration::from_millis(1200);

pub const OFF_TEXT: &str = "Wake word is off. Microphone stays off until you start voice.";

/// What the host should do about the wake microphone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeAction {
    None,
    /// Open the wake microphone (`now` has passed the cooldown).
    Arm,
    /// Close the wake microphone.
    Disarm,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WakeLogic {
    /// AI on and the wake word saved as enabled.
    allowed: bool,
    /// The user wants wake listening (not paused by Stop or a failure).
    wanted: bool,
    failed: bool,
    listening: bool,
    voice_active: bool,
    busy: bool,
    not_before: Option<f64>,
    phrase: String,
    status: String,
}

impl Default for WakeLogic {
    fn default() -> Self {
        Self {
            allowed: false,
            wanted: false,
            failed: false,
            listening: false,
            voice_active: false,
            busy: false,
            not_before: None,
            phrase: "Hey Naby".into(),
            status: OFF_TEXT.into(),
        }
    }
}

impl WakeLogic {
    pub fn status(&self) -> &str {
        &self.status
    }

    pub fn listening(&self) -> bool {
        self.listening
    }

    /// Armed means wanted and not failed (the toggle reads "Pause").
    pub fn armed(&self) -> bool {
        self.wanted && !self.failed
    }

    pub fn allowed(&self) -> bool {
        self.allowed
    }

    pub fn paused(&self) -> bool {
        self.allowed && !self.armed()
    }

    pub fn phrase(&self) -> &str {
        &self.phrase
    }

    /// Settings were saved (or loaded at startup). Re-arms when enabled.
    pub fn configure(&mut self, ai_on: bool, wake_on: bool, phrase: &str) -> WakeAction {
        self.phrase = phrase.to_owned();
        let was = self.listening;
        self.allowed = ai_on && wake_on;
        self.failed = false;
        self.wanted = self.allowed;
        self.listening = false;
        self.not_before = None;
        self.status = if self.allowed {
            "Preparing wake listening\u{2026}".into()
        } else {
            OFF_TEXT.into()
        };
        if was {
            WakeAction::Disarm
        } else {
            WakeAction::None
        }
    }

    /// The host opened the wake microphone.
    pub fn armed_ok(&mut self) {
        self.listening = true;
        self.status = format!(
            "Wake microphone on \u{b7} say \u{201c}{}\u{201d}. Detection stays on this PC.",
            self.phrase
        );
    }

    /// Arming or listening failed: pause until the user resumes.
    pub fn failed(&mut self, message: &str) -> WakeAction {
        let was = self.listening;
        self.failed = true;
        self.listening = false;
        self.status = message.to_owned();
        if was {
            WakeAction::Disarm
        } else {
            WakeAction::None
        }
    }

    /// The phrase was heard. The wake microphone closes before voice starts.
    pub fn detected(&mut self) -> WakeAction {
        self.listening = false;
        self.status = "Wake phrase heard \u{b7} starting voice\u{2026}".into();
        WakeAction::Disarm
    }

    /// A voice conversation started: the wake microphone must be off.
    pub fn voice_started(&mut self) -> WakeAction {
        self.voice_active = true;
        let was = self.listening;
        self.listening = false;
        if self.wanted {
            self.status = "Wake word will resume after the voice conversation.".into();
        }
        if was {
            WakeAction::Disarm
        } else {
            WakeAction::None
        }
    }

    /// Voice did not start after a wake phrase.
    pub fn voice_failed_to_start(&mut self) {
        self.voice_active = false;
        self.wanted = false;
        self.failed = true;
        self.status =
            "Voice did not start. Check Voice settings, then resume wake listening.".into();
    }

    /// End conversation: re-arm after the cooldown when wake listening is enabled.
    pub fn voice_ended(&mut self, now: f64) {
        self.voice_active = false;
        if self.allowed {
            self.wanted = true;
            self.failed = false;
            self.not_before = Some(now + RESUME_COOLDOWN.as_secs_f64());
            self.status = "Conversation ended \u{b7} wake word will resume.".into();
        }
    }

    /// Explicit Stop: pause wake listening (and everything else).
    pub fn explicit_stop(&mut self) -> WakeAction {
        self.voice_active = false;
        let was = self.listening;
        self.wanted = false;
        self.listening = false;
        self.not_before = None;
        if self.allowed {
            self.status = "Wake word paused \u{b7} wake microphone off.".into();
        }
        if was {
            WakeAction::Disarm
        } else {
            WakeAction::None
        }
    }

    /// The Pause / Resume toggle. `on` true resumes.
    pub fn set_listening(&mut self, on: bool) -> WakeAction {
        if !on {
            return self.explicit_stop();
        }
        if !self.allowed {
            self.status = "Enable wake word in Voice settings first.".into();
            return WakeAction::None;
        }
        self.wanted = true;
        self.failed = false;
        self.not_before = None;
        self.status = if self.voice_active {
            "Wake word will resume after the voice conversation.".into()
        } else {
            "Preparing wake listening\u{2026}".into()
        };
        WakeAction::None
    }

    /// Chat or setup work that must not overlap the wake microphone started or ended.
    pub fn set_busy(&mut self, busy: bool) {
        self.busy = busy;
    }

    /// Whether the host should open the wake microphone now.
    pub fn poll(&self, now: f64) -> WakeAction {
        if self.allowed
            && self.wanted
            && !self.failed
            && !self.listening
            && !self.voice_active
            && !self.busy
            && self.not_before.is_none_or(|t| now >= t)
        {
            WakeAction::Arm
        } else {
            WakeAction::None
        }
    }

    /// Seconds from `now` until `poll` can arm (for scheduling one wake-up, never polling).
    pub fn arm_delay(&self, now: f64) -> Option<f64> {
        if self.allowed && self.wanted && !self.failed && !self.listening && !self.voice_active {
            self.not_before.map(|t| (t - now).max(0.0))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enabled() -> WakeLogic {
        let mut w = WakeLogic::default();
        w.configure(true, true, "Hey Naby");
        w
    }

    #[test]
    fn nothing_listens_until_enabled() {
        let w = WakeLogic::default();
        assert_eq!(w.poll(0.0), WakeAction::None);
        assert_eq!(w.status(), OFF_TEXT);
        let mut off = WakeLogic::default();
        off.configure(true, false, "Hey Naby");
        assert_eq!(off.poll(0.0), WakeAction::None);
        off.configure(false, true, "Hey Naby");
        assert_eq!(off.poll(0.0), WakeAction::None);
    }

    #[test]
    fn enabled_wake_arms_and_reports_the_phrase() {
        let mut w = enabled();
        assert_eq!(w.poll(0.0), WakeAction::Arm);
        w.armed_ok();
        assert!(w.listening() && w.armed());
        assert!(w.status().contains("Hey Naby") && w.status().contains("stays on this PC"));
        assert_eq!(w.poll(1.0), WakeAction::None);
    }

    #[test]
    fn a_detection_closes_the_wake_microphone_before_voice_starts() {
        let mut w = enabled();
        w.armed_ok();
        assert_eq!(w.detected(), WakeAction::Disarm);
        assert_eq!(w.voice_started(), WakeAction::None);
        assert_eq!(
            w.poll(5.0),
            WakeAction::None,
            "no wake microphone during a conversation"
        );
    }

    #[test]
    fn end_conversation_resumes_wake_after_the_cooldown() {
        let mut w = enabled();
        w.armed_ok();
        w.voice_started();
        w.voice_ended(10.0);
        assert!(w.status().contains("will resume"));
        assert_eq!(w.poll(10.5), WakeAction::None);
        assert!(w.arm_delay(10.5).unwrap() > 0.6);
        assert_eq!(w.poll(11.3), WakeAction::Arm);
    }

    #[test]
    fn end_conversation_resumes_even_after_a_stop_paused_it_before() {
        let mut w = enabled();
        w.explicit_stop();
        assert_eq!(w.poll(1.0), WakeAction::None);
        w.voice_started();
        w.voice_ended(2.0);
        assert_eq!(w.poll(4.0), WakeAction::Arm);
    }

    #[test]
    fn explicit_stop_pauses_until_resumed() {
        let mut w = enabled();
        w.armed_ok();
        assert_eq!(w.explicit_stop(), WakeAction::Disarm);
        assert!(w.paused() && !w.listening());
        assert_eq!(w.status(), "Wake word paused \u{b7} wake microphone off.");
        assert_eq!(w.poll(100.0), WakeAction::None);
        w.set_listening(true);
        assert_eq!(w.poll(100.0), WakeAction::Arm);
        assert!(!w.paused());
    }

    #[test]
    fn a_failure_pauses_without_retrying() {
        let mut w = enabled();
        assert_eq!(
            w.failed("No Windows speech recognizer is installed."),
            WakeAction::None
        );
        assert_eq!(w.poll(1.0), WakeAction::None);
        assert!(w.status().contains("recognizer"));
        w.set_listening(true);
        assert_eq!(w.poll(1.0), WakeAction::Arm);
    }

    #[test]
    fn resuming_while_disabled_explains_where_to_enable_it() {
        let mut w = WakeLogic::default();
        w.set_listening(true);
        assert!(w.status().contains("Voice settings"));
        assert_eq!(w.poll(0.0), WakeAction::None);
    }

    #[test]
    fn saving_settings_rearms_or_turns_wake_off() {
        let mut w = enabled();
        w.armed_ok();
        assert_eq!(w.configure(true, false, "Hey Naby"), WakeAction::Disarm);
        assert_eq!(w.status(), OFF_TEXT);
        assert_eq!(w.configure(true, true, "Hello Naby"), WakeAction::None);
        assert_eq!(w.poll(0.0), WakeAction::Arm);
        assert_eq!(w.phrase(), "Hello Naby");
    }

    #[test]
    fn voice_that_fails_to_start_pauses_wake() {
        let mut w = enabled();
        w.armed_ok();
        w.detected();
        w.voice_failed_to_start();
        assert_eq!(w.poll(9.0), WakeAction::None);
        assert!(w.status().contains("Voice did not start"));
    }

    #[test]
    fn busy_work_blocks_arming() {
        let mut w = enabled();
        w.set_busy(true);
        assert_eq!(w.poll(0.0), WakeAction::None);
        w.set_busy(false);
        assert_eq!(w.poll(0.0), WakeAction::Arm);
    }
}
