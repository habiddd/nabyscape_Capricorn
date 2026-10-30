//! Local speech engines (`AssistantLocalVoice`, `AssistantManagedSpeech`, `AssistantSpeechWorker`):
//! transcribers and speakers behind the pipeline's traits.
//!
//! Listening: Whistle (in [`super::whistle`]), an external server (`whisper.cpp` `/inference` or
//! OpenAI-compatible `/audio/transcriptions`), a managed `whisper-server.exe`, or a Sherpa
//! executable for Moonshine/Qwen packs. Speaking: Windows speech synthesis, an external
//! `/audio/speech` server, Piper, or a private Sherpa worker process (`--speech-worker`) that
//! keeps one native voice model loaded for the session and speaks over its own stdin/stdout
//! (no network port). All addresses must be loopback.
//!
//! The optional ASR bridge script `tools/local_voice_server.py` is embedded as before and can be
//! saved from the Voice setup page.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use naby_core::assistant::models::LocalVoiceOptions;
use naby_core::assistant::speech::{local_endpoint, TtsModel};
use naby_core::assistant::storage::SpeechStorage;
use serde_json::{json, Value};
use url::Url;

use super::dsp::{f32_to_pcm16, to_playback_pcm, wav_decode, wav_encode};
use super::pipeline::Speaker;
use super::whistle::{Transcriber, Transcript};

/// `tools/local_voice_server.py`, embedded as in the C# build (`NabyScape.LocalAsrServer`).
pub const ASR_SERVER_SCRIPT: &str = include_str!("../../../../../tools/local_voice_server.py");
/// The default file name the setup page suggests when saving the script.
pub const ASR_SERVER_FILE: &str = "naby-local-asr.py";

const MAX_TRANSCRIPT_RESPONSE: usize = 65_536;
const MAX_TTS_WAV: u64 = 12 * 1024 * 1024;
pub const MAX_SEGMENT_CHARS: usize = 2000;

// ----- multipart and HTTP -------------------------------------------------------------------------

/// A `multipart/form-data` body with one file part and text fields.
pub fn multipart(
    boundary: &str,
    file_name: &str,
    mime: &str,
    file: &[u8],
    fields: &[(&str, &str)],
) -> Vec<u8> {
    let mut body = Vec::with_capacity(file.len() + 512);
    for (name, value) in fields {
        body.extend_from_slice(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n").as_bytes(),
        );
    }
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{file_name}\"\r\nContent-Type: {mime}\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(file);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}

fn agent() -> ureq::Agent {
    // No proxy, no redirects: local services only.
    ureq::AgentBuilder::new()
        .redirects(0)
        .timeout_connect(std::time::Duration::from_secs(5))
        .timeout(std::time::Duration::from_secs(120))
        .user_agent(naby_core::http::USER_AGENT)
        .build()
}

fn http_failure(error: ureq::Error, what: &str) -> String {
    match error {
        ureq::Error::Status(code, _) => format!("{what} answered with HTTP {code}."),
        ureq::Error::Transport(_) => {
            "Local speech service is unavailable. Check its port in Voice settings, then start voice again.".to_owned()
        }
    }
}

fn read_bounded(response: ureq::Response, limit: u64) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    response
        .into_reader()
        .take(limit + 1)
        .read_to_end(&mut out)
        .map_err(|_| "The speech service closed the connection.".to_owned())?;
    if out.len() as u64 > limit {
        return Err("The speech service reply exceeded its limit.".into());
    }
    Ok(out)
}

/// Joins an endpoint base and a relative path (`new Uri(base, "inference")`).
pub fn join(base: &Url, relative: &str) -> Result<Url, String> {
    let mut base = base.clone();
    if !base.path().ends_with('/') {
        let path = format!("{}/", base.path());
        base.set_path(&path);
    }
    base.join(relative)
        .map_err(|_| "Enter a valid local speech address.".to_owned())
}

/// An external or managed ASR server.
pub struct HttpAsr {
    base: Url,
    whisper_cpp: bool,
    model: String,
}

impl HttpAsr {
    /// `protocol` is `WhisperCpp` or anything else for the OpenAI-compatible route.
    pub fn new(url: &str, protocol: &str, model: &str) -> Result<Self, String> {
        let base = local_endpoint(url).map_err(|e| e.message().to_owned())?;
        Ok(Self {
            base,
            whisper_cpp: protocol == "WhisperCpp",
            model: model.to_owned(),
        })
    }

