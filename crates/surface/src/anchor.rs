//! The anchor grammar (design doc 71 §2.3) — what a `code:` link, a `GraphNode.ref`, a
//! `Sequence` step `ref` and an `Excerpt` path have in common:
//!
//! ```text
//! Anchor       := CodeAnchor | BlockAnchor | TheuthAnchor
//! CodeAnchor   := <path>#L<a>-L<b> | <path>#L<a> | <path>     # root-relative, 1-based, inclusive
//! BlockAnchor  := block:<blockId>
//! TheuthAnchor := theuth://<kind>/<id><locator>?<query>?      # 144 §6.1: a citation
//!   kind       := notes | sources | knowledge | meetings | sessions
//!   locator    := #<blockId>            notes, sources
//!               | @mm:ss | @h:mm:ss     meetings; sources with a duration
//!               | #<turn>               sessions (1-based)
//!               | (none)                knowledge (`@<commit>` after the id is part of the id)
//!   query      := s=<sha8>              optional drift hash: first 8 hex of sha256(block text)
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
    /// A `theuth://` citation (144 §6.1).
    Theuth(TheuthAnchor),
}

/// The target kind of a [`TheuthAnchor`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TheuthKind {
    /// `theuth://notes/<id>[#<block>]`.
    Notes,
    /// `theuth://sources/<id>[#<block> | @mm:ss]`.
    Sources,
    /// `theuth://knowledge/<id>[@<commit>]`.
    Knowledge,
    /// `theuth://meetings/<id>[@mm:ss]`.
    Meetings,
    /// `theuth://sessions/<id>[#<turn>]`.
    Sessions,
}

impl TheuthKind {
    /// Every kind, grammar order.
    pub const ALL: [TheuthKind; 5] = [
        TheuthKind::Notes,
        TheuthKind::Sources,
        TheuthKind::Knowledge,
        TheuthKind::Meetings,
        TheuthKind::Sessions,
    ];

    /// The path segment.
    pub fn as_str(self) -> &'static str {
        match self {
            TheuthKind::Notes => "notes",
            TheuthKind::Sources => "sources",
            TheuthKind::Knowledge => "knowledge",
            TheuthKind::Meetings => "meetings",
            TheuthKind::Sessions => "sessions",
        }
    }

    /// Inverse of [`Self::as_str`].
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == s)
    }
}

/// Where inside the target a [`TheuthAnchor`] points.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Locator {
    /// `#<blockId>` (notes, sources).
    Block(String),
    /// `@mm:ss` / `@h:mm:ss` as milliseconds (meetings; sources with a duration).
    Time(u64),
    /// `#<turn>`, 1-based (sessions).
    Turn(u32),
}

/// One parsed `theuth://` citation.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TheuthAnchor {
    /// Target kind.
    pub kind: TheuthKind,
    /// Target id, verbatim (a knowledge id keeps its `/` segments and any `@<commit>`).
    pub id: String,
    /// The locator, when given.
    pub locator: Option<Locator>,
    /// `?s=<sha8>`: 8 lowercase hex, when given.
    pub sha8: Option<String>,
}

impl Locator {
    /// The locator as written: `#<block>`, `@mm:ss` / `@h:mm:ss`, `#<turn>`.
    pub fn text(&self) -> String {
        match self {
            Locator::Block(b) => format!("#{b}"),
            Locator::Turn(t) => format!("#{t}"),
            Locator::Time(ms) => {
                let secs = ms / 1000;
                let (h, m, sec) = (secs / 3600, (secs % 3600) / 60, secs % 60);
                if h > 0 {
                    format!("@{h}:{m:02}:{sec:02}")
                } else {
                    format!("@{m:02}:{sec:02}")
                }
            }
        }
    }
}

/// The scheme every citation starts with.
pub const THEUTH_SCHEME: &str = "theuth://";

