# 207 — ATTENTION-FLAT-LIST: one scored list, dismiss that leaves it to the agent, session-stopped cards

Status: decided 2026-11-21 (Sky, rulings 1–11 below), written by W0 on `feat/207-attention` off main `729093ffc`. Number 207 was free: no `docs/design/20[7-9]*` on any branch (`git log --all -- 'docs/design/20[5-9]*'` shows 205 and 206 only), no `feat/207-*` branch but this one. Round 1 of the attention redesign (plan sections 1, 2, 5, 7, 8): builders A (engine, `crates/roster` and the request owners) and C (desktop UI, `components/attention`) work in parallel against the frozen contract `/tmp/theuth-207-contract.md`; the graph (plan sections 3–4) is round 2. Every `path:line` below was re-opened in this worktree.

## 0 Decisions in one screen

Sky's rulings (binding):

| # | Ruling | Lands in |
|---|---|---|
| R1 | One flat list, no sections or groups, ordered by an engine-computed score; every user-input request is in it | §3, §6 |
| R2 | Nothing is hidden by tier; base score per kind: asks that stop an agent 600, mesh notices 450, yours-to-do 300, shown 120, session stopped 150 failed / 80 otherwise, answered by double 20 | §3.1 |
| R3 | Live session +300; blocks 40 each max 200; deadline 0–250 under 50 % remaining; risk 100 / 30; task 0–100; age 1/min max 60; weights in config; every item carries `score` and `reasons` | §3 |
| R4 | One "session stopped" item per session for every stop, with reason, summary, next steps, a reply box that starts the next turn; expires 45 min after the stop (config) | §4 |
| R5 | Every item carries `project` (via lineage), `parent_session`, `root_session`, a display path, `risk`, `deadline_ms`, `blocks`, `session_live`, `expires_ms` | §2 |
| R6 | Dismiss is not a refusal and never wakes a session; per-kind semantics | §5 |
| R7 | Bulk dismiss / snooze by ids or filter in one RPC with per-id outcomes; allow/deny N by looping the existing RPC | §5.3 |
| R8 | Search, sort, kind chips, project filter, multi-select, keyboard, virtualised, answer on the card, stacking, snooze presets | §6 |
| R9 | `AttentionChanged` carries the changed items; clients patch, never refetch per event | §7 |
| R10 | Fix the alert classifier for home and autoresearch items; Source items stop hard-coding rank | §8 |
| R11 | Out of scope: graph, new sources, Swift/C# companions, cross-engine live state | §12 |

W0's calls where the rulings left a choice (each marked "(W0)" where it is made):

