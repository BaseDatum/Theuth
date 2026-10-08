# 137 — Contract (W1 TREE → W2 LOOP, W3 UI, closer)

Status: contract v1 · 2026-10-08 · what `crates/agent/src/endless/**` and the store provide; what W2 calls. Fixes the paths/derives of design 137 §13. Where this file and 137-endless.md differ, this file wins for Rust signatures; the design wins for behaviour.

## 0. Ownership recap

W1 writes: `crates/agent/src/endless/{mod,units,tree,fold,guards,compactor,pump,recall}.rs`, `crates/agent/src/store/endless.rs`, `crates/agent/migrations/0038_endless.sql`, `crates/agent/tests/fixtures/endless/`, and these small edits outside: `lib.rs` (`pub mod endless;`, `AgentConfig.endless`, `SessionSummary.memory`, re-exports), `store.rs` (`pub mod endless;`, `memory` column in `SessionRow`/`session_cols!`/`summary`, one `delete_endless_in` call in `delete_session`), `store/capture.rs` (`ATTACHED` += the three tables), `lifecycle.rs` (`CreateSessionOptions.memory`), `crates/runtime/src/side_call.rs` (`CallKind::Endless`), `crates/rpc/src/api.rs` (four trait methods + `Err("not yet")` stub bodies — jsonrpsee's `#[rpc]` trait is implemented in the same crate, so bodies are required), `crates/rpc/src/export.rs` (new types in the `export!` list).

W2 replaces the four stub bodies in `rpc/api.rs`, registers the event (§6), constructs `Endless` in the supervisor, and calls the API below from `frame.rs` / `loop_.rs` / `context.rs` / tools. W2 never edits `endless/**`; interface changes go through a message to W1 (or, after W1 is done, a small PR against `endless/mod.rs` only).

## 1. Config (`crates/agent/src/endless/mod.rs`, re-exported as `agent::EndlessConfig`)

```rust
/// `agent.endless` (137 §11.1). All optional in YAML (`#[serde(default)]`).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct EndlessConfig {
    pub default_chat: bool,       // false
    pub default_desk: bool,       // TRUE (R1-D7)
    pub default_meeting: bool,    // false (R1-D6)
    pub view_bytes: u32,          // 128_000 (R1-D2; clamped 16_000..=256_000 at use)
    pub meeting_view_bytes: u32,  // 32_000 (R1-D6)
    pub backfill_max_units: u32,  // 2_000 (R1-D4)
    pub node_bytes: u32,      // 512
    pub slack: f32,           // 0.125 (clamped 0.0..=0.5)
    pub settle_secs: u32,     // 45
    pub jobs: u8,             // 8
    pub tries: u8,            // 5
    pub retry_secs: u32,      // 10
    pub cap_chars: u32,       // 30_000
    pub model: Option<String> // None → `agent.title_model` → the session's model (R1-D3)
}
/// R1-D3, for Settings / the panel: `agent.endless.model` → `agent.title_model` → `session_model`.
pub fn resolved_compactor_model(cfg: &AgentConfig, session_model: &str) -> String;
```
`AgentConfig.endless: EndlessConfig` (serde default). Settings writes go through the existing `write_nested_config_value(path, "agent", "endless", key, value)` path — W2/W3.

## 2. Wire types (defined in `agent::endless`, re-exported at `agent::*`, exported to TS by `rpc/src/export.rs`)

All: `#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]`, `#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]`; enums `#[serde(rename_all = "snake_case")]`; `u64` fields carry `ts(type = "number")`.

