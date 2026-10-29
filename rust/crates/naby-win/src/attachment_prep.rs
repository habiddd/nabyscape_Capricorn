//! Turns files and pasted pictures into chat attachments (`AssistantAttachments.Read`,
//! `Pasted`, `EncodedImage`).
//!
//! * Pictures are decoded to at most 1400 px and re-encoded as JPEG (quality 82) so a photo never
//!   travels at full size ("Image · optimized for chat"). PNG, JPEG and BMP are decoded here;
//!   other formats ask the user to convert them.
//! * DOCX, PPTX, XLSX and ODT become text through [`crate::office_text`].
//! * Everything else follows `Attachment::from_file` (text and code, PDF, video, audio).
//!
//! Runs on a worker thread; nothing is kept after the attachment is built.

use std::path::Path;

use naby_core::assistant::attachments::{MAXIMUM_FILE_BYTES, MAXIMUM_PICTURE_BYTES};
use naby_core::assistant::Attachment;
use naby_render::Image;
use naby_ui::assistant::ports::AttachmentPreparer;

use crate::clipboard::dib_to_rgba;
use crate::office_text;

/// Longest side of an optimized picture.
const MAXIMUM_EDGE: u32 = 1400;
const JPEG_QUALITY: u8 = 82;
const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "bmp", "tif", "tiff", "webp"];
const OFFICE_EXTENSIONS: &[&str] = &["docx", "pptx", "xlsx", "odt"];
const CONVERT: &str = "Convert this image to PNG or JPEG before attaching.";

pub struct HostPreparer;

impl AttachmentPreparer for HostPreparer {
    fn prepare_file(&self, path: &Path) -> Result<Attachment, String> {
        prepare_file(path)
    }

    fn prepare_image(&self, bytes: &[u8]) -> Result<Attachment, String> {
        if bytes.is_empty() || bytes.len() as u64 > MAXIMUM_FILE_BYTES {
            return Err("Choose a clipboard image up to 16 MB.".into());
        }
        optimized(
            "Pasted image.jpg",
            bytes,
            "Pasted image \u{b7} optimized for chat",
        )
    }
}

fn file_name(path: &Path) -> String {
    let name: String = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
        .chars()
        .take(160)
        .filter(|c| !c.is_control())
        .collect();
    if name.is_empty() {
        "attachment".into()
    } else {
        name
    }
}

pub fn prepare_file(path: &Path) -> Result<Attachment, String> {
    let meta = std::fs::metadata(path)
        .ok()
        .filter(|m| m.is_file() && m.len() > 0 && m.len() <= MAXIMUM_FILE_BYTES)
        .ok_or("Choose a nonempty file up to 16 MB.")?;
    let extension = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let name = file_name(path);
    if IMAGE_EXTENSIONS.contains(&extension.as_str()) {
        let bytes = std::fs::read(path).map_err(|_| "The file could not be read.".to_owned())?;
        let stem = Path::new(&name)
            .file_stem()
            .map_or_else(|| "image".to_owned(), |s| s.to_string_lossy().into_owned());
        return optimized(
            &format!("{stem}.jpg"),
            &bytes,
            "Image \u{b7} optimized for chat",
        );
    }
    if OFFICE_EXTENSIONS.contains(&extension.as_str()) {
        let bytes = std::fs::read(path).map_err(|_| "The file could not be read.".to_owned())?;
        let text = office_text::extract(&bytes, &extension)?;
        return Attachment::from_text(
            &name,
            text,
            "Document \u{b7} extracted text; embedded media and formatting are not included",
        )
        .map_err(|e| e.message().to_owned());
    }
    let _ = meta;
    Attachment::from_file(path).map_err(|e| e.message().to_owned())
}

