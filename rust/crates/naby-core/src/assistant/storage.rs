//! Where installed speech models live and how they are removed
//! (`AssistantSpeechInstall` path layout, `AssistantSpeechRemoval.cs`).
//!
//! Downloading and extracting is a host task; this module owns the layout under
//! `%LOCALAPPDATA%\NabyScape\local-speech` and the safe deletion rules: only catalog ids are ever
//! resolved, imported model files and shared engines are never deletion targets, and a link or
//! junction anywhere in a model's storage stops the removal before anything is deleted.

use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use super::models::LocalVoiceOptions;
use super::speech::{asr_by_id, local_path, tts_by_id};
use super::{AssistantError, Result};

/// Folder under the data directory that holds all speech engines and models.
pub const FOLDER: &str = "local-speech";

/// Normalize `.` and `..` components lexically (like `Path.GetFullPath` for absolute paths).
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn same_or_inside(path: &Path, target: &Path) -> bool {
    let (p, t) = (
        normalize(path).to_string_lossy().to_lowercase(),
        normalize(target).to_string_lossy().to_lowercase(),
    );
    p == t || p.starts_with(&format!("{t}{}", std::path::MAIN_SEPARATOR))
}

/// A size for the installed-models list.
pub fn storage_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if bytes < 1024 {
        format!("{bytes} B")
    } else if b < KB * KB {
        format!("{:.1} KB", b / KB)
    } else if b < KB * KB * KB {
        format!("{:.1} MB", b / (KB * KB))
    } else {
        format!("{:.2} GB", b / (KB * KB * KB))
    }
}

/// The managed speech storage plus the "something is changing" state the settings page shows.
pub struct SpeechStorage {
    root: PathBuf,
    busy: AtomicBool,
    testing: AtomicBool,
    status: Mutex<String>,
}

