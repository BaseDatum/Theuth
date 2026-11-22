# 208 contract — Attention graph (round 2: builders B and D, integrator F)

Single source of truth for **B** (Rust graph engine), **D** (desktop UI canvas, on fixtures) and **F** (integration). Design and rationale: `docs/design/208-attention-graph.md` (read §0, §3 and §7 first). Base: branch `feat/208-attention-graph` at the commit that adds this file, off main `fab7d5f7a`. B and D each work in their own worktree off that commit; nobody talks to the other; where this file and the code disagree, the code wins and the builder records the deviation in `docs/design/208-as-built-<B|D>.md`.

---

## 0 Rules for everyone (restated; binding)

These are the machine's rules as practised in round 1 (`207-as-built.md` "Gates", `/tmp/theuth-207-*-gate.sh`); `207-contract.md` §1/§11 states only the first two of them and quotes workspace-wide `just` gates, which **do not apply** this round.

1. **One build at a time, machine-wide.** Every cargo, bun, tsc, vitest or vite command runs inside a gate script that holds `/tmp/theuth-build.lock` (a directory lock: `until mkdir /tmp/theuth-build.lock 2>/dev/null; do sleep 10; done; trap 'rmdir /tmp/theuth-build.lock' EXIT`). Never create, remove or wait on it by hand outside such a script; never run a build while another session holds it.
2. **One hold per gate script.** A script takes the lock once, runs its steps, releases it on exit. After a release wait **≥ 45 s** before taking it again (others queue).
3. `CARGO_BUILD_JOBS=4`, `--test-threads=4`, and **your own `CARGO_TARGET_DIR`** inside your worktree (`<worktree>/target`). If you clone a `target/` from another worktree, delete `target/debug/build/{mlx-sys,opusic-sys,apple-metal,screencapturekit,apple-cf}-*` first (207 lost two holds to cached absolute paths).
4. **No workspace-wide commands**: no `cargo build/test/clippy --workspace`, no `just clippy`, `just test`, `just fmt`. Crate-scoped (`-p agent -p rpc-compose -p theuth`) only. `cargo fmt -p <crate>` is fine.
5. Skip build/lint/tests mid-flight; write code, then run your gate once; fix; re-run.
6. Commit with `git -c commit.gpgsign=false commit` (a plain commit hangs on GPG pinentry). Commit after every green step.
7. **Hard time budget: 75 minutes per builder**, commits after every green step; at the budget, commit what is green, write the as-built note with what is not done, stop.
8. No emoji anywhere (`crates/desktop/ui/src/lib/no-emoji.test.ts`; arrows U+2190–21FF are allowed). lucide icons, shadcn primitives.
9. Do not edit a file outside your ownership (§1). Need a change there? Record it in your as-built note for F.

Gate script names: `/tmp/theuth-208-b-gate.sh`, `/tmp/theuth-208-d-gate.sh`, `/tmp/theuth-208-f-gate.sh`; logs beside them (`.log`). Copy the structure of `/tmp/theuth-207-i2-gate.sh` (lock loop, trap, `run name cmd…` writing filtered output to the log).

---

## 1 Ownership (no overlap)

**B may touch:**
- new `crates/agent/src/graph.rs` (and `crates/agent/src/graph/*.rs` if split), new `crates/agent/src/rpc_graph.rs`
- `crates/agent/src/lib.rs`: `mod`/`pub use` lines, `rpc_module` merge list (`:64-78`), `declare_wire` (`:4426-4437`), `Plugin::start` (spawn the actor)
- `crates/agent/src/communication.rs`: `PeerMessageChanged` only (`:767-782`)
- `crates/agent/src/supervisor.rs`: `peer_changed` only (`:2204-2224`); a new `pub(crate)` accessor if the actor needs one (no behaviour change)
- `crates/agent/src/store.rs`, `crates/agent/src/store/communication.rs`: new read/write functions only (no change to existing ones)
- new `crates/agent/migrations/0073_graph.sql`
- `crates/rpc-compose/src/client_mirror.rs` (one entry), `crates/rpc-compose/tests/fixtures/wire/{agent.graph.json (new), agent.peer-message.json}`, `crates/rpc-compose/tests/{wire_golden.rs, wire_topics.rs}` (one family line each), any `wire_parity_inc*.rs` that lists agent families
- `crates/sessionsync/src/events.rs`: only if the `Copy` removal needs it (`:566-573`)
- `crates/theuth/tests/fixtures/wire/openrpc-v1.json` — only via the accept step in §5.4
- `docs/design/208-as-built-B.md`

**D may touch:**
- new `crates/desktop/ui/src/components/attention/graph/**`
- `crates/desktop/ui/src/components/attention/model.ts` + `model.test.ts`: the subtree filter (§8.2) only
- `crates/desktop/ui/src/components/attention/Page.tsx`: mount the region, highlight, subtree, show-in-graph wiring (§8.6)
- `crates/desktop/ui/src/components/attention/Toolbar.tsx`: the subtree chip (§8.6)
- `crates/desktop/ui/src/components/attention/Card.tsx` (+ `Card.test.ts`): `onShowInGraph` (§8.6)
- `crates/desktop/ui/src/components/sidebar/AgentTree.tsx`: extract `stateTone` (§8.5), no behaviour change
- `crates/desktop/ui/package.json`, `crates/desktop/ui/bun.lock`: `@xyflow/react` exactly `13.0.2` (`bun add @xyflow/react@13.0.2 --exact`, inside the D gate hold)
- new `crates/desktop/ui/src/dev/attention-graph-fixture.{html,tsx}`
- `crates/desktop/ui/src/rpc/bindings/**` — git-ignored, hand-patched per §7, **never committed** (copy the generated tree from `/Users/sky/Develop/skymoore/theuth/crates/desktop/ui/src/rpc/bindings/` first)
- `docs/design/208-as-built-D.md`

**F:** integration only (§11): merges, regenerated bindings, the gates, literal fixes where the generated types differ from §7, `docs/design/208-as-built.md`.

**Nobody touches:** any `sessions` migration or column, `SESSIONS_WIRE_SCHEMA` (stays **70**, `crates/agent/src/store.rs:732`), `store::capture::ATTACHED` (`crates/agent/src/store/capture.rs:19-40`), `crates/roster/**`, `AgentState`, companions (`crates/desktop/companion/**`, `crates/desktop/companion-windows/**`), `lib/shortcuts.ts`, `lib/hotkeys.ts`, chat diagram code (`components/chat/surface/Graph.tsx`, `lib/graph-layout*`).

---

## 2 Rust types (B) — `crates/agent/src/graph.rs`

Derives exactly as the neighbouring wire types (`SessionActivity`, `crates/agent/src/lib.rs:1732`; `PeerMessageChanged`, `communication.rs:767-782`). Drop `Eq` from a type if a field does not allow it (`ObservedMeta` carries `detail: serde_json::Value`): then use `PartialEq` only and say so in the as-built note. Re-export every public type from `lib.rs` (`pub use graph::{…}`) so they are `agent::AgentGraph` etc.

```rust
//! 208: the Attention graph — one aggregate of every active and recently active session tree and the peer messages
//! between them, served as one snapshot (`agent_graph`) and one coalesced delta topic (`agent.graph`). Lease-driven:
//! dormant (only the `last_event_ms` clock) until a client asks; see `docs/design/208-attention-graph.md` §5.

use runtime::SessionId;
use serde::{Deserialize, Serialize};

use crate::{ObservedMeta, SessionActivity};

/// 208 D9: what a graph node is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum GraphNodeKind {
    /// A root chat or routine run (no parent, no desk).
    Root,
    /// A hired agent's desk session.
    Desk,
    /// A subagent: native, or an ACP harness child (`agent` set).
    Child,
    /// A watch-only child a harness runs inside itself (141 D2).
    Observed,
    /// A boundary endpoint outside this engine's sessions: a mesh peer or a contact (`peer:<name>`).
    Peer,
}

/// 208 D7: what a session is parked on besides its `AgentState`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum GraphWait {
    /// A pending question to the user: ask, hand-off or secret (`waits.kind = 'question'`).
    User,
    /// A shell job or pty (`waits.kind` `job` | `pty_wait`).
    Job,
    /// Its children's results (`waits.kind = 'child_result'`).
    Children,
}

/// 208 D7: how the session's last turn ended (207 §4's mapping of `stop_reason`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum GraphOutcome {
    /// Any provider stop reason that is not one of the three below.
    Finished,
    /// `error`.
    Failed,
    /// `cancelled`.
    Cancelled,
    /// `timed_out`.
    TimedOut,
}

/// 208 §2.1: one session (or boundary endpoint) on the graph.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct GraphNode {
    /// Session id; a boundary node is `peer:<name>`.
    pub id: String,
    /// What the node is.
    pub kind: GraphNodeKind,
    /// The parent session; `None` for roots, desks and boundary nodes.
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub parent: Option<String>,
    /// The root of the node's tree (itself for a root, desk or boundary node).
    pub root: String,
    /// Root/desk: the title (empty → `<desk> desk`, else the short id); child: the peer name; boundary: the name.
    pub title: String,
    /// The peer name of a child; `None` for roots, desks and boundary nodes.
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub name: Option<String>,
    /// The harness (`agents.yaml` key) of an ACP session; `None` for theuth sessions.
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub agent: Option<String>,
    /// The session's model id ("" for boundary nodes).
    pub model: String,
    /// The root's project.
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub project: Option<String>,
    /// The desk's roster agent.
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub desk: Option<String>,
    /// Live facts (`ActivityOf`); remote-owned: `Running` when `SessionSummary.running`, else `Idle`; boundary: idle default.
    pub activity: SessionActivity,
    /// What it waits on besides its state, sorted, deduplicated.
    pub waits: Vec<GraphWait>,
    /// The last turn's outcome while the session is not running; `None` before any turn, while running, and for
    /// `interrupted` / `suspended` / `harness_continued`.
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub outcome: Option<GraphOutcome>,
    /// Watch-only children: the harness's view (141).
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub observed: Option<ObservedMeta>,
    /// The owning engine (`SessionSummary.owner`; "" for boundary nodes).
    pub owner: String,
    /// Owned by another engine: state is last-known (208 §6).
    pub remote: bool,
    /// ms since epoch the session was created (boundary: its first exchange).
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub started_ms: u64,
    /// ms since epoch of the session's last event of any kind (208 §2.3); boundary: its newest exchange.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub last_event_ms: u64,
}

/// 208 D8: the peer messages between one unordered pair of nodes, `a < b` (byte order of the ids).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct GraphEdge {
    /// `"<a>|<b>"`.
    pub id: String,
    /// The smaller node id.
    pub a: String,
    /// The larger node id.
    pub b: String,
    /// Exchanges sent a → b in the window.
    pub ab: u32,
    /// Exchanges sent b → a in the window.
    pub ba: u32,
    /// Of those, a child's final result to its parent (request id `result:*`).
    pub results: u32,
    /// Exchanges still in flight (`accepted` or `held`).
    pub open: u32,
    /// ms since epoch of the newest exchange change.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub last_ms: u64,
    /// Sender of the newest exchange (`a` or `b`).
    pub last_from: String,
    /// The newest exchange's id (for the popover).
    pub last_id: String,
}

/// 208 §2.2: what happened to one exchange (a live edge event).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum GraphPulseKind {
    /// Accepted into the recipient's intake.
    Sent,
    /// Consumed by the recipient's turn.
    Delivered,
    /// Answered.
    Replied,
    /// A child's final result reached its parent.
    Result,
    /// Suppressed or failed.
    Failed,
}

/// 208 §2.2: one exchange's state change, for the UI to animate along `edge`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct GraphPulse {
    /// The edge (`GraphEdge.id`).
    pub edge: String,
    /// Sender node id.
    pub from: String,
    /// Recipient node id.
    pub to: String,
    /// What happened.
    pub kind: GraphPulseKind,
    /// The exchange id.
    pub id: String,
    /// ms since epoch.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub at_ms: u64,
}

/// 208 D10: idle trees left out of a snapshot over the node cap.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct GraphOlder {
    /// Trees left out.
    pub roots: u32,
    /// Nodes in them.
    pub nodes: u32,
    /// The newest `last_event_ms` among them.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub newest_ms: u64,
}

/// 208 §4: where a client is in the delta stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct GraphCursor {
    /// The activation (`AgentGraph.epoch_ms`).
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub epoch_ms: u64,
    /// The last applied `seq`.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub seq: u64,
}

/// 208 §4: the `agent_graph` reply.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct AgentGraph {
    /// ms since epoch this activation started; a new value = a new stream (`seq` restarts).
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub epoch_ms: u64,
    /// The last published delta's `seq` (0 = none yet); the next delta is `seq + 1`.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub seq: u64,
    /// ms since epoch of this reply.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub at_ms: u64,
    /// The window served (requested, defaulted and clamped).
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub window_ms: u64,
    /// The caller's cursor was current: every list is empty, keep your state.
    pub unchanged: bool,
    /// Every node in the window, parents before children.
    pub nodes: Vec<GraphNode>,
    /// Every message edge whose two ends are in `nodes`.
    pub edges: Vec<GraphEdge>,
    /// Trees left out over the node cap.
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub older: Option<GraphOlder>,
}

/// 208 §4: coalesced changes since the previous event (at most one per `FLUSH_MS`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[schemars(transform = wire::serde_type_tag::<Self>)]
#[serde(tag = "type")]
pub struct AgentGraphChanged {
    /// The activation this belongs to.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub epoch_ms: u64,
    /// 1-based per activation, +1 per event.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub seq: u64,
    /// ms since epoch.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub at_ms: u64,
    /// Nodes added or changed (whole nodes), parents before children.
    #[serde(default)]
    pub nodes: Vec<GraphNode>,
    /// Node ids gone (session deleted); their edges go too.
    #[serde(default)]
    pub removed: Vec<String>,
    /// Edges added or changed (whole aggregates).
    #[serde(default)]
    pub edges: Vec<GraphEdge>,
    /// Edge ids gone.
    #[serde(default)]
    pub edges_removed: Vec<String>,
    /// Exchange state changes since the previous event, oldest first, at most `MAX_PULSES`.
    #[serde(default)]
    pub pulses: Vec<GraphPulse>,
    /// The current over-cap stub (unchanged when equal to the last one sent).
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub older: Option<GraphOlder>,
    /// Too much changed: every list is empty; call `agent_graph` again.
    #[serde(default)]
    pub resync: bool,
}

impl wire::WireEvent for AgentGraphChanged {
    const TOPIC: &'static str = "agent.graph";

    fn policy(json: &serde_json::Value) -> wire::EventPolicy {
        wire::EventPolicy {
            always_wanted: true,
            ..wire::EventPolicy::legacy_fields(json)
        }
    }
}
```

