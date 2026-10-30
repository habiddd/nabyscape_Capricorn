//! Server-sent events and the Chat Completions stream (`AssistantStreaming.cs`).

use serde_json::Value;

use super::models::Protocol;
use super::{AssistantError, Result};

const MAX_TOTAL_BYTES: usize = 4 * 1024 * 1024;
const MAX_LINE_BYTES: usize = 128 * 1024;
const MAX_EVENT_BYTES: usize = 256 * 1024;
const MAX_FRAMES: usize = 32768;
const MAX_EVENT_NAME: usize = 128;

/// One complete SSE frame (terminated by a blank line).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StreamEvent {
    /// The `event:` name, empty for the default `message`.
    pub event: String,
    /// The joined `data:` lines.
    pub data: String,
}

/// Incremental, bounded SSE parser. Feed it raw response bytes in any chunking; a frame is only
/// produced after its blank-line boundary, so truncated JSON from a dropped connection is never
/// promoted into a completed answer.
#[derive(Debug, Default)]
pub struct SseParser {
    line: Vec<u8>,
    data: String,
    kind: String,
    has_data: bool,
    total: usize,
    frames: usize,
    error: Option<&'static str>,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// The limit that stopped the stream, if any. After an error further input is ignored.
    pub fn error(&self) -> Option<AssistantError> {
        self.error.map(AssistantError::io)
    }

    fn fail(&mut self, message: &'static str) {
        self.error.get_or_insert(message);
    }

    pub fn feed(&mut self, chunk: &[u8]) -> Vec<StreamEvent> {
        let mut events = Vec::new();
        if self.error.is_some() {
            return events;
        }
        self.total += chunk.len();
        if self.total > MAX_TOTAL_BYTES {
            self.fail("The voice stream exceeded its size limit.");
            return events;
        }
        for &byte in chunk {
            if byte != b'\n' {
                self.line.push(byte);
                if self.line.len() > MAX_LINE_BYTES {
                    self.fail("The voice stream line exceeded its limit.");
                    return events;
                }
                continue;
            }
            let line = std::mem::take(&mut self.line);
            let Ok(text) = String::from_utf8(line) else {
                self.fail("The voice stream contained invalid text.");
                return events;
            };
            let value = text.trim_end_matches('\r');
            if value.is_empty() {
                if self.has_data {
                    self.frames += 1;
                    if self.frames > MAX_FRAMES {
                        self.fail("Too many voice stream events.");
                        return events;
                    }
                    events.push(StreamEvent {
                        event: std::mem::take(&mut self.kind),
                        data: std::mem::take(&mut self.data),
                    });
                    self.has_data = false;
                }
                self.kind.clear();
            } else if let Some(rest) = value.strip_prefix("data:") {
                if self.has_data {
                    self.data.push('\n');
                }
                self.has_data = true;
                self.data.push_str(rest.trim_start_matches(' '));
                if self.data.len() > MAX_EVENT_BYTES {
                    self.fail("The voice stream event exceeded its limit.");
                    return events;
                }
            } else if let Some(rest) = value.strip_prefix("event:") {
                self.kind = rest.trim().to_string();
                if self.kind.len() > MAX_EVENT_NAME {
                    self.fail("Invalid voice stream event.");
                    return events;
                }
            }
        }
        events
    }
}

/// A tool-call fragment of a streamed Chat Completions delta.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolDelta {
    pub index: usize,
    pub id: String,
    pub name: String,
    pub arguments: String,
}

/// The interesting content of one streamed chunk.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Delta {
    /// `finish_reason`, when this chunk carries one (always `stop` or `tool_calls`).
    pub finish: Option<String>,
    pub tool_calls: Vec<ToolDelta>,
    /// Spoken/visible text (never `reasoning_content` or progress frames).
    pub text: String,
}

/// What a streamed event means for the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamStep {
    /// Not a chunk this client reads (other event names, keep-alives, empty `choices`).
    Ignore,
    /// `[DONE]`.
    Done,
    /// Hermes `hermes.tool.progress`.
    Progress,
    Delta(Delta),
}

const STOPPED: &str = "The provider stopped the streamed reply. Check its session before retrying.";

