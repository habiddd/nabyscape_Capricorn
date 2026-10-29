//! The private speech worker (`nabyscape.exe --speech-worker`): loads the pinned Sherpa-ONNX
//! v1.13.8 C API and one voice model, then answers sentence requests over stdin/stdout frames
//! (`AssistantSpeechWorker.Run`, `AssistantSpeechNative`). The native engine is never loaded in
//! the UI process; when the session ends the parent closes the pipe and the worker exits, which
//! frees all model memory. Layouts follow the pinned `c-api.h`.

use std::ffi::{c_char, c_void, CStr, CString};
use std::io::{stdin, stdout};
use std::path::{Path, PathBuf};

use windows::core::{PCSTR, PCWSTR};
use windows::Win32::Foundation::{FreeLibrary, HMODULE};
use windows::Win32::System::LibraryLoader::{
    GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
    LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR,
};

use super::dsp::{f32_to_pcm16, wav_encode};
use super::local::{read_frame, write_frame, WorkerSetup};

const PINNED_VERSION: &str = "1.13.8";

#[repr(C)]
#[derive(Clone, Copy)]
struct Vits {
    model: *const c_char,
    lexicon: *const c_char,
    tokens: *const c_char,
    data: *const c_char,
    noise: f32,
    noise_w: f32,
    length: f32,
    dict: *const c_char,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct Matcha {
    acoustic: *const c_char,
    vocoder: *const c_char,
    lexicon: *const c_char,
    tokens: *const c_char,
    data: *const c_char,
    noise: f32,
    length: f32,
    dict: *const c_char,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct Kokoro {
    model: *const c_char,
    voices: *const c_char,
    tokens: *const c_char,
    data: *const c_char,
    length: f32,
    dict: *const c_char,
    lexicon: *const c_char,
    lang: *const c_char,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct Kitten {
    model: *const c_char,
    voices: *const c_char,
    tokens: *const c_char,
    data: *const c_char,
    length: f32,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct Zipvoice {
    tokens: *const c_char,
    encoder: *const c_char,
    decoder: *const c_char,
    vocoder: *const c_char,
    data: *const c_char,
    lexicon: *const c_char,
    feat: f32,
    shift: f32,
    rms: f32,
    guidance: f32,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct Pocket {
    flow: *const c_char,
    main: *const c_char,
    encoder: *const c_char,
    decoder: *const c_char,
    conditioner: *const c_char,
    vocab: *const c_char,
    scores: *const c_char,
    cache: i32,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct Supertonic {
    duration: *const c_char,
    text: *const c_char,
    vector: *const c_char,
    vocoder: *const c_char,
    json: *const c_char,
    indexer: *const c_char,
    voice: *const c_char,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct Model {
    vits: Vits,
    threads: i32,
    debug: i32,
    provider: *const c_char,
    matcha: Matcha,
    kokoro: Kokoro,
    kitten: Kitten,
    zipvoice: Zipvoice,
    pocket: Pocket,
    supertonic: Supertonic,
}
#[repr(C)]
struct Config {
    model: Model,
    rules: *const c_char,
    sentences: i32,
    fars: *const c_char,
    silence: f32,
}
#[repr(C)]
struct Generation {
    silence: f32,
    speed: f32,
    speaker: i32,
    reference: *const f32,
    reference_length: i32,
    reference_rate: i32,
    reference_text: *const c_char,
    steps: i32,
    extra: *const c_char,
}
#[repr(C)]
struct Audio {
    samples: *const f32,
    count: i32,
    rate: i32,
}

type Create = unsafe extern "C" fn(*const Config) -> *mut c_void;
type Generate = unsafe extern "C" fn(
    *mut c_void,
    *const c_char,
    *const Generation,
    *const c_void,
    *mut c_void,
) -> *const Audio;
type Destroy = unsafe extern "C" fn(*mut c_void);
type DestroyAudio = unsafe extern "C" fn(*const Audio);
type Version = unsafe extern "C" fn() -> *const c_char;

fn zeroed<T: Copy>() -> T {
    // SAFETY: only used for the plain-old-data config structs above, where all-zero (null
    // pointers, 0 numbers) is the "unset" value the C API expects.
    unsafe { std::mem::zeroed() }
}

struct Engine {
    library: HMODULE,
    engine: *mut c_void,
    generate: Generate,
    destroy: Destroy,
    destroy_audio: DestroyAudio,
    strings: Vec<CString>,
    setup: WorkerSetup,
}

impl Drop for Engine {
    fn drop(&mut self) {
        // SAFETY: the engine and library were created in `load` and are released once.
        unsafe {
            if !self.engine.is_null() {
                (self.destroy)(self.engine);
            }
            let _ = FreeLibrary(self.library);
        }
    }
}

fn weights(folder: &Path) -> Result<String, String> {
    for name in ["model.int8.onnx", "model.onnx"] {
        if folder.join(name).is_file() {
            return Ok(name.to_owned());
        }
    }
    let mut found: Option<String> = None;
    for entry in std::fs::read_dir(folder)
        .map_err(|_| "The voice model file is missing.".to_owned())?
        .flatten()
    {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(".onnx") {
            if found.is_some() {
                return Err("Choose a voice folder with one model file.".into());
            }
            found = Some(name);
        }
    }
    found.ok_or_else(|| "The voice model file is missing.".into())
}

impl Engine {
    fn load(setup: WorkerSetup) -> Result<Engine, String> {
        let library_path = PathBuf::from(&setup.library);
        if library_path
            .file_name()
            .is_none_or(|n| n != "sherpa-onnx-c-api.dll")
            || !library_path.is_file()
        {
            return Err("Prepare the pinned speech engine again.".into());
        }
        let wide: Vec<u16> = library_path
            .as_os_str()
            .to_string_lossy()
            .encode_utf16()
            .chain(Some(0))
            .collect();
        // SAFETY: absolute path to the pinned DLL; its dependencies sit in its own folder (the
        // engine's `bin` folder is the sibling).
        let library = unsafe {
            LoadLibraryExW(
                PCWSTR(wide.as_ptr()),
                None,
                LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
            )
        }
        .map_err(|_| "The speech engine could not be loaded. Prepare it again.".to_owned())?;
        let symbol = |name: &'static [u8]| -> Result<usize, String> {
            // SAFETY: NUL-terminated export name on a loaded module.
            unsafe { GetProcAddress(library, PCSTR(name.as_ptr())) }
                .map(|f| f as usize)
                .ok_or_else(|| {
                    "The speech engine is missing a function. Prepare it again.".to_owned()
                })
        };
        let mut engine = Engine {
            library,
            engine: std::ptr::null_mut(),
            // Placeholders replaced below; never called before assignment.
            generate: unsafe_fn_placeholder_generate,
            destroy: unsafe_fn_placeholder_destroy,
            destroy_audio: unsafe_fn_placeholder_destroy_audio,
            strings: Vec::new(),
            setup: setup.clone(),
        };
        let version: Version = {
            let f = symbol(b"SherpaOnnxGetVersionStr\0")?;
            // SAFETY: the export has the documented signature.
            unsafe { std::mem::transmute::<usize, Version>(f) }
        };
        // SAFETY: returns a static NUL-terminated string.
        let text = unsafe { CStr::from_ptr(version()) }
            .to_string_lossy()
            .into_owned();
        if text.trim_start_matches('v') != PINNED_VERSION {
            return Err(
                "The speech engine version differs from the pinned catalog. Prepare it again."
                    .into(),
            );
        }
        // SAFETY: the exports have the documented signatures of the pinned c-api.h.
        unsafe {
            engine.generate = std::mem::transmute::<usize, Generate>(symbol(
                b"SherpaOnnxOfflineTtsGenerateWithConfig\0",
            )?);
            engine.destroy =
                std::mem::transmute::<usize, Destroy>(symbol(b"SherpaOnnxDestroyOfflineTts\0")?);
            engine.destroy_audio = std::mem::transmute::<usize, DestroyAudio>(symbol(
                b"SherpaOnnxDestroyOfflineTtsGeneratedAudio\0",
            )?);
        }
        let create: Create = {
            let f = symbol(b"SherpaOnnxCreateOfflineTts\0")?;
            // SAFETY: documented signature.
            unsafe { std::mem::transmute::<usize, Create>(f) }
        };
        let folder = PathBuf::from(&setup.folder);
        let text_ptr = |s: &str, strings: &mut Vec<CString>| -> *const c_char {
            let c = CString::new(s).unwrap_or_default();
            let p = c.as_ptr();
            strings.push(c);
            p
        };
        let mut strings: Vec<CString> = Vec::new();
        let file = |name: &str, strings: &mut Vec<CString>| {
            text_ptr(&folder.join(name).display().to_string(), strings)
        };
        let mut model: Model = zeroed();
        model.threads =
            (std::thread::available_parallelism().map_or(2, |n| n.get()) / 2).clamp(1, 4) as i32;
        model.provider = text_ptr("cpu", &mut strings);
        let _ = (
            zeroed::<Vits>(),
            zeroed::<Matcha>(),
            zeroed::<Zipvoice>(),
            zeroed::<Pocket>(),
        );
        if setup.engine == "supertonic" {
            model.supertonic = Supertonic {
                duration: file("duration_predictor.int8.onnx", &mut strings),
                text: file("text_encoder.int8.onnx", &mut strings),
                vector: file("vector_estimator.int8.onnx", &mut strings),
                vocoder: file("vocoder.int8.onnx", &mut strings),
                json: file("tts.json", &mut strings),
                indexer: file("unicode_indexer.bin", &mut strings),
                voice: file("voice.bin", &mut strings),
            };
        } else {
            let weights = weights(&folder)?;
            if setup.engine == "kitten" {
                model.kitten = Kitten {
                    model: file(&weights, &mut strings),
                    voices: file("voices.bin", &mut strings),
                    tokens: file("tokens.txt", &mut strings),
                    data: file("espeak-ng-data", &mut strings),
                    length: 1.0,
                };
            } else if setup.engine == "kokoro" {
                let lexicons: Vec<String> = ["lexicon-us-en.txt", "lexicon-zh.txt"]
                    .iter()
                    .filter(|n| folder.join(n).is_file())
                    .map(|n| folder.join(n).display().to_string())
                    .collect();
                model.kokoro = Kokoro {
                    model: file(&weights, &mut strings),
                    voices: file("voices.bin", &mut strings),
                    tokens: file("tokens.txt", &mut strings),
                    data: file("espeak-ng-data", &mut strings),
                    length: 1.0,
                    lexicon: text_ptr(&lexicons.join(","), &mut strings),
                    lang: text_ptr(
                        if setup.language == "zh" {
                            "zh"
                        } else {
                            "en-us"
                        },
                        &mut strings,
                    ),
                    dict: if folder.join("dict").is_dir() {
                        file("dict", &mut strings)
                    } else {
                        std::ptr::null()
                    },
                };
            } else {
                return Err("This voice does not support the persistent native engine.".into());
            }
        }
        let config = Config {
            model,
            rules: std::ptr::null(),
            sentences: 1,
            fars: std::ptr::null(),
            silence: 0.2,
        };
        // SAFETY: `config` and every string it points to (kept alive in `strings`) are valid for
        // the call.
        let handle = unsafe { create(&config) };
        engine.strings = strings;
        if handle.is_null() {
            return Err(
                "The voice model could not be loaded. Check the files or choose another voice."
                    .into(),
            );
        }
        engine.engine = handle;
        Ok(engine)
    }

    fn speak(&self, text: &str) -> Result<Vec<u8>, String> {
        if text.trim().is_empty() || text.chars().count() > 2000 {
            return Err("The voice segment is empty or too long.".into());
        }
        let c_text = CString::new(text).map_err(|_| "Invalid text.".to_owned())?;
        let extra = CString::new(serde_json::json!({"lang": self.setup.language}).to_string())
            .unwrap_or_default();
        let config = Generation {
            silence: 0.2,
            speed: 1.0,
            speaker: self.setup.speaker,
            reference: std::ptr::null(),
            reference_length: 0,
            reference_rate: 0,
            reference_text: std::ptr::null(),
            steps: self.setup.steps,
            extra: extra.as_ptr(),
        };
        // SAFETY: the engine handle is live; the strings outlive the call; the returned audio
        // is read within its stated count and then destroyed.
        unsafe {
            let audio = (self.generate)(
                self.engine,
                c_text.as_ptr(),
                &config,
                std::ptr::null(),
                std::ptr::null_mut(),
            );
            if audio.is_null() {
                return Err("The voice returned no audio.".into());
            }
            let a = &*audio;
            let result = if a.samples.is_null()
                || !(8000..=48000).contains(&a.rate)
                || a.count <= 0
                || i64::from(a.count) > i64::from(a.rate) * 120
            {
                Err("The generated voice exceeds its audio limit.".to_owned())
            } else {
                let samples = std::slice::from_raw_parts(a.samples, a.count as usize);
                Ok(wav_encode(a.rate as u32, &f32_to_pcm16(samples)))
            };
            (self.destroy_audio)(audio);
            result
        }
    }
}

unsafe extern "C" fn unsafe_fn_placeholder_generate(
    _: *mut c_void,
    _: *const c_char,
    _: *const Generation,
    _: *const c_void,
    _: *mut c_void,
) -> *const Audio {
    std::ptr::null()
}
unsafe extern "C" fn unsafe_fn_placeholder_destroy(_: *mut c_void) {}
unsafe extern "C" fn unsafe_fn_placeholder_destroy_audio(_: *const Audio) {}

/// Runs the worker until the parent closes the pipe. Returns the process exit code.
pub fn run() -> i32 {
    let mut input = stdin().lock();
    let mut output = stdout().lock();
    let reply_error = |output: &mut dyn std::io::Write| {
        let _ = write_frame(output, &[1]);
    };
    let setup: WorkerSetup = match read_frame(&mut input, 32768)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
    {
        Some(s) => s,
        None => return 1,
    };
    let engine = match Engine::load(setup) {
        Ok(e) => e,
        Err(_) => {
            // Never send native errors, local model paths or speech content anywhere.
            reply_error(&mut output);
            return 1;
        }
    };
    if write_frame(&mut output, &[0]).is_err() {
        return 1;
    }
    loop {
        let Ok(bytes) = read_frame(&mut input, 8000) else {
            return 0;
        };
        let Ok(text) = String::from_utf8(bytes) else {
            reply_error(&mut output);
            return 1;
        };
        match engine.speak(&text) {
            Ok(wav) if wav.len() <= 12 * 1024 * 1024 => {
                let mut reply = Vec::with_capacity(wav.len() + 1);
                reply.push(0);
                reply.extend_from_slice(&wav);
                if write_frame(&mut output, &reply).is_err() {
                    return 0;
                }
            }
            _ => {
                reply_error(&mut output);
                return 1;
            }
        }
    }
}
