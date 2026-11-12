//! 137 — Endless: an optional summary-tree memory mode per session. Units
//! are derived from `messages` rows ([`units`]), a background pump
//! ([`pump`]) summarises them into a binary tree of ≤ `node_bytes` lines
//! ([`tree`], [`compactor`], [`guards`]), and a slack fold ([`fold`]) turns the
//! tree into the fixed-size view every turn starts from. [`Endless`] is the
//! one handle the loop, the tools and the RPC call (137-contract.md §4).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use parking_lot::{Mutex, RwLock};
use runtime::{Runtime, SessionId};
use serde::{Deserialize, Serialize};

use crate::store::Store;

pub mod compactor;
pub mod fold;
pub mod guards;
pub mod pump;
#[cfg(test)]
mod tests;
pub mod tree;
pub mod units;

pub use fold::{ViewBlock, ViewRender};
pub use tree::{Kind, Node, Unit};

/// `agent.endless` (137 §11.1). Every key optional in YAML; the whole struct
/// is what Settings › Memory reads and writes (`endless_settings_get/set`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(default)]
pub struct EndlessConfig {
    /// New chat ∞ switch initial state.
    pub default_chat: bool,
    /// Desks without an explicit `memory:` field (R1-D7: on).
    pub default_desk: bool,
    /// Meeting sessions (R1-D6).
    pub default_meeting: bool,
    /// View budget in bytes (16_000..=256_000; R1-D2: 128 KB).
    pub view_bytes: u32,
    /// View budget for a meeting head's `## Earlier in this meeting` (R1-D6).
    pub meeting_view_bytes: u32,
    /// R1-D4: at switch-on the tree covers only the last this many units.
    pub backfill_max_units: u32,
    /// One summary line's ceiling in bytes.
    pub node_bytes: u32,
    /// Slack band of the fold (0.0..=0.5), D5.
    pub slack: f32,
    /// How long a turn waits for the view to be fully built (D10).
    pub settle_secs: u32,
    /// Compactor calls in flight per session.
    pub jobs: u8,
    /// Attempts per node before the best candidate wins (D7).
    pub tries: u8,
    /// Delay before a failed node is retried.
    pub retry_secs: u32,
    /// Tool-result cap in chars before a unit is cut head ⅔ / tail ⅓ (§4).
    pub cap_chars: u32,
    /// Compactor model; `None` = the session's own model (D8).
    pub model: Option<String>,
    /// 194: the compactor model's effort; `None` = thinking off (R1 Q1). Only
    /// with `model` (validated against it, dropped when a new `model` does not take it).
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub effort: Option<String>,
    /// 137F E6: one attempt's bound in seconds for a cloud compactor (10..=3600).
    #[cfg_attr(feature = "ts", ts(as = "Option<u32>", optional))]
    pub attempt_secs: u32,
    /// 137F E6: one attempt's bound in seconds for a local compactor (30..=7200).
    #[cfg_attr(feature = "ts", ts(as = "Option<u32>", optional))]
    pub local_attempt_secs: u32,
    /// 137F E8: a local compactor sees at most this many bytes of the `<chat>` view, newest kept (4_000..=256_000).
    #[cfg_attr(feature = "ts", ts(as = "Option<u32>", optional))]
    pub local_ctx_bytes: u32,
    /// 137F E9: compactor calls in flight per local endpoint, engine-wide (1..=16).
    #[cfg_attr(feature = "ts", ts(as = "Option<u8>", optional))]
    pub local_jobs: u8,
    /// 137G G2: unsummarized lines the view shows verbatim, newest first, in bytes (4_000..=128_000); older ones fold into one placeholder line.
    #[cfg_attr(feature = "ts", ts(as = "Option<u32>", optional))]
    pub raw_bytes: u32,
    /// 137G G3: the compactor's `<chat>` context cap in bytes for every model (8_000..=256_000); a local one takes the smaller of this and `local_ctx_bytes`.
    #[cfg_attr(feature = "ts", ts(as = "Option<u32>", optional))]
    pub compactor_ctx_bytes: u32,
    /// 137G G6: consecutive unsummarized units one compactor call summarizes (1..=64).
    #[cfg_attr(feature = "ts", ts(as = "Option<u8>", optional))]
    pub batch_units: u8,
    /// 137G G11: USD one compactor call's prompt may cost (0.001..=10); over it the context shrinks, else the line folds mechanically.
    #[cfg_attr(feature = "ts", ts(as = "Option<f64>", optional))]
    pub max_usd_per_call: f64,
    /// 137G G11: USD a chat's compactor may spend per UTC day (0..=1000, 0 = no cap); reaching it pauses settling (`paused_by: "spend"`).
    #[cfg_attr(feature = "ts", ts(as = "Option<f64>", optional))]
    pub max_usd_per_session_day: f64,
}

