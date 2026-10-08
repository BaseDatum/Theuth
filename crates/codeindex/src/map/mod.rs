//! The CodeMap map (design doc 147): one stored, generated map per indexed
//! root — perspectives → drillable elements → one small `Graph` and a few
//! prose fields each. This file is the contract (147 §4.5 types, §10.3 actor
//! messages); the work lives in the owner modules:
//!
//! - [`store`] — the map's own SQLite beside the index (§9); DDL here, queries W1
//! - [`skeleton`] / [`detect`] — what the index decides, zero tokens (§5, §7); W1
//! - [`generator`] / [`validate`] — what the model decides, validated (§6); W2
//! - [`export`] / [`import`] — omm's `.omm/` layout both ways (§13) and the
//!   `code_overview` text form (§12); W4
//!
//! Every reply carries [`MapMeta`]; ts-rs bindings under `ts` as `lens.rs`.

use std::path::PathBuf;

use runtime::SessionId;
use serde::{Deserialize, Serialize};
pub use surface::GraphProps;

use crate::lens::MapRow;
use crate::profile::Profile;

pub mod detect;
pub mod export;
pub mod generator;
pub mod generator_facts;
pub mod generator_write;
pub mod import;
pub mod skeleton;
pub mod store;
pub mod validate;

/// Nodes per diagram the generator asks for (147 O4, §5.3).
pub const DIAGRAM_NODES: usize = 12;
/// Nodes per diagram accepted with a provenance warning; 21+ is a repair (§5.3).
pub const DIAGRAM_NODES_HARD: usize = 20;
/// Element depth below which the structure is a listing, not a diagram (W7).
pub const MAX_DEPTH: u8 = 3;
/// `CodeMapChanged.elements` cap before `more` counts the rest (§10.1).
pub const CHANGED_ELEMENTS: usize = 200;
/// `MapListing.symbols` cap (§4.5).
pub const LISTING_SYMBOLS: usize = 50;
/// Per-call budget of one generator side call (§6.4).
pub const CALL_BUDGET: std::time::Duration = std::time::Duration::from_secs(120);

/// `cluster:<sha8>` for a model-made group (§4.2): SHA-256 over the member
/// node ids sorted and joined by `\n`, first 8 hex. Keyed by what it
/// contains: renaming changes nothing, re-partitioning changes everything.
pub fn cluster_id(members: &[String]) -> String {
    use sha2::Digest;
    let mut sorted: Vec<&str> = members.iter().map(String::as_str).collect();
    sorted.sort_unstable();
    sorted.dedup();
    let hash = sha2::Sha256::digest(sorted.join("\n").as_bytes());
    format!(
        "cluster:{:02x}{:02x}{:02x}{:02x}",
        hash[0], hash[1], hash[2], hash[3]
    )
}

/// `<perspective>/<node>`: the element id of `node` in `perspective`; the
/// perspective's root element is the perspective id alone, its node
/// `perspective:<id>`.
pub fn element_id(perspective: &str, node: &str) -> String {
    format!("{perspective}/{node}")
}

// --- messages (§10.3) --------------------------------------------------------

/// `ask` → `Result<MapIndex, String>`: perspectives and their trees (§10.2
/// `codemap_overview`). Runs the first skeleton pass inline when the map file
/// is missing; never reconciles, never walks.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Overview {
    /// Indexed root.
    pub root: PathBuf,
}

/// `ask` → `Result<MapElement, String>`: one element with its diagram, prose,
/// edges and children (`codemap_element`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Element {
    /// Indexed root.
    pub root: PathBuf,
    /// Element id `<perspective>/<node id>` (§4.2); the perspective id alone is its root element.
    pub element: String,
}

/// `ask` → `Result<MapJob, String>`: start (or attach to) the root's one
/// generation job, charged to `session` (S1/S2; `codemap_generate`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Generate {
    /// Indexed root.
    pub root: PathBuf,
    /// The opening session — pays, and must be a live native session (C2).
    pub session: SessionId,
    /// `Some` = regenerate that subtree only; `None` = everything stale or without prose.
    pub element: Option<String>,
}

/// `ask` → `Result<MapJob, String>`: abort the root's running job (§6.7;
/// `codemap_cancel`). Stored elements stay.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Cancel {
    /// Indexed root.
    pub root: PathBuf,
}

