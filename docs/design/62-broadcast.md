# 62 — Broadcast: one message to many sessions, and a peer directory that shows who is alive

Status: design, 2026-09-22. Extends 50 (§5.1 directory disclosure, §5.2 one
send tool, §6.3 turn scopes, §8 bounds, §10 owner API), 19 (`/agents` roster,
`AgentInfo`/`AgentState`), 47 §4.5 (input origin), 48 (the `/shake`
Pick→Review picker), 25 (desktop feature table), 51 §4 (retention/private),
61 (job idle wakes). Amends 50 §5.1 and §5.2 explicitly (see Amendments).

Scope: three things that share one need — knowing which sessions are alive
and putting one message in front of several of them:

1. **Human broadcast.** The owner sends one message to every running root
   session, or to a set they pick, from the TUI, desktop and mobile (and
   owner-only from Slack/Signal), choosing per broadcast whether it lands as
   a steer (inside the running turn) or as the next turn.
2. **Activity, visible.** A per-session activity record (turn state, last
   active, running jobs/PTYs, live children) that the human roster and the
   broadcast picker read, computed once on the `Agents` actor.
3. **Model-side parity.** `discover_peers` returns that activity so an
   orchestrator can tell which peers are actually working; `send_message`
   accepts several `to` addresses in one call with one receipt per recipient.

Not in v1 (and the seams left for them):
- Cross-node broadcast (`crates/nodes` children of another harness): rows are
  `Skipped{reason:"remote node"}`; the selector type already carries them.
- A distinct `AgentState::Asking` for a session blocked in `ask_user` — today
  it reads as `Running`; `SessionActivity.asks` is reserved as `Option<u16>`
  and stays `None` until the ask registry is reachable from the supervisor.
- A broadcast *history* view (who was told what, when). Each recipient's
  transcript holds the message with its marker line; the receipt is returned
  to the caller and not stored separately.
- Reply collection for human broadcasts ("tell me when each has answered").
  The per-row receipt carries `input_id`, so a later view can correlate.
- Broadcasting to remote A2A peers from the human UI. Model `send_message`
  fan-out reaches them; the human path stays local (no credential use from a
  UI button).

## 0. Decisions in one screen

1. **One new owner RPC, `broadcast`, with a server-side selector and a
   per-session receipt.** `targets` is `Running | Roots | Sessions([..])`;
   the supervisor resolves it into an exact id list at send time, routes each
   through the existing `human_input()` path, and returns one row per session
   (`Started | Queued | Skipped | Failed`). `dry_run: true` returns the rows
   without sending — that is the preview every frontend shows before the
   button. *Rejected:* frontends looping `steer`/`send_input` themselves —
   Slack/Signal and the TUI would each reimplement "running" and the result
   would differ per client; the `Cancel` fan-out (`supervisor.rs:2097`) shows
   the actor already owns this shape.
2. **"Running" means root sessions only; children are explicit.** The default
   selector picks roots (`parent == None`) whose state is `Running | Awaiting
   | Approval` or that own a running job/PTY. A child appears in the picker
   under its root and can be ticked, but is never swept in by `Running` or
   `Roots`. *Rejected:* including subagents by default — the parent is the
   authority over its children (50 §0.2); a broadcast that also hits every
   child makes the parent and children act on the same instruction twice.
3. **Delivery is chosen per broadcast: `Steer` (default) or `NextTurn`.**
   `Steer` is today's behaviour (idle → turn now; mid-turn → after the tool
   batch, `loop_.rs:980-1018`). `NextTurn` is new: the input is queued with
   `defer: true`; `drain_steer()` (`loop_.rs:2797`) leaves it, `next_batch()`
   at `TurnDone` (`loop_.rs:795`, `:1640`) takes it. Same queue, same
   `InputQueued`/`InputInjected`/`InputWithdrawn` lifecycle, so `withdraw`
   works for both. *Rejected:* a second queue or a supervisor-side hold —
   the session actor is the only place that knows when its turn ends.