impl TheuthAnchor {
    /// Parse `theuth://<kind>/<id><locator>?<query>?`; `Err` is the message a located error carries.
    pub fn parse(href: &str) -> Result<Self, String> {
        let Some(rest) = href.strip_prefix(THEUTH_SCHEME) else {
            return Err(format!("not a theuth:// link: {href:?}"));
        };
        if rest.chars().any(char::is_whitespace) {
            return Err(format!(
                "theuth:// link must not contain whitespace: {href:?}"
            ));
        }
        let (rest, query) = match rest.split_once('?') {
            Some((r, q)) => (r, Some(q)),
            None => (rest, None),
        };
        let sha8 = match query {
            None | Some("") => None,
            Some(q) => {
                let Some(s) = q.strip_prefix("s=") else {
                    return Err(format!("theuth:// query must be s=<sha8>, got ?{q}"));
                };
                if s.len() != 8
                    || !s
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
                {
                    return Err(format!("s= must be 8 lowercase hex characters, got {s:?}"));
                }
                Some(s.to_owned())
            }
        };
        let Some((kind, target)) = rest.split_once('/') else {
            return Err(format!(
                "theuth://<kind>/<id> needs a kind and an id: {href:?}"
            ));
        };
        let Some(kind) = TheuthKind::parse(kind) else {
            return Err(format!(
                "theuth:// kind must be notes, sources, knowledge, meetings or sessions, not {kind:?}"
            ));
        };
        let (id, locator) = match kind {
            // A knowledge id has `/` segments and may end `@<commit>`; it takes no locator.
            TheuthKind::Knowledge => {
                if target.contains('#') {
                    return Err("a knowledge citation takes no #locator".into());
                }
                (target, None)
            }
            _ => match target.find(['#', '@']) {
                None => (target, None),
                Some(at) => {
                    let (id, loc) = target.split_at(at);
                    let (mark, value) = loc.split_at(1);
                    let locator = match (kind, mark) {
                        (TheuthKind::Notes | TheuthKind::Sources, "#") => block(value)?,
                        (TheuthKind::Sources | TheuthKind::Meetings, "@") => {
                            Locator::Time(time_ms(value)?)
                        }
                        (TheuthKind::Sessions, "#") => Locator::Turn(
                            value
                                .parse::<u32>()
                                .ok()
                                .filter(|t| *t >= 1)
                                .ok_or_else(|| {
                                    format!("#<turn> must be a 1-based number, got #{value}")
                                })?,
                        ),
                        _ => {
                            return Err(format!(
                                "theuth://{}/ takes no {mark}<locator>",
                                kind.as_str()
                            ));
                        }
                    };
                    (id, Some(locator))
                }
            },
        };
        if id.is_empty() || (kind != TheuthKind::Knowledge && id.contains('/')) {
            return Err(format!(
                "theuth://{}/<id> needs one id segment: {href:?}",
                kind.as_str()
            ));
        }
        Ok(Self {
            kind,
            id: id.to_owned(),
            locator,
            sha8,
        })
    }

    /// Render back to the canonical href.
    pub fn href(&self) -> String {
        let mut s = format!("{THEUTH_SCHEME}{}/{}", self.kind.as_str(), self.id);
        if let Some(l) = &self.locator {
            s.push_str(&l.text());
        }
        if let Some(sha) = &self.sha8 {
            s.push_str("?s=");
            s.push_str(sha);
        }
        s
    }
}

/// `#<blockId>`: non-blank, no `/`, `#`, `?`.
fn block(v: &str) -> Result<Locator, String> {
    if v.is_empty() || v.contains(['/', '#', '?', '@']) {
        return Err(format!("#<block> needs a block id, got #{v:?}"));
    }
    Ok(Locator::Block(v.to_owned()))
}

/// `mm:ss` or `h:mm:ss` → milliseconds; seconds (and minutes under an hour) < 60.
fn time_ms(v: &str) -> Result<u64, String> {
    let bad = || format!("expected @mm:ss or @h:mm:ss, got @{v}");
    let num = |p: &str| -> Result<u64, String> {
        (!p.is_empty() && p.len() <= 4 && p.bytes().all(|b| b.is_ascii_digit()))
            .then(|| p.parse::<u64>().ok())
            .flatten()
            .ok_or_else(bad)
    };
    let parts: Vec<u64> = v.split(':').map(num).collect::<Result<_, _>>()?;
    match parts[..] {
        [m, s] if s < 60 => Ok((m * 60 + s) * 1000),
        [h, m, s] if m < 60 && s < 60 => Ok(((h * 60 + m) * 60 + s) * 1000),
        _ => Err(bad()),
    }
}

