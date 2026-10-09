//! Validation of every generator reply (147 §6.6, in order): JSON → stage
//! struct (unknown keys ignored, stray top-level prose folded into `fields`),
//! Mermaid → `Graph` via `surface::graph::from_mermaid`, caps, anchors, edge evidence, perspective
//! choice, edge labels that say *why* and never *how many*. Errors are
//! located lines (`path · what · expected`, the `<surface_error>` shape of
//! 30 §5) for one repair message; a second failure keeps the structure view.
//!
//! owner: W2 — design §6.6

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::{DIAGRAM_NODES_HARD, GraphProps};

/// Description cap in words (§6.6 rule 3).
pub const DESCRIPTION_WORDS: usize = 120;
/// Edge-label cap in words (§6.6 rule 3).
pub const EDGE_LABEL_WORDS: usize = 4;
/// Edges per diagram ≤ this × nodes (§6.6 rule 3).
pub const EDGES_PER_NODE: usize = 2;
/// Label fragments that mean a count (§6.6 rule 7; a digit counts too).
pub const COUNT_WORDS: [&str; 6] = ["N", "deps", "imports", "refs", "calls", "×"];
/// Rule 7's repair line.
pub const COUNTS_FROM_EVIDENCE: &str = "counts are rendered from evidence; write the reason";

/// One validation error, located for the repair message.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Located {
    /// JSON pointer or `mermaid:<line>:<col>`.
    pub path: String,
    /// What is wrong.
    pub what: String,
    /// What was expected.
    pub expected: String,
}

impl Located {
    /// Shorthand for the three strings.
    pub fn new(
        path: impl Into<String>,
        what: impl Into<String>,
        expected: impl Into<String>,
    ) -> Self {
        Self {
            path: path.into(),
            what: what.into(),
            expected: expected.into(),
        }
    }
}

impl std::fmt::Display for Located {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} · {} · {}", self.path, self.what, self.expected)
    }
}

impl From<surface::graph::MermaidError> for Located {
    fn from(e: surface::graph::MermaidError) -> Self {
        Self {
            path: format!("mermaid:{}:{}", e.line, e.col),
            what: e.what,
            expected: e.expected,
        }
    }
}

/// Whitespace-separated words.
pub fn words(text: &str) -> usize {
    text.split_whitespace().count()
}

/// Rule 7: a label with a digit or a [`COUNT_WORDS`] token, or over
/// [`EDGE_LABEL_WORDS`] words, is an error ("counts are rendered from
/// evidence; write the reason").
pub fn check_edge_label(path: &str, label: &str) -> anyhow::Result<Option<Located>> {
    let label = label.trim();
    if label.is_empty() {
        return Ok(Some(Located::new(
            path,
            "edge has no label",
            "why the edge exists, in at most four words",
        )));
    }
    if label.chars().any(|c| c.is_ascii_digit()) {
        return Ok(Some(Located::new(
            path,
            format!("label {label:?} carries a number"),
            COUNTS_FROM_EVIDENCE,
        )));
    }
    let counts = label
        .split(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | '(' | ')' | '·' | '/'))
        .filter(|w| !w.is_empty())
        .any(|w| {
            COUNT_WORDS.iter().any(|cw| {
                if *cw == "N" || *cw == "×" {
                    w == *cw
                } else {
                    w.eq_ignore_ascii_case(cw)
                }
            })
        });
    if counts || label.contains('×') {
        return Ok(Some(Located::new(
            path,
            format!("label {label:?} names a count"),
            COUNTS_FROM_EVIDENCE,
        )));
    }
    if words(label) > EDGE_LABEL_WORDS {
        return Ok(Some(Located::new(
            path,
            format!("label {label:?} has {} words", words(label)),
            format!("at most {EDGE_LABEL_WORDS} words"),
        )));
    }
    if label.chars().count() > surface::MAX_GRAPH_LABEL_CHARS {
        return Ok(Some(Located::new(
            path,
            format!("label is {} characters", label.chars().count()),
            format!("at most {} characters", surface::MAX_GRAPH_LABEL_CHARS),
        )));
    }
    Ok(None)
}

/// Rule 3's description cap: `None` when within [`DESCRIPTION_WORDS`].
pub fn check_description(path: &str, text: &str) -> Option<Located> {
    let n = words(text);
    if text.trim().is_empty() {
        return Some(Located::new(
            path,
            "description is empty",
            "2–5 sentences saying what it is and which paths it covers",
        ));
    }
    (n > DESCRIPTION_WORDS).then(|| {
        Located::new(
            path,
            format!("description has {n} words"),
            format!("at most {DESCRIPTION_WORDS} words"),
        )
    })
}

