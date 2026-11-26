//! The `<whiteboard>` outline (contract §3.2): what the agent reads, and what `crates/agent`
//! parses into facts and diffs. One item per line, `\n` separated, no trailing spaces.

use std::fmt::Write as _;

use super::model::{Card, CardKind, CardState, Frame, Link, Mark, MarkState, Snapshot, Source, preset_size};

/// Card lines before the overflow line.
pub const MAX_CARD_LINES: usize = 200;

/// The QSTR of §3.2: `"` + text with `\` → `\\`, `"` → `\"`, newlines → space, cut to `max`
/// chars with `…`, + `"`.
pub fn quote(s: &str, max: usize) -> String {
    let flat: String = s
        .chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect();
    let body: String = if flat.chars().count() > max {
        let mut t: String = flat.chars().take(max.saturating_sub(1)).collect();
        t.push('…');
        t
    } else {
        flat
    };
    let mut out = String::with_capacity(body.len() + 2);
    out.push('"');
    for c in body.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Parse a leading QSTR of `s`; returns `(text, rest after the closing quote)`.
pub fn unquote(s: &str) -> Option<(String, &str)> {
    let rest = s.strip_prefix('"')?;
    let mut out = String::new();
    let mut it = rest.char_indices();
    while let Some((i, c)) = it.next() {
        match c {
            '\\' => out.push(it.next()?.1),
            '"' => return Some((out, &rest[i + 1..])),
            c => out.push(c),
        }
    }
    None
}

/// An XML-ish attribute value: `&quot; &amp; &lt; &gt;` escaped.
pub fn attr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\n' | '\r' => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

/// FNV-1a 32 over `bytes`, low 16 bits, 4 lowercase hex — the change detector of sticky and diagram lines.
pub fn fnv16(bytes: &[u8]) -> String {
    let mut h: u32 = 0x811c_9dc5;
    for b in bytes {
        h ^= *b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    format!("{:04x}", h & 0xffff)
}

/// Group boxes `(id, [x, y, w, h])` into rows (§3.2): sort by `y`; a box joins the current row when
/// its vertical centre lies within the row's first box's `[y, y+h]`; rows by top, boxes in a row by `x`.
pub fn rows(items: &[(String, [f64; 4])]) -> Vec<Vec<String>> {
    let mut sorted: Vec<&(String, [f64; 4])> = items.iter().collect();
    sorted.sort_by(|a, b| a.1[1].total_cmp(&b.1[1]).then(a.1[0].total_cmp(&b.1[0])));
    let mut out: Vec<RowAcc<'_>> = Vec::new();
    for it in sorted {
        let centre = it.1[1] + it.1[3] / 2.0;
        match out.last_mut() {
            Some((top, bottom, row)) if centre >= *top && centre <= *bottom => row.push(it),
            _ => out.push((it.1[1], it.1[1] + it.1[3], vec![it])),
        }
    }
    out.into_iter()
        .map(|(_, _, mut row)| {
            row.sort_by(|a, b| a.1[0].total_cmp(&b.1[0]));
            row.into_iter().map(|i| i.0.clone()).collect()
        })
        .collect()
}

fn alias_n(a: &str) -> u32 {
    a.get(1..).and_then(|n| n.parse().ok()).unwrap_or(u32::MAX)
}

/// The cards the outline lists (not archived), with the frame each sits in (`None` = loose).
fn listed(s: &Snapshot) -> Vec<(&Card, Option<&Frame>)> {
    s.cards
        .iter()
        .filter(|c| c.state != CardState::Archived)
        .map(|c| (c, c.frame.as_deref().and_then(|f| s.frame(f))))
        .collect()
}

/// Frames in reading order (by `y`, then `x`).
fn frames_in_order(s: &Snapshot) -> Vec<&Frame> {
    let mut f: Vec<&Frame> = s.frames.iter().collect();
    f.sort_by(|a, b| a.y.total_cmp(&b.y).then(a.x.total_cmp(&b.x)).then(a.id.cmp(&b.id)));
    f
}

/// The frame line (without the leading `+`).
pub fn frame_line(s: &Snapshot, fr: &Frame) -> String {
    let mut l = format!("frame {} {}", s.name(&fr.id), quote(&fr.title, 80));
    if let Some(p) = fr.preset.as_deref()
        && let Some((w, h)) = preset_size(p)
    {
        let _ = write!(l, " · {p} {w}×{h}");
    }
    l
}

/// The card line.
pub fn card_line(s: &Snapshot, c: &Card) -> String {
    let mut l = format!("{} {} {}", s.name(&c.id), c.kind.as_str(), quote(&c.title, 80));
    let n = c.versions.len();
    if !c.kind.in_place() && n > 0 {
        let _ = write!(l, " v{n}/{n}");
    }
    if c.placeholder.is_some() {
        l.push_str(" · placeholder");
    } else {
        l.push_str(" · ");
        l.push_str(c.state.as_str());
        if c.state == CardState::Dimmed
            && let Some(t) = &c.tag
        {
            l.push(' ');
            l.push_str(&quote(t, 120));
        }
    }
    for e in extras(c) {
        l.push_str(" · ");
        l.push_str(&e);
    }
    l
}

/// The opaque extras of a card line.
fn extras(c: &Card) -> Vec<String> {
    let Some(v) = c.versions.last() else {
        return Vec::new();
    };
    match &v.source {
        Source::Sticky { text } => vec![
            format!("{} lines", text.lines().filter(|l| !l.trim().is_empty()).count()),
            format!("#{}", fnv16(text.as_bytes())),
        ],
        Source::Diagram { lang, code } => {
            vec![format!("#{}", fnv16(format!("{}\n{code}", lang.as_str()).as_bytes()))]
        }
        Source::Excerpt { path, start, end, .. } => vec![format!("{path}#L{start}-L{end}")],
        Source::Web { url } => host(url).map(|h| vec![h.to_owned()]).unwrap_or_default(),
        Source::View { path } => vec![path.clone()],
        Source::Task { task } => vec![format!("task {task}")],
        _ => Vec::new(),
    }
}

fn host(url: &str) -> Option<&str> {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let h = rest.split(['/', '?', '#']).next()?;
    let h = h.rsplit_once('@').map(|(_, h)| h).unwrap_or(h);
    (!h.is_empty()).then_some(h)
}

/// The link line.
pub fn link_line(s: &Snapshot, l: &Link) -> String {
    let mut out = format!("{} {} {} {}", s.name(&l.id), s.name(&l.from), l.arrow.glyph(), s.name(&l.to));
    if let Some(t) = l.label.as_deref().filter(|t| !t.is_empty()) {
        out.push(' ');
        out.push_str(&quote(t, 80));
    }
    out
}

/// A mark's caption (`page says: …`, `lines A–B`, `hunk …`), unquoted.
pub fn caption(m: &Mark) -> Option<String> {
    let r = m.region.as_ref()?;
    let label = r.label.trim();
    if label.is_empty() {
        return None;
    }
    Some(match r.kind.as_str() {
        "page" => format!("page says: {label}"),
        "hunk" if !label.starts_with("hunk") => format!("hunk {label}"),
        _ => label.to_owned(),
    })
}

/// The mark line.
pub fn mark_line(s: &Snapshot, m: &Mark) -> String {
    let mut l = match (&m.card, m.ver) {
        (Some(c), Some(v)) => format!("{} on {} v{v}", s.name(&m.id), s.name(c)),
        (Some(c), None) => format!("{} on {}", s.name(&m.id), s.name(c)),
        _ => format!("{} on canvas", s.name(&m.id)),
    };
    l.push_str(" · ");
    l.push_str(m.state.as_str());
    if let Some(c) = caption(m) {
        l.push_str(" · ");
        l.push_str(&quote(&c, 120));
    }
    if let Some(h) = m.hw.as_deref().filter(|h| !h.trim().is_empty()) {
        l.push_str(" · handwriting: ");
        l.push_str(&quote(h, 200));
    }
    l
}

/// What [`render`] produced, beside the text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    /// Card lines.
    pub cards: u32,
    /// Link lines.
    pub links: u32,
    /// Mark lines.
    pub marks: u32,
}

/// Rows of aliases, top to bottom, each left to right.
pub type Rows = Vec<Vec<String>>;
/// A row being built: its top, its bottom, its boxes.
type RowAcc<'a> = (f64, f64, Vec<&'a (String, [f64; 4])>);

