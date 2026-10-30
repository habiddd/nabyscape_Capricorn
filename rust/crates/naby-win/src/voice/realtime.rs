//! Realtime voice over a WebSocket (`AssistantVoiceSession`): OpenAI Realtime and Gemini Live.
//!
//! * [`messages`]: the JSON the two protocols send, pure and tested.
//! * [`Session`]: the protocol state machine (transcripts, interruptions, tool calls, bounded
//!   audio), fed received JSON text and returning what to send and what to tell the host.
//! * [`connect`]: TCP (public addresses, or loopback for `ws://localhost`) + rustls + tungstenite,
//!   with the destination checks of the C# `AssistantVoiceSocket`: no redirects, no proxy.
//! * [`run`]: the single IO thread loop tying a socket, the session and the playback queue.
//!
//! Microphone capture lives in the host (`Capture`) and reaches the loop as [`Cmd::Mic`].

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use naby_core::assistant::models::{Call, Provider, Tool};
use naby_core::assistant::providers::{is_local_host, is_public_address};
use naby_core::assistant::voice::{realtime_endpoint, validate_voice_image};
use serde_json::{json, Value};
use tungstenite::client::IntoClientRequest;
use tungstenite::protocol::WebSocketConfig;
use tungstenite::{Message, WebSocket};
use url::Url;

use super::dsp::peak;
use super::playback::PcmQueue;

const MAXIMUM_MESSAGE: usize = 4 * 1024 * 1024;
const MAXIMUM_TEXT: usize = 128 * 1024;
const MAXIMUM_AUDIO_DELTA: usize = 512 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const READY_TIMEOUT: Duration = Duration::from_secs(20);
const PING_EVERY: Duration = Duration::from_secs(20);

// ----- messages ---------------------------------------------------------------------------

pub mod messages {
    use super::*;

    pub fn gemini_model(model: &str) -> String {
        if model.starts_with("models/") {
            model.to_owned()
        } else {
            format!("models/{model}")
        }
    }

    /// The first message of a session (`Setup`).
    pub fn setup(provider: &Provider, instructions: &str, tools: &[Tool]) -> String {
        if provider.realtime_protocol == "GeminiLive" {
            let declarations: Vec<Value> = tools
                .iter()
                .map(|t| json!({"name": t.name, "description": t.description, "parametersJsonSchema": t.parameters}))
                .collect();
            let tools_value = if declarations.is_empty() {
                json!([])
            } else {
                json!([{ "functionDeclarations": declarations }])
            };
            return json!({"setup": {
                "model": gemini_model(&provider.realtime_model),
                "generationConfig": {"responseModalities": ["AUDIO"], "speechConfig": {"voiceConfig": {"prebuiltVoiceConfig": {"voiceName": provider.voice}}}},
                "systemInstruction": {"parts": [{"text": instructions}]},
                "inputAudioTranscription": {}, "outputAudioTranscription": {},
                "realtimeInputConfig": {"automaticActivityDetection": {"disabled": false}, "activityHandling": "START_OF_ACTIVITY_INTERRUPTS"},
                "tools": tools_value
            }})
            .to_string();
        }
        let tools_value: Vec<Value> = tools
            .iter()
            .map(|t| json!({"type": "function", "name": t.name, "description": t.description, "parameters": t.parameters}))
            .collect();
        json!({"type": "session.update", "session": {
            "type": "realtime", "model": provider.realtime_model, "instructions": instructions,
            "output_modalities": ["audio"],
            "audio": {
                "input": {"format": {"type": "audio/pcm", "rate": 24000},
                    "transcription": {"model": "whisper-1"},
                    "turn_detection": {"type": "server_vad", "threshold": 0.5, "prefix_padding_ms": 300, "silence_duration_ms": 500, "create_response": true, "interrupt_response": false}},
                "output": {"format": {"type": "audio/pcm", "rate": 24000}, "voice": provider.voice}
            },
            "tools": tools_value
        }})
        .to_string()
    }

    /// A history line for OpenAI sessions.
    pub fn text_item(role: &str, text: &str) -> String {
        json!({"type": "conversation.item.create", "item": {"type": "message", "role": role,
            "content": [{"type": if role == "assistant" { "output_text" } else { "input_text" }, "text": text}]}})
        .to_string()
    }

    /// Gemini history seeding.
    pub fn gemini_history(lines: &[(bool, String)]) -> String {
        let turns: Vec<Value> = lines
            .iter()
            .map(|(user, text)| json!({"role": if *user { "user" } else { "model" }, "parts": [{"text": text}]}))
            .collect();
        json!({"clientContent": {"turns": turns, "turnComplete": false}}).to_string()
    }

    pub fn user_text(gemini: bool, text: &str) -> Vec<String> {
        if gemini {
            vec![json!({"clientContent": {"turns": [{"role": "user", "parts": [{"text": text}]}], "turnComplete": true}}).to_string()]
        } else {
            vec![
                text_item("user", text),
                "{\"type\":\"response.create\"}".into(),
            ]
        }
    }

    pub fn audio_append(gemini: bool, pcm: &[u8]) -> String {
        let data = base64::engine::general_purpose::STANDARD.encode(pcm);
        if gemini {
            json!({"realtimeInput": {"audio": {"data": data, "mimeType": "audio/pcm;rate=16000"}}})
                .to_string()
        } else {
            json!({"type": "input_audio_buffer.append", "audio": data}).to_string()
        }
    }

    /// An explicitly shared image (`CaptureImage`). `data_url` must already be validated.
    pub fn image(gemini: bool, data_url: &str) -> String {
        let comma = data_url.find(',').unwrap_or(0);
        if gemini {
            let mime = data_url
                .get(5..)
                .and_then(|r| r.split(';').next())
                .unwrap_or("image/jpeg");
            json!({"realtimeInput": {"video": {"mimeType": mime, "data": &data_url[comma + 1..]}}})
                .to_string()
        } else {
            json!({"type": "conversation.item.create", "item": {"type": "message", "role": "user",
                "content": [{"type": "input_image", "image_url": data_url}]}})
            .to_string()
        }
    }

