# 155 — ACTIVITY: a stalled child wakes its parent, waits on state, terminal busy/quiet, alert rules

Status: contract for design 155 ACTIVITY, written by worker C from Sky's rulings D1–D10 (`docs/design/155/rulings.md`) on branch `integrate/155`, base `origin/main` 6aadda9a plus 0ff16c99 (recon sheets). Built by three omp ACP workers (W1 ENGINE-ACTIVITY, W2 PTY-SIGNALS, W3 ALERTS) against the frozen signatures in §9, which the `155 C: skeleton` commit already compiles. Every `path:line` below was re-opened in this worktree.

## 0 Decisions in one screen

Sky's rulings (fixed, `docs/design/155/rulings.md:9-18`):

| # | Ruling | Lands in |
|---|---|---|
| D1 | One engine activity monitor per ACP child (+ PTY signals) feeds three consumers | §3, `crates/agent/src/activity.rs` (new) |
| D2 | A stalled ACP child wakes its **parent only**, like `<job_idle>`; no attention item | §4 |
| D3 | Stall = no update within 60 s of a delivered prompt, or 180 s mid-turn silence with no tool call in progress; config; one wake per episode | §3.3 |
| D4 | `wait_peer(to, until, timeout)` — pinned to the child's identity, `not_running` when it goes away, non-blocking, durable | §5 |
| D5 | PTY busy/quiet from generic signals only: screen, process tree, CPU, reaction to input; `<job_idle>` carries `why` | §6 |
| D6 | Alerts: engine decides *whether*, clients decide *how*; only needs-you and unseen-finished; never for the session in view; short re-checking delay; done ≠ idle per device; every alert says why | §7 |
| D7 | Capability entry per harness, as data (`reports: {permission, turn_end, session_load}`) | §8 |
| D8 | No decision model in v1 | §12 |
| D9 | Out of scope: process survival, per-harness screen rules, the model deciding state, worktrees, network signals | §12 |
| D10 | Built by omp ACP workers, contract-first | §10 |

C's calls (where the rulings left a choice; each is marked "(C's call)" where it is made):

- **C-a** `wait_peer` is journaled as a `waits` row of a new kind `peer_state` that outlives its turn exactly like `child_result` (139 Q8), not as a reminder; boot re-arms it from a `Scope::Here` scan (§5.4).
- **C-b** The pinned identity is the child's `SessionId`. A respawn after an engine restart keeps the id and keeps the wait; a new child under the same name is a new id and never satisfies it (§5.2).
- **C-c** `blocked`/`idle`/`done`/`stalled` are computed by one pure function `peer_state_of` in `crates/agent/src/activity.rs` from the supervisor's existing maps plus the monitor; precedence blocked > stalled > done > idle > running (§5.1).
- **C-d** The stall monitor is a pure struct (`ChildActivity`) with explicit `now: Instant` arguments; the actor arms one `Stall` timer at the next deadline. Tests use a fake clock by passing instants (§3.4).
- **C-e** `wait_peer until=idle|done` on a harness whose entry lacks `turn_end` is refused at registration (`harness does not report turn end`) instead of hanging forever (§8.2).
- **C-f** The alert's *whether* splits in two layers: roster computes `alert` (class, why, episode id, delay) for every connection; rpc masks it per connection for focus and for the finished-seen set, because focus (129) and device identity exist only at the connection (§7.3–7.5).
- **C-g** The finished-seen set is keyed by the rpc connection id (`RpcPeer` carries no device id); it lives in memory on the rpc `Server` with `FocusMap` and dies with the connection or the engine — a reconnect re-shows unseen finishes once (§7.5).
- **C-h** `alert_delay_ms` is a `roster:` key (`roster.alert_delay_ms`, default 2000), since the attention actor already reads `rt.section("roster")` (§7.4).
- **C-i** PTY thresholds are two static `tools.pty_*` keys (`pty_reaction_secs` 10, `pty_tree_cpu_pct` 2), wired like `pty_shell` as one `PtyThresholds` value, not live knobs like `pty_idle_secs` (§6.6).
- **C-j** The tree sampler is `platform::ProcessTree::sample` (the Windows Job Object accounting is already there; unix walks descendants with `libc`); `crates/tools/src/proctree.rs` holds only the quiet rule. No new crate: nothing in the workspace depends on `sysinfo`/`libproc`/`procfs`/`nix` (§6.2).
- **C-k** `Reports` for an entry without a `reports:` block comes from a built-in table keyed by command basename (omp, opencode, claude-agent-acp, codex-acp); unknown → all false (§8.1).
- **C-l** Attention kind → alert class: needs_you = Approval, Decision, Hire, Blocked, Review, Handoff, Secret, Ask, Resume, Budget-paused; finished = Finished; none = Budget-warning, Node, Show, Double (§7.2).
- **C-m** The stall wake carries no attention item and no `AgentEvent`; the only observable outside the parent is the `MessageDelivered` event `inject` already publishes (§4.2).

## 1 Problem and evidence

The waves of the last two weeks ran 5–8 omp children over ACP per orchestrator. What the orchestrator cannot see today (all cites re-opened here):

1. **A child that stops sending updates looks exactly like one that is working.** The child actor publishes `TextDelta`/`ToolCallStarted` as updates arrive (`crates/agent/src/acp_child.rs:1397-1490`) and the roster shows `Running` while a turn is open (`crates/agent/src/supervisor.rs:6033-6034`), but nothing measures the time since the last update. The only timer on a turn is the continuation quiet period of 120 s (`crates/agent/src/acp_child.rs:76-78`), which closes a turn that was already over. The seam map records the cost: "omp PTYs never wake you on completion — poll with reminders" and six manual recoveries in nine days (`docs/design/155/recon-theuth-seams.md:97-102`, `:719-721`).
2. **There is no wait for a state.** `send_message{await_reply}` waits for a reply (`crates/agent/src/peers.rs:496-520` journals it as `WaitKind::PeerReply`), `wait(id, pattern)` for a regex (`crates/tools/src/pty.rs:2513-2534`), `remind_me_in` for time. Nothing waits for "that child is idle / blocked / done" (`docs/design/155/recon-theuth-seams.md:20-22`).
3. **A PTY job's only signal is a quiet screen.** `Idle::saw` compares rendered rows (`crates/tools/src/pty.rs:1997-2002`); a `sleep 300` under `bash`, a long compile with no output and a finished program all look the same; a `send()` that the program ignored is indistinguishable from one it is thinking about. The `<job_idle>` wrapper has no `why` (`crates/agent/src/supervisor.rs:3106-3110`).
4. **Notifications are per client and per event, not per decision.** The desktop posts a system notification on every `ApprovalRequested` and on root `TurnFinished` when hidden (`crates/desktop/ui/src/App.tsx:473-488`, `crates/desktop/ui/src/lib/notify.ts:7-12`); the macOS companion classifies by kind alone (`crates/desktop/companion/Sources/CompanionCore/Policy.swift:102-110`). Neither knows whether Sky is looking at that session, neither re-checks after a delay, neither distinguishes "finished and seen" from "finished". herdr's rules — server decides whether, client decides how; only →blocked and background completion; never the active view; delay that re-checks; per-client seen — are the borrow (`docs/design/155/recon-herdr-surface.md:29-34`, `:72-76`).
5. **Harness capability is not data.** `acp::agents::Entry` has `command/args/env/description/model/skills/skills_dir/mcp` (`crates/acp/src/agents.rs:20-54`); whether a harness reports permission requests or returns from `session/prompt` when done is folklore (omp keeps working after its prompt returned — the continuation turn, `crates/agent/src/acp_child.rs:947-969`).
6. **The model cannot decide state.** Zero-shot 42–49 % on labelled screens, 25–32 false "waiting" verdicts on 224 real screens (`docs/design/155/eval-decision-model.md:11-13`, `:21`). Deterministic signals only (D8).

## 2 What exists today

### 2.1 ACP child lifecycle (crates/agent)