```rust
pub enum MemoryMode { #[default] Epochs, Endless }        // as_str(): "epochs" | "endless"; FromStr; Default = Epochs
pub enum UnitFilter { #[default] All, Meeting }           // R1-D6; "all" | "meeting"; `keeps(&Unit) -> bool`; stored in sessions.endless_filter
pub struct BackfillEstimate { pub units: u64, pub eta_secs: u64, pub usd: f64 }   // R1-D4
pub struct ViewLine { pub id: u64, pub n: u64, pub level: u8, pub text: String, pub built: bool }
pub struct GuardTrips { pub copy: u32, pub grounding: u32, pub status: u32, pub format: u32, pub empty: u32, pub size: u32 }
pub struct EndlessStats { pub calls: u32, pub guard_trips: GuardTrips, pub input_tokens: u64, pub cache_read_tokens: u64, pub cache_write_tokens: u64, pub output_tokens: u64, pub usd_estimate: f64 }
pub struct Backfill { pub done: u64, pub total: u64, pub eta_secs: u64 }
pub struct SessionMemory {
    pub mode: MemoryMode,
    pub view: Vec<ViewLine>,       // empty for an epochs session
    pub settling: bool,
    pub unbuilt: u32,
    pub units: u64,
    pub stats: EndlessStats,
    pub model: String,             // compactor model id ("" for epochs)
    pub view_bytes: u32,           // R1b: the budget in force (session's own, else agent.endless.view_bytes)
    pub backfill: Option<Backfill>,
}
/// Bus event, `{"type":"EndlessProgress", …}` (`#[serde(tag = "type")]`), ≤ 1 per 500 ms per session.
pub struct EndlessProgress { pub session: SessionId, pub built: u64, pub total: u64, pub settling: bool, pub error: Option<String>, pub backfill: Option<Backfill> }
```

Edits W1 makes to existing wire types:
- `agent::CreateSessionOptions.memory: Option<MemoryMode>` (`#[serde(default)]`, `ts(optional)`); `None` = `agent.endless.default_chat` → W2 resolves and calls `store.set_session_memory(id, mode)` after `create_session` (no change to `Store::create_session`).
- `agent::SessionSummary.memory: MemoryMode` (`#[serde(default)]`), read from `sessions.memory`.
- `runtime::side_call::CallKind::Endless` (`"endless"`): the compactor's `model_calls.kind`.

## 3. RPC (`crates/rpc/src/api.rs`) — W1 adds the declarations and stub bodies, W2 the real bodies

```rust
#[method(name = "session_memory", with_extensions)]
async fn session_memory(&self, session: SessionId) -> RpcResult<SessionMemory>;
#[method(name = "endless_zoom", with_extensions)]
async fn endless_zoom(&self, session: SessionId, id: u64, n: u64) -> RpcResult<Vec<ViewLine>>;
#[method(name = "endless_recall", with_extensions)]
async fn endless_recall(&self, session: SessionId, query: String, limit: Option<u8>) -> RpcResult<Vec<ViewLine>>;
#[method(name = "set_session_memory", with_extensions)]
async fn set_session_memory(&self, session: SessionId, mode: MemoryMode) -> RpcResult<SessionMemory>;
```
Stub bodies return `ErrorObjectOwned::owned(ASK_FAILED, "not yet", None)`. Semantics for W2: `session_memory` on an epochs session → `mode: epochs`, empty view, zero stats (use `Endless::memory`). `set_session_memory` while a turn runs → error `busy`.

## 4. The service: `agent::endless::Endless` (one per engine, `Clone`, `Arc` inside)

