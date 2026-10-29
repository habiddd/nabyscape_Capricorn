//! Provider save rules, credential target names and the unsaved-draft guard
//! (`AssistantProviderPersistence`, `AssistantKeys`, `AssistantProviderDraftGuard`,
//! `AssistantProviderImageCheck`, `AssistantProviderSettings.ReadFields`).
//!
//! API keys are never stored here: the host implements [`Keys`] over Windows Credential Manager
//! using [`credential_target`] as the target name.

use std::collections::HashMap;
use std::sync::Mutex;

use url::Url;

use super::models::{Options, Protocol, Provider};
use super::providers::validate_endpoint;
use super::voice::validate_realtime_url;
use super::{is_id, len16, new_id, AssistantError, Result};

/// Documents the assistant owns in the store. Wiring happens in the host.
pub const DOCUMENT_NAME: &str = super::models::DOCUMENT_NAME;

/// Largest stored API key, in UTF-8 bytes.
pub const MAXIMUM_STORED_KEY_BYTES: usize = 2560;

/// Windows Credential Manager target for a provider's key
/// (`AssistantKeys.Target`): `NabyScape/Assistant/<32-hex id>`.
pub fn credential_target(id: &str) -> Result<String> {
    if !is_id(id) {
        return Err(AssistantError::invalid("Invalid provider identity."));
    }
    Ok(format!("NabyScape/Assistant/{id}"))
}

/// Trim and validate a key before it is stored (`AssistantKeys.Write`).
pub fn prepare_key_for_storage(value: &str) -> Result<String> {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed.len() > MAXIMUM_STORED_KEY_BYTES
        || value.contains(['\r', '\n', '\0'])
    {
        return Err(AssistantError::invalid("Enter a valid API key."));
    }
    Ok(trimmed.to_string())
}

/// Secret storage owned by the host (`IAssistantKeys`).
pub trait Keys {
    fn read(&self, id: &str) -> Result<Option<String>>;
    fn exists(&self, id: &str) -> Result<bool>;
    fn write(&self, id: &str, value: &str) -> Result<()>;
    fn remove(&self, id: &str) -> Result<()>;
}

/// In-memory keys for tests and for hosts without a credential store.
#[derive(Default)]
pub struct MemoryKeys {
    values: Mutex<HashMap<String, String>>,
}

impl MemoryKeys {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Keys for MemoryKeys {
    fn read(&self, id: &str) -> Result<Option<String>> {
        Ok(self.values.lock().expect("keys lock").get(id).cloned())
    }
    fn exists(&self, id: &str) -> Result<bool> {
        Ok(self.values.lock().expect("keys lock").contains_key(id))
    }
    fn write(&self, id: &str, value: &str) -> Result<()> {
        self.values
            .lock()
            .expect("keys lock")
            .insert(id.into(), value.into());
        Ok(())
    }
    fn remove(&self, id: &str) -> Result<()> {
        self.values.lock().expect("keys lock").remove(id);
        Ok(())
    }
}

/// `scheme://host[:port]` of a URL, empty when it is not absolute (`GetLeftPart(Authority)`).
fn origin(value: &str) -> String {
    match Url::parse(value) {
        Ok(url) if url.has_host() => format!(
            "{}://{}{}",
            url.scheme(),
            url.host_str().unwrap_or(""),
            url.port().map(|p| format!(":{p}")).unwrap_or_default()
        ),
        _ => String::new(),
    }
}

/// Whether saving `candidate` over `original` must move to a new credential identity.
fn rotates(original: &Provider, saved: &Provider, replacement_key: &str) -> bool {
    !replacement_key.is_empty()
        || origin(&original.base_url) != origin(&saved.base_url)
        || origin(&original.realtime_url) != origin(&saved.realtime_url)
        || (saved.is_hermes()
            && original.base_url.trim_end_matches('/') != saved.base_url.trim_end_matches('/'))
}

/// Save a provider edit. Changed credentials are staged under a fresh identity, so a failed
/// metadata write can never pair a new server's credential with a previously saved server.
/// Returns the options to adopt; `current` is never modified.
pub fn save_provider(
    current: &Options,
    original_id: &str,
    candidate: &Provider,
    replacement_key: &str,
    keys: &dyn Keys,
    persist: &mut dyn FnMut(&Options) -> Result<()>,
) -> Result<Options> {
    let original = current
        .providers
        .iter()
        .find(|p| p.id == original_id)
        .ok_or_else(|| AssistantError::invalid("Choose a saved provider to edit."))?;
    let mut next = current.clone();
    let mut saved = candidate.clone();
    saved.hermes_session_id.clear();
    let rotate = rotates(original, &saved, replacement_key);
    saved.id = if rotate {
        new_id()
    } else {
        original.id.clone()
    };
    if let Some(slot) = next.providers.iter_mut().find(|p| p.id == original_id) {
        *slot = saved.clone();
    }
    next.selected_provider = saved.id.clone();
    if next.selected_voice_provider == original_id {
        next.selected_voice_provider = saved.id.clone();
    }
    let staged = (|| {
        if !replacement_key.is_empty() {
            keys.write(&saved.id, replacement_key)?;
        }
        persist(&next)
    })();
    if let Err(error) = staged {
        if rotate {
            // Unreferenced staged credential; never used by a profile.
            let _ = keys.remove(&saved.id);
        }
        return Err(error);
    }
    if rotate {
        // The old credential is unreferenced now and cannot be used by this profile.
        let _ = keys.remove(original_id);
    }
    Ok(next)
}

/// Which key a draft connection check may use (`DraftKey`): a freshly typed key, or the saved
/// key when the destination (and, for Hermes, the exact path) did not change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeySource {
    Entered,
    Saved(String),
    None,
}