- `AcpChild` actor: `harness: Option<Harness>`, `queue: VecDeque<Queued>`, `turn: Option<Turn>` (`crates/agent/src/acp_child.rs:88-120`). An input is queued while the connection is down or a turn is in flight (`:1697-1719`); `next` pops the queue when idle (`:1131-1138`); `start` opens the turn and calls `prompt` (`:856-876`); **`prompt` is the delivery point** — `harness.prompt(text)` returns a future resolved as `PromptDone` (`:879-887`). There is no spawn queue or permit inside the actor; permits are the supervisor's pools and are held before the actor sees the input, so "delivered" = the `prompt` call.
- Updates: `Update::Session { update, .. }` (`:1854-1881`): a turnless update opens a continuation turn if it `continues` (`:936-945`); `translate` turns `AgentMessageChunk`, `AgentThoughtChunk`, `ToolCall`, `ToolCallUpdate` into events (`:1420-1487`); `ToolCallStatus::Completed|Failed` end a call (`:1514-1520`). `Turn.open: Vec<String>` holds calls started and not finished (`:188-189`).
- Turn end: `PromptDone` → `finish(stop, …)` → `PeerTurnFinished` to the supervisor (`:1020-1028`), `TurnFinished`, `TurnReturned` (`:1069-1080`); the supervisor's `child_finished` → `hand_off` delivers the result to the parent and clears the `child_result` wait (`crates/agent/src/supervisor.rs:2055-2135`, `crates/agent/src/supervisor/child_waits.rs:49-57`).
- Permission: `Update::Permission` → `ExternalApproval` through the tool registry (`crates/agent/src/acp_child.rs:1253-1301`); the supervisor reads it as `AgentState::Approval` (`crates/agent/src/supervisor.rs:6019-6020`).
- State words: `AgentState = Idle | Running | Awaiting | Approval | Error | Cancelled` (`crates/agent/src/lib.rs:1477-1497`), derived by `state_of` from `approvals`, `waits`, `running`, `last_stop` (`crates/agent/src/supervisor.rs:6013-6042`); a watch-only child's `ChildState` (`crates/acp/src/harness.rs:169-194`) is folded by `state_of_row` — `RequiresAction → Approval`, `Failed → Error`, else `Idle` (`:6047-6068`).
- Respawn after a restart: `respawn_acp_children` collects mid-prompt children and `child_result` waits (`crates/agent/src/supervisor/acp_resume.rs:116-122`); `Respawn` → `session/load` + a restored note or a composed hand-off (`crates/agent/src/acp_child.rs:1998-2013`, `:1159-1168`); a child that cannot be respawned is reported once as `<child_lost …>` to its parent (`crates/agent/src/supervisor/acp_resume.rs:342-347`).

### 2.2 Inputs that wake a session

- `Agents::inject(to, from, text, wrapped, scope)` is the one path: a remote owner gets `harness_enqueue`, a local session gets an `Automatic` ticket and `route`/`park`, then `MessageDelivered` (`crates/agent/src/supervisor.rs:1481-1545`).
- `<job_idle>`: `Notice::JobIdle { op, event: JobEvent::Idle {…} }` (`crates/tools/src/activity.rs:189-197`) is handled by `Agents::notice` — dropped when the op is retired, capped by `spill::cap`, wrapped by `job_idle(...)`, injected under the op's scope (`crates/agent/src/supervisor.rs:2905-2949`); the wrapper is `<job_idle id command name? quiet secs nudge user_typed? at>` (`:3092-3111`).
- Durable waits: `WaitKind = PeerReply | ChildResult | Job | PtyWait | Question` (`crates/tools/src/journal.rs:214-225`); `WaitRow { session, tool_call_id, kind, target, deadline, payload }` (`:254-267`); `Store::wait_put` / `wait_clear` / `waits_of` / `child_waits(scope)` (`crates/agent/src/store/durability.rs:1232-1276`, `:1392`). Boot: `rebuild_waits_of` re-arms only `PeerReply` rows of open turns (`crates/agent/src/supervisor/waits.rs:215-220`); `child_result` rows outlive their turn, survive compaction (`crates/agent/src/store/durability.rs:185-191`) and are consumed by the respawn pass.
- Peer tools are plain `tools::Tool` impls registered in `supervisor::start` (`crates/agent/src/supervisor.rs:9511-9523`); `discover_peers` returns `PeerInfo { …, activity: Option<PeerActivity> }` with `PeerActivity { state, last_active, turn_secs, jobs, ptys, children }` (`crates/agent/src/communication.rs:337-350`), filled by `peer_activity` (`crates/agent/src/supervisor.rs:5422-5445`).

### 2.3 PTY jobs (crates/tools)

- `Idle { base, last_change, last_rows, last_fire, nudges, backoff }` (`crates/tools/src/pty.rs:1974-1981`); `saw` (`:1997-2002`), `deadline` (`:2006-2019`), `fire` with the `wait` suppression and 61 §1.4 backoff (`:2024-2038`). The job task `run` ticks every `OUTPUT_EVERY` = 500 ms while dirty (`:2080`, `:2113-2120`), fires at `due` and emits `JobEvent::Idle` through `j.activity.notify` and the bus (`:2124-2143`).
- `send` writes, signals, then `settle`s on 300 ms quiet / 2 s max (`:2472-2508`); `reply` touches the detector (`:2621-2622`).
- The PTY child pid and its process group: `Spawned { …, pid, tree: ProcessTree }` from `child.process_id()` and `ProcessTree::adopt(pid)` (`:1811-1819`, `:1847-1852`); `PtyHandle.pid` "== pgid: the child is a session leader" (`:483-484`) and `PtyHandle.tree` (`:486`). `platform::ProcessTree` offers `id`, `signal`, `alive` only (`crates/platform/src/process.rs:21-61`).
- Config: `ToolsConfig { …, pty_ready_timeout_secs, pty_idle_secs, …, pty_shell }` (`crates/tools/src/lib.rs:486-515`, defaults `:517-533`); `pty_idle_secs` is live through `SetPtyIdle` (`crates/tools/src/registry.rs:457-467`, `:817-830`), `pty_shell` is static and only wired through `StartTool::new` (`crates/tools/src/registry.rs:1883-1892`, `crates/tools/src/pty.rs:2687-2701`).
- Dependencies: `libc` on unix (`crates/tools/Cargo.toml:46-47`), no process-info crate anywhere in the workspace (`Cargo.toml:98`, `:255-257`).

### 2.4 Attention and clients (crates/roster, crates/rpc, desktop, companion)

- `AttentionKind` — 14 kinds (`crates/roster/src/attention.rs:39-73`); `AttentionItem { id, kind, title, detail, at_ms, …, waiting_since_ms, summary, next_steps, context, priority, rank }` (`:78-137`); `derive(inputs, states, now_ms)` pure (`:343`); the actor `recheck`s on every list and publishes `AttentionChanged` on a changed id set (`:1299-1309`). `Finished` is "a receipt" (`:70-72`), ids `finished:<session>:<turn>` (`:833`).
- rpc returns roster's struct verbatim: `attention_list -> Vec<roster::attention::AttentionItem>` (`crates/rpc/src/api.rs:2251-2252`, `:7405-7407`), exported to TS by `export_bindings` (`crates/rpc/src/export.rs:470-473`).
- Focus: `set_event_focus(sessions)` keeps `FocusMap = HashMap<peer id, Arc<HashSet<String>>>` per connection, removed when the connection closes (`crates/rpc/src/api.rs:2373-2374`, `:2458`, `:7926-7941`); today it filters only delta events (`:7946-7951`).
- Desktop: `notifyIfHidden` posts when the window is unfocused or hidden, no pause setting (`crates/desktop/ui/src/lib/notify.ts:7-12`); callers for `ApprovalRequested` and root `TurnFinished`, both skipped while the companion is alive (`crates/desktop/ui/src/App.tsx:473-488`).
- macOS companion: `NotificationPolicy { prefs, pausedUntil, … }` with `classify` by kind (`crates/desktop/companion/Sources/CompanionCore/Policy.swift:70-110`), `pause(until:)` (`:199`); `Notifier` ticks at 1 Hz and posts banners (`crates/desktop/companion/Sources/Companion/Notifications/Notifier.swift:13-81`, `:147-154`); the Codable `AttentionItem` lives in `crates/desktop/companion/Sources/TheuthKit/Models.swift:45-106` and decodes unknown kinds as `.other` (`:10-13`).
- Harness entries: `Entry` (`crates/acp/src/agents.rs:20-54`), built from the public registry by `entry_from_distribution` / `install` (`crates/acp/src/registry.rs:147-173`, `:179-210`, `:270`); Sky's `agents.yaml` entries are hand-written and have no capability data.