/// The rows of every frame (reading order) and of the loose cards.
pub fn layout(s: &Snapshot) -> (Vec<(&Frame, Rows)>, Rows) {
    let cards = listed(s);
    let boxes = |frame: Option<&str>| -> Vec<(String, [f64; 4])> {
        cards
            .iter()
            .filter(|(_, f)| f.map(|f| f.id.as_str()) == frame)
            .map(|(c, _)| (s.name(&c.id).to_owned(), c.rect()))
            .collect()
    };
    let frames = frames_in_order(s)
        .into_iter()
        .map(|f| (f, rows(&boxes(Some(&f.id)))))
        .collect();
    (frames, rows(&boxes(None)))
}

/// The `<whiteboard>` block of §3.2.
pub fn render(doc: &str, title: &str, s: &Snapshot) -> (String, Counts) {
    let mut cards: Vec<&Card> = listed(s).into_iter().map(|(c, _)| c).collect();
    cards.sort_by_key(|c| (alias_n(s.name(&c.id)), c.id.clone()));
    let shown: std::collections::HashSet<&str> = cards.iter().map(|c| c.id.as_str()).collect();
    let mut links: Vec<&Link> = s
        .links
        .iter()
        .filter(|l| shown.contains(l.from.as_str()) && shown.contains(l.to.as_str()))
        .collect();
    links.sort_by_key(|l| (alias_n(s.name(&l.id)), l.id.clone()));
    let mut marks: Vec<&Mark> = s
        .marks
        .iter()
        .filter(|m| matches!(m.state, MarkState::Sent | MarkState::Pinned))
        .collect();
    marks.sort_by_key(|m| (alias_n(s.name(&m.id)), m.id.clone()));
    let listed_cards = cards.len().min(MAX_CARD_LINES);
    let counts = Counts {
        cards: listed_cards as u32,
        links: links.len() as u32,
        marks: marks.len() as u32,
    };
    let mut out = format!(
        "<whiteboard id=\"{}\" title=\"{}\" cards=\"{}\" links=\"{}\" marks=\"{}\">\n",
        attr(doc),
        attr(title),
        counts.cards,
        counts.links,
        counts.marks
    );
    let (frames, loose) = layout(s);
    for (f, rows) in &frames {
        out.push_str(&frame_line(s, f));
        out.push('\n');
        push_rows(&mut out, rows);
    }
    if !loose.is_empty() {
        out.push_str("loose\n");
        push_rows(&mut out, &loose);
    }
    for c in cards.iter().take(MAX_CARD_LINES) {
        out.push_str(&card_line(s, c));
        out.push('\n');
    }
    if cards.len() > MAX_CARD_LINES {
        let frame = frames.first().map(|(f, _)| s.name(&f.id)).unwrap_or("f1");
        let _ = writeln!(
            out,
            "… {} more cards — wb_read(frame=\"{frame}\") for one frame",
            cards.len() - MAX_CARD_LINES
        );
    }
    for l in &links {
        out.push_str(&link_line(s, l));
        out.push('\n');
    }
    for m in &marks {
        out.push_str(&mark_line(s, m));
        out.push('\n');
    }
    out.push_str("</whiteboard>");
    (out, counts)
}