`PeerMessageChanged` (`communication.rs:771`) — drop `Copy` from its derive (keep the rest), append:

```rust
    /// 208 D6: the sender endpoint (`session:<uuid>` | `peer:<name>`).
    #[serde(default)]
    pub from: String,
    /// 208 D6: the recipient endpoint, as addressed.
    #[serde(default)]
    pub to: String,
    /// 208 D6: the exchange is a child's final result to its parent (request id `result:*`).
    #[serde(default)]
    pub result: bool,
```

`peer_changed` (`supervisor.rs:2204-2224`) already reads `exchange_record(receipt.id)`: fill `from = rec.from`, `to = receipt.to`, `result = rec.request_id.starts_with("result:")` (if the record lacks `request_id`, add it to the record read; when the record is missing: `from = ""`, `result = false`). The per-participant publish loop stays.

---

## 3 Migration (B) — `crates/agent/migrations/0073_graph.sql`

Next free number verified: `0001`–`0072` exist with no gap (`0072_endless_ledger.sql`).

```sql
-- 208: the Attention graph. Engine-local: neither table is in store::capture::ATTACHED, so nothing replicates and
-- SESSIONS_WIRE_SCHEMA stays 70. No replicated table changes.

-- 208 §2.3: the time of each session's last bus event, upserted in 30 s batches with max().
CREATE TABLE session_last_event (
    session TEXT    PRIMARY KEY NOT NULL,
    at_ms   INTEGER NOT NULL
);

-- 208 §5: the window read of message history (`WHERE updated_at >= ?`).
CREATE INDEX peer_exchanges_updated ON peer_exchanges (updated_at);
```

`updated_at` is RFC 3339 `TEXT`; the bound is compared as text. Fractional seconds of varying length can misorder rows inside one second; harmless for a window bound (note it in a comment of the query). If a store test enumerates every table and requires it to be `ATTACHED` or listed as local, add `session_last_event` to that local list (the module doc of `store/capture.rs:3-5` names the local tables: add it there too). `SessionDeleted` deletes the session's row.

Store functions (new, in `store.rs` or `store/communication.rs`):

```rust
pub async fn upsert_last_events(&self, rows: &[(SessionId, u64)]) -> anyhow::Result<()>; // one transaction, at_ms = max(at_ms, excluded.at_ms)
pub async fn last_events(&self, ids: &[SessionId]) -> anyhow::Result<HashMap<SessionId, u64>>;
pub async fn delete_last_event(&self, id: SessionId) -> anyhow::Result<()>;
pub async fn graph_waits(&self, ids: &[SessionId]) -> anyhow::Result<HashMap<SessionId, Vec<String>>>; // SELECT session_id, kind FROM waits WHERE session_id IN (…)
pub async fn last_stops(&self, ids: &[SessionId]) -> anyhow::Result<HashMap<SessionId, String>>; // sessions.last_stop
pub async fn exchanges_since(&self, since: DateTime<Utc>, limit: i64) -> anyhow::Result<Vec<ExchangeEdgeRow>>;
// SELECT id, sender, recipient, sender_session, recipient_session, request_id, state, created_at, updated_at
//   FROM peer_exchanges WHERE updated_at >= ? ORDER BY accepted_seq DESC LIMIT ?
```

Verify (and record) that a `waits` row is deleted when its wait resolves (`crates/agent/src/supervisor/waits.rs`); if rows outlive their waits, derive `GraphWait` from the in-memory state instead (`View.waits`, `View.approvals`, `supervisor/view.rs:21-41`) and say so.

---

## 4 The actor (B)

### 4.1 Constants (in `graph.rs`; no config this round)

```rust
pub const GRAPH_ACTOR: &str = "graph";
pub const LEASE_SECS: u64 = 300;
pub const DEFAULT_WINDOW_MS: u64 = 24 * 3_600_000;
pub const MAX_WINDOW_MS: u64 = 7 * 24 * 3_600_000;
pub const MAX_NODES: usize = 1_500;
pub const MAX_EDGES: usize = 3_000;
pub const HISTORY_ROWS: i64 = 5_000;
pub const MAX_INCLUDE: usize = 200;
pub const FLUSH_MS: u64 = 250;
pub const MAX_UPSERTS: usize = 500;
pub const MAX_PULSES: usize = 100;
pub const PERSIST_SECS: u64 = 30;
pub const QUIET_BUMP_MS: u64 = 30_000;
```

### 4.2 Shape

- `#[derive(kameo::Actor)] pub struct Graph { rt, store, last_event: HashMap<SessionId, u64>, unsaved: HashSet<SessionId>, model: Option<Model>, lease_until_ms: u64, … }`, spawned in `agent`'s `Plugin::start` after the `Agents` actor with `Self::spawn_with_mailbox(…, runtime::subscriber_mailbox())`, registered as `GRAPH_ACTOR`, subscribed (`rt.subscribe::<Self, T>(&actor)`, the pattern of `crates/roster/src/attention.rs:1765-1775`) to `AgentEvent`, `PeerMessageChanged`, `PeerDelivered`, `tools::ApprovalRequested`, `tools::ApprovalResolved`, `tools::AskRequested`, `tools::AskResolved`, and the jobs actor's published event type if there is one (record which). A `PERSIST_SECS` interval and a 60 s prune interval (the `RescoreTick` pattern, `attention.rs:1785-1804`).
- Messages (internal, `pub`):

```rust
/// `ask` → `anyhow::Result<AgentGraph>`: the `agent_graph` RPC.
#[derive(Clone, Debug)]
pub struct Snapshot { pub window_ms: Option<u64>, pub include: Vec<SessionId>, pub cursor: Option<GraphCursor> }
```
  The handler returns `DelegatedReply` when it must seed (the seed runs in a spawned task; events arriving meanwhile are buffered as dirty ids and applied after the seed; a second `Snapshot` during the seed waits for it).

### 4.3 Behaviour

1. **Every event, always**: `session = event.session()` (and for `PeerMessageChanged` both local ends; `SubagentSpawned`/`ObservedChildChanged`: the child and the parent) → `last_event.insert(s, now_ms)`, `unsaved.insert(s)`. Every `PERSIST_SECS`: `upsert_last_events(unsaved)` in a spawned task. `SessionDeleted` → `delete_last_event`.
2. **Dormant** (`model == None`): nothing else.
3. **`Snapshot`**: `window = window_ms.unwrap_or(DEFAULT_WINDOW_MS).min(MAX_WINDOW_MS)`; `lease_until_ms = now + LEASE_SECS·1000`; `include` truncated to `MAX_INCLUDE`.
   - No model → **seed** (a new `epoch_ms = now`, `seq = 0`): `ListSessions` (in-process), `ListActivity` (live set + roots), `last_events` (+ the in-memory map, max), `select_window` (§4.4), `ActivityOf(chosen)`, `graph_waits`, `last_stops`, `exchanges_since(now − window, HISTORY_ROWS)` → `aggregate`; reply the full `AgentGraph`.
   - Model, `cursor == Some({epoch_ms, seq})` current → add any `include` id (and ancestors) missing from the model as dirty structure (they go out in the next delta), reply `unchanged: true` with empty lists.
   - Model, cursor absent or stale → add includes, then reply the full model (current `epoch_ms`, `seq`).
   - The model's window becomes the max window requested during this lease.