    pub fn from_options(local: &LocalVoiceOptions) -> Result<Self, String> {
        Self::new(&local.asr_url, &local.asr_protocol, &local.asr_model)
    }

    /// Whether the server answers `GET health` (used while a managed server starts).
    pub fn healthy(&self) -> bool {
        join(&self.base, "health")
            .ok()
            .and_then(|u| {
                agent()
                    .get(u.as_str())
                    .timeout(std::time::Duration::from_secs(1))
                    .call()
                    .ok()
            })
            .is_some_and(|r| r.status() == 200)
    }
}

/// The recognized text of an ASR JSON reply.
pub fn parse_asr_reply(bytes: &[u8]) -> Result<String, String> {
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|_| "The speech service returned an unreadable reply.".to_owned())?;
    Ok(value
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_owned())
}

impl Transcriber for HttpAsr {
    fn transcribe(
        &mut self,
        pcm: &[f32],
        _: Option<&str>,
        _: &[String],
    ) -> Result<Transcript, String> {
        let wav = wav_encode(16_000, &f32_to_pcm16(pcm));
        let boundary = format!("naby{}", naby_core::assistant::new_id());
        let mut fields = vec![("response_format", "json")];
        if self.whisper_cpp {
            fields.push(("language", "auto"));
        } else {
            fields.push(("model", self.model.as_str()));
        }
        let body = multipart(&boundary, "voice.wav", "audio/wav", &wav, &fields);
        let url = join(
            &self.base,
            if self.whisper_cpp {
                "inference"
            } else {
                "audio/transcriptions"
            },
        )?;
        let response = agent()
            .post(url.as_str())
            .set(
                "Content-Type",
                &format!("multipart/form-data; boundary={boundary}"),
            )
            .send_bytes(&body)
            .map_err(|e| http_failure(e, "Local ASR"))?;
        let text = parse_asr_reply(&read_bounded(response, MAX_TRANSCRIPT_RESPONSE as u64)?)?;
        Ok(Transcript {
            text,
            ..Transcript::default()
        })
    }
}

/// An external `/audio/speech` server (Kokoro-FastAPI and compatibles).
pub struct HttpTts {
    base: Url,
    model: String,
    voice: String,
}

impl HttpTts {
    pub fn from_options(local: &LocalVoiceOptions) -> Result<Self, String> {
        Ok(Self {
            base: local_endpoint(&local.tts_url).map_err(|e| e.message().to_owned())?,
            model: local.tts_model.clone(),
            voice: local.voice.clone(),
        })
    }
}

impl Speaker for HttpTts {
    fn synthesize(&mut self, text: &str, cancel: &AtomicBool) -> Result<Vec<u8>, String> {
        if cancel.load(Ordering::SeqCst) {
            return Ok(Vec::new());
        }
        let body = json!({"model": self.model, "input": text, "voice": self.voice, "response_format": "wav", "stream": false});
        let response = agent()
            .post(join(&self.base, "audio/speech")?.as_str())
            .send_json(body)
            .map_err(|e| http_failure(e, "Local TTS"))?;
        let wav = read_bounded(response, MAX_TTS_WAV)?;
        to_playback_pcm(&wav_decode(&wav)?)
    }
}

// ----- the private speech worker protocol ------------------------------------------------------------

/// Frames are a little-endian `i32` length and the payload.
pub fn write_frame<W: Write + ?Sized>(out: &mut W, payload: &[u8]) -> std::io::Result<()> {
    out.write_all(&(payload.len() as i32).to_le_bytes())?;
    out.write_all(payload)?;
    out.flush()
}

pub fn read_frame(input: &mut impl Read, maximum: usize) -> std::io::Result<Vec<u8>> {
    let mut size = [0u8; 4];
    input.read_exact(&mut size)?;
    let length = i32::from_le_bytes(size);
    if length < 0 || length as usize > maximum {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Invalid voice-worker frame size.",
        ));
    }
    let mut bytes = vec![0u8; length as usize];
    input.read_exact(&mut bytes)?;
    Ok(bytes)
}

/// What the worker is asked to load (the C# `Setup` record).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct WorkerSetup {
    pub library: String,
    pub folder: String,
    pub engine: String,
    pub language: String,
    pub speaker: i32,
    pub steps: i32,
}

/// Diffusion steps for `Supertonic` by speech preference (`12 / 8 / 4`).
pub fn worker_steps(preference: &str) -> i32 {
    match preference {
        "Quality" => 12,
        "Light" => 4,
        _ => 8,
    }
}