pub fn draft_key_source(
    entered: &str,
    saved: Option<&Provider>,
    candidate: &Provider,
) -> KeySource {
    if !entered.is_empty() {
        return KeySource::Entered;
    }
    let Some(saved) = saved else {
        return KeySource::None;
    };
    let (Ok(old), Ok(next)) = (Url::parse(&saved.base_url), Url::parse(&candidate.base_url)) else {
        return KeySource::None;
    };
    if origin(old.as_str()) != origin(next.as_str()) {
        return KeySource::None;
    }
    if candidate.is_hermes()
        && old.path().trim_end_matches('/') != next.path().trim_end_matches('/')
    {
        return KeySource::None;
    }
    KeySource::Saved(saved.id.clone())
}

/// The fields a provider editor can change.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DraftIdentity {
    pub name: String,
    pub base_url: String,
    pub model: String,
    pub protocol: String,
    pub vision: bool,
    pub image_output: bool,
    pub realtime_url: String,
    pub realtime_model: String,
    pub realtime_protocol: String,
    pub voice: String,
}

impl DraftIdentity {
    pub fn of(provider: &Provider) -> Self {
        Self {
            name: provider.name.clone(),
            base_url: provider.base_url.clone(),
            model: provider.model.clone(),
            protocol: provider.protocol.clone(),
            vision: provider.vision,
            image_output: provider.image_output,
            realtime_url: provider.realtime_url.clone(),
            realtime_model: provider.realtime_model.clone(),
            realtime_protocol: provider.realtime_protocol.clone(),
            voice: provider.voice.clone(),
        }
    }

    fn trimmed(&self) -> Self {
        Self {
            name: self.name.trim().into(),
            base_url: self.base_url.trim().into(),
            model: self.model.trim().into(),
            protocol: self.protocol.clone(),
            vision: self.vision,
            image_output: self.image_output,
            realtime_url: self.realtime_url.trim().into(),
            realtime_model: self.realtime_model.trim().into(),
            realtime_protocol: self.realtime_protocol.clone(),
            voice: self.voice.trim().into(),
        }
    }
}

/// Whether the editor holds unsaved connection changes (`HasProviderDraft`).
pub fn has_provider_draft(
    saved: &Provider,
    draft: &DraftIdentity,
    key_entered: bool,
    loading: bool,
) -> bool {
    !loading && (key_entered || draft.trimmed() != DraftIdentity::of(saved))
}

/// The user's answer to "Keep your connection changes?".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DraftChoice {
    Save,
    Discard,
    KeepEditing,
}

/// What a guarded action (switching provider, adding or removing one) should do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DraftAction {
    /// No draft, or it was discarded: carry on.
    Proceed,
    /// Save the draft first; carry on only if the save succeeds.
    SaveThenProceed,
    /// Stay in the editor.
    Stay,
}

