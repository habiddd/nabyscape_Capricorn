//! The two widget navigation tools (`AssistantWidgetTools.cs`): list the enabled widget pages
//! and open one. They only change the visible page; a navigate call becomes a
//! [`Command::NavigatePage`] on the [`CommandQueue`], executed by the UI thread.

use std::sync::{Arc, Mutex};

use naby_core::assistant::tools::{
    chat_tools as _chat_tools, check_call, list_response, navigate_response, no_pages_response,
    not_found_response, page_choices, pages_from_widgets, resolve_navigation, widget_tools,
    Navigation, Outcome, WidgetAction, WidgetPage,
};
use naby_core::assistant::{Call, Tool};
use naby_core::command::Command;
use naby_core::settings::{is_tool_name, widget_display_name, Settings};

use super::ports::{CommandQueue, ToolRunner};

/// Which panel page (dot) a widget lives on (the overlay's `PageFor`).
pub fn page_key(widget: &str) -> String {
    if is_tool_name(widget) {
        return widget.to_owned();
    }
    match widget {
        "media" | "volume" => "media",
        "timer" | "focus" => "focus",
        "stopwatch" => "stopwatch",
        "voice" => "voice",
        "shortcuts" => "access",
        "tools" => "tools",
        _ => "status",
    }
    .to_owned()
}

/// Display name of a page (the overlay's `PageName`).
pub fn page_name(key: &str) -> String {
    if is_tool_name(key) {
        return widget_display_name(key).unwrap_or(key).to_owned();
    }
    match key {
        "media" => "Media",
        "focus" => "Focus",
        "stopwatch" => "Stopwatch",
        "voice" => "Naby voice",
        "status" => "Status",
        "tools" => "Tools",
        _ => "Launch",
    }
    .to_owned()
}

#[derive(Default)]
struct Inner {
    assistant_enabled: bool,
    navigation: bool,
    pages: Vec<WidgetPage>,
    /// `(widget id, page id)` for every enabled widget.
    widget_pages: Vec<(String, String)>,
    enabled_widgets: Vec<String>,
    current: String,
}

/// The shared tool runner for chat and the Hermes bridge.
#[derive(Clone)]
pub struct WidgetTools {
    inner: Arc<Mutex<Inner>>,
    queue: CommandQueue,
}