4. **A broadcast text carries a one-line marker.** The stored user message is
   `[broadcast b7f3 · 4 sessions]\n<text>`. Recipients are peers on one
   project; without the marker each assumes it alone was told and four agents
   start the same fix. *Rejected:* verbatim text with a UI-only badge — the
   model, not the UI, is the reader that needs the fact.
5. **Activity is computed on the `Agents` actor from state it already holds,
   plus one bulk ask to the `Jobs` actor.** `running`, `approvals`, `waits`,
   `child_turns`, `last_stop` (`supervisor.rs:143-180`) already feed
   `agents()` (`:3663`) and `directory()` (`:3212`); a new `last_input` map
   is set in `route()` (`:736`) and `inject()` (`:838`). Job/PTY counts come
   from one `JobCounts(Vec<SessionId>)` message on `Jobs`
   (`crates/tools/src/jobs.rs:499`). Cost stays out (it is a SQLite read,
   `store/accounting.rs:512`). *Rejected:* a separate activity actor or a
   heartbeat table — duplicate state that can disagree with the roster.
6. **`AgentInfo` grows an `activity` field; `list_activity()` returns the
   active set across roots.** The `/agents` roster and desktop `AgentTree`
   get the new columns for free (serde default, ts-rs regenerated). The
   picker needs every owner-visible session that is alive, not one subtree,
   hence `list_activity()`, authorized like `list_sessions`. *Rejected:*
   folding activity into `SessionSummary` — that is the static, DB-backed row
   (`lib.rs:1020`) and the sidebar list is virtualized over hundreds of rows.
7. **`PeerInfo` gains `activity: Option<PeerActivity>`; titles stay
   same-tree.** Every peer the policy routes to sees liveness detail
   (fine-grained state, `last_active`, `jobs`, `ptys`, `children`); this
   amends 50 §5.1's "coarse liveness" because the coarse five-value
   `PeerState` cannot answer "who is actually working". `title` is disclosed
   only for `Parent|Child|Sibling|Ancestor|Descendant` (the caller's own
   tree); other roots still show only alias/brief name. Remote A2A peers have
   `activity: None` — "a card is not a heartbeat" stands. `discover_peers`
   gains `active_only` and sorts running-first, then `last_active` desc.
   *Rejected:* exposing `cwd`/`model`/`last` across roots — unchanged 50 §5.1
   minimum disclosure for strangers.
8. **`send_message.to` accepts one address or a list; the receipt becomes a
   list.** The tool resolves every address first (new `Resolve` ask), then
   issues one `Deliver` per recipient; `structured.receipts[]` has one
   `PeerReceipt` each, `is_error` only when every recipient failed.
   `await_reply` waits for all replies within one timeout and reports each.
   Cap 16 recipients per call; `request_id` dedup already keys on
   (request_id, addressee, body), so one id covers the fan-out. *Rejected:*
   a new `broadcast_message` tool — 50 §5.2 "keep one send tool"; and a
   `Deliver` that takes a list — receipts, waits and idempotency are
   per-exchange in the supervisor and should stay that way.
9. **No new event type.** Recipients emit `TurnStarted`/`InputQueued` as for
   any human input (`lib.rs:1197,1306`); the caller gets the receipt in the
   RPC reply. *Rejected:* `AgentEvent::Broadcast` for cross-device toasts —
   nothing consumes it yet; add when a history view exists.

## 1. Why

Sky runs several sessions on one project at once (design 59/60 builds:
3–4 omp children plus orchestrator roots). Telling them all "stop touching
`Cargo.lock`" today means switching sessions and pasting four times, and
the orchestrator model cannot even see which of its peers are alive:
`PeerInfo` (`communication.rs:295`) has no timestamp, no job count, and its
`PeerState` collapses `Approval` into `Running`. The human roster is better
(`AgentState`, `lib.rs:676`) but only per subtree and without jobs/PTYs, which
is exactly what marks a session as "working" while its model is idle between
`<job_result>` wakes (61).