impl Default for EndlessConfig {
    fn default() -> Self {
        Self {
            default_chat: false,
            default_desk: true,
            default_meeting: false,
            view_bytes: 128_000,
            meeting_view_bytes: 32_000,
            backfill_max_units: 2_000,
            node_bytes: 512,
            slack: 0.125,
            settle_secs: 45,
            jobs: 8,
            tries: 5,
            retry_secs: 10,
            cap_chars: 30_000,
            model: None,
            effort: None,
            attempt_secs: 120,
            local_attempt_secs: 600,
            local_ctx_bytes: 24_000,
            local_jobs: 1,
            raw_bytes: 32_000,
            compactor_ctx_bytes: 48_000,
            batch_units: 16,
            max_usd_per_call: 0.10,
            max_usd_per_session_day: 5.0,
        }
    }
}

impl EndlessConfig {
    /// The fold parameters, clamped to the documented ranges.
    pub fn fold_cfg(&self) -> fold::FoldCfg {
        self.fold_cfg_for(self.view_bytes)
    }

    /// Fold parameters for a per-session / per-render budget.
    pub fn fold_cfg_for(&self, view_bytes: u32) -> fold::FoldCfg {
        fold::FoldCfg {
            view_bytes: view_bytes.clamp(16_000, 256_000),
            slack: self.slack.clamp(0.0, 0.5),
            raw_bytes: self.raw_bytes.clamp(4_000, 128_000),
        }
    }
}

/// R1-D3: the compactor model id — `agent.endless.model` → `agent.title_model`
/// → the session's model. What Settings and the Memory panel display.
pub fn resolved_compactor_model(cfg: &crate::AgentConfig, session_model: &str) -> String {
    cfg.endless
        .model
        .clone()
        .or_else(|| cfg.title_model.clone())
        .unwrap_or_else(|| session_model.to_owned())
}

/// R1-D6: which parts become units (`sessions.endless_filter`, fixed at switch-on).
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum UnitFilter {
    /// Every part (§4 table).
    #[default]
    All,
    /// Meetings: `user` texts starting with `[meeting` and `tool` calls
    /// `meeting_topic` / `meeting_brief`; everything else dropped.
    Meeting,
}

impl UnitFilter {
    /// The stored string form.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Meeting => "meeting",
        }
    }

    /// Whether `unit` enters the tree.
    pub fn keeps(self, unit: &tree::Unit) -> bool {
        match self {
            Self::All => true,
            Self::Meeting => match unit.kind {
                tree::Kind::User => unit.text.starts_with("[meeting"),
                tree::Kind::Tool => {
                    unit.text.starts_with("meeting_topic ")
                        || unit.text.starts_with("meeting_brief ")
                }
                _ => false,
            },
        }
    }
}

impl std::str::FromStr for UnitFilter {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "all" => Ok(Self::All),
            "meeting" => Ok(Self::Meeting),
            _ => Err(format!("unknown unit filter: {s:?}")),
        }
    }
}

/// A session's memory mode (`sessions.memory`, D1).
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum MemoryMode {
    /// 07/11 epochs: carried transcript, folded into summary heads.
    #[default]
    Epochs,
    /// 137: the summary tree and its view.
    Endless,
}

impl MemoryMode {
    /// The stored string form.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Epochs => "epochs",
            Self::Endless => "endless",
        }
    }
}

impl std::fmt::Display for MemoryMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for MemoryMode {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "epochs" => Ok(Self::Epochs),
            "endless" => Ok(Self::Endless),
            _ => Err(format!("unknown memory mode: {s:?}")),
        }
    }
}

