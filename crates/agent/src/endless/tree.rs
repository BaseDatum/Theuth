//! 137 §5.1: the summary tree. `node(0, i)` is unit *i*; `node(l, i)` merges
//! `(l−1, 2i)` and `(l−1, 2i+1)` and covers units `[i·2^l, (i+1)·2^l)`,
//! addressed as `id+n` with `id = i·2^l`, `n = 2^l`. Pure; the store rows
//! are [`Node`] with `session_id` and `created_at` beside them.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A unit's kind (§4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// The user's words.
    User,
    /// The agent's reply text.
    Talk,
    /// A tool call: `fn_name` + compact JSON arguments.
    Tool,
    /// A tool result (capped, §4).
    Echo,
    /// A subagent's report.
    Work,
    /// Imported memory (reserved).
    Note,
}

impl Kind {
    /// The stored string form (also the tag in a line: `kind: text`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Talk => "talk",
            Self::Tool => "tool",
            Self::Echo => "echo",
            Self::Work => "work",
            Self::Note => "note",
        }
    }
}

impl std::str::FromStr for Kind {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        Ok(match s {
            "user" => Self::User,
            "talk" => Self::Talk,
            "tool" => Self::Tool,
            "echo" => Self::Echo,
            "work" => Self::Work,
            "note" => Self::Note,
            _ => return Err(format!("unknown unit kind: {s:?}")),
        })
    }
}

/// One unit (§4): a part of one `messages` row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unit {
    /// 0-based index in the session, assigned once.
    pub i: u64,
    /// The row.
    pub seq: i64,
    /// Part index within the row.
    pub part: u16,
    /// Kind of the part.
    pub kind: Kind,
    /// The unit's text (without the `kind: ` tag).
    pub text: String,
    /// UTF-8 bytes of `kind + ": " + text`.
    pub size: u32,
    /// The row's time.
    pub at: Option<DateTime<Utc>>,
}

impl Unit {
    /// `kind: text` — what the compactor is shown and what `zoom(id, 1)` returns.
    pub fn tagged(&self) -> String {
        format!("{}: {}", self.kind.as_str(), self.text)
    }
}

/// One built line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    /// Level (`n = 2^l`).
    pub l: u8,
    /// Index within the level.
    pub i: u64,
    /// The summary line.
    pub text: String,
    /// UTF-8 bytes of `text`.
    pub size: u32,
    /// Verbatim unit or `a + "\n" + b` that fit: no model call.
    pub free: bool,
    /// Compactor model id; `None` for a free node.
    pub model: Option<String>,
    /// Attempts used; negative when no candidate passed every guard (§6).
    pub tries: i32,
    /// 137G: a mechanical excerpt (`endless_fold`, G7), not a model summary.
    pub mechanical: bool,
}

/// `(id, n)` of `(l, i)`.
pub fn id_n(l: u8, i: u64) -> (u64, u64) {
    let n = 1u64 << l;
    (i * n, n)
}

/// `(l, i)` of a valid `id+n` (n a power of two, `id % n == 0`), else `None`.
pub fn addr(id: u64, n: u64) -> Option<(u8, u64)> {
    if n == 0 || !n.is_power_of_two() || !id.is_multiple_of(n) {
        return None;
    }
    Some((n.trailing_zeros() as u8, id / n))
}

/// Levels a tree over `t` units can have: `(l, i)` exists iff `i·2^l < t`
/// AND `(i+1)·2^l ≤ t` — a node needs its full span (a right-edge node with
/// a missing sibling is not a node; its left child stands in the view).
pub fn exists(l: u8, i: u64, t: u64) -> bool {
    let (id, n) = id_n(l, i);
    id < t && id.saturating_add(n) <= t
}

/// The in-memory tree of one session.
#[derive(Clone, Debug, Default)]
pub struct Tree {
    nodes: BTreeMap<(u8, u64), Node>,
}

impl Tree {
    /// A tree from stored nodes.
    pub fn from_nodes(nodes: impl IntoIterator<Item = Node>) -> Self {
        Self {
            nodes: nodes.into_iter().map(|n| ((n.l, n.i), n)).collect(),
        }
    }

    /// The node at `(l, i)`, if built.
    pub fn get(&self, l: u8, i: u64) -> Option<&Node> {
        self.nodes.get(&(l, i))
    }

    /// Whether `(l, i)` is built.
    pub fn built(&self, l: u8, i: u64) -> bool {
        self.nodes.contains_key(&(l, i))
    }

    /// Add or replace a node.
    pub fn insert(&mut self, node: Node) {
        self.nodes.insert((node.l, node.i), node);
    }

    /// Built nodes.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// No node built yet.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Every built node, by `(l, i)`.
    pub fn iter(&self) -> impl Iterator<Item = &Node> {
        self.nodes.values()
    }

    /// Level-0 nodes built, for progress.
    pub fn built_at(&self, l: u8) -> u64 {
        self.nodes.range((l, 0)..(l, u64::MAX)).count() as u64
    }
}

/// A unit that fits is its own node (§5.1).
pub fn free_unit(unit: &Unit, node_bytes: u32) -> Option<String> {
    let line = unit.tagged();
    (line.len() <= node_bytes as usize).then_some(line)
}

/// Two children whose `a + "\n" + b` fits merge for free (§5.1).
pub fn free_merge(a: &str, b: &str, node_bytes: u32) -> Option<String> {
    (a.len() + 1 + b.len() <= node_bytes as usize).then(|| format!("{a}\n{b}"))
}

/// 137G mechanical fold: the unit's tagged text, whitespace collapsed, cut to
/// at most `node_bytes` as head ⅔ + ` … ` + tail ⅓ (deterministic, no model).
/// `None` when the unit is a free node anyway.
pub fn mechanical_unit(unit: &Unit, node_bytes: u32) -> Option<String> {
    const GAP: &str = " … ";
    if free_unit(unit, node_bytes).is_some() {
        return None;
    }
    let flat = unit
        .tagged()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let max = node_bytes as usize;
    if flat.len() <= max {
        return Some(flat);
    }
    let room = max.saturating_sub(GAP.len());
    let head = super::guards::cut_utf8(&flat, room * 2 / 3);
    let mut from = flat.len() - (room - head.len());
    while !flat.is_char_boundary(from) {
        from += 1;
    }
    Some(format!("{head}{GAP}{}", &flat[from..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addressing_round_trips_and_rejects() {
        assert_eq!(id_n(0, 7), (7, 1));
        assert_eq!(id_n(3, 2), (16, 8));
        assert_eq!(addr(16, 8), Some((3, 2)));
        assert_eq!(addr(7, 1), Some((0, 7)));
        assert_eq!(addr(12, 8), None, "id % n != 0");
        assert_eq!(addr(8, 6), None, "n not a power of two");
        assert_eq!(addr(0, 0), None);
        assert!(exists(3, 0, 8));
        assert!(!exists(3, 0, 7), "span past T");
        assert!(exists(0, 6, 7));
    }

    #[test]
    fn free_nodes_fit_exactly() {
        let u = Unit {
            i: 0,
            seq: 0,
            part: 0,
            kind: Kind::User,
            text: "x".repeat(506),
            size: 512,
            at: None,
        };
        assert_eq!(free_unit(&u, 512).unwrap().len(), 512);
        let big = Unit {
            text: "x".repeat(507),
            ..u
        };
        assert!(free_unit(&big, 512).is_none());
        assert_eq!(free_merge("a", "b", 3).as_deref(), Some("a\nb"));
        assert!(free_merge("ab", "b", 3).is_none());
    }
}
