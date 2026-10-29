//! Text extraction from Office documents (`AssistantAttachments.DocumentText`): DOCX, PPTX, XLSX
//! and ODT are zip archives of XML; only the text is read, never any embedded media, macro or
//! external reference.
//!
//! Bounds match the C# code: at most 2000 archive entries, 200 content parts, 4 MiB of expanded
//! XML and 60,000 characters of text. DTDs are refused (no entity expansion), so a hostile file
//! cannot blow up in memory.

use std::cmp::Ordering;
use std::io::{Cursor, Read};

/// Result of an extraction; `Err` carries the user-facing message.
pub type Extracted = Result<String, String>;

const MAXIMUM_ENTRIES: usize = 2000;
const MAXIMUM_PARTS: usize = 200;
const MAXIMUM_EXPANDED: u64 = 4 * 1024 * 1024;
const MAXIMUM_TEXT: usize = 60_000;
const MAXIMUM_DEPTH: usize = 128;

/// Extracts the text of `bytes`, a document with the lower-case extension (with the dot, or
/// without) `extension`.
pub fn extract(bytes: &[u8], extension: &str) -> Extracted {
    let extension = extension.trim_start_matches('.').to_ascii_lowercase();
    if !matches!(extension.as_str(), "docx" | "pptx" | "xlsx" | "odt") {
        return Err("This file type is not supported.".into());
    }
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| {
        "This document could not be read. It may be damaged or encrypted.".to_owned()
    })?;
    if archive.len() > MAXIMUM_ENTRIES {
        return Err("This document contains too many embedded items.".into());
    }
    let mut names: Vec<(String, usize)> = (0..archive.len())
        .filter_map(|i| {
            let name = archive.name_for_index(i)?.to_owned();
            let wanted = match extension.as_str() {
                "docx" => name == "word/document.xml",
                "pptx" => name.starts_with("ppt/slides/slide") && name.ends_with(".xml"),
                "xlsx" => name.starts_with("xl/worksheets/sheet") && name.ends_with(".xml"),
                _ => name == "content.xml",
            };
            wanted.then_some((name, i))
        })
        .collect();
    names.sort_by(|a, b| natural(&a.0, &b.0));
    if names.len() > MAXIMUM_PARTS {
        return Err("The document contains too many sections. Split it into smaller files.".into());
    }
    let mut expanded = 0u64;
    let mut read =
        |archive: &mut zip::ZipArchive<Cursor<&[u8]>>, index: usize| -> Result<Element, String> {
            let entry = archive
                .by_index(index)
                .map_err(|_| "This document could not be read.".to_owned())?;
            expanded += entry.size();
            if expanded > MAXIMUM_EXPANDED {
                return Err("The expanded document is too large.".into());
            }
            let mut xml = Vec::new();
            entry
                .take(MAXIMUM_EXPANDED + 1)
                .read_to_end(&mut xml)
                .map_err(|_| "This document could not be read.".to_owned())?;
            if xml.len() as u64 > MAXIMUM_EXPANDED {
                return Err("The expanded document is too large.".into());
            }
            parse(&xml)
        };
    let mut shared: Vec<String> = Vec::new();
    if extension == "xlsx" {
        let strings =
            (0..archive.len()).find(|i| archive.name_for_index(*i) == Some("xl/sharedStrings.xml"));
        if let Some(index) = strings {
            let root = read(&mut archive, index)?;
            shared = root
                .descendants()
                .filter(|n| n.name == "si")
                .map(|si| {
                    si.descendants()
                        .filter(|t| t.name == "t")
                        .map(Element::text)
                        .collect::<String>()
                })
                .collect();
        }
    }
    let mut result = String::new();
    for (name, index) in names {
        let root = read(&mut archive, index)?;
        if extension == "xlsx" {
            result.push_str(&name);
            result.push_str("\r\n");
            for row in root.descendants().filter(|n| n.name == "row") {
                for cell in row.children.iter().filter(|n| n.name == "c") {
                    let mut value = cell
                        .descendants()
                        .find(|n| n.name == "v")
                        .map(Element::text)
                        .unwrap_or_else(|| {
                            cell.descendants()
                                .filter(|n| n.name == "t")
                                .map(Element::text)
                                .collect()
                        });
                    if cell.attribute("t") == Some("s") {
                        if let Some(shared) = value
                            .trim()
                            .parse::<usize>()
                            .ok()
                            .and_then(|i| shared.get(i))
                        {
                            value = shared.clone();
                        }
                    }
                    result.push_str(cell.attribute("r").unwrap_or(""));
                    result.push_str(": ");
                    result.push_str(&value);
                    result.push('\t');
                }
                result.push_str("\r\n");
            }
        } else {
            for paragraph in root
                .descendants()
                .filter(|n| n.name == "p" || (extension == "odt" && n.name == "h"))
            {
                result.push_str(&paragraph.text());
                result.push_str("\r\n");
            }
        }
        if result.encode_utf16().count() > MAXIMUM_TEXT {
            return Err("This document exceeds 60,000 text characters.".into());
        }
        result.push_str("\r\n");
    }
    if result.is_empty() {
        return Err("No readable text was found in this document.".into());
    }
    Ok(result)
}