    pub fn tool_result(gemini: bool, call: &Call, result: &str) -> String {
        if gemini {
            json!({"toolResponse": {"functionResponses": [{"id": call.id, "name": call.name, "response": {"result": result}}]}}).to_string()
        } else {
            json!({"type": "conversation.item.create", "item": {"type": "function_call_output", "call_id": call.id, "output": result}}).to_string()
        }
    }

    pub fn truncate(item: &str, index: i64, played_ms: u64) -> String {
        json!({"type": "conversation.item.truncate", "item_id": item, "content_index": index, "audio_end_ms": played_ms}).to_string()
    }
}

// ----- the protocol state machine -------------------------------------------------------------

/// What the session tells the host.
#[derive(Debug, Clone, PartialEq)]
pub enum Out {
    Ready,
    Status(String),
    Transcript {
        user: bool,
        text: String,
    },
    ToolCall(Call),
    /// The provider cancelled these pending calls.
    CancelCalls(Vec<String>),
}

/// A fatal protocol problem with a user-safe message.
pub type Failure = String;

pub struct Session {
    pub gemini: bool,
    tool_names: Vec<String>,
    user_text: String,
    assistant_text: String,
    ready_calls: Vec<Call>,
    pending: Vec<String>,
    seen_calls: Vec<String>,
    response_active: bool,
    ignore_audio: bool,
    audio_item: String,
    audio_index: i64,
    audio_start_ms: u64,
    output_bytes: usize,
    ready: bool,
}

fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

impl Session {
    pub fn new(gemini: bool, tools: &[Tool]) -> Self {
        Self {
            gemini,
            tool_names: tools.iter().map(|t| t.name.clone()).collect(),
            user_text: String::new(),
            assistant_text: String::new(),
            ready_calls: Vec::new(),
            pending: Vec::new(),
            seen_calls: Vec::new(),
            response_active: false,
            ignore_audio: false,
            audio_item: String::new(),
            audio_index: 0,
            audio_start_ms: 0,
            output_bytes: 0,
            ready: false,
        }
    }

    pub fn ready(&self) -> bool {
        self.ready
    }

    pub fn pending_calls(&self) -> &[String] {
        &self.pending
    }

    /// A tool result was delivered; returns whether OpenAI should be told to continue.
    pub fn finish_call(&mut self, id: &str) -> bool {
        let removed = self
            .pending
            .iter()
            .position(|p| p == id)
            .map(|i| self.pending.remove(i))
            .is_some();
        removed && !self.gemini && !self.response_active && self.pending.is_empty()
    }

    pub fn is_pending(&self, id: &str) -> bool {
        self.pending.iter().any(|p| p == id)
    }

    /// The user spoke over the reply (or typed): cancel the running reply, trim the heard audio
    /// and drop queued speech. Returns the messages to send.
    pub fn interrupt(&mut self, played_ms: u64, queue: &PcmQueue) -> Vec<String> {
        let mut events = Vec::new();
        let played = played_ms.saturating_sub(self.audio_start_ms);
        if !self.gemini && self.response_active {
            events.push("{\"type\":\"response.cancel\"}".to_owned());
        }
        if !self.gemini && !self.audio_item.is_empty() {
            events.push(messages::truncate(
                &self.audio_item,
                self.audio_index,
                played,
            ));
        }
        queue.flush();
        self.audio_item.clear();
        self.audio_start_ms = 0;
        self.output_bytes = 0;
        self.ignore_audio = !self.gemini;
        self.assistant_text.clear();
        self.ready_calls.clear();
        self.response_active = false;
        self.pending.clear();
        events
    }

    /// Handles one received text frame. `Err` is fatal (the session ends with the message).
    pub fn handle(
        &mut self,
        text: &str,
        queue: &PcmQueue,
        sends: &mut Vec<String>,
    ) -> Result<Vec<Out>, Failure> {
        let invalid = || {
            "Voice received an invalid response or disconnected. Start voice again to reconnect."
                .to_owned()
        };
        if text.len() > MAXIMUM_MESSAGE {
            return Err(invalid());
        }
        let value: Value = serde_json::from_str(text).map_err(|_| invalid())?;
        if !value.is_object() {
            return Err(invalid());
        }
        let mut out = Vec::new();
        if self.gemini {
            self.gemini_event(&value, queue, sends, &mut out)?;
        } else {
            self.openai_event(&value, queue, sends, &mut out)?;
        }
        Ok(out)
    }