- **D1** No new `AttentionKind`. "Session stopped" extends `finished` with a `stop` field. Reason: the wire classifier refuses a new output enum variant (`crates/rpc-core/src/compatibility.rs:380-384`, "output variant added without a documented consumer fallback" → `ReviewRequired` → `wire_schema_snapshot.rs:19` fails), and companions already decode `finished`.
- **D2** Base by kind with three id-prefix overrides: `autoresearch:launch:*` (kind `blocked`) is a stop-ask (600); `resume:boot:*` is a notice (120); a budget at 80 % that is not paused is a notice (120). Notices are not in Sky's table; 120 = "shown".
- **D3** The live bonus applies to every kind except `double` (its session's pending approval is already an item) and `finished` (a stopped session is not live by construction).
- **D4** Task points only for items with a task priority (`weight × 10`, +10 when the task's project is in `tasks.project_order`); items without a task get 0, not the old "medium" 5.
- **D5** `blocks` = sessions transitively waiting on the item's session through `AgentState::Awaiting` edges (`send_message{await_reply}`), local live rows only.
- **D6** Session-stopped cards for root and desk sessions only (130 E3's rule: a subagent's parent reports for it); a new turn on the session removes its card.
- **D7** Dismissing a decision on an idle task keeps the task `blocked` and swaps the blocker `decision:<id>` → `left:decision:<id>`; moving it to `todo`/`doing` without a wake would leave a `doing` checkout for the orphan sweep (`crates/tasks/src/ledger.rs:140-180`) to resume — a wake by another route.
- **D8** A dismissed approval is reported on the wire as `decision: deny` plus a new `dismissed: true` flag, not a new `Decision` variant: `Decision` is in `ApprovalResolved`, which the companions decode.
- **D9** The UI sends explicit ids for bulk actions (exactly what the user sees); the filter form exists for clients without the list and carries `before_ms` so it never touches an item that arrived after the caller looked.
- **D10** Each `AttentionChanged` carries `order` (every id in engine order), so clients never sort by a cached score that drifts with age and deadline.
- **D11** Keyboard: `E` dismisses (A, D, S, X, J/K, `/` as ruled); `Esc` clears search, then selection.

## 1 Problem, with evidence

1. **Thirteen sections, one order nobody shares.** The rail groups rows by kind in a fixed order with an Urgent group on top (`crates/desktop/ui/src/components/attention/model.ts:9-71`); the engine orders by task rank, then waiting time, then age (`crates/roster/src/attention.rs:971-982`); every non-task item ranks 5, so an approval that stops a running agent sorts with a shown artifact. Rows carry no buttons (`components/attention/Row.tsx:21`), so every answer is two clicks.
2. **Nothing says why an item is where it is.** `rank` is a task number; deadlines exist (`questions.deadline`, `crates/tools/src/journal.rs:197-221`; approval timeout 300 s, `crates/tools/src/approval.rs:297-305`) but are not on the item; the risk of an approval is on `ApprovalRequested` (`approval.rs:44-55`) and never read by `derive`.
3. **Dismiss does different things by accident.** `attention_dismiss` only writes `attention_state.dismissed_ms` (`attention.rs:1757-1780`, `:2067-2081`); the request behind it waits for its own timeout: an approval denies itself after 300 s as "approval timed out", a secret request errors after 1800 s, a hand-off after 1800 s. An ask item cannot be dismissed at all — `derive` stamps it `at_ms = now_ms` on every pass (`attention.rs:502`), so `dismissed_ms >= at_ms` is false on the next derive — and the UI's ask "Dismiss" is `answer_ask(id, null)` (`components/hud/queries.ts:216-224`), which **cancels the session's turn** (`crates/agent/src/lib.rs:557-567`). A decision dismissed in the inbox leaves its task blocked forever with no trace for the agent.
4. **Failures are invisible.** `attention_finished` is written only for a root's `end_turn` (`attention.rs:1892-1902`, `:1546-1552`); an errored, cancelled or timed-out turn writes nothing, and other providers' clean stop reasons (`stop`, `max_tokens`) write nothing either.
5. **No project, no lineage.** A row says `from <session title>`; the project and the tree it sits in are a click away.
6. **Every event refetches the whole list.** `useAttention` invalidates on eleven topics (`components/hud/queries.ts:37-58`); `AttentionChanged` carries only id diffs (`attention.rs:172-192`). 176 measured multi-second stalls from exactly this pattern.
7. **Two alert bugs, confirmed.** `class_of` matches kind `Home` on `own("home:")` (`crates/roster/src/alert.rs:59`), but home items are `node:<engine>` (`attention.rs:791`) and `misplaced:<task>` (`:817`): they never alert. Autoresearch items are kinds `Review`/`Blocked` with ids `autoresearch:*` (`crates/autoresearch/src/actor.rs:5703-5790`), so `own("review:")`/`own("blocked:")` fail and a study's launch press never alerts. Source items hard-code `rank: 5` (autoresearch `actor.rs:5687`, double `crates/double/src/actor.rs:811`) or 4 (contacts `crates/a2a/src/contacts.rs:3107`).

## 2 Data model

### 2.1 `AttentionItem` (additive)

All new fields are `#[serde(default)]`: old engines omit them, old clients ignore them, and the classifier rates them `Additive` because the receiver view does not require them (`compatibility.rs:289-295`).

| Field | Type | Meaning |
|---|---|---|
| `score` | `u32` | Sum of the reasons' points (§3). |
| `reasons` | `Vec<AttentionReason>` | `{signal, points, text}`, points desc; zero-point signals omitted, `base` always present. |
| `risk` | `Option<tools::Risk>` | Approvals: `ApprovalRequested.risk`. |
| `tool` | `Option<String>` | Approvals: the tool name (search, stacking). |
| `deadline_ms` | `Option<u64>` | When the request resolves itself: approval/ask/hand-off from the journal row's `deadline`, else arrival + the configured timeout; secret = `created_ms + secrets.request_timeout_secs`; double in grace = the grace end. `None` for decisions and everything else. |
| `expires_ms` | `Option<u64>` | When the item leaves the list on its own: `finished` = stop + `roster.attention.stopped_ttl_mins`. |
| `blocks` | `u32` | Sessions stopped behind this item (D5). |
| `session_live` | `bool` | The item's session is `running`, `awaiting` or `approval` right now (`AgentState`, `crates/agent/src/lib.rs:1866-1885`, via `ListActivity`), or `SessionSummary.running` for a session another engine runs. Task-born items use the checkout session only, never the `task_events` fallback. |
| `project` | `Option<String>` | `SessionSummary.project` of the item's session — the store already resolves the root's label for every row (`root_col!`, `crates/agent/src/store.rs:473-507`); a task-born item without a known session falls back to `task.project`. |
| `root_session`, `parent_session` | `Option<String>` | From `SessionSummary.parent`, walked in memory over the `ListSessions` reply `gather` already fetches (`attention.rs:1364-1366`). |
| `path` | `Vec<String>` | Display path, root first: root title (desk: `<agent> desk`; empty title: short id), then each child's peer name. |
| `stop` | `Option<AttentionStop>` | `finished` items: `{outcome: finished\|failed\|cancelled\|timed_out, reason: <raw stop_reason>, error}`. |

`AttentionKind` is unchanged (D1). `rank` keeps its 132 meaning for old clients; nothing in the engine sorts by it any more.

### 2.2 `attention_finished` (roster migration 0008)

```sql
ALTER TABLE attention_finished ADD COLUMN outcome TEXT NOT NULL DEFAULT 'finished';
ALTER TABLE attention_finished ADD COLUMN stop_reason TEXT NOT NULL DEFAULT 'end_turn';
ALTER TABLE attention_finished ADD COLUMN error TEXT;
```

Engine-local like 0006; not in roster's replicated set (`crates/roster/src/lib.rs:3840`: `agents, budget, handoffs, engine_agents`). **No replicated `sessions` column is added**; `SESSIONS_WIRE_SCHEMA` (70, `crates/agent/src/store.rs:732`) is untouched. Packets gain an optional JSON field (`left_to_agent_ms`) inside `packets.json`: no tasks migration.

### 2.3 Config (`roster.attention`)

`RosterConfig` (`crates/roster/src/config.rs`) gains `attention: AttentionConfig` with `stopped_ttl_mins: 45`, `rescore_secs: 60` and `weights` (every number of §3). Read on every recheck through `rt.section("roster")` like `alert_delay_ms`; the runtime's config is a boot snapshot (no watcher, `crates/theuth/src/main.rs:1595-1598`), so a weight change needs an engine restart, not a rebuild.

## 3 Scoring

`score = base + live + blocks + deadline + risk + task + age`, integers, computed in `derive` at `now_ms` for every item including Source items.

### 3.1 Signals (defaults)

| Signal | Points | Rule |
|---|---|---|
| base | 600 / 450 / 300 / 120 / 150 / 80 / 20 | stop-ask: approval, ask, secret, handoff, decision, resume (not `resume:boot:`), contact_first, `autoresearch:launch:` · mesh: node, home · yours: review, hire, blocked, budget paused · shown: show · notice (D2): budget at 80 %, `resume:boot:` = 120 · finished: failed or timed out 150, else 80 · double 20 |
| live | 300 | `session_live`, except `double` and `finished` (D3) |
| blocks | 40 × n, max 200 | D5 |
| deadline | 0–250 | `start = waiting_since_ms ?? at_ms`; `share = (deadline − now) / (deadline − start)`; `share ≥ 0.5 → 0`; `now ≥ deadline` or no span → 250; else `round_half_up(250 × (1 − 2 × share))` |
| risk | 100 / 30 / 0 | destructive / mutating / read-only or none |
| task | 0–100 | items with `priority`: `min(100, weight × 10 + (rank ≥ 100 ? 10 : 0))` (weights 9/7/5/2, 132's `rank()`); none → 0 (D4) |
| age | 0–60 | `min(60, floor((now − (waiting_since_ms ?? at_ms)) / 60 000))` |

Order: `score` desc → `waiting_since_ms` asc (None last) → `at_ms` desc → `id` asc. The last key makes the order total, so every client shows the same list.

Reason texts (snapshots at derive time; the UI re-renders countdowns from `deadline_ms`): base = the kind's word ("approval", "question", "machine offline", "failed", …); live = "agent waiting on you" for stop-asks, else "session running"; "blocks 3 agents"; "denies itself in 1:42" (approval), "double answers in 0:14", "times out in 12:05" (the rest); "destructive" / "mutating"; "high priority" (+ " · ranked project"); "waiting 12 min" ("stopped 12 min ago" for `finished`, "12 min ago" without `waiting_since_ms`).

### 3.2 Worked example

`builder` (a child of "Ship 2.5 release", project `release-2.5`, ranked second in `tasks.project_order`) asks to run `kubectl delete deployment relay-canary -n prod` (destructive). It arrived 3 min ago; the approval times out after 300 s, so 2 min remain. Its parent and its own child `linter` are both in `awaiting` on it. Its session checks out a `high` task.

| Signal | Computation | Points |
|---|---|---|
| base | approval | 600 |
| live | `builder` is in `approval` | 300 |
| risk | destructive | 100 |
| blocks | 2 × 40 | 80 |
| task | 7 × 10 + 10 (ranked project) | 80 |
| deadline | share = 120 / 300 = 0.4 → 250 × (1 − 0.8) | 50 |
| age | 3 min | 3 |
| **score** | | **1213** |

Reason line on the card: "blocks 2 agents · denies itself in 2:00". In the same list a review of a `high` task in the first ranked project, waiting 30 min, scores 300 + 80 + 30 = 410; an unreachable engine seen 3 h ago 450 + 60 = 510; a cancelled session 20 min ago 80 + 20 = 100. The 34-item fixture (contract §9) is this table end to end.

## 4 Session stopped

- **When.** `TurnFinished` for a root or desk session (`reports_finished`, `attention.rs:1546-1552`, unchanged; D6), any `stop_reason` except `interrupted` and `suspended` (the resume path owns those): `error` → failed, `cancelled` → cancelled, `timed_out` → timed out, anything else (`end_turn`, `stop`, `max_tokens`, …) → finished. The existing skip when an approval or ask is still pending stays.
- **What.** One row per session (`ON CONFLICT(session)` already replaces); `summary` and `next_steps` from `parse_finished` as today; for a failed turn whose text is empty the summary falls back to the turn's last `AgentEvent::Error` message (≤ 160 chars), also stored in `error`.
- **Leaves.** At `at_ms + stopped_ttl_mins` (default 45; the coarse tick deletes the row), on dismiss (hidden, versioned as today), and on `TurnStarted` for the session (the row is deleted: the session is live again — the reply box's own turn does this).
- **Reply box.** The card's input sends `send_input(session, text, [], null)` (`crates/agent/src/rpc_turns.rs:34-41`, trait `TurnsApi`, owner `agent`, TS `method.agent.*`): the one action in the list that deliberately starts a turn. `send_message` is peer-to-peer (`crates/agent/src/rpc_messaging.rs:80-93`) and not used. Next steps render as chips that fill the input.

## 5 Dismiss

### 5.1 Rule

Dismiss tells the agent "the user leaves this to you". It never wakes a session: no wake row, no user message, no `send_input`. A request that a live tool call is parked on is answered in that call (the turn is already running, so that is not a wake). A request nobody is parked on is recorded where the agent will see it the next time something else wakes it.

### 5.2 Per kind (today → 207)

| Kind | Today | 207 | Bulk outcome |
|---|---|---|---|
| approval | row hidden; the call waits until `tools.approval_timeout_secs` (300) and gets "`<tool>`: approval timed out" | new `tools::approval::Dismiss{id}`: the call is not run; tool result "`<tool>`: not run — the user left this call to you without approving it. Do not retry it as is; find a way that needs no approval, or say in your reply why it is needed."; `ApprovalResolved{decision: deny, timed_out: false, dismissed: true}`; journal answer `"dismissed"`, replay honours it the same way; the double takes no label; activity row "left approval … to the agent (not run)" | `not_run` |
| ask | not dismissable (`at_ms = now`); the UI's Dismiss cancels the turn | new `tools::LeaveAsk{id}`: the tool returns **ok** "ask_user: the user left these questions to you — decide yourself, say in your reply what you chose, and continue." plus the `{id: null}` map; `AskResolved{dismissed: false, timed_out: false, left_to_agent: true}`; no cancel; journal answer `{"left_to_agent": true}`; the double takes no label | `left_to_agent` |
| decision, live (`escalate` still inside `roster.decision_wait_secs`) | row hidden; the task stays blocked | new `tasks::LeaveDecision`: the waiting `escalate` returns "decision: left to you — the user chose not to decide. Pick the option you would recommend, record it with task_note, and continue."; the blocker is dropped and the task returns to `doing`/`todo` **without** a wake; the escalate ask card is dropped | `left_to_agent` |
| decision, idle (parked) | as above | `tasks::LeaveDecision` with no live waiter: blocker `decision:<id>` → `left:decision:<id>` (D7), the task stays `blocked`, no wake; thread line "**decision** (`decision:<id>`): left to the agent — the user did not decide; choose yourself"; packet `left_to_agent_ms`; the desk's wake text for that task gains "The user left `decision:<id>` to you: choose yourself, record what you chose with task_note, then remove the `left:decision:<id>` blocker with task_update." on every later wake until the blocker is gone. `left:` blockers make no item, so the item leaves the list | `left_for_later` |
| secret | row hidden; the tool errors after 1800 s "no answer after 1800 s" | new `secrets::Waive{id}`: the replicated row's outcome `proceed_without` (beside `stored`/`declined`/`timeout`); the polling tool (on any engine) returns **ok** "no secret is coming: the user left this to you — proceed without <names>, and say in your reply what you could not do." | `proceed_without` |
| handoff | row hidden; the tool waits until its timeout | `handoff::resolve(id, "proceed_without")` (beside `done`/`cancelled`/`timeout`); `handoff_resolve` still accepts only `done`/`cancelled` from clients; tool result **ok** "nobody is coming to the desktop: the user left this to you — proceed without it (after <n> s)" | `proceed_without` |
| resume, contact_first, review, hire, budget, node, home, blocked, show, finished, double, autoresearch:* | row hidden (119, versioned) | unchanged: row hidden (119) | `hidden` |

A request already resolved elsewhere answers `gone`; a resolver error answers `failed` with the message and writes nothing. Hidden kinds write `attention_state`; resolved kinds do not (their item leaves because the request did).

**Open question Q1 (owner):** a dismissed approval could go to the double instead of "not run" (the double answers at its level for that class, or the call is not run when it has no level). Not built; one branch in `Dismiss` if chosen.

### 5.3 Bulk

`attention_dismiss_many(target)` and `attention_snooze_many(target, until_ms)` on `BoardsApi` (owner `roster`): `target` is `{ids}` or `{filter: {kinds, projects, before_ms}}`, exactly one; per-id `AttentionOutcome` in `ids` order (filter: engine order); one recheck and one `AttentionChanged` after the batch. `attention_dismiss(id)` becomes `attention_dismiss_many({ids: [id]})` (same signature; the per-kind semantics now apply to old clients too). Allow / Deny N: the UI loops `resolve_approval(id, decision)` (owner `tasks`, `crates/tasks/src/rpc_hub.rs:18-19`), at most 6 in flight; no new RPC.

## 6 UI

- **Shell.** `/attention` keeps master–detail. The rail widens to 440 px and holds one virtualised list (`@tanstack/react-virtual`, measured rows, the `SessionList.tsx:338-477` pattern) of cards in engine order; the detail pane stays for kinds that need reading (decision, review, show, ask with more than one question or free text, approval arguments over 3 lines, hand-off, secret forms) and gains a "Why here" block: score and every reason with its points.
- **Toolbar.** Search (`/`), sort menu (Priority · Expiring soonest · Oldest waiting · Newest · By agent · By project), kind chips with counts (any combination; unknown kinds under "Other"), project select (All · each project · No project). A count line "34 items · 12 shown".
- **Card.** Line 1: kind chip (`finished` shows the outcome word), stack count, priority chip, the reason line on the right (the first two present of blocks · deadline · risk · live · task when urgent or high · age; the deadline re-rendered every second). Line 2: title, two lines. Line 3: detail (approvals: the command in mono). Line 4: `project · Root › child · agent · age`. Inline actions: approval Allow / Deny (stack: Allow all N / Deny all N), ask options as buttons for a single-choice question, decision answer field, secret "Enter secret…" expanding the existing `SecretRequestCard`, hand-off Done / Cancel, hire, review, budget, home actions as today (`KindActions`, moved out of `hud/Hud.tsx`), session-stopped summary + next-step chips + reply box. Snooze (menu: an hour · tomorrow 09:00 · a week, `hud/model.ts` `snoozeUntil`) and Dismiss on every card; a selection checkbox on hover and whenever a selection exists.
- **Stacking.** Approvals with the same `session` and `tool` collapse into one card at the position of their highest member; the list stays flat underneath.
- **Selection and bulk bar.** Click toggles, Shift-click ranges (`lib/selection.ts` `nextSelection`); the bar offers Dismiss N · Snooze N · Allow N · Deny N (the last two only when every selected item is an approval) · Clear; a toast summarises outcomes ("12 dismissed · 3 left to agents · 2 not run · 1 already gone").
- **Keyboard** (list focused, not typing): J/K move, Enter opens the detail, A allow, D deny, S snooze until tomorrow 09:00, E dismiss (D11), X select, Shift+X range, `/` search, Esc clears search then selection.
- **Conventions.** No emoji (`lib/no-emoji.test.ts`), lucide icons, shadcn primitives; logic in a pure `model.ts` with vitest tests in the node environment, components tested with `renderToStaticMarkup`.

## 7 Live updates

`AttentionChanged` gains `items` (every item added or changed since the previous event, `alert` blanked), `order` (all ids in engine order), `seq` (1-based per engine run), `epoch_ms` (the run) and `alerts_changed`. The actor publishes after a derive when the id set, the order, any item's content (excluding `score`, `reasons`, `alert`), or any `alert` changed; a tick every `rescore_secs` (60) re-derives, prunes expired stopped rows, and publishes only if something of that list changed — age and deadline drift reorder at most once per tick.

Clients: upsert `items` (keeping their own cached `alert`), drop `ids_removed`, reorder by `order`; refetch `attention_list` only on `seq` 0 (an engine before 207), a gap or new epoch, a length different from `count`, `alerts_changed` (alerts are masked per connection, 155 §7.3–7.5), or a reconnect. Time-dependent display (countdowns, "waiting 12 min", expiry) is computed on the client from `deadline_ms`, `waiting_since_ms` and `expires_ms`; an item past `expires_ms` is hidden locally before the engine's tick removes it. Old clients still refetch on every event; they get somewhat more events (order changes, content changes), at most one per tick from drift.

## 8 Alerts

- `class_of`: `Home` alerts on `node:` and `misplaced:`; `Review` and `Blocked` alert on `autoresearch:` as well as their own prefixes; `Finished` keeps `AlertClass::Finished` for every outcome (no new class: an output variant would fail the classifier).
- `why_of`: finished per outcome ("turn failed: <error>", "turn cancelled", "turn timed out", "turn ended: <summary>"); autoresearch "a study waits for you".
- Source items: `derive` fills `priority`/`rank` from the item's session checkout like every session item when the Source left `priority` empty; the hard-coded 5 / 4 stop mattering (score ignores `rank` without a priority).

## 9 Amendments

- **104** A2: an ask's Dismiss no longer ends the turn — the agent decides (§5.2). A4 (grouped rail) is replaced by the flat list. A10: the UI patches from `AttentionChanged` instead of invalidating on eleven topics (kept only for engines without `seq`).
- **119** stands for hidden kinds. Request kinds (approval, ask, decision, secret, handoff) are resolved, not hidden. The ask item's `at_ms` becomes its first-seen time (`ask_since`), stable across derives; the double's `at_ms` becomes the card's arrival time, stable across derives.
- **130** E1: `AttentionChanged` carries items and order (§7). E3: receipts for every stop outcome, deleted on the next `TurnStarted`, expiring after `stopped_ttl_mins`.
- **132** D4 is amended: the engine's order is the score order (§3), not rank; it is the default sort and every client's Priority view; the client may offer other sorts. The Urgent group goes with the groups; urgency is a task signal and a chip. D10's sixty-row world is replaced by the 207 fixture.
- **155** §7.2: rows for Home and autoresearch items as §8; finished why per outcome. §7.3's "AttentionChanged is unchanged (clients refetch)" is superseded by §7.
- **176**: the list follows its rule (patch, no refetch per event).

## 10 Migration and compatibility

- Wire: only additive fields (`AttentionItem`, `AttentionChanged`, `ApprovalResolved.dismissed`, `AskResolved.left_to_agent`), new types and two new methods. `just wire-schema-accept` after the classifier reports only `Additive`/`Nonbreaking`.
- Old desktop UIs: render the new fields as nothing; their ask "Dismiss" still calls `answer_ask(null)` and cancels the turn; their other Dismiss buttons call `attention_dismiss`, which now has 207's semantics.
- Companions (macOS `crates/desktop/companion/Sources/TheuthKit/Models.swift:9-43`, Windows `Theuth.Companion.Rpc/Models.cs`): unchanged this round; unknown kinds decode to `.other`, unknown fields are ignored; failed and cancelled stops appear in their Done zone as "finished"; their order stays their own until round 3.
- Roster migration 0008 is additive; existing `attention_finished` rows read as `finished` / `end_turn`.
- `SessionSummary` and the replicated `sessions` table are untouched.

## 11 Test plan

Engine (A): scoring table (each signal, caps, rounding, D2–D4 overrides, order with ties) mirrored on the 34-item fixture; lineage map (root, parent, project, path for desk/child/unknown); `blocks` (chains, cycles, unresolvable names); stopped rows per stop reason, `TurnStarted` deletion, TTL prune; per-kind dismissal with live and parked waiters (approval not run with the new text, ask left with ok text and no cancel, decision live/idle, secret, hand-off, hidden kinds, gone, failed); bulk target validation and filter matching (`""` project, `before_ms`); `AttentionChanged` content/order/alert detection and seq; alert classifier rows; wire fixtures and the OpenRPC snapshot.

UI (C): `model.ts` search, sort (all six, stability), kind/project filter, stacking, selection over entries, reason line and countdown, expiry, `applyChange` (upsert, remove, reorder, every refetch trigger), dismiss target; card render per kind from the fixture (renderToStaticMarkup); the stopped card's reply sends `send_input`; bulk bar shows Allow/Deny only for all-approval selections; no-emoji scan.

## 12 Open questions for the owner

- **Q1** Dismissed approval: "not run" (built) or hand it to the double?
- **Q2** Stopped cards for subagents too (D6 keeps roots and desks)?
- **Q3** A stop the user caused (Stop button) still gets a card (80 + age, 45 min) — keep, or skip cancels by the user?
- **Q4** Live bonus on non-ask kinds (a show or review from a running session gets +300): keep, or limit to stop-asks?
- **Q5** Notices (budget at 80 %, restart notice) at 120 — not in the ruling's table.
- **Q6** Weights in config need an engine restart (no hot reload); acceptable?

## 13 Follow-ups (not in this round)

- The graph (plan §3–4): aggregate, `last_event_ms`, message edges, tidy-tree canvas.
- New sources: drafts (need a persisted row), MCP login, failed schedules.
- macOS and Windows companions: engine order, `stop`, project, the new dismiss semantics.
- Cross-engine live state (`session_live` and `blocks` for sessions another engine runs beyond `SessionSummary.running`).
- A "left to the agent" chip on the task page (the thread line and `Packet.left_to_agent_ms` carry it now).
- Hire and budget items still fall back to `at_ms = now` without their task, so their dismissal does not stick (119); give them a stable first-seen time.