/// Compares names with digit runs ordered by value (`NumericOrdering`): `slide2` < `slide10`.
fn natural(a: &str, b: &str) -> Ordering {
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(p), Some(q)) if p.is_ascii_digit() && q.is_ascii_digit() => {
                let mut left = String::new();
                while let Some(c) = x.next_if(char::is_ascii_digit) {
                    left.push(c);
                }
                let mut right = String::new();
                while let Some(c) = y.next_if(char::is_ascii_digit) {
                    right.push(c);
                }
                let (l, r) = (left.trim_start_matches('0'), right.trim_start_matches('0'));
                let order = l.len().cmp(&r.len()).then_with(|| l.cmp(r));
                if order != Ordering::Equal {
                    return order;
                }
            }
            (Some(p), Some(q)) => {
                if p != q {
                    return p.cmp(&q);
                }
                x.next();
                y.next();
            }
        }
    }
}

// ---- a tiny XML reader -------------------------------------------------------------------

/// An element with its local name (the namespace prefix is dropped), attributes and children.
#[derive(Debug, Default)]
struct Element {
    name: String,
    attributes: Vec<(String, String)>,
    children: Vec<Element>,
    /// Text directly inside this element, in order with the children (`None` = a child here).
    content: Vec<Content>,
}

#[derive(Debug)]
enum Content {
    Text(String),
    Child(usize),
}

impl Element {
    fn attribute(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    /// All descendants in document order (not including self).
    fn descendants(&self) -> Descendants<'_> {
        Descendants {
            stack: vec![self.children.iter()],
        }
    }

    /// Concatenated text of this element and everything below it.
    fn text(&self) -> String {
        let mut out = String::new();
        self.collect_text(&mut out);
        out
    }

    fn collect_text(&self, out: &mut String) {
        for part in &self.content {
            match part {
                Content::Text(t) => out.push_str(t),
                Content::Child(i) => self.children[*i].collect_text(out),
            }
        }
    }
}

struct Descendants<'a> {
    stack: Vec<std::slice::Iter<'a, Element>>,
}

impl<'a> Iterator for Descendants<'a> {
    type Item = &'a Element;

    fn next(&mut self) -> Option<&'a Element> {
        loop {
            let top = self.stack.last_mut()?;
            match top.next() {
                Some(element) => {
                    self.stack.push(element.children.iter());
                    return Some(element);
                }
                None => {
                    self.stack.pop();
                }
            }
        }
    }
}

fn local(name: &str) -> &str {
    name.rsplit(':').next().unwrap_or(name)
}

