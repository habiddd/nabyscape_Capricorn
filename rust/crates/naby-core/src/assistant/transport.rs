//! HTTP execution for the assistant (`AssistantTransport.cs`, `AssistantProviderApi.cs`,
//! streaming from `AssistantStreaming.cs`).
//!
//! Requests are plain data; the [`HttpExecutor`] trait is the only thing that touches the
//! network, so every path is testable with an injected executor. The real executor
//! ([`UreqExecutor`]) is blocking: call it from a worker thread and share an
//! `Arc<AtomicBool>` to cancel.

use std::io::Read;
use std::net::{IpAddr, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::Value;
use url::Url;

use super::attachments::{bytes_from_data_url, Attachment, AttachmentSource, MAXIMUM_FILE_BYTES};
use super::media::media_uri;
use super::models::{Message, Protocol, Provider, Reply, Tool};
use super::providers::{
    active_tools, authenticate, build_request, failure_message, invalid_response, is_local_host,
    is_public_address, models_request, parse_models_page, parse_response, sort_models,
    validate_endpoint, validate_input, validate_key, MAXIMUM_BODY_BYTES,
};
use super::sse::{parse_stream_event, ChatStream, SseParser, StreamStep};
use super::{AssistantError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
}

impl Method {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
        }
    }
}

/// A fully built request: the same shape for every provider kind.
#[derive(Debug, Clone, PartialEq)]
pub struct HttpRequest {
    pub method: Method,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl HttpRequest {
    /// Case-insensitive header lookup.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// The body parsed as JSON (tests and diagnostics).
    pub fn json(&self) -> Option<Value> {
        serde_json::from_slice(&self.body).ok()
    }
}

/// A response whose body is still a stream, so large or long-lived bodies stay bounded.
pub struct HttpResponse {
    pub status: u16,
    pub content_type: String,
    pub content_length: Option<u64>,
    pub body: Box<dyn Read + Send>,
}

impl HttpResponse {
    pub fn new(status: u16, content_type: &str, body: Vec<u8>) -> Self {
        Self {
            status,
            content_type: content_type.into(),
            content_length: Some(body.len() as u64),
            body: Box::new(std::io::Cursor::new(body)),
        }
    }

    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// Sends a request. Implementations must not follow redirects and must enforce `timeout` as an
/// overall deadline, returning [`AssistantError::Timeout`] when it passes.
pub trait HttpExecutor: Send + Sync {
    fn execute(
        &self,
        request: &HttpRequest,
        timeout: Duration,
        cancel: &AtomicBool,
    ) -> Result<HttpResponse>;
}

const CONNECT_FAILED: &str = "The assistant could not connect securely to the configured provider.";

/// Blocking executor over `ureq`.
///
/// It uses a dedicated agent (same user agent and connection reuse as `http::agent()`)
/// because the shared agent follows redirects and has a 30 s read timeout; provider calls must
/// block redirects (a credential must never follow a redirect to another origin) and agent
/// backends like Hermes can stay silent for minutes. Destinations are re-checked against DNS
/// before sending: a public host may not resolve to a private address, nor a local host to a
/// non-loopback one.
pub struct UreqExecutor {
    agent: ureq::Agent,
}

impl Default for UreqExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl UreqExecutor {
    pub fn new() -> Self {
        let agent = ureq::AgentBuilder::new()
            .user_agent(crate::http::USER_AGENT)
            .redirects(0)
            .timeout_connect(Duration::from_secs(15))
            .max_idle_connections(2)
            .build();
        Self { agent }
    }

