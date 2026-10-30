//! Chat attachments (`AssistantAttachments`, `AssistantAttachmentSummary`, `AssistantBase64`).
//!
//! An attachment holds a path (or already-prepared inline data), never the base64 text of a
//! file: the bytes are read and encoded only when a request is built, and the encoded text is
//! dropped with that request. This keeps a 16 MB attachment from living in memory as ~21 MB of
//! base64 for the whole life of a chat.

use std::borrow::Cow;
use std::io::Read;
use std::path::{Path, PathBuf};

use base64::alphabet;
use base64::engine::general_purpose::{GeneralPurpose, GeneralPurposeConfig, STANDARD};
use base64::Engine;

/// Decoder with the same acceptance as .NET (non-canonical pad bits allowed).
const LENIENT: GeneralPurpose = GeneralPurpose::new(
    &alphabet::STANDARD,
    GeneralPurposeConfig::new().with_decode_allow_trailing_bits(true),
);

use super::models::Provider;
use super::{len16, AssistantError, Result};

pub const MAXIMUM_FILES: usize = 6;
pub const MAXIMUM_FILE_BYTES: u64 = 16 * 1024 * 1024;
pub const MAXIMUM_PAYLOAD: u64 = 24 * 1024 * 1024;
pub const MAXIMUM_FILE_TEXT: usize = 60_000;
/// Total extracted text across the attachments of one message.
pub const MAXIMUM_MESSAGE_TEXT: usize = 100_000;
/// An inline screenshot / picture data URL may not exceed this many characters.
pub const MAXIMUM_INLINE_IMAGE: usize = 3 * 1024 * 1024;
/// An optimized chat picture may not exceed this many bytes.
pub const MAXIMUM_PICTURE_BYTES: u64 = 2 * 1024 * 1024;
const MAXIMUM_TEXT_FILE_BYTES: u64 = 256 * 1024;

/// Where an attachment's payload lives.
#[derive(Debug, Clone, PartialEq)]
pub enum AttachmentSource {
    /// Text documents carry their payload in [`Attachment::text`].
    None,
    /// A complete `data:<mime>;base64,...` URL (host-prepared or provider-returned).
    Inline(String),
    /// A file read and base64-encoded lazily at send time; `bytes` is its size when attached.
    Path { path: PathBuf, bytes: u64 },
    /// An `http(s)` image address returned by a provider.
    Remote(String),
}

/// One attached file (`AssistantAttachment`).
#[derive(Debug, Clone, PartialEq)]
pub struct Attachment {
    pub name: String,
    pub mime: String,
    pub source: AttachmentSource,
    /// Extracted document text.
    pub text: Option<String>,
    /// Ordered PDF pages / sampled video frames as inline JPEG data URLs.
    pub frames: Vec<String>,
    pub note: String,
    /// Provider that returned this media (for authenticated downloads).
    pub provider_id: String,
    pub origin: String,
}

fn data_url_len(mime: &str, bytes: u64) -> u64 {
    ("data:".len() + mime.len() + ";base64,".len()) as u64 + bytes.div_ceil(3) * 4
}

impl Attachment {
    fn base(name: &str, mime: &str, source: AttachmentSource) -> Self {
        Self {
            name: name.into(),
            mime: mime.into(),
            source,
            text: None,
            frames: Vec::new(),
            note: String::new(),
            provider_id: String::new(),
            origin: String::new(),
        }
    }

    /// A prepared inline file (`data:` URL).
    pub fn inline(name: &str, mime: &str, data_url: String, note: &str) -> Self {
        let mut a = Self::base(name, mime, AttachmentSource::Inline(data_url));
        a.note = note.into();
        a
    }

    /// A host-prepared image (already PNG/JPEG encoded and size-bounded).
    pub fn inline_image(name: &str, mime: &str, bytes: &[u8], note: &str) -> Result<Self> {
        if mime != "image/png" && mime != "image/jpeg" {
            return Err(AssistantError::invalid("Use a PNG or JPEG image."));
        }
        if bytes.is_empty() || bytes.len() as u64 > MAXIMUM_PICTURE_BYTES {
            return Err(AssistantError::io(
                "This image is too complex. Resize it before attaching.",
            ));
        }
        Ok(Self::inline(name, mime, data_url(mime, bytes), note))
    }

