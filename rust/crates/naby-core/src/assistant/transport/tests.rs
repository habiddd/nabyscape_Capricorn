//! Transport behavior with an injected executor, plus loopback tests of the real executor.

use std::io::{Cursor, Read, Write};
use std::net::TcpListener;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use super::*;
use crate::assistant::conversation::{Hooks, SendRequest};
use crate::assistant::models::{Call, Role};
use crate::assistant::{new_id, Conversation};

const PROTOCOLS: [&str; 4] = ["ChatCompletions", "Responses", "Anthropic", "Gemini"];

type Seen = Arc<Mutex<Vec<HttpRequest>>>;
type Handler = Box<dyn FnMut(&HttpRequest) -> Result<HttpResponse> + Send>;

struct Script {
    handler: Mutex<Handler>,
    seen: Seen,
}

impl HttpExecutor for Script {
    fn execute(&self, request: &HttpRequest, _: Duration, _: &AtomicBool) -> Result<HttpResponse> {
        self.seen.lock().unwrap().push(request.clone());
        (self.handler.lock().unwrap())(request)
    }
}

fn transport(
    handler: impl FnMut(&HttpRequest) -> Result<HttpResponse> + Send + 'static,
) -> (Transport, Seen) {
    let seen: Seen = Arc::default();
    let script = Script {
        handler: Mutex::new(Box::new(handler)),
        seen: seen.clone(),
    };
    (Transport::new(Box::new(script)), seen)
}

fn json_response(status: u16, body: &Value) -> Result<HttpResponse> {
    Ok(HttpResponse::new(
        status,
        "application/json",
        body.to_string().into_bytes(),
    ))
}

