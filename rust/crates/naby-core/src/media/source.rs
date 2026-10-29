//! Media source choice (`MediaSourceSelection`): automatic, none, or a fixed app.

pub const NONE_ID: &str = "@none";
pub const AUTO_ID: &str = "@auto";

/// Selection is local to the running instance. `None` never issues a transport command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaSourceSelection {
    id: String,
}

impl Default for MediaSourceSelection {
    fn default() -> Self {
        MediaSourceSelection {
            id: AUTO_ID.to_owned(),
        }
    }
}

impl MediaSourceSelection {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn is_none(&self) -> bool {
        self.id == NONE_ID
    }

    /// `None` selects "no source".
    pub fn select(&mut self, id: Option<&str>) {
        self.id = id.unwrap_or(NONE_ID).to_owned();
    }

    /// Picks the session to show. A fixed choice that is gone falls back to automatic.
    pub fn resolve<S: AsRef<str>>(
        &mut self,
        sessions: &[S],
        current: Option<&str>,
        previous: Option<&str>,
    ) -> Option<String> {
        if self.is_none() {
            return None;
        }
        let has = |id: &str| sessions.iter().any(|s| s.as_ref() == id);
        if self.id != AUTO_ID {
            if has(&self.id) {
                return Some(self.id.clone());
            }
            self.id = AUTO_ID.to_owned();
        }
        if let Some(p) = previous.filter(|p| has(p)) {
            return Some(p.to_owned());
        }
        if let Some(c) = current.filter(|c| has(c)) {
            return Some(c.to_owned());
        }
        sessions.first().map(|s| s.as_ref().to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn none_survives_enumeration_until_reselected() {
        let mut sel = MediaSourceSelection::new();
        let sessions = ["Spotify.exe", "chrome.exe"];
        assert_eq!(sel.id(), AUTO_ID);
        assert_eq!(
            sel.resolve(&sessions, Some("Spotify.exe"), None).as_deref(),
            Some("Spotify.exe")
        );
        sel.select(None);
        assert!(sel.is_none());
        assert_eq!(
            sel.resolve(&sessions, Some("chrome.exe"), Some("Spotify.exe")),
            None
        );
        assert_eq!(
            sel.resolve(&["chrome.exe", "new-app.exe"], Some("new-app.exe"), None),
            None
        );
        assert_eq!(sel.resolve::<&str>(&[], None, None), None);
        sel.select(Some("chrome.exe"));
        assert_eq!(
            sel.resolve(&sessions, Some("Spotify.exe"), None).as_deref(),
            Some("chrome.exe")
        );
        sel.select(Some(AUTO_ID));
        assert_eq!(
            sel.resolve(&sessions, Some("Spotify.exe"), None).as_deref(),
            Some("Spotify.exe")
        );
        assert_eq!(
            sel.resolve(&sessions, Some("chrome.exe"), None).as_deref(),
            Some("chrome.exe")
        );
        sel.select(Some("gone.exe"));
        assert_eq!(
            sel.resolve(&sessions, Some("chrome.exe"), None).as_deref(),
            Some("chrome.exe")
        );
        assert_eq!(sel.id(), AUTO_ID);
    }

    #[test]
    fn previous_wins_over_current_then_first() {
        let mut sel = MediaSourceSelection::new();
        let s = ["a", "b", "c"];
        assert_eq!(sel.resolve(&s, Some("b"), Some("c")).as_deref(), Some("c"));
        assert_eq!(
            sel.resolve(&s, Some("zz"), Some("yy")).as_deref(),
            Some("a")
        );
    }
}