    /// A document whose text the host (or this module) extracted.
    pub fn from_text(name: &str, text: String, note: &str) -> Result<Self> {
        if len16(&text) > MAXIMUM_FILE_TEXT {
            return Err(AssistantError::io(
                "This document exceeds 60,000 text characters. Split it into smaller files.",
            ));
        }
        if text.contains('\0') {
            return Err(AssistantError::io(
                "This file does not contain supported text.",
            ));
        }
        let mut a = Self::base(name, "text/plain", AttachmentSource::None);
        a.text = Some(text);
        a.note = note.into();
        Ok(a)
    }

    /// Attach pre-rendered PDF pages or sampled video frames (inline JPEG data URLs).
    pub fn with_frames(mut self, frames: Vec<String>, note: &str) -> Self {
        self.frames = frames;
        if !note.is_empty() {
            self.note = note.into();
        }
        self
    }

    /// Media returned by a provider.
    pub fn remote_media(
        provider: &Provider,
        name: &str,
        mime: &str,
        source: AttachmentSource,
    ) -> Self {
        let mut a = Self::base(name, mime, source);
        a.note = format!("From {}", provider.name);
        a.provider_id = provider.id.clone();
        a.origin = provider.base_url.clone();
        a
    }

    pub fn is_image(&self) -> bool {
        self.mime.starts_with("image/")
    }

    pub fn is_video(&self) -> bool {
        self.mime.starts_with("video/")
    }

    pub fn is_pdf(&self) -> bool {
        self.mime == "application/pdf"
    }

    /// Length of the payload as it will appear on the wire (the C# `Source.Length`).
    pub fn source_len(&self) -> u64 {
        match &self.source {
            AttachmentSource::None => 0,
            AttachmentSource::Inline(s) | AttachmentSource::Remote(s) => len16(s) as u64,
            AttachmentSource::Path { bytes, .. } => data_url_len(&self.mime, *bytes),
        }
    }

    /// Prepared size counted against the 24 MB per-message budget (`Weight`).
    pub fn weight(&self) -> u64 {
        self.source_len()
            + self.text.as_deref().map_or(0, |t| len16(t) as u64)
            + self.frames.iter().map(|f| len16(f) as u64).sum::<u64>()
    }

    pub fn text_len(&self) -> usize {
        self.text.as_deref().map_or(0, len16)
    }

