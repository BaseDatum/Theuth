# 141 hand-off — C (CONTRACT)

Branch `feat/141-acp-subagents`, commit `5ba6ace9` + the stamp commit after it. Workers W1–W5 cut from it.
Everything below compiles workspace-wide; **no behaviour change** beyond the
SDK bump and the carried-but-unrouted wire `sessionId`.

## 1. SDK (D1, §4.1)

`Cargo.toml` (`:268`): `agent-client-protocol = { version = "=3.1.0", features = ["process", "stdio", "unstable_end_turn_token_usage", "unstable_subagents"] }`.

Lock: `cargo update -p agent-client-protocol --precise 3.1.0` moved exactly
`agent-client-protocol 2.2.0→3.1.0`, `-derive 2.2.0→3.1.0`, `-schema 1.9.1→1.10.2`
and dropped the `schema→schemars` and `schema→tracing` edges (15 lines, nothing
else). `cargo tree -p acp -e features -i agent-client-protocol` lists the four
features; no `schemars` feature.

## 2. Migration — **0041 confirmed**

`crates/agent/migrations/0041_observed_children.sql`. Checked before writing:
`ls crates/agent/migrations | tail -3` → `0038 0039 0040_harness_outbox.sql`;
main's last migration commit is 139-R2 (`58d30af3`). **One deviation from
§11.1**: a ninth column `observed_cost_rule TEXT NULL` — §5.1/§8.1 make
`ObservedMeta.cost_rule` a stored, per-row value that is *not* derivable from
`observed_source` alone (omp background vs sync, §7.6), and the verbatim SQL had
no column for it. NULL reads as the source's default (`store::default_cost_rule`:
rfd → `display_only`, opencode_* → `unreported`, omp_task → `counted`).
Indexes `sessions_parent` and `sessions_observed_identity` as specified.

## 3. Contract types as landed

### 3.1 `crates/agent/src/lib.rs` (§8.1)

```rust
pub enum SessionKind { Chat, Advisor, Observed }            // "observed"

// cost::str_enum! → derives + as_str() + Display + FromStr (stored strings = serde names)
pub enum ObservedSource { Rfd, OpencodeExt, OpencodeTask, OmpTask }
pub enum ObservedState  { Pending, Running, RequiresAction, Idle, Completed, Failed, Cancelled, Unknown }
impl ObservedState { pub fn is_terminal(self) -> bool }     // completed | failed | cancelled

pub use acp::CostRule;   // Counted | InsideParent | DisplayOnly | Unreported — defined in acp (see 3.2)

pub struct ObservedMeta {
    pub source: ObservedSource,
    pub external: String,                 // = sessions.acp_session
    pub state: ObservedState,
    pub stop: Option<String>,             // ts: optional = nullable
    pub can_cancel: bool,
    pub description: Option<String>,
    pub tool_call: Option<String>,
    pub cost_rule: CostRule,
    #[serde(default)] pub detail: serde_json::Value,   // Null when none stored
}

AgentEvent::ObservedChildChanged { parent: SessionId, child: SessionId, name: String,
                                   model: Option<String>, observed: ObservedMeta }
// AgentEvent::session() → Some(child)

AgentInfo      { …, kind: SessionKind, #[serde(default)] observed: Option<ObservedMeta> }
SessionSummary { …, kind, memory, #[serde(default)] observed: Option<ObservedMeta> }
Loaded         { …, resume, #[serde(default)] kind: SessionKind, #[serde(default)] observed: Option<ObservedMeta> }
```

`crates/agent/src/cost.rs`: `CostScope::Observed = "observed"`; `str_enum!` is
now `pub(crate) use`d so lib.rs can reuse it.

### 3.2 `crates/acp/src/harness.rs` (§7.1), re-exported from `crates/acp/src/lib.rs`

```rust
pub enum Update {
    Session { session: String, update: SessionUpdate },   // session = wire params.sessionId
    Subagent(ChildAnnounce),
    Permission(Permission),                                // Permission gained `pub session: String`
    Peer(PeerMessage),
    Closed(String),
}

#[serde(rename_all = "snake_case")]
pub enum ChildSource { Rfd, OpencodeExt, OpencodeTask, OmpTask }

#[serde(rename_all = "snake_case")]  // + schemars + ts(export); as_str/Display/FromStr
pub enum CostRule { Counted, InsideParent, DisplayOnly, Unreported }

#[serde(tag = "state", rename_all = "snake_case")]
pub enum ChildState {
    Pending, Running, RequiresAction,
    Idle { stop_reason: Option<String>, usage: Option<schema::v1::Usage> },
    Completed, Failed { error: Option<String> }, Cancelled, Unknown,
}

pub struct ChildAnnounce {
    pub source: ChildSource, pub child: String, pub parent: Option<String>,
    pub title: Option<String>, pub description: Option<String>, pub model: Option<String>,
    pub state: Option<ChildState>, pub can_cancel: Option<bool>, pub tool_call: Option<String>,
    pub cost_rule: Option<CostRule>,
    #[serde(default)] pub detail: serde_json::Map<String, serde_json::Value>,
}
impl ChildAnnounce { pub fn new(source: ChildSource, child: impl Into<String>) -> Self }  // identity only

impl Harness { pub fn cancel_session(&self, child: &str) -> Result<(), String> }  // CancelNotification::new(SessionId(child.into())) — working body
```

