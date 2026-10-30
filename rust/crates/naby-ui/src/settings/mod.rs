//! The Settings window content (C# `UI/Settings`): navigation, search, the category pages, the
//! live-preview rail, Undo, change feedback and the debounced live apply.
//!
//! `SettingsView` is platform free. The Windows host (`naby-win::settings_host`) owns the window,
//! feeds [`SettingsView::frame`] pointer and keyboard input in **logical pixels** (physical
//! divided by the DPI scale), and executes the [`Command`]s it emits.
//!
//! # Applying changes
//!
//! Every edit goes through [`SettingsView::save`], which restarts the 180 ms debounce timer.
//! When it fires the working copy is normalized, compared with the last saved snapshot to build
//! the "Saved · ..." line and the Undo batch, and published as `Command::ApplySettings`. The host
//! saves and applies it, then calls [`SettingsView::saved`] with the result.
//!
//! # Naby AI hook
//!
//! The "Naby AI" category draws an [`ai::AiSettingsPage`]. The assistant tasks replace the default
//! stub with `SettingsView::set_ai_page`; navigation entry points are
//! [`SettingsView::open_ai`] (`OpenAiSettings`, `OpenAiHermesSettings`, `OpenAiVoiceSettings`).
//!
//! File map: one file per C# Settings partial (see the `mod` list).

pub mod ai;
mod ambient;
mod app_picker;
pub mod bounds;
pub mod categories;
mod chrome;
mod control_appearance;
mod controls;
mod customization;
mod diagnostics;
mod dock_placement;
mod dock_position_editor;
mod feature_resources;
mod feature_status;
pub mod feedback;
mod help;
mod home;
mod interaction_features;
mod kit;
mod language_accessibility;
mod material;
mod media_experience;
mod nav;
mod notifications;
mod organization;
mod pages;
mod pinned_preview;
mod preview;
mod preview_motion;
mod reset;
pub mod scene;
pub mod search;
pub mod undo;
mod updates;
mod weather;
mod widget_order;

#[cfg(test)]
mod tests;

pub use ai::{AiContext, AiSettingsPage, AiTarget, AssistantAiPage, DefaultAiPage};
pub use app_picker::AppChoice;
pub use chrome::{caption_button_names, window_title, NAV_WIDTH, RAIL_WIDTH};
pub use help::setting_description;
pub use pinned_preview::{pinned_height, should_pin};
pub use weather::WeatherFetch;

use crate::host::Snapshots;
use categories::{category, category_title, root};
use naby_core::command::Command;
use naby_core::i18n;
use naby_core::readiness::{AiInput, VoiceInput};
use naby_core::settings::reset::ResetScope;
use naby_core::settings::{BackdropResult, Settings};
use naby_core::store::Store;
use naby_render::{Canvas, Fonts, InputState, Rect, Theme, Ui, UiMemory};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use undo::Undo;

/// Debounce between the last edit and the save (C# `saveTimer`, 180 ms).
pub const SAVE_DELAY: f64 = 0.18;

/// A display the user can pin the dock to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorChoice {
    /// The device name stored in `Settings.Monitor` (`\\.\DISPLAY1`).
    pub device: String,
    /// `"\\.\DISPLAY1 · 1920 × 1080"`.
    pub label: String,
}

/// What the Updates section shows (the update service is owned by the host).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UpdateInfo {
    pub installed_version: String,
    pub platform: String,
    pub status: String,
    /// Version text of the available update, `None` before anything was checked.
    pub available: Option<String>,
    pub busy: bool,
    pub can_download: bool,
    pub can_install: bool,
    pub can_restore: bool,
    /// `OfficialUpdates.IsAutomaticInstallPath` / `IsInstalled`: picks the footnote.
    pub install_kind: InstallKind,
    pub recovery: String,
}

/// Where the running executable lives (`OfficialUpdates.IsAutomaticInstallPath`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum InstallKind {
    /// Installed under the current executable name: automatic install and recovery work.
    Automatic,
    /// Installed by an older setup.
    Legacy,
    /// A portable copy.
    #[default]
    Portable,
}

