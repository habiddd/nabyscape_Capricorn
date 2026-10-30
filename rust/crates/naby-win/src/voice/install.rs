//! Speech pack install and removal (`AssistantSpeechInstall`, `AssistantSpeechRemoval`), over the
//! layout and safe-deletion rules in `naby_core::assistant::storage`.
//!
//! Everything optional lives under `%LOCALAPPDATA%\NabyScape\local-speech`, outside the app, and
//! is never loaded at startup. Downloads are pinned: the size is checked against the catalog, the
//! SHA-256 is verified before anything is moved or extracted, archives are unpacked into a
//! staging folder with path and link checks, and the folder is listed in `installed.txt` only
//! after a complete extraction. Whistle and the default Piper voice ship beside the app; Piper is
//! unpacked from those files on first use, with no download.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use naby_core::assistant::models::LocalVoiceOptions;
use naby_core::assistant::speech::{
    asr_by_id, asr_models, tts_by_id, tts_models, AsrModel, Asset, Hardware, TtsModel,
    KOKORO_FOLDER,
};
use naby_core::assistant::storage::{storage_size, SpeechStorage};
use naby_ui::assistant::voice::{InstalledModel, SpeechPanel};
use sha2::{Digest, Sha256};

use super::whistle::{self, LockEntry};
use crate::wake::Waker;

pub const NATIVE_UNAVAILABLE: &str = "Built-in speech packs require a native Windows x64 process and CPU with AVX2/FMA. On ARM64, use Whistle, external local ASR/TTS services or Windows voice.";

/// Largest total size of an unpacked archive.
const MAXIMUM_EXPANDED: u64 = 3 * 1024 * 1024 * 1024;
const MAXIMUM_ENTRIES: usize = 20_000;

// ----- pure checks ---------------------------------------------------------------------------------

fn has_file(path: &Path, size: u64) -> bool {
    fs::metadata(path).is_ok_and(|m| {
        m.is_file()
            && if size == 0 {
                m.len() > 0
            } else {
                m.len() == size
            }
    })
}

/// A runtime or model folder is complete when `installed.txt` lists files that all exist (and
/// `exe`, when given, exists).
pub fn ready(folder: &Path, exe: Option<&Path>) -> bool {
    if exe.is_some_and(|e| !e.is_file()) {
        return false;
    }
    let Ok(list) = fs::read_to_string(folder.join("installed.txt")) else {
        return false;
    };
    list.lines().all(|rel| folder.join(rel).is_file())
}

/// The first file that proves a native listening model was unpacked.
pub fn asr_marker(storage: &SpeechStorage, model: &AsrModel) -> PathBuf {
    let root = storage.qwen_root(model);
    if model.engine == "moonshine" {
        let onnx = root.join("encoder_model.onnx");
        if onnx.is_file() {
            onnx
        } else {
            root.join("encoder_model.ort")
        }
    } else {
        root.join("encoder.int8.onnx")
    }
}

pub fn has_asr_weights(storage: &SpeechStorage, model: &AsrModel) -> bool {
    if !model.native() {
        return has_file(&storage.asr_path(model), model.asset.bytes);
    }
    let folder = storage.root().join("models").join(&model.id);
    let marker = asr_marker(storage, model);
    let present = if model.source_path.is_empty() {
        ready(&folder, Some(&marker))
    } else {
        marker.is_file()
    };
    present
        && model.files.as_ref().is_none_or(|files| {
            files
                .iter()
                .all(|f| has_file(&storage.qwen_root(model).join(&f.path), f.asset.bytes))
        })
}

fn voice_directory(storage: &SpeechStorage, model: &TtsModel) -> PathBuf {
    if !model.source_path.is_empty() {
        return PathBuf::from(&model.source_path);
    }
    if model.engine == "kokoro" {
        // The extracted folder that holds voices.bin.
        let root = storage.kokoro_root();
        return find_folder_with(&root, "voices.bin").unwrap_or_else(|| root.join(KOKORO_FOLDER));
    }
    storage.voice_root(model).join(&model.folder)
}

/// The folder (at any depth) that contains `file`.
pub fn find_folder_with(root: &Path, file: &str) -> Option<PathBuf> {
    let mut pending = vec![root.to_path_buf()];
    while let Some(folder) = pending.pop() {
        let Ok(entries) = fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.file_name().is_some_and(|n| n == file) {
                return Some(folder);
            }
        }
    }
    None
}

pub fn has_tts_weights(storage: &SpeechStorage, model: &TtsModel) -> bool {
    if model.id == "windows" {
        return true;
    }
    if model.package.is_some() {
        let ok = !model.source_path.is_empty() || ready(&storage.voice_root(model), None);
        return ok && voice_directory(storage, model).is_dir();
    }
    let onnx = storage.tts_path(model);
    let json = PathBuf::from(format!("{}.json", onnx.display()));
    let card = PathBuf::from(format!("{}.MODEL_CARD", onnx.display()));
    has_file(&onnx, model.bytes)
        && has_file(&json, 0)
        && (!model.source_path.is_empty() || card.is_file())
}

/// Directory a native voice loads from (used by the speech worker).
pub fn tts_directory(storage: &SpeechStorage, model: &TtsModel) -> PathBuf {
    voice_directory(storage, model)
}

/// Whether the engines and files for a pack are all present (`Installed`).
pub fn installed(storage: &SpeechStorage, asr: &AsrModel, tts: &TtsModel, gpu: bool) -> bool {
    if !native_supported() {
        return false;
    }
    let sherpa = |tts_exe: bool| ready(&storage.sherpa_root(), Some(&storage.sherpa_exe(tts_exe)));
    let asr_ok = if asr.native() {
        sherpa(false) && has_asr_weights(storage, asr)
    } else {
        ready(
            &storage.engine_folder(false),
            Some(&storage.whisper_exe(false)),
        ) && (!gpu
            || ready(
                &storage.engine_folder(true),
                Some(&storage.whisper_exe(true)),
            ))
            && has_file(&storage.asr_path(asr), asr.asset.bytes)
    };
    let tts_ok = tts.id == "windows"
        || if tts.package.is_some() {
            sherpa(true) && has_tts_weights(storage, tts)
        } else {
            ready(&storage.piper_folder(), Some(&storage.piper_exe()))
                && has_tts_weights(storage, tts)
        };
    asr_ok && tts_ok
}

/// Native packs need a 64-bit x86 process with AVX2 and FMA (`AssistantSpeechPlatform`).
pub fn native_supported() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        std::arch::is_x86_feature_detected!("avx2") && std::arch::is_x86_feature_detected!("fma")
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        false
    }
}

/// Lowercase hex SHA-256.
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Hash of a file by streaming.
pub fn hash_file(path: &Path) -> std::io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 128 * 1024];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// Bytes an install needs on disk (`need` in `Install`).
pub fn bytes_needed(asr: &AsrModel, tts: &TtsModel, gpu: bool) -> u64 {
    use naby_core::assistant::speech as s;
    let sherpa = s::sherpa_engine().bytes;
    let multi = if asr.files.is_some() {
        2
    } else if asr.native() {
        4
    } else {
        1
    };
    let mut need = asr.download_bytes() * multi
        + s::whisper_cpu().bytes * 3
        + if gpu { s::whisper_gpu().bytes * 4 } else { 0 }
        + s::visual_cpp_redistributable().bytes
        + 512 * 1024 * 1024;
    if let Some(p) = &tts.package {
        need += p.bytes * 4 + sherpa * 5;
    } else if tts.id != "windows" {
        need += tts.bytes + s::piper_engine().bytes * 4;
    }
    if asr.native() {
        need += sherpa * 5;
    }
    need
}

// ----- download and unpack -------------------------------------------------------------------------

/// Cancellation or failure while installing.
#[derive(Debug)]
pub enum Fail {
    Cancelled,
    Message(String),
}

impl From<std::io::Error> for Fail {
    fn from(e: std::io::Error) -> Self {
        Fail::Message(format!("Disk error: {e}"))
    }
}

