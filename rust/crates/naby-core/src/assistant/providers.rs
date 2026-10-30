//! Provider catalog, endpoint rules, per-protocol request building and reply parsing
//! (`AssistantProviderCatalog.cs`, `AssistantTransport.cs`, `AssistantProviderApi.cs`).
//!
//! Nothing here performs I/O: [`build_request`] returns an [`HttpRequest`] and
//! [`parse_response`] reads an already-fetched JSON value.

use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use serde_json::{json, Map, Value};
use url::{Host, Url};

use super::attachments::{image_parts, Attachment};
use super::media::{
    attachment_parts, check_attachments, extract_images, part_image, read_media, Bad, BadReply,
};
use super::models::{Call, Message, Protocol, Provider, Reply, Role, Tool};
use super::transport::{HttpRequest, Method};
use super::{is_id, len16, new_id, AssistantError, Result};

/// A request body may not exceed this many bytes, nor a response.
pub const MAXIMUM_BODY_BYTES: usize = 32 * 1024 * 1024;
/// Conversation text limit (128 K characters).
pub const MAXIMUM_TEXT_CHARACTERS: usize = 128 * 1024;
const MAXIMUM_CALLS: usize = 16;

/// Provider presets offered by "Add provider".
pub const PRESETS: [&str; 11] = [
    "OpenAI",
    "Anthropic",
    "Google Gemini",
    "Ollama",
    "LM Studio",
    "xAI",
    "Hermes Agent",
    "Custom Chat Completions",
    "Custom Responses",
    "Custom Anthropic",
    "Custom Gemini",
];

/// Create a provider from a preset. Model and modality support are explicit user choices,
/// never inferred from a brand.
pub fn create(name: &str) -> Result<Provider> {
    let (protocol, url) = match name {
        "OpenAI" => ("Responses", "https://api.openai.com/v1/"),
        "Anthropic" => ("Anthropic", "https://api.anthropic.com/v1/"),
        "Google Gemini" => (
            "Gemini",
            "https://generativelanguage.googleapis.com/v1beta/",
        ),
        "Ollama" => ("ChatCompletions", "http://localhost:11434/v1/"),
        "LM Studio" => ("ChatCompletions", "http://localhost:1234/v1/"),
        "xAI" => ("ChatCompletions", "https://api.x.ai/v1/"),
        "Hermes Agent" => ("ChatCompletions", "http://127.0.0.1:8642/v1/"),
        "Custom Chat Completions" => ("ChatCompletions", ""),
        "Custom Responses" => ("Responses", ""),
        "Custom Anthropic" => ("Anthropic", ""),
        "Custom Gemini" => ("Gemini", ""),
        _ => return Err(AssistantError::invalid("Unknown provider preset.")),
    };
    let hermes = name == "Hermes Agent";
    Ok(Provider {
        name: name.into(),
        protocol: protocol.into(),
        base_url: url.into(),
        model: if hermes {
            "hermes-agent".into()
        } else {
            String::new()
        },
        backend: if hermes {
            "Hermes".into()
        } else {
            String::new()
        },
        vision: false,
        tools: false,
        ..Provider::default()
    })
}

// ----- endpoint rules --------------------------------------------------------------------

pub fn is_local_host(host: &str) -> bool {
    let host = host.trim_matches(['[', ']']);
    host.eq_ignore_ascii_case("localhost")
        || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// Whether a resolved address is a public unicast address (`IsPublicAddress`).
pub fn is_public_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(v4) => is_public_v4(v4),
        IpAddr::V6(v6) => {
            if let Some(mapped) = v6.to_ipv4_mapped() {
                return is_public_v4(mapped);
            }
            is_public_v6(v6)
        }
    }
}

fn is_public_v4(ip: Ipv4Addr) -> bool {
    let b = ip.octets();
    !matches!(b[0], 0 | 10 | 127)
        && b[0] < 224
        && !(b[0] == 100 && (64..=127).contains(&b[1]))
        && !(b[0] == 169 && b[1] == 254)
        && !(b[0] == 172 && (16..=31).contains(&b[1]))
        && !(b[0] == 192 && (b[1] == 168 || (b[1] == 0 && matches!(b[2], 0 | 2))))
        && !(b[0] == 198 && (matches!(b[1], 18 | 19) || (b[1] == 51 && b[2] == 100)))
        && !(b[0] == 203 && b[1] == 0 && b[2] == 113)
}

fn is_public_v6(ip: Ipv6Addr) -> bool {
    let b = ip.octets();
    (0x20..=0x3f).contains(&b[0])
        && !(b[0] == 0x20 && b[1] == 1 && b[2] == 0x0d && b[3] == 0xb8)
        && !(b[0] == 0x20 && b[1] == 1 && b[2] == 0 && b[3] == 0)
}