    /// The source as the string the wire builders expect: the data URL (read and encoded now for
    /// path attachments) or the remote address.
    pub fn source_string(&self) -> Result<Cow<'_, str>> {
        match &self.source {
            AttachmentSource::None => Ok(Cow::Borrowed("")),
            AttachmentSource::Inline(s) | AttachmentSource::Remote(s) => Ok(Cow::Borrowed(s)),
            AttachmentSource::Path { path, .. } => {
                let bytes = read_bounded(path)?;
                Ok(Cow::Owned(data_url(&self.mime, &bytes)))
            }
        }
    }

    /// Raw bytes of an inline or path attachment.
    pub fn bytes(&self) -> Result<Vec<u8>> {
        match &self.source {
            AttachmentSource::Path { path, .. } => read_bounded(path),
            AttachmentSource::Inline(s) => bytes_from_data_url(s),
            _ => Err(AssistantError::io("Invalid or oversized inline file.")),
        }
    }

    /// Read a file from disk into an attachment, applying the C# type and size rules.
    ///
    /// Images are limited to PNG/JPEG within 2 MB (re-encoding other formats is a host job: pass
    /// the result to [`Attachment::inline_image`]); PDF/video frames are added by the host with
    /// [`Attachment::with_frames`]; Office documents need host text extraction
    /// ([`Attachment::from_text`]).
    pub fn from_file(path: &Path) -> Result<Self> {
        let meta = std::fs::metadata(path)
            .map_err(|_| AssistantError::io("Choose a nonempty file up to 16 MB."))?;
        if !meta.is_file() || meta.len() == 0 || meta.len() > MAXIMUM_FILE_BYTES {
            return Err(AssistantError::io("Choose a nonempty file up to 16 MB."));
        }
        let mut name: String = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
            .chars()
            .take(160)
            .filter(|c| !c.is_control())
            .collect();
        if name.is_empty() {
            name = "attachment".into();
        }
        let extension = path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        let ext = extension.as_str();
        let on_disk = |mime: &str, note: &str| {
            let mut a = Self::base(
                &name,
                mime,
                AttachmentSource::Path {
                    path: path.to_path_buf(),
                    bytes: meta.len(),
                },
            );
            a.note = note.into();
            a
        };
        match ext {
            "png" | "jpg" | "jpeg" => {
                let mime = if ext == "png" { "image/png" } else { "image/jpeg" };
                let mut head = [0u8; 8];
                let n = std::fs::File::open(path)
                    .and_then(|mut f| f.read(&mut head))
                    .map_err(|_| AssistantError::io("Choose a nonempty file up to 16 MB."))?;
                let ok = if ext == "png" {
                    head[..n].starts_with(&[0x89, b'P', b'N', b'G'])
                } else {
                    head[..n].starts_with(&[0xFF, 0xD8, 0xFF])
                };
                if !ok {
                    return Err(AssistantError::io("This file is not a valid PNG or JPEG image."));
                }
                if meta.len() > MAXIMUM_PICTURE_BYTES {
                    return Err(AssistantError::io(
                        "This image is too complex. Resize it before attaching.",
                    ));
                }
                Ok(on_disk(mime, "Image · sent as attached"))
            }
            "gif" | "bmp" | "tif" | "tiff" | "webp" => Err(AssistantError::io(
                "Convert this image to PNG or JPEG before attaching.",
            )),
            "pdf" => Ok(on_disk("application/pdf", "PDF · original document")),
            "mp4" | "webm" | "mov" | "avi" | "mkv" => {
                let mime = match ext {
                    "mp4" => "video/mp4",
                    "webm" => "video/webm",
                    "mov" => "video/quicktime",
                    "avi" => "video/x-msvideo",
                    _ => "video/x-matroska",
                };
                Ok(on_disk(
                    mime,
                    "Video · original file for Gemini; no preview frames were created",
                ))
            }
            "mp3" | "wav" | "m4a" | "ogg" | "flac" => {
                let mime = match ext {
                    "mp3" => "audio/mpeg",
                    "wav" => "audio/wav",
                    "m4a" => "audio/mp4",
                    "ogg" => "audio/ogg",
                    _ => "audio/flac",
                };
                Ok(on_disk(mime, "Audio · requires Gemini audio input"))
            }
            "docx" | "pptx" | "xlsx" | "odt" => Err(AssistantError::io(
                "Office documents are converted to text by the app before attaching.",
            )),
            _ if TEXT_EXTENSIONS.contains(&ext) => {
                if meta.len() > MAXIMUM_TEXT_FILE_BYTES {
                    return Err(AssistantError::io(
                        "This text file is too large. Split it into smaller files.",
                    ));
                }
                let bytes = read_bounded(path)?;
                let slice = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&bytes);
                let text = std::str::from_utf8(slice).map_err(|_| {
                    AssistantError::io("This file does not contain supported text.")
                })?;
                Self::from_text(
                    &name,
                    text.to_string(),
                    "Document · extracted text; embedded media and formatting are not included",
                )
            }
            _ => Err(AssistantError::io(
                "This file type is not supported. Attach an image, PDF, video, audio, text, code, DOCX, PPTX, XLSX or ODT file.",
            )),
        }
    }
}

const TEXT_EXTENSIONS: &[&str] = &[
    "txt", "md", "csv", "tsv", "json", "jsonl", "xml", "html", "htm", "css", "js", "ts", "tsx",
    "jsx", "py", "cs", "cpp", "c", "h", "java", "rs", "go", "sql", "yaml", "yml", "toml", "ini",
    "log", "srt", "vtt", "sh", "ps1",
];