    fn audio(
        &mut self,
        encoded: &str,
        item: &str,
        index: i64,
        queue: &PcmQueue,
        sends: &mut Vec<String>,
        out: &mut Vec<Out>,
    ) -> Result<(), Failure> {
        let invalid = || {
            "Voice received an invalid response or disconnected. Start voice again to reconnect."
                .to_owned()
        };
        if encoded.len() > MAXIMUM_AUDIO_DELTA {
            return Err(invalid());
        }
        let pcm = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|_| invalid())?;
        if pcm.len() & 1 != 0 {
            return Err(invalid());
        }
        if self.ignore_audio {
            return Ok(());
        }
        if !item.is_empty() && self.audio_item != item {
            self.audio_item = item.to_owned();
            self.audio_index = index;
            self.audio_start_ms = (self.output_bytes / 48) as u64;
        }
        if queue.push(&pcm) {
            self.output_bytes += pcm.len();
            return Ok(());
        }
        // A very long reply exhausted the bounded queue: interrupt only that reply.
        let played = queue.played_ms();
        sends.extend(self.interrupt(played, queue));
        self.ignore_audio = true;
        out.push(Out::Status(
            "Voice reply exceeded the playback limit. Ask for a shorter answer.".into(),
        ));
        Ok(())
    }

    fn call(
        &mut self,
        id: &str,
        name: &str,
        arguments: &str,
        out: &mut Vec<Out>,
    ) -> Result<(), Failure> {
        let invalid = || {
            "Voice received an invalid response or disconnected. Start voice again to reconnect."
                .to_owned()
        };
        if id.is_empty()
            || id.len() > 200
            || name.is_empty()
            || name.len() > 100
            || arguments.len() > MAXIMUM_TEXT
        {
            return Err(invalid());
        }
        match serde_json::from_str::<Value>(arguments) {
            Ok(v) if v.is_object() => {}
            _ => return Err(invalid()),
        }
        if !self.tool_names.iter().any(|t| t == name) {
            return Err(invalid());
        }
        if self.seen_calls.iter().any(|c| c == id) {
            return Ok(());
        }
        if self.seen_calls.len() >= 256 || self.pending.len() >= 32 {
            return Err(invalid());
        }
        self.seen_calls.push(id.to_owned());
        self.pending.push(id.to_owned());
        let call = Call::new(id, name, arguments);
        if self.gemini {
            out.push(Out::ToolCall(call));
        } else {
            self.ready_calls.push(call);
        }
        Ok(())
    }

    fn openai_event(
        &mut self,
        e: &Value,
        queue: &PcmQueue,
        sends: &mut Vec<String>,
        out: &mut Vec<Out>,
    ) -> Result<(), Failure> {
        let invalid = || {
            "Voice received an invalid response or disconnected. Start voice again to reconnect."
                .to_owned()
        };
        match s(e, "type") {
            "session.updated" => {
                self.ready = true;
                out.push(Out::Ready);
            }
            "response.created" => {
                self.response_active = true;
                self.ignore_audio = false;
            }
            "response.done" => {
                self.response_active = false;
                let status = e.get("response").map_or("", |r| s(r, "status"));
                if status == "failed" {
                    return Err(invalid());
                }
                let calls = std::mem::take(&mut self.ready_calls);
                if status == "completed" {
                    out.extend(calls.into_iter().map(Out::ToolCall));
                } else {
                    let ids: Vec<String> = calls.into_iter().map(|c| c.id).collect();
                    self.pending.retain(|p| !ids.contains(p));
                    out.push(Out::CancelCalls(ids));
                }
            }
            "response.output_audio.delta" => {
                let index = e.get("content_index").and_then(Value::as_i64).unwrap_or(0);
                self.audio(s(e, "delta"), s(e, "item_id"), index, queue, sends, out)?;
            }
            "response.output_audio_transcript.done" => {
                if !self.ignore_audio {
                    final_text(out, false, s(e, "transcript"))?;
                }
            }
            "response.output_text.done" => {
                if !self.ignore_audio {
                    final_text(out, false, s(e, "text"))?;
                }
            }
            "conversation.item.input_audio_transcription.completed" => {
                final_text(out, true, s(e, "transcript"))?;
            }
            "conversation.item.input_audio_transcription.failed" => {
                out.push(Out::Status(
                    "Voice transcription unavailable for this turn.".into(),
                ));
            }
            "response.function_call_arguments.done" => {
                self.call(s(e, "call_id"), s(e, "name"), s(e, "arguments"), out)?;
            }
            "input_audio_buffer.speech_started" => {
                let played = queue.played_ms();
                let events = self.interrupt(played, queue);
                out.push(Out::CancelCalls(Vec::new()));
                sends.extend(events);
            }
            "error" => return Err(invalid()),
            _ => {}
        }
        Ok(())
    }

    fn gemini_event(
        &mut self,
        e: &Value,
        queue: &PcmQueue,
        sends: &mut Vec<String>,
        out: &mut Vec<Out>,
    ) -> Result<(), Failure> {
        if let Some(error) = e.get("error") {
            // Raw provider messages may echo credentials or input; only the code is used.
            let code = error.get("code").and_then(Value::as_i64).unwrap_or(0);
            return Err(match code {
                401 | 403 => "Gemini rejected access. Check the Google AI Studio API key and model access in Settings > AI.",
                404 => "Gemini Live model not found. Check the realtime model ID in Settings > AI.",
                429 => "Gemini Live quota exceeded. Check your Google AI Studio limits or try again later.",
                400 => "Gemini rejected the voice configuration. Check the realtime model, voice name and supported capabilities in Settings > AI.",
                _ => "Gemini rejected the voice session. Check the API key, live model and quota in Google AI Studio.",
            }
            .into());
        }
        if e.get("setupComplete").is_some() {
            self.ready = true;
            out.push(Out::Ready);
        }
        if e.get("goAway").is_some() {
            return Err("Voice session is ending. Start voice again to reconnect.".into());
        }
        if let Some(ids) = e
            .get("toolCallCancellation")
            .and_then(|c| c.get("ids"))
            .and_then(Value::as_array)
        {
            let ids: Vec<String> = ids
                .iter()
                .filter_map(|i| i.as_str().map(str::to_owned))
                .collect();
            self.pending.retain(|p| !ids.contains(p));
            out.push(Out::CancelCalls(ids));
        }
        if let Some(content) = e.get("serverContent") {
            if content.get("interrupted").and_then(Value::as_bool) == Some(true) {
                let played = queue.played_ms();
                self.interrupt(played, queue);
                out.push(Out::CancelCalls(Vec::new()));
            }
            if let Some(t) = content.get("inputTranscription") {
                append_text(&mut self.user_text, s(t, "text"))?;
            }
            if !self.ignore_audio {
                if let Some(t) = content.get("outputTranscription") {
                    append_text(&mut self.assistant_text, s(t, "text"))?;
                }
            }
            if let Some(parts) = content
                .get("modelTurn")
                .and_then(|t| t.get("parts"))
                .and_then(Value::as_array)
            {
                for part in parts {
                    if let Some(data) = part.get("inlineData") {
                        if s(data, "mimeType") != "audio/pcm;rate=24000" {
                            return Err("Voice received an invalid response or disconnected. Start voice again to reconnect.".into());
                        }
                        self.audio(s(data, "data"), "", 0, queue, sends, out)?;
                    }
                }
            }
            if content.get("turnComplete").and_then(Value::as_bool) == Some(true) {
                let user = std::mem::take(&mut self.user_text);
                let assistant = std::mem::take(&mut self.assistant_text);
                final_text(out, true, &user)?;
                final_text(out, false, &assistant)?;
                self.ignore_audio = false;
            }
        }
        if let Some(functions) = e
            .get("toolCall")
            .and_then(|c| c.get("functionCalls"))
            .and_then(Value::as_array)
        {
            let user = std::mem::take(&mut self.user_text);
            final_text(out, true, &user)?;
            for f in functions {
                let args = f.get("args").map_or("{}".to_owned(), Value::to_string);
                self.call(s(f, "id"), s(f, "name"), &args, out)?;
            }
        }
        Ok(())
    }
}