/// Accept only a public HTTPS base URL or a localhost HTTP URL, without credentials, query or
/// fragment, and return it with a trailing slash (`ValidateEndpoint`).
pub fn validate_endpoint(value: &str) -> Result<Url> {
    let invalid = || {
        AssistantError::invalid(
            "Use a public HTTPS provider base URL or localhost HTTP URL without credentials, query, or fragment.",
        )
    };
    if value.trim().is_empty() || len16(value) > 2048 {
        return Err(invalid());
    }
    let uri = Url::parse(value).map_err(|_| invalid())?;
    let host = uri.host().ok_or_else(invalid)?;
    let host_text = uri.host_str().unwrap_or("");
    let local = match &host {
        Host::Domain(d) => d.eq_ignore_ascii_case("localhost"),
        Host::Ipv4(a) => a.is_loopback(),
        Host::Ipv6(a) => a.is_loopback(),
    };
    if !uri.username().is_empty()
        || uri.password().is_some()
        || uri.fragment().is_some()
        || uri.query().is_some()
        || !(uri.scheme() == "https" || (uri.scheme() == "http" && local))
        || host_text.is_empty()
        || host_text.ends_with('.')
        || host_text.contains('%')
    {
        return Err(invalid());
    }
    if !local {
        let public = match &host {
            Host::Ipv4(a) => is_public_address(IpAddr::V4(*a)),
            Host::Ipv6(a) => is_public_address(IpAddr::V6(*a)),
            Host::Domain(d) => {
                let d = d.to_ascii_lowercase();
                d.contains('.')
                    && !d.ends_with(".local")
                    && !d.ends_with(".internal")
                    && !d.ends_with(".localhost")
            }
        };
        if !public {
            return Err(AssistantError::invalid(
                "The provider destination must be public or localhost.",
            ));
        }
    }
    let with_slash = format!("{}/", uri.as_str().trim_end_matches('/'));
    Url::parse(&with_slash).map_err(|_| invalid())
}

/// `Uri.EscapeDataString`: everything except RFC 3986 unreserved characters is %-encoded.
pub fn escape_data_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn validate_call(call: &Call) -> Result<()> {
    let bad = || AssistantError::invalid("The provider returned an invalid tool call.");
    let id_len = len16(&call.id);
    let args_len = len16(&call.arguments);
    if !(1..=256).contains(&id_len)
        || call.id.chars().any(char::is_control)
        || !valid_name(&call.name)
        || !(1..=32768).contains(&args_len)
        || call
            .thought_signature
            .as_ref()
            .is_some_and(|s| s.len() > MAXIMUM_BODY_BYTES)
    {
        return Err(bad());
    }
    match serde_json::from_str::<Value>(&call.arguments) {
        Ok(Value::Object(_)) => Ok(()),
        Ok(_) => Err(AssistantError::invalid(
            "Tool arguments must be a JSON object.",
        )),
        Err(_) => Err(AssistantError::invalid(
            "Tool arguments must be a JSON object.",
        )),
    }
}

/// Validate the key text that will go into a header.
pub fn validate_key(key: Option<&str>) -> Result<()> {
    if let Some(key) = key {
        if len16(key) > 4096 || key.chars().any(char::is_control) {
            return Err(AssistantError::invalid("The API key is invalid."));
        }
    }
    Ok(())
}

/// Credential headers for a provider (`Authenticate`).
pub fn authenticate(
    headers: &mut Vec<(String, String)>,
    provider: &Provider,
    key: Option<&str>,
) -> Result<()> {
    validate_key(key)?;
    if let Some(key) = key.filter(|k| !k.trim().is_empty()) {
        match provider.protocol_kind() {
            Some(Protocol::Anthropic) => headers.push(("x-api-key".into(), key.into())),
            Some(Protocol::Gemini) => headers.push(("x-goog-api-key".into(), key.into())),
            _ => headers.push(("Authorization".into(), format!("Bearer {key}"))),
        }
    }
    if provider.protocol_kind() == Some(Protocol::Anthropic) {
        headers.push(("anthropic-version".into(), "2023-06-01".into()));
    }
    Ok(())
}

// ----- input validation -------------------------------------------------------------------