/// Replies are a status byte (0 = ok) then the WAV audio.
pub fn worker_reply(bytes: Vec<u8>) -> Result<Vec<u8>, String> {
    match bytes.split_first() {
        None => Err("The voice worker returned an empty reply.".into()),
        Some((0, rest)) => Ok(rest.to_vec()),
        Some(_) => Err(
            "Local voice generation failed. Check your voice pack or choose Windows voice.".into(),
        ),
    }
}

/// A child `nabyscape.exe --speech-worker` holding one loaded native voice.
pub struct WorkerTts {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    stdout: std::process::ChildStdout,
}

impl WorkerTts {
    pub fn start(setup: &WorkerSetup, job: Option<&Job>) -> Result<Self, String> {
        let exe =
            std::env::current_exe().map_err(|_| "The app executable is unavailable.".to_owned())?;
        let mut command = std::process::Command::new(exe);
        command
            .arg("--speech-worker")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        let mut child = command
            .spawn()
            .map_err(|_| "Could not load the persistent voice engine. Prepare the selected pack again, or choose Windows voice.".to_owned())?;
        if let Some(job) = job {
            job.assign(&child)?;
        }
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            let _ = child.kill();
            return Err("Could not load the persistent voice engine.".into());
        };
        let mut worker = Self {
            child,
            stdin,
            stdout,
        };
        let setup_bytes = serde_json::to_vec(setup).map_err(|e| e.to_string())?;
        let init = write_frame(&mut worker.stdin, &setup_bytes)
            .and_then(|()| read_frame(&mut worker.stdout, 12 * 1024 * 1024 + 1))
            .map_err(|_| "Could not load the persistent voice engine. Prepare the selected pack again, or choose Windows voice.".to_owned())
            .and_then(worker_reply);
        init.map(|_| worker)
    }
}

impl Speaker for WorkerTts {
    fn synthesize(&mut self, text: &str, cancel: &AtomicBool) -> Result<Vec<u8>, String> {
        if cancel.load(Ordering::SeqCst) {
            return Ok(Vec::new());
        }
        if text.chars().count() > MAX_SEGMENT_CHARS {
            return Err("The voice segment is empty or too long.".into());
        }
        write_frame(&mut self.stdin, text.as_bytes())
            .and_then(|()| read_frame(&mut self.stdout, 12 * 1024 * 1024 + 1))
            .map_err(|_| {
                "Local voice generation failed. Check your voice pack or choose Windows voice."
                    .to_owned()
            })
            .and_then(worker_reply)
            .and_then(|wav| to_playback_pcm(&wav_decode(&wav)?))
    }
}

impl Drop for WorkerTts {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// ----- engines run as child processes ---------------------------------------------------------------------

/// A Windows job object: children assigned to it die when it closes (`KILL_ON_JOB_CLOSE`).
pub struct Job {
    #[cfg(windows)]
    handle: windows::Win32::Foundation::HANDLE,
}

// SAFETY: a job handle is a kernel object handle usable from any thread.
unsafe impl Send for Job {}
// SAFETY: see above; the handle is only used for assignment and closed once in Drop.
unsafe impl Sync for Job {}

impl Job {
    #[cfg(windows)]
    pub fn new() -> Result<Self, String> {
        use windows::Win32::System::JobObjects::{
            CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        };
        // SAFETY: creates a job object and configures it with a fully initialized struct.
        unsafe {
            let handle = CreateJobObjectW(None, None)
                .map_err(|_| "Could not create a managed speech session.".to_owned())?;
            let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
            .map_err(|_| "Could not configure speech-process cleanup.".to_owned())?;
            Ok(Self { handle })
        }
    }

    #[cfg(not(windows))]
    pub fn new() -> Result<Self, String> {
        Ok(Self {})
    }

    #[cfg(windows)]
    pub fn assign(&self, child: &std::process::Child) -> Result<(), String> {
        use std::os::windows::io::AsRawHandle;
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::System::JobObjects::AssignProcessToJobObject;
        // SAFETY: both handles are valid for the call.
        unsafe { AssignProcessToJobObject(self.handle, HANDLE(child.as_raw_handle())) }
            .map_err(|_| "Could not attach speech engine cleanup.".to_owned())
    }

    #[cfg(not(windows))]
    pub fn assign(&self, _: &std::process::Child) -> Result<(), String> {
        Ok(())
    }
}

#[cfg(windows)]
impl Drop for Job {
    fn drop(&mut self) {
        // SAFETY: closing the handle created in `new`; the OS then ends every child.
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(self.handle);
        }
    }
}