fn msg(text: impl Into<String>) -> Fail {
    Fail::Message(text.into())
}

/// Downloads `asset` to `target`: HTTPS only (unless `allow_http`, tests), exact size, SHA-256
/// verified before the file appears. An existing verified file is reused.
pub fn download(
    asset: &Asset,
    target: &Path,
    title: &str,
    cancel: &AtomicBool,
    allow_http: bool,
    progress: &mut dyn FnMut(&str, f64),
) -> Result<(), Fail> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    if has_file(target, asset.bytes) {
        progress(&format!("Verifying {title}\u{2026}"), 0.0);
        if hash_file(target)?.eq_ignore_ascii_case(&asset.sha256) {
            return Ok(());
        }
    }
    if !allow_http && !asset.url.starts_with("https://") {
        return Err(msg("The download address is not HTTPS."));
    }
    let partial = PathBuf::from(format!("{}.partial", target.display()));
    let result = (|| -> Result<(), Fail> {
        let response = naby_core::http::agent()
            .get(&asset.url)
            .call()
            .map_err(|e| msg(format!("Check your internet connection and retry. {e}")))?;
        if !allow_http && !response.get_url().starts_with("https://") {
            return Err(msg("The download redirected outside HTTPS."));
        }
        if asset.bytes > 0 {
            if let Some(declared) = response
                .header("content-length")
                .and_then(|v| v.parse::<u64>().ok())
            {
                if declared != asset.bytes {
                    return Err(msg("Download size differs from the pinned model catalog."));
                }
            }
        }
        let cap = if asset.bytes > 0 {
            asset.bytes
        } else {
            128 * 1024
        };
        let mut reader = response.into_reader();
        let mut out = fs::File::create(&partial)?;
        let mut hasher = Sha256::new();
        let mut buffer = vec![0u8; 128 * 1024];
        let (mut total, mut last) = (0u64, Instant::now());
        loop {
            if cancel.load(Ordering::SeqCst) {
                return Err(Fail::Cancelled);
            }
            let n = reader.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            total += n as u64;
            if total > cap {
                return Err(msg("Download exceeds its expected size."));
            }
            out.write_all(&buffer[..n])?;
            hasher.update(&buffer[..n]);
            if last.elapsed() > Duration::from_millis(150) {
                last = Instant::now();
                let done = total as f64 / 1_048_576.0;
                let text = if asset.bytes > 0 {
                    format!(
                        "Downloading {title} \u{b7} {done:.1} / {:.1} MB",
                        asset.bytes as f64 / 1_048_576.0
                    )
                } else {
                    format!("Downloading {title} \u{b7} {done:.1} MB")
                };
                progress(
                    &text,
                    if asset.bytes > 0 {
                        total as f64 * 100.0 / asset.bytes as f64
                    } else {
                        0.0
                    },
                );
            }
        }
        out.flush()?;
        drop(out);
        let hash: String = hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        if (asset.bytes > 0 && total != asset.bytes) || !hash.eq_ignore_ascii_case(&asset.sha256) {
            return Err(msg("Checksum verification failed. Retry the download."));
        }
        fs::rename(&partial, target)?;
        Ok(())
    })();
    let _ = fs::remove_file(&partial);
    result
}

/// Downloads Whistle's model and engine for `arch` from the lock `entries` into `folder`, each
/// verified against its pinned size and SHA-256 before it appears there.
pub fn install_whistle_files(
    entries: &[LockEntry],
    arch: &str,
    folder: &Path,
    cancel: &AtomicBool,
    allow_http: bool,
    progress: &mut dyn FnMut(&str, f64),
) -> Result<(), Fail> {
    // Resolve both files first so nothing is downloaded when one cannot be installed.
    let mut wanted = Vec::new();
    for name in [whistle::MODEL_FILE, whistle::ENGINE_FILE] {
        let entry = entries
            .iter()
            .find(|e| e.name == name && (e.arch == arch || e.arch == "any"))
            .ok_or_else(|| {
                msg(format!(
                    "Whistle is not available for this processor ({arch})."
                ))
            })?;
        if !entry.pinned() {
            return Err(msg("Whistle's checksums are not pinned in this build."));
        }
        wanted.push((name, entry));
    }
    for (name, entry) in wanted {
        let asset = Asset {
            url: entry.url.clone(),
            bytes: entry.bytes,
            sha256: entry.sha256.clone(),
        };
        download(
            &asset,
            &folder.join(name),
            name,
            cancel,
            allow_http,
            progress,
        )?;
    }
    Ok(())
}

// ----- built-in Piper ------------------------------------------------------------------------------

/// The Piper engine archive shipped beside the executable (the pinned zip itself).
pub const PIPER_BUNDLE_ENGINE: &str = "piper_windows_amd64.zip";
/// The voice shipped beside the executable.
pub const BUNDLED_PIPER_VOICE: &str = "es_MX-ald-x_low";
/// Every Piper file the setup ships, as listed in `piper.lock`.
pub const PIPER_BUNDLE: [&str; 4] = [
    PIPER_BUNDLE_ENGINE,
    "es_MX-ald-x_low.onnx",
    "es_MX-ald-x_low.onnx.json",
    "es_MX-ald-x_low.onnx.MODEL_CARD",
];

/// The executable's folder, where the setup ships Whistle and Piper.
pub fn bundle_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
}

/// Piper and `voice` are unpacked and verified in the speech storage (x64 only).
pub fn piper_ready(storage: &SpeechStorage, voice: &TtsModel) -> bool {
    cfg!(target_arch = "x86_64")
        && ready(&storage.piper_folder(), Some(&storage.piper_exe()))
        && has_tts_weights(storage, voice)
}

/// `dir` holds the engine archive and all three files of `voice`.
pub fn piper_bundled(dir: Option<&Path>, voice: &TtsModel) -> bool {
    let Some(dir) = dir else { return false };
    [
        PIPER_BUNDLE_ENGINE.to_owned(),
        format!("{}.onnx", voice.id),
        format!("{}.onnx.json", voice.id),
        format!("{}.onnx.MODEL_CARD", voice.id),
    ]
    .iter()
    .all(|name| dir.join(name).is_file())
}

/// Copies a shipped file to where the installer expects it; `download` then verifies it by size
/// and SHA-256 (and fetches it again if the copy is wrong).
fn seed(bundle: Option<&Path>, name: &str, target: &Path) -> Result<(), Fail> {
    let Some(from) = bundle.map(|b| b.join(name)).filter(|f| f.is_file()) else {
        return Ok(());
    };
    if !target.exists() {
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(from, target)?;
    }
    Ok(())
}

/// Installs the Piper engine and `voice` into the speech storage. Files shipped in `bundle` are
/// copied and verified, so nothing is downloaded when the setup shipped them.
pub fn install_piper_files(
    storage: &SpeechStorage,
    voice: &TtsModel,
    bundle: Option<&Path>,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&str, f64),
) -> Result<(), Fail> {
    use naby_core::assistant::speech as sp;
    if !cfg!(target_arch = "x86_64") {
        return Err(msg(
            "Piper runs on x64 Windows only; the Windows voice speaks on this PC.",
        ));
    }
    if !ready(&storage.piper_folder(), Some(&storage.piper_exe())) {
        // `install_package` names the archive after the folder: piper-2023.11.14-2.zip.
        let archive = storage.downloads().join("piper-2023.11.14-2.zip");
        seed(bundle, PIPER_BUNDLE_ENGINE, &archive)?;
    }
    install_package(
        storage,
        &sp::piper_engine(),
        &storage.piper_folder(),
        Some(&storage.piper_exe()),
        "Piper speech engine",
        cancel,
        progress,
    )?;
    let onnx = storage.tts_path(voice);
    let files = [
        (".onnx", "", voice.bytes, &voice.hash, voice.name.as_str()),
        (
            ".onnx.json",
            ".json",
            0,
            &voice.config_hash,
            "Voice configuration",
        ),
        (
            "MODEL_CARD",
            ".MODEL_CARD",
            0,
            &voice.card_hash,
            "Voice attribution and model card",
        ),
    ];
    for (remote, local, bytes, hash, title) in files {
        let target = PathBuf::from(format!("{}{local}", onnx.display()));
        seed(bundle, &format!("{}.onnx{local}", voice.id), &target)?;
        let asset = Asset {
            url: sp::voice_url(voice, remote),
            bytes,
            sha256: hash.clone(),
        };
        download(&asset, &target, title, cancel, false, progress)?;
    }
    fs::write(storage.root().join("SOURCES.txt"), SOURCES)?;
    Ok(())
}

