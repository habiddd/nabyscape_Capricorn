//! Note text bounds (C# `ProductivityService.Bound`).

/// Limit of the scratch note, in UTF-16 units like the C# `string.Length`.
pub const MAX_NOTE: usize = 50_000;

/// Truncates to at most `limit` UTF-16 units without splitting a character.
pub fn bound(text: &str, limit: usize) -> String {
    let mut units = 0;
    let mut end = text.len();
    for (index, ch) in text.char_indices() {
        units += ch.len_utf16();
        if units > limit {
            end = index;
            break;
        }
    }
    text[..end].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bound_truncates_by_utf16_units() {
        assert_eq!(bound(&"n".repeat(51_000), MAX_NOTE).len(), 50_000);
        assert_eq!(bound("abc", 5), "abc");
        // A surrogate pair counts as two units and is never split.
        assert_eq!(bound("a😀b", 2), "a");
        assert_eq!(bound("a😀b", 3), "a😀");
    }
}
