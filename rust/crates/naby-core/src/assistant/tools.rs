//! Widget navigation tools offered to the model, and the MCP bridge protocol that exposes the
//! same two tools to Hermes (`AssistantWidgetTools.cs`, `AssistantWidgetBridge.cs`,
//! `BridgeRequest` in `AssistantWakeRuntime.cs`).
//!
//! The tools only change which widget page is visible: they never read or edit widget contents
//! and never start timers, recordings, playback or any computer action. The host maps
//! [`WidgetAction`] onto a `Command` and answers with [`navigate_response`].

use serde_json::{json, Map, Value};

use super::models::{Call, Provider, Tool};
use super::new_id;

pub const LIST_TOOL: &str = "naby_list_widget_pages";
pub const NAVIGATE_TOOL: &str = "naby_navigate_widget";
/// The longest tool-argument text accepted from a model.
pub const MAXIMUM_ARGUMENT_CHARACTERS: usize = 4096;
/// The longest page id accepted in a navigate call.
pub const MAXIMUM_PAGE_CHARACTERS: usize = 64;

/// An enabled widget page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WidgetPage {
    pub id: String,
    pub title: String,
    /// The first enabled widget on this page (what gets opened).
    pub widget: String,
}

/// Group enabled widgets by page (`AssistantWidgetPages`): `widgets` are `(widget id, page id)`
/// in display order, `title` maps a page id to its name.
pub fn pages_from_widgets(
    widgets: &[(String, String)],
    title: &dyn Fn(&str) -> String,
) -> Vec<WidgetPage> {
    let mut pages: Vec<WidgetPage> = Vec::new();
    for (widget, page) in widgets {
        if !pages.iter().any(|p| &p.id == page) {
            pages.push(WidgetPage {
                id: page.clone(),
                title: title(page),
                widget: widget.clone(),
            });
        }
    }
    pages
}

/// Values the `page` argument may take: page ids, enabled widget ids, `next`, `previous`.
pub fn page_choices(pages: &[WidgetPage], enabled_widgets: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let all = pages
        .iter()
        .map(|p| p.id.clone())
        .chain(enabled_widgets.iter().cloned())
        .chain(["next".to_string(), "previous".to_string()]);
    for id in all {
        if !out.contains(&id) {
            out.push(id);
        }
    }
    out
}

/// The tool definitions (`WidgetTools`); empty when the assistant or navigation is off.
pub fn widget_tools(assistant_enabled: bool, navigation: bool, choices: &[String]) -> Vec<Tool> {
    if !assistant_enabled || !navigation {
        return Vec::new();
    }
    vec![
        Tool::new(
            LIST_TOOL,
            "List enabled NabyScape widget pages and the current page. Read-only; does not inspect widget contents.",
            json!({ "type": "object", "properties": {}, "additionalProperties": false }),
        ),
        Tool::new(
            NAVIGATE_TOOL,
            "Open an enabled NabyScape widget page, or move to the next/previous page. Only changes the visible page; never starts a timer, recording, playback, app or computer action.",
            json!({
                "type": "object",
                "properties": { "page": { "type": "string", "enum": choices } },
                "required": ["page"],
                "additionalProperties": false
            }),
        ),
    ]
}

/// The tools a chat request carries (`ChatWidgetTools`): Hermes manages its own, and the
/// provider's tool flag is derived from what is offered.
pub fn chat_tools(provider: &mut Provider, tools: Vec<Tool>) -> Vec<Tool> {
    let tools = if provider.is_hermes() {
        Vec::new()
    } else {
        tools
    };
    provider.tools = !tools.is_empty();
    tools
}

/// What a validated tool call asks the host to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WidgetAction {
    ListPages,
    Navigate(String),
}

/// Result of checking a tool call before the host touches the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Answer the model with this JSON text immediately (an error).
    Respond(String),
    Action(WidgetAction),
}

fn error_json(message: &str) -> String {
    json!({ "ok": false, "error": message }).to_string()
}

/// Validate a model's tool call (`ExecuteWidgetTool`, before the UI is involved).
pub fn check_call(call: &Call, assistant_enabled: bool, navigation: bool) -> Outcome {
    if !assistant_enabled || !navigation {
        return Outcome::Respond(error_json(
            "AI widget navigation is off. Enable it in Naby AI settings.",
        ));
    }
    if call.arguments.encode_utf16().count() > MAXIMUM_ARGUMENT_CHARACTERS {
        return Outcome::Respond(error_json("Tool arguments are too large."));
    }
    let bad = || {
        Outcome::Respond(error_json(
            "Use an offered widget tool with the documented arguments.",
        ))
    };
    let Ok(Value::Object(args)) = serde_json::from_str::<Value>(&call.arguments) else {
        return bad();
    };
    if call.name == LIST_TOOL && args.is_empty() {
        return Outcome::Action(WidgetAction::ListPages);
    }
    if call.name != NAVIGATE_TOOL || args.len() != 1 {
        return bad();
    }
    match args.get("page").and_then(Value::as_str) {
        Some(page) if (1..=MAXIMUM_PAGE_CHARACTERS).contains(&page.encode_utf16().count()) => {
            Outcome::Action(WidgetAction::Navigate(page.to_string()))
        }
        _ => bad(),
    }
}

