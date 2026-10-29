//! Ported from `UI/Shared/{Localization,LocalizationCatalog,Accessibility}.cs`.
//!
//! English source captions are the catalog keys. Call [`t`] only for application-owned UI text,
//! never for transcripts, user labels, file names, provider ids or persisted values.
//! A missing key or a blank translation falls back to the English caption.

mod accessibility;
mod catalog;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::fmt::Display;
use std::sync::OnceLock;

pub use accessibility::{scaled_font_size, Accessibility, MAX_TEXT_SCALE, MIN_TEXT_SCALE};
pub use catalog::{Entry, ENTRIES};

/// A selectable interface language.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Language {
    pub code: &'static str,
    /// The language's own name, as shown in the language picker.
    pub name: &'static str,
}

const LANGUAGES: [Language; 3] = [
    Language {
        code: "en",
        name: "English",
    },
    Language {
        code: "es",
        name: "Español",
    },
    Language {
        code: "id",
        name: "Bahasa Indonesia",
    },
];

/// The supported interface languages (`en`, `es`, `id`), in picker order.
pub fn languages() -> &'static [Language] {
    &LANGUAGES
}

/// The language code used when `code` is not one of [`languages`] (C# `NormalizeLanguageAccessibility`).
pub fn normalize_language(code: &str) -> &'static str {
    LANGUAGES
        .iter()
        .find(|l| l.code == code)
        .map_or("en", |l| l.code)
}

fn index() -> &'static HashMap<&'static str, usize> {
    static INDEX: OnceLock<HashMap<&'static str, usize>> = OnceLock::new();
    INDEX.get_or_init(|| ENTRIES.iter().enumerate().map(|(i, e)| (e.0, i)).collect())
}

/// The catalog row for an English caption.
pub fn entry(english: &str) -> Option<&'static Entry> {
    index().get(english).map(|&i| &ENTRIES[i])
}

/// Translates an English caption into `lang` (`"es"`, `"id"`; anything else is English).
/// Unknown captions and blank translations return the English text unchanged.
pub fn t<'a>(lang: &str, key: &'a str) -> &'a str {
    let Some(e) = entry(key) else { return key };
    let translated = match lang {
        "es" => e.1,
        "id" => e.2,
        _ => return key,
    };
    if translated.is_empty() {
        key
    } else {
        translated
    }
}

/// C# `Localization.Format`: translates `template` then substitutes `{0}`, `{1}`, ... with `args`.
/// Placeholders without an argument are kept verbatim; `{{` and `}}` are literal braces.
pub fn format(lang: &str, template: &str, args: &[&dyn Display]) -> String {
    let text = t(lang, template);
    let mut out = String::with_capacity(text.len() + 8);
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        match c {
            '{' if matches!(chars.peek(), Some((_, '{'))) => {
                chars.next();
                out.push('{');
            }
            '}' if matches!(chars.peek(), Some((_, '}'))) => {
                chars.next();
                out.push('}');
            }
            '{' => {
                let rest = &text[i + 1..];
                let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
                let arg = (digits > 0 && rest[digits..].starts_with('}'))
                    .then(|| rest[..digits].parse::<usize>().ok())
                    .flatten()
                    .and_then(|n| args.get(n));
                match arg {
                    Some(arg) => {
                        out.push_str(&arg.to_string());
                        for _ in 0..=digits {
                            chars.next();
                        }
                    }
                    None => out.push('{'),
                }
            }
            _ => out.push(c),
        }
    }
    out
}