/// The unsaved-draft guard (`ConfirmProviderChanges`): a pending draft blocks a switch until the
/// user saves or discards it.
pub fn guard_switch(has_draft: bool, choice: Option<DraftChoice>) -> DraftAction {
    if !has_draft {
        return DraftAction::Proceed;
    }
    match choice {
        Some(DraftChoice::Save) => DraftAction::SaveThenProceed,
        Some(DraftChoice::Discard) => DraftAction::Proceed,
        Some(DraftChoice::KeepEditing) | None => DraftAction::Stay,
    }
}

/// Validate editor fields over the saved provider and produce the provider to save or test
/// (`ReadFields`).
pub fn apply_draft(
    original: &Provider,
    draft: &DraftIdentity,
    allow_empty_model: bool,
) -> Result<Provider> {
    let d = draft.trimmed();
    let mut candidate = original.clone();
    candidate.name = d.name;
    candidate.base_url = d.base_url;
    candidate.model = d.model;
    candidate.protocol = if d.protocol.is_empty() {
        "ChatCompletions".into()
    } else {
        d.protocol
    };
    candidate.vision = d.vision;
    candidate.tools = false;
    candidate.realtime_url = d.realtime_url;
    candidate.realtime_model = d.realtime_model;
    candidate.realtime_protocol = if d.realtime_protocol.is_empty() {
        "OpenAIRealtime".into()
    } else {
        d.realtime_protocol
    };
    candidate.voice = d.voice;
    candidate.image_output = d.image_output
        && !candidate.is_hermes()
        && candidate.protocol_kind() != Some(Protocol::Anthropic);
    if candidate.name.is_empty() {
        return Err(AssistantError::invalid("Enter a provider name."));
    }
    if candidate.is_hermes() && candidate.protocol != "ChatCompletions" {
        return Err(AssistantError::invalid(
            "Select ChatCompletions for Hermes Agent.",
        ));
    }
    if !candidate.base_url.is_empty() {
        validate_endpoint(&candidate.base_url)?;
    }
    if !allow_empty_model && candidate.model.is_empty() && candidate.realtime_model.is_empty() {
        return Err(AssistantError::invalid(
            "Enter a chat model or realtime model ID.",
        ));
    }
    if !candidate.model.is_empty() && candidate.base_url.is_empty() {
        return Err(AssistantError::invalid("Enter an API base URL for chat."));
    }
    if !candidate.realtime_url.is_empty() {
        validate_realtime_url(&candidate.realtime_url)?;
    }
    if len16(&candidate.name) > 60 {
        candidate.name = candidate.name.chars().take(60).collect();
    }
    Ok(candidate)
}

/// Image replies are not available through Anthropic, and Hermes manages its own
/// (`RefreshCapabilities`).
pub fn image_replies_supported(provider: &Provider) -> bool {
    !provider.is_hermes() && provider.protocol_kind() != Some(Protocol::Anthropic)
}

// ----- image support check ----------------------------------------------------------------

const RED_SQUARE_PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAEAAAABACAIAAAAlC+aJAAAAS0lEQVR42u3PQQkAAAgAsetfWiP4FgYrsKZeS0BAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEDgsqnc8OJg6Ln3AAAAAElFTkSuQmCC";

/// A generated 64 x 64 red PNG. No personal image is ever sent for the check.
pub fn image_test_data() -> String {
    format!("data:image/png;base64,{RED_SQUARE_PNG}")
}