fn unescape(text: &str) -> Result<String, String> {
    if !text.contains('&') {
        return Ok(text.to_owned());
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        let end = after
            .find(';')
            .ok_or("The document contains invalid XML.")?;
        let entity = &after[..end];
        match entity {
            "amp" => out.push('&'),
            "lt" => out.push('<'),
            "gt" => out.push('>'),
            "quot" => out.push('"'),
            "apos" => out.push('\''),
            _ => {
                let code = entity
                    .strip_prefix("#x")
                    .or_else(|| entity.strip_prefix("#X"))
                    .map(|h| u32::from_str_radix(h, 16))
                    .or_else(|| entity.strip_prefix('#').map(|d| d.parse::<u32>()))
                    .and_then(Result::ok)
                    .and_then(char::from_u32)
                    .ok_or("The document contains an unsupported XML entity.")?;
                out.push(code);
            }
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

fn parse(bytes: &[u8]) -> Result<Element, String> {
    let invalid = || "The document contains invalid XML.".to_owned();
    let text = std::str::from_utf8(bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes))
        .map_err(|_| invalid())?;
    if text.contains("<!DOCTYPE") || text.contains("<!ENTITY") {
        return Err("The document uses a feature that is not allowed (DTD).".into());
    }
    let mut stack: Vec<Element> = vec![Element::default()];
    let mut rest = text;
    while !rest.is_empty() {
        let Some(open) = rest.find('<') else {
            push_text(&mut stack, rest)?;
            break;
        };
        if open > 0 {
            push_text(&mut stack, &rest[..open])?;
        }
        rest = &rest[open..];
        if let Some(tail) = rest.strip_prefix("<!--") {
            let end = tail.find("-->").ok_or_else(invalid)?;
            rest = &tail[end + 3..];
        } else if let Some(tail) = rest.strip_prefix("<![CDATA[") {
            let end = tail.find("]]>").ok_or_else(invalid)?;
            let top = stack.last_mut().ok_or_else(invalid)?;
            top.content.push(Content::Text(tail[..end].to_owned()));
            rest = &tail[end + 3..];
        } else if let Some(tail) = rest.strip_prefix("<?") {
            let end = tail.find("?>").ok_or_else(invalid)?;
            rest = &tail[end + 2..];
        } else if let Some(tail) = rest.strip_prefix("</") {
            let end = tail.find('>').ok_or_else(invalid)?;
            let name = local(tail[..end].trim());
            let done = stack.pop().ok_or_else(invalid)?;
            if done.name != name || stack.is_empty() {
                return Err(invalid());
            }
            attach(&mut stack, done)?;
            rest = &tail[end + 1..];
        } else {
            let tail = &rest[1..];
            let end = tag_end(tail).ok_or_else(invalid)?;
            let mut body = &tail[..end];
            let empty = body.ends_with('/');
            if empty {
                body = &body[..body.len() - 1];
            }
            let (name, attributes) = parse_tag(body)?;
            let element = Element {
                name: local(&name).to_owned(),
                attributes,
                ..Element::default()
            };
            if empty {
                attach_element(&mut stack, element)?;
            } else {
                if stack.len() > MAXIMUM_DEPTH {
                    return Err("The document is nested too deeply.".into());
                }
                stack.push(element);
            }
            rest = &tail[end + 1..];
        }
    }
    if stack.len() != 1 {
        return Err(invalid());
    }
    let mut document = stack.pop().ok_or_else(invalid)?;
    document.children.pop().ok_or_else(invalid)
}

/// The index of the `>` closing a tag, skipping quoted attribute values.
fn tag_end(tail: &str) -> Option<usize> {
    let mut quote: Option<char> = None;
    for (i, c) in tail.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (None, '"') | (None, '\'') => quote = Some(c),
            (None, '>') => return Some(i),
            _ => {}
        }
    }
    None
}

