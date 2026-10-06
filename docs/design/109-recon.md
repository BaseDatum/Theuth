# 109 — Recon: what survives a kill of the theuth engine (`t`) today

*Read-only source recon, 2026-10-06, on `~/Develop/theuth` main `ef1ad0a3`. Produced for design 109 (durability). Every claim cites `path#Lstart-Lend` relative to the repo root; "not found" = not located within the recon budget, not a claim of absence. The design doc (`109-durability.md`) builds on this; where line numbers drift, the cited symbols still name the place.*

## 1. Process model

- **Engine `t`** (`t --headless` under the desktop app, or TUI/`-p`): `main.rs::run` builds the runtime, starts the plugin list in order, then `select!`s on the shutdown token / SIGINT / `parent_gone()`, sleeps 100 ms "to flush", calls `rpc::engine::release()` and `std::process::exit(0)` — `crates/theuth/src/main.rs#L1505-L1521`. Plugin order: tools, secrets, document, knowledge, web, browser, scrape, codeindex, routes, computer, artifact, app, search_providers, **agent**, … link, nodes, **schedule**, mesh, replica, sessionsync, roster, **tasks**, sandbox, a2a, update, voice, talk, decision, meeting, rpc — `#L1546-L1591`. A SIGKILL skips all of this; only the 100 ms courtesy flush is lost, nothing in it is required for correctness (sqlite commits are synchronous).
- **Desktop app → daemon**: the app spawns `t --headless` with `THEUTH_PARENT_PID=<app pid>` (`crates/desktop/src/launch.rs#L259-L271`); the daemon polls that pid every 5 s and shuts itself down when the app is gone (`main.rs#L1527-L1541`). The app has `relaunch()` (`launch.rs#L106`) but automatic restart on daemon death was not found; on Windows a kill-on-close Job Object kills the daemon with the app (`launch.rs#L519`). `t --install-service` writes launchd `RunAtLoad`+`KeepAlive` / systemd `Restart=on-failure RestartSec=2` (`crates/platform/src/service.rs#L216-L218`, `#L243`).
- **`t subprocess` Monty workers**: pool of `max_concurrent+1` processes of the engine's own binary (`crates/agent/src/supervisor.rs#L7378-L7386`). Children of the engine via monty-pool; detachment not found → die with/after the engine; the heap is recoverable only from the last dump (§4).
- **shell() jobs**: brush spawns externals with `ProcessGroupPolicy::NewProcessGroup`; every pgid is adopted as a `ProcessTree` (`crates/tools/src/jobs.rs#L1068-L1092`). On *graceful* shutdown the `Jobs` actor gets `KillAll` and SIGKILLs every group (`jobs.rs#L645-L660`, `#L1018-L1021`); the per-job charge is released as `"killed at shutdown before it settled"` (`#L986`). On SIGKILL of the engine nothing runs: job process groups are **orphaned and keep running** (own pgid, stdout to a pipe whose reader is gone; `setsid` escapees explicitly allowed, `#L646`). Rows live only in memory (`14-jobs.md#L304-L311`).
- **PTY jobs**: same `Jobs` actor, `portable_pty` child in its own group (`crates/tools/src/pty.rs#L1-L5`, `#L21-L22`); same fate as shell jobs. Screen/scrollback buffer is in memory (`PtyHandle`, `pty.rs#L468-L473`); only the log file is on disk.
- **ACP harness children (omp/opencode)**: spawned by the `agent_client_protocol` SDK's `AcpAgent::new(config)` over stdio (`crates/acp/src/harness.rs#L427-L439`); `Link.close()` drops a oneshot that ends the connection, doc says "kills the process group" (`#L752-L757`). Process-group/`setsid` handling is inside the SDK — not found in theuth code. Engine SIGKILL closes the child's stdio; whether the harness exits is the harness's choice (knowledge: omp acp exits).
- **Sensors (CPython shim)**: `kill_on_drop` + `setsid` (`crates/agent/src/sensor/cpython.rs#L51`, `#L63`).
- **Sandbox VMs**: microsandbox SDK; mounts/ports pending spec is "process-local; after an engine restart the actor re-issues set_mounts/set_ports from its record before `start`" (`crates/sandbox/src/vm/microsandbox.rs#L55-L57`); existing VMs are *adopted* on `create` (`#L451`). Whether the VM process survives engine death: not found in theuth code (it is the msb runtime's process).

## 2. Startup sequence (agent plugin, `supervisor::start`)

1. Read `agent:`/`surface:` config; load or create mesh identity (sessions.owner = engine id) — `supervisor.rs#L7308-L7323`.
2. `Store::open(sessions.sqlite)`: WAL, busy 5 s, `sqlx::migrate!`, backfill `UPDATE sessions SET owner = ? WHERE owner = ''`, seed `seq` from `MAX(seq) FROM session_log` and the HLC clock from `MAX(hlc)` — `store.rs#L515-L585`.
3. **`unresolve_open_calls`**: `UPDATE model_calls SET status = ?, settled_at = ? WHERE status IN (?, ?) RETURNING owner_id` (pending/dispatched → `unresolved`, "neither lost nor resent") — `store/accounting.rs#L467-L472`, called `supervisor.rs#L7325-L7330`.
4. Background: replicate `projects`/`cost_owners` once the replica actor exists — `#L7331-L7351`.
5. `private::sweep_stale()` removes scratch dirs of dead processes (lock-proven) — `#L7352-L7356`.
6. Vault, providers, Monty pool, `spill::sweep` (7 days), catalog, model resolutions — `#L7357-L7428`.
7. `load_topology`: mirror persisted roots and parent links into the communication policy, parents first — `#L7441`, `#L7630-L7661`.
8. **`fail_interrupted("interrupted_by_restart")`**: `UPDATE peer_exchanges SET state = 'failed', error = ?, updated_at = ? WHERE state = 'accepted'` and `UPDATE a2a_tasks SET status = 'failed' … WHERE status NOT IN ('completed','failed','canceled','rejected')` — `store/communication.rs#L1445-L1476`; logged `"communication: {} accepted exchange(s) and {} task(s) failed: interrupted_by_restart"` (`supervisor.rs#L7442-L7450`). Nothing re-runs.
9. `Agents` actor spawned with every map empty (`sessions`, `waits`, `bound`, `delivered`, `owned_children`, `last_stop`, `seen`, `approvals` …) — `#L7451-L7489`.
10. After the activity sink registers: `ledger.certify()` then **`rearm_reminders`**: rows whose session no longer exists are deleted, the rest `tell(RearmReminder{…})` to the jobs actor — `#L1942-L1980`; overdue ones fire at once with `late="true"` (`crates/tools/src/reminder.rs#L139-L140`, `#L289-L290`).
11. **Sessions are loaded lazily**: `Agents::session()` on first input/`LoadSession` reads the row, `store.frame(id)` (latest epoch head + messages ≥ tail_from), todos; `resumed = epoch > 0 || !history.is_empty()` and the activity ledger is told the row "belonged to an earlier process whose work cannot be proven stopped" — `supervisor.rs#L596-L628`, `#L706`. No eager replay of any session at boot.
12. Other plugins at boot: **schedule** `UPDATE runs SET finished_at = ?, result = 'lost' WHERE finished_at IS NULL` (`crates/schedule/src/store.rs#L46-L67`); **tasks** release the `dispatching` placeholder claims only (`crates/tasks/src/lib.rs#L1285-L1289`), then 30 s sweep / dispatch ticks (`#L1304-L1311`); **meeting** re-queues `Post`/`Summarizing` manifests and re-sends the summary form "after a restart" (`crates/meeting/src/lib.rs#L3495-L3504`, `#L3428-L3433`).
13. UI signal: TUI PTY viewer shows `"Daemon restarted."` when a page's daemon epoch changes (`crates/tui/src/model.rs#L2027-L2030`, `#L2230`); `PtyScreen.epoch` "Daemon epoch; `None` until the harness has one" (`pty.rs#L102-L103`). A session-level "restarted" event to the model or chat UI: not found.

## 3. Durable records inventory (`crates/agent/migrations/`)

| table | key | written | read on restart |
|---|---|---|---|
| `sessions` (0001 + many ALTERs: name, effort, cut_at, agent, acp_session, cwd, pinned, todo_version, context_revision, scope, alias, project, archived, tags, harness_model, unattended, owner, epoch, desk, harness_commands, context_size, acp_skills) | `id` | at create / each setting change | lazily per session; `owner` backfilled at open |
| `messages` | `(session_id, seq)` + excluded, vision, at, thinking_ms | `Turn::persist` → `Store::append` after each row is final (§4) | `Store::frame` on first touch |
| `epochs` | `(session_id, n)`; `at_seq, tail_from, head` | at compaction | frame = latest epoch |
| `todo_snapshots` | `(session_id, at_seq, k)` | with the message that changed todos | yes (`store.todos`) |
| `errors` (rebuilt 0023, `id TEXT PK`) | `at_seq` | on `record_error` | UI only |
| `inner_calls` (0020) | `(session_id, parent_id, n)` | `INSERT OR REPLACE` at `ToolCallStarted` (output NULL), completed at finish — `store.rs#L1392-L1456`; "Never part of the model frame" | UI reload only |
| `peer_exchanges` (0011) | `id` UNIQUE, `UNIQUE(sender, request_id)`, `fingerprint`, `state` ∈ accepted/consumed/replied/suppressed/failed, `history_seq` | accepted before ack; consumed sets `history_seq` once (`store/communication.rs#L1114-L1115`) | `fail_interrupted` fails every `accepted` |
| `peer_bindings`, `a2a_contexts`, `a2a_tasks`, `a2a_task_work` (0011) | ids | on bind / request | a2a_tasks non-terminal → failed |
| `cost_owners`, `model_calls`, `remote_meters` (0015), `remote_turns` (0029/0031) | `call_id` PK; `request_id, attempt, turn_id, status` | `reserve_call` (pending) → `mark_dispatched` "commit before invoking transport" → `settle_call` (`accounting.rs#L328-L416`) | open calls → `unresolved` |
| `projects` (0018) | name | UI | replicated via record replica |
| `reminders` (0019) | `(session, id)`; `message, set_at, fires_at, seconds` | insert on `ReminderEvent::Set`, delete on cancel/fire (`supervisor.rs#L1983-L2017`) | **yes — re-armed** |
| `session_log`, `session_cursors`, `session_epochs` (0023) | `seq` PK / `(origin, peer)` / `session_id` | one log row per touched session per committed write tx (§7) | seq + HLC seeds; sync Hello |
| **not in sessions.sqlite** | `tasks.sqlite`: tasks, wakes, task_events, write_budget, packets (`crates/tasks/src/store.rs#L1`); `schedule.sqlite`: runs (`schedule/src/store.rs#L1`); runtime dumps `<data_dir>/runtime/<session>.monty` + names JSON (`repl.rs#L1003`, `#L1353-L1379`); job logs; artifacts store; meeting manifests | | |

**No table exists for**: approvals, jobs/PTYs, ask_user/desktop_handoff questions, in-flight tool calls, send_message waits, subagent ownership (`owned_children`), the streaming assistant text.

## 4. The turn loop under a kill (`crates/agent/src/loop_.rs`)

- **Input accepted**: `take_input` persists the user row first; the ticket is acknowledged (`InputConsumed`) only "once the store has it" — `#L3175-L3210`. A peer input is consumed durably (`consume_exchange` sets `state='consumed', history_seq`). Kill after this: the user row is on disk; on resume the frame ends in a user message → the next input simply continues (no auto-run of the pending turn: "auto-runs nothing", `00-architecture.md#L484`).
- **Frame built / model call streaming**: `reserve_call` + `mark_dispatched` rows exist. The assistant text is captured in memory during `stream_once` and persisted **only at End** (`persist_timed(ChatMessage::assistant(content), thinking)` after `end.captured_content`, `#L4025-L4043`; tool-use reply at `#L4079-L4082`). Kill mid-stream: **everything streamed is lost**; `model_calls` row becomes `unresolved` at next start. The user sees the partial text vanish on reload; the model sees nothing of it.
- **Tool calls**: the assistant `tool_use` row is persisted *before* execution (`#L4082`), `ToolCallStarted` published (`#L4106`), registry batch runs in parallel, python cells after (`#L4142-L4192`). **All tool results of the batch are persisted as one tool message after every call returns** (`#L4202-L4333`). So: side effect happens, then the result row. Kill between: side effect done, no record. Next turn start: `repair_dangling` appends `"ERROR: not run: the harness exited before this tool completed"` for every call in a trailing assistant row (`#L2758-L2782`, called at turn start `#L3401`) — the model is told "not run" even when the effect did happen (false negative by design; note the pair-invariant motive, 36 §3).
- **Pending approval**: `Approvals.pending: HashMap<ApprovalId, (ApprovalRequested, oneshot::Sender<Decision>)>` with a timeout → Deny (`crates/tools/src/approval.rs#L95-L96`, `#L120-L140`). In-memory only; a kill loses it; after restart the UI has no card (snapshot derives from `pending`, `#L176-L188`) and the tool row is repaired as "not run". Approval precedes `Start`, so a job never ran unapproved (`14-jobs.md#L317-L319`).
- **Python cell mid-flight**: `Repl::run` dumps heap + inventory **after every cell** (`repl.rs#L1292`, `#L1353-L1379`, atomic rename `#L1786`); a crashed worker restores the previous dump and the model sees `"RuntimeError: runtime crashed ({e}); state restored to after the previous cell — this cell's effects are lost"` (`#L1231-L1244`). On an *engine* kill there is no such message: the cell's `tool_use` row is repaired as "not run" (above); the next turn's `## Runtime` inventory comes from `repl::saved_inventory` (`loop_.rs#L4798`, `repl.rs#L820`). Inner tool calls already executed inside the cell stay executed; their `inner_calls` rows stay with `output NULL` (started, never finished). A dump that fails to restore is renamed `.stale` and reported as `"<runtime> saved state could not be restored … the runtime is empty."` (`repl.rs#L1055-L1080`).
- **Subagent spawn**: the child session row + `child:<id>` scope are durable; `owned_children`, `child_turns`, `permits` are in memory (`supervisor.rs#L186-L199`, `#L7477-L7484`). The child's result row is accepted with `request_id = "result:{turn}"` from `session:{child}` to `session:{parent}` ("Durable dedup: this turn's result was already accounted for") — `#L1612-L1636`. After restart: a child that finishes is not running (no actor); a result accepted but not yet consumed was **failed by `fail_interrupted`** and is never injected; the parent only learns via the UI/`session_read`.
- **`send_message await_reply`**: `Wait` entries in `Agents.waits` (`#L240-L250`, `#L7473`); `PeerMessagePage` doc: "Pending sets derive from live waiters, never abandoned oneshots, so after a daemon restart both are empty and a request's durable outcome is its `state`" (`communication.rs#L520-L522`). The reply, if it arrives, still lands as a durable `replied` row; the awaiting tool call is repaired "not run".
- **`ask_user` / `desktop_handoff`**: oneshot in `AskUser.pending`, timeout → dismissed (`crates/tools/src/ask.rs#L164-L189`). In-memory only.
- **Assistant row committed**: final `persist_timed` → `TurnFinished`. `last_stop` is in memory (`supervisor.rs#L190-L191`), so after restart a cold child reports no `finished_at` (known WAKE-LIVE-CHILD issue).

## 5. Jobs, PTYs, reminders, schedules, meetings

- **shell() jobs**: "Jobs are in memory; the log file is durable. If the harness stops while a job runs, the process group is killed (§2) and the job is gone. The next time that session's actor is created … `Jobs` has no row, so nothing is injected. **Rejected for v0:** a `jobs` table so a resumed session gets `<job_result exit="lost">`" — `docs/design/14-jobs.md#L304-L311`. The model gets no `<job_result>`; its `shell()` call row (if mid-turn) is repaired "not run". Under SIGKILL the process group is *not* killed (no `KillAll`), so the job keeps running headless.
- **PTY jobs**: identical actor; screen buffer in memory; no persisted screen (`pty.rs#L468-L473`). Idle/`<job_idle>` wake-ups vanish.
- **remind_me_in**: durable row at set, deleted at cancel/fire (`supervisor.rs#L1983-L2017`); re-armed at start by `rearm_reminders` → `RearmReminder`, overdue fire immediately `late="true"` (`#L1949-L1980`, `reminder.rs#L97`, `#L289-L290`). Confirmed survival mechanism.
- **Schedules/loops**: files are the definition; run history in `schedule.sqlite.runs`; a run with `finished_at IS NULL` at open becomes `result='lost'` (`schedule/src/store.rs#L63`). `runs`/`until`/`max_runs` counts are "a query over it" at boot (`#L1-L3`). The in-flight run's session is an ordinary session (lazy resume, no auto-continue). `mode: restart` is a watcher option, unrelated to engine restart (`schedule/src/lib.rs#L200`).
- **Tasks**: `tasks.sqlite` durable; only `dispatching` claims are released at start (`tasks/src/lib.rs#L1285-L1289`); a wake claimed by a desk stays claimed; `sweep` every 30 s orphans `doing` checkouts whose engine is unreachable ≥ `orphan_secs` (120) or whose session is stale ≥ `stale_secs` (900) with no live tree, `epoch + 1`, takes the session when it is on another engine (68 §4.4), and wakes the owner `Resume` (`ledger.rs#L1893-L1995`; config `tasks/src/config.rs#L10-L13`, `#L31-L32`). A local engine restart does **not** by itself orphan or resume anything.
- **Meetings**: `Post`/`Summarizing`/ended-`Stopping` manifests are resumed or re-queued at boot (`meeting/src/lib.rs#L3495-L3504`); live capture meters "not persisted" (`#L287`); a live transcript batch in flight is an ordinary turn (lost like §4).

## 6. Subagents and external harness children

- **theuth subagents**: own session rows with `parent`; resumed lazily like any session on the next input (`supervisor.rs#L596-L628`); no auto-resume. Parent learns a post-restart result only if the child *finishes a new turn after restart* (then `hand_off` writes a fresh exchange row); results accepted-but-unconsumed before the kill are `failed: interrupted_by_restart`. Dedup key `(sender=session:child, request_id="result:<TurnId>")` + fingerprint (`#L1612-L1621`; `07-context.md#L977-L978`).
- **ACP children**: `sessions.acp_session` stored on `Connected` (`acp_child.rs#L1496`); respawn passes `resume = store.acp_session(id)` and stored `cwd` to `session/load` (`supervisor.rs#L887-L911`, `harness.rs#L478`, `#L555` "acp session/load failed, starting a new session"); a stored id not restored records `"{name}: previous harness session not restored; starting fresh"` (`acp_child.rs#L1476-L1495`). `Shutdown` → "The supervisor then respawns from the row with `session/load`" (`#L350-L353`); `resume_child` exists (`#L1809` "harness replaced by resume_child") and a cancelled replayed turn gets `CANCELLED_NOTE` (`#L364-L366`). Nothing respawns a child at engine start; the next input/`send_message` to its session does. Delta handoff: not found (the "previous attempt interrupted" paragraph is written by hand today).
- **Sandbox VMs**: see §1; the actor re-issues mounts from its record (`microsandbox.rs#L55-L57`) and adopts existing VMs (`#L451`).

## 7. Design 68 replication as a durability primitive

- Single write connection with a `sqlite3session` attached; **every** mutating store method runs in one `WriteTx`; `commit()` takes the changeset, splits it by session, refuses the whole tx with `StoreError::NotOwner` if any touched session is foreign, inserts `INSERT INTO session_log (seq, session_id, epoch, changeset, hlc)` per session **inside the same transaction**, then commits and bumps a `watch` (`store.rs#L164-L264`; `68-sessions.md#L47`). It is a *physical after-image log* (sqlite changeset of committed rows), not a write-ahead journal of intents: a row is logged iff it committed, atomically. Not attached: `projects, cost_owners, peer_exchanges, peer_bindings, a2a_*`, and the 68 tables (`68-contract.md#L46`).
- HLC: `replica::Clock::new(hlc_origin(&me), MAX(hlc))` at open (`store.rs#L571-L581`; `68-contract.md#L16`). Origin `"local"` when no engine id — such rows never ship.
- Ownership: `session_take(id, reason ∈ user|write|orphan)`: ask owner to `Release` (stop actor, flush, last seq) within `release_wait_secs` 10, or if unreachable ≥ `takeover_secs` 300 (orphan takes skip the gate) write `owner=B, epoch=e+1` — `68-sessions.md#L59-L64`; `sessionsync/src/owner.rs#L57-L100`, `#L157-L175`. Stale owner demotes, marks unshipped rows `orphaned`, surfaces them once, prunes after 7 days (`#L65-L67`; `owner.rs#L209-L225`). "Resume takes the lost engine's session": the tasks sweep calls `taker.take(&c.session)` and the `Resume` wake lands on the taken session, else "resume from the last progress note" (`ledger.rs#L1949-L1992`; `68-sessions.md#L68-L70`).
- As a journal of intents: no — it records only committed rows and never the in-flight items of §4 (streams, approvals, waits, jobs). It *does* give exactly-once ordered replay of committed state to a follower (`sync.rs#L2-L12`: Hello → snapshot/log → live; applied cursors in `session_cursors`), and the outbox prune keeps an offline peer's catch-up (`replica/src/engine.rs#L522-L529`).

## 8. Known gaps (lost on kill)

1. Streaming assistant text before End — `loop_.rs#L4025-L4043`.
2. Tool results of an in-flight batch; effects already applied but recorded as "not run" — `#L4202-L4333`, `#L2758-L2782`.
3. Pending approvals — `approval.rs#L95-L96`; ask/handoff — `ask.rs#L164-L189`.
4. `send_message` waits / bound requests — `supervisor.rs#L240-L250`, `communication.rs#L520-L522`.
5. Accepted-unconsumed exchanges and non-terminal a2a tasks fail — `store/communication.rs#L1442-L1476`.
6. shell/PTY jobs, screens, `<job_result>`/`<job_idle>` — `14-jobs.md#L304-L311`; under SIGKILL the processes also leak.
7. Monty cell in flight; inner_calls rows left with `output NULL` — `repl.rs#L1292`, `store.rs#L1392-L1456`.
8. Last turn stop/finished_at, input dedup `seen` — `supervisor.rs#L190-L197` ("ponytail: process-local, lost on restart; persist beside the row if retries must survive engine restarts").
9. Owned children / child turns / permits — `#L186-L189`, `#L7477-L7484`.
10. Network a2a correlation — `communication/network.rs#L407` "Rebuilt empty on restart".
11. Per-session communication opt-outs, compaction overrides, pin — `communication.rs#L422`, `lib.rs#L364`, `#L415`; channel session switches "memory-only" (`slack/src/commands.rs#L944`).
12. Sandbox pending mounts — `microsandbox.rs#L55-L57`; decision capture map — `decision/src/tool.rs#L29`.
13. Meeting meters — `meeting/src/lib.rs#L287`.
14. Operation ids / live proof of work — `tools/src/activity.rs#L58-L60` ("an unclean restart loses the live proof anyway").
15. Schedule run in flight → `lost` — `schedule/src/store.rs#L63`.
16. ACP child process and its harness-side context unless `session/load` succeeds — `harness.rs#L555`, `acp_child.rs#L1476-L1485`.

## 9. Idempotency & effects

- `peer_exchanges`: `UNIQUE(sender, request_id)` + `fingerprint` ("a reused request id with changed content is a conflict, not a retry") — `migrations/0011_communication.sql#L10-L41`; child results use `request_id = "result:{turn}"` (`supervisor.rs#L1613`).
- `model_calls.call_id` PK with `request_id, attempt`; reserve → dispatched (committed before transport) → settle "replay-safe: the row is updated, never appended" (`accounting.rs#L328-L384`).
- Tool call ids: provider `call_id`; inner `<parent>.<n>` (`store.rs#L503`); `inner_call_started` is `INSERT OR REPLACE` (`#L1404`).
- Tasks: checkout `epoch` bumped on orphan (`ledger.rs#L1948`); wakes `ON CONFLICT(key) DO UPDATE … claimed_by = NULL` (`tasks/src/store.rs#L362-L364`).
- App invocations: "a running row survives a restart as `unknown`" (`artifact/src/store.rs#L2247-L2248`); input ids: process-local `seen` (`supervisor.rs#L195-L197`).
- **Not journaled**: every tool side effect (fs writes, git, shell, HTTP, MCP, send_message dispatch) — effects precede the single batch result row (§4); no outbox/intent log exists.

## 10. Ten sharpest facts

1. Nothing re-runs after restart: boot only *marks* (`unresolved`, `failed: interrupted_by_restart`, `lost`) — `supervisor.rs#L7325-L7330`, `#L7442-L7450`, `schedule/src/store.rs#L63`.
2. Sessions are never loaded at boot; resume happens on first input via `store.frame` — `supervisor.rs#L596-L628`.
3. The user row is committed before the ticket is acknowledged; assistant text only at stream End — `loop_.rs#L3203-L3210`, `#L4041-L4043`.
4. Tool-use row before execution; one tool-result row after the whole batch; dangling calls are repaired as "not run" regardless of real effect — `#L4082`, `#L4329-L4333`, `#L2769-L2771`.
5. Reminders are the only in-flight waiters that survive (durable row + re-arm, overdue fire `late`) — `supervisor.rs#L1949-L1980`.
6. Approvals, asks, awaits and jobs are oneshots/HashMaps with no table — `approval.rs#L96`, `ask.rs#L167-L169`, `supervisor.rs#L7473`, `14-jobs.md#L304-L311`.
7. Every store write is one tx that also appends a per-session `session_log` changeset row — a committed-state log, not an intent journal — `store.rs#L164-L261`.
8. `fail_interrupted` fails *every* accepted exchange, including a child's finished result the parent had not yet consumed — `store/communication.rs#L1445-L1456`.
9. The Monty heap is dumped atomically after every cell; a cell cut by an engine kill leaves the previous dump and no runtime message to the model (only the "not run" repair) — `repl.rs#L1292`, `#L1353-L1365`.
10. ACP children are resumed only on demand via stored `acp_session`/`cwd` + `session/load`; a failed load is logged, recorded as an error, and starts fresh — `supervisor.rs#L887-L911`, `acp_child.rs#L1476-L1498`, `harness.rs#L555`.
