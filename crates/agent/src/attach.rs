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

/// §5.2: the standing `attachments` block, registered by the agent plugin
/// beside its own. A pure function of nothing, so the system cache holds.
pub const PROMPT: &str = "**Attachments.** A user message may begin with lines \
`[attached <kind> <id> \"<title>\" · <details>]` (older messages: `[doc <id> \"<title>\"]`, the same thing \
for notes and knowledge). Each is a reference the user attached to that message — none of its content is in \
your context yet. When the request concerns it, fetch what you need with the tool for its kind, and read big \
things in pieces:\n\n\
| kind | first look | then |\n\
|---|---|---|\n\
| note, knowledge | `doc_read(<id>)` outline | `doc_read(<id>, format=\"markdown\", blocks=[…])`; edit with \
`doc_insert`/`doc_replace`. `knowledge <id>@<commit>` is a past revision: `knowledge_get(\"<id>@<commit>\")`, read-only. |\n\
| artifact | `artifact_read(<id>)` (header: name, kind, version, size; first 2000 lines or the rendered PDF/document) | \
`offset`/`limit`/`ranges`/`tail` like `read_file`; images come back as pictures; archives as a listing; a markdown \
artifact names its document id for `doc_read`. |\n\
| session | `session_read(<id>)` outline: numbered turns, one line each | `session_read(<id>, turns=[a,b])` for the \
full text of those turns. |\n\
| meeting | `meeting_read(meeting=<id>)` transcript (400 lines per call, `from_ms`/`to_ms`); `doc_read(\"meeting:<id>\")` \
for the notes and summary | narrow by time or speaker. |\n\n\
Call it by its title in your prose, never by its id. Several lines mean several attachments; ask which when the \
request is ambiguous. A reference belongs to the message it came with; things attached earlier are in the \
transcript above. If a tool reports the item missing, deleted or unreadable, say so plainly.";

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

/// What a session is called when its stored title is empty (66 §8, Sky's
/// ruling: a human name, never an id): the first user line with the markers
/// stripped, cut at [`TITLE_CHARS`] with an ellipsis, else
/// `Untitled chat · <Mon d>` from `created_at` (local time).
pub fn session_title(title: &str, first_user: Option<&str>, created_at: chrono::DateTime<chrono::Utc>) -> String {
    if !title.trim().is_empty() {
        return title.to_owned();
    }
    if let Some(line) = first_user.and_then(|t| parse(t).body.lines().map(str::trim).find(|l| !l.is_empty())) {
        let mut out: String = line.chars().take(TITLE_CHARS).collect();
        if line.chars().nth(TITLE_CHARS).is_some() {
            out.push('…');
        }
        return out;
    }
    format!("Untitled chat · {}", created_at.with_timezone(&chrono::Local).format("%b %-d"))
}

/// Characters of the first user line kept as a fallback session title.
pub const TITLE_CHARS: usize = 60;

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

    #[test]
    fn session_title_prefers_title_then_first_user_line_then_date() {
        let created = chrono::DateTime::parse_from_rfc3339("2026-09-18T14:05:00Z").unwrap().to_utc();
        assert_eq!(session_title("Fix relay", Some("hello"), created), "Fix relay");
        let first = "[attached note abc \"Groceries\" · 1 KB]\n\n  add milk to the list  \nsecond line";
        assert_eq!(session_title(" ", Some(first), created), "add milk to the list");
        let long = "x".repeat(70);
        assert_eq!(session_title("", Some(&long), created), format!("{}…", "x".repeat(60)));
        assert_eq!(session_title("", Some(&"y".repeat(60)), created), "y".repeat(60));
        let expected = format!("Untitled chat · {}", created.with_timezone(&chrono::Local).format("%b %-d"));
        assert_eq!(session_title("", Some("[attached note abc \"Groceries\"]"), created), expected);
        assert_eq!(session_title("", None, created), expected);
    }
}
