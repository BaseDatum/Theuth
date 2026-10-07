//! `Graph` on text surfaces (design doc 65 §3.3): the lowering to a mermaid flowchart that
//! `merman` draws in the TUI, and the adjacency list every text channel (and a too-narrow TUI)
//! falls back to.

use std::collections::BTreeMap;

use markdown::Line;
use unicode_width::UnicodeWidthStr;

use crate::render::{BOLD, DIM, PLAIN, span};
use crate::spec::{GraphDirection, GraphEdgeStyle, GraphKind, GraphProps, GraphState};

/// Mermaid keywords an id must not be; they get a `_` suffix.
const RESERVED: [&str; 11] = [
    "end",
    "subgraph",
    "graph",
    "flowchart",
    "direction",
    "style",
    "class",
    "classDef",
    "click",
    "participant",
    "actor",
];

/// `raw` ids as mermaid identifiers: `[A-Za-z0-9_]` only (others become `_`), keywords
/// suffixed, collisions disambiguated with `_2`, `_3`, … in first-seen order.
pub(crate) fn mermaid_ids<'a>(raw: impl IntoIterator<Item = &'a str>) -> BTreeMap<&'a str, String> {
    let mut out = BTreeMap::new();
    let mut taken = std::collections::BTreeSet::new();
    for id in raw {
        if out.contains_key(id) {
            continue;
        }
        let mut base: String = id
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        if base.is_empty() || RESERVED.contains(&base.as_str()) {
            base.push('_');
        }
        let mut candidate = base.clone();
        let mut n = 2;
        while !taken.insert(candidate.clone()) {
            candidate = format!("{base}_{n}");
            n += 1;
        }
        out.insert(id, candidate);
    }
    out
}

/// A label inside `"…"`: the quote and angle brackets as mermaid entity codes.
pub(crate) fn quoted(label: &str) -> String {
    let escaped = label
        .replace('"', "#quot;")
        .replace('<', "#lt;")
        .replace('>', "#gt;");
    format!("\"{escaped}\"")
}

/// `id["label"]` in the shape `kind` asks for. `actor` is a circle: merman 0.7 (the only consumer
/// of this text) does not draw mermaid's `[/ /]` parallelogram, and a graph that never draws
/// on the TUI is worse than a round actor.
fn node_decl(id: &str, label: &str, kind: Option<GraphKind>) -> String {
    let l = quoted(label);
    match kind.unwrap_or(GraphKind::Box) {
        GraphKind::Box => format!("{id}[{l}]"),
        GraphKind::Rounded => format!("{id}({l})"),
        GraphKind::Pill => format!("{id}([{l}])"),
        GraphKind::Diamond => format!("{id}{{{l}}}"),
        GraphKind::Cylinder => format!("{id}[({l})]"),
        GraphKind::Actor | GraphKind::Circle => format!("{id}(({l}))"),
        GraphKind::Hexagon => format!("{id}{{{{{l}}}}}"),
        GraphKind::Note => format!("{id}>{l}]"),
    }
}

/// The graph as `flowchart LR|TB` source: one `subgraph` per group with its members declared
/// inside, ungrouped nodes after, then the edges.
pub fn to_mermaid(p: &GraphProps) -> String {
    let ids = mermaid_ids(
        p.groups
            .iter()
            .flatten()
            .map(|g| g.id.as_str())
            .chain(p.nodes.iter().map(|n| n.id.as_str())),
    );
    let id = |raw: &str| ids.get(raw).map(String::as_str).unwrap_or("_");
    let dir = match p.direction.unwrap_or(GraphDirection::LR) {
        GraphDirection::LR => "LR",
        GraphDirection::TB => "TB",
    };
    let mut out = format!("flowchart {dir}\n");
    let groups = p.groups.as_deref().unwrap_or_default();
    for g in groups {
        out.push_str(&format!("  subgraph {}[{}]\n", id(&g.id), quoted(&g.label)));
        for n in p.nodes.iter().filter(|n| n.group.as_deref() == Some(&g.id)) {
            out.push_str(&format!("    {}\n", node_decl(id(&n.id), &n.label, n.kind)));
        }
        out.push_str("  end\n");
    }
    for n in p.nodes.iter().filter(|n| {
        !n.group
            .as_deref()
            .is_some_and(|g| groups.iter().any(|x| x.id == g))
    }) {
        out.push_str(&format!("  {}\n", node_decl(id(&n.id), &n.label, n.kind)));
    }
    for e in &p.edges {
        let arrow = match e.style.unwrap_or(GraphEdgeStyle::Solid) {
            GraphEdgeStyle::Solid => "-->",
            GraphEdgeStyle::Dashed => "-.->",
        };
        match &e.label {
            Some(l) if !l.is_empty() => out.push_str(&format!(
                "  {} {arrow}|{}| {}\n",
                id(&e.from),
                quoted(l),
                id(&e.to)
            )),
            _ => out.push_str(&format!("  {} {arrow} {}\n", id(&e.from), id(&e.to))),
        }
    }
    out
}

