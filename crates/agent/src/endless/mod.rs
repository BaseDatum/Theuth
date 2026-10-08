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
use parking_lot::Mutex;
use runtime::{Runtime, SessionId};
use serde::{Deserialize, Serialize};

use crate::store::Store;

pub mod compactor;
pub mod fold;
#[cfg(test)]
mod tests;
pub mod guards;
pub mod pump;
pub mod tree;
pub mod units;

pub use fold::{ViewBlock, ViewRender};
pub use tree::{Kind, Node, Unit};

/// `agent.endless` (137 §11.1). Every key optional in YAML.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct EndlessConfig {
    /// New chat ∞ switch initial state.
    pub default_chat: bool,
    /// Desks without an explicit `memory:` field.
    pub default_desk: bool,
    /// View budget in bytes (16_000..=256_000).
    pub view_bytes: u32,
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
}

impl Default for EndlessConfig {
    fn default() -> Self {
        Self {
            default_chat: false,
            default_desk: false,
            view_bytes: 64_000,
            node_bytes: 512,
            slack: 0.125,
            settle_secs: 45,
            jobs: 8,
            tries: 5,
            retry_secs: 10,
            cap_chars: 30_000,
            model: None,
        }
    }
}

impl EndlessConfig {
    /// The fold parameters, clamped to the documented ranges.
    pub fn fold_cfg(&self) -> fold::FoldCfg {
        fold::FoldCfg {
            view_bytes: self.view_bytes.clamp(16_000, 256_000),
            slack: self.slack.clamp(0.0, 0.5),
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
}

/// Guard trips by kind (§6).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct GuardTrips {
    pub copy: u32,
    pub grounding: u32,
    pub status: u32,
    pub format: u32,
    pub empty: u32,
    pub size: u32,
}

/// Per-session compactor totals (§6, the Memory panel header).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct EndlessStats {
    pub calls: u32,
    pub guard_trips: GuardTrips,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub input_tokens: u64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub cache_read_tokens: u64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub cache_write_tokens: u64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub output_tokens: u64,
    pub usd_estimate: f64,
}

/// Backfill progress (D14).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Backfill {
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub done: u64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub total: u64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub eta_secs: u64,
}

/// RPC `session_memory` (§11.2).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SessionMemory {
    pub mode: MemoryMode,
    /// Empty for an epochs session.
    pub view: Vec<ViewLine>,
    pub settling: bool,
    pub unbuilt: u32,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub units: u64,
    pub stats: EndlessStats,
    /// Compactor model id; `""` for an epochs session.
    pub model: String,
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub backfill: Option<Backfill>,
}

/// Bus event on the session stream, ≤ 1 per 500 ms per session (§11.2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type")]
pub struct EndlessProgress {
    pub session: SessionId,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub built: u64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub total: u64,
    pub settling: bool,
    /// First failure of a node (reported once per node).
    pub error: Option<String>,
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub backfill: Option<Backfill>,
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
    pub model: models::Resolved,
    pub effort: Option<genai::chat::ReasoningEffort>,
    pub agent: String,
    /// A private tree's own store; `None` = the engine's.
    pub store: Option<Store>,
}

pub(crate) struct Inner {
    pub(crate) store: Store,
    pub(crate) rt: Runtime,
    pub(crate) client: genai::Client,
    pub(crate) auth: crate::supervisor::Auth,
    pub(crate) catalog: Arc<models::Catalog>,
    pub(crate) cfg: EndlessConfig,
    /// Live sessions and their pumps.
    pub(crate) sessions: Mutex<HashMap<SessionId, pump::Handle>>,
    /// Tests inject a scripted compactor here; `None` = the model (D8).
    pub(crate) compactor_for_tests: Option<Arc<dyn compactor::Compactor>>,
}

/// The one handle for the endless mode (137-contract.md §4). Cheap to clone.
#[derive(Clone)]
pub struct Endless {
    inner: Arc<Inner>,
}

impl Endless {
    /// Construct once per engine, beside the other side-call slots (the
    /// supervisor; `Auth` is crate-private).
    pub(crate) fn new(
        store: Store,
        rt: Runtime,
        client: genai::Client,
        auth: crate::supervisor::Auth,
        catalog: Arc<models::Catalog>,
        cfg: EndlessConfig,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                store,
                rt,
                client,
                auth,
                catalog,
                cfg,
                sessions: Mutex::new(HashMap::new()),
                compactor_for_tests: None,
            }),
        }
    }

    /// The config in force.
    pub fn config(&self) -> &EndlessConfig {
        &self.inner.cfg
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

    /// The loop's switch (D14): attached ∧ `endless` ∧ backfill finished.
    pub fn active(&self, session: SessionId) -> bool {
        self.inner
            .sessions
            .lock()
            .get(&session)
            .is_some_and(pump::Handle::active)
    }

    /// 8.1: wait until every view part is built, at most `max`. `true` = settled.
    pub async fn settle(&self, session: SessionId, max: Duration) -> bool {
        pump::settle(&self.inner, session, max).await
    }

    /// 8.1: the view for this turn; freezes it as the `<chat>` context of the
    /// units the turn commits (D6). Once per turn, after `settle`, before the
    /// input row is logged.
    pub async fn render_for_turn(&self, session: SessionId) -> anyhow::Result<ViewRender> {
        pump::render_for_turn(&self.inner, session).await
    }

    /// 8.3: rows reached `messages`; the pump derives the units itself.
    pub fn on_rows_committed(&self, session: SessionId) {
        pump::nudge(&self.inner, session);
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
    pub async fn date(
        &self,
        session: SessionId,
        id: u64,
    ) -> Result<Option<DateTime<Utc>>, String> {
        pump::date(&self.inner, session, id).await
    }

    /// `/endless on|off` (8.6); the caller guarantees the session is idle.
    pub async fn set_mode(
        &self,
        session: SessionId,
        mode: MemoryMode,
        epoch: u32,
    ) -> anyhow::Result<SetMode> {
        pump::set_mode(&self.inner, session, mode, epoch).await
    }

    /// RPC `session_memory` body data.
    pub async fn memory(&self, session: SessionId) -> anyhow::Result<SessionMemory> {
        pump::memory(&self.inner, session).await
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
pub const VIEW_DOC: &str = "The view: the whole session, oldest first, inside <chat> tags, as one-line
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