fn append_text(buffer: &mut String, text: &str) -> Result<(), Failure> {
    if buffer.len() + text.len() > MAXIMUM_TEXT {
        return Err(
            "Voice received an invalid response or disconnected. Start voice again to reconnect."
                .into(),
        );
    }
    buffer.push_str(text);
    Ok(())
}

fn final_text(out: &mut Vec<Out>, user: bool, text: &str) -> Result<(), Failure> {
    if text.len() > MAXIMUM_TEXT {
        return Err("Voice text is too large.".into());
    }
    if !text.trim().is_empty() {
        out.push(Out::Transcript {
            user,
            text: text.to_owned(),
        });
    }
    Ok(())
}

// ----- the socket ------------------------------------------------------------------------------

/// A TCP stream, optionally wrapped in TLS.
pub enum Stream {
    Plain(TcpStream),
    Tls(Box<rustls::StreamOwned<rustls::ClientConnection, TcpStream>>),
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Stream::Plain(s) => s.read(buf),
            Stream::Tls(s) => s.read(buf),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Stream::Plain(s) => s.write(buf),
            Stream::Tls(s) => s.write(buf),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Stream::Plain(s) => s.flush(),
            Stream::Tls(s) => s.flush(),
        }
    }
}

pub type Socket = WebSocket<Stream>;

fn tls_config() -> Result<Arc<rustls::ClientConfig>, String> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|e| e.to_string())?
    .with_root_certificates(roots)
    .with_no_client_auth();
    Ok(Arc::new(config))
}

/// Resolves `host` and keeps only the addresses the destination rules allow: public addresses
/// for remote hosts, loopback for `localhost` and loopback IPs.
pub fn allowed_addresses(host: &str, port: u16) -> Vec<std::net::SocketAddr> {
    let local = is_local_host(host);
    (host.trim_matches(['[', ']']), port)
        .to_socket_addrs()
        .map(|it| {
            it.filter(|a| {
                if local {
                    a.ip().is_loopback()
                } else {
                    is_public_address(a.ip())
                }
            })
            .collect()
        })
        .unwrap_or_default()
}

/// Connects (blocking, bounded by [`CONNECT_TIMEOUT`]). The key is sent in a header, never in
/// the URL.
pub fn connect(endpoint: &Url, key: Option<&str>, gemini: bool) -> Result<Socket, String> {
    let fail = || {
        "Voice connection failed. Check the endpoint, model, voice and API key, then start again."
            .to_owned()
    };
    let host = endpoint.host_str().ok_or_else(fail)?.to_owned();
    let tls = endpoint.scheme() == "wss";
    let port = endpoint.port().unwrap_or(if tls { 443 } else { 80 });
    let deadline = Instant::now() + CONNECT_TIMEOUT;
    let mut tcp = None;
    for address in allowed_addresses(&host, port) {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        if let Ok(stream) = TcpStream::connect_timeout(&address, left.min(Duration::from_secs(15)))
        {
            tcp = Some(stream);
            break;
        }
    }
    let tcp = tcp.ok_or_else(|| {
        "Voice destination unavailable. Check the network and voice endpoint.".to_owned()
    })?;
    let _ = tcp.set_nodelay(true);
    let _ = tcp.set_read_timeout(Some(CONNECT_TIMEOUT));
    let _ = tcp.set_write_timeout(Some(Duration::from_secs(10)));
    let timeouts = tcp.try_clone().map_err(|_| fail())?;
    let stream = if tls {
        let name = rustls::pki_types::ServerName::try_from(host.clone()).map_err(|_| fail())?;
        let conn = rustls::ClientConnection::new(tls_config()?, name).map_err(|_| fail())?;
        Stream::Tls(Box::new(rustls::StreamOwned::new(conn, tcp)))
    } else {
        Stream::Plain(tcp)
    };
    let mut request = endpoint
        .as_str()
        .into_client_request()
        .map_err(|_| fail())?;
    if let Some(key) = key.filter(|k| !k.trim().is_empty()) {
        let (name, value) = if gemini {
            ("x-goog-api-key", key.to_owned())
        } else {
            ("Authorization", format!("Bearer {key}"))
        };
        request.headers_mut().insert(
            name,
            value
                .parse()
                .map_err(|_| "The API key contains invalid characters.".to_owned())?,
        );
    }
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAXIMUM_MESSAGE))
        .max_frame_size(Some(MAXIMUM_MESSAGE));
    let (socket, _) = tungstenite::client::client_with_config(request, stream, Some(config))
        .map_err(|_| fail())?;
    // From here on reads return after a short wait so the loop can also send.
    let _ = timeouts.set_read_timeout(Some(Duration::from_millis(25)));
    Ok(socket)
}

// ----- the loop -----------------------------------------------------------------------------------

/// Commands for the IO loop.
pub enum Cmd {
    /// A microphone chunk (16-bit LE mono at the session's input rate).
    Mic(Vec<u8>),
    Text(String),
    /// A validated image data URL.
    Image(String),
    ToolResult {
        call: Call,
        result: String,
    },
    Stop,
}

/// Reports from the loop to the host.
pub enum Report {
    Out(Out),
    /// The mic level of the last chunk.
    MicLevel(f32),
    Failed(String),
    Ended,
}