/// Reject any conversation the provider adapters must not send (`ValidateInput`).
pub fn validate_input(provider: &Provider, messages: &[Message], tools: &[Tool]) -> Result<()> {
    if provider.model.trim().is_empty()
        || len16(&provider.model) > 256
        || provider.model.chars().any(char::is_control)
    {
        return Err(AssistantError::invalid(
            "Enter the model identifier supplied by your provider.",
        ));
    }
    if provider.protocol_kind().is_none() {
        return Err(AssistantError::invalid(
            "Select a supported assistant protocol.",
        ));
    }
    if messages.is_empty() || messages.len() > 100 || tools.len() > 32 {
        return Err(AssistantError::invalid(
            "The assistant conversation or tool list is too large.",
        ));
    }
    let mut text_length = 0usize;
    let mut call_count = 0usize;
    let mut payload: u64 = 0;
    let mut known: HashSet<&str> = HashSet::new();
    let mut pending: HashSet<&str> = HashSet::new();
    let mut names: HashSet<&str> = HashSet::new();
    for tool in tools {
        let raw = tool.parameters.to_string();
        if !valid_name(&tool.name)
            || !names.insert(&tool.name)
            || len16(&tool.description) > 4096
            || !tool.parameters.is_object()
            || raw.len() > 32768
        {
            return Err(AssistantError::invalid(
                "The assistant tool definition is invalid.",
            ));
        }
        payload += (len16(&tool.description) + raw.len()) as u64;
    }
    for message in messages {
        let message_text = len16(&message.text);
        text_length += message_text;
        payload += message_text as u64;
        if text_length > MAXIMUM_TEXT_CHARACTERS {
            return Err(AssistantError::invalid(
                "The conversation text is too large. Start a new chat.",
            ));
        }
        if let Some(image) = &message.image_data_url {
            if !provider.vision || message.role != Role::User {
                return Err(AssistantError::invalid(
                    "Enable vision only for a model that supports user images.",
                ));
            }
            image_parts(image)?;
            payload += len16(image) as u64;
        }
        if message.role == Role::User && !message.attachments.is_empty() {
            check_attachments(provider, &message.attachments)?;
            payload += message
                .attachments
                .iter()
                .map(Attachment::weight)
                .sum::<u64>();
            text_length += message
                .attachments
                .iter()
                .map(Attachment::text_len)
                .sum::<usize>();
            if text_length > MAXIMUM_TEXT_CHARACTERS {
                return Err(AssistantError::invalid(
                    "The attached text and conversation exceed 128,000 characters. Start a new chat or split the documents.",
                ));
            }
        }
        if !message.calls.is_empty() {
            if !provider.tools
                || message.role != Role::Assistant
                || message.calls.len() > MAXIMUM_CALLS
            {
                return Err(AssistantError::invalid(
                    "Tool calls are not enabled or valid for this message.",
                ));
            }
            for call in &message.calls {
                validate_call(call)?;
                payload += (len16(&call.arguments)
                    + call.thought_signature.as_ref().map_or(0, |s| len16(s)))
                    as u64;
                call_count += 1;
                if call_count > 128 || !known.insert(&call.id) {
                    return Err(AssistantError::invalid(
                        "Duplicate or excessive tool calls in conversation.",
                    ));
                }
                pending.insert(&call.id);
            }
        }
        if message.role == Role::Tool
            && !message
                .call_id
                .as_deref()
                .is_some_and(|id| pending.remove(id))
        {
            return Err(AssistantError::invalid(
                "A tool result must match an earlier assistant call exactly once.",
            ));
        }
        if !message.protocol_items.is_empty() {
            if message.role != Role::Assistant || message.protocol_items.len() > 128 {
                return Err(AssistantError::invalid(
                    "Invalid provider continuation items.",
                ));
            }
            for item in &message.protocol_items {
                if !item.is_object() {
                    return Err(AssistantError::invalid(
                        "Invalid provider continuation item.",
                    ));
                }
                payload += item.to_string().len() as u64;
            }
        }
        if payload > MAXIMUM_BODY_BYTES as u64 {
            return Err(AssistantError::invalid(
                "The assistant request is too large. Start a new chat or reduce the image size.",
            ));
        }
    }
    Ok(())
}

// ----- request building -------------------------------------------------------------------

fn text_part(kind: &str, text: &str) -> Value {
    json!({ "type": kind, "text": text })
}

fn gemini_local(id: &str) -> bool {
    id.starts_with("gemini_local_")
}

fn arguments_node(call: &Call) -> Value {
    serde_json::from_str(&call.arguments).unwrap_or(Value::Null)
}

fn user_attachment_parts(provider: &Provider, message: &Message) -> Result<Vec<Value>> {
    let mut parts = Vec::new();
    if message.role == Role::User {
        for file in &message.attachments {
            parts.extend(attachment_parts(provider, file, true)?);
        }
    }
    Ok(parts)
}