/// Answer for `naby_list_widget_pages`.
pub fn list_response(current: &str, pages: &[WidgetPage]) -> String {
    json!({
        "ok": true,
        "current": current,
        "pages": pages.iter().map(|p| json!({ "id": p.id, "title": p.title })).collect::<Vec<_>>()
    })
    .to_string()
}

/// How a navigate request resolves against the enabled pages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Navigation {
    NoPages,
    NotFound,
    /// Index into the page list.
    Page(usize),
}

/// Resolve `requested` (a page id, widget id, `next` or `previous`) like `AssistantNavigate`.
/// `widget_pages` maps each enabled widget id to the page that hosts it.
pub fn resolve_navigation(
    requested: &str,
    pages: &[WidgetPage],
    widget_pages: &[(String, String)],
    active_page: &str,
) -> Navigation {
    if pages.is_empty() {
        return Navigation::NoPages;
    }
    let page = requested.trim().to_lowercase();
    let current = pages.iter().position(|p| p.id == active_page);
    let index = if page == "next" || page == "previous" {
        let len = pages.len() as i64;
        let step = if page == "next" { 1 } else { -1 };
        Some(match current {
            None => {
                if page == "next" {
                    0
                } else {
                    pages.len() - 1
                }
            }
            Some(i) => ((i as i64 + step + len) % len) as usize,
        })
    } else {
        let hosted = widget_pages
            .iter()
            .find(|(widget, _)| *widget == page)
            .map(|(_, hosting)| hosting.as_str());
        pages
            .iter()
            .position(|p| p.id == page || hosted.is_some_and(|h| p.id == h))
    };
    index.map_or(Navigation::NotFound, Navigation::Page)
}

pub fn no_pages_response() -> String {
    error_json("No widget pages are enabled. Enable widgets in Settings first.")
}

pub fn not_found_response() -> String {
    error_json(
        "That page is disabled or unavailable. Use naby_list_widget_pages for enabled pages.",
    )
}

pub fn unavailable_response() -> String {
    error_json("The island is unavailable.")
}

/// Answer after navigating; `opened` is whether the page is now visible.
pub fn navigate_response(page: &WidgetPage, opened: bool) -> String {
    json!({
        "ok": opened,
        "page": page.id,
        "title": page.title,
        "message": "Opened the widget page. Its contents were not changed."
    })
    .to_string()
}

// ----- MCP bridge ---------------------------------------------------------------------------

/// Longest line the bridge reads in either direction.
pub const MAXIMUM_LINE_BYTES: usize = 16384;
/// Longest tool-arguments payload the stdio side forwards.
pub const MAXIMUM_FORWARDED_BYTES: usize = 8192;

/// Name of the per-user pipe the running app serves (the user's SID keeps other accounts out).
pub fn bridge_pipe_name(user_sid: &str) -> String {
    format!("NabyScape.WidgetPages.{user_sid}")
}

const BRIDGE_UNAVAILABLE: &str = "Start NabyScape on this Windows account and enable AI widget navigation in Naby AI settings. The bridge does not launch the app or execute other actions.";

/// The app side of the bridge (`BridgeRequest`): list tools or run one call.
pub fn bridge_dispatch(
    request: &Value,
    tools: &[Tool],
    execute: &mut dyn FnMut(&Call) -> String,
) -> Value {
    match request
        .get("operation")
        .and_then(Value::as_str)
        .unwrap_or("")
    {
        "tools" => json!({
            "tools": tools.iter().map(|t| json!({
                "name": t.name,
                "description": t.description,
                "inputSchema": t.parameters,
                "annotations": {
                    "readOnlyHint": t.name == LIST_TOOL,
                    "destructiveHint": false,
                    "openWorldHint": false
                }
            })).collect::<Vec<_>>()
        }),
        "call" => {
            let name = request.get("name").and_then(Value::as_str);
            let arguments = request.get("arguments");
            let (Some(name), Some(arguments)) = (name, arguments) else {
                return json!({ "ok": false, "error": "Unsupported widget bridge request." });
            };
            let call = Call::new(&new_id(), name, &arguments.to_string());
            serde_json::from_str(&execute(&call)).unwrap_or_else(
                |_| json!({ "ok": false, "error": "Unsupported widget bridge request." }),
            )
        }
        _ => json!({ "ok": false, "error": "Unsupported widget bridge request." }),
    }
}

