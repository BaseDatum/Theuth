# 19 — Subagents: a picker, a board, and results that always flow up

Status: shipped 2026-09-13 · steps 1–6 of §7 · extends `01-config-and-subagents.md`
§2.7 (events, UI), `00-architecture.md` §7 (RPC), §8 (TUI), `09-signal.md` §8.
Where this doc disagrees with those, this doc wins; §9 lists the edits.
Every file:line below was read on 2026-09-13.

## 0. The problem, in the tree

01 §2.2 item 5 promised: *the human sees children in the TUI roster, can
open one to read its transcript, and can steer it.* What shipped is
`/agents` (a static list) and a `focus` mode — a second notion of "current
session" layered on the first, with its own field, its own Esc semantics and
its own reload path. Between those the human is blind and mute:

| Fact | Where |
|---|---|
| A child's `TextDelta`, `ToolCall*`, `Error`, `Compacting`, `Harvesting` are dropped unless that child is focused. The parent view shows `↳ spawned x` and, minutes later, a result card. | `tui/src/model.rs:1750-1752` |
| `focus` is a parallel current-session: `Model.focus`, `shown() = focus.or(session)`, Esc-with-empty-editor unfocuses, `/agents` always lists the **root**'s children even when a child is shown. | `model.rs:989-990, :1085-1087`, `ui.rs:226-228`, `model.rs:1978-1981` |
| The status line carries one number: `agents: N running`. Nothing says *which*, *doing what*, *for how long*, *blocked on whom*. | `model.rs:2134-2139` |
| A child blocked in `send_message{await_reply}` waits up to 120 s for the parent model. A human who opens it and types is *queued behind the blocked tool call*; the waiter key `(waiter, from)` compares session ids, so a human never resolves it. | `supervisor.rs:928-931`, `peers.rs:175-181` |
| `subscribe_events(parent)` never receives children's events, nor `SubagentSpawned` (its `session()` is the child). A Tauri client must subscribe to everything and filter itself. | `rpc/src/api.rs:770-776`, `lib.rs:761` |
| `ListChildren` is one level, store rows only: no running flag, no depth, no last text. `Cancel` fans out one level. After `--resume`, `/focus` fails until `/agents` runs. | `supervisor.rs:1122-1135`, `:662-672`, `model.rs:2052-2054` |
| `Loaded` has no `name`/`parent`; a child's header shows the model only. | `lib.rs:183-200`, `model.rs:2155` |

What already works and must stay: **any** child turn that finishes delivers
a `<subagent_result>` to the parent — `child_finished` runs on every child
`TurnFinished`, whoever started the turn (`supervisor.rs:491-509, 464-488`).
A human who steers a finished child gets, when that turn ends, a second
result card in the parent, and the parent model reacts to it. That is the
behaviour the whole design leans on; it is not new, it is verified in §7.

The model side is fine and stays as is: children are detached, results
arrive as `<subagent_result>`, peers talk through `send_message`, the roster
is not in the prompt (07 §10). This doc is about the **human** side.

## 1. The shape

Three ideas, no more:

1. **A subagent is a session.** Opening one is a session switch — the
   mechanism `/sessions` already has ("the one you leave keeps running").
   There is one current session, `Model.session`. `focus`, `shown()`,
   `Child`, `children`, the Esc unfocus branch, and `/focus` are deleted.
2. **`/agents` is a picker** over the current session's subtree — running
   and past children alike — with live state on every row. Pick one and you
   are in it: its transcript, its status line, its editor. Type and you steer
   it: `Steer` while it runs, `SendInput` when it is idle or finished, an
   *answer* when it is blocked asking (§2.3). When that turn ends the parent
   gets its result card, as today. **`/parent`** takes you up one level; with
   the default `max_depth: 1` that is the top-level session.
3. **A board** under the transcript shows what is live in the subtree while
   anything is live, so you see every child's state without leaving the
   parent. Board = live, transcript = history — the rule tool cards already
   follow (00 §8 "nothing ever looks frozen").