/// `ask` → `Result<Vec<MapHit>, String>`: elements by name, or by a path →
/// the element chain whose anchors contain it (`codemap_search`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Search {
    /// Indexed root.
    pub root: PathBuf,
    /// A name fragment or a root-relative path.
    pub query: String,
    /// Hits; default [`SEARCH_LIMIT`].
    pub limit: Option<u32>,
}

/// Default `codemap_search` hits.
pub const SEARCH_LIMIT: u32 = 20;

/// `ask` → `Result<MapExport, String>`: write `<root>/.omm/` (S3, §13.1;
/// `codemap_export`). Refuses when a foreign `.omm/` exists (§10.2).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Export {
    /// Indexed root.
    pub root: PathBuf,
}

// --- reply types (§4.5) ------------------------------------------------------

/// Freshness, state and timing on every map reply.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct MapMeta {
    /// Canonical root.
    pub root: PathBuf,
    /// Per-root sequence the reply reflects (shared with `CodeReconciled`).
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub seq: u64,
    /// [`store::MAP_SCHEMA_VERSION`] of the file answering.
    pub schema: u32,
    /// Where the map is between "no index" and "ready".
    pub state: MapState,
    /// The index's `map_state` as `"<files>:<max mtime_ns>"`.
    pub index_state: String,
    /// `meta.git_head`, when the root is a repository.
    pub git_head: Option<String>,
    /// Elements whose facts hash moved since generation (§8).
    pub stale_elements: u32,
    /// The running job, when one exists.
    pub job: Option<MapJob>,
    /// Engine time for this reply.
    pub ms: u32,
}

/// `MapMeta.state`.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum MapState {
    /// No index for the root.
    None,
    /// Skeleton only: structure view, no prose (§5.6).
    Structure,
    /// A job is running.
    Generating,
    /// Every element has prose.
    Ready,
    /// Some elements failed validation twice or were never reached.
    Partial,
}

/// One generation job (§6.1, §6.7): the reply of `codemap_generate` /
/// `codemap_cancel` and `MapMeta.job`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct MapJob {
    /// `jobs.id`, a UUIDv7.
    pub id: String,
    /// The opening session that pays (S1).
    pub session: SessionId,
    /// Unix ms.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub started_ms: u64,
    /// `select | top | flow | describe | done | failed | cancelled` (§10.1).
    pub stage: String,
    /// Elements with prose.
    pub done: u32,
    /// Elements planned.
    pub total: u32,
    /// Side calls made.
    pub calls: u32,
    /// Σ `SideReply` cost so far.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub cost_nanos: u64,
    /// Currency of `cost_nanos`.
    pub currency: String,
    /// Why it stopped, when `failed`.
    pub error: Option<String>,
}

/// Reply of `codemap_overview`: every perspective with its tree.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct MapIndex {
    /// Freshness and state.
    pub meta: MapMeta,
    /// Ordered by `perspectives.order_no`; `overall-architecture` first.
    pub perspectives: Vec<MapPerspective>,
    /// The project in one line (`project_profile`, 60 §8).
    pub profile: ProfileLine,
}

/// `project_profile` cut to the header line of the Map tab.
#[derive(
    Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ProfileLine {
    /// `cargo` | `node` | `python` | `go` | `mixed` | `unknown`.
    pub kind: String,
    /// `cargo` | `npm` | `pnpm` | `bun` | `yarn` | `uv` | `poetry` | `pipenv` | `pip` | `go`.
    pub package_manager: Option<String>,
    /// `cargo test` | `vitest` | `pytest` | ….
    pub test_runner: Option<String>,
    /// Manifests and lockfiles found, root-relative.
    pub manifests: Vec<String>,
}

impl From<&Profile> for ProfileLine {
    fn from(p: &Profile) -> Self {
        Self {
            kind: p.kind.clone(),
            package_manager: p.package_manager.clone(),
            test_runner: p.test_runner.clone(),
            manifests: p.manifests.clone(),
        }
    }
}

/// One perspective (§7) and its element tree.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct MapPerspective {
    /// A catalogue kind (`overall-architecture`, …) or `imported:<dir>`.
    pub id: String,
    /// The catalogue kind; `imported` for a read `.omm/`.
    pub kind: String,
    /// Display title.
    pub title: String,
    /// Who made it.
    pub origin: MapOrigin,
    /// Elements in the tree.
    pub elements: u32,
    /// Stale elements in the tree.
    pub stale: u32,
    /// Pre-order, ≤ depth [`MAX_DEPTH`]; the perspective's root element first.
    pub tree: Vec<MapTreeRow>,
}