fn tool() -> Tool {
    Tool::new(
        "inspect_app",
        "Inspect a selected app",
        json!({"type": "object", "properties": {}}),
    )
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

fn no_cancel() -> AtomicBool {
    AtomicBool::new(false)
}

fn user(text: &str) -> Vec<Message> {
    vec![Message::user(text)]
}

fn body_text(request: &HttpRequest) -> String {
    String::from_utf8(request.body.clone()).unwrap()
}

#[test]
fn each_protocol_sends_one_authenticated_request_and_parses_the_reply() {
    for protocol in PROTOCOLS {
        let (t, seen) = transport(move |_| json_response(200, &reply_json(protocol, false)));
        let mut image = Message::user("Describe this");
        image.image_data_url = Some("data:image/jpeg;base64,AQID".into());
        let reply = t
            .complete(
                &provider(protocol),
                Some("fixture-key"),
                &[Message::system("Answer"), image],
                &[tool()],
                &no_cancel(),
            )
            .unwrap();
        assert_eq!((reply.text.as_str(), reply.calls.len()), ("Hello", 0));
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        let auth = match protocol {
            "Anthropic" => seen[0].header("x-api-key"),
            "Gemini" => seen[0].header("x-goog-api-key"),
            _ => seen[0]
                .header("authorization")
                .and_then(|v| v.strip_prefix("Bearer ")),
        };
        assert_eq!(auth, Some("fixture-key"), "{protocol}");
        assert!(seen[0].url.starts_with("https://provider.example/v1/"));
        assert!(!seen[0].url.contains('?'));
    }
}

#[test]
fn bounded_voice_outcomes_preserve_valid_replay_after_http_tool_history() {
    for protocol in PROTOCOLS {
        let mut requests = 0;
        let (t, seen) = transport(move |request| {
            requests += 1;
            let wire = body_text(request);
            match requests {
                1 => return json_response(200, &reply_json(protocol, true)),
                2 => assert!(
                    wire.contains("call_1") && wire.contains("executed once"),
                    "{protocol}"
                ),
                _ => assert!(
                    wire.contains("voice outcome one")
                        && wire.contains("voice outcome two")
                        && !wire.contains("call_1")
                        && wire.contains("Continue after voice"),
                    "{protocol}: {wire}"
                ),
            }
            json_response(200, &reply_json(protocol, false))
        });
        let mut conversation = Conversation::new();
        let p = provider(protocol);
        let tools = [tool()];
        let mut executed = 0;
        let mut execute = |_: &Call, _: &AtomicBool| {
            executed += 1;
            Ok("executed once".to_string())
        };
        let mut request = SendRequest::new(&p, "Fixture", "Inspect");
        request.tools = &tools;
        conversation
            .send(
                &t,
                request,
                &mut Hooks {
                    execute: &mut execute,
                    refresh_screen: None,
                    voice_text: None,
                    voice_progress: None,
                },
            )
            .unwrap();
        assert_eq!(conversation.messages().len(), 4);
        assert_eq!(conversation.messages()[1].calls[0].id, "call_1");
        assert_eq!(conversation.messages()[2].role, Role::Tool);
        for i in 0..22 {
            conversation.record(Role::User, &format!("ordinary request {i}"));
            conversation.record(Role::Assistant, &format!("ordinary reply {i}"));
        }
        assert_eq!(conversation.messages().len(), 48);
        // Two bound-crossing outcomes must leave a history every adapter accepts.
        let epoch = conversation.epoch();
        conversation.record_voice_outcome(
            epoch,
            &Call::new("voice-one", "inspect_app", "{}"),
            "voice outcome one",
        );
        conversation.record_voice_outcome(
            epoch,
            &Call::new("voice-two", "inspect_app", "{}"),
            "voice outcome two",
        );
        let mut never =
            |_: &Call, _: &AtomicBool| -> Result<String> { panic!("No repeat action expected") };
        let mut request = SendRequest::new(&p, "Fixture", "Continue after voice");
        request.tools = &tools;
        conversation
            .send(
                &t,
                request,
                &mut Hooks {
                    execute: &mut never,
                    refresh_screen: None,
                    voice_text: None,
                    voice_progress: None,
                },
            )
            .unwrap();
        assert_eq!(seen.lock().unwrap().len(), 3, "{protocol}");
        assert_eq!(conversation.messages().last().unwrap().text, "Hello");
        assert_eq!(executed, 1);
    }
}

#[test]
fn invalid_empty_and_malformed_responses_use_safe_fixed_errors() {
    for protocol in PROTOCOLS {
        let mut unknown = reply_json(protocol, true)
            .to_string()
            .replace("inspect_app", "unknown_tool");
        let bodies = vec![
            String::new(),
            "null".to_string(),
            "{}".to_string(),
            "[]".to_string(),
            "{\"error\":{\"message\":\"fixture-secret\"}}".to_string(),
            std::mem::take(&mut unknown),
        ];
        for body in bodies {
            let (t, _) = transport(move |_| {
                Ok(HttpResponse::new(
                    200,
                    "application/json",
                    body.clone().into_bytes(),
                ))
            });
            let error = t
                .complete(
                    &provider(protocol),
                    Some("fixture-secret"),
                    &user("test"),
                    &[tool()],
                    &no_cancel(),
                )
                .unwrap_err();
            assert!(matches!(error, AssistantError::Io(_)), "{protocol}");
            assert!(!error.to_string().contains("fixture-secret"));
        }
    }
}

#[test]
fn oversized_responses_are_rejected_whether_or_not_the_length_is_declared() {
    let size = MAXIMUM_BODY_BYTES + 1;
    for declared in [true, false] {
        let (t, _) = transport(move |_| {
            let mut response = HttpResponse::new(200, "application/json", Vec::new());
            response.body = Box::new(std::io::repeat(b'x').take(size as u64));
            response.content_length = declared.then_some(size as u64);
            Ok(response)
        });
        let error = t
            .complete(
                &provider("Responses"),
                None,
                &user("test"),
                &[],
                &no_cancel(),
            )
            .unwrap_err();
        assert!(
            matches!(error, AssistantError::Io(_)),
            "declared={declared}"
        );
        assert!(error.to_string().contains("size limit"));
    }
}

#[test]
fn http_errors_and_redirects_are_reported_once_without_following_or_leaking() {
    for status in [302u16, 401, 429, 500] {
        let (t, seen) = transport(move |_| {
            Ok(HttpResponse::new(
                status,
                "text/plain",
                b"fixture-secret".to_vec(),
            ))
        });
        let mut p = provider("ChatCompletions");
        p.base_url = "http://localhost:1234/v1/".into();
        let error = t
            .complete(&p, None, &user("test"), &[], &no_cancel())
            .unwrap_err();
        assert!(
            matches!(&error, AssistantError::Io(m) if m.starts_with(&format!("HTTP {status}:"))),
            "{error}"
        );
        assert!(!error.to_string().contains("fixture-secret"));
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert!(seen[0].header("authorization").is_none());
    }
    // Provider error text is included when it is JSON.
    let (t, _) =
        transport(|_| json_response(400, &json!({"error": {"message": "Model not found"}})));
    let error = t
        .complete(
            &provider("Responses"),
            Some("k"),
            &user("test"),
            &[],
            &no_cancel(),
        )
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "HTTP 400: Check the protocol, model and request support. Provider: Model not found"
    );
}

