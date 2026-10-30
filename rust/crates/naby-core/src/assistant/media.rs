//! Attachment encoding for each wire protocol and extraction of images a provider returned
//! (`AssistantTransportMedia.cs`).

use serde_json::{json, Value};
use url::Url;

use super::attachments::{
    image_parts, inline_payload, Attachment, AttachmentSource, MAXIMUM_PAYLOAD,
};
use super::models::{Protocol, Provider};
use super::providers::{is_local_host, validate_endpoint};
use super::{len16, AssistantError, Result};

/// Marker for "the provider reply is malformed or unsupported".
#[derive(Debug)]
pub(crate) struct BadReply;

pub(crate) type Bad<T> = std::result::Result<T, BadReply>;

fn protocol_of(provider: &Provider) -> Protocol {
    provider
        .protocol_kind()
        .unwrap_or(Protocol::ChatCompletions)
}

fn part_text(provider: &Provider, text: &str) -> Value {
    match protocol_of(provider) {
        Protocol::Gemini => json!({ "text": text }),
        Protocol::Responses => json!({ "type": "input_text", "text": text }),
        _ => json!({ "type": "text", "text": text }),
    }
}

/// An image part for `source` (an inline PNG/JPEG data URL).
pub(crate) fn part_image(provider: &Provider, source: &str) -> Result<Value> {
    let (mime, data) = image_parts(source)?;
    Ok(match protocol_of(provider) {
        Protocol::Responses => json!({ "type": "input_image", "image_url": source }),
        Protocol::Anthropic => json!({
            "type": "image",
            "source": { "type": "base64", "media_type": mime, "data": data }
        }),
        Protocol::Gemini => json!({ "inlineData": { "mimeType": mime, "data": data } }),
        Protocol::ChatCompletions => {
            json!({ "type": "image_url", "image_url": { "url": source } })
        }
    })
}

/// The wire parts for one attachment. With `encode == false` only the rules are checked and
/// path-backed files are not read.
pub(crate) fn attachment_parts(
    provider: &Provider,
    file: &Attachment,
    encode: bool,
) -> Result<Vec<Value>> {
    let protocol = protocol_of(provider);
    let lazy_path = !encode && matches!(file.source, AttachmentSource::Path { .. });
    if let Some(text) = &file.text {
        return Ok(vec![part_text(
            provider,
            &format!(
                "Attached document: {}\n{}\n<attachment>\n{}\n</attachment>",
                file.name, file.note, text
            ),
        )]);
    }
    if file.is_image() {
        if !provider.vision {
            return Err(AssistantError::invalid(
                "Enable Image input in AI settings for a model that supports images.",
            ));
        }
        if lazy_path {
            return Ok(vec![Value::Null]);
        }
        return Ok(vec![part_image(provider, &file.source_string()?)?]);
    }
    let video = file.is_video();
    let pdf = file.is_pdf();
    if protocol == Protocol::Gemini {
        if (pdf || video) && !provider.vision {
            return Err(AssistantError::invalid(
                "Enable Image input for this multimodal model before sending PDFs or video.",
            ));
        }
        if lazy_path {
            return Ok(vec![Value::Null]);
        }
        let source = file.source_string()?;
        let data = inline_payload(&source, &file.mime)?;
        return Ok(vec![
            json!({ "inlineData": { "mimeType": file.mime, "data": data } }),
        ]);
    }
    if video || (pdf && provider.is_hermes()) {
        if !provider.vision {
            return Err(AssistantError::invalid(
                "This provider receives PDF pages and video frames as images. Enable Image input for a compatible model.",
            ));
        }
        if file.frames.is_empty() {
            return Err(AssistantError::invalid(
                "Windows could not create video frames. Use a Gemini video model, or attach a supported MP4 video.",
            ));
        }
        let mut parts = vec![part_text(
            provider,
            &format!(
                "Attached {}. {}. These images are ordered pages or evenly sampled frames; do not claim to have read unseen pages, full motion or audio.",
                file.name, file.note
            ),
        )];
        for frame in &file.frames {
            parts.push(part_image(provider, frame)?);
        }
        return Ok(parts);
    }
    if !pdf {
        return Err(AssistantError::invalid(
            "Audio files require a Gemini model with audio input. Choose that provider or attach a transcript.",
        ));
    }
    if !provider.vision {
        return Err(AssistantError::invalid(
            "Enable Image input for a PDF-capable model before sending PDFs.",
        ));
    }
    if lazy_path {
        return Ok(vec![Value::Null]);
    }
    let source = file.source_string()?;
    let data = inline_payload(&source, &file.mime)?;
    Ok(vec![match protocol {
        Protocol::Responses => json!({
            "type": "input_file", "filename": file.name, "file_data": source
        }),
        Protocol::Anthropic => json!({
            "type": "document", "title": file.name,
            "source": { "type": "base64", "media_type": "application/pdf", "data": data }
        }),
        _ => json!({
            "type": "file", "file": { "filename": file.name, "file_data": source }
        }),
    }])
}