/// Facts only the host knows. The host pushes a fresh copy with
/// [`SettingsView::set_host_info`] when something changes; every field has a sensible default.
#[derive(Debug, Clone)]
pub struct HostInfo {
    pub monitors: Vec<MonitorChoice>,
    /// Work area height of the dock's display in layout pixels (limits "Top offset").
    pub work_area_height: f64,
    /// Work area width in layout pixels (the dock-position editor's map).
    pub work_area_width: f64,
    /// Registration status of the panel, visibility and tuck hotkeys.
    pub hotkey_status: [String; 3],
    pub brightness_status: String,
    pub frame_status: String,
    /// Result of the last Settings backdrop request.
    pub backdrop: Option<BackdropResult>,
    pub ai: AiInput,
    pub voice: VoiceInput,
    /// The media player gives enough track details and timing for lyrics.
    pub lyrics_can_time: bool,
    pub app_version: String,
    pub platform: String,
    pub update: UpdateInfo,
    pub diagnostics_report: String,
    pub settings_dir: String,
    /// Windows high-contrast colors (window, window text, highlight, highlight text).
    pub system_high_contrast: Option<[naby_core::color::Rgba; 4]>,
    /// Display scale of the dock's monitor in percent (the position editor shows it).
    pub display_scale_percent: f64,
    /// Installed apps for "Add installed app"; `None` while the host is still listing them.
    pub installed_apps: Option<Vec<AppChoice>>,
    /// Alpha of the window background: below 255 a native Mica/Acrylic backdrop shows through.
    pub window_alpha: u8,
}

impl Default for HostInfo {
    fn default() -> Self {
        HostInfo {
            monitors: Vec::new(),
            work_area_height: 1040.0,
            work_area_width: 1920.0,
            hotkey_status: Default::default(),
            brightness_status: String::new(),
            frame_status: String::new(),
            backdrop: None,
            ai: AiInput {
                enabled: false,
                chat_configured: false,
                needs_attention: false,
                chat_verified: false,
            },
            voice: VoiceInput {
                voice_connected: false,
                ai_enabled: false,
                speech_busy: false,
                managed_local_voice: false,
                chat_configured: false,
                voice_configured: false,
                local_voice_enabled: false,
            },
            lyrics_can_time: false,
            app_version: env!("CARGO_PKG_VERSION").to_owned(),
            platform: "win-x64".to_owned(),
            update: UpdateInfo {
                installed_version: env!("CARGO_PKG_VERSION").to_owned(),
                platform: "win-x64".to_owned(),
                status: "Updates have not been checked.".to_owned(),
                ..UpdateInfo::default()
            },
            diagnostics_report: String::new(),
            settings_dir: String::new(),
            system_high_contrast: None,
            display_scale_percent: 100.0,
            installed_apps: None,
            window_alpha: 255,
        }
    }
}

/// A confirmation dialog drawn over the window (C# `MessageBox`).
#[derive(Debug, Clone)]
pub(crate) struct Modal {
    pub title: String,
    pub body: String,
    pub confirm: String,
    pub action: ModalAction,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ModalAction {
    Reset(ResetScope),
    Restore,
    RestorePreviousVersion,
}

/// What a frame reports back to the host.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct FrameOutcome {
    /// Seconds until the view wants another frame (caret blink, debounce, animation).
    pub repaint_after: Option<f64>,
    /// Text the user copied or cut in a field.
    pub clipboard: Option<String>,
    /// A text field has keyboard focus (global shortcuts must not swallow typed keys).
    pub wants_text_input: bool,
    /// The pointer is over something clickable (the host shows the hand cursor).
    pub wants_hand: bool,
}