#[test]
fn cancellation_timeouts_and_connection_failures_are_distinct() {
    // Cancelled before sending: nothing is executed.
    let (t, seen) = transport(|_| unreachable!());
    let cancel = AtomicBool::new(true);
    assert_eq!(
        t.complete(&provider("Responses"), None, &user("x"), &[], &cancel),
        Err(AssistantError::Cancelled)
    );
    assert!(seen.lock().unwrap().is_empty());
    // Deadline reached.
    let (t, _) = transport(|_| Err(AssistantError::Timeout("x".into())));
    assert!(matches!(
        t.complete(&provider("Responses"), None, &user("x"), &[], &no_cancel()),
        Err(AssistantError::Timeout(m)) if m == "The assistant request timed out. Try again or check the provider."
    ));
    // Cancelled while the body downloads.
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    let (t, _) = transport(move |_| {
        flag.store(true, Ordering::SeqCst);
        json_response(200, &reply_json("Responses", false))
    });
    assert_eq!(
        t.complete(&provider("Responses"), None, &user("x"), &[], &cancel),
        Err(AssistantError::Cancelled)
    );
    // Connection failure text differs for streamed replies.
    let (t, _) = transport(|_| Err(AssistantError::io(CONNECT_FAILED)));
    assert_eq!(
        t.complete(
            &provider("ChatCompletions"),
            None,
            &user("x"),
            &[],
            &no_cancel()
        )
        .unwrap_err()
        .to_string(),
        CONNECT_FAILED
    );
    let error = t
        .complete_streaming(
            &provider("ChatCompletions"),
            None,
            &user("x"),
            &[],
            &mut |_| Ok(()),
            &mut |_| {},
            &no_cancel(),
        )
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "The voice provider connection was interrupted."
    );
    assert!(Transport::ureq().with_timeout(Duration::ZERO).is_err());
    assert!(Transport::ureq()
        .with_timeout(Duration::from_secs(181))
        .is_err());
    assert!(Transport::ureq()
        .with_timeout(Duration::from_secs(20))
        .is_ok());
}

#[test]
fn hermes_and_image_replies_get_a_longer_deadline() {
    let seen_timeout = Arc::new(Mutex::new(Vec::new()));
    struct Probe(Arc<Mutex<Vec<Duration>>>, Value);
    impl HttpExecutor for Probe {
        fn execute(
            &self,
            _: &HttpRequest,
            timeout: Duration,
            _: &AtomicBool,
        ) -> Result<HttpResponse> {
            self.0.lock().unwrap().push(timeout);
            json_response(200, &self.1)
        }
    }
    let t = Transport::new(Box::new(Probe(
        seen_timeout.clone(),
        reply_json("ChatCompletions", false),
    )));
    let mut hermes = provider("ChatCompletions");
    hermes.backend = "Hermes".into();
    t.complete(
        &provider("ChatCompletions"),
        None,
        &user("x"),
        &[],
        &no_cancel(),
    )
    .unwrap();
    t.complete(&hermes, None, &user("x"), &[], &no_cancel())
        .unwrap();
    let mut images = provider("ChatCompletions");
    images.image_output = true;
    t.complete(&images, None, &user("x"), &[], &no_cancel())
        .unwrap();
    assert_eq!(
        *seen_timeout.lock().unwrap(),
        [
            Duration::from_secs(90),
            Duration::from_secs(180),
            Duration::from_secs(180)
        ]
    );
    // A caller-chosen deadline is respected.
    let t = Transport::new(Box::new(Probe(
        seen_timeout.clone(),
        reply_json("ChatCompletions", false),
    )))
    .with_timeout(Duration::from_secs(20))
    .unwrap();
    t.complete(&hermes, None, &user("x"), &[], &no_cancel())
        .unwrap();
    assert_eq!(
        seen_timeout.lock().unwrap().last(),
        Some(&Duration::from_secs(20))
    );
}

