# 19 — Subagent board: observing and steering any number of children

Status: design v0 · 2026-09-13 · not started · extends `01-config-and-subagents.md`
§2.7 (events, UI), `00-architecture.md` §7 (RPC), §8 (TUI), `09-signal.md` §8.
Where this doc disagrees with those, this doc wins; §9 lists the edits.
Every file:line below was read on 2026-09-13.

## 0. The problem, in the tree

01 §2.2 item 5 promised: *the human sees children in the TUI roster, can
focus one to read its transcript, and can steer it.* What shipped is
`/agents` (a static list) and `/focus <name>` (a reload). Between those two
the human is blind and mute:

| Fact | Where |
|---|---|
| A child's `TextDelta`, `ToolCall*`, `Error`, `Compacting`, `Harvesting` are dropped unless that child is focused. The parent view shows `↳ spawned x` and, minutes later, a result card. | `tui/src/model.rs:1750-1752` |
| `/focus` **clears** `entries`, `jobs`, `usage`, `context` and reloads from the store; Esc reloads the root the same way. Root text streamed while focused is not shown live and is unrecoverable until persisted. | `model.rs:2078-2084`, `:1420-1442` |
| The status line carries one number: `agents: N running`. Nothing says *which*, *doing what*, *for how long*, *blocked on whom*. | `model.rs:2134-2139` |
| A child blocked in `send_message{await_reply}` waits up to 120 s for the parent model. A human who focuses it and types is *queued behind the blocked tool call*; the waiter key `(waiter, from)` compares session ids, so a human never resolves it. | `supervisor.rs:928-931`, `peers.rs:175-181` |
| `subscribe_events(parent)` never receives children's events, nor `SubagentSpawned` (its `session()` is the child). A Tauri client must subscribe to everything and filter itself, or subscribe per child after discovering it. | `rpc/src/api.rs:770-776`, `lib.rs:761` |
| `ListChildren` is one level, store rows only: no running flag, no depth, no last text. `Cancel` fans out one level. After `--resume`, `/focus` fails until `/agents` runs. | `supervisor.rs:1122-1135`, `:662-672`, `model.rs:2052-2054` |
| `Loaded` has no `name`/`parent`; a focused header shows the model only. | `lib.rs:183-200`, `model.rs:2155` |
| Signal: `/agents` only. | `signal/src/commands.rs:262-276` |

The model side is fine and stays as is: children are detached, results
arrive as `<subagent_result>`, peers talk through `send_message`, the roster
is not in the prompt (07 §10). This doc is about the **human** side: one
API surface for a roster with live state, one TUI shape that shows every
child's state without leaving the parent, and one addressing form that
steers any child from anywhere.

## 1. Principles applied

- **One list of commands** (00 §8): everything below is an `rpc::Api` method
  or an existing `AgentEvent`. The TUI calls the trait in-process; Tauri and
  Signal get the same thing for free.
- **Derive, don't duplicate**: no new `AgentEvent` variant. Running, current
  tool, awaiting-reply, error, elapsed are all already on the bus as
  `TurnStarted/Finished`, `ToolCallStarted/Finished`, `Error`,
  `SubagentSpawned`. A frontend that has the primitive events computes the
  board; a frontend attaching late asks for one snapshot (`list_agents`).
  (Rejected: a supervisor-published `SubagentState` event — a second copy of
  state that already streams, which drifts.)
- **Board = live, transcript = history.** What is running is shown in a
  place that updates in place; what finished becomes a card once. Same rule
  tool cards already follow (00 §8 "nothing ever looks frozen").
- **Text-native addressing.** Children are named; `@name` in the editor is
  the steer, on every frontend, with completion where there is a screen.
  (Rejected: number keys / tabs — collide with typing; a mouse-driven tree
  — no mouse capture, 00 §8.)
- **Scale by rule, not by pane.** N is bounded by `max_concurrent` (8) per
  parent but unbounded over a session's life and over depth. The board shows
  what is *live*, caps rows, and idle children fold into a count; `/agents`
  expands. (Rejected: a side pane — 25 columns of an 80–120 column terminal
  taken from markdown for a list that is empty most of the time. Tauri gets
  the tree pane; the API here is what it needs.)

## 2. API surface (`rpc::Api`)

### 2.1 `list_agents(session) -> Vec<AgentInfo>`

The subtree under `session` (children, grandchildren, …; `Store::descendants`
via one recursive CTE on `sessions.parent`), pre-order, each with live state
from the supervisor:

```rust
#[derive(Clone, Debug, Serialize, Deserialize)]           // agent crate, ts-rs behind the feature
pub struct AgentInfo {
    pub id: SessionId,
    pub name: String,                 // peer name (unique per parent)
    pub parent: SessionId,
    pub depth: u8,                    // 1 = direct child of `session`
    pub agent: Option<String>,        // external harness name (06), None = wth
    pub model: String,
    pub state: AgentState,
    pub turns: u32,                   // finished turns (store count of user rows, cheap)
    pub started_at: Option<DateTime<Utc>>,  // current turn start; None when idle
    pub last: String,                 // first line of the last assistant text, ≤ 80 chars
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum AgentState {
    Idle,
    Running,
    Awaiting { from: String },        // blocked in send_message{await_reply} on `from` ("parent" or a child name)
    Approval { tool: String },        // an ApprovalRequested for this session is unanswered
    Error,                            // last turn ended stop_reason = "error"
    Cancelled,                        // last turn ended "cancelled" (incl. max_runtime timeout)
}
```

Sources, all already held by `Agents`: `running` set (`supervisor.rs:495-504`),
`waiters` keys `(waiter, from)` → `Awaiting` for the `waiter` (`:74`),
`child_turns` gains `started_at: Instant` next to its timer (`:149-152`),
store `last_turn_text` (`store.rs`) for `last`. `Approval` comes from the
bus: `Agents` subscribes to `ApprovalRequested`/`ApprovalResolved`
(`tools/src/approval.rs:38, :56`, both carry `id`; the request carries
`session` and `tool`) and keeps `HashMap<SessionId, (ApprovalId, String)>` —
the same two events the TUI already renders the modal from, so no new ask on
the registry. `Error`/`Cancelled` come from the last `TurnFinished.stop_reason`
per session, kept in a small `HashMap<SessionId, String>` the supervisor
already sees the event for.

`Roster { session }` (the compaction-head line `Subagents: a (running), b
(idle)`, `:986-1007`) is rendered *from* this vector: one source of state.
`ListChildren`/`list_children` are removed; the two callers (`/agents` in TUI
and Signal) move to `list_agents`.

### 2.2 `subscribe_events(session?, descendants?)`

`descendants: true` (default `false` — wire-compatible) delivers every event
whose session is `session` or in its subtree, plus `SubagentSpawned` whose
`parent` is in the subtree. The RPC plugin keeps the set: one `list_agents`
on subscribe, then adds `child` on each `SubagentSpawned` it forwards. The
in-process TUI is unaffected (it subscribes to the bus).
(Rejected: stamping `root`/`parent` on every `AgentEvent` — a field on 20
variants for one consumer; rejected: per-child subscriptions from the client
— it cannot subscribe before it knows the child exists, and the spawn event
is the one it would miss.)

### 2.3 Steering (behaviour, no new methods)

- `steer` / `send_input` already take any session id. New rule in
  `Agents::input`: if the target has a pending waiter `(target, _)` — it is
  blocked in `send_message{await_reply}` — the text **resolves the waiter**
  instead of queueing: the child's tool call returns the human's words, tagged
  by publishing `MessageDelivered { session: target, from: "you", text }` so
  both transcripts show it. A human answer is the most useful steer there is;
  today it is the one that cannot land (§0 row 4).
- `cancel(session)` fans out to every **running descendant**, not one level.
  Idle descendants are left alone (01 §2.8 unchanged).
- `Loaded` gains `name: Option<String>` and `parent: Option<SessionId>` so a
  focused view can head itself without a roster lookup.

### 2.4 Not added

