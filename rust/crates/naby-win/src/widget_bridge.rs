//! The widget-pages bridge for Hermes MCP (`AssistantWidgetBridge.cs`).
//!
//! Two halves:
//! * The running app serves a per-user named pipe (`\\.\pipe\NabyScape.WidgetPages.<SID>`,
//!   accessible only to that user, local clients only). Each connection carries one JSON line
//!   request and gets one JSON line answer from [`bridge_dispatch`]; only the two bounded
//!   widget tools exist, and a stalled client is cut off after 8 seconds.
//! * `nabyscape --widget-mcp` is a stdio MCP server ([`run_stdio`]) that forwards `tools/list`
//!   and `tools/call` to that pipe. It never launches the app and never executes anything else.
//!
//! The line framing and the request handling are pure and tested everywhere; the pipe itself is
//! Windows-only.

use std::io::{BufRead, Read, Write};

use naby_core::assistant::tools::{
    bridge_dispatch, mcp_handle_line, MAXIMUM_FORWARDED_BYTES, MAXIMUM_LINE_BYTES,
};
use naby_ui::assistant::ports::ToolRunner;
use serde_json::Value;

/// Why a line could not be read.
#[derive(Debug, PartialEq, Eq)]
pub enum LineError {
    TooLong,
    Closed,
    Invalid,
}

/// Reads one `\n` terminated line of at most [`MAXIMUM_LINE_BYTES`] bytes. A final line without
/// a newline is returned too; a closed stream with nothing read is [`LineError::Closed`].
pub fn read_line(reader: &mut dyn Read) -> Result<String, LineError> {
    let mut line: Vec<u8> = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        match reader.read(&mut byte) {
            Ok(0) => {
                if line.is_empty() {
                    return Err(LineError::Closed);
                }
                break;
            }
            Ok(_) => {
                if byte[0] == b'\n' {
                    break;
                }
                if line.len() >= MAXIMUM_LINE_BYTES {
                    return Err(LineError::TooLong);
                }
                line.push(byte[0]);
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return Err(LineError::Closed),
        }
    }
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    String::from_utf8(line).map_err(|_| LineError::Invalid)
}

/// The app side: one request line in, one answer line out.
pub fn serve_line(tools: &dyn ToolRunner, line: &str) -> String {
    let unsupported = || r#"{"ok":false,"error":"Unsupported widget bridge request."}"#.to_owned();
    let Ok(request) = serde_json::from_str::<Value>(line) else {
        return unsupported();
    };
    if depth(&request) > 12 {
        return unsupported();
    }
    let offered = tools.tools();
    bridge_dispatch(&request, &offered, &mut |call| tools.execute(call)).to_string()
}

fn depth(value: &Value) -> usize {
    match value {
        Value::Array(items) => 1 + items.iter().map(depth).max().unwrap_or(0),
        Value::Object(map) => 1 + map.values().map(depth).max().unwrap_or(0),
        _ => 0,
    }
}

/// The stdio MCP loop (`RunStdio`). `forward` sends one request to the running app.
pub fn run_stdio_with(
    input: &mut dyn BufRead,
    output: &mut dyn Write,
    version: &str,
    forward: &mut dyn FnMut(&Value) -> Result<Value, ()>,
) {
    // Requests with oversized tool arguments are refused before they reach the pipe.
    let mut guarded = |request: &Value| -> Result<Value, ()> {
        if request.to_string().len() > MAXIMUM_FORWARDED_BYTES {
            return Err(());
        }
        forward(request)
    };
    loop {
        let line = match read_line(input) {
            Ok(line) => line,
            Err(LineError::Invalid) => continue,
            Err(_) => break,
        };
        if let Some(answer) = mcp_handle_line(&line, version, &mut guarded) {
            if writeln!(output, "{answer}")
                .and_then(|()| output.flush())
                .is_err()
            {
                break;
            }
        }
    }
}

/// `nabyscape --widget-mcp`: serves MCP over stdio until the client closes it.
pub fn run_stdio() -> i32 {
    let version = env!("CARGO_PKG_VERSION");
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let mut output = std::io::stdout().lock();
    run_stdio_with(&mut input, &mut output, version, &mut |request| {
        forward(request)
    });
    0
}

#[cfg(windows)]
fn forward(request: &Value) -> Result<Value, ()> {
    os::forward(request)
}

#[cfg(not(windows))]
fn forward(_: &Value) -> Result<Value, ()> {
    Err(())
}

#[cfg(windows)]
pub use os::WidgetBridge;