/// Message-level attachment checks (`CheckAttachments`). Never reads path-backed files.
pub fn check_attachments(provider: &Provider, files: &[Attachment]) -> Result<()> {
    let weight: u64 = files.iter().map(Attachment::weight).sum();
    if files.len() > super::attachments::MAXIMUM_FILES || weight > MAXIMUM_PAYLOAD {
        return Err(AssistantError::invalid(
            "Attach up to 6 files and 24 MB of prepared content per message.",
        ));
    }
    let text: usize = files.iter().map(Attachment::text_len).sum();
    if text > super::attachments::MAXIMUM_MESSAGE_TEXT {
        return Err(AssistantError::invalid(
            "Attached documents exceed 100,000 text characters in total. Split them into separate messages.",
        ));
    }
    for file in files {
        if len16(&file.name) > 180
            || file.text_len() > super::attachments::MAXIMUM_FILE_TEXT
            || file.name.chars().any(char::is_control)
        {
            return Err(AssistantError::invalid(
                "The attachment name or text is too long.",
            ));
        }
        attachment_parts(provider, file, false)?;
    }
    Ok(())
}

// ----- provider-returned media ------------------------------------------------------------

/// An address a provider returned for an image, if it is one this app may fetch
/// (`MediaUri`).
pub fn media_uri(source: &str, origin: &str) -> Result<Url> {
    let bad = || AssistantError::invalid("Invalid image address.");
    if len16(source) > 8192 {
        return Err(bad());
    }
    let uri = Url::parse(source).map_err(|_| bad())?;
    if !uri.username().is_empty() || uri.password().is_some() || uri.fragment().is_some() {
        return Err(bad());
    }
    let mut clean = uri.clone();
    clean.set_query(None);
    clean.set_fragment(None);
    validate_endpoint(clean.as_str())?;
    if uri.host_str().is_some_and(is_local_host) {
        let allowed = Url::parse(origin).ok();
        let same = allowed.is_some_and(|a| {
            a.scheme() == uri.scheme()
                && a.host_str() == uri.host_str()
                && a.port_or_known_default() == uri.port_or_known_default()
        });
        if !same {
            return Err(AssistantError::invalid(
                "Local media must belong to the connected provider.",
            ));
        }
    }
    Ok(uri)
}

const MAX_RETURNED_FILES: usize = 12;