/// The Settings window.
pub struct SettingsView {
    pub(crate) lang: String,
    /// The working copy every control edits.
    pub(crate) s: Settings,
    pub(crate) store: Option<Arc<Store>>,
    /// JSON of the last preferences saved (C# `savedSettings`).
    pub(crate) saved: Value,
    undo: Undo,
    pub(crate) current: String,
    pub(crate) mem: UiMemory,
    pub(crate) now: f64,
    pub(crate) save_at: Option<f64>,
    pub(crate) feedback: String,
    pub(crate) feedback_error: bool,
    pub(crate) retry: bool,
    seen_version: u64,
    pub(crate) host: HostInfo,
    pub(crate) snaps: Option<Snapshots>,
    /// Number edit buffers: label -> (text, had focus last frame).
    pub(crate) fields: HashMap<String, (String, bool)>,
    /// Open expanders by key.
    pub(crate) open: HashSet<String>,
    /// Last measured heights of the section cards.
    pub(crate) section_h: HashMap<String, f32>,
    pub(crate) search: String,
    pub(crate) search_index: Vec<search::SearchItem>,
    pub(crate) search_focus: bool,
    pub(crate) pv: preview_motion::PreviewState,
    pub(crate) modal: Option<Modal>,
    pub(crate) reveal: Option<nav::Reveal>,
    /// Content-space positions registered this frame (section title / control name -> y).
    pub(crate) positions: HashMap<String, f32>,
    pub(crate) repaint: bool,
    pub(crate) out: Vec<Command>,
    pub(crate) ai_page: Box<dyn AiSettingsPage>,
    pub(crate) wired: String,
    pub(crate) drag: widget_order::DragState,
    pub(crate) shortcut_name: String,
    pub(crate) shortcut_target: String,
    pub(crate) weather_query: String,
    pub(crate) hand: bool,
    pub(crate) content_origin: f32,
    pub(crate) last_section: String,
    pub(crate) search_id: u64,
    pub(crate) search_enter: Option<bool>,
    pub(crate) weather_focus: bool,
    pub(crate) control_ids: HashMap<String, u64>,
    pub(crate) page_viewport: Rect,
    pub(crate) reset_choice: usize,
    pub(crate) lang_draft: Option<language_accessibility::LanguageDraft>,
    pub(crate) weather: weather::WeatherSearch,
    pub(crate) weather_fetch: WeatherFetch,
    pub(crate) picker: Option<app_picker::Picker>,
    pub(crate) picker_focused: bool,
    pub(crate) restart_note: Option<String>,
    pub(crate) diag_note: Option<String>,
    pub(crate) sound_note: Option<String>,
    pub(crate) lyrics_note: Option<String>,
    #[cfg(test)]
    pub(crate) rects: HashMap<String, Rect>,
}

impl SettingsView {
    /// Opens on the Start page. `store` is used for reset and restore of preferences.
    pub fn new(settings: Settings, store: Option<Arc<Store>>) -> SettingsView {
        let saved = serde_json::to_value(&settings).unwrap_or(Value::Null);
        SettingsView {
            lang: i18n::normalize_language(&settings.ui_language).to_owned(),
            s: settings,
            store,
            saved,
            undo: Undo::default(),
            current: "Home".to_owned(),
            mem: UiMemory::new(),
            now: 0.0,
            save_at: None,
            feedback: "Changes save automatically on this device.".to_owned(),
            feedback_error: false,
            retry: false,
            seen_version: 0,
            host: HostInfo::default(),
            snaps: None,
            fields: HashMap::new(),
            open: HashSet::new(),
            section_h: HashMap::new(),
            search: String::new(),
            search_index: search::build_index(),
            search_focus: false,
            pv: preview_motion::PreviewState::default(),
            modal: None,
            reveal: None,
            positions: HashMap::new(),
            repaint: false,
            out: Vec::new(),
            ai_page: Box::new(ai::DefaultAiPage::default()),
            wired: String::new(),
            drag: widget_order::DragState::default(),
            shortcut_name: String::new(),
            shortcut_target: String::new(),
            weather_query: String::new(),
            hand: false,
            content_origin: 0.0,
            last_section: String::new(),
            search_id: 0,
            search_enter: None,
            weather_focus: false,
            control_ids: HashMap::new(),
            page_viewport: Rect::default(),
            reset_choice: 0,
            lang_draft: None,
            weather: weather::WeatherSearch::default(),
            weather_fetch: Arc::new(naby_core::weather::default_fetch),
            picker: None,
            picker_focused: false,
            restart_note: None,
            diag_note: None,
            sound_note: None,
            lyrics_note: None,
            #[cfg(test)]
            rects: HashMap::new(),
        }
    }