4. **Active, per event**: structure first — `SubagentSpawned{parent, child, name, agent, model}` / `ObservedChildChanged` add or patch the child (kind `child`/`observed`); a `TurnStarted` for a session not in the model reads its summary (one row; add the ancestors too); `SessionDeleted` → remove + `removed` + its edges in `edges_removed`; `TitleChanged`, `ModelChanged`, `SessionOwnerChanged` patch; `TurnStarted` clears `outcome`; `TurnFinished{stop_reason}` sets `outcome = outcome_of(stop_reason)`. Then mark dirty: every event's session(s), except the delta-class events (`TextDelta`, `ReasoningDelta`, `ToolCallProgress`, `VisionProgress`), which mark dirty only when `now − published.last_event_ms ≥ QUIET_BUMP_MS`.
   - `PeerMessageChanged{id, state, from, to, result}`: dedupe by `(id, state)` (it is published once per local participant); `endpoint(from)`/`endpoint(to)` → node ids (a `peer:` end becomes a boundary node; a relative or unparsable address is skipped); on `accepted`: `ab`/`ba` += 1, `results` += 1 when `result`, `open` += 1, pulse `sent` (or `result` when `result`); `held`: none; `consumed`: pulse `delivered`, `open` −1 once per id; `replied`: pulse `replied`, `open` −1 once per id; `suppressed`/`failed`: pulse `failed`, `open` −1 once per id. Each updates `last_ms`, `last_from`, `last_id` and marks the edge dirty.
   - `PeerDelivered` only moves `last_event_ms` (the pulse came from `PeerMessageChanged`).
5. **Flush** (scheduled on the first dirty mark at `max(now, last_flush + FLUSH_MS)`; not reset by later marks): `ActivityOf(dirty sessions)` + `graph_waits(dirty)` in a spawned task, build the nodes, drop those equal to the last published copy (keep a `published: HashMap<String, GraphNode>` and the same for edges), then if anything is left: `seq += 1`, publish `AgentGraphChanged`. More than `MAX_UPSERTS` nodes+edges → publish `{resync: true}` (all lists empty) and mark the published maps stale (the next `Snapshot` replies in full).
6. **Prune** (60 s): if `now > lease_until_ms` → `model = None` (no event). Else drop silently (no event) nodes that are not live, not included, without a descendant in the model, and `last_event_ms < now − window − 60 s`; drop edges with an end gone.

### 4.4 Pure functions (unit-tested without actors)

```rust
pub(crate) fn node_kind(s: &SessionSummary) -> Option<GraphNodeKind>; // Advisor/Ask → None; Observed → Observed; desk set → Desk; parent set → Child; else Root
pub(crate) fn outcome_of(stop_reason: &str) -> Option<GraphOutcome>; // "error"→Failed, "cancelled"→Cancelled, "timed_out"→TimedOut, "interrupted"|"suspended"|"harness_continued"→None, else Finished
pub(crate) fn wait_of(kind: &str) -> Option<GraphWait>;              // question→User, job|pty_wait→Job, child_result→Children, peer_reply→None
pub(crate) fn endpoint(addr: &str) -> Option<String>;                // "session:<uuid>"→"<uuid>"; "peer:<name>"→"peer:<name>"; "peer:<c>/session:<u>"→"peer:<c>"; else None
pub(crate) fn pair(a: &str, b: &str) -> (String, String, String);    // (id "<lo>|<hi>", lo, hi), byte order
pub(crate) fn title_of(s: &SessionSummary) -> String;                // as 208 §2.1 (same rule as roster lineage_map's labels)
pub(crate) fn select_window(
    sessions: &[SessionSummary], last: &HashMap<SessionId, u64>, live: &HashSet<SessionId>,
    include: &[SessionId], now_ms: u64, window_ms: u64, max_nodes: usize,
) -> (Vec<SessionId>, Option<GraphOlder>);                          // parents before children; private and Advisor/Ask excluded
pub(crate) fn aggregate(rows: &[ExchangeEdgeRow], max_edges: usize) -> (Vec<GraphEdge>, Vec<String> /* peer node ids */);
```
An observed node's `activity.state` is its `observed_state` mapped exactly as `AgentInfo.state` maps it for watch-only rows (141 §5.2; find and call the existing mapping, do not write a second); its `outcome` is `completed` → finished, `failed` → failed, `cancelled` → cancelled, else none.

`select_window` rules (208 §5): a tree enters when one of its nodes is live, has `last ≥ now − window`, or is in `include`; inside an entered tree a node enters when it is live, recent, included, or an ancestor of an entered node (the root always). Over `max_nodes`: drop whole trees that have no live or included node, oldest newest-`last` first, into `GraphOlder`. `last` for a session without a clock entry = `max(updated_at, finished_at)` in ms. Private sessions (51 §4.5, `View.private` / `private_member`) are excluded.

---

## 5 RPC, event registration, wire (B)

### 5.1 `crates/agent/src/rpc_graph.rs`

```rust
#[rpc_core_macros::rpc_descriptors(owner = "agent", descriptors = rpc_descriptors, audience = "owner")]
#[rpc(server, client)]
pub trait AgentGraphApi {
    /// 208 §4: every active and recently active session tree and the peer messages between them, in one reply.
    /// `window_ms` null = 24 h (clamped to 7 d); `include`: sessions to show regardless of the window (with their
    /// ancestors; at most 200); a current `cursor` answers `unchanged: true` with empty lists. Each call keeps the
    /// `agent.graph` delta stream alive for 300 s.
    #[method(name = "agent_graph", with_extensions)]
    async fn agent_graph(
        &self,
        ext: &Extensions,
        window_ms: Option<u64>,
        include: Vec<SessionId>,
        cursor: Option<crate::GraphCursor>,
    ) -> RpcResult<crate::AgentGraph>;
}
```

Impl: `ask::<Graph, _>(GRAPH_ACTOR, Snapshot { window_ms, include, cursor })`, errors through `rpc_core::failed`. Module fn `pub(super) fn rpc_module(server) -> DescribedModule` like `rpc_sessions.rs:307`; add to the merge list in `lib.rs:64-78`. Copy the authorization of `list_sessions` (owner audience; `rpc_sessions.rs:24`); no mesh audience.

### 5.2 Registration

1. `declare_wire` (`lib.rs:4426-4437`): `rt.register_wire_event::<AgentGraphChanged>().await?;` after `PeerDelivered`.
2. `crates/rpc-compose/src/client_mirror.rs` `republish!` list (`:62-102`): add `agent::AgentGraphChanged` after `agent::PeerDelivered`.
3. Golden fixtures (`crates/rpc-compose/tests/fixtures/wire/`): new `agent.graph.json` in the shape of `agent.peer-message.json` with two entries — an empty delta (`seq: 1`, all lists empty, `older: null`, `resync: false`) and one carrying one node (kind `child`, `activity` idle, `waits: ["job"]`, `outcome: null`), one edge, one pulse (`result`); `agent.peer-message.json` gains `from`, `to`, `result` in serialization order. Add the `family::<agent::AgentGraphChanged>(…, "agent.graph")` line beside the others (`tests/wire_golden.rs:82-87`), the topic in `tests/wire_topics.rs:36-41`, and every `wire_parity_inc*.rs` that enumerates agent families.
4. `just wire-schema` (= `cargo test -p theuth --bin t --quiet export_bindings`), then the classifier `cargo test -p theuth --test wire_schema_snapshot`. Only `Additive`/`Nonbreaking` lines may appear, and only for: method `agent_graph`; topic `agent.graph@1`; `agent.peer-message@1` `from`/`to`/`result`; the new definitions. Then accept (`cp bindings/openrpc.json crates/theuth/tests/fixtures/wire/openrpc-v1.json`, the body of `just wire-schema-accept`). `ReviewRequired`/`Breaking` → stop, fix, never accept.
5. Known reds on main, not yours (207-as-built "Reds"): `wire_golden::legacy_event_families` and `wire_parity_inc4::agent_golden_parity` (`SubagentSpawned` `agent`/`model` missing in `agent.events.json`). Report them; do not fix them unless your change touches that fixture.

---

## 6 Engine acceptance (B)

Behaviour:
1. `agent_graph(null, [], null)` on a fresh engine seeds and returns every session tree active in 24 h with live state, waits, outcomes, `last_event_ms`, and the window's edges; parents before children; `seq 0`; `epoch_ms` = activation time.
2. The same call with the returned cursor answers `unchanged: true` with empty lists.
3. While leased, a burst of events in 250 ms publishes one `AgentGraphChanged`; an event that changes nothing visible publishes nothing; token deltas re-publish a node at most every 30 s.
4. A peer message between two sessions in the model yields an edge upsert and a `sent` pulse, then `delivered`/`replied`; a child's final result yields a `result` pulse and `results + 1` on the parent–child pair.
5. 300 s after the last call the model is dropped and nothing is published; the next call is a new epoch.
6. `last_event_ms` survives an engine restart (store reopen) within 30 s.
7. `SESSIONS_WIRE_SCHEMA` is 70; `ATTACHED` unchanged; no `sessions` column added.

Tests to write (in `crates/agent`, deterministic, no wall clock — pass `now_ms`): `graph::tests` — `node_kind` (root, desk, child, ACP child, observed, advisor/ask → none), `outcome_of` (every row of §4.4), `wait_of`, `endpoint` (session, peer, contact, relative, malformed), `pair` (ordering), `select_window` (recent, live, included + ancestors, old child of an active root excluded, root always with an entered node, cap → `older` oldest first, live trees never folded, private excluded), `aggregate` (directions, results, open, last_from/last_id, peer boundary ids, `MAX_EDGES`), the coalescer (dirty set + flush spacing + equality suppression + `resync` over `MAX_UPSERTS`, driven with a fake clock), lease expiry; `store` — `upsert_last_events` max semantics and round trip through a reopen, `exchanges_since` bound, `graph_waits`; an actor-level test through `spawn_test_agents` (the rig 207 used, `crates/roster/src/lib.rs` `dismiss_many_mixed_batch_publishes_once_and_wakes_nobody`): `Snapshot` seeds, a `SubagentSpawned` + `TurnStarted` publishes exactly one delta with both nodes, a second `Snapshot` with the cursor is `unchanged`; `communication` — `PeerMessageChanged` carries `from`/`to`/`result` for a normal message and a child result.