/// Decodes PNG, JPEG or BMP to straight RGB at display size. Transparent pixels become white.
fn decode_rgb(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    let (width, height, rgba) =
        if bytes.starts_with(&[0x89, b'P', b'N', b'G']) || bytes.starts_with(&[0xFF, 0xD8]) {
            let (w, h) =
                Image::probe_size(bytes).ok_or("This file is not a valid PNG or JPEG image.")?;
            if w == 0
                || h == 0
                || u64::from(w) * u64::from(h) > 80_000_000
                || w.max(h) > w.min(h).saturating_mul(30)
            {
                return Err("Image dimensions are too large to preview safely.".into());
            }
            let image = Image::decode(bytes, MAXIMUM_EDGE, MAXIMUM_EDGE)
                .ok_or("This image could not be decoded.")?;
            (image.width(), image.height(), image.data().to_vec())
        } else if bytes.starts_with(b"BM") && bytes.len() > 14 {
            let image = dib_to_rgba(&bytes[14..]).map_err(|_| CONVERT.to_owned())?;
            let (w, h) = (image.width, image.height);
            if u64::from(w) * u64::from(h) > 80_000_000 {
                return Err("Image dimensions are too large to preview safely.".into());
            }
            // The DIB reader returns straight alpha; the blend below expects premultiplied pixels.
            let mut rgba = image.rgba;
            for px in rgba.chunks_exact_mut(4) {
                let a = u16::from(px[3]);
                for c in &mut px[..3] {
                    *c = ((u16::from(*c) * a + 127) / 255) as u8;
                }
            }
            (w, h, rgba)
        } else {
            return Err(CONVERT.into());
        };
    let mut rgb = Vec::with_capacity((width * height * 3) as usize);
    for px in rgba.chunks_exact(4) {
        // Premultiplied over white: c + (255 - a).
        let back = 255 - u16::from(px[3]);
        for c in &px[..3] {
            rgb.push((u16::from(*c) + back).min(255) as u8);
        }
    }
    // Downscale BMP (decoded at full size) to the limit.
    if width.max(height) > MAXIMUM_EDGE {
        return Ok(shrink(width, height, &rgb));
    }
    Ok((width, height, rgb))
}

/// Box-style nearest downscale of an RGB buffer to the longest-edge limit.
fn shrink(width: u32, height: u32, rgb: &[u8]) -> (u32, u32, Vec<u8>) {
    let (w, h) = Image::fit_size(width, height, MAXIMUM_EDGE, MAXIMUM_EDGE);
    let mut out = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..h {
        let sy = (u64::from(y) * u64::from(height) / u64::from(h)) as u32;
        for x in 0..w {
            let sx = (u64::from(x) * u64::from(width) / u64::from(w)) as u32;
            let at = ((sy * width + sx) * 3) as usize;
            out.extend_from_slice(&rgb[at..at + 3]);
        }
    }
    (w, h, out)
}

