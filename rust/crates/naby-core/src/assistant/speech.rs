//! Local speech catalog, hardware-based recommendations and the validation of user-supplied
//! model files (`AssistantSpeechCatalog.cs`, `AssistantSpeechRecommendations.cs`,
//! `AssistantOwnSpeech.cs`, `AssistantLocalVoiceSettings` rules).
//!
//! Hardware scanning, downloads, installs and removal are host tasks. Every URL and SHA-256 below
//! is pinned: the installer verifies size and hash before extracting anything.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use url::Url;

use super::models::{LocalVoiceOptions, Options};
use super::providers::validate_endpoint;
use super::{AssistantError, Result};

pub const WHISPER_REVISION: &str = "5359861c739e955e79d9a303bcbc70fb988958b1";
pub const PIPER_REVISION: &str = "5041a7a49ae5b3a0f30e6837769eb03fdbb231bd";
pub const SHERPA_FOLDER: &str = "sherpa-onnx-v1.13.8-win-x64-shared-MT-Release";
pub const KOKORO_FOLDER: &str = "kokoro-int8-multi-lang-v1_1";

/// A pinned download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    pub url: String,
    pub bytes: u64,
    pub sha256: String,
}

fn asset(url: &str, bytes: u64, sha256: &str) -> Asset {
    Asset {
        url: url.into(),
        bytes,
        sha256: sha256.into(),
    }
}

/// One file of a multi-file model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelFile {
    pub path: String,
    pub asset: Asset,
}

/// Engines and shared runtimes (not models).
pub fn whisper_cpu() -> Asset {
    asset(
        "https://github.com/ggml-org/whisper.cpp/releases/download/v1.8.7/whisper-bin-x64.zip",
        4386743,
        "d9627486e1c34a03745880485593473e047294260ce9a3cb0aa8deaf15b99af6",
    )
}
pub fn whisper_gpu() -> Asset {
    asset("https://github.com/ggml-org/whisper.cpp/releases/download/v1.8.7/whisper-cublas-12.4.0-bin-x64.zip", 460076656, "ce5e66c7e45985f497e3a81c9d33723f19b3118993b47f53b8351df5cbb20402")
}
pub fn piper_engine() -> Asset {
    asset(
        "https://github.com/rhasspy/piper/releases/download/2023.11.14-2/piper_windows_amd64.zip",
        22477236,
        "f3c58906402b24f3a96d92145f58acba6d86c9b5db896d207f78dc80811efcea",
    )
}
pub fn visual_cpp_redistributable() -> Asset {
    asset("https://download.visualstudio.microsoft.com/download/pr/990c720a-46ce-4dc7-87b9-c2eb615d34b9/CC0FF0EB1DC3F5188AE6300FAEF32BF5BEEBA4BDD6E8E445A9184072096B713B/VC_redist.x64.exe", 25635768, "cc0ff0eb1dc3f5188ae6300faef32bf5beeba4bdd6e8e445a9184072096b713b")
}
pub fn sherpa_engine() -> Asset {
    asset(&format!("https://github.com/k2-fsa/sherpa-onnx/releases/download/v1.13.8/{SHERPA_FOLDER}.tar.bz2"), 24805859, "6dffdc715a4465b989446a6105265d2cb345e7101591a17d35534b6758f6e8df")
}
pub fn kokoro_package() -> Asset {
    asset("https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/kokoro-int8-multi-lang-v1_1.tar.bz2", 147031220, "a1e94694776049035c4f2c6529f003aaece993c76aae9a78995831c3c4dcafc6")
}

fn whisper(id: &str, bytes: u64, hash: &str) -> Asset {
    asset(
        &format!(
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/{WHISPER_REVISION}/ggml-{id}.bin"
        ),
        bytes,
        hash,
    )
}

fn release_model(name: &str, bytes: u64, hash: &str, tts: bool) -> Asset {
    asset(
        &format!(
            "https://github.com/k2-fsa/sherpa-onnx/releases/download/{}{name}.tar.bz2",
            if tts { "tts-models/" } else { "asr-models/" }
        ),
        bytes,
        hash,
    )
}

/// A listening (speech recognition) model.
#[derive(Debug, Clone, PartialEq)]
pub struct AsrModel {
    pub id: String,
    pub name: String,
    pub ram_gb: f64,
    pub free_gb: f64,
    /// VRAM needed; infinite when the engine cannot use a GPU.
    pub gpu_gb: f64,
    pub asset: Asset,
    pub engine: String,
    pub folder: String,
    pub language: String,
    pub files: Option<Vec<ModelFile>>,
    pub info: String,
    pub source_path: String,
}

impl AsrModel {
    /// Native sherpa-onnx engines (everything except whisper.cpp).
    pub fn native(&self) -> bool {
        self.engine != "whisper"
    }

    pub fn download_bytes(&self) -> u64 {
        self.files
            .as_ref()
            .map_or(self.asset.bytes, |f| f.iter().map(|x| x.asset.bytes).sum())
    }

    pub fn supports(&self, language: &str) -> bool {
        self.language == "multi" || self.language == language
    }
}

/// A speaking (speech synthesis) voice.
#[derive(Debug, Clone, PartialEq)]
pub struct TtsModel {
    pub id: String,
    pub name: String,
    pub path: String,
    pub bytes: u64,
    pub hash: String,
    pub config_hash: String,
    pub card_hash: String,
    pub package: Option<Asset>,
    pub locale: String,
    pub speaker: i32,
    pub free_gb: f64,
    pub engine: String,
    pub folder: String,
    pub info: String,
    pub source_path: String,
}

impl TtsModel {
    pub fn language(&self) -> &str {
        if !self.locale.is_empty() {
            &self.locale
        } else if self.id.starts_with("id_") {
            "id"
        } else if self.id.starts_with("es_") {
            "es"
        } else if self.id.starts_with("en_") {
            "en"
        } else {
            "any"
        }
    }

    pub fn supports(&self, language: &str) -> bool {
        let own = self.language();
        matches!(own, "any" | "multi") || own == language
    }
}

#[allow(clippy::too_many_arguments)]
fn asr(
    id: &str,
    name: &str,
    ram_gb: f64,
    free_gb: f64,
    gpu_gb: f64,
    asset: Asset,
    engine: &str,
    folder: &str,
    language: &str,
    files: Option<Vec<ModelFile>>,
    info: &str,
) -> AsrModel {
    AsrModel {
        id: id.into(),
        name: name.into(),
        ram_gb,
        free_gb,
        gpu_gb,
        asset,
        engine: engine.into(),
        folder: folder.into(),
        language: language.into(),
        files,
        info: info.into(),
        source_path: String::new(),
    }
}

#[allow(clippy::too_many_arguments)]
fn tts(
    id: &str,
    name: &str,
    path: &str,
    bytes: u64,
    hashes: (&str, &str, &str),
    package: Option<Asset>,
    locale: &str,
    free_gb: f64,
    engine: &str,
    folder: &str,
    info: &str,
) -> TtsModel {
    TtsModel {
        id: id.into(),
        name: name.into(),
        path: path.into(),
        bytes,
        hash: hashes.0.into(),
        config_hash: hashes.1.into(),
        card_hash: hashes.2.into(),
        package,
        locale: locale.into(),
        speaker: 0,
        free_gb,
        engine: engine.into(),
        folder: folder.into(),
        info: info.into(),
        source_path: String::new(),
    }
}