`CostRule` lives in `acp` (not `agent` as §8.1's table says) because
`ChildAnnounce.cost_rule` needs it and `agent` depends on `acp`, not the
reverse; `agent` re-exports it, so `agent::CostRule` is the same type. The
`agent` crate's `ts` feature now enables `acp/ts` so the binding exports.

Rename note: `ChildSource` (acp) ↔ `ObservedSource` (agent) are distinct enums
with identical `snake_case` names — W2 maps with a 4-arm `match` (or
`a.source.as_str().parse()` via serde).

### 3.3 Handlers (`Link::spawn`) — carried, not routed

- typed `SessionNotification` → `Update::Session { session: n.session_id.0.to_string(), update: n.update }` (the id is no longer dropped).
- untyped `child_update` → `Update::Session { session: params.rootSessionId, update }` — same flattened payload as before; W1 replaces with announces + child traffic.
- `RequestPermissionRequest` → `Permission { session: r.session_id.0.to_string(), … }`.

### 3.4 `crates/acp/src/omp/mod.rs` — stub signatures (W5 replaces the module)

`TaskFrame { tool_call_id, background, batch_state, rows: Vec<Progress> }`;
`Progress` (all F19 fields, `camelCase` serde, everything but `id`/`status` optional; `RecentTool`, `RetryState`);
`Progress::announce(&self, &TaskFrame) -> ChildAnnounce` (identity, nested
parent, tool call, cost rule from `background`, state from `status`, model —
**no `detail`** yet);
`progress_rows(&SessionUpdate) -> Option<TaskFrame>` → `None`;
`Tailer { home, parent_file, artifacts }` with `locate → None`, `child_path`
(real), `read → (vec![], cursor)`, `sidecar_done → false`, `settled_jobs → (vec![], cursor)`;
`Call`, `Usage`, `Entry::{Session, Model, User, Assistant, ToolStart, ToolResult, Other}`;
`to_updates(&[Entry]) -> Vec<SessionUpdate>` → empty.

## 4. Store API (`crates/agent/src/store.rs`, §11.2)

```rust
pub struct ObservedUpsert<'a> { pub parent: SessionId, pub source: ObservedSource, pub external: &'a str,
    pub name: Option<&'a str>, pub model: Option<&'a str>, pub state: Option<ObservedState>,
    pub stop: Option<Option<&'a str>>, pub can_cancel: Option<bool>, pub description: Option<&'a str>,
    pub tool_call: Option<&'a str>, pub cost_rule: Option<CostRule>,
    pub detail: Option<&'a serde_json::Map<String, serde_json::Value>> }
impl ObservedUpsert<'a> { pub fn new(parent, source, external) -> Self }   // all else None

impl Store {
    pub async fn upsert_observed(&self, u: ObservedUpsert<'_>) -> Result<(SessionId, bool /*created*/)>;
    pub async fn observed_children(&self, parent: SessionId) -> Result<Vec<(SessionSummary, ObservedMeta)>>; // direct children, oldest first
    pub async fn observed_child(&self, parent, source: ObservedSource, external: &str) -> Result<Option<SessionId>>;
    pub async fn observed_cursor(&self, id) -> Result<Option<u64>>;          // extra: W5/W2 resume read
    pub async fn set_observed_cursor(&self, id, cursor: u64) -> Result<()>;
    pub async fn mark_observed_lost(&self, live_parents: &[SessionId]) -> Result<Vec<SessionId>>;
}
pub fn default_cost_rule(ObservedSource) -> CostRule;
```

Semantics of `upsert_observed`: resolve by `(parent, observed_source, acp_session)`;
create → `kind='observed'`, `agent` NULL, `model` = parent's `harness_model` else
`model`, `name`/`title` = `name` (else `external`), `observed_state='pending'`,
cost owner via `ensure_owner_in(id, Some(parent))`; a parent without a row is an
error. Then one `UPDATE … COALESCE(?, col)` applies the `Some` fields;
`stop: Some(None)` clears; `detail` merges top-level keys into the stored JSON.
`SessionRow`/`session_cols!` carry `acp_session` + the nine `observed_*`
columns; `SessionSummary::observed` is `Some` iff `observed_source` is set.
`kind_str`/`kind_of` (`store/advisor.rs`) know `"observed"`.

Tests (`cargo test -p agent --lib -- store::`): `observed_upsert_is_idempotent_per_parent_source_external`,
`observed_children_lists_under_the_acp_child_in_order` (also asserts
`descendants` pre-order with an observed grandchild), `kind_of_observed_round_trips`,
`mark_observed_lost_touches_non_terminal_rows_only`, `observed_cursor_round_trip`,
`migration_0041_adds_the_observed_indexes`.

## 5. Revision 1 Q7 — dreaming exclusion: **done here**

The dreaming session walk is `Store::ledger_since` in
`crates/agent/src/store/dreaming.rs:122` (store code, nobody's wave file). The
listed-sessions loop now skips `kind == "observed"` beside `"advisor"`; lineage
(`root_of`) still sees the rows. Nothing for W2 to do.

## 6. Stub sites other waves replace

| site | marker | wave |
|---|---|---|
| `crates/acp/src/harness.rs` untyped-notification handler (`child_update` branch, `rootSessionId` attribution) | `// 141 C stub: W1 replaces` | W1 |
| `crates/acp/src/omp/mod.rs` (whole module) | module doc + per-fn `141 C stub` | W5 |
| `crates/agent/src/acp_child.rs` `impl Message<Update>`: `Update::Subagent(a) => debug!` arm; `Update::Session { update, .. }` ignores `session` | `// 141 C stub: W2 replaces` / `// 141 C:` | W2 |
| `crates/tui/src/model.rs` transcript match: `AgentEvent::ObservedChildChanged { .. } => {}` | `// 141 C stub: W4 replaces` | W4 |
| `crates/slack/src/channel.rs` `Thread::apply` silent arm gained `ObservedChildChanged` | `// 141 C stub` | W4 (stays silent per §10.2; W4 adds the `route()` board row) |
| `crates/telegram` | no exhaustive `AgentEvent` match → untouched | W4 |

rpc `admitted`/`wanted` (`api.rs`) are **real**, not stubs: `announces_child()`
treats `ObservedChildChanged` like `SubagentSpawned`; test
`observed_child_changed_enrols_like_subagent_spawned`.

## 7. Files touched outside C's list (mechanical, compile-only)

- `crates/rpc/src/acp.rs:1267` loopback test: `Update::Session { session, update }` destructure (+ asserts `session == harness.session_id()`).
- `crates/agent/src/store/dreaming.rs:122` (Q7, §5).
- `crates/agent/Cargo.toml` `ts` feature += `acp/ts`.
- `crates/agent/src/supervisor.rs` `info_row` (`observed: row.observed.clone()`) and `LoadSession` (`kind`, `observed` from the row) — plumbing only, so `list_agents`/`load_session` carry the fields; W2 owns the file from here.
- `crates/tui/src/model.rs:12041-12057` `/cost` rows: `CostScope` is matched exhaustively there (F29 missed it); the `Observed` arms read "watch-only subagent, counted beside the harness" / " · watch-only" — W4 may reword.
- Struct literals gaining `observed: None`: `agent/session_read.rs`, `slack/{board,ui}.rs`, `telegram/{commands,ui}.rs`, `tui/{model,attach}.rs`, `roster/desk.rs` (tests, plus the `SubagentSpawned` row at `tui/model.rs:5965`).

## 8. Bindings

`just ts` writes `/bindings` (gitignored) and `ts-desktop` copies into
`crates/desktop/ui/src/rpc/bindings/` (gitignored). `crates/agent/bindings/*.ts`
is a stale tracked copy from 2026-09-26 that nothing reads (no `kind`, no
`SessionKind.ts`); left alone. New generated files: `ObservedMeta.ts`,
`ObservedSource.ts`, `ObservedState.ts`, `CostRule.ts`; changed:
`SessionKind.ts`, `AgentEvent.ts`, `AgentInfo.ts`, `SessionSummary.ts`,
`Loaded.ts`, `CostScope.ts`. Hand-written `src/rpc/extra.ts` `AgentInfo` gained
`observed?: ObservedMeta | null` and the `kind` doc line. `Loaded.kind` is
`ts(optional = nullable)` (→ `kind?: SessionKind`), matching `AgentInfo`/
`SessionSummary`; three existing W3 `Loaded` literals would otherwise fail `tsc`.

## 9. Gates run (all green)

| gate | result |
|---|---|
| `cargo tree -p acp -e features -i agent-client-protocol` | default, process, stdio, unstable_end_turn_token_usage, unstable_subagents; no schemars |
| `git diff Cargo.lock` | 15 lines: the three SDK packages + dropped `schemars`/`tracing` edges |
| `cargo check -p acp --all-targets` | ok |
| `cargo test -p acp` | 23 passed |
| `cargo test -p agent --lib -- store::` | 93 passed (6 new, §4) |
| `cargo test -p rpc --lib -- api::` | 26 passed (incl. `observed_child_changed_enrols_like_subagent_spawned`) |
| `just ts-desktop` (`cargo test -p rpc --features ts export_bindings` + copy) | 8 passed; bindings in §8 written |
| desktop `bun run check` (`tsc -b --noEmit`) | clean |
| `cargo check -p tui -p slack -p telegram -p roster --all-targets` | ok |
| `cargo check --workspace --all-targets` (under `/tmp/theuth-build.lock`, `BUILD_SLOTS=1`) | ok (only the pre-existing `block v0.1.6` future-incompat note) |

Not run (not mine): clippy, `cargo test -p tui/slack/telegram`, vitest, vite build.
