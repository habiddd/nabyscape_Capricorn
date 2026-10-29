//! Chat Markdown parsed into blocks and styled spans (`AssistantMarkdown.cs`).
//!
//! The renderer paints these natively: no HTML or script is ever interpreted, and media is
//! extracted by the transport and shown separately.

use url::Url;

/// A reply longer than this is shortened in the transcript (the full text stays copyable).
pub const MAXIMUM_CHARACTERS: usize = 24000;
/// At most this many source lines are laid out.
pub const MAXIMUM_LINES: usize = 600;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Span {
    Text(String),
    Code(String),
    Bold(String),
    Italic(String),
    Link { text: String, url: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Bullet {
    /// `- item`, `* item`, `+ item`.
    Dot,
    /// `1. item` (the number as written).
    Number(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    /// An empty source line (a small vertical gap).
    Spacer,
    /// `---`, `***`, `___`.
    Rule,
    /// A fenced code block (without the fences, trailing whitespace removed).
    Code(String),
    /// A heading, list item, quote or paragraph line.
    Line {
        /// 1..=6 for headings, 0 for body text.
        heading: u8,
        quote: bool,
        bullet: Option<Bullet>,
        spans: Vec<Span>,
    },
}

fn is_rule(line: &str) -> bool {
    let t = line.trim();
    t.len() >= 3 && t.chars().all(|c| matches!(c, '-' | '*' | '_'))
}

fn bullet(line: &str) -> Option<(Bullet, &str)> {
    let t = line.trim_start();
    let mut chars = t.chars();
    let first = chars.next()?;
    if matches!(first, '-' | '*' | '+') && t[1..].starts_with(' ') {
        return Some((Bullet::Dot, &t[2..]));
    }
    let digits = t.bytes().take_while(u8::is_ascii_digit).count();
    if digits > 0 && t[digits..].starts_with(". ") {
        return Some((Bullet::Number(t[..digits].to_string()), &t[digits + 2..]));
    }
    None
}

/// Parse a reply into blocks.
pub fn parse(source: &str) -> Vec<Block> {
    let text = if source.chars().count() > MAXIMUM_CHARACTERS {
        let head: String = source.chars().take(MAXIMUM_CHARACTERS).collect();
        format!("{head}\n\n[Long reply shortened here. Use Copy message for the full text.]")
    } else {
        source.to_string()
    };
    let mut blocks = Vec::new();
    let mut fenced = false;
    let mut code = String::new();
    let flush = |blocks: &mut Vec<Block>, code: &mut String| {
        blocks.push(Block::Code(code.trim_end().to_string()));
        code.clear();
    };
    let cleaned = text.replace('\r', "");
    for raw in cleaned.split('\n').take(MAXIMUM_LINES) {
        let mut line = raw.trim_end();
        if line.trim_start().starts_with("```") {
            if fenced {
                flush(&mut blocks, &mut code);
            }
            fenced = !fenced;
            continue;
        }
        if fenced {
            code.push_str(line);
            code.push('\n');
            continue;
        }
        if line.is_empty() {
            blocks.push(Block::Spacer);
            continue;
        }
        if is_rule(line) {
            blocks.push(Block::Rule);
            continue;
        }
        let hashes = line.bytes().take_while(|&b| b == b'#').count();
        let mut heading = 0u8;
        if (1..=6).contains(&hashes) && line.len() > hashes && line.as_bytes()[hashes] == b' ' {
            heading = hashes as u8;
            line = &line[hashes + 1..];
        }
        let mut quote = false;
        if line.trim_start().starts_with('>') {
            quote = true;
            line = line.trim_start()[1..].trim_start();
        }
        let (bullet, body) = match bullet(line) {
            Some((b, rest)) => (Some(b), rest),
            None => (None, line),
        };
        blocks.push(Block::Line {
            heading,
            quote,
            bullet,
            spans: spans(body),
        });
    }
    if fenced && !code.is_empty() {
        flush(&mut blocks, &mut code);
    }
    blocks
}

/// Length of the run of characters before the first one matching `stop`; `None` when the run is
/// empty or the text ends first (the closing delimiter would be missing).
fn run(text: &str, stop: impl Fn(char) -> bool) -> Option<usize> {
    for (i, c) in text.char_indices() {
        if stop(c) {
            return (i > 0).then_some(i);
        }
    }
    None
}

/// One inline match at the start of `text`, as `(length, span)`.
fn inline_at(text: &str) -> Option<(usize, Span)> {
    if let Some(body) = text.strip_prefix('`') {
        let end = run(body, |c| c == '`' || c == '\n')?;
        return body[end..]
            .starts_with('`')
            .then(|| (end + 2, Span::Code(body[..end].to_string())));
    }
    for delim in ["**", "__"] {
        if let Some(body) = text.strip_prefix(delim) {
            let stop = delim.chars().next().unwrap_or('*');
            if let Some(end) = run(body, |c| c == stop || c == '\n') {
                if body[end..].starts_with(delim) {
                    return Some((end + 4, Span::Bold(body[..end].to_string())));
                }
            }
        }
    }
    if let Some(body) = text.strip_prefix('*') {
        let end = run(body, |c| c == '*' || c == '\n')?;
        return body[end..]
            .starts_with('*')
            .then(|| (end + 2, Span::Italic(body[..end].to_string())));
    }
    if let Some(body) = text.strip_prefix('[') {
        let end = run(body, |c| c == ']' || c == '\n')?;
        let after = body[end..].strip_prefix("](")?;
        let scheme = if after.starts_with("https://") {
            8
        } else if after.starts_with("http://") {
            7
        } else {
            return None;
        };
        let url_len = after
            .char_indices()
            .find(|(_, c)| c.is_whitespace() || *c == ')')
            .map(|(i, _)| i)?;
        if url_len <= scheme || !after[url_len..].starts_with(')') {
            return None;
        }
        return Some((
            1 + end + 2 + url_len + 1,
            Span::Link {
                text: body[..end].to_string(),
                url: after[..url_len].to_string(),
            },
        ));
    }
    None
}

/// Split a line into styled spans.
pub fn spans(text: &str) -> Vec<Span> {
    let mut out = Vec::new();
    let mut plain = String::new();
    let mut i = 0;
    while i < text.len() {
        let c = text[i..].chars().next().unwrap_or(' ');
        if matches!(c, '`' | '*' | '_' | '[') {
            if let Some((len, span)) = inline_at(&text[i..]) {
                if !plain.is_empty() {
                    out.push(Span::Text(std::mem::take(&mut plain)));
                }
                out.push(span);
                i += len;
                continue;
            }
        }
        plain.push(c);
        i += c.len_utf8();
    }
    if !plain.is_empty() {
        out.push(Span::Text(plain));
    }
    out
}

/// Whether a link may be opened: absolute `http(s)` without credentials.
pub fn is_safe_link(url: &str) -> bool {
    Url::parse(url).is_ok_and(|u| {
        matches!(u.scheme(), "http" | "https") && u.username().is_empty() && u.password().is_none()
    })
}

/// Text suitable for speech: fenced code dropped and Markdown punctuation removed.
pub fn spoken(text: &str) -> String {
    let text: String = text.chars().take(12000).collect();
    let mut out = String::with_capacity(text.len());
    let mut rest = text.as_str();
    while let Some(start) = rest.find("```") {
        match rest[start + 3..].find("```") {
            Some(len) => {
                out.push_str(&rest[..start]);
                out.push(' ');
                rest = &rest[start + 3 + len + 3..];
            }
            None => break,
        }
    }
    out.push_str(rest);
    out.retain(|c| !matches!(c, '#' | '*' | '`' | '_' | '>'));
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(blocks: &[Block], index: usize) -> (&u8, &bool, &Option<Bullet>, &Vec<Span>) {
        match &blocks[index] {
            Block::Line {
                heading,
                quote,
                bullet,
                spans,
            } => (heading, quote, bullet, spans),
            other => panic!("not a line: {other:?}"),
        }
    }

    #[test]
    fn markdown_code_block_and_lists() {
        let blocks = parse(
            "# Title\n\nSome **bold**, *italic* and `code` plus [site](https://example.com/a).\n- one\n* two\n+ three\n1. first\n12. twelfth\n\n```rust\nfn main() {}\n  indented  \n```\n> quoted\n---\n",
        );
        let (heading, ..) = line(&blocks, 0);
        assert_eq!(*heading, 1);
        assert_eq!(blocks[1], Block::Spacer);
        let (_, _, _, spans) = line(&blocks, 2);
        assert_eq!(
            spans,
            &vec![
                Span::Text("Some ".into()),
                Span::Bold("bold".into()),
                Span::Text(", ".into()),
                Span::Italic("italic".into()),
                Span::Text(" and ".into()),
                Span::Code("code".into()),
                Span::Text(" plus ".into()),
                Span::Link {
                    text: "site".into(),
                    url: "https://example.com/a".into()
                },
                Span::Text(".".into()),
            ]
        );
        for (index, expected) in [
            (3, Bullet::Dot),
            (4, Bullet::Dot),
            (5, Bullet::Dot),
            (6, Bullet::Number("1".into())),
            (7, Bullet::Number("12".into())),
        ] {
            assert_eq!(line(&blocks, index).2.as_ref(), Some(&expected), "{index}");
        }
        assert_eq!(line(&blocks, 3).3, &vec![Span::Text("one".into())]);
        assert_eq!(blocks[8], Block::Spacer);
        assert_eq!(blocks[9], Block::Code("fn main() {}\n  indented".into()));
        let (_, quote, ..) = line(&blocks, 10);
        assert!(*quote);
        assert_eq!(blocks[11], Block::Rule);
        assert_eq!(blocks.len(), 13, "trailing empty line is a spacer");
    }

    #[test]
    fn unterminated_fence_is_still_shown_and_headings_need_a_space() {
        let blocks = parse("```\nlet a = 1;");
        assert_eq!(blocks, vec![Block::Code("let a = 1;".into())]);
        let blocks = parse("#nospace\n####### seven\n### three");
        assert_eq!(*line(&blocks, 0).0, 0);
        assert_eq!(*line(&blocks, 1).0, 0);
        assert_eq!(*line(&blocks, 2).0, 3);
        assert_eq!(parse("```\n```"), vec![Block::Code(String::new())]);
    }

    #[test]
    fn inline_rules_match_the_original_expressions() {
        assert_eq!(
            spans("**a*b**"),
            vec![
                Span::Text("*".into()),
                Span::Italic("a".into()),
                Span::Text("b**".into())
            ]
        );
        assert_eq!(spans("__under__"), vec![Span::Bold("under".into())]);
        assert_eq!(spans("a_b_c"), vec![Span::Text("a_b_c".into())]);
        assert_eq!(spans("`unclosed"), vec![Span::Text("`unclosed".into())]);
        assert_eq!(
            spans("[x](ftp://a)"),
            vec![Span::Text("[x](ftp://a)".into())]
        );
        assert_eq!(spans("**"), vec![Span::Text("**".into())]);
        assert_eq!(
            spans("héllo **wörld**"),
            vec![Span::Text("héllo ".into()), Span::Bold("wörld".into())]
        );
    }

    #[test]
    fn long_replies_are_shortened_and_line_count_is_capped() {
        let long = "x".repeat(30000);
        let blocks = parse(&long);
        let Block::Line { spans, .. } = &blocks[0] else {
            panic!()
        };
        assert_eq!(spans, &vec![Span::Text("x".repeat(24000))]);
        assert!(
            matches!(&blocks[2], Block::Line { spans, .. } if matches!(&spans[0], Span::Text(t) if t.starts_with("[Long reply shortened")))
        );
        let many = "a\n".repeat(1000);
        assert_eq!(parse(&many).len(), 600);
    }

    #[test]
    fn links_open_only_when_safe() {
        assert!(is_safe_link("https://example.com/x"));
        assert!(is_safe_link("http://example.com"));
        assert!(!is_safe_link("javascript:alert(1)"));
        assert!(!is_safe_link("file:///c:/windows"));
        assert!(!is_safe_link("https://user:pw@example.com/"));
        assert!(!is_safe_link("not a url"));
    }

    #[test]
    fn spoken_text_drops_code_and_markup() {
        assert_eq!(
            spoken("# Hi **there**\n```\ncode\n```\nbye `x` > _y_"),
            "Hi there\n \nbye x  y"
        );
        assert_eq!(spoken("```unterminated"), "unterminated");
        assert_eq!(spoken("  plain  "), "plain");
        assert_eq!(spoken(&"a".repeat(20000)).len(), 12000);
    }
}
