# 109 as built — LIVE: resumed subagents showed idle (○) after a restart

Branch `fix/109-resumed-children-live` (cut from main c9d74534).

## The bug as observed (2026-10-07 23:18)

After an engine restart a chat's subagents came back and kept working (109: native children resume
their open turn, ACP children are respawned), but their sidebar rows stayed on the hollow idle glyph
and the parent's "N subagents running" glyph stayed off.

## The seams as found

1. **Supervisor `running` set.** `Agents::state_of` answers `Running` only from `self.running`, filled
   by exactly one watcher arm: `TurnStarted`. `Turn::resume` (`loop_.rs` ~4430) publishes
   `TurnResumed`, never `TurnStarted`, so a resumed child was `Idle` to `ListAgents`, `list_sessions`
   (`SessionSummary.running`), the picker and the roster — and had no `child_turns` entry/`started_at`.
2. **ACP respawn.** `acp_child.rs` `prompt()` publishes **nothing** (sends `session/prompt`, routes
   `PromptDone`). But `Respawn` runs `queue_respawn` (front-queues the restored note / delta handoff)
   → `next` → `start` → `open`, and `open` publishes `TurnStarted` like any first prompt. The
   supervisor already read that: a respawned ACP child was never invisible to `running`.
3. **Desktop.** `reduce.ts` `trackRow` patched the parent's child row on `TurnStarted`/`TurnFinished`
   only; `apply()`'s `TurnResumed` set the banner but not `running`/`waiting`/`activity`, so
   `runningSubagents` counted nothing even for a client connected during the resume.

## What changed where

- `crates/agent/src/supervisor.rs` — the watcher arm is now
  `TurnStarted { session, .. } | TurnResumed { session, .. }`, same body (insert into `running`; a
  child with no `child_turns` entry gets a permit-free `ChildTurn`, `started_at: now`, ACP timer).
  Two tests next to the R2.4 timer test: `resumed_child_turn_counts_as_running` (bus `TurnResumed`
  → `ListAgents` row `Running` + `started_at`; `TurnFinished` → `Idle`) and
  `respawned_acp_child_counts_as_running` (fake-node child, spawn turn ended, actor told
  `Respawn(4000)` → fake gets a `<resume_handoff>` prompt, row `Running`, timer armed; `end_turn` → `Idle`).
- `crates/desktop/ui/src/store/reduce.ts` — `apply()` `TurnResumed` also sets `running: true,
  waiting: true, activity: "thinking"` (the liveness bits of `TurnStarted`; no entry/model/turnSeq
  change — there is no input text to echo). `trackRow`: `case "TurnResumed"` falls through to the
  `TurnStarted` patch (`state: running`, `started_at: now()`), no `note` → no parent chat line.
- `crates/desktop/ui/src/store/reduce.test.ts` — `109 LIVE` test: idle child row → `TurnResumed` →
  row `running` + `started_at`, parent entries unchanged, slice `running/waiting/thinking`,
  `runningSubagents` 1 → `TurnFinished` → idle, 0.
- No loop event, RPC, `AgentState`, `rpc/extra.ts` or migration change.

## Gates (worktree target, CARGO_INCREMENTAL=0)

- `cargo test -p agent --lib -- supervisor:: acp_child::`: 105 passed (incl. the two new tests).
- `cargo test -p agent`: 460 passed, 0 failed, 2 ignored (58.5 s). The first full run reported one
  lib failure the log filter dropped the name of; `--lib` and a second full run passed 460/460 (flake).
- `cargo clippy -p agent --tests -- -D warnings`: clean.
- `rustfmt --edition 2024 --check crates/agent/src/supervisor.rs`: clean.
- `just ts-desktop`: ok. `bun run check` (tsc -b): clean.
- `bunx vitest run src/store src/components/sidebar`: 9 files, 89 tests passed.

## Deviations from the brief

- Seam B: no engine change (see above). Its test lives in `supervisor.rs`, not `acp_resume.rs`: it
  needs the fake-node helpers (`node`, `spawn_on`, `tap`) only there, and `respawn_acp_child` refuses
  a node child (local-harness gate), so the actor is told `Respawn` directly; the gate itself stays
  covered by `respawn_decision_table`.
- `trackRow` sets `started_at: now()` (what `TurnStarted` sets), not the brief's `row.started_at ?? now`.
- `rustfmt` on `supervisor.rs` also reflowed one pre-existing unformatted call (`human_input`, ~2762).
