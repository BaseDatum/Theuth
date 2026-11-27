//! Picture fences on chat channels (design doc 164 D2, D6–D8).
//!
//! Signal, Telegram and Slack send a closed `json-render` (alias `jsonrender`), `mermaid` or
//! `openui` fence as a PNG a connected client draws, or as the text [`picture_fallback`] when
//! nobody can. [`picture_parts`] splits a reply at those fences; [`picture_preview`] is the
//! live text of a draft that is still streaming; [`flatten`] is the text-only rendering for
//! callers that send no pictures. None of them ever returns a fence body verbatim.

use std::borrow::Cow;
use std::ops::Range;

use crate::fences::fences;
use crate::render::{plain_text, spec_lines_text};
use crate::{Spec, export, mermaid_lines, parse};

/// The line a live preview shows where a picture is being drawn (D8).
pub const DRAWING: &str = "_drawing…_";
/// Longest `export::lower` fallback, in bytes; a longer lowering becomes the placeholder.
const MAX_LOWERED: usize = 3500;
/// What a fallback must never contain: a picture fence opener or a spec object (D7).
const FORBIDDEN: [&str; 5] = [
    "```json-render",
    "```jsonrender",
    "```mermaid",
    "```openui",
    "{\"root\"",
];

/// The fence languages a client draws as a picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PictureLang {
    /// `json-render` or `jsonrender`.
    JsonRender,
    /// `mermaid` (alias `mmd`).
    Mermaid,
    /// `openui`.
    Openui,
}

impl PictureLang {
    /// The picture language of a fence info string: its first word, ASCII case-insensitive.
    pub fn of(info: &str) -> Option<Self> {
        let word = info.split_whitespace().next()?;
        [
            ("json-render", Self::JsonRender),
            ("jsonrender", Self::JsonRender),
            ("mermaid", Self::Mermaid),
            ("mmd", Self::Mermaid),
            ("openui", Self::Openui),
        ]
        .into_iter()
        .find_map(|(name, lang)| word.eq_ignore_ascii_case(name).then_some(lang))
    }

    /// The canonical info string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::JsonRender => "json-render",
            Self::Mermaid => "mermaid",
            Self::Openui => "openui",
        }
    }

    /// What the placeholder calls a fence with no title or caption.
    fn noun(self) -> &'static str {
        match self {
            Self::JsonRender | Self::Mermaid => "diagram",
            Self::Openui => "interactive panel",
        }
    }
}

/// One picture fence of a text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PictureFence<'a> {
    /// The language.
    pub lang: PictureLang,
    /// Everything between the fence lines; to the end of the text when unclosed.
    pub body: &'a str,
    /// Whether a closing line was found. An unclosed fence in a finished text is broken.
    pub closed: bool,
    /// In the text split: the opening line's start to the closing line's end (its line break
    /// excluded); to the end of the text when unclosed.
    pub range: Range<usize>,
}

impl PictureFence<'_> {
    /// The text that stands in for the picture: [`picture_fallback`] for a closed fence, the
    /// placeholder for an unclosed one.
    pub fn fallback(&self, width: usize) -> String {
        if self.closed {
            picture_fallback(self.lang, self.body, width)
        } else {
            placeholder(self.lang.noun())
        }
    }

    /// The picture's caption (D6): the root json-render element's `title` prop, if any.
    pub fn title(&self) -> Option<String> {
        if self.lang != PictureLang::JsonRender || !self.closed {
            return None;
        }
        parse(self.body).ok().and_then(|p| root_title(&p.spec))
    }

    /// Alt text for the picture (D6): the first line of the json-render lowering, else the
    /// title or caption, else `diagram` / `interactive panel`. Never a fence body.
    pub fn alt(&self) -> String {
        let spec = (self.lang == PictureLang::JsonRender && self.closed)
            .then(|| parse(self.body).ok())
            .flatten()
            .map(|p| p.spec);
        let first = spec.as_ref().and_then(|s| {
            export::lower(s)
                .lines()
                .map(|l| l.trim_matches(|c: char| c.is_whitespace() || "#*>-_|`".contains(c)))
                .find(|l| !l.is_empty())
                .map(str::to_owned)
        });
        let alt = first
            .or_else(|| spec.as_ref().and_then(label_of))
            .unwrap_or_else(|| self.lang.noun().to_owned());
        if is_clean(&alt) {
            alt.chars().take(1000).collect()
        } else {
            self.lang.noun().to_owned()
        }
    }
}