fn add_media(
    files: &mut Vec<Attachment>,
    provider: &Provider,
    name: &str,
    mime: &str,
    source: &str,
) -> Bad<()> {
    let same = |f: &Attachment| match &f.source {
        AttachmentSource::Inline(s) | AttachmentSource::Remote(s) => s == source,
        _ => false,
    };
    if source.is_empty() || files.iter().any(same) {
        return Ok(());
    }
    let weight: u64 = files.iter().map(Attachment::weight).sum();
    if files.len() >= MAX_RETURNED_FILES
        || len16(source) as u64 > MAXIMUM_PAYLOAD
        || weight + len16(source) as u64 > MAXIMUM_PAYLOAD
    {
        return Err(BadReply);
    }
    let mut mime = mime.to_string();
    let attachment_source;
    if source.starts_with("data:") {
        let split = source.find(';').ok_or(BadReply)?;
        if !(5..=100).contains(&split) {
            return Err(BadReply);
        }
        mime = source[5..split].to_string();
        if !mime.starts_with("image/") {
            return Err(BadReply);
        }
        inline_payload(source, &mime).map_err(|_| BadReply)?;
        attachment_source = AttachmentSource::Inline(source.to_string());
    } else {
        let Ok(uri) = media_uri(source, &provider.base_url) else {
            return Ok(());
        };
        if mime.starts_with("image/") {
            let path = uri.path().to_ascii_lowercase();
            let extension = path.rsplit_once('.').map_or("", |(_, e)| e);
            mime = match extension {
                "jpg" | "jpeg" => "image/jpeg".into(),
                "gif" => "image/gif".into(),
                "webp" => "image/webp".into(),
                _ => mime,
            };
        }
        attachment_source = AttachmentSource::Remote(source.to_string());
    }
    let mut name: String = name.chars().filter(|c| !c.is_control()).take(180).collect();
    if name.is_empty() {
        name = format!("Image {}", files.len() + 1);
    }
    files.push(Attachment::remote_media(
        provider,
        &name,
        &mime,
        attachment_source,
    ));
    Ok(())
}

fn string_of<'a>(value: &'a Value, name: &str) -> &'a str {
    value.get(name).and_then(Value::as_str).unwrap_or("")
}

/// Pick an image out of one content part (`ReadMedia`). Returns whether the part was media.
pub(crate) fn read_media(
    part: &Value,
    provider: &Provider,
    files: &mut Vec<Attachment>,
) -> Bad<bool> {
    if let Some(s) = part.as_str() {
        add_media(files, provider, "", "image/png", s)?;
        return Ok(true);
    }
    if !part.is_object() {
        return Ok(false);
    }
    let kind = string_of(part, "type");
    let name = string_of(part, "filename");
    if let Some(inline) = part.get("inlineData").or_else(|| part.get("inline_data")) {
        let mut mime = string_of(inline, "mimeType");
        if mime.is_empty() {
            mime = string_of(inline, "mime_type");
        }
        if !mime.starts_with("image/") {
            return Ok(false);
        }
        let source = format!("data:{};base64,{}", mime, string_of(inline, "data"));
        add_media(files, provider, name, mime, &source)?;
        return Ok(true);
    }
    if kind == "image_generation_call" {
        let mime = match string_of(part, "output_format") {
            "jpeg" => "image/jpeg",
            "webp" => "image/webp",
            _ => "image/png",
        };
        let source = format!("data:{};base64,{}", mime, string_of(part, "result"));
        add_media(files, provider, name, mime, &source)?;
        return Ok(true);
    }
    if let Some(image) = part.get("image_url") {
        let source = image
            .as_str()
            .map_or_else(|| string_of(image, "url").to_string(), str::to_string);
        add_media(files, provider, name, "image/png", &source)?;
        return Ok(true);
    }
    if matches!(kind, "image" | "output_image" | "image_url") {
        let mut url = string_of(part, "url").to_string();
        if let Some(source) = part.get("source") {
            url = if string_of(source, "type") == "base64" {
                format!(
                    "data:{};base64,{}",
                    string_of(source, "media_type"),
                    string_of(source, "data")
                )
            } else {
                string_of(source, "url").to_string()
            };
        }
        if url.is_empty() {
            let b64 = string_of(part, "b64_json");
            if !b64.is_empty() {
                url = format!("data:image/png;base64,{b64}");
            }
        }
        add_media(files, provider, name, "image/png", &url)?;
        return Ok(true);
    }
    Ok(false)
}

const UNAVAILABLE_REMOTE: &str =
    "[Image unavailable: the returned address is not accessible from this app.]";
const UNAVAILABLE_LOCAL: &str = "[Image unavailable: the agent returned a server-local file path. Ask it to return image data or an accessible image URL.]";

struct MarkdownImage<'a> {
    len: usize,
    name: &'a str,
    url: &'a str,
}