fn parse_tag(body: &str) -> Result<(String, Vec<(String, String)>), String> {
    let invalid = || "The document contains invalid XML.".to_owned();
    let body = body.trim();
    let name_end = body.find(char::is_whitespace).unwrap_or(body.len());
    let name = body[..name_end].to_owned();
    if name.is_empty() {
        return Err(invalid());
    }
    let mut attributes = Vec::new();
    let mut rest = body[name_end..].trim_start();
    while !rest.is_empty() {
        let eq = rest.find('=').ok_or_else(invalid)?;
        let key = local(rest[..eq].trim()).to_owned();
        let after = rest[eq + 1..].trim_start();
        let quote = after
            .chars()
            .next()
            .filter(|c| matches!(c, '"' | '\''))
            .ok_or_else(invalid)?;
        let close = after[1..].find(quote).ok_or_else(invalid)?;
        attributes.push((key, unescape(&after[1..1 + close])?));
        rest = after[close + 2..].trim_start();
    }
    Ok((name, attributes))
}

fn push_text(stack: &mut [Element], text: &str) -> Result<(), String> {
    let top = stack
        .last_mut()
        .ok_or("The document contains invalid XML.")?;
    top.content.push(Content::Text(unescape(text)?));
    Ok(())
}

fn attach(stack: &mut [Element], element: Element) -> Result<(), String> {
    let parent = stack
        .last_mut()
        .ok_or("The document contains invalid XML.")?;
    parent.children.push(element);
    parent
        .content
        .push(Content::Child(parent.children.len() - 1));
    Ok(())
}