fn qwen17_files() -> Vec<ModelFile> {
    let base = "https://huggingface.co/thieunv-asilla/sherpa-onnx-qwen3-asr-1.7B-int8/resolve/69eb686fd94a4a865bb5340a3d6ac0d7f1fec0d5/";
    [
        (
            "README.md",
            1787,
            "c67121ba17a3718d059c58c59b2391464a8fc0ebefbfc38444e33fc454517461",
        ),
        (
            "conv_frontend.onnx",
            48080441,
            "3cb27a9fe94d95c938e476f2012b21aba2ec0bfceef33b0e58acd208946bafdd",
        ),
        (
            "decoder.int8.onnx",
            2037458645,
            "c43c853fa6e97d08365cb8a5502b360b595cd43c00dc60e4d8ca7cc18cad460b",
        ),
        (
            "encoder.int8.onnx",
            314222162,
            "a5deedae034ece715de8ed204378d8c77f889af3a60c2566581135e84cced7cd",
        ),
        (
            "tokenizer/merges.txt",
            1671853,
            "8831e4f1a044471340f7c0a83d7bd71306a5b867e95fd870f74d0c5308a904d5",
        ),
        (
            "tokenizer/tokenizer_config.json",
            12487,
            "4942d005604266809309cabc9f4e9cb89ce855d59b14681fdc0e1cc62ea26c4c",
        ),
        (
            "tokenizer/vocab.json",
            2776833,
            "ca10d7e9fb3ed18575dd1e277a2579c16d108e32f27439684afa0e10b1440910",
        ),
    ]
    .iter()
    .map(|(path, bytes, hash)| ModelFile {
        path: (*path).into(),
        asset: asset(&format!("{base}{path}"), *bytes, hash),
    })
    .collect()
}

fn build_asr() -> Vec<AsrModel> {
    let inf = f64::INFINITY;
    let qwen06 = asset("https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-qwen3-asr-0.6B-int8-2026-03-25.tar.bz2", 878702423, "393f8a14e2f5fb96746aaab342997a40641001fbd5bf9592a080a8329178ee96");
    let qwen17 = qwen17_files();
    vec![
        asr("tiny", "Whisper tiny · light", 4.0, 0.8, 1.0, whisper("tiny", 77691713, "be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21"), "whisper", "", "multi", None, ""),
        asr("base", "Whisper base · everyday", 6.0, 1.3, 1.5, whisper("base", 147951465, "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe"), "whisper", "", "multi", None, ""),
        asr("small", "Whisper small · clearer recognition", 8.0, 2.5, 3.0, whisper("small", 487601967, "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b"), "whisper", "", "multi", None, ""),
        asr("large-v3-turbo", "Whisper large-v3-turbo · GPU quality", 16.0, 4.0, 6.0, whisper("large-v3-turbo", 1624555275, "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69"), "whisper", "", "multi", None, ""),
        asr("qwen3-asr-0.6b-int8", "Qwen3-ASR 0.6B INT8 · 2026 CPU", 8.0, 3.5, inf, qwen06, "qwen", "sherpa-onnx-qwen3-asr-0.6B-int8-2026-03-25", "multi", None, ""),
        asr("base-q5_1", "Whisper base Q5 · 2 GB GPU / light CPU", 4.0, 1.0, 1.4, whisper("base-q5_1", 59707625, "422f1ae452ade6f30a004d7e5c6a43195e4433bc370bf23fac9cc591f01a8898"), "whisper", "", "multi", None, ""),
        asr("small-q5_1", "Whisper small Q5 · 4 GB GPU", 8.0, 2.0, 2.8, whisper("small-q5_1", 190085487, "ae85e4a935d7a567bd102fe55afc16bb595bdb618e11b2fc7591bc08120411bb"), "whisper", "", "multi", None, ""),
        asr("large-v3-turbo-q5_0", "Whisper large-v3-turbo Q5 · 6 GB GPU", 8.0, 3.0, 4.5, whisper("large-v3-turbo-q5_0", 574041195, "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2"), "whisper", "", "multi", None, ""),
        asr("large-v3-turbo-q8_0", "Whisper large-v3-turbo Q8 · 8 GB GPU", 12.0, 4.0, 6.0, whisper("large-v3-turbo-q8_0", 874188075, "317eb69c11673c9de1e1f0d459b253999804ec71ac4c23c17ecf5fbe24e259a1"), "whisper", "", "multi", None, ""),
        asr("moonshine-v2-tiny-en", "Moonshine v2 tiny · 2026 English CPU", 4.0, 0.6, inf, release_model("sherpa-onnx-moonshine-tiny-en-quantized-2026-02-27", 29858559, "9ec31b342d8fa3240c3b81b8f82e1cf7e3ac467c93ca5a999b741d5887164f8d", false), "moonshine", "sherpa-onnx-moonshine-tiny-en-quantized-2026-02-27", "en", None, "English only · compact 2026 native CPU pack"),
        asr("moonshine-v2-base-en", "Moonshine v2 base · 2026 English CPU", 8.0, 1.0, inf, release_model("sherpa-onnx-moonshine-base-en-quantized-2026-02-27", 111266225, "43232c1d13013d37317163baec3135bd771a186a4356f28c889bab453bb0e891", false), "moonshine", "sherpa-onnx-moonshine-base-en-quantized-2026-02-27", "en", None, "English only · CPU · model loads for each utterance"),
        asr("moonshine-v2-base-es", "Moonshine v2 base · 2026 Spanish CPU", 8.0, 1.0, inf, release_model("sherpa-onnx-moonshine-base-es-quantized-2026-02-27", 50846902, "850c3dcc5dfccc8b1feb10bb221b11d6039b6f5c626241729f46863771016383", false), "moonshine", "sherpa-onnx-moonshine-base-es-quantized-2026-02-27", "es", None, "Spanish only · CPU · model loads for each utterance"),
        asr("qwen3-asr-1.7b-int8", "Qwen3-ASR 1.7B INT8 · 2026 large CPU", 24.0, 7.0, inf, qwen17[0].asset.clone(), "qwen", "", "multi", Some(qwen17), "Community ONNX conversion by thieunv-asilla of Qwen's model · 2.40 GB download · CPU latency can be high · recommended for 32 GB RAM, 12+ threads"),
    ]
}

fn build_tts() -> Vec<TtsModel> {
    vec![
        tts("windows", "Windows voice · already on this PC", "", 0, ("", "", ""), None, "", 0.1, "piper", "", ""),
        tts("kokoro-v1.1-int8-en", "Kokoro 82M v1.1 INT8 · English", "", 147031220, ("", "", ""), Some(kokoro_package()), "en", 1.2, "kokoro", "", "English voice · native CPU · loads only while speaking"),
        tts("id_ID-news_tts-medium", "Piper · Indonesian", "id/id_ID/news_tts/medium", 62950044, ("ed8f02aa593f7af6b19acbdb8142e0da0dd72f46194eb33d38e0eb10a52597e8", "1ef677072668a5e172e0759b1d3871f129009d1167f093325a17607f7add5ad7", "cc134d8c8fce95d2486a0e5ae60b40c17451a876cf7d36df0f4110589fc40538"), None, "", 0.4, "piper", "", ""),
        tts("en_US-ljspeech-high", "Piper · English (US)", "en/en_US/ljspeech/high", 114199011, ("5d4f08ba6a2a48c44592eed3ce56bf85e9de3dd4e20df90541ae68a8310c029a", "7e1f4634af596d83cca997fb7a931ba80b70f8a316a2655ee69c55365e0ace14", "289f7421072d689a3d91f6c632f486af46f9b8b04417536104a4ea667b8a394b"), None, "", 0.4, "piper", "", ""),
        tts("es_ES-sharvard-medium", "Piper · Spanish", "es/es_ES/sharvard/medium", 76733615, ("40febfb1679c69a4505ff311dc136e121e3419a13a290ef264fdf43ddedd0fb1", "7438c9b699c72b0c3388dae1b68d3f364dc66a2150fe554a1c11f03372957b2c", "37655314d08fe8a0900abc628d3bcbee48ffbdd8fad9306b89525a0ada20fefc"), None, "", 0.4, "piper", "", ""),
        tts("kitten-nano-v0.8-int8-en", "KittenTTS nano v0.8 INT8 · light English", "", 31220690, ("", "", ""), Some(release_model("kitten-nano-en-v0_8-int8", 31220690, "6fa5be852612ce761094ba74ee6123b4fc4acfefa79bf64dc63acae4a83af2fd", true)), "en", 0.35, "kitten", "kitten-nano-en-v0_8-int8", "Compact English CPU voice · no reference recording needed"),
        tts("supertonic-3-int8", "Supertonic 3 INT8 · 2026 multilingual", "", 128774318, ("", "", ""), Some(release_model("sherpa-onnx-supertonic-3-tts-int8-2026-05-11", 128774318, "82fa96f91c4ef8abaae3a14a3f4153facf88bed821d1f7331cec2700f432c427", true)), "multi", 1.0, "supertonic", "sherpa-onnx-supertonic-3-tts-int8-2026-05-11", "2026 multilingual CPU voice · preset speaker · OpenRAIL-M model license; read its terms before use"),
        tts("es_MX-ald-x_low", "Piper · Spanish (Latin America) · smallest", "es/es_MX/ald/x_low", 20986952, ("d8aae54aee9eafb37fb85d6da676facafca31c2d84d11acdca1a5b6dd82b1df6", "5b372adf1f8c8d8f0610b9057fc2df001ad3f5f42cd9d6f853a78bef3f8b0644", "75de85e71247db75274624efd6d816babf37cfd34358abed402b0e979f4aa5b9"), None, "", 0.2, "piper", "", "Included with NabyScape · lowest memory"),
        tts("es_ES-carlfm-x_low", "Piper · Spanish (Spain) · small", "es/es_ES/carlfm/x_low", 28130791, ("d69677323a907cd4963f42b29c20a98b5d6bfa7f3e64df339915e4650c00d125", "d9bdfa9ff01eb2bc9e62e7d2593939d1e4c4d8eb7cf75f972731539d12399966", "a6e62a3d36c37c7702d877f784001208b952e2bf4d17c72a6b5da442fbefb4b3"), None, "", 0.2, "piper", "", "Small Spanish (Spain) voice"),
        tts("en_US-amy-low", "Piper · English (US) · small", "en/en_US/amy/low", 63104526, ("a5a91abb7de0f104358a25aded480ddacf1ff0762886325886ec406a2e86aab3", "2250a9a605b8dc35a116717fadc5056695dd809e34a15d02f72a0f52d53d3ebb", "71b07ded301a96346096a71d8f0d64cbb5f49d5d51004dce28bbb7504e3f65f0"), None, "", 0.3, "piper", "", "Smallest English Piper voice"),
    ]
}