/// Everything a session needs.
pub struct Config {
    pub provider: Provider,
    pub key: Option<String>,
    pub instructions: String,
    pub tools: Vec<Tool>,
    /// Recent history `(is_user, text)`.
    pub history: Vec<(bool, String)>,
}

/// Input sample rate the provider expects.
pub fn input_rate(provider: &Provider) -> u32 {
    if provider.realtime_protocol == "GeminiLive" {
        16_000
    } else {
        24_000
    }
}

/// Validates the provider and returns the connect URL.
pub fn endpoint_for(provider: &Provider) -> Result<Url, String> {
    if !matches!(
        provider.realtime_protocol.as_str(),
        "OpenAIRealtime" | "GeminiLive"
    ) {
        return Err(
            "This voice protocol is not supported. Choose OpenAIRealtime or GeminiLive.".into(),
        );
    }
    if provider.realtime_model.trim().is_empty()
        || provider.realtime_model.len() > 200
        || provider.voice.trim().is_empty()
        || provider.voice.len() > 100
    {
        return Err("Configure a realtime model and voice before starting voice.".into());
    }
    realtime_endpoint(
        &provider.realtime_url,
        &provider.realtime_model,
        provider.realtime_protocol == "GeminiLive",
    )
    .map_err(|e| e.message().to_owned())
}

fn is_timeout(e: &tungstenite::Error) -> bool {
    matches!(e, tungstenite::Error::Io(io) if matches!(io.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut))
}