    fn check_destination(url: &str) -> Result<()> {
        let parsed = Url::parse(url).map_err(|_| AssistantError::io(CONNECT_FAILED))?;
        let host = parsed
            .host_str()
            .unwrap_or("")
            .trim_matches(['[', ']'])
            .to_string();
        let local = is_local_host(&host);
        if host.parse::<IpAddr>().is_ok() {
            return Ok(());
        }
        let port = parsed.port_or_known_default().unwrap_or(443);
        let addresses: Vec<_> = (host.as_str(), port)
            .to_socket_addrs()
            .map_err(|_| AssistantError::io(CONNECT_FAILED))?
            .collect();
        let allowed = |a: &std::net::SocketAddr| {
            if local {
                a.ip().is_loopback()
            } else {
                is_public_address(a.ip())
            }
        };
        if addresses.is_empty() || !addresses.iter().all(allowed) {
            return Err(AssistantError::io(
                "The provider destination is unavailable or disallowed.",
            ));
        }
        Ok(())
    }
}

fn timed_out(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
    ) || error.to_string().to_ascii_lowercase().contains("timed out")
}

impl HttpExecutor for UreqExecutor {
    fn execute(
        &self,
        request: &HttpRequest,
        timeout: Duration,
        cancel: &AtomicBool,
    ) -> Result<HttpResponse> {
        if cancel.load(Ordering::Relaxed) {
            return Err(AssistantError::Cancelled);
        }
        Self::check_destination(&request.url)?;
        let mut call = self
            .agent
            .request(request.method.as_str(), &request.url)
            .timeout(timeout);
        for (name, value) in &request.headers {
            call = call.set(name, value);
        }
        let result = match request.method {
            Method::Get => call.call(),
            Method::Post => call.send_bytes(&request.body),
        };
        let response = match result {
            Ok(response) | Err(ureq::Error::Status(_, response)) => response,
            Err(ureq::Error::Transport(t)) => {
                let text = t.to_string().to_ascii_lowercase();
                return Err(if text.contains("timed out") || text.contains("timeout") {
                    AssistantError::Timeout("The assistant request timed out.".into())
                } else {
                    AssistantError::io(CONNECT_FAILED)
                });
            }
        };
        let status = response.status();
        let content_type = response
            .header("content-type")
            .unwrap_or("")
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        let content_length = response
            .header("content-length")
            .and_then(|v| v.trim().parse().ok());
        Ok(HttpResponse {
            status,
            content_type,
            content_length,
            body: Box::new(response.into_reader()),
        })
    }
}

// ----- bounded reading ---------------------------------------------------------------------

fn read_error(
    error: &std::io::Error,
    cancel: &AtomicBool,
    timeout_message: &str,
) -> AssistantError {
    if cancel.load(Ordering::Relaxed) || error.kind() == std::io::ErrorKind::Interrupted {
        AssistantError::Cancelled
    } else if timed_out(error) {
        AssistantError::Timeout(timeout_message.into())
    } else {
        AssistantError::io("The provider connection was interrupted.")
    }
}

/// Read a body of at most `maximum` bytes (`ReadBounded`).
fn read_all(
    response: &mut HttpResponse,
    maximum: usize,
    cancel: &AtomicBool,
    timeout_message: &str,
    too_large: &str,
) -> Result<Vec<u8>> {
    if response.content_length.is_some_and(|l| l > maximum as u64) {
        return Err(AssistantError::io(too_large));
    }
    let mut out = Vec::new();
    let mut chunk = [0u8; 16384];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(AssistantError::Cancelled);
        }
        let n = response
            .body
            .read(&mut chunk)
            .map_err(|e| read_error(&e, cancel, timeout_message))?;
        if n == 0 {
            return Ok(out);
        }
        if out.len() + n > maximum {
            return Err(AssistantError::io(too_large));
        }
        out.extend_from_slice(&chunk[..n]);
    }
}

const TOO_LARGE: &str = "The provider response exceeded the size limit.";
const REQUEST_TIMEOUT: &str = "The assistant request timed out. Try again or check the provider.";
const VOICE_TIMEOUT: &str =
    "The voice reply timed out. Check the provider session before retrying.";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(90);

/// What the conversation needs from a provider connection.
pub trait AssistantTransport {
    fn complete(
        &self,
        provider: &Provider,
        key: Option<&str>,
        messages: &[Message],
        tools: &[Tool],
        cancel: &AtomicBool,
    ) -> Result<Reply>;

    /// Complete while forwarding spoken text as it arrives. Providers that cannot stream answer
    /// in one piece, which is forwarded once if the turn ended without tool calls.
    #[allow(clippy::too_many_arguments)]
    fn complete_streaming(
        &self,
        provider: &Provider,
        key: Option<&str>,
        messages: &[Message],
        tools: &[Tool],
        text: &mut dyn FnMut(&str) -> Result<()>,
        progress: &mut dyn FnMut(&str),
        cancel: &AtomicBool,
    ) -> Result<Reply> {
        let _ = progress;
        let reply = self.complete(provider, key, messages, tools, cancel)?;
        if reply.calls.is_empty() && !reply.text.is_empty() {
            text(&reply.text)?;
        }
        Ok(reply)
    }
}