/// Every listening model, legacy Whisper ones first.
pub fn asr_models() -> &'static [AsrModel] {
    static MODELS: OnceLock<Vec<AsrModel>> = OnceLock::new();
    MODELS.get_or_init(build_asr)
}

/// Every speaking voice (the Windows voice first).
pub fn tts_models() -> &'static [TtsModel] {
    static MODELS: OnceLock<Vec<TtsModel>> = OnceLock::new();
    MODELS.get_or_init(build_tts)
}

pub fn asr_by_id(id: &str) -> Result<&'static AsrModel> {
    asr_models()
        .iter()
        .find(|m| m.id == id)
        .ok_or_else(|| AssistantError::invalid("Choose a supported listening model."))
}

pub fn tts_by_id(id: &str) -> Result<&'static TtsModel> {
    tts_models()
        .iter()
        .find(|m| m.id == id)
        .ok_or_else(|| AssistantError::invalid("Choose a supported speaking voice."))
}

/// Download address of a Piper voice file; `suffix` is `.onnx`, `.onnx.json` or `MODEL_CARD`.
pub fn voice_url(model: &TtsModel, suffix: &str) -> String {
    let file = if suffix == "MODEL_CARD" {
        suffix.to_string()
    } else {
        format!("{}{}", model.id, suffix)
    };
    format!(
        "https://huggingface.co/rhasspy/piper-voices/resolve/{PIPER_REVISION}/{}/{file}",
        model.path
    )
}

// ----- hardware and recommendations -------------------------------------------------------

/// What a hardware scan reported (`SpeechHardware`).
#[derive(Debug, Clone, PartialEq)]
pub struct Hardware {
    pub cpu: String,
    pub threads: u32,
    pub ram_gb: f64,
    pub free_gb: f64,
    pub gpu: String,
    pub vram_gb: f64,
    pub free_vram_gb: f64,
    pub cuda: bool,
    pub supported: bool,
}

/// `{0:0.#}`: one decimal, dropped when zero.
fn g1(value: f64) -> String {
    let rounded = (value * 10.0).round() / 10.0;
    if (rounded - rounded.trunc()).abs() < 1e-9 {
        format!("{}", rounded as i64)
    } else {
        format!("{rounded:.1}")
    }
}

impl Hardware {
    pub fn fits(&self, model: &AsrModel) -> bool {
        self.supported && self.ram_gb + 0.5 >= model.ram_gb && self.free_gb >= model.free_gb
    }

    pub fn use_gpu(&self, model: &AsrModel) -> bool {
        model.engine == "whisper" && self.cuda && self.free_vram_gb >= model.gpu_gb
    }

    pub fn fits_pack(&self, asr: &AsrModel, tts: &TtsModel) -> bool {
        self.fits(asr) && self.free_gb >= pack_free_gb(asr, tts)
    }

    pub fn recommended(&self) -> &'static AsrModel {
        recommend_pack(self, "en", "Balanced", true).asr
    }

    pub fn recommend(&self, preference: &str) -> &'static AsrModel {
        recommend_pack(self, "en", preference, true).asr
    }

    pub fn tier(&self) -> String {
        if self.cuda && self.vram_gb >= 1.5 {
            let v = self.vram_gb;
            let size = if v >= 7.5 {
                "8+"
            } else if v >= 5.5 {
                "6"
            } else if v >= 3.5 {
                "4"
            } else {
                "2"
            };
            format!("NVIDIA {size} GB VRAM")
        } else {
            let r = self.ram_gb;
            let size = if r >= 31.5 {
                "32+"
            } else if r >= 15.5 {
                "16"
            } else if r >= 7.5 {
                "8"
            } else {
                "under 8"
            };
            format!("CPU · {size} GB RAM")
        }
    }

    pub fn summary(&self) -> String {
        let vram = if self.vram_gb > 0.0 {
            format!(
                " · VRAM {} GB ({} GB free)",
                g1(self.vram_gb),
                g1(self.free_vram_gb)
            )
        } else {
            " · dedicated VRAM not confirmed".into()
        };
        format!(
            "{} · {} threads\nRAM {} GB · {} GB available\n{}{}",
            self.cpu,
            self.threads,
            g1(self.ram_gb),
            g1(self.free_gb),
            self.gpu,
            vram
        )
    }
}

/// Free disk/memory a recognition + voice pair needs (native engines share their runtime).
pub fn pack_free_gb(asr: &AsrModel, tts: &TtsModel) -> f64 {
    if asr.native() {
        asr.free_gb.max(tts.free_gb)
    } else {
        asr.free_gb + tts.free_gb
    }
}

/// A listening model, a voice and whether recognition runs on the GPU.
#[derive(Debug, Clone, PartialEq)]
pub struct Pack {
    pub asr: &'static AsrModel,
    pub tts: &'static TtsModel,
    pub gpu: bool,
    pub reason: String,
}

/// Hardware tiers offered in the manual profile list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HardwareProfile {
    pub name: &'static str,
    pub ram_gb: u32,
    pub vram_gb: u32,
    pub asr_id: &'static str,
}

pub const PROFILES: [HardwareProfile; 7] = [
    HardwareProfile {
        name: "CPU · RAM 8 GB",
        ram_gb: 8,
        vram_gb: 0,
        asr_id: "moonshine-v2-base-en",
    },
    HardwareProfile {
        name: "CPU · RAM 16 GB",
        ram_gb: 16,
        vram_gb: 0,
        asr_id: "qwen3-asr-0.6b-int8",
    },
    HardwareProfile {
        name: "CPU · RAM 32 GB",
        ram_gb: 32,
        vram_gb: 0,
        asr_id: "qwen3-asr-1.7b-int8",
    },
    HardwareProfile {
        name: "NVIDIA · VRAM 2 GB",
        ram_gb: 8,
        vram_gb: 2,
        asr_id: "base-q5_1",
    },
    HardwareProfile {
        name: "NVIDIA · VRAM 4 GB",
        ram_gb: 8,
        vram_gb: 4,
        asr_id: "small-q5_1",
    },
    HardwareProfile {
        name: "NVIDIA · VRAM 6 GB",
        ram_gb: 16,
        vram_gb: 6,
        asr_id: "large-v3-turbo-q5_0",
    },
    HardwareProfile {
        name: "NVIDIA · VRAM 8 GB+",
        ram_gb: 16,
        vram_gb: 8,
        asr_id: "large-v3-turbo-q8_0",
    },
];