/// One line of the view (or of a zoom / recall result) as the UI sees it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ViewLine {
    /// First unit the line covers.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub id: u64,
    /// Units covered (a power of two).
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub n: u64,
    /// Tree level (`n == 2^level`).
    pub level: u8,
    /// The line (placeholder or verbatim unit when not built, D10).
    pub text: String,
    /// A built summary line (not a placeholder).
    pub built: bool,
    /// The message's row time for an `n == 1` line (W3: zoom shows its date); `None` otherwise.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub at: Option<DateTime<Utc>>,
    /// 137G: a line made by the mechanical fold (an excerpt, not a model summary).
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<bool>", optional))]
    pub mechanical: bool,
}

/// Guard trips by kind (§6).
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct GuardTrips {
    /// G1 trips.
    pub copy: u32,
    /// G2 trips.
    pub grounding: u32,
    /// G3 trips.
    pub status: u32,
    /// G4 trips.
    pub format: u32,
    /// G5 trips.
    pub empty: u32,
    /// Size-limit retries.
    pub size: u32,
}

/// Per-session compactor totals (§6, the Memory panel header).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct EndlessStats {
    /// Compactor attempts recorded.
    pub calls: u32,
    /// Retries by guard.
    pub guard_trips: GuardTrips,
    /// Prompt tokens, cache buckets included.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub input_tokens: u64,
    /// Prompt tokens read from cache.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub cache_read_tokens: u64,
    /// Prompt tokens written to cache.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub cache_write_tokens: u64,
    /// Completion tokens.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub output_tokens: u64,
    /// Catalog-rate estimate in USD.
    pub usd_estimate: f64,
}

/// Backfill progress (D14) and cost (R1-D4): also the shape of
/// [`Endless::backfill_estimate`] before a backfill starts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Backfill {
    /// History units with a level-0 line.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub done: u64,
    /// History units to cover (after the `backfill_max_units` cap).
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub total: u64,
    /// Remaining × measured seconds per unit (5 s before any node is built).
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub eta_secs: u64,
    /// Remaining × measured USD per unit ($0.02 before any node is built).
    #[serde(default)]
    pub usd_estimate: f64,
}

/// A view part that keeps failing to build (137F). Engine → UI.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Stalled {
    /// Consecutive failed attempts of the failing node that blocks most (the frontier first).
    pub failures: u32,
    /// The last error, one line, ≤ 300 chars (secrets never appear: it is the provider's message).
    pub error: String,
    /// RFC 3339 UTC time of the pump's own next retry; `None` while a retry is running.
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub next_retry_at: Option<String>,
    /// The compactor model id that failed.
    pub model: String,
    /// The failing node is the level-0 frontier: newer lines wait behind it.
    pub blocking: bool,
}

/// Unsummarized lines waiting for the compactor (137G). Engine → UI. TS name
/// `EndlessBacklog`: `dreaming::Backlog` owns `Backlog.ts`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(
    feature = "ts",
    derive(ts_rs::TS),
    ts(export, rename = "EndlessBacklog")
)]
pub struct Backlog {
    /// Level-0 lines not yet built.
    pub lines: u32,
    /// Of those, how many the view currently shows folded into one placeholder line (the raw budget).
    pub folded: u32,
    /// Estimate at the measured throughput; 0 when paused or unknown.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub eta_secs: u64,
    /// Settling is paused for this session (no compactor calls).
    pub paused: bool,
    /// Who paused: `"user"` (`endless_pause`) or `"spend"` (the G11 day cap); `None` while running.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub paused_by: Option<String>,
}