/// Provider adapter: builds requests, executes them and parses replies.
pub struct Transport {
    executor: Box<dyn HttpExecutor>,
    timeout: Duration,
}

impl Transport {
    pub fn new(executor: Box<dyn HttpExecutor>) -> Self {
        Self {
            executor,
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// The production transport (blocking `ureq` executor, 90 s per request).
    pub fn ureq() -> Self {
        Self::new(Box::new(UreqExecutor::new()))
    }

    /// Override the request deadline (`requestTimeout`, up to three minutes).
    pub fn with_timeout(mut self, timeout: Duration) -> Result<Self> {
        if timeout.is_zero() || timeout > Duration::from_secs(180) {
            return Err(AssistantError::invalid(
                "The request timeout is out of range.",
            ));
        }
        self.timeout = timeout;
        Ok(self)
    }

    fn deadline(&self, agent_work: bool) -> Duration {
        if agent_work && self.timeout == DEFAULT_TIMEOUT {
            Duration::from_secs(180)
        } else {
            self.timeout
        }
    }

    fn send(
        &self,
        request: &HttpRequest,
        timeout: Duration,
        cancel: &AtomicBool,
        timeout_message: &str,
        connect_message: &str,
    ) -> Result<HttpResponse> {
        if cancel.load(Ordering::Relaxed) {
            return Err(AssistantError::Cancelled);
        }
        self.executor
            .execute(request, timeout, cancel)
            .map_err(|e| match e {
                AssistantError::Timeout(_) => AssistantError::Timeout(timeout_message.into()),
                AssistantError::Io(m) if m == CONNECT_FAILED => AssistantError::io(connect_message),
                other => other,
            })
    }

    fn failure(
        &self,
        response: &mut HttpResponse,
        key: Option<&str>,
        cancel: &AtomicBool,
    ) -> AssistantError {
        let body = if response.content_length.is_some_and(|l| l > 32768) {
            None
        } else {
            read_all(response, 32768, cancel, REQUEST_TIMEOUT, TOO_LARGE).ok()
        };
        if cancel.load(Ordering::Relaxed) {
            return AssistantError::Cancelled;
        }
        AssistantError::io(failure_message(response.status, body.as_deref(), key))
    }

    /// Non-streaming completion for every protocol (`Complete`).
    pub fn complete(
        &self,
        provider: &Provider,
        key: Option<&str>,
        messages: &[Message],
        tools: &[Tool],
        cancel: &AtomicBool,
    ) -> Result<Reply> {
        if cancel.load(Ordering::Relaxed) {
            return Err(AssistantError::Cancelled);
        }
        let request = build_request(provider, key, messages, tools, false)?;
        let timeout = self.deadline(provider.is_hermes() || provider.image_output);
        let mut response = self.send(&request, timeout, cancel, REQUEST_TIMEOUT, CONNECT_FAILED)?;
        if !response.is_success() {
            return Err(self.failure(&mut response, key, cancel));
        }
        let bytes = read_all(
            &mut response,
            MAXIMUM_BODY_BYTES,
            cancel,
            REQUEST_TIMEOUT,
            TOO_LARGE,
        )?;
        let root: Value = serde_json::from_slice(&bytes).map_err(|_| invalid_response())?;
        parse_response(provider, &root, active_tools(provider, tools))
    }

    /// Chat Completions streaming (`CompleteStreaming`). Other protocols, and providers with
    /// image output, answer in one piece.
    #[allow(clippy::too_many_arguments)]
    pub fn complete_streaming(
        &self,
        provider: &Provider,
        key: Option<&str>,
        messages: &[Message],
        tools: &[Tool],
        text: &mut dyn FnMut(&str) -> Result<()>,
        progress: &mut dyn FnMut(&str),
        cancel: &AtomicBool,
    ) -> Result<Reply> {
        if provider.protocol_kind() != Some(Protocol::ChatCompletions) || provider.image_output {
            let reply = self.complete(provider, key, messages, tools, cancel)?;
            if reply.calls.is_empty() && !reply.text.is_empty() {
                text(&reply.text)?;
            }
            return Ok(reply);
        }
        if cancel.load(Ordering::Relaxed) {
            return Err(AssistantError::Cancelled);
        }
        let request = build_request(provider, key, messages, tools, true)?;
        let active = active_tools(provider, tools);
        let timeout = self.deadline(provider.is_hermes());
        let connect = "The voice provider connection was interrupted.";
        let mut response = self.send(&request, timeout, cancel, VOICE_TIMEOUT, connect)?;
        if !response.is_success() {
            return Err(self.failure(&mut response, key, cancel));
        }
        if response
            .content_length
            .is_some_and(|l| l > MAXIMUM_BODY_BYTES as u64)
        {
            return Err(AssistantError::io(
                "The provider response exceeded its limit.",
            ));
        }
        if response.content_type != "text/event-stream" {
            // Some compatible servers return JSON even when stream=true. Consume this same
            // response; never resend an agent request as an automatic fallback.
            let bytes = read_all(
                &mut response,
                MAXIMUM_BODY_BYTES,
                cancel,
                VOICE_TIMEOUT,
                TOO_LARGE,
            )?;
            let root: Value = serde_json::from_slice(&bytes).map_err(|_| invalid_response())?;
            let reply = parse_response(provider, &root, active)?;
            if reply.calls.is_empty() && !reply.text.is_empty() {
                text(&reply.text)?;
            }
            return Ok(reply);
        }
        let mut parser = SseParser::new();
        let mut stream = ChatStream::new(active.is_empty());
        let mut last_progress: Option<Instant> = None;
        let mut buffer = [0u8; 8192];
        'read: loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(AssistantError::Cancelled);
            }
            let n = response
                .body
                .read(&mut buffer)
                .map_err(|e| read_error(&e, cancel, VOICE_TIMEOUT))?;
            if n == 0 {
                break;
            }
            let events = parser.feed(&buffer[..n]);
            for event in events {
                match parse_stream_event(Protocol::ChatCompletions, &event)? {
                    StreamStep::Done => break 'read,
                    StreamStep::Ignore => {}
                    StreamStep::Progress => {
                        let now = Instant::now();
                        if last_progress
                            .is_none_or(|t| now.duration_since(t) > Duration::from_secs(1))
                        {
                            last_progress = Some(now);
                            progress("Hermes is working with tools…");
                        }
                    }
                    StreamStep::Delta(delta) => {
                        if let Some(forward) = stream.push(&delta)? {
                            text(&forward)?;
                        }
                    }
                }
            }
            if let Some(error) = parser.error() {
                return Err(error);
            }
        }
        if let Some(error) = parser.error() {
            return Err(error);
        }
        let early = stream.early;
        let root = stream.into_response()?;
        let reply = parse_response(provider, &root, active)?;
        if !early && reply.calls.is_empty() && !reply.text.is_empty() {
            text(&reply.text)?;
        }
        Ok(reply)
    }