/// Runs a connected session until Stop or a failure. Blocking: call from its own thread.
pub fn run(
    mut socket: Socket,
    config: Config,
    queue: Arc<PcmQueue>,
    cmds: Receiver<Cmd>,
    report: Sender<Report>,
    mut on_ready: impl FnMut() + Send,
) {
    let gemini = config.provider.realtime_protocol == "GeminiLive";
    let mut session = Session::new(gemini, &config.tools);
    let send = |socket: &mut Socket, text: String| -> Result<(), String> {
        socket
            .send(Message::text(text))
            .map_err(|_| "Voice upload failed. Start voice again to reconnect.".to_owned())
    };
    let fail = |report: &Sender<Report>, message: String| {
        let _ = report.send(Report::Failed(message));
    };
    let tools: &[Tool] = if config.provider.tools {
        &config.tools
    } else {
        &[]
    };
    if send(
        &mut socket,
        messages::setup(&config.provider, &config.instructions, tools),
    )
    .is_err()
    {
        return fail(
            &report,
            "Voice upload failed. Start voice again to reconnect.".into(),
        );
    }
    let started = Instant::now();
    let mut seeded = false;
    let mut last_ping = Instant::now();
    let result: Result<(), String> = (|| {
        loop {
            // Outgoing commands first; the microphone is held back until the session is ready.
            loop {
                match cmds.try_recv() {
                    Ok(Cmd::Stop) | Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        return Ok(())
                    }
                    Ok(Cmd::Mic(pcm)) => {
                        if session.ready() && seeded {
                            let samples = super::dsp::pcm16_to_f32(&pcm);
                            let _ = report.send(Report::MicLevel(peak(&samples)));
                            send(&mut socket, messages::audio_append(gemini, &pcm))?;
                        }
                    }
                    Ok(Cmd::Text(text)) => {
                        let played = queue.played_ms();
                        for m in session.interrupt(played, &queue) {
                            send(&mut socket, m)?;
                        }
                        for m in messages::user_text(gemini, &text) {
                            send(&mut socket, m)?;
                        }
                    }
                    Ok(Cmd::Image(data_url)) => {
                        validate_voice_image(&data_url).map_err(|e| e.message().to_owned())?;
                        send(&mut socket, messages::image(gemini, &data_url))?;
                    }
                    Ok(Cmd::ToolResult { call, result }) => {
                        if session.is_pending(&call.id) {
                            send(&mut socket, messages::tool_result(gemini, &call, &result))?;
                            if session.finish_call(&call.id) {
                                send(&mut socket, "{\"type\":\"response.create\"}".into())?;
                            }
                        }
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                }
            }
            if last_ping.elapsed() >= PING_EVERY {
                last_ping = Instant::now();
                let _ = socket.send(Message::Ping(tungstenite::Bytes::new()));
            }
            if !session.ready() && started.elapsed() > READY_TIMEOUT {
                return Err("Voice connection timed out. Check the network and voice endpoint, then try again.".into());
            }
            match socket.read() {
                Ok(Message::Text(text)) => {
                    let mut sends = Vec::new();
                    let outs = session.handle(text.as_str(), &queue, &mut sends)?;
                    for m in sends {
                        send(&mut socket, m)?;
                    }
                    for out in outs {
                        if out == Out::Ready && !seeded {
                            seeded = true;
                            on_ready();
                            // Seed bounded text history only; images go out only on request.
                            let lines: Vec<(bool, String)> = config.history.iter().rev().take(80).rev().cloned().collect();
                            let total: usize = lines.iter().map(|(_, t)| t.len()).sum();
                            if total > 512 * 1024 {
                                return Err("Voice history is too large.".into());
                            }
                            if gemini && !lines.is_empty() {
                                send(&mut socket, messages::gemini_history(&lines))?;
                            } else if !gemini {
                                for (user, text) in &lines {
                                    send(&mut socket, messages::text_item(if *user { "user" } else { "assistant" }, text))?;
                                }
                            }
                        }
                        let _ = report.send(Report::Out(out));
                    }
                }
                Ok(Message::Binary(_)) => {
                    return Err("Voice received an invalid response or disconnected. Start voice again to reconnect.".into())
                }
                Ok(Message::Close(_)) => return Err("Voice disconnected. Start voice again to reconnect.".into()),
                Ok(_) => {}
                Err(e) if is_timeout(&e) => {}
                Err(_) => return Err("Voice received an invalid response or disconnected. Start voice again to reconnect.".into()),
            }
        }
    })();
    let _ = socket.close(None);
    let _ = socket.flush();
    match result {
        Ok(()) => {
            let _ = report.send(Report::Ended);
        }
        Err(message) => fail(&report, message),
    }
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;
    use std::sync::mpsc::channel;

    use super::*;

    fn provider(protocol: &str) -> Provider {
        Provider {
            realtime_protocol: protocol.into(),
            realtime_model: "rt-1".into(),
            realtime_url: "ws://127.0.0.1:1/".into(),
            voice: "alloy".into(),
            tools: true,
            ..Provider::default()
        }
    }

    fn tools() -> Vec<Tool> {
        vec![Tool::new(
            "naby_navigate_widget",
            "Open a page",
            json!({"type": "object", "properties": {"page": {"type": "string"}}}),
        )]
    }

    #[test]
    fn setup_messages_follow_each_protocol() {
        let openai: Value = serde_json::from_str(&messages::setup(
            &provider("OpenAIRealtime"),
            "Be brief",
            &tools(),
        ))
        .unwrap();
        assert_eq!(openai["type"], "session.update");
        assert_eq!(openai["session"]["model"], "rt-1");
        assert_eq!(openai["session"]["audio"]["input"]["format"]["rate"], 24000);
        assert_eq!(openai["session"]["audio"]["output"]["voice"], "alloy");
        assert_eq!(
            openai["session"]["tools"][0]["name"],
            "naby_navigate_widget"
        );
        assert_eq!(
            openai["session"]["audio"]["input"]["turn_detection"]["interrupt_response"],
            false
        );

        let gemini: Value = serde_json::from_str(&messages::setup(
            &provider("GeminiLive"),
            "Be brief",
            &tools(),
        ))
        .unwrap();
        assert_eq!(gemini["setup"]["model"], "models/rt-1");
        assert_eq!(
            gemini["setup"]["generationConfig"]["speechConfig"]["voiceConfig"]
                ["prebuiltVoiceConfig"]["voiceName"],
            "alloy"
        );
        assert_eq!(
            gemini["setup"]["tools"][0]["functionDeclarations"][0]["name"],
            "naby_navigate_widget"
        );
        assert_eq!(messages::gemini_model("models/x"), "models/x");
        let none: Value =
            serde_json::from_str(&messages::setup(&provider("GeminiLive"), "i", &[])).unwrap();
        assert_eq!(none["setup"]["tools"], json!([]));
    }

    #[test]
    fn media_and_text_messages() {
        let a: Value = serde_json::from_str(&messages::audio_append(false, &[1, 2, 3, 4])).unwrap();
        assert_eq!(a["type"], "input_audio_buffer.append");
        assert_eq!(a["audio"], "AQIDBA==");
        let g: Value = serde_json::from_str(&messages::audio_append(true, &[1, 2])).unwrap();
        assert_eq!(
            g["realtimeInput"]["audio"]["mimeType"],
            "audio/pcm;rate=16000"
        );
        let img = "data:image/png;base64,AAAA";
        let gi: Value = serde_json::from_str(&messages::image(true, img)).unwrap();
        assert_eq!(gi["realtimeInput"]["video"]["mimeType"], "image/png");
        assert_eq!(gi["realtimeInput"]["video"]["data"], "AAAA");
        let oi: Value = serde_json::from_str(&messages::image(false, img)).unwrap();
        assert_eq!(oi["item"]["content"][0]["image_url"], img);
        assert_eq!(messages::user_text(false, "hi").len(), 2);
        assert_eq!(messages::user_text(true, "hi").len(), 1);
        let call = Call::new("c1", "naby_navigate_widget", "{}");
        let r: Value = serde_json::from_str(&messages::tool_result(false, &call, "ok")).unwrap();
        assert_eq!(r["item"]["call_id"], "c1");
        let gr: Value = serde_json::from_str(&messages::tool_result(true, &call, "ok")).unwrap();
        assert_eq!(
            gr["toolResponse"]["functionResponses"][0]["response"]["result"],
            "ok"
        );
        let t: Value = serde_json::from_str(&messages::truncate("item", 2, 1500)).unwrap();
        assert_eq!(
            (t["content_index"].as_i64(), t["audio_end_ms"].as_u64()),
            (Some(2), Some(1500))
        );
    }

    fn feed(session: &mut Session, queue: &PcmQueue, value: Value) -> (Vec<Out>, Vec<String>) {
        let mut sends = Vec::new();
        let out = session
            .handle(&value.to_string(), queue, &mut sends)
            .unwrap();
        (out, sends)
    }

    #[test]
    fn openai_events_produce_transcripts_audio_and_gated_tool_calls() {
        let queue = PcmQueue::new();
        let mut s = Session::new(false, &tools());
        let (out, _) = feed(&mut s, &queue, json!({"type": "session.updated"}));
        assert_eq!(out, vec![Out::Ready]);
        feed(&mut s, &queue, json!({"type": "response.created"}));
        let pcm = base64::engine::general_purpose::STANDARD.encode([0u8; 4800]);
        feed(
            &mut s,
            &queue,
            json!({"type": "response.output_audio.delta", "delta": pcm, "item_id": "i1", "content_index": 0}),
        );
        assert_eq!(queue.queued_ms(), 100);
        let (out, _) = feed(
            &mut s,
            &queue,
            json!({"type": "conversation.item.input_audio_transcription.completed", "transcript": "open weather"}),
        );
        assert_eq!(
            out,
            vec![Out::Transcript {
                user: true,
                text: "open weather".into()
            }]
        );
        let (out, _) = feed(
            &mut s,
            &queue,
            json!({"type": "response.output_audio_transcript.done", "transcript": "Opening weather."}),
        );
        assert_eq!(
            out,
            vec![Out::Transcript {
                user: false,
                text: "Opening weather.".into()
            }]
        );
        // A tool call is held until the response completes.
        let (out, _) = feed(
            &mut s,
            &queue,
            json!({"type": "response.function_call_arguments.done", "call_id": "c1", "name": "naby_navigate_widget", "arguments": "{\"page\":\"weather\"}"}),
        );
        assert!(out.is_empty());
        let (out, _) = feed(
            &mut s,
            &queue,
            json!({"type": "response.done", "response": {"status": "completed"}}),
        );
        assert!(matches!(&out[..], [Out::ToolCall(c)] if c.id == "c1"));
        assert!(s.is_pending("c1"));
        // After the result is delivered OpenAI continues the reply.
        assert!(s.finish_call("c1"));
        assert!(!s.finish_call("c1"));
    }

    #[test]
    fn unknown_tools_bad_arguments_and_provider_errors_are_fatal() {
        let queue = PcmQueue::new();
        let mut sends = Vec::new();
        let mut s = Session::new(false, &tools());
        let bad_tool = json!({"type": "response.function_call_arguments.done", "call_id": "c", "name": "format_disk", "arguments": "{}"}).to_string();
        assert!(s.handle(&bad_tool, &queue, &mut sends).is_err());
        let bad_args = json!({"type": "response.function_call_arguments.done", "call_id": "c", "name": "naby_navigate_widget", "arguments": "[1]"}).to_string();
        assert!(s.handle(&bad_args, &queue, &mut sends).is_err());
        assert!(s
            .handle("{\"type\":\"error\"}", &queue, &mut sends)
            .is_err());
        assert!(s.handle("not json", &queue, &mut sends).is_err());
        assert!(s.handle("[]", &queue, &mut sends).is_err());
        let failed = json!({"type": "response.done", "response": {"status": "failed"}}).to_string();
        assert!(s.handle(&failed, &queue, &mut sends).is_err());
        let odd = base64::engine::general_purpose::STANDARD.encode([0u8; 3]);
        let odd_audio = json!({"type": "response.output_audio.delta", "delta": odd}).to_string();
        assert!(s.handle(&odd_audio, &queue, &mut sends).is_err());
    }

    #[test]
    fn speaking_over_the_reply_cancels_truncates_and_flushes() {
        let queue = PcmQueue::new();
        let mut s = Session::new(false, &tools());
        feed(&mut s, &queue, json!({"type": "response.created"}));
        let pcm = base64::engine::general_purpose::STANDARD.encode(vec![0u8; 48_000]);
        feed(
            &mut s,
            &queue,
            json!({"type": "response.output_audio.delta", "delta": pcm, "item_id": "item-9", "content_index": 0}),
        );
        queue.take(24_000);
        let (_, sends) = feed(
            &mut s,
            &queue,
            json!({"type": "input_audio_buffer.speech_started"}),
        );
        assert_eq!(sends.len(), 2);
        assert!(sends[0].contains("response.cancel"));
        let truncate: Value = serde_json::from_str(&sends[1]).unwrap();
        assert_eq!(truncate["item_id"], "item-9");
        assert!(queue.is_empty());
        // Audio of the cancelled reply is ignored until a new response starts.
        feed(
            &mut s,
            &queue,
            json!({"type": "response.output_audio.delta", "delta": pcm, "item_id": "item-9"}),
        );
        assert!(queue.is_empty());
        feed(&mut s, &queue, json!({"type": "response.created"}));
        feed(
            &mut s,
            &queue,
            json!({"type": "response.output_audio.delta", "delta": pcm, "item_id": "item-10"}),
        );
        assert!(!queue.is_empty());
    }

    #[test]
    fn gemini_events_accumulate_transcripts_until_the_turn_completes() {
        let queue = PcmQueue::new();
        let mut s = Session::new(true, &tools());
        let (out, _) = feed(&mut s, &queue, json!({"setupComplete": {}}));
        assert_eq!(out, vec![Out::Ready]);
        feed(
            &mut s,
            &queue,
            json!({"serverContent": {"inputTranscription": {"text": "hello "}}}),
        );
        feed(
            &mut s,
            &queue,
            json!({"serverContent": {"inputTranscription": {"text": "there"}, "outputTranscription": {"text": "Hi!"}}}),
        );
        let pcm = base64::engine::general_purpose::STANDARD.encode([0u8; 960]);
        feed(
            &mut s,
            &queue,
            json!({"serverContent": {"modelTurn": {"parts": [{"inlineData": {"mimeType": "audio/pcm;rate=24000", "data": pcm}}]}}}),
        );
        assert_eq!(queue.queued_ms(), 20);
        let (out, _) = feed(
            &mut s,
            &queue,
            json!({"serverContent": {"turnComplete": true}}),
        );
        assert_eq!(
            out,
            vec![
                Out::Transcript {
                    user: true,
                    text: "hello there".into()
                },
                Out::Transcript {
                    user: false,
                    text: "Hi!".into()
                }
            ]
        );
        let (out, _) = feed(
            &mut s,
            &queue,
            json!({"toolCall": {"functionCalls": [{"id": "g1", "name": "naby_navigate_widget", "args": {"page": "tasks"}}]}}),
        );
        assert!(matches!(&out[..], [Out::ToolCall(c)] if c.arguments.contains("tasks")));
        let (out, _) = feed(
            &mut s,
            &queue,
            json!({"toolCallCancellation": {"ids": ["g1"]}}),
        );
        assert_eq!(out, vec![Out::CancelCalls(vec!["g1".into()])]);
        assert!(!s.is_pending("g1"));
        // Wrong audio format is rejected; interruption flushes audio.
        let bad = json!({"serverContent": {"modelTurn": {"parts": [{"inlineData": {"mimeType": "audio/pcm;rate=16000", "data": "AAAA"}}]}}}).to_string();
        assert!(s.handle(&bad, &queue, &mut Vec::new()).is_err());
        feed(
            &mut s,
            &queue,
            json!({"serverContent": {"interrupted": true}}),
        );
        assert!(queue.is_empty());
    }

    #[test]
    fn gemini_errors_never_echo_the_provider_message() {
        let queue = PcmQueue::new();
        let mut s = Session::new(true, &[]);
        let e = s
            .handle(
                &json!({"error": {"code": 403, "message": "key AIza-secret rejected"}}).to_string(),
                &queue,
                &mut Vec::new(),
            )
            .unwrap_err();
        assert!(e.contains("rejected access") && !e.contains("secret"));
        assert!(s
            .handle("{\"goAway\":{}}", &queue, &mut Vec::new())
            .is_err());
    }

    #[test]
    fn a_full_playback_queue_interrupts_only_that_reply() {
        let queue = PcmQueue::new();
        queue.push(&vec![
            0u8;
            naby_core::assistant::voice::MAXIMUM_SPOKEN_PCM_BYTES
                - 2
        ]);
        let mut s = Session::new(false, &[]);
        let pcm = base64::engine::general_purpose::STANDARD.encode([0u8; 8]);
        let mut sends = Vec::new();
        let out = s
            .handle(
                &json!({"type": "response.output_audio.delta", "delta": pcm, "item_id": "x"})
                    .to_string(),
                &queue,
                &mut sends,
            )
            .unwrap();
        assert!(matches!(&out[..], [Out::Status(m)] if m.contains("playback limit")));
        assert!(queue.is_empty());
    }

    #[test]
    fn endpoint_rules_and_protocol_validation() {
        let mut p = provider("OpenAIRealtime");
        p.realtime_url = "wss://api.openai.com/v1/realtime".into();
        assert_eq!(
            endpoint_for(&p).unwrap().as_str(),
            "wss://api.openai.com/v1/realtime?model=rt-1"
        );
        p.realtime_url = "wss://api.openai.com/v1/realtime?key=secret".into();
        assert!(endpoint_for(&p).is_err());
        p.realtime_url = "ws://example.com/rt".into();
        assert!(endpoint_for(&p).is_err(), "plain ws is loopback only");
        let mut q = provider("Other");
        q.realtime_url = "wss://api.openai.com/v1/realtime".into();
        assert!(endpoint_for(&q).unwrap_err().contains("not supported"));
        let mut g = provider("GeminiLive");
        g.realtime_url = naby_core::assistant::voice::default_realtime_url("GeminiLive").into();
        assert!(!endpoint_for(&g).unwrap().as_str().contains('?'));
        assert_eq!(input_rate(&g), 16_000);
        assert_eq!(input_rate(&p), 24_000);
        g.voice.clear();
        assert!(endpoint_for(&g).unwrap_err().contains("model and voice"));
    }

    #[test]
    fn destinations_are_filtered_to_public_or_loopback() {
        assert!(!allowed_addresses("127.0.0.1", 80).is_empty());
        assert!(allowed_addresses("10.0.0.5", 80).is_empty());
        assert!(allowed_addresses("192.168.1.2", 443).is_empty());
        assert!(allowed_addresses("169.254.169.254", 80).is_empty());
        assert!(!allowed_addresses("localhost", 80).is_empty());
    }

    /// A tiny local server speaking enough OpenAI Realtime for one exchange.
    #[test]
    #[allow(clippy::result_large_err)]
    fn a_session_runs_over_a_loopback_websocket_end_to_end() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut ws = tungstenite::accept_hdr(
                stream,
                |req: &tungstenite::handshake::server::Request, resp| {
                    assert_eq!(
                        req.headers().get("authorization").unwrap(),
                        "Bearer sk-test"
                    );
                    assert!(req.uri().to_string().contains("model=rt-1"));
                    Ok(resp)
                },
            )
            .unwrap();
            let setup = ws.read().unwrap().into_text().unwrap();
            assert!(setup.contains("session.update"));
            ws.send(Message::text(
                json!({"type": "session.updated"}).to_string(),
            ))
            .unwrap();
            let pcm = base64::engine::general_purpose::STANDARD.encode([0u8; 960]);
            ws.send(Message::text(
                json!({"type": "response.created"}).to_string(),
            ))
            .unwrap();
            ws.send(Message::text(
                json!({"type": "response.output_audio.delta", "delta": pcm, "item_id": "a"})
                    .to_string(),
            ))
            .unwrap();
            ws.send(Message::text(
                json!({"type": "response.output_audio_transcript.done", "transcript": "Hello!"})
                    .to_string(),
            ))
            .unwrap();
            // The client's microphone chunk arrives after the session is ready.
            loop {
                let text = ws.read().unwrap().into_text().unwrap();
                if text.contains("input_audio_buffer.append") {
                    break;
                }
            }
            let _ = ws.close(None);
        });
        let mut p = provider("OpenAIRealtime");
        p.realtime_url = format!("ws://127.0.0.1:{port}/rt");
        let endpoint = endpoint_for(&p).unwrap();
        let socket = connect(&endpoint, Some("sk-test"), false).unwrap();
        let (cmd_tx, cmd_rx) = channel();
        let (report_tx, report_rx) = channel();
        let queue = Arc::new(PcmQueue::new());
        let q = queue.clone();
        let config = Config {
            provider: p,
            key: None,
            instructions: "hi".into(),
            tools: vec![],
            history: vec![(true, "earlier".into())],
        };
        let runner = std::thread::spawn(move || run(socket, config, q, cmd_rx, report_tx, || {}));
        let mut transcript = None;
        let mut sent_mic = false;
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            match report_rx.recv_timeout(Duration::from_millis(50)) {
                Ok(Report::Out(Out::Transcript { text, .. })) => {
                    transcript = Some(text);
                    cmd_tx.send(Cmd::Mic(vec![1u8; 960])).unwrap();
                    sent_mic = true;
                }
                Ok(Report::Failed(m)) => {
                    assert!(sent_mic, "failed early: {m}");
                    break;
                }
                Ok(Report::Ended) => break,
                _ => {}
            }
        }
        assert_eq!(transcript.as_deref(), Some("Hello!"));
        assert_eq!(queue.queued_ms(), 20);
        let _ = cmd_tx.send(Cmd::Stop);
        runner.join().unwrap();
        server.join().unwrap();
    }
}
