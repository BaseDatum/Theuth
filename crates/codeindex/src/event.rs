//! Bus events (73 §5.1): one `CodeToolRan` per `code_*` call and one
//! `CodeReconciled` per index write, coalesced to ≥ 250 ms per root. Rows
//! never ride events; the lenses fetch them (`lens.rs`).

use std::path::PathBuf;

use runtime::SessionId;
use serde::{Deserialize, Serialize};
use tools::ToolOutput;

use crate::index::Reconciled;

/// Paths listed on a `CodeReconciled` before `more` counts the rest.
pub const RECONCILED_PATHS: usize = 200;

/// Wire events of the codeindex plugin; `type` is the variant name.
// `CodeToolRan` carries the summary inline: one allocation per call is the
// cheaper shape for an event the bridge serialises once.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type")]
pub enum CodeindexEvent {
    /// A `code_*` tool ran for `session` on `root` (errors too).
    CodeToolRan {
        /// `ToolCx.session`: scoped by `subscribe_events`.
        session: SessionId,
        /// Canonical root, as `Envelope.root`.
        root: PathBuf,
        /// `ToolCx.call_id`.
        call_id: String,
        /// `code_map | code_find | code_outline | code_refs | code_symbol | code_impact`.
        tool: String,
        /// The tool's arguments as received, minus `budget` and `root`.
        args: serde_json::Value,
        /// Compact envelope of the result.
        summary: CodeToolSummary,
        /// Per-root monotonic sequence shared with `CodeReconciled` and lens replies.
        #[cfg_attr(feature = "ts", ts(type = "number"))]
        seq: u64,
    },
    /// Files of `root` were re-parsed or removed (inline, background or `touch`).
    /// No `session`: global; the UI filters by root.
    CodeReconciled {
        /// Canonical root.
        root: PathBuf,
        /// Root-relative paths written, ≤ [`RECONCILED_PATHS`].
        reparsed: Vec<String>,
        /// Root-relative paths deleted, ≤ [`RECONCILED_PATHS`].
        removed: Vec<String>,
        /// Paths beyond the two caps.
        #[serde(default, skip_serializing_if = "is_zero")]
        more: usize,
        /// A background task is still writing.
        stale: bool,
        /// Files it still has to write.
        pending: usize,
        /// The reconcile guard fired (60 §8).
        suspect_root: bool,
        /// Why, when `suspect_root`.
        hint: Option<String>,
        /// `meta.git_head`, when the root is a repository.
        git_head: Option<String>,
        /// Per-root monotonic sequence.
        #[cfg_attr(feature = "ts", ts(type = "number"))]
        seq: u64,
    },
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// `CodeToolRan.summary`: the envelope's freshness fields plus the one thing
/// the call was about (73 §5.1).
#[derive(Clone, Debug, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct CodeToolSummary {
    /// Rows the model got (the tool's main list, after the budget cut).
    pub rows: usize,
    /// Rows the budget dropped.
    pub dropped: usize,
    /// `ToolOutput::is_error`.
    pub is_error: bool,
    /// Envelope freshness (60 §8).
    pub stale: bool,
    /// Envelope freshness (60 §8).
    pub pending: usize,
    /// Envelope freshness (60 §8).
    pub truncated_walk: bool,
    /// Envelope freshness (60 §8).
    pub suspect_root: bool,
    /// Envelope freshness (60 §8).
    pub hint: Option<String>,
    /// `code_map` only: `focus` as given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus: Option<Vec<String>>,
    /// `code_outline` only: the outlined file, root-relative.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// `code_refs` / `code_symbol`: the one definition matched; `None` with `candidates`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub definition: Option<SummaryDefinition>,
    /// `code_impact` only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub impact: Option<SummaryImpact>,
}

/// `CodeToolRan.summary.definition`.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SummaryDefinition {
    /// Stable identity (60 §8).
    pub moniker: String,
    /// Root-relative path.
    pub path: String,
    /// 1-based.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub line_start: i64,
    /// 1-based, inclusive.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub line_end: i64,
    /// Name.
    pub name: String,
    /// Kind.
    pub kind: String,
}

/// `CodeToolRan.summary.impact`: the band sizes of one `code_impact`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SummaryImpact {
    /// `files | range`.
    pub mode: String,
    /// `{old, new}` for `range`.
    pub range: Option<SummaryRange>,
    /// As given.
    pub include_tests: bool,
    /// Band sizes.
    pub changed: usize,
    /// Band sizes.
    pub direct: usize,
    /// Band sizes.
    pub transitive: usize,
    /// Band sizes.
    pub tests: usize,
    /// Honesty counters (60 §8).
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub ambiguous_skipped: i64,
    /// Honesty counters (60 §8).
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub unresolved_imports: i64,
}

/// `code_impact`'s `range`.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SummaryRange {
    /// Old side.
    pub old: String,
    /// New side (`WORKTREE` for uncommitted changes).
    pub new: String,
}

impl CodeToolSummary {
    /// Read the compact summary off a tool's output: freshness from the
    /// reconcile, counts and the target from the structured envelope.
    pub(crate) fn of(
        tool: &str,
        args: &serde_json::Value,
        r: &Reconciled,
        out: &anyhow::Result<ToolOutput>,
    ) -> Self {
        let mut s = Self {
            is_error: out.as_ref().map_or(true, |o| o.is_error),
            stale: r.stale,
            pending: r.pending,
            truncated_walk: r.truncated_walk,
            suspect_root: r.suspect_root,
            hint: r.hint.map(str::to_owned),
            ..Self::default()
        };
        if tool == "code_map" {
            s.focus = serde_json::from_value(args["focus"].clone()).ok();
        }
        let Some(v) = out.as_ref().ok().and_then(|o| o.structured.as_ref()) else {
            return s;
        };
        let len = |k: &str| v[k].as_array().map_or(0, Vec::len);
        s.dropped = v["dropped"].as_u64().map_or(0, |n| n as usize);
        s.rows = match tool {
            "code_map" | "code_find" => len("rows"),
            "code_outline" => len("symbols"),
            "code_refs" => len("refs") + len("candidates"),
            "code_symbol" => len("callers") + len("callees"),
            "code_impact" => len("changed") + len("direct") + len("transitive") + len("tests"),
            _ => 0,
        };
        match tool {
            "code_outline" => s.path = v["path"].as_str().map(str::to_owned),
            "code_refs" | "code_symbol" => {
                s.definition = serde_json::from_value(v["definition"].clone()).ok();
            }
            "code_impact" => {
                s.impact = Some(SummaryImpact {
                    mode: v["mode"].as_str().unwrap_or_default().to_owned(),
                    range: serde_json::from_value(v["range"].clone()).ok(),
                    include_tests: v["include_tests"].as_bool().unwrap_or(false),
                    changed: len("changed"),
                    direct: len("direct"),
                    transitive: len("transitive"),
                    tests: len("tests"),
                    ambiguous_skipped: v["ambiguous_skipped"].as_i64().unwrap_or(0),
                    unresolved_imports: v["unresolved_imports"].as_i64().unwrap_or(0),
                });
            }
            _ => {}
        }
        s
    }
}

/// The tool's args for the event: the JSON object minus `budget`, `root`
/// and absent optionals.
pub(crate) fn event_args<A: Serialize>(a: &A) -> serde_json::Value {
    let mut v = serde_json::to_value(a).unwrap_or(serde_json::Value::Null);
    if let Some(m) = v.as_object_mut() {
        m.retain(|k, v| k != "budget" && k != "root" && !v.is_null());
    }
    v
}