/// Parse one anchor string; `Err` is the message a located error carries.
pub fn parse(s: &str) -> Result<Anchor, String> {
    if s.starts_with(THEUTH_SCHEME) {
        return TheuthAnchor::parse(s).map(Anchor::Theuth);
    }
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

    fn theuth(kind: TheuthKind, id: &str, locator: Option<Locator>, sha8: Option<&str>) -> Anchor {
        Anchor::Theuth(TheuthAnchor {
            kind,
            id: id.into(),
            locator,
            sha8: sha8.map(str::to_owned),
        })
    }

    #[test]
    fn theuth_grammar_accepts_the_five_kinds_and_their_locators() {
        let id = "01k7abcdefghijklmnopqrstuv";
        assert_eq!(
            parse(&format!("theuth://notes/{id}#b12?s=7f2a9c1e")),
            Ok(theuth(
                TheuthKind::Notes,
                id,
                Some(Locator::Block("b12".into())),
                Some("7f2a9c1e")
            ))
        );
        assert_eq!(
            parse(&format!("theuth://notes/{id}")),
            Ok(theuth(TheuthKind::Notes, id, None, None))
        );
        assert_eq!(
            parse(&format!("theuth://sources/{id}#7f2a")),
            Ok(theuth(
                TheuthKind::Sources,
                id,
                Some(Locator::Block("7f2a".into())),
                None
            ))
        );
        assert_eq!(
            parse(&format!("theuth://sources/{id}@41:12")),
            Ok(theuth(
                TheuthKind::Sources,
                id,
                Some(Locator::Time(2_472_000)),
                None
            ))
        );
        assert_eq!(
            parse("theuth://meetings/m1@1:02:03"),
            Ok(theuth(
                TheuthKind::Meetings,
                "m1",
                Some(Locator::Time(3_723_000)),
                None
            ))
        );
        assert_eq!(
            parse("theuth://sessions/0192a4c1-7e3b#7"),
            Ok(theuth(
                TheuthKind::Sessions,
                "0192a4c1-7e3b",
                Some(Locator::Turn(7)),
                None
            ))
        );
        assert_eq!(
            parse("theuth://knowledge/decisions/prod-dr@3f2a9c1"),
            Ok(theuth(
                TheuthKind::Knowledge,
                "decisions/prod-dr@3f2a9c1",
                None,
                None
            ))
        );
        // Round trip.
        for href in [
            format!("theuth://notes/{id}#b12?s=7f2a9c1e"),
            format!("theuth://sources/{id}@41:12"),
            "theuth://meetings/m1@1:02:03".to_owned(),
            "theuth://sessions/s#7".to_owned(),
            "theuth://knowledge/a/b".to_owned(),
        ] {
            assert_eq!(TheuthAnchor::parse(&href).unwrap().href(), href);
        }
    }

    #[test]
    fn theuth_grammar_rejects_bad_kinds_locators_and_hashes() {
        let bad = [
            "theuth://",
            "theuth://notes",
            "theuth://notes/",
            "theuth://boards/x#1",
            "theuth://notes/a/b#1",
            "theuth://notes/x@1:00",
            "theuth://notes/x#",
            "theuth://meetings/x#b1",
            "theuth://meetings/x@99",
            "theuth://meetings/x@1:60",
            "theuth://sessions/x#0",
            "theuth://sessions/x#b1",
            "theuth://sessions/x@1:00",
            "theuth://knowledge/a/b#1",
            "theuth://notes/x#1?s=ABCDEF01",
            "theuth://notes/x#1?s=7f2a",
            "theuth://notes/x#1?t=7f2a9c1e",
            "theuth://notes/x y",
        ];
        for s in bad {
            assert!(parse(s).is_err(), "{s:?} should not parse");
        }
        assert_eq!(
            parse("theuth://boards/x").unwrap_err(),
            "theuth:// kind must be notes, sources, knowledge, meetings or sessions, not \"boards\""
        );
        assert_eq!(
            parse("theuth://notes/x@1:00").unwrap_err(),
            "theuth://notes/ takes no @<locator>"
        );
    }
}