fn push_rows(out: &mut String, rows: &[Vec<String>]) {
    for (i, r) in rows.iter().enumerate() {
        let _ = writeln!(out, "  row {}: {}", i + 1, r.join(" "));
    }
}

/// One paragraph saying what a card's current version is and how to read it.
pub fn source_summary(c: &Card) -> (String, Option<String>) {
    let Some(v) = c.versions.last() else {
        return match &c.placeholder {
            Some(p) => (format!("placeholder {} — nothing placed yet", quote(p, 80)), None),
            None => ("no version yet".into(), None),
        };
    };
    match &v.source {
        Source::Page { session, call_id } => (
            format!(
                "an .html page from call {call_id}{}",
                session.as_deref().map(|s| format!(" of session {s}")).unwrap_or_default()
            ),
            Some(format!("artifact_read(\"{call_id}\")")),
        ),
        Source::Diagram { lang, code } => (
            format!("{} diagram, {} lines", lang.as_str(), code.lines().count()),
            None,
        ),
        Source::Note { doc } => (format!("note {doc}"), Some(format!("doc_read(\"{doc}\")"))),
        Source::Excerpt { path, start, end, .. } => (format!("excerpt {path}#L{start}-L{end}"), None),
        Source::Diff { session, frozen, .. } => match frozen {
            Some(f) => (
                format!("diff frozen as call {} of session {}", f.call_id, f.session),
                Some(format!("artifact_read(\"{}\")", f.call_id)),
            ),
            None => (
                format!(
                    "a working-tree diff{}",
                    session.as_deref().map(|s| format!(" of session {s}")).unwrap_or_default()
                ),
                session.as_deref().map(|s| format!("session_read(\"{s}\")")),
            ),
        },
        Source::Terminal { session, job } => (
            format!(
                "terminal job {job}{}",
                session.as_deref().map(|s| format!(" of session {s}")).unwrap_or_default()
            ),
            session.as_deref().map(|s| format!("session_read(\"{s}\")")),
        ),
        Source::Web { url } => (format!("web page {url}"), None),
        Source::Image { session, call_id, path, blob } => match (call_id, path, blob) {
            (Some(call), _, _) => (
                format!(
                    "image from call {call}{}",
                    session.as_deref().map(|s| format!(" of session {s}")).unwrap_or_default()
                ),
                Some(format!("artifact_read(\"{call}\")")),
            ),
            (None, Some(p), _) => (format!("image file {p}"), Some(format!("read_file(\"{p}\")"))),
            (None, None, Some(b)) => (format!("stored image {b}"), None),
            _ => ("image".into(), None),
        },
        Source::Sticky { text } => (format!("sticky text {}", quote(text, 280)), None),
        Source::Session { session } => (
            format!("session {session}"),
            Some(format!("session_read(\"{session}\")")),
        ),
        Source::View { path } => (
            format!("live app view {path} — the user sees it update and can work in it; wb_crop shows how it last looked"),
            None,
        ),
        Source::Task { task } => (
            format!("task {task}, shown live (state, owner, priority, blockers) — the card title follows the task's"),
            Some(format!("task_get(\"{task}\")")),
        ),
    }
}