/// The best voice for a recognizer that fits the machine (`RecommendVoice`).
pub fn recommend_voice(
    h: &Hardware,
    a: &AsrModel,
    language: &str,
    preference: &str,
) -> Option<&'static TtsModel> {
    let ids: [&str; 2] = if (preference == "Light" || h.ram_gb < 12.0) && language == "en" {
        ["kitten-nano-v0.8-int8-en", "supertonic-3-int8"]
    } else {
        ["supertonic-3-int8", "kitten-nano-v0.8-int8-en"]
    };
    let preferred = ids.iter().filter_map(|id| tts_by_id(id).ok());
    let others = tts_models()
        .iter()
        .filter(|v| v.package.is_none() && v.id != "windows");
    let windows = tts_by_id("windows").ok();
    preferred
        .chain(others)
        .chain(windows)
        .find(|v| v.supports(language) && h.fits_pack(a, v))
}

/// The pack for a manual hardware profile, or `None` when this machine cannot run it.
pub fn profile_pack(
    h: &Hardware,
    profile: &HardwareProfile,
    language: &str,
    preference: &str,
) -> Option<Pack> {
    let mut id = profile.asr_id;
    if profile.vram_gb == 0 && profile.ram_gb == 8 {
        id = match language {
            "en" => "moonshine-v2-base-en",
            "es" => "moonshine-v2-base-es",
            _ => "base-q5_1",
        };
    }
    let a = asr_by_id(id).ok()?;
    if !a.supports(language)
        || h.ram_gb + 0.5 < profile.ram_gb as f64
        || !h.fits(a)
        || (profile.vram_gb > 0 && (h.vram_gb + 0.5 < profile.vram_gb as f64 || !h.use_gpu(a)))
    {
        return None;
    }
    let t = recommend_voice(h, a, language, preference)?;
    Some(Pack {
        asr: a,
        tts: t,
        gpu: profile.vram_gb > 0,
        reason: format!(
            "{} · TTS stays on CPU to leave VRAM for recognition and other apps.",
            profile.name
        ),
    })
}

/// Conservative budgets, not a benchmark: keep both language and the whole pack within memory.
pub fn recommend_pack(h: &Hardware, language: &str, preference: &str, allow_gpu: bool) -> Pack {
    let mut candidates: Vec<&str> = Vec::new();
    if allow_gpu && h.cuda {
        if preference != "Light" {
            if h.vram_gb >= 7.5 {
                candidates.push("large-v3-turbo-q8_0");
            }
            if h.vram_gb >= 5.5 {
                candidates.push("large-v3-turbo-q5_0");
            }
            if h.vram_gb >= 3.5 {
                candidates.push("small-q5_1");
            }
        }
        candidates.push("base-q5_1");
    }
    if preference == "Quality" && h.ram_gb >= 31.5 && h.threads >= 12 {
        candidates.push("qwen3-asr-1.7b-int8");
    }
    if preference != "Light" && h.threads >= 6 && h.ram_gb >= 15.5 {
        candidates.push("qwen3-asr-0.6b-int8");
    }
    if language == "en" {
        candidates.push(if preference == "Light" {
            "moonshine-v2-tiny-en"
        } else {
            "moonshine-v2-base-en"
        });
    }
    if language == "es" {
        candidates.push("moonshine-v2-base-es");
    }
    if preference != "Light" && h.threads >= 8 {
        candidates.push("small-q5_1");
    }
    candidates.push("base-q5_1");
    candidates.push("tiny");
    let mut seen: Vec<&str> = Vec::new();
    for id in candidates {
        if seen.contains(&id) {
            continue;
        }
        seen.push(id);
        let Ok(a) = asr_by_id(id) else { continue };
        if !h.fits(a) || !a.supports(language) {
            continue;
        }
        let gpu = allow_gpu && h.use_gpu(a);
        if a.engine == "whisper" && id.starts_with("large-") && !gpu {
            continue;
        }
        if let Some(t) = recommend_voice(h, a, language, preference) {
            return Pack {
                asr: a,
                tts: t,
                gpu,
                reason: format!(
                    "{} detected · {} Based on available memory, language and {} priority.",
                    h.tier(),
                    if gpu {
                        "ASR on NVIDIA GPU, TTS on CPU."
                    } else {
                        "Native CPU speech; dedicated VRAM is not needed."
                    },
                    preference.to_lowercase()
                ),
            };
        }
    }
    Pack {
        asr: asr_by_id("tiny").expect("catalog entry"),
        tts: tts_by_id("windows").expect("catalog entry"),
        gpu: false,
        reason: "Not enough memory or unsupported CPU. Close other apps or use an external speech service.".into(),
    }
}

pub fn asr_details(a: &AsrModel) -> String {
    if !a.info.is_empty() {
        a.info.clone()
    } else if a.engine == "qwen" {
        "2026 multilingual recognition · CPU INT8 · loads for each utterance".into()
    } else {
        "Multilingual Whisper · CPU / supported NVIDIA GPU · model stays loaded during the voice session".into()
    }
}

pub fn voice_details(t: &TtsModel) -> String {
    if !t.info.is_empty() {
        t.info.clone()
    } else if t.id == "windows" {
        "Default installed Windows voice; its language must match your selection".into()
    } else {
        "Piper native CPU voice for the selected language".into()
    }
}

pub fn asr_link(a: &AsrModel) -> &'static str {
    if a.id == "qwen3-asr-1.7b-int8" {
        "https://huggingface.co/thieunv-asilla/sherpa-onnx-qwen3-asr-1.7B-int8"
    } else if a.engine == "moonshine" {
        "https://k2-fsa.github.io/sherpa/onnx/moonshine/models-v2.html"
    } else if a.engine == "qwen" {
        "https://github.com/QwenLM/Qwen3-ASR"
    } else {
        "https://github.com/ggml-org/whisper.cpp"
    }
}

pub fn voice_link(t: &TtsModel) -> String {
    match t.engine.as_str() {
        "supertonic" => "https://huggingface.co/Supertone/supertonic-3/blob/main/LICENSE".into(),
        "kitten" => "https://huggingface.co/KittenML/kitten-tts-nano-0.8-int8".into(),
        _ if t.package.is_some() => "https://huggingface.co/hexgrad/Kokoro-82M-v1.1-zh".into(),
        _ => voice_url(t, "MODEL_CARD").replace("/resolve/", "/blob/"),
    }
}

// ----- local services and user model files -------------------------------------------------

/// A localhost speech service address (`LocalAssistantVoice.Endpoint`).
pub fn local_endpoint(value: &str) -> Result<Url> {
    let uri = validate_endpoint(value)?;
    let loopback = match uri.host() {
        Some(url::Host::Domain(d)) => d.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(a)) => a.is_loopback(),
        Some(url::Host::Ipv6(a)) => a.is_loopback(),
        None => false,
    };
    if !loopback {
        return Err(AssistantError::invalid(
            "Local voice URLs must use localhost or a loopback IP. Chat can use your separately configured provider.",
        ));
    }
    Ok(uri)
}

pub const OWN_ASR_ENGINES: [&str; 4] = [
    "Installed pack",
    "Whisper GGML file",
    "Qwen3-ASR INT8 folder",
    "Moonshine v2 folder",
];
pub const OWN_TTS_ENGINES: [&str; 6] = [
    "Installed pack",
    "Windows voice",
    "Piper ONNX file",
    "Kokoro folder",
    "KittenTTS folder",
    "Supertonic folder",
];

/// Reasons a user-supplied model file or folder is rejected.
fn model_error(message: &str) -> AssistantError {
    AssistantError::io(message)
}