/// Build the JSON body for the conversation (`Build`).
pub fn build_body(
    provider: &Provider,
    messages: &[Message],
    tools: &[Tool],
    stream: bool,
) -> Result<Value> {
    let protocol = provider
        .protocol_kind()
        .ok_or_else(|| AssistantError::invalid("Select a supported assistant protocol."))?;
    let mut body = Map::new();
    let mut items: Vec<Value> = Vec::new();
    if protocol != Protocol::Gemini {
        body.insert("model".into(), json!(provider.model));
    }
    let calls: std::collections::HashMap<&str, &Call> = messages
        .iter()
        .flat_map(|m| m.calls.iter())
        .map(|c| (c.id.as_str(), c))
        .collect();
    let system = messages
        .iter()
        .filter(|m| m.role == Role::System)
        .map(|m| m.text.as_str())
        .collect::<Vec<_>>()
        .join("\n\n");
    for message in messages {
        let anthropic = protocol == Protocol::Anthropic;
        if matches!(protocol, Protocol::Anthropic | Protocol::Gemini)
            && message.role == Role::System
        {
            continue;
        }
        match protocol {
            Protocol::ChatCompletions => {
                let mut entry = Map::new();
                entry.insert("role".into(), json!(message.role.as_str()));
                entry.insert("content".into(), json!(message.text));
                let has_files = message.role == Role::User && !message.attachments.is_empty();
                if message.image_data_url.is_some() || has_files {
                    let mut content = vec![text_part("text", &message.text)];
                    if let Some(image) = &message.image_data_url {
                        content.push(part_image(provider, image)?);
                    }
                    content.extend(user_attachment_parts(provider, message)?);
                    entry.insert("content".into(), Value::Array(content));
                }
                if message.role == Role::Tool {
                    entry.insert("tool_call_id".into(), json!(message.call_id));
                }
                if !message.calls.is_empty() {
                    let list: Vec<Value> = message
                        .calls
                        .iter()
                        .map(|c| {
                            json!({
                                "id": c.id, "type": "function",
                                "function": { "name": c.name, "arguments": c.arguments }
                            })
                        })
                        .collect();
                    entry.insert("tool_calls".into(), Value::Array(list));
                }
                items.push(Value::Object(entry));
            }
            Protocol::Responses => {
                if message.role == Role::Tool {
                    items.push(json!({
                        "type": "function_call_output",
                        "call_id": message.call_id,
                        "output": message.text
                    }));
                    continue;
                }
                if !message.protocol_items.is_empty() {
                    items.extend(message.protocol_items.iter().cloned());
                    continue;
                }
                let has_files = message.role == Role::User && !message.attachments.is_empty();
                if !message.text.is_empty() || message.image_data_url.is_some() || has_files {
                    let kind = if message.role == Role::Assistant {
                        "output_text"
                    } else {
                        "input_text"
                    };
                    let mut content = vec![text_part(kind, &message.text)];
                    if let Some(image) = &message.image_data_url {
                        content.push(json!({ "type": "input_image", "image_url": image }));
                    }
                    content.extend(user_attachment_parts(provider, message)?);
                    items.push(json!({ "role": message.role.as_str(), "content": content }));
                }
                for call in &message.calls {
                    items.push(json!({
                        "type": "function_call", "call_id": call.id,
                        "name": call.name, "arguments": call.arguments
                    }));
                }
            }
            Protocol::Anthropic | Protocol::Gemini => {
                let mut parts: Vec<Value> = Vec::new();
                if !message.protocol_items.is_empty() {
                    parts.extend(message.protocol_items.iter().cloned());
                } else if message.role == Role::Tool {
                    let call = message
                        .call_id
                        .as_deref()
                        .and_then(|id| calls.get(id))
                        .ok_or_else(|| {
                            AssistantError::invalid(
                                "A tool result must match an earlier assistant call exactly once.",
                            )
                        })?;
                    if anthropic {
                        parts.push(json!({
                            "type": "tool_result", "tool_use_id": call.id, "content": message.text
                        }));
                    } else {
                        let mut result = json!({
                            "name": call.name, "response": { "result": message.text }
                        });
                        if !gemini_local(&call.id) {
                            result["id"] = json!(call.id);
                        }
                        parts.push(json!({ "functionResponse": result }));
                    }
                } else {
                    if !message.text.is_empty() {
                        parts.push(if anthropic {
                            text_part("text", &message.text)
                        } else {
                            json!({ "text": message.text })
                        });
                    }
                    if let Some(source) = &message.image_data_url {
                        let (mime, data) = image_parts(source)?;
                        parts.push(if anthropic {
                            json!({ "type": "image", "source": {
                                "type": "base64", "media_type": mime, "data": data } })
                        } else {
                            json!({ "inlineData": { "mimeType": mime, "data": data } })
                        });
                    }
                    for call in &message.calls {
                        if anthropic {
                            parts.push(json!({
                                "type": "tool_use", "id": call.id, "name": call.name,
                                "input": arguments_node(call)
                            }));
                        } else {
                            let mut function = json!({
                                "name": call.name, "args": arguments_node(call)
                            });
                            if !gemini_local(&call.id) {
                                function["id"] = json!(call.id);
                            }
                            let mut part = json!({ "functionCall": function });
                            if let Some(signature) = &call.thought_signature {
                                part["thoughtSignature"] = json!(signature);
                            }
                            parts.push(part);
                        }
                    }
                    parts.extend(user_attachment_parts(provider, message)?);
                }
                if parts.is_empty() {
                    parts.push(if anthropic {
                        text_part("text", " ")
                    } else {
                        json!({ "text": " " })
                    });
                }
                let role = if message.role == Role::Assistant {
                    if anthropic {
                        "assistant"
                    } else {
                        "model"
                    }
                } else {
                    "user"
                };
                let field = if anthropic { "content" } else { "parts" };
                // Parallel tool results must form a single user turn in both protocols.
                match items.last_mut() {
                    Some(previous) if previous["role"] == role => {
                        if let Some(Value::Array(existing)) = previous.get_mut(field) {
                            existing.extend(parts);
                        }
                    }
                    _ => items.push(json!({ "role": role, field: parts })),
                }
            }
        }
    }
    let mut definitions: Vec<Value> = Vec::new();
    for tool in tools {
        definitions.push(match protocol {
            Protocol::ChatCompletions => json!({
                "type": "function",
                "function": {
                    "name": tool.name, "description": tool.description,
                    "parameters": tool.parameters
                }
            }),
            Protocol::Responses => json!({
                "name": tool.name, "description": tool.description,
                "parameters": tool.parameters, "type": "function", "strict": false
            }),
            Protocol::Anthropic => json!({
                "name": tool.name, "description": tool.description,
                "input_schema": tool.parameters
            }),
            Protocol::Gemini => json!({
                "name": tool.name, "description": tool.description,
                "parametersJsonSchema": tool.parameters
            }),
        });
    }
    match protocol {
        Protocol::ChatCompletions => {
            body.insert("messages".into(), Value::Array(items));
            body.insert("stream".into(), json!(stream));
        }
        Protocol::Responses => {
            body.insert("input".into(), Value::Array(items));
            body.insert("store".into(), json!(false));
            body.insert("include".into(), json!(["reasoning.encrypted_content"]));
            body.insert("max_output_tokens".into(), json!(4096));
        }
        Protocol::Anthropic => {
            body.insert("messages".into(), Value::Array(items));
            body.insert("max_tokens".into(), json!(4096));
            if !system.is_empty() {
                body.insert("system".into(), json!(system));
            }
        }
        Protocol::Gemini => {
            body.insert("contents".into(), Value::Array(items));
            body.insert(
                "generationConfig".into(),
                json!({ "maxOutputTokens": 4096 }),
            );
            if !system.is_empty() {
                body.insert(
                    "systemInstruction".into(),
                    json!({ "parts": [{ "text": system }] }),
                );
            }
        }
    }
    if !definitions.is_empty() {
        body.insert(
            "tools".into(),
            if protocol == Protocol::Gemini {
                json!([{ "functionDeclarations": definitions }])
            } else {
                Value::Array(definitions)
            },
        );
    }
    if provider.image_output && !provider.is_hermes() {
        match protocol {
            Protocol::Responses => {
                let entry = body.entry("tools").or_insert_with(|| json!([]));
                if let Value::Array(list) = entry {
                    list.push(json!({ "type": "image_generation" }));
                }
            }
            Protocol::Gemini => {
                body["generationConfig"]["responseModalities"] = json!(["TEXT", "IMAGE"]);
            }
            Protocol::ChatCompletions => {
                body.insert("modalities".into(), json!(["text", "image"]));
            }
            Protocol::Anthropic => {}
        }
    }
    Ok(Value::Object(body))
}