/// RPC `session_memory` (§11.2).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SessionMemory {
    /// The session's mode.
    pub mode: MemoryMode,
    /// Empty for an epochs session.
    pub view: Vec<ViewLine>,
    /// Some view part is still being built.
    pub settling: bool,
    /// Parts rendered per D10 right now.
    pub unbuilt: u32,
    /// Units so far (`T`).
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub units: u64,
    /// Compactor totals.
    pub stats: EndlessStats,
    /// Compactor model id; `""` for an epochs session.
    pub model: String,
    /// The view budget in force (R1b: the session's own, else `agent.endless.view_bytes`).
    pub view_bytes: u32,
    /// D14 progress while history is being covered.
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub backfill: Option<Backfill>,
    /// 137F: the failing node that blocks most; `None` when nothing is failing.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub stalled: Option<Stalled>,
    /// 137G: the unsettled backlog; `None` when nothing waits and the session is not paused.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub backlog: Option<Backlog>,
    /// 137G: this chat's own compactor pick (`set_session_compactor_model`); `None` = inherit.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub model_override: Option<String>,
    /// 137G: which link chose `model`: `"chat"` | `"agent"` | `"settings"` | `"title"` | `"session"`;
    /// `""` for an epochs session.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<String>", optional))]
    pub model_source: String,
    /// 194: the effort `model` runs with — this chat's own pick
    /// (`set_session_compactor_effort`), else the effort of the link that chose
    /// `model` (the agent's `memory_effort`, `agent.endless.effort`); `None` =
    /// thinking off. An effort `model` does not take is dropped (`None`).
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub effort: Option<String>,
    /// 194 R1: this chat's own compactor effort (`set_session_compactor_effort`),
    /// stored beside `model_override`; `None` = inherit. The panel's chips show it.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub effort_override: Option<String>,
    /// 137G G11: this chat's compactor spend in the current UTC day (USD, `model_calls` kind `endless`).
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<f64>", optional))]
    pub spend_today_usd: f64,
}

/// Bus event on the session stream, ≤ 1 per 500 ms per session (§11.2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[schemars(transform = wire::serde_type_tag::<Self>)]
#[serde(tag = "type")]
pub struct EndlessProgress {
    /// The session.
    pub session: SessionId,
    /// Nodes built.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub built: u64,
    /// History units to cover.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub total: u64,
    /// Some view part is still being built.
    pub settling: bool,
    /// First failure of a node (reported once per node).
    pub error: Option<String>,
    /// D14 progress while history is being covered.
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub backfill: Option<Backfill>,
    /// 137F: as [`SessionMemory::stalled`]; emitted whenever it appears, its count changes or it clears.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub stalled: Option<Stalled>,
    /// 137G: as [`SessionMemory::backlog`].
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub backlog: Option<Backlog>,
}

impl wire::WireEvent for EndlessProgress {
    const TOPIC: &'static str = "agent.endless-progress";
}

/// Reply of [`Endless::set_mode`].
#[derive(Clone, Debug, PartialEq)]
pub struct SetMode {
    /// The state after the switch.
    pub memory: SessionMemory,
    /// `/endless off`: the `<context_summary epoch=n style="endless">` head
    /// text the caller writes as the next epoch head (8.6).
    pub head: Option<String>,
}

/// What `attach` registers for a live session.
#[derive(Clone)]
pub struct Attached {
    /// Compactor model id.
    pub model: models::Resolved,
    /// The session's reasoning effort.
    pub effort: Option<genai::chat::ReasoningEffort>,
    /// The agent name for the COMPACT prompt (`theuth` or the desk).
    pub agent: String,
    /// A private tree's own store; `None` = the engine's.
    pub store: Option<Store>,
}

pub(crate) struct Inner {
    pub(crate) store: Store,
    pub(crate) rt: Runtime,
    pub(crate) client: genai::Client,
    pub(crate) auth: crate::supervisor::Auth,
    /// The model catalog; replaced live by [`Endless::set_catalog`] (R1-16).
    pub(crate) catalog: RwLock<Arc<models::Catalog>>,
    /// `agent.endless` in force; replaced live by [`Endless::set_config`].
    pub(crate) cfg: Arc<RwLock<EndlessConfig>>,
    /// `agent.title_model` (R1-D3 chain); replaced live by [`Endless::set_title_model`].
    pub(crate) title_model: RwLock<Option<String>>,
    /// Live sessions and their pumps.
    pub(crate) sessions: Mutex<HashMap<SessionId, pump::Handle>>,
    /// 137F E9: one queue per local endpoint, engine-wide.
    pub(crate) queues: pump::Queues,
    /// Tests inject scripted compactors here, by resolved model id; `None` = the model (D8).
    pub(crate) compactor_for_tests: Option<pump::CompactorFactory>,
}

/// The one handle for the endless mode (137-contract.md §4). Cheap to clone.
#[derive(Clone)]
pub struct Endless {
    inner: Arc<Inner>,
}