Principles behind them: **one list of commands** (00 §8) — everything below
is an `rpc::Api` method or an existing `AgentEvent`, so Tauri and Signal get
it for free; **derive, don't duplicate** — no new `AgentEvent` variant, the
board is computed from `TurnStarted/Finished`, `ToolCallStarted/Finished`,
`Error`, `SubagentSpawned`, and a late-attaching frontend asks for one
snapshot (`list_agents`); **scale by rule, not by pane** — N is bounded by
`max_concurrent` (8) per parent but unbounded over a session's life and over
depth, so the board shows what is live and caps rows, the picker shows
everything.

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
    pub started_at: Option<DateTime<Utc>>,  // current turn start; None when not running
    pub finished_at: Option<DateTime<Utc>>, // last TurnFinished; the picker's "3m ago"
    pub last: String,                 // first line of the last assistant text, ≤ 80 chars
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum AgentState {
    Idle,                             // never ran or finished normally
    Running,
    Awaiting { from: String },        // blocked in send_message{await_reply} on `from` ("parent" or a child name)
    Approval { tool: String },        // an ApprovalRequested for this session is unanswered
    Error,                            // last turn ended stop_reason = "error"
    Cancelled,                        // last turn ended "cancelled" (incl. max_runtime timeout)
}
```

Sources, all already held by `Agents`: `running` set (`supervisor.rs:495-504`),
`waiters` keys `(waiter, from)` → `Awaiting` for the `waiter` (`:74`),
`child_turns` gains `started_at: Instant` next to its timer (`:149-152`;
follow-up turns get an entry too, timer-less), store `last_turn_text` for
`last`. `Approval` comes from the bus: `Agents` subscribes to
`ApprovalRequested`/`ApprovalResolved` (`tools/src/approval.rs:38, :56`) and
keeps `HashMap<SessionId, (ApprovalId, String)>` — the same two events the
TUI renders the modal from, no new ask on the registry. `Error`/`Cancelled`
and `finished_at` come from the last `TurnFinished` per session, kept in a
small map the supervisor already sees the event for.

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

- `steer` / `send_input` already take any session id, and a finished child's
  turn already reports to its parent (§0). New rule in `Agents::input`: if
  the target has a pending waiter `(target, _)` — it is blocked in
  `send_message{await_reply}` — the text **resolves the waiter** instead of
  queueing: the child's tool call returns the human's words, and
  `MessageDelivered { session: target, from: "you", text }` is published so
  the transcript shows it. A human answer is the most useful steer there is;
  today it is the one that cannot land.
- `cancel(session)` fans out to every **running descendant**, not one level.
  Idle descendants are left alone (01 §2.8 unchanged).
- `Loaded` gains `name: Option<String>` and `parent: Option<SessionId>` so a
  child's view can head itself and `/parent` knows where to go without a
  roster lookup.

### 2.4 Not added

No server-side "current agent" (a frontend concern; Signal has none); no
`rename_agent` (names are the model's handle; renaming mid-run breaks
`send_message`); no `pause`; no `steer_all`. Model-facing tools unchanged
(01 §2.4 stands: no `list_subagents`, `cancel_subagent`, `wait`).

## 3. TUI

### 3.1 `/agents`: the picker

Same widget as `/sessions` (`ui.rs:362-382`): a list under the editor, ↑↓,
Enter switches, Esc closes. Rows are `list_agents(current)`, running first in
spawn order, then awaiting/approval, then the rest newest-finished first;
depth ≥ 2 indented under its parent:

```
agents  ⏎ open  esc close  (the one you leave keeps running)
● ⠹ grep-logs      running   read_file crates/agent/src/loop_.rs      1m32s
  ⏳ fix-tests      awaiting  parent: "should I also update the docs?"  48s
  ⚠ scout          approval  shell `cargo test -p agent`
     ↳ probe       idle      "Ports 80 and 443 answer; 8080 is …"     3m ago
  ✗ deploy-check   error     model: 429 rate limited                  12m ago
  · summarise      idle      "Three findings: the cache miss is …"    41m ago