fn get<'a>(value: &'a Value, name: &str) -> &'a str {
    value.get(name).and_then(Value::as_str).unwrap_or("")
}

/// Interpret one SSE event of a streamed reply. Only Chat Completions providers stream.
pub fn parse_stream_event(protocol: Protocol, event: &StreamEvent) -> Result<StreamStep> {
    if protocol != Protocol::ChatCompletions {
        return Err(AssistantError::invalid(
            "Streaming is only available for Chat Completions providers.",
        ));
    }
    if event.data == "[DONE]" {
        return Ok(StreamStep::Done);
    }
    if event.event == "hermes.tool.progress" {
        return Ok(StreamStep::Progress);
    }
    if event.event == "error" {
        return Err(AssistantError::io(STOPPED));
    }
    if !event.event.is_empty() && event.event != "message" {
        return Ok(StreamStep::Ignore);
    }
    let invalid_stream = || {
        AssistantError::io(
            "The provider returned an invalid voice stream. Disable streaming in Voice settings for an incompatible endpoint.",
        )
    };
    let root: Value = serde_json::from_str(&event.data).map_err(|_| invalid_stream())?;
    if root.get("error").is_some() || root.get("failed") == Some(&Value::Bool(true)) {
        return Err(AssistantError::io(STOPPED));
    }
    let Some(choices) = root.get("choices").and_then(Value::as_array) else {
        return Err(AssistantError::io("Invalid voice stream."));
    };
    let Some(choice) = choices.first() else {
        return Ok(StreamStep::Ignore);
    };
    let mut out = Delta::default();
    let reason = get(choice, "finish_reason");
    if !reason.is_empty() {
        if reason != "stop" && reason != "tool_calls" {
            return Err(AssistantError::io(
                "The streamed reply was interrupted or incomplete. Check the provider session before retrying.",
            ));
        }
        out.finish = Some(reason.to_string());
    }
    let Some(delta) = choice.get("delta") else {
        return Ok(StreamStep::Delta(out));
    };
    if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
        for call in calls {
            let index = call
                .get("index")
                .and_then(Value::as_i64)
                .filter(|i| (0..8).contains(i))
                .ok_or_else(|| AssistantError::io("Unexpected tool call in the voice stream."))?;
            let function = call.get("function");
            out.tool_calls.push(ToolDelta {
                index: index as usize,
                id: get(call, "id").to_string(),
                name: function.map_or("", |f| get(f, "name")).to_string(),
                arguments: function.map_or("", |f| get(f, "arguments")).to_string(),
            });
        }
    }
    out.text = get(delta, "content").to_string();
    Ok(StreamStep::Delta(out))
}

/// Accumulates a streamed Chat Completions reply into the final message.
#[derive(Debug)]
pub struct ChatStream {
    content: String,
    calls: std::collections::BTreeMap<usize, (String, String, String)>,
    finish: String,
    /// When no tools are offered the text is forwarded as it arrives; otherwise it is held back
    /// until the turn is known to be text-only.
    pub early: bool,
}

impl ChatStream {
    pub fn new(early: bool) -> Self {
        Self {
            content: String::new(),
            calls: Default::default(),
            finish: String::new(),
            early,
        }
    }

    /// Apply a delta; returns the text to forward immediately (early mode only).
    pub fn push(&mut self, delta: &Delta) -> Result<Option<String>> {
        if let Some(reason) = &delta.finish {
            self.finish = reason.clone();
        }
        for call in &delta.tool_calls {
            if self.early {
                return Err(AssistantError::io(
                    "Unexpected tool call in the voice stream.",
                ));
            }
            let part = self.calls.entry(call.index).or_default();
            part.0.push_str(&call.id);
            part.1.push_str(&call.name);
            part.2.push_str(&call.arguments);
            if part.0.len() > 128 || part.1.len() > 128 || part.2.len() > 16000 {
                return Err(AssistantError::io(
                    "The streamed tool call exceeded its limit.",
                ));
            }
        }
        if delta.text.is_empty() {
            return Ok(None);
        }
        if self.content.len() + delta.text.len() > 64000 {
            return Err(AssistantError::io(
                "The spoken reply exceeded its text limit.",
            ));
        }
        self.content.push_str(&delta.text);
        Ok(self.early.then(|| delta.text.clone()))
    }