impl Endless {
    /// Construct once per engine, beside the other side-call slots (the
    /// supervisor; `Auth` is crate-private). W2 wires the call site.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn new(
        store: Store,
        rt: Runtime,
        client: genai::Client,
        auth: crate::supervisor::Auth,
        catalog: Arc<models::Catalog>,
        cfg: EndlessConfig,
        title_model: Option<String>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                store,
                rt,
                client,
                auth,
                catalog: RwLock::new(catalog),
                cfg: Arc::new(RwLock::new(cfg)),
                title_model: RwLock::new(title_model),
                sessions: Mutex::new(HashMap::new()),
                queues: pump::Queues::default(),
                compactor_for_tests: None,
            }),
        }
    }

    /// The config in force.
    pub fn config(&self) -> EndlessConfig {
        self.inner.cfg.read().clone()
    }

    /// Settings › Memory: the new `agent.endless`, in force from now on —
    /// every pump reads the shared config; a changed view budget or slack
    /// re-folds at the next render (the fold cache is keyed by its parameters).
    /// A session whose compactor changes with it is re-kicked (137F E2).
    pub fn set_config(&self, cfg: EndlessConfig) {
        *self.inner.cfg.write() = cfg;
        pump::reresolve(&self.inner, None);
    }

    /// `SetTitleModel`: the R1-D3 chain's second link, live (137F E1).
    pub fn set_title_model(&self, model: Option<String>) {
        *self.inner.title_model.write() = model;
        pump::reresolve(&self.inner, None);
    }

    /// `SetModel` / `SetEffort` on a session: the chain's last link, live
    /// (137F E1). `model: None` keeps the session's model.
    pub fn set_session_model(
        &self,
        session: SessionId,
        model: Option<models::Resolved>,
        effort: Option<genai::chat::ReasoningEffort>,
    ) {
        if let Some(h) = self.inner.sessions.lock().get_mut(&session) {
            if let Some(m) = model {
                h.attached.model = m;
            }
            h.attached.effort = effort;
        }
        pump::reresolve(&self.inner, Some(session));
    }

    /// A provider was added, refreshed or removed, or a model's limits set
    /// (the catalog and the gateways changed): every session's model takes
    /// its new catalog entry (177 E6) and its compactor is re-resolved (R1-16).
    pub fn set_catalog(&self, catalog: Arc<models::Catalog>) {
        for h in self.inner.sessions.lock().values_mut() {
            if let Some((_, m)) = catalog.get(&h.attached.model.id) {
                h.attached.model.model = Some(m.clone());
            }
        }
        *self.inner.catalog.write() = catalog;
        pump::reresolve(&self.inner, None);
    }

    /// 177 E9: one real summary call for `model` on a fixed sample; writes no memory.
    pub async fn test_summary(&self, model: &str) -> crate::SummaryTest {
        pump::test_summary(&self.inner, model).await
    }

    /// `settle` returned early because the frontier keeps failing (R1-15).
    pub fn frontier_failing(&self, session: SessionId) -> bool {
        pump::frontier_failing(&self.inner, session)
    }

    /// Engine start (109): one pump per `sessions.memory = 'endless'` session.
    /// Returns how many pumps started.
    pub async fn resume_all(&self) -> anyhow::Result<usize> {
        pump::resume_all(&self.inner).await
    }

    /// Register a live session (idempotent); starts its pump when the mode is `endless`.
    pub async fn attach(&self, session: SessionId, attached: Attached) -> anyhow::Result<()> {
        pump::attach(&self.inner, session, attached).await
    }

    /// The actor stopped: stop the pump and drop in-memory state.
    pub fn detach(&self, session: SessionId) {
        pump::detach(&self.inner, session);
    }

    /// `sessions.memory` (a store read; works for unattached sessions).
    pub async fn mode(&self, session: SessionId) -> anyhow::Result<MemoryMode> {
        self.store_for(session).session_memory(session).await
    }

    /// The loop's switch (D14): attached ∧ `endless` ∧ backfill finished ∧
    /// filter ≠ `Meeting` (a meeting keeps head D + its verbatim tail; only the
    /// pump and `render_with_budget` serve it).
    pub fn active(&self, session: SessionId) -> bool {
        self.inner
            .sessions
            .lock()
            .get(&session)
            .is_some_and(pump::Handle::active)
    }

    /// §9: the session has a tree the three tools can read — attached ∧
    /// `endless`, any filter, backfill or not. A meeting session's toolset
    /// lists `zoom`/`recall`/`date`; this is their gate (W4 hand-off 3).
    pub fn served(&self, session: SessionId) -> bool {
        self.inner
            .sessions
            .lock()
            .get(&session)
            .is_some_and(pump::Handle::served)
    }

    /// 8.1: wait until every view part is built, at most `max`. `true` = settled.
    pub async fn settle(&self, session: SessionId, max: Duration) -> bool {
        pump::settle(&self.inner, session, max).await
    }

    /// 8.1: the view for this turn; freezes it as the `<chat>` context of the
    /// units the turn commits (D6). Once per turn, after `settle`, before the
    /// input row is logged.
    pub async fn render_for_turn(&self, session: SessionId) -> anyhow::Result<ViewRender> {
        pump::render_for_turn(&self.inner, session, None, false).await
    }

    /// 137G G1/G5: [`Endless::render_for_turn`] scaled to at most
    /// `max_bytes` of view budget (`view_bytes + raw_bytes` shrink together);
    /// `again` re-renders the current turn's view (a refused request) instead
    /// of opening a new turn.
    pub async fn render_for_turn_within(
        &self,
        session: SessionId,
        max_bytes: usize,
        again: bool,
    ) -> anyhow::Result<ViewRender> {
        pump::render_for_turn(&self.inner, session, Some(max_bytes), again).await
    }

    /// 8.3: rows reached `messages`; the pump derives the units itself.
    pub fn on_rows_committed(&self, session: SessionId) {
        pump::nudge(&self.inner, session);
    }

    /// R1-D2 / R1b: the session's view budget (a user control, persisted in
    /// `sessions.endless_view_bytes`; clamped 16_000..=256_000 at use). The
    /// fold re-fits at the next render: a smaller budget merges down, a
    /// larger one re-folds at the new budget (never cuts text).
    pub async fn set_view_bytes(&self, session: SessionId, bytes: u32) -> anyhow::Result<()> {
        pump::set_view_bytes(&self.inner, session, Some(bytes)).await
    }

    /// R1-D1: the same render as the turn's, without freezing a context —
    /// a native child's first message.
    pub async fn render_snapshot(&self, session: SessionId) -> anyhow::Result<Vec<ViewBlock>> {
        pump::render_snapshot(&self.inner, session).await
    }

    /// R1-D6: the view re-folded to `bytes`, as lines (a meeting head's
    /// `## Earlier in this meeting`); works for a `UnitFilter::Meeting`
    /// session, whose `active()` is false.
    pub async fn render_with_budget(
        &self,
        session: SessionId,
        bytes: u32,
    ) -> anyhow::Result<Vec<ViewLine>> {
        pump::render_with_budget(&self.inner, session, bytes).await
    }

    /// R1-D4: ETA and cost of switching the session on (or of the backfill under way).
    pub async fn backfill_estimate(&self, session: SessionId) -> anyhow::Result<Backfill> {
        pump::backfill_estimate(&self.inner, session).await
    }

    /// `zoom(id, n)` (§9).
    pub async fn zoom(&self, session: SessionId, id: u64, n: u64) -> Result<Vec<ViewLine>, String> {
        pump::zoom(&self.inner, session, id, n).await
    }

    /// `recall(query, limit)` (§9).
    pub async fn recall(
        &self,
        session: SessionId,
        query: &str,
        limit: u8,
    ) -> Result<Vec<ViewLine>, String> {
        pump::recall(&self.inner, session, query, limit).await
    }

    /// `date(id)` (§9): the unit's row time.
    pub async fn date(&self, session: SessionId, id: u64) -> Result<Option<DateTime<Utc>>, String> {
        pump::date(&self.inner, session, id).await
    }

    /// `/endless on|off` (8.6); the caller guarantees the session is idle. A
    /// session with a transcript backfills under R1-D4; `filter` (R1-D6) and
    /// the base are fixed at switch-on.
    pub async fn set_mode(
        &self,
        session: SessionId,
        mode: MemoryMode,
        epoch: u32,
        filter: UnitFilter,
    ) -> anyhow::Result<SetMode> {
        pump::set_mode(&self.inner, session, mode, filter, epoch, None).await
    }

    /// [`Endless::set_mode`] whose `/endless off` head is scaled to at most
    /// `max_bytes` of view budget (the session model's turn allowance, R1-7).
    pub async fn set_mode_within(
        &self,
        session: SessionId,
        mode: MemoryMode,
        epoch: u32,
        filter: UnitFilter,
        max_bytes: usize,
    ) -> anyhow::Result<SetMode> {
        pump::set_mode(&self.inner, session, mode, filter, epoch, Some(max_bytes)).await
    }

    /// RPC `session_memory` body data.
    pub async fn memory(&self, session: SessionId) -> anyhow::Result<SessionMemory> {
        pump::memory(&self.inner, session).await
    }

    /// RPC `endless_retry` (137F): reset every failed node's backoff, wake
    /// the pump, return the fresh state. An epochs session is untouched.
    pub async fn retry(&self, session: SessionId) -> anyhow::Result<SessionMemory> {
        pump::retry(&self.inner, session).await
    }

    /// RPC `endless_fold` (137G): every unbuilt level-0 unit becomes a
    /// mechanical excerpt node at once (no model call); the pump merges on.
    /// An epochs session is untouched.
    pub async fn fold(&self, session: SessionId) -> anyhow::Result<SessionMemory> {
        pump::fold_all(&self.inner, session).await
    }

    /// RPC `endless_pause` (137G): `true` stops new compactor calls and
    /// cancels those in flight (no failures), keeping the backlog; `false`
    /// resumes. Persisted in `sessions.endless_paused`.
    pub async fn pause(&self, session: SessionId, paused: bool) -> anyhow::Result<SessionMemory> {
        pump::pause(&self.inner, session, paused).await
    }

    /// RPC `set_session_compactor_model` (137G): this chat's own compactor
    /// (`None` clears = inherit); refused when the id does not resolve;
    /// persisted in `sessions.endless_model`; live (re-resolve + kick, 137F).
    /// 194: the chat's own effort goes with a cleared model, and with a new
    /// one that does not take it.
    pub async fn set_model(
        &self,
        session: SessionId,
        model: Option<String>,
    ) -> anyhow::Result<SessionMemory> {
        pump::set_model(&self.inner, session, model, false, None).await
    }

    /// RPC `set_session_compactor_effort` (194): this chat's own compactor
    /// effort (`None` clears); refused when the model in force does not take
    /// it; persisted in `sessions.endless_effort`; live.
    pub async fn set_effort(
        &self,
        session: SessionId,
        effort: Option<String>,
    ) -> anyhow::Result<SessionMemory> {
        pump::set_effort(&self.inner, session, effort).await
    }

    /// The roster at desk open (137G): the agent's `memory_model` default
    /// for this desk session (`None` clears) with its `memory_effort` (194;
    /// cleared with the model); persisted, live.
    pub async fn set_agent_model(
        &self,
        session: SessionId,
        model: Option<String>,
        effort: Option<String>,
    ) -> anyhow::Result<SessionMemory> {
        pump::set_model(&self.inner, session, model, true, effort).await
    }

    /// The constant VIEW_DOC appended to the system prompt in endless sessions (§9).
    pub fn view_doc() -> &'static str {
        VIEW_DOC
    }

    fn store_for(&self, session: SessionId) -> Store {
        self.inner.store_for(session)
    }
}