/// `![name](<url> "title")` with a `data:image/*;base64,` or `http(s)://` address.
fn parse_markdown_image(s: &str) -> Option<MarkdownImage<'_>> {
    let rest = s.strip_prefix("![")?;
    let end = rest.find([']', '\r', '\n'])?;
    if !rest[end..].starts_with("](") {
        return None;
    }
    let name = &rest[..end];
    if name.chars().count() > 180 {
        return None;
    }
    let body_start = 2 + end + 2;
    let mut i = body_start;
    if s[i..].starts_with('<') {
        i += 1;
    }
    let url_start = i;
    let tail = &s[i..];
    let url_len = if let Some(after) = tail.strip_prefix("data:image/") {
        let sub = after
            .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-')))
            .unwrap_or(after.len());
        if sub == 0 || !after[sub..].starts_with(";base64,") {
            return None;
        }
        let data = &after[sub + ";base64,".len()..];
        let n = data
            .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=')))
            .unwrap_or(data.len());
        if n == 0 {
            return None;
        }
        "data:image/".len() + sub + ";base64,".len() + n
    } else if tail.starts_with("http://") || tail.starts_with("https://") {
        let n = tail
            .find(|c: char| c.is_whitespace() || matches!(c, ')' | '<' | '>'))
            .unwrap_or(tail.len());
        let scheme = if tail.starts_with("https") { 8 } else { 7 };
        if n <= scheme {
            return None;
        }
        n
    } else {
        return None;
    };
    let url = &s[url_start..url_start + url_len];
    i = url_start + url_len;
    if s[i..].starts_with('>') {
        i += 1;
    }
    // Optional ` "title"`.
    let after = &s[i..];
    let trimmed = after.trim_start();
    if trimmed.len() < after.len() && trimmed.starts_with('"') {
        if let Some(close) = trimmed[1..].find(['"', '\r', '\n']) {
            if trimmed[1 + close..].starts_with('"') {
                i += after.len() - trimmed.len() + close + 2;
            }
        }
    }
    if !s[i..].starts_with(')') {
        return None;
    }
    Some(MarkdownImage {
        len: i + 1,
        name,
        url,
    })
}

/// `![alt](/path)`, `![alt](file:...)` or `![alt](C:\path)`: a path only the agent's machine has.
fn local_markdown_image_len(s: &str) -> Option<usize> {
    let rest = s.strip_prefix("![")?;
    let end = rest.find([']', '\r', '\n'])?;
    if !rest[end..].starts_with("](") {
        return None;
    }
    let target = &rest[end + 2..];
    let bytes = target.as_bytes();
    let local = target.starts_with('/')
        || target.starts_with("file:")
        || (bytes.len() > 2
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'\\' | b'/'));
    if !local {
        return None;
    }
    let close = target.find([')', '\r', '\n'])?;
    if !target[close..].starts_with(')') || close == 0 {
        return None;
    }
    Some(2 + end + 2 + close + 1)
}

