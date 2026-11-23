//! Char- and byte-boundary-safe truncation, once. Every helper returns a borrow when
//! nothing was cut. The ellipsis is U+2026 (`…`). Callers that need a different suffix,
//! a word boundary or display width keep their own.

use std::borrow::Cow;

/// The ellipsis the helpers append.
const ELLIPSIS: char = '\u{2026}';

/// The first `n` chars of `s` (all of `s` when it is shorter).
#[must_use]
pub fn head_chars(s: &str, n: usize) -> &str {
    s.char_indices().nth(n).map_or(s, |(i, _)| &s[..i])
}

/// The longest prefix of `s` within `max` bytes, cut on a char boundary.
#[must_use]
pub fn cut_bytes(s: &str, max: usize) -> &str {
    &s[..s.floor_char_boundary(max)]
}

/// `s` itself when it has at most `max` chars; else its first `max - 1` chars plus `…`, so
/// the result never exceeds `max` chars. `max == 0` yields the empty string.
#[must_use]
pub fn ellipsize(s: &str, max: usize) -> Cow<'_, str> {
    if s.chars().nth(max).is_none() {
        return Cow::Borrowed(s);
    }
    let Some(keep) = max.checked_sub(1) else {
        return Cow::Borrowed("");
    };
    let mut out = String::from(head_chars(s, keep));
    out.push(ELLIPSIS);
    Cow::Owned(out)
}

/// `s` itself when it has at most `max` chars; else its first `max` chars plus `…`
/// (`max + 1` chars in all).
#[must_use]
pub fn clip_then_ellipsis(s: &str, max: usize) -> Cow<'_, str> {
    let head = head_chars(s, max);
    if head.len() == s.len() {
        return Cow::Borrowed(s);
    }
    let mut out = String::from(head);
    out.push(ELLIPSIS);
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn head_chars_counts_chars_not_bytes() {
        assert_eq!(head_chars("héllo", 2), "hé");
        assert_eq!(head_chars("héllo", 5), "héllo");
        assert_eq!(head_chars("héllo", 99), "héllo");
        assert_eq!(head_chars("héllo", 0), "");
    }

    #[test]
    fn cut_bytes_never_splits_a_char() {
        assert_eq!(cut_bytes("héllo", 2), "h");
        assert_eq!(cut_bytes("héllo", 3), "hé");
        assert_eq!(cut_bytes("héllo", 99), "héllo");
        assert_eq!(cut_bytes("héllo", 0), "");
    }

    #[test]
    fn ellipsize_fits_within_max_chars() {
        assert!(matches!(ellipsize("héllo", 5), Cow::Borrowed("héllo")));
        assert_eq!(ellipsize("héllo wörld", 5), "héll…");
        assert_eq!(ellipsize("héllo", 1), "…");
        assert_eq!(ellipsize("héllo", 0), "");
        assert_eq!(ellipsize("", 0), "");
    }

    #[test]
    fn clip_then_ellipsis_keeps_max_chars_then_appends() {
        assert!(matches!(
            clip_then_ellipsis("héllo", 5),
            Cow::Borrowed("héllo")
        ));
        assert_eq!(clip_then_ellipsis("héllo wörld", 5), "héllo…");
        assert_eq!(clip_then_ellipsis("héllo", 0), "…");
        assert_eq!(clip_then_ellipsis("", 0), "");
    }
}