#[test]
fn replies_return_images_as_attachments() {
    let p = provider("ChatCompletions");
    let body = json!({"choices": [{"message": {"content": "Here you go ![cat](data:image/png;base64,AQID) done"}}]});
    let (t, _) = transport(move |_| json_response(200, &body));
    let reply = t
        .complete(&p, None, &user("draw"), &[], &no_cancel())
        .unwrap();
    assert_eq!(reply.text, "Here you go  done");
    assert_eq!(reply.attachments.len(), 1);
    assert_eq!(reply.attachments[0].mime, "image/png");
    assert_eq!(reply.attachments[0].bytes().unwrap(), vec![1, 2, 3]);
    // A reply that is only an image is valid.
    let only = json!({"output": [{"type": "image_generation_call", "result": "AQID", "output_format": "jpeg"}]});
    let (t, _) = transport(move |_| json_response(200, &only));
    let reply = t
        .complete(
            &provider("Responses"),
            None,
            &user("draw"),
            &[],
            &no_cancel(),
        )
        .unwrap();
    assert!(reply.text.is_empty() && reply.attachments[0].mime == "image/jpeg");
}

// ----- streaming ----------------------------------------------------------------------------

struct Trickle(Cursor<Vec<u8>>);

impl Read for Trickle {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = buf.len().min(1);
        self.0.read(&mut buf[..n])
    }
}

fn sse(body: &str) -> Result<HttpResponse> {
    let mut response = HttpResponse::new(200, "text/event-stream", Vec::new());
    response.content_length = None;
    response.body = Box::new(Trickle(Cursor::new(body.as_bytes().to_vec())));
    Ok(response)
}

fn delta(text: &str) -> String {
    format!(
        "data: {}\n\n",
        json!({"choices": [{"delta": {"content": text}}]})
    )
}

fn finish(reason: &str) -> String {
    format!(
        "data: {}\n\n",
        json!({"choices": [{"delta": {}, "finish_reason": reason}]})
    )
}

#[test]
fn streamed_text_is_forwarded_as_it_arrives_and_the_reply_is_assembled() {
    let body = format!(
        "{}{}{}data: [DONE]\n\n",
        delta("Hel"),
        delta("lo"),
        finish("stop")
    );
    let (t, seen) = transport(move |_| sse(&body));
    let mut spoken = Vec::new();
    let reply = t
        .complete_streaming(
            &provider("ChatCompletions"),
            Some("k"),
            &user("hi"),
            &[],
            &mut |s| {
                spoken.push(s.to_string());
                Ok(())
            },
            &mut |_| {},
            &no_cancel(),
        )
        .unwrap();
    assert_eq!(reply.text, "Hello");
    assert_eq!(spoken, ["Hel", "lo"]);
    let seen = seen.lock().unwrap();
    assert_eq!(seen[0].header("accept"), Some("text/event-stream"));
    assert_eq!(seen[0].json().unwrap()["stream"], true);
}

#[test]
fn streamed_tool_turns_are_buffered_until_the_final_message_is_known() {
    let call = |args: &str, id: &str, name: &str| {
        format!(
            "data: {}\n\n",
            json!({"choices": [{"delta": {"tool_calls": [
            {"index": 0, "id": id, "function": {"name": name, "arguments": args}}]}}]})
        )
    };
    let body = format!(
        "{}{}{}{}",
        delta("Let me check. "),
        call("{\"a\"", "call_1", "inspect_"),
        call(":1}", "", "app"),
        finish("tool_calls")
    );
    let (t, _) = transport(move |_| sse(&body));
    let mut spoken = Vec::new();
    let reply = t
        .complete_streaming(
            &provider("ChatCompletions"),
            None,
            &user("hi"),
            &[tool()],
            &mut |s| {
                spoken.push(s.to_string());
                Ok(())
            },
            &mut |_| {},
            &no_cancel(),
        )
        .unwrap();
    assert!(
        spoken.is_empty(),
        "intro text before a tool call is never spoken"
    );
    assert_eq!(reply.calls.len(), 1);
    assert_eq!(
        (
            reply.calls[0].id.as_str(),
            reply.calls[0].name.as_str(),
            reply.calls[0].arguments.as_str()
        ),
        ("call_1", "inspect_app", "{\"a\":1}")
    );
    assert_eq!(reply.text, "Let me check. ");
    // A tool-offering turn that ends as text is spoken once at the end.
    let body = format!("{}{}", delta("Plain answer"), finish("stop"));
    let (t, _) = transport(move |_| sse(&body));
    let mut spoken = Vec::new();
    t.complete_streaming(
        &provider("ChatCompletions"),
        None,
        &user("hi"),
        &[tool()],
        &mut |s| {
            spoken.push(s.to_string());
            Ok(())
        },
        &mut |_| {},
        &no_cancel(),
    )
    .unwrap();
    assert_eq!(spoken, ["Plain answer"]);
}