/// The line the Models page and Voice setup show for the built-in Piper voice.
pub fn piper_status_text(storage: &SpeechStorage, voice: &TtsModel) -> String {
    if !cfg!(target_arch = "x86_64") {
        "Piper runs on x64 Windows only; the Windows voice speaks on this PC.".into()
    } else if piper_ready(storage, voice) {
        "Ready \u{b7} runs on this PC only while Naby speaks.".into()
    } else if piper_bundled(bundle_dir().as_deref(), voice) {
        "Included with NabyScape \u{b7} prepared on first use, no download.".into()
    } else {
        format!(
            "Not installed \u{b7} about {:.0} MB download.",
            (voice.bytes + naby_core::assistant::speech::piper_engine().bytes) as f64 / 1e6
        )
    }
}

/// Unpacks a zip into `staging` with the same safety rules as the C# installer.
pub fn extract_zip(archive: &Path, staging: &Path, cancel: &AtomicBool) -> Result<(), Fail> {
    let file = fs::File::open(archive)?;
    let mut zip = zip::ZipArchive::new(file).map_err(|_| msg("The runtime archive is damaged."))?;
    if zip.len() > 10_000 {
        return Err(msg("Runtime archive has too many entries."));
    }
    let mut expanded = 0u64;
    for i in 0..zip.len() {
        if cancel.load(Ordering::SeqCst) {
            return Err(Fail::Cancelled);
        }
        let mut entry = zip
            .by_index(i)
            .map_err(|_| msg("The runtime archive is damaged."))?;
        expanded += entry.size();
        if expanded > MAXIMUM_EXPANDED {
            return Err(msg("Runtime archive is too large."));
        }
        let unix = entry.unix_mode().unwrap_or(0);
        if unix & 0xF000 == 0xA000 {
            return Err(msg("Unsafe runtime archive path."));
        }
        let Some(relative) = entry.enclosed_name() else {
            return Err(msg("Unsafe runtime archive path."));
        };
        let path = staging.join(relative);
        if entry.is_dir() {
            fs::create_dir_all(&path)?;
            continue;
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut out = fs::File::create(&path)?;
        std::io::copy(&mut entry, &mut out)?;
    }
    Ok(())
}

/// Checks a `tar -tf` listing before extraction: no absolute paths, drive letters or `..`.
pub fn check_tar_listing(listing: &str) -> Result<(), Fail> {
    let names: Vec<&str> = listing.lines().filter(|l| !l.is_empty()).collect();
    if names.len() > MAXIMUM_ENTRIES {
        return Err(msg("Too many files in the model package."));
    }
    for name in names {
        let name = name.trim_end_matches('\r');
        let unsafe_path = name.contains(':')
            || name.starts_with(['/', '\\'])
            || name.split(['/', '\\']).any(|part| part == "..");
        if unsafe_path {
            return Err(msg("Unsafe speech-package path."));
        }
    }
    Ok(())
}

/// Checks a `tar -tvf` listing: only regular files and folders (no links or devices).
pub fn check_tar_details(listing: &str) -> Result<(), Fail> {
    if listing
        .lines()
        .filter(|l| !l.is_empty())
        .any(|l| !l.starts_with('-') && !l.starts_with('d'))
    {
        return Err(msg(
            "Model packages containing links or devices are not accepted.",
        ));
    }
    Ok(())
}

/// Unpacks a `.tar.bz2` with the tar that ships with Windows 10+, after checking its listing.
#[cfg(windows)]
pub fn extract_tar(archive: &Path, staging: &Path, cancel: &AtomicBool) -> Result<(), Fail> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    let tar = PathBuf::from(root).join("System32").join("tar.exe");
    if !tar.is_file() {
        return Err(msg("These model packs require the tar component included with current Windows. Update Windows, or use a Whisper/Piper pack."));
    }
    let run = |args: &[&std::ffi::OsStr]| -> Result<String, Fail> {
        let out = Command::new(&tar)
            .args(args)
            .creation_flags(0x0800_0000)
            .stdin(Stdio::null())
            .output()
            .map_err(|_| msg("Could not unpack the speech model."))?;
        if !out.status.success() {
            return Err(msg("Windows could not unpack the verified model. Check available disk space and retry."));
        }
        if out.stdout.len() > 2 * 1024 * 1024 {
            return Err(msg("The package listing is too large."));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    };
    use std::ffi::OsStr;
    check_tar_listing(&run(&[OsStr::new("-tf"), archive.as_os_str()])?)?;
    check_tar_details(&run(&[OsStr::new("-tvf"), archive.as_os_str()])?)?;
    if cancel.load(Ordering::SeqCst) {
        return Err(Fail::Cancelled);
    }
    run(&[
        OsStr::new("-xf"),
        archive.as_os_str(),
        OsStr::new("-C"),
        staging.as_os_str(),
    ])?;
    scan_expanded(staging, cancel)
}

#[cfg(not(windows))]
pub fn extract_tar(_: &Path, _: &Path, _: &AtomicBool) -> Result<(), Fail> {
    Err(msg(NATIVE_UNAVAILABLE))
}

/// Rejects links and oversized trees in an unpacked folder.
pub fn scan_expanded(root: &Path, cancel: &AtomicBool) -> Result<(), Fail> {
    let mut size = 0u64;
    let mut pending = vec![root.to_path_buf()];
    while let Some(folder) = pending.pop() {
        for entry in fs::read_dir(&folder)?.flatten() {
            if cancel.load(Ordering::SeqCst) {
                return Err(Fail::Cancelled);
            }
            let meta = fs::symlink_metadata(entry.path())?;
            if meta.file_type().is_symlink() {
                return Err(msg("Unsafe link in speech package."));
            }
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                if meta.file_attributes() & 0x400 != 0 {
                    return Err(msg("Unsafe link in speech package."));
                }
            }
            if meta.is_dir() {
                pending.push(entry.path());
            } else {
                size += meta.len();
                if size > MAXIMUM_EXPANDED {
                    return Err(msg("The unpacked model is too large."));
                }
            }
        }
    }
    Ok(())
}