Gate (`/tmp/theuth-208-b-gate.sh`, one hold, §0):
```
cargo fmt -p agent -p rpc-compose
cargo clippy --keep-going -p agent -p rpc-compose -p sessionsync --all-targets -- -D warnings -A clippy::chunks_exact_to_as_chunks -A clippy::single_match
cargo test --no-fail-fast -p agent --lib graph -- --test-threads=4
cargo test --no-fail-fast -p agent --lib store -- --test-threads=4
cargo test --no-fail-fast -p agent --lib communication -- --test-threads=4
cargo test --no-fail-fast -p rpc-compose -- --test-threads=4
cargo test -p theuth --bin t --quiet export_bindings
cargo test -p theuth --test wire_schema_snapshot
# accept only after reading the classifier output (§5.2 step 4):
cp bindings/openrpc.json crates/theuth/tests/fixtures/wire/openrpc-v1.json
cargo test -p theuth --test wire_schema_snapshot
```
Then, in the same hold, regenerate the UI bindings without `bun install` (`cd crates/desktop/ui && bun run wire:generate`, then the `rm`/`mkdir`/`cp` lines of `just ts-desktop`, `justfile:114-130`) and `diff` the new/changed files against §7; record any difference in `208-as-built-B.md` (§7 is the source: a name or shape difference is B's to fix).

---

## 7 Generated TypeScript (exact; D hand-patches it, B's generator must emit the same)

Field order inside interfaces is alphabetical, as the generator writes it. Output (`*.output.ts`) fields are all required; receiver views (`*.receiver.ts`) make `#[serde(default)]` fields optional. Doc comments may differ; names and shapes may not.

### 7.1 `method.agent.output.ts` (and the same shapes in `method.agent.receiver.ts`)

`SessionActivity`, `AgentState`, `ObservedMeta`, `SessionId` already exist in this file. Add:

```ts
/**
 * 208 D9: what a graph node is.
 */
export type GraphNodeKind = ("root" | "desk" | "child" | "observed" | "peer")
/**
 * 208 D7: what a session is parked on besides its `AgentState`.
 */
export type GraphWait = ("user" | "job" | "children")
/**
 * 208 D7: how the session's last turn ended (207 §4's mapping of `stop_reason`).
 */
export type GraphOutcome = ("finished" | "failed" | "cancelled" | "timed_out")
/**
 * 208 §2.2: what happened to one exchange (a live edge event).
 */
export type GraphPulseKind = ("sent" | "delivered" | "replied" | "result" | "failed")
/**
 * This interface was referenced by `Contract`'s JSON-Schema
 * via the `definition` "agent_graphResult".
 */
export type AgentGraphResult = AgentGraph
export interface AgentGraph {
at_ms: number
edges: GraphEdge[]
epoch_ms: number
nodes: GraphNode[]
older: (GraphOlder | null)
seq: number
unchanged: boolean
window_ms: number
}
export interface GraphNode {
activity: SessionActivity
agent: (string | null)
desk: (string | null)
id: string
kind: GraphNodeKind
last_event_ms: number
model: string
name: (string | null)
observed: (ObservedMeta | null)
outcome: (GraphOutcome | null)
owner: string
parent: (string | null)
project: (string | null)
remote: boolean
root: string
started_ms: number
title: string
waits: GraphWait[]
}
export interface GraphEdge {
a: string
ab: number
b: string
ba: number
id: string
last_from: string
last_id: string
last_ms: number
open: number
results: number
}
export interface GraphPulse {
at_ms: number
edge: string
from: string
id: string
kind: GraphPulseKind
to: string
}
export interface GraphOlder {
newest_ms: number
nodes: number
roots: number
}
export interface GraphCursor {
epoch_ms: number
seq: number
}
```
`Contract` gains `AgentGraph`, `GraphCursor`, `GraphEdge`, `GraphNode`, `GraphNodeKind`, `GraphOlder`, `GraphOutcome`, `GraphPulse`, `GraphPulseKind`, `GraphWait`, `agent_graphResult: AgentGraphResult`. (`GraphPulse*` are only reached through the event; if the generator leaves them out of the method file, import them from the event file — §7.3.)

### 7.2 `method.agent.input.ts`

```ts
/**
 * @minItems 3
 * @maxItems 3
 *
 * This interface was referenced by `Contract`'s JSON-Schema
 * via the `definition` "agent_graphParams".
 */
export type AgentGraphParams = [(number | null), SessionId[], (GraphCursor | null)]
export interface GraphCursor {
epoch_ms: number
seq: number
}
```
`Contract` gains `GraphCursor`, `agent_graphParams: AgentGraphParams`.

### 7.3 `event.agent_2egraph.output.ts` (new; `.receiver.ts` the same with `?:` on the `#[serde(default)]` fields: `nodes`, `removed`, `edges`, `edges_removed`, `pulses`, `older`, `resync`)

```ts
/* Generated from OpenRPC; do not edit. */
// The generator inlines every referenced definition here; a hand patch may import them instead:
import type { GraphEdge, GraphNode, GraphOlder, GraphPulse } from './method.agent.output'

export interface Contract {
AgentGraphChanged: AgentGraphChanged
}
export interface AgentGraphChanged {
at_ms: number
edges: GraphEdge[]
edges_removed: string[]
epoch_ms: number
nodes: GraphNode[]
older: (GraphOlder | null)
pulses: GraphPulse[]
removed: string[]
resync: boolean
seq: number
type: "AgentGraphChanged"
}
```

`event.agent_2epeer_2dmessage.output.ts`: `PeerMessageChanged` gains `from: string`, `result: boolean`, `to: string` (receiver: `?:`).

### 7.4 `client.ts`

Check the alias numbers at the top of the file (`D4` = `method.agent.input`, `D5` = `method.agent.output`, `D181` = `method.agent.receiver` in the current tree; new event imports get the next free numbers).

```ts
// interface MethodMap
  "agent_graph": { params: [D4.Contract["agent_graphParams"][0], D4.Contract["agent_graphParams"][1], D4.Contract["agent_graphParams"][2]]; result: D5.Contract["agent_graphResult"] }
// METHODS: insert "agent_graph" in sorted position
// interface ReceiverResults
  "agent_graph": D181.Contract["agent_graphResult"]
// imports
import type * as Dg from './event.agent_2egraph.output'
import type * as Dgr from './event.agent_2egraph.receiver'
// interface TopicPayloads and TaggedTopicPayloads
  "agent.graph@1": Dg.Contract["AgentGraphChanged"]
// interface TopicReceiverPayloads
  "agent.graph@1": Dgr.Contract["AgentGraphChanged"]
// const TOPICS
  "agent.graph@1": ["AgentGraphChanged"]
```
`methods.json` (copied beside the bindings) gains `agent_graph`; `discovery` reads the live engine's OpenRPC, so the hand patch matters only for types.

---

## 8 UI (D)

### 8.1 Files — `crates/desktop/ui/src/components/attention/graph/`

| File | Content |
|---|---|
| `layout.ts` | §8.3, pure |
| `ttl.ts` | §8.4, pure |
| `reducer.ts` | §8.4, pure |
| `selectors.ts` | §8.4, pure |
| `prefs.ts` | §8.5: `attention.graph` localStorage + zustand store (the `lib/sidebar-linger.ts` pattern: `globalThis.localStorage?.getItem`) |
| `queries.ts` | `useAgentGraph` + `GraphSource` (§8.5) |
| `fixture.ts` | §9 verbatim |
| `NodeBody.tsx` | the node's content at three LODs; no `@xyflow/react` import (testable in node) |
| `Strip.tsx` | the collapsed strip; no `@xyflow/react` import |
| `Region.tsx` | `GraphRegion`: strip, resize, full screen, keys, lazy canvas, node bar |
| `Canvas.tsx` | `export default function Canvas(props)`; the only importer of `@xyflow/react` and `@xyflow/react/dist/style.css`; node and edge types; MiniMap; Controls |
| `edges.tsx` | `TreeEdge` (smooth-step, solid, result pulse), `MessageEdge` (bezier, dashed, animated, count label) — imported only by `Canvas.tsx` |
| `NodeBar.tsx`, `EdgePopover.tsx`, `TtlPopover.tsx` | node actions, message popover (`list_peer_messages`), TTL settings |
| `layout.test.ts`, `ttl.test.ts`, `reducer.test.ts`, `selectors.test.ts`, `prefs.test.ts`, `NodeBody.test.ts`, `Strip.test.ts` | §10.2; `.test.ts` only (`vite.config.ts:119` includes `src/**/*.test.ts`), components via `createElement` + `renderToStaticMarkup` |

Imports of wire types: `import type { AgentGraph, GraphCursor, GraphEdge, GraphNode, GraphOlder, GraphPulse } from '@/rpc/bindings/method.agent.output'`, `import type { AgentGraphChanged } from '@/rpc/bindings/event.agent_2egraph.output'`.

`Region.tsx` loads the canvas with `const Canvas = lazy(() => import('./Canvas'))` inside `<Suspense>` (the `components/costs/Resident.tsx:12` pattern), rendered only while expanded, so `@xyflow/react` is its own chunk. Nothing outside `Canvas.tsx`/`edges.tsx` imports `@xyflow/react`.

### 8.2 `model.ts` — the subtree filter (edit)

```ts
/** 208 §7.4: the list restricted to one graph subtree; `ids` = the subtree's session ids as the graph knows them now. */
export type SubtreeFilter = { session: string; label: string; ids: ReadonlySet<string> }
export type ListFilter = { kinds: ReadonlySet<string>; project: ProjectFilter; query: string; subtree: SubtreeFilter | null }
export const EMPTY_FILTER: ListFilter = { kinds: new Set(), project: { type: 'all' }, query: '', subtree: null }
// matchesFilter gains, before the query test:
//   if (f.subtree && !(typeof item.session === 'string' && f.subtree.ids.has(item.session))) return false
```
`pruneFilter` keeps `subtree` untouched (it spreads `f`). `Page.tsx` `readView` returns `subtree: null`; `saveView` does not store it. Every existing `ListFilter` literal in tests gains `subtree: null`.

### 8.3 `layout.ts` — tidy tree (exported API)

```ts
import type { GraphNode } from '@/rpc/bindings/method.agent.output'

export type Box = { w: number; h: number; gx: number; gy: number; tree: number }
/** Node box 200 × 64; sibling gap 24; level gap 56 (row pitch 120); gap between trees 80. */
export const BOX: Box = { w: 200, h: 64, gx: 24, gy: 56, tree: 80 }
export type Pos = { x: number; y: number }

/** parent id → child ids among `nodes` (only parents present in `nodes`), ordered by started_ms asc, then id asc. */
export function childOrder(nodes: readonly GraphNode[]): Map<string, string[]>
/** Structural key: `id(child,child(grandchild))`, recursive, in child order. */
export function treeSignature(root: string, children: ReadonlyMap<string, readonly string[]>): string
/**
 * One tree, positions of each box's top-left relative to the tree: y = depth × (h + gy); x by contours —
 * a leaf has left [0], right [w]; children are placed left to right, each shifted by
 * max over shared depths d of (accRight[d] + gx − childLeft[d]) (the first child: 0); the merged contour takes the
 * min of lefts and max of rights per depth; the parent's x = (x of first child + x of last child) / 2; its contour is
 * [x, …childLefts] / [x + w, …childRights]; finally shift so the minimum left is 0. width = max right − 0.
 */
export function layoutTree(root: string, children: ReadonlyMap<string, readonly string[]>, box?: Box): { pos: Map<string, Pos>; width: number }
/**
 * Sticky root slots (208 D13). Order: trees with a previous slot by that x asc (ties: input order), then trees without
 * one in input order (callers pass creation order: root started_ms asc, id asc). cursor = 0; each tree:
 * x = prev === undefined ? cursor : max(prev, cursor); cursor = x + width + gap. prev null = compact packing.
 */
export function packRoots(trees: readonly { root: string; width: number }[], prev: ReadonlyMap<string, number> | null, gap?: number): Map<string, number>
/** layoutTree per root (memoised by treeSignature through `cache`) + packRoots; absolute positions and the new slots. */
export function layoutForest(nodes: readonly GraphNode[], prev: ReadonlyMap<string, number> | null, cache?: LayoutCache, box?: Box): { pos: Map<string, Pos>; slots: Map<string, number> }
export type LayoutCache = Map<string, { pos: Map<string, Pos>; width: number }>
/** A bounded cache (LRU by insertion, `limit` entries). */
export function layoutCache(limit?: number): LayoutCache
```
The canvas keeps `slots` in a ref; Fit (and reopening the region) calls `layoutForest(…, null)`.

### 8.4 `ttl.ts`, `reducer.ts`, `selectors.ts` (exported API)

```ts
// ttl.ts
export type TtlPrefs = { child_min: number; root_min: number; desk_min: number; edge_min: number }
export const TTL_DEFAULTS: TtlPrefs = { child_min: 10, root_min: 240, desk_min: 1440, edge_min: 5 }
/** child, observed → child_min; root, peer → root_min; desk → desk_min; in ms. */
export function nodeTtlMs(node: GraphNode, ttl: TtlPrefs): number
/** live(activity.state.state) (lib/session-tree.ts:14) || waits.length > 0. */
export function isLiveNode(node: GraphNode): boolean
/** 1 while age < 0.75·ttl; then 1 − 3·(age − 0.75·ttl)/ttl; 0 when age ≥ ttl (expired). */
export function fadeOpacity(ageMs: number, ttlMs: number): number
/** The session `from` of an `awaiting` node points at: 'parent' → parent; 'session:<id>' → id; else the parent when its
 * name equals `from`, else a child, else a sibling with that name; null otherwise. */
export function awaitTarget(node: GraphNode, nodes: ReadonlyMap<string, GraphNode>): string | null
export type NodeView = { node: GraphNode; opacity: number; pinned: boolean; ghost: boolean; attention: number; expiresAt: number | null }
export type EdgeView = { edge: GraphEdge; active: boolean; animated: boolean }
export type VisibleGraph = { nodes: Map<string, NodeView>; edges: EdgeView[]; hidden: number }
/**
 * 208 §3.1. Non-peer nodes: pinned = attention > 0 || isLiveNode || a descendant (in state.nodes) that is pinned by
 * attention or liveness; pinned → opacity 1, expiresAt null. Else opacity = fadeOpacity(now − last_event_ms, ttl),
 * expiresAt = last_event_ms + ttl; opacity 0 → ghost (opacity 0.25) when a descendant is visible, else hidden.
 * Peer nodes: visible when an edge joins them to a visible non-peer node and fadeOpacity(age, root ttl) > 0.
 * Edges: both ends visible; animated = open > 0 || awaitTarget(a) === b || awaitTarget(b) === a;
 * active = animated || now − last_ms < edge ttl. hidden = state.nodes.size − nodes.size.
 */
export function visibleGraph(state: GraphState, attention: ReadonlyMap<string, number>, ttl: TtlPrefs, now: number): VisibleGraph
/** ms until something visible changes: 5000 while a visible node fades (0.75·ttl ≤ age < ttl) or an edge is active and
 * not animated; else min(next fade start, next edge fold) − now; clamped to [1000, 60000]. */
export function nextTtlTick(state: GraphState, view: VisibleGraph, ttl: TtlPrefs, now: number): number
/** max of the four TTLs in ms, clamped to 7 d: the `window_ms` asked of the engine. */
export function windowMs(ttl: TtlPrefs): number

// reducer.ts
export type GraphState = {
  epoch: number; seq: number; at: number; window: number
  nodes: ReadonlyMap<string, GraphNode>; edges: ReadonlyMap<string, GraphEdge>
  pulses: readonly GraphPulse[]; older: GraphOlder | null
}
export const PULSE_KEEP = 200
export function fromSnapshot(s: AgentGraph): GraphState
/** A reply to a renewal: `unchanged` → `state` itself; else fromSnapshot(s). */
export function mergeSnapshot(state: GraphState | null, s: AgentGraph): GraphState
/**
 * Checked in this order: null state → { state: null, resnapshot: true }; ev.epoch_ms ≠ epoch → { state (unchanged),
 * resnapshot: true }; ev.seq ≤ seq → { state, resnapshot: false } (stale); ev.resync or ev.seq > seq + 1 →
 * { state (unchanged), resnapshot: true }.
 * Else: delete `removed` ids and every edge touching them; upsert nodes; delete edges_removed; upsert edges;
 * pulses = [...pulses, ...ev.pulses].slice(-PULSE_KEEP); older = ev.older; seq = ev.seq; at = ev.at_ms.
 * Untouched nodes and edges keep object identity; the maps are new objects.
 */
export function applyDelta(state: GraphState | null, ev: AgentGraphChanged): { state: GraphState | null; resnapshot: boolean }
export function cursorOf(state: GraphState | null): GraphCursor | null

// selectors.ts
/** [id, parent, …, root]; [] when id is unknown. */
export function pathToRoot(nodes: ReadonlyMap<string, GraphNode>, id: string): string[]
/** id and every descendant in `nodes`. */
export function subtreeIds(nodes: ReadonlyMap<string, GraphNode>, id: string): Set<string>
/** session → number of items that are not expired (expires_ms) and carry a session. */
export function attentionBySession(items: readonly AttentionItem[], now: number): Map<string, number>
/** Sessions of those items, unique, sorted, at most 200: `include` for agent_graph. */
export function includeSessions(items: readonly AttentionItem[], now: number): string[]
/** Over visible non-peer nodes: needYou = attention > 0 || state approval || waits has 'user';
 * working = isLiveNode and not needYou; finished = not live, outcome !== null, not needYou. */
export function stripCounts(view: VisibleGraph): { needYou: number; working: number; finished: number }
/** '4 need you · 11 working · 14 finished' (all three parts always). */
export function stripText(c: { needYou: number; working: number; finished: number }): string
/** root/desk/peer: title; child/observed: name ?? title. */
export function nodeLabel(node: GraphNode): string
/** Row 2's first part: state approval → stateText; awaiting → stateText; waits user → 'waiting on you';
 * job → 'waiting on a job'; children → 'waiting on children'; running → 'running'; error → 'error';
 * else the outcome word ('finished' | 'failed' | 'cancelled' | 'timed out') or 'idle'. */
export function statusText(node: GraphNode): string
```

### 8.5 Indicators, prefs, data hook

Indicators (S3, 208 §7.2), reused as they are: `GLYPH`, `ago` (`components/sidebar/AgentTree.tsx:23,33`); `HarnessGlyph`, `shortModel` (`components/sidebar/harnessGlyph.tsx:79,89`); `stateText`, `modelLabel` (`components/agents/sessions.ts:15,18`); `counts` (`components/chat/broadcast.ts:89`); `ActivityDot` (`components/chat/SubagentStrip.tsx:57`); `WatchDot`, `EyeMark` (`components/chat/ObservedBar.tsx:30,47`); `live`, `observedState`, `observedLabel` (`lib/session-tree.ts:14,30,42`). `ago` takes an ISO string: pass `new Date(node.last_event_ms).toISOString()`.

`AgentTree.tsx` edit: move the colour logic of `SubagentRow` (`:94-104`: running → `backgroundColors.subagents.text` + `animate-pulse`; approval/awaiting → `backgroundColors.approval.text`; error → `text-danger`; else `text-fg-tertiary`) into `export function stateTone(state: AgentState): string` in the same file; `SubagentRow` calls it; output identical.

`NodeBody` props: `{ node: GraphNode; lod: 'full' | 'compact' | 'dot'; attention: number; highlighted: boolean; selected: boolean; opacity: number; now: number }`; `memo`. Contents per 208 §7.2: glyph (`observed` → `WatchDot(node.observed.state)`, else `GLYPH[node.activity.state.state]` in `stateTone`), `HarnessGlyph(node.agent)` when set, `nodeLabel`, badge `attention` (danger ring + count) when > 0; row 2 `statusText · shortModel(model) · counts(activity) (when jobs or ptys) · ago`; `remote · <owner>` chip; `EyeMark` for observed; `data-node-id`, `data-lod`, `aria-label` = `nodeLabel · statusText`.

`prefs.ts`:
```ts
export const GRAPH_PREFS_KEY = 'attention.graph'
export type GraphPrefs = { expanded: boolean; height: number; follow: boolean; ttl: TtlPrefs }
export const GRAPH_PREFS_DEFAULT: GraphPrefs = { expanded: true, height: 320, follow: false, ttl: TTL_DEFAULTS }
/** Parse; garbage or missing parts → defaults; height clamped to [160, 2000]; ttl minutes: child [1, 1440], root and desk [1, 10080], edge [1, 240]; non-integers rounded. */
export function readGraphPrefs(raw: string | null): GraphPrefs
export const useGraphPrefs: UseBoundStore<StoreApi<GraphPrefs & { set: (p: Partial<GraphPrefs>) => void }>>
```
Under `md` (the `useIsMobile`/breakpoint helper the sidebar uses) the region starts collapsed regardless of `expanded`; full screen is session state, never stored.

`queries.ts`:
```ts
export type GraphSource = {
  snapshot: (windowMs: number, include: readonly string[], cursor: GraphCursor | null) => Promise<AgentGraph>
  subscribe: (cb: (ev: AgentGraphChanged) => void) => () => void
}
/** Dev fixture only: replace the RPC source (null restores it). */
export function setGraphSource(s: GraphSource | null): void
/**
 * supported = useQuerySupport(['agent_graph']) (or a source override). On mount: snapshot(windowMs(ttl), include, null).
 * Each event → applyDelta; resnapshot → one snapshot(…, null) (never two at once: a pending flag plus a dirty flag, the
 * 207 `patchEvent` rule). Every 120 s: snapshot(…, cursorOf(state)) → mergeSnapshot. `include` gains a session not in
 * state.nodes → snapshot(…, cursorOf(state)) debounced 1 s. A connection state change back to connected → snapshot(…, null).
 * Default source: api.agent_graph + onTopic('agent.graph@1', …) (store/sessions.ts:939).
 */
export function useAgentGraph(ttl: TtlPrefs, include: readonly string[]): { state: GraphState | null; supported: boolean; error: string | null }
```

### 8.6 Page, Toolbar, Card (edits) and `GraphRegion`

```ts
export type GraphRegionProps = {
  items: readonly AttentionItem[] | undefined   // the 207 cache, for attention counts and include
  highlight: string | null                      // session of the focused entry's head, else of the open item
  focus: { session: string; nonce: number } | null   // "Show in graph": expand, centre (no zoom change), highlight
  subtree: string | null                        // the current filter's session
  onSubtree: (f: SubtreeFilter | null) => void  // node click; Region re-sends it when the subtree's ids change
}
export function GraphRegion(props: GraphRegionProps): JSX.Element | null   // null when !supported
```
- `Page.tsx`: render `<GraphRegion …/>` between the header (`:284-292`) and the row `<div className="flex min-h-0 flex-1 overflow-hidden">` (`:293`), outside the list's empty/pending branches; `filtered` includes `filter.subtree !== null`; `onSubtree` → `setView({ filter: { ...filter, subtree }, sort })`; `highlight` from `focused` (else the `active` item); pass `onShowInGraph` (stable through the `latest` ref, `:189-193`) to `Card`/`StackCard`, which sets `focus = { session, nonce: n + 1 }`.
- `Toolbar.tsx`: when `filter.subtree`, a chip "In <label>" with a lucide `X` button (`aria-label="Clear tree filter"`) → `onFilter({ ...filter, subtree: null })`. No new prop.
- `Card.tsx`: `FrameProps` gains `onShowInGraph?: (session: string) => void`; when set and `item.session` is a string, a ghost `xs` icon button (lucide `Network`, `aria-label="Show in graph"`, title "Show in graph") in the actions row (`:127`); `StackCard` passes it with its head's session.
- `GraphRegion` collapsed: a 32 px bar: chevron, `stripText(stripCounts(view))`, "+N older trees" when `older`, the subtree chip mirror; click or `G` expands. Expanded: `ResizeHandle` (`components/ui/resize-handle.tsx`, `orientation="horizontal"`, `onMove` sets height within [160, 0.8 × page height], `onNudge` ±16, `onReset` 320), the lazy `Canvas`, a toolbar (Fit, Follow, Full screen, TTL). Full screen: `fixed inset-0 z-40` over the app with a header (back chevron, title "Agents", strip text); `Esc` leaves it.
- Keys: a `keydown` listener on `window` while `GraphRegion` is mounted, ignoring events with modifiers other than Shift, `defaultPrevented`, or whose target matches the list's `CONTROL` selector (`Page.tsx:78`, export it) or is editable: `g` → toggle expanded, `G`(Shift) → toggle full screen, `Escape` → leave full screen (only when full screen).
- Node click: select, then `onSubtree({ session: id, label: nodeLabel(node), ids: subtreeIds(state.nodes, id) })` and the node bar (Filter list to this tree · Open session · Clear); "Open session" → `navigate({ to: '/c/$session', params: { session: id } })`; peer nodes offer neither (click selects only). Clicking the selected node again clears the subtree. Edge click (message) → `EdgePopover`: names, `ab`/`ba`, results, last time; "Load messages" calls `api.list_peer_messages(<a's session if a is a session else b>, null, 20, false)` and lists those whose counterpart is the other end; tree edges do nothing.
- Canvas: `ReactFlow` with `nodes` from `layoutForest` (type `session`, `draggable: false`, `selectable: true`, `style.opacity`), `edges` = tree edges (`id: 't:<child>'`, source parent, target child, type `tree`) + message edges (type `message`), `onlyRenderVisibleElements` when > 50 nodes, `nodesDraggable={false}`, `nodesConnectable={false}`, `minZoom={0.1}`, `maxZoom={2}`, `fitView` only on the first mount, `onMove` → bucket zoom into the LOD (≥ 0.75 full, ≥ 0.4 compact, else dot) held in state (nodes read the bucket through a context, not the raw zoom), `forceColorMode` from the app theme, `<Background variant={BackgroundVariant.Dots} />`, `<Controls showInteractive={false} />`, `<MiniMap nodeColor=…>` (danger for attention, the state tone otherwise). Highlight: the node and `pathToRoot` ids accent, every other node and edge at 60 % opacity. Follow (208 D18): on a new pulse or a node whose state changed, `setCenter` on it unless the pointer is down or the last user move was < 3 s ago. Reduced motion (`matchMedia('(prefers-reduced-motion: reduce)')`): no `animated`, no pulses, `duration: 0`.
- Folding (208 D17): a node whose subtree (excluding itself) has ≥ 8 visible nodes, none pinned, renders its children as one chip node "+N finished" (`id: 'fold:<parent>'`), expanded per session on click.

---

## 9 Fixture — `components/attention/graph/fixture.ts` (D creates it verbatim)

40 nodes over 8 trees + 2 boundary nodes; every kind, state, wait, outcome; 18 message edges; a scripted delta sequence. Ids are readable strings (real ones are UUIDs; nothing parses them).

```ts
// 208: the Attention graph's fixture — 40 nodes in 8 trees plus 2 boundary peers, every node kind, AgentState, wait and
// outcome; 18 pair edges; a scripted delta sequence. Tests use GRAPH_FIXTURE (a fixed instant); the dev page shifts
// times with graphFixture(Date.now()). Expected TTL outcomes: contract §9.2.
import type { AgentState, AgentGraph, GraphEdge, GraphNode, GraphNodeKind, GraphOutcome, GraphWait, ObservedMeta, ObservedState, SessionActivity } from '@/rpc/bindings/method.agent.output'
import type { AgentGraphChanged } from '@/rpc/bindings/event.agent_2egraph.output'

export const GRAPH_FIXTURE_NOW = 1_800_000_000_000
const MIN = 60_000

const LIVE = new Set(['running', 'awaiting', 'approval'])
function activity(state: AgentState, extra: Partial<SessionActivity> = {}): SessionActivity {
  return { asks: null, children: 0, jobs: 0, last_active: null, live: LIVE.has(state.state), ptys: 0, state, turn_started_at: null, waiting_since_ms: null, ...extra }
}
function observed(state: ObservedState): ObservedMeta {
  return { can_cancel: false, cost_rule: 'inside_parent', description: null, detail: null, external: 'x', source: 'omp_task', state, stop: null, tool_call: null }
}
const RUN: AgentState = { state: 'running' }
const IDLE: AgentState = { state: 'idle' }
const ERR: AgentState = { state: 'error' }

type Spec = {
  id: string; kind: GraphNodeKind; parent?: string; title: string; ago: number; state?: AgentState
  outcome?: GraphOutcome; waits?: GraphWait[]; agent?: string; project?: string; desk?: string; remote?: boolean
  observed?: ObservedState; jobs?: number; started?: number; model?: string
}
const SPECS: Spec[] = [
  // r1 Ship 2.5 release — a busy root: approval, awaiting, job wait, ACP children, observed children
  { id: 'r1', kind: 'root', title: 'Ship 2.5 release', ago: 0, state: RUN, project: 'release-2.5', started: 60 },
  { id: 'r1.builder', kind: 'child', parent: 'r1', title: 'builder', ago: 1, state: { state: 'approval', tool: 'bash' }, started: 58 },
  { id: 'r1.builder.linter', kind: 'child', parent: 'r1.builder', title: 'linter', ago: 2, state: { state: 'awaiting', from: 'builder' }, started: 50 },
  { id: 'r1.builder.tests', kind: 'child', parent: 'r1.builder', title: 'tests', ago: 3, state: RUN, waits: ['job'], jobs: 2, started: 49 },
  { id: 'r1.docs', kind: 'child', parent: 'r1', title: 'docs', ago: 6, outcome: 'finished', started: 57 },
  { id: 'r1.reviewer', kind: 'child', parent: 'r1', title: 'reviewer', ago: 8, outcome: 'failed', started: 56 },
  { id: 'r1.scout', kind: 'child', parent: 'r1', title: 'scout', ago: 12, outcome: 'finished', started: 55 },
  { id: 'r1.claude', kind: 'child', parent: 'r1', title: 'impl', ago: 0, state: RUN, agent: 'claude', model: 'anthropic/claude-sonnet-5', started: 54 },
  { id: 'r1.claude.explore', kind: 'observed', parent: 'r1.claude', title: 'explore', ago: 1, state: RUN, observed: 'running', started: 40 },
  { id: 'r1.claude.plan', kind: 'observed', parent: 'r1.claude', title: 'plan', ago: 9, outcome: 'finished', observed: 'completed', started: 41 },
  { id: 'r1.codex', kind: 'child', parent: 'r1', title: 'port', ago: 4, state: ERR, outcome: 'failed', agent: 'codex', model: 'openai/gpt-5.5', started: 53 },
  // r2 Fix flaky CI — idle root, cancelled / failed / timed out children, an omp harness with observed tasks
  { id: 'r2', kind: 'root', title: 'Fix flaky CI', ago: 30, outcome: 'finished', project: 'theuth', started: 120 },
  { id: 'r2.bisect', kind: 'child', parent: 'r2', title: 'bisect', ago: 20, state: { state: 'cancelled' }, outcome: 'cancelled', started: 110 },
  { id: 'r2.repro', kind: 'child', parent: 'r2', title: 'repro', ago: 9.5, state: ERR, outcome: 'failed', started: 109 },
  { id: 'r2.repro.min', kind: 'child', parent: 'r2.repro', title: 'minimise', ago: 11, outcome: 'finished', started: 100 },
  { id: 'r2.verify', kind: 'child', parent: 'r2', title: 'verify', ago: 7, outcome: 'timed_out', started: 108 },
  { id: 'r2.omp', kind: 'child', parent: 'r2', title: 'sweep', ago: 7, outcome: 'finished', agent: 'omp', started: 107 },
  { id: 'r2.omp.task1', kind: 'observed', parent: 'r2.omp', title: 'task 1', ago: 5, outcome: 'finished', observed: 'completed', started: 90 },
  { id: 'r2.omp.task2', kind: 'observed', parent: 'r2.omp', title: 'task 2', ago: 3, state: ERR, outcome: 'failed', observed: 'failed', started: 89 },
  // r3 ops desk — a desk with an ask (waiting on you), an expired child, and a live grandchild pinning its parent
  { id: 'r3', kind: 'desk', title: 'ops desk', ago: 300, desk: 'ops', outcome: 'finished', started: 5000 },
  { id: 'r3.sweep', kind: 'child', parent: 'r3', title: 'nightly-sweep', ago: 40, state: RUN, waits: ['user'], started: 400 },
  { id: 'r3.rotate', kind: 'child', parent: 'r3', title: 'rotate-keys', ago: 200, outcome: 'finished', started: 399 },
  { id: 'r3.audit', kind: 'child', parent: 'r3', title: 'audit', ago: 2, outcome: 'finished', started: 398 },
  { id: 'r3.audit.grep', kind: 'child', parent: 'r3.audit', title: 'grep', ago: 0, state: RUN, started: 30 },
  { id: 'r3.audit.diff', kind: 'child', parent: 'r3.audit', title: 'diff', ago: 1, state: { state: 'awaiting', from: 'audit' }, started: 29 },
  // r4 Nightly routine — a root in its fade, children expired
  { id: 'r4', kind: 'root', title: 'Nightly routine 11-21', ago: 230, outcome: 'finished', project: 'theuth', started: 600 },
  { id: 'r4.summarize', kind: 'child', parent: 'r4', title: 'summarize', ago: 232, outcome: 'finished', started: 599 },
  { id: 'r4.index', kind: 'child', parent: 'r4', title: 'index', ago: 235, outcome: 'finished', started: 598 },
  // r5 Refactor store — an expired root kept by a live descendant waiting on its children
  { id: 'r5', kind: 'root', title: 'Refactor store', ago: 250, outcome: 'finished', started: 700 },
  { id: 'r5.migrator', kind: 'child', parent: 'r5', title: 'migrator', ago: 0, state: RUN, waits: ['children'], started: 690 },
  { id: 'r5.migrator.check', kind: 'child', parent: 'r5.migrator', title: 'check', ago: 1, state: RUN, started: 20 },
  { id: 'r5.old', kind: 'child', parent: 'r5', title: 'old-pass', ago: 245, outcome: 'finished', started: 689 },
  // r6 Billing export — owned by another engine (last-known state)
  { id: 'r6', kind: 'root', title: 'Billing export', ago: 15, state: RUN, project: 'billing', remote: true, started: 100 },
  { id: 'r6.fetch', kind: 'child', parent: 'r6', title: 'fetch', ago: 16, outcome: 'finished', remote: true, started: 99 },
  { id: 'r6.render', kind: 'child', parent: 'r6', title: 'render', ago: 15, state: RUN, remote: true, started: 98 },
  // r7 concierge desk — two attention items on the desk, a child in its fade
  { id: 'r7', kind: 'desk', title: 'concierge desk', ago: 50, desk: 'concierge', outcome: 'finished', started: 9000 },
  { id: 'r7.reply', kind: 'child', parent: 'r7', title: 'reply', ago: 9, outcome: 'finished', started: 60 },
  // r8 Scratch — an expired root with nothing to keep it
  { id: 'r8', kind: 'root', title: 'Scratch', ago: 241, outcome: 'finished', started: 300 },
  // boundary peers
  { id: 'peer:alice', kind: 'peer', title: 'alice', ago: 50, started: 3000 },
  { id: 'peer:mini-ops', kind: 'peer', title: 'mini-ops', ago: 120, started: 3000 },
]

const rootOf = (id: string): string => (id.startsWith('peer:') ? id : (id.split('.')[0] as string))

function node(s: Spec, now: number): GraphNode {
  const state = s.state ?? IDLE
  return {
    activity: activity(state, { jobs: s.jobs ?? 0 }),
    agent: s.agent ?? null,
    desk: s.desk ?? null,
    id: s.id,
    kind: s.kind,
    last_event_ms: now - s.ago * MIN,
    model: s.kind === 'peer' ? '' : (s.model ?? 'anthropic/claude-opus-5-5'),
    name: s.kind === 'child' || s.kind === 'observed' ? s.title : null,
    observed: s.observed ? observed(s.observed) : null,
    outcome: s.outcome ?? null,
    owner: s.kind === 'peer' ? '' : s.remote ? 'mini' : 'studio',
    parent: s.parent ?? null,
    project: s.project ?? (s.parent ? (SPECS.find((x) => x.id === rootOf(s.id))?.project ?? null) : null),
    remote: s.remote ?? s.kind === 'peer',
    root: rootOf(s.id),
    started_ms: now - (s.started ?? 0) * MIN,
    title: s.title,
    waits: s.waits ?? [],
  }
}

type EdgeSpec = [a: string, b: string, ab: number, ba: number, results: number, open: number, ago: number, lastFrom: string]
const EDGES: EdgeSpec[] = [
  ['r1', 'r1.builder', 3, 1, 0, 0, 2, 'r1.builder'],
  ['r1.builder', 'r1.builder.linter', 2, 1, 1, 1, 1, 'r1.builder'],
  ['r1.builder', 'r1.builder.tests', 1, 0, 0, 0, 3, 'r1.builder'],
  ['r1', 'r1.docs', 1, 1, 1, 0, 6, 'r1.docs'],
  ['r1', 'r1.reviewer', 1, 1, 1, 0, 8, 'r1.reviewer'],
  ['r1.builder', 'r1.reviewer', 4, 3, 0, 0, 9, 'r1.reviewer'],
  ['r1.claude', 'r1.codex', 2, 2, 0, 0, 4, 'r1.codex'],
  ['r1', 'r2', 1, 0, 0, 0, 25, 'r1'],
  ['r2', 'r2.repro', 1, 1, 1, 0, 9.5, 'r2.repro'],
  ['r2', 'r2.verify', 1, 1, 1, 0, 7, 'r2.verify'],
  ['r3', 'r3.sweep', 1, 2, 0, 0, 40, 'r3.sweep'],
  ['r3.audit', 'r3.audit.diff', 0, 1, 0, 1, 1, 'r3.audit.diff'],
  ['r3', 'r6', 0, 2, 0, 0, 15, 'r6'],
  ['peer:alice', 'r7', 3, 2, 0, 0, 50, 'r7'],
  ['peer:mini-ops', 'r3', 1, 1, 0, 0, 120, 'r3'],
  ['r5.migrator', 'r5.migrator.check', 1, 0, 0, 1, 1, 'r5.migrator'],
  ['r1', 'r8', 1, 0, 0, 0, 241, 'r1'],
  ['r4', 'r4.summarize', 0, 1, 1, 0, 232, 'r4.summarize'],
]

function edge([a, b, ab, ba, results, open, ago, lastFrom]: EdgeSpec, now: number, i: number): GraphEdge {
  return { a, ab, b, ba, id: `${a}|${b}`, last_from: lastFrom, last_id: `x-${String(i).padStart(2, '0')}`, last_ms: now - ago * MIN, open, results }
}

export const GRAPH_FIXTURE_EPOCH = GRAPH_FIXTURE_NOW - 60 * MIN

export function graphFixture(now: number): AgentGraph {
  return {
    at_ms: now,
    edges: EDGES.map((e, i) => edge(e, now, i)),
    epoch_ms: now - 60 * MIN,
    nodes: SPECS.map((s) => node(s, now)),
    older: { newest_ms: now - 300 * MIN, nodes: 12, roots: 3 },
    seq: 7,
    unchanged: false,
    window_ms: 1440 * MIN,
  }
}
export const GRAPH_FIXTURE: AgentGraph = graphFixture(GRAPH_FIXTURE_NOW)

/** Open attention items per session at the fixture instant (the 207 list joined on item.session). */
export const GRAPH_FIXTURE_ATTENTION: ReadonlyMap<string, number> = new Map([
  ['r1.builder', 1],
  ['r3', 1],
  ['r3.sweep', 1],
  ['r7', 2],
])

const base = (seq: number, atMs: number): AgentGraphChanged => ({ at_ms: atMs, edges: [], edges_removed: [], epoch_ms: GRAPH_FIXTURE_EPOCH, nodes: [], older: null, pulses: [], removed: [], resync: false, seq, type: 'AgentGraphChanged' })
const at = (sec: number) => GRAPH_FIXTURE_NOW + sec * 1000
const fmt = (state: AgentState, outcome: GraphOutcome | null, sec: number): GraphNode => ({
  ...node({ id: 'r1.builder.fmt', kind: 'child', parent: 'r1.builder', title: 'fmt', ago: 0, state, started: 0 }, GRAPH_FIXTURE_NOW),
  last_event_ms: at(sec),
  outcome,
})

/** Applied in order to GRAPH_FIXTURE (seq 7). Expected reducer results: contract §9.4. */
export const GRAPH_FIXTURE_DELTAS: AgentGraphChanged[] = [
  // 0: builder spawns fmt
  { ...base(8, at(1)), nodes: [{ ...node(SPECS[1] as Spec, GRAPH_FIXTURE_NOW), activity: activity({ state: 'approval', tool: 'bash' }, { children: 3 }), last_event_ms: at(1) }, fmt(RUN, null, 1)] },
  // 1: fmt messages builder
  { ...base(9, at(2)), edges: [{ a: 'r1.builder', ab: 0, b: 'r1.builder.fmt', ba: 1, id: 'r1.builder|r1.builder.fmt', last_from: 'r1.builder.fmt', last_id: 'x-90', last_ms: at(2), open: 1, results: 0 }], pulses: [{ at_ms: at(2), edge: 'r1.builder|r1.builder.fmt', from: 'r1.builder.fmt', id: 'x-90', kind: 'sent', to: 'r1.builder' }] },
  // 2: builder's approval allowed: running again
  { ...base(10, at(3)), nodes: [{ ...node(SPECS[1] as Spec, GRAPH_FIXTURE_NOW), activity: activity(RUN, { children: 3 }), last_event_ms: at(3) }] },
  // 3: fmt finishes; its result reaches builder
  { ...base(11, at(4)), nodes: [fmt(IDLE, 'finished', 4)], edges: [{ a: 'r1.builder', ab: 0, b: 'r1.builder.fmt', ba: 2, id: 'r1.builder|r1.builder.fmt', last_from: 'r1.builder.fmt', last_id: 'x-91', last_ms: at(4), open: 0, results: 1 }], pulses: [{ at_ms: at(4), edge: 'r1.builder|r1.builder.fmt', from: 'r1.builder.fmt', id: 'x-91', kind: 'result', to: 'r1.builder' }] },
  // 4: Scratch deleted
  { ...base(12, at(5)), removed: ['r8'], edges_removed: ['r1|r8'] },
  // 5: stale (seq 12 again)
  { ...base(12, at(6)) },
  // 6: gap (seq 14 after 12)
  { ...base(14, at(7)) },
  // 7: a new activation
  { ...base(1, at(8)), epoch_ms: GRAPH_FIXTURE_EPOCH + 1 },
  // 8: resync
  { ...base(13, at(9)), resync: true },
]
```

### 9.1 Expected layout (exact; `BOX` defaults)

Case A — `R1` with children `A`, `B` (in that order); `A` with children `A1`, `A2`; then a separate root `R2` (leaf), packed with `prev = null`:

| node | x | y |
|---|---|---|
| R1 | 224 | 0 |
| A | 112 | 120 |
| B | 336 | 120 |
| A1 | 0 | 240 |
| A2 | 224 | 240 |
| R2 | 616 | 0 |

`layoutTree('R1').width = 536`; `packRoots` → `R1: 0`, `R2: 616` (536 + 80).

Case B (contour tuck) — `P` with children `C1` (leaf), `C2`; `C2` with children `D1`, `D2`: `C2` is shifted by `max(d0: 200 + 24 − 112) = 112`.

| node | x | y |
|---|---|---|
| P | 112 | 0 |
| C1 | 0 | 120 |
| C2 | 224 | 120 |
| D1 | 112 | 240 |
| D2 | 336 | 240 |

width 536.

Sticky packing — trees `[{R1, 536}, {R2, 200}]` with `prev = {R1: 0, R2: 616}` → unchanged; R1 grows to width 600 → `R2: 680`; R1 gone (`[{R2, 200}]`, same prev) → `R2: 616`; a new `R3` (width 200) after it → `R3: 896`; `prev = null` → `R2: 0`, `R3: 280`.

### 9.2 Expected TTL outcomes (defaults, `GRAPH_FIXTURE_ATTENTION`)

At `GRAPH_FIXTURE_NOW` (T0), `+4 min` (T1), `+10 min` (T2). `pin` = pinned (opacity 1), `ghost` = 0.25, `—` = hidden; numbers are opacities (compare with `toBeCloseTo(x, 3)`).

| node | T0 | T1 | T2 |
|---|---|---|---|
| r1, r1.builder, r1.builder.linter, r1.builder.tests, r1.claude, r1.claude.explore | pin | pin | pin |
| r1.docs | 1 | — | — |
| r1.reviewer | 0.85 | — | — |
| r1.scout | — | — | — |
| r1.claude.plan | 0.55 | — | — |
| r1.codex | 1 | 0.85 | — |
| r2 | 1 | 1 | 1 |
| r2.bisect, r2.repro.min | — | — | — |
| r2.repro | 0.4 | — | — |
| r2.verify | 1 | — | — |
| r2.omp | 1 | ghost | — |
| r2.omp.task1 | 1 | 0.55 | — |
| r2.omp.task2 | 1 | 1 | — |
| r3, r3.sweep, r3.audit, r3.audit.grep, r3.audit.diff | pin | pin | pin |
| r3.rotate | — | — | — |
| r4 | 0.375 | 0.325 | — |
| r4.summarize, r4.index | — | — | — |
| r5, r5.migrator, r5.migrator.check | pin | pin | pin |
| r5.old | — | — | — |
| r6, r6.render | pin | pin | pin |
| r6.fetch | — | — | — |
| r7 | pin | pin | pin |
| r7.reply | 0.55 | — | — |
| r8 | — | — | — |
| peer:alice | 1 | 1 | 1 |
| peer:mini-ops | 1 | 1 | 1 |

At T0: 31 visible, `hidden = 9`; `stripText` = `'4 need you · 11 working · 14 finished'`. `r1.docs.expiresAt = NOW + 4 min`; `r3.audit` is pinned by its live children although itself idle; `r5` by `r5.migrator`; `r3` and `r7` by attention.

### 9.3 Expected edges at T0

| edge | view |
|---|---|
| `r1.builder\|r1.builder.linter`, `r3.audit\|r3.audit.diff`, `r5.migrator\|r5.migrator.check` | active, animated |
| `r1\|r1.builder`, `r1.builder\|r1.builder.tests`, `r1.claude\|r1.codex` | active, not animated |
| `r1\|r1.docs`, `r1\|r1.reviewer`, `r1.builder\|r1.reviewer`, `r1\|r2`, `r2\|r2.repro`, `r2\|r2.verify`, `r3\|r3.sweep`, `r3\|r6`, `peer:alice\|r7`, `peer:mini-ops\|r3` | folded |
| `r1\|r8`, `r4\|r4.summarize` | not drawn (an end hidden) |

`awaitTarget(r1.builder.linter) = 'r1.builder'` (the parent's name is `builder`); `awaitTarget(r3.audit.diff) = 'r3.audit'`.

### 9.4 Expected reducer results (from `fromSnapshot(GRAPH_FIXTURE)`, deltas applied in order)

| delta | result |
|---|---|
| 0 | seq 8; 41 nodes; `r1.builder.fmt` present with parent `r1.builder`; every other node object identical (`toBe`) to before except `r1.builder` |
| 1 | seq 9; 19 edges; 1 pulse |
| 2 | seq 10; `r1.builder` state running |
| 3 | seq 11; `r1.builder.fmt` outcome finished; that edge `results 1`, `open 0`; 2 pulses, the last `result` |
| 4 | seq 12; 40 nodes, no `r8`; 18 edges, no `r1\|r8` |
| 5 | stale: `resnapshot false`, state object unchanged (`toBe`) |
| 6 | gap: `resnapshot true`, state unchanged |
| 7 | new epoch: `resnapshot true` |
| 8 | (applied to the state after 4) resync: `resnapshot true` |

`applyDelta(null, any)` → `{ state: null, resnapshot: true }`. `mergeSnapshot(s, { ...GRAPH_FIXTURE, unchanged: true, nodes: [], edges: [] })` returns `s` itself.

---

## 10 Dev fixture page and UI acceptance (D)

### 10.1 `src/dev/attention-graph-fixture.{html,tsx}`

The `src/dev/attention-fixture.{html,tsx}` pattern (bare HTML loading `/theme.js`, `/theme.css` and the module; a fake `Api` Proxy; `active().connection` set open with discovery `legacy`; a memory-history router with `/attention` → `Page`; the providers). Additionally:
- `setGraphSource(…)` with a simulated engine seeded from `graphFixture(Date.now())`: an accelerated clock (`?speed=`, default 60: one real second = one simulated minute) driving the TTL (the region's `now` comes from the simulated clock: export a `setGraphClock(fn | null)` from `queries.ts` for this page only, default `Date.now`).
- A script, looping: every 2 s a random event among — spawn a child under a live node; a state change (running → awaiting → running, running → approval → running, running → idle with an outcome); a message between two visible nodes (edge upsert + `sent`, 1 s later `delivered`, sometimes `replied`); a child result (`result` pulse); a new root every ~30 s; a remote root's state flip. Deltas go through `applyDelta` exactly as live ones (seq + 1 each); one deliberate gap per minute to exercise the re-snapshot path.
- Attention items whose sessions are fixture nodes (`r1.builder`, `r3`, `r3.sweep`, `r7` ×2): take items from `attentionFixture(Date.now())` (`components/settings/attention-fixtures.ts`) and set `session`, `root_session`, `parent_session`, `path` to match; served by the fake `attention_list`.
- `?theme=dark` like the other pages. Header comment: how to run (`bunx vite`, open `/src/dev/attention-graph-fixture.html`).

Seen in a browser (headless Chromium is fine): collapsed strip text, expanded canvas with dots, minimap, controls; nodes at the three LODs; a needs-you ring; an animated edge; a fading node; Show in graph from a card; a node click filtering the list; `G`, `Shift+G`, `Esc`. Record what was seen (and screenshots' paths under `/tmp/`) in `208-as-built-D.md`.

### 10.2 Tests (vitest, node env)

- `layout.test.ts` — §9.1 cases A and B exactly; sticky packing table; determinism (same input → same output, child order by `started_ms` then id); `treeSignature` changes only on structure; `layoutForest` reuses a cached tree object for an unchanged signature.
- `ttl.test.ts` — the §9.2 table at T0/T1/T2 (every row), `hidden` count, `expiresAt` of `r1.docs`; `fadeOpacity` at 0, 0.75·ttl, 0.875·ttl, ttl; pinning by attention, by liveness, by a live grandchild; ghost; peer visibility follows its edges; §9.3 edge table; `awaitTarget` (parent name, `parent`, `session:` id, child, sibling, unknown → null); `nextTtlTick` bounds; `windowMs(TTL_DEFAULTS) = 1440 min`.
- `reducer.test.ts` — §9.4 row by row; identity preservation.
- `selectors.test.ts` — `pathToRoot('r1.builder.linter') = ['r1.builder.linter', 'r1.builder', 'r1']`; `subtreeIds('r3')` = the 6 `r3*` ids; `stripCounts` at T0 = `{needYou: 4, working: 11, finished: 14}` and its text; `attentionBySession` skips expired items and items without a session; `includeSessions` unique, sorted, capped at 200; `statusText` for each state/wait/outcome.
- `prefs.test.ts` — defaults on `null`/garbage; clamps; partial objects.
- `NodeBody.test.ts` — one node per LOD from the fixture (`r1.builder` full: "approval: bash", badge 1, ring class; `r1.claude` shows the claude harness glyph; `r1.claude.explore` the eye mark; `r6` "remote · mini"; dot LOD renders no text); no emoji.
- `Strip.test.ts` — the T0 text and "+3 older trees".
- `model.test.ts` — `matchesFilter` with a subtree (in, out, item without session), `EMPTY_FILTER.subtree === null`, `pruneFilter` keeps the subtree.
- `Card.test.ts` — "Show in graph" rendered with `onShowInGraph` and a session, absent otherwise.

Gate (`/tmp/theuth-208-d-gate.sh`, one hold, §0), in `crates/desktop/ui` (node_modules: `bun install` once inside the hold, which `bun add @xyflow/react@13.0.2 --exact` does):
```
bun add @xyflow/react@13.0.2 --exact      # first hold only; commits package.json + bun.lock
bun run check                               # tsc -b --noEmit, against the hand-patched bindings
bunx vitest run src/components/attention src/components/sidebar src/lib/no-emoji.test.ts
bun run build                               # the canvas must be its own chunk: list dist/assets and record the chunk containing "react-flow" and its size
```

UI behaviour checklist:
1. The region sits above the list and detail; collapsed strip text from `stripText`; expanded canvas with dotted background, minimap, controls; persisted height/expanded; full screen; md: collapsed, opens full screen.
2. Positions are the tidy tree; nothing moves when only a state changes; new trees appear at the right.
3. Indicators are the sidebar's (§8.5); three LODs by zoom.
4. TTL fading and expiry per §3.1 from the simulated clock; ghosts keep trees whole; TTL popover edits apply at once.
5. Message edges: dashed, animated while open/awaited, folded after 5 min, count label, popover with on-demand messages.
6. List ↔ graph: highlight of the focused/open card's session and its path; Show in graph; node click filters the list (chip in the toolbar, clearable); Open session.
7. Live updates through `applyDelta`; re-snapshot only on gap/epoch/resync/reconnect; renewal every 120 s.
8. `@xyflow/react` only in the lazy chunk; reduced motion honoured; no emoji.

---

## 11 Integration notes (F)

1. Merge D, then B, into `feat/208-attention-graph` (or the reverse; there is no file overlap by §1). Conflicts can only be in files outside §1 — stop and report if any.
2. Run B's gate steps that touch the wire once more on the merged tree (`export_bindings`, the classifier — accept only `Additive`/`Nonbreaking` for the §5.2 paths), then regenerate the UI bindings (`bun run wire:generate` + the `ts-desktop` copy lines) and discard D's hand patches. Any name or shape difference between §7 and the generated files is B's to fix (this contract is the source); literal fixes in D's tests/fixture for required-field differences are F's.
3. `PeerMessageChanged` output gains required `from`/`to`/`result`: grep `src/` for `PeerMessageChanged` literals and add them (`peer_2dmessage` consumers).
4. Wire the real data: `useAgentGraph` default source (api + `onTopic('agent.graph@1')`); check that `subscribe_topics(null, null, null)` delivers `agent.graph@1` (the `always_wanted` policy) by a dev engine run if one is available; otherwise record "not run against a live engine".
5. UI gate on the merged tree (`bun run check`, `bun run test`, `bun run build`) and B's crate-scoped gate once; record counts in `docs/design/208-as-built.md` (the 207 format: history table, gates table, reds with evidence, not verified).
6. Not this round: companions, cross-engine relay, drive-from-node (208 §13).