## 2. Facts relied on (recon 2026-09-22, `docs/design/scratch-broadcast-recon.md`)

- `send_input` (`crates/rpc/src/api.rs:148`) and `steer` (`:157`) both call
  `human_input()` → `input()` → `route()` (`crates/agent/src/supervisor.rs:775,
  717, 736`) → `LiveActor::tell_input(UserInput)` (`:253`), for native and
  ACP actors alike. Origin is `InputOrigin::External` (`continuation.rs:45`).
- `Session::handle(UserInput)` (`crates/agent/src/loop_.rs:980`): no turn in
  flight → `start_turn` now; otherwise push onto `shared.steer` and publish
  `InputQueued`. Mid-turn drain: `drain_steer()` `:2797` (between tool
  batches, filtered by `TurnScope::admits`); turn end: `TurnDone` `:1640` →
  `next_batch()` `:795`. There is **no** "hold until the turn ends" path.
- `Input` (`loop_.rs:139`): `text, files, replay, ticket, scope, peer`.
  `UserInput` (`:121`): `id, text, files, ticket, scope`. `QueuedInput`
  (`lib.rs:146`): `{id, text}`. `WithdrawInput` `:1360`, `QueuedInputs`
  `:1386` are native-only (`native()`, `supervisor.rs:286`).
- `AgentInfo` (`lib.rs:643`): `id, name, parent, depth, agent, model, state,
  started_at, finished_at, last, project, retention`. `AgentState` (`:676`):
  `Idle | Running | Awaiting{from} | Approval{tool} | Error | Cancelled`.
  Built by `agents()` `supervisor.rs:3663-3699` from `self.approvals`,
  `self.running`, `self.waits`, `self.last_stop`, `self.child_turns`.
- `SessionSummary` (`lib.rs:1020`) is static (SQLite, `store.rs:137`,
  ordered by `pinned, updated_at`; `updated_at` bumps at `store.rs:1460`).
- `Cancel` (`supervisor.rs:2097`): builds the id set (session + descendants),
  loops with a liveness guard, cleans up once; reply `()`.
- `PeerInfo` (`communication.rs:295-311`), `PeerState` (`:270-281`),
  `PeerPage` (`:319`), `ListPeers` (`:392`), handled at `supervisor.rs:3169`
  via `directory()` `:3212-3300` (policy filter at `:3241`, state at
  `:3271-3277`).
- `SendMessageArgs` (`peers.rs:185`): `to: String, message, await_reply,
  reply_to, request_id`. Gate check `:246-250`; `Deliver` (`supervisor.rs:338`)
  → `Delivered{receipt: PeerReceipt, reply: Option<oneshot>}` (`:358`);
  `resolve()` `:872` handles `Remote | Session | Relative("parent") |
  Relative(child name)`. `PeerReceipt` (`communication.rs:190`).
- `Jobs` actor (`crates/tools/src/jobs.rs:499`, `sessions: HashMap<SessionId,
  SessionJobs>`); `ListJobs(SessionId)` `:275`; `ListTerminals{sessions:
  Vec<SessionId>}` (`pty.rs:933`) already takes a vector.
- RPC plumbing (worked example `set_pty_idle`): trait `api.rs:307`, impl
  `:1839`; TS types generated by `just ts` (`export.rs:1-5`, `export!` list
  `:16-244` for new types); hand-written `crates/desktop/ui/src/rpc/api.ts`
  `interface Api` + `METHODS` (`:487`), enforced by `api.test.ts:12`; TUI
  calls in process (`crates/tui/src/lib.rs:455`). Relay/link transparent.
- TUI: slash table `crates/tui/src/model.rs:~1225`, dispatch `:5559-5657`;
  `Shake` picker `:2133-2147`, keys `shake_key()` `:7197-7229`
  (Space ticks, Enter Pick→Review→fire, Esc back); `Menu::Shake` `:1608`.