impl WidgetTools {
    pub fn new(queue: CommandQueue) -> Self {
        Self {
            inner: Arc::default(),
            queue,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Recomputes the offered pages (settings changed, assistant switched on/off).
    pub fn update(&self, settings: &Settings, assistant_enabled: bool, navigation: bool) {
        let widget_pages: Vec<(String, String)> = settings
            .widgets
            .iter()
            .filter(|w| w.enabled)
            .map(|w| (w.id.clone(), page_key(&w.id)))
            .collect();
        let pages = pages_from_widgets(&widget_pages, &page_name);
        let mut inner = self.lock();
        inner.assistant_enabled = assistant_enabled;
        inner.navigation = navigation;
        inner.enabled_widgets = widget_pages.iter().map(|(w, _)| w.clone()).collect();
        inner.pages = pages;
        inner.widget_pages = widget_pages;
    }

    /// The page the dock shows when the host knows it.
    pub fn set_current(&self, page: &str) {
        self.lock().current = page.to_owned();
    }

    pub fn current(&self) -> String {
        self.lock().current.clone()
    }

    /// The tool list for a provider (`ChatWidgetTools`: Hermes manages its own tools).
    pub fn chat_tools(&self, provider: &mut naby_core::assistant::Provider) -> Vec<Tool> {
        _chat_tools(provider, self.tools())
    }
}

impl ToolRunner for WidgetTools {
    fn tools(&self) -> Vec<Tool> {
        let inner = self.lock();
        let choices = page_choices(&inner.pages, &inner.enabled_widgets);
        widget_tools(inner.assistant_enabled, inner.navigation, &choices)
    }

    fn execute(&self, call: &Call) -> String {
        let (enabled, navigation) = {
            let inner = self.lock();
            (inner.assistant_enabled, inner.navigation)
        };
        match check_call(call, enabled, navigation) {
            Outcome::Respond(text) => text,
            Outcome::Action(WidgetAction::ListPages) => {
                let inner = self.lock();
                let current = if inner.current.is_empty() {
                    "unavailable"
                } else {
                    &inner.current
                };
                list_response(current, &inner.pages)
            }
            Outcome::Action(WidgetAction::Navigate(requested)) => {
                let resolved = {
                    let inner = self.lock();
                    match resolve_navigation(
                        &requested,
                        &inner.pages,
                        &inner.widget_pages,
                        &inner.current,
                    ) {
                        Navigation::NoPages => Err(no_pages_response()),
                        Navigation::NotFound => Err(not_found_response()),
                        Navigation::Page(index) => Ok(inner.pages[index].clone()),
                    }
                };
                match resolved {
                    Err(text) => text,
                    Ok(page) => {
                        self.queue.push(Command::NavigatePage(page.widget.clone()));
                        self.lock().current = page.id.clone();
                        navigate_response(&page, true)
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn tools() -> (WidgetTools, CommandQueue) {
        let queue = CommandQueue::new(Arc::new(|| {}));
        let tools = WidgetTools::new(queue.clone());
        let mut settings = naby_core::settings::presets::recommended();
        for w in &mut settings.widgets {
            w.enabled = matches!(w.id.as_str(), "media" | "timer" | "notes" | "volume");
        }
        tools.update(&settings, true, true);
        (tools, queue)
    }

    fn call(name: &str, args: &str) -> Call {
        Call::new("call-1", name, args)
    }

    fn parse(text: &str) -> Value {
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn list_returns_enabled_pages_without_commands() {
        let (tools, queue) = tools();
        let reply = parse(&tools.execute(&call("naby_list_widget_pages", "{}")));
        assert_eq!(reply["ok"], true);
        let ids: Vec<&str> = reply["pages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["id"].as_str().unwrap())
            .collect();
        assert!(ids.contains(&"media") && ids.contains(&"focus") && ids.contains(&"notes"));
        assert_eq!(reply["current"], "unavailable");
        assert!(queue.drain().is_empty());
    }

    #[test]
    fn navigate_pushes_one_command_for_the_first_widget_of_the_page() {
        let (tools, queue) = tools();
        let reply = parse(&tools.execute(&call("naby_navigate_widget", r#"{"page":"timer"}"#)));
        assert_eq!(reply["ok"], true);
        assert_eq!(reply["page"], "focus");
        assert_eq!(queue.drain(), vec![Command::NavigatePage("timer".into())]);
        assert_eq!(tools.current(), "focus");
        let next = parse(&tools.execute(&call("naby_navigate_widget", r#"{"page":"next"}"#)));
        assert_eq!(next["ok"], true);
        assert_eq!(queue.drain().len(), 1);
    }

    #[test]
    fn disabled_widgets_and_bad_arguments_never_reach_the_ui() {
        let (tools, queue) = tools();
        let missing = parse(&tools.execute(&call("naby_navigate_widget", r#"{"page":"weather"}"#)));
        assert_eq!(missing["ok"], false);
        let bad = parse(&tools.execute(&call("naby_navigate_widget", r#"{"page":"media","x":1}"#)));
        assert_eq!(bad["ok"], false);
        let unknown = parse(&tools.execute(&call("shutdown_pc", "{}")));
        assert_eq!(unknown["ok"], false);
        assert!(queue.drain().is_empty());
    }

    #[test]
    fn navigation_off_offers_no_tools_and_refuses_calls() {
        let (tools, queue) = tools();
        tools.update(&naby_core::settings::presets::recommended(), true, false);
        assert!(tools.tools().is_empty());
        let reply = parse(&tools.execute(&call("naby_navigate_widget", r#"{"page":"media"}"#)));
        assert_eq!(reply["ok"], false);
        assert!(queue.drain().is_empty());
    }

    #[test]
    fn offered_tools_enumerate_the_page_choices() {
        let (tools, _) = tools();
        let offered = tools.tools();
        assert_eq!(offered.len(), 2);
        let choices = offered[1].parameters["properties"]["page"]["enum"]
            .as_array()
            .unwrap();
        assert!(choices.iter().any(|c| c == "next"));
        assert!(choices.iter().any(|c| c == "timer"));
    }
}
