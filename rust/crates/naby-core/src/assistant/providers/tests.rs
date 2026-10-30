//! Ported from `AssistantProviderTests.cs` (request shapes, endpoint rules, reply parsing).

use std::net::IpAddr;

use serde_json::{json, Value};

use super::*;
use crate::assistant::models::Role;

const PROTOCOLS: [&str; 4] = ["ChatCompletions", "Responses", "Anthropic", "Gemini"];
const IMAGE: &str = "data:image/jpeg;base64,AQID";

fn tool() -> Tool {
    Tool::new(
        "inspect_app",
        "Inspect a selected app",
        json!({"type": "object", "properties": {}}),
    )
}

fn messages() -> Vec<Message> {
    let mut user = Message::user("Describe this");
    user.image_data_url = Some(IMAGE.into());
    vec![Message::system("Answer in Indonesian"), user]
}

fn provider(protocol: &str) -> Provider {
    Provider {
        protocol: protocol.into(),
        base_url: "https://provider.example/v1/".into(),
        model: "user-chosen-model".into(),
        tools: true,
        vision: true,
        ..Provider::default()
    }
}

fn reply_json(protocol: &str, call: bool) -> Value {
    match (protocol, call) {
        ("ChatCompletions", true) => {
            json!({"choices": [{"message": {"content": null, "tool_calls": [
            {"type": "function", "id": "call_1", "function": {"name": "inspect_app", "arguments": "{}"}}]}}]})
        }
        ("ChatCompletions", false) => json!({"choices": [{"message": {"content": "Hello"}}]}),
        ("Responses", true) => json!({"status": "completed", "output": [
            {"type": "reasoning", "id": "rs_1", "summary": [], "encrypted_content": "opaque"},
            {"type": "function_call", "id": "fc_1", "call_id": "call_1", "name": "inspect_app", "arguments": "{}"}]}),
        ("Responses", false) => json!({"output": [{"type": "message", "role": "assistant",
            "content": [{"type": "output_text", "text": "Hello"}]}]}),
        ("Anthropic", true) => {
            json!({"content": [{"type": "tool_use", "id": "call_1", "name": "inspect_app", "input": {}}]})
        }
        ("Anthropic", false) => json!({"content": [{"type": "text", "text": "Hello"}]}),
        (_, true) => json!({"candidates": [{"content": {"parts": [
            {"functionCall": {"id": "call_1", "name": "inspect_app", "args": {}}, "thoughtSignature": "opaque"}]}}]}),
        (_, false) => json!({"candidates": [{"content": {"parts": [{"text": "Hello"}]}}]}),
    }
}

fn build(protocol: &str, messages: &[Message], tools: &[Tool]) -> HttpRequest {
    build_request(
        &provider(protocol),
        Some("fixture-key"),
        messages,
        tools,
        false,
    )
    .unwrap()
}

fn wire(request: &HttpRequest) -> String {
    String::from_utf8(request.body.clone()).unwrap()
}

#[test]
fn provider_presets_retain_explicit_capabilities_and_a_hermes_agent_alias() {
    assert_eq!(PRESETS.len(), 11);
    for name in PRESETS {
        let provider = create(name).unwrap();
        let hermes = name == "Hermes Agent";
        assert_eq!(provider.model, if hermes { "hermes-agent" } else { "" });
        assert_eq!(provider.backend, if hermes { "Hermes" } else { "" });
        assert!(!provider.vision && !provider.tools);
        assert_eq!(provider.name, name);
        if !name.starts_with("Custom") {
            validate_endpoint(&provider.base_url).unwrap();
        }
    }
    assert!(matches!(
        create("unrecognized"),
        Err(AssistantError::Invalid(_))
    ));
    assert_eq!(create("OpenAI").unwrap().protocol, "Responses");
    assert_eq!(create("Google Gemini").unwrap().protocol, "Gemini");
    assert_eq!(
        create("Anthropic").unwrap().base_url,
        "https://api.anthropic.com/v1/"
    );
    assert_eq!(create("Custom Anthropic").unwrap().base_url, "");
    assert_eq!(
        create("LM Studio").unwrap().base_url,
        "http://localhost:1234/v1/"
    );
}

