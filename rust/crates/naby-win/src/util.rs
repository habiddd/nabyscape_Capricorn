//! Small string helpers shared by the Win32 wrappers (pure, tested on every platform).

/// UTF-16 with a trailing NUL, ready for `PCWSTR`.
pub fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Text up to the first NUL of a fixed-size UTF-16 buffer.
pub fn from_wide(buffer: &[u16]) -> String {
    let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end])
}

/// Copies `text` into a fixed UTF-16 field (tray tip, balloon title), always NUL-terminated,
/// never splitting a surrogate pair. Returns the number of code units written (without NUL).
pub fn copy_into_fixed(dst: &mut [u16], text: &str) -> usize {
    if dst.is_empty() {
        return 0;
    }
    let capacity = dst.len() - 1;
    let mut written = 0;
    let mut pair = [0u16; 2];
    for ch in text.chars() {
        let units = ch.encode_utf16(&mut pair);
        if written + units.len() > capacity {
            break;
        }
        dst[written..written + units.len()].copy_from_slice(units);
        written += units.len();
    }
    dst[written..].fill(0);
    written
}

/// Replaces `%NAME%` references using `lookup`; unknown names are left as written, like
/// `Environment.ExpandEnvironmentVariables`.
pub fn expand_env(text: &str, lookup: &dyn Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('%') {
            Some(end) if end > 0 => {
                let name = &after[..end];
                match lookup(name) {
                    Some(value) => out.push_str(&value),
                    None => {
                        out.push('%');
                        out.push_str(name);
                        out.push('%');
                    }
                }
                rest = &after[end + 1..];
            }
            _ => {
                out.push('%');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_round_trips_and_stops_at_nul() {
        let w = wide("Naby\u{1F600}");
        assert_eq!(*w.last().unwrap(), 0);
        assert_eq!(from_wide(&w), "Naby\u{1F600}");
        assert_eq!(from_wide(&[b'a' as u16, 0, b'b' as u16]), "a");
    }

    #[test]
    fn fixed_fields_truncate_without_splitting_surrogates() {
        let mut field = [7u16; 6];
        let n = copy_into_fixed(&mut field, "ab\u{1F600}\u{1F600}");
        assert_eq!(n, 4);
        assert_eq!(from_wide(&field), "ab\u{1F600}");
        assert_eq!(field[4], 0);
        let mut tiny = [9u16; 2];
        assert_eq!(copy_into_fixed(&mut tiny, "\u{1F600}"), 0);
        assert_eq!(tiny, [0, 0]);
        assert_eq!(copy_into_fixed(&mut [], "x"), 0);
    }

    #[test]
    fn expands_known_variables_only() {
        let env = |name: &str| (name == "APPDATA").then(|| r"C:\Users\n\AppData".to_string());
        assert_eq!(expand_env(r"%APPDATA%\x", &env), r"C:\Users\n\AppData\x");
        assert_eq!(expand_env("%NOPE%/a", &env), "%NOPE%/a");
        assert_eq!(expand_env("100% sure", &env), "100% sure");
        assert_eq!(expand_env("%%", &env), "%%");
    }
}