Constructed by the supervisor (W2) next to the other side-call slots:
```rust
impl Endless {
    pub(crate) fn new(store: Store, rt: Runtime, client: genai::Client, auth: crate::supervisor::Auth,
                      catalog: Arc<models::Catalog>, cfg: EndlessConfig,
                      title_model: Option<String> /* agent.title_model, R1-D3 chain */) -> Self;   // crate-private: `Auth` is

    /// Engine start (109): one pump per `sessions.memory = 'endless'` session with unbuilt nodes.
    /// Returns how many pumps started. Models resolve from `sessions.model`; agent name from `sessions.desk` or "theuth".
    pub async fn resume_all(&self) -> anyhow::Result<usize>;

    /// A session actor came up (or `set_mode(Endless)` ran): register its model/effort/agent name and
    /// (for a private tree) its own memory `Store`. Idempotent; starts the pump when the mode is `endless`.
    pub async fn attach(&self, session: SessionId, attached: Attached) -> anyhow::Result<()>;
    // pub struct Attached { pub model: models::Resolved, pub effort: Option<genai::chat::ReasoningEffort>,
    //                       pub agent: String, pub store: Option<Store> /* a private tree's own store */ }
    /// The actor stopped: stop the pump, drop in-memory state. Nodes stay in the store.
    pub fn detach(&self, session: SessionId);

    /// `sessions.memory` (a store read; works for unattached sessions).
    pub async fn mode(&self, session: SessionId) -> anyhow::Result<MemoryMode>;
    /// The loop's switch (D14): attached ∧ mode == Endless ∧ backfill finished. Sync, cheap.
    pub fn active(&self, session: SessionId) -> bool;

    /// 8.1: wait until every view part is built, at most `max`. `true` = settled.
    pub async fn settle(&self, session: SessionId, max: Duration) -> bool;
    /// 8.1: the view for THIS turn. Freezes the rendered view as the `<chat>` context of every unit the
    /// turn will commit (D6). Call once per turn, after `settle`, BEFORE the input row is logged.
    pub async fn render_for_turn(&self, session: SessionId) -> anyhow::Result<ViewRender>;
    /// 8.3: rows committed to `messages` (any role, any time, incl. steering). A nudge: the pump
    /// derives units from the store itself (restart-safe), so no rows are passed.
    pub fn on_rows_committed(&self, session: SessionId);

    /// R1-D2 / R1b: the session's view budget — a USER control, persisted in `sessions.endless_view_bytes`
    /// (`None` clears it → `agent.endless.view_bytes`). Works attached or not. The fold re-fits at the next
    /// render/turn boundary: a smaller budget merges down; a larger one re-folds at the new budget — no
    /// text is ever cut (see §8 for the purity note). `SessionMemory.view_bytes` reports the value in force.
    pub async fn set_view_bytes(&self, session: SessionId, bytes: Option<u32>) -> anyhow::Result<()>;
    /// R1-D1: the turn's render without freezing a context (a native child's first message).
    pub async fn render_snapshot(&self, session: SessionId) -> anyhow::Result<Vec<ViewBlock>>;
    /// R1-D6: the view folded to `bytes` (W4: `agent.endless.meeting_view_bytes` for the meeting head).
    pub async fn render_with_budget(&self, session: SessionId, bytes: u32) -> anyhow::Result<Vec<ViewBlock>>;
    /// R1-D4: `{units, eta_secs, usd}` of the backfill a switch-on would start (or the one under way);
    /// first 20 built nodes calibrate, else 5 s / $0.02 per unit. Works for unattached sessions too.
    pub async fn backfill_estimate(&self, session: SessionId) -> anyhow::Result<BackfillEstimate>;

    /// Tools (§9). Errors are user-facing strings for the tool result.
    pub async fn zoom(&self, session: SessionId, id: u64, n: u64) -> Result<Vec<ViewLine>, String>;   // n==1 → one line, text = "kind: <whole unit>"
    pub async fn recall(&self, session: SessionId, query: &str, limit: u8) -> Result<Vec<ViewLine>, String>; // text ends with " (raw)" for fallback rows
    pub async fn date(&self, session: SessionId, id: u64) -> Result<Option<chrono::DateTime<chrono::Utc>>, String>;

    /// `/endless on|off` (8.6). Caller guarantees the session is idle; a transcript is fine (R1-D7 desks).
    /// `Endless`: fixes `filter` and the R1-D4 base (`max(0, T − backfill_max_units)`, filter applied) the
    /// FIRST time a tree is started for the session (later switches keep them), sets the column, starts the
    /// backfill (D14), returns `memory.backfill` with the ETA. `Epochs`: stops the pump, sets the column,
    /// returns `head = Some(text)` — the `<context_summary epoch=n style="endless">…</context_summary>`
    /// text W2 writes as the next epoch head. `filter` is ignored for `Epochs`.
    pub async fn set_mode(&self, session: SessionId, mode: MemoryMode, filter: UnitFilter, epoch: u32) -> anyhow::Result<SetMode>;

    /// RPC `session_memory` body data; `mode: epochs` + empty view for a non-endless session.
    pub async fn memory(&self, session: SessionId) -> anyhow::Result<SessionMemory>;

    /// The constant VIEW_DOC (§9) to append to the system prompt; `view_doc()` is byte-stable.
    pub fn view_doc() -> &'static str;
}

pub struct ViewRender { pub blocks: Vec<ViewBlock>, pub settled: bool, pub unbuilt: u32 }
/// One cache-marked text block. `blocks[0].text` starts with "<chat>\n", the last ends with "</chat>";
/// at most four blocks (three deepest band boundaries + end). `mark` = place a cache mark here
/// (Anthropic `cache_control: ephemeral` via genai `CacheControl::Ephemeral` on the user message's
/// `options`; OpenAI-like: same split, `prompt_cache_key = session`). W2 maps each block to one
/// `ContentPart::Text` of the first user message (or one user message each, provider permitting).
pub struct ViewBlock { pub text: String, pub mark: bool }
pub struct SetMode { pub memory: SessionMemory, pub head: Option<String> }
```