#[test]
fn endpoints_block_cleartext_remote_private_ip_credentials_query_and_fragments() {
    for url in [
        "",
        "http://provider.example/v1",
        "https://a:b@provider.example/v1",
        "https://provider.example/#fragment",
        "https://provider.example/?key=secret",
        "https://192.168.1.1",
        "https://169.254.169.254",
        "https://10.0.0.1",
        "https://[fc00::1]",
        "https://[::ffff:192.168.1.1]",
        "https://app.local",
        "https://app.internal",
        "https://localhost.evil/../#x",
        "https://provider.example./",
        "https://nodot",
        "ftp://provider.example/",
        "not a url",
    ] {
        assert!(
            matches!(validate_endpoint(url), Err(AssistantError::Invalid(_))),
            "{url}"
        );
    }
    for url in [
        "https://provider.example/v1",
        "http://localhost:11434/v1",
        "http://127.0.0.1:1234/v1",
        "http://[::1]:1234/v1",
        "https://api.openai.com/v1/",
    ] {
        validate_endpoint(url).unwrap();
    }
    assert_eq!(
        validate_endpoint("https://provider.example/v1")
            .unwrap()
            .as_str(),
        "https://provider.example/v1/"
    );
    assert_eq!(
        validate_endpoint("http://localhost:11434")
            .unwrap()
            .as_str(),
        "http://localhost:11434/"
    );
    let ip = |s: &str| s.parse::<IpAddr>().unwrap();
    assert!(is_public_address(ip("8.8.8.8")) && !is_public_address(ip("100.64.0.1")));
    for private in [
        "0.1.2.3",
        "10.1.1.1",
        "127.0.0.1",
        "169.254.1.1",
        "172.16.0.1",
        "172.31.255.255",
        "192.168.0.1",
        "192.0.2.1",
        "198.18.0.1",
        "198.51.100.1",
        "203.0.113.5",
        "224.0.0.1",
        "::1",
        "fe80::1",
        "2001:db8::1",
        "2001::1",
        "::ffff:10.0.0.1",
    ] {
        assert!(!is_public_address(ip(private)), "{private}");
    }
    for public in ["1.1.1.1", "172.32.0.1", "2606:4700::1111", "::ffff:8.8.8.8"] {
        assert!(is_public_address(ip(public)), "{public}");
    }
}

#[test]
fn openai_request_shape() {
    let request = build("ChatCompletions", &messages(), &[tool()]);
    assert_eq!(request.method, Method::Post);
    assert_eq!(request.url, "https://provider.example/v1/chat/completions");
    assert_eq!(request.header("authorization"), Some("Bearer fixture-key"));
    assert_eq!(request.header("Content-Type"), Some("application/json"));
    assert_eq!(request.header("accept"), Some("application/json"));
    assert!(request.header("x-api-key").is_none() && request.header("anthropic-version").is_none());
    let body = request.json().unwrap();
    assert_eq!(body["model"], "user-chosen-model");
    assert_eq!(body["stream"], false);
    let list = body["messages"].as_array().unwrap();
    assert_eq!(
        list[0],
        json!({"role": "system", "content": "Answer in Indonesian"})
    );
    assert_eq!(list[1]["role"], "user");
    assert_eq!(
        list[1]["content"][0],
        json!({"type": "text", "text": "Describe this"})
    );
    assert_eq!(list[1]["content"][1]["image_url"]["url"], IMAGE);
    assert_eq!(body["tools"][0]["type"], "function");
    assert_eq!(body["tools"][0]["function"]["name"], "inspect_app");
    assert_eq!(body["tools"][0]["function"]["parameters"]["type"], "object");
    // Streaming changes only the flag and the Accept header.
    let streaming = build_request(
        &provider("ChatCompletions"),
        Some("k"),
        &messages(),
        &[tool()],
        true,
    )
    .unwrap();
    assert_eq!(streaming.header("accept"), Some("text/event-stream"));
    assert_eq!(streaming.json().unwrap()["stream"], true);
    // A plain text message keeps string content.
    let plain = build("ChatCompletions", &[Message::user("hi")], &[]);
    assert_eq!(plain.json().unwrap()["messages"][0]["content"], "hi");
    assert!(plain.json().unwrap().get("tools").is_none());
}

#[test]
fn responses_request_shape() {
    let request = build("Responses", &messages(), &[tool()]);
    assert_eq!(request.url, "https://provider.example/v1/responses");
    assert_eq!(request.header("authorization"), Some("Bearer fixture-key"));
    let body = request.json().unwrap();
    assert_eq!(body["store"], false);
    assert_eq!(body["include"], json!(["reasoning.encrypted_content"]));
    assert_eq!(body["max_output_tokens"], 4096);
    assert_eq!(body["model"], "user-chosen-model");
    let input = body["input"].as_array().unwrap();
    assert_eq!(
        input[0]["content"][0],
        json!({"type": "input_text", "text": "Answer in Indonesian"})
    );
    assert_eq!(
        input[1]["content"][1],
        json!({"type": "input_image", "image_url": IMAGE})
    );
    assert_eq!(body["tools"][0]["type"], "function");
    assert_eq!(body["tools"][0]["strict"], false);
    assert_eq!(body["tools"][0]["parameters"]["type"], "object");
    // Assistant history uses output_text.
    let history = vec![
        Message::user("a"),
        Message::assistant("b"),
        Message::user("c"),
    ];
    let body = build("Responses", &history, &[]).json().unwrap();
    assert_eq!(body["input"][1]["content"][0]["type"], "output_text");
}