#[test]
fn broken_streams_fail_with_fixed_messages_and_never_complete_partially() {
    let run = |body: String| {
        let (t, _) = transport(move |_| sse(&body));
        t.complete_streaming(
            &provider("ChatCompletions"),
            None,
            &user("hi"),
            &[],
            &mut |_| Ok(()),
            &mut |_| {},
            &no_cancel(),
        )
    };
    // The connection dropped before any completion status.
    assert!(run(delta("partial"))
        .unwrap_err()
        .to_string()
        .contains("ended before its completion status"));
    // A frame cut mid-JSON is not promoted.
    assert!(run(format!("{}data: {{\"choices\":[{{\"delta\"", delta("a"))).is_err());
    assert!(
        run(format!("event: error\ndata: {{}}\n\n{}", finish("stop")))
            .unwrap_err()
            .to_string()
            .contains("stopped the streamed reply")
    );
    assert!(run(finish("length"))
        .unwrap_err()
        .to_string()
        .contains("interrupted or incomplete"));
    assert!(run("data: not json\n\n".into())
        .unwrap_err()
        .to_string()
        .contains("invalid voice stream"));
    assert!(run(format!("data: {{\"error\":{{}}}}\n\n{}", finish("stop"))).is_err());
    let tool_when_none_offered = format!(
        "data: {}\n\n",
        json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": "x"}]}}]})
    );
    assert!(run(tool_when_none_offered)
        .unwrap_err()
        .to_string()
        .contains("Unexpected tool call"));
    let huge = format!(
        "data: {}\n\n",
        json!({"choices": [{"delta": {"content": "x".repeat(70_000)}}]})
    );
    assert!(run(huge).is_err());
    // Frames that are not for this client are skipped.
    let ok = run(format!(
        "event: ping\ndata: {{}}\n\n{}{}",
        delta("Hi"),
        finish("stop")
    ))
    .unwrap();
    assert_eq!(ok.text, "Hi");
}

#[test]
fn streaming_servers_that_answer_with_json_are_consumed_without_resending() {
    let (t, seen) = transport(|_| json_response(200, &reply_json("ChatCompletions", false)));
    let mut spoken = Vec::new();
    let reply = t
        .complete_streaming(
            &provider("ChatCompletions"),
            None,
            &user("hi"),
            &[],
            &mut |s| {
                spoken.push(s.to_string());
                Ok(())
            },
            &mut |_| {},
            &no_cancel(),
        )
        .unwrap();
    assert_eq!(
        (
            reply.text.as_str(),
            spoken.as_slice(),
            seen.lock().unwrap().len()
        ),
        ("Hello", ["Hello".to_string()].as_slice(), 1)
    );
    // Other protocols and image replies use one piece.
    for protocol in ["Responses", "Anthropic", "Gemini"] {
        let (t, seen) = transport(move |_| json_response(200, &reply_json(protocol, false)));
        let mut spoken = String::new();
        t.complete_streaming(
            &provider(protocol),
            None,
            &user("hi"),
            &[],
            &mut |s| {
                spoken.push_str(s);
                Ok(())
            },
            &mut |_| {},
            &no_cancel(),
        )
        .unwrap();
        assert_eq!(spoken, "Hello");
        assert_eq!(seen.lock().unwrap()[0].json().unwrap().get("stream"), None);
    }
    let mut images = provider("ChatCompletions");
    images.image_output = true;
    let (t, seen) = transport(|_| json_response(200, &reply_json("ChatCompletions", false)));
    t.complete_streaming(
        &images,
        None,
        &user("hi"),
        &[],
        &mut |_| Ok(()),
        &mut |_| {},
        &no_cancel(),
    )
    .unwrap();
    assert_eq!(seen.lock().unwrap()[0].json().unwrap()["stream"], false);
}

#[test]
fn hermes_progress_frames_are_throttled_and_never_spoken() {
    let progress = "event: hermes.tool.progress\ndata: {\"tool\":\"search\"}\n\n";
    let body = format!(
        "{progress}{progress}{progress}{}{}",
        delta("Done."),
        finish("stop")
    );
    let (t, _) = transport(move |_| sse(&body));
    let mut hermes = provider("ChatCompletions");
    hermes.backend = "Hermes".into();
    let mut notes = Vec::new();
    let mut spoken = String::new();
    let reply = t
        .complete_streaming(
            &hermes,
            None,
            &user("hi"),
            &[],
            &mut |s| {
                spoken.push_str(s);
                Ok(())
            },
            &mut |n| notes.push(n.to_string()),
            &no_cancel(),
        )
        .unwrap();
    assert_eq!(reply.text, "Done.");
    assert_eq!(spoken, "Done.");
    assert_eq!(notes, ["Hermes is working with tools…"]);
}