## 3 Activity monitor for ACP children

### 3.1 What it records (per child, in memory, in the child actor)

`ChildActivity` (new `crates/agent/src/activity.rs`, §9.1) is owned by `AcpChild` as `activity: ChildActivity` and fed from four places in `crates/agent/src/acp_child.rs`:

| field | set by | where |
|---|---|---|
| `delivered: Option<Instant>` | `delivered(now)` when `prompt` hands the text to `harness.prompt` | `prompt` (`:879-887`), including busy retries (`:2015-2031`) |
| `first_update`, `last_update: Option<Instant>` | `update(now, what)` on every `Update::Session` of our session that `translate` sees | the catch-all arm (`:1854-1881`), before `translate` |
| `tools_in_progress: u32` | `+1` on `ToolCall` whose status is not terminal, `-1` on `ToolCallUpdate` with `Completed`/`Failed` (never below 0; a `ToolCall` already `Completed` counts 0) | `translate` (`:1435-1482`), `finished` (`:1514-1520`) |
| `last: String` | tool title on `ToolCall`, `"message"` on `AgentMessageChunk`, `"thinking"` on `AgentThoughtChunk`; other updates leave it | same |
| turn end | `turn_ended(now)` | `finish` (`:994`), `closed`, `CancelTurn` |

A continuation turn (`:947-969`) is a turn for the monitor: `delivered` stays `None` (nothing was prompted), so only the `silent` rule applies to it.

### 3.2 Phases (D3)

- `no_first_update`: `delivered` is set, `first_update` is `None`, and `now - delivered ≥ first_update_secs`.
- `silent`: a turn is open, `last_update` is set, `tools_in_progress == 0`, and `now - last_update ≥ silent_secs`.
- A tool call in progress is never a stall: with `tools_in_progress > 0` the `silent` deadline is `None`. Rationale: a 20-minute `cargo build` inside the child is one `ToolCall` with no updates until it ends.
- A stall **episode** starts at the first fire and ends at the next update, turn end or respawn; one wake per episode (`fire` returns `Some` once, then `None` until re-armed by `update`/`delivered`). The next episode after a re-arm fires again (that is the 61 §1.3 rule without the backoff; stalls are rarer than quiet screens).
- `first_update_secs = 0` or `silent_secs = 0` disables that rule.

### 3.3 Where the actor checks

The actor keeps one `AbortHandle` (`stall: Option<AbortHandle>` on `AcpChild`) armed with `Self::timer(me, deadline - now, StallCheck)` (`timer` at `crates/agent/src/acp_child.rs:890-900`) whenever `activity.deadline(now)` changes; `StallCheck` calls `activity.fire(now)` and, on `Some(stall)`, tells the supervisor `ChildStalled { child, stall: Some(stall) }`; the next `update` tells `ChildStalled { child, stall: None }` so the supervisor's `stalled_secs` clears. No polling.

### 3.4 Pure and testable (C-d)

Every method takes `now: Instant`; nothing inside calls `Instant::now()`. The unit tests (§11) drive it with `base + Duration` values. `deadline` is a pure function of the fields; `fire(now)` is the only mutator that starts an episode.

### 3.5 Config

`agent.activity.*`, a nested `#[serde(default)]` struct on `AgentConfig` exactly like `broadcast`/`compaction` (`crates/agent/src/lib.rs:3275-3282`, `:3328-3340`):

```yaml
agent:
  activity:
    first_update_secs: 60   # D3; 0 = never
    silent_secs: 180        # D3; 0 = never
```

Read once at `supervisor::start` through `agent_config` (`crates/agent/src/supervisor.rs:9083-9089`) and handed to each child at spawn with `AcpChild::with_activity(cfg)`. No Settings writer, no live knob (the 137 `SetEndlessSettings` writer at `:7395-7417` is the pattern if one is wanted later).

### 3.6 Gating by the capability entry (D7)

The monitor runs for every ACP child. The *interpretation* is gated (§8.2): `no_first_update`/`silent` wake the parent regardless (they are time facts), but the §5 words `idle`/`done` are never produced for a harness without `turn_end`, and nothing from the monitor ever becomes a needs-you alert (§7) — a harness without `permission` has no path from "quiet" to Sky.

## 4 The stalled wake

### 4.1 Shape (D2)

A new input to the **parent session only**, rendered by `child_stalled(...)` in `crates/agent/src/supervisor.rs` next to `job_idle` (`:3092-3111`) and delivered with the same `inject` (`:1481-1545`) under `from = "session:<child>"` — the `from` `<child_lost>` uses (`crates/agent/src/supervisor/acp_resume.rs:346`):

```
<child_stalled child="w2" session="0199…" phase="silent" quiet_secs="184" last="shell: cargo check" at="14:07">
w2 has sent no session update for 184 s and no tool call is in progress. Ask it what it is doing with send_message, wait on it with wait_peer, or check its session with session_read; a child mid-build usually answers.
</child_stalled>
```

`phase` is `no_first_update` | `silent`; `last` is `ChildActivity.last` (empty attribute omitted); `quiet_secs` is seconds since `delivered` (`no_first_update`) or since `last_update` (`silent`); `at` is `clock::hhmm` like every wrapper (74 §4). One sentence of guidance per phase: `no_first_update` says "its prompt was delivered N s ago and nothing came back; the harness may be loading or stuck at startup".

### 4.2 Flow

`AcpChild` → `ChildStalled { child, stall: Some(Stall { phase, quiet_secs, last }) }` → `Agents` handler: look up the child's row (`self.sessions[child].parent`), record `activity.stalls.insert(child, (phase, now))`, render, `inject(parent, …)`. A root ACP session (no parent) records the stall and injects nothing. No attention item, no new `AgentEvent` (C-m): `inject` already publishes `MessageDelivered { session: parent, from: "session:<child>", text }` (`crates/agent/src/supervisor.rs:1538-1544`) which UIs show as the parent's inbox line. `ChildStalled { stall: None }` removes the entry.

### 4.3 Across an engine restart (109/146)

Mid-prompt children are respawned at boot (`crates/agent/src/supervisor/acp_resume.rs:89-122`); the new actor's `ChildActivity` starts empty. The respawn's `session/load` + restored note or hand-off is queued (`crates/agent/src/acp_child.rs:1159-1168`) and prompted through the same `prompt`, which calls `delivered(now)`: the timers restart from the respawn. An episode in flight before the restart is forgotten (it lived in memory); if the child is still silent after the respawn, a new episode fires once more — accepted, the parent is told the truth twice at most. A child that is not respawned gets the existing `<child_lost>` (`acp_resume.rs:342-347`), never a `<child_stalled>`.

## 5 Wait on state

### 5.1 The words (D4), defined against the existing state model (C-c)

`peer_state_of(&PeerFacts) -> PeerStateWord` in `crates/agent/src/activity.rs` is pure; the supervisor builds `PeerFacts` for a child from what it already holds:

| word | true when | facts from |
|---|---|---|
| `blocked` | an unanswered permission ask keyed to the child — `approvals` contains it, which is what `state_of` renders as `Approval` (`crates/agent/src/supervisor.rs:6019-6020`) — or an unanswered `ask_user` of the child, or `observed.state == RequiresAction` (`:6057`) | `approval`, `ask`, `requires_action` |
| `stalled` | the supervisor's `activity.stalls` has the child (§4.2) | `stalled_secs` |
| `idle` | the child is live, not `running` (`:6033`), has no turn in flight (`IsRunning` false, `crates/agent/src/acp_child.rs:2050-2053`) and can be prompted again (connection up, `dead` unset) | `live`, `running`, `connected` |
| `done` | `idle` **and** its result was delivered — no `waits(child_result)` row targets it any more (`crates/agent/src/supervisor/child_waits.rs:49-57` clears it in `hand_off`) **and** nothing is queued for it (`ledger.activity(child).is_none()`, the predicate `idle_check` uses, `crates/agent/src/supervisor.rs:2963-2966`) | `result_delivered`, `queued` |
| `running` | none of the above | — |
| `not_running` | the child's session row is gone, or it has no live actor and is not a respawn candidate (closed, deleted, `<child_lost>`), or its harness connection ended for good (`dead` set) | `live`, `dead` |

Precedence: `blocked` > `stalled` > `done` > `idle` > `running`; `not_running` is terminal and beats everything. §8.2 gating: without `turn_end`, `idle` and `done` are never returned (the word stays `running`).

### 5.2 The call

`wait_peer(to, until, timeout_secs=None)` — a runtime function (not `DIRECT`), registered next to `discover_peers`/`peer_message` (`crates/agent/src/supervisor.rs:9520-9521`), `RISK = ReadOnly`, `RETURNS = "dict"`, gated like `send_message` on the communication policy (`crates/agent/src/peers.rs:7-10`).