#[test]
fn anthropic_request_shape() {
    let request = build("Anthropic", &messages(), &[tool()]);
    assert_eq!(request.url, "https://provider.example/v1/messages");
    assert_eq!(request.header("x-api-key"), Some("fixture-key"));
    assert_eq!(request.header("anthropic-version"), Some("2023-06-01"));
    assert!(request.header("authorization").is_none());
    let body = request.json().unwrap();
    assert_eq!(body["model"], "user-chosen-model");
    assert_eq!(body["max_tokens"], 4096);
    assert_eq!(body["system"], "Answer in Indonesian");
    let list = body["messages"].as_array().unwrap();
    assert_eq!(list.len(), 1, "system text moves to the system field");
    assert_eq!(list[0]["role"], "user");
    assert_eq!(
        list[0]["content"][0],
        json!({"type": "text", "text": "Describe this"})
    );
    assert_eq!(
        list[0]["content"][1],
        json!({"type": "image", "source": {"type": "base64", "media_type": "image/jpeg", "data": "AQID"}})
    );
    assert_eq!(body["tools"][0]["input_schema"]["type"], "object");
    assert!(body["tools"][0].get("parameters").is_none());
    // Consecutive turns of the same role merge, assistants are "assistant".
    let history = vec![
        Message::user("a"),
        Message::user("b"),
        Message::assistant("c"),
    ];
    let body = build("Anthropic", &history, &[]).json().unwrap();
    assert_eq!(body["messages"].as_array().unwrap().len(), 2);
    assert_eq!(body["messages"][0]["content"].as_array().unwrap().len(), 2);
    assert_eq!(body["messages"][1]["role"], "assistant");
    // Empty content is replaced by a single space.
    let body = build("Anthropic", &[Message::user("")], &[])
        .json()
        .unwrap();
    assert_eq!(body["messages"][0]["content"][0]["text"], " ");
}

#[test]
fn gemini_request_shape() {
    let request = build("Gemini", &messages(), &[tool()]);
    assert_eq!(
        request.url,
        "https://provider.example/v1/models/user-chosen-model:generateContent"
    );
    assert_eq!(request.header("x-goog-api-key"), Some("fixture-key"));
    let body = request.json().unwrap();
    assert!(body.get("model").is_none());
    assert_eq!(body["generationConfig"]["maxOutputTokens"], 4096);
    assert_eq!(
        body["systemInstruction"]["parts"][0]["text"],
        "Answer in Indonesian"
    );
    let contents = body["contents"].as_array().unwrap();
    assert_eq!(contents.len(), 1);
    assert_eq!(contents[0]["role"], "user");
    assert_eq!(
        contents[0]["parts"][1]["inlineData"],
        json!({"mimeType": "image/jpeg", "data": "AQID"})
    );
    assert_eq!(
        body["tools"][0]["functionDeclarations"][0]["name"],
        "inspect_app"
    );
    // Models with reserved characters are escaped in the route.
    let mut odd = provider("Gemini");
    odd.model = "tuned/model one".into();
    let request = build_request(&odd, None, &[Message::user("hi")], &[], false).unwrap();
    assert!(
        request
            .url
            .ends_with("/models/tuned%2Fmodel%20one:generateContent"),
        "{}",
        request.url
    );
    assert!(request.header("x-goog-api-key").is_none());
    let history = vec![Message::user("a"), Message::assistant("b")];
    let body = build("Gemini", &history, &[]).json().unwrap();
    assert_eq!(body["contents"][1]["role"], "model");
}

#[test]
fn gemini_preserves_strict_nested_json_schema_in_parameters_json_schema() {
    let schema = json!({"type": "object", "additionalProperties": false,
        "$defs": {"target": {"type": "object", "additionalProperties": false,
            "properties": {"title": {"type": "string"}}, "required": ["title"]}},
        "properties": {"target": {"$ref": "#/$defs/target"}}, "required": ["target"]});
    let strict = Tool::new("inspect_app", "Inspect a selected app", schema.clone());
    let body = build("Gemini", &messages(), &[strict]).json().unwrap();
    let declaration = &body["tools"][0]["functionDeclarations"][0];
    assert!(declaration.get("parameters").is_none());
    assert_eq!(declaration["parametersJsonSchema"], schema);
    assert_eq!(
        declaration["parametersJsonSchema"]["additionalProperties"],
        false
    );
}