#[test]
fn a_cancelled_stream_stops_reading() {
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    let (t, _) =
        transport(move |_| sse(&format!("{}{}{}", delta("a"), delta("b"), finish("stop"))));
    let error = t
        .complete_streaming(
            &provider("ChatCompletions"),
            None,
            &user("hi"),
            &[],
            &mut |_| {
                flag.store(true, Ordering::SeqCst);
                Ok(())
            },
            &mut |_| {},
            &cancel,
        )
        .unwrap_err();
    assert_eq!(error, AssistantError::Cancelled);
}

// ----- model list, checks and media ---------------------------------------------------------

#[test]
fn model_lists_follow_pages_dedupe_and_sort() {
    let (t, seen) = transport(|request| {
        if request.url.contains("pageToken=t2") {
            json_response(
                200,
                &json!({"models": [{"name": "models/zeta"}, {"name": "models/alpha"}]}),
            )
        } else {
            json_response(
                200,
                &json!({"models": [{"name": "models/Beta"}, {"name": "models/alpha"}], "nextPageToken": "t2"}),
            )
        }
    });
    let ids = t
        .list_models(&provider("Gemini"), Some("k"), &no_cancel())
        .unwrap();
    assert_eq!(ids, ["alpha", "Beta", "zeta"]);
    let urls: Vec<String> = seen.lock().unwrap().iter().map(|r| r.url.clone()).collect();
    assert_eq!(
        urls,
        [
            "https://provider.example/v1/models?pageSize=1000",
            "https://provider.example/v1/models?pageSize=1000&pageToken=t2"
        ]
    );
    // A repeated cursor ends paging; pages are capped at 20.
    let (t, seen) = transport(|_| {
        json_response(
            200,
            &json!({"data": [{"id": "m"}], "has_more": true, "last_id": "same"}),
        )
    });
    assert_eq!(
        t.list_models(&provider("Anthropic"), None, &no_cancel())
            .unwrap(),
        ["m"]
    );
    assert_eq!(seen.lock().unwrap().len(), 2);
    let (t, seen) = transport({
        let mut n = 0;
        move |_| {
            n += 1;
            json_response(
                200,
                &json!({"data": [{"id": format!("m{n}")}], "has_more": true, "last_id": format!("c{n}")}),
            )
        }
    });
    assert_eq!(
        t.list_models(&provider("Anthropic"), None, &no_cancel())
            .unwrap()
            .len(),
        20
    );
    assert_eq!(seen.lock().unwrap().len(), 20);
    // Errors.
    let (t, _) = transport(|_| json_response(200, &json!({"data": []})));
    assert!(t
        .list_models(&provider("ChatCompletions"), None, &no_cancel())
        .unwrap_err()
        .to_string()
        .contains("returned no models"));
    let (t, _) = transport(|_| json_response(200, &json!({"nope": []})));
    assert!(t
        .list_models(&provider("ChatCompletions"), None, &no_cancel())
        .unwrap_err()
        .to_string()
        .contains("Paste the model ID manually"));
    let (t, _) = transport(|_| json_response(401, &json!({})));
    assert!(t
        .list_models(&provider("ChatCompletions"), None, &no_cancel())
        .unwrap_err()
        .to_string()
        .starts_with("HTTP 401"));
    let mut bad = provider("ChatCompletions");
    bad.base_url = "http://remote.example/v1".into();
    assert!(t.list_models(&bad, None, &no_cancel()).is_err());
}