/// Tools the provider is allowed to see: Hermes manages its own, others need the tool flag.
pub fn active_tools<'a>(provider: &Provider, tools: &'a [Tool]) -> &'a [Tool] {
    if provider.tools && !provider.is_hermes() {
        tools
    } else {
        &[]
    }
}

/// Chat route relative to the base URL.
pub fn chat_route(provider: &Provider) -> Result<String> {
    Ok(match provider.protocol_kind() {
        Some(Protocol::ChatCompletions) => "chat/completions".into(),
        Some(Protocol::Responses) => "responses".into(),
        Some(Protocol::Anthropic) => "messages".into(),
        Some(Protocol::Gemini) => format!(
            "models/{}:generateContent",
            escape_data_string(&provider.model)
        ),
        None => {
            return Err(AssistantError::invalid(
                "Select a supported assistant protocol.",
            ))
        }
    })
}

fn join(base: &Url, route: &str) -> Result<String> {
    let full = format!("{}{}", base.as_str(), route);
    Url::parse(&full)
        .map(|u| u.to_string())
        .map_err(|_| AssistantError::invalid("Use a public HTTPS provider base URL or localhost HTTP URL without credentials, query, or fragment."))
}

/// Build the HTTP request for one chat turn, with the same validation order as the C#
/// `Complete`/`CompleteStreaming` (endpoint, input, key, size).
///
/// `stream` is only meaningful for Chat Completions providers without image output.
pub fn build_request(
    provider: &Provider,
    api_key: Option<&str>,
    messages: &[Message],
    tools: &[Tool],
    stream: bool,
) -> Result<HttpRequest> {
    let endpoint = validate_endpoint(&provider.base_url)?;
    validate_input(provider, messages, tools)?;
    validate_key(api_key)?;
    let active = active_tools(provider, tools);
    let body = build_body(provider, messages, active, stream)?;
    let bytes = serde_json::to_vec(&body)
        .map_err(|_| AssistantError::invalid("The assistant request is invalid."))?;
    if bytes.len() > MAXIMUM_BODY_BYTES {
        return Err(AssistantError::invalid(
            "The assistant request is too large. Reduce the conversation or image size.",
        ));
    }
    let route = chat_route(provider)?;
    let mut headers = vec![
        ("Content-Type".to_string(), "application/json".to_string()),
        (
            "Accept".to_string(),
            if stream {
                "text/event-stream"
            } else {
                "application/json"
            }
            .to_string(),
        ),
    ];
    if provider.is_hermes() {
        if provider.protocol_kind() != Some(Protocol::ChatCompletions) {
            return Err(AssistantError::invalid(
                "Hermes Agent uses Chat Completions. Select that protocol.",
            ));
        }
        if !provider.hermes_session_id.is_empty() {
            if !is_id(&provider.hermes_session_id) {
                return Err(AssistantError::invalid(
                    "The Hermes session is invalid. Clear chat and try again.",
                ));
            }
            headers.push((
                "X-Hermes-Session-Id".into(),
                format!("nabyscape-{}", provider.hermes_session_id),
            ));
        }
        headers.push(("Idempotency-Key".into(), new_id()));
    }
    authenticate(&mut headers, provider, api_key)?;
    Ok(HttpRequest {
        method: Method::Post,
        url: join(&endpoint, &route)?,
        headers,
        body: bytes,
    })
}