    // ---- public API for the host -----------------------------------------------------------

    /// The working preferences (what the controls show).
    pub fn settings(&self) -> &Settings {
        &self.s
    }

    /// The category id on screen (`"Home"`, `"Appearance"`, ...).
    pub fn current_page(&self) -> &str {
        &self.current
    }

    /// The localized title of the page on screen.
    pub fn page_title(&self) -> String {
        category_title(&self.lang, &self.current)
    }

    /// Opens a category by its id or a friendly alias; unknown ids open Home.
    pub fn open_page(&mut self, page: &str) {
        let id = nav::resolve_page(page);
        self.navigate(id);
        if page.eq_ignore_ascii_case("voice") {
            self.open_ai(AiTarget::Voice);
        }
    }

    /// Opens the Naby AI category and asks the AI page to show `target`
    /// (`OpenAiSettings`, `OpenAiHermesSettings`, `OpenAiVoiceSettings`).
    pub fn open_ai(&mut self, target: AiTarget) {
        self.navigate("AI");
        self.ai_page.open(target);
    }

    /// Replaces the default Naby AI page (assistant tasks call this once).
    pub fn set_ai_page(&mut self, page: Box<dyn AiSettingsPage>) {
        self.ai_page = page;
    }

    /// The Naby AI category is showing and a worker published something it displays.
    pub fn ai_dirty(&self) -> bool {
        self.current == "AI" && self.ai_page.dirty()
    }

    /// Pushes fresh host facts (monitors, hotkey status, update state, ...).
    pub fn set_host_info(&mut self, info: HostInfo) {
        self.host = info;
        self.repaint = true;
    }

    /// The host saved (or failed to save) what `Command::ApplySettings` carried.
    pub fn saved(&mut self, ok: bool) {
        if ok {
            self.retry = false;
        } else {
            self.set_feedback(
                "Not saved · changes are active for this session only. Choose Retry save.",
                true,
            );
        }
    }

    /// The answer to a `Command::Settings(SettingsRequest::PickFile(..))`; `None` means the
    /// dialog was cancelled.
    pub fn file_picked(&mut self, purpose: naby_core::command::FilePurpose, path: Option<String>) {
        let Some(path) = path else { return };
        self.handle_file(purpose, &path);
    }

    /// The text of the status line (the "Saved ..." feedback), untranslated.
    pub fn feedback_text(&self) -> &str {
        &self.feedback
    }

    pub fn feedback_is_error(&self) -> bool {
        self.feedback_error
    }

    /// Whether Undo has a batch to restore.
    pub fn can_undo(&self) -> bool {
        self.undo.available()
    }

    /// Applies an edit exactly like a control would (debounced save, preview refresh).
    pub fn edit(&mut self, change: impl FnOnce(&mut Settings)) {
        change(&mut self.s);
        self.save();
    }

    /// Runs the pending save now (Settings closing, Undo, Reset).
    pub fn flush(&mut self) {
        if self.save_at.is_some() {
            self.commit();
        }
    }

    /// Frees the view's memory (window closed): fonts cache is the host's, here the widget
    /// state, edit buffers and caches go.
    pub fn release(&mut self) {
        self.flush();
        self.mem.clear();
        self.fields = HashMap::new();
        self.section_h = HashMap::new();
        self.positions = HashMap::new();
        self.snaps = None;
        self.pv = preview_motion::PreviewState::default();
        self.search_index = Vec::new();
        self.ai_page.on_close();
    }

    /// Commands produced outside of a frame (Undo from a shortcut, Reset).
    pub fn drain(&mut self, out: &mut Vec<Command>) {
        out.append(&mut self.out);
    }

    // ---- saving ------------------------------------------------------------------------------

    /// `Save()`: a preference changed; restart the debounce and refresh the preview.
    pub(crate) fn save(&mut self) {
        self.pv.revision += 1;
        self.feedback = "Saving changes…".to_owned();
        self.feedback_error = false;
        self.save_at = Some(self.now + SAVE_DELAY);
        self.repaint = true;
    }