/// Handle one line of the stdio MCP server (`RunStdio`). `forward` sends a request to the running
/// app over its pipe. Returns the response line, or `None` for notifications.
pub fn mcp_handle_line(
    line: &str,
    server_version: &str,
    forward: &mut dyn FnMut(&Value) -> Result<Value, ()>,
) -> Option<String> {
    let invalid = |id: Option<&Value>| {
        Some(
            json!({
                "jsonrpc": "2.0",
                "id": id.cloned().unwrap_or(Value::Null),
                "error": {
                    "code": -32602,
                    "message": "Invalid request, or NabyScape is not available. Start the app and enable AI widget navigation."
                }
            })
            .to_string(),
        )
    };
    let Ok(root) = serde_json::from_str::<Value>(line) else {
        return invalid(None);
    };
    if root.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return invalid(None);
    }
    let id = root.get("id")?;
    if !(id.is_number() || id.is_string()) || id.to_string().len() > 100 {
        return invalid(None);
    }
    let Some(method) = root.get("method").and_then(Value::as_str) else {
        return invalid(Some(id));
    };
    let result: Value = match method {
        "initialize" => {
            let asked = root
                .pointer("/params/protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or("");
            let version = if matches!(asked, "2024-11-05" | "2025-03-26" | "2025-06-18") {
                asked
            } else {
                "2024-11-05"
            };
            json!({
                "protocolVersion": version,
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": { "name": "nabyscape", "version": server_version }
            })
        }
        "ping" => json!({}),
        "tools/list" => match forward(&json!({ "operation": "tools" })) {
            Ok(value) => value,
            Err(()) => return invalid(Some(id)),
        },
        "tools/call" => {
            let outcome = (|| {
                let params = root.get("params")?;
                let name = params.get("name")?.as_str()?;
                let arguments = params
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                let reply =
                    forward(&json!({ "operation": "call", "name": name, "arguments": arguments }))
                        .ok()?;
                let error = reply.get("ok") == Some(&Value::Bool(false));
                Some(json!({
                    "content": [{ "type": "text", "text": reply.to_string() }],
                    "isError": error
                }))
            })();
            outcome.unwrap_or_else(|| {
                json!({
                    "content": [{ "type": "text", "text": BRIDGE_UNAVAILABLE }],
                    "isError": true
                })
            })
        }
        _ => {
            return Some(
                json!({
                    "jsonrpc": "2.0", "id": id,
                    "error": { "code": -32601, "message": "Method not supported" }
                })
                .to_string(),
            )
        }
    };
    let mut response = Map::new();
    response.insert("jsonrpc".into(), json!("2.0"));
    response.insert("id".into(), id.clone());
    response.insert("result".into(), result);
    Some(Value::Object(response).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pages() -> Vec<WidgetPage> {
        let widgets: Vec<(String, String)> = [
            ("weather", "weather"),
            ("tasks", "productivity"),
            ("notes", "productivity"),
            ("timer", "productivity"),
        ]
        .iter()
        .map(|(w, p)| (w.to_string(), p.to_string()))
        .collect();
        pages_from_widgets(&widgets, &|p| p.to_uppercase())
    }

    #[test]
    fn tools_are_offered_only_when_enabled_and_never_to_hermes() {
        let choices = page_choices(&pages(), &["weather".into(), "notes".into()]);
        assert_eq!(
            choices,
            ["weather", "productivity", "notes", "next", "previous"]
        );
        assert!(widget_tools(false, true, &choices).is_empty());
        assert!(widget_tools(true, false, &choices).is_empty());
        let tools = widget_tools(true, true, &choices);
        assert_eq!(
            tools.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
            [LIST_TOOL, NAVIGATE_TOOL]
        );
        assert_eq!(
            tools[1].parameters["properties"]["page"]["enum"],
            json!(choices)
        );
        assert_eq!(tools[1].parameters["required"], json!(["page"]));
        assert_eq!(tools[0].parameters["additionalProperties"], false);
        let mut chat = Provider::default();
        let offered = chat_tools(&mut chat, tools.clone());
        assert_eq!(offered.len(), 2);
        assert!(chat.tools);
        let mut hermes = Provider {
            backend: "Hermes".into(),
            tools: true,
            ..Provider::default()
        };
        assert!(chat_tools(&mut hermes, tools).is_empty());
        assert!(!hermes.tools);
        // Every offered tool is valid for the provider adapters.
        let provider = Provider {
            tools: true,
            base_url: "https://p.example/v1/".into(),
            model: "m".into(),
            ..Provider::default()
        };
        super::super::providers::validate_input(
            &provider,
            &[super::super::Message::user("hi")],
            &widget_tools(true, true, &choices),
        )
        .unwrap();
    }

    fn call(name: &str, args: &str) -> Call {
        Call::new("1", name, args)
    }

    #[test]
    fn calls_are_validated_before_the_ui_is_involved() {
        let err = |c: Call| match check_call(&c, true, true) {
            Outcome::Respond(text) => serde_json::from_str::<Value>(&text).unwrap()["error"]
                .as_str()
                .unwrap()
                .to_string(),
            other => panic!("{other:?}"),
        };
        assert_eq!(
            check_call(&call(LIST_TOOL, "{}"), true, true),
            Outcome::Action(WidgetAction::ListPages)
        );
        assert_eq!(
            check_call(&call(NAVIGATE_TOOL, r#"{"page":"weather"}"#), true, true),
            Outcome::Action(WidgetAction::Navigate("weather".into()))
        );
        assert!(
            matches!(check_call(&call(LIST_TOOL, "{}"), true, false), Outcome::Respond(t) if t.contains("navigation is off"))
        );
        assert!(
            matches!(check_call(&call(LIST_TOOL, "{}"), false, true), Outcome::Respond(t) if t.contains("navigation is off"))
        );
        let doc = "Use an offered widget tool with the documented arguments.";
        for (name, args) in [
            (LIST_TOOL, r#"{"x":1}"#),
            (NAVIGATE_TOOL, "{}"),
            (NAVIGATE_TOOL, r#"{"page":1}"#),
            (NAVIGATE_TOOL, r#"{"page":""}"#),
            (NAVIGATE_TOOL, r#"{"page":"a","extra":1}"#),
            (NAVIGATE_TOOL, "[]"),
            (NAVIGATE_TOOL, "not json"),
            ("other", "{}"),
        ] {
            assert_eq!(err(call(name, args)), doc, "{name} {args}");
        }
        let long = format!(r#"{{"page":"{}"}}"#, "p".repeat(65));
        assert_eq!(err(call(NAVIGATE_TOOL, &long)), doc);
        let huge = format!(r#"{{"page":"{}"}}"#, "p".repeat(5000));
        assert_eq!(
            err(call(NAVIGATE_TOOL, &huge)),
            "Tool arguments are too large."
        );
    }

    #[test]
    fn navigation_resolves_pages_widgets_and_neighbours() {
        let pages = pages();
        let widgets: Vec<(String, String)> = [
            ("weather", "weather"),
            ("tasks", "productivity"),
            ("notes", "productivity"),
        ]
        .iter()
        .map(|(w, p)| (w.to_string(), p.to_string()))
        .collect();
        let go = |req: &str, active: &str| resolve_navigation(req, &pages, &widgets, active);
        assert_eq!(go(" Weather ", "productivity"), Navigation::Page(0));
        assert_eq!(go("productivity", "weather"), Navigation::Page(1));
        assert_eq!(go("notes", "weather"), Navigation::Page(1));
        assert_eq!(go("next", "weather"), Navigation::Page(1));
        assert_eq!(go("next", "productivity"), Navigation::Page(0));
        assert_eq!(go("previous", "weather"), Navigation::Page(1));
        assert_eq!(go("next", "chat"), Navigation::Page(0));
        assert_eq!(go("previous", "chat"), Navigation::Page(1));
        assert_eq!(go("media", "weather"), Navigation::NotFound);
        assert_eq!(
            resolve_navigation("next", &[], &[], ""),
            Navigation::NoPages
        );
        let ok: Value = serde_json::from_str(&navigate_response(&pages[1], true)).unwrap();
        assert_eq!(ok["ok"], true);
        assert_eq!(ok["page"], "productivity");
        assert_eq!(
            ok["message"],
            "Opened the widget page. Its contents were not changed."
        );
        let list: Value = serde_json::from_str(&list_response("chat", &pages)).unwrap();
        assert_eq!(list["current"], "chat");
        assert_eq!(list["pages"][1]["title"], "PRODUCTIVITY");
        assert!(not_found_response().contains("naby_list_widget_pages"));
        assert!(no_pages_response().contains("Enable widgets"));
    }

    #[test]
    fn bridge_lists_and_calls_through_the_pipe_protocol() {
        let tools = widget_tools(true, true, &["weather".to_string()]);
        let listed = bridge_dispatch(
            &json!({"operation": "tools"}),
            &tools,
            &mut |_| unreachable!(),
        );
        assert_eq!(listed["tools"][0]["annotations"]["readOnlyHint"], true);
        assert_eq!(listed["tools"][1]["annotations"]["readOnlyHint"], false);
        assert_eq!(listed["tools"][1]["inputSchema"]["required"][0], "page");
        let mut seen = None;
        let reply = bridge_dispatch(
            &json!({"operation": "call", "name": NAVIGATE_TOOL, "arguments": {"page": "weather"}}),
            &tools,
            &mut |c| {
                seen = Some(c.clone());
                r#"{"ok":true}"#.to_string()
            },
        );
        assert_eq!(reply["ok"], true);
        let seen = seen.unwrap();
        assert_eq!(
            (seen.name.as_str(), seen.arguments.as_str()),
            (NAVIGATE_TOOL, r#"{"page":"weather"}"#)
        );
        assert!(crate::assistant::is_id(&seen.id));
        for bad in [
            json!({"operation": "other"}),
            json!({"operation": "call", "name": 5, "arguments": {}}),
            json!({"operation": "call", "name": "x"}),
            json!({}),
        ] {
            let r = bridge_dispatch(&bad, &tools, &mut |_| unreachable!());
            assert_eq!(r["error"], "Unsupported widget bridge request.");
        }
        assert_eq!(
            bridge_pipe_name("S-1-5-21"),
            "NabyScape.WidgetPages.S-1-5-21"
        );
    }

    fn rpc(line: &str, forward: &mut dyn FnMut(&Value) -> Result<Value, ()>) -> Option<Value> {
        mcp_handle_line(line, "3.0.0", forward).map(|s| serde_json::from_str(&s).unwrap())
    }

    #[test]
    fn mcp_stdio_speaks_the_supported_subset() {
        let mut offline = |_: &Value| Err(());
        let init = rpc(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}"#, &mut offline).unwrap();
        assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(init["result"]["serverInfo"]["name"], "nabyscape");
        assert_eq!(init["result"]["serverInfo"]["version"], "3.0.0");
        let init = rpc(r#"{"jsonrpc":"2.0","id":"a","method":"initialize","params":{"protocolVersion":"1999"}}"#, &mut offline).unwrap();
        assert_eq!(init["result"]["protocolVersion"], "2024-11-05");
        assert_eq!(init["id"], "a");
        assert_eq!(
            rpc(r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#, &mut offline).unwrap()["result"],
            json!({})
        );
        // Notifications get no reply.
        assert!(rpc(
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            &mut offline
        )
        .is_none());
        // Unsupported method.
        let r = rpc(
            r#"{"jsonrpc":"2.0","id":3,"method":"resources/list"}"#,
            &mut offline,
        )
        .unwrap();
        assert_eq!(r["error"]["code"], -32601);
        // App not running.
        let r = rpc(
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/list"}"#,
            &mut offline,
        )
        .unwrap();
        assert_eq!(r["error"]["code"], -32602);
        assert_eq!(r["id"], 4);
        let r = rpc(
            r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"x"}}"#,
            &mut offline,
        )
        .unwrap();
        assert_eq!(r["result"]["isError"], true);
        assert!(r["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Start NabyScape"));
        // Forwarding.
        let mut app = |req: &Value| -> Result<Value, ()> {
            Ok(if req["operation"] == "tools" {
                json!({"tools": []})
            } else {
                assert_eq!(req["arguments"], json!({}));
                json!({"ok": false, "error": "nope"})
            })
        };
        let r = rpc(
            r#"{"jsonrpc":"2.0","id":6,"method":"tools/list"}"#,
            &mut app,
        )
        .unwrap();
        assert_eq!(r["result"], json!({"tools": []}));
        let r = rpc(r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"naby_navigate_widget"}}"#, &mut app).unwrap();
        assert_eq!(r["result"]["isError"], true);
        assert!(r["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("nope"));
        // Malformed input.
        for line in [
            "not json",
            r#"{"jsonrpc":"1.0","id":1,"method":"ping"}"#,
            r#"{"jsonrpc":"2.0","id":{"a":1},"method":"ping"}"#,
        ] {
            let r = rpc(line, &mut offline).unwrap();
            assert_eq!(r["error"]["code"], -32602);
            assert_eq!(r["id"], Value::Null);
        }
        let r = rpc(r#"{"jsonrpc":"2.0","id":9}"#, &mut offline).unwrap();
        assert_eq!(r["id"], 9);
        assert_eq!(r["error"]["code"], -32602);
    }
}