// ----- reply parsing ----------------------------------------------------------------------

fn get<'a>(value: &'a Value, name: &str) -> &'a str {
    value.get(name).and_then(Value::as_str).unwrap_or("")
}

fn read_array<'a>(value: &'a Value, name: &str) -> Bad<&'a Vec<Value>> {
    value.get(name).and_then(Value::as_array).ok_or(BadReply)
}

struct Collector<'a> {
    provider: &'a Provider,
    tools: &'a [Tool],
    text: String,
    text_len: usize,
    calls: Vec<Call>,
    files: Vec<Attachment>,
}

impl Collector<'_> {
    fn add_text(&mut self, value: &str) -> Bad<()> {
        let value = extract_images(value, self.provider, &mut self.files)?;
        let n = len16(&value);
        if self.text_len + n > MAXIMUM_TEXT_CHARACTERS {
            return Err(BadReply);
        }
        self.text_len += n;
        self.text.push_str(&value);
        Ok(())
    }

    fn add_call(
        &mut self,
        id: &str,
        name: &str,
        arguments: &str,
        signature: Option<String>,
    ) -> Bad<()> {
        let call = Call {
            id: id.into(),
            name: name.into(),
            arguments: arguments.into(),
            thought_signature: signature,
        };
        validate_call(&call).map_err(|_| BadReply)?;
        if self.calls.len() >= MAXIMUM_CALLS
            || self.calls.iter().any(|c| c.id == id)
            || !self.tools.iter().any(|t| t.name == name)
        {
            return Err(BadReply);
        }
        self.calls.push(call);
        Ok(())
    }

    fn media(&mut self, part: &Value) -> Bad<bool> {
        read_media(part, self.provider, &mut self.files)
    }
}