impl Inner {
    /// The session's store: a private tree's own, else the engine's.
    pub(crate) fn store_for(&self, session: SessionId) -> Store {
        self.sessions
            .lock()
            .get(&session)
            .and_then(|h| h.attached.store.clone())
            .unwrap_or_else(|| self.store.clone())
    }
}

/// §9 VIEW_DOC, verbatim.
pub const VIEW_DOC: &str =
    "The view: the whole session, oldest first, inside <chat> tags, as one-line
summaries. Each line is  id+n|text  — the n messages from id on, summarized
(newlines shown as spaces). A summary tags each item with its kind: user,
talk, tool, echo, work, note. A short message is its own line, word for word.
Recent lines cover one message each; the older the messages, the more a line
covers. A message not summarized yet shows as \"(not summarized yet: zoom
it)\" or verbatim. No message appears in full, not even the last ones.
Navigating: zoom(id, n) opens line id+n into the two lines it was made from;
zoom(id, 1) gives message id in full. recall(words) searches every message's
summary line. Zoom or recall whenever a summary only mentions something you
need — what your last reply said, a decision, a past attempt, an exact
error, where a file is — before you act, guess or ask. date(id) gives the
date and time of message id. You keep no memory between turns beyond this
view, so say in your reply what you learned that will matter later.";
