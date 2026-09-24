//! Attachment markers (design doc 66 §3.3, §5.1): a user message may begin
//! with lines `[attached <kind> <id> "<title>" · <meta>]`, one reference per
//! line, followed by the typed body. Only leading lines are markers; the
//! first line that is not a well-formed marker starts the body, so a
//! bracketed line typed after the first sentence is never misread.
//!
//! Legacy spellings from 59 §5.4 are accepted: `[doc <DocId> "<title>"]` is a
//! note, `[doc knowledge:<id> "<title>"]` is a knowledge concept.
//!
//! The same grammar lives in `lib/attach.ts`; both are checked against
//! `tests/fixtures/attach_markers.json`.

use serde::{Deserialize, Serialize};

/// What a reference points at. `id` is whatever the read tool for the kind
/// accepts: `DocId`, concept id (optionally `@<commit>`), artifact `call_id`,
/// `SessionId`, `MeetingId`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// A free-standing note (`DocId`).
    Note,
    /// A knowledge concept, optionally a past revision (`id@<commit>`).
    Knowledge,
    /// One artifact version (`call_id`).
    Artifact,
    /// A past chat (`SessionId`).
    Session,
    /// A meeting (`MeetingId`).
    Meeting,
}

impl Kind {
    /// Every kind, marker order.
    pub const ALL: [Kind; 5] = [Kind::Note, Kind::Knowledge, Kind::Artifact, Kind::Session, Kind::Meeting];

    /// The marker spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Note => "note",
            Kind::Knowledge => "knowledge",
            Kind::Artifact => "artifact",
            Kind::Session => "session",
            Kind::Meeting => "meeting",
        }
    }

    /// Inverse of [`Kind::as_str`].
    pub fn parse(s: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.as_str() == s)
    }
}

impl std::fmt::Display for Kind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One attached reference: `title`/`meta` are frozen at attach time and are
/// display text only; `meta` may be empty.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachRef {
    /// What it is.
    pub kind: Kind,
    /// The id the read tool for `kind` accepts.
    pub id: String,
    /// Human name as the user saw it; never empty.
    pub title: String,
    /// Display-only facts (`pdf v2 · 3 pages`); may be empty.
    #[serde(default)]
    pub meta: String,
}

/// Leading marker lines split from the body. `body` is the text after the
/// last marker line and its newline, verbatim.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Parsed<'a> {
    /// The references, arrival order.
    pub refs: Vec<AttachRef>,
    /// The typed body.
    pub body: &'a str,
}

/// `[attached <kind> <id> "<title>" · <meta>]`; `"` and `\` in the title are
/// escaped, ` · <meta>` is omitted when `meta` is empty.
pub fn marker(r: &AttachRef) -> String {
    let mut out = String::with_capacity(r.id.len() + r.title.len() + r.meta.len() + 24);
    out.push_str("[attached ");
    out.push_str(r.kind.as_str());
    out.push(' ');
    out.push_str(&r.id);
    out.push_str(" \"");
    for c in r.title.chars() {
        if c == '"' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    if !r.meta.is_empty() {
        out.push_str(" · ");
        out.push_str(&r.meta);
    }
    out.push(']');
    out
}

/// Split leading marker lines from `text`. A malformed line is body text.
pub fn parse(text: &str) -> Parsed<'_> {
    let mut refs = Vec::new();
    let mut rest = text;
    loop {
        let (line, after) = match rest.split_once('\n') {
            Some((l, a)) => (l, a),
            None => (rest, ""),
        };
        let Some(r) = parse_line(line) else { break };
        refs.push(r);
        // Consume the line and its newline; a final marker with no newline
        // leaves an empty body.
        rest = if rest.len() > line.len() { after } else { "" };
        if rest.is_empty() {
            break;
        }
    }
    Parsed { refs, body: rest }
}

/// One marker line, or `None` when it is not well-formed.
pub fn parse_line(line: &str) -> Option<AttachRef> {
    let inner = line.strip_prefix('[')?.strip_suffix(']')?;
    let (head, tail) = inner.split_once(' ')?;
    let (kind, id, tail) = if head == "attached" {
        let (kind, tail) = tail.split_once(' ')?;
        let (id, tail) = tail.split_once(' ')?;
        (Kind::parse(kind)?, id, tail)
    } else if head == "doc" {
        let (id, tail) = tail.split_once(' ')?;
        match id.strip_prefix("knowledge:") {
            Some(concept) => (Kind::Knowledge, concept, tail),
            None => (Kind::Note, id, tail),
        }
    } else {
        return None;
    };
    if id.is_empty() || id.chars().any(char::is_whitespace) {
        return None;
    }
    let (title, tail) = quoted(tail)?;
    let meta = if tail.is_empty() {
        ""
    } else {
        let m = tail.strip_prefix(" · ")?;
        if m.contains(']') {
            return None;
        }
        m
    };
    Some(AttachRef { kind, id: id.to_owned(), title, meta: meta.to_owned() })
}

/// `"<title>"` with `\x` → `x`; returns the title and what follows the
/// closing quote.
fn quoted(s: &str) -> Option<(String, &str)> {
    let s = s.strip_prefix('"')?;
    let mut title = String::new();
    let mut chars = s.char_indices();
    while let Some((i, c)) = chars.next() {
        match c {
            '\\' => title.push(chars.next()?.1),
            '"' => return Some((title, &s[i + 1..])),
            _ => title.push(c),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Deserialize)]
    struct Fixture {
        cases: Vec<Case>,
    }

    #[derive(Deserialize)]
    struct Case {
        input: String,
        refs: Vec<AttachRef>,
        body: String,
    }

    fn fixture() -> Vec<Case> {
        let json = include_str!("../tests/fixtures/attach_markers.json");
        serde_json::from_str::<Fixture>(json).expect("fixture parses").cases
    }

    #[test]
    fn fixture_parses_as_specified() {
        for case in fixture() {
            let got = parse(&case.input);
            assert_eq!(got.refs, case.refs, "refs for {:?}", case.input);
            assert_eq!(got.body, case.body, "body for {:?}", case.input);
        }
    }

    #[test]
    fn fixture_refs_round_trip_through_marker() {
        for case in fixture() {
            if case.refs.is_empty() {
                continue;
            }
            let mut text = case.refs.iter().map(marker).collect::<Vec<_>>().join("\n");
            if !case.body.is_empty() {
                text.push('\n');
                text.push_str(&case.body);
            }
            let got = parse(&text);
            assert_eq!(got.refs, case.refs, "round trip of {text:?}");
            assert_eq!(got.body, case.body, "round trip body of {text:?}");
        }
    }

    #[test]
    fn marker_escapes_quote_and_backslash_only() {
        let r = AttachRef {
            kind: Kind::Artifact,
            id: "toolu_01".into(),
            title: r#"say "hi" \ now · then"#.into(),
            meta: "pdf v2".into(),
        };
        assert_eq!(marker(&r), r#"[attached artifact toolu_01 "say \"hi\" \\ now · then" · pdf v2]"#);
        assert_eq!(parse_line(&marker(&r)), Some(r));
    }

    #[test]
    fn empty_meta_omits_separator() {
        let r = AttachRef { kind: Kind::Note, id: "abc".into(), title: "T".into(), meta: String::new() };
        assert_eq!(marker(&r), r#"[attached note abc "T"]"#);
    }
}