Frame contract for W2 (D4/§7.3, enforced by W2's tests): `[tools][system (+ VIEW_DOC)][user: view blocks …, input block]`; no head, no carried history; per-turn system items move into the input block. `Endless` never touches the frame.

Pump/event hook: the pump publishes `EndlessProgress` with `rt.publish(...)` on the `Runtime` given to `new`. **W2 adds `agent::EndlessProgress` to `rpc/src/bridge.rs` (`forward!` list + `rt.subscribe::<Bridge, EndlessProgress>`).**

## 5. Store (`crates/agent/src/store/endless.rs`, `impl Store`)

```rust
pub async fn session_memory(&self, id: SessionId) -> anyhow::Result<MemoryMode>;
pub async fn set_session_memory(&self, id: SessionId, mode: MemoryMode) -> anyhow::Result<()>;
pub async fn endless_meta(&self, id) -> anyhow::Result<EndlessMeta>;          // {mode, filter, base, view_bytes: Option<u32>} (sessions.memory / endless_filter / endless_base / endless_view_bytes)
pub async fn set_endless_meta(&self, id, filter: UnitFilter, base: u64) -> anyhow::Result<()>;
pub async fn set_endless_view_bytes(&self, id, bytes: Option<u32>) -> anyhow::Result<()>;   // R1b
pub async fn endless_sessions(&self) -> anyhow::Result<Vec<EndlessSessionRow>>;   // memory='endless': {id, model, effort, desk}
pub(crate) async fn delete_endless_in(conn: &mut SqliteConnection, sid: &str) -> anyhow::Result<()>; // called from delete_session
// units / nodes / calls — used by endless::* only; shapes in endless::tree
pub async fn endless_units(&self, id) -> anyhow::Result<Vec<UnitRow>>;                 // store::endless::UnitRow {i, seq, part, kind, size, turn}
pub async fn endless_units_put(&self, id, rows: &[UnitRow]) -> anyhow::Result<()>;
pub async fn endless_nodes(&self, id) -> anyhow::Result<Vec<endless::tree::Node>>;
pub async fn endless_node_put(&self, id, node: &Node) -> anyhow::Result<()>;          // upsert (ON CONFLICT DO UPDATE → the FTS update trigger fires)
pub async fn endless_calls_put(&self, id, l: u8, i: u64, calls: &[compactor::CallRow]) -> anyhow::Result<()>;
pub async fn endless_stats(&self, id) -> anyhow::Result<EndlessStats>;
pub async fn endless_fts_sync(&self) -> anyhow::Result<()>;                           // rebuild the FTS mirror if it drifted (replicas)
pub async fn endless_recall(&self, id, query: &str, limit: u32) -> anyhow::Result<Vec<(u64, String)>>; // (i, text) by rank
pub async fn endless_units_like(&self, id, needle: &str, limit: u32) -> anyhow::Result<Vec<u64>>;   // raw fallback: unit indexes whose message text contains needle
pub async fn endless_messages_from(&self, id, seq: i64) -> anyhow::Result<Vec<EndlessMessage>>;     // {seq, message: ChatMessage, excluded_whole: bool, at: Option<DateTime<Utc>>}, cut_at respected
```

### 5.1 Migration `crates/agent/migrations/0038_endless.sql`

Numbered 0038 on purpose: 0036 is the highest on the base, 0037 is left as the spare slot for design 136 (DREAMING, landing first); the closer renumbers if 136 takes more than one.
```sql
ALTER TABLE sessions ADD COLUMN memory TEXT NOT NULL DEFAULT 'epochs';
ALTER TABLE sessions ADD COLUMN endless_base INTEGER NOT NULL DEFAULT 0;      -- R1-D4
ALTER TABLE sessions ADD COLUMN endless_filter TEXT NOT NULL DEFAULT 'all';   -- R1-D6
ALTER TABLE sessions ADD COLUMN endless_view_bytes INTEGER NULL;              -- R1b (NULL = config)
CREATE TABLE endless_units (
  session_id TEXT NOT NULL REFERENCES sessions(id),
  i INTEGER NOT NULL, seq INTEGER NOT NULL, part INTEGER NOT NULL,
  kind TEXT NOT NULL, size INTEGER NOT NULL, turn INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (session_id, i));
CREATE TABLE endless_nodes (
  session_id TEXT NOT NULL REFERENCES sessions(id),
  l INTEGER NOT NULL, i INTEGER NOT NULL,
  text TEXT NOT NULL, size INTEGER NOT NULL, free INTEGER NOT NULL DEFAULT 0,
  model TEXT NULL, tries INTEGER NOT NULL DEFAULT 0, created_at TEXT NOT NULL,
  PRIMARY KEY (session_id, l, i));
CREATE TABLE endless_calls (
  call_id TEXT PRIMARY KEY, session_id TEXT NOT NULL REFERENCES sessions(id),
  l INTEGER NOT NULL, i INTEGER NOT NULL, try INTEGER NOT NULL,
  bytes INTEGER NULL, guard TEXT NULL, usage_json TEXT NULL, usd_estimate REAL NULL,
  latency_ms INTEGER NOT NULL, created_at TEXT NOT NULL);
CREATE INDEX endless_calls_session ON endless_calls(session_id, created_at);
CREATE VIRTUAL TABLE endless_lines_fts USING fts5(text, session_id UNINDEXED, i UNINDEXED, tokenize = 'porter unicode61');
-- triggers: AI/AU/AD on endless_nodes WHEN l = 0 → insert / delete+insert / delete in endless_lines_fts
```
`turn` is the per-session count of `render_for_turn` calls when the unit committed (the frozen-context key, D6). The three tables ride in `capture::ATTACHED` (replicated per session like `messages`); the FTS table is engine-local and rebuilt (`'rebuild'`) once by `resume_all` when its row count differs from level-0's.

## 6. Pure modules (unit-tested, no I/O)

- `endless::units`: `pub fn derive(seq: i64, at: Option<DateTime<Utc>>, m: &ChatMessage, cap_chars: u32, next_i: u64) -> Vec<Unit>` (§4 table; `Unit { i, seq, part, kind, text, size, at }`, `Kind { User, Talk, Tool, Echo, Work, Note }`). `work` = a user `Text` starting with `<message from="session:`.
- `endless::tree`: `Node { l: u8, i: u64, text, size, free }`, `Tree` (a `BTreeMap<(u8,u64), Node>` + `t: u64`), `id_n(l,i) -> (id, n)`, `addr(id, n) -> Option<(l,i)>`, `free_merge(a, b, node_bytes) -> Option<String>`.
- `endless::fold`: `Fold { parts: Vec<Part>, t: u64, cfg: FoldCfg { view_bytes, slack } }`, `Fold::rebuild(tree, t, cfg)` (the pure function of (tree, T)), `append(i)`, `fit(tree)`, `size(tree)`; `render(tree, fold, units_text: impl Fn(u64)->Option<String>) -> Vec<ViewBlock>` with D10 placeholders and the §7.2 marks; `render_lines(...) -> Vec<ViewLine>` for the panel.
- `endless::guards`: `Trip { Copy, Grounding, Status, Format, Empty, Size }`, `check(line, input, ruler, phrases, node_bytes) -> Result<String /*cleaned*/, Trip>`, `correction(trip, line, node_bytes) -> String`, `ruler(node_bytes) -> String`, `PROMPT_PHRASES`.
- `endless::compactor`: `COMPACT` prompt (`compact_prompt.txt`, `prompt(agent)`), `pub trait Compactor { fn compress<'a>(&'a self, ctx: &'a str, convo: &'a [ChatMessage]) -> BoxFuture<'a, Result<Reply, String>>; fn model(&self) -> &Resolved; }` (`Step::{Unit{kind,text,earlier}, Merge{a,b,earlier}}`, `Step::render(node_bytes)` = block 2), `ModelCompactor` over `backend::Backend` (`request(transport, ctx, convo)` places the marks), `build(&dyn Compactor, ctx, step, node_bytes, tries) -> Built { text, tries: i32, calls: Vec<CallRow> }` (size + guard retries, best-candidate rule), `estimate_usd` / `observation` (the `loop_::estimate` formula).
- `endless::pump`: per-session task; `resume_all`, rule-3 frontier ordering, `jobs`, retry, progress throttle, backfill accounting, the §9 tools, `set_mode`, `off_head(epoch, view)`.

## 7. Decisions recorded here (design silent or adjusted)

1. `on_rows_committed` takes no rows: units are derived from `messages` beyond the last cached `seq`, so a restart or a missed nudge never loses units.
2. Frozen contexts live in memory per session (last 8 turns); after a restart an unbuilt node uses the current view as `<chat>` (cache efficiency, not correctness).
3. Private sessions: `attach(.., store: Some(private_store))`; the pump uses that store; nothing of theirs touches the durable store.
4. Pricing: `endless_calls.usd_estimate` uses the catalog `models::Cost` formula of `loop_::estimate` (private; duplicated verbatim in `compactor.rs`); the same observation is also settled into `model_calls` as `CallKind::Endless` via `Store::reserve_call`/`settle_call`.
5. `active()` flips when every level-0 node for units before the backfill mark is built (the tree "covers its history"). The mark is "units with `turn == 0`" — rows that committed outside an endless turn (history at `/endless on`, a desk's existing transcript); a fresh endless session or a restart mid-session never backfills.
6. The live view is `Fold::rebuild(tree, T)` recomputed on change (cached), i.e. literally the pure function; the lab's incremental fold with a progressively built tree is not reproducible from rows, so purity wins over its last few points of prefix stability (measured on the fixture: identical part-prefix share 0.7+ turn to turn at 32 KB).
7. The compactor's context block and step block are two consecutive `user` messages (genai marks are per message); Anthropic merges consecutive same-role turns. If a provider rejects that, W2/closer: fold them into one message with two `ContentPart::Text`s (loses the per-block mark).
8. One `model_calls` row per node build (attempts' usage summed), `endless_calls` one row per attempt.

## 8. Revision 1 (design §17) — what changed here

- Config: `view_bytes` 128_000, `default_desk` true, new `default_meeting`, `meeting_view_bytes`, `backfill_max_units`; `resolved_compactor_model(&AgentConfig, session_model)`; `Endless::new` takes `title_model`.
- Base (R1-D4): the fold tiles `[base, T)`; the view's first line is `0+<base>|(earlier history, not summarized; recall searches it)` when `base > 0` (`fold::EARLIER`); `zoom(id, 1)` works for any unit, `zoom(id, n>1)` below the base is `No line`; recall's raw fallback searches every unit (it always did: `endless_units` caches units below the base too — they are derived and cached, just never built). `SessionMemory.view[0]` is that line (`built: false`, `n = base`).
- Filter (R1-D6): applied at ingestion; dropped parts never become units (indexes stay dense). Fixed at first switch-on with the base, persisted in `sessions`.
- Per-session budget (R1-D2 / R1b): `set_view_bytes(session, Option<u32>)` persists to `sessions.endless_view_bytes` and takes effect at the next render. Smaller budget: the re-fold merges down. Larger budget: because the live view is the pure `rebuild(tree, base, T, cfg)`, the re-fold at the new budget shows the view the fold *would* have produced at that budget — finer lines in the recent region, every one an existing built node, no text cut. This differs from the brief's "a larger one just stops merging": honouring that literally would make the view path-dependent (unreproducible after a restart); the closer may overrule by keeping the previous `Fold` and only updating `cfg` + `fit()` (one-line change in `Live::ensure_fold`).
- `render_snapshot` / `render_with_budget(bytes)`: no turn counted, no context frozen; `render_with_budget` re-folds at `bytes` without disturbing the live fold.
- Backfill ETA/cost: `backfill_estimate`; `Backfill.total` is now the covered span (`T − base` at switch-on), not `T`.
- W2 call for R1-D7 desks at landing: `set_mode(desk, Endless, UnitFilter::All, next_epoch)` when idle; `info!` the `backfill_estimate` first. W4 meetings: `set_mode(meeting, Endless, UnitFilter::Meeting, …)`, head section from `render_with_budget(meeting, cfg.endless.meeting_view_bytes)`.