fn parse_inner(provider: &Provider, root: &Value, tools: &[Tool]) -> Bad<Reply> {
    let protocol = provider.protocol_kind().ok_or(BadReply)?;
    if !root.is_object() || root.get("error").is_some() {
        return Err(BadReply);
    }
    let mut c = Collector {
        provider,
        tools,
        text: String::new(),
        text_len: 0,
        calls: Vec::new(),
        files: Vec::new(),
    };
    let mut continuation: Vec<Value> = Vec::new();
    match protocol {
        Protocol::ChatCompletions => {
            let choices = read_array(root, "choices")?;
            let choice = choices.first().ok_or(BadReply)?;
            if matches!(
                get(choice, "finish_reason"),
                "length" | "content_filter" | "error" | "cancelled"
            ) {
                return Err(BadReply);
            }
            let message = choice.get("message").ok_or(BadReply)?;
            if let Some(content) = message.get("content") {
                if let Some(text) = content.as_str() {
                    c.add_text(text)?;
                } else if let Some(parts) = content.as_array() {
                    for part in parts {
                        if get(part, "type") == "text" {
                            c.add_text(get(part, "text"))?;
                        } else {
                            c.media(part)?;
                        }
                    }
                }
            }
            if let Some(images) = message.get("images").and_then(Value::as_array) {
                for part in images {
                    c.media(part)?;
                }
            }
            c.add_text(get(message, "refusal"))?;
            if let Some(calls) = message.get("tool_calls").filter(|v| !v.is_null()) {
                for call in calls.as_array().ok_or(BadReply)? {
                    if get(call, "type") != "function" {
                        return Err(BadReply);
                    }
                    let function = call.get("function").ok_or(BadReply)?;
                    c.add_call(
                        get(call, "id"),
                        get(function, "name"),
                        get(function, "arguments"),
                        None,
                    )?;
                }
            }
        }
        Protocol::Responses => {
            if matches!(
                get(root, "status"),
                "failed" | "incomplete" | "cancelled" | "queued" | "in_progress"
            ) {
                return Err(BadReply);
            }
            for item in read_array(root, "output")? {
                let kind = get(item, "type");
                if matches!(
                    kind,
                    "message" | "reasoning" | "function_call" | "image_generation_call"
                ) {
                    continuation.push(item.clone());
                }
                if kind == "message" {
                    for part in read_array(item, "content")? {
                        match get(part, "type") {
                            "output_text" => c.add_text(get(part, "text"))?,
                            "refusal" => c.add_text(get(part, "refusal"))?,
                            _ => {
                                c.media(part)?;
                            }
                        }
                    }
                } else if kind == "function_call" {
                    c.add_call(
                        get(item, "call_id"),
                        get(item, "name"),
                        get(item, "arguments"),
                        None,
                    )?;
                } else if kind != "reasoning" && !c.media(item)? {
                    return Err(BadReply);
                }
            }
        }
        Protocol::Anthropic => {
            if matches!(get(root, "stop_reason"), "max_tokens" | "pause_turn") {
                return Err(BadReply);
            }
            for item in read_array(root, "content")? {
                let kind = get(item, "type");
                if !matches!(kind, "text" | "tool_use" | "thinking" | "redacted_thinking")
                    && !c.media(item)?
                {
                    return Err(BadReply);
                }
                continuation.push(item.clone());
                if kind == "text" {
                    c.add_text(get(item, "text"))?;
                } else if kind == "tool_use" {
                    let input = item.get("input").ok_or(BadReply)?.to_string();
                    c.add_call(get(item, "id"), get(item, "name"), &input, None)?;
                }
            }
        }
        Protocol::Gemini => {
            let candidates = read_array(root, "candidates")?;
            let candidate = candidates.first().ok_or(BadReply)?;
            let finish = get(candidate, "finishReason");
            if !finish.is_empty() && finish != "STOP" {
                return Err(BadReply);
            }
            let content = candidate.get("content").ok_or(BadReply)?;
            for part in read_array(content, "parts")? {
                continuation.push(part.clone());
                if part.get("thought") != Some(&Value::Bool(true)) {
                    c.add_text(get(part, "text"))?;
                }
                if let Some(function) = part.get("functionCall") {
                    let mut id = get(function, "id").to_string();
                    if id.is_empty() {
                        id = format!("gemini_local_{}", new_id());
                    }
                    let args = function
                        .get("args")
                        .map_or_else(|| "{}".to_string(), Value::to_string);
                    let signature = Some(get(part, "thoughtSignature"))
                        .filter(|s| !s.is_empty())
                        .map(str::to_string);
                    c.add_call(&id, get(function, "name"), &args, signature)?;
                } else if part.get("text").is_none() && !c.media(part)? {
                    return Err(BadReply);
                }
            }
        }
    }
    if (c.text.is_empty() && c.calls.is_empty() && c.files.is_empty()) || continuation.len() > 128 {
        return Err(BadReply);
    }
    Ok(Reply {
        text: c.text,
        calls: c.calls,
        protocol_items: continuation,
        attachments: c.files,
    })
}

/// Parse a provider reply (`Parse`). Any malformed, partial or unsupported reply becomes the
/// fixed safe error; provider text and secrets are never echoed.
pub fn parse_response(provider: &Provider, root: &Value, tools: &[Tool]) -> Result<Reply> {
    parse_inner(provider, root, tools).map_err(|_| invalid_response())
}

pub(crate) fn invalid_response() -> AssistantError {
    AssistantError::io("The provider returned an invalid or unsupported response.")
}

// ----- model listing and failures ---------------------------------------------------------

/// Request for one page of the model list (`Models`).
pub fn models_request(provider: &Provider, key: Option<&str>, cursor: &str) -> Result<HttpRequest> {
    let endpoint = validate_endpoint(&provider.base_url)?;
    let query = match provider.protocol_kind() {
        Some(Protocol::Gemini) => {
            if cursor.is_empty() {
                "?pageSize=1000".to_string()
            } else {
                format!("?pageSize=1000&pageToken={}", escape_data_string(cursor))
            }
        }
        Some(Protocol::Anthropic) => {
            if cursor.is_empty() {
                "?limit=100".to_string()
            } else {
                format!("?limit=100&after_id={}", escape_data_string(cursor))
            }
        }
        _ => String::new(),
    };
    let mut headers = Vec::new();
    authenticate(&mut headers, provider, key)?;
    headers.push(("Accept".into(), "application/json".into()));
    Ok(HttpRequest {
        method: Method::Get,
        url: join(&endpoint, &format!("models{query}"))?,
        headers,
        body: Vec::new(),
    })
}