/// Who wrote an element or perspective.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum MapOrigin {
    /// The skeleton: basenames and mechanical labels (§5.6).
    Structure,
    /// A validated model pass (§6).
    Generated,
    /// A read-only `<root>/.omm/` (§4.6).
    Imported,
}

impl MapOrigin {
    /// The stored string form (`perspectives.origin`, `elements.origin`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Structure => "structure",
            Self::Generated => "generated",
            Self::Imported => "imported",
        }
    }
}

/// One row of a perspective's tree (sidebar, list form).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct MapTreeRow {
    /// Element id `<perspective>/<node>`.
    pub id: String,
    /// Node id (§4.2): `unit:` | `sym:` | `cluster:` | `ext:` | `bundle:`.
    pub node: String,
    /// Parent element id; `None` for the perspective's root element.
    pub parent: Option<String>,
    /// 0 for the root element.
    pub depth: u8,
    /// Display name.
    pub name: String,
    /// Shortest covering root-relative path, when one exists.
    pub path: Option<String>,
    /// `unit | file | symbol | cluster | external | bundle | perspective`.
    pub kind: String,
    /// Children in the tree.
    pub children: u32,
    /// No diagram: the detail pane lists files and symbols.
    pub leaf: bool,
    /// Facts hash moved since generation.
    pub stale: bool,
    /// Who wrote it.
    pub origin: MapOrigin,
    /// `read` | `edited` activity marks (O10); filled client-side, empty from the engine.
    pub marks: Vec<String>,
}

/// Reply of `codemap_element`: one element, one level live.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct MapElement {
    /// Freshness and state.
    pub meta: MapMeta,
    /// Element id.
    pub id: String,
    /// Node id.
    pub node: String,
    /// Perspective id.
    pub perspective: String,
    /// Parent element id.
    pub parent: Option<String>,
    /// `(element id, name)` from the perspective root to this element.
    pub crumbs: Vec<(String, String)>,
    /// Display name.
    pub name: String,
    /// Shortest covering path.
    pub path: Option<String>,
    /// `unit | file | symbol | cluster | external | bundle | perspective`.
    pub kind: String,
    /// Member node ids (a `cluster:`'s units, a `bundle:`'s remainder).
    pub members: Vec<String>,
    /// Index paths this element covers.
    pub anchors: Vec<MapAnchor>,
    /// Prose (§4.3); empty fields omitted in every view.
    pub fields: MapFields,
    /// The diagram whose nodes are the children; `None` for a leaf.
    pub diagram: Option<GraphProps>,
    /// `to_mermaid(diagram)` for Copy Mermaid.
    pub mermaid: Option<String>,
    /// Edges between the children, with evidence (§4.4).
    pub edges: Vec<MapEdge>,
    /// Children, with thumbnails for groups.
    pub children: Vec<MapChild>,
    /// Elements of other perspectives for the same node ("also in …").
    pub also_in: Vec<String>,
    /// Facts hash moved since generation.
    pub stale: bool,
    /// Who wrote it.
    pub origin: MapOrigin,
    /// How it was written; `None` for a structure-only element.
    pub provenance: Option<MapProvenance>,
    /// Files and symbols, for leaves and depth > [`MAX_DEPTH`].
    pub listing: Option<MapListing>,
}

/// One child of an element: the node's row plus a bounded thumbnail.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct MapChild {
    /// Element id of the child.
    pub id: String,
    /// Node id.
    pub node: String,
    /// Display name.
    pub name: String,
    /// Shortest covering path.
    pub path: Option<String>,
    /// `unit | file | symbol | cluster | external | bundle`.
    pub kind: String,
    /// Parsed files under its anchors.
    pub files: u32,
    /// Σ `files.lines`.
    pub lines: u32,
    /// Σ `files.rank` / Σ all, 0–1.
    pub share: f32,
    /// No diagram of its own.
    pub leaf: bool,
    /// Facts hash moved since generation.
    pub stale: bool,
    /// The child's diagram, ≤ [`DIAGRAM_NODES`] nodes; labels dropped client-side (W6).
    pub thumbnail: Option<GraphProps>,
}

/// One index path an element covers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct MapAnchor {
    /// Root-relative path (a directory prefix or a file).
    pub path: String,
    /// 1-based first line, for symbol anchors.
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub line_start: Option<i64>,
    /// 1-based last line, inclusive.
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub line_end: Option<i64>,
    /// The path is in `files` now (an imported element may point at nothing).
    pub exists: bool,
}