```

Row anatomy: glyph · name · state word · detail · age. Detail by state:
running → `preparing <tool>` / `running <tool> <first arg>` / `streaming`
(the words the status line already uses); awaiting → the question's first
line; approval → tool and described args; error → the message; idle →
`last`. Age is elapsed since `started_at` while running, since
`finished_at` otherwise. Glyphs are text, not emoji (00 §8); the name
colour is the peer magenta of `⟵` cards. `/agents` with no children says
`no subagents`. Ctrl+O on a row — no: the picker is a list; the transcript
is one Enter away.

Enter is `Command::Open(id)` — the exact path `/sessions` takes today
(`Load`, replace entries, `model_id`/`effort`/`queued` from `Loaded`). The
session left keeps running; its events are no longer rendered but nothing is
lost — the store has them, `Load` replays them. (Same semantics as
`/sessions`; the one known gap — text mid-stream at the moment you switch
back is shown from its next delta, the earlier part only after the turn
persists — is `/sessions`' gap too and is fixed there if it is fixed.)

### 3.2 In a subagent

The view is a session view. Header: `↳ grep-logs  child of <parent title>
· claude-haiku-4-5 · running 1m32s`; at depth 2: `↳ scout ▸ probe`. The
status line is the child's. Plain input steers the child: `Steer` while it
runs (`InputQueued` card, Alt+Up withdraws, as today), `SendInput` when idle,
finished, errored or cancelled — a new child turn, whose end delivers a fresh
`<subagent_result>` to the parent (§0). If the child is `Awaiting`, the
input answers it (§2.3) and the editor's placeholder says so beforehand:
`answer grep-logs (asked parent: "should I …")`. `/cancel` cancels this
child. `/agents` here lists *this* child's subtree (usually empty →
`no subagents`). `/model`, `/effort`, `/compact`, `/undo` act on the child
exactly as on any session — it is one.

**`/parent`** opens the parent (`Loaded.parent`), i.e. the top-level session
at the default depth; at depth 2 it climbs one level and a second `/parent`
reaches the top. At the top it says `this is the top-level session`. A
`/parent`-then-`/agents` round trip is how you hop between siblings.

### 3.3 The board

A block between the transcript tail and the editor, present only while at
least one descendant of the *current* session is not idle. One row per live
agent, the same row format as the picker minus the age column collapsing to
elapsed, indented by depth, running first then awaiting/approval then
error/cancelled; rows capped at 6, overflow folds into `… 3 more (/agents)`.
Idle children never appear on the board — they are history, the picker has
them.

```
╭ agents ─────────────────────────────────────────────────────────────────╮
│ ⠹ grep-logs      running   read_file crates/agent/src/loop_.rs   1m32s  │
│ ⏳ fix-tests      awaiting  parent: "should I also update the docs?" 48s │
│ ⚠ scout          approval  shell `cargo test -p agent`                  │
╰──────────────────────────────────────────────────────────────────────────╯
```

Rows derive from events routed by session id into `Model.agents:
BTreeMap<SessionId, AgentRow>` (name, parent, state, current tool, last
text line, started_at) — seeded by `list_agents(current)` on open/resume,
updated by `SubagentSpawned`, `TurnStarted/Finished`,
`ToolCallPending/Started/Finished`, `Error`, `ApprovalRequested/Resolved`
for any id in the map (today these are dropped for non-shown sessions,
`model.rs:1750-1752`; now they update a row before the drop). Elapsed ticks
while the frame loop runs (it runs while anything animates). The board is
not focusable and takes no keys; it is a display, like the status line.

What changes in the parent transcript: nothing is added for running
children (the board has them). Two things that are silent today become
one-line interjects, because they must not be silent: a child's `Error`
(`↳ grep-logs error: model: 429 …`, red) and a child's `TurnFinished
cancelled` (`↳ grep-logs cancelled after 10m (max_runtime)`). Results keep
arriving as `⟵ name` cards (01 §2.7). When the board is hidden and idle
children exist, the status line shows `agents: 5 idle`; when rows show, the
count goes (redundant).

### 3.4 Approvals

The modal already carries `[agent: name]` (01 §2.7). It gains `o open` beside
`y`/`n`: answers nothing, opens the child so the human can read what led to
the request, and the modal stays up. (Rejected: auto-opening the child on
its approval — it yanks the editor from under a sentence being typed.)

### 3.5 Open and resume

On `--resume`, on `/sessions` switch, on `/agents`, and on `/parent`, the
TUI calls `list_agents(current)` once and seeds `Model.agents`, so names,
states and the board are right before any event arrives (fixes the
`--resume` blindness at `model.rs:2052-2054`). No private path: Signal and
Tauri make the same call.

### 3.6 Later, if a transcript shows the need

`@name text` in the editor as a steer that does not switch views (answer a
child's question while the parent is mid-stream). The picker + `/parent`
round trip covers it in three keystrokes today; add the shortcut when
someone reaches for it.

## 4. Signal (09)

`/agents` renders `list_agents` for the bound session: `name · state · last
· age` (state word as in §3.1; no glyphs). `/cancel <name>`. No opening of a
child (09 §8 stands: one bound session per chat; a child is
`wth --resume <id>` territory).

## 5. Tauri (18)

Nothing specific: `list_agents(root)` + `subscribe_events(root, descendants:
true)` is a live tree; `send_input`/`steer`/`cancel` per node; `ts-rs` emits
`AgentInfo`/`AgentState`. The picker and board are the terminal projections
of the same data; a GUI shows a sidebar tree with the selected session's
transcript beside it.

## 6. What the model sees

Unchanged. The roster is not in the prompt (07 §10); `Roster` for the
compaction head renders from `list_agents`, same text. A human steer to a
child is an ordinary user message to that child (01 §2.3); a human answer to
an awaiting child arrives as the tool's return value, exactly as the
parent's would. The parent sees a `<subagent_result>` when the steered turn
ends, exactly as after the spawn turn — it does not know a human intervened,
and does not need to: the result speaks for itself.

## 7. Implementation order

Each step leaves `wth` runnable; formatters and the suite once per step.

1. **Store + supervisor.** `Store::descendants(root)` (recursive CTE),
   `AgentInfo`/`AgentState`, `ListAgents` (replaces `ListChildren`; `Roster`
   renders from it), `child_turns.started_at`, last-`TurnFinished` map,
   `ApprovalRequested/Resolved` subscription, human input resolves waiters +
   `MessageDelivered{from: "you"}`, recursive `Cancel`, `Loaded.name/parent`.
   Check (headless, RPC): parent spawns `a`, `b`; `list_agents` shows both
   `running` with `started_at`; `b` asks parent with `await_reply`;
   `steer(b, "teal")` → `b`'s tool output is `teal`, both sessions get
   `MessageDelivered from=you`; after `a` finishes, `send_input(a, "and
   the tests?")` → `a` runs again and the parent receives a **second**
   `<subagent_result name="a">`; `cancel(root)` with a depth-2 child running
   → grandchild `TurnFinished cancelled`.
   **Verified** (headless, claude-haiku-4-5): two children spawned;
   `list_agents` showed both `running` with `started_at`; `b` reached
   `awaiting parent`; `steer(b, "teal")` produced `MessageDelivered from=you`
   on `b` and a `<subagent_result name="b">` containing `teal` in the root;
   `send_input(a, …)` after `a` had finished produced a second
   `<subagent_result name="a">` ("ALPHA AGAIN"); the descendants
   subscription received the children's `TextDelta` and both
   `SubagentSpawned`; `load_session(b)` returned `name=b`, `parent=root`.
   Not exercised: depth-2 cancel fan-out with a real model.
2. **RPC.** `list_agents`, `subscribe_events(session, descendants)`, remove
   `list_children`. Check: a subscription on the root with `descendants:
   true` receives the children's `TextDelta` and the `SubagentSpawned`;
   without it, neither (today's behaviour).
3. **TUI: delete focus, add the picker and `/parent`.** Remove `Model.focus`,
   `shown()`, `Child`/`children`, `/focus`, the Esc unfocus branch; `/agents`
   → `Menu::Agents` picker → `Command::Open`; `/parent`; child header from
   `Loaded`. Check (pty): `/agents` lists a running and a finished child;
   Enter on the finished one shows its transcript; typing starts a new child
   turn; `/parent` returns to the root, which shows the new `⟵ name` card
   when the child ends; Esc with an empty editor no longer switches anything.
4. **TUI: board + interjects + approval `o`.** Check (pty, two children, one
   asking the parent): board shows `⠹` and `⏳` rows with tool and question;
   opening the awaiting child shows the answer placeholder and typing
   returns the text to its `send_message`; a child `Error` interjects red in
   the parent; a child's approval modal `o` opens it.
5. **Signal.** `/agents` from `list_agents`, `/cancel <name>`.
6. **Docs.** §9 edits, in the same commit as step 3 (12 §0 rule).

Deviations: `Model.agents` is a `HashMap<SessionId, AgentInfo>`, not a
`BTreeMap` (`SessionId` has no `Ord`); `AgentInfo` has no `turns` field
(dropped as unused).

## 8. Rejected

- **Focus** (a second current-session with its own Esc and reload). Two
  notions of "where am I" for one screen; the picker + `/parent` is the
  `/sessions` model applied to the tree, nothing new to learn.
- **Interleaving children's streams into the parent transcript** (oh-my-pi
  collapsed task cards, updated live). Readable at one child; at three the
  parent's own words are pushed off screen every second. The board is the
  live line; the card comes once.
- **A right-hand tree pane in the TUI.** 25 columns of an 80–120 column
  terminal taken from markdown for a list that is empty most of the time.
  Tauri gets the tree pane; the API here is what it needs.
- **`SubagentState` event / server-side board.** Derived state on the wire
  drifts from the primitives, and every frontend already receives them.
- **A server-side "current agent".** Frontends differ (Signal has none);
  state belongs where it is rendered.
- **Auto-opening a child on its approval or error.** Yanks the editor.
  Errors interject in the parent; approvals offer `o`.
- **Numbered agents (`1`–`9` to switch).** Digits are typed constantly;
  names are the model's handle and unique per parent; the picker is ↑↓⏎.
- **Human replies bypassing the waiter (always queue).** Leaves the child
  blocked up to `reply_timeout_secs` with the answer sitting in its queue.
- **Suppressing the result card for human-steered child turns.** The parent
  model should react to what its child did, whoever asked for it; the human
  chose to steer *inside the parent's task*. Wanting a private side
  conversation with a child is a new session, not a child.
- **Child transcripts mirrored into the parent's history in sqlite** (so
  `--resume` shows everything). The store has both; `load_session` per child
  is the read.

## 9. Amendments when shipped

- `00-architecture.md` §7: `subscribe_events(session?, descendants?)`; §8:
  picker/board/`/parent` paragraph after the peer-cards paragraph; §12 known
  gaps: strike "no per-child observability".
- `01-config-and-subagents.md` §2.2 item 5 and §2.7: "focus" → picker +
  `/parent`, pointer here; `list_children` → `list_agents`; §2.8: cancel
  fan-out is recursive.
- `09-signal.md` §8: `/agents` columns, `/cancel <name>`.
- `06-acp.md` §3.6: ACP children appear in the picker/board with
  `Running`/`Idle`/`Approval` only (no `Awaiting`: they have no
  `send_message`, §3.8).

## 10. Not in this doc

Permits and `max_runtime_secs` for follow-up child turns (today spawn-turn
only, `supervisor.rs:876-897`); eviction of idle session actors (00 §12);
an MCP back-channel so ACP children can message their parent (06 §3.8);
`deltas: false` on `subscribe_events` for a GUI that only wants the tree
(add when a measured client asks for it); replaying in-flight stream text on
a session switch (`/sessions` and this picker share it).
