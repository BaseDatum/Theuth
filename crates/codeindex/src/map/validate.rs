//! Validation of every generator reply (147 §6.6, in order): JSON → stage
//! struct (`deny_unknown_fields`), Mermaid → `Graph` via
//! `surface::graph::from_mermaid`, caps, anchors, edge evidence, perspective
//! choice, edge labels that say *why* and never *how many*. Errors are
//! located lines (`path · what · expected`, the `<surface_error>` shape of
//! 30 §5) for one repair message; a second failure keeps the structure view.
//!
//! owner: W2 — design §6.6

use serde::{Deserialize, Serialize};

use super::{GraphProps, not_built};

/// Description cap in words (§6.6 rule 3).
pub const DESCRIPTION_WORDS: usize = 120;
/// Edge-label cap in words (§6.6 rule 3).
pub const EDGE_LABEL_WORDS: usize = 4;
/// Edges per diagram ≤ this × nodes (§6.6 rule 3).
pub const EDGES_PER_NODE: usize = 2;
/// Label fragments that mean a count (§6.6 rule 7; a digit counts too).
pub const COUNT_WORDS: [&str; 6] = ["N", "deps", "imports", "refs", "calls", "×"];

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

impl std::fmt::Display for Located {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} · {} · {}", self.path, self.what, self.expected)
    }
}

/// Rule 7: a label with a digit or a [`COUNT_WORDS`] token, or over
/// [`EDGE_LABEL_WORDS`] words, is an error ("counts are rendered from
/// evidence; write the reason").
pub fn check_edge_label(path: &str, label: &str) -> anyhow::Result<Option<Located>> {
    let _ = (path, label);
    Err(not_built("validate::check_edge_label", 2))
}

/// Rules 2–3 on a parsed diagram: node ids ⊆ `issued` handles, ≤
/// [`super::DIAGRAM_NODES_HARD`] nodes (13–20 = a warning in provenance),
/// edges ≤ [`EDGES_PER_NODE`] × nodes, labels within the surface caps. The
/// returned list is empty when valid.
pub fn check_diagram(g: &GraphProps, issued: &[String]) -> anyhow::Result<Vec<Located>> {
    let _ = (g, issued);
    Err(not_built("validate::check_diagram", 2))
}

/// The repair user message: the previous output then one line per error.
pub fn repair_message(previous: &str, errors: &[Located]) -> anyhow::Result<String> {
    let _ = (previous, errors);
    Err(not_built("validate::repair_message", 2))
}