fn optimized(name: &str, bytes: &[u8], note: &str) -> Result<Attachment, String> {
    let (width, height, rgb) = decode_rgb(bytes)?;
    let mut out = Vec::new();
    let encoder = jpeg_encoder::Encoder::new(&mut out, JPEG_QUALITY);
    encoder
        .encode(
            &rgb,
            width as u16,
            height as u16,
            jpeg_encoder::ColorType::Rgb,
        )
        .map_err(|_| "This image could not be prepared.".to_owned())?;
    if out.len() as u64 > MAXIMUM_PICTURE_BYTES {
        return Err("This image is too complex. Resize it before attaching.".into());
    }
    Attachment::inline_image(name, "image/jpeg", &out, note).map_err(|e| e.message().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use naby_core::assistant::AttachmentSource;
    use std::io::{Cursor, Write};

    fn png(w: u32, h: u32, alpha: u8) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, w, h);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            let mut writer = enc.write_header().unwrap();
            let data: Vec<u8> = (0..w * h)
                .flat_map(|i| [(i % 251) as u8, 90, 160, alpha])
                .collect();
            writer.write_image_data(&data).unwrap();
        }
        out
    }

    fn bmp_24(w: u32, h: u32) -> Vec<u8> {
        let row = (w * 3).div_ceil(4) * 4;
        let mut dib = Vec::new();
        dib.extend_from_slice(&40u32.to_le_bytes());
        dib.extend_from_slice(&(w as i32).to_le_bytes());
        dib.extend_from_slice(&(h as i32).to_le_bytes());
        dib.extend_from_slice(&1u16.to_le_bytes());
        dib.extend_from_slice(&24u16.to_le_bytes());
        dib.extend_from_slice(&[0; 24]);
        dib.extend((0..row * h).map(|i| (i % 200) as u8));
        let mut file = b"BM".to_vec();
        file.extend_from_slice(&((14 + dib.len()) as u32).to_le_bytes());
        file.extend_from_slice(&[0; 4]);
        file.extend_from_slice(&54u32.to_le_bytes());
        file.extend(dib);
        file
    }

    fn inline_bytes(a: &Attachment) -> Vec<u8> {
        match &a.source {
            AttachmentSource::Inline(_) => a.bytes().unwrap(),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn pictures_become_small_jpegs_named_after_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("holiday photo.png");
        std::fs::write(&path, png(2800, 1400, 255)).unwrap();
        let attachment = prepare_file(&path).unwrap();
        assert_eq!(attachment.name, "holiday photo.jpg");
        assert_eq!(attachment.mime, "image/jpeg");
        assert_eq!(attachment.note, "Image \u{b7} optimized for chat");
        let jpeg = inline_bytes(&attachment);
        assert!(jpeg.starts_with(&[0xFF, 0xD8]));
        assert_eq!(
            Image::probe_size(&jpeg),
            Some((1400, 700)),
            "longest side limited to 1400"
        );
        assert!((jpeg.len() as u64) < MAXIMUM_PICTURE_BYTES);
    }

    #[test]
    fn transparent_pictures_land_on_white_and_bmp_files_decode() {
        let attachment = HostPreparer.prepare_image(&png(64, 64, 0)).unwrap();
        assert_eq!(attachment.name, "Pasted image.jpg");
        let decoded = Image::decode(&inline_bytes(&attachment), 64, 64).unwrap();
        let [r, g, b, _] = decoded.pixel(10, 10);
        assert!(
            r > 240 && g > 240 && b > 240,
            "transparent pixels become white: {r} {g} {b}"
        );

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("scan.bmp");
        std::fs::write(&path, bmp_24(30, 20)).unwrap();
        let attachment = prepare_file(&path).unwrap();
        assert_eq!(attachment.name, "scan.jpg");
        assert_eq!(
            Image::probe_size(&inline_bytes(&attachment)),
            Some((30, 20))
        );
    }

    #[test]
    fn unsupported_pictures_and_bad_sizes_have_clear_errors() {
        let dir = tempfile::tempdir().unwrap();
        let gif = dir.path().join("anim.gif");
        std::fs::write(&gif, b"GIF89a....").unwrap();
        assert_eq!(prepare_file(&gif).unwrap_err(), CONVERT);
        let empty = dir.path().join("empty.txt");
        std::fs::write(&empty, b"").unwrap();
        assert_eq!(
            prepare_file(&empty).unwrap_err(),
            "Choose a nonempty file up to 16 MB."
        );
        assert_eq!(
            prepare_file(&dir.path().join("missing.png")).unwrap_err(),
            "Choose a nonempty file up to 16 MB."
        );
        let strip = dir.path().join("strip.png");
        std::fs::write(&strip, png(3000, 20, 255)).unwrap();
        assert_eq!(
            prepare_file(&strip).unwrap_err(),
            "Image dimensions are too large to preview safely."
        );
        let exe = dir.path().join("tool.exe");
        std::fs::write(&exe, b"MZ").unwrap();
        assert!(prepare_file(&exe).unwrap_err().contains("not supported"));
        assert_eq!(
            HostPreparer.prepare_image(&[]).unwrap_err(),
            "Choose a clipboard image up to 16 MB."
        );
    }

    #[test]
    fn office_documents_and_text_files_become_text_attachments() {
        let dir = tempfile::tempdir().unwrap();
        let docx = dir.path().join("report.docx");
        {
            let mut zip = zip::ZipWriter::new(std::fs::File::create(&docx).unwrap());
            zip.start_file(
                "word/document.xml",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            zip.write_all(
                b"<w:document xmlns:w=\"w\"><w:body><w:p><w:r><w:t>Quarterly numbers</w:t></w:r></w:p></w:body></w:document>",
            )
            .unwrap();
            zip.finish().unwrap();
        }
        let attachment = prepare_file(&docx).unwrap();
        assert_eq!(attachment.name, "report.docx");
        assert_eq!(
            attachment.text.as_deref(),
            Some("Quarterly numbers\r\n\r\n")
        );
        assert_eq!(attachment.mime, "text/plain");
        assert!(attachment.note.starts_with("Document"));

        let broken = dir.path().join("broken.docx");
        std::fs::write(&broken, b"PK not really").unwrap();
        assert!(prepare_file(&broken)
            .unwrap_err()
            .contains("could not be read"));

        let notes = dir.path().join("notes.md");
        std::fs::write(&notes, "# Title").unwrap();
        let attachment = prepare_file(&notes).unwrap();
        assert_eq!(attachment.text.as_deref(), Some("# Title"));
        let _ = Cursor::new(Vec::<u8>::new());
    }
}
