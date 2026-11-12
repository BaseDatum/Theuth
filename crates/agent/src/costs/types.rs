//! Wire types of the cost cube and the calls ledger (160 §3.1, §4). Frozen
//! contract: field names and serde/TS shapes are what the UI codes against.

use serde::{Deserialize, Serialize};

use crate::cost::SpendOwnerKind;

/// Where a cell's money comes from (160 D5).
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum CostBasis {
    /// A provider-returned price (`returned_*`).
    Known,
    /// An external harness reported the money (remote meters).
    Reported,
    /// theuth's price table × tokens (`estimated_*`, or repriced at read time).
    Estimated,
    /// No money: counted, `$0`.
    Unpriced,
}

impl CostBasis {
    /// The `CostCells::basis` code: 0 known · 1 reported · 2 estimated · 3 unpriced.
    pub fn code(self) -> u8 {
        match self {
            Self::Known => 0,
            Self::Reported => 1,
            Self::Estimated => 2,
            Self::Unpriced => 3,
        }
    }
}

/// A cube state the client already holds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct CostCubeCursor {
    /// Epoch of the held cube.
    pub epoch: String,
    /// Revision of the held cube.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub rev: u64,
}

/// One root session of the ledger, live or deleted — the cube's session dimension.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct CostRoot {
    /// Root session id.
    pub id: String,
    /// The root's title; "" for a deleted root (H1).
    pub title: String,
    /// Attribution class (spend_report's rules, D6).
    pub owner_kind: SpendOwnerKind,
    /// Agent name, routine name, "you" or "other" (spend_report's rules, D6).
    pub owner: String,
    /// `sessions.project` of the root.
    pub project: Option<String>,
    /// The root's session row is gone.
    pub deleted: bool,
    /// Root creation time.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub created_at_ms: u64,
}

/// A dimension entry with its stable index (indices never change within an epoch).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct CostRootEntry {
    /// Index the cells' `root` column points at.
    pub ix: u32,
    /// The root.
    pub root: CostRoot,
}

/// One model of the cube's model dimension.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct CostModel {
    /// Raw id as recorded (`anthropic/claude-haiku-4-5-20251001`).
    pub id: String,
    /// Canonical display name (`claude-haiku-4-5`); aliases share it.
    pub name: String,
    /// Provider of the model (`anthropic`).
    pub provider: String,
}

/// Columnar cells: every array has the same length; index i is one cell =
/// one (hour, root, model, provider, kind, via, basis) combination.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct CostCells {
    /// UTC epoch hour of the bucket = floor(unix_seconds / 3600).
    pub hour: Vec<u32>,
    /// Index into `roots`.
    pub root: Vec<u32>,
    /// Index into `models`.
    pub model: Vec<u32>,
    /// Index into `providers`.
    pub provider: Vec<u32>,
    /// Index into `kinds`.
    pub kind: Vec<u32>,
    /// Index into `vias`.
    pub via: Vec<u32>,
    /// 0 known · 1 reported · 2 estimated · 3 unpriced.
    pub basis: Vec<u8>,
    /// USD nanos.
    #[cfg_attr(feature = "ts", ts(type = "Array<number>"))]
    pub nanos: Vec<u64>,
    /// Attempts (native) or turns (reported).
    pub calls: Vec<u32>,
    /// Failed + cancelled + unresolved attempts.
    pub failed: Vec<u32>,
    /// Attempts priced at read time.
    pub repriced: Vec<u32>,
    /// Input tokens INCLUDING cache read and cache write.
    #[cfg_attr(feature = "ts", ts(type = "Array<number>"))]
    pub input: Vec<u64>,
    /// Output tokens.
    #[cfg_attr(feature = "ts", ts(type = "Array<number>"))]
    pub output: Vec<u64>,
    /// Cache-read tokens.
    #[cfg_attr(feature = "ts", ts(type = "Array<number>"))]
    pub cache_read: Vec<u64>,
    /// Cache-write tokens.
    #[cfg_attr(feature = "ts", ts(type = "Array<number>"))]
    pub cache_write: Vec<u64>,
    /// cache_read × (input rate − cache-read rate) from the catalog; 0 when a rate is missing.
    #[cfg_attr(feature = "ts", ts(type = "Array<number>"))]
    pub cache_saved_nanos: Vec<u64>,
    /// Reported money whose hour is approximate (turn-weighted or fallback placement).
    #[cfg_attr(feature = "ts", ts(type = "Array<number>"))]
    pub approx_nanos: Vec<u64>,
}

/// Unpriced attempts of one reason.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct CostReason {
    /// `unknown_reason`, else the status.
    pub reason: String,
    /// Attempts.
    pub calls: u32,
}