    /// List the model IDs a provider advertises (`Models`): up to 20 pages within 30 s.
    pub fn list_models(
        &self,
        provider: &Provider,
        key: Option<&str>,
        cancel: &AtomicBool,
    ) -> Result<Vec<String>> {
        validate_endpoint(&provider.base_url)?;
        let started = Instant::now();
        let limit = Duration::from_secs(30);
        let mut result: Vec<String> = Vec::new();
        let mut cursor = String::new();
        for _ in 0..20 {
            let elapsed = started.elapsed();
            if elapsed >= limit {
                return Err(AssistantError::Timeout(REQUEST_TIMEOUT.into()));
            }
            let request = models_request(provider, key, &cursor)?;
            let mut response = self.send(
                &request,
                limit - elapsed,
                cancel,
                REQUEST_TIMEOUT,
                CONNECT_FAILED,
            )?;
            if !response.is_success() {
                return Err(self.failure(&mut response, key, cancel));
            }
            let bytes = read_all(
                &mut response,
                4 * 1024 * 1024,
                cancel,
                REQUEST_TIMEOUT,
                TOO_LARGE,
            )?;
            let (ids, next) = parse_models_page(provider, &bytes)?;
            for id in ids {
                if !result.contains(&id) {
                    result.push(id);
                }
            }
            if result.len() > 5000 || next.is_empty() || next == cursor {
                break;
            }
            cursor = next;
        }
        if result.is_empty() {
            return Err(AssistantError::io(
                "The provider returned no models. Check your key and permissions, or enter a model ID manually.",
            ));
        }
        Ok(sort_models(result))
    }