/// `%VAR%` expansion like `Environment.ExpandEnvironmentVariables`.
fn expand_environment(value: &str) -> String {
    let mut out = String::new();
    let mut rest = value;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        match rest[start + 1..].find('%') {
            Some(len) => {
                let name = &rest[start + 1..start + 1 + len];
                match std::env::var(name).ok().filter(|_| !name.is_empty()) {
                    Some(v) => out.push_str(&v),
                    None => out.push_str(&rest[start..start + len + 2]),
                }
                rest = &rest[start + len + 2..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// A local drive path (no relative or UNC paths) from the text a user typed.
pub fn local_path(value: &str) -> Result<PathBuf> {
    let expanded = expand_environment(value.trim().trim_matches('"'));
    let path = PathBuf::from(&expanded);
    if !path.is_absolute() || expanded.starts_with("\\\\") {
        return Err(model_error(
            "Choose a local file or folder using its full drive path.",
        ));
    }
    Ok(path)
}

fn require_file(path: &Path, extension: &str) -> Result<()> {
    if !extension.is_empty()
        && !path
            .to_string_lossy()
            .to_ascii_lowercase()
            .ends_with(&extension.to_ascii_lowercase())
    {
        return Err(model_error(&format!("Expected a {extension} model file.")));
    }
    if !fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() > 0) {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        return Err(model_error(&format!(
            "Missing model file: {name}. Choose the complete model and its matching support files."
        )));
    }
    Ok(())
}

fn require_files(folder: &Path, files: &[&str]) -> Result<()> {
    if !folder.is_dir() {
        return Err(model_error("Choose an existing, extracted model folder."));
    }
    for file in files {
        require_file(&folder.join(file), "")?;
    }
    Ok(())
}

/// `<name>.ort` or `<name>.onnx` inside a Moonshine folder.
pub fn moonshine_file(folder: &Path, name: &str) -> Result<PathBuf> {
    for suffix in [".ort", ".onnx"] {
        let path = folder.join(format!("{name}{suffix}"));
        if fs::metadata(&path).is_ok_and(|m| m.is_file() && m.len() > 0) {
            return Ok(path);
        }
    }
    Err(model_error(&format!(
        "Missing {name}.ort or {name}.onnx in the Moonshine folder."
    )))
}

/// The weights file of a voice folder: `model.int8.onnx`, `model.onnx` or the only `.onnx`.
pub fn weights(folder: &Path) -> Result<PathBuf> {
    if !folder.is_dir() {
        return Err(model_error(
            "The model folder was moved or is unavailable. Choose it again.",
        ));
    }
    for name in ["model.int8.onnx", "model.onnx"] {
        let path = folder.join(name);
        if path.is_file() {
            require_file(&path, "")?;
            return Ok(path);
        }
    }
    let mut onnx: Vec<PathBuf> = fs::read_dir(folder)
        .map_err(|_| model_error("The model folder was moved or is unavailable. Choose it again."))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("onnx"))
        })
        .take(2)
        .collect();
    if onnx.len() != 1 {
        return Err(model_error(
            "Choose a voice folder with model.onnx, model.int8.onnx or exactly one ONNX model.",
        ));
    }
    let path = onnx.remove(0);
    require_file(&path, "")?;
    Ok(path)
}

/// The listening model the options select; user-supplied files are validated here.
pub fn own_asr(options: &LocalVoiceOptions) -> Result<AsrModel> {
    if !options.own_models || options.own_asr_engine == "Installed pack" {
        return Ok(asr_by_id(&options.managed_asr)?.clone());
    }
    let path = local_path(&options.own_asr_path)?;
    let engine = match options.own_asr_engine.as_str() {
        "Whisper GGML file" => {
            require_file(&path, ".bin")?;
            let mut file = fs::File::open(&path)
                .map_err(|_| model_error("Missing model file. Choose the complete model."))?;
            let length = file.metadata().map(|m| m.len()).unwrap_or(0);
            let mut magic = [0u8; 4];
            if length < 1024 * 1024
                || file.read_exact(&mut magic).is_err()
                || u32::from_le_bytes(magic) != 0x67676d6c
            {
                return Err(model_error(
                    "Choose a whisper.cpp GGML .bin model. GGUF chat models and safetensors are not supported here.",
                ));
            }
            "whisper"
        }
        "Qwen3-ASR INT8 folder" => {
            require_files(
                &path,
                &[
                    "conv_frontend.onnx",
                    "encoder.int8.onnx",
                    "decoder.int8.onnx",
                    "tokenizer/vocab.json",
                    "tokenizer/merges.txt",
                ],
            )?;
            "qwen"
        }
        "Moonshine v2 folder" => {
            require_files(&path, &["tokens.txt"])?;
            moonshine_file(&path, "encoder_model")?;
            moonshine_file(&path, "decoder_model_merged")?;
            "moonshine"
        }
        _ => return Err(model_error("Choose a supported listening model format.")),
    };
    let bytes = if engine == "whisper" {
        fs::metadata(&path).map(|m| m.len()).unwrap_or(0)
    } else {
        0
    };
    Ok(AsrModel {
        id: "own-asr".into(),
        name: "Your listening model".into(),
        ram_gb: 0.0,
        free_gb: 0.0,
        gpu_gb: if engine == "whisper" {
            1.4
        } else {
            f64::INFINITY
        },
        asset: asset("", bytes, ""),
        engine: engine.into(),
        folder: String::new(),
        language: "multi".into(),
        files: None,
        info: String::new(),
        source_path: path.to_string_lossy().into_owned(),
    })
}

/// The speaking voice the options select; user-supplied files are validated here.
pub fn own_tts(options: &LocalVoiceOptions) -> Result<TtsModel> {
    if !options.own_models || options.own_tts_engine == "Installed pack" {
        return Ok(tts_by_id(&options.managed_tts)?.clone());
    }
    if options.own_tts_engine == "Windows voice" {
        return Ok(tts_by_id("windows")?.clone());
    }
    let path = local_path(&options.own_tts_path)?;
    let engine = match options.own_tts_engine.as_str() {
        "Piper ONNX file" => {
            require_file(&path, ".onnx")?;
            let config = PathBuf::from(format!("{}.json", path.to_string_lossy()));
            require_file(&config, "")?;
            if fs::metadata(&config).map(|m| m.len()).unwrap_or(0) > 512 * 1024 {
                return Err(model_error("The Piper configuration exceeds 512 KB."));
            }
            let text = fs::read_to_string(&config)
                .map_err(|_| model_error("The Piper configuration could not be read."))?;
            let json: serde_json::Value = serde_json::from_str(&text).map_err(|_| {
                model_error("Piper needs a matching .onnx.json with an audio sample_rate from 8000 to 48000 Hz.")
            })?;
            let rate = json
                .pointer("/audio/sample_rate")
                .and_then(serde_json::Value::as_i64);
            if !rate.is_some_and(|r| (8000..=48000).contains(&r)) {
                return Err(model_error(
                    "Piper needs a matching .onnx.json with an audio sample_rate from 8000 to 48000 Hz.",
                ));
            }
            if options.own_speaker > 0 {
                let speakers = json.get("num_speakers").and_then(serde_json::Value::as_i64);
                if !speakers.is_some_and(|s| (options.own_speaker as i64) < s) {
                    return Err(model_error(
                        "That speaker ID is not available in this Piper voice.",
                    ));
                }
            }
            "piper"
        }
        "Kokoro folder" | "KittenTTS folder" => {
            if options.speech_language != "en" {
                return Err(model_error(
                    "These Kokoro/Kitten adapters use English voices. Select English, or use an external service for other configurations.",
                ));
            }
            weights(&path)?;
            require_files(
                &path,
                &[
                    "voices.bin",
                    "tokens.txt",
                    "espeak-ng-data/phontab",
                    "espeak-ng-data/phondata",
                    "espeak-ng-data/phonindex",
                ],
            )?;
            if options.own_tts_engine == "Kokoro folder" {
                "kokoro"
            } else {
                "kitten"
            }
        }
        "Supertonic folder" => {
            require_files(
                &path,
                &[
                    "duration_predictor.int8.onnx",
                    "text_encoder.int8.onnx",
                    "vector_estimator.int8.onnx",
                    "vocoder.int8.onnx",
                    "tts.json",
                    "unicode_indexer.bin",
                    "voice.bin",
                ],
            )?;
            "supertonic"
        }
        _ => return Err(model_error("Choose a supported speaking model format.")),
    };
    if !(0..=255).contains(&options.own_speaker) {
        return Err(model_error(
            "Speaker ID must be between 0 and 255 and exist in your model.",
        ));
    }
    let bytes = if engine == "piper" {
        fs::metadata(&path).map(|m| m.len()).unwrap_or(0)
    } else {
        0
    };
    Ok(TtsModel {
        id: "own-tts".into(),
        name: "Your speaking model".into(),
        path: String::new(),
        bytes,
        hash: String::new(),
        config_hash: String::new(),
        card_hash: String::new(),
        package: (engine != "piper").then(|| asset("", 0, "")),
        locale: options.speech_language.clone(),
        speaker: options.own_speaker,
        free_gb: 0.0,
        engine: engine.into(),
        folder: String::new(),
        info: String::new(),
        source_path: path.to_string_lossy().into_owned(),
    })
}