/// `wb_read(frame=…)`: one frame's rows and cards, each with its source summary, plus the links
/// and marks among them. Starts with `<whiteboard_frame …>` so it is never taken for a baseline.
pub fn render_frame(doc: &str, s: &Snapshot, frame: &Frame) -> (String, Counts) {
    let mut cards: Vec<&Card> = listed(s)
        .into_iter()
        .filter(|(_, f)| f.is_some_and(|f| f.id == frame.id))
        .map(|(c, _)| c)
        .collect();
    cards.sort_by_key(|c| (alias_n(s.name(&c.id)), c.id.clone()));
    let ids: std::collections::HashSet<&str> = cards.iter().map(|c| c.id.as_str()).collect();
    let mut out = format!(
        "<whiteboard_frame whiteboard=\"{}\" frame=\"{}\">\n",
        attr(doc),
        s.name(&frame.id)
    );
    out.push_str(&frame_line(s, frame));
    out.push('\n');
    let (frames, _) = layout(s);
    if let Some((_, rows)) = frames.iter().find(|(f, _)| f.id == frame.id) {
        push_rows(&mut out, rows);
    }
    for c in &cards {
        out.push_str(&card_line(s, c));
        out.push('\n');
        let (what, read) = source_summary(c);
        let _ = write!(out, "  {what}");
        if let Some(r) = read {
            let _ = write!(out, " — read with {r}");
        }
        out.push('\n');
        if c.kind == CardKind::Excerpt
            && let Some(Source::Excerpt { text, .. }) = c.versions.last().map(|v| &v.source)
        {
            let _ = writeln!(out, "  text: {}", quote(text, 400));
        }
    }
    let mut links = 0;
    for l in s.links.iter().filter(|l| ids.contains(l.from.as_str()) && ids.contains(l.to.as_str())) {
        out.push_str(&link_line(s, l));
        out.push('\n');
        links += 1;
    }
    let mut marks = 0;
    for m in s.marks.iter().filter(|m| {
        matches!(m.state, MarkState::Sent | MarkState::Pinned)
            && m.card.as_deref().is_some_and(|c| ids.contains(c))
    }) {
        out.push_str(&mark_line(s, m));
        out.push('\n');
        marks += 1;
    }
    out.push_str("</whiteboard_frame>");
    (
        out,
        Counts {
            cards: cards.len() as u32,
            links,
            marks,
        },
    )
}