/// Model IDs found on one page plus the continuation cursor (empty when finished).
pub fn parse_models_page(provider: &Provider, body: &[u8]) -> Result<(Vec<String>, String)> {
    let unsupported = || {
        AssistantError::io(
            "This provider does not expose a compatible model list. Paste the model ID manually.",
        )
    };
    let root: Value = serde_json::from_slice(body).map_err(|_| unsupported())?;
    let gemini = provider.protocol_kind() == Some(Protocol::Gemini);
    let items = root
        .get(if gemini { "models" } else { "data" })
        .and_then(Value::as_array)
        .ok_or_else(unsupported)?;
    let mut ids = Vec::new();
    for item in items {
        let Some(id) = item
            .get(if gemini { "name" } else { "id" })
            .and_then(Value::as_str)
        else {
            continue;
        };
        let value = if gemini {
            id.strip_prefix("models/").unwrap_or(id)
        } else {
            id
        };
        let n = len16(value);
        if (1..=160).contains(&n) && !value.chars().any(char::is_control) {
            ids.push(value.to_string());
        }
    }
    let next = if gemini {
        get(&root, "nextPageToken").to_string()
    } else if provider.protocol_kind() == Some(Protocol::Anthropic)
        && root.get("has_more") == Some(&Value::Bool(true))
    {
        get(&root, "last_id").to_string()
    } else {
        String::new()
    };
    Ok((ids, next))
}

/// Deduplicate (ordinal) and sort model IDs case-insensitively, as the picker lists them.
pub fn sort_models(ids: Vec<String>) -> Vec<String> {
    let mut unique: Vec<String> = ids
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    unique.sort_by_cached_key(|id| (id.to_uppercase(), id.clone()));
    unique
}

fn hide_keys(detail: &str) -> String {
    // (?i)(sk-|bearer\s+)[a-z0-9_\-\.]{8,}
    let lower = detail.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut out = String::with_capacity(detail.len());
    let mut i = 0;
    let token = |b: u8| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.');
    while i < bytes.len() {
        let mut start = None;
        if bytes[i..].starts_with(b"sk-") {
            start = Some(i + 3);
        } else if bytes[i..].starts_with(b"bearer") {
            let mut j = i + 6;
            while j < bytes.len() && (bytes[j] as char).is_ascii_whitespace() {
                j += 1;
            }
            if j > i + 6 {
                start = Some(j);
            }
        }
        if let Some(s) = start {
            let mut e = s;
            while e < bytes.len() && token(bytes[e]) {
                e += 1;
            }
            if e - s >= 8 {
                out.push_str("[key hidden]");
                i = e;
                continue;
            }
        }
        // Copy the original character (detail is valid UTF-8; advance by char boundary).
        let ch_len = detail[i..].chars().next().map_or(1, char::len_utf8);
        out.push_str(&detail[i..i + ch_len]);
        i += ch_len;
    }
    out
}

/// User-facing message for a non-success HTTP status (`Failure`). The provider's own error
/// text is included when short, with the API key and key-shaped tokens removed.
pub fn failure_message(status: u16, body: Option<&[u8]>, key: Option<&str>) -> String {
    let hint = match status {
        401 => "API key missing, invalid or expired.",
        403 => "Your key does not have permission for this model.",
        404 => "Check the API base URL and model ID.",
        429 => "Rate limit or quota reached. Check your provider balance.",
        500.. => "The provider is temporarily unavailable.",
        300..=399 => "Redirects are blocked; enter the final provider URL.",
        _ => "Check the protocol, model and request support.",
    };
    let mut detail = String::new();
    if let Some(Ok(root)) = body
        .filter(|b| b.len() <= 32768)
        .map(serde_json::from_slice::<Value>)
    {
        if let Some(error) = root.get("error") {
            detail = match error {
                Value::String(s) => s.clone(),
                other => other
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            };
        } else if let Some(Value::String(s)) = root.get("detail") {
            detail = s.clone();
        }
    }
    if let Some(key) = key.filter(|k| !k.is_empty()) {
        detail = detail.replace(key, "[key hidden]");
    }
    detail = hide_keys(&detail);
    let detail: String = detail
        .chars()
        .filter(|c| !c.is_control() || *c == ' ')
        .take(360)
        .collect();
    let mut message = format!("HTTP {status}: {hint}");
    if !detail.is_empty() {
        message.push_str(" Provider: ");
        message.push_str(&detail);
    }
    message
}

#[cfg(test)]
mod tests;