/// The `(listening, speaking)` names shown in the setup summary.
pub fn local_voice_names(local: &LocalVoiceOptions) -> (String, String) {
    if !local.enabled {
        return ("Realtime provider".into(), "Realtime provider".into());
    }
    if local.managed {
        match (own_asr(local), own_tts(local)) {
            (Ok(a), Ok(t)) => (a.name, t.name),
            _ => (
                "Model files need attention".into(),
                "Open Voice setup".into(),
            ),
        }
    } else {
        (local.asr_model.clone(), local.tts_engine.clone())
    }
}

/// The language name for a speaking-voice code (used in the spoken-reply instructions).
pub fn language_name(code: &str) -> &'static str {
    match code {
        "id" => "Indonesian",
        "es" => "Spanish",
        "fr" => "French",
        "de" => "German",
        "pt" => "Portuguese",
        "it" => "Italian",
        "ja" => "Japanese",
        "ko" => "Korean",
        "ru" => "Russian",
        "ar" => "Arabic",
        "nl" => "Dutch",
        "tr" => "Turkish",
        "hi" => "Hindi",
        "vi" => "Vietnamese",
        _ => "English",
    }
}

/// Options after choosing an installed speech pack (`UseSpeechPack`). `installed` is whether the
/// host found the pack's files and engines.
pub fn use_speech_pack(
    options: &Options,
    asr: &AsrModel,
    tts: &TtsModel,
    gpu: bool,
    preference: &str,
    language: &str,
    installed: bool,
) -> Result<Options> {
    if !installed {
        return Err(AssistantError::invalid(
            "Install the selected speech pack first.",
        ));
    }
    let mut next = options.clone();
    let voice = &mut next.local_voice;
    voice.enabled = true;
    voice.managed = true;
    voice.own_models = false;
    voice.managed_asr = asr.id.clone();
    voice.managed_tts = tts.id.clone();
    voice.managed_gpu = gpu;
    voice.speech_preference = preference.into();
    voice.speech_language = language.into();
    next.enabled = true;
    Ok(next)
}

