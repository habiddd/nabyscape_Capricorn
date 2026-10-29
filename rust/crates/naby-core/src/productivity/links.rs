//! Tracker stripping for copied links (C# `CleanLink`).

use url::Url;

const TRACKING_KEYS: [&str; 9] = [
    "fbclid", "gclid", "dclid", "msclkid", "mc_cid", "mc_eid", "igshid", "_ga", "_gl",
];
const SIGNATURE_KEYS: [&str; 6] = [
    "x-amz-signature",
    "x-goog-signature",
    "signature",
    "sig",
    "oauth_signature",
    "hmac",
];
const MAX_LINK: usize = 8192;
/// Meeting links stored on calendar events.
pub const MAX_MEETING_URL: usize = 2048;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkError(pub String);

impl std::fmt::Display for LinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for LinkError {}

fn hex(b: u8) -> Option<u8> {
    (b as char).to_digit(16).map(|d| d as u8)
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Printable ASCII only, and every `%` introduces two hex digits.
fn well_formed(value: &str) -> bool {
    let bytes = value.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b <= 0x20
            || b >= 0x7f
            || matches!(
                b,
                b'\\' | b'"' | b'<' | b'>' | b'^' | b'`' | b'{' | b'|' | b'}'
            )
        {
            return false;
        }
        if b == b'%' {
            let escaped = bytes.get(i + 1).copied().and_then(hex).is_some()
                && bytes.get(i + 2).copied().and_then(hex).is_some();
            if !escaped {
                return false;
            }
            i += 3;
            continue;
        }
        i += 1;
    }
    true
}

/// Trims and validates an absolute HTTP(S) URL without credentials (C# `ValidateUrl`).
pub fn validate_url(input: &str, maximum_length: usize) -> Result<String, LinkError> {
    let value = input.trim();
    let error = || {
        LinkError(format!(
            "Enter a complete HTTP or HTTPS URL without credentials, up to {maximum_length} characters."
        ))
    };
    if value.is_empty() || value.len() > maximum_length || !well_formed(value) {
        return Err(error());
    }
    let url = Url::parse(value).map_err(|_| error())?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none_or(str::is_empty)
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(error());
    }
    Ok(value.to_string())
}

/// Removes `utm_*` and common click-id parameters. URLs carrying a signature parameter are
/// returned untouched, as are paths, escaping, parameter order and fragments.
pub fn try_clean_link(input: &str) -> Result<String, LinkError> {
    let value = validate_url(input, MAX_LINK)?;
    let fragment = value.find('#');
    let end = fragment.unwrap_or(value.len());
    let query = match value.find('?') {
        Some(q) if q <= end => q,
        _ => return Ok(value),
    };
    let parameters: Vec<&str> = value[query + 1..end].split('&').collect();
    let keys: Vec<String> = parameters
        .iter()
        .map(|part| percent_decode(part.split('=').next().unwrap_or("")).to_lowercase())
        .collect();
    if keys.iter().any(|k| SIGNATURE_KEYS.contains(&k.as_str())) {
        return Ok(value);
    }
    let remaining: Vec<&str> = parameters
        .iter()
        .zip(&keys)
        .filter(|(_, k)| !k.starts_with("utm_") && !TRACKING_KEYS.contains(&k.as_str()))
        .map(|(p, _)| *p)
        .collect();
    if remaining.len() == parameters.len() {
        return Ok(value);
    }
    let mut out = value[..query].to_string();
    if !remaining.is_empty() {
        out.push('?');
        out.push_str(&remaining.join("&"));
    }
    if let Some(f) = fragment {
        out.push_str(&value[f..]);
    }
    Ok(out)
}

/// `None` when the text is not a valid link.
pub fn clean_link(input: &str) -> Option<String> {
    try_clean_link(input).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_link_strips_trackers() {
        assert_eq!(
            clean_link("https://example.com/a?q=keep&utm_source=test&fbclid=1&gclid=2#anchor")
                .unwrap(),
            "https://example.com/a?q=keep#anchor"
        );
        assert_eq!(
            clean_link("  https://EXAMPLE.com/a%2fb/../c?q=a%2Bb&q=two+words&utm_SOURCE=ad&empty=&%66bclid=x#part%2F?utm_x=keep  ").unwrap(),
            "https://EXAMPLE.com/a%2fb/../c?q=a%2Bb&q=two+words&empty=#part%2F?utm_x=keep"
        );
        assert_eq!(
            clean_link("https://example.com/?UTM_a=1&GCLID=2#hash").unwrap(),
            "https://example.com/#hash"
        );
        assert_eq!(
            clean_link("https://example.com/#?utm_a=keep").unwrap(),
            "https://example.com/#?utm_a=keep"
        );
        assert_eq!(
            clean_link("https://example.com/?a=1&a=2").unwrap(),
            "https://example.com/?a=1&a=2"
        );
        assert_eq!(
            clean_link(
                "https://example.com/?dclid=x&msclkid=x&mc_cid=x&mc_eid=x&igshid=x&_ga=x&_gl=x"
            )
            .unwrap(),
            "https://example.com/"
        );
        assert_eq!(
            clean_link("HTTPS://example.com/?uTm_X=y").unwrap(),
            "HTTPS://example.com/"
        );
    }

    #[test]
    fn signed_links_remain_unchanged() {
        for key in [
            "X-Amz-Signature",
            "x-goog-signature",
            "Signature",
            "%73ig",
            "oauth_signature",
            "hmac",
        ] {
            let url = format!("https://example.com/p%2f?utm_source=x&{key}=a%2Bb#f");
            assert_eq!(clean_link(&format!(" {url} ")).unwrap(), url);
        }
    }

    #[test]
    fn invalid_links_are_rejected() {
        let long = format!("https://example.com/{}", "a".repeat(8192));
        for value in [
            "",
            "abc",
            "ftp://example.com",
            "javascript:alert(1)",
            "https://user:pass@example.com/",
            "https://user@example.com",
            "https://example.com/%zz",
            "https://example.com/a b",
            "https://example.com/%4",
            "https://example.com/\nsecret",
            "file:///C:/private.txt",
            "example.com",
            &long,
        ] {
            assert!(
                try_clean_link(value).is_err(),
                "{value:?} should be rejected"
            );
        }
    }
}