No `focus`/`view` state on the server (a frontend concern); no `rename_agent`
(names are the model's handle; renaming them mid-run breaks `send_message`);
no `pause`; no `steer_all`. Model-facing tools unchanged (01 §2.4 stands:
no `list_subagents`, `cancel_subagent`, `wait`).

## 3. TUI

### 3.1 Views: focus is a switch, not a reload

`Model` today has one `entries`, `streaming`, `usage`, `context`, `queued`,
`jobs`, `model_id`, `effort` — the root's, overwritten on `/focus`. These
move into `struct View` in `Model.views: BTreeMap<SessionId, View>`;
`Model.shown: SessionId` picks the one rendered. `apply_agent` routes each
event to `views[ev.session()]`, creating the view on first sight (a child
spawned this run) or on `/focus` (a `Load` fills it once). Every view keeps
receiving deltas whether shown or not; switching is instant and lossless in
both directions — the root keeps streaming while you read a child, and the
child's text is complete when you come back to it.

Memory: a `View` is the entries of one session, the same thing the root
holds today; `Model::tail` already bounds what is *laid out* per frame
(00 §8). A child view that is idle and unfocused for the whole session costs
its transcript once. (Rejected: keeping only the shown view live and
replaying from the store on switch — that is today's behaviour, and the
store lags the stream by a tool batch.)

### 3.2 The board

A block between the transcript tail and the editor, present only while at
least one descendant is not idle (or while pinned by `/agents`). One row per
agent, indented by depth, running first in spawn order, then awaiting /
approval, then error / cancelled; idle rows only when pinned. Rows are
derived from `views[id]`'s last events plus the `list_agents` snapshot taken
on attach/resume:

```
╭ agents ─────────────────────────────────────────────────────────────────╮
│ ⠹ grep-logs      running  read_file crates/agent/src/loop_.rs   1m32s  4⚙ │
│ ⏳ fix-tests      awaiting parent  "should I also update the docs?"  48s │
│ ⚠ scout          approval  shell `cargo test -p agent`                   │
│ ✗ deploy-check   error  model: 429 rate limited                          │
│    ↳ probe       idle  ✓ 2 turns  "Ports 80 and 443 answer; 8080 is …"   │
│ … 3 idle  (/agents to list)                                              │
╰──────────────────────────────────────────────────────────────────────────╯
```

Row anatomy: glyph + name (14 cols, the same magenta as `⟵` peer cards,
00 §8) · state word · detail · elapsed · tool count. Detail by state:
running → `preparing <tool>` / `running <tool> <first arg>` / `streaming`
(exactly the words the status line uses for the root); awaiting → the
question's first line; approval → the tool and its described args; error →
the message; idle → `✓ n turns` and `last`. Elapsed ticks from
`started_at` while the frame loop runs (it already runs while anything
animates). Rows are capped at 6; overflow folds into the `…` line. The
board is not focusable and takes no keys; it is a display, like the status
line. Glyphs are text, not emoji (00 §8).

What changes in the root transcript: nothing is added for running children
(the board has them). Two things that are dropped today become one-line
interjects, because they must not be silent: a child's `Error`
(`↳ grep-logs error: model: 429 …`, red) and a child's `TurnFinished` with
`cancelled` (`↳ grep-logs cancelled after 10m (max_runtime)`). Results keep
arriving as `⟵ name` cards (01 §2.7).

### 3.3 Addressing: `@name`

Typing `@` at the start of the editor opens the same popup the slash
commands use, listing the tree's names with their board state; Tab/Enter
completes `@grep-logs `. Submitting `@grep-logs use the second option` sends
it to that child — `Steer` if it is running, `SendInput` if idle, and per
§2.3 an answer if it is awaiting — **without changing the shown view**. The
parent view echoes `◆ you → grep-logs` (one card, collapsed), and the child
view gets the ordinary `◆ you` card via `InputQueued`/`TurnStarted`.
`@parent` from a focused child addresses its parent; `@root` the root.
Names resolve against the whole tree; a duplicate across parents is an
error naming both (`ambiguous: deploy/probe, scout/probe` — address with
`@deploy/probe`), the same shape `Catalog::resolve` uses for models.

Plain input in a focused view still goes to the focused session (today's
behaviour, `model.rs:2015-2046`). `/cancel` acts on the shown session;
`/cancel <name>` on that agent. Alt+Up (withdraw) acts on the shown session.

### 3.4 Focus, header, keys

- `/focus <name>` switches `shown` (view created and `Load`ed on first use;
  works after `--resume` because names come from the `list_agents` snapshot,
  not from `SubagentSpawned` seen this run). Esc with an empty editor returns
  to the parent of the shown view (one level; repeat to reach the root).
- Header while focused: `↳ grep-logs  child of root · claude-haiku-4-5 ·
  running 1m32s` (from `Loaded.name/parent` and the view's state). At depth
  2: `↳ scout ▸ probe`.
- Alt+Right / Alt+Left cycle `shown` through root and the board's row order
  (Tab completes commands, Alt+Up withdraws — both taken). The status line
  shows `alt+←→ agents` while a board exists so the binding is discoverable.
- Approval modal from a child keeps `[agent: name]` (01 §2.7) and adds
  `f focus`, a third key beside `y`/`n`, which answers nothing and switches
  the view so the human can read what led to the request. (Rejected:
  auto-focusing on a child's approval — it yanks the editor from under a
  sentence being typed.)
- Status line gains the board summary when the board is hidden (no live
  rows): `agents: 5 idle`. When rows show, the count is redundant and goes.

### 3.5 Attach and resume

On `--resume` and on the first frame of a new session with a `parent`-less
row, the TUI calls `list_agents(root)` once and seeds `views` (names, state,
`last`) so `/focus`, `@name` and the board are correct before any event
arrives. Same call after `/agents`. Signal and Tauri do the same call; the
TUI has no private path.

## 4. Signal (09)

`/agents` renders `list_agents`: `name · state · last · age` (state word as
in §3.2; no glyphs — Signal renders text). `@name text` steers per §3.3
rules; the reply is `→ grep-logs (queued)` / `(started)` / `(answered)`.
No `/focus` (09 §8 stands: one bound session per chat; a child's transcript
is `wth --resume <id>` territory). `/cancel <name>`.

## 5. Tauri (18)

Nothing specific: `list_agents(root)` + `subscribe_events(root, descendants:
true)` is a live tree; `send_input`/`steer` per node; `ts-rs` emits
`AgentInfo`/`AgentState`. The board of §3.2 is the terminal projection of
the same data; a GUI shows it as a sidebar tree with the shown session's
transcript beside it.

## 6. What the model sees

Unchanged. The roster is not in the prompt (07 §10); `Roster` for the
compaction head now renders from `list_agents`, same text. A human answer to
an awaiting child arrives as the tool's return value, exactly as the
parent's would; the child cannot tell and does not need to. A human `@name`
steer is an ordinary user message to that child (01 §2.3).

## 7. Implementation order

Each step leaves `wth` runnable; skip formatters/tests mid-step, run once at
the end of each.

1. **Store + supervisor.** `Store::descendants(root)` (recursive CTE),
   `AgentInfo`/`AgentState`, `ListAgents` (replaces `ListChildren`; `Roster`
   renders from it), `child_turns.started_at`, last-stop map, human input
   resolves waiters + `MessageDelivered{from: "you"}`, recursive `Cancel`,
   `Loaded.name/parent`. Check (headless, RPC): parent spawns `a`, `b`;
   `list_agents` shows both `running` with `started_at`; `b` asks parent with
   `await_reply`; `steer(b, "teal")` → `b`'s tool output is `teal` and both
   sessions show `MessageDelivered from=you`; `cancel(root)` with a
   depth-2 child running → grandchild `TurnFinished cancelled`.
2. **RPC.** `list_agents`, `subscribe_events(session, descendants)`, remove
   `list_children`. Check: a subscription on the root with `descendants:
   true` receives the children's `TextDelta` and the `SubagentSpawned`;
   without it, neither (today's behaviour).
3. **TUI views.** `View` + `views` + `shown`; `/focus` and Esc switch without
   `Load` after the first; `Loaded` header. Check (pty): `/focus a` while the
   root streams, Esc → root text that streamed meanwhile is present; `/focus
   a` again → `a`'s text is complete.
4. **TUI board + `@name` + interjects + keys.** Check (pty, two children,
   one asking the parent): board shows `⠹` and `⏳` rows with tool/question;
   `@a hello` → `InputQueued` on `a`, `◆ you → a` in the root; `@b yes` →
   `b`'s `send_message` returns `yes`; Alt+Right lands on `a`; a child
   `Error` interjects red in the root.
5. **Signal.** `/agents` from `list_agents`, `@name`, `/cancel <name>`.
6. **Docs.** §9 edits, in the same commit as step 4 (12 §0 rule).

## 8. Rejected

- **Interleaving children's streams into the parent transcript** (oh-my-pi
  collapsed task cards, updated live). Readable at one child; at three the
  parent's own words are pushed off screen every second. The board is the
  live line; the card comes once.
- **A right-hand tree pane in the TUI.** §1.
- **`SubagentState` event / server-side board.** §1 — derived state on the
  wire drifts from the primitives, and every frontend already receives them.
- **`focus` as server state** (one "current agent" per client tracked by
  `Agents`). Frontends differ (Signal has none); state belongs where it is
  rendered.
- **Auto-focus on child approval / on child error.** Yanks the editor.
  Errors interject in the parent instead.
- **Numbered agents (`1`–`9` to switch).** Digits are typed constantly;
  names are already the model's handle and are unique per parent.
- **Human replies bypassing the waiter (always queue).** Leaves the child
  blocked up to `reply_timeout_secs` with the answer sitting in its queue.
- **Broadcast steer (`@all`).** No transcript has needed it; one
  `spawn_subagent` `task` carries shared context (01 §2.4).
- **Child transcripts mirrored into the parent's history in sqlite** (so
  `--resume` shows everything). The store already has both; `load_session`
  per child is the read.

## 9. Amendments when shipped

- `00-architecture.md` §7: `subscribe_events(session?, descendants?)`; §8:
  the board paragraph and `@name` after the peer-cards paragraph; §12 known
  gaps: strike "no per-child observability".
- `01-config-and-subagents.md` §2.7: replace the `/agents`/`/focus` sentence
  with a pointer here; `list_children` → `list_agents`; §2.8: cancel fan-out
  is recursive.
- `09-signal.md` §8: `/agents` columns, `@name`, `/cancel <name>`.
- `06-acp.md` §3.6: ACP children appear on the board with `Running`/`Idle`
  only (no `Awaiting`: they have no `send_message`, §3.8); `Approval` via
  `ExternalApproval` is shown the same way.

## 10. Not in this doc

Permits and `max_runtime_secs` for follow-up child turns (today spawn-turn
only, `supervisor.rs:876-897`); eviction of idle session actors (00 §12);
an MCP back-channel so ACP children can message their parent (06 §3.8);
`deltas: false` on `subscribe_events` for a GUI that only wants the tree
(add when a measured client asks for it).
