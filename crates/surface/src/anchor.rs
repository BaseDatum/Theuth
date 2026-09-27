//! The anchor grammar (design doc 71 §2.3) — what a `code:` link, a `GraphNode.ref`, a
//! `Sequence` step `ref` and an `Excerpt` path have in common:
//!
//! ```text
//! Anchor      := CodeAnchor | BlockAnchor
//! CodeAnchor  := <path>#L<a>-L<b> | <path>#L<a> | <path>     # root-relative, 1-based, inclusive
//! BlockAnchor := block:<blockId>
//! ```
//!
//! A path is root-relative: no leading `/`, no `.`/`..` segment, no empty segment, no `#`, no `:`
//! (so `path:12` is refused with the `#L12` form named, and no scheme — `http:`, `file:`, `C:` — gets
//! through). The rule keeps a `ref` from aiming outside the board's root
//! (65 §0.6 as amended by 71 §2.3).

/// One parsed anchor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Anchor {
    /// A file, or a 1-based inclusive line range inside it (`#L<a>` is `(a, a)`).
    Code {
        /// Root-relative path.
        path: String,
        /// `(first, last)` line, inclusive; `None` for the whole file.
        lines: Option<(u32, u32)>,
    },
    /// A block of the same board.
    Block {
        /// The block id.
        id: String,
    },
}

/// Parse one anchor string; `Err` is the message a located error carries.
pub fn parse(s: &str) -> Result<Anchor, String> {
    if let Some(id) = s.strip_prefix("block:") {
        return if id.is_empty() || id.chars().any(char::is_whitespace) {
            Err("block:<id> needs a non-blank id".into())
        } else {
            Ok(Anchor::Block { id: id.to_owned() })
        };
    }
    let (path, fragment) = match s.split_once('#') {
        Some((p, f)) => (p, Some(f)),
        None => (s, None),
    };
    check_path(path)?;
    let lines = match fragment {
        None => None,
        Some(f) => Some(parse_lines(f)?),
    };
    Ok(Anchor::Code {
        path: path.to_owned(),
        lines,
    })
}

/// `L<a>` or `L<a>-L<b>`, `1 ≤ a ≤ b`.
fn parse_lines(f: &str) -> Result<(u32, u32), String> {
    let bad = || format!("expected #L<a> or #L<a>-L<b>, got #{f}");
    let line = |t: &str| -> Result<u32, String> {
        t.strip_prefix('L')
            .and_then(|n| n.parse::<u32>().ok())
            .filter(|n| *n >= 1)
            .ok_or_else(bad)
    };
    let (a, b) = match f.split_once('-') {
        Some((a, b)) => (line(a)?, line(b)?),
        None => {
            let a = line(f)?;
            (a, a)
        }
    };
    if b < a {
        return Err(format!("L{b} is before L{a}"));
    }
    Ok((a, b))
}

/// A root-relative path: what `Excerpt.path` and the path half of a `CodeAnchor` must be.
pub fn check_path(path: &str) -> Result<(), String> {
    if path.is_empty() {
        return Err("empty path".into());
    }
    if path.starts_with('/') || path.starts_with('\\') {
        return Err(format!(
            "path must be root-relative, not absolute: {path:?}"
        ));
    }
    if let Some((scheme, _)) = path.split_once(':')
        && !scheme.is_empty()
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
        && scheme
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic())
    {
        return Err(format!("path must not carry a scheme: {scheme}:"));
    }
    if path.contains(':') {
        return Err(format!(
            "path must not contain ':'; a line is #L<n>, not :<n>: {path:?}"
        ));
    }
    if path.contains('#') {
        return Err("path must not contain '#'; the line range follows a single '#'".into());
    }
    if path.chars().any(|c| c.is_whitespace() && c != ' ') {
        return Err("path must not contain line breaks or tabs".into());
    }
    for seg in path.split('/') {
        match seg {
            "" => return Err(format!("empty path segment in {path:?}")),
            "." | ".." => return Err(format!("path must not contain {seg:?}: {path:?}")),
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(path: &str, lines: Option<(u32, u32)>) -> Anchor {
        Anchor::Code {
            path: path.into(),
            lines,
        }
    }

    #[test]
    fn grammar_accepts_the_three_code_forms_and_block() {
        assert_eq!(
            parse("crates/surface/src/lib.rs#L71-L84"),
            Ok(code("crates/surface/src/lib.rs", Some((71, 84))))
        );
        assert_eq!(parse("a/b.rs#L7"), Ok(code("a/b.rs", Some((7, 7)))));
        assert_eq!(parse("a/b.rs#L7-L7"), Ok(code("a/b.rs", Some((7, 7)))));
        assert_eq!(parse("README.md"), Ok(code("README.md", None)));
        assert_eq!(parse("with space/x.rs"), Ok(code("with space/x.rs", None)));
        assert_eq!(parse("block:b12"), Ok(Anchor::Block { id: "b12".into() }));
    }

    #[test]
    fn grammar_rejects_escapes_schemes_and_bad_ranges() {
        let bad = [
            "",
            "/etc/passwd",
            "../x",
            "a/../x",
            "./x",
            "a//b",
            "http://example.com/x.rs",
            "http:x",
            "file:///x",
            "C:\\x",
            "a\tb",
            "a/b.rs#L0",
            "a/b.rs#L10-L9",
            "a/b.rs#70",
            "a/b.rs#L1-",
            "a/b.rs#L1-L2-L3",
            "crates/rpc/src/api.rs:1",
            "block:",
            "block:a b",
        ];
        for s in bad {
            assert!(parse(s).is_err(), "{s:?} should not parse");
        }
        assert_eq!(
            parse("../x").unwrap_err(),
            "path must not contain \"..\": \"../x\""
        );
        assert_eq!(
            parse("http://x").unwrap_err(),
            "path must not carry a scheme: http:"
        );
        assert_eq!(parse("a#L9-L3").unwrap_err(), "L3 is before L9");
        assert_eq!(
            parse("a/b.rs:12").unwrap_err(),
            "path must not contain ':'; a line is #L<n>, not :<n>: \"a/b.rs:12\""
        );
        assert!(check_path("a#b.rs").is_err());
    }
}