impl SpeechStorage {
    /// `data_dir` is `naby_core::paths::data_dir()`.
    pub fn new(data_dir: &Path) -> Self {
        Self {
            root: data_dir.join(FOLDER),
            busy: AtomicBool::new(false),
            testing: AtomicBool::new(false),
            status: Mutex::new("Scan your PC to find a suitable listening model.".into()),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// An install, removal or speech test is running.
    pub fn busy(&self) -> bool {
        self.busy.load(Ordering::SeqCst) || self.testing.load(Ordering::SeqCst)
    }

    pub fn set_testing(&self, testing: bool) {
        self.testing.store(testing, Ordering::SeqCst);
    }

    pub fn status(&self) -> String {
        self.status.lock().expect("status lock").clone()
    }

    fn report(&self, text: &str) {
        *self.status.lock().expect("status lock") = text.into();
    }

    /// Try to start an exclusive operation (install or removal).
    pub fn begin(&self) -> bool {
        !self.testing.load(Ordering::SeqCst)
            && self
                .busy
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
    }

    pub fn end(&self) {
        self.busy.store(false, Ordering::SeqCst);
    }

    // ----- layout ---------------------------------------------------------------------

    pub fn engine_folder(&self, gpu: bool) -> PathBuf {
        self.root.join("engines").join(if gpu {
            "whisper-1.8.7-cuda12"
        } else {
            "whisper-1.8.7-cpu"
        })
    }

    pub fn whisper_exe(&self, gpu: bool) -> PathBuf {
        self.engine_folder(gpu)
            .join("Release")
            .join("whisper-server.exe")
    }

    pub fn piper_folder(&self) -> PathBuf {
        self.root.join("engines").join("piper-2023.11.14-2")
    }

    pub fn piper_exe(&self) -> PathBuf {
        self.piper_folder().join("piper").join("piper.exe")
    }

    pub fn sherpa_root(&self) -> PathBuf {
        self.root.join("engines").join("sherpa-1.13.8")
    }

    pub fn sherpa_exe(&self, tts: bool) -> PathBuf {
        self.sherpa_root()
            .join(super::speech::SHERPA_FOLDER)
            .join("bin")
            .join(if tts {
                "sherpa-onnx-offline-tts.exe"
            } else {
                "sherpa-onnx-offline.exe"
            })
    }

    pub fn downloads(&self) -> PathBuf {
        self.root.join("downloads")
    }

    pub fn asr_path(&self, model: &super::speech::AsrModel) -> PathBuf {
        if model.source_path.is_empty() {
            self.root
                .join("models")
                .join(format!("ggml-{}.bin", model.id))
        } else {
            PathBuf::from(&model.source_path)
        }
    }

    pub fn tts_path(&self, model: &super::speech::TtsModel) -> PathBuf {
        if model.source_path.is_empty() {
            self.root.join("voices").join(format!("{}.onnx", model.id))
        } else {
            PathBuf::from(&model.source_path)
        }
    }

    pub fn qwen_root(&self, model: &super::speech::AsrModel) -> PathBuf {
        if model.source_path.is_empty() {
            self.root.join("models").join(&model.id).join(&model.folder)
        } else {
            PathBuf::from(&model.source_path)
        }
    }

    pub fn kokoro_root(&self) -> PathBuf {
        self.root.join("voices").join("kokoro-v1.1-int8")
    }

    pub fn voice_root(&self, model: &super::speech::TtsModel) -> PathBuf {
        if model.engine == "kokoro" {
            self.kokoro_root()
        } else {
            self.root.join("voices").join(&model.id)
        }
    }

    // ----- removal --------------------------------------------------------------------

    /// Everything that belongs to a catalog model (weights, caches and partial downloads).
    pub fn removal_paths(&self, id: &str, tts: bool) -> Result<Vec<PathBuf>> {
        let downloads = self.downloads();
        let archive = |url: &str| {
            if url.to_lowercase().ends_with(".tar.bz2") {
                ".tar.bz2"
            } else {
                ".zip"
            }
        };
        let targets: Vec<PathBuf> = if tts {
            let model = tts_by_id(id)?;
            if model.id == "windows" || !model.source_path.is_empty() {
                return Err(AssistantError::invalid(
                    "Windows voices and imported model files cannot be uninstalled here.",
                ));
            }
            match &model.package {
                Some(package) => {
                    let folder = self.voice_root(model);
                    let name = folder
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    vec![
                        folder,
                        downloads.join(format!("{name}{}", archive(&package.url))),
                    ]
                }
                None => {
                    let file = self.tts_path(model);
                    let text = file.to_string_lossy().into_owned();
                    vec![
                        file,
                        PathBuf::from(format!("{text}.json")),
                        PathBuf::from(format!("{text}.MODEL_CARD")),
                    ]
                }
            }
        } else {
            let model = asr_by_id(id)?;
            if !model.source_path.is_empty() {
                return Err(AssistantError::invalid(
                    "Imported model files cannot be uninstalled here.",
                ));
            }
            if model.native() {
                let cache = if model.files.is_some() {
                    String::new()
                } else {
                    archive(&model.asset.url).to_string()
                };
                vec![
                    self.root.join("models").join(&model.id),
                    PathBuf::from(format!(
                        "{}{cache}",
                        downloads.join(&model.id).to_string_lossy()
                    )),
                ]
            } else {
                vec![self.asr_path(model)]
            }
        };
        let mut paths: Vec<PathBuf> = Vec::new();
        for target in targets {
            let partial = PathBuf::from(format!("{}.partial", target.to_string_lossy()));
            for path in [target, partial] {
                let path = normalize(&path);
                if !paths.iter().any(|p| {
                    p.to_string_lossy()
                        .eq_ignore_ascii_case(&path.to_string_lossy())
                }) {
                    paths.push(path);
                }
            }
        }
        Ok(paths)
    }

    /// Whether any file of this model is present.
    pub fn has_managed_model(&self, id: &str, tts: bool) -> Result<bool> {
        if tts && id == "windows" {
            return Ok(false);
        }
        Ok(self.removal_paths(id, tts)?.iter().any(|p| p.exists()))
    }

    /// Disk used by a model including caches. Link validation is the same as for removal, so a
    /// measurement never traverses an external link.
    pub fn managed_model_bytes(&self, id: &str, tts: bool) -> Result<u64> {
        let paths = self.removal_paths(id, tts)?;
        for path in &paths {
            self.validate_removal(path)?;
        }
        let mut bytes = 0u64;
        for path in &paths {
            if path.is_file() {
                bytes += fs::metadata(path).map(|m| m.len()).unwrap_or(0);
            } else if path.is_dir() {
                let mut pending = vec![path.clone()];
                while let Some(folder) = pending.pop() {
                    for entry in fs::read_dir(&folder).map_err(io_error)?.flatten() {
                        let p = entry.path();
                        if p.is_dir() {
                            pending.push(p);
                        } else {
                            bytes += entry.metadata().map(|m| m.len()).unwrap_or(0);
                        }
                    }
                }
            }
        }
        Ok(bytes)
    }

    /// Whether the saved voice setup uses this model (so removing it needs a warning).
    pub fn model_in_use(&self, options: &LocalVoiceOptions, id: &str, tts: bool) -> bool {
        if !options.enabled
            || !options.managed
            || (tts && options.own_models && options.own_tts_engine == "Windows voice")
        {
            return false;
        }
        let Ok(paths) = self.removal_paths(id, tts) else {
            return false;
        };
        let contains = |path: &Path| paths.iter().any(|target| same_or_inside(path, target));
        let own_engine = if tts {
            &options.own_tts_engine
        } else {
            &options.own_asr_engine
        };
        if options.own_models && own_engine != "Installed pack" {
            let own = if tts {
                &options.own_tts_path
            } else {
                &options.own_asr_path
            };
            return !own.trim().is_empty() && local_path(own).is_ok_and(|p| contains(&p));
        }
        if tts {
            return match tts_by_id(&options.managed_tts) {
                Ok(model) => {
                    model.id != "windows"
                        && contains(&if model.package.is_some() {
                            self.voice_root(model)
                        } else {
                            self.tts_path(model)
                        })
                }
                Err(_) => false,
            };
        }
        match asr_by_id(&options.managed_asr) {
            Ok(model) => contains(&if model.native() {
                self.root.join("models").join(&model.id)
            } else {
                self.asr_path(model)
            }),
            Err(_) => false,
        }
    }

    fn validate_removal(&self, path: &Path) -> Result<()> {
        let boundary = normalize(&self.root);
        let target = normalize(path);
        let inside = ["models", "voices", "downloads"]
            .iter()
            .any(|area| target.starts_with(boundary.join(area)) && target != boundary.join(area));
        if !inside {
            return Err(AssistantError::io(
                "The model path is outside managed speech storage.",
            ));
        }
        // Check ancestors and every descendant before any recursive deletion. A directory
        // junction must never turn model removal into external deletion.
        let mut current = Some(target.as_path());
        while let Some(p) = current {
            check_link(p)?;
            if p == boundary {
                break;
            }
            current = p.parent();
        }
        let mut pending = Vec::new();
        if target.is_dir() {
            pending.push(target.clone());
        }
        while let Some(folder) = pending.pop() {
            for entry in fs::read_dir(&folder).map_err(io_error)?.flatten() {
                let p = entry.path();
                check_link(&p)?;
                if p.is_dir() {
                    pending.push(p);
                }
            }
        }
        Ok(())
    }

    /// Delete a model and its caches. `release` runs after validation and before deletion (stop
    /// any voice session that might hold the files); if it fails nothing is deleted.
    ///
    /// Returns `Ok(false)` when another operation is running or the removal failed (the reason is
    /// in [`SpeechStorage::status`]); invalid ids are an error.
    pub fn uninstall_model(
        &self,
        id: &str,
        tts: bool,
        release: Option<&dyn Fn() -> Result<()>>,
    ) -> Result<bool> {
        if self.busy() {
            return Ok(false);
        }
        let paths = self.removal_paths(id, tts)?;
        let name = if tts {
            tts_by_id(id)?.name.clone()
        } else {
            asr_by_id(id)?.name.clone()
        };
        if !self.begin() {
            return Ok(false);
        }
        self.report(&format!("Uninstalling {name}…"));
        // Preflight before stopping voice or modifying preferences.
        let outcome = (|| -> Result<()> {
            for path in &paths {
                self.validate_removal(path)?;
            }
            if let Some(release) = release {
                release()?;
            }
            for path in &paths {
                self.validate_removal(path)?;
            }
            for path in &paths {
                if path.is_dir() {
                    fs::remove_dir_all(path).map_err(io_error)?;
                } else if path.is_file() {
                    fs::remove_file(path).map_err(io_error)?;
                }
            }
            Ok(())
        })();
        self.end();
        match outcome {
            Ok(()) => {
                self.report(&format!(
                    "{name} uninstalled. You can install it again at any time."
                ));
                Ok(true)
            }
            Err(error) => {
                self.report(&format!(
                    "Could not finish uninstalling {name}. {} Check the installed models list and retry.",
                    error.message()
                ));
                Ok(false)
            }
        }
    }
}

fn io_error(_: std::io::Error) -> AssistantError {
    AssistantError::io("The model files could not be read or removed.")
}

fn check_link(path: &Path) -> Result<()> {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return Ok(());
    };
    #[allow(unused_mut)]
    let mut link = meta.file_type().is_symlink();
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        link |= meta.file_attributes() & 0x400 != 0;
    }
    if link {
        return Err(AssistantError::io(
            "A link was found in this model's storage. Remove the link manually before uninstalling.",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;
    use crate::assistant::speech::{asr_models, tts_models};

    fn put(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "fixture").unwrap();
    }

    fn fixture() -> (tempfile::TempDir, SpeechStorage) {
        let dir = tempfile::tempdir().unwrap();
        let storage = SpeechStorage::new(dir.path());
        (dir, storage)
    }

    fn archive(url: &str) -> &'static str {
        if url.ends_with(".tar.bz2") {
            ".tar.bz2"
        } else {
            ".zip"
        }
    }

    #[test]
    fn every_asr_model_removes_weights_and_cache_while_preserving_siblings_and_engines() {
        let (_dir, speech) = fixture();
        let keep = speech.root().join("engines").join("keep.txt");
        let sibling = speech.root().join("models").join("keep.txt");
        put(&keep);
        put(&sibling);
        for model in asr_models() {
            let target = if model.native() {
                speech.root().join("models").join(&model.id)
            } else {
                speech.asr_path(model)
            };
            if model.native() {
                put(&target.join("nested").join("weights.onnx"));
            } else {
                put(&target);
            }
            let cache = speech.downloads().join(&model.id);
            if model.native() {
                if model.files.is_some() {
                    put(&cache.join("nested").join("weights.onnx"));
                } else {
                    put(&PathBuf::from(format!(
                        "{}{}",
                        cache.display(),
                        archive(&model.asset.url)
                    )));
                }
            }
            put(&PathBuf::from(format!("{}.partial", target.display())));
            assert!(
                speech.has_managed_model(&model.id, false).unwrap(),
                "{}",
                model.id
            );
            assert!(speech.managed_model_bytes(&model.id, false).unwrap() > 0);
            assert!(
                speech.uninstall_model(&model.id, false, None).unwrap(),
                "{}",
                model.id
            );
            assert!(
                !speech.has_managed_model(&model.id, false).unwrap(),
                "{}",
                model.id
            );
            assert!(keep.exists() && sibling.exists());
            assert_eq!(
                speech.status(),
                format!(
                    "{} uninstalled. You can install it again at any time.",
                    model.name
                )
            );
        }
    }

    #[test]
    fn every_tts_model_removes_its_package_or_piper_support_files_and_caches() {
        let (_dir, speech) = fixture();
        let keep = speech.root().join("engines").join("keep.txt");
        put(&keep);
        for model in tts_models().iter().filter(|m| m.id != "windows") {
            if let Some(package) = &model.package {
                let folder = speech.voice_root(model);
                put(&folder.join("nested").join("weights.onnx"));
                let name = folder.file_name().unwrap().to_string_lossy().into_owned();
                put(&speech
                    .downloads()
                    .join(format!("{name}{}", archive(&package.url))));
            } else {
                for suffix in ["", ".json", ".MODEL_CARD", ".partial"] {
                    put(&PathBuf::from(format!(
                        "{}{suffix}",
                        speech.tts_path(model).display()
                    )));
                }
            }
            assert!(
                speech.has_managed_model(&model.id, true).unwrap(),
                "{}",
                model.id
            );
            assert!(
                speech.uninstall_model(&model.id, true, None).unwrap(),
                "{}",
                model.id
            );
            assert!(
                !speech.has_managed_model(&model.id, true).unwrap(),
                "{}",
                model.id
            );
            assert!(keep.exists());
        }
    }

    #[test]
    fn windows_voices_and_unknown_or_traversal_ids_are_rejected_without_deleting_files() {
        let (_dir, speech) = fixture();
        let keep = speech.root().join("models").join("keep.txt");
        put(&keep);
        for (id, tts) in [
            ("windows", true),
            ("../models/keep", false),
            ("unknown", true),
        ] {
            assert!(
                matches!(
                    speech.uninstall_model(id, tts, None),
                    Err(AssistantError::Invalid(_))
                ),
                "{id}"
            );
            assert!(keep.exists() && !speech.busy());
        }
        assert!(!speech.has_managed_model("windows", true).unwrap());
        assert!(!speech.has_managed_model("base", false).unwrap());
    }

    #[test]
    fn selected_managed_and_imported_references_are_detected_without_owning_external_files() {
        let (dir, speech) = fixture();
        let model = &asr_models()[0];
        let mut options = LocalVoiceOptions {
            enabled: true,
            managed: true,
            managed_asr: model.id.clone(),
            managed_tts: "windows".into(),
            ..LocalVoiceOptions::default()
        };
        assert!(speech.model_in_use(&options, &model.id, false));
        assert!(!speech.model_in_use(&options, "windows", true));
        options.own_models = true;
        options.own_asr_engine = "Whisper GGML file".into();
        options.own_asr_path = speech.asr_path(model).to_string_lossy().into_owned();
        assert!(speech.model_in_use(&options, &model.id, false));
        let external = dir.path().join("user-owned.bin");
        put(&external);
        options.own_asr_path = external.to_string_lossy().into_owned();
        assert!(!speech.model_in_use(&options, &model.id, false));
        put(&speech.asr_path(model));
        assert!(speech.uninstall_model(&model.id, false, None).unwrap());
        assert!(external.exists());
        // Off, or not managed: never in use. A "Windows voice" own engine holds no files.
        options.enabled = false;
        assert!(!speech.model_in_use(&options, &model.id, false));
        let voice = tts_by_id("supertonic-3-int8").unwrap();
        let mut options = LocalVoiceOptions {
            enabled: true,
            managed: true,
            managed_tts: voice.id.clone(),
            ..LocalVoiceOptions::default()
        };
        assert!(speech.model_in_use(&options, &voice.id, true));
        options.own_models = true;
        options.own_tts_engine = "Windows voice".into();
        assert!(!speech.model_in_use(&options, &voice.id, true));
        assert!(!speech.model_in_use(&options, "nonsense", true));
    }

    #[test]
    fn voice_shutdown_runs_first_and_concurrent_operations_cannot_start() {
        let (_dir, speech) = fixture();
        let model = &asr_models()[0];
        put(&speech.asr_path(model));
        let entered = Cell::new(false);
        let release = || {
            entered.set(true);
            assert!(speech.busy());
            assert!(
                speech.asr_path(model).exists(),
                "files stay until the voice is released"
            );
            assert!(
                !speech.uninstall_model(&model.id, false, None).unwrap(),
                "no concurrent uninstall"
            );
            assert!(!speech.begin(), "no concurrent install");
            Ok(())
        };
        assert!(speech
            .uninstall_model(&model.id, false, Some(&release))
            .unwrap());
        assert!(entered.get() && !speech.busy());
        assert!(!speech.asr_path(model).exists());
        // A speech test blocks removal as well.
        put(&speech.asr_path(model));
        speech.set_testing(true);
        assert!(!speech.uninstall_model(&model.id, false, None).unwrap());
        speech.set_testing(false);
        assert!(speech.asr_path(model).exists());
    }

    #[test]
    fn a_failed_release_leaves_model_files_intact_and_reports_failure() {
        let (_dir, speech) = fixture();
        let model = &asr_models()[0];
        put(&speech.asr_path(model));
        let release = || Err(AssistantError::io("fixture preference failure"));
        assert!(!speech
            .uninstall_model(&model.id, false, Some(&release))
            .unwrap());
        assert!(speech.asr_path(model).exists() && !speech.busy());
        assert!(speech.status().starts_with("Could not finish"));
        assert!(speech.status().contains("fixture preference failure"));
    }

    #[cfg(unix)]
    #[test]
    fn links_cannot_redirect_model_removal_outside_managed_storage() {
        let (dir, speech) = fixture();
        let model = asr_models().iter().find(|m| m.native()).unwrap();
        let target = speech.root().join("models").join(&model.id);
        let outside = dir.path().join("external");
        put(&outside.join("keep.txt"));
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&outside, &target).unwrap();
        assert!(!speech.uninstall_model(&model.id, false, None).unwrap());
        assert!(outside.join("keep.txt").exists());
        assert!(speech.status().contains("link"));
        assert!(speech.managed_model_bytes(&model.id, false).is_err());
        // A link inside the model folder is refused too.
        fs::remove_file(&target).unwrap();
        put(&target.join("weights.onnx"));
        std::os::unix::fs::symlink(&outside, target.join("inner")).unwrap();
        assert!(!speech.uninstall_model(&model.id, false, None).unwrap());
        assert!(target.join("weights.onnx").exists() && outside.join("keep.txt").exists());
    }

