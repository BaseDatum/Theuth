# 207 contract — Attention flat list (round 1: builders A and C)

Single source of truth for **A** (Rust attention engine) and **C** (desktop UI list). Design and rationale: `docs/design/207-attention-flat-list.md` on `feat/207-attention` (read §0 and §5 first). Base: main `729093ffc`. Each builder works in its own worktree off `feat/207-attention`; nobody talks to the other; where this file and the code disagree, the code wins and the builder records the deviation in `docs/design/207-as-built-<A|C>.md`.

Rules for both: no build or test runs while another session holds `/tmp/theuth-build.lock` (never touch it); skip build/lint/tests mid-flight, run the gates once at the end. No emoji anywhere. No new dependencies (`@tanstack/react-virtual` is already in `crates/desktop/ui/package.json:30`).

---

## 1 Ownership (no overlap)

**A may touch:**
- `crates/roster/src/{attention.rs, alert.rs, config.rs, rpc.rs, rpc_support.rs, lib.rs, handoff.rs, desk.rs}`, new `crates/roster/src/score.rs`, new `crates/roster/migrations/0008_attention_stopped.sql`
- `crates/tools/src/{approval.rs, ask.rs, registry.rs, lib.rs}` (lib.rs: re-exports only)
- `crates/agent/src/supervisor/waits.rs` (approval replay of a dismissal only)
- `crates/tasks/src/{lib.rs, ledger.rs, escalate.rs, packet.rs, rpc_hub.rs}` (rpc_hub only if the activity receipt needs it)
- `crates/secrets/src/{lib.rs, tools.rs, requests.rs}`
- `crates/double/src/actor.rs`, `crates/autoresearch/src/actor.rs` (+ `actor/tests.rs`), `crates/a2a/src/contacts.rs` (+ `contacts/e2e_tests.rs`): item construction, labels, deadline only
- `crates/rpc-compose/tests/fixtures/wire/{roster.attention.json, tools.approval-resolved.json, tools.ask-resolved.json}`, `crates/rpc-compose/src/api/tests.rs`, `crates/rpc-compose/tests/alerts.rs`
- `crates/theuth/tests/fixtures/wire/openrpc-v1.json` (only via `just wire-schema-accept`)
- `docs/design/207-as-built-A.md`

**C may touch:**
- `crates/desktop/ui/src/components/attention/**`
- `crates/desktop/ui/src/components/hud/{Hud.tsx, Hud.test.ts, queries.ts}`
- `crates/desktop/ui/src/components/settings/{sandbox-queries.ts, attention-fixtures.ts (new), Contacts.test.ts (import line only)}`
- `crates/desktop/ui/src/routes/attention.tsx, attention.index.tsx, attention.$id.tsx`
- `crates/desktop/ui/src/rpc/bindings/**` — git-ignored, hand-patched per §8, **never committed**
- `docs/design/207-as-built-C.md`