fn read_bounded(path: &Path) -> Result<Vec<u8>> {
    let file = std::fs::File::open(path)
        .map_err(|_| AssistantError::io("The attached file is no longer available."))?;
    let mut out = Vec::new();
    file.take(MAXIMUM_FILE_BYTES + 1)
        .read_to_end(&mut out)
        .map_err(|_| AssistantError::io("The attached file could not be read."))?;
    if out.len() as u64 > MAXIMUM_FILE_BYTES {
        return Err(AssistantError::io("The file exceeds 16 MB."));
    }
    Ok(out)
}

/// `data:<mime>;base64,<payload>`.
pub fn data_url(mime: &str, bytes: &[u8]) -> String {
    let mut out = String::with_capacity(data_url_len(mime, bytes.len() as u64) as usize);
    out.push_str("data:");
    out.push_str(mime);
    out.push_str(";base64,");
    STANDARD.encode_string(bytes, &mut out);
    out
}

/// Decode the payload of an inline data URL (`AssistantAttachments.Bytes`).
pub fn bytes_from_data_url(source: &str) -> Result<Vec<u8>> {
    let comma = source.find(',');
    match comma {
        Some(c)
            if source.starts_with("data:")
                && c <= 150
                && source.len() as u64 <= MAXIMUM_PAYLOAD =>
        {
            let payload: String = source[c + 1..]
                .chars()
                .filter(|c| !matches!(c, ' ' | '\t' | '\r' | '\n'))
                .collect();
            LENIENT
                .decode(payload)
                .map_err(|_| AssistantError::io("Invalid or oversized inline file."))
        }
        _ => Err(AssistantError::io("Invalid or oversized inline file.")),
    }
}

/// Whether `text` is acceptable to .NET `Convert.FromBase64String` (whitespace is ignored,
/// padding is at most two trailing `=`, pad bits are not checked).
pub fn is_valid_base64(text: &str) -> bool {
    let mut count = 0usize;
    let mut padding = 0usize;
    for b in text.bytes() {
        match b {
            b' ' | b'\t' | b'\r' | b'\n' => continue,
            b'=' => {
                padding += 1;
                if padding > 2 {
                    return false;
                }
            }
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'+' | b'/' => {
                if padding > 0 {
                    return false;
                }
            }
            _ => return false,
        }
        count += 1;
    }
    count.is_multiple_of(4)
}

/// Validate an inline PNG/JPEG screenshot data URL and split it into `(mime, base64)`
/// (`AssistantTransport.Image`).
pub fn image_parts(value: &str) -> Result<(&'static str, &str)> {
    let split = value.find(',');
    let too_large = || AssistantError::invalid("The screenshot is invalid or too large.");
    let Some(split) = split else {
        return Err(too_large());
    };
    if len16(value) > MAXIMUM_INLINE_IMAGE {
        return Err(too_large());
    }
    let mime = match &value[..split] {
        "data:image/jpeg;base64" => "image/jpeg",
        "data:image/png;base64" => "image/png",
        _ => "",
    };
    let data = &value[split + 1..];
    if mime.is_empty()
        || data.is_empty()
        || data
            .bytes()
            .any(|b| !(b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'=')))
    {
        return Err(AssistantError::invalid(
            "Use an inline PNG or JPEG screenshot.",
        ));
    }
    if !is_valid_base64(data) {
        return Err(AssistantError::invalid(
            "The screenshot encoding is invalid.",
        ));
    }
    Ok((mime, data))
}

/// Validate `data:<mime>;base64,<payload>` and return the payload (`AssistantTransport.Inline`).
pub fn inline_payload<'a>(source: &'a str, mime: &str) -> Result<&'a str> {
    let prefix = format!("data:{mime};base64,");
    if !source.starts_with(&prefix)
        || source.len() <= prefix.len()
        || source.len() as u64 > MAXIMUM_PAYLOAD
    {
        return Err(AssistantError::invalid(
            "Invalid or oversized attachment data.",
        ));
    }
    let payload = &source[prefix.len()..];
    if !is_valid_base64(payload) {
        return Err(AssistantError::invalid("Invalid attachment encoding."));
    }
    Ok(payload)
}