    #[test]
    fn paths_outside_managed_areas_are_never_removed_and_layout_matches_the_installer() {
        let (_dir, speech) = fixture();
        assert!(speech
            .validate_removal(&speech.root().join("engines").join("x"))
            .is_err());
        assert!(speech
            .validate_removal(&speech.root().join("models"))
            .is_err());
        assert!(speech
            .validate_removal(&speech.root().join("models").join("..").join("engines"))
            .is_err());
        assert!(speech
            .validate_removal(&speech.root().join("models").join("x"))
            .is_ok());
        let tiny = asr_by_id("tiny").unwrap();
        assert_eq!(
            speech.asr_path(tiny),
            speech.root().join("models").join("ggml-tiny.bin")
        );
        let voice = tts_by_id("en_US-ljspeech-high").unwrap();
        assert_eq!(
            speech.tts_path(voice),
            speech
                .root()
                .join("voices")
                .join("en_US-ljspeech-high.onnx")
        );
        let kokoro = tts_by_id("kokoro-v1.1-int8-en").unwrap();
        assert_eq!(
            speech.voice_root(kokoro),
            speech.root().join("voices").join("kokoro-v1.1-int8")
        );
        let qwen = asr_by_id("qwen3-asr-0.6b-int8").unwrap();
        assert_eq!(
            speech.qwen_root(qwen),
            speech
                .root()
                .join("models")
                .join(&qwen.id)
                .join(&qwen.folder)
        );
        assert!(speech
            .sherpa_exe(true)
            .ends_with("bin/sherpa-onnx-offline-tts.exe"));
        assert!(speech
            .whisper_exe(true)
            .to_string_lossy()
            .contains("whisper-1.8.7-cuda12"));
        assert!(speech.piper_exe().ends_with("piper/piper.exe"));
        assert_eq!(FOLDER, "local-speech");
        assert!(speech.root().ends_with("local-speech"));
        let mut imported = tiny.clone();
        imported.source_path = "/user/model.bin".into();
        assert_eq!(speech.asr_path(&imported), PathBuf::from("/user/model.bin"));
    }

    #[test]
    fn sizes_are_formatted_for_the_storage_page() {
        assert_eq!(storage_size(0), "0 B");
        assert_eq!(storage_size(1023), "1023 B");
        assert_eq!(storage_size(1536), "1.5 KB");
        assert_eq!(storage_size(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(storage_size(3 * 1024 * 1024 * 1024), "3.00 GB");
    }
}