/// Cube metadata.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct CostCubeMeta {
    /// This engine's name (the ledger is per engine, H4).
    pub engine: String,
    /// Earliest native call or placed reported money.
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub first_call_ms: Option<u64>,
    /// When this cube state was built.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub built_at_ms: u64,
    /// Sessions before the ledger existed (history_coverage = 'unknown'), H3.
    pub pre_ledger_sessions: u32,
    /// Unpriced attempts by reason (status / unknown_reason), for the Basis card.
    pub unpriced: Vec<CostReason>,
    /// Hash of the price table used for repricing and cache savings.
    pub price_table: String,
}

/// `cost_cube(since)` answer: a full snapshot, or the changes since `since`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct CostCube {
    /// Cube identity; a new one per engine start and full rebuild.
    pub epoch: String,
    /// Revision within the epoch.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub rev: u64,
    /// true: replace everything the client holds. false: a delta.
    pub full: bool,
    /// Delta only: every cell of these UTC hours is replaced by the cells of this answer
    /// (cells carry every root of those hours). Empty when `full`.
    pub hours: Vec<u32>,
    /// Provider dictionary (complete, append-only within an epoch).
    pub providers: Vec<String>,
    /// Kind dictionary (complete, append-only within an epoch).
    pub kinds: Vec<String>,
    /// Via dictionary (complete, append-only within an epoch).
    pub vias: Vec<String>,
    /// Model dictionary (complete, append-only within an epoch).
    pub models: Vec<CostModel>,
    /// Full: every root (ix = position). Delta: new or changed roots only.
    pub roots: Vec<CostRootEntry>,
    /// The cells.
    pub cells: CostCells,
    /// Metadata.
    pub meta: CostCubeMeta,
}

/// Bus/wire event: the cube moved to `rev` (at most once per second).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type")]
pub struct CostCubeChanged {
    /// Cube epoch.
    pub epoch: String,
    /// New revision.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub rev: u64,
}

impl wire::WireEvent for CostCubeChanged {
    const TOPIC: &'static str = "agent.cost-cube";
}

/// Keyset position in the calls ledger (exclusive).
#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct CostCallsCursor {
    /// Time of the row.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub at_ms: u64,
    /// Row id.
    pub id: String,
}

/// `cost_calls` filter and page.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(default)]
pub struct CostCallsQuery {
    /// Inclusive lower bound.
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub from_ms: Option<u64>,
    /// Exclusive upper bound.
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub to_ms: Option<u64>,
    /// Root session ids (the UI maps owner / project / session filters to roots).
    pub roots: Option<Vec<String>>,
    /// Providers.
    pub providers: Option<Vec<String>>,
    /// Raw model ids.
    pub models: Option<Vec<String>>,
    /// Kinds.
    pub kinds: Option<Vec<String>>,
    /// Vias.
    pub vias: Option<Vec<String>>,
    /// Bases.
    pub bases: Option<Vec<CostBasis>>,
    /// Exclusive cursor: rows strictly older than it.
    pub before: Option<CostCallsCursor>,
    /// Default 200, max 1000.
    pub limit: Option<u32>,
}

/// One ledger row: a native attempt or a placed piece of reported money.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct CostCall {
    /// `call_id`, or `meter:<meter id>:<k>` for a placed piece of reported money.
    pub id: String,
    /// Reserve time (native) or placed time (reported).
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub at_ms: u64,
    /// Root session.
    pub root: String,
    /// The owner session (may be a subagent of `root`).
    pub session: String,
    /// `theuth` or the harness name.
    pub via: String,
    /// Provider.
    pub provider: Option<String>,
    /// Raw model id.
    pub model: Option<String>,
    /// `CallKind` string; `harness` for reported money.
    pub kind: String,
    /// `CallStatus` string; `reported` for reported money.
    pub status: String,
    /// Money basis.
    pub basis: CostBasis,
    /// USD nanos.
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub nanos: Option<u64>,
    /// Input tokens incl. cache.
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub input: Option<u64>,
    /// Output tokens.
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub output: Option<u64>,
    /// Cache-read tokens.
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub cache_read: Option<u64>,
    /// Cache-write tokens.
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub cache_write: Option<u64>,
    /// Priced at read time from the catalog.
    pub repriced: bool,
    /// Why there is no money (status / unknown_reason), for unpriced rows.
    pub reason: Option<String>,
}

/// One `cost_calls` page, newest first.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct CostCallsPage {
    /// Rows.
    pub calls: Vec<CostCall>,
    /// Cursor for the next page; `None` at the end.
    pub next: Option<CostCallsCursor>,
}