#[test]
fn connection_and_image_checks_send_minimal_requests() {
    let (t, seen) =
        transport(|_| json_response(200, &json!({"choices": [{"message": {"content": "OK"}}]})));
    let mut p = provider("ChatCompletions");
    p.image_output = true;
    t.check_connection(&p, Some("k"), &no_cancel()).unwrap();
    let body = seen.lock().unwrap()[0].json().unwrap();
    assert_eq!(body["messages"].as_array().unwrap().len(), 1);
    assert_eq!(
        body["messages"][0]["content"],
        "Reply with OK only. This is a connection check."
    );
    assert!(body.get("tools").is_none() && body.get("modalities").is_none());
    let (t, _) =
        transport(|_| json_response(200, &json!({"choices": [{"message": {"content": "  "}}]})));
    assert!(matches!(
        t.check_connection(&p, None, &no_cancel()),
        Err(AssistantError::Invalid(_))
    ));
    // A reply with no text and no media cannot even be parsed.
    let (t, _) =
        transport(|_| json_response(200, &json!({"choices": [{"message": {"content": ""}}]})));
    assert!(t.check_connection(&p, None, &no_cancel()).is_err());

    for (answer, expected) in [("Red.", true), ("blue", false)] {
        let (t, seen) = transport(move |_| {
            json_response(200, &json!({"choices": [{"message": {"content": answer}}]}))
        });
        assert_eq!(
            t.check_image_support(&p, None, &no_cancel()).unwrap(),
            expected
        );
        let body = seen.lock().unwrap()[0].json().unwrap();
        assert_eq!(
            body["messages"][0]["content"][1]["image_url"]["url"],
            crate::assistant::persistence::image_test_data()
        );
        assert!(
            body.get("modalities").is_none(),
            "image output is switched off for the check"
        );
    }
    let mut blind = p.clone();
    blind.vision = false;
    assert!(matches!(
        t_unused().check_image_support(&blind, None, &no_cancel()),
        Err(AssistantError::Invalid(_))
    ));
}

fn t_unused() -> Transport {
    transport(|_| unreachable!()).0
}

#[test]
fn media_downloads_authenticate_only_the_provider_that_returned_them() {
    let p = provider("ChatCompletions");
    let file = |url: &str| {
        Attachment::remote_media(&p, "i", "image/png", AttachmentSource::Remote(url.into()))
    };
    let (t, seen) = transport(|_| Ok(HttpResponse::new(200, "image/png", vec![1, 2, 3])));
    assert_eq!(
        t.download_media(
            &file("https://provider.example/v1/files/a.png"),
            Some(&p),
            Some("k"),
            &no_cancel()
        )
        .unwrap(),
        [1, 2, 3]
    );
    assert_eq!(
        t.download_media(
            &file("https://cdn.example.com/a.png"),
            Some(&p),
            Some("k"),
            &no_cancel()
        )
        .unwrap(),
        [1, 2, 3]
    );
    assert_eq!(
        t.download_media(
            &file("https://provider.example/other/a.png"),
            Some(&p),
            Some("k"),
            &no_cancel()
        )
        .unwrap(),
        [1, 2, 3]
    );
    let other = Provider {
        id: new_id(),
        ..p.clone()
    };
    t.download_media(
        &file("https://provider.example/v1/files/a.png"),
        Some(&other),
        Some("k"),
        &no_cancel(),
    )
    .unwrap();
    t.download_media(
        &file("https://provider.example/v1/files/a.png"),
        None,
        None,
        &no_cancel(),
    )
    .unwrap();
    let auth: Vec<Option<String>> = seen
        .lock()
        .unwrap()
        .iter()
        .map(|r| r.header("authorization").map(str::to_string))
        .collect();
    assert_eq!(auth, [Some("Bearer k".to_string()), None, None, None, None]);
    // Inline data never touches the network.
    let inline = Attachment::inline("i", "image/png", "data:image/png;base64,AQID".into(), "");
    assert_eq!(
        t.download_media(&inline, None, None, &no_cancel()).unwrap(),
        [1, 2, 3]
    );
    assert_eq!(seen.lock().unwrap().len(), 5);
    // Unreachable addresses, local files from other servers and HTTP failures are refused.
    assert!(t
        .download_media(&file("https://10.0.0.1/a.png"), None, None, &no_cancel())
        .is_err());
    assert!(t
        .download_media(&file("http://localhost:9/a.png"), None, None, &no_cancel())
        .is_err());
    let (t, _) = transport(|_| Ok(HttpResponse::new(404, "text/plain", Vec::new())));
    assert!(t
        .download_media(
            &file("https://cdn.example.com/a.png"),
            None,
            None,
            &no_cancel()
        )
        .unwrap_err()
        .to_string()
        .contains("HTTP 404"));
    let (t, _) = transport(|_| {
        let mut r = HttpResponse::new(200, "image/png", Vec::new());
        r.content_length = Some(MAXIMUM_FILE_BYTES + 1);
        Ok(r)
    });
    assert!(t
        .download_media(
            &file("https://cdn.example.com/a.png"),
            None,
            None,
            &no_cancel()
        )
        .is_err());
}

// ----- the real executor over loopback ------------------------------------------------------