**Nobody touches:** `crates/desktop/companion/**` (Swift), `crates/desktop/companion-windows/**` (C#), `crates/agent/src/store*.rs`, any `sessions` migration, `SESSIONS_WIRE_SCHEMA`, `components/attention/requirements.ts` and `components/hud/requirements.ts` (route gating; the new methods are feature-detected, §10.6).

---

## 2 Rust types (A)

### 2.1 `crates/roster/src/attention.rs` — new types

Derives exactly as the neighbouring wire types (`AttentionKind` `:36-38`, `ItemContext` `:158-159`). The UI's TS comes from schemars through OpenRPC (`crates/desktop/ui/scripts/generate-wire.mjs`), the `ts_rs` derive is kept for parity with the neighbours.

```rust
/// 207 D3: one contributor to an item's score.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum AttentionSignal {
    /// The kind's base (207 §3.1).
    Base,
    /// The item's session is live right now.
    Live,
    /// Sessions stopped behind the item.
    Blocks,
    /// The request resolves itself soon.
    Deadline,
    /// The approval's risk class.
    Risk,
    /// The task's priority and project rank.
    Task,
    /// Minutes waited.
    Age,
}

/// 207 D3: one line of an item's score breakdown.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct AttentionReason {
    /// Which signal.
    pub signal: AttentionSignal,
    /// Its points (> 0, except `base` which is always listed).
    pub points: u32,
    /// Short text for the card ("blocks 3 agents", "denies itself in 1:42"); a snapshot at derive time.
    pub text: String,
}

/// 207 D5: how a session's turn stopped (`finished` items).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum StopOutcome {
    /// Any provider stop reason that is not one of the three below (`end_turn`, `stop`, `max_tokens`, …).
    Finished,
    /// `stop_reason == "error"`.
    Failed,
    /// `stop_reason == "cancelled"` (Stop, a parent's cancel, `max_runtime`).
    Cancelled,
    /// `stop_reason == "timed_out"` (the peer-work deadline).
    TimedOut,
}

/// 207 D5: the stop behind a `finished` item.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct AttentionStop {
    /// Outcome class.
    pub outcome: StopOutcome,
    /// The raw `TurnFinished.stop_reason`.
    pub reason: String,
    /// The turn's last `AgentEvent::Error` message, cut to `SUMMARY_MAX` chars; `None` when there was none.
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub error: Option<String>,
}

/// 207 §5.3: what a bulk call acts on — exactly one of `ids` (non-empty) or `filter`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct AttentionTarget {
    /// Item ids, in the caller's order.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub ids: Vec<String>,
    /// Every listed item matching this, in engine order.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub filter: Option<AttentionFilter>,
}

/// 207 §5.3: a filter over the current list. Every part that is set must match.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct AttentionFilter {
    /// Any of these kinds; empty = every kind.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub kinds: Vec<AttentionKind>,
    /// Any of these projects; `""` matches items without a project; empty = every project.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub projects: Vec<String>,
    /// Only items with `at_ms <= before_ms` (the newest the caller had seen); `None` = no bound.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional, type = "number | null"))]
    pub before_ms: Option<u64>,
}

/// 207 §5.2: what a bulk call did to one id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum AttentionResolution {
    /// Hidden (119, versioned): review, hire, budget, node, home, blocked, show, finished, double, resume, contact_first, autoresearch:*.
    Hidden,
    /// Snoozed until the given time.
    Snoozed,
    /// A live tool call was told to decide itself (ask, live decision).
    LeftToAgent,
    /// Recorded for the agent's next wake; nothing woken (parked decision).
    LeftForLater,
    /// An approval the user left to the agent: the call was not run.
    NotRun,
    /// A secret request or hand-off resolved "none is coming, proceed without".
    ProceedWithout,
    /// Not in the list any more, or already resolved elsewhere.
    Gone,
    /// The resolver failed; nothing was written. See `error`.
    Failed,
}

/// 207 §5.3: one id's result of a bulk call.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct AttentionOutcome {
    /// The item id.
    pub id: String,
    /// What happened.
    pub resolution: AttentionResolution,
    /// The resolver's message when `resolution` is `failed`.
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub error: Option<String>,
}
```

### 2.2 `AttentionItem` — appended fields (after `alert`, in this order)

Every one `#[serde(default)]` (absence accepted by the receiver view ⇒ the classifier rates them `Additive`, `crates/rpc-core/src/compatibility.rs:289-295`). Also update the `id` doc comment to list every real id form: `approval:<id>` · `ask:<id>` · `decision:<task>:<decision>` · `hire:<agent>` · `blocked:<task>` · `review:<task>` · `budget:<agent>` · `node:<engine>` (kind node or home) · `misplaced:<task>` · `handoff:<id>` · `secret:<id>` · `show:<call_id>` · `resume:<session>:<turn>` · `resume:boot:<ms>` · `finished:<session>:<turn>` · `double:<question>` · `contact_first:<contact>:<export>` · `contact_roster:<record>` · `autoresearch:{finished,paused,reverted,pair,launch}:…`.

```rust
    /// 207 D3: the engine's score — the sum of `reasons`; the list is ordered by it (§3.4).
    #[serde(default)]
    pub score: u32,
    /// 207 D3: the score's contributors, points desc (ties in `AttentionSignal` order); `base` always present.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<Vec<AttentionReason>>", optional))]
    pub reasons: Vec<AttentionReason>,
    /// 207 R5: approvals — `ApprovalRequested.risk`; `None` for other kinds.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub risk: Option<tools::Risk>,
    /// 207 R5: approvals — the tool name (`ApprovalRequested.tool`); `None` for other kinds.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub tool: Option<String>,
    /// 207 R5: ms since epoch the request resolves itself (approval/ask/hand-off/secret timeout, a double's grace end).
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional, type = "number | null"))]
    pub deadline_ms: Option<u64>,
    /// 207 R4: ms since epoch the item leaves the list on its own (`finished`: stop + `stopped_ttl_mins`).
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional, type = "number | null"))]
    pub expires_ms: Option<u64>,
    /// 207 D5: sessions stopped behind this item (transitive `awaiting` on its session).
    #[serde(default)]
    pub blocks: u32,
    /// 207 R3: the item's session is running / awaiting / in approval right now.
    #[serde(default)]
    pub session_live: bool,
    /// 207 R5: the session's effective project (its root's), else the task's; `None` when neither.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub project: Option<String>,
    /// 207 R5: the root of the item's session tree (itself for a root).
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub root_session: Option<String>,
    /// 207 R5: the item's session's parent; `None` for a root.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub parent_session: Option<String>,
    /// 207 R5: display path, root first: the root's title, then each child's peer name; empty when the session is unknown.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<Vec<String>>", optional))]
    pub path: Vec<String>,
    /// 207 R4: `finished` items — how the turn stopped.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub stop: Option<AttentionStop>,
```

Add `impl AttentionItem { pub fn new(id: String, kind: AttentionKind, title: String, detail: Option<String>, at_ms: u64) -> Self }` returning every other field at its default (`waiting_since_ms: Some(at_ms)`, `rank: tasks::Priority::Medium.weight()`, the rest `None`/empty/0/false) — exactly the `item` closure of `derive` (`attention.rs:384-406`), which then calls it. The three Sources (`autoresearch/src/actor.rs:5663-5690`, `double/src/actor.rs:793-813`, `a2a/src/contacts.rs:3085-3108`, `:3115-…`) switch to `AttentionItem { field: …, ..AttentionItem::new(…) }` so future fields never touch them again.

Session ids stay `String` like `session`. `Eq`: `AttentionChanged` derives `Eq` today; add `Eq` to `AttentionItem` if every field allows it (all of them do as far as W0 read: `AskRequested`, `ItemContext`, `Alert`, `Priority`, `Risk`, the new types are `Eq`), else drop `Eq` from `AttentionChanged`.

### 2.3 `AttentionChanged` — appended fields

```rust
    /// 207 §7: every item added or changed (content other than `score`/`reasons`/`alert`) since the previous
    /// event, with `alert` set to `None` (alerts are per connection; see `alerts_changed`).
    #[serde(default)]
    pub items: Vec<AttentionItem>,
    /// 207 §7: every listed id in engine order after this derive.
    #[serde(default)]
    pub order: Vec<String>,
    /// 207 §7: 1-based per engine run, +1 per published event; 0 = an engine before 207.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub seq: u64,
    /// 207 §7: ms since epoch the `Attention` actor started (a new value = a new run; `seq` restarts).
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub epoch_ms: u64,
    /// 207 §7: some item's `alert` appeared, changed or went away: refetch `attention_list` (masked per connection).
    #[serde(default)]
    pub alerts_changed: bool,
```

Topic stays `roster.attention`, `VERSION` stays 1.

### 2.4 Other crates

`crates/tools/src/approval.rs`:
```rust
// ApprovalResolved gains (after timed_out):
    /// 207: the user left the call to the agent (Attention dismiss): not run; `decision` is `deny`, `timed_out` false.
    #[serde(default)]
    pub dismissed: bool,

/// 207: `ask` → `bool` (true when it was pending): the user leaves this call to the agent — it is not run.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Dismiss {
    /// The request.
    pub id: ApprovalId,
}
```
Internal (not on the wire): the pending oneshot carries `enum Verdict { Decided(Decision), Dismissed }`; `RequestApproval`'s waiter maps `Dismissed` → `ApprovalResolved { decision: Deny, timed_out: false, dismissed: true }`. The journal row settles `Answered`, `answer = json!("dismissed")`, `answered_by = Human`. `HonourAnswer` gains `dismissed: bool` (serde default if it derives Serialize) and the `honoured` map stores `(Decision, bool)`; `crates/agent/src/supervisor/waits.rs:200-212` maps a stored answer `"dismissed"` to `HonourAnswer { decision: Deny, dismissed: true, .. }` (a `Decision` parse keeps working for `"allow"`/`"deny"`).

`crates/tools/src/registry.rs:2043-2050`, new arm before `timed_out`:
```rust
Ok(r) if r.dismissed => return ToolOutput::err(format!(
    "{}: not run — the user left this call to you without approving it. Do not retry it as is; find a way that needs no approval, or say in your reply why it is needed.",
    spec.name
)),
```

`crates/tools/src/ask.rs`:
```rust
// AskResolved gains (after timed_out):
    /// 207: the user left the questions to the agent (Attention dismiss); the turn goes on.
    #[serde(default)]
    pub left_to_agent: bool,

/// 207: `ask` → `Option<SessionId>`: the owning session if `id` was pending and is now resolved as
/// "left to the agent". Never cancels the turn (unlike `AnswerAsk { answers: None }`).
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct LeaveAsk {
    /// The request.
    pub id: AskId,
}
```
Internal: `Outcome` gains `left_to_agent: bool`; the waiter channel carries it. `outcome()` (`ask.rs:543-549`), first arm:
```rust
_ if out.left_to_agent => {
    // ok, not err: the agent continues; the structured map is `{<question id>: null}` per question.
    ToolOutput::ok("ask_user: the user left these questions to you — decide yourself, say in your reply what you chose, and continue.")
        .with structured = { "<id>": null, … }
}
```
The journal row settles `Answered` with `answer = json!({"left_to_agent": true})`, `answered_by = Human`. The published `AskResolved` has `dismissed: false, timed_out: false, left_to_agent: true`. `crates/tools/src/lib.rs` re-exports `LeaveAsk` and `approval::Dismiss` beside their neighbours.

`crates/double/src/actor.rs`: `ApprovalResolved` with `dismissed` and `AskResolved` with `left_to_agent` drop the pending card exactly like `AskResolved.dismissed` (`:1041-1055`) — no label. `item()` (`:762-814`): `at_ms` = the card's arrival time (a field on `PendingCard`; add `since: DateTime<Utc>` if none exists), `waiting_since_ms: None` as today, `deadline_ms: pc.grace_until` (ms) while in grace, `None` once answered. All 14 `ApprovalResolved { … }` and 9 `AskResolved { … }` literals in the workspace gain the new field (`false`).

`crates/tasks`:
```rust
// packet.rs — Packet gains (last field):
    /// 207 §5.2: ms since epoch the user left this decision to the agent (Attention dismiss).
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional, type = "number | null"))]
    pub left_to_agent_ms: Option<u64>,

// lib.rs
/// 207: blocker prefix of a decision the user left to the agent: `left:decision:<id>`.
pub const LEFT_PREFIX: &str = "left:";
/// 207: what a live `escalate` returns when the user leaves its decision (`decision: <this>`).
pub const LEFT_ANSWER: &str =
    "left to you — the user chose not to decide. Pick the option you would recommend, record it with task_note, and continue.";

/// 207: `ask` → `Result<LeftDecision, String>`: the user leaves `decision` to its task's owner. Never wakes anyone.
/// `Err("no task is blocked on …")` when no task carries the blocker.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LeaveDecision {
    /// `decision:<id>`.
    pub decision: String,
    /// Who (`rpc_core::Client::actor`).
    pub actor: String,
}
/// 207: whether an `escalate` was still waiting live for the answer.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, kameo::Reply)]
pub struct LeftDecision { pub live: bool }

// escalate.rs
impl Escalations { pub fn waiting(&self, decision: &str) -> bool }

// ledger.rs
/// 207: the thread line, `Packet.left_to_agent_ms`, a `disposition` event `left_to_agent:<decision>`; then
/// live → drop the blocker and, when none is left on a blocked task, `doing` under a live unfenced checkout
/// else `todo`; parked → replace the blocker by `left:<decision>`, state unchanged. Never calls `wake`.
pub async fn leave_decision(&mut self, decision: &str, actor: &str, live: bool) -> R<()>;
```
Handler order: `live = escalations.waiting(&d)`; `ledger.leave_decision(&d, &actor, live)`; if live `escalations.answer(&d, LEFT_ANSWER)`; `tokio::spawn(drop_card(d))`. Thread line: `**decision** (`<decision>`): left to the agent — the user did not decide; choose yourself`.

`crates/roster/src/desk.rs` `wake_text` (`:258-…`): for each blocker of the task starting with `tasks::LEFT_PREFIX`, append `\n\nThe user left `<rest>` to you: choose yourself, record what you chose with task_note, then remove the `<blocker>` blocker with task_update.` (update `wake_text_snapshot`).

`crates/secrets`:
```rust
/// 207: outcome of a request the user left to the agent (beside `stored` | `declined` | `timeout`).
pub const PROCEED_WITHOUT: &str = "proceed_without";
/// 207: `ask` → `Result<bool, String>` (true when it was unanswered): resolve as "none is coming".
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Waive { pub id: RequestId }
```
`Waive` writes `requests::answer(pool, id, PROCEED_WITHOUT, now)` and wakes the local waiter like `Answer` (`lib.rs:546-588`). The wait loop's `Outcome` gains `waived: bool` (`outcome == PROCEED_WITHOUT`); `not_stored` (`tools.rs:192-198`) and every other caller return `ToolOutput::ok(format!("no secret is coming: the user left this to you — proceed without {names}, and say in your reply what you could not do."))` with `names` = the requested field names joined by ", ".

`crates/roster/src/handoff.rs`: `pub const PROCEED_WITHOUT: &str = "proceed_without";`; `result()` (`:300-313`): `proceed_without` → `ToolOutput::ok(format!("nobody is coming to the desktop: the user left this to you — proceed without it (after {elapsed_secs} s)"))`, structured unchanged. `ResolveHandoff` keeps refusing anything but `done`/`cancelled`.

---

## 3 Scoring (A; pure, in `crates/roster/src/score.rs`)

### 3.1 Config — `crates/roster/src/config.rs`

```rust
// RosterConfig gains (struct is already #[serde(default)]):
    /// 207: `roster.attention` — the flat list's score weights and timers.
    pub attention: AttentionConfig,

/// 207: `roster.attention:` — read on every recheck (boot snapshot: a change needs an engine restart).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AttentionConfig {
    /// A `finished` item leaves the list this long after the stop.
    pub stopped_ttl_mins: u64,
    /// The coarse re-score / expiry tick.
    pub rescore_secs: u64,
    /// Score weights.
    pub weights: ScoreWeights,
}
impl Default for AttentionConfig { stopped_ttl_mins: 45, rescore_secs: 60, weights: ScoreWeights::default() }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScoreWeights {
    pub stops_agent: u32,      // 600
    pub mesh: u32,             // 450
    pub yours: u32,            // 300
    pub shown: u32,            // 120
    pub notice: u32,           // 120
    pub stopped_failed: u32,   // 150
    pub stopped: u32,          // 80
    pub double: u32,           // 20
    pub live: u32,             // 300
    pub blocks_each: u32,      // 40
    pub blocks_max: u32,       // 200
    pub deadline_max: u32,     // 250
    pub destructive: u32,      // 100
    pub mutating: u32,         // 30
    pub task_per_weight: u32,  // 10
    pub task_ranked_project: u32, // 10
    pub task_max: u32,         // 100
    pub age_per_min: u32,      // 1
    pub age_max: u32,          // 60
}
```
YAML (`config.yaml`):
```yaml
roster:
  attention:
    stopped_ttl_mins: 45
    rescore_secs: 60
    weights:
      live: 300
```
Not on the wire (no schemars). `Inputs` gains `config: AttentionConfig` (Default) so existing tests keep compiling; `gather` fills it from `rt.section::<RosterConfig>("roster").unwrap_or_default().attention`.

### 3.2 Base class

```rust
pub enum BaseClass { StopsAgent, Mesh, Yours, Shown, Notice, StoppedFailed, Stopped, Double }
pub fn base_class(item: &AttentionItem) -> BaseClass {
    use AttentionKind as K;
    let id = item.id.as_str();
    match item.kind {
        K::Approval | K::Ask | K::Secret | K::Handoff | K::Decision | K::ContactFirst => StopsAgent,
        K::Resume => if id.starts_with("resume:boot:") { Notice } else { StopsAgent },
        K::Node | K::Home => Mesh,
        K::Blocked => if id.starts_with("autoresearch:launch:") { StopsAgent } else { Yours },
        K::Review | K::Hire => Yours,
        K::Budget => if item.detail.as_deref().is_some_and(|d| d.starts_with("paused")) { Yours } else { Notice },
        K::Show => Shown,
        K::Finished => match item.stop.as_ref().map(|s| s.outcome) {
            Some(StopOutcome::Failed | StopOutcome::TimedOut) => StoppedFailed,
            _ => Stopped,
        },
        K::Double => Double,
    }
}
```

Base text (the `base` reason's `text`), first match wins:

| Condition | Text |
|---|---|
| kind finished | `finished` / `failed` / `cancelled` / `timed out` (by `stop.outcome`; `finished` when `stop` is None) |
| id `autoresearch:launch:` | `launch waits for your press` |
| id `resume:boot:` | `restart notice` |
| id `misplaced:` | `agent elsewhere` |
| id `contact_roster:` | `mesh alert` |
| kind home | `machine offline` |
| kind budget | `budget paused` (detail starts `paused`) else `budget at 80 %` |
| else by kind | approval `approval` · ask `question` · secret `secret request` · handoff `hand-off` · decision `decision` · resume `interrupted turn` · contact_first `first contact` · node `engine unreachable` · review `review` · hire `hire` · blocked `blocked task` · show `shown to you` · double `answered by your double` |

### 3.3 Formula

```rust
/// 207 §3: score and reasons of `item` at `now_ms`. Pure. Integer arithmetic except the deadline share.
pub fn score(item: &AttentionItem, w: &ScoreWeights, now_ms: u64) -> (u32, Vec<AttentionReason>)
```
1. `base = w[base_class(item)]`, always pushed.
2. `live = w.live` if `item.session_live && !matches!(item.kind, Double | Finished)`; text `agent waiting on you` when `base_class == StopsAgent`, else `session running`.
3. `blocks = min(item.blocks * w.blocks_each, w.blocks_max)` when `item.blocks > 0`; text `blocks 1 agent` / `blocks {n} agents`.
4. Deadline, when `item.deadline_ms = Some(d)`: `start = item.waiting_since_ms.unwrap_or(item.at_ms)`; if `d <= start || now_ms >= d` → `w.deadline_max`; else `share = (d - now_ms) as f64 / (d - start) as f64`; `share >= 0.5` → 0; else `(w.deadline_max as f64 * (1.0 - 2.0 * share) + 0.5).floor() as u32` (round half up). Pushed only when > 0. Text: `now_ms >= d` → `due now`; else `{verb} {countdown(d - now_ms)}` with verb `denies itself in` (approval), `double answers in` (double), `times out in` (others); `countdown(ms)`: `s = ms / 1000`; `h = s / 3600`; `h > 0` → `format!("{h}:{:02}:{:02}", s % 3600 / 60, s % 60)` else `format!("{}:{:02}", s / 60, s % 60)`.
5. Risk: `Some(Destructive)` → `w.destructive` text `destructive`; `Some(Mutating)` → `w.mutating` text `mutating`; else nothing.
6. Task, only when `item.priority = Some(p)`: `min(w.task_max, (item.rank % 100) * w.task_per_weight + if item.rank >= 100 { w.task_ranked_project } else { 0 })` (`rank = project_score × 100 + weight`, 132's `tasks::rank::rank`); text `{p.as_str()} priority` + ` · ranked project` when `rank >= 100`.
7. Age: `since = item.waiting_since_ms.unwrap_or(item.at_ms)`; `mins = now_ms.saturating_sub(since) / 60_000`; points `min(mins * w.age_per_min, w.age_max)`, pushed when > 0. Text: with `waiting_since_ms` → `waiting {mins} min` (`waiting {mins/60} h` from 60); without → `stopped {mins} min ago` for finished, `{mins} min ago` otherwise (`… {mins/60} h ago` from 60).
8. Sort reasons by points desc, ties by `AttentionSignal` declaration order; `score = Σ points`.

### 3.4 Order and tie-break

`score` desc → `waiting_since_ms` asc (`None` last) → `at_ms` desc → `id` asc (byte order). Replaces `attention.rs:971-982`. `rank` is still filled as today and no longer sorts anything.

### 3.5 Derive inputs (A)

`Inputs` gains (all `Default`):
```rust
    /// 207: session id → where it sits (from `ListSessions`).
    pub lineage: HashMap<String, Lineage>,
    /// 207: sessions live now (`ListActivity` state running/awaiting/approval, or `SessionSummary.running`).
    pub live: HashSet<String>,
    /// 207 D5: session id → sessions stopped behind it.
    pub blocks: HashMap<String, u32>,
    /// 207: item id → deadline ms (approval, ask, hand-off, secret).
    pub deadlines: HashMap<String, u64>,
    /// 207: weights and timers.
    pub config: crate::config::AttentionConfig,

/// 207: where a session sits in its tree.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Lineage {
    pub project: Option<String>,
    pub root: String,
    pub parent: Option<String>,
    pub path: Vec<String>,
}
/// 207: lineage of every listed session; `desks` = session → desk agent name (for a desk root's title).
pub fn lineage_map(sessions: &[agent::SessionSummary], desks: &HashMap<String, String>) -> HashMap<String, Lineage>;
/// 207 D5: session → count of distinct sessions transitively awaiting it.
pub fn blocks_map(rows: &[agent::AgentInfo]) -> HashMap<String, u32>;
/// 207: the live set.
pub fn live_set(rows: &[agent::AgentInfo], sessions: &[agent::SessionSummary]) -> HashSet<String>;
```
- `lineage_map`: walk `parent` over the `ListSessions` reply (≤ 32 hops, like `desk_map` `attention.rs:1624-1640`). `project = s.project` (already the root's). Path segments: root → `title` trimmed, else `<desk> desk` when the root is a desk, else the first 8 chars of its id; every child → `name` (peer name), else `title`, else 8 chars. `root` = the root's id; `parent = s.parent`.
- `blocks_map`: for each row `T` with `state = Awaiting { from }` resolve `from`: `"parent"` → `T.parent` (when `!= T.id`); a row whose `parent == T.id && name == from` (a child); a row with `parent == T.parent && id != T.id && name == from` (a sibling); `from` = `session:<uuid>` → that id; else unresolved. Edge `T → target`. `blocks[S]` = number of distinct `T != S` from which `S` is reachable along edges (visited set; cycles safe).
- `live_set`: rows with `state ∈ {Running, Awaiting{..}, Approval{..}}`, plus every `SessionSummary` with `running == true`.
- Deadlines (`gather`, cached per id on the actor, retained to listed ids): approval `approval:<id>` and ask `ask:<id>` and hand-off `handoff:<id>` → `DurableStore.question_get(<id>)` `deadline` when a row exists; else arrival + `tools.approval_timeout_secs` / `tools.ask_timeout_secs` (`tools::ToolsConfig`) / `sandbox.handoff_timeout_secs` (handoff.rs `SandboxSection`). Secret → `created_ms + secrets.request_timeout_secs` (`secrets::SecretsConfig`).

`derive` fills, for every item including `inputs.external`: `project/root_session/parent_session/path` from `lineage[session]` (task-born items without a known session: `project = task.project`); `session_live = live.contains(session)` (task-born items: only when `session` is the task's checkout session); `blocks = blocks[session]`; approvals: `risk`, `tool`, `deadline_ms`; asks/secrets/hand-offs: `deadline_ms`; finished: `stop`, `expires_ms = at_ms + stopped_ttl_mins × 60 000`; External items whose Source left `priority` empty get `(priority, rank) = by_session(session)`. Then drop hidden (119) and expired (`expires_ms <= now`), then `score`, then sort (§3.4).

Fixes inside `derive`: ask items `at_ms` = `ask_since` (was `now_ms`, `attention.rs:502`); approval title `"{who} wants to run {tool}"` with `who(desk, session)` (was the raw session id, `:453`), and `who` falls back to the lineage's last path segment before the 8-char id; the `Blocked` branch ignores blockers starting with `tasks::LEFT_PREFIX` (`:605`: `any(|b| !b.starts_with("decision:") && !b.starts_with(tasks::LEFT_PREFIX))`).

---

## 4 Session stopped (A)

Migration `crates/roster/migrations/0008_attention_stopped.sql`:
```sql
-- 207 R4: the session-stopped card — every stop outcome of a root / desk turn, not only `end_turn`.
-- Engine-local like 0006 (not replicated). Existing rows read as a clean finish.
ALTER TABLE attention_finished ADD COLUMN outcome TEXT NOT NULL DEFAULT 'finished';   -- finished | failed | cancelled | timed_out
ALTER TABLE attention_finished ADD COLUMN stop_reason TEXT NOT NULL DEFAULT 'end_turn';
ALTER TABLE attention_finished ADD COLUMN error TEXT;                                  -- ≤ 160 chars
```
No other migration this round: tasks (`0009` stays the last; `Packet.left_to_agent_ms` lives in `packets.json`), secrets (`0003`), agent (`0072`) untouched. **No replicated `sessions` column is added.**

`Finished` gains `outcome: StopOutcome, stop_reason: String, error: Option<String>`; `record_finished` / `finished()` read and write them.

Actor (`AgentEvent` handler `attention.rs:1872-1907`):
- `Error { session, message, .. }` → keep the last message per running session (`errors: HashMap<SessionId, String>`, cleared on `TurnStarted`).
- `TurnFinished { stop_reason }`: `"interrupted" | "suspended"` → drop the text and error, no row. Otherwise `finish_turn(session, turn, outcome, stop_reason, error)` with `outcome` = `error` → Failed, `cancelled` → Cancelled, `timed_out` → TimedOut, else Finished. `finish_turn` keeps its pending-approval/ask skip and `reports_finished`. Summary: `parse_finished(text, fallback)` where `fallback` = the error message for Failed/TimedOut when present, else the root title.
- `TurnStarted { session, .. }` → `DELETE FROM attention_finished WHERE session = ?` (and recheck when a row went).
- Rescore tick (`RescoreTick`, a self-`tell` every `rescore_secs`, armed in `start`): `DELETE FROM attention_finished WHERE at_ms <= now - ttl`, then `recheck_quietly`.

Titles: `{who} finished` / `{who} failed` / `{who} was cancelled` / `{who} timed out`; `detail` = summary (Failed with no summary: the error).

---

## 5 Dismiss and snooze (A)

### 5.1 Actor messages (`attention.rs`) — replace `DismissAttention` and `SnoozeAttention`

```rust
/// `ask` → `Result<Vec<AttentionOutcome>, String>`: 207 §5 per kind; one recheck after the batch.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DismissMany { pub target: AttentionTarget, pub actor: String }
/// `ask` → `Result<Vec<AttentionOutcome>, String>`: hide every target id until `until_ms` (every kind; outcome `snoozed`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SnoozeMany { pub target: AttentionTarget, pub until_ms: u64 }
/// `tell` (internal): the coarse tick.
#[derive(Clone, Copy, Debug)]
pub struct RescoreTick;
```
Target resolution: `ids` non-empty and `filter` None → those ids, outcomes in that order; `ids` empty and `filter` Some → every item of a fresh derive matching it (kinds any-of, projects any-of with `""` = `None`, `at_ms <= before_ms`), in engine order; anything else → `Err("attention target: give ids or a filter, not both")` / `Err("attention target: give ids or a filter")`. An id not in the current list → `gone`.

### 5.2 Per kind (`DismissMany`)

| Item | Call | Resolution |
|---|---|---|
| approval | `ActorRef::<tools::Approval>::lookup(tools::APPROVAL_ACTOR)` ask `tools::approval::Dismiss { id }`; true → record activity `ActivityKind::ApprovalResolved`, text `left approval <id> to the agent (not run)` | `not_run`; false → `gone` |
| ask | `ASK_ACTOR` ask `tools::LeaveAsk { id }` (never `agent::answer_ask`, which cancels) | `Some(_)` → `left_to_agent`; `None` → `gone` |
| decision | `tasks::actor()` ask `tasks::LeaveDecision { decision: item.decision, actor }` | `live` → `left_to_agent`; else `left_for_later`; `Err("no task is blocked …")` → `gone`; other Err → `failed` |
| secret | `secrets::actor()` ask `secrets::Waive { id }` | true → `proceed_without`; false → `gone` |
| handoff | `crate::handoff::resolve(pool, id, handoff::PROCEED_WITHOUT, now)` | true → `proceed_without`; false → `gone` |
| every other kind and id (incl. `resume:*`, `contact_first:*`, `autoresearch:*`, `double:*`, `finished:*`, `show:*`, `node:*`, `misplaced:*`) | `set_state(pool, id, None, now)` | `hidden` |

Actor errors (`SendError`, lookup `None`) → `failed` with the message; nothing else is written for that id. `SnoozeMany` → `set_state(pool, id, Some(until_ms), now)` for every target id present → `snoozed`, absent → `gone`.

### 5.3 RPC (`crates/roster/src/rpc.rs`, trait `BoardsApi`, owner `roster`)

```rust
    /// 207 §5: dismiss by ids or by filter in one call — per kind, a dismissal leaves the request to the
    /// agent and never wakes a session; per-id outcomes (ids order; filter: engine order).
    #[method(name = "attention_dismiss_many", with_extensions)]
    async fn attention_dismiss_many(
        &self,
        target: roster::attention::AttentionTarget,
    ) -> RpcResult<Vec<roster::attention::AttentionOutcome>>;
    /// 207 §5: hide every target item until `until_ms` (ms since epoch); per-id outcomes.
    #[method(name = "attention_snooze_many")]
    async fn attention_snooze_many(
        &self,
        target: roster::attention::AttentionTarget,
        until_ms: u64,
    ) -> RpcResult<Vec<roster::attention::AttentionOutcome>>;
```
Impls: `send(&attention_actor()?, DismissMany { target, actor: rpc_core::Client::actor(ext) })`, `send(&attention_actor()?, SnoozeMany { target, until_ms })`. Existing methods keep their signatures: `attention_dismiss(id)` becomes `with_extensions` and sends `DismissMany { target: { ids: [id] }, actor }`, returning `Ok(())` unless the outcome is `failed` (→ `rpc_core::failed(error)`); `attention_snooze(id, until_ms)` sends `SnoozeMany`. Doc comment of `attention_dismiss` gains "207: per-kind semantics of `attention_dismiss_many`". No other RPC is added: Allow/Deny N = the client loops `resolve_approval` (owner `tasks`).

### 5.4 Events and registration

`AttentionChanged` is already registered (`roster::declare_wire`, `crates/roster/src/lib.rs:3796-3800`) and mirrored (`crates/rpc-compose/src/client_mirror.rs:95`); same for `tools::ApprovalResolved` / `tools::AskResolved` (`:68`, `:70`). Nothing new to register; steps:
1. Update the hand-written golden fixtures: `crates/rpc-compose/tests/fixtures/wire/roster.attention.json` (input and notification gain `items: [], order: [], seq: 0, epoch_ms: 0, alerts_changed: false` in serialization order — add a second entry with one item and `seq: 3`), `tools.approval-resolved.json` (`dismissed`), `tools.ask-resolved.json` (`left_to_agent`). Tests that read them: `crates/rpc-compose/tests/wire_golden.rs:96,98,155`, `wire_parity_inc3.rs:69-71`, `wire_parity_inc7.rs:55-67`, `wire_topics.rs`.
2. `crates/rpc-compose/src/api/tests.rs:675-697` `attention_changed_wire_shape`: the JSON literal gains the new fields.
3. `just wire-schema-accept` after reading the classifier report: only `Additive` / `Nonbreaking` lines may appear (new methods, new properties, new definitions). Anything `ReviewRequired`/`Breaking` means the contract was broken — stop and fix, never accept.
4. `just ts-desktop` regenerates the UI bindings (git-ignored).

### 5.5 Publishing (`recheck`)

The actor keeps `last: Vec<AttentionItem>` (as last published, alert included), `seq: u64`, `epoch_ms: u64` (set in `start`). After each derive (`items` with `alert` decorated): `added` = ids new; `removed` = ids gone; `changed` = items whose value with `score`, `reasons`, `alert` cleared differs from the previous; `order_changed` = id sequence differs; `alerts_changed` = any `alert` differs by id. Publish when any is non-empty / true: `seq += 1`; `AttentionChanged { count, ids_added, ids_removed, at_ms, items: added ∪ changed (each with alert: None, in engine order), order, seq, epoch_ms, alerts_changed }`; then `last = items`. The two empty-diff nudges (`BootPass` `:1939-1951`, `AlertDue` `:2035-2047`) go: the rule above covers them (a boot count changes the item's title; an alert appearing sets `alerts_changed`).

---

## 6 Alerts (A, `crates/roster/src/alert.rs`)

```rust
K::Review => own("review:") || own("autoresearch:"),
K::Blocked => own("blocked:") || own("autoresearch:"),
K::Home => own("node:") || own("misplaced:"),
```
`why_of`: `autoresearch:` ids → `a study waits for you`; `Finished` by `item.stop` outcome: Failed → `turn failed: <error or summary first line>` (`turn failed` when empty), Cancelled → `turn cancelled`, TimedOut → `turn timed out`, Finished/None → today's text. Doc comment of `class_of` loses "External Source items … never match a row". Class enum unchanged.

---

## 7 Generated TypeScript (exact; C hand-patches it, A's `just ts-desktop` regenerates it)

C first copies the generated tree from `/Users/sky/Develop/skymoore/theuth/crates/desktop/ui/src/rpc/bindings/` into this worktree's `crates/desktop/ui/src/rpc/bindings/` (git-ignored) and patches the files below. Field order inside interfaces is alphabetical, as the generator writes it. Output (`*.output.ts`) fields are all required; receiver/input fields with a serde default are optional.

### 7.1 `method.roster.output.ts` (and the same shapes in `method.roster.receiver.ts`, every field `?:`)

```ts
/**
 * 207 D3: one contributor to an item's score.
 * 
 * This interface was referenced by `Contract`'s JSON-Schema
 * via the `definition` "AttentionSignal".
 */
export type AttentionSignal = ("base" | "live" | "blocks" | "deadline" | "risk" | "task" | "age")
/**
 * 207 D5: how a session's turn stopped (`finished` items).
 * 
 * This interface was referenced by `Contract`'s JSON-Schema
 * via the `definition` "StopOutcome".
 */
export type StopOutcome = ("finished" | "failed" | "cancelled" | "timed_out")
/**
 * This interface was referenced by `Contract`'s JSON-Schema
 * via the `definition` "Risk".
 */
export type Risk = ("read_only" | "mutating" | "destructive")
/**
 * 207 §5.2: what a bulk call did to one id.
 * 
 * This interface was referenced by `Contract`'s JSON-Schema
 * via the `definition` "AttentionResolution".
 */
export type AttentionResolution = ("hidden" | "snoozed" | "left_to_agent" | "left_for_later" | "not_run" | "proceed_without" | "gone" | "failed")
/**
 * This interface was referenced by `Contract`'s JSON-Schema
 * via the `definition` "attention_dismiss_manyResult".
 */
export type AttentionDismissManyResult = AttentionOutcome[]
/**
 * This interface was referenced by `Contract`'s JSON-Schema
 * via the `definition` "attention_snooze_manyResult".
 */
export type AttentionSnoozeManyResult = AttentionOutcome[]
/**
 * 207 D3: one line of an item's score breakdown.
 * 
 * This interface was referenced by `Contract`'s JSON-Schema
 * via the `definition` "AttentionReason".
 */
export interface AttentionReason {
/**
 * Its points (> 0, except `base` which is always listed).
 */
points: number
/**
 * Which signal.
 */
signal: AttentionSignal
/**
 * Short text for the card ("blocks 3 agents", "denies itself in 1:42"); a snapshot at derive time.
 */
text: string
}
/**
 * 207 D5: the stop behind a `finished` item.
 * 
 * This interface was referenced by `Contract`'s JSON-Schema
 * via the `definition` "AttentionStop".
 */
export interface AttentionStop {
error: (string | null)
outcome: StopOutcome
reason: string
}
/**
 * 207 §5.3: one id's result of a bulk call.
 * 
 * This interface was referenced by `Contract`'s JSON-Schema
 * via the `definition` "AttentionOutcome".
 */
export interface AttentionOutcome {
error: (string | null)
id: string
resolution: AttentionResolution
}
```
`export interface AttentionItem` gains (merged alphabetically among the existing fields):
```ts
blocks: number
deadline_ms: (number | null)
expires_ms: (number | null)
parent_session: (string | null)
path: string[]
project: (string | null)
reasons: AttentionReason[]
risk: (Risk | null)
root_session: (string | null)
score: number
session_live: boolean
stop: (AttentionStop | null)
tool: (string | null)
```
`export interface Packet` gains `left_to_agent_ms: (number | null)`. `export interface Contract` gains `AttentionOutcome: AttentionOutcome`, `AttentionReason: AttentionReason`, `AttentionResolution: AttentionResolution`, `AttentionSignal: AttentionSignal`, `AttentionStop: AttentionStop`, `Risk: Risk`, `StopOutcome: StopOutcome`, `attention_dismiss_manyResult: AttentionDismissManyResult`, `attention_snooze_manyResult: AttentionSnoozeManyResult`.

### 7.2 `method.roster.input.ts`

```ts
/**
 * What kind of decision an item wants.
 * 
 * This interface was referenced by `Contract`'s JSON-Schema
 * via the `definition` "AttentionKind".
 */
export type AttentionKind = ("approval" | "decision" | "hire" | "blocked" | "review" | "budget" | "node" | "handoff" | "secret" | "ask" | "show" | "resume" | "double" | "finished" | "home" | "contact_first")
/**
 * @minItems 1
 * @maxItems 1
 * 
 * This interface was referenced by `Contract`'s JSON-Schema
 * via the `definition` "attention_dismiss_manyParams".
 */
export type AttentionDismissManyParams = [AttentionTarget]
/**
 * @minItems 2
 * @maxItems 2
 * 
 * This interface was referenced by `Contract`'s JSON-Schema
 * via the `definition` "attention_snooze_manyParams".
 */
export type AttentionSnoozeManyParams = [AttentionTarget, number]
/**
 * 207 §5.3: what a bulk call acts on — exactly one of `ids` (non-empty) or `filter`.
 * 
 * This interface was referenced by `Contract`'s JSON-Schema
 * via the `definition` "AttentionTarget".
 */
export interface AttentionTarget {
filter?: (AttentionFilter | null)
ids?: string[]
}
/**
 * 207 §5.3: a filter over the current list. Every part that is set must match.
 * 
 * This interface was referenced by `Contract`'s JSON-Schema
 * via the `definition` "AttentionFilter".
 */
export interface AttentionFilter {
before_ms?: (number | null)
kinds?: AttentionKind[]
projects?: string[]
}
```
(`AttentionKind` only if the file does not define it yet.) `Contract` gains `AttentionFilter`, `AttentionKind` (if new), `AttentionTarget`, `attention_dismiss_manyParams: AttentionDismissManyParams`, `attention_snooze_manyParams: AttentionSnoozeManyParams`.

### 7.3 `event.roster_2eattention.output.ts` (and `.receiver.ts`, new fields `?:`)

```ts
export interface AttentionChanged {
alerts_changed: boolean
at_ms: number
count: number
epoch_ms: number
ids_added: string[]
ids_removed: string[]
items: AttentionItem[]
order: string[]
seq: number
type: "AttentionChanged"
}
```
The generator inlines `AttentionItem` and its nested definitions into this file; a hand patch may instead `import type { AttentionItem } from './method.roster.output'` (structurally identical).

`event.tools_2eapproval_2dresolved.output.ts`: `ApprovalResolved` gains `dismissed: boolean`. `event.tools_2eask_2dresolved.output.ts`: `AskResolved` gains `left_to_agent: boolean`.

### 7.4 `client.ts`

```ts
// interface MethodMap (D2 = method.roster.input, D3 = method.roster.output — check the aliases at the top of the file)
  "attention_dismiss_many": { params: [D2.Contract["attention_dismiss_manyParams"][0]]; result: D3.Contract["attention_dismiss_manyResult"] }
  "attention_snooze_many": { params: [D2.Contract["attention_snooze_manyParams"][0], D2.Contract["attention_snooze_manyParams"][1]]; result: D3.Contract["attention_snooze_manyResult"] }
// METHODS: insert "attention_dismiss_many" after "attention_dismiss", "attention_snooze_many" after "attention_snooze"
// interface ReceiverResults (D178 = method.roster.receiver)
  "attention_dismiss_many": D178.Contract["attention_dismiss_manyResult"]
  "attention_snooze_many": D178.Contract["attention_snooze_manyResult"]
```
`receive.ts` is not patched: methods missing from its table pass through unchanged.

---

## 8 UI module plan (C)

### 8.1 Files

| File | Action |
|---|---|
| `components/attention/model.ts` | Rewrite the list half: delete `GROUP_ORDER`, `GROUP_OF`, `Group`, `groupItems`; keep `CONTEXT_MAX`, `clampText`, `lastAssistantBefore`, `CodeRef`, `parseCodeRef`, `nextAfter`, `PointerRoute`, `pointerRoute`, `urlHost`; add §8.2 |
| `components/attention/model.test.ts` | Drop the `groupItems` tests; add §11.2's |
| `components/attention/labels.ts` (new) | `KIND_LABEL`, `itemLabel`, `ATTENTION_EMPTY`, `doubleIds` moved from `hud/Hud.tsx`; `stopWord(stop)`; `KIND_LABEL.finished = 'stopped'` |
| `components/attention/actions.tsx` (new) | `KindActions`, `HomeActions`, `AllowOnce` moved from `hud/Hud.tsx:252-424`; Snooze becomes a menu (`SnoozeMenu`, presets `hour` · `tomorrow` · `week` from `hud/model.ts` `snoozeUntil`); Dismiss calls the bulk hook with one id |
| `components/attention/Card.tsx` (new) | `Card` (one item) and `StackCard` (an approval stack); replaces `Row.tsx` |
| `components/attention/Row.tsx` | Delete |
| `components/attention/Reply.tsx` (new) | The stopped card's next-step chips + input + Send → `send_input` |
| `components/attention/Toolbar.tsx` (new) | Search, sort select, kind chips, project select |
| `components/attention/BulkBar.tsx` (new) | Selection count, Dismiss N, Snooze N (menu), Allow N / Deny N (all approvals only), Clear |
| `components/attention/Why.tsx` (new) | "Why here": score + every reason with points; rendered by `Screen` above the kind's screen |
| `components/attention/Page.tsx` | Rewrite: toolbar, virtualised entries, selection, keyboard, bulk bar; rail `md:w-[440px]` |
| `components/attention/Screen.tsx` | `TITLE.finished = 'Session stopped'`; render `<Why item/>`; imports from `./labels` |
| `components/attention/PlainScreens.tsx` | `FinishedScreen`: outcome word, error, next steps, `<Reply/>`; `KindActions` from `./actions` |
| `components/attention/AskScreen.tsx` | Its Dismiss: when `attention_dismiss_many` is supported, `dismissMany([item.id])` (left to the agent) and the button reads "Leave to agent"; else today's `answer_ask(id, null)` |
| `components/attention/shared.tsx`, `source.tsx` | `itemLabel` from `./labels`; `Source` shows `pathText(item)` and the project |
| `components/attention/queries.ts` | Add `useAttentionBulk()` and `useReply()` (§8.3) |
| `components/attention/double.test.ts` | Drop the rail-group test; import `doubleIds` from `./labels` |
| `components/attention/Card.test.ts` (new) | §11.2 |
| `components/hud/Hud.tsx` | Remove `KIND_LABEL`, `itemLabel`, `ATTENTION_EMPTY`, `AttentionList`, `AttentionCard`, `EmptyNote`, `doubleIds`, `HomeActions`, `AllowOnce`, `KindActions` and `Section` if nothing imports it (grep), and their now-unused imports. No re-exports |
| `components/hud/Hud.test.ts` | Drop the `AttentionList` secret test (moved to `Card.test.ts`) |
| `components/hud/queries.ts` | `useAttention` patch mode (§8.4); `?mock=crowded` serves `attentionFixture(Date.now())`; `useAttentionActions().dismiss` uses `attention_dismiss_many` when supported |
| `components/settings/attention-fixtures.ts` (new) | §9 verbatim |
| `components/settings/sandbox-queries.ts` | Delete `ATTENTION_CROWDED`, `CROWD`, `Crowd`, `WEIGHT`, `CROWD_TASK`, `CROWD_PACKETS`; keep `crowdedFlag`, `ATTENTION_SIM`, `PACKET_SIM` |
| `components/settings/Contacts.test.ts` | `import { KindActions } from '@/components/attention/actions'` |
| `routes/attention.index.tsx` | The first visible entry under the current filter/sort (shared state, §8.5), else nothing |

### 8.2 `model.ts` — exported API (pure, no React, no fetch)

```ts
import type { AttentionItem, AttentionKind, AttentionOutcome } from '@/rpc/bindings/method.roster.output'
import type { AttentionTarget } from '@/rpc/bindings/method.roster.input'
import type { AttentionChanged } from '@/rpc/bindings/event.roster_2eattention.output'

export type SortKey = 'priority' | 'expiring' | 'oldest' | 'newest' | 'agent' | 'project'
/** Menu order and labels. */
export const SORTS: readonly { key: SortKey; label: string }[]
// [priority 'Priority', expiring 'Expiring soonest', oldest 'Oldest waiting', newest 'Newest', agent 'By agent', project 'By project']

export type ProjectFilter = { type: 'all' } | { type: 'none' } | { type: 'project'; name: string }
export type ListFilter = { kinds: ReadonlySet<string>; project: ProjectFilter; query: string }
export const EMPTY_FILTER: ListFilter  // kinds empty = every kind, project all, query ''
/** Chip order: stop-asks, mesh, yours, receipts; kinds outside it sort after, by name. */
export const KIND_ORDER: readonly AttentionKind[]
// ['approval','ask','decision','secret','handoff','resume','contact_first','node','home','review','hire','blocked','budget','show','finished','double']

/** Lowercased haystack: title, detail, agent, tool, task, project, path (joined by space), summary, itemLabel, session's first 8 chars. */
export function searchText(item: AttentionItem): string
/** Every whitespace-separated term of `query` (lowercased) occurs in `searchText`; '' matches all. */
export function matchesQuery(item: AttentionItem, query: string): boolean
/** Kind (empty set = any), project (`none` = project null) and query. */
export function matchesFilter(item: AttentionItem, f: ListFilter): boolean
/** Stable; ties keep the input (engine) order. priority: input order · expiring: min(deadline_ms, expires_ms) asc, items with neither last · oldest: (waiting_since_ms ?? at_ms) asc · newest: at_ms desc · agent: (agent ?? path.at(-1) ?? '') localeCompare, '' last · project: project localeCompare, null last. */
export function sortItems(items: readonly AttentionItem[], key: SortKey): AttentionItem[]

export type Entry =
  | { type: 'item'; key: string; item: AttentionItem }            // key = item.id
  | { type: 'stack'; key: string; items: AttentionItem[] }        // key = stackKey; items in input order, ≥ 2
/** `stack:<session>:<tool>` for an approval with both, else null. */
export function stackKey(item: AttentionItem): string | null
/** Approvals sharing a stackKey collapse into one entry at the first member's position; a lone member stays an item. */
export function stackEntries(items: readonly AttentionItem[]): Entry[]
/** dropExpired(now) → matchesFilter → sortItems → stackEntries. */
export function visibleEntries(items: readonly AttentionItem[], f: ListFilter, sort: SortKey, now: number): Entry[]
export function entryIds(e: Entry): string[]
/** The item a card shows: the item, or a stack's first member. */
export function entryHead(e: Entry): AttentionItem
/** Selection holds entry keys; expand to item ids in entry order. */
export function selectedIds(entries: readonly Entry[], selection: ReadonlySet<string>): string[]
/** True when the selection is non-empty and every selected item is an approval. */
export function allApprovals(entries: readonly Entry[], selection: ReadonlySet<string>): boolean
/** Kinds present with counts, in KIND_ORDER then unknown kinds by name. */
export function kindCounts(items: readonly AttentionItem[]): { kind: string; n: number }[]
/** Projects present with counts, by name; null ("No project") last when present. */
export function projectsOf(items: readonly AttentionItem[]): { name: string | null; n: number }[]

/** '1:42', '12:05', '1:02:03'; ≤ 0 → '0:00'. */
export function countdown(ms: number): string
/** null without deadline_ms; 'due now' when passed; else 'denies itself in …' (approval) · 'double answers in …' (double) · 'times out in …'. */
export function deadlineText(item: AttentionItem, now: number): string | null
/** The card's reason line: the first two present of, in this order, blocks · deadline (text recomputed by deadlineText) · risk · live · task (only when priority is urgent or high) · age; joined ' · '; '' when none. Base never shows. */
export function reasonLine(item: AttentionItem, now: number): string
/** 'release-2.5 · Ship 2.5 release › builder' — project (when set) then path joined ' › '; '' when both empty. */
export function pathText(item: AttentionItem): string
export function isExpired(item: AttentionItem, now: number): boolean   // expires_ms !== null && expires_ms <= now
/** The key after/before `current` (delta 1/-1), clamped; the first key when current is null or absent; null for []. */
export function moveFocus(keys: readonly string[], current: string | null, delta: 1 | -1): string | null
/** { ids } for a bulk call. */
export function bulkTarget(ids: readonly string[]): AttentionTarget
/** '12 dismissed · 3 left to agents · 2 not run · 1 already gone' — one part per resolution present, in AttentionResolution order, zero parts omitted: hidden 'N dismissed' · snoozed 'N snoozed' · left_to_agent 'N left to agents' · left_for_later 'N left for later' · not_run 'N not run' · proceed_without 'N told to go without' · gone 'N already gone' · failed 'N failed'. */
export function outcomeSummary(outcomes: readonly AttentionOutcome[]): string

export type Cursor = { epoch: number; seq: number }
/**
 * Apply one AttentionChanged to the cached list.
 * seq 0 (engine before 207) or no cache → { next: cache, refetch: true, cursor: seq ? {epoch_ms, seq} : cursor }.
 * Else: drop ids_removed; upsert ev.items keeping the cached item's alert (alert of a new id: null); next = ev.order mapped
 * through the map (ids not in order are dropped). refetch when: a gap (cursor set and (cursor.epoch !== ev.epoch_ms or
 * ev.seq !== cursor.seq + 1)), an id of order missing from the map, next.length !== ev.count, or ev.alerts_changed.
 * cursor = { epoch: ev.epoch_ms, seq: ev.seq }.
 */
export function applyChange(cache: readonly AttentionItem[] | undefined, ev: AttentionChanged, cursor: Cursor | null): { next: AttentionItem[] | undefined; refetch: boolean; cursor: Cursor | null }
```

### 8.3 Hooks (`components/attention/queries.ts`)

```ts
/** 207: one RPC for N items; toasts outcomeSummary; invalidates ATTENTION_KEY only when the engine has no AttentionChanged items (seq 0). */
export function useAttentionBulk(): {
  dismiss: (ids: readonly string[]) => Promise<AttentionOutcome[]>            // api.attention_dismiss_many(bulkTarget(ids))
  snooze: (ids: readonly string[], until: number) => Promise<AttentionOutcome[]> // api.attention_snooze_many(bulkTarget(ids), until)
  resolve: (approvals: readonly ApprovalId[], decision: 'allow' | 'deny') => Promise<boolean[]> // loops api.resolve_approval, ≤ 6 in flight
  pending: boolean
}
/** 207 R4: the stopped card's reply — api.send_input(session, text.trim(), [], null); toast 'Sent to <pathText>'. */
export function useReply(): { send: (session: string, text: string) => Promise<string>; pending: boolean }
```
Under `?sim=` / `?mock=crowded` both resolve locally (no RPC): dismiss/snooze remove the ids from the cached list and return `hidden`/`snoozed` outcomes; resolve returns `true`s; send returns the session.

### 8.4 `useAttention` (`hud/queries.ts`)

- `const patch = useQuerySupport(['attention_dismiss_many'])`; a `cursor` ref (`Cursor | null`).
- `onTopic('roster.attention@1', ev => …)`: `applyChange(qc.getQueryData(ATTENTION_KEY), ev, cursor)`; `setQueryData` when `next` is defined; invalidate `ATTENTION_KEY` when `refetch`; invalidate `[...PACKET_KEY, item.packet]` for each `ev.items` entry with a packet; invalidate `SECRET_REQUESTS_KEY` when any of `ids_added`/`ids_removed`/`items` ids starts with `secret:`.
- The ten legacy topics (`ATTENTION_TOPICS` minus `roster.attention@1`) keep invalidating only while `cursor` is null or `!patch` (an engine that never sent `seq > 0`).
- On a connection state change back to connected (`useConnection(s => s.state)` in `store/connection.ts`): `cursor = null`, invalidate `ATTENTION_KEY`.
- `?mock=crowded` → `attentionFixture(Date.now())`; `?sim=` unchanged (`ATTENTION_SIM`).

### 8.5 Page behaviour

- State in `Page` (lifted into a small context so `routes/attention.index.tsx` reads the same visible order): `filter: ListFilter`, `sort: SortKey` (persisted in `localStorage` key `attention.view` as `{sort, kinds, project}`; query not persisted), `selection: ReadonlySet<string>` (entry keys), `anchor: string | null`, `focus: string | null` (entry key), `now` (a 1 s interval while any visible item has `deadline_ms`, else 30 s).
- List: `useVirtualizer({ count, getScrollElement, estimateSize: () => 112, overscan: 8, measureElement })`, one row per entry; `aria-selected` on the focused card; the active route's item highlighted.
- Selection: click on the card's checkbox or `X` toggles `nextSelection(prev, keys, anchor, key, 'toggle')`; Shift-click or Shift+X → `'range'`; Esc → `'clear'` (after clearing the search first). Entries that leave the list leave the selection.
- Keyboard handler on the list container, ignored when the event target is an input, textarea, select or contenteditable: `j`/`k` → `moveFocus` (and scroll into view), Enter → open `/attention/$id` of the focused head, `a`/`d` → allow/deny the focused approval or every member of a focused stack, `s` → snooze focused (or selected) until `snoozeUntil('tomorrow')`, `e` → dismiss focused (or selected), `x`/Shift+`x` → select, `/` → focus the search input.
- Bulk bar visible when the selection is non-empty: "N selected", Dismiss, Snooze (menu), Allow N / Deny N when `allApprovals`, Clear. Results toast `outcomeSummary`.
- Empty states: no items → `ATTENTION_EMPTY`; items but none match → "Nothing matches" + "Clear filters".
- Card (`Card.tsx`): `<li>` with: row 1 kind chip (`itemLabel`; finished → `stopWord(item.stop)`), `×N` for stacks, `PriorityChip` (moved from `Row.tsx`), `DoubleChip`, the `done` chip for a finished alert, right-aligned `reasonLine(item, now)` in `text-fg-tertiary tabular-nums`; row 2 title (`line-clamp-2`); row 3 detail (approval: mono, one line); row 4 `pathText · agent · age(at_ms)`; row 5 inline actions: `KindActions` (size `xs`) except kinds that need the pane (decision, review, show; an ask with more than one question or a question without options) which get an "Open" link; a single-question ask with options renders one button per option (`answer_ask(id, [{kind:'choice', index}])` via `useAttentionActions().ask`); secret "Enter secret…" toggles the existing `SecretRequestCard` inline; finished → `<Reply/>`. Stack card: the head's rows + "Allow all N" / "Deny all N" + an expander listing members (each with its own Allow/Deny).

### 8.6 Labels

`KIND_LABEL` keeps every key of today's (`hud/Hud.tsx:163-180`) with `finished: 'stopped'`; `stopWord(stop)`: `finished` → 'finished', `failed` → 'failed', `cancelled` → 'cancelled', `timed_out` → 'timed out', null → 'finished'. `itemLabel` unchanged in behaviour (`misplaced:` → 'agent elsewhere').

---

## 9 Fixture — `crates/desktop/ui/src/components/settings/attention-fixtures.ts` (C creates it verbatim)

34 items, every kind, every 207 field, in engine order, scored with the default weights at `now` with `tasks.project_order = ["theuth", "release-2.5"]`. A's `score.rs` tests mirror the table in §9.2 (build the items with these fields, call `score`, compare; sort, compare order).

### 9.1 Source

```ts
// 207: the flat Attention list's fixture — 34 items in the engine's order (score desc, waiting_since asc, at_ms desc,
// id asc), every kind and every 207 field, scored with the default weights of `roster.attention.weights` at `now`.
// `?mock=crowded` serves `attentionFixture(Date.now())`; tests use `ATTENTION_FIXTURE` (a fixed instant). The scores
// and reasons are the engine's (207 contract §Scoring): crates/roster tests mirror this table.
import type { AttentionItem, AttentionReason, AttentionSignal } from '@/rpc/bindings/method.roster.output'

/** A fixed instant for tests: 2026-11-21 14:00:00 UTC. */
export const ATTENTION_FIXTURE_NOW = Date.UTC(2026, 10, 21, 14, 0, 0)

const R1 = '019a2070-0000-7000-8000-000000000001'
const BUILDER = '019a2070-0000-7000-8000-000000000011'
const LINTER = '019a2070-0000-7000-8000-000000000012'
const TESTER = '019a2070-0000-7000-8000-000000000013'
const OPS = '019a2070-0000-7000-8000-000000000002'
const RESEARCH = '019a2070-0000-7000-8000-000000000003'
const READER = '019a2070-0000-7000-8000-000000000031'
const TALK = '019a2070-0000-7000-8000-000000000004'
const LAB = '019a2070-0000-7000-8000-000000000005'
const WRITER = '019a2070-0000-7000-8000-000000000006'
const DIGEST = '019a2070-0000-7000-8000-000000000007'
const MIGRATE = '019a2070-0000-7000-8000-000000000008'
const SCRATCH = '019a2070-0000-7000-8000-000000000009'
const CHANGELOG = '019a2070-0000-7000-8000-000000000010'

/** Session id → what the engine resolves through the lineage (project on the root; path root first). */
const LINEAGE: Record<string, Pick<AttentionItem, 'project' | 'root_session' | 'parent_session' | 'path'>> = {
  [R1]: { project: "release-2.5", root_session: R1, parent_session: null, path: ["Ship 2.5 release"] },
  [BUILDER]: { project: "release-2.5", root_session: R1, parent_session: R1, path: ["Ship 2.5 release", "builder"] },
  [LINTER]: { project: "release-2.5", root_session: R1, parent_session: BUILDER, path: ["Ship 2.5 release", "builder", "linter"] },
  [TESTER]: { project: "release-2.5", root_session: R1, parent_session: R1, path: ["Ship 2.5 release", "tester"] },
  [OPS]: { project: "theuth", root_session: OPS, parent_session: null, path: ["ops desk"] },
  [RESEARCH]: { project: null, root_session: RESEARCH, parent_session: null, path: ["Competitor research"] },
  [READER]: { project: null, root_session: RESEARCH, parent_session: RESEARCH, path: ["Competitor research", "reader"] },
  [TALK]: { project: "theuth", root_session: TALK, parent_session: null, path: ["Talk view polish"] },
  [LAB]: { project: null, root_session: LAB, parent_session: null, path: ["Stripe export (lab)"] },
  [WRITER]: { project: "docs", root_session: WRITER, parent_session: null, path: ["writer desk"] },
  [DIGEST]: { project: "docs", root_session: DIGEST, parent_session: null, path: ["Weekly digest"] },
  [MIGRATE]: { project: "theuth", root_session: MIGRATE, parent_session: null, path: ["Migrate the tasks store"] },
  [SCRATCH]: { project: null, root_session: SCRATCH, parent_session: null, path: ["Scratch: regex help"] },
  [CHANGELOG]: { project: "release-2.5", root_session: CHANGELOG, parent_session: null, path: ["Changelog draft"] },
}

const at = (session: string): Pick<AttentionItem, 'session' | 'project' | 'root_session' | 'parent_session' | 'path'> => ({ session, ...LINEAGE[session]! })
const r = (signal: AttentionSignal, points: number, text: string): AttentionReason => ({ signal, points, text })

const BLANK: Omit<AttentionItem, 'id' | 'kind' | 'title' | 'at_ms' | 'score' | 'reasons'> = {
  detail: null, task: null, agent: null, session: null, approval: null, decision: null, packet: null, ask: null, turn: null,
  waiting_since_ms: null, summary: null, next_steps: [], context: null, priority: null, rank: 5, alert: null,
  risk: null, tool: null, deadline_ms: null, expires_ms: null, blocks: 0, session_live: false,
  project: null, root_session: null, parent_session: null, path: [], stop: null,
}

const item = (over: Partial<AttentionItem> & Pick<AttentionItem, 'id' | 'kind' | 'title' | 'at_ms' | 'score' | 'reasons'>): AttentionItem => ({ ...BLANK, ...over })

/** The list as `attention_list` returns it at `now`. */
export function attentionFixture(now: number): AttentionItem[] {
  return [
    item({ id: "approval:019a2072-0000-7000-8000-000000000001", kind: 'approval', title: "builder wants to run shell", detail: "kubectl delete deployment relay-canary -n prod", at_ms: now - 180_000, waiting_since_ms: now - 180_000, approval: "019a2072-0000-7000-8000-000000000001", priority: "high", risk: "destructive", tool: "shell", ...at(BUILDER), rank: 107, context: { request: "Ship 2.5: roll the canary back if the error rate stays above 2 %.", last_reply: "Error rate is 3.1 % for ten minutes; rolling the canary back." }, deadline_ms: now + 120_000, blocks: 2, session_live: true, score: 1213, reasons: [r('base', 600, "approval"), r('live', 300, "agent waiting on you"), r('risk', 100, "destructive"), r('blocks', 80, "blocks 2 agents"), r('task', 80, "high priority · ranked project"), r('deadline', 50, "denies itself in 2:00"), r('age', 3, "waiting 3 min")] }),
    item({ id: "ask:019a2073-0000-7000-8000-000000000002", kind: 'ask', title: "tester asks: Run the slow integration suite too?", detail: "Yes · No · Only the relay tests", at_ms: now - 1_500_000, waiting_since_ms: now - 1_500_000, priority: "high", ...at(TESTER), rank: 107, ask: { type: 'AskRequested', id: "019a2073-0000-7000-8000-000000000002", session: TESTER, questions: [{"id": "slow", "question": "Run the slow integration suite too? It adds forty minutes.", "options": [{"label": "Yes"}, {"label": "No"}, {"label": "Only the relay tests"}], "recommended": 2}] }, deadline_ms: now + 300_000, blocks: 1, session_live: true, score: 1212, reasons: [r('base', 600, "question"), r('live', 300, "agent waiting on you"), r('deadline', 167, "times out in 5:00"), r('task', 80, "high priority · ranked project"), r('blocks', 40, "blocks 1 agent"), r('age', 25, "waiting 25 min")] }),
    item({ id: "decision:019a2071-0000-7000-8000-000000000001:decision:019a2074-0000-7000-8000-000000000001", kind: 'decision', title: "Roll back 2.4.1 now, or ship the hotfix in forty minutes?", detail: "ops · Ship 2.4.1", at_ms: now - 60_000, waiting_since_ms: now - 60_000, task: "019a2071-0000-7000-8000-000000000001", agent: "ops", decision: "decision:019a2074-0000-7000-8000-000000000001", packet: "decision:019a2074-0000-7000-8000-000000000001", priority: "urgent", ...at(OPS), rank: 209, blocks: 3, session_live: true, score: 1121, reasons: [r('base', 600, "decision"), r('live', 300, "agent waiting on you"), r('blocks', 120, "blocks 3 agents"), r('task', 100, "urgent priority · ranked project"), r('age', 1, "waiting 1 min")] }),
    item({ id: "approval:019a2072-0000-7000-8000-000000000004", kind: 'approval', title: "ops wants to run fetch", detail: "GET https://api.crunchbase.com/v4/entities/organizations/initech", at_ms: now - 240_000, waiting_since_ms: now - 240_000, agent: "ops", approval: "019a2072-0000-7000-8000-000000000004", risk: "mutating", tool: "fetch", ...at(OPS), deadline_ms: now + 60_000, session_live: true, score: 1084, reasons: [r('base', 600, "approval"), r('live', 300, "agent waiting on you"), r('deadline', 150, "denies itself in 1:00"), r('risk', 30, "mutating"), r('age', 4, "waiting 4 min")] }),
    item({ id: "approval:019a2072-0000-7000-8000-000000000003", kind: 'approval', title: "ops wants to run fetch", detail: "GET https://api.crunchbase.com/v4/entities/organizations/globex", at_ms: now - 120_000, waiting_since_ms: now - 120_000, agent: "ops", approval: "019a2072-0000-7000-8000-000000000003", risk: "mutating", tool: "fetch", ...at(OPS), deadline_ms: now + 180_000, session_live: true, score: 932, reasons: [r('base', 600, "approval"), r('live', 300, "agent waiting on you"), r('risk', 30, "mutating"), r('age', 2, "waiting 2 min")] }),
    item({ id: "approval:019a2072-0000-7000-8000-000000000002", kind: 'approval', title: "ops wants to run fetch", detail: "GET https://api.crunchbase.com/v4/entities/organizations/acme", at_ms: now - 60_000, waiting_since_ms: now - 60_000, agent: "ops", approval: "019a2072-0000-7000-8000-000000000002", risk: "mutating", tool: "fetch", ...at(OPS), deadline_ms: now + 240_000, session_live: true, score: 931, reasons: [r('base', 600, "approval"), r('live', 300, "agent waiting on you"), r('risk', 30, "mutating"), r('age', 1, "waiting 1 min")] }),
    item({ id: "approval:019a2072-0000-7000-8000-000000000005", kind: 'approval', title: "reader wants to run browser", detail: "log in to stripe.com as sky@theuth.dev", at_ms: now - 30_000, waiting_since_ms: now - 30_000, approval: "019a2072-0000-7000-8000-000000000005", risk: "mutating", tool: "browser", ...at(READER), deadline_ms: now + 270_000, session_live: true, score: 930, reasons: [r('base', 600, "approval"), r('live', 300, "agent waiting on you"), r('risk', 30, "mutating")] }),
    item({ id: "ask:019a2073-0000-7000-8000-000000000001", kind: 'ask', title: "Competitor research asks: Which three competitors should the table cover?", detail: "2 questions", at_ms: now - 600_000, waiting_since_ms: now - 600_000, ...at(RESEARCH), ask: { type: 'AskRequested', id: "019a2073-0000-7000-8000-000000000001", session: RESEARCH, questions: [{"id": "which", "question": "Which three competitors should the table cover?", "options": [{"label": "Acme, Globex, Initech", "description": "The three in last quarter’s deck."}, {"label": "The top three by revenue", "description": "Needs the Crunchbase pull first."}], "recommended": 0}, {"id": "depth", "question": "How deep per competitor: one row or one page?", "options": []}] }, deadline_ms: now + 1_200_000, session_live: true, score: 910, reasons: [r('base', 600, "question"), r('live', 300, "agent waiting on you"), r('age', 10, "waiting 10 min")] }),
    item({ id: "handoff:019a2076-0000-7000-8000-000000000001", kind: 'handoff', title: "Stripe export (lab) needs you: Stripe wants a one-time code from your phone", detail: "lab", at_ms: now - 360_000, waiting_since_ms: now - 360_000, ...at(LAB), deadline_ms: now + 1_440_000, session_live: true, score: 906, reasons: [r('base', 600, "hand-off"), r('live', 300, "agent waiting on you"), r('age', 6, "waiting 6 min")] }),
    item({ id: "secret:019a2075-0000-7000-8000-000000000001", kind: 'secret', title: "Stripe export (lab) asks for a secret: STRIPE_API_KEY", detail: "lab", at_ms: now - 120_000, waiting_since_ms: now - 120_000, ...at(LAB), deadline_ms: now + 1_680_000, session_live: true, score: 902, reasons: [r('base', 600, "secret request"), r('live', 300, "agent waiting on you"), r('age', 2, "waiting 2 min")] }),
    item({ id: "secret:019a2075-0000-7000-8000-000000000002", kind: 'secret', title: "writer asks for a secret: Postgres login", detail: "this Mac", at_ms: now - 1_680_000, waiting_since_ms: now - 1_680_000, agent: "writer", ...at(WRITER), deadline_ms: now + 120_000, score: 845, reasons: [r('base', 600, "secret request"), r('deadline', 217, "times out in 2:00"), r('age', 28, "waiting 28 min")] }),
    item({ id: "decision:019a2071-0000-7000-8000-000000000002:decision:019a2074-0000-7000-8000-000000000002", kind: 'decision', title: "Keep the pricing change in Highlights, or move it under Changes?", detail: "writer · Release notes 2.5", at_ms: now - 2_700_000, waiting_since_ms: now - 2_700_000, task: "019a2071-0000-7000-8000-000000000002", agent: "writer", decision: "decision:019a2074-0000-7000-8000-000000000002", packet: "decision:019a2074-0000-7000-8000-000000000002", priority: "medium", ...at(WRITER), score: 695, reasons: [r('base', 600, "decision"), r('task', 50, "medium priority"), r('age', 45, "waiting 45 min")] }),
    item({ id: `resume:${TALK}:019a2077-0000-7000-8000-000000000001`, kind: 'resume', title: "Talk view polish was interrupted by a restart — resume?", at_ms: now - 1_200_000, waiting_since_ms: now - 1_200_000, ...at(TALK), score: 620, reasons: [r('base', 600, "interrupted turn"), r('age', 20, "waiting 20 min")] }),
    item({ id: "autoresearch:launch:blurb-tone:3", kind: 'blocked', title: "Study blurb-tone: launch #3 needs your press · exposure 10 %", at_ms: now - 720_000, waiting_since_ms: now - 720_000, ...at(TALK), score: 612, reasons: [r('base', 600, "launch waits for your press"), r('age', 12, "waiting 12 min")] }),
    item({ id: "contact_first:maria:calendar", kind: 'contact_first', title: "maria (maria-mbp) wants to reach calendar — Allow once · Always · Block", detail: "1 waiting · a contact outside your mesh", at_ms: now - 420_000, waiting_since_ms: now - 420_000, score: 607, reasons: [r('base', 600, "first contact"), r('age', 7, "waiting 7 min")] }),
    item({ id: "node:engine-studio-0002", kind: 'node', title: "studio-mac is unreachable", detail: "last seen 2026-11-21 11:02 UTC", at_ms: now - 10_800_000, waiting_since_ms: now - 10_800_000, score: 510, reasons: [r('base', 450, "engine unreachable"), r('age', 60, "waiting 3 h")] }),
    item({ id: "misplaced:019a2071-0000-7000-8000-000000000003", kind: 'home', title: "analyst isn't on this Mac; it's on lab", detail: "Move it there with Move to ▸", at_ms: now - 300_000, waiting_since_ms: now - 300_000, task: "019a2071-0000-7000-8000-000000000003", agent: "analyst", priority: "medium", score: 505, reasons: [r('base', 450, "agent elsewhere"), r('task', 50, "medium priority"), r('age', 5, "waiting 5 min")] }),
    item({ id: "show:toolu_01AZQL17aTdoAMD49A3uZkDa", kind: 'show', title: "Ship 2.5 release showed you Canary error rate", detail: "image · 44.0 KB · v2", at_ms: now - 240_000, waiting_since_ms: now - 240_000, priority: "high", ...at(R1), rank: 107, session_live: true, score: 504, reasons: [r('live', 300, "session running"), r('base', 120, "shown to you"), r('task', 80, "high priority · ranked project"), r('age', 4, "waiting 4 min")] }),
    item({ id: "node:engine-lab-0001", kind: 'home', title: "lab is offline — 2 tasks are waiting", detail: "Waiting: Nightly export, Index rebuild. Nothing runs elsewhere while it is off. Move them here, or keep waiting.", at_ms: now - 1_800_000, waiting_since_ms: now - 1_800_000, score: 480, reasons: [r('base', 450, "machine offline"), r('age', 30, "waiting 30 min")] }),
    item({ id: "contact_roster:rec-7f3a", kind: 'node', title: "joan’s roster lists a machine you never paired", detail: "remove that machine from your mesh, then ask joan to remove you and pair again", at_ms: now - 120_000, waiting_since_ms: now - 120_000, score: 452, reasons: [r('base', 450, "mesh alert"), r('age', 2, "waiting 2 min")] }),
    item({ id: "blocked:019a2071-0000-7000-8000-000000000005", kind: 'blocked', title: "“Vendor DPA review” is blocked", detail: "on person:legal", at_ms: now - 5_400_000, waiting_since_ms: now - 5_400_000, task: "019a2071-0000-7000-8000-000000000005", agent: "analyst", priority: "medium", score: 410, reasons: [r('base', 300, "blocked task"), r('age', 60, "waiting 1 h"), r('task', 50, "medium priority")] }),
    item({ id: "review:019a2071-0000-7000-8000-000000000004", kind: 'review', title: "The relay now retries with jitter; the flaky test passed 200 of 200 runs.", detail: "“Fix the relay reconnect storm” · by builder", at_ms: now - 1_800_000, waiting_since_ms: now - 1_800_000, task: "019a2071-0000-7000-8000-000000000004", agent: "builder", packet: "review:019a2071-0000-7000-8000-000000000004", priority: "high", ...at(MIGRATE), rank: 207, score: 410, reasons: [r('base', 300, "review"), r('task', 80, "high priority · ranked project"), r('age', 30, "waiting 30 min")] }),
    item({ id: "hire:summarizer", kind: 'hire', title: "research proposes hiring summarizer (Summarizer)", detail: "reports to theuth · budget 20 USD/month", at_ms: now - 7_200_000, waiting_since_ms: now - 7_200_000, agent: "summarizer", score: 360, reasons: [r('base', 300, "hire"), r('age', 60, "waiting 2 h")] }),
    item({ id: "budget:research", kind: 'budget', title: "research spent 61.20 of 60.00 USD (102 %)", detail: "paused at the cap; un-pause to override for the month", at_ms: now - 900_000, waiting_since_ms: now - 900_000, agent: "research", score: 315, reasons: [r('base', 300, "budget paused"), r('age', 15, "waiting 15 min")] }),
    item({ id: "autoresearch:paused:blurb-tone:budget", kind: 'blocked', title: "Study blurb-tone paused: budget reached", at_ms: now - 480_000, waiting_since_ms: now - 480_000, ...at(TALK), score: 308, reasons: [r('base', 300, "blocked task"), r('age', 8, "waiting 8 min")] }),
    item({ id: `finished:${MIGRATE}:019a2078-0000-7000-8000-000000000002`, kind: 'finished', title: "Migrate the tasks store failed", detail: "migration 0010 failed: UNIQUE constraint failed: wakes.key", at_ms: now - 300_000, summary: "migration 0010 failed: UNIQUE constraint failed: wakes.key", priority: "urgent", ...at(MIGRATE), rank: 209, expires_ms: now + 2_400_000, stop: { outcome: 'failed', reason: "error", error: "migration 0010 failed: UNIQUE constraint failed: wakes.key" }, score: 255, reasons: [r('base', 150, "failed"), r('task', 100, "urgent priority · ranked project"), r('age', 5, "stopped 5 min ago")] }),
    item({ id: `finished:${CHANGELOG}:019a2078-0000-7000-8000-000000000004`, kind: 'finished', title: "Changelog draft timed out", detail: "peer work deadline passed after 40 min", at_ms: now - 2_400_000, summary: "Drafted Highlights and Changes; Fixes not started.", ...at(CHANGELOG), next_steps: ["Finish the Fixes section"], expires_ms: now + 300_000, stop: { outcome: 'timed_out', reason: "timed_out", error: "peer work deadline passed after 40 min" }, score: 190, reasons: [r('base', 150, "timed out"), r('age', 40, "stopped 40 min ago")] }),
    item({ id: "budget:analyst", kind: 'budget', title: "analyst spent 48.00 of 60.00 USD (80 %)", detail: "at 80 %: the cap pauses it", at_ms: now - 12_000_000, waiting_since_ms: now - 12_000_000, agent: "analyst", score: 180, reasons: [r('base', 120, "budget at 80 %"), r('age', 60, "waiting 3 h")] }),
    item({ id: "show:toolu_01AZQL17aTdoAMD49A3uZkDb", kind: 'show', title: "Talk view polish showed you Talk view, before and after", detail: "image · 210.0 KB · v1", at_ms: now - 2_400_000, waiting_since_ms: now - 2_400_000, ...at(TALK), score: 160, reasons: [r('base', 120, "shown to you"), r('age', 40, "waiting 40 min")] }),
    item({ id: `resume:boot:${now - 180_000}`, kind: 'resume', title: "Restarted 13:57 · 3 sessions resumed · 2 subagents back", at_ms: now - 180_000, waiting_since_ms: now - 180_000, score: 123, reasons: [r('base', 120, "restart notice"), r('age', 3, "waiting 3 min")] }),
    item({ id: `finished:${SCRATCH}:019a2078-0000-7000-8000-000000000003`, kind: 'finished', title: "Scratch: regex help was cancelled", at_ms: now - 1_200_000, ...at(SCRATCH), expires_ms: now + 1_500_000, stop: { outcome: 'cancelled', reason: "cancelled", error: null }, score: 100, reasons: [r('base', 80, "cancelled"), r('age', 20, "stopped 20 min ago")] }),
    item({ id: `finished:${DIGEST}:019a2078-0000-7000-8000-000000000001`, kind: 'finished', title: "Weekly digest finished", detail: "Digest sent to 14 readers; two links were dead and are dropped.", at_ms: now - 600_000, summary: "Digest sent to 14 readers; two links were dead and are dropped.", ...at(DIGEST), next_steps: ["Replace the two dead links", "Move the send to Monday 08:00"], alert: { id: `finished:${DIGEST}:019a2078-0000-7000-8000-000000000001@${now - 600_000}`, class: 'finished', why: "turn ended: Digest sent to 14 readers; two links were dead and are dropped.", at_ms: now - 600_000 }, expires_ms: now + 2_100_000, stop: { outcome: 'finished', reason: "end_turn", error: null }, score: 90, reasons: [r('base', 80, "finished"), r('age', 10, "stopped 10 min ago")] }),
    item({ id: "double:019a2079-0000-7000-8000-000000000002", kind: 'double', title: "double answered Approve for “Bundle the docs deploy”", at_ms: now - 3_000_000, ...at(READER), score: 70, reasons: [r('age', 50, "50 min ago"), r('base', 20, "answered by your double")] }),
    item({ id: "double:019a2079-0000-7000-8000-000000000001", kind: 'double', title: "double answers Allow for `git push origin feat/relay-jitter` in 14 s — take over", detail: new Date(now + 14_000).toISOString(), at_ms: now - 16_000, ...at(OPS), deadline_ms: now + 14_000, session_live: true, score: 37, reasons: [r('base', 20, "answered by your double"), r('deadline', 17, "double answers in 0:14")] }),
  ]
}

export const ATTENTION_FIXTURE: AttentionItem[] = attentionFixture(ATTENTION_FIXTURE_NOW)
```

### 9.2 Expected scores and order

| # | id | kind | score | reasons (signal points) |
|---|---|---|---|---|
| 1 | `approval:019a2072-0000-7000-8000-000000000001` | approval | 1213 | base 600 · live 300 · risk 100 · blocks 80 · task 80 · deadline 50 · age 3 |
| 2 | `ask:019a2073-0000-7000-8000-000000000002` | ask | 1212 | base 600 · live 300 · deadline 167 · task 80 · blocks 40 · age 25 |
| 3 | `decision:019a2071-0000-7000-8000-000000000001:decision:01…` | decision | 1121 | base 600 · live 300 · blocks 120 · task 100 · age 1 |
| 4 | `approval:019a2072-0000-7000-8000-000000000004` | approval | 1084 | base 600 · live 300 · deadline 150 · risk 30 · age 4 |
| 5 | `approval:019a2072-0000-7000-8000-000000000003` | approval | 932 | base 600 · live 300 · risk 30 · age 2 |
| 6 | `approval:019a2072-0000-7000-8000-000000000002` | approval | 931 | base 600 · live 300 · risk 30 · age 1 |
| 7 | `approval:019a2072-0000-7000-8000-000000000005` | approval | 930 | base 600 · live 300 · risk 30 |
| 8 | `ask:019a2073-0000-7000-8000-000000000001` | ask | 910 | base 600 · live 300 · age 10 |
| 9 | `handoff:019a2076-0000-7000-8000-000000000001` | handoff | 906 | base 600 · live 300 · age 6 |
| 10 | `secret:019a2075-0000-7000-8000-000000000001` | secret | 902 | base 600 · live 300 · age 2 |
| 11 | `secret:019a2075-0000-7000-8000-000000000002` | secret | 845 | base 600 · deadline 217 · age 28 |
| 12 | `decision:019a2071-0000-7000-8000-000000000002:decision:01…` | decision | 695 | base 600 · task 50 · age 45 |
| 13 | `resume:019a2070-0000-7000-8000-000000000004:019a2077-0000…` | resume | 620 | base 600 · age 20 |
| 14 | `autoresearch:launch:blurb-tone:3` | blocked | 612 | base 600 · age 12 |
| 15 | `contact_first:maria:calendar` | contact_first | 607 | base 600 · age 7 |
| 16 | `node:engine-studio-0002` | node | 510 | base 450 · age 60 |
| 17 | `misplaced:019a2071-0000-7000-8000-000000000003` | home | 505 | base 450 · task 50 · age 5 |
| 18 | `show:toolu_01AZQL17aTdoAMD49A3uZkDa` | show | 504 | live 300 · base 120 · task 80 · age 4 |
| 19 | `node:engine-lab-0001` | home | 480 | base 450 · age 30 |
| 20 | `contact_roster:rec-7f3a` | node | 452 | base 450 · age 2 |
| 21 | `blocked:019a2071-0000-7000-8000-000000000005` | blocked | 410 | base 300 · age 60 · task 50 |
| 22 | `review:019a2071-0000-7000-8000-000000000004` | review | 410 | base 300 · task 80 · age 30 |
| 23 | `hire:summarizer` | hire | 360 | base 300 · age 60 |
| 24 | `budget:research` | budget | 315 | base 300 · age 15 |
| 25 | `autoresearch:paused:blurb-tone:budget` | blocked | 308 | base 300 · age 8 |
| 26 | `finished:019a2070-0000-7000-8000-000000000008:019a2078-00…` | finished | 255 | base 150 · task 100 · age 5 |
| 27 | `finished:019a2070-0000-7000-8000-000000000010:019a2078-00…` | finished | 190 | base 150 · age 40 |
| 28 | `budget:analyst` | budget | 180 | base 120 · age 60 |
| 29 | `show:toolu_01AZQL17aTdoAMD49A3uZkDb` | show | 160 | base 120 · age 40 |
| 30 | `resume:boot:BOOTMS` | resume | 123 | base 120 · age 3 |
| 31 | `finished:019a2070-0000-7000-8000-000000000009:019a2078-00…` | finished | 100 | base 80 · age 20 |
| 32 | `finished:019a2070-0000-7000-8000-000000000007:019a2078-00…` | finished | 90 | base 80 · age 10 |
| 33 | `double:019a2079-0000-7000-8000-000000000002` | double | 70 | age 50 · base 20 |
| 34 | `double:019a2079-0000-7000-8000-000000000001` | double | 37 | base 20 · deadline 17 |

Ties: #21–#22 (410) break on `waiting_since_ms` (blocked 90 min before review 30 min). Doubles have `session_live` true/false but never a `live` reason (D3). `finished` items have no `waiting_since_ms`; their age text is "stopped N min ago".

---

## 10 Kind-list duplication

**Update this round:**
- Rust: `alert.rs` `class_of` / `why_of` (§6); `score.rs` `base_class` + base text (new, §3.2); `attention.rs` `context_kind` (unchanged list, re-read); `derive`'s `Blocked` branch (`left:`).
- UI: `components/attention/labels.ts` `KIND_LABEL` (moved from `hud/Hud.tsx:163-180`); `model.ts` `KIND_ORDER` (new) replacing `GROUP_ORDER`/`GROUP_OF` (`model.ts:9-46`, deleted); `Screen.tsx` `SCREEN` and `TITLE` (`:15-38`); `sandbox-queries.ts` `CROWD` (deleted).

**Leave alone (round 3):**
- Swift: `crates/desktop/companion/Sources/TheuthKit/Models.swift:9-43` (`AttentionKind`, `known`, `needsYou`), `Sources/CompanionCore/CardAnatomy.swift:103` (`stoppableKinds`), `RailModel.swift:6`, `Sources/Companion/RailPanel.swift:295`, `Policy.swift`, their fixtures under `Tests/TheuthKitTests/Fixtures`.
- C#: `crates/desktop/companion-windows/src/Theuth.Companion.Rpc/{Models.cs, Models.149.cs}`, `Theuth.Companion.Core/{AgentsModel.cs, AskRouter.cs, CardAnatomy.cs, CardModel.cs, KeyGrammar.cs, PanelModel.cs, Policy.cs, RailModel.cs}`.

---

## 11 Acceptance

### 11.1 A — engine

Behaviour:
1. `attention_list` returns every item with `score`, `reasons`, `project`, `root_session`, `parent_session`, `path`, `session_live`, `blocks`; approvals with `risk`, `tool`, `deadline_ms`; asks/secrets/hand-offs with `deadline_ms`; finished with `stop`, `expires_ms`; in §3.4 order.
2. Every stop outcome of a root/desk turn makes one `finished:` row per session; `interrupted`/`suspended` make none; the next `TurnStarted` deletes it; the tick deletes it after `stopped_ttl_mins`.
3. Dismissal per §5.2, each verified on a live waiter: approval returns the not-run text and publishes `dismissed: true`; ask returns ok text, publishes `left_to_agent: true`, the turn is **not** cancelled; live decision → `escalate` returns `decision: <LEFT_ANSWER>`, blocker gone, no `wakes` row; parked decision → `left:decision:<id>` blocker, task still `blocked`, no `wakes` row, packet `left_to_agent_ms` set, thread line written, no item; secret → `proceed_without` + ok text; hand-off → `proceed_without` + ok text; other kinds hidden (119).
4. Bulk: ids order; filter (kinds, `""` project, `before_ms`); both/neither refused; one `AttentionChanged` per batch; `gone`/`failed` outcomes.
5. `AttentionChanged` carries `items`/`order`/`seq`/`epoch_ms`/`alerts_changed` per §5.5; the tick publishes nothing when nothing changed.
6. Alerts: `node:`/`misplaced:` home items and `autoresearch:*` review/blocked items alert; finished `why` per outcome.
7. The double takes no label from a dismissed approval or a left ask; double items have a stable `at_ms` and `deadline_ms` in grace.

Tests to write (deterministic, in the owning crates): `score.rs` — each signal, caps (blocks 6 → 200, age 90 → 60, task urgent ranked → 100), deadline rounding at share 0.5 / 0.4 / 0.0 / passed / zero span, D2 overrides, D3 (double, finished), D4 (no priority → no task reason), the §9.2 table and order; `attention.rs` — `lineage_map` (root, desk root, child, grandchild, unknown parent), `blocks_map` (parent chain, child-name, sibling-name, `session:` id, cycle, unresolved), `live_set`, ask `at_ms` stable across derives, `left:` blocker makes no item, finished rows per stop reason, `TurnStarted` deletion, TTL prune, `changed`-detection (content vs score-only vs order vs alert), target validation and filter matching; `tools` — approval `Dismiss` result text and event, ask `LeaveAsk` ok text, no cancel, journal answers, replay of `"dismissed"`; `tasks` — `leave_decision` live and parked (no `wakes` row in either), `wake_text` line; `secrets` — `Waive` outcome and tool text; `roster::handoff` — `proceed_without` result; `double` — no label; wire fixtures + `attention_changed_wire_shape`.

Gates (quoted from the justfile, run once at the end, never while `/tmp/theuth-build.lock` is held by another session):
```
just fmt                      # cargo fmt --all
just clippy                   # cargo clippy --workspace --all-targets -- -D warnings
just test                     # cargo test --workspace
just wire-schema              # cargo test -p theuth --bin t --quiet export_bindings
just wire-schema-accept       # only after the classifier shows Additive/Nonbreaking only
just ts-desktop               # regenerate UI bindings (git-ignored), then hand them to integration
```
Narrower loop while working: `cargo test -p roster -p tools -p tasks -p secrets -p double -p rpc-compose`.

### 11.2 C — UI

Behaviour:
1. `/attention` shows one flat virtualised list in engine order from `?mock=crowded` (34 cards) with toolbar, chips with counts, project select, and the empty/no-match states.
2. Sorts reorder as §8.2; Priority is the engine order; stacks show "×3" for the three `fetch` approvals of `ops desk` at the position of the highest one (#4).
3. Search matches title, agent, tool, task, project, path.
4. Selection: toggle, shift-range across entries, stack selects all members; bulk bar with Allow/Deny only for all-approval selections; outcomes toast.
5. Keyboard J/K/Enter/A/D/S/E/X/Shift+X/`/`/Esc as §8.5, inert while typing.
6. Cards answer inline per §8.5; finished cards show outcome, summary, next-step chips and a reply box that calls `send_input(session, text, [], null)`.
7. Reason line from `reasonLine`; countdowns tick; expired finished cards disappear without an event.
8. Live updates patch the cache (no refetch per event) per §8.4.
9. No emoji; lucide icons; shadcn primitives.

Tests to write (vitest, node env, `renderToStaticMarkup` for components): `model.test.ts` — `matchesQuery` multi-term, `matchesFilter` (kinds any-of, `none` project), every `sortItems` key with ties on the fixture, `stackEntries` (three ops approvals → one stack at index 3 of the fixture order, a lone approval stays), `selectedIds`/`allApprovals`, `kindCounts` (16 kinds present), `projectsOf` (null last), `countdown`, `deadlineText`, `reasonLine` (#1 → 'blocks 2 agents · denies itself in 2:00' at the fixture instant), `pathText`, `isExpired`, `moveFocus`, `outcomeSummary`, `applyChange` (upsert keeps alert, removal, reorder, each refetch trigger: seq 0, gap, epoch change, missing id, count mismatch, alerts_changed); `Card.test.ts` — an approval card (Allow/Deny, mono command, reason line), a stack card ("Allow all 3"), a finished-failed card (outcome word, error, reply input), a secret card rendering the `SecretRequestCard` row (moved from `Hud.test.ts`), a single-question ask with option buttons; keep `double.test.ts` and `Contacts.test.ts` passing.

Gates (quoted from the justfile, run once at the end):
```
just desktop-check            # cd crates/desktop/ui && bun run check && bun run test && bun run build
```
Narrower loop: `cd crates/desktop/ui && bunx vitest run src/components/attention src/components/hud src/lib/no-emoji.test.ts`.

---

## 12 Integration notes (orchestrator)

- Merge C then A; run `just ts-desktop` on the merged tree; C's hand-patched bindings are discarded; any name mismatch between §7 and the generated files is A's to fix (this contract is the source).
- Old clients: the desktop UI before 207 still cancels a turn with its ask Dismiss (`answer_ask(null)`); companions show failed stops as finished. Both are recorded in the design note §10.