#[test]
fn each_protocol_parses_replies_and_calls() {
    for protocol in PROTOCOLS {
        let p = provider(protocol);
        let reply = parse_response(&p, &reply_json(protocol, false), &[tool()]).unwrap();
        assert_eq!(
            (reply.text.as_str(), reply.calls.len()),
            ("Hello", 0),
            "{protocol}"
        );
        let reply = parse_response(&p, &reply_json(protocol, true), &[tool()]).unwrap();
        assert_eq!(reply.calls.len(), 1, "{protocol}");
        assert_eq!(
            (
                reply.calls[0].id.as_str(),
                reply.calls[0].name.as_str(),
                reply.calls[0].arguments.as_str()
            ),
            ("call_1", "inspect_app", "{}")
        );
        if protocol == "Gemini" {
            assert_eq!(reply.calls[0].thought_signature.as_deref(), Some("opaque"));
        }
        assert_eq!(
            reply.protocol_items.is_empty(),
            protocol == "ChatCompletions"
        );
    }
}

#[test]
fn each_protocol_round_trips_tool_ids_arguments_and_opaque_continuation() {
    for protocol in PROTOCOLS {
        let p = provider(protocol);
        let first = parse_response(&p, &reply_json(protocol, true), &[tool()]).unwrap();
        let mut assistant = Message::assistant(&first.text);
        assistant.calls = first.calls.clone();
        assistant.protocol_items = first.protocol_items.clone();
        let history = vec![
            Message::user("inspect"),
            assistant,
            Message::tool_result(&first.calls[0].id, "selected app result"),
        ];
        let request = build_request(&p, None, &history, &[tool()], false).unwrap();
        let body = request.json().unwrap();
        let text = wire(&request);
        assert!(
            text.contains("call_1") && text.contains("selected app result"),
            "{protocol}"
        );
        match protocol {
            "ChatCompletions" => {
                assert!(text.contains("tool_call_id") && text.contains("tool_calls"))
            }
            "Responses" => {
                assert!(
                    text.contains("function_call_output") && text.contains("encrypted_content")
                );
                let count = body["input"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|i| i["type"] == "function_call")
                    .count();
                assert_eq!(count, 1);
            }
            "Anthropic" => {
                assert!(text.contains("tool_result"));
                assert_eq!(body["messages"][2]["role"], "user");
            }
            _ => assert!(text.contains("functionResponse") && text.contains("thoughtSignature")),
        }
    }
}

#[test]
fn parallel_tool_results_form_a_single_user_turn() {
    for protocol in ["Anthropic", "Gemini"] {
        let p = provider(protocol);
        let mut assistant = Message::assistant("");
        assistant.calls = vec![
            Call::new("a", "inspect_app", "{}"),
            Call::new("b", "inspect_app", "{}"),
        ];
        let history = vec![
            Message::user("go"),
            assistant,
            Message::tool_result("a", "one"),
            Message::tool_result("b", "two"),
        ];
        let body = build_request(&p, None, &history, &[tool()], false)
            .unwrap()
            .json()
            .unwrap();
        let field = if protocol == "Anthropic" {
            "content"
        } else {
            "parts"
        };
        let turns = if protocol == "Anthropic" {
            &body["messages"]
        } else {
            &body["contents"]
        };
        assert_eq!(turns.as_array().unwrap().len(), 3, "{protocol}");
        assert_eq!(turns[2][field].as_array().unwrap().len(), 2, "{protocol}");
    }
    // Gemini local call ids are never sent back.
    let p = provider("Gemini");
    let mut assistant = Message::assistant("");
    assistant.calls = vec![Call::new("gemini_local_x", "inspect_app", "{}")];
    let history = vec![
        Message::user("go"),
        assistant,
        Message::tool_result("gemini_local_x", "r"),
    ];
    let text = wire(&build_request(&p, None, &history, &[tool()], false).unwrap());
    assert!(!text.contains("gemini_local_x"));
}

#[test]
fn invalid_empty_malformed_and_unoffered_replies_use_safe_fixed_errors() {
    for protocol in PROTOCOLS {
        let p = provider(protocol);
        let mut unknown = reply_json(protocol, true);
        let text = unknown.to_string().replace("inspect_app", "unknown_tool");
        unknown = serde_json::from_str(&text).unwrap();
        let bodies = [
            json!(null),
            json!({}),
            json!([]),
            json!({"error": {"message": "fixture-secret"}}),
            unknown,
        ];
        for body in bodies {
            let error = parse_response(&p, &body, &[tool()]).unwrap_err();
            assert!(matches!(error, AssistantError::Io(_)), "{protocol} {body}");
            assert!(!error.to_string().contains("fixture-secret"));
            assert_eq!(
                error.to_string(),
                "The provider returned an invalid or unsupported response."
            );
        }
    }
    for body in [
        json!({"status": "incomplete", "output": []}),
        json!({"status": "failed", "output": [{"type": "message", "content": []}]}),
    ] {
        assert!(parse_response(&provider("Responses"), &body, &[tool()]).is_err());
    }
    assert!(parse_response(
        &provider("ChatCompletions"),
        &json!({"choices": [{"finish_reason": "length", "message": {"content": "cut"}}]}),
        &[]
    )
    .is_err());
    assert!(parse_response(
        &provider("Anthropic"),
        &json!({"stop_reason": "max_tokens", "content": [{"type": "text", "text": "cut"}]}),
        &[]
    )
    .is_err());
    assert!(parse_response(
        &provider("Gemini"),
        &json!({"candidates": [{"finishReason": "SAFETY", "content": {"parts": [{"text": "x"}]}}]}),
        &[]
    )
    .is_err());
    // Refusals are shown as text; Gemini thoughts are not.
    let reply = parse_response(
        &provider("ChatCompletions"),
        &json!({"choices": [{"message": {"content": null, "refusal": "No."}}]}),
        &[],
    )
    .unwrap();
    assert_eq!(reply.text, "No.");
    let reply = parse_response(
        &provider("Gemini"),
        &json!({"candidates": [{"content": {"parts": [
        {"thought": true, "text": "thinking"}, {"text": "answer"}]}}]}),
        &[],
    )
    .unwrap();
    assert_eq!(reply.text, "answer");
    // Oversized text is rejected.
    let big = "x".repeat(MAXIMUM_TEXT_CHARACTERS + 1);
    assert!(parse_response(
        &provider("ChatCompletions"),
        &json!({"choices": [{"message": {"content": big}}]}),
        &[]
    )
    .is_err());
}