    pub(crate) fn set_feedback(&mut self, text: &str, error: bool) {
        self.feedback = text.to_owned();
        self.feedback_error = error;
        self.retry = error && self.current != "AI";
        self.repaint = true;
    }

    /// `CommitSettings()`: normalize, publish, describe, remember Undo.
    pub(crate) fn commit(&mut self) {
        self.save_at = None;
        let mut next = self.s.clone();
        next.normalize();
        let after = serde_json::to_value(&next).unwrap_or(Value::Null);
        self.undo.capture(&self.saved, &after);
        let summary = feedback::describe(&self.saved, &next, &self.lang);
        self.saved = after;
        self.s = next.clone();
        self.out.push(Command::ApplySettings(Box::new(next)));
        self.set_feedback(&summary, false);
        self.retry = false;
    }

    /// Re-sends the current preferences after a failed save.
    pub(crate) fn retry_save(&mut self) {
        let next = self.s.clone();
        self.out.push(Command::ApplySettings(Box::new(next)));
        self.set_feedback("Saving changes…", false);
        self.retry = false;
    }

    /// `UndoSettings()`.
    pub(crate) fn undo_last(&mut self) {
        self.flush();
        let mut restored = self.s.clone();
        if !self.undo.apply(&mut restored) {
            return;
        }
        restored.normalize();
        self.saved = serde_json::to_value(&restored).unwrap_or(Value::Null);
        self.s = restored.clone();
        self.undo.clear();
        self.out.push(Command::ApplySettings(Box::new(restored)));
        self.pv.revision += 1;
        self.set_feedback(
            "Undone · your previous appearance, layout or widget choices are saved.",
            false,
        );
    }

    /// Adopts preferences another part of the app changed (dock dragged, widget toggled in the
    /// dock) unless an edit is waiting to be saved (`SettingsApplied`).
    fn sync_external(&mut self, snaps: &Snapshots) {
        let version = snaps.settings.version();
        if version == self.seen_version {
            return;
        }
        self.seen_version = version;
        if self.save_at.is_some() {
            return;
        }
        let incoming = serde_json::to_value(&**snaps.settings.arc()).unwrap_or(Value::Null);
        if incoming == self.saved {
            return;
        }
        let incoming_settings: Settings = (**snaps.settings.arc()).clone();
        if self.saved.is_null() {
            self.saved = incoming;
            self.s = incoming_settings;
            return;
        }
        self.undo.capture(&self.saved, &incoming);
        let summary = feedback::describe(&self.saved, &incoming_settings, &self.lang);
        self.saved = incoming;
        self.s = incoming_settings;
        self.set_feedback(&summary, false);
        self.pv.revision += 1;
    }

    /// Moves to a category (C# `Navigate`).
    pub fn navigate(&mut self, id: &str) {
        let id = category(id).id;
        let same = id == self.current;
        self.search.clear();
        self.lang_draft = None;
        self.restart_note = None;
        self.pv.on_navigate(id, same);
        self.current = id.to_owned();
        if !same {
            self.mem.set_scroll_offset("page", 0.0);
            self.mem.drop_focus();
        }
        self.feedback = if id == "AI" {
            "Connection fields use Save provider. Voice and wake word changes use their Save buttons.".to_owned()
        } else {
            "Changes save automatically on this device.".to_owned()
        };
        self.feedback_error = false;
        self.retry = false;
        self.repaint = true;
    }

    /// Opens the confirmation for resetting preferences.
    pub(crate) fn ask_reset(&mut self, scope: ResetScope) {
        use naby_core::settings::reset;
        self.modal = Some(Modal {
            title: "Reset preferences".to_owned(),
            body: reset::summary(scope),
            confirm: "OK".to_owned(),
            action: ModalAction::Reset(scope),
        });
        self.repaint = true;
    }