fn attach_element(stack: &mut [Element], element: Element) -> Result<(), String> {
    attach(stack, element)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;
    use zip::CompressionMethod;

    fn archive(files: &[(&str, &str)]) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut out);
            let options =
                SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
            for (name, content) in files {
                zip.start_file(*name, options).unwrap();
                zip.write_all(content.as_bytes()).unwrap();
            }
            zip.finish().unwrap();
        }
        out.into_inner()
    }

    const W: &str = "xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"";

    #[test]
    fn docx_text_comes_from_paragraphs_with_entities_decoded() {
        let xml = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><w:document {W}><w:body><w:p><w:r><w:t>Hello </w:t></w:r><w:r><w:t xml:space=\"preserve\">&amp; welcome</w:t></w:r></w:p><w:p><w:r><w:t>Second &#233; line</w:t></w:r></w:p><w:p/></w:body></w:document>"
        );
        let docx = archive(&[
            ("[Content_Types].xml", "<Types/>"),
            ("word/document.xml", &xml),
        ]);
        assert_eq!(
            extract(&docx, ".docx").unwrap(),
            "Hello & welcome\r\nSecond \u{e9} line\r\n\r\n\r\n"
        );
    }

    #[test]
    fn pptx_slides_are_read_in_numeric_order() {
        let slide = |text: &str| {
            format!("<p:sld xmlns:p=\"p\" xmlns:a=\"a\"><a:p><a:r><a:t>{text}</a:t></a:r></a:p></p:sld>")
        };
        let pptx = archive(&[
            ("ppt/slides/slide10.xml", &slide("ten")),
            ("ppt/slides/slide2.xml", &slide("two")),
            ("ppt/slides/slide1.xml", &slide("one")),
            ("ppt/slides/_rels/slide1.xml.rels", "<r/>"),
            ("ppt/theme/theme1.xml", "<t/>"),
        ]);
        let text = extract(&pptx, "pptx").unwrap();
        let order: Vec<&str> = text.lines().filter(|l| !l.is_empty()).collect();
        assert_eq!(order, ["one", "two", "ten"]);
    }

    #[test]
    fn xlsx_cells_use_shared_strings_and_keep_their_references() {
        let strings =
            "<sst><si><t>Name</t></si><si><r><t>Total</t></r><r><t> due</t></r></si></sst>";
        let sheet = "<worksheet><sheetData><row r=\"1\"><c r=\"A1\" t=\"s\"><v>0</v></c><c r=\"B1\" t=\"s\"><v>1</v></c></row><row r=\"2\"><c r=\"A2\"><v>42</v></c><c r=\"B2\" t=\"inlineStr\"><is><t>inline</t></is></c></row></sheetData></worksheet>";
        let xlsx = archive(&[
            ("xl/sharedStrings.xml", strings),
            ("xl/worksheets/sheet1.xml", sheet),
        ]);
        let text = extract(&xlsx, "xlsx").unwrap();
        assert_eq!(
            text,
            "xl/worksheets/sheet1.xml\r\nA1: Name\tB1: Total due\t\r\nA2: 42\tB2: inline\t\r\n\r\n"
        );
    }

    #[test]
    fn odt_headings_and_paragraphs_are_read() {
        let content = "<office:document-content xmlns:office=\"o\" xmlns:text=\"t\"><office:body><office:text><text:h>Title</text:h><text:p>Body <text:span>text</text:span></text:p></office:text></office:body></office:document-content>";
        let odt = archive(&[
            ("mimetype", "application/vnd.oasis.opendocument.text"),
            ("content.xml", content),
        ]);
        assert_eq!(extract(&odt, "odt").unwrap(), "Title\r\nBody text\r\n\r\n");
    }

    #[test]
    fn unsafe_or_oversized_documents_are_refused() {
        // DTDs (entity expansion) are never processed.
        let evil = format!("<!DOCTYPE d [<!ENTITY x \"boom\">]><w:document {W}><w:p><w:t>&x;</w:t></w:p></w:document>");
        let docx = archive(&[("word/document.xml", &evil)]);
        assert!(extract(&docx, "docx").unwrap_err().contains("not allowed"));

        // Too many archive entries.
        let files: Vec<(String, &str)> = (0..2001).map(|i| (format!("f{i}.txt"), "x")).collect();
        let refs: Vec<(&str, &str)> = files.iter().map(|(n, c)| (n.as_str(), *c)).collect();
        assert_eq!(
            extract(&archive(&refs), "docx").unwrap_err(),
            "This document contains too many embedded items."
        );

        // Expanded size bomb.
        let big = format!(
            "<w:document {W}><w:p><w:t>{}</w:t></w:p></w:document>",
            "a".repeat(5 * 1024 * 1024)
        );
        assert_eq!(
            extract(&archive(&[("word/document.xml", &big)]), "docx").unwrap_err(),
            "The expanded document is too large."
        );

        // Too much text.
        let long = format!(
            "<w:document {W}><w:p><w:t>{}</w:t></w:p></w:document>",
            "b".repeat(70_000)
        );
        assert_eq!(
            extract(&archive(&[("word/document.xml", &long)]), "docx").unwrap_err(),
            "This document exceeds 60,000 text characters."
        );

        // Not an archive, nothing readable, unknown type, broken XML.
        assert!(extract(b"not a zip", "docx")
            .unwrap_err()
            .contains("could not be read"));
        assert_eq!(
            extract(&archive(&[("other.xml", "<a/>")]), "docx").unwrap_err(),
            "No readable text was found in this document."
        );
        assert!(extract(b"x", "exe").is_err());
        let broken = archive(&[("word/document.xml", "<w:document><w:p></w:document>")]);
        assert!(extract(&broken, "docx")
            .unwrap_err()
            .contains("invalid XML"));
    }

    #[test]
    fn natural_order_compares_numbers_by_value() {
        assert_eq!(natural("slide2.xml", "slide10.xml"), Ordering::Less);
        assert_eq!(natural("a", "a"), Ordering::Equal);
        assert_eq!(natural("sheet01.xml", "sheet1.xml"), Ordering::Equal);
        assert_eq!(natural("b1", "a2"), Ordering::Greater);
    }
}