/// The derived file (§3.3 export): `# <title>`, then the block's lines without its open/close lines.
pub fn export_text(title: &str, block: &str) -> String {
    let mut out = format!("# {title}\n\n");
    for line in block.lines().skip(1) {
        if line.starts_with("</whiteboard") {
            break;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::wb::model::{
        Arrow, CardState, DiagramLang, Link, LinkStyle, Mark, MarkState, Region, Snapshot, Version,
        tests::card,
    };

    pub(crate) const DOC: &str = "01a2f3c4-5b6d-7e8f-9a0b-1c2d3e4f5a6b";
    pub(crate) const SESSION: &str = "01a20fc8-92a6-73e6-a622-3ac9e51ae923";

    fn v(source: Source) -> Version {
        Version {
            source,
            at: 1_795_570_100_000,
            by: format!("session:{SESSION}"),
            note: None,
        }
    }

    fn page(call: &str) -> Version {
        v(Source::Page { session: Some(SESSION.into()), call_id: call.into() })
    }

    /// The document behind the §3.2 example.
    pub(crate) fn fixture() -> Snapshot {
        let id = |n: u32| format!("01KAR3ZC4M00000000000000{n:02}");
        let frame = Frame {
            id: id(9),
            title: "Variants".into(),
            x: 36.0,
            y: 48.0,
            w: 610.0,
            h: 424.0,
            colour: "neutral".into(),
            preset: Some("phone".into()),
            by: "owner".into(),
        };
        let mut c1 = card(&id(1), CardKind::Image, [60.0, 80.0, 180.0, 390.0], "Today", vec![v(Source::Image {
            session: None,
            call_id: None,
            path: Some("/tmp/today.png".into()),
            blob: None,
        })]);
        let mut c2 = card(&id(2), CardKind::Page, [250.0, 80.0, 180.0, 390.0], "Mock A — grouped", vec![page("toolu_01A")]);
        c2.state = CardState::Dimmed;
        c2.tag = Some("header kept; grouping rejected".into());
        let mut c3 = card(&id(3), CardKind::Page, [440.0, 90.0, 180.0, 390.0], "Mock B — flat list", vec![page("toolu_01B"), page("toolu_01C")]);
        for c in [&mut c1, &mut c2, &mut c3] {
            c.frame = Some(id(9));
        }
        let c4 = card(&id(4), CardKind::Sticky, [50.0, 494.0, 220.0, 150.0], "Requirements", vec![v(Source::Sticky {
            text: "- Triage with one thumb\n- Never lose a decision\n- Group only when > 20 items".into(),
        })]);
        let c5 = card(&id(5), CardKind::Diagram, [300.0, 500.0, 360.0, 260.0], "Item lifecycle", vec![v(Source::Diagram {
            lang: DiagramLang::Mermaid,
            code: "stateDiagram-v2\n  new --> seen\n  seen --> done\n  seen --> snoozed".into(),
        })]);
        let link = Link {
            id: id(6),
            from: id(4),
            to: id(5),
            from_port: None,
            to_port: None,
            label: Some("drives".into()),
            style: LinkStyle::Solid,
            arrow: Arrow::End,
            by: "owner".into(),
        };
        let mark = |n: u32, state: MarkState| Mark {
            id: id(n),
            strokes: vec![],
            card: Some(id(3)),
            ver: Some(2),
            bb: [0.0, 0.0, 10.0, 10.0],
            state,
            region: Some(Region {
                kind: "page".into(),
                label: "tab bar, item 3".into(),
                selector: None,
                role: None,
                rect: None,
                lines: None,
            }),
            hw: Some("no third tab".into()),
            sent_at: 1_795_570_300_000,
            blob: true,
            baked: false,
        };
        let marks = vec![
            mark(10, MarkState::Answered),
            mark(11, MarkState::Answered),
            mark(12, MarkState::Answered),
            mark(13, MarkState::Sent),
        ];
        let mut aliases = std::collections::BTreeMap::new();
        for (n, a) in [(1, "c1"), (2, "c2"), (3, "c3"), (4, "c4"), (5, "c5"), (6, "l1"), (9, "f1"), (10, "m1"), (11, "m2"), (12, "m3"), (13, "m4")] {
            aliases.insert(id(n), a.to_owned());
        }
        Snapshot {
            cards: vec![c1, c2, c3, c4, c5],
            links: vec![link],
            frames: vec![frame],
            strokes: vec![],
            marks,
            aliases,
        }
    }

    #[test]
    fn golden_outline() {
        let want = include_str!("../../tests/fixtures/whiteboard/outline.txt");
        let (text, counts) = render(DOC, "Attention inbox", &fixture());
        assert_eq!(format!("{text}\n"), want);
        assert_eq!(counts, Counts { cards: 5, links: 1, marks: 1 });
    }

    #[test]
    fn rows_reading_order() {
        let b = |id: &str, x: f64, y: f64, h: f64| (id.to_owned(), [x, y, 100.0, h]);
        // A tall card's row takes a shorter neighbour whose centre is inside it, even if a bit lower;
        // a card whose centre falls below starts the next row.
        let r = rows(&[
            b("c3", 300.0, 20.0, 100.0),
            b("c1", 0.0, 0.0, 200.0),
            b("c2", 150.0, 90.0, 100.0),
            b("c4", 0.0, 250.0, 50.0),
            b("c5", -50.0, 260.0, 30.0),
        ]);
        assert_eq!(r, vec![vec!["c1", "c2", "c3"], vec!["c5", "c4"]]);
        assert!(rows(&[]).is_empty());
    }

    #[test]
    fn quote_unquote() {
        for (s, max) in [("plain", 80), ("a \"quoted\" \\ back", 80), ("line\nbreak", 80), ("é ünïcode", 3)] {
            let q = quote(s, max);
            let tail = format!("{q} · tail");
            let (back, rest) = unquote(&tail).unwrap();
            assert_eq!(rest, " · tail");
            if s.chars().count() <= max {
                assert_eq!(back, s.replace('\n', " "));
            } else {
                assert_eq!(back.chars().count(), max);
                assert!(back.ends_with('…'));
            }
        }
        assert_eq!(quote("a\"b", 80), "\"a\\\"b\"");
        assert_eq!(quote(&"x".repeat(81), 80).chars().count(), 82);
        assert!(unquote("\"unterminated").is_none());
        assert!(unquote("no quote").is_none());
    }

    #[test]
    fn export_drops_the_wrapper() {
        let (text, _) = render(DOC, "Attention inbox", &fixture());
        let e = export_text("Attention inbox", &text);
        assert!(e.starts_with("# Attention inbox\n\nframe f1 \"Variants\""));
        assert!(!e.contains("<whiteboard"));
        assert!(e.ends_with("handwriting: \"no third tab\"\n"));
    }
}