- Desktop: React/Tauri v2; `Composer.tsx:363-373` steer/send fork with
  optimistic `InputQueued`; `ContextPicker.tsx:31-44` dialog; `AgentTree.tsx:
  76-86` seeded from `list_agents`; `store/sessions.ts:180,251`; reducer
  `store/reduce.ts:499,627`; platform `lib/platform.ts:2-7`; slash mirror
  `components/chat/slash.ts:18` (hand-kept copy of `tui::model::COMMANDS`).
- Parity rule: 25 §table row 235 (one row per feature across slash popup,
  menu, dialog), `docs/design/cross-platform/recon-android.md:229` ("every
  desktop feature is operable from Android").

## 3. Shapes

### 3.1 Delivery and the deferred flag (crates/agent)

```rust
/// How a human input lands on a busy session (62 §0.3).
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum Delivery { #[default] Steer, NextTurn }
```
- `UserInput` and `Input` gain `defer: bool` (`#[serde(default)]` where
  serialized). `Steer`/`SendInput` gain `delivery: Delivery` (default
  `Steer`) so the single-session RPCs get the same choice for free.
- `drain_steer()`: partition keeps `(_, i) if i.defer`; `next_batch()`
  clears `defer` on what it takes (a deferred input that opens a turn is an
  ordinary opener). `TurnScope::admits` unchanged.
- `AgentEvent::InputQueued` gains `defer: bool` (serde default) so clients
  can label the queued card "next turn".
- ACP child (`acp_child.rs:898`): the ACP prompt path has no mid-turn
  injection; `defer` is a no-op there and the receipt row says
  `Queued{delivery: Steer}` honestly.

### 3.2 Activity

```rust
/// Live facts about one session, computed on `Agents` (62 §0.5).
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
pub struct SessionActivity {
    pub state: AgentState,                 // as `agents()` computes it
    pub last_active: Option<DateTime<Utc>>, // max(last_input, last_stop, turn start); store updated_at when cold
    pub turn_started_at: Option<DateTime<Utc>>,
    pub jobs: u16,                         // running background jobs (non-PTY)
    pub ptys: u16,                         // running PTY jobs
    pub children: u16,                     // descendants with a live actor
    pub asks: Option<u16>,                 // reserved (Not in v1)
    pub live: bool,                        // has an actor now (vs stored/cold)
}
```
- `AgentInfo.activity: SessionActivity` (new field; the old `state`,
  `started_at`, `finished_at` remain for compatibility, `state` duplicates
  `activity.state`).
- New supervisor field `last_input: HashMap<SessionId, DateTime<Utc>>`, set
  in `route()` and `inject()`; cleared with the other maps on
  `SessionDeleted`.
- New `Jobs` message `JobCounts(pub Vec<SessionId>) -> HashMap<SessionId,
  (u16 /*jobs*/, u16 /*ptys*/)>` beside `ListJobs` (`jobs.rs:275`); one ask
  per `agents()`/`list_activity()`/`directory()` call, never per row.
- New supervisor message `ListActivity` (`ask` → `Vec<AgentInfo>`): every
  session with a live actor **or** a non-zero job count, plus every root,
  depth computed from its own root (0 for roots). This is the picker's
  source; cold children are omitted (a cold child is not a broadcast
  target).

### 3.3 Broadcast

```rust
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BroadcastTargets {
    Running,                       // roots with state Running|Awaiting|Approval or jobs+ptys>0
    Roots,                         // every root with a live actor
    Sessions { ids: Vec<SessionId> }, // explicit; roots or children; deduplicated
}

pub struct BroadcastRequest {
    pub targets: BroadcastTargets,
    pub text: String,
    pub files: Vec<PathBuf>,       // 17 §2 image files; copied per recipient by path
    pub delivery: Delivery,
    pub dry_run: bool,
}

#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum BroadcastOutcome {
    Started,                       // was idle; a turn opened (TurnStarted follows)
    Queued { input_id: InputId, delivery: Delivery },
    Skipped { reason: String },    // dry run rows, "remote node", "no actor and not a root", "excluded: private of another owner"
    Failed { error: String },
}

pub struct BroadcastRow {
    pub session: SessionId,
    pub title: String,
    pub parent: Option<SessionId>,
    pub before: SessionActivity,   // what the picker showed / what was true at send
    pub outcome: BroadcastOutcome,
}

pub struct BroadcastReceipt {
    pub id: Uuid,                  // the marker's short id is its first 4 hex chars
    pub at: DateTime<Utc>,
    pub rows: Vec<BroadcastRow>,
}
```
Supervisor `impl Message<Broadcast>`:
1. Resolve `targets` against `store.list_sessions()` + activity (one
   `JobCounts` ask). `Running`/`Roots` filter `parent.is_none()`.
   `Sessions` keeps the given order, drops unknown ids as
   `Failed{"unknown session"}`.
2. Cap: `agent.broadcast.max_targets` (default 32); above it →
   `Err("too many targets")` before any send.
3. `dry_run` → every row `Skipped{"dry run"}` with `before` filled; return.
4. Otherwise mint `id`, build `text = format!("[broadcast {short} · {n}
   sessions]\n{text}")`, and for each row call the same `input(session,
   UserInput{ id: Some(minted), text, files, defer, .. },
   InputOrigin::External)` used by `human_input()`; an idle session yields
   `Started`, a busy one `Queued`. Per-row errors are captured, never abort
   the loop (unlike `Cancel`, which has nothing to report).
5. No new event; recipients publish `TurnStarted`/`InputQueued`.

Authorization: the RPC layer authorizes every resolved id like
`send_input` does (51 §4.2 `PrivateMap`: a private session of another
device is `Skipped{"not visible from this device"}`, never `Failed` with a
title). ACP children are legal targets (same `tell_input`); remote-node
children are `Skipped`.

### 3.4 Peer directory (model-facing)

```rust
pub struct PeerActivity {
    pub state: PeerLiveState, // Idle|Running|Approval|Awaiting|Error|Cancelled — the AgentState names, plus Stored|Offline
    pub last_active: Option<DateTime<Utc>>,
    pub turn_secs: Option<u64>, // age of the current turn
    pub jobs: u16, pub ptys: u16, pub children: u16,
}
// PeerInfo additions
pub activity: Option<PeerActivity>, // None for Remote (A2a)
pub title: Option<String>,          // same-tree relationships only (62 §0.7)
```
`directory()` computes `activity` from the same fields as `agents()` (one
`JobCounts` ask for the whole permitted set). `PeerState` stays for
compatibility and is derived from `activity.state` (unchanged mapping).

`DiscoverPeersArgs` gains `active_only: bool` (default false) — keeps rows
whose `activity.state ∈ {Running, Approval, Awaiting}` or `jobs+ptys > 0`.
Default order: running first, then `last_active` desc, then the current
`created_at` order. The tool description says what the fields mean in one
line each ("`last_active` is the last input or stop, not a heartbeat").

### 3.5 `send_message` fan-out (model-facing)

```rust
#[derive(Deserialize)]
#[serde(untagged)]
pub enum Recipients { One(String), Many(Vec<String>) }
pub struct SendMessageArgs { pub to: Recipients, /* rest unchanged */ }
```
Tool flow (`peers.rs:242`):
1. Gate as today (`:246-250`).
2. Normalize to `Vec<String>`, dedupe, reject `> 16` or empty with a
   terminal error (`is_error`), no sends.
3. New `Resolve { from, to: Vec<String> }` ask → `Vec<Result<Target,
   String>>`; if any fails and `to` had more than one entry, return the
   per-address errors and **send nothing** (a partial fan-out is the
   surprising outcome; the model can resend the good subset).
4. One `Deliver` per recipient, sequentially (the supervisor is one actor;
   ordering is deterministic and receipts arrive in `to` order). Same
   `request_id`, `reply_to`, `text` for each; dedup is per addressee already.
5. `await_reply`: collect every `reply` oneshot with one shared deadline
   (`reply_timeout_secs`); each receipt ends `Replied` or stays `Accepted/
   Consumed` with the note "timed out; message not recalled".
6. Result: text is one line per recipient (`session:… accepted id=…`);
   `structured = { receipts: [PeerReceipt...] }` (a single `to` still returns
   a one-element list — one shape, the description says so). `is_error` only
   when every recipient failed at step 3/4.

Recipient wrapper unchanged: `<message from=… id=…>`. *Rejected:* a `cc`
attribute listing co-recipients — discloses addresses the recipient's own
policy may not route to.

## 4. Owner API (rpc::Api)

```text
broadcast(req: BroadcastRequest) -> BroadcastReceipt          # new
list_activity() -> Vec<AgentInfo>                             # new; active set across roots
send_input(session?, text, files, delivery?)                  # delivery added, default Steer
steer(session, text, id?, files, delivery?)                   # delivery added
list_agents(session) -> Vec<AgentInfo>                        # AgentInfo.activity added
```
Plumbing per 25/61 checklist: trait + impl in `api.rs`; `export!` lines for
`Delivery, SessionActivity, BroadcastTargets, BroadcastRequest,
BroadcastOutcome, BroadcastRow, BroadcastReceipt`; `just ts`; `api.ts`
`interface Api` + `METHODS` (`api.test.ts` enforces); 25 table row.
`InputQueued.defer` and `AgentInfo.activity` are additive with serde
defaults — old clients ignore them.

## 5. Frontends

### 5.1 TUI (`crates/tui`)
- `/broadcast [running|roots] [text]` in `COMMANDS`. With text and a
  selector: dry run → Review → Enter sends. Without: opens
  `Menu::Broadcast(Box<Broadcast>)` modelled on `Shake` (`model.rs:2133`):
  `stage: Pick | Compose | Review`, `rows: Vec<AgentInfo>` from
  `list_activity()`, `selected: BTreeSet<SessionId>`, `delivery`, `text`.
  Pick: rows grouped root → children (indented like `/agents`), each `glyph
  name state jobs/ptys age`; Space ticks; `r` ticks all running roots; `a`
  all roots; `d` toggles Steer/NextTurn; Enter → Compose (single-line editor,
  the same input widget as steer); Enter → Review shows the dry-run rows
  (`before.state`, delivery) and the marker line; Enter fires
  `Command::Broadcast(req)` → `UiEvent::Broadcasted(receipt)` rendered as a
  compact table (title · outcome). Esc backs one stage.
- `/agents` rows append `jobs/ptys` and `last_active` age from
  `AgentInfo.activity`; the live-event overlay (`AgentRow.fresh`, design 19
  §3.3) keeps working because `activity` is snapshot data like `state`.
- Status line: after a broadcast, "sent to 4 (2 started, 2 queued)".

### 5.2 Desktop & mobile (`crates/desktop/ui`)
- `components/chat/BroadcastDialog.tsx`: a `Dialog` (sheet on mobile via the
  existing `useIsMobile` pattern, `components/ui/sidebar.tsx:34`) with three
  panes in one scroll: **Targets** (checkbox rows from `list_activity()`,
  grouped root → children, quick chips "Running roots" / "All roots" /
  "None", each row shows state badge, `jobs/ptys`, last-active age),
  **Message** (textarea + image attach reusing the composer's file path),
  **Delivery** (segmented control Steer / Next turn). Footer: the dry-run
  summary ("4 sessions · 2 busy → queued, 2 idle → start now") refreshed on
  every change via `broadcast({dry_run:true})`, and a single button
  "Send to 4 sessions" (the label is the confirmation, no second prompt).
- Entry points: sidebar header action "Broadcast…"; slash `/broadcast` in
  `slash.ts` (mirror of the TUI table); composer overflow "Send to many…"
  prefilled with the composer text.
- Result: a toast with per-row outcomes; failures listed by title.
- `AgentTree.tsx` rows: state badge + `jobs/ptys` counts + relative
  `last_active`; `store/sessions.ts.children` already holds `AgentInfo`.
- Reducer: `InputQueued.defer` → queued card shows "next turn". No other
  reducer change.
- No Tauri-only logic; the dialog is plain React over `api.broadcast`.

### 5.3 Slack / Signal / Telegram (owner only)
`/broadcast running <text>`, `/broadcast roots <text>`, `/broadcast
<id>,<id> <text>`; optional leading `--next` for `NextTurn`. Replies with
the receipt as compact lines (`title — started|queued|skipped: reason`).
Telegram's `/broadcast` takes the same text form (98 §10.3).
Non-owner contacts are refused as for `/communication` (50 §10).

## 6. Threats

| Threat | Handling |
|---|---|
| Broadcast reaches a private session of another device | RPC authorizes each resolved id (51 §4.2); row `Skipped{"not visible from this device"}`, no title leaked |
| Accidental blast to dozens of sessions | `max_targets` (32) refused before any send; every frontend shows the dry-run row list and the count in the button label |
| Four agents execute the same instruction | Marker line `[broadcast … · N sessions]` in the stored text (§0.4); prompt.md gains one sentence: a broadcast is shared context, act only on the part addressed to you |
| Model fan-out as a spam vector | 16 recipients per call; existing 50 §8 pending-work bounds apply per exchange; communication gate checked once per call and per `Deliver` |
| Activity fields leak across roots | Liveness only (state, times, counts) for routed peers; `title` only same-tree; `cwd/model/last` never (50 §5.1 kept) |
| Deferred input lost on restart | Same as today's steer queue (in-memory; 50 §7 durability is for peer exchanges). Receipt says `Queued`; a restart drops it as it drops a steer. Documented, not fixed here |
| Marker text spoofed by a peer message | The marker is prepended by the supervisor to human input only; peer input is wrapped `<message …>` (50 §9) and cannot start with a bare user line |
| Remote A2A peer sees more than before | `activity: None`, `title: None` for `PeerRelationship::Remote` |

## 7. Config

```yaml
agent:
  broadcast:
    max_targets: 32          # human broadcast cap
  communication:
    directory_activity: true # PeerInfo.activity / title(same-tree); false → 50 §5.1 rows as before
    send_max_recipients: 16  # send_message fan-out cap
```

## Amendments

- 50 §5.1: directory rows carry `activity` and same-tree `title` (§0.7);
  "coarse liveness" becomes "liveness detail, no content". The alternative it
  beat: a second, richer directory tool — two directories with two
  disclosure rules.
- 50 §5.2: `send_message.to` is one address or up to 16; receipts are a
  list. Beat: a separate `broadcast_message` tool.
- 50 §10: add `broadcast`, `list_activity`, `delivery` on `send_input`/
  `steer` to the owner API inventory; frontend table gets the 62 row.
- 19 (roster): `AgentInfo.activity`; `/agents` columns.
- 47 §4.5: a broadcast row is `InputOrigin::External` per recipient (a human
  spoke), so each opens its own reminder budget as a typed message would.
- 25 §table: row 62 — slash popup, sidebar action, `BroadcastDialog`,
  `AgentTree` badges; mobile sheet variant.
- 61 §2.3: `<job_idle>` and a broadcast may land in the same batch; no
  ordering guarantee, none needed.

## Build steps

| # | Step | Where | Done when |
|---|---|---|---|
| 1 | `Delivery`/`defer` through `UserInput`/`Input`; `drain_steer` skips, `next_batch` takes; `InputQueued.defer`; `delivery` on `Steer`/`SendInput` | `crates/agent/src/loop_.rs`, `lib.rs`, `supervisor.rs` | Unit test in `loop_.rs`: a deferred input queued mid-turn is absent from `drain_steer()` and present in the `TurnDone` batch; `withdraw` removes it; `cargo check -p agent` green |
| 2 | Activity: `last_input` map, `JobCounts` on `Jobs`, `SessionActivity`, `AgentInfo.activity`, `ListActivity` | `crates/tools/src/jobs.rs`, `crates/agent/src/supervisor.rs`, `lib.rs` | Test: a root with a running PTY and idle model reports `state: Idle, ptys: 1, live: true`; `ListActivity` excludes a cold child, includes a cold root |
| 3 | `Broadcast` message + RPC `broadcast`, `list_activity`, `delivery` params; TS export; `api.ts`/`METHODS` | `supervisor.rs`, `crates/rpc/src/api.rs`, `export.rs`, `crates/desktop/ui/src/rpc/api.ts` | Test: `Running` selects only roots; dry run sends nothing and returns `before`; busy → `Queued`, idle → `Started`, unknown id → `Failed`; over cap → `Err`; `bun test api.test.ts` green |
| 4 | TUI `/broadcast` picker + `/agents` columns | `crates/tui/src/model.rs`, `lib.rs`, `ui.rs` | Manual: tick two roots, NextTurn, see receipt table; `/agents` shows `jobs/ptys` and age |
| 5 | Desktop/mobile `BroadcastDialog`, sidebar action, `slash.ts`, `AgentTree` badges, reducer `defer` | `crates/desktop/ui/src/components/…` | vitest for dialog selection logic (chips, dry-run label); iOS sheet renders; the 25 row |
| 6 | Slack/Signal `/broadcast` | `crates/slack`, `crates/signal` | Owner command returns receipt lines; non-owner refused |
| 7 | `PeerInfo.activity`/`title`, `active_only`, ordering; `send_message` `Recipients`, `Resolve`, receipts list; tool descriptions; prompt.md sentence | `crates/agent/src/communication.rs`, `supervisor.rs`, `peers.rs`, `frame.rs` | Tests: cross-root row has `title: None` and `activity: Some`; remote row `activity: None`; two-recipient send yields two receipts, one bad address yields no sends; `active_only` drops an idle jobless root |
| 8 | Docs: this doc's As-built sections, 50/19/25 amendments, README slash entry, `skills/` mention of `to: [..]` | `docs/`, `~/.theuth/skills` | Each amendment lands in the same commit as its step |

One commit per step, `cargo check` scoped to touched crates (repo is not
rustfmt/clippy-clean workspace-wide: format only touched hunks). Steps 1–3
are sequential; 4, 5, 6 can run in parallel worktrees off step 3; 7 depends
on 2 only and may run beside 3.

## Open

- Should `Running` also include roots whose only activity is a pending
  approval? Current answer: yes (they are mid-turn; the broadcast queues
  behind the approval and the row's `before.state` says so).
- `NextTurn` on an ACP child is silently `Steer`. Acceptable for v1, or
  refuse with `Skipped{"acp: steer only"}`? Leaning refuse-if-explicit, allow
  when the selector was `Running`/`Roots` (children are never in those).
- Whether the composer's existing "interject" (cancel-then-steer,
  `Composer.tsx:329-366`) should exist for broadcast. Not in v1: cancelling four
  sessions at once is a different, more destructive action.
- `last_active` for a session whose only recent activity is a streaming
  model response without tool calls: `last_input` and `last_stop` miss it.
  If it matters, `TurnStarted` handling in the supervisor can bump it; cheap
  to add in step 2 if the test shows a gap.

## Status

2026-09-22 — designed; recon in `docs/design/scratch-broadcast-recon.md`
(delete once step 1 lands). Sky's calls in the interview: delivery chosen
per broadcast (default steer); "all running" = roots only; the model gets
multi-recipient `send_message` and an activity-bearing `discover_peers`.