/// Whether the model's reply to the image check names the red sample.
pub fn image_reply_recognized(reply: &str) -> bool {
    reply
        .trim()
        .trim_matches(|c| matches!(c, '.' | '!' | ' '))
        .eq_ignore_ascii_case("red")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assistant::attachments::image_parts;

    fn provider(url: &str) -> Provider {
        Provider {
            base_url: url.into(),
            ..Provider::default()
        }
    }

    fn options_with(old: &Provider) -> Options {
        Options {
            selected_provider: old.id.clone(),
            selected_voice_provider: old.id.clone(),
            providers: vec![old.clone()],
            ..Options::default()
        }
    }

    #[test]
    fn credential_target_names_match_the_credential_manager_layout() {
        let id = new_id();
        assert_eq!(
            credential_target(&id).unwrap(),
            format!("NabyScape/Assistant/{id}")
        );
        assert!(credential_target("abc").is_err());
        assert!(credential_target(&"g".repeat(32)).is_err());
        assert!(prepare_key_for_storage("  sk-123  ").is_ok_and(|k| k == "sk-123"));
        assert!(prepare_key_for_storage("   ").is_err());
        assert!(prepare_key_for_storage("a\nb").is_err());
        assert!(prepare_key_for_storage(&"k".repeat(2561)).is_err());
    }

    #[test]
    fn failed_provider_save_never_associates_a_staged_key_with_the_old_destination() {
        let old = provider("https://old.example/v1/");
        let options = options_with(&old);
        let keys = MemoryKeys::new();
        keys.write(&old.id, "old-key").unwrap();
        let mut candidate = provider("https://new.example/v1/");
        candidate.id = old.id.clone();
        let mut staged = String::new();
        let result = save_provider(
            &options,
            &old.id,
            &candidate,
            "new-key",
            &keys,
            &mut |next| {
                staged = next.selected_provider.clone();
                assert_ne!(staged, old.id);
                assert_eq!(keys.read(&staged).unwrap().as_deref(), Some("new-key"));
                Err(AssistantError::io("disk full"))
            },
        );
        assert!(matches!(result, Err(AssistantError::Io(_))));
        assert_eq!(options.providers[0].base_url, "https://old.example/v1/");
        assert_eq!(options.selected_provider, old.id);
        assert_eq!(keys.read(&old.id).unwrap().as_deref(), Some("old-key"));
        assert!(!keys.exists(&staged).unwrap());
    }

    #[test]
    fn destination_changes_rotate_identity_without_carrying_a_key() {
        let old = provider("https://old.example/v1/");
        let options = options_with(&old);
        let keys = MemoryKeys::new();
        keys.write(&old.id, "old-key").unwrap();
        let mut candidate = provider("https://new.example/v1/");
        candidate.id = old.id.clone();
        let saved =
            save_provider(&options, &old.id, &candidate, "", &keys, &mut |_| Ok(())).unwrap();
        assert_ne!(saved.selected_provider, old.id);
        assert_eq!(saved.selected_voice_provider, saved.selected_provider);
        assert!(!keys.exists(&saved.selected_provider).unwrap());
        assert!(!keys.exists(&old.id).unwrap());
        assert_eq!(options.providers[0].base_url, "https://old.example/v1/");
    }

    #[test]
    fn unchanged_destination_keeps_identity_and_key() {
        let old = provider("https://api.example/v1/");
        let options = options_with(&old);
        let keys = MemoryKeys::new();
        keys.write(&old.id, "key").unwrap();
        let mut candidate = old.clone();
        candidate.model = "m2".into();
        let saved =
            save_provider(&options, &old.id, &candidate, "", &keys, &mut |_| Ok(())).unwrap();
        assert_eq!(saved.selected_provider, old.id);
        assert_eq!(saved.providers[0].model, "m2");
        assert_eq!(keys.read(&old.id).unwrap().as_deref(), Some("key"));
        // A replacement key rotates the identity and removes the old credential.
        let saved =
            save_provider(&options, &old.id, &candidate, "new", &keys, &mut |_| Ok(())).unwrap();
        assert_ne!(saved.selected_provider, old.id);
        assert_eq!(
            keys.read(&saved.selected_provider).unwrap().as_deref(),
            Some("new")
        );
        assert!(!keys.exists(&old.id).unwrap());
        // Hermes rotates when its path changes even on the same origin.
        let mut hermes = provider("http://127.0.0.1:8642/v1/");
        hermes.backend = "Hermes".into();
        let options = options_with(&hermes);
        let mut moved = hermes.clone();
        moved.base_url = "http://127.0.0.1:8642/p/work/v1/".into();
        let saved =
            save_provider(&options, &hermes.id, &moved, "", &keys, &mut |_| Ok(())).unwrap();
        assert_ne!(saved.selected_provider, hermes.id);
        assert!(save_provider(&options, "missing", &moved, "", &keys, &mut |_| Ok(())).is_err());
    }

    #[test]
    fn draft_guard_blocks_unsaved_switch() {
        let saved = provider("https://api.example/v1/");
        let mut draft = DraftIdentity::of(&saved);
        assert!(!has_provider_draft(&saved, &draft, false, false));
        // Whitespace-only edits are not drafts; real edits, a typed key or loading are.
        draft.model = format!("  {}  ", saved.model);
        assert!(!has_provider_draft(&saved, &draft, false, false));
        draft.model = "other".into();
        assert!(has_provider_draft(&saved, &draft, false, false));
        assert!(!has_provider_draft(&saved, &draft, false, true));
        assert!(has_provider_draft(
            &saved,
            &DraftIdentity::of(&saved),
            true,
            false
        ));
        // A pending draft blocks the switch until saved or discarded.
        assert_eq!(guard_switch(true, None), DraftAction::Stay);
        assert_eq!(
            guard_switch(true, Some(DraftChoice::KeepEditing)),
            DraftAction::Stay
        );
        assert_eq!(
            guard_switch(true, Some(DraftChoice::Save)),
            DraftAction::SaveThenProceed
        );
        assert_eq!(
            guard_switch(true, Some(DraftChoice::Discard)),
            DraftAction::Proceed
        );
        assert_eq!(guard_switch(false, None), DraftAction::Proceed);
    }

    #[test]
    fn draft_fields_are_validated_like_the_editor() {
        let saved = provider("https://api.example/v1/");
        let mut draft = DraftIdentity::of(&saved);
        draft.name = "  ".into();
        assert!(apply_draft(&saved, &draft, false).is_err());
        draft.name = "Mine".into();
        assert!(apply_draft(&saved, &draft, false).is_err());
        draft.model = "gpt".into();
        let ok = apply_draft(&saved, &draft, false).unwrap();
        assert_eq!(
            (ok.name.as_str(), ok.model.as_str(), ok.tools),
            ("Mine", "gpt", false)
        );
        draft.base_url = "http://remote.example/v1".into();
        assert!(apply_draft(&saved, &draft, false).is_err());
        draft.base_url = String::new();
        assert!(apply_draft(&saved, &draft, false).is_err());
        draft.base_url = saved.base_url.clone();
        draft.image_output = true;
        draft.protocol = "Anthropic".into();
        assert!(!apply_draft(&saved, &draft, false).unwrap().image_output);
        draft.protocol = "Responses".into();
        assert!(apply_draft(&saved, &draft, false).unwrap().image_output);
        draft.realtime_url = "ws://remote.example/".into();
        assert!(apply_draft(&saved, &draft, false).is_err());
        draft.realtime_url = "wss://provider.example/realtime".into();
        assert!(apply_draft(&saved, &draft, false).is_ok());
        let mut hermes = saved.clone();
        hermes.backend = "Hermes".into();
        let mut hd = DraftIdentity::of(&hermes);
        hd.model = "hermes-agent".into();
        hd.protocol = "Responses".into();
        assert!(apply_draft(&hermes, &hd, false).is_err());
        hd.protocol = "ChatCompletions".into();
        hd.image_output = true;
        assert!(!apply_draft(&hermes, &hd, false).unwrap().image_output);
        assert!(!image_replies_supported(&hermes));
    }

    #[test]
    fn drafts_only_reuse_a_saved_key_for_the_same_destination() {
        let saved = provider("https://api.example/v1/");
        let same = provider("https://api.example/v2/");
        let other = provider("https://evil.example/v1/");
        assert_eq!(
            draft_key_source("typed", Some(&saved), &other),
            KeySource::Entered
        );
        assert_eq!(
            draft_key_source("", Some(&saved), &same),
            KeySource::Saved(saved.id.clone())
        );
        assert_eq!(draft_key_source("", Some(&saved), &other), KeySource::None);
        assert_eq!(draft_key_source("", None, &same), KeySource::None);
        let mut hermes = provider("http://127.0.0.1:8642/v1/");
        hermes.backend = "Hermes".into();
        let mut profile = hermes.clone();
        profile.base_url = "http://127.0.0.1:8642/p/x/v1/".into();
        assert_eq!(
            draft_key_source("", Some(&hermes), &profile),
            KeySource::None
        );
        assert_eq!(
            draft_key_source("", Some(&hermes), &hermes),
            KeySource::Saved(hermes.id.clone())
        );
    }

    #[test]
    fn image_check_sample_is_a_valid_inline_png() {
        let data = image_test_data();
        assert_eq!(image_parts(&data).unwrap().0, "image/png");
        assert!(data.len() < 400);
        for ok in ["red", "Red.", " RED! ", "red ."] {
            assert!(image_reply_recognized(ok), "{ok}");
        }
        for bad in ["blue", "the color is red", ""] {
            assert!(!image_reply_recognized(bad), "{bad}");
        }
    }
}