    pub(crate) fn ask_restore(&mut self) {
        self.modal = Some(Modal {
            title: "Restore preferences".to_owned(),
            body: "Restore the last preference backup? Startup, clipboard capture/history and saved content are kept.".to_owned(),
            confirm: "OK".to_owned(),
            action: ModalAction::Restore,
        });
        self.repaint = true;
    }

    /// The theme the window draws with: the user's colors, or the Windows high-contrast palette.
    fn make_theme(&self, scale: f32) -> Theme {
        let base = match self.host.system_high_contrast {
            Some([window, text, highlight, highlight_text]) => {
                Theme::high_contrast_for(&self.s, window, text, highlight, highlight_text)
            }
            None => Theme::from_settings(&self.s),
        };
        base.with_dpi(scale)
    }

    /// Paints the whole window into `canvas` (physical pixels) and applies `input` (logical
    /// pixels). Commands for the host are appended to `out`.
    pub fn frame(
        &mut self,
        canvas: &mut Canvas,
        fonts: &Fonts,
        input: &InputState,
        snaps: &Snapshots,
        scale: f32,
        out: &mut Vec<Command>,
    ) -> FrameOutcome {
        let scale = if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        };
        self.now = input.time;
        self.snaps = Some(snaps.clone());
        self.sync_external(snaps);
        self.lang = i18n::normalize_language(&self.s.ui_language).to_owned();
        if self.save_at.is_some_and(|at| at <= self.now) {
            self.commit();
        }
        let theme = self.make_theme(scale);
        let (cw, ch) = canvas.size();
        let area = Rect::new(0.0, 0.0, cw as f32 / scale, ch as f32 / scale);
        let pal = kit::Pal::of(&theme);
        let mut mem = std::mem::take(&mut self.mem);
        let blind = kit::blind(input);
        let mut wants_hand = false;
        self.positions.clear();
        #[cfg(test)]
        self.rects.clear();
        for pass in 0..4 {
            let quiet;
            let real: &InputState = if pass == 0 {
                input
            } else {
                quiet = input.without_events();
                &quiet
            };
            self.repaint = false;
            canvas.clear(pal.window.with_alpha(self.host.window_alpha));
            {
                let overlay = self.modal.is_some() || self.picker.is_some();
                let main_input = if overlay { &blind } else { real };
                let mut ui = Ui::new(canvas, fonts, &theme, main_input, &mut mem, area);
                self.layout(&mut ui, area, &pal);
                wants_hand |= self.hand;
            }
            if self.modal.is_some() || self.picker.is_some() {
                let mut ui = Ui::new(canvas, fonts, &theme, real, &mut mem, area);
                if self.modal.is_some() {
                    self.modal_ui(&mut ui, area, &pal);
                } else {
                    self.picker_ui(&mut ui, area, &pal);
                }
            }
            let again = mem.take_repaint() | std::mem::take(&mut self.repaint);
            if !again {
                break;
            }
        }
        let clipboard = mem.take_clipboard();
        let wants_text_input = mem.wants_text_input();
        let mut repaint_after = mem.repaint_after();
        self.mem = mem;
        if let Some(at) = self.save_at {
            let wait = (at - self.now).max(0.0);
            repaint_after = Some(repaint_after.map_or(wait, |r| r.min(wait)));
        }
        if let Some(wait) = self.pv.animation_wait(self.now) {
            repaint_after = Some(repaint_after.map_or(wait, |r| r.min(wait)));
        }
        out.append(&mut self.out);
        FrameOutcome {
            repaint_after,
            clipboard,
            wants_text_input,
            wants_hand,
        }
    }

    /// Seconds until the next debounce/animation deadline when nothing else wakes the view.
    pub fn next_wake(&self, now: f64) -> Option<f64> {
        let mut wait = self.save_at.map(|at| (at - now).max(0.0));
        if let Some(w) = self.pv.animation_wait(now) {
            wait = Some(wait.map_or(w, |x| x.min(w)));
        }
        wait
    }

    /// The current category's root builder name (`SettingsRoot`).
    pub(crate) fn root(&self) -> &str {
        root(&self.current)
    }

    pub(crate) fn tr<'k>(&self, key: &'k str) -> &'k str {
        i18n::t(&self.lang, key)
    }
}