/// Lists every file under `root` relative to it.
pub fn list_files(root: &Path) -> std::io::Result<Vec<String>> {
    let mut out = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(folder) = pending.pop() {
        for entry in fs::read_dir(&folder)?.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if let Ok(rel) = path.strip_prefix(root) {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    out.sort();
    Ok(out)
}

/// Installs one runtime or model package into `folder` through a staging folder.
pub fn install_package(
    storage: &SpeechStorage,
    asset: &Asset,
    folder: &Path,
    must_contain: Option<&Path>,
    title: &str,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&str, f64),
) -> Result<(), Fail> {
    if ready(folder, must_contain) {
        return Ok(());
    }
    let tar = asset.url.to_lowercase().ends_with(".tar.bz2");
    let name = folder
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let archive = storage
        .downloads()
        .join(format!("{name}{}", if tar { ".tar.bz2" } else { ".zip" }));
    download(asset, &archive, title, cancel, false, progress)?;
    progress(&format!("Installing {title}\u{2026}"), 100.0);
    let staging = PathBuf::from(format!(
        "{}.staging-{}",
        folder.display(),
        naby_core::assistant::new_id()
    ));
    let result = (|| -> Result<(), Fail> {
        fs::create_dir_all(&staging)?;
        if tar {
            extract_tar(&archive, &staging, cancel)?;
        } else {
            extract_zip(&archive, &staging, cancel)?;
        }
        if let Some(exe) = must_contain {
            if let Ok(rel) = exe.strip_prefix(folder) {
                if !staging.join(rel).is_file() {
                    return Err(msg(
                        "The package does not contain its expected model/engine file.",
                    ));
                }
            }
        }
        let files = list_files(&staging)?;
        fs::write(staging.join("installed.txt"), files.join("\n"))?;
        if folder.exists() {
            fs::remove_dir_all(folder)?;
        }
        if let Some(parent) = folder.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::rename(&staging, folder)?;
        Ok(())
    })();
    if staging.exists() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

/// Installs a multi-file model (`ModelFiles`).
fn install_model_files(
    storage: &SpeechStorage,
    model: &AsrModel,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&str, f64),
) -> Result<(), Fail> {
    let folder = storage.root().join("models").join(&model.id);
    if ready(&folder, Some(&asr_marker(storage, model))) {
        return Ok(());
    }
    let staging = PathBuf::from(format!(
        "{}.staging-{}",
        folder.display(),
        naby_core::assistant::new_id()
    ));
    let result = (|| -> Result<(), Fail> {
        let mut names = Vec::new();
        for file in model.files.as_deref().unwrap_or_default() {
            let cached = storage.downloads().join(&model.id).join(&file.path);
            download(
                &file.asset,
                &cached,
                &format!("{} \u{b7} {}", model.name, file.path),
                cancel,
                false,
                progress,
            )?;
            let target = staging.join(&file.path);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&cached, &target)?;
            names.push(file.path.clone());
        }
        fs::write(staging.join("installed.txt"), names.join("\n"))?;
        if folder.exists() {
            fs::remove_dir_all(&folder)?;
        }
        fs::rename(&staging, &folder)?;
        Ok(())
    })();
    if staging.exists() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

const SOURCES: &str = "Optional speech components downloaded by NabyScape:\nWhisper.cpp v1.8.7 (MIT): https://github.com/ggml-org/whisper.cpp/tree/v1.8.7\nWhisper weights (MIT): https://huggingface.co/ggerganov/whisper.cpp\nPiper native engine 2023.11.14-2 (MIT): https://github.com/rhasspy/piper/tree/2023.11.14-2\nPiper uses eSpeak NG (GPL): https://github.com/espeak-ng/espeak-ng and ONNX Runtime (MIT): https://github.com/microsoft/onnxruntime\nPiper voice terms and dataset attribution: see each .MODEL_CARD beside the voice file.\nSherpa-ONNX v1.13.8 (Apache-2.0): https://github.com/k2-fsa/sherpa-onnx/tree/v1.13.8\nQwen3-ASR (Apache-2.0): https://github.com/QwenLM/Qwen3-ASR\nKokoro v1.1 (Apache-2.0): https://huggingface.co/hexgrad/Kokoro-82M-v1.1-zh\nMoonshine v2: https://huggingface.co/UsefulSensors (see bundled LICENSE)\nKittenTTS nano v0.8 (Apache-2.0): https://huggingface.co/KittenML/kitten-tts-nano-0.8-int8\nSupertonic 3 (OpenRAIL-M model): https://huggingface.co/Supertone/supertonic-3/blob/main/LICENSE\nSherpa packages include ONNX Runtime and eSpeak NG components; see their bundled license notices.\nNVIDIA CUDA runtime terms: https://docs.nvidia.com/cuda/eula/index.html\nMicrosoft Visual C++ runtime terms: https://aka.ms/VisualStudioRedistribution\n";

/// Runs the whole install of a pack on the calling thread.
pub fn install_pack(
    storage: &SpeechStorage,
    hardware: &Hardware,
    asr: &AsrModel,
    tts: &TtsModel,
    prefer_gpu: bool,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&str, f64),
) -> Result<(), Fail> {
    use naby_core::assistant::speech as s;
    if !native_supported() || !hardware.supported {
        return Err(msg(NATIVE_UNAVAILABLE));
    }
    if !hardware.fits_pack(asr, tts) {
        return Err(msg(format!(
            "This pack needs a budget of {:.1} GB total RAM and {:.1} GB available. Close other apps or choose a smaller model/voice.",
            asr.ram_gb,
            s::pack_free_gb(asr, tts)
        )));
    }
    let gpu = prefer_gpu && hardware.use_gpu(asr);
    let need = bytes_needed(asr, tts, gpu);
    fs::create_dir_all(storage.root())?;
    if let Some(free) = free_disk_bytes(storage.root()) {
        if free < need {
            return Err(msg(format!(
                "Free at least {:.1} GB on the drive for this pack and installation space.",
                need as f64 / 1_073_741_824.0
            )));
        }
    }
    ensure_cpp_runtime(storage, cancel, progress)?;
    let sherpa = |tts_exe: bool| storage.sherpa_exe(tts_exe);
    if asr.native() {
        install_package(
            storage,
            &s::sherpa_engine(),
            &storage.sherpa_root(),
            Some(&sherpa(false)),
            "Sherpa native speech engine",
            cancel,
            progress,
        )?;
        if asr.files.is_some() {
            install_model_files(storage, asr, cancel, progress)?;
        } else {
            install_package(
                storage,
                &asr.asset,
                &storage.root().join("models").join(&asr.id),
                Some(&asr_marker(storage, asr)),
                &asr.name,
                cancel,
                progress,
            )?;
        }
    } else {
        install_package(
            storage,
            &s::whisper_cpu(),
            &storage.engine_folder(false),
            Some(&storage.whisper_exe(false)),
            "Whisper CPU engine",
            cancel,
            progress,
        )?;
        if gpu {
            install_package(
                storage,
                &s::whisper_gpu(),
                &storage.engine_folder(true),
                Some(&storage.whisper_exe(true)),
                "Whisper GPU engine",
                cancel,
                progress,
            )?;
        }
        download(
            &asr.asset,
            &storage.asr_path(asr),
            &asr.name,
            cancel,
            false,
            progress,
        )?;
    }
    if let Some(package) = &tts.package {
        install_package(
            storage,
            &s::sherpa_engine(),
            &storage.sherpa_root(),
            Some(&sherpa(true)),
            "Sherpa native speech engine",
            cancel,
            progress,
        )?;
        install_package(
            storage,
            package,
            &storage.voice_root(tts),
            None,
            &tts.name,
            cancel,
            progress,
        )?;
    } else if tts.id != "windows" {
        install_piper_files(storage, tts, bundle_dir().as_deref(), cancel, progress)?;
    }
    fs::write(storage.root().join("SOURCES.txt"), SOURCES)?;
    Ok(())
}

// ----- platform pieces ---------------------------------------------------------------------------------

#[cfg(windows)]
fn free_disk_bytes(path: &Path) -> Option<u64> {
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let wide: Vec<u16> = path
        .display()
        .to_string()
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let mut free = 0u64;
    // SAFETY: `wide` is NUL terminated; the out pointer is a live local.
    unsafe { GetDiskFreeSpaceExW(PCWSTR(wide.as_ptr()), Some(&mut free), None, None) }.ok()?;
    Some(free)
}

#[cfg(not(windows))]
fn free_disk_bytes(_: &Path) -> Option<u64> {
    None
}

/// Microsoft's x64 C++ runtime that the native engines need (>= 14.44.35211).
#[cfg(windows)]
pub fn cpp_runtime_present() -> bool {
    use windows::core::w;
    use windows::Win32::System::Registry::{
        RegCloseKey, RegGetValueW, RegOpenKeyExW, HKEY, HKEY_LOCAL_MACHINE, KEY_WOW64_64KEY,
        RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
    };
    // SAFETY: plain registry reads into local buffers.
    unsafe {
        let mut key = HKEY::default();
        if RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            w!(r"SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\x64"),
            None,
            windows::Win32::System::Registry::KEY_READ | KEY_WOW64_64KEY,
            &mut key,
        )
        .is_err()
        {
            return false;
        }
        let mut installed = 0u32;
        let mut size = 4u32;
        let ok = RegGetValueW(
            key,
            None,
            w!("Installed"),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut installed as *mut u32).cast()),
            Some(&mut size),
        );
        let mut buffer = [0u16; 64];
        let mut bytes = (buffer.len() * 2) as u32;
        let version_ok = RegGetValueW(
            key,
            None,
            w!("Version"),
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut bytes),
        );
        let _ = RegCloseKey(key);
        if ok.is_err() || installed != 1 || version_ok.is_err() {
            return false;
        }
        let text = String::from_utf16_lossy(&buffer)
            .trim_matches(char::from(0))
            .trim_start_matches(['v', 'V'])
            .to_owned();
        version_at_least(&text, (14, 44, 35211))
    }
}