#[cfg(windows)]
mod os {
    use super::*;
    use naby_core::assistant::tools::bridge_pipe_name;
    use std::os::windows::io::{FromRawHandle, RawHandle};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{channel, RecvTimeoutError};
    use std::sync::Arc;
    use std::thread::JoinHandle;
    use std::time::{Duration, Instant};
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::Foundation::{
        CloseHandle, LocalFree, ERROR_PIPE_CONNECTED, HANDLE, HLOCAL,
    };
    use windows::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
        SDDL_REVISION_1,
    };
    use windows::Win32::Security::{
        GetTokenInformation, TokenUser, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY,
        TOKEN_USER,
    };
    use windows::Win32::Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX};
    use windows::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE,
        PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    use windows::Win32::System::IO::CancelIoEx;

    const SERVE_DEADLINE: Duration = Duration::from_secs(8);
    const CONNECT_DEADLINE: Duration = Duration::from_millis(2500);

    /// The current user's SID as a string (`S-1-5-21-...`).
    fn user_sid() -> Option<String> {
        // SAFETY: standard token query. The buffer is sized by the first call, the SID string
        // returned by Windows is copied and then freed with LocalFree, and the token handle is
        // closed on every path.
        unsafe {
            let mut token = HANDLE::default();
            OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).ok()?;
            let mut needed = 0u32;
            let _ = GetTokenInformation(token, TokenUser, None, 0, &mut needed);
            let mut buffer = vec![0u8; needed as usize];
            let result = GetTokenInformation(
                token,
                TokenUser,
                Some(buffer.as_mut_ptr().cast()),
                needed,
                &mut needed,
            );
            let sid = if result.is_ok() {
                let user = &*(buffer.as_ptr().cast::<TOKEN_USER>());
                let mut text = PWSTR::null();
                ConvertSidToStringSidW(user.User.Sid, &mut text)
                    .ok()
                    .map(|()| {
                        let value = text.to_string().unwrap_or_default();
                        let _ = LocalFree(Some(HLOCAL(text.0.cast())));
                        value
                    })
            } else {
                None
            };
            let _ = CloseHandle(token);
            sid.filter(|s| !s.is_empty())
        }
    }

    fn pipe_path() -> Option<String> {
        Some(format!(r"\\.\pipe\{}", bridge_pipe_name(&user_sid()?)))
    }

    /// Creates the (single) server instance: only this user may connect, never remotely.
    fn create_instance(path: &str, sid: &str, first: bool) -> Option<HANDLE> {
        let sddl: Vec<u16> = format!("D:P(A;;GA;;;{sid})")
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let name: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: the security descriptor is allocated by Windows, used only for the
        // CreateNamedPipeW call (which copies it) and freed right after; `name` and `sddl` are
        // NUL-terminated and outlive the calls.
        unsafe {
            let mut descriptor = PSECURITY_DESCRIPTOR::default();
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(sddl.as_ptr()),
                SDDL_REVISION_1,
                &mut descriptor,
                None,
            )
            .ok()?;
            let attributes = SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: descriptor.0,
                bInheritHandle: false.into(),
            };
            let mode = if first {
                PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE
            } else {
                PIPE_ACCESS_DUPLEX
            };
            let handle = CreateNamedPipeW(
                PCWSTR(name.as_ptr()),
                mode,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                16384,
                16384,
                0,
                Some(&attributes),
            );
            let _ = LocalFree(Some(HLOCAL(descriptor.0)));
            (!handle.is_invalid()).then_some(handle)
        }
    }

    /// Serves one connection: read a line, answer, hang up. A client that stalls is cancelled.
    fn serve(pipe: HANDLE, tools: &dyn ToolRunner) {
        let (done_tx, done_rx) = channel::<()>();
        let raw = pipe.0 as isize;
        let watchdog = std::thread::spawn(move || {
            if let Err(RecvTimeoutError::Timeout) = done_rx.recv_timeout(SERVE_DEADLINE) {
                // SAFETY: cancels the blocked synchronous I/O on a handle that stays valid
                // until `serve` joins this thread.
                unsafe {
                    let _ = CancelIoEx(HANDLE(raw as *mut _), None);
                }
            }
        });
        // SAFETY: `pipe` is a valid handle this function owns; the File takes ownership and
        // closes it. It is wrapped in ManuallyDrop-free fashion: after `file` drops, the
        // handle is closed exactly once.
        let mut file = unsafe { std::fs::File::from_raw_handle(pipe.0 as RawHandle) };
        if let Ok(line) = read_line(&mut file) {
            let answer = serve_line(tools, &line);
            let _ = writeln!(file, "{answer}");
            let _ = file.flush();
        }
        // SAFETY: the pipe is still open (owned by `file`); disconnect before closing.
        unsafe {
            let _ = DisconnectNamedPipe(HANDLE(file_handle(&file)));
        }
        drop(done_tx);
        drop(file);
        let _ = watchdog.join();
    }

    fn file_handle(file: &std::fs::File) -> *mut std::ffi::c_void {
        use std::os::windows::io::AsRawHandle;
        file.as_raw_handle()
    }

    /// The running server. Dropping it stops the thread.
    pub struct WidgetBridge {
        stop: Arc<AtomicBool>,
        path: Option<String>,
        thread: Option<JoinHandle<()>>,
    }

    impl WidgetBridge {
        /// Starts serving `tools`; `None` when the pipe could not be created (another copy is
        /// serving it, or the user identity is unavailable).
        pub fn start(tools: Arc<dyn ToolRunner>) -> Option<WidgetBridge> {
            let sid = user_sid()?;
            let path = format!(r"\\.\pipe\{}", bridge_pipe_name(&sid));
            let first = create_instance(&path, &sid, true)?;
            let stop = Arc::new(AtomicBool::new(false));
            let flag = stop.clone();
            let thread_path = path.clone();
            let handle = first.0 as isize;
            let thread = std::thread::Builder::new()
                .name("NabyScape widget bridge".into())
                .spawn(move || {
                    let mut next = Some(HANDLE(handle as *mut _));
                    while !flag.load(Ordering::SeqCst) {
                        let pipe = match next.take() {
                            Some(pipe) => pipe,
                            None => match create_instance(&thread_path, &sid, false) {
                                Some(pipe) => pipe,
                                None => {
                                    std::thread::sleep(Duration::from_millis(150));
                                    continue;
                                }
                            },
                        };
                        // SAFETY: waits for a client on a pipe instance this thread owns.
                        let connected = unsafe { ConnectNamedPipe(pipe, None) };
                        let ok = connected.is_ok()
                            // SAFETY: reads the calling thread's last error after the call.
                            || unsafe { windows::Win32::Foundation::GetLastError() } == ERROR_PIPE_CONNECTED;
                        if flag.load(Ordering::SeqCst) || !ok {
                            // SAFETY: closes the unused instance exactly once.
                            unsafe {
                                let _ = CloseHandle(pipe);
                            }
                            if !ok && !flag.load(Ordering::SeqCst) {
                                std::thread::sleep(Duration::from_millis(150));
                            }
                            continue;
                        }
                        serve(pipe, &*tools);
                    }
                })
                .ok()?;
            Some(WidgetBridge {
                stop,
                path: Some(path),
                thread: Some(thread),
            })
        }
    }

    impl Drop for WidgetBridge {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            // Wake the server out of ConnectNamedPipe with a throwaway client.
            if let Some(path) = self.path.take() {
                let _ = std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(path);
            }
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    /// The client side: one request line, one answer line (`Request`).
    pub fn forward(request: &Value) -> Result<Value, ()> {
        let payload = request.to_string();
        if payload.len() > MAXIMUM_FORWARDED_BYTES {
            return Err(());
        }
        let path = pipe_path().ok_or(())?;
        let started = Instant::now();
        let mut pipe = loop {
            match std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
            {
                Ok(file) => break file,
                Err(_) if started.elapsed() < CONNECT_DEADLINE => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(_) => return Err(()),
            }
        };
        writeln!(pipe, "{payload}").map_err(|_| ())?;
        pipe.flush().map_err(|_| ())?;
        let line = read_line(&mut pipe).map_err(|_| ())?;
        serde_json::from_str(&line).map_err(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use naby_core::command::Command;
    use naby_ui::assistant::ports::CommandQueue;
    use naby_ui::assistant::widget_tools::WidgetTools;
    use std::io::Cursor;
    use std::sync::Arc;

    fn tools() -> (WidgetTools, CommandQueue) {
        let queue = CommandQueue::new(Arc::new(|| {}));
        let tools = WidgetTools::new(queue.clone());
        let mut settings = naby_core::settings::presets::recommended();
        for w in &mut settings.widgets {
            w.enabled = matches!(w.id.as_str(), "media" | "timer" | "notes");
        }
        tools.update(&settings, true, true);
        (tools, queue)
    }

    /// Both halves wired together in memory: the "pipe" calls `serve_line` directly.
    fn session(lines: &[&str], tools: &WidgetTools) -> Vec<Value> {
        let input = lines.join("\n") + "\n";
        let mut output = Vec::new();
        run_stdio_with(
            &mut Cursor::new(input.into_bytes()),
            &mut output,
            "3.0.0",
            &mut |request| {
                let answer = serve_line(tools, &request.to_string());
                serde_json::from_str(&answer).map_err(|_| ())
            },
        );
        String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    #[test]
    fn hermes_can_list_tools_and_navigate_through_the_bridge() {
        let (tools, queue) = tools();
        let answers = session(
            &[
                r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}"#,
                r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
                r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
                r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"naby_navigate_widget","arguments":{"page":"timer"}}}"#,
                r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"naby_navigate_widget","arguments":{"page":"weather"}}}"#,
                r#"{"jsonrpc":"2.0","id":5,"method":"ping"}"#,
            ],
            &tools,
        );
        assert_eq!(answers.len(), 5, "notifications get no answer");
        assert_eq!(answers[0]["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(answers[0]["result"]["serverInfo"]["name"], "nabyscape");
        let listed: Vec<&str> = answers[1]["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(listed, ["naby_list_widget_pages", "naby_navigate_widget"]);
        assert_eq!(
            answers[1]["result"]["tools"][0]["annotations"]["readOnlyHint"],
            true
        );
        assert_eq!(answers[2]["result"]["isError"], false);
        assert_eq!(queue.drain(), vec![Command::NavigatePage("timer".into())]);
        assert_eq!(
            answers[3]["result"]["isError"], true,
            "a disabled page is an error result"
        );
        assert!(queue.drain().is_empty());
        assert_eq!(answers[4]["result"], serde_json::json!({}));
    }

    #[test]
    fn the_app_side_refuses_anything_but_the_two_tools() {
        let (tools, queue) = tools();
        let unsupported = |line: &str| serve_line(&tools, line);
        assert!(unsupported("not json").contains("Unsupported"));
        assert!(unsupported(r#"{"operation":"shell"}"#).contains("Unsupported"));
        assert!(
            unsupported(r#"{"operation":"call","name":"naby_navigate_widget"}"#)
                .contains("Unsupported")
        );
        let reply: Value = serde_json::from_str(&unsupported(
            r#"{"operation":"call","name":"format_disk","arguments":{}}"#,
        ))
        .unwrap();
        assert_eq!(reply["ok"], false);
        assert!(queue.drain().is_empty());
        let deep = format!("{}1{}", "[".repeat(40), "]".repeat(40));
        assert!(
            unsupported(&format!(r#"{{"operation":"tools","x":{deep}}}"#)).contains("Unsupported")
        );
    }

    #[test]
    fn lines_are_bounded_and_crlf_tolerant() {
        assert_eq!(
            read_line(&mut Cursor::new(b"abc\r\ndef".to_vec())).unwrap(),
            "abc"
        );
        let mut two = Cursor::new(b"one\ntwo".to_vec());
        assert_eq!(read_line(&mut two).unwrap(), "one");
        assert_eq!(read_line(&mut two).unwrap(), "two");
        assert_eq!(read_line(&mut two), Err(LineError::Closed));
        let long = vec![b'x'; MAXIMUM_LINE_BYTES + 5];
        assert_eq!(read_line(&mut Cursor::new(long)), Err(LineError::TooLong));
        assert_eq!(
            read_line(&mut Cursor::new(vec![0xFF, 0xFE, b'\n'])),
            Err(LineError::Invalid)
        );
    }

    #[test]
    fn oversized_arguments_never_reach_the_pipe() {
        let (tools, queue) = tools();
        let big = "a".repeat(9000);
        let line = format!(
            r#"{{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{{"name":"naby_navigate_widget","arguments":{{"page":"{big}"}}}}}}"#
        );
        let answers = session(&[line.as_str()], &tools);
        assert_eq!(answers[0]["result"]["isError"], true);
        assert!(queue.drain().is_empty());
    }

    #[test]
    fn when_the_app_is_not_running_the_client_explains_how_to_start_it() {
        let input = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#.to_owned() + "\n";
        let mut output = Vec::new();
        run_stdio_with(
            &mut Cursor::new(input.into_bytes()),
            &mut output,
            "3.0.0",
            &mut |_| Err(()),
        );
        let answer: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(answer["error"]["code"], -32602);
    }
}
