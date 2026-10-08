# 146 RESTART-CLARITY — design + as-built

Branch `fix/146-restart-clarity`, worktree `theuth-146`, from `origin/main` `8fbade19`. Brief:
`~/.theuth/briefs/2026-10-08/146.md`; recon: `143-recon/report.md`. Background: 109 §8 (ACP
children), 139 as-built §7 (`child_result` waits). Never touches `crates/acp/**` or
`crates/agent/src/acp_child.rs` (141 owns them).

## 1. What happened (recon, verified)

Restart 14:12Z: two full boots 3 s apart (rpc :53279, :53379). The boot respawn of ACP children
(`respawn_acp_children`) was triggered only by `child_waits(Here)`; an orchestrator that had ended
its turn had no such wait, so its three mid-prompt omp children (125R1 W1–W3) were never respawned,
nobody was told, and the dead children's dangling `ToolCall` rendered as a running tool card.
`open_turns` ran 4× per boot at ~1.1 s each (correlated `MAX(at)` per `turn_steps` row).

## 2. Decisions (orchestrator, 15:35) and how they landed

| # | decision | as built | files |
|---|---|---|---|
| D1 | Respawn every mid-prompt ACP child at boot, whatever the parent's wait state | Trigger set = `child_waits(Here)` ∪ `Store::midprompt_acp_children(since)`: ACP children here (`agent IS NOT NULL` — the harness marker `set_acp` writes; `acp_session` arrives only once the harness connected —, `parent IS NOT NULL`, `kind != 'advisor'`, `updated_at > now − acp_respawn_window_days`) whose **last `messages` row** is `User`, `Tool`, or `Assistant` with a `ToolCall` part. Parent gate: row exists, `here`, not archived. Deduped by child; count logged (`146: mid-prompt ACP children in the window n=…`) | `store/durability.rs`, `supervisor/acp_resume.rs`, `durability/mod.rs` (config) |
| D2 | After `resume_scan`, ≤ `acp_respawn_parallel` (4) at once, through `respawn_acp_child` | `Agents.boot: Option<Boot>` holds the queue; `pump_respawns` spawns while `inflight < parallel`; a slot frees on the child's first real `TurnStarted` (harness accepted the prompt → *respawned*) or a `TurnFinished` before one (its `fail` turn → *lost*). The actor already publishes `TurnStarted` after `session/load`; nothing was added around the call | `supervisor/acp_resume.rs`, `supervisor.rs` (`AgentEvent` hook) |
| D3 | Tell the parent about every child that cannot come back | `lose()`: `append_local_note(child, "interrupted by the engine restart at <t>; not respawned: <why>")` + `inject(parent, …)` of `<child_lost name session at why>` with the one-line advice (Automatic origin; cross-engine via 139 R2). Reasons: parent archived / owned elsewhere / gone (no inject: nobody to tell), child owned elsewhere, harness not local, `cwd` unreadable, spawn error, harness did not connect | `supervisor/acp_resume.rs` |
| D4 | One `resume:boot:<ts>` attention item with live counts + one INFO line | `agent::durability::BootPass { at_ms, resuming, respawned, waiting, lost: Vec<(session, name)> }` published on the bus by the pass and on every settle; `Attention` keeps the latest (`Inputs.boot`), `derive` emits `resume:boot:<at_ms>` (title `boot_title`: "Restarted 14:12 · 3 sessions resumed · 2 subagents back · 1 waiting · 3 lost (a, b, c)"), `session` = first lost child so the HUD's Dismiss renders; a count change with the same id publishes an empty-diff `AttentionChanged` so the HUD refetches; settled + lost = 0 → `ClearBoot` 60 s later; lost > 0 → stays until dismissed. Log: `146: boot pass resuming= respawned= waiting= lost= lost_names=` | `roster/src/attention.rs`, `durability/mod.rs` |
| D5 | Boot query: one window pass, index, computed once | `NEWEST` = `ROW_NUMBER() OVER (PARTITION BY session_id, turn_id ORDER BY (kind = 'assistant_partial'), n DESC)` + `MAX(at) OVER (PARTITION BY …)` over the whole table (partials sort last, `last_at` still sees them); migration **0042** `turn_steps_turn_at(session_id, turn_id, at)` (0041 is 141's `observed_children`); `OpenTurnRow` alias; `ActivityRegistered` computes `open_here()` once and passes it to `reap_jobs_in`, `rebuild_waits`, `resume_scan`, `deliver_spared_results`; `reap_jobs(store)` kept for other callers | `store/durability.rs`, `migrations/0042_turn_steps_turn_at.sql`, `durability/jobs.rs`, `supervisor/{resume,waits}.rs`, `supervisor.rs` |
| D6 | flock single-instance guard | **Already existed**: `rpc::engine::acquire` (`File::try_lock` on `<data_dir>/engine.lock`, main.rs before `Runtime::new`). What was missing: a refused headless/desktop daemon exited **3**, which launchd (`KeepAlive.SuccessfulExit=false`) / systemd (`on-failure`) restart-loop. Now `Host::Headless \| Desktop` print the holder line (`engine already running (pid …)`) and exit **0**; TUI/-p/--acp keep 3. Desktop `launch.rs::start`: a daemon that exits before listening is followed by one `probe()` — attach to the holder instead of an error (10 lines) | `theuth/src/main.rs`, `desktop/src/launch.rs` |
| D7 | UI ≤ 40 lines | (a) `interruptDangling(entries)` in `reduce.ts`, applied in `loadHistory` when `loaded.agent && !loaded.running`: the trailing result-less `tool` entry gets `interrupted: true`; `MessageTool` drops the spinner and shows "interrupted by restart" greyed. (b) `TextDelta`/`ToolCallPending`/`ToolCallStarted` set `running: true`; `trackRow` wakes an `idle` parent row to `running` on those events (an `awaiting` row is kept). Vitest ×2 | `ui/src/store/{reduce,sessions}.ts`, `ui/src/components/chat/MessageTool.tsx`, `reduce.test.ts` |

## 3. Deviations from the brief (and why)

- **`finished_at` is per turn, not per session**: every active orchestrator has it set (last turn's end);
  ACP children never set it. The brief's `finished_at IS NULL` test would have excluded every live parent,
  so the parent gate is *exists ∧ here ∧ not archived* and the child filter drops it.
- **Mid-prompt predicate includes a trailing `Tool` row** (tool result the model never answered): on the
  backup DB 5 of 25 mid-prompt children were in that state; the brief named only `User` / dangling `ToolCall`.
- **"Window exceeded" is not a lost reason**: children outside `acp_respawn_window_days` are not scanned
  (bounded scan is the point); nothing is written for them.
- **Migration number 0042**, not 0041 (taken by 141's `observed_children` in `theuth-141*`).
- **D6 already existed** (63 §0.1); the change is the exit code + desktop attach, not a new lock.
- **D7(a) marks the tool entry (`interrupted: true`) instead of converting it to `kind:"interrupted"`**:
  the `interrupted` kind renders through `MessageInterrupted` ("interrupted — resuming"), not the ToolCard,
  and loses the call's name/args. The flag keeps the card and the brief's wording.
- **Parent gone (row deleted)**: note on the child only — there is no session to inject into.
- **A connect failure also yields the child's own `<subagent_result stop="error">`** (109 §7.3) plus our
  `<child_lost>`: two messages, both true; kept for uniformity of the lost rule.
- **A quiet boot (0 resumed, 0 respawned, 0 lost) raises no item** — the brief did not say; a notice on
  every boot would be noise.

## 4. Pacing ceiling (ponytail)

A respawned child whose actor dies without `TurnStarted`/`TurnFinished` (panic) keeps its slot; the
pass then never reports `waiting = 0` and the item stays. `acp_child`'s 120 s connect timeout covers the
realistic case (hung harness → `fail` turn). Upgrade path: a per-child timer that tells `Agents` a
`RespawnSettled` after `CONNECT_TIMEOUT + 10 s`.

## 5. Proof — `just restart-clarity` (`scripts/restart-clarity-smoke.sh` → `scripts/restart-clarity/smoke.ts`)

One scratch engine (`/tmp/146-smoke/engine`, fake OpenAI provider as the parent's model, the real
`omp acp` as the child). P1: spawn `w146` with "`sleep 150` then DONE", wait for its `ToolCall` row,
delete the parent's `child_result` wait (the 125R1 shape), `kill -9`, boot. P4: same with the parent
archived first. P5: second `t --headless` on the data dir. P6: `sqlite3` timing on a `.backup` copy.

| proof | result |
|---|---|
| P1 `146: boot pass` respawned=1 lost=0 (mid-prompt scan n=1, no wait row) | TBD |
| P2 omp process back ≤ 30 s; parent receives DONE later | TBD |
| P3 `list_sessions` child `running: true` during the respawned prompt | TBD |
| P4 archived parent → child lost: note, `<child_lost name="w146b">` in the parent, `resume:boot:*` "1 lost (w146b)" | TBD |
| P5 second engine exits 0 ≤ 1 s with the holder line; first unaffected | TBD |
| P6 `open_turns` on the backup (25 581 rows / 1 027 turns): old 1.12 s → new 0.085 s; with 0042: old 0.069 s, new 0.085 s | measured 15:40 (sqlite3 `.timer`), result sets identical (EXCEPT both ways = 0) |

## 6. Gates

TBD