#[cfg(not(windows))]
pub fn cpp_runtime_present() -> bool {
    false
}

/// `14.44.35211.0` >= `(14, 44, 35211)`.
pub fn version_at_least(text: &str, min: (u32, u32, u32)) -> bool {
    let mut parts = text
        .split('.')
        .map(|p| p.trim().parse::<u32>().unwrap_or(0));
    let got = (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    );
    got >= min
}

#[cfg(windows)]
fn ensure_cpp_runtime(
    storage: &SpeechStorage,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&str, f64),
) -> Result<(), Fail> {
    if cpp_runtime_present() {
        return Ok(());
    }
    let asset = naby_core::assistant::speech::visual_cpp_redistributable();
    let setup = storage.downloads().join("VC_redist.x64.exe");
    download(
        &asset,
        &setup,
        "Microsoft Visual C++ runtime",
        cancel,
        false,
        progress,
    )?;
    progress(
        "Installing Microsoft Visual C++ runtime. Complete the Windows administrator prompt.",
        100.0,
    );
    // Hash-verified above; Windows shows its own elevation prompt (Start-Process -Verb RunAs).
    let status = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            &format!(
                "$p = Start-Process -FilePath '{}' -ArgumentList '/install /quiet /norestart' -Verb RunAs -Wait -PassThru; exit $p.ExitCode",
                setup.display().to_string().replace('\'', "''")
            ),
        ])
        .status()
        .map_err(|_| msg("Could not start Microsoft runtime setup."))?;
    match status.code() {
        Some(3010) => Err(msg("Microsoft runtime installed. Restart Windows, then choose Install & use again.")),
        Some(0) if cpp_runtime_present() => Ok(()),
        _ => Err(msg("Microsoft runtime setup did not finish successfully. Complete its Windows administrator prompt, then retry.")),
    }
}

#[cfg(not(windows))]
fn ensure_cpp_runtime(
    _: &SpeechStorage,
    _: &AtomicBool,
    _: &mut dyn FnMut(&str, f64),
) -> Result<(), Fail> {
    Err(msg(NATIVE_UNAVAILABLE))
}

/// Scans RAM, threads and an NVIDIA GPU (`SpeechHardware.Scan`).
pub fn scan_hardware() -> Hardware {
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get() as u32);
    let (ram, free) = memory_gb();
    let (gpu, vram, free_vram, cuda) = nvidia();
    Hardware {
        cpu: std::env::var("PROCESSOR_IDENTIFIER").unwrap_or_else(|_| "CPU".into()),
        threads,
        ram_gb: ram,
        free_gb: free,
        gpu,
        vram_gb: vram,
        free_vram_gb: free_vram,
        cuda,
        supported: native_supported(),
    }
}

#[cfg(windows)]
fn memory_gb() -> (f64, f64) {
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut status = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    // SAFETY: `status` is a live struct with its length set.
    if unsafe { GlobalMemoryStatusEx(&mut status) }.is_ok() {
        const GB: f64 = 1_073_741_824.0;
        (
            status.ullTotalPhys as f64 / GB,
            status.ullAvailPhys as f64 / GB,
        )
    } else {
        (0.0, 0.0)
    }
}

#[cfg(not(windows))]
fn memory_gb() -> (f64, f64) {
    (0.0, 0.0)
}

