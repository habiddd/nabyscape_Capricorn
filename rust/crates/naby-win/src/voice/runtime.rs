//! The voice runtime (`AssistantRuntime` voice half): one object per app that owns hands-free wake
//! listening, the active conversation (local or realtime), the microphone and audio checks and
//! the speech pack installer, and publishes [`VoiceSnapshot`]s for the views.
//!
//! Memory and power rules: nothing runs while voice is off except the wake listener when the user
//! enabled hands-free. A conversation owns its capture thread, playback thread, model lease and
//! child processes, and releases all of them when it ends. Whistle (and its DLL) unload a minute
//! after the last conversation.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use naby_core::assistant::speech::{asr_by_id, own_asr, own_tts, tts_by_id};
use naby_core::assistant::storage::SpeechStorage;
use naby_core::assistant::voice::VoiceTurn;
use naby_core::assistant::{Options, State};
use naby_ui::assistant::service::AssistantService;
use naby_ui::assistant::voice::engine::{self, Listening, Speaking};
use naby_ui::assistant::voice::{
    AudioCheck, AudioCheckKind, MicChoice, MicTest, Recognizer, SpeechPanel, SpokenTurn, VoiceHost,
    VoiceLine, VoiceSnapshot, WakeStatus,
};

use super::capture::Capture;
use super::dsp::peak;
use super::install::{self, SpeechInstaller};
use super::local::{
    self, HttpAsr, HttpTts, Job, ManagedWhisper, PiperTts, SherpaAsr, WindowsTts, WorkerTts,
};
use super::pipeline::{self, Collected, Collector, Event, Outcome, Replier, Speaker, TurnOptions};
use super::playback::Playback;
use super::realtime::{self, Cmd, Out, Report};
use super::wake::{self, WakeListener};
use super::wake_logic::{WakeAction, WakeLogic};
use super::whistle::{self, Transcriber, Whistle};
use crate::microphones;
use crate::wake::Waker;

/// Lines kept for the captions.
const MAX_LINES: usize = 8;
const TEST_SECONDS: usize = 4;
const SAMPLE_SENTENCE: &str = "Hello. This is Naby. Your speaking voice is ready.";

/// What the app shell should do after a wake phrase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    /// Open the voice room and start the conversation.
    OpenAndStart,
}

struct Active {
    id: u64,
    cancel: Arc<AtomicBool>,
    turn: SpokenTurn,
    capture: Option<Capture>,
    playback: Option<Playback>,
    threads: Vec<JoinHandle<()>>,
    realtime: Option<Sender<Cmd>>,
    /// Released when the conversation ends so Whistle's idle countdown starts.
    lease: Option<whistle::WhistleLease>,
    /// Child processes die with this job.
    job: Option<Arc<Job>>,
    closing: bool,
}

struct Dynamic {
    snap: VoiceSnapshot,
}

struct Inner {
    service: AssistantService,
    waker: Waker,
    installer: Arc<SpeechInstaller>,
    whistle: Whistle,
    started: Instant,
    dynamic: Mutex<Dynamic>,
    statics: Mutex<(u64, Arc<VoiceSnapshot>)>,
    wake: Mutex<WakeLogic>,
    listener: Mutex<Option<SendListener>>,
    active: Mutex<Option<Active>>,
    next_id: AtomicU64,
    mic_test: Mutex<(MicTest, Option<Capture>)>,
    check: Mutex<AudioCheck>,
    requests: Mutex<Vec<Request>>,
    last_level: Mutex<Instant>,
}

/// WinRT recognizers are free-threaded (MTA); the listener is only touched under its mutex.
struct SendListener(WakeListener);
// SAFETY: `SpeechRecognizer` and its continuous session are agile (free-threaded) WinRT objects
// created in the MTA; every call goes through the `listener` mutex.
unsafe impl Send for SendListener {}