#[test]
fn disabled_or_malformed_tool_calls_and_partial_outputs_are_rejected() {
    for protocol in PROTOCOLS {
        let p = provider(protocol);
        // The provider only sees tools when its flag is set.
        let offered: &[Tool] = active_tools(
            &Provider {
                tools: false,
                ..p.clone()
            },
            &[],
        );
        assert!(offered.is_empty());
        assert!(
            parse_response(&p, &reply_json(protocol, true), &[]).is_err(),
            "{protocol}"
        );
        let text = reply_json(protocol, true).to_string();
        let malformed = text
            .replace("call_1", "")
            .replace("inspect_app", "invalid name");
        let body: Value = serde_json::from_str(&malformed).unwrap();
        assert!(parse_response(&p, &body, &[tool()]).is_err(), "{protocol}");
    }
    // Hermes never gets local tools.
    let hermes = Provider {
        backend: "Hermes".into(),
        tools: true,
        ..provider("ChatCompletions")
    };
    assert!(active_tools(&hermes, &[tool()]).is_empty());
    assert_eq!(active_tools(&provider("Responses"), &[tool()]).len(), 1);
    let request =
        build_request(&hermes, None, &messages_without_image(), &[tool()], false).unwrap();
    assert!(request.json().unwrap().get("tools").is_none());
}

fn messages_without_image() -> Vec<Message> {
    vec![Message::system("s"), Message::user("hi")]
}

#[test]
fn capability_input_text_call_id_and_tool_count_limits_fail_before_network() {
    let mut p = provider("ChatCompletions");
    p.vision = false;
    let build = |p: &Provider, m: &[Message], t: &[Tool], key: Option<&str>| {
        build_request(p, key, m, t, false)
    };
    assert!(matches!(
        build(&p, &messages(), &[tool()], None),
        Err(AssistantError::Invalid(_))
    ));
    p.vision = true;
    p.model = String::new();
    assert!(build(&p, &messages(), &[tool()], None).is_err());
    p.model = "selected".into();
    let mut forged = Message::assistant("");
    forged.calls = vec![Call::new("", "inspect_app", "{}")];
    let mut image_for_assistant = Message::assistant("x");
    image_for_assistant.image_data_url = Some(IMAGE.into());
    let mut external = Message::user("bad image");
    external.image_data_url = Some("https://external.example/image.jpg".into());
    for invalid in [
        vec![],
        vec![Message::user(&"a".repeat(MAXIMUM_TEXT_CHARACTERS + 1))],
        vec![Message::tool_result("unknown", "orphan")],
        vec![external],
        vec![forged],
        vec![image_for_assistant],
        vec![Message::user("x"); 101],
    ] {
        assert!(matches!(
            build(&p, &invalid, &[tool()], None),
            Err(AssistantError::Invalid(_))
        ));
    }
    assert!(build(&p, &[Message::user("test")], &vec![tool(); 33], None).is_err());
    assert!(
        build(&p, &[Message::user("test")], &[tool(), tool()], None).is_err(),
        "duplicate tool names"
    );
    let mut oversized_tool = tool();
    oversized_tool.description = "d".repeat(4097);
    assert!(build(&p, &[Message::user("test")], &[oversized_tool], None).is_err());
    let mut bad_name = tool();
    bad_name.name = "bad name".into();
    assert!(build(&p, &[Message::user("test")], &[bad_name], None).is_err());
    let mut array_schema = tool();
    array_schema.parameters = json!([]);
    assert!(build(&p, &[Message::user("test")], &[array_schema], None).is_err());
    for key in ["bad\r\nkey", &"k".repeat(4097)] {
        assert!(matches!(
            build(&p, &messages(), &[tool()], Some(key)),
            Err(AssistantError::Invalid(_))
        ));
    }
    for url in ["http://remote.example/v1", "", "https://192.168.0.2/v1"] {
        let mut remote = p.clone();
        remote.base_url = url.into();
        assert!(
            build(&remote, &messages(), &[tool()], None).is_err(),
            "{url}"
        );
    }
    let mut unknown = p.clone();
    unknown.protocol = "Mystery".into();
    assert!(build(&unknown, &messages(), &[tool()], None).is_err());
    // Tool calls on messages need the provider's tool flag, and duplicates are refused.
    let mut calls = Message::assistant("");
    calls.calls = vec![
        Call::new("c", "inspect_app", "{}"),
        Call::new("c", "inspect_app", "{}"),
    ];
    assert!(build(&p, &[Message::user("x"), calls.clone()], &[tool()], None).is_err());
    calls.calls.truncate(1);
    let ok = vec![
        Message::user("x"),
        calls.clone(),
        Message::tool_result("c", "r"),
    ];
    assert!(build(&p, &ok, &[tool()], None).is_ok());
    p.tools = false;
    assert!(build(&p, &ok, &[tool()], None).is_err());
    // Arguments must be a JSON object.
    p.tools = true;
    calls.calls = vec![Call::new("c", "inspect_app", "[1]")];
    assert!(build(&p, &[Message::user("x"), calls], &[tool()], None).is_err());
    // Hermes needs Chat Completions and a valid session.
    let mut hermes = provider("Responses");
    hermes.backend = "Hermes".into();
    assert!(build(&hermes, &messages_without_image(), &[], None).is_err());
}