/// The state glyph in front of a node line.
fn glyph(state: Option<GraphState>) -> &'static str {
    match state.unwrap_or(GraphState::Default) {
        GraphState::Active => "◉",
        GraphState::Done => "●",
        GraphState::Muted => "○",
        GraphState::Danger => "✗",
        GraphState::Default => "•",
    }
}

/// `text` cut to `width` columns with a trailing `…` (0 = unbounded).
pub(crate) fn truncate(text: String, width: usize) -> String {
    if width == 0 || text.width() <= width {
        return text;
    }
    let mut out = String::new();
    let mut w = 0;
    for c in text.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if w + cw > width.saturating_sub(1) {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push('…');
    out
}

/// The graph as text: each group's label as a bold header over its indented members (a nested
/// group indents one level deeper than its parent, 123 §4.4), ungrouped nodes after; one line
/// per node — `◉ api (:8080) [SERVICE] → db (writes), ✗→ queue` — the state glyph, the label,
/// `sub` in parentheses, `tag` in brackets, then the targets of its outgoing edges with any
/// edge label in parentheses (`✗→` before a `danger` edge's target). Lines are cut to `width`.
pub fn adjacency_lines(p: &GraphProps, width: usize) -> Vec<Line> {
    let label_of: BTreeMap<&str, &str> = p
        .nodes
        .iter()
        .map(|n| (n.id.as_str(), n.label.as_str()))
        .collect();
    let node_line = |n: &crate::spec::GraphNode, indent: usize| -> Line {
        let targets: Vec<String> = p
            .edges
            .iter()
            .filter(|e| e.from == n.id)
            .map(|e| {
                let to = label_of.get(e.to.as_str()).copied().unwrap_or(&e.to);
                let danger = if e.state == Some(GraphState::Danger) {
                    "✗→ "
                } else {
                    ""
                };
                match &e.label {
                    Some(l) if !l.is_empty() => format!("{danger}{to} ({l})"),
                    _ => format!("{danger}{to}"),
                }
            })
            .collect();
        let mut text = format!("{}{} {}", " ".repeat(indent), glyph(n.state), n.label);
        if let Some(sub) = n.sub.as_deref().filter(|s| !s.is_empty()) {
            text.push_str(&format!(" ({sub})"));
        }
        if let Some(tag) = n.tag.as_deref().filter(|t| !t.is_empty()) {
            text.push_str(&format!(" [{tag}]"));
        }
        if !targets.is_empty() {
            text.push_str(" → ");
            text.push_str(&targets.join(", "));
        }
        vec![span(truncate(text, width), PLAIN)]
    };
    let groups = p.groups.as_deref().unwrap_or_default();
    let known = |id: &str| groups.iter().any(|g| g.id == id);
    let mut out = Vec::new();
    // Depth-first over the `parent` links so a nested group sits under its parent, one level in.
    fn group_block(
        g: &crate::spec::GraphGroup,
        depth: usize,
        p: &GraphProps,
        groups: &[crate::spec::GraphGroup],
        node_line: &dyn Fn(&crate::spec::GraphNode, usize) -> Line,
        width: usize,
        out: &mut Vec<Line>,
    ) {
        let indent = " ".repeat(2 * depth);
        out.push(vec![span(
            truncate(format!("{indent}{}", g.label), width),
            BOLD,
        )]);
        for n in p.nodes.iter().filter(|n| n.group.as_deref() == Some(&g.id)) {
            out.push(node_line(n, 2 * depth + 2));
        }
        if depth < crate::MAX_GRAPH_GROUP_DEPTH {
            for c in groups.iter().filter(|c| c.parent.as_deref() == Some(&g.id)) {
                group_block(c, depth + 1, p, groups, node_line, width, out);
            }
        }
    }
    for g in groups
        .iter()
        .filter(|g| !g.parent.as_deref().is_some_and(known))
    {
        group_block(g, 0, p, groups, &node_line, width, &mut out);
    }
    for n in p
        .nodes
        .iter()
        .filter(|n| !n.group.as_deref().is_some_and(known))
    {
        out.push(node_line(n, 0));
    }
    if out.is_empty() {
        out.push(vec![span("(empty graph)", DIM)]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::plain_text;
    use crate::spec::{GraphEdge, GraphGroup, GraphNode};
    use crate::{Component, mermaid_lines, parse};

    macro_rules! fixture {
        ($name:literal) => {
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/fixtures/json-render/",
                $name,
                ".json"
            ))
        };
    }

    const VALID: [(&str, &str); 3] = [
        ("graph-architecture", fixture!("graph-architecture")),
        ("graph-dependency-lr", fixture!("graph-dependency-lr")),
        ("graph-pipeline-tb", fixture!("graph-pipeline-tb")),
    ];

    fn graph(body: &str) -> GraphProps {
        let spec = parse(body).expect("valid fixture").spec;
        spec.elements
            .values()
            .find_map(|e| match &e.component {
                Component::Graph(g) => Some(g.clone()),
                _ => None,
            })
            .expect("a Graph element")
    }

    fn node(id: &str, label: &str, group: Option<&str>) -> GraphNode {
        GraphNode {
            id: id.into(),
            label: label.into(),
            kind: None,
            group: group.map(Into::into),
            state: None,
            r#ref: None,
            sub: None,
            tag: None,
            icon: None,
        }
    }

    /// The lowering is the fixture oracle: every valid fixture draws under merman at 100 columns.
    #[test]
    fn every_valid_fixture_lowers_to_mermaid_that_merman_draws() {
        for (name, body) in VALID {
            let src = to_mermaid(&graph(body));
            let wide = mermaid_lines(&src, 0).unwrap_or_else(|e| panic!("{name}: {e}\n{src}"));
            let text = wide.as_deref().map(plain_text).unwrap_or_default();
            let lines = mermaid_lines(&src, 100)
                .unwrap_or_else(|e| panic!("{name}: {e}\n{src}"))
                .unwrap_or_else(|| panic!("{name}: merman declined or too wide\n{src}\n{text}"));
            assert!(!lines.is_empty(), "{name}");
        }
    }

    #[test]
    fn lowering_shapes_groups_ids_and_escapes() {
        let p = GraphProps {
            nodes: vec![
                GraphNode {
                    kind: Some(GraphKind::Cylinder),
                    ..node("db-main", "sessions.sqlite", Some("store"))
                },
                GraphNode {
                    kind: Some(GraphKind::Diamond),
                    ..node("end", "ok?", None)
                },
                node("db_main", "say \"hi\" <b>", None),
            ],
            edges: vec![
                GraphEdge {
                    from: "end".into(),
                    to: "db-main".into(),
                    label: Some("writes".into()),
                    style: Some(GraphEdgeStyle::Dashed),
                    state: None,
                    arrow: None,
                },
                GraphEdge {
                    from: "db_main".into(),
                    to: "db_main".into(),
                    label: None,
                    style: None,
                    state: None,
                    arrow: None,
                },
            ],
            groups: Some(vec![GraphGroup {
                id: "store".into(),
                label: "Store".into(),
                parent: None,
                kind: None,
            }]),
            direction: Some(GraphDirection::TB),
            legend: None,
            title: None,
        };
        assert_eq!(
            to_mermaid(&p),
            "flowchart TB\n  subgraph store[\"Store\"]\n    db_main[(\"sessions.sqlite\")]\n  end\n  end_{\"ok?\"}\n  db_main_2[\"say #quot;hi#quot; #lt;b#gt;\"]\n  end_ -.->|\"writes\"| db_main\n  db_main_2 --> db_main_2\n"
        );
        assert!(mermaid_lines(&to_mermaid(&p), 0).expect("parses").is_some());
    }

    #[test]
    fn adjacency_list_at_40_columns() {
        let lines = adjacency_lines(&graph(VALID[0].1), 40);
        assert_eq!(
            plain_text(&lines),
            "Edge\n  • Browser → API (HTTPS), CDN\n  ○ CDN → API (origin)\nCore\n  • API → Worker (enqueue), Postgres (r…\n  ◉ Worker → Postgres (writes), Mailer …\n  • Postgres\n✗ Mailer"
        );
        assert!(
            lines
                .iter()
                .all(|l| l.iter().map(|s| s.text.width()).sum::<usize>() <= 40)
        );
        assert!(lines[0][0].style.bold && !lines[1][0].style.bold);
        assert_eq!(
            plain_text(&adjacency_lines(&graph(VALID[0].1), 0))
                .lines()
                .nth(4),
            Some("  • API → Worker (enqueue), Postgres (reads)")
        );
    }
}