/// Cheap to clone.
#[derive(Clone)]
pub struct VoiceRuntime {
    inner: Arc<Inner>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl VoiceRuntime {
    pub fn new(service: AssistantService, data_dir: &std::path::Path, waker: Waker) -> Self {
        let installer = SpeechInstaller::new(data_dir, waker.clone());
        let whistle = Whistle::native(installer.whistle_dir());
        let inner = Arc::new(Inner {
            service,
            waker,
            installer,
            whistle,
            started: Instant::now(),
            dynamic: Mutex::new(Dynamic {
                snap: VoiceSnapshot::default(),
            }),
            statics: Mutex::new((0, Arc::new(VoiceSnapshot::default()))),
            wake: Mutex::new(WakeLogic::default()),
            listener: Mutex::new(None),
            active: Mutex::new(None),
            next_id: AtomicU64::new(1),
            mic_test: Mutex::new((MicTest::default(), None)),
            check: Mutex::new(AudioCheck::default()),
            requests: Mutex::new(Vec::new()),
            last_level: Mutex::new(Instant::now()),
        });
        Self { inner }
    }

    pub fn installer(&self) -> &Arc<SpeechInstaller> {
        &self.inner.installer
    }

    pub fn service(&self) -> &AssistantService {
        &self.inner.service
    }

    /// Requests the shell should act on (a wake phrase was heard).
    pub fn take_requests(&self) -> Vec<Request> {
        std::mem::take(&mut *lock(&self.inner.requests))
    }

    /// A conversation is connecting or running.
    pub fn active(&self) -> bool {
        lock(&self.inner.active).is_some()
    }

    /// Dock label while voice is on (`Listening`, `Thinking…`, ...).
    pub fn label(&self) -> String {
        let s = lock(&self.inner.dynamic).snap.clone();
        if !s.active {
            return String::new();
        }
        match s.phase {
            State::Listening => "Listening",
            State::Thinking => "Thinking\u{2026}",
            State::Speaking => "Speaking",
            State::Connecting => "Connecting\u{2026}",
            State::Error => "Voice needs attention",
            _ => "",
        }
        .to_owned()
    }

    // ---- snapshot -------------------------------------------------------------------------

    fn now(&self) -> f64 {
        self.inner.started.elapsed().as_secs_f64()
    }

    fn notify(&self) {
        (self.inner.waker)();
    }

    fn publish(&self, edit: impl FnOnce(&mut VoiceSnapshot)) {
        {
            let mut d = lock(&self.inner.dynamic);
            edit(&mut d.snap);
            d.snap.revision += 1;
        }
        self.notify();
    }

    fn publish_level(&self, mic: Option<f32>, out: Option<f32>) {
        let mut last = lock(&self.inner.last_level);
        if last.elapsed() < Duration::from_millis(70) {
            return;
        }
        *last = Instant::now();
        drop(last);
        self.publish(|s| {
            if let Some(m) = mic {
                s.mic_level = m;
            }
            if let Some(o) = out {
                s.output_level = o;
            }
        });
    }

    fn statics(&self, options: &Options) -> VoiceSnapshot {
        let local = &options.local_voice;
        let engines = engine::resolve(local);
        let ready_state = self.inner.service.setup();
        let (engine_ready, problem) = match engines.listening {
            Listening::Whistle => {
                match whistle::availability(&self.inner.installer.whistle_dir()) {
                    Ok(()) => (true, String::new()),
                    Err(e) => (false, e.message()),
                }
            }
            Listening::NeedsSetup => (false, engines.note.clone().unwrap_or_default()),
            _ => (true, engines.note.clone().unwrap_or_default()),
        };
        let speaker = match engines.speaking {
            Speaking::Realtime => "Realtime provider".to_owned(),
            Speaking::Windows => "Windows voice".to_owned(),
            Speaking::Piper => match engine::piper_voice(local) {
                Some(v)
                    if install::piper_ready(self.inner.installer.storage(), v)
                        || install::piper_bundled(install::bundle_dir().as_deref(), v) =>
                {
                    v.name.clone()
                }
                _ => "Windows voice \u{b7} Piper not installed".to_owned(),
            },
            Speaking::Pack => own_tts(local).map_or("Speech pack voice".into(), |t| t.name),
            Speaking::External => format!("External server \u{b7} {}", local.tts_engine),
        };
        let microphone = {
            let id = &options.microphone_id;
            if id.is_empty() {
                microphones::DEFAULT_NAME.to_owned()
            } else {
                microphones::list()
                    .into_iter()
                    .find(|m| &m.id == id)
                    .map_or("Selected microphone".into(), |m| m.name)
            }
        };
        VoiceSnapshot {
            engine: engine::label(local),
            speaker,
            engine_ready,
            engine_problem: problem,
            microphone,
            configured: options.enabled && ready_state.voice.configured,
            setup_hint: if !options.enabled {
                "Open Voice settings to enable AI and choose a speech connection.".into()
            } else {
                ready_state.voice.detail.clone()
            },
            ..VoiceSnapshot::default()
        }
    }

    /// The merged snapshot: live state from the conversation, settings-derived facts cached per
    /// settings revision, wake status from the state machine.
    pub fn snapshot(&self) -> Arc<VoiceSnapshot> {
        let revision = self.inner.service.revision();
        {
            let mut cached = lock(&self.inner.statics);
            if cached.0 != revision || cached.0 == 0 {
                let options = self.inner.service.options();
                *cached = (revision.max(1), Arc::new(self.statics(&options)));
            }
        }
        let fixed = lock(&self.inner.statics).1.clone();
        let mut snap = lock(&self.inner.dynamic).snap.clone();
        snap.engine = fixed.engine.clone();
        snap.speaker = fixed.speaker.clone();
        snap.engine_ready = fixed.engine_ready;
        snap.engine_problem = fixed.engine_problem.clone();
        snap.microphone = fixed.microphone.clone();
        snap.configured = fixed.configured;
        snap.setup_hint = fixed.setup_hint.clone();
        let wake = lock(&self.inner.wake);
        snap.wake = WakeStatus {
            enabled: wake.allowed(),
            listening: wake.listening(),
            paused: wake.paused(),
            text: wake.status().to_owned(),
            phrase: wake.phrase().to_owned(),
        };
        Arc::new(snap)
    }

    // ---- wake ---------------------------------------------------------------------------------

    /// Applies the saved wake settings (startup, after saving) and arms the listener when allowed.
    pub fn configure_wake(&self) {
        let options = self.inner.service.options();
        let action = lock(&self.inner.wake).configure(
            options.enabled,
            options.wake_word.enabled,
            &options.wake_word.phrase,
        );
        self.apply_wake_action(action);
        self.notify();
        self.wake_tick();
    }

    fn apply_wake_action(&self, action: WakeAction) {
        if action == WakeAction::Disarm {
            self.release_listener();
        }
    }

    /// Drops the listener off the calling thread (its callbacks may be what called us).
    fn release_listener(&self) {
        let listener = lock(&self.inner.listener).take();
        if let Some(mut l) = listener {
            let _ = std::thread::Builder::new()
                .name("NabyScape wake stop".into())
                .spawn(move || l.0.stop());
        }
    }

    /// Opens the wake microphone when the state machine says so; otherwise schedules exactly one
    /// wake-up for the cooldown end (no polling).
    pub fn wake_tick(&self) {
        let now = self.now();
        let (action, delay) = {
            let w = lock(&self.inner.wake);
            (w.poll(now), w.arm_delay(now))
        };
        match action {
            WakeAction::Arm => self.arm_wake(),
            _ => {
                if let Some(delay) = delay {
                    let me = self.clone();
                    let _ = std::thread::Builder::new()
                        .name("NabyScape wake timer".into())
                        .spawn(move || {
                            std::thread::sleep(Duration::from_secs_f64(delay.max(0.05)));
                            me.wake_tick();
                        });
                }
            }
        }
    }

    fn arm_wake(&self) {
        // Mark busy so concurrent ticks do not open the microphone twice.
        lock(&self.inner.wake).set_busy(true);
        let me = self.clone();
        let spawned = std::thread::Builder::new()
            .name("NabyScape wake start".into())
            .spawn(move || {
                let options = me.inner.service.options();
                let (m1, m2) = (me.clone(), me.clone());
                let result = WakeListener::start(
                    &options.wake_word.phrase,
                    &options.wake_word.recognizer_id,
                    options.wake_word.confidence,
                    move || m1.wake_detected(),
                    move |message| {
                        lock(&m2.inner.wake).failed(&message);
                        m2.release_listener();
                        m2.notify();
                    },
                );
                let mut w = lock(&me.inner.wake);
                w.set_busy(false);
                match result {
                    Ok(listener) => {
                        w.armed_ok();
                        drop(w);
                        *lock(&me.inner.listener) = Some(SendListener(listener));
                    }
                    Err(message) => {
                        w.failed(&format!("Wake word unavailable: {message}"));
                    }
                }
                me.notify();
            });
        if spawned.is_err() {
            lock(&self.inner.wake).set_busy(false);
        }
    }

    fn wake_detected(&self) {
        let action = lock(&self.inner.wake).detected();
        self.apply_wake_action(action);
        lock(&self.inner.requests).push(Request::OpenAndStart);
        self.notify();
    }

    /// The shell could not open the voice room or start voice after a wake phrase.
    pub fn wake_start_failed(&self) {
        lock(&self.inner.wake).voice_failed_to_start();
        self.notify();
    }

    // ---- conversation -----------------------------------------------------------------------------

    fn local_options(&self) -> Options {
        self.inner.service.options()
    }

    /// Starts a conversation on `turn`'s chat (explicit user action or a wake phrase).
    pub fn start(&self, turn: SpokenTurn) {
        let id = self.inner.next_id.fetch_add(1, Ordering::SeqCst);
        let cancel = Arc::new(AtomicBool::new(false));
        {
            let mut active = lock(&self.inner.active);
            if active.is_some() {
                return;
            }
            *active = Some(Active {
                id,
                cancel: cancel.clone(),
                turn: turn.clone(),
                capture: None,
                playback: None,
                threads: Vec::new(),
                realtime: None,
                lease: None,
                job: None,
                closing: false,
            });
        }
        let action = lock(&self.inner.wake).voice_started();
        self.apply_wake_action(action);
        self.publish(|s| {
            *s = VoiceSnapshot {
                active: true,
                phase: State::Connecting,
                status: "Connecting voice\u{2026}".into(),
                lines: Vec::new(),
                ..VoiceSnapshot::default()
            };
        });
        turn.set_phase(State::Connecting, "Connecting voice\u{2026}", true);
        let me = self.clone();
        let spawned = std::thread::Builder::new()
            .name("NabyScape voice start".into())
            .spawn(move || {
                let result = me.launch(&turn, id, &cancel);
                if let Err(message) = result {
                    if !cancel.load(Ordering::SeqCst) {
                        me.fail(id, &message);
                    }
                }
            });
        if spawned.is_err() {
            self.fail(id, "Voice could not start.");
        }
    }

    fn with_active<R>(&self, id: u64, f: impl FnOnce(&mut Active) -> R) -> Option<R> {
        let mut guard = lock(&self.inner.active);
        match guard.as_mut() {
            Some(a) if a.id == id && !a.closing => Some(f(a)),
            _ => None,
        }
    }

    /// The conversation failed: report it, stop capture and playback, keep wake paused.
    fn fail(&self, id: u64, message: &str) {
        self.publish(|s| {
            s.phase = State::Error;
            s.status = message.to_owned();
        });
        if let Some(turn) = self.with_active(id, |a| a.turn.clone()) {
            turn.set_phase(State::Error, message, false);
        }
        self.teardown(id, false);
    }

    fn launch(&self, turn: &SpokenTurn, id: u64, cancel: &Arc<AtomicBool>) -> Result<(), String> {
        let options = self.local_options();
        if !options.enabled {
            return Err("Enable AI companion in Voice settings first.".into());
        }
        let engines = engine::resolve(&options.local_voice);
        if engines.listening == Listening::Realtime {
            self.launch_realtime(turn, id, &options, cancel)
        } else {
            self.launch_local(turn, id, &options, cancel)
        }
    }

    // ---- local conversation ------------------------------------------------------------------------

    /// Builds the listening engine for the saved options.
    fn make_transcriber(
        &self,
        options: &Options,
        job: &Arc<Job>,
        cancel: &AtomicBool,
        status: &dyn Fn(&str),
    ) -> Result<Box<dyn Transcriber + Send>, String> {
        let local = &options.local_voice;
        let engines = engine::resolve(local);
        let storage = self.inner.installer.storage();
        match engines.listening {
            Listening::Whistle => {
                if !engine::whistle_supports(&local.speech_language) {
                    return Err(engines.note.unwrap_or_default());
                }
                status("Loading Whistle\u{2026}");
                let lease = self.inner.whistle.acquire().map_err(|e| e.message())?;
                // Whistle auto-detects the language; the lease is also kept to release later.
                Ok(Box::new(WhistleAdapter(Some(lease))))
            }
            Listening::External => Ok(Box::new(HttpAsr::from_options(local)?)),
            Listening::Pack | Listening::OwnFiles => {
                let model = own_asr(local).map_err(|e| e.message().to_owned())?;
                let tts = own_tts(local).map_err(|e| e.message().to_owned())?;
                let gpu = local.managed_gpu;
                if local.own_models || !local.managed {
                    // Own files: the engines must be present; the files are the user's.
                } else if !install::installed(storage, &model, &tts, gpu) {
                    return Err(
                        "Prepare your voice setup first: Voice settings \u{2192} Models.".into(),
                    );
                }
                if model.native() {
                    status(&format!(
                        "{} selected \u{b7} the native model loads when you speak.",
                        model.name
                    ));
                    Ok(Box::new(SherpaAsr {
                        exe: storage.sherpa_exe(false),
                        storage: storage.clone(),
                        model,
                        job: job.clone(),
                    }))
                } else {
                    status("Loading the local listening model\u{2026}");
                    let gpu = gpu && storage.whisper_exe(true).is_file();
                    let server = ManagedWhisper::start(
                        &storage.whisper_exe(gpu),
                        &storage.asr_path(&model),
                        gpu,
                        job,
                        cancel,
                    )?;
                    Ok(Box::new(server))
                }
            }
            Listening::NeedsSetup => Err(engines
                .note
                .unwrap_or_else(|| "Choose a listening engine in Voice settings.".into())),
            Listening::Realtime => Err("Realtime voice has its own session.".into()),
        }
    }

    fn make_speaker(
        &self,
        options: &Options,
        job: &Arc<Job>,
        status: &dyn Fn(&str),
    ) -> Result<Box<dyn Speaker + Send>, String> {
        let local = &options.local_voice;
        let engines = engine::resolve(local);
        let storage = self.inner.installer.storage();
        match engines.speaking {
            Speaking::Windows | Speaking::Realtime => {
                Ok(Box::new(WindowsTts::new(&local.speech_language)))
            }
            Speaking::External => Ok(Box::new(HttpTts::from_options(local)?)),
            Speaking::Piper => {
                let Some(voice) = engine::piper_voice(local) else {
                    return Ok(Box::new(WindowsTts::new(&local.speech_language)));
                };
                let bundle = install::bundle_dir();
                if !install::piper_ready(storage, voice)
                    && install::piper_bundled(bundle.as_deref(), voice)
                    && install::cpp_runtime_present()
                    && storage.begin()
                {
                    status("Preparing Piper\u{2026}");
                    // A failure here only means the Windows voice speaks (checked below).
                    let _ = install::install_piper_files(
                        storage,
                        voice,
                        bundle.as_deref(),
                        &AtomicBool::new(false),
                        &mut |_, _| {},
                    );
                    storage.end();
                }
                if install::piper_ready(storage, voice) {
                    Ok(Box::new(PiperTts {
                        exe: storage.piper_exe(),
                        model: storage.tts_path(voice),
                        speaker: 0,
                        length_scale: (1.0 / engine::piper_speed(local)) as f32,
                        job: job.clone(),
                    }))
                } else {
                    status("Piper is not installed \u{b7} the Windows voice speaks. Install it in Settings \u{2192} Voice \u{2192} Models.");
                    Ok(Box::new(WindowsTts::new(&local.speech_language)))
                }
            }
            Speaking::Pack => {
                let model = own_tts(local).map_err(|e| e.message().to_owned())?;
                if model.id == "windows" {
                    return Ok(Box::new(WindowsTts::new(&local.speech_language)));
                }
                if model.package.is_some() {
                    status(&format!(
                        "Loading {} \u{b7} stays ready during this voice session\u{2026}",
                        model.name
                    ));
                    let setup = local::worker_setup(storage, &model, local);
                    Ok(Box::new(WorkerTts::start(&setup, Some(job))?))
                } else {
                    Ok(Box::new(PiperTts {
                        exe: storage.piper_exe(),
                        model: storage.tts_path(&model),
                        speaker: model.speaker,
                        length_scale: 1.0,
                        job: job.clone(),
                    }))
                }
            }
        }
    }

    fn launch_local(
        &self,
        turn: &SpokenTurn,
        id: u64,
        options: &Options,
        cancel: &Arc<AtomicBool>,
    ) -> Result<(), String> {
        if self.inner.service.provider().is_none() {
            return Err("Add and select a chat provider first. Local speech listens and speaks on this PC; your chat provider writes the replies.".into());
        }
        let job = Arc::new(Job::new()?);
        let status = |text: &str| {
            let t = text.to_owned();
            self.publish(|s| s.status = t);
        };
        let transcriber = self.make_transcriber(options, &job, cancel, &status)?;
        let speaker = self.make_speaker(options, &job, &status)?;
        if cancel.load(Ordering::SeqCst) {
            return Ok(());
        }
        let local = options.local_voice.clone();
        let queue_holder = {
            let me = self.clone();
            Playback::start(move |message| {
                me.fail(
                    id,
                    &format!("The local voice audio device stopped. {message}"),
                )
            })?
        };
        let queue = queue_holder.queue.clone();
        let (utterances, inbox) = channel::<Vec<f32>>();
        let processing = Arc::new(AtomicBool::new(false));
        let collector = Mutex::new(Collector::new(local.sensitivity, local.silence_ms));
        let (flag, mine) = (processing.clone(), self.clone());
        let on_audio = move |chunk: &[f32]| {
            let level = peak(chunk);
            mine.publish_level(Some(level), None);
            let mut collector = lock(&collector);
            if flag.load(Ordering::SeqCst) {
                collector.reset();
                return;
            }
            if let Collected::Utterance(audio) = collector.push(chunk) {
                flag.store(true, Ordering::SeqCst);
                collector.reset();
                let _ = utterances.send(audio);
            }
        };
        let failer = self.clone();
        let capture = Capture::start(&options.microphone_id, 16_000, on_audio, move |m| {
            failer.fail(
                id,
                &format!("Voice stopped. {m} End voice and check Windows sound settings."),
            )
        })?;
        let turn_thread = {
            let me = self.clone();
            let turn = turn.clone();
            let cancel = cancel.clone();
            let queue = queue.clone();
            let processing = processing.clone();
            let keywords = engine::keywords(options);
            let stream = local.stream_replies;
            let mut transcriber = transcriber;
            let mut speaker = speaker;
            std::thread::Builder::new()
                .name("NabyScape voice turns".into())
                .spawn(move || {
                    let mut replier = ChatReplier {
                        turn: turn.clone(),
                        cancel: cancel.clone(),
                    };
                    let mut state = VoiceTurn::new();
                    // One tick of level reporting while the reply plays.
                    for audio in inbox {
                        if cancel.load(Ordering::SeqCst) {
                            break;
                        }
                        state = VoiceTurn::new();
                        let opts = TurnOptions {
                            language: None,
                            keywords: &keywords,
                            stream_replies: stream,
                        };
                        let outcome = pipeline::run_turn(
                            Some(&audio),
                            None,
                            &mut *transcriber,
                            &mut replier,
                            &mut *speaker,
                            &*queue,
                            &opts,
                            &mut state,
                            &cancel,
                            &mut |event| me.on_turn_event(id, &turn, event),
                        );
                        if let Outcome::Failed(message) = &outcome {
                            me.fail(id, &format!("Voice stopped. {message}"));
                            break;
                        }
                        processing.store(false, Ordering::SeqCst);
                    }
                    let _ = state;
                })
                .map_err(|e| e.to_string())?
        };
        let ok = self.with_active(id, |a| {
            a.capture = Some(capture);
            a.playback = Some(queue_holder);
            a.threads.push(turn_thread);
            a.job = Some(job);
        });
        if ok.is_none() {
            self.teardown(id, false);
            return Ok(());
        }
        self.publish(|s| {
            s.phase = State::Listening;
            s.status = "Listening locally \u{b7} speak, then pause. Speech stays on this PC; chat uses the selected chat provider.".into();
        });
        turn.set_phase(State::Listening, "Listening", true);
        Ok(())
    }

    fn on_turn_event(&self, id: u64, turn: &SpokenTurn, event: Event) {
        if self.with_active(id, |_| ()).is_none() {
            return;
        }
        match event {
            Event::Phase(phase) => {
                self.publish(|s| {
                    s.phase = phase;
                    if phase == State::Listening {
                        s.mic_level = 0.0;
                        s.output_level = 0.0;
                    }
                });
                turn.set_phase(phase, "Voice", true);
            }
            Event::Status(text) => self.publish(|s| s.status = text),
            Event::User(text) => self.publish(|s| {
                s.partial.clear();
                s.lines.push(VoiceLine { user: true, text });
                trim_lines(&mut s.lines);
            }),
            Event::Spoken(sentence) => self.publish(|s| {
                match s.lines.last_mut() {
                    Some(l) if !l.user => {
                        l.text.push(' ');
                        l.text.push_str(&sentence);
                    }
                    _ => s.lines.push(VoiceLine {
                        user: false,
                        text: sentence,
                    }),
                }
                trim_lines(&mut s.lines);
            }),
            Event::Timing(t) => self.publish(|s| {
                s.timing = Some(t.summary());
                s.timing_detail = Some(t.detail());
            }),
        }
    }

    // ---- realtime conversation --------------------------------------------------------------------------

    fn launch_realtime(
        &self,
        turn: &SpokenTurn,
        id: u64,
        options: &Options,
        cancel: &Arc<AtomicBool>,
    ) -> Result<(), String> {
        let mut provider = options
            .voice_provider()
            .cloned()
            .ok_or("Choose a realtime voice provider in Voice settings.")?;
        let endpoint = realtime::endpoint_for(&provider)?;
        let key = self.inner.service.key_for(&provider.id);
        let gemini = provider.realtime_protocol == "GeminiLive";
        let tools = turn.tools();
        provider.tools = provider.tools && !tools.is_empty();
        let history: Vec<(bool, String)> = turn
            .history()
            .into_iter()
            .map(|m| (matches!(m.role, naby_core::assistant::Role::User), m.text))
            .collect();
        let queue_holder = {
            let me = self.clone();
            Playback::start(move |m| me.fail(id, &format!("Audio device unavailable. {m}")))?
        };
        let queue = queue_holder.queue.clone();
        self.publish(|s| s.status = "Connecting voice\u{2026}".into());
        let socket = realtime::connect(&endpoint, key.as_deref(), gemini)?;
        if cancel.load(Ordering::SeqCst) {
            return Ok(());
        }
        let (cmd_tx, cmd_rx) = channel::<Cmd>();
        let (report_tx, report_rx) = channel::<Report>();
        let config = realtime::Config {
            provider: provider.clone(),
            key: None,
            instructions: turn.realtime_instructions(),
            tools: tools.clone(),
            history,
        };
        let rate = realtime::input_rate(&provider);
        let io = {
            let q = queue.clone();
            std::thread::Builder::new()
                .name("NabyScape voice socket".into())
                .spawn(move || realtime::run(socket, config, q, cmd_rx, report_tx, || {}))
                .map_err(|e| e.to_string())?
        };
        // Reports: transcripts, tool calls, levels and failures.
        let reporter = {
            let me = self.clone();
            let turn = turn.clone();
            let cmds = cmd_tx.clone();
            let cancel = cancel.clone();
            let epoch = turn.epoch();
            std::thread::Builder::new()
                .name("NabyScape voice reports".into())
                .spawn(move || {
                    for report in report_rx {
                        match report {
                            Report::Out(Out::Ready) => {
                                me.publish(|s| {
                                    s.phase = State::Listening;
                                    s.status = "Voice connected".into();
                                });
                                turn.set_phase(State::Listening, "Voice connected", true);
                            }
                            Report::Out(Out::Status(text)) => me.publish(|s| s.status = text),
                            Report::Out(Out::Transcript { user, text }) => {
                                turn.record_line(user, &text);
                                me.publish(|s| {
                                    s.phase = if user {
                                        State::Thinking
                                    } else {
                                        State::Speaking
                                    };
                                    s.lines.push(VoiceLine { user, text });
                                    trim_lines(&mut s.lines);
                                });
                            }
                            Report::Out(Out::ToolCall(call)) => {
                                let (turn, cmds, cancel) =
                                    (turn.clone(), cmds.clone(), cancel.clone());
                                // Tool work runs off the report loop so audio keeps flowing.
                                let _ = std::thread::Builder::new()
                                    .name("NabyScape voice tool".into())
                                    .spawn(move || {
                                        if cancel.load(Ordering::SeqCst) {
                                            return;
                                        }
                                        let result = turn.execute_tool(&call);
                                        turn.record_outcome(epoch, &call, &result);
                                        let _ = cmds.send(Cmd::ToolResult { call, result });
                                    });
                            }
                            Report::Out(Out::CancelCalls(_)) => {}
                            Report::MicLevel(level) => me.publish_level(Some(level), Some(0.0)),
                            Report::Failed(message) => {
                                me.fail(id, &message);
                                break;
                            }
                            Report::Ended => break,
                        }
                    }
                })
                .map_err(|e| e.to_string())?
        };
        let mic = cmd_tx.clone();
        let failer = self.clone();
        let capture = Capture::start(
            &options.microphone_id,
            rate,
            move |chunk| {
                let _ = mic.send(Cmd::Mic(super::dsp::f32_to_pcm16(chunk)));
            },
            move |m| failer.fail(id, &format!("Audio device unavailable. {m}")),
        )?;
        let output_level = {
            // Output level follows the playback queue while the reply is heard.
            let me = self.clone();
            let q = queue.clone();
            let cancel = cancel.clone();
            std::thread::Builder::new()
                .name("NabyScape voice meter".into())
                .spawn(move || {
                    while !cancel.load(Ordering::SeqCst) {
                        std::thread::sleep(Duration::from_millis(100));
                        let playing = q.pending_ms() > 0;
                        me.publish_level(None, Some(if playing { q.level() } else { 0.0 }));
                        if playing {
                            me.publish_phase_speaking();
                        }
                    }
                })
                .map_err(|e| e.to_string())?
        };
        let ok = self.with_active(id, |a| {
            a.capture = Some(capture);
            a.playback = Some(queue_holder);
            a.realtime = Some(cmd_tx);
            a.threads.extend([io, reporter, output_level]);
        });
        if ok.is_none() {
            self.teardown(id, false);
        }
        Ok(())
    }

    fn publish_phase_speaking(&self) {
        let mut d = lock(&self.inner.dynamic);
        if d.snap.phase == State::Listening || d.snap.phase == State::Thinking {
            d.snap.phase = State::Speaking;
            d.snap.revision += 1;
            drop(d);
            self.notify();
        }
    }

    // ---- ending -------------------------------------------------------------------------------------------

    /// Ends the active conversation off the calling thread. `resume_wake` re-arms wake listening
    /// after the cooldown (End conversation); an explicit Stop passes false.
    fn teardown(&self, id: u64, resume_wake: bool) {
        let taken = {
            let mut guard = lock(&self.inner.active);
            match guard.as_mut() {
                Some(a) if a.id == id && !a.closing => {
                    a.closing = true;
                    a.cancel.store(true, Ordering::SeqCst);
                    Some((a.turn.clone(), a.realtime.clone()))
                }
                _ => None,
            }
        };
        let Some((turn, rt)) = taken else { return };
        turn.stop();
        if let Some(rt) = rt {
            let _ = rt.send(Cmd::Stop);
        }
        self.publish(|s| {
            s.active = false;
            if s.phase != State::Error {
                s.phase = State::Idle;
                s.status = "Voice ended \u{b7} microphone off.".into();
            }
            s.mic_level = 0.0;
            s.output_level = 0.0;
            s.partial.clear();
        });
        let me = self.clone();
        let _ = std::thread::Builder::new()
            .name("NabyScape voice stop".into())
            .spawn(move || {
                let active = lock(&me.inner.active).take();
                if let Some(mut a) = active {
                    // Hardware first, then workers: the microphone is off before this returns.
                    if let Some(mut c) = a.capture.take() {
                        c.stop();
                    }
                    drop(a.realtime.take());
                    if let Some(mut p) = a.playback.take() {
                        p.stop();
                    }
                    for t in a.threads.drain(..) {
                        let _ = t.join();
                    }
                    // Dropping these unloads Whistle's lease and kills child engines.
                    drop(a.lease.take());
                    drop(a.job.take());
                }
                turn.set_phase(State::Idle, "Voice ended", false);
                turn.stop();
                let now = me.now();
                if resume_wake {
                    lock(&me.inner.wake).voice_ended(now);
                }
                me.notify();
                me.wake_tick();
            });
    }

    fn end_active(&self, resume_wake: bool) {
        let id = lock(&self.inner.active).as_ref().map(|a| a.id);
        if let Some(id) = id {
            self.teardown(id, resume_wake);
        } else if resume_wake {
            let now = self.now();
            lock(&self.inner.wake).voice_ended(now);
            self.wake_tick();
        }
    }

    /// End conversation: wake listening resumes when enabled.
    pub fn end(&self) {
        self.end_active(true);
    }

    /// Stop: ends everything and pauses wake listening.
    pub fn stop(&self) {
        let action = lock(&self.inner.wake).explicit_stop();
        self.apply_wake_action(action);
        self.end_active(false);
        self.cancel_checks();
        self.notify();
    }

    /// The app is quitting or the machine locked: everything off.
    pub fn shutdown(&self) {
        self.stop();
    }

    pub fn send_text(&self, text: &str) {
        let rt = lock(&self.inner.active)
            .as_ref()
            .and_then(|a| a.realtime.clone());
        if let Some(rt) = rt {
            let _ = rt.send(Cmd::Text(text.to_owned()));
        }
    }

    /// Sends the shared screen to a realtime session (explicit user action only).
    pub fn share_frame(&self) {
        let (turn, rt) = match lock(&self.inner.active).as_ref() {
            Some(a) => (a.turn.clone(), a.realtime.clone()),
            None => return,
        };
        if let (Some(rt), Some(frame)) = (rt, turn.shared_frame(&AtomicBool::new(false))) {
            let _ = rt.send(Cmd::Image(frame));
        }
    }

    // ---- microphone and audio checks ------------------------------------------------------------------------

    fn cancel_checks(&self) {
        let capture = lock(&self.inner.mic_test).1.take();
        drop(capture);
        lock(&self.inner.mic_test).0.running = false;
    }

    fn start_mic_test(&self, then: Option<Sender<Vec<f32>>>) {
        self.cancel_checks();
        if self.active() {
            lock(&self.inner.mic_test).0 = MicTest {
                running: false,
                level: 0.0,
                result: "End the current conversation before testing.".into(),
            };
            self.notify();
            return;
        }
        let options = self.local_options();
        let target = 16_000 * TEST_SECONDS;
        let samples = Arc::new(Mutex::new(Vec::<f32>::with_capacity(target)));
        let (s2, me) = (samples.clone(), self.clone());
        let done = Arc::new(AtomicBool::new(false));
        let d2 = done.clone();
        let started = Capture::start(
            &options.microphone_id,
            16_000,
            move |chunk| {
                let mut all = lock(&s2);
                all.extend_from_slice(chunk);
                let level = peak(chunk);
                let finished = all.len() >= target;
                drop(all);
                lock(&me.inner.mic_test).0.level = level;
                me.notify();
                if finished && !d2.swap(true, Ordering::SeqCst) {
                    let fin = me.clone();
                    let samples = s2.clone();
                    let then = then.clone();
                    // Stop the device from another thread (this one is the capture thread).
                    let _ = std::thread::Builder::new().name("NabyScape mic test end".into()).spawn(move || {
                        let capture = lock(&fin.inner.mic_test).1.take();
                        drop(capture);
                        let audio = std::mem::take(&mut *lock(&samples));
                        let max = peak(&audio);
                        {
                            let mut t = lock(&fin.inner.mic_test);
                            t.0.running = false;
                            t.0.level = 0.0;
                            t.0.result = if max < 0.01 {
                                "Microphone opened, but no audible input was detected. Check your input device and volume, then retry.".into()
                            } else {
                                "Microphone checked \u{b7} audio detected. Transcription and speaking were not tested. Microphone is now off.".into()
                            };
                        }
                        if let Some(tx) = then {
                            let _ = tx.send(audio);
                        }
                        fin.notify();
                    });
                }
            },
            {
                let me = self.clone();
                move |m| {
                    let mut t = lock(&me.inner.mic_test);
                    t.0.running = false;
                    t.0.result = format!("The microphone stopped. {m}");
                    drop(t);
                    me.notify();
                }
            },
        );
        match started {
            Ok(capture) => {
                let mut t = lock(&self.inner.mic_test);
                t.0 = MicTest {
                    running: true,
                    level: 0.0,
                    result: String::new(),
                };
                t.1 = Some(capture);
            }
            Err(e) => {
                lock(&self.inner.mic_test).0 = MicTest {
                    running: false,
                    level: 0.0,
                    result: e,
                };
            }
        }
        self.notify();
    }

    fn set_check(&self, running: bool, message: &str, ok: Option<bool>) {
        *lock(&self.inner.check) = AudioCheck {
            running,
            level: 0.0,
            message: message.to_owned(),
            ok,
        };
        self.notify();
    }

    fn run_check(&self, kind: AudioCheckKind) {
        if self.active() {
            self.set_check(
                false,
                "End the current conversation and finish model setup before testing.",
                Some(false),
            );
            return;
        }
        match kind {
            AudioCheckKind::Microphone => {
                self.start_mic_test(None);
                let result = lock(&self.inner.mic_test).0.clone();
                self.set_check(
                    result.running,
                    if result.running {
                        "Microphone on for four seconds. Say a short sentence."
                    } else {
                        &result.result
                    },
                    None,
                );
            }
            AudioCheckKind::Listening => {
                let (tx, rx) = channel::<Vec<f32>>();
                self.start_mic_test(Some(tx));
                self.set_check(
                    true,
                    "Microphone on for four seconds. Say a short sentence.",
                    None,
                );
                let me = self.clone();
                let _ = std::thread::Builder::new().name("NabyScape listening check".into()).spawn(move || {
                    let Ok(audio) = rx.recv() else {
                        me.set_check(false, "Test cancelled or timed out. Microphone and test engines are closed.", Some(false));
                        return;
                    };
                    me.set_check(true, "Microphone off \u{b7} testing the saved listening model\u{2026}", None);
                    let options = me.local_options();
                    let job = match Job::new() {
                        Ok(j) => Arc::new(j),
                        Err(e) => return me.set_check(false, &format!("Test needs attention \u{b7} {e}"), Some(false)),
                    };
                    let cancel = AtomicBool::new(false);
                    let result = me
                        .make_transcriber(&options, &job, &cancel, &|_| {})
                        .and_then(|mut t| t.transcribe(&audio, None, &engine::keywords(&options)));
                    match result {
                        Ok(t) if t.text.is_empty() => me.set_check(false, "Listening model replied but recognized no words. Try speaking clearly or choose another model.", Some(false)),
                        Ok(t) => me.set_check(false, &format!("Transcription checked: \u{201c}{}\u{201d}. This sample was not sent to chat.", t.text), Some(true)),
                        Err(e) => me.set_check(false, &format!("Test needs attention \u{b7} {e}"), Some(false)),
                    }
                });
            }
            AudioCheckKind::Speaking => {
                self.set_check(true, "Preparing the saved speaking voice\u{2026}", None);
                let me = self.clone();
                let _ = std::thread::Builder::new().name("NabyScape speaking check".into()).spawn(move || {
                    let options = me.local_options();
                    let outcome = (|| -> Result<(), String> {
                        let job = Arc::new(Job::new()?);
                        let mut speaker = me.make_speaker(&options, &job, &|_| {})?;
                        let pcm = speaker.synthesize(SAMPLE_SENTENCE, &AtomicBool::new(false))?;
                        let playback = Playback::start(|_| {})?;
                        me.set_check(true, "Playing sample \u{b7} your microphone is off.", None);
                        if !playback.queue.push(&pcm) {
                            return Err("The sample exceeded the playback limit.".into());
                        }
                        let deadline = Instant::now() + Duration::from_secs(60);
                        while playback.queue.pending_ms() > 0 && Instant::now() < deadline {
                            std::thread::sleep(Duration::from_millis(50));
                        }
                        Ok(())
                    })();
                    match outcome {
                        Ok(()) => me.set_check(false, "Speaking sample finished. If you heard it, your selected voice and speaker work. Listening was not tested.", Some(true)),
                        Err(e) => me.set_check(false, &format!("Test needs attention \u{b7} {e}"), Some(false)),
                    }
                });
            }
        }
    }
}

fn trim_lines(lines: &mut Vec<VoiceLine>) {
    if lines.len() > MAX_LINES {
        let extra = lines.len() - MAX_LINES;
        lines.drain(..extra);
    }
}

/// Whistle's lease as a boxed transcriber (keeps the lease alive with the conversation).
struct WhistleAdapter(Option<whistle::WhistleLease>);

impl Transcriber for WhistleAdapter {
    fn transcribe(
        &mut self,
        pcm: &[f32],
        language: Option<&str>,
        keywords: &[String],
    ) -> Result<whistle::Transcript, String> {
        self.0
            .as_mut()
            .ok_or_else(|| "Whistle is not loaded.".to_owned())?
            .transcribe(pcm, language, keywords)
    }
}

/// The chat provider as the pipeline's replier.
struct ChatReplier {
    turn: SpokenTurn,
    cancel: Arc<AtomicBool>,
}

impl Replier for ChatReplier {
    fn reply(
        &mut self,
        text: &str,
        on_text: &mut dyn FnMut(&str) -> bool,
        _: &AtomicBool,
    ) -> Result<(), String> {
        self.turn.reply(text, on_text, &self.cancel)
    }
}

/// The `VoiceHost` the views get: the runtime plus the window's chat.
pub struct VoiceBridge {
    pub runtime: VoiceRuntime,
    pub turn: SpokenTurn,
}

impl VoiceHost for VoiceBridge {
    fn snapshot(&self) -> Arc<VoiceSnapshot> {
        self.runtime.snapshot()
    }
    fn start(&self) {
        self.runtime.start(self.turn.clone());
    }
    fn end(&self) {
        self.runtime.end();
    }
    fn stop(&self) {
        self.runtime.stop();
    }
    fn send_text(&self, text: &str) {
        self.runtime.send_text(text);
    }
    fn microphones(&self) -> Vec<MicChoice> {
        microphones::list()
            .into_iter()
            .map(|m| MicChoice {
                id: m.id,
                name: m.name,
            })
            .collect()
    }
    fn mic_test(&self, on: bool) {
        if on {
            self.runtime.start_mic_test(None);
        } else {
            self.runtime.cancel_checks();
            self.runtime.notify();
        }
    }
    fn mic_state(&self) -> MicTest {
        lock(&self.runtime.inner.mic_test).0.clone()
    }
    fn set_wake_listening(&self, on: bool) {
        self.runtime.configure_wake();
        if !on {
            let action = lock(&self.runtime.inner.wake).explicit_stop();
            self.runtime.apply_wake_action(action);
        }
        self.runtime.notify();
    }
    fn recognizers(&self) -> Vec<Recognizer> {
        wake::recognizers()
    }
    fn speech(&self) -> SpeechPanel {
        self.runtime
            .inner
            .installer
            .panel(&self.runtime.local_options().local_voice)
    }
    fn scan_hardware(&self) {
        self.runtime.inner.installer.scan();
    }
    fn install_pack(&self, asr: &str, tts: &str, gpu: bool) {
        if self.runtime.active() {
            self.runtime.end();
        }
        self.runtime.inner.installer.install(asr, tts, gpu);
    }
    fn remove_model(&self, id: &str, tts: bool) {
        let runtime = self.runtime.clone();
        let release: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            if runtime.active() {
                runtime.end();
                // Wait for the conversation to release its files.
                for _ in 0..100 {
                    if !runtime.active() {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        });
        self.runtime.inner.installer.remove(id, tts, release);
    }
    fn cancel_speech(&self) {
        self.runtime.inner.installer.cancel();
    }
    fn run_audio_check(&self, kind: AudioCheckKind) {
        self.runtime.run_check(kind);
    }
    fn audio_check(&self) -> AudioCheck {
        lock(&self.runtime.inner.check).clone()
    }
    fn whistle_status(&self) -> String {
        whistle::status_text(&self.runtime.inner.installer.whistle_dir())
    }
    fn whistle_ready(&self) -> bool {
        whistle::availability(&self.runtime.inner.installer.whistle_dir()).is_ok()
    }
    fn install_whistle(&self) {
        self.runtime.inner.installer.install_whistle();
    }
    fn piper_status(&self, voice: &str) -> String {
        let installer = &self.runtime.inner.installer;
        match tts_by_id(voice) {
            Ok(_) if installer.busy() => installer.storage().status(),
            Ok(v) => install::piper_status_text(installer.storage(), v),
            Err(_) => "Choose a Piper voice.".into(),
        }
    }
    fn piper_ready(&self, voice: &str) -> bool {
        tts_by_id(voice)
            .is_ok_and(|v| install::piper_ready(self.runtime.inner.installer.storage(), v))
    }
    fn install_piper(&self, voice: &str) {
        if self.runtime.active() {
            self.runtime.end();
        }
        self.runtime.inner.installer.install_piper(voice);
    }
}

/// Used by the app shell to tell the readiness summary about speech work.
pub fn speech_service_state(
    runtime: &VoiceRuntime,
) -> naby_core::assistant::readiness::SpeechService {
    let options = runtime.local_options();
    let local = &options.local_voice;
    let storage: &Arc<SpeechStorage> = runtime.inner.installer.storage();
    let installed = match (own_asr(local), own_tts(local)) {
        (Ok(a), Ok(t)) => install::installed(storage, &a, &t, local.managed_gpu),
        _ => false,
    };
    let _ = asr_by_id;
    naby_core::assistant::readiness::SpeechService {
        busy: runtime.inner.installer.busy(),
        status: storage.status(),
        installed,
    }
}