    /// One short chat request that proves the key, address and model work
    /// (`TestConnection`). Tools, vision and image output are switched off for the check.
    pub fn check_connection(
        &self,
        provider: &Provider,
        key: Option<&str>,
        cancel: &AtomicBool,
    ) -> Result<()> {
        let mut candidate = provider.clone();
        candidate.tools = false;
        candidate.vision = false;
        candidate.image_output = false;
        let reply = self.complete(
            &candidate,
            key,
            &[Message::user(
                "Reply with OK only. This is a connection check.",
            )],
            &[],
            cancel,
        )?;
        if reply.text.trim().is_empty() {
            return Err(AssistantError::invalid(
                "The API responded but returned no text. Select a text/chat model.",
            ));
        }
        Ok(())
    }

    /// Send a generated red square and report whether the model named its color
    /// (`TestImageSupport`). The provider must have image input switched on.
    pub fn check_image_support(
        &self,
        provider: &Provider,
        key: Option<&str>,
        cancel: &AtomicBool,
    ) -> Result<bool> {
        if !provider.vision {
            return Err(AssistantError::invalid(
                "Enable Image input and Save provider first.",
            ));
        }
        let mut candidate = provider.clone();
        candidate.tools = false;
        candidate.image_output = false;
        let mut message = Message::user(
            "Name the dominant color of the attached image. Reply with one color word only.",
        );
        message.image_data_url = Some(super::persistence::image_test_data());
        let reply = self.complete(&candidate, key, &[message], &[], cancel)?;
        Ok(super::persistence::image_reply_recognized(&reply.text))
    }

    /// Fetch bytes of a provider-returned image (`DownloadMedia`). The credential is only sent to
    /// the provider that returned the image and only under its API path.
    pub fn download_media(
        &self,
        file: &Attachment,
        provider: Option<&Provider>,
        key: Option<&str>,
        cancel: &AtomicBool,
    ) -> Result<Vec<u8>> {
        let source = match &file.source {
            AttachmentSource::Inline(s) => return bytes_from_data_url(s),
            AttachmentSource::Remote(s) => s.as_str(),
            AttachmentSource::Path { .. } => return file.bytes(),
            AttachmentSource::None => {
                return Err(AssistantError::io("Invalid or oversized inline file."))
            }
        };
        let uri = media_uri(source, &file.origin)?;
        let mut headers = Vec::new();
        if let Some(provider) = provider {
            if provider.id == file.provider_id && provider.base_url == file.origin {
                let basis = validate_endpoint(&provider.base_url)?;
                let same_origin = basis.scheme() == uri.scheme()
                    && basis.host_str() == uri.host_str()
                    && basis.port_or_known_default() == uri.port_or_known_default();
                if same_origin && uri.path().starts_with(basis.path()) {
                    authenticate(&mut headers, provider, key)?;
                }
            }
        }
        validate_key(key)?;
        let request = HttpRequest {
            method: Method::Get,
            url: uri.to_string(),
            headers,
            body: Vec::new(),
        };
        let mut response = self.send(
            &request,
            Duration::from_secs(30),
            cancel,
            REQUEST_TIMEOUT,
            CONNECT_FAILED,
        )?;
        if !response.is_success() {
            return Err(AssistantError::io(format!(
                "The image could not be downloaded (HTTP {}).",
                response.status
            )));
        }
        read_all(
            &mut response,
            MAXIMUM_FILE_BYTES as usize,
            cancel,
            REQUEST_TIMEOUT,
            "The provider response exceeded the size limit.",
        )
    }

    /// Validate a conversation without sending it (used by the UI before enabling Send).
    pub fn validate(
        &self,
        provider: &Provider,
        messages: &[Message],
        tools: &[Tool],
    ) -> Result<()> {
        validate_endpoint(&provider.base_url)?;
        validate_input(provider, messages, tools)
    }
}

impl AssistantTransport for Transport {
    fn complete(
        &self,
        provider: &Provider,
        key: Option<&str>,
        messages: &[Message],
        tools: &[Tool],
        cancel: &AtomicBool,
    ) -> Result<Reply> {
        Transport::complete(self, provider, key, messages, tools, cancel)
    }

    fn complete_streaming(
        &self,
        provider: &Provider,
        key: Option<&str>,
        messages: &[Message],
        tools: &[Tool],
        text: &mut dyn FnMut(&str) -> Result<()>,
        progress: &mut dyn FnMut(&str),
        cancel: &AtomicBool,
    ) -> Result<Reply> {
        Transport::complete_streaming(self, provider, key, messages, tools, text, progress, cancel)
    }
}

#[cfg(test)]
mod tests;