#[test]
fn hermes_requests_carry_session_and_idempotency_headers() {
    let mut hermes = provider("ChatCompletions");
    hermes.backend = "Hermes".into();
    hermes.hermes_session_id = new_id();
    let first = build_request(&hermes, Some("key"), &messages_without_image(), &[], false).unwrap();
    assert_eq!(
        first.header("x-hermes-session-id"),
        Some(format!("nabyscape-{}", hermes.hermes_session_id).as_str())
    );
    let second =
        build_request(&hermes, Some("key"), &messages_without_image(), &[], false).unwrap();
    assert!(is_id(first.header("Idempotency-Key").unwrap()));
    assert_ne!(
        first.header("idempotency-key"),
        second.header("idempotency-key")
    );
    hermes.hermes_session_id = "not-a-session".into();
    assert!(build_request(&hermes, None, &messages_without_image(), &[], false).is_err());
    hermes.hermes_session_id = String::new();
    let none = build_request(&hermes, None, &messages_without_image(), &[], false).unwrap();
    assert!(none.header("x-hermes-session-id").is_none() && none.header("authorization").is_none());
}

#[test]
fn image_output_adds_each_protocols_modality_switch() {
    let mut p = provider("Responses");
    p.image_output = true;
    let body = build_request(&p, None, &messages_without_image(), &[], false)
        .unwrap()
        .json()
        .unwrap();
    assert_eq!(body["tools"], json!([{"type": "image_generation"}]));
    let body = build_request(&p, None, &messages_without_image(), &[tool()], false)
        .unwrap()
        .json()
        .unwrap();
    assert_eq!(body["tools"].as_array().unwrap().len(), 2);
    p.protocol = "Gemini".into();
    let body = build_request(&p, None, &messages_without_image(), &[], false)
        .unwrap()
        .json()
        .unwrap();
    assert_eq!(
        body["generationConfig"]["responseModalities"],
        json!(["TEXT", "IMAGE"])
    );
    assert_eq!(body["generationConfig"]["maxOutputTokens"], 4096);
    p.protocol = "ChatCompletions".into();
    let body = build_request(&p, None, &messages_without_image(), &[], false)
        .unwrap()
        .json()
        .unwrap();
    assert_eq!(body["modalities"], json!(["text", "image"]));
    p.protocol = "Anthropic".into();
    let body = build_request(&p, None, &messages_without_image(), &[], false)
        .unwrap()
        .json()
        .unwrap();
    assert!(body.get("modalities").is_none() && body.get("tools").is_none());
    p.protocol = "ChatCompletions".into();
    p.backend = "Hermes".into();
    let body = build_request(&p, None, &messages_without_image(), &[], false)
        .unwrap()
        .json()
        .unwrap();
    assert!(body.get("modalities").is_none());
}