/// One piece of a text split by [`picture_parts`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PicturePart<'a> {
    /// Prose (and non-picture fences), verbatim; it keeps the newlines around a fence.
    Text(&'a str),
    /// A picture fence, the fence lines removed.
    Picture(PictureFence<'a>),
}

/// Split `text` at its picture fences, in order. Concatenating the `Text` parts and the fence
/// lines gives `text` back; fences nested in another fenced block are content.
pub fn picture_parts(text: &str) -> Vec<PicturePart<'_>> {
    let mut out = Vec::new();
    let mut pos = 0;
    for f in fences(text) {
        let Some(lang) = PictureLang::of(f.lang) else {
            continue;
        };
        if f.range.start > pos {
            out.push(PicturePart::Text(&text[pos..f.range.start]));
        }
        out.push(PicturePart::Picture(PictureFence {
            lang,
            body: f.body,
            closed: f.closed,
            range: f.range.clone(),
        }));
        pos = f.range.end;
    }
    if pos < text.len() {
        out.push(PicturePart::Text(&text[pos..]));
    }
    out
}

/// The text standing in for a closed `lang` fence with `body` (D7). Never the body:
/// - json-render that parses: its text rendering in a plain ```` ``` ```` block when it fits
///   `width`, else the markdown of [`export::lower`] (≤ 3500 bytes), else the placeholder;
/// - mermaid: its `width`-column drawing in a plain block when it renders, else the placeholder;
/// - openui, json-render that does not parse: the placeholder.
///
/// The placeholder is `[<title or caption or "diagram"/"interactive panel"> — open theuth to
/// see it]`. A result that would contain a picture fence opener or `{"root"` is the placeholder.
pub fn picture_fallback(lang: PictureLang, body: &str, width: usize) -> String {
    let noun = lang.noun();
    let text = match lang {
        PictureLang::JsonRender => match parse(body) {
            Ok(p) => {
                let label = label_of(&p.spec).filter(|l| is_clean(l));
                let text = spec_lines_text(&p.spec, width)
                    .map(|lines| boxed(&plain_text(&lines)))
                    .filter(|t| is_clean(t))
                    .or_else(|| {
                        let md = compact(&export::lower(&p.spec));
                        (!md.is_empty() && md.len() <= MAX_LOWERED && is_clean(&md)).then_some(md)
                    });
                return text.unwrap_or_else(|| placeholder(label.as_deref().unwrap_or(noun)));
            }
            Err(_) => None,
        },
        PictureLang::Mermaid => match mermaid_lines(body, width) {
            Ok(Some(lines)) => Some(boxed(&plain_text(&lines))),
            _ => None,
        },
        PictureLang::Openui => None,
    };
    text.filter(|t| is_clean(t))
        .unwrap_or_else(|| placeholder(noun))
}

/// The live text of a streaming draft (D8): `text` with every picture fence, open or closed,
/// replaced by the [`DRAWING`] line. A trailing fence opener whose info string is still
/// streaming in (`` ```mer ``) counts as a picture fence too.
pub fn picture_preview(text: &str) -> Cow<'_, str> {
    let mut out = String::new();
    let mut pos = 0;
    for f in fences(text) {
        let opening_only =
            !f.closed && f.range.end == text.len() && !text[f.range.clone()].contains('\n');
        if PictureLang::of(f.lang).is_none() && !(opening_only && is_lang_prefix(f.lang)) {
            continue;
        }
        out.push_str(&text[pos..f.range.start]);
        out.push_str(DRAWING);
        pos = f.range.end;
    }
    if pos == 0 {
        return Cow::Borrowed(text);
    }
    out.push_str(&text[pos..]);
    Cow::Owned(out)
}

/// `text` without its picture fences, open or closed: what a voice clip reads (164 D9 — the
/// picture is in the chat; nobody wants a diagram read aloud).
pub fn strip_pictures(text: &str) -> Cow<'_, str> {
    let parts = picture_parts(text);
    if parts.iter().all(|p| matches!(p, PicturePart::Text(_))) {
        return Cow::Borrowed(text);
    }
    Cow::Owned(
        parts
            .into_iter()
            .filter_map(|p| match p {
                PicturePart::Text(t) => Some(t),
                PicturePart::Picture(_) => None,
            })
            .collect(),
    )
}