/// `(name, vram GB, free vram GB, cuda)` from `nvidia-smi` when present.
fn nvidia() -> (String, f64, f64, bool) {
    let mut command = std::process::Command::new("nvidia-smi");
    command.args([
        "--query-gpu=name,memory.total,memory.free",
        "--format=csv,noheader,nounits",
    ]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let Ok(output) = command.output() else {
        return ("No NVIDIA GPU detected".into(), 0.0, 0.0, false);
    };
    parse_nvidia(&String::from_utf8_lossy(&output.stdout))
        .unwrap_or_else(|| ("No NVIDIA GPU detected".into(), 0.0, 0.0, false))
}

/// Parses the first CSV line `name, total MiB, free MiB`.
pub fn parse_nvidia(text: &str) -> Option<(String, f64, f64, bool)> {
    let line = text.lines().find(|l| !l.trim().is_empty())?;
    let mut parts = line.split(',').map(str::trim);
    let name = parts.next()?.to_owned();
    let total: f64 = parts.next()?.parse().ok()?;
    let free: f64 = parts.next()?.parse().ok()?;
    Some((name, total / 1024.0, free / 1024.0, true))
}

// ----- the service ------------------------------------------------------------------------------------------

struct State {
    status: String,
    progress: f64,
    hardware: Option<Hardware>,
    installing: bool,
    scanning: bool,
    cancel: Arc<AtomicBool>,
}

/// Install and removal work for the settings pages. Threads exist only while it runs.
pub struct SpeechInstaller {
    storage: Arc<SpeechStorage>,
    state: Mutex<State>,
    waker: Waker,
}

impl SpeechInstaller {
    pub fn new(data_dir: &Path, waker: Waker) -> Arc<Self> {
        Arc::new(Self {
            storage: Arc::new(SpeechStorage::new(data_dir)),
            state: Mutex::new(State {
                status: "Scan your PC to find a suitable listening model.".into(),
                progress: 0.0,
                hardware: None,
                installing: false,
                scanning: false,
                cancel: Arc::new(AtomicBool::new(false)),
            }),
            waker,
        })
    }

    pub fn storage(&self) -> &Arc<SpeechStorage> {
        &self.storage
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn report(&self, status: &str, progress: f64) {
        {
            let mut s = self.state();
            s.status = status.to_owned();
            s.progress = progress.clamp(0.0, 100.0);
        }
        (self.waker)();
    }

    pub fn busy(&self) -> bool {
        let s = self.state();
        s.installing || s.scanning || self.storage.busy()
    }

    pub fn hardware(&self) -> Option<Hardware> {
        self.state().hardware.clone()
    }

    pub fn cancel(&self) {
        self.state().cancel.store(true, Ordering::SeqCst);
    }

    /// Whether a catalog pack is fully installed.
    pub fn pack_installed(&self, asr_id: &str, tts_id: &str, gpu: bool) -> bool {
        match (asr_by_id(asr_id), tts_by_id(tts_id)) {
            (Ok(a), Ok(t)) => installed(&self.storage, a, t, gpu),
            _ => false,
        }
    }

    pub fn scan(self: &Arc<Self>) {
        {
            let mut s = self.state();
            if s.scanning || s.installing {
                return;
            }
            s.scanning = true;
        }
        self.report("Scanning this PC\u{2026}", 0.0);
        let me = self.clone();
        let spawned = std::thread::Builder::new()
            .name("NabyScape hardware scan".into())
            .spawn(move || {
                let hardware = scan_hardware();
                let text = if hardware.supported {
                    format!("{}\nScan finished.", hardware.summary())
                } else {
                    NATIVE_UNAVAILABLE.to_owned()
                };
                {
                    let mut s = me.state();
                    s.hardware = Some(hardware);
                    s.scanning = false;
                }
                me.report(&text, 0.0);
            });
        if spawned.is_err() {
            self.state().scanning = false;
        }
    }

    pub fn install(self: &Arc<Self>, asr_id: &str, tts_id: &str, gpu: bool) {
        let (Ok(asr), Ok(tts)) = (asr_by_id(asr_id), tts_by_id(tts_id)) else {
            self.report("Choose a catalog model first.", 0.0);
            return;
        };
        let cancel = {
            let mut s = self.state();
            if s.installing || s.scanning || !self.storage.begin() {
                return;
            }
            s.installing = true;
            s.cancel = Arc::new(AtomicBool::new(false));
            s.cancel.clone()
        };
        self.report("Checking available memory and disk space\u{2026}", 0.0);
        let me = self.clone();
        let spawned = std::thread::Builder::new().name("NabyScape speech install".into()).spawn(move || {
            let hardware = me.hardware().unwrap_or_else(scan_hardware);
            me.state().hardware = Some(hardware.clone());
            let mut progress = |text: &str, value: f64| me.report(text, value);
            let result = install_pack(&me.storage, &hardware, asr, tts, gpu, &cancel, &mut progress);
            me.storage.end();
            me.state().installing = false;
            match result {
                Ok(()) => me.report("Installed and verified.", 100.0),
                Err(Fail::Cancelled) => me.report(
                    "Installation cancelled. Completed verified downloads are kept; choose Install & use to retry.",
                    0.0,
                ),
                Err(Fail::Message(m)) => me.report(&format!("Installation failed: {m}"), 0.0),
            }
        });
        if spawned.is_err() {
            self.storage.end();
            self.state().installing = false;
        }
    }

    /// Where Settings installs Whistle (see `whistle::locate`).
    pub fn whistle_dir(&self) -> PathBuf {
        self.storage.root().join(whistle::INSTALL_FOLDER)
    }

    /// Downloads Whistle on a worker; progress shows on the Models page.
    pub fn install_whistle(self: &Arc<Self>) {
        let cancel = {
            let mut s = self.state();
            if s.installing || s.scanning || !self.storage.begin() {
                return;
            }
            s.installing = true;
            s.cancel = Arc::new(AtomicBool::new(false));
            s.cancel.clone()
        };
        self.report("Preparing the Whistle download\u{2026}", 0.0);
        let me = self.clone();
        let spawned = std::thread::Builder::new()
            .name("NabyScape Whistle install".into())
            .spawn(move || {
                let result = whistle::Lock::embedded()
                    .map_err(|e| msg(e.message()))
                    .and_then(|lock| {
                        let mut progress = |text: &str, value: f64| me.report(text, value);
                        install_whistle_files(
                            lock.entries(),
                            whistle::current_arch(),
                            &me.whistle_dir(),
                            &cancel,
                            false,
                            &mut progress,
                        )
                    });
                me.storage.end();
                me.state().installing = false;
                match result {
                    Ok(()) => me.report("Whistle installed and verified.", 100.0),
                    Err(Fail::Cancelled) => me.report(
                        "Whistle installation cancelled. Choose Install Whistle to retry.",
                        0.0,
                    ),
                    Err(Fail::Message(m)) => {
                        me.report(&format!("Whistle installation failed: {m}"), 0.0)
                    }
                }
            });
        if spawned.is_err() {
            self.storage.end();
            self.state().installing = false;
        }
    }

    /// Installs Piper and `voice_id` on a worker (offline when the setup shipped the files).
    pub fn install_piper(self: &Arc<Self>, voice_id: &str) {
        let Ok(voice) = tts_by_id(voice_id) else {
            self.report("Choose a Piper voice.", 0.0);
            return;
        };
        let cancel = {
            let mut s = self.state();
            if s.installing || s.scanning || !self.storage.begin() {
                return;
            }
            s.installing = true;
            s.cancel = Arc::new(AtomicBool::new(false));
            s.cancel.clone()
        };
        self.report("Preparing Piper\u{2026}", 0.0);
        let me = self.clone();
        let spawned = std::thread::Builder::new()
            .name("NabyScape Piper install".into())
            .spawn(move || {
                let mut progress = |text: &str, value: f64| me.report(text, value);
                let result =
                    ensure_cpp_runtime(&me.storage, &cancel, &mut progress).and_then(|()| {
                        let bundle = bundle_dir();
                        install_piper_files(
                            &me.storage,
                            voice,
                            bundle.as_deref(),
                            &cancel,
                            &mut progress,
                        )
                    });
                me.storage.end();
                me.state().installing = false;
                match result {
                    Ok(()) => me.report("Piper installed and verified.", 100.0),
                    Err(Fail::Cancelled) => me.report(
                        "Piper installation cancelled. Choose Install Piper to retry.",
                        0.0,
                    ),
                    Err(Fail::Message(m)) => {
                        me.report(&format!("Piper installation failed: {m}"), 0.0)
                    }
                }
            });
        if spawned.is_err() {
            self.storage.end();
            self.state().installing = false;
        }
    }

    /// Removes a model on a worker. `release` ends any voice session holding the files.
    pub fn remove(self: &Arc<Self>, id: &str, tts: bool, release: Arc<dyn Fn() + Send + Sync>) {
        if self.busy() {
            return;
        }
        let me = self.clone();
        let id = id.to_owned();
        let spawned = std::thread::Builder::new()
            .name("NabyScape speech removal".into())
            .spawn(move || {
                me.report("Uninstalling\u{2026}", 0.0);
                let release_fn = || -> naby_core::assistant::Result<()> {
                    release();
                    Ok(())
                };
                let _ = me.storage.uninstall_model(&id, tts, Some(&release_fn));
                let status = me.storage.status();
                me.report(&status, 0.0);
            });
        let _ = spawned;
    }

    /// The Models page's data.
    pub fn panel(&self, local: &LocalVoiceOptions) -> SpeechPanel {
        let (status, progress, hardware, installing) = {
            let s = self.state();
            (
                s.status.clone(),
                s.progress,
                s.hardware.clone(),
                s.installing || s.scanning,
            )
        };
        let list = |tts: bool| -> Vec<InstalledModel> {
            let entries: Vec<(String, String)> = if tts {
                tts_models()
                    .iter()
                    .filter(|m| m.id != "windows")
                    .map(|m| (m.id.clone(), m.name.clone()))
                    .collect()
            } else {
                asr_models()
                    .iter()
                    .map(|m| (m.id.clone(), m.name.clone()))
                    .collect()
            };
            entries
                .into_iter()
                .filter(|(id, _)| self.storage.has_managed_model(id, tts).unwrap_or(false))
                .map(|(id, name)| {
                    let bytes = self.storage.managed_model_bytes(&id, tts).unwrap_or(0);
                    InstalledModel {
                        selected: self.storage.model_in_use(local, &id, tts),
                        size: storage_size(bytes),
                        id,
                        name,
                    }
                })
                .collect()
        };
        SpeechPanel {
            busy: installing || self.storage.busy(),
            status,
            progress,
            can_cancel: self.state().installing,
            hardware,
            installed_asr: list(false),
            installed_tts: list(true),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;

    use super::*;

    fn serve(body: Vec<u8>, declared: Option<usize>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = listener.accept() {
                let mut buf = [0u8; 2048];
                let _ = s.read(&mut buf);
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    declared.unwrap_or(body.len())
                );
                let _ = s.write_all(head.as_bytes());
                let _ = s.write_all(&body);
            }
        });
        format!("http://127.0.0.1:{port}/file")
    }

    fn asset(url: String, body: &[u8]) -> Asset {
        Asset {
            url,
            bytes: body.len() as u64,
            sha256: sha256_hex(body),
        }
    }

    fn whistle_entry(name: &str, arch: &str, body: &[u8]) -> LockEntry {
        LockEntry {
            name: name.into(),
            arch: arch.into(),
            url: serve(body.to_vec(), None),
            bytes: body.len() as u64,
            sha256: sha256_hex(body),
        }
    }

    #[test]
    #[ignore = "downloads about 18 MB from Hugging Face"]
    fn whistle_installs_from_the_embedded_lock() {
        let dir = tempfile::tempdir().unwrap();
        let lock = whistle::Lock::embedded().unwrap();
        install_whistle_files(
            lock.entries(),
            whistle::current_arch(),
            dir.path(),
            &AtomicBool::new(false),
            false,
            &mut |_, _| {},
        )
        .unwrap();
        assert_eq!(
            whistle::availability_in(dir.path(), &lock, whistle::current_arch(), false),
            Ok(())
        );
    }

    #[test]
    fn piper_lock_matches_the_catalog() {
        use naby_core::assistant::speech as sp;
        let lock = whistle::Lock::parse(include_str!("../../../../piper.lock")).unwrap();
        let names: Vec<&str> = lock.entries().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, PIPER_BUNDLE);
        assert!(lock.entries().iter().all(|e| e.arch == "x64" && e.pinned()));
        let engine = sp::piper_engine();
        let first = &lock.entries()[0];
        assert_eq!(
            (first.url.as_str(), first.bytes, first.sha256.as_str()),
            (engine.url.as_str(), engine.bytes, engine.sha256.as_str())
        );
        let v = sp::tts_by_id(BUNDLED_PIPER_VOICE).unwrap();
        let wanted = [
            (".onnx", v.bytes, &v.hash),
            (".onnx.json", 4877, &v.config_hash),
            ("MODEL_CARD", 589, &v.card_hash),
        ];
        for (entry, (suffix, bytes, hash)) in lock.entries()[1..].iter().zip(wanted) {
            assert_eq!(entry.url, sp::voice_url(v, suffix));
            assert_eq!((entry.bytes, &entry.sha256), (bytes, hash));
        }
        for name in PIPER_BUNDLE {
            assert!(crate::install::PAYLOAD_FILES.contains(&name), "{name}");
        }
    }

    #[test]
    fn piper_bundled_needs_all_four_files() {
        let dir = tempfile::tempdir().unwrap();
        let voice = tts_by_id(BUNDLED_PIPER_VOICE).unwrap();
        assert!(!piper_bundled(None, voice));
        for name in PIPER_BUNDLE {
            assert!(!piper_bundled(Some(dir.path()), voice));
            fs::write(dir.path().join(name), b"x").unwrap();
        }
        assert!(piper_bundled(Some(dir.path()), voice));
        assert!(!piper_bundled(
            Some(dir.path()),
            tts_by_id("en_US-amy-low").unwrap()
        ));
    }

    /// Reads the files from `NABY_PIPER_BUNDLE` when set (then nothing may be downloaded);
    /// writes the spoken sample to `NABY_PIPER_WAV` when set.
    #[test]
    #[ignore = "downloads about 45 MB (or reads NABY_PIPER_BUNDLE) and runs the real piper.exe"]
    #[cfg(windows)]
    fn piper_installs_and_speaks_spanish() {
        use super::super::dsp::wav_encode;
        use super::super::local::{Job, PiperTts};
        use super::super::pipeline::Speaker;
        let dir = tempfile::tempdir().unwrap();
        let storage = SpeechStorage::new(dir.path());
        let voice = tts_by_id(BUNDLED_PIPER_VOICE).unwrap();
        let bundle = std::env::var_os("NABY_PIPER_BUNDLE").map(PathBuf::from);
        let mut seen = Vec::new();
        install_piper_files(
            &storage,
            voice,
            bundle.as_deref(),
            &AtomicBool::new(false),
            &mut |text, _| seen.push(text.to_owned()),
        )
        .unwrap();
        assert!(piper_ready(&storage, voice));
        if bundle.is_some() {
            assert!(
                !seen.iter().any(|t| t.starts_with("Downloading")),
                "downloaded: {seen:?}"
            );
        }
        let mut tts = PiperTts {
            exe: storage.piper_exe(),
            model: storage.tts_path(voice),
            speaker: 0,
            length_scale: 1.0,
            job: Arc::new(Job::new().unwrap()),
        };
        let pcm = tts
            .synthesize(
                "Hola, soy Naby. \u{bf}En qu\u{e9} te puedo ayudar hoy?",
                &AtomicBool::new(false),
            )
            .unwrap();
        // The speaker returns 24 kHz mono 16-bit playback audio.
        let seconds = pcm.len() as f64 / 2.0 / 24_000.0;
        println!("spoke {seconds:.2} s ({} bytes of PCM)", pcm.len());
        assert!(seconds > 0.5, "{seconds}");
        if let Some(path) = std::env::var_os("NABY_PIPER_WAV") {
            fs::write(path, wav_encode(24_000, &pcm)).unwrap();
        }
    }

    #[test]
    fn whistle_installs_the_model_and_this_arch_engine_verified() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("whistle");
        let entries = [
            whistle_entry("whistle.cact", "any", b"model"),
            whistle_entry("needle.exe", "x64", b"engine x64"),
        ];
        install_whistle_files(
            &entries,
            "x64",
            &folder,
            &AtomicBool::new(false),
            true,
            &mut |_, _| {},
        )
        .unwrap();
        assert_eq!(fs::read(folder.join("whistle.cact")).unwrap(), b"model");
        assert_eq!(fs::read(folder.join("needle.exe")).unwrap(), b"engine x64");

        // No engine for the arch, an unpinned entry, or a wrong hash: refused, nothing written.
        let other = dir.path().join("other");
        let missing = install_whistle_files(
            &entries,
            "arm64",
            &other,
            &AtomicBool::new(false),
            true,
            &mut |_, _| {},
        );
        assert!(matches!(missing, Err(Fail::Message(m)) if m.contains("arm64")));
        let mut unpinned = whistle_entry("whistle.cact", "any", b"model");
        unpinned.sha256 = whistle::UNPINNED.into();
        let refused = install_whistle_files(
            &[unpinned],
            "x64",
            &other,
            &AtomicBool::new(false),
            true,
            &mut |_, _| {},
        );
        assert!(matches!(refused, Err(Fail::Message(m)) if m.contains("pinned")));
        let mut wrong = whistle_entry("whistle.cact", "any", b"model");
        wrong.sha256 = sha256_hex(b"different");
        let bad = install_whistle_files(
            &[wrong],
            "x64",
            &other,
            &AtomicBool::new(false),
            true,
            &mut |_, _| {},
        );
        assert!(bad.is_err());
        assert!(!other.join("whistle.cact").exists());
    }

    #[test]
    fn a_verified_download_appears_only_after_the_hash_matches() {
        let body = b"model bytes".to_vec();
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("models").join("m.bin");
        let a = asset(serve(body.clone(), None), &body);
        let mut seen = Vec::new();
        download(
            &a,
            &target,
            "Model",
            &AtomicBool::new(false),
            true,
            &mut |t, _| seen.push(t.to_owned()),
        )
        .unwrap();
        assert_eq!(fs::read(&target).unwrap(), body);
        assert!(!target.with_extension("bin.partial").exists());
        // An existing verified file is reused without a request (the server is gone).
        download(
            &a,
            &target,
            "Model",
            &AtomicBool::new(false),
            true,
            &mut |_, _| {},
        )
        .unwrap();
        assert!(
            seen.is_empty()
                || seen[0].starts_with("Downloading")
                || seen[0].starts_with("Verifying")
        );
    }

    #[test]
    fn a_wrong_hash_wrong_size_or_non_https_download_is_refused() {
        let body = b"model bytes".to_vec();
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("m.bin");
        let mut bad = asset(serve(body.clone(), None), &body);
        bad.sha256 = sha256_hex(b"something else");
        let Err(Fail::Message(m)) = download(
            &bad,
            &target,
            "Model",
            &AtomicBool::new(false),
            true,
            &mut |_, _| {},
        ) else {
            panic!()
        };
        assert!(m.contains("Checksum"));
        assert!(!target.exists());
        // Declared size differs from the catalog.
        let sized = asset(serve(body.clone(), Some(99)), &body);
        let Err(Fail::Message(m)) = download(
            &sized,
            &target,
            "Model",
            &AtomicBool::new(false),
            true,
            &mut |_, _| {},
        ) else {
            panic!()
        };
        assert!(m.contains("size"), "{m}");
        // HTTP is refused outright in production.
        let plain = asset("http://example.com/x".into(), &body);
        assert!(matches!(
            download(&plain, &target, "Model", &AtomicBool::new(false), false, &mut |_, _| {}),
            Err(Fail::Message(m)) if m.contains("HTTPS")
        ));
    }

    #[test]
    fn cancelling_stops_the_download_and_leaves_no_partial_file() {
        let body = vec![7u8; 400_000];
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("m.bin");
        let a = asset(serve(body.clone(), None), &body);
        let cancel = AtomicBool::new(true);
        assert!(matches!(
            download(&a, &target, "Model", &cancel, true, &mut |_, _| {}),
            Err(Fail::Cancelled)
        ));
        assert!(!target.exists());
        assert!(!dir.path().join("m.bin.partial").exists());
    }

    fn zip_with(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut out);
            for (name, data) in entries {
                w.start_file(*name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                w.write_all(data).unwrap();
            }
            w.finish().unwrap();
        }
        out.into_inner()
    }

    #[test]
    fn zips_unpack_into_staging_and_traversal_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let good = dir.path().join("good.zip");
        fs::write(
            &good,
            zip_with(&[
                ("Release/whisper-server.exe", b"exe"),
                ("Release/ggml.dll", b"dll"),
            ]),
        )
        .unwrap();
        let staging = dir.path().join("staging");
        fs::create_dir_all(&staging).unwrap();
        extract_zip(&good, &staging, &AtomicBool::new(false)).unwrap();
        assert_eq!(
            list_files(&staging).unwrap(),
            vec!["Release/ggml.dll", "Release/whisper-server.exe"]
        );
        let evil = dir.path().join("evil.zip");
        fs::write(&evil, zip_with(&[("../escape.txt", b"x")])).unwrap();
        let other = dir.path().join("other");
        fs::create_dir_all(&other).unwrap();
        assert!(
            matches!(extract_zip(&evil, &other, &AtomicBool::new(false)), Err(Fail::Message(m)) if m.contains("Unsafe"))
        );
        assert!(!dir.path().join("escape.txt").exists());
    }

    #[test]
    fn packages_install_through_staging_and_are_listed_in_installed_txt() {
        let dir = tempfile::tempdir().unwrap();
        let storage = SpeechStorage::new(dir.path());
        let archive = zip_with(&[("Release/whisper-server.exe", b"exe")]);
        let a = asset(serve(archive.clone(), None), &archive);
        // `install_package` insists on HTTPS, so stage the archive where it would cache it.
        let folder = storage.engine_folder(false);
        let cached = storage.downloads().join(format!(
            "{}.zip",
            folder.file_name().unwrap().to_string_lossy()
        ));
        download(
            &a,
            &cached,
            "engine",
            &AtomicBool::new(false),
            true,
            &mut |_, _| {},
        )
        .unwrap();
        let exe = storage.whisper_exe(false);
        install_package(
            &storage,
            &a,
            &folder,
            Some(&exe),
            "engine",
            &AtomicBool::new(false),
            &mut |_, _| {},
        )
        .unwrap();
        assert!(ready(&folder, Some(&exe)));
        assert!(fs::read_to_string(folder.join("installed.txt"))
            .unwrap()
            .contains("Release/whisper-server.exe"));
        assert!(!fs::read_dir(folder.parent().unwrap())
            .unwrap()
            .flatten()
            .any(|e| e.file_name().to_string_lossy().contains("staging")));
        // A package missing its executable leaves nothing behind.
        let other = storage.piper_folder();
        let odd = zip_with(&[("readme.txt", b"hi")]);
        let b = asset(serve(odd.clone(), None), &odd);
        let cached = storage.downloads().join(format!(
            "{}.zip",
            other.file_name().unwrap().to_string_lossy()
        ));
        download(
            &b,
            &cached,
            "piper",
            &AtomicBool::new(false),
            true,
            &mut |_, _| {},
        )
        .unwrap();
        assert!(install_package(
            &storage,
            &b,
            &other,
            Some(&storage.piper_exe()),
            "piper",
            &AtomicBool::new(false),
            &mut |_, _| {}
        )
        .is_err());
        assert!(!other.exists());
    }

    #[test]
    fn tar_listings_reject_escapes_and_links() {
        assert!(check_tar_listing("model/\nmodel/tokens.txt\n").is_ok());
        assert!(check_tar_listing("../evil\n").is_err());
        assert!(check_tar_listing("/abs/path\n").is_err());
        assert!(check_tar_listing("C:/x\n").is_err());
        assert!(check_tar_listing("a/../../b\n").is_err());
        assert!(check_tar_details(
            "drwxr-xr-x 0 u g 0 Jan 1 model/\n-rw-r--r-- 0 u g 5 Jan 1 model/a\n"
        )
        .is_ok());
        assert!(check_tar_details("lrwxrwxrwx 0 u g 0 Jan 1 model/link -> /etc\n").is_err());
    }

    #[test]
    fn readiness_requires_every_listed_file() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path();
        assert!(!ready(f, None));
        fs::write(f.join("a.bin"), b"1").unwrap();
        fs::write(f.join("installed.txt"), "a.bin\nb.bin").unwrap();
        assert!(!ready(f, None));
        fs::write(f.join("b.bin"), b"2").unwrap();
        assert!(ready(f, None));
        assert!(!ready(f, Some(&f.join("missing.exe"))));
    }

    #[test]
    fn catalog_packs_are_not_installed_in_an_empty_folder_and_windows_voice_needs_no_files() {
        let dir = tempfile::tempdir().unwrap();
        let storage = SpeechStorage::new(dir.path());
        let tiny = asr_by_id("tiny").unwrap();
        let windows = tts_by_id("windows").unwrap();
        assert!(!installed(&storage, tiny, windows, false));
        assert!(has_tts_weights(&storage, windows));
        assert!(!has_asr_weights(&storage, tiny));
        // The model file alone is not enough without its engine.
        let path = storage.asr_path(tiny);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, vec![0u8; tiny.asset.bytes as usize]).unwrap();
        assert!(has_asr_weights(&storage, tiny));
        assert!(!installed(&storage, tiny, windows, false));
    }

    #[test]
    fn disk_budgets_and_runtime_versions() {
        let tiny = asr_by_id("tiny").unwrap();
        let windows = tts_by_id("windows").unwrap();
        let small = bytes_needed(tiny, windows, false);
        let gpu = bytes_needed(tiny, windows, true);
        assert!(gpu > small + 1_000_000_000, "the CUDA engine is large");
        assert!(version_at_least("14.44.35211.0", (14, 44, 35211)));
        assert!(version_at_least("14.50.1.0", (14, 44, 35211)));
        assert!(!version_at_least("14.38.33135", (14, 44, 35211)));
        assert_eq!(
            parse_nvidia("NVIDIA GeForce RTX 4070, 12282, 10000\n")
                .unwrap()
                .1
                .round(),
            12.0
        );
        assert!(parse_nvidia("").is_none());
    }

    #[test]
    fn the_installer_panel_lists_installed_models_and_removal_is_guarded() {
        let dir = tempfile::tempdir().unwrap();
        let installer = SpeechInstaller::new(dir.path(), crate::wake::noop());
        let tiny = asr_by_id("tiny").unwrap();
        let path = installer.storage().asr_path(tiny);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"weights").unwrap();
        let mut local = LocalVoiceOptions {
            enabled: true,
            managed: true,
            managed_asr: "tiny".into(),
            ..LocalVoiceOptions::default()
        };
        let panel = installer.panel(&local);
        let model = panel.installed_asr.iter().find(|m| m.id == "tiny").unwrap();
        assert!(model.selected && model.size.ends_with("B"));
        assert!(panel.installed_tts.is_empty());
        // Removal runs on a worker and deletes the files through the guarded storage rules.
        local.managed = false;
        let released = Arc::new(AtomicBool::new(false));
        let flag = released.clone();
        installer.remove(
            "tiny",
            false,
            Arc::new(move || flag.store(true, Ordering::SeqCst)),
        );
        for _ in 0..200 {
            if !path.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!path.exists());
        assert!(
            released.load(Ordering::SeqCst),
            "voice is released before files are deleted"
        );
    }
}