#[test]
fn attachments_are_encoded_lazily_into_the_request() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("report.pdf");
    std::fs::write(&file, b"%PDF-1.4 fixture").unwrap();
    let attachment = Attachment::from_file(&file).unwrap();
    let mut message = Message::user("Summarize");
    message.attachments = vec![attachment.clone()];
    assert!(matches!(
        attachment.source,
        crate::assistant::AttachmentSource::Path { .. }
    ));
    let expected = crate::assistant::attachments::data_url("application/pdf", b"%PDF-1.4 fixture");
    let request = build("Responses", &[message.clone()], &[]);
    assert_eq!(
        request.json().unwrap()["input"][0]["content"][1]["file_data"],
        expected
    );
    let request = build("Anthropic", &[message.clone()], &[]);
    assert_eq!(
        request.json().unwrap()["messages"][0]["content"][1]["source"]["data"],
        expected.split_once(',').unwrap().1
    );
    let request = build("ChatCompletions", &[message.clone()], &[]);
    assert_eq!(
        request.json().unwrap()["messages"][0]["content"][1]["file"]["filename"],
        "report.pdf"
    );
    // Validation never reads the file: a vanished file only fails at build time.
    std::fs::remove_file(&file).unwrap();
    assert!(validate_input(&provider("Responses"), &[message.clone()], &[]).is_ok());
    assert!(build_request(&provider("Responses"), None, &[message], &[], false).is_err());
    // Attachments only travel on user messages.
    let mut assistant = Message::assistant("x");
    assistant.attachments = vec![Attachment::from_text("n.txt", "secret".into(), "").unwrap()];
    let text = wire(&build("Responses", &[Message::user("hi"), assistant], &[]));
    assert!(!text.contains("secret"));
    // Extracted document text is wrapped with its name and note.
    let mut doc = Message::user("Read");
    doc.attachments = vec![Attachment::from_text("notes.md", "line".into(), "Document").unwrap()];
    let body = build("ChatCompletions", &[doc], &[]).json().unwrap();
    assert_eq!(
        body["messages"][0]["content"][1]["text"],
        "Attached document: notes.md\nDocument\n<attachment>\nline\n</attachment>"
    );
}

#[test]
fn attachment_limits_are_enforced_before_sending() {
    let p = provider("ChatCompletions");
    let doc = |n: usize| Attachment::from_text("a.txt", "x".repeat(n), "").unwrap();
    let with = |files: Vec<Attachment>| {
        let mut m = Message::user("hi");
        m.attachments = files;
        m
    };
    assert!(validate_input(&p, &[with(vec![doc(60_000), doc(40_000)])], &[]).is_ok());
    assert!(validate_input(&p, &[with(vec![doc(60_000), doc(40_001)])], &[]).is_err());
    assert!(validate_input(&p, &[with(vec![doc(10); 7])], &[]).is_err());
    let mut named = doc(1);
    named.name = "n".repeat(181);
    assert!(validate_input(&p, &[with(vec![named])], &[]).is_err());
    let mut control = doc(1);
    control.name = "bad\nname".into();
    assert!(validate_input(&p, &[with(vec![control])], &[]).is_err());
    // Images need image input; audio needs Gemini.
    let image = Attachment::inline("a.jpg", "image/jpeg", IMAGE.into(), "");
    let mut no_vision = p.clone();
    no_vision.vision = false;
    assert!(validate_input(&no_vision, &[with(vec![image.clone()])], &[]).is_err());
    assert!(validate_input(&p, &[with(vec![image])], &[]).is_ok());
    let audio = Attachment::inline(
        "a.mp3",
        "audio/mpeg",
        "data:audio/mpeg;base64,AQID".into(),
        "",
    );
    assert!(validate_input(&p, &[with(vec![audio.clone()])], &[]).is_err());
    assert!(validate_input(&provider("Gemini"), &[with(vec![audio])], &[]).is_ok());
}

