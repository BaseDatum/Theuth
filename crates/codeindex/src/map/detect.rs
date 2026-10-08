//! Detectors (147 §5.2 bindings/process edges, §7 perspective eligibility):
//! indexed queries over `symbols.name`, `symbols.signature`,
//! `imports.specifier`, `files.path` and `refs.name` — never a file read.
//! Each hit carries a confidence (`import` 0.9, `name-pattern` 0.6) and the
//! perspective's eligibility is the §7 threshold.
//!
//! owner: W1 — design §5.2 (inferred edges), §5.3, §7

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use super::{MapEvidence, not_built};

/// omm's twelve perspectives (§7), catalogue order; `overall-architecture`
/// is always eligible and always chosen.
pub const PERSPECTIVES: [&str; 12] = [
    "overall-architecture",
    "request-lifecycle",
    "data-flow",
    "dependency-map",
    "external-integrations",
    "state-transitions",
    "route-page-map",
    "command-surface",
    "extension-points",
    "pipeline",
    "orchestration",
    "storage",
];

/// The flow perspectives that get one stage-D call per entry point (§6.2).
pub const FLOW_PERSPECTIVES: [&str; 4] = [
    "request-lifecycle",
    "data-flow",
    "pipeline",
    "orchestration",
];

/// Confidence of an import-specifier hit.
pub const CONFIDENCE_IMPORT: f64 = 0.9;
/// Confidence of a name-pattern hit.
pub const CONFIDENCE_NAME: f64 = 0.6;
/// Samples kept per detector.
pub const SAMPLES: usize = 3;

/// One perspective's detector summary: the facts stage A receives and
/// `perspectives.eligible` stores (§5.3).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Detection {
    /// A [`PERSPECTIVES`] kind.
    pub kind: String,
    /// The §7 threshold is met.
    pub eligible: bool,
    /// Matching rows.
    pub hits: u32,
    /// `path#L`, ≤ [`SAMPLES`].
    pub samples: Vec<String>,
    /// Best confidence among the hits.
    pub confidence: f64,
}

/// A unit→unit edge a detector inferred (bindings, process/socket); drawn
/// solid with the detector named, unlike a model-inferred edge.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DetectedEdge {
    /// Source unit path.
    pub from: String,
    /// Target unit path.
    pub to: String,
    /// `MapEvidence::Bindings` or `MapEvidence::Process`.
    pub evidence: MapEvidence,
}

/// Run the twelve perspective detectors over an index pool; one
/// [`Detection`] per [`PERSPECTIVES`] entry, catalogue order.
pub async fn perspectives(pool: &SqlitePool) -> anyhow::Result<Vec<Detection>> {
    let _ = pool;
    Err(not_built("detect::perspectives", 1))
}

/// The generated-bindings and process/socket edges of §5.2 between the given
/// unit paths (root-relative, no trailing slash).
pub async fn inferred_edges(
    pool: &SqlitePool,
    units: &[String],
) -> anyhow::Result<Vec<DetectedEdge>> {
    let _ = (pool, units);
    Err(not_built("detect::inferred_edges", 1))
}