/// omm's prose fields in reading order (§4.3); `diagram` is typed, not here.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct MapFields {
    /// What it is and which paths it covers; empty for a structure-only element.
    pub description: String,
    /// Why it is shaped this way.
    pub context: Option<String>,
    /// Rules it obeys.
    pub constraint: Option<String>,
    /// Risks, only when a metric trips.
    pub concern: Option<String>,
    /// Open work.
    pub todo: Option<String>,
    /// Anything else.
    pub note: Option<String>,
}

/// One edge between an element's children (§4.4).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct MapEdge {
    /// Child node id.
    pub from: String,
    /// Child node id.
    pub to: String,
    /// Why, ≤ 4 words, never a count (§6.6 rule 7).
    pub label: String,
    /// Computed: ≥ 1 non-`Inferred` evidence joins the endpoints.
    pub backed: bool,
    /// What the index holds between the endpoints, or the model's reason.
    pub evidence: Vec<MapEvidence>,
}

/// Why an edge exists (§4.4); the engine renders counts from these.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MapEvidence {
    /// A manifest dependency between two units.
    Manifest {
        /// Unit path.
        from: String,
        /// Unit path.
        to: String,
        /// `cargo-path | npm-workspace | pyproject`.
        kind: String,
    },
    /// Aggregated `imports` rows.
    Imports {
        /// Rows.
        count: u32,
        /// One `path#L`.
        sample: String,
    },
    /// Aggregated cross-file `refs` rows with `confidence ≥ 0.5`.
    Refs {
        /// Rows.
        count: u32,
        /// One `path#L`.
        sample: String,
        /// Mean confidence of the rows.
        confidence: f64,
    },
    /// One call site the model asked for.
    Call {
        /// `path#L`.
        site: String,
        /// Caller moniker.
        from: String,
        /// Callee moniker.
        to: String,
    },
    /// Generated bindings: an exporter in one unit, `bindings/` in another (detector-inferred, drawn solid).
    Bindings {
        /// `ts-rs` | ….
        generator: String,
        /// Root-relative output directory.
        output_dir: String,
    },
    /// A process or socket edge (detector-inferred, drawn solid).
    Process {
        /// `path#L`.
        site: String,
        /// `spawn | dial | listen`.
        verb: String,
    },
    /// Model-asserted, dashed; the reason is shown.
    Inferred {
        /// The model's reason.
        reason: String,
    },
}

/// How an element was written (§8).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct MapProvenance {
    /// Model id; `imported` for `.omm/` elements.
    pub model: String,
    /// HEAD at generation.
    pub git_head: Option<String>,
    /// Index `map_state` at generation.
    pub index_state: String,
    /// Unix ms.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub generated_ms: u64,
    /// `model_calls` request ids.
    pub call_ids: Vec<String>,
    /// Σ cost of those calls.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub cost_nanos: u64,
    /// Currency of `cost_nanos`.
    pub currency: String,
    /// Σ input tokens.
    pub input_tokens: u32,
    /// Σ output tokens.
    pub output_tokens: u32,
    /// A repair round ran, or the diagram fell back to structure (§6.6).
    pub repaired: bool,
    /// Facts hash at generation (§5.4).
    pub facts_hash: String,
}

/// Files and symbols of a leaf, live from the index.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct MapListing {
    /// `(path, lines, share)`.
    pub files: Vec<(String, u32, f32)>,
    /// Top-ranked symbols, ≤ [`LISTING_SYMBOLS`].
    pub symbols: Vec<MapRow>,
    /// Symbols beyond the cap.
    pub more: u32,
}

/// One `codemap_search` hit.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct MapHit {
    /// Element id.
    pub element: String,
    /// Display name.
    pub name: String,
    /// Shortest covering path.
    pub path: Option<String>,
    /// Element ids from the perspective root to the hit.
    pub chain: Vec<String>,
}

/// Reply of `codemap_export` (§13.1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct MapExport {
    /// `<root>/.omm/`.
    pub dir: PathBuf,
    /// Elements written.
    pub elements: u32,
    /// Files written.
    pub files: u32,
    /// Element ids skipped (imported perspectives, invalid names) with the reason.
    pub skipped: Vec<String>,
}

#[cfg(test)]
mod tests;