/// Options after choosing the user's own model files (`UseOwnSpeech`). The files are validated,
/// GPU use is kept only when the scanned hardware supports it, and `installed` reports whether
/// the engines they need are present.
pub fn use_own_speech(
    options: &Options,
    mut local: LocalVoiceOptions,
    hardware: Option<&Hardware>,
    installed: &dyn Fn(&AsrModel, &TtsModel, bool) -> bool,
) -> Result<Options> {
    let asr = own_asr(&local)?;
    let tts = own_tts(&local)?;
    local.managed_gpu = local.managed_gpu && hardware.is_some_and(|h| h.use_gpu(&asr));
    if !installed(&asr, &tts, local.managed_gpu) {
        return Err(AssistantError::invalid(
            "Prepare the required engines and check your model files first.",
        ));
    }
    let mut next = options.clone();
    next.local_voice = local;
    next.enabled = true;
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hw(ram: f64, free: f64, threads: u32, vram: f64, free_vram: f64, cuda: bool) -> Hardware {
        Hardware {
            cpu: "Test CPU".into(),
            threads,
            ram_gb: ram,
            free_gb: free,
            gpu: "Test GPU".into(),
            vram_gb: vram,
            free_vram_gb: free_vram,
            cuda,
            supported: true,
        }
    }

    #[test]
    fn catalog_is_complete_unique_and_pinned() {
        assert_eq!(asr_models().len(), 13);
        assert_eq!(tts_models().len(), 10);
        let ald = tts_by_id("es_MX-ald-x_low").unwrap();
        assert_eq!(ald.language(), "es");
        assert_eq!(
            voice_url(ald, ".onnx"),
            format!("https://huggingface.co/rhasspy/piper-voices/resolve/{PIPER_REVISION}/es/es_MX/ald/x_low/es_MX-ald-x_low.onnx")
        );
        let mut ids: Vec<&str> = asr_models().iter().map(|m| m.id.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 13);
        for m in asr_models() {
            assert!(m.asset.url.starts_with("https://"), "{}", m.id);
            assert_eq!(m.asset.sha256.len(), 64, "{}", m.id);
            assert!(m.asset.bytes > 0);
        }
        for m in tts_models().iter().filter(|m| m.id != "windows") {
            match &m.package {
                Some(p) => assert_eq!(p.sha256.len(), 64),
                None => {
                    assert_eq!(m.hash.len(), 64);
                    assert_eq!(m.config_hash.len(), 64);
                    assert_eq!(m.card_hash.len(), 64);
                }
            }
        }
        let tiny = asr_by_id("tiny").unwrap();
        assert_eq!(
            tiny.asset.url,
            format!("https://huggingface.co/ggerganov/whisper.cpp/resolve/{WHISPER_REVISION}/ggml-tiny.bin")
        );
        assert!(!tiny.native() && asr_by_id("moonshine-v2-base-en").unwrap().native());
        let big = asr_by_id("qwen3-asr-1.7b-int8").unwrap();
        assert_eq!(big.files.as_ref().unwrap().len(), 7);
        assert_eq!(
            big.download_bytes(),
            1787 + 48080441 + 2037458645 + 314222162 + 1671853 + 12487 + 2776833
        );
        assert!(asr_by_id("nope").is_err() && tts_by_id("nope").is_err());
        assert_eq!(whisper_gpu().bytes, 460076656);
        assert_eq!(kokoro_package().bytes, 147031220);
        assert!(sherpa_engine()
            .url
            .ends_with("sherpa-onnx-v1.13.8-win-x64-shared-MT-Release.tar.bz2"));
        assert_eq!(piper_engine().bytes, 22477236);
        assert_eq!(visual_cpp_redistributable().bytes, 25635768);
        assert_eq!(whisper_cpu().bytes, 4386743);
        let id = tts_by_id("id_ID-news_tts-medium").unwrap();
        assert_eq!(
            voice_url(id, ".onnx"),
            format!("https://huggingface.co/rhasspy/piper-voices/resolve/{PIPER_REVISION}/id/id_ID/news_tts/medium/id_ID-news_tts-medium.onnx")
        );
        assert!(voice_link(id).contains("/blob/") && voice_link(id).ends_with("/MODEL_CARD"));
        assert_eq!(id.language(), "id");
        assert!(tts_by_id("windows").unwrap().supports("fr"));
        assert!(!tts_by_id("kitten-nano-v0.8-int8-en")
            .unwrap()
            .supports("es"));
        assert!(asr_by_id("moonshine-v2-base-es").unwrap().supports("es"));
        assert!(!asr_by_id("moonshine-v2-base-es").unwrap().supports("en"));
    }

    #[test]
    fn recommendations_follow_the_hardware_tier() {
        // 8 GB laptop: native English CPU recognizer with the light voice.
        let small = hw(8.0, 4.0, 4, 0.0, 0.0, false);
        let pack = recommend_pack(&small, "en", "Balanced", true);
        assert_eq!(
            (pack.asr.id.as_str(), pack.tts.id.as_str(), pack.gpu),
            ("moonshine-v2-base-en", "kitten-nano-v0.8-int8-en", false)
        );
        assert!(pack.reason.starts_with("CPU · 8 GB RAM detected"));
        // 16 GB, 8 threads: Qwen 0.6B.
        let mid = hw(16.0, 9.0, 8, 0.0, 0.0, false);
        assert_eq!(
            recommend_pack(&mid, "en", "Balanced", true).asr.id,
            "qwen3-asr-0.6b-int8"
        );
        assert_eq!(
            recommend_pack(&mid, "en", "Light", true).asr.id,
            "moonshine-v2-tiny-en"
        );
        // 32 GB, 16 threads, Quality: the 1.7B model.
        let big = hw(32.0, 20.0, 16, 0.0, 0.0, false);
        assert_eq!(
            recommend_pack(&big, "en", "Quality", true).asr.id,
            "qwen3-asr-1.7b-int8"
        );
        assert_eq!(
            recommend_pack(&big, "en", "Balanced", true).asr.id,
            "qwen3-asr-0.6b-int8"
        );
        // NVIDIA tiers.
        let gpu8 = hw(32.0, 20.0, 16, 8.0, 7.0, true);
        let pack = recommend_pack(&gpu8, "en", "Balanced", true);
        assert_eq!(
            (pack.asr.id.as_str(), pack.gpu),
            ("large-v3-turbo-q8_0", true)
        );
        assert!(pack.reason.contains("ASR on NVIDIA GPU, TTS on CPU."));
        let gpu4 = hw(16.0, 9.0, 8, 4.0, 3.5, true);
        assert_eq!(
            recommend_pack(&gpu4, "en", "Balanced", true).asr.id,
            "small-q5_1"
        );
        assert_eq!(
            recommend_pack(&gpu4, "en", "Light", true).asr.id,
            "base-q5_1"
        );
        assert!(!recommend_pack(&gpu8, "en", "Balanced", false).gpu);
        // Other languages pick multilingual recognizers and voices.
        let pack = recommend_pack(&small, "id", "Balanced", true);
        assert_eq!(pack.asr.id, "base-q5_1");
        assert_eq!(pack.tts.id, "supertonic-3-int8");
        assert_eq!(
            recommend_pack(&small, "es", "Balanced", true).asr.id,
            "moonshine-v2-base-es"
        );
        // Nothing fits: fall back to the tiny model with the Windows voice and say why.
        let tiny = hw(2.0, 0.3, 2, 0.0, 0.0, false);
        let pack = recommend_pack(&tiny, "en", "Balanced", true);
        assert_eq!(
            (pack.asr.id.as_str(), pack.tts.id.as_str()),
            ("tiny", "windows")
        );
        assert!(pack.reason.starts_with("Not enough memory"));
        let mut unsupported = small.clone();
        unsupported.supported = false;
        assert_eq!(
            recommend_pack(&unsupported, "en", "Balanced", true).tts.id,
            "windows"
        );
        assert_eq!(small.recommended().id, "moonshine-v2-base-en");
        assert_eq!(small.recommend("Light").id, "moonshine-v2-tiny-en");
    }

    #[test]
    fn hardware_labels_and_profiles() {
        assert_eq!(hw(8.0, 4.0, 4, 0.0, 0.0, false).tier(), "CPU · 8 GB RAM");
        assert_eq!(hw(31.9, 4.0, 4, 0.0, 0.0, false).tier(), "CPU · 32+ GB RAM");
        assert_eq!(
            hw(4.0, 1.0, 4, 0.0, 0.0, false).tier(),
            "CPU · under 8 GB RAM"
        );
        assert_eq!(hw(16.0, 4.0, 4, 6.0, 5.0, true).tier(), "NVIDIA 6 GB VRAM");
        assert_eq!(hw(16.0, 4.0, 4, 1.0, 1.0, true).tier(), "CPU · 16 GB RAM");
        assert_eq!(
            hw(15.9, 8.25, 12, 6.0, 5.0, true).summary(),
            "Test CPU · 12 threads\nRAM 15.9 GB · 8.3 GB available\nTest GPU · VRAM 6 GB (5 GB free)"
        );
        assert!(hw(8.0, 4.0, 4, 0.0, 0.0, false)
            .summary()
            .ends_with(" · dedicated VRAM not confirmed"));
        let h = hw(16.0, 9.0, 8, 6.0, 5.5, true);
        let cpu8 = &PROFILES[0];
        assert_eq!(
            profile_pack(&h, cpu8, "es", "Balanced").unwrap().asr.id,
            "moonshine-v2-base-es"
        );
        assert_eq!(
            profile_pack(&h, cpu8, "fr", "Balanced").unwrap().asr.id,
            "base-q5_1"
        );
        let gpu6 = &PROFILES[5];
        let pack = profile_pack(&h, gpu6, "en", "Balanced").unwrap();
        assert!(pack.gpu && pack.asr.id == "large-v3-turbo-q5_0");
        assert!(pack
            .reason
            .starts_with("NVIDIA · VRAM 6 GB · TTS stays on CPU"));
        assert!(
            profile_pack(&h, &PROFILES[6], "en", "Balanced").is_none(),
            "needs 8 GB VRAM"
        );
        assert!(profile_pack(
            &hw(8.0, 4.0, 4, 0.0, 0.0, false),
            &PROFILES[2],
            "en",
            "Balanced"
        )
        .is_none());
        assert!(
            pack_free_gb(asr_by_id("tiny").unwrap(), tts_by_id("windows").unwrap()) - 0.9 < 1e-9
        );
        assert_eq!(
            pack_free_gb(
                asr_by_id("moonshine-v2-base-en").unwrap(),
                tts_by_id("supertonic-3-int8").unwrap()
            ),
            1.0
        );
        assert!(asr_details(asr_by_id("base").unwrap()).starts_with("Multilingual Whisper"));
        assert!(
            asr_details(asr_by_id("qwen3-asr-0.6b-int8").unwrap()).starts_with("2026 multilingual")
        );
        assert_eq!(
            voice_details(tts_by_id("windows").unwrap()),
            "Default installed Windows voice; its language must match your selection"
        );
        assert!(
            voice_details(tts_by_id("en_US-ljspeech-high").unwrap()).starts_with("Piper native")
        );
        assert_eq!(
            asr_link(asr_by_id("tiny").unwrap()),
            "https://github.com/ggml-org/whisper.cpp"
        );
        assert!(voice_link(tts_by_id("kokoro-v1.1-int8-en").unwrap()).contains("Kokoro"));
    }

    #[test]
    fn local_services_must_be_loopback() {
        assert!(local_endpoint("http://127.0.0.1:8080/").is_ok());
        assert!(local_endpoint("http://localhost:8880/v1").is_ok());
        assert!(local_endpoint("http://[::1]:8080/").is_ok());
        assert!(local_endpoint("https://speech.example.com/").is_err());
        assert!(local_endpoint("http://192.168.1.4:8080/").is_err());
        assert!(local_endpoint("").is_err());
    }

    fn put(path: &Path, bytes: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    #[test]
    fn own_listening_models_are_validated() {
        let dir = tempfile::tempdir().unwrap();
        let mut options = LocalVoiceOptions {
            own_models: true,
            ..LocalVoiceOptions::default()
        };
        // Installed pack falls back to the catalog.
        options.own_asr_engine = "Installed pack".into();
        assert_eq!(own_asr(&options).unwrap().id, "base");
        // Whisper GGML: magic number and size.
        options.own_asr_engine = "Whisper GGML file".into();
        let model = dir.path().join("ggml-own.bin");
        options.own_asr_path = model.to_string_lossy().into_owned();
        assert!(own_asr(&options).is_err());
        let mut ggml = vec![0u8; 1024 * 1024 + 8];
        ggml[..4].copy_from_slice(&0x67676d6cu32.to_le_bytes());
        put(&model, &ggml);
        let own = own_asr(&options).unwrap();
        assert_eq!(
            (own.id.as_str(), own.engine.as_str(), own.gpu_gb),
            ("own-asr", "whisper", 1.4)
        );
        assert_eq!(own.asset.bytes, ggml.len() as u64);
        assert_eq!(own.source_path, model.to_string_lossy());
        put(&model, &vec![7u8; 2 * 1024 * 1024]);
        assert!(own_asr(&options).unwrap_err().to_string().contains("GGML"));
        options.own_asr_path = dir.path().join("x.txt").to_string_lossy().into_owned();
        assert!(own_asr(&options).is_err());
        options.own_asr_path = "relative/model.bin".into();
        assert!(own_asr(&options)
            .unwrap_err()
            .to_string()
            .contains("full drive path"));
        // Folders.
        options.own_asr_engine = "Moonshine v2 folder".into();
        let folder = dir.path().join("moon");
        options.own_asr_path = folder.to_string_lossy().into_owned();
        put(&folder.join("tokens.txt"), b"t");
        assert!(own_asr(&options)
            .unwrap_err()
            .to_string()
            .contains("encoder_model"));
        put(&folder.join("encoder_model.ort"), b"e");
        put(&folder.join("decoder_model_merged.onnx"), b"d");
        let own = own_asr(&options).unwrap();
        assert_eq!(own.engine, "moonshine");
        assert!(own.native());
        options.own_asr_engine = "Qwen3-ASR INT8 folder".into();
        assert!(own_asr(&options).is_err());
        options.own_asr_engine = "Something else".into();
        assert!(own_asr(&options)
            .unwrap_err()
            .to_string()
            .contains("supported listening"));
    }

    #[test]
    fn own_voices_are_validated() {
        let dir = tempfile::tempdir().unwrap();
        let mut options = LocalVoiceOptions {
            own_models: true,
            own_tts_engine: "Piper ONNX file".into(),
            ..LocalVoiceOptions::default()
        };
        let voice = dir.path().join("voice.onnx");
        options.own_tts_path = voice.to_string_lossy().into_owned();
        put(&voice, b"onnx");
        assert!(own_tts(&options)
            .unwrap_err()
            .to_string()
            .contains("Missing model file"));
        let config = dir.path().join("voice.onnx.json");
        put(
            &config,
            br#"{"audio":{"sample_rate":22050},"num_speakers":3}"#,
        );
        let own = own_tts(&options).unwrap();
        assert_eq!(
            (own.engine.as_str(), own.bytes, own.speaker),
            ("piper", 4, 0)
        );
        assert!(own.package.is_none());
        options.own_speaker = 2;
        assert_eq!(own_tts(&options).unwrap().speaker, 2);
        options.own_speaker = 3;
        assert!(own_tts(&options)
            .unwrap_err()
            .to_string()
            .contains("speaker ID"));
        options.own_speaker = 0;
        put(&config, br#"{"audio":{"sample_rate":4000}}"#);
        assert!(own_tts(&options).is_err());
        put(&config, b"not json");
        assert!(own_tts(&options).is_err());
        // Windows voice and installed pack need no files.
        options.own_tts_engine = "Windows voice".into();
        assert_eq!(own_tts(&options).unwrap().id, "windows");
        options.own_tts_engine = "Installed pack".into();
        assert_eq!(own_tts(&options).unwrap().id, "windows");
        // Kokoro / Kitten need English and the espeak data.
        options.own_tts_engine = "Kokoro folder".into();
        let folder = dir.path().join("kokoro");
        options.own_tts_path = folder.to_string_lossy().into_owned();
        options.speech_language = "id".into();
        assert!(own_tts(&options)
            .unwrap_err()
            .to_string()
            .contains("English"));
        options.speech_language = "en".into();
        assert!(own_tts(&options).is_err());
        put(&folder.join("model.int8.onnx"), b"m");
        for file in [
            "voices.bin",
            "tokens.txt",
            "espeak-ng-data/phontab",
            "espeak-ng-data/phondata",
            "espeak-ng-data/phonindex",
        ] {
            put(&folder.join(file), b"x");
        }
        let own = own_tts(&options).unwrap();
        assert_eq!(own.engine, "kokoro");
        assert!(own.package.is_some());
        options.own_tts_engine = "KittenTTS folder".into();
        assert_eq!(own_tts(&options).unwrap().engine, "kitten");
        options.own_speaker = 256;
        assert!(own_tts(&options)
            .unwrap_err()
            .to_string()
            .contains("Speaker ID"));
        options.own_tts_engine = "Unknown".into();
        assert!(own_tts(&options)
            .unwrap_err()
            .to_string()
            .contains("supported speaking"));
        // Weights discovery rules.
        let loose = dir.path().join("loose");
        put(&loose.join("a.onnx"), b"a");
        assert!(weights(&loose).unwrap().ends_with("a.onnx"));
        put(&loose.join("b.onnx"), b"b");
        assert!(weights(&loose).is_err());
        put(&loose.join("model.onnx"), b"m");
        assert!(weights(&loose).unwrap().ends_with("model.onnx"));
        assert!(weights(&dir.path().join("missing")).is_err());
    }

    #[test]
    fn choosing_speech_updates_a_copy_of_the_options() {
        let base = Options {
            enabled: false,
            ..Options::default()
        };
        let asr = asr_by_id("small-q5_1").unwrap();
        let tts = tts_by_id("supertonic-3-int8").unwrap();
        assert!(use_speech_pack(&base, asr, tts, true, "Quality", "es", false).is_err());
        let next = use_speech_pack(&base, asr, tts, true, "Quality", "es", true).unwrap();
        let v = &next.local_voice;
        assert!(next.enabled && v.enabled && v.managed && !v.own_models && v.managed_gpu);
        assert_eq!(
            (
                v.managed_asr.as_str(),
                v.managed_tts.as_str(),
                v.speech_preference.as_str(),
                v.speech_language.as_str()
            ),
            ("small-q5_1", "supertonic-3-int8", "Quality", "es")
        );
        assert!(
            !base.enabled && !base.local_voice.enabled,
            "the original is untouched"
        );

        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("moon");
        put(&folder.join("tokens.txt"), b"t");
        put(&folder.join("encoder_model.ort"), b"e");
        put(&folder.join("decoder_model_merged.ort"), b"d");
        let own = LocalVoiceOptions {
            own_models: true,
            own_asr_engine: "Moonshine v2 folder".into(),
            own_asr_path: folder.to_string_lossy().into_owned(),
            own_tts_engine: "Windows voice".into(),
            managed_gpu: true,
            ..LocalVoiceOptions::default()
        };
        let ready = |_: &AsrModel, _: &TtsModel, gpu: bool| {
            assert!(!gpu, "a native recognizer never uses the GPU");
            true
        };
        let cuda = hw(32.0, 20.0, 16, 8.0, 7.0, true);
        let next = use_own_speech(&base, own.clone(), Some(&cuda), &ready).unwrap();
        assert!(next.enabled && next.local_voice.own_models && !next.local_voice.managed_gpu);
        assert!(use_own_speech(&base, own.clone(), None, &|_, _, _| false).is_err());
        let broken = LocalVoiceOptions {
            own_asr_path: "relative".into(),
            ..own
        };
        assert!(use_own_speech(&base, broken, None, &ready).is_err());
    }

    #[test]
    fn names_and_environment_expansion() {
        let mut local = LocalVoiceOptions::default();
        assert_eq!(
            local_voice_names(&local),
            ("Realtime provider".into(), "Realtime provider".into())
        );
        local.enabled = true;
        local.managed = true;
        assert_eq!(
            local_voice_names(&local),
            (
                "Whisper base · everyday".into(),
                "Windows voice · already on this PC".into()
            )
        );
        local.managed = false;
        assert_eq!(
            local_voice_names(&local),
            ("tiny".into(), "Windows voice".into())
        );
        local.managed = true;
        local.own_models = true;
        local.own_asr_path = "relative".into();
        assert_eq!(local_voice_names(&local).1, "Open Voice setup");
        assert_eq!(language_name("id"), "Indonesian");
        assert_eq!(language_name("xx"), "English");
        std::env::set_var("NABY_TEST_ROOT", "/data");
        assert_eq!(expand_environment("%NABY_TEST_ROOT%/m.bin"), "/data/m.bin");
        assert_eq!(
            expand_environment("%NABY_MISSING_VAR%/m"),
            "%NABY_MISSING_VAR%/m"
        );
        assert_eq!(expand_environment("50% done"), "50% done");
    }
}