/// Remove returned images from the text and collect them as attachments (`ExtractImages`).
pub(crate) fn extract_images(
    text: &str,
    provider: &Provider,
    files: &mut Vec<Attachment>,
) -> Bad<String> {
    if !text.contains("![") && !text.contains("MEDIA:") {
        return Ok(text.to_string());
    }
    // Pass 1: Markdown images with data or http(s) addresses.
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while let Some(rel) = text[i..].find("![") {
        let start = i + rel;
        out.push_str(&text[i..start]);
        match parse_markdown_image(&text[start..]) {
            Some(image) => {
                let before = files.len();
                add_media(files, provider, image.name, "image/png", image.url)?;
                let known = files.len() > before
                    || files.iter().any(|f| match &f.source {
                        AttachmentSource::Inline(s) | AttachmentSource::Remote(s) => s == image.url,
                        _ => false,
                    });
                if !known {
                    out.push_str(UNAVAILABLE_REMOTE);
                }
                i = start + image.len;
            }
            None => {
                out.push_str("![");
                i = start + 2;
            }
        }
    }
    out.push_str(&text[i..]);
    // Pass 2/3: `MEDIA:` lines.
    let mut lines: Vec<String> = Vec::new();
    for line in out.split('\n') {
        let trimmed = line.trim();
        let Some(rest) = trimmed.strip_prefix("MEDIA:") else {
            lines.push(line.to_string());
            continue;
        };
        let rest = rest.trim_start();
        let is_url = (rest.starts_with("http://") || rest.starts_with("https://"))
            && !rest.chars().any(char::is_whitespace);
        let rb = rest.as_bytes();
        let is_local = rest.starts_with('/')
            || rest.starts_with("file:")
            || (rb.len() > 2
                && rb[0].is_ascii_alphabetic()
                && rb[1] == b':'
                && matches!(rb[2], b'\\' | b'/'));
        if is_url {
            add_media(files, provider, "Agent image", "image/png", rest)?;
            lines.push(String::new());
        } else if is_local {
            lines.push(UNAVAILABLE_LOCAL.to_string());
        } else {
            lines.push(line.to_string());
        }
    }
    let out = lines.join("\n");
    // Pass 4: local Markdown image paths.
    let mut result = String::with_capacity(out.len());
    let mut i = 0;
    while let Some(rel) = out[i..].find("![") {
        let start = i + rel;
        result.push_str(&out[i..start]);
        match local_markdown_image_len(&out[start..]) {
            Some(len) => {
                result.push_str(UNAVAILABLE_LOCAL);
                i = start + len;
            }
            None => {
                result.push_str("![");
                i = start + 2;
            }
        }
    }
    result.push_str(&out[i..]);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(protocol: &str) -> Provider {
        Provider {
            protocol: protocol.into(),
            base_url: "https://provider.example/v1/".into(),
            model: "m".into(),
            vision: true,
            ..Provider::default()
        }
    }

    #[test]
    fn markdown_images_are_collected_and_removed() {
        let p = provider("ChatCompletions");
        let mut files = Vec::new();
        let text = "Here ![cat](https://cdn.example.com/cat.JPG) and ![inline](data:image/png;base64,AQID \"t\") done";
        let out = extract_images(text, &p, &mut files).unwrap();
        assert_eq!(out, "Here  and  done");
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].mime, "image/jpeg");
        assert_eq!(files[0].name, "cat");
        assert_eq!(files[1].mime, "image/png");
        assert_eq!(files[0].provider_id, p.id);
        assert_eq!(files[0].origin, p.base_url);
    }

    #[test]
    fn unreachable_and_local_images_are_replaced_by_a_notice() {
        let p = provider("ChatCompletions");
        let mut files = Vec::new();
        let out = extract_images(
            "a ![x](https://10.0.0.1/a.png) b ![y](C:\\temp\\a.png) c ![z](/srv/a.png)\nMEDIA: /srv/b.png\nMEDIA: https://cdn.example.com/c.webp\nend",
            &p,
            &mut files,
        )
        .unwrap();
        assert!(out.contains("not accessible from this app"));
        assert_eq!(out.matches("server-local file path").count(), 3);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].mime, "image/webp");
        assert_eq!(files[0].name, "Agent image");
        assert!(out.ends_with("\n\nend") || out.ends_with("\nend"));
    }

    #[test]
    fn media_limits() {
        let p = provider("Responses");
        let mut files = Vec::new();
        for i in 0..12 {
            let url = format!("https://cdn.example.com/{i}.png");
            add_media(&mut files, &p, "", "image/png", &url).unwrap();
        }
        assert!(add_media(
            &mut files,
            &p,
            "",
            "image/png",
            "https://cdn.example.com/x.png"
        )
        .is_err());
        // Duplicates are ignored without error.
        add_media(
            &mut files,
            &p,
            "",
            "image/png",
            "https://cdn.example.com/1.png",
        )
        .unwrap();
        assert!(add_media(
            &mut Vec::new(),
            &p,
            "",
            "image/png",
            "data:text/plain;base64,AQID"
        )
        .is_err());
    }

    #[test]
    fn local_media_must_belong_to_the_provider() {
        let origin = "http://localhost:1234/v1/";
        assert!(media_uri("http://localhost:1234/files/a.png", origin).is_ok());
        assert!(media_uri("http://localhost:9999/files/a.png", origin).is_err());
        assert!(media_uri("http://localhost:1234/a.png", "https://other.example/").is_err());
        assert!(media_uri("https://u:p@cdn.example.com/a.png", origin).is_err());
        assert!(media_uri("https://cdn.example.com/a.png#x", origin).is_err());
        assert!(media_uri("https://192.168.1.2/a.png", origin).is_err());
        assert!(media_uri("https://cdn.example.com/a.png?sig=1", origin).is_ok());
    }

    #[test]
    fn read_media_understands_each_provider_shape() {
        let p = provider("Gemini");
        let mut files = Vec::new();
        let parts = [
            json!({"inlineData": {"mimeType": "image/png", "data": "AQID"}}),
            json!({"inline_data": {"mime_type": "image/jpeg", "data": "AQIE"}}),
            json!({"type": "image_generation_call", "output_format": "webp", "result": "AQIF"}),
            json!({"image_url": {"url": "https://cdn.example.com/a.png"}}),
            json!({"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "AQIG"}}),
            json!({"type": "output_image", "b64_json": "AQIH"}),
        ];
        for part in &parts {
            assert!(read_media(part, &p, &mut files).unwrap());
        }
        assert_eq!(files.len(), 6);
        assert!(!read_media(
            &json!({"inlineData": {"mimeType": "audio/wav", "data": "AQID"}}),
            &p,
            &mut files
        )
        .unwrap());
        assert!(!read_media(&json!({"type": "text"}), &p, &mut files).unwrap());
        assert!(!read_media(&json!(5), &p, &mut files).unwrap());
    }

    #[test]
    fn attachment_parts_follow_each_protocol() {
        let pdf = Attachment::inline(
            "a.pdf",
            "application/pdf",
            "data:application/pdf;base64,AQID".into(),
            "",
        );
        let r = attachment_parts(&provider("Responses"), &pdf, true).unwrap();
        assert_eq!(r[0]["type"], "input_file");
        let a = attachment_parts(&provider("Anthropic"), &pdf, true).unwrap();
        assert_eq!(a[0]["type"], "document");
        assert_eq!(a[0]["source"]["data"], "AQID");
        let c = attachment_parts(&provider("ChatCompletions"), &pdf, true).unwrap();
        assert_eq!(c[0]["file"]["filename"], "a.pdf");
        let g = attachment_parts(&provider("Gemini"), &pdf, true).unwrap();
        assert_eq!(g[0]["inlineData"]["mimeType"], "application/pdf");
        // Hermes receives pages as images, and needs frames.
        let mut hermes = provider("ChatCompletions");
        hermes.backend = "Hermes".into();
        assert!(attachment_parts(&hermes, &pdf, true).is_err());
        let mut framed = pdf.clone();
        framed.frames = vec!["data:image/jpeg;base64,AQID".into()];
        let parts = attachment_parts(&hermes, &framed, true).unwrap();
        assert_eq!(parts.len(), 2);
        assert!(parts[0]["text"]
            .as_str()
            .unwrap()
            .starts_with("Attached a.pdf."));
        // Capability gates.
        let mut no_vision = provider("Responses");
        no_vision.vision = false;
        assert!(attachment_parts(&no_vision, &pdf, true).is_err());
        let audio = Attachment::inline(
            "a.mp3",
            "audio/mpeg",
            "data:audio/mpeg;base64,AQID".into(),
            "",
        );
        assert!(attachment_parts(&provider("Anthropic"), &audio, true).is_err());
        assert!(attachment_parts(&provider("Gemini"), &audio, true).is_ok());
        let doc = Attachment::from_text("n.txt", "hello".into(), "note").unwrap();
        let parts = attachment_parts(&provider("Responses"), &doc, true).unwrap();
        assert_eq!(parts[0]["type"], "input_text");
        assert!(parts[0]["text"]
            .as_str()
            .unwrap()
            .contains("<attachment>\nhello\n</attachment>"));
    }
}
