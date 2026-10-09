# 155 ACTIVITY — as built (closer Z, 2026-10-09)

Branch `integrate/155` in `~/Develop/theuth-155-int`, skeleton `ec3b1d2d` (contract `docs/design/155-activity.md`), base `origin/main` 6aadda9a. Three omp workers merged `--no-ff`, then `origin/main` (design 153) folded, then the hand-offs, gates and the five live proofs below.

## 1 Merges and the fold

| step | commit | from | content |
|---|---|---|---|
| 1 | `4e02b83e` | `feat/155-w2` 9b2e043b | PTY signals: `platform::ProcessTree::sample`, `tools::proctree::TreeQuiet`, `Idle::defer`, reaction check, `<job_idle why>` |
| 2 | `32e65702` | `feat/155-w1` 84e55387 | engine activity: `ChildActivity`, `StallCheck`, `<child_stalled>`, `wait_peer` + `peer_state` waits, `acp::Reports`, `stalled_secs` |
| 3 | `9aa205b1` | `feat/155-w3` 18332813 | alerts: `roster::alert`, rpc masking + per-connection seen, desktop `AlertNotifier`, companion `Alert` policy + panel focus |
| 4 | `362a9d2e` | `origin/main` 253f1ada | fold of 153 VOICE-ANYWHERE (80 commits, 114 files) |
| 5 | `296c0d38` | Z | H1: `lose()` settles `peer_state` waits; rig test |

All three worker merges were clean. **The fold had zero textual conflicts**: 153's additions (`ModelRole::Polish`, polish.rs, the 17 rpc methods, `dictation_start(opts)`, the companion voice parts, the removed hand-written `VoiceStatus`) and 155's additions landed in disjoint hunks of `supervisor.rs`, `lib.rs`, `api.rs`, `export.rs`, `model.rs`, `api.ts`/`extra.ts`, `Models.swift`, `Store.swift`, `SettingsWindow.swift`, `CompanionCore/*`. Both sides are in; nothing was dropped or hand-edited. The semantic check is the gate set below, all green on the folded tree.

## 2 Hand-offs