/// `text` with every picture fence as its [`PictureFence::fallback`]: the text-only rendering
/// for a channel that sends no pictures (30 §9, 164 D7). Everything else is byte-identical.
pub fn flatten(text: &str, width: usize) -> String {
    let mut out = String::with_capacity(text.len());
    for part in picture_parts(text) {
        match part {
            PicturePart::Text(t) => out.push_str(t),
            PicturePart::Picture(f) => out.push_str(&f.fallback(width)),
        }
    }
    out
}

/// A non-empty info string a picture language starts with.
fn is_lang_prefix(info: &str) -> bool {
    !info.is_empty()
        && !info.contains(char::is_whitespace)
        && ["json-render", "jsonrender", "mermaid", "openui"]
            .iter()
            .any(|l| l.len() >= info.len() && l[..info.len()].eq_ignore_ascii_case(info))
}

/// `[label — open theuth to see it]`.
fn placeholder(label: &str) -> String {
    format!("[{label} — open theuth to see it]")
}

/// `text` inside a plain code block.
fn boxed(text: &str) -> String {
    format!("```\n{text}\n```")
}

/// `md` trimmed, runs of blank lines collapsed to one.
fn compact(md: &str) -> String {
    let mut out = String::with_capacity(md.len());
    let mut blank = false;
    for line in md.trim().lines() {
        let line = line.trim_end();
        if line.is_empty() {
            blank = true;
            continue;
        }
        if blank && !out.is_empty() {
            out.push('\n');
        }
        blank = false;
        out.push_str(line);
        out.push('\n');
    }
    out.truncate(out.trim_end().len());
    out
}

/// Free of every [`FORBIDDEN`] marker.
fn is_clean(text: &str) -> bool {
    !FORBIDDEN.iter().any(|m| text.contains(m))
}

/// The root element's `title` prop, when it is a non-empty string.
fn root_title(spec: &Spec) -> Option<String> {
    let root = spec.elements.get(&spec.root)?;
    let v = serde_json::to_value(&root.component).ok()?;
    v.get("props")?
        .get("title")?
        .as_str()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
}