/// Rules 2–3 on a parsed diagram: node ids ⊆ `issued` handles, each once, ≤
/// [`super::DIAGRAM_NODES_HARD`] nodes (13–20 = a warning in provenance),
/// edges ≤ [`EDGES_PER_NODE`] × nodes between declared nodes, node labels
/// within the surface cap. Edge labels are rule 7's, checked by the stage
/// validators through [`check_edge_label`]. The list is empty when valid.
pub fn check_diagram(g: &GraphProps, issued: &[String]) -> anyhow::Result<Vec<Located>> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for (i, n) in g.nodes.iter().enumerate() {
        let path = format!("/mermaid/nodes/{i}");
        if !issued.contains(&n.id) {
            out.push(Located::new(
                &path,
                format!("node id `{}` was not issued", n.id),
                format!("one of {}", list(issued)),
            ));
        }
        if !seen.insert(n.id.as_str()) {
            out.push(Located::new(
                &path,
                format!("node `{}` declared twice", n.id),
                "each handle once",
            ));
        }
        if n.label.chars().count() > surface::MAX_GRAPH_LABEL_CHARS {
            out.push(Located::new(
                &path,
                format!("label is {} characters", n.label.chars().count()),
                format!("at most {} characters", surface::MAX_GRAPH_LABEL_CHARS),
            ));
        }
    }
    if g.nodes.is_empty() {
        out.push(Located::new(
            "/mermaid",
            "the diagram has no nodes",
            "at least one node",
        ));
    }
    if g.nodes.len() > DIAGRAM_NODES_HARD {
        out.push(Located::new(
            "/mermaid",
            format!("{} nodes", g.nodes.len()),
            format!(
                "at most {} (aim for {}); merge or bundle the rest",
                DIAGRAM_NODES_HARD,
                super::DIAGRAM_NODES
            ),
        ));
    }
    if g.edges.len() > EDGES_PER_NODE * g.nodes.len().max(1) {
        out.push(Located::new(
            "/mermaid",
            format!("{} edges for {} nodes", g.edges.len(), g.nodes.len()),
            format!("at most {EDGES_PER_NODE} edges per node; draw the ones that explain"),
        ));
    }
    for (i, e) in g.edges.iter().enumerate() {
        let path = format!("/mermaid/edges/{i}");
        for end in [&e.from, &e.to] {
            if !g.nodes.iter().any(|n| n.id == *end) {
                out.push(Located::new(
                    &path,
                    format!("edge endpoint `{end}` is not a node"),
                    "both ends declared nodes",
                ));
            }
        }
        // Edge labels: the stage validators check the effective label (a JSON
        // override wins over the mermaid text) with `check_edge_label`.
    }
    Ok(out)
}

/// The repair user message: the original facts message (schema and example
/// included — the model must not answer from the previous text alone), the
/// previous output, then one line per error.
pub fn repair_message(facts: &str, previous: &str, errors: &[Located]) -> anyhow::Result<String> {
    anyhow::ensure!(!errors.is_empty(), "repair_message without errors");
    let mut out = String::with_capacity(facts.len() + previous.len() + 320);
    out.push_str(
        "Your previous reply to the request below did not validate. Answer the request again: \
         return the corrected JSON object and nothing else — same schema, same handles, \
         every error fixed.\n\n<request>\n",
    );
    out.push_str(facts.trim());
    out.push_str("\n</request>\n\n<previous>\n");
    out.push_str(previous.trim());
    out.push_str("\n</previous>\n\n<errors>\n");
    for e in errors {
        out.push_str(&e.to_string());
        out.push('\n');
    }
    out.push_str("</errors>\n");
    Ok(out)
}

fn list(ids: &[String]) -> String {
    let mut s = ids
        .iter()
        .take(12)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    if ids.len() > 12 {
        s.push_str(", …");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use surface::{GraphEdge, GraphNode};

    fn node(id: &str) -> GraphNode {
        GraphNode {
            id: id.into(),
            label: id.into(),
            kind: None,
            group: None,
            state: None,
            r#ref: None,
            sub: None,
            tag: None,
            icon: None,
        }
    }

    fn edge(from: &str, to: &str, label: &str) -> GraphEdge {
        GraphEdge {
            from: from.into(),
            to: to.into(),
            label: Some(label.into()),
            style: None,
            state: None,
            arrow: None,
        }
    }

    #[test]
    fn labels_with_counts_are_rejected() {
        for bad in [
            "4 deps",
            "mesh · 4 deps",
            "N imports",
            "shared refs",
            "Calls into",
            "a × b",
            "replica change feeds and mesh ids",
        ] {
            let l = check_edge_label("/e", bad).unwrap();
            assert!(l.is_some(), "{bad} should be rejected");
        }
        assert_eq!(
            check_edge_label("/e", "4 deps").unwrap().unwrap().expected,
            COUNTS_FROM_EVIDENCE
        );
        for ok in [
            "board anchors",
            "sends user input",
            "spawns the relay",
            "prefs",
        ] {
            assert_eq!(check_edge_label("/e", ok).unwrap(), None, "{ok}");
        }
        assert!(check_edge_label("/e", "").unwrap().is_some());
    }

    #[test]
    fn diagram_caps_and_handles() {
        let issued: Vec<String> = (1..=25).map(|i| format!("g{i}")).collect();
        let mut g = GraphProps {
            nodes: vec![node("g1"), node("g2"), node("zz")],
            edges: vec![edge("g1", "g2", "why"), edge("g1", "g3", "why")],
            groups: None,
            direction: None,
            legend: None,
            title: None,
        };
        let errs = check_diagram(&g, &issued).unwrap();
        assert_eq!(errs.len(), 2, "{errs:#?}");
        assert!(errs[0].what.contains("zz"));
        assert!(errs[1].what.contains("g3"));

        g.nodes = (1..=21).map(|i| node(&format!("g{i}"))).collect();
        g.edges.clear();
        let errs = check_diagram(&g, &issued).unwrap();
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].what, "21 nodes");

        g.nodes = vec![node("g1"), node("g2")];
        g.edges = (0..5).map(|_| edge("g1", "g2", "why")).collect();
        let errs = check_diagram(&g, &issued).unwrap();
        assert_eq!(errs[0].what, "5 edges for 2 nodes");

        let msg = repair_message("{\"stage\":\"top\"}", "{}", &errs).unwrap();
        assert!(msg.contains("<request>\n{\"stage\":\"top\"}\n</request>"));
        assert!(msg.contains("<previous>\n{}\n</previous>"));
        assert!(msg.contains("/mermaid · 5 edges for 2 nodes · "));
    }
}