#[test]
fn failure_messages_hint_at_the_cause_and_never_leak_keys() {
    let body = |v: Value| v.to_string().into_bytes();
    assert_eq!(
        failure_message(401, None, None),
        "HTTP 401: API key missing, invalid or expired."
    );
    assert_eq!(
        failure_message(403, None, None),
        "HTTP 403: Your key does not have permission for this model."
    );
    assert_eq!(
        failure_message(404, None, None),
        "HTTP 404: Check the API base URL and model ID."
    );
    assert_eq!(
        failure_message(429, None, None),
        "HTTP 429: Rate limit or quota reached. Check your provider balance."
    );
    assert_eq!(
        failure_message(503, None, None),
        "HTTP 503: The provider is temporarily unavailable."
    );
    assert_eq!(
        failure_message(302, None, None),
        "HTTP 302: Redirects are blocked; enter the final provider URL."
    );
    assert_eq!(
        failure_message(400, None, None),
        "HTTP 400: Check the protocol, model and request support."
    );
    assert_eq!(
        failure_message(
            400,
            Some(&body(json!({"error": {"message": "bad model"}}))),
            None
        ),
        "HTTP 400: Check the protocol, model and request support. Provider: bad model"
    );
    assert!(
        failure_message(400, Some(&body(json!({"error": "plain"}))), None)
            .ends_with("Provider: plain")
    );
    assert!(
        failure_message(400, Some(&body(json!({"detail": "other"}))), None)
            .ends_with("Provider: other")
    );
    let leaked = failure_message(
        401,
        Some(&body(
            json!({"error": {"message": "key fixture-secret-123 and sk-abcdefgh12345 and Bearer abc.def-ghijkl"}}),
        )),
        Some("fixture-secret-123"),
    );
    assert!(
        !leaked.contains("fixture-secret-123")
            && !leaked.contains("sk-abcdefgh")
            && !leaked.contains("abc.def"),
        "{leaked}"
    );
    assert_eq!(leaked.matches("[key hidden]").count(), 3);
    // Short key-like tokens are left alone; control characters are dropped; text is bounded.
    assert!(
        failure_message(400, Some(&body(json!({"error": "sk-short"}))), None).ends_with("sk-short")
    );
    let long = failure_message(
        400,
        Some(&body(
            json!({"error": format!("a\u{7}{}", "b".repeat(1000))}),
        )),
        None,
    );
    assert!(!long.contains('\u{7}') && long.chars().filter(|c| *c == 'b').count() == 359);
    // Unparseable or oversized bodies add nothing.
    assert!(failure_message(500, Some(b"<html>"), None).ends_with("temporarily unavailable."));
    assert!(
        failure_message(500, Some(&vec![b' '; 40_000]), None).ends_with("temporarily unavailable.")
    );
}

#[test]
fn model_listing_requests_and_pages() {
    let list = |protocol: &str, cursor: &str| {
        models_request(&provider(protocol), Some("k"), cursor).unwrap()
    };
    let chat = list("ChatCompletions", "");
    assert_eq!(
        (chat.method, chat.url.as_str()),
        (Method::Get, "https://provider.example/v1/models")
    );
    assert_eq!(chat.header("authorization"), Some("Bearer k"));
    assert!(chat.body.is_empty());
    assert_eq!(
        list("Responses", "").url,
        "https://provider.example/v1/models"
    );
    assert_eq!(
        list("Gemini", "").url,
        "https://provider.example/v1/models?pageSize=1000"
    );
    assert_eq!(
        list("Gemini", "a b").url,
        "https://provider.example/v1/models?pageSize=1000&pageToken=a%20b"
    );
    assert_eq!(list("Gemini", "").header("x-goog-api-key"), Some("k"));
    assert_eq!(
        list("Anthropic", "").url,
        "https://provider.example/v1/models?limit=100"
    );
    assert_eq!(
        list("Anthropic", "m/1").url,
        "https://provider.example/v1/models?limit=100&after_id=m%2F1"
    );
    assert_eq!(
        list("Anthropic", "").header("anthropic-version"),
        Some("2023-06-01")
    );
    assert!(models_request(&provider("Responses"), Some("a\nb"), "").is_err());

    let page = |protocol: &str, body: Value| {
        parse_models_page(&provider(protocol), body.to_string().as_bytes())
    };
    let (ids, next) = page("ChatCompletions", json!({"data": [{"id": "b"}, {"id": "a"}, {"id": ""}, {"nope": 1}, {"id": "bad\nid"}, {"id": "x".repeat(161)}]})).unwrap();
    assert_eq!(
        (ids, next.as_str()),
        (vec!["b".to_string(), "a".to_string()], "")
    );
    let (ids, next) = page("Gemini", json!({"models": [{"name": "models/gemini-pro"}, {"name": "tuned"}], "nextPageToken": "t2"})).unwrap();
    assert_eq!(
        (ids, next.as_str()),
        (vec!["gemini-pro".to_string(), "tuned".to_string()], "t2")
    );
    let (_, next) = page(
        "Anthropic",
        json!({"data": [{"id": "claude"}], "has_more": true, "last_id": "claude"}),
    )
    .unwrap();
    assert_eq!(next, "claude");
    let (_, next) = page(
        "Anthropic",
        json!({"data": [], "has_more": false, "last_id": "claude"}),
    )
    .unwrap();
    assert_eq!(next, "");
    for bad in [json!({}), json!({"data": 3}), json!([])] {
        assert!(page("ChatCompletions", bad).is_err());
    }
    assert!(parse_models_page(&provider("Gemini"), b"{\"data\": []}").is_err());
    assert!(parse_models_page(&provider("Gemini"), b"nope").is_err());
    assert_eq!(
        sort_models(vec![
            "b".into(),
            "A".into(),
            "a".into(),
            "b".into(),
            "C".into()
        ]),
        ["A", "a", "b", "C"]
    );
}

#[test]
fn role_enum_names_match_the_wire() {
    assert_eq!(Role::System.as_str(), "system");
    assert_eq!(Role::Tool.as_str(), "tool");
    assert_eq!(escape_data_string("a b/c~d-e_f.g"), "a%20b%2Fc~d-e_f.g");
}