/// Title, else the export caption (`Graph: 3 nodes, 2 edges`).
fn label_of(spec: &Spec) -> Option<String> {
    root_title(spec).or_else(|| {
        spec.elements
            .get(&spec.root)
            .and_then(|e| export::caption(&e.component))
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    const GRAPH: &str = include_str!("../fixtures/json-render/graph-architecture.json");
    const READINESS: &str = include_str!("../fixtures/json-render/valid-release-readiness.json");
    const BROKEN: &str = include_str!("../fixtures/json-render/broken-unknown-type.json");
    const SMALL: &str = "{\"root\":\"s\",\"elements\":{\"s\":{\"type\":\"Separator\"}}}\n";

    fn fence(lang: &str, body: &str) -> String {
        format!("```{lang}\n{body}```")
    }

    fn assert_clean(out: &str) {
        for m in FORBIDDEN {
            assert!(!out.contains(m), "{m} in:\n{out}");
        }
    }

    #[test]
    fn langs_are_the_first_word_case_insensitive() {
        assert_eq!(
            PictureLang::of("json-render"),
            Some(PictureLang::JsonRender)
        );
        assert_eq!(PictureLang::of("jsonrender"), Some(PictureLang::JsonRender));
        assert_eq!(PictureLang::of("Mermaid title"), Some(PictureLang::Mermaid));
        assert_eq!(PictureLang::of("mmd"), Some(PictureLang::Mermaid));
        assert_eq!(PictureLang::of("MMD"), Some(PictureLang::Mermaid));
        assert_eq!(PictureLang::of("openui"), Some(PictureLang::Openui));
        assert_eq!(PictureLang::of("json"), None);
        assert_eq!(PictureLang::of(""), None);
    }

    /// Prose, a Graph, mermaid, openui, a broken spec and an unclosed fence, in order.
    #[test]
    fn parts_split_a_mixed_reply_in_order() {
        let text = format!(
            "Intro.\n\n{g}\n\nMid.\n{m}\n{o}\n```rust\nfn x() {{}}\n```\n{b}\nTail.\n```mermaid\ngraph LR\n  A",
            g = fence("json-render", GRAPH),
            m = fence("mermaid", "graph LR\n  A --> B\n"),
            o = fence("openui", "<Card/>\n"),
            b = fence("jsonrender", BROKEN),
        );
        let parts = picture_parts(&text);
        // The fence lines' span, found by their text.
        let at = |s: &str| {
            let i = text.find(s).unwrap();
            i..i + s.len()
        };
        let pic = |lang, body, closed, range| {
            PicturePart::Picture(PictureFence {
                lang,
                body,
                closed,
                range,
            })
        };
        assert_eq!(
            parts,
            vec![
                PicturePart::Text("Intro.\n\n"),
                pic(
                    PictureLang::JsonRender,
                    GRAPH,
                    true,
                    at(&fence("json-render", GRAPH))
                ),
                PicturePart::Text("\n\nMid.\n"),
                pic(
                    PictureLang::Mermaid,
                    "graph LR\n  A --> B\n",
                    true,
                    at(&fence("mermaid", "graph LR\n  A --> B\n"))
                ),
                PicturePart::Text("\n"),
                pic(
                    PictureLang::Openui,
                    "<Card/>\n",
                    true,
                    at(&fence("openui", "<Card/>\n"))
                ),
                PicturePart::Text("\n```rust\nfn x() {}\n```\n"),
                pic(
                    PictureLang::JsonRender,
                    BROKEN,
                    true,
                    at(&fence("jsonrender", BROKEN))
                ),
                PicturePart::Text("\nTail.\n"),
                pic(
                    PictureLang::Mermaid,
                    "graph LR\n  A",
                    false,
                    text.rfind("```mermaid").unwrap()..text.len()
                ),
            ]
        );
        assert_eq!(picture_parts("plain\n"), vec![PicturePart::Text("plain\n")]);
        assert!(picture_parts("").is_empty());
        assert_eq!(
            strip_pictures(&text),
            "Intro.\n\n\n\nMid.\n\n\n```rust\nfn x() {}\n```\n\nTail.\n"
        );
        assert!(matches!(strip_pictures("plain"), Cow::Borrowed("plain")));
    }

    #[test]
    fn fallback_goldens_for_every_branch() {
        // json-render that fits: the text box.
        let small = picture_fallback(PictureLang::JsonRender, READINESS, 60);
        assert!(small.starts_with("```\n╭─ Release readiness "), "{small}");
        assert!(small.ends_with("\n```"), "{small}");
        // json-render too wide for the box: the lowering.
        let wide = picture_fallback(PictureLang::JsonRender, READINESS, 8);
        assert!(
            !wide.starts_with("```")
                && !wide.starts_with('[')
                && wide.contains("Release readiness"),
            "{wide}"
        );
        // A lowering over the cap, no title: the plain placeholder.
        let rows: Vec<String> = (0..400)
            .map(|i| format!("[\"row {i}\",\"{}\"]", "x".repeat(20)))
            .collect();
        let table = format!(
            "{{\"root\":\"t\",\"elements\":{{\"t\":{{\"type\":\"Table\",\"props\":{{\"columns\":[\"a\",\"b\"],\"rows\":[{}]}}}}}}}}",
            rows.join(",")
        );
        assert_eq!(
            picture_fallback(PictureLang::JsonRender, &table, 8),
            "[diagram — open theuth to see it]"
        );
        // Broken json-render, openui: the placeholder.
        assert_eq!(
            picture_fallback(PictureLang::JsonRender, BROKEN, 60),
            "[diagram — open theuth to see it]"
        );
        assert_eq!(
            picture_fallback(PictureLang::Openui, "<Card title=\"x\"/>", 60),
            "[interactive panel — open theuth to see it]"
        );
        // mermaid that draws: the box; one that does not: the placeholder.
        let m = picture_fallback(PictureLang::Mermaid, "graph LR\n  A --> B\n", 60);
        assert!(
            m.starts_with("```\n") && m.contains('A') && m.contains('B'),
            "{m}"
        );
        assert!(!m.contains("-->"), "{m}");
        assert_eq!(
            picture_fallback(PictureLang::Mermaid, "flowchart LR\n  A[f(x)] --> B\n", 60),
            "[diagram — open theuth to see it]"
        );
        // An unclosed fence is broken.
        let open = PictureFence {
            lang: PictureLang::JsonRender,
            body: SMALL,
            closed: false,
            range: 0..0,
        };
        assert_eq!(open.fallback(60), "[diagram — open theuth to see it]");
        // A spec whose rendering would carry a fence opener: the placeholder, titled.
        let sneaky = "{\"root\":\"c\",\"elements\":{\"c\":{\"type\":\"Card\",\"props\":{\"title\":\"Plan\"},\"children\":[\"t\"]},\"t\":{\"type\":\"Text\",\"props\":{\"text\":\"```mermaid\"}}}}";
        assert_eq!(
            picture_fallback(PictureLang::JsonRender, sneaky, 60),
            "[Plan — open theuth to see it]"
        );
    }

    #[test]
    fn titles_and_alt_text() {
        let f = |body| PictureFence {
            lang: PictureLang::JsonRender,
            body,
            closed: true,
            range: 0..0,
        };
        assert_eq!(f(READINESS).title().as_deref(), Some("Release readiness"));
        assert_eq!(f(GRAPH).title(), None);
        assert_eq!(f(GRAPH).alt(), "Diagram: 6 nodes, 7 edges");
        assert_eq!(f(BROKEN).alt(), "diagram");
        let m = PictureFence {
            lang: PictureLang::Mermaid,
            body: "graph LR\n  A --> B\n",
            closed: true,
            range: 0..0,
        };
        assert_eq!((m.title(), m.alt().as_str()), (None, "diagram"));
    }

    /// Every fixture spec, valid or broken, at a phone width and a narrow one: no fence info
    /// string, no spec object.
    #[test]
    fn no_fixture_fallback_leaks_the_body() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/json-render");
        let mut n = 0;
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|e| e != "json") {
                continue;
            }
            let body = std::fs::read_to_string(&path).unwrap();
            for width in [60, 20] {
                let out = picture_fallback(PictureLang::JsonRender, &body, width);
                assert_clean(&out);
                assert!(!out.contains(body.trim()), "{}", path.display());
                let flat = flatten(&format!("a\n```json-render\n{body}```\nb\n"), width);
                assert_clean(&flat);
            }
            n += 1;
        }
        assert!(n > 100, "only {n} fixtures");
    }

    #[test]
    fn flatten_never_leaks_a_body() {
        let text = format!(
            "Takeaway.\n\n{}\n\nBroken:\n\n{}\n\n{}\n\n{}\n\n```json-render\n{SMALL}",
            fence("json-render", GRAPH),
            fence("json-render", BROKEN),
            fence("mermaid", "flowchart LR\n  A[f(x)] --> B\n"),
            fence("openui", "<Card/>\n"),
        );
        let out = flatten(&text, 60);
        assert_clean(&out);
        assert!(out.starts_with("Takeaway.\n\n```\nEdge\n"), "{out}");
        assert!(
            out.ends_with(
                "Broken:\n\n[diagram — open theuth to see it]\n\n[diagram — open theuth to see \
                 it]\n\n[interactive panel — open theuth to see it]\n\n[diagram — open theuth to \
                 see it]"
            ),
            "{out}"
        );
        assert_eq!(
            flatten("no fences\n```rs\nx\n```\n", 60),
            "no fences\n```rs\nx\n```\n"
        );
    }

    /// D8: every prefix of a streaming reply previews without a byte of the fence body.
    #[test]
    fn preview_never_shows_a_body_while_streaming() {
        let text = format!(
            "Before.\n\n{}\n\nAfter.\n\n{}\nEnd.",
            fence("json-render", GRAPH),
            fence("mermaid", "graph LR\n  Alpha --> Beta\n")
        );
        for end in (0..=text.len()).filter(|i| text.is_char_boundary(*i)) {
            let p = picture_preview(&text[..end]);
            assert!(!p.contains("\"root\""), "at {end}: {p}");
            assert!(!p.contains("Alpha"), "at {end}: {p}");
            assert!(!p.contains("```j") && !p.contains("```m"), "at {end}: {p}");
        }
        assert_eq!(
            picture_preview(&text),
            "Before.\n\n_drawing…_\n\nAfter.\n\n_drawing…_\nEnd."
        );
        assert_eq!(picture_preview("Before.\n```mer"), "Before.\n_drawing…_");
        assert_eq!(picture_preview("x\n```rs\ny"), "x\n```rs\ny");
        assert!(matches!(picture_preview("plain"), Cow::Borrowed("plain")));
    }
}
