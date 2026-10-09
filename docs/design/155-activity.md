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
- **C-g** The seen set is keyed by the link device id when the connection carries one, else by the connection id; it lives in memory on the rpc `Server` and is lost at engine restart (a restart re-alerts unseen finishes once) (§7.5).
- **C-h** `alert_delay_ms` is a `roster:` key (`roster.alert_delay_ms`, default 2000), since the attention actor already reads `rt.section("roster")` (§7.4).
- **C-i** PTY thresholds are two static `tools.pty_*` keys (`pty_reaction_secs` 10, `pty_tree_cpu_pct` 2), wired like `pty_shell`, not live knobs like `pty_idle_secs` (§6.5).
- **C-j** `proctree.rs` is per-OS code over `libc`/`windows-sys` (no new crate); nothing in the workspace depends on `sysinfo`/`libproc`/`procfs`/`nix` (§6.2).
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

## 4 The stalled wake

## 5 Wait on state

## 6 Terminal jobs: busy or quiet

## 7 Alert rules

## 8 Capability entries

## 9 Frozen signatures

## 10 Waves and file ownership

## 11 Proof list

## 12 Out of scope

## 13 Inconsistencies and open questions