- **H1** `crates/agent/src/supervisor/acp_resume.rs` `lose`: `self.check_peer_waits(child).await` after the `<child_lost>` inject (inside the `if let Some(parent)` arm, outside the "no longer exists" filter, so a wait on a vanished row also settles). `Boot::for_tests()` (`#[cfg(test)]`) and `lose` made `pub(super)` for the rig test `supervisor::activity::tests::lost_child_settles_its_wait_at_once`: a `done` wait on a child whose actor is gone receives `<child_lost>` then `<peer_state state="not_running">` ("w1 is gone: …"), the `waits` row and the in-memory entry are cleared.
- **H2** `BUILD_SLOTS=1 BUILD_LABEL=155 build-lock mbx test -p rpc --features ts --lib --quiet export_bindings` → 8 passed; `cp -R bindings/. crates/desktop/ui/src/rpc/bindings/` produced **no diff** (843 files identical: W3's `Alert.ts` and 153's `DictationOpts.ts` were both already committed). No tsc fallout.
- **H3** worker deviations from the contract (kept, not re-litigated):
  - W2: `StartTool::new` has no `thresholds` param — `PtyThresholds` lives on the `Jobs` actor; `TreeQuiet::saw(None)` keeps the last verdict; `platform::process::TREE_MAX` is `pub`; `Run.child` is `Option`.
  - W1: `PeerFacts.dead` is always `false` (the dead-connection edge is in W1's notes; a dead harness surfaces as the actor stopping → `live: false`); `idle` is satisfied by `done`; `wait_peer` on a `not_running` child is an error at registration; native children count `reports` all-true; the boot re-arm runs `check_peer_waits` on the child's first transition, not at boot.
  - W3: the companion focus call site is `Sources/Companion/AttentionPanel.swift` `PanelController.syncFocus()` → `RpcClient.setEventFocus`; desktop legacy notifications are held `LEGACY_GRACE_MS` = 3 s (`crates/desktop/ui/src/lib/alerts.ts:10`) for the engine's decision; `AttentionItem.engineDecides` exists in the Swift model (`Models.swift:72`); seen is per rpc connection, not per device (C-g).

## 3 Gates (folded tree 296c0d38; `BUILD_LABEL=155 ~/.theuth/bin/build-lock mbx …`, CARGO_BUILD_JOBS=4, logs in `/tmp/155/`)

| # | command | result |
|---|---|---|
| 1 | `cargo fmt -p agent -p acp -p tools -p platform -p roster -p rpc -p tui` (bare) | clean; `cargo fmt --check` on the same set exit 0 |
| 2a | `mbx test -p agent -p acp -p tools -p platform -p roster -p rpc --no-run` | exit 0, 0 warnings (`gate2-norun.log`) |
| 2b | bare `mbx test -p agent -p acp -p tools -p platform -p roster -p rpc` | all green: agent 606 passed/2 ignored, tools 192/1 ignored, rpc 79 + alerts 16 + prompts 2, roster 56, acp 47, platform 6, notebooks 1+1. No `jobs::`/`shell::*probe*` flake on this run (`gate2-run.log`) |
| 3 | `mbx clippy -p agent -p acp -p tools -p platform -p roster -p rpc --tests` | exit 0, 0 warnings (`gate3-clippy.log`) |
| 4 | `BUILD_SLOTS=1 … mbx check --workspace --exclude desktop --tests` | exit 0, 0 warnings (`gate4-check.log`) |
| 5a | `crates/desktop/ui`: `bun install`, `packages/apps`: `bun install` | 622 / 484 packages |
| 5b | `bunx tsc --noEmit -p .` | exit 0, 0 errors |
| 5c | `BUILD_SLOTS=1 … bunx vitest run` | 237 files, 1567 tests passed |
| 5d | `bun run build` | built in 6.26 s |
| 6a | `swift build --package-path crates/desktop/companion` | Build complete (81 s) |
| 6b | `crates/desktop/companion/test.sh` | 237 tests in 39 suites passed |

Nothing red; no fix-ups beyond H1.

## 4 Live proofs (`t` 2.14.0 from this tree, `/tmp/155/t`; data `/tmp/155/data`; left in place)

Setup, as the brief: BLOCK-style `config.yaml` with `agent: {model: anthropic/claude-haiku-4-5, communication: true, subagents.max_runtime_secs: 600}`, `secrets: {key_store: file}` (no secrets copied), `tools: {approvals: false}`, `rpc.listen 127.0.0.1:7798`, `mesh: {relay: none, discovery: false, bind: 127.0.0.1:0}`, `enabled: false` for slack telegram signal link nodes update dreaming browser computer meeting codeindex web scrape routes sandbox a2a decision voice talk schedule. **Lowered for the proofs**: `agent.activity.first_update_secs: 15`, `agent.activity.silent_secs: 30`, `tools.pty_idle_secs: 20`. Fake harness `/tmp/155/fake.py` (python3, newline JSON-RPC: answers `initialize`, `session/new`, holds `session/prompt` until a control file `/tmp/155/fake/<tag>.cmd` says `tool_start` / `tool_done` / `text …` / `end`), registered in `acp/agents.yaml` as `fake` with `reports: {permission: true, turn_end: true, session_load: false}`. Driver `/tmp/155/call.ts` (bun, `scripts/apps/rpc.ts`), events in `/tmp/155/events.jsonl`, proof 4 in `/tmp/155/proof4.ts` (`proof4.out`). Evidence = `messages` rows (`sqlite3 -readonly …/sessions.sqlite`) and wire events; times UTC.

The engine must inherit `ANTHROPIC_API_KEY`: the first start from a kernel without it failed every turn with `ApiKeyEnvNotFound` and was restarted from a shell that has it (nothing printed).

| # | proof | result | evidence |
|---|---|---|---|
| 1a | Stall `no_first_update` | **PASS** | parent `01a12248-9711…`, child kid1 `01a12248-9e7a…`; prompt delivered 20:09:07; `messages` seq 4 at 20:09:22 `<child_stalled child="kid1" phase="no_first_update" quiet_secs="15">`; exactly one row |
| 1b | tool call open ⇒ no wake; `silent` after it closes | **PASS** | `tool_start` 20:10:00 → no new stall by 20:10:55; `tool_done` 20:11:00 → seq 6 at 20:11:30 `phase="silent" quiet_secs="30"`; one row |
| 2 | `wait_peer(kid1, until=[done])` | **PASS** | tool result seq 10: `{state: 'stalled', holds: False, since_secs: 18}`, `waits` row kind `peer_state`; fake `end` 20:12:09 → child `TurnFinished` 20:12:09.892, `MessageDelivered` "kid1 is done…" 20:12:09.914 (22 ms), `<peer_state state="done">` message seq 14 (queued behind the hand-off turn, started 20:12:11.338); `waits` empty |
| 3a | `start(bash)`, `send("sleep 60")` | **PASS** | sent 20:12:47; no `<job_idle>` during the sleep (the 20 s clock alone would have fired ≈20:13:07); seq 6 at 20:14:07 `<job_idle id="j1" quiet="20" secs="83" nudge="1" why="screen unchanged for 20 s, process tree quiet">` — 20 s after `sleep` returned |
| 3b | no reaction to input | **PASS (recipe amended)** | the brief's `stty -echo; cat` + `send("x")` **reacts**: `cat` writes `x` back, the screen changes, no fire (observed 20:14:56). With `stty -echo; cat > /dev/null` then `send("x")` at 20:15:52 → seq 18 at 20:16:02 `<job_idle … quiet="10" nudge="2" why="no reaction to input">` |
| 4 | alerts, connections A (focus S) / B (none) / C (fresh) | **PASS** | S `01a1224f-945a…` `TurnFinished` 20:16:43.916; B at +0.6 s `alert: null` (inside the 2 s delay); B at +2.2 s `alert {class: finished, why: "turn ended: ok", id: finished:…@1791577003918}`; A `alert: null`; B `set_event_focus([S])` then `[]` → B `alert: null` (seen); C fresh sees the alert |
| 5 | restart re-arm | **PASS** | parent `01a12250-05c3…`, kid2 `01a12250-120e…`, `peer_state` row written 20:17:17; SIGTERM 20:17:31 (drain 10 s); restart 20:17:58: row still present, boot pass `resuming=1 respawned=1`, kid2 respawned (new fake pid, prompted 20:17:58); fake `end` 20:18:25 → `MessageDelivered` "kid2 is done…" 20:18:25.350, message seq 14 `<peer_state state="done">`; `waits` empty. Side observation per §4.3: the respawned child fired a second `no_first_update` at 20:18:13 — the documented "told twice at most" |

Cleanup: both engines SIGTERMed (exit 0), watcher and fakes gone; `/tmp/155` kept.

## 5 What remains (not in this wave)

- Windows companion consuming `alert` and `set_event_focus` (D6 follow-up brief); `Models.cs` ignores the field today.
- Per-device seen: seen is per rpc connection (C-g); a reconnect re-shows unseen finishes once.
- Decision-model reasons ("an error is showing", "it is asking you something") — D8, out of v1.
- `PeerFacts.dead` wiring (W1 deviation) if a dead harness connection should read `not_running` before its actor stops.
- Brief recipe for proof 3b should say `cat > /dev/null` (or any foreground program that neither echoes nor writes).