    pub fn finished(&self) -> bool {
        !self.finish.is_empty()
    }

    /// The complete non-streamed shape, ready for [`super::providers::parse_response`].
    pub fn into_response(self) -> Result<Value> {
        if self.finish.is_empty() {
            return Err(AssistantError::io(
                "The voice stream ended before its completion status.",
            ));
        }
        let mut message = serde_json::json!({ "content": self.content });
        if !self.calls.is_empty() {
            let list: Vec<Value> = self
                .calls
                .into_values()
                .map(|(id, name, args)| {
                    serde_json::json!({
                        "id": id, "type": "function",
                        "function": { "name": name, "arguments": args }
                    })
                })
                .collect();
            message["tool_calls"] = Value::Array(list);
        }
        Ok(serde_json::json!({
            "choices": [{ "finish_reason": self.finish, "message": message }]
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn events(chunks: &[&[u8]]) -> Vec<StreamEvent> {
        let mut parser = SseParser::new();
        chunks.iter().flat_map(|c| parser.feed(c)).collect()
    }

    #[test]
    fn sse_split_across_chunks() {
        let all = b"event: message\ndata: {\"a\":1}\r\n\r\ndata: one\ndata: two\n\n: comment\nevent:  custom \ndata:[DONE]\n\n";
        let whole = events(&[all]);
        assert_eq!(whole.len(), 3);
        assert_eq!(whole[0].event, "message");
        assert_eq!(whole[0].data, "{\"a\":1}");
        assert_eq!(whole[1].event, "");
        assert_eq!(whole[1].data, "one\ntwo");
        assert_eq!(whole[2].event, "custom");
        assert_eq!(whole[2].data, "[DONE]");
        // Every possible split point, including inside a multi-byte character and in the CRLF.
        let text = "data: héllo ✓\r\n\r\ndata: two\n\n".as_bytes();
        let reference = events(&[text]);
        assert_eq!(reference.len(), 2);
        for cut in 0..text.len() {
            let (a, b) = text.split_at(cut);
            assert_eq!(events(&[a, b]), reference, "cut {cut}");
        }
        // Byte-at-a-time.
        let singles: Vec<&[u8]> = text.chunks(1).collect();
        assert_eq!(events(&singles), reference);
    }

    #[test]
    fn truncated_frames_are_never_delivered() {
        assert!(events(&[b"data: {\"choices\":[{\"delta\""]).is_empty());
        assert!(events(&[b"data: partial\n"]).is_empty());
        assert_eq!(events(&[b"data: x\n\n"]).len(), 1);
        // A blank line resets the event name even when no data followed.
        let e = events(&[b"event: a\n\ndata: x\n\n"]);
        assert_eq!(e[0].event, "");
    }

    #[test]
    fn limits_stop_the_parser() {
        let mut parser = SseParser::new();
        assert!(parser.feed(&vec![b'a'; 128 * 1024 + 1]).is_empty());
        assert!(parser.error().is_some());
        assert!(parser.feed(b"data: x\n\n").is_empty());
        let mut parser = SseParser::new();
        let big = format!("data: {}\n", "a".repeat(100 * 1024));
        let mut out = Vec::new();
        for _ in 0..4 {
            out.extend(parser.feed(big.as_bytes()));
        }
        assert!(parser.error().is_some() && out.is_empty());
        let mut parser = SseParser::new();
        for _ in 0..43 {
            parser.feed(&vec![b'\n'; 100_000]);
        }
        assert!(parser.error().is_some());
        let mut parser = SseParser::new();
        parser.feed(&[0xff, b'\n']);
        assert!(parser.error().is_some());
        let mut parser = SseParser::new();
        parser.feed(format!("event: {}\n", "x".repeat(200)).as_bytes());
        assert!(parser.error().is_some());
        let mut parser = SseParser::new();
        let frames = "data: 1\n\n".repeat(32769);
        let mut count = 0;
        for chunk in frames.as_bytes().chunks(60_000) {
            count += parser.feed(chunk).len();
        }
        assert!(parser.error().is_some() && count == 32768);
    }

    fn step(data: &str) -> Result<StreamStep> {
        parse_stream_event(
            Protocol::ChatCompletions,
            &StreamEvent {
                event: String::new(),
                data: data.into(),
            },
        )
    }

    #[test]
    fn chat_stream_events_map_to_deltas() {
        assert_eq!(step("[DONE]").unwrap(), StreamStep::Done);
        let StreamStep::Delta(d) =
            step(r#"{"choices":[{"delta":{"content":"Hi","reasoning_content":"hidden"}}]}"#)
                .unwrap()
        else {
            panic!()
        };
        assert_eq!(d.text, "Hi");
        assert!(d.finish.is_none());
        let StreamStep::Delta(d) = step(
            r#"{"choices":[{"delta":{"tool_calls":[{"index":1,"id":"c","function":{"name":"n","arguments":"{"}}]},"finish_reason":"tool_calls"}]}"#,
        )
        .unwrap() else {
            panic!()
        };
        assert_eq!(d.finish.as_deref(), Some("tool_calls"));
        assert_eq!(d.tool_calls[0].index, 1);
        assert_eq!(step(r#"{"choices":[]}"#).unwrap(), StreamStep::Ignore);
        for bad in [
            "not json",
            r#"{"error":{}}"#,
            r#"{"failed":true}"#,
            r#"{"nochoices":1}"#,
            r#"{"choices":[{"finish_reason":"length","delta":{}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":9}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"id":"x"}]}}]}"#,
        ] {
            assert!(step(bad).is_err(), "{bad}");
        }
        let named = |event: &str| {
            parse_stream_event(
                Protocol::ChatCompletions,
                &StreamEvent {
                    event: event.into(),
                    data: "{}".into(),
                },
            )
        };
        assert_eq!(named("hermes.tool.progress").unwrap(), StreamStep::Progress);
        assert!(named("error").is_err());
        assert_eq!(named("ping").unwrap(), StreamStep::Ignore);
        assert!(parse_stream_event(Protocol::Anthropic, &StreamEvent::default()).is_err());
    }

    #[test]
    fn accumulator_builds_the_final_message() {
        let mut stream = ChatStream::new(false);
        let mk = |text: &str, finish: Option<&str>, calls: Vec<ToolDelta>| Delta {
            finish: finish.map(str::to_string),
            tool_calls: calls,
            text: text.into(),
        };
        stream.push(&mk("intro ", None, vec![])).unwrap();
        stream
            .push(&mk(
                "",
                None,
                vec![ToolDelta {
                    index: 0,
                    id: "c1".into(),
                    name: "naby_".into(),
                    arguments: "{\"pa".into(),
                }],
            ))
            .unwrap();
        stream
            .push(&mk(
                "",
                Some("tool_calls"),
                vec![ToolDelta {
                    index: 0,
                    id: String::new(),
                    name: "list".into(),
                    arguments: "ge\":1}".into(),
                }],
            ))
            .unwrap();
        let value = stream.into_response().unwrap();
        let call = &value["choices"][0]["message"]["tool_calls"][0];
        assert_eq!(call["id"], "c1");
        assert_eq!(call["function"]["name"], "naby_list");
        assert_eq!(call["function"]["arguments"], "{\"page\":1}");
        assert_eq!(value["choices"][0]["message"]["content"], "intro ");
        // Tool calls are rejected when no tools were offered, and the stream needs a finish.
        let mut early = ChatStream::new(true);
        assert_eq!(
            early.push(&mk("hi", None, vec![])).unwrap().as_deref(),
            Some("hi")
        );
        assert!(early
            .push(&mk(
                "",
                None,
                vec![ToolDelta {
                    index: 0,
                    id: "x".into(),
                    name: "n".into(),
                    arguments: String::new()
                }]
            ))
            .is_err());
        assert!(early.into_response().is_err());
        let mut long = ChatStream::new(true);
        assert!(long.push(&mk(&"x".repeat(64001), None, vec![])).is_err());
    }
}