fn native_command(exe: &Path) -> std::process::Command {
    let mut command = std::process::Command::new(exe);
    if let Some(dir) = exe.parent() {
        command.current_dir(dir);
    }
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    command
}

/// Threads for model inference: half the cores, 1 to 8 (`Math.Clamp(ProcessorCount/2,1,8)`).
pub fn inference_threads() -> usize {
    (std::thread::available_parallelism().map_or(2, |n| n.get()) / 2).clamp(1, 8)
}

/// A managed `whisper-server.exe` bound to a random loopback port and a secret route.
pub struct ManagedWhisper {
    child: Option<std::process::Child>,
    asr: Option<HttpAsr>,
}

impl ManagedWhisper {
    /// Starts the server and waits until `/health` answers (up to 120 s).
    pub fn start(
        exe: &Path,
        model: &Path,
        gpu: bool,
        job: &Job,
        cancel: &AtomicBool,
    ) -> Result<Self, String> {
        let port = {
            let listener =
                std::net::TcpListener::bind(("127.0.0.1", 0)).map_err(|e| e.to_string())?;
            listener.local_addr().map_err(|e| e.to_string())?.port()
        };
        let route = format!("/{}", naby_core::assistant::new_id());
        let mut command = native_command(exe);
        command
            .args(["-m"])
            .arg(model)
            .args(["--host", "127.0.0.1", "--port"])
            .arg(port.to_string())
            .args(["--request-path", &route, "-l", "auto", "-t"])
            .arg(inference_threads().to_string());
        if !gpu {
            command.arg("-ng");
        }
        let mut child = command
            .spawn()
            .map_err(|_| "Could not start the installed speech engine.".to_owned())?;
        job.assign(&child)?;
        let url = format!("http://127.0.0.1:{port}{route}/");
        let asr = HttpAsr::new(&url, "WhisperCpp", "")?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
        loop {
            if cancel.load(Ordering::SeqCst) {
                let _ = child.kill();
                return Err("Cancelled.".into());
            }
            if let Ok(Some(status)) = child.try_wait() {
                return Err(format!(
                    "Local speech engine stopped ({}). Reinstall the speech pack; the Microsoft Visual C++ x64 runtime may be required.",
                    status.code().unwrap_or(-1)
                ));
            }
            if asr.healthy() {
                return Ok(Self {
                    child: Some(child),
                    asr: Some(asr),
                });
            }
            if std::time::Instant::now() > deadline {
                let _ = child.kill();
                return Err("The local listening model took too long to load. Choose a smaller model or check the speech pack.".into());
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
    }
}

impl Transcriber for ManagedWhisper {
    fn transcribe(
        &mut self,
        pcm: &[f32],
        language: Option<&str>,
        keywords: &[String],
    ) -> Result<Transcript, String> {
        self.asr
            .as_mut()
            .ok_or_else(|| "The listening engine is not running.".to_owned())?
            .transcribe(pcm, language, keywords)
    }
}

impl Drop for ManagedWhisper {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Command-line arguments for `sherpa-onnx-offline.exe` on one utterance.
pub fn sherpa_args(
    storage: &SpeechStorage,
    model: &naby_core::assistant::speech::AsrModel,
    wav: &Path,
) -> Result<Vec<String>, String> {
    let folder = storage.qwen_root(model);
    let p = |name: &str| folder.join(name).display().to_string();
    let mut args = Vec::new();
    if model.engine == "moonshine" {
        let file = |name: &str| {
            naby_core::assistant::speech::moonshine_file(&folder, name)
                .map(|f| f.display().to_string())
                .map_err(|e| e.message().to_owned())
        };
        args.push(format!("--moonshine-encoder={}", file("encoder_model")?));
        args.push(format!(
            "--moonshine-merged-decoder={}",
            file("decoder_model_merged")?
        ));
        args.push(format!("--tokens={}", p("tokens.txt")));
    } else {
        args.push(format!(
            "--qwen3-asr-conv-frontend={}",
            p("conv_frontend.onnx")
        ));
        args.push(format!("--qwen3-asr-encoder={}", p("encoder.int8.onnx")));
        args.push(format!("--qwen3-asr-decoder={}", p("decoder.int8.onnx")));
        args.push(format!("--qwen3-asr-tokenizer={}", p("tokenizer")));
    }
    args.push(format!("--num-threads={}", inference_threads()));
    args.push("--".into());
    args.push(wav.display().to_string());
    Ok(args)
}

/// The text of the JSON object Sherpa prints among its logs.
pub fn parse_sherpa_output(output: &str, model_name: &str) -> Result<String, String> {
    let (Some(first), Some(last)) = (output.find('{'), output.rfind('}')) else {
        return Err(format!(
            "{model_name} returned no transcript. Try speaking again or choose another pack."
        ));
    };
    if last < first {
        return Err(format!(
            "{model_name} returned no transcript. Try speaking again or choose another pack."
        ));
    }
    parse_asr_reply(&output.as_bytes()[first..=last])
}

/// Moonshine and Qwen packs: one `sherpa-onnx-offline.exe` run per utterance (the model loads
/// and unloads with the process).
pub struct SherpaAsr {
    pub exe: PathBuf,
    pub storage: std::sync::Arc<SpeechStorage>,
    pub model: naby_core::assistant::speech::AsrModel,
    pub job: std::sync::Arc<Job>,
}

impl Transcriber for SherpaAsr {
    fn transcribe(
        &mut self,
        pcm: &[f32],
        _: Option<&str>,
        _: &[String],
    ) -> Result<Transcript, String> {
        let dir = std::env::temp_dir().join(format!(
            "NabyScape-speech-{}",
            naby_core::assistant::new_id()
        ));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let result = (|| {
            let file = dir.join("utterance.wav");
            std::fs::write(&file, wav_encode(16_000, &f32_to_pcm16(pcm)))
                .map_err(|e| e.to_string())?;
            let args = sherpa_args(&self.storage, &self.model, &file)?;
            let mut child = native_command(&self.exe)
                .args(args)
                .spawn()
                .map_err(|_| "Could not start the installed speech engine.".to_owned())?;
            self.job.assign(&child)?;
            let mut stdout = child.stdout.take().ok_or("No output.")?;
            let mut stderr = child.stderr.take();
            let drain = std::thread::spawn(move || {
                let mut text = String::new();
                if let Some(e) = stderr.as_mut() {
                    let _ = e.take(128 * 1024).read_to_string(&mut text);
                }
            });
            let mut output = String::new();
            let _ = (&mut stdout)
                .take(128 * 1024 + 1)
                .read_to_string(&mut output);
            let status = child.wait().map_err(|e| e.to_string())?;
            let _ = drain.join();
            if output.len() > 128 * 1024 {
                return Err(format!("{} output exceeded its limit.", self.model.name));
            }
            if !status.success() {
                return Err(format!(
                    "{} could not complete inference (exit {}). Reinstall the pack or choose a smaller model.",
                    self.model.name,
                    status.code().unwrap_or(-1)
                ));
            }
            parse_sherpa_output(&output, &self.model.name)
        })();
        let _ = std::fs::remove_dir_all(&dir);
        result.map(|text| Transcript {
            text,
            ..Transcript::default()
        })
    }
}

/// Piper CLI: one process per sentence, raw 16-bit output at the voice's own sample rate.
pub struct PiperTts {
    pub exe: PathBuf,
    pub model: PathBuf,
    pub speaker: i32,
    /// Piper's `--length_scale`: above 1.0 speaks slower, below 1.0 faster.
    pub length_scale: f32,
    pub job: std::sync::Arc<Job>,
}

/// `audio.sample_rate` of a Piper voice config.
pub fn piper_sample_rate(config: &[u8]) -> Result<u32, String> {
    let value: Value = serde_json::from_slice(config)
        .map_err(|_| "The installed voice has an unsupported audio format.".to_owned())?;
    let rate = value["audio"]["sample_rate"].as_u64().unwrap_or(0) as u32;
    if (8000..=48000).contains(&rate) {
        Ok(rate)
    } else {
        Err("The installed voice has an unsupported audio format.".into())
    }
}

impl Speaker for PiperTts {
    fn synthesize(&mut self, text: &str, cancel: &AtomicBool) -> Result<Vec<u8>, String> {
        if cancel.load(Ordering::SeqCst) {
            return Ok(Vec::new());
        }
        let mut command = native_command(&self.exe);
        command
            .arg("--model")
            .arg(&self.model)
            .arg("--output-raw")
            .arg("--quiet");
        if (self.length_scale - 1.0).abs() > 0.01 {
            command
                .arg("--length_scale")
                .arg(format!("{:.2}", self.length_scale));
        }
        if self.speaker > 0 {
            command.arg("--speaker").arg(self.speaker.to_string());
        }
        command.stdin(std::process::Stdio::piped());
        let mut child = command
            .spawn()
            .map_err(|_| "Could not start the installed speech engine.".to_owned())?;
        self.job.assign(&child)?;
        let mut stdin = child.stdin.take().ok_or("No input.")?;
        let flat = text.replace(['\n', '\r'], " ");
        let _ = writeln!(stdin, "{flat}");
        drop(stdin);
        let mut stdout = child.stdout.take().ok_or("No output.")?;
        let mut stderr = child.stderr.take();
        let drain = std::thread::spawn(move || {
            if let Some(e) = stderr.as_mut() {
                let _ = std::io::copy(e, &mut std::io::sink());
            }
        });
        let mut audio = Vec::new();
        let _ = (&mut stdout)
            .take(48_000 * 2 * 120 + 1)
            .read_to_end(&mut audio);
        let status = child.wait().map_err(|e| e.to_string())?;
        let _ = drain.join();
        if !status.success() || audio.is_empty() || audio.len() > 48_000 * 2 * 120 {
            return Err("The installed Piper voice could not synthesize speech. Reinstall the selected voice or choose Windows voice.".into());
        }
        let config =
            std::fs::read(format!("{}.json", self.model.display())).map_err(|e| e.to_string())?;
        let rate = piper_sample_rate(&config)?;
        to_playback_pcm(&wav_decode(&wav_encode(rate, &audio))?)
    }
}

/// The voice a Sherpa worker loads for a catalog voice.
pub fn worker_setup(
    storage: &SpeechStorage,
    model: &TtsModel,
    local: &LocalVoiceOptions,
) -> WorkerSetup {
    WorkerSetup {
        library: storage
            .sherpa_root()
            .join(naby_core::assistant::speech::SHERPA_FOLDER)
            .join("lib")
            .join("sherpa-onnx-c-api.dll")
            .display()
            .to_string(),
        folder: super::install::tts_directory(storage, model)
            .display()
            .to_string(),
        engine: model.engine.clone(),
        language: local.speech_language.clone(),
        speaker: model.speaker,
        steps: worker_steps(&local.speech_preference),
    }
}

// ----- Windows speech synthesis ----------------------------------------------------------------------------

/// `Windows.Media.SpeechSynthesis` (the default voice, or one matching the speech language).
#[cfg(windows)]
pub struct WindowsTts {
    language: String,
}

#[cfg(windows)]
impl WindowsTts {
    pub fn new(language: &str) -> Self {
        Self {
            language: language.to_owned(),
        }
    }
}

#[cfg(windows)]
impl Speaker for WindowsTts {
    fn synthesize(&mut self, text: &str, cancel: &AtomicBool) -> Result<Vec<u8>, String> {
        use windows::core::HSTRING;
        use windows::Media::SpeechSynthesis::SpeechSynthesizer;
        use windows::Storage::Streams::DataReader;
        if cancel.load(Ordering::SeqCst) {
            return Ok(Vec::new());
        }
        let _com = crate::devices::ComApartment::mta();
        let fail = |_| "No Windows speech engine is installed.".to_owned();
        let synth = SpeechSynthesizer::new().map_err(fail)?;
        if let Ok(voices) = SpeechSynthesizer::AllVoices() {
            let wanted = self.language.to_ascii_lowercase();
            for voice in voices {
                let tag = voice
                    .Language()
                    .map(|l| l.to_string().to_ascii_lowercase())
                    .unwrap_or_default();
                if !wanted.is_empty() && tag.split('-').next() == Some(wanted.as_str()) {
                    let _ = synth.SetVoice(&voice);
                    break;
                }
            }
        }
        let stream = synth
            .SynthesizeTextToStreamAsync(&HSTRING::from(text))
            .and_then(|op| op.get())
            .map_err(|_| {
                "Windows could not synthesize speech with the installed voice.".to_owned()
            })?;
        let size = stream.Size().map_err(|e| e.to_string())? as u32;
        if u64::from(size) > MAX_TTS_WAV {
            return Err("The speech reply exceeded its limit.".into());
        }
        let reader =
            DataReader::CreateDataReader(&stream.GetInputStreamAt(0).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        reader
            .LoadAsync(size)
            .and_then(|op| op.get())
            .map_err(|e| e.to_string())?;
        let mut bytes = vec![0u8; size as usize];
        reader.ReadBytes(&mut bytes).map_err(|e| e.to_string())?;
        to_playback_pcm(&wav_decode(&bytes)?)
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use std::net::TcpListener;

    use super::*;

    #[test]
    fn the_embedded_asr_script_is_the_repository_tool() {
        assert!(ASR_SERVER_SCRIPT.contains("faster_whisper"));
        assert!(ASR_SERVER_SCRIPT.contains("/v1/audio/transcriptions"));
        assert!(ASR_SERVER_SCRIPT.contains("127.0.0.1"));
    }

    #[test]
    fn multipart_bodies_have_fields_then_the_file() {
        let body = multipart(
            "b0",
            "voice.wav",
            "audio/wav",
            b"RIFF",
            &[("response_format", "json"), ("language", "auto")],
        );
        let text = String::from_utf8_lossy(&body);
        assert!(text.starts_with(
            "--b0\r\nContent-Disposition: form-data; name=\"response_format\"\r\n\r\njson\r\n"
        ));
        assert!(text.contains("name=\"file\"; filename=\"voice.wav\"\r\nContent-Type: audio/wav\r\n\r\nRIFF\r\n--b0--\r\n"));
    }

    #[test]
    fn endpoints_must_be_loopback_and_join_like_uri() {
        assert!(HttpAsr::new("http://127.0.0.1:8080/", "WhisperCpp", "").is_ok());
        assert!(HttpAsr::new("http://localhost:8178/v1/", "OpenAI", "base").is_ok());
        assert!(HttpAsr::new("https://api.example.com/", "OpenAI", "base").is_err());
        assert!(HttpAsr::new("http://192.168.1.4:8080/", "WhisperCpp", "").is_err());
        let base = Url::parse("http://127.0.0.1:8178/v1").unwrap();
        assert_eq!(
            join(&base, "audio/transcriptions").unwrap().as_str(),
            "http://127.0.0.1:8178/v1/audio/transcriptions"
        );
        let route = Url::parse("http://127.0.0.1:9/abc/").unwrap();
        assert_eq!(
            join(&route, "health").unwrap().as_str(),
            "http://127.0.0.1:9/abc/health"
        );
    }

    fn serve_once(
        reply: Vec<u8>,
        content_type: &'static str,
    ) -> (String, std::sync::mpsc::Receiver<Vec<u8>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut data = Vec::new();
            let mut buf = [0u8; 8192];
            loop {
                let n = s.read(&mut buf).unwrap();
                data.extend_from_slice(&buf[..n]);
                if let Some(end) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&data[..end]).to_ascii_lowercase();
                    let length = head
                        .lines()
                        .find_map(|l| l.strip_prefix("content-length: "))
                        .and_then(|v| v.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    if data.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            let _ = tx.send(data);
            let head = format!("HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", reply.len());
            let _ = s.write_all(head.as_bytes());
            let _ = s.write_all(&reply);
        });
        (format!("http://127.0.0.1:{port}/"), rx)
    }

    #[test]
    fn whisper_cpp_servers_get_a_wav_upload_and_return_text() {
        let (url, seen) = serve_once(br#"{"text":" hello naby "}"#.to_vec(), "application/json");
        let mut asr = HttpAsr::new(&url, "WhisperCpp", "").unwrap();
        let t = asr.transcribe(&vec![0.1; 1600], None, &[]).unwrap();
        assert_eq!(t.text, "hello naby");
        let request = String::from_utf8_lossy(&seen.recv().unwrap()).into_owned();
        assert!(
            request.starts_with("POST /inference "),
            "{}",
            &request[..40]
        );
        assert!(request.contains("multipart/form-data; boundary=naby"));
        assert!(request.contains("name=\"language\"\r\n\r\nauto"));
        assert!(request.contains("RIFF"));
    }

    #[test]
    fn openai_compatible_servers_get_the_model_name() {
        let (url, seen) = serve_once(br#"{"text":"ok"}"#.to_vec(), "application/json");
        let mut asr = HttpAsr::new(&url, "OpenAI-compatible", "large-v3-turbo").unwrap();
        assert_eq!(asr.transcribe(&[0.0; 160], None, &[]).unwrap().text, "ok");
        let request = String::from_utf8_lossy(&seen.recv().unwrap()).into_owned();
        assert!(request.starts_with("POST /audio/transcriptions "));
        assert!(request.contains("name=\"model\"\r\n\r\nlarge-v3-turbo"));
    }

    #[test]
    fn a_dead_server_gives_the_actionable_message() {
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let mut asr = HttpAsr::new(&format!("http://127.0.0.1:{port}/"), "WhisperCpp", "").unwrap();
        let e = asr.transcribe(&[0.1; 160], None, &[]).unwrap_err();
        assert!(e.contains("Local speech service is unavailable"), "{e}");
    }

    #[test]
    fn an_external_tts_reply_becomes_playback_pcm() {
        let wav = wav_encode(16_000, &f32_to_pcm16(&vec![0.2; 1600]));
        let (url, seen) = serve_once(wav, "audio/wav");
        let local = LocalVoiceOptions {
            tts_url: url,
            tts_model: "kokoro".into(),
            voice: "af_heart".into(),
            ..LocalVoiceOptions::default()
        };
        let mut tts = HttpTts::from_options(&local).unwrap();
        let pcm = tts
            .synthesize("Hello there.", &AtomicBool::new(false))
            .unwrap();
        // 100 ms at 16 kHz becomes 100 ms at 24 kHz.
        assert!((pcm.len() as i64 - 4800).abs() < 200, "{}", pcm.len());
        let request = String::from_utf8_lossy(&seen.recv().unwrap()).into_owned();
        assert!(request.starts_with("POST /audio/speech "));
        assert!(
            request.contains("\"input\":\"Hello there.\"")
                && request.contains("\"voice\":\"af_heart\"")
        );
        assert!(request.contains("\"response_format\":\"wav\""));
    }

    #[test]
    fn worker_frames_round_trip_and_reject_bad_sizes() {
        let mut buf = Vec::new();
        write_frame(&mut buf, b"hello").unwrap();
        assert_eq!(read_frame(&mut Cursor::new(&buf), 100).unwrap(), b"hello");
        assert!(read_frame(&mut Cursor::new(&buf), 3).is_err());
        assert!(read_frame(&mut Cursor::new((-1i32).to_le_bytes().to_vec()), 100).is_err());
        assert!(read_frame(&mut Cursor::new(vec![1u8]), 100).is_err());
        assert_eq!(worker_reply(vec![0, 1, 2]).unwrap(), vec![1, 2]);
        assert!(worker_reply(vec![1]).is_err());
        assert!(worker_reply(vec![]).is_err());
        assert_eq!(
            (
                worker_steps("Quality"),
                worker_steps("Light"),
                worker_steps("Balanced")
            ),
            (12, 4, 8)
        );
        let setup = WorkerSetup {
            library: "l".into(),
            folder: "f".into(),
            engine: "kokoro".into(),
            language: "en".into(),
            speaker: 3,
            steps: 8,
        };
        let back: WorkerSetup =
            serde_json::from_slice(&serde_json::to_vec(&setup).unwrap()).unwrap();
        assert_eq!(back, setup);
    }

    #[test]
    fn sherpa_output_and_piper_configs_parse() {
        let log = "Started\n{\"lang\":\"en\",\"text\":\" what time is it \",\"tokens\":[]}\nDone";
        assert_eq!(
            parse_sherpa_output(log, "Moonshine").unwrap(),
            "what time is it"
        );
        assert!(parse_sherpa_output("no json here", "Moonshine")
            .unwrap_err()
            .contains("returned no transcript"));
        assert_eq!(
            piper_sample_rate(br#"{"audio":{"sample_rate":22050}}"#).unwrap(),
            22050
        );
        assert!(piper_sample_rate(br#"{"audio":{"sample_rate":100}}"#).is_err());
        assert!(piper_sample_rate(b"x").is_err());
    }

    #[test]
    fn sherpa_arguments_follow_the_engine() {
        let dir = tempfile::tempdir().unwrap();
        let storage = SpeechStorage::new(dir.path());
        let qwen = naby_core::assistant::speech::asr_by_id("qwen3-asr-0.6b-int8").unwrap();
        let args = sherpa_args(&storage, qwen, Path::new("u.wav")).unwrap();
        assert!(args[0].starts_with("--qwen3-asr-conv-frontend="));
        assert!(args.iter().any(|a| a.starts_with("--qwen3-asr-tokenizer=")));
        assert_eq!(args[args.len() - 2], "--");
        assert_eq!(args.last().unwrap(), "u.wav");
        assert!(args.iter().any(|a| a.starts_with("--num-threads=")));
        let _ = Job::new();
        assert!(inference_threads() >= 1);
    }
}