/// Serve one HTTP exchange on a loopback port; returns the port and the request text.
fn serve(response: Vec<u8>) -> (u16, std::thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback listener");
    let port = listener.local_addr().unwrap().port();
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut request = Vec::new();
        let mut buffer = [0u8; 4096];
        loop {
            let n = stream.read(&mut buffer).unwrap_or(0);
            if n == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..n]);
            let text = String::from_utf8_lossy(&request).to_string();
            if let Some(split) = text.find("\r\n\r\n") {
                let length = text[..split]
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                    })
                    .unwrap_or(0);
                if request.len() >= split + 4 + length {
                    break;
                }
            }
        }
        stream.write_all(&response).unwrap();
        stream.flush().unwrap();
        String::from_utf8_lossy(&request).to_string()
    });
    (port, handle)
}

fn local(port: u16) -> Provider {
    Provider {
        protocol: "ChatCompletions".into(),
        base_url: format!("http://127.0.0.1:{port}/v1/"),
        model: "local-model".into(),
        ..Provider::default()
    }
}

fn http(status: &str, headers: &str, body: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

#[test]
fn ureq_executor_sends_the_built_request_over_loopback() {
    let reply = reply_json("ChatCompletions", false).to_string();
    let (port, server) = serve(http("200 OK", "Content-Type: application/json\r\n", &reply));
    let t = Transport::ureq();
    let result = t
        .complete(
            &local(port),
            Some("local-key"),
            &user("Hello over the wire"),
            &[],
            &no_cancel(),
        )
        .unwrap();
    assert_eq!(result.text, "Hello");
    let request = server.join().unwrap();
    assert!(
        request.starts_with("POST /v1/chat/completions HTTP/1.1"),
        "{request}"
    );
    let lower = request.to_ascii_lowercase();
    assert!(lower.contains("authorization: bearer local-key"));
    assert!(lower.contains("content-type: application/json"));
    assert!(lower.contains("user-agent: nabyscape/"));
    assert!(request.contains("Hello over the wire"));
}

#[test]
fn ureq_executor_never_follows_redirects_and_reports_http_errors() {
    let (port, server) = serve(http(
        "302 Found",
        "Location: https://untrusted.example/steal\r\n",
        "",
    ));
    let error = Transport::ureq()
        .complete(
            &local(port),
            Some("local-key"),
            &user("x"),
            &[],
            &no_cancel(),
        )
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "HTTP 302: Redirects are blocked; enter the final provider URL."
    );
    server.join().unwrap();
    let (port, server) = serve(http(
        "401 Unauthorized",
        "Content-Type: application/json\r\n",
        "{\"error\":{\"message\":\"bad key local-key\"}}",
    ));
    let error = Transport::ureq()
        .complete(
            &local(port),
            Some("local-key"),
            &user("x"),
            &[],
            &no_cancel(),
        )
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "HTTP 401: API key missing, invalid or expired. Provider: bad key [key hidden]"
    );
    server.join().unwrap();
}

#[test]
fn ureq_executor_streams_server_sent_events() {
    let body = format!(
        "{}{}{}data: [DONE]\n\n",
        delta("Hel"),
        delta("lo"),
        finish("stop")
    );
    let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream; charset=utf-8\r\nConnection: close\r\n\r\n";
    let (port, server) = serve(format!("{head}{body}").into_bytes());
    let mut spoken = Vec::new();
    let reply = Transport::ureq()
        .complete_streaming(
            &local(port),
            None,
            &user("hi"),
            &[],
            &mut |s| {
                spoken.push(s.to_string());
                Ok(())
            },
            &mut |_| {},
            &no_cancel(),
        )
        .unwrap();
    assert_eq!(reply.text, "Hello");
    assert_eq!(spoken, ["Hel", "lo"]);
    assert!(server.join().unwrap().contains("\"stream\":true"));
}

#[test]
fn ureq_executor_refuses_unreachable_and_disallowed_destinations() {
    // A closed loopback port is a connection failure, not a panic or a hang.
    let port = {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let error = Transport::ureq()
        .complete(&local(port), None, &user("x"), &[], &no_cancel())
        .unwrap_err();
    assert_eq!(error.to_string(), CONNECT_FAILED);
    // `localhost` must resolve to loopback only; a public name must not resolve to private space.
    assert!(UreqExecutor::check_destination("http://127.0.0.1:1/").is_ok());
    assert!(UreqExecutor::check_destination("not a url").is_err());
}