/// The note shown under an attachment for the selected provider (`AssistantAttachmentSummary`).
pub fn describe(file: &Attachment, provider: Option<&Provider>) -> String {
    let native = provider.is_some_and(|p| p.protocol == "Gemini");
    let hermes = provider.is_some_and(|p| p.is_hermes());
    let mut note = file.note.clone();
    if file.is_pdf() {
        note = if hermes {
            format!(
                "PDF · first {} pages sent as images; later pages are not included.",
                file.frames.len()
            )
        } else {
            "PDF · original document sent to the provider; PDF support depends on the selected model."
                .into()
        };
    } else if file.is_video() {
        note = if native {
            "Video · original video sent to Gemini".into()
        } else {
            format!(
                "Video · {} sampled frames · audio is not included.",
                file.frames.len()
            )
        };
    }
    if (file.is_image() || !file.frames.is_empty()) && !provider.is_some_and(|p| p.vision) {
        note.push_str(
            " Image input is Off. Enable it and save your provider before sending visual content.",
        );
    }
    note
}

/// Check that a draft may take one more attachment (`AttachFiles` / `AttachPreparedImage`).
pub fn check_draft_add(existing: &[Attachment], incoming: &Attachment) -> Result<()> {
    if existing.len() >= MAXIMUM_FILES {
        return Err(AssistantError::invalid("Attach up to 6 files per message."));
    }
    let weight: u64 = existing.iter().map(Attachment::weight).sum::<u64>() + incoming.weight();
    if weight > MAXIMUM_PAYLOAD {
        return Err(AssistantError::invalid(
            "Prepared attachments exceed 24 MB. Send fewer or smaller files.",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_acceptance_matches_dotnet() {
        for ok in [
            "",
            "AQID",
            "AQ==",
            "AQID\r\n",
            "A Q\t=\r=\n",
            "AR==",
            "AAA=",
            "AA==\n",
        ] {
            assert!(is_valid_base64(ok), "{ok:?}");
        }
        for bad in [
            "AQ=",
            "A===",
            "AA=A",
            "====",
            "AQ==A",
            "AQID\u{a0}",
            "AQID\u{b}",
            "AQID_",
            "AAAA=",
            "=AAA",
        ] {
            assert!(!is_valid_base64(bad), "{bad:?}");
        }
        // Non-canonical pad bits are accepted, like Convert.FromBase64String.
        assert!(is_valid_base64("AAAAAR=="));
        assert!(is_valid_base64(" \tAAAA\r\nA A=\r=\n"));
    }

    #[test]
    fn image_parts_reports_the_documented_messages() {
        let url = |d: &str| format!("data:image/jpeg;base64,{d}");
        assert_eq!(image_parts(&url("AQID")).unwrap(), ("image/jpeg", "AQID"));
        let err = |v: &str| image_parts(v).unwrap_err().to_string();
        assert_eq!(err(&url("")), "Use an inline PNG or JPEG screenshot.");
        assert_eq!(err(&url("AQID_")), "Use an inline PNG or JPEG screenshot.");
        assert_eq!(err(&url("AQ=")), "The screenshot encoding is invalid.");
        assert_eq!(
            err("https://x.example/a.jpg"),
            "The screenshot is invalid or too large."
        );
        let huge = url(&"A".repeat(3 * 1024 * 1024));
        assert_eq!(err(&huge), "The screenshot is invalid or too large.");
        assert_eq!(
            err("data:image/gif;base64,AQID"),
            "Use an inline PNG or JPEG screenshot."
        );
        assert_eq!(
            inline_payload("data:application/pdf;base64,AQID", "application/pdf").unwrap(),
            "AQID"
        );
        assert_eq!(
            inline_payload("data:application/pdf;base64,", "application/pdf")
                .unwrap_err()
                .to_string(),
            "Invalid or oversized attachment data."
        );
        assert_eq!(
            inline_payload("data:application/pdf;base64,AQ=", "application/pdf")
                .unwrap_err()
                .to_string(),
            "Invalid attachment encoding."
        );
    }

    #[test]
    fn path_attachments_are_encoded_lazily() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("shot.png");
        let mut bytes = vec![0x89, b'P', b'N', b'G'];
        bytes.extend_from_slice(&[7u8; 3000]);
        std::fs::write(&file, &bytes).unwrap();
        let a = Attachment::from_file(&file).unwrap();
        assert!(matches!(a.source, AttachmentSource::Path { .. }));
        assert!(a.is_image() && a.mime == "image/png");
        let url = a.source_string().unwrap().into_owned();
        assert_eq!(url.len() as u64, a.source_len());
        assert_eq!(a.weight(), a.source_len());
        assert_eq!(bytes_from_data_url(&url).unwrap(), bytes);
        // No payload copy is held by the attachment itself.
        assert!(std::mem::size_of::<Attachment>() < 400);
    }

    #[test]
    fn file_rules_follow_the_csharp_reader() {
        let dir = tempfile::tempdir().unwrap();
        let p = |n: &str, data: &[u8]| {
            let f = dir.path().join(n);
            std::fs::write(&f, data).unwrap();
            f
        };
        assert!(Attachment::from_file(&p("empty.txt", b"")).is_err());
        let doc = Attachment::from_file(&p("a.md", "héllo".as_bytes())).unwrap();
        assert_eq!(doc.text.as_deref(), Some("héllo"));
        assert_eq!(doc.mime, "text/plain");
        let bom = Attachment::from_file(&p("b.txt", b"\xEF\xBB\xBFhi")).unwrap();
        assert_eq!(bom.text.as_deref(), Some("hi"));
        assert!(Attachment::from_file(&p("c.txt", b"a\0b")).is_err());
        assert!(Attachment::from_file(&p("d.txt", &[0xff, 0xfe, 0xfd])).is_err());
        let big = "x".repeat(60_001);
        assert!(Attachment::from_file(&p("e.txt", big.as_bytes())).is_err());
        assert!(Attachment::from_file(&p("f.xyz", b"data")).is_err());
        assert!(Attachment::from_file(&p("g.png", b"not a png")).is_err());
        assert!(Attachment::from_file(&p("h.gif", b"GIF89a....")).is_err());
        let pdf = Attachment::from_file(&p("i.pdf", b"%PDF-1.4")).unwrap();
        assert!(pdf.is_pdf() && pdf.frames.is_empty());
        let video = Attachment::from_file(&p("v.mp4", b"....")).unwrap();
        assert_eq!(video.mime, "video/mp4");
        let audio = Attachment::from_file(&p("s.mp3", b"ID3")).unwrap();
        assert_eq!(audio.mime, "audio/mpeg");
    }

    #[test]
    fn draft_limits() {
        let a = Attachment::from_text("a.txt", "x".into(), "").unwrap();
        let six = vec![a.clone(); 6];
        assert!(check_draft_add(&six, &a).is_err());
        assert!(check_draft_add(&six[..5], &a).is_ok());
        let heavy =
            Attachment::inline("h.pdf", "application/pdf", "x".repeat(24 * 1024 * 1024), "");
        assert!(check_draft_add(&[heavy], &a).is_err());
    }

    #[test]
    fn summaries_depend_on_provider() {
        let mut pdf = Attachment::inline(
            "a.pdf",
            "application/pdf",
            "data:application/pdf;base64,AQID".into(),
            "",
        );
        pdf.frames = vec!["f".into(), "g".into()];
        let hermes = Provider {
            backend: "Hermes".into(),
            vision: true,
            ..Provider::default()
        };
        assert!(describe(&pdf, Some(&hermes)).contains("first 2 pages sent as images"));
        let plain = Provider::default();
        let text = describe(&pdf, Some(&plain));
        assert!(text.starts_with("PDF · original document") && text.contains("Image input is Off"));
        let mut video = Attachment::inline(
            "a.mp4",
            "video/mp4",
            "data:video/mp4;base64,AQID".into(),
            "",
        );
        video.frames = vec!["f".into()];
        let gemini = Provider {
            protocol: "Gemini".into(),
            vision: true,
            ..Provider::default()
        };
        assert_eq!(
            describe(&video, Some(&gemini)),
            "Video · original video sent to Gemini"
        );
        assert!(describe(&video, Some(&hermes)).starts_with("Video · 1 sampled frames"));
    }
}
