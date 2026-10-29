//! Reset and restore of preferences from the page (C# `SettingsReset.cs`). The confirmation is
//! the modal dialog; these run when it is accepted.

use super::SettingsView;
use naby_core::command::Command;
use naby_core::settings::reset::{self, ResetScope};
use serde_json::Value;

impl SettingsView {
    /// `FlushPreferenceChanges`: pending edits are applied before the stored values change.
    fn flush_preferences(&mut self) {
        self.mem.drop_focus();
        self.flush();
    }

    /// `ResetPreferences(category)` after the user confirmed.
    pub(crate) fn reset_preferences(&mut self, scope: ResetScope) {
        self.flush_preferences();
        let Some(store) = self.store.clone() else {
            self.set_feedback("Reset aborted: preferences cannot be stored here.", true);
            return;
        };
        let mut next = self.s.clone();
        match reset::reset_stored(&store, &mut next, scope) {
            Ok(()) => {
                self.adopt(next);
                let page = self.current.clone();
                self.navigate(&page);
                self.set_feedback(
                    "Preferences reset. The backup can be restored in Application.",
                    false,
                );
            }
            Err(e) => {
                self.log_error(&e);
                let message = format!("Reset aborted: {e}");
                self.set_feedback(&message, true);
            }
        }
    }

    /// `RestorePreferences()` after the user confirmed.
    pub(crate) fn restore_preferences(&mut self) {
        self.flush_preferences();
        let Some(store) = self.store.clone() else {
            self.set_feedback("Restore aborted: preferences cannot be stored here.", true);
            return;
        };
        let mut next = self.s.clone();
        match reset::restore_stored(&store, &mut next) {
            Ok(()) => {
                self.adopt(next);
                let page = self.current.clone();
                self.navigate(&page);
                self.set_feedback("Preference backup restored.", false);
            }
            Err(e) => {
                self.log_error(&e);
                let message = format!("Restore aborted: {e}");
                self.set_feedback(&message, true);
            }
        }
    }

    /// Makes `next` the working and saved preferences and tells the host.
    fn adopt(&mut self, mut next: naby_core::settings::Settings) {
        next.normalize();
        self.saved = serde_json::to_value(&next).unwrap_or(Value::Null);
        self.s = next.clone();
        self.lang_draft = None;
        self.pv.revision += 1;
        self.out.push(Command::ApplySettings(Box::new(next)));
    }
}