- `to`: one of the caller's own children by name or `session:<uuid>`; anything else → error `wait_peer: <to> is not one of your children`. Resolution happens once; the result is a `SessionId` — the **pinned identity** (C-b).
- `until`: a non-empty subset of `blocked | idle | done | stalled`; an empty list or another word → error.
- `timeout_secs`: `None` = no deadline; `Some(0)` → error.
- Returns at once: `{ child: "<name>", session: "<id>", state: "<word>", holds: bool, since_secs: N }`. `holds = true` when `state ∈ until` already — then nothing is registered and no wake follows (the caller has its answer).
- Otherwise a `WaitRow { session: caller, tool_call_id: cx.call_id, kind: PeerState, target: child, deadline: now + timeout, payload: {until, name, since_ms} }` is written with `wait_put` (`crates/agent/src/store/durability.rs:1232-1249`) and an in-memory entry goes into `Agents.activity.waits`.
- At most one `peer_state` wait per (caller, child): a second registration replaces the first (same `tool_call_id` semantics as `wait_put`'s upsert; the old entry is dropped silently).

### 5.3 The wake

On every state-relevant transition the supervisor already handles — `ApprovalRequested`/`ApprovalResolved` (`crates/agent/src/supervisor.rs:2563-2596`), `TurnStarted`/`TurnFinished` (`:2481-2482`, `:2309-2310`), `hand_off` (`:2080`), `ChildStalled` (§4.2), a child actor stopping, `<child_lost>` — `Agents::check_peer_waits(child)` recomputes `peer_state_of` for the waits targeting that child and settles those whose word is in `until`, plus every wait on a `not_running` child. Settling = `wait_clear`, remove the entry, `inject(caller, "session:<child>", body, wrapped, None)` with:

```
<peer_state child="w2" session="0199…" state="done" since_secs="3" at="14:09">
w2 is done: its result was delivered to you and nothing is queued for it.
</peer_state>
```

`since_secs` is seconds since that word became true (0 when unknown). Terminal outcomes use the same tag: `state="not_running"` ("w2 is gone: …why…") and `state="timeout"` ("w2 is still running after N s"). A timeout is one `tokio` sleep per wait armed at registration and at boot re-arm, checked against the row (a settled wait's timer finds nothing and does nothing).

### 5.4 Durable (C-a)

`WaitKind::PeerState` (`"peer_state"`) joins `crates/tools/src/journal.rs:214-249`. Like `child_result` the row outlives its turn: `cut_journal_in` must spare it (`crates/agent/src/store/durability.rs:187-191` — the `kind != 'child_result'` clause becomes `kind NOT IN ('child_result', 'peer_state')`). Boot, after `rearm_reminders_for(None)` and before `resume_scan` (`crates/agent/src/supervisor.rs:2602-2606`): `Store::peer_state_waits(Scope::Here)` (same shape as `child_waits`, `:1392`) → for each row: child row gone or not live and not a respawn candidate → settle `not_running`; deadline past → settle `timeout`; else rebuild the in-memory entry, re-arm the timer and run `check_peer_waits(child)` once (the condition may already hold). A caller that lives on another engine (139) gets its wake through `inject`'s `harness_enqueue` path (`:1489-1518`) like any input.

### 5.5 `discover_peers` (D4, last bullet)

`PeerActivity` gains `stalled_secs: Option<u64>` (`#[serde(default)]`), filled from `activity.stalls` in `peer_activity` (`crates/agent/src/supervisor.rs:5422-5445`) and printed as ` stalled=184s` in the text rendering (`crates/agent/src/peers.rs:622-636`). The description string (`:569-583`) gains the word.

## 6 Terminal jobs: busy or quiet

### 6.1 Signals (D5)

Four generic signals, no per-program rules: the rendered screen (already `Idle::saw`, `crates/tools/src/pty.rs:1997-2002`), the processes under the PTY, their CPU, and the reaction to input. The job task `run` (`:2044`) already owns the leader pid (`j.pty.pid`, `:483-484`) and the tree (`j.pty.tree`, `:486`).

### 6.2 Process-tree sampler (C-j)

- `platform::TreeSample { processes: u32, cpu: Duration }` and `ProcessTree::sample(&self) -> Option<TreeSample>` in `crates/platform/src/process.rs` (next to `alive`, `:59-63`), because the Windows tree is a Job Object whose `QueryInformationJobObject(JobObjectBasicAccountingInformation)` already returns `ActiveProcesses`, `TotalUserTime` and `TotalKernelTime` in the call `alive` makes (`:290-301`). Unix walks the descendants of the leader: macOS `libc::proc_listchildpids` recursively + `proc_pidinfo(PROC_PIDTASKINFO)` → `pti_total_user + pti_total_system` (libc 0.2.189 in `Cargo.lock` has all four symbols); Linux `/proc/<pid>/stat` fields 4 (ppid), 14 and 15 (utime, stime) over `/proc/*`; the walk is bounded (≤ 256 processes) and `None` on any error. No new crate: the workspace has `libc` and `windows-sys 0.61` with `Win32_System_JobObjects` already (`crates/platform/Cargo.toml:16`, `:25-33`).
- `crates/tools/src/proctree.rs` (new) holds the rule, `TreeQuiet` (§9.3): it takes samples about once a second **only while the screen is unchanged** (a `tokio::time::interval(1 s)` arm in `run`, enabled when `idle.is_some()` and the last tick was not dirty), keeps the previous sample, and answers `quiet(now) -> bool`:
  - `cpu` : `(sample.cpu - prev.cpu) / (sample.at - prev.at) ≤ pty_tree_cpu_pct %` of one core;
  - `foreground`: unix — `libc::tcgetpgrp(master_fd) == pty.pid` (the program theuth started is in front; a `sleep 300` run by an interactive shell sits in its own foreground group, a persistent helper process of opencode does not); Windows — `processes ≤ baseline`, the count at the first sample after spawn. `MasterPty::as_raw_fd` gives the fd (portable-pty 0.9).
  - `quiet = cpu && foreground`. A tree that cannot be sampled (`None`) counts as quiet so the 61 behaviour is unchanged where the sampler is unavailable.

### 6.3 The job_idle rule

`Idle::deadline` stays the clock (`:2006-2019`); at the deadline the fire arm (`:2124-2143`) asks `TreeQuiet::quiet(now)` first: busy → no fire, and the detector is re-armed by treating the busy sample as a touch (`last_change = now` is wrong — the screen did not change — so `Idle` gains `defer(now)` that moves the deadline by `base` without resetting `nudges`). Quiet → fire as today with `why = "screen unchanged for {quiet} s, process tree quiet"`. A `wait` in flight still suppresses (`:2027`).

### 6.4 Reaction check

`send` (`:2472-2493`) records `*lock(&pty.sent_at) = Some(Instant::now())` after the write (model sends only; a human's `PtyInput` already marks `user_input`, `:1797-1798`). In `run`: while `sent_at` is set and younger than `pty_reaction_secs`, every tick and every tree sample checks movement — a screen change (`Idle::saw` saw different rows) or a non-quiet tree clears `sent_at`. When `sent_at` ages past `pty_reaction_secs` with no movement: fire once `JobEvent::Idle { why: "no reaction to input", quiet_secs: since last change, nudge: idle.nudges + 1, … }` through the same `activity.notify` + bus path, clear `sent_at`, and leave `Idle`'s own deadline alone. Suppressed like any fire while `waiting > 0` (the model is in `wait(pattern)`; its timeout is the answer).

### 6.5 What the model sees

`JobEvent::Idle` gains `why: String` (`#[serde(default)]`, `crates/tools/src/jobs.rs:234-254`); `Notice::JobIdle` carries the event unchanged (`crates/tools/src/activity.rs:189-197`); `Agents::notice` destructures it (`crates/agent/src/supervisor.rs:2906-2920`) and `job_idle(...)` renders `why="…"` after `nudge` (`:3092-3111`):

```
<job_idle id="j3" command="bash" quiet="63" secs="412" nudge="1" why="screen unchanged for 63 s, process tree quiet" at="14:11">
```

Desktop: `reduce.ts` ignores the new field (`crates/desktop/ui/src/store/reduce.ts:784-785`); the fixture in `reduce.test.ts:532` gets `why: ''`.

### 6.6 Config (C-i)

Two static keys on `ToolsConfig` (`crates/tools/src/lib.rs:486-533`), wired like `pty_shell` through `StartTool::new` (`crates/tools/src/registry.rs:1883-1892`) as one `PtyThresholds` value:

```yaml
tools:
  pty_reaction_secs: 10   # after send(): no screen or tree movement for this long → <job_idle why="no reaction to input">; 0 = off
  pty_tree_cpu_pct: 2     # a process tree using more than this % of one core over a 1 s sample is busy
```

No live knob, no RPC, no Settings row: `pty_idle_secs` keeps its `SetPtyIdle` path untouched.

## 7 Alert rules

### 7.1 Shape (D6)

`AttentionItem` gains `alert: Option<Alert>` (`#[serde(default)]`, TS `alert?: Alert | null`), with

```
Alert { id: "<item id>@<episode start ms>", class: needs_you | finished, why: "<one short sentence>", at_ms: <episode start> }
```

`id` is stable for the life of one episode (one continuous presence of the item in the derived list) and changes when the item disappears and comes back; clients post one notification per `alert.id`.

### 7.2 Class and why per kind (C-l)

| `AttentionKind` (`crates/roster/src/attention.rs:39-73`) | class | why |
|---|---|---|
| Approval | needs_you | "waiting for allow / deny on `<tool>`" |
| Decision | needs_you | "a task is blocked on your decision" |
| Hire | needs_you | "a proposed agent waits for hire / decline" |
| Blocked | needs_you | "a task is blocked on you" |
| Review | needs_you | "a task waits for your review" |
| Handoff | needs_you | "an agent in a VM needs you at its desktop" |
| Secret | needs_you | "a session asked for a secret" |
| Ask | needs_you | "a session asked you a question" |
| Resume | needs_you | "an interrupted turn needs the Resume button" |
| Budget | needs_you when `detail` starts with `paused`, else none | "an agent is paused at its budget cap" |
| Finished | finished | "turn ended: <summary first line>" |
| Node, Show, Double | none | — |

`class_of`/`why_of` live in `crates/roster/src/alert.rs` (new) as pure functions; `derive` stays untouched and the actor decorates afterwards (§7.4). Items from external `Source`s (126) and `resume:boot:*` get `None`.

### 7.3 Suppress while in view (C-f)

Focus is per connection (129): `FocusMap` on the rpc `Server` (`crates/rpc/src/api.rs:2458`, `:7926-7941`). `attention_list` becomes `with_extensions` (`:2251-2252`, `:7405-7407`) and, per item with an `alert` and a `session`, blanks `alert` when `session ∈ focus[peer]`. The roster never learns about focus. `AttentionChanged` is unchanged (clients refetch).

### 7.4 Delay that re-checks (C-h)

`roster.alert_delay_ms` (default 2000) on `RosterConfig` (`crates/roster/src/config.rs:9-25`; the actor reads `rt.section("roster")` the way `recheck` reads `tasks`, `crates/roster/src/attention.rs:1289-1293`). `Episodes` on the `Attention` actor maps item id → episode start ms (first derive that contained it; `retain` drops ids that left the list). After `derive`, `alert` is set only when `now ≥ start + delay` — the item must still be there at the re-check, which is the herdr rule (the 300 ms permission ask that resolves itself never alerts). When some item is inside its delay window the actor arms one `AlertDue` tell at the earliest `start + delay` so the next `recheck` publishes `AttentionChanged` without a client having to poll.

### 7.5 Done ≠ idle: seen per connection (C-g)

`finished` alerts carry a seen bit **per connection**: the rpc `Server` keeps `SeenMap = Arc<RwLock<HashMap<u64 /* peer id */, HashSet<String /* alert id */>>>>`, dropped with the connection like `FocusMap`. A finished alert is marked seen for a connection when that connection focuses the item's session after the finish — eagerly in `set_event_focus` (which asks `ListAttention` and marks every `finished` alert whose session is in the new set) and lazily in `attention_list` (an item whose session is focused is marked before being blanked). `attention_list` blanks `alert` on `finished` items already seen by that connection. Each client therefore shows "done" until it has looked; another client's look changes nothing. A reconnect is a fresh set (`RpcPeer` carries no device id, `crates/link/src/types.rs:284-289`), so a restarted client sees unseen finishes again, once; a client that wants memory across its own restarts keeps its own list (the companion has settings storage, the desktop `localStorage`).

### 7.6 Clients decide how (D6)

- **Desktop UI** (`crates/desktop/ui/src/App.tsx:473-488`, `crates/desktop/ui/src/lib/notify.ts:7-12`): the two event-driven `notifyIfHidden` calls stay for items **without** `alert` (nothing changes for them); for items with `alert` the attention `QueryObserver` (`App.tsx:496-500`) posts one `notifyIfHidden(title, alert.why)` per unseen `alert.id` (a `Set<string>` in the store), skipping while `companion` is alive as today. Pause: none exists today and none is added.
- **macOS companion** (`Notifier.swift:13-81`, `Policy.swift:70-110`): `AttentionItem` Codable gains `alert: Alert?` (`crates/desktop/companion/Sources/TheuthKit/Models.swift:45-106`); `NotificationPolicy.update` treats an item with `alert` as class `needs_you → .blocking`, `finished → .deferrable`, keyed by `alert.id` for dedupe (one banner per alert id), respects `pausedUntil` (`:199`) and the existing prefs; items without `alert` keep `classify(item)` exactly as today. The `why` is the banner body.
- **Windows companion**: untouched (D6); its `Models.cs` ignores unknown JSON fields, so the new field is harmless.

## 8 Capability entries

### 8.1 Data (D7, C-k)

```yaml
omp:
  command: omp
  args: [acp]
  reports:            # optional; all false when absent and the command is unknown
    permission: true  # sends session/request_permission for gated tools
    turn_end: true    # session/prompt returns when the turn is over (no continuation work)
    session_load: true # advertises loadSession and resumes a session id
```

`acp::agents::Reports { permission, turn_end, session_load }` (all `bool`, `Default` = all false) and `Entry.reports: Option<Reports>` (`#[serde(default, skip_serializing_if = "Option::is_none")]`, `crates/acp/src/agents.rs:20-54`). `Entry::reports(&self) -> Reports` returns the block when written, else `Reports::for_command(&self.command, &self.args)` — a table by command basename and the npx package name:

| harness | permission | turn_end | session_load | why |
|---|---|---|---|---|
| `omp` | true | true | true | omp returns from `session/prompt` and may keep working afterwards; theuth already folds that work into a continuation turn that closes after `CONTINUATION_QUIET` 120 s (`crates/agent/src/acp_child.rs:76-78`, `:947-969`), so `idle` is late by up to 120 s, never wrong |
| `opencode`, `opencode2` | true | true | false | 1.x/2.x return at turn end; `session/load` is version-dependent (conservative) |
| `@agentclientprotocol/claude-agent-acp` | true | true | false | ACP adapter; resume unverified |
| `codex-acp`, `@agentclientprotocol/codex-acp` | true | true | false | same |
| anything else | false | false | false | unknown |

`Reports::for_registry(id)` gives the registry installer the same rows by registry id (`crates/acp/src/registry.rs:163-173`, `:200-209`, `:270-279` set `reports: Reports::for_registry(&agent.id)` so an installed entry is explicit on disk). `EntryInfo` (`crates/acp/src/agents.rs:82-84`) gains `reports: Reports` for `/acp` and the UI.

### 8.2 Gating

- Without `turn_end`: silence after output is never `idle`/`done` (§5.1 returns `running`); `wait_peer until=idle|done` is refused at registration with `wait_peer: <name>'s harness does not report turn end; wait on blocked|stalled or use send_message{await_reply}` (C-e). Stalls are time facts and still wake the parent (§3.6).
- Without `permission`: nothing derived from a child's quiet is ever a needs-you alert (§7.2 has no such row in v1; the invariant binds future rows).
- `session_load` is informational in v1 (the respawn path already probes `loadSession` itself, `crates/acp/src/harness.rs:326-356` per the seam map); it is recorded so 146's RESUME brief can read it.

## 9 Frozen signatures

These are law (working rules): the `155 C: skeleton` commit contains exactly these shapes with neutral bodies; workers fill bodies and may add private items, never change a frozen name, field, type or default. A deviation goes in the DONE message.

### 9.1 `crates/agent/src/activity.rs` (new; W1)

```rust
/// `agent.activity` (155 §3.5).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ActivityConfig {
    pub first_update_secs: u64, // 60; 0 = never
    pub silent_secs: u64,       // 180; 0 = never
}

#[serde(rename_all = "snake_case")] // + JsonSchema, ts
pub enum StallPhase { NoFirstUpdate, Silent }
impl StallPhase { pub const fn as_str(self) -> &'static str } // "no_first_update" | "silent"

/// One `session/update` as the monitor sees it.
pub enum ChildUpdate<'a> { Message, Thinking, ToolStarted(&'a str), ToolFinished, Other }

/// One wake (§4.1).
pub struct Stall { pub phase: StallPhase, pub quiet_secs: u64, pub last: String }

/// The per-child monitor (§3). Pure: every method takes `now`.
pub struct ChildActivity { /* private */ }
impl ChildActivity {
    pub fn new(cfg: ActivityConfig) -> Self;
    pub fn delivered(&mut self, now: Instant);
    pub fn update(&mut self, now: Instant, what: ChildUpdate<'_>);
    pub fn turn_ended(&mut self, now: Instant);
    pub fn deadline(&self, now: Instant) -> Option<Instant>;
    pub fn fire(&mut self, now: Instant) -> Option<Stall>;
    pub fn tools_in_progress(&self) -> u32;
    pub fn last(&self) -> &str;
    pub fn stalled(&self) -> Option<StallPhase>;   // the open episode's phase
}

/// The §5.1 words; `until` accepts the first four after `running`.
#[serde(rename_all = "snake_case")] // + JsonSchema, ts
pub enum PeerStateWord { Running, Blocked, Idle, Done, Stalled, NotRunning, Timeout }
impl PeerStateWord {
    pub const fn as_str(self) -> &'static str;
    pub const fn waitable(self) -> bool; // Blocked | Idle | Done | Stalled
}

/// What the supervisor knows about one child (§5.1 table).
pub struct PeerFacts {
    pub live: bool, pub dead: bool, pub running: bool,
    pub approval: bool, pub ask: bool, pub requires_action: bool,
    pub stalled_secs: Option<u64>, pub result_delivered: bool, pub queued: bool,
    pub reports: acp::Reports,
}
pub fn peer_state_of(f: &PeerFacts) -> PeerStateWord;

/// One registered `wait_peer` (§5.2).
pub struct PeerWait {
    pub caller: SessionId, pub call_id: String, pub child: SessionId, pub name: String,
    pub until: Vec<PeerStateWord>, pub deadline: Option<Instant>, pub since: Instant,
}

/// Supervisor-side state: `Agents.activity` (§4.2, §5.2).
#[derive(Default)]
pub struct Monitor {
    pub stalls: HashMap<SessionId, (StallPhase, Instant)>,
    pub waits: HashMap<(SessionId, SessionId), PeerWait>, // (caller, child)
}

/// `tell` from the child actor (§4.2): `Some` opens an episode, `None` ends it.
pub struct ChildStalled { pub child: SessionId, pub stall: Option<Stall> }
/// `ask` → `Result<PeerWaitRegistered, String>` (§5.2).
pub struct WaitPeer {
    pub session: SessionId, pub call_id: String, pub to: String,
    pub until: Vec<PeerStateWord>, pub timeout_secs: Option<u64>,
}
#[derive(Serialize)] // the tool's `structured`
pub struct PeerWaitRegistered {
    pub child: SessionId, pub name: String, pub state: PeerStateWord, pub holds: bool, pub since_secs: u64,
}
```

### 9.2 crates/agent, other files (W1)

```rust
// lib.rs
pub mod activity;
pub use activity::{ActivityConfig, ChildStalled, PeerStateWord, StallPhase, WaitPeer, PeerWaitRegistered};
pub struct AgentConfig { /* … */ pub activity: ActivityConfig, /* … */ }          // after `endless`

// communication.rs
pub struct PeerActivity { /* … */ #[serde(default)] pub stalled_secs: Option<u64> } // §5.5

// acp_child.rs
pub struct AcpChild { /* … */ activity: ChildActivity, stall: Option<AbortHandle> }
impl AcpChild { pub fn with_activity(self, cfg: ActivityConfig) -> Self; }
struct StallCheck;                                                                  // timer message (§3.3)

// supervisor.rs
pub struct Agents { /* … */ pub(crate) activity: crate::activity::Monitor }
pub(crate) fn job_idle(id, command, name, quiet, secs, nudge, user_typed, why: &str, text, now) -> String; // why="…" after nudge
pub(crate) fn child_stalled(name: &str, session: SessionId, phase: StallPhase, quiet_secs: u64, last: &str, body: &str, now) -> String;
pub(crate) fn peer_state(name: &str, session: SessionId, state: PeerStateWord, since_secs: u64, body: &str, now) -> String;
impl Message<ChildStalled> for Agents { type Reply = (); }                          // supervisor/activity.rs
impl Message<WaitPeer> for Agents { type Reply = Result<PeerWaitRegistered, String>; }
impl Agents {                                                                        // supervisor/activity.rs
    pub(crate) async fn check_peer_waits(&mut self, child: SessionId);               // §5.3, every transition
    pub(crate) async fn rebuild_peer_waits(&mut self);                               // §5.4, boot
}
// registration: reg.ask(RegisterTool(Arc::new(WaitPeerTool))) after PeerMessageTool

// peers.rs
pub struct WaitPeerArgs { pub to: String, pub until: Vec<PeerStateWord>, #[serde(default)] pub timeout_secs: Option<u64> }
pub struct WaitPeerTool;  // Tool: NAME = "wait_peer", RISK = ReadOnly, RETURNS = "dict", not DIRECT

// store/durability.rs
pub async fn peer_state_waits(&self, scope: Scope) -> anyhow::Result<Vec<WaitRow>>; // kind = 'peer_state'
```

### 9.3 crates/tools and crates/platform (W2)

```rust
// tools/src/journal.rs
pub enum WaitKind { PeerReply, ChildResult, Job, PtyWait, Question, PeerState }     // "peer_state"

// tools/src/jobs.rs
JobEvent::Idle { /* … */ #[serde(default)] why: String }

// tools/src/lib.rs
pub struct ToolsConfig { /* … */ pub pty_reaction_secs: u64, pub pty_tree_cpu_pct: u8 } // 10, 2
pub mod proctree; pub use proctree::{PtyThresholds, TreeQuiet};

// tools/src/proctree.rs (new)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PtyThresholds { pub reaction: Duration, pub tree_cpu_pct: u8 }
impl PtyThresholds { pub fn from_config(cfg: &ToolsConfig) -> Self; }
/// The §6.2 rule over successive `platform::TreeSample`s.
pub struct TreeQuiet { /* private */ }
impl TreeQuiet {
    pub fn new(thresholds: PtyThresholds) -> Self;
    pub fn saw(&mut self, now: Instant, sample: Option<platform::TreeSample>, foreground_is_leader: bool);
    pub fn quiet(&self) -> bool;         // true until two samples exist
    pub fn last_sample(&self) -> Option<(Instant, platform::TreeSample)>;
    pub fn moved_since(&self, t: Instant) -> bool;
}

// tools/src/pty.rs
pub(crate) struct PtyHandle { /* … */ sent_at: Mutex<Option<Instant>> }
impl Idle { fn defer(&mut self, now: Instant); }
impl StartTool { pub fn new(jobs, shells, ready_timeout, idle_default, pty_shell, thresholds: PtyThresholds) -> Self; }

// platform/src/process.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TreeSample { pub processes: u32, pub cpu: Duration }
impl ProcessTree { pub fn sample(&self) -> Option<TreeSample>; }
```

### 9.4 crates/acp (W1)

```rust
// agents.rs
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)] // + ts
#[serde(default)]
pub struct Reports { pub permission: bool, pub turn_end: bool, pub session_load: bool }
impl Reports {
    pub fn for_command(command: &str, args: &[String]) -> Self; // §8.1 table
    pub fn for_registry(id: &str) -> Self;
}
pub struct Entry { /* … */ #[serde(default, skip_serializing_if = "Option::is_none")] pub reports: Option<Reports> }
impl Entry { pub fn reports(&self) -> Reports; }
pub struct EntryInfo { /* … */ #[serde(default)] pub reports: Reports }
```

### 9.5 crates/roster and crates/rpc (W3)

```rust
// roster/src/alert.rs (new)
#[serde(rename_all = "snake_case")] // + JsonSchema, ts
pub enum AlertClass { NeedsYou, Finished }
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)] // + ts
pub struct Alert { pub id: String, pub class: AlertClass, pub why: String, #[ts(type = "number")] pub at_ms: u64 }
pub fn class_of(item: &AttentionItem) -> Option<AlertClass>;
pub fn why_of(item: &AttentionItem) -> String;
#[derive(Default)]
pub struct Episodes { /* item id → episode start ms */ }
impl Episodes {
    pub fn alert(&mut self, item: &AttentionItem, now_ms: u64, delay_ms: u64) -> Option<Alert>;
    pub fn retain<'a>(&mut self, live: impl Iterator<Item = &'a str>);
    pub fn next_due(&self, now_ms: u64, delay_ms: u64) -> Option<u64>;
}

// roster/src/attention.rs
pub struct AttentionItem { /* … */ #[serde(default)] #[ts(optional = nullable)] pub alert: Option<Alert> } // last field
pub struct Attention { /* … */ episodes: Episodes }
pub struct AlertDue;                                        // timer message (§7.4)

// roster/src/config.rs
pub struct RosterConfig { /* … */ pub alert_delay_ms: u64 } // 2000

// rpc/src/api.rs
pub type SeenMap = Arc<RwLock<HashMap<u64, HashSet<String>>>>;
pub struct Server { /* … */ pub seen: SeenMap }
#[method(name = "attention_list", with_extensions)]
async fn attention_list(&self) -> RpcResult<Vec<roster::attention::AttentionItem>>;
```

Wire additions the clients see after the bindings regen: `AttentionItem.alert?: Alert | null`, `Alert`, `AlertClass`, `PeerActivity.stalled_secs`, `JobEvent.JobIdle.why`, `PeerStateWord`, `StallPhase`, `EntryInfo.reports` / `Reports`.

## 10 Waves and file ownership

| wave | owns | must not touch |
|---|---|---|
| **W1 ENGINE-ACTIVITY** | `crates/agent/src/activity.rs`, `acp_child.rs` (hooks §3.1, §3.3), `supervisor.rs` (`child_stalled`/`peer_state` wrappers, `job_idle` `why`, `peer_activity`, registration), `supervisor/activity.rs` (new; `ChildStalled`, `WaitPeer`, `check_peer_waits`, boot re-arm), `peers.rs` (`WaitPeerTool`, `discover_peers` text), `communication.rs`, `store/durability.rs` (`peer_state_waits`, `cut_journal_in`), `lib.rs` (config), `crates/acp/src/agents.rs`, `crates/acp/src/registry.rs` | anything under `crates/tools`, `crates/platform`, `crates/roster`, `crates/rpc`, `crates/desktop` |
| **W2 PTY-SIGNALS** | `crates/tools/src/pty.rs`, `proctree.rs` (new), `activity.rs`, `jobs.rs`, `lib.rs` (config), `registry.rs` (wiring), `crates/tools/Cargo.toml` only if truly needed, `crates/platform/src/process.rs` (`sample`) and `crates/platform/Cargo.toml` features | `crates/tools/src/journal.rs` (`WaitKind::PeerState` is in the skeleton; W1 reads it), `crates/agent/**` (the `why` rendering is W1's), `crates/desktop/ui/src/store/reduce.test.ts` (fixture already updated by C) |
| **W3 ALERTS** | `crates/roster/src/alert.rs` (new), `attention.rs` (decorate + `AlertDue`), `config.rs`, `crates/rpc/src/api.rs` (`attention_list` masking, `set_event_focus` → seen), `crates/desktop/ui/**`, `crates/desktop/companion/Sources/Companion/Notifications/**`, `crates/desktop/companion/Sources/CompanionCore/Policy.swift`, `crates/desktop/companion/Sources/TheuthKit/Models.swift` (the Codable `Alert`) | `crates/agent/**`, `crates/tools/**`, `crates/acp/**`, `crates/desktop/companion-windows/**` |

PIN dependencies: none at wave start — every cross-wave type is in the `155 C: skeleton` commit (`WaitKind::PeerState`, `JobEvent::Idle.why`, `acp::Reports`, `Alert`, `PeerStateWord`, `TreeSample`). W1 renders `why` from the field W2 fills; W3 reads `Reports` only through `EntryInfo`. If a worker must change a frozen shape, it messages the orchestrator first and lists the deviation in DONE.

Integrator (after the three DONEs): merge in the order W2 → W1 → W3 (tools below agent below roster/rpc), `cargo fmt -p` the touched crates, `mbx check -p agent -p tools -p roster -p rpc -p acp -p platform --tests`, the bindings export + copy, `bunx tsc --noEmit -p crates/desktop/ui`, `swift build` for the companion, then the live proofs of §11 on a scratch engine and the as-built `docs/design/155-as-built.md`.

The skeleton commit also carries one-line literal updates outside the three waves' crates — `crates/nodes/src/lib.rs` (test `acp::Entry`), `crates/tui/src/model.rs` (test `EntryInfo`), `crates/double/src/actor.rs` and `crates/autoresearch/src/actor.rs` (`AttentionItem.alert: None`), `crates/desktop/ui/src/store/reduce.test.ts` (`why: ''`) — so `--workspace` still compiles. No wave touches those again.

## 11 Proof list

Unit tests (one per bullet, in the owning crate; fake clocks by passing `Instant`s / `now_ms`):

- W1 `activity.rs`: `no_first_update` fires at `delivered + 60 s` and not at 59; `silent` fires 180 s after the last update only when `tools_in_progress == 0`; a `ToolStarted` suspends the `silent` deadline and `ToolFinished` resumes it from the finish; one `Some` per episode, re-armed by `update`; `first_update_secs = 0` never fires; a continuation turn (no `delivered`) only fires `silent`.
- W1 `activity.rs`: `peer_state_of` precedence table (blocked beats stalled beats done beats idle); `reports.turn_end = false` turns idle/done into running; `dead` → not_running.
- W1 `supervisor/activity.rs` (actor rig as `child_waits.rs` tests): `wait_peer` returns `holds: true` without a row when the state already holds; a registered wait writes a `peer_state` row and wakes the caller with `<peer_state state="done">` after `hand_off`; a second child spawned under the same name does not satisfy the first wait (pinning); the child's actor stopping settles `not_running`; a timeout settles `timeout` and clears the row; boot `peer_state_waits(Scope::Here)` re-arms a row, settles a past deadline, settles a missing child.
- W1 `supervisor.rs`: `child_stalled` wrapper renders every attribute and escapes `last`; `job_idle` renders `why`.
- W1 `acp::agents`: `Reports::for_command` rows of §8.1; an explicit `reports:` block round-trips through `agents.yaml`; an entry without it serialises without the key.
- W2 `proctree.rs`: `TreeQuiet` is quiet with no samples, busy when cpu delta exceeds the threshold, busy when `foreground_is_leader` is false, quiet again after one quiet sample; `moved_since`.
- W2 `pty.rs`: the idle rule with a fake `TreeQuiet` — screen unchanged + tree busy → no fire and `defer`; screen unchanged + tree quiet → fire with the quiet `why`; reaction check — `send()` then neither screen nor tree moves for `reaction` → one fire with `why = "no reaction to input"`, suppressed while `waiting > 0`, no second fire.
- W2 `platform::process`: `sample` on the test's own process group returns `processes ≥ 1` and a cpu that does not decrease between two calls (unix and Windows).
- W3 `alert.rs`: `class_of`/`why_of` for all 14 kinds (table §7.2); `Episodes::alert` is `None` before `start + delay`, `Some` with a stable id after, a new id after the item left and returned; `next_due`.
- W3 `api.rs`: `attention_list` blanks `alert` for a focused session and keeps it for an unfocused one on a second connection; a `finished` alert blanked once by focus stays blanked for that connection and visible on another; the seen set dies with the connection.
- W3 desktop (vitest): one `notifyIfHidden` per `alert.id`, none for the focused session, old path for items without `alert`. Companion (Swift Testing): `NotificationPolicy.update` posts one banner per `alert.id`, respects `pausedUntil`, falls back to `classify` without `alert`.

Live proofs on a scratch engine (integrator, recorded in the as-built with timestamps):

1. A fake ACP child (the `Rig` fake harness pattern, `crates/agent/src/acp_child.rs:3466`) that accepts the prompt and sends nothing wakes its parent with `<child_stalled phase="no_first_update">` within 60 s + one tick; one that streams a tool call and then goes silent with the call still open never wakes; after the call completes it wakes with `phase="silent"` at 180 s.
2. `wait_peer(w1, until=["done"])` from a parent returns `holds: false`, and the parent is woken with `<peer_state state="done">` within one second of the child's result delivery; `wait_peer(w1, until=["blocked"])` wakes when the child asks for a permission.
3. `start(command="bash")`, `send("sleep 300")`: no `<job_idle>` for 300 s although the screen never changes; after `sleep` ends, `<job_idle why="screen unchanged for … s, process tree quiet">` within `pty_idle_secs`. `start(command="bash")`, `send("cat")` then `send("x")` with no echo (`stty -echo` first): `<job_idle why="no reaction to input">` at 10 s.
4. A root session finishes a turn while the desktop shows another session: one notification; the same with the finished session in view: none; focusing the session afterwards clears "done" on that client and not on the companion.
5. Engine restart with a `peer_state` wait open: the wait is re-armed and still wakes.

## 12 Out of scope

- The decision model and any model-written state reason (D8; `docs/design/155/eval-decision-model.md:21`).
- Process survival across an engine restart (109 D14 stands); PTY state across a restart (`pty_screens` snapshots remain the only trace).
- Per-harness screen rules, manifests, OSC title/progress parsing, plugins (D9; herdr A1 is explicitly not borrowed).
- An attention item for a stalled child (D2 — v1 wakes the parent only); a `Blocked{why}` word on `AgentState`.
- Network-traffic signals for PTY busy/quiet; engine-managed worktrees.
- The Windows companion (follow-up brief); push to phones (42, not built).
- A `t agents wait` CLI; a `wait_state` for PTY jobs (`wait_peer` is for children; a PTY still uses `wait(pattern)` and `<job_idle why>`).
- Persisting the finished-seen set across reconnects (C-g); live knobs/Settings rows for the new config keys.

## 13 Inconsistencies and open questions

1. **`reports.turn_end` for omp** (§8.1): set true because the continuation turn already covers omp's post-prompt work; the cost is `idle` arriving up to `CONTINUATION_QUIET` = 120 s late. If Sky wants `done` faster for omp, the knob is `CONTINUATION_QUIET`, not this table.
2. **`blocked` and `ask_user`**: the pending-ask map is in tools (`tools::PendingAsks`), not on `Agents`; W1 reads it through `ListAsks`-style access or the `SessionActivity.asks` count (`crates/agent/src/lib.rs:1348-1363`). The §5.1 fact is `ask: bool`; how it is fetched is W1's.
3. **`done` for a child the parent never waited on** (a `spawn_subagent` with no `child_result` row because `call_id` was empty, `crates/agent/src/supervisor/child_waits.rs:19-22`): `result_delivered` is true vacuously; `done` then equals `idle ∧ !queued`. Acceptable.
4. **Seen per connection vs per device** (C-g): D6 says per device. `RpcPeer` has no device id; a per-device key needs the link device registry wired into rpc, which is out of this wave. Clients persist their own set if they want more.
5. **Focus and the companion**: the companion never calls `set_event_focus` today; its finished items therefore stay "done" until the desktop focuses the session on the *same* connection — which never happens (different connections). W3 must either make the companion call `set_event_focus([session])` when the user opens an item, or accept that the companion's done bit clears only by dismiss. Decide in W3's DONE.
6. **`is_delta` focus filtering** (`crates/rpc/src/api.rs:7946-7951`) is unchanged: `AttentionChanged` still reaches every connection; suppression happens in `attention_list`, not the stream.
7. **Windows foreground rule** (§6.2) is weaker than unix (`processes ≤ baseline` instead of `tcgetpgrp`); a PowerShell `Start-Sleep` is in-process and shows only as low CPU — the reaction check still covers an ignored `send`.
8. **Two wake inputs in one tick**: a child can be `stalled` and then `done` within a second (the harness answers and ends the turn). The parent then receives `<child_stalled>` and `<peer_state state="done">` back to back; both are true at their time. No coalescing in v1.
9. The seam map's `Idle` line numbers (`crates/tools/src/pty.rs:1972-2038`) and the recon's `state_of` lines (`supervisor.rs:5924-5955`) have drifted on this base; the cites in this document are the current ones (`state_of` is at `:6013-6042`).
