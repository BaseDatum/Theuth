# 139 as-built — closer (integration of C + W1 … W4, Z's tree, the merged-tree gates)

Branch `feat/139-mesh-durability`, worktree `theuth-139`, from C's contract `1afe827c` + W4, W1,
W3, W2 merged in that order (`4832674c`, zero textual conflicts; the merged tree compiled at the
first crate-level check), then this note's commits, then `origin/main` folded (`4606b76e`, 138
UI-FIXES; one conflict in `Composer.tsx`, both post-send resets kept). Design:
`docs/design/139-mesh-durability.md` (v0 + Revision 1). Per-wave notes: `139-as-built-{C,W1,W2,W3,W4}.md`;
Z's evidence: `139-smoke.md`. This note lists what the closer changed across seams, the decisions the
waves left open, the deviations from the design, Q12's upgrade rule, the merged-tree gate numbers,
and what stays open. Line numbers are of the tree at the last commit below.

## 1. What 139 changed, where (whole wave)

| area | before | after | files |
|---|---|---|---|
| Journal readers (D1, I1, I9) | `open_turns()`, `jobs_open()` global; every boot consumer acted on the owner's turns | `Scope { Here, All }` required (no `Default`); `open_turns(Scope)`, `open_turn(session)`, `jobs_open(Scope)` (+ `engine IN ('', me)` for `Here`), `child_waits(Scope)`, `journal_prune(Scope, keep_days)`, `jobs_settle_transferred`; `resume_scan`, `rebuild_waits`, `Drain`, `OpenTurns`, `reap_jobs`, `respawn_acp_children`, `deliver_spared_results`, attention `resume:` items, the schedule plugin all pass `Here`; `resume_turn` has an `ensure!(row.here)` second line of defence | `store/durability.rs` (C), `supervisor/{resume,drain,waits,acp_resume}.rs` (W1), `roster/src/attention.rs`, `schedule/src/lib.rs` (W1), `durability/jobs.rs` (W3) |
| Replicas display (D2) | `Loaded.resume` said "resuming…" for a foreign turn | `ResumeState { owner, here, cause }`; banner *Running on A* / *Paused on A* / *Taken from A — resumed here*; TUI divider names the owner | `lib.rs` (C), `supervisor/drain.rs` (W1), desktop `ResumeBanner.tsx`, `reduce.ts`, `sessions.ts`, `OwnerOfflineCard.tsx`, `tui/src/model.rs` (W4) |
| Release is a drain (D3, D16, D19, I3) | `release` = `unload` (`tell_cancel` + `kill()`, cancel often dropped; `turn_finished{cancelled}` above `last_seq`) | `DrainSession { session, reason, deadline }` (`suspended{release}` first, `Draining`, `wait_idle`, `Jobs::KillSession`, `tell_cancel` + `stop_gracefully` + `wait_for_shutdown`), `release` drains for `release_wait_secs − 1`, then `flush`; the taker waits `release_wait_secs + drain_secs` (Q4); `UnloadSession` stops gracefully; a cancel while `draining` is a suspension in `round` **and** `run_batch`; `draining_sessions` refuses inputs (`SessionDraining:<reason>`) until demote or `2 × (window + 1 s)` | `supervisor/transfer.rs`, `supervisor.rs`, `loop_.rs`, `sessionsync/src/owner.rs` (W2) |
| Resume on take (D4, D5, I4, I5) | `Take::run` ended at the publish | step 9: `open_turn` → `resume_policy(transferred, reason, by_user, mode)` → `ResumeTurn { cause: Transfer \| Takeover, from }` or `ResumePending` (Attention item + local note) or nothing (`off`) | `sessionsync/src/owner.rs` (W2), `roster/src/attention.rs` (W1) |
| Cause (D4, §6.3) | `ResumeKind` only | `ResumeCause { Crash, Shutdown, Update, Transfer, Takeover }` on `ResumePlan`, `ResumeTurn`, `Resume`, `TurnResumed{cause, from}`, `resume_reports.cause`, `ResumeState.cause`; `restart_note`/`interrupted_note` spell it (`drain`/`parent_gone` → `shutdown`, Q2) | `durability/mod.rs`, `lib.rs` (C), `loop_.rs` (W2), `loop_/resume_calls.rs` (W3) |
| Jobs and PTYs (D6, D14, I6) | reaper `killpg`'d replicated pgids; `pty_screens` engine-local and unfenced | `jobs.engine` column (0038), reaper filters `engine IN ('', me)`; `KillSession` (per-session `KillAll`, rows + screens before the signal); `settle_transferred` on the new owner (no signal, `LostReason::OwnerChanged`, tells `Jobs`, returns the `Reaped`); `pty_screens` attached; `lost_page`/`SendInput` wording "moved from A" | `migrations/0038_mesh_durability.sql`, `store/capture.rs`, `durability/journal.rs` (C), `tools/src/{jobs,pty}.rs`, `durability/jobs.rs` (W3) |
| Questions and waits (D7, I7) | boot-only global `rebuild_waits`; answers unrouted | `rebuild_waits_for(session)` at resume time on the new owner; `answer_ask`/`resolve_approval` routed to the question row's owner (forward, never take) | `supervisor/waits.rs` (W1), `rpc/src/api.rs` (W4) |
| ACP children (D13) | `waits(child_result)` had no writer; respawn gated on the harness entry only | `child_waits::{child_spawned, child_result_delivered, settle_on_transfer}`; `respawn_acp_children` iterates `child_waits(Here)` with `row.here` + harness + readable `cwd` gates; `cut_journal_in` skips `child_result` (Q8) | `supervisor/child_waits.rs` (W3), `supervisor/acp_resume.rs` (W1), `store/durability.rs` (C) |
| RPC (D11, D18) | `resume_turn` local on a non-owner | `resume_turn` through `forward::route` (`Take` → `ResumeTurn{Takeover}`, Q15); `SESSION_DRAINING = -32012` + `FrontDoorError::SessionDraining`; composer retries once | `rpc/src/{forward,api}.rs` (C, W4, closer), desktop `lib/owner.ts`, `Composer.tsx` (W4) |
| Retention (D15) | `journal_prune` had no caller | hourly tick on the owner with `Scope::Here`, `durability.journal_keep_days` | `supervisor.rs` (W2) |
| Owner backfill (D17) | silent | `info!("68: claimed {n} unowned session(s) as {me}")`; copied-file guard is a follow-on (Q6) | `store.rs` (W1) |
| Inject guard (Q3) | `inject`/`rearm_reminders` would spawn an actor for a foreign session | `inject` checks `remote_owner(to)`: logs and **drops** (see §3); `rearm_reminders` runs under the same gate | `supervisor.rs` (W1) |

## 2. Cross-seam hand-offs closed by the closer (commit 91ba125f)

| hand-off | from → to | what the closer did |
|---|---|---|
| `on_resume_here` body | W2 dispatcher → W3 bodies | `transfer::on_resume_here` now calls `child_waits::settle_on_transfer(agents, session, from)` then `durability::jobs::settle_transferred(&store, session, from)`; W2's interim `settle_transferred_jobs` deleted. Returns the lost `JobId`s |
| `lost_jobs` in the resume report | W3 ("`Reaped.jobs.len()` is the count") → loop | `Resume.lost_jobs: Vec<JobId>` → `Turn.resume_lost_jobs` → `report()` writes them into `resume_reports.lost_jobs` and `ResumeReport.lost_jobs` (109 R2 §1.5's zeros become real for transfers; single-engine resumes keep `[]` — the reaper reports those by injection) |
| ordering `rebuild_waits_for` → `on_resume_here` | W1 ↔ W2 | kept as W1 wrote it (`resume.rs:138-143`): `rebuild_waits_for` touches `questions`/`waits(peer_reply)`, `on_resume_here` touches `waits(child_result)`/`jobs` — disjoint rows, order immaterial |
| `-32011` collision | C (`SESSION_DRAINING = -32011`) vs 69 Y (`NOT_HUB = -32011`) | `SESSION_DRAINING = -32012` (`forward.rs:49`); api.rs tests key on the constant; desktop `sessionDraining()` keys on the payload (unchanged), `owner.test.ts` uses `-32012` for the draining rows and keeps the `-32011` NotHub row |
| D16 refusal string | W2 (`session draining: <session> is moving…`) vs W4 (`strip_prefix("SessionDraining:")`) | aligned on the wire form: `transfer::DRAINING_PREFIX = "SessionDraining:"`, `draining_error(reason) = "SessionDraining:<reason>"` (no `:<next_owner>` — `Msg::Release` carries none); `api::session_draining` owns the user-facing wording |
| Q10 client-peer flag | Revision 1 → W2 policy / W4 could not | `TakeSession.by_user: bool` (`sessionsync/src/lib.rs`), carried on `owner::Take`, read only by `resume_policy`: `!transferred ∧ (reason == User ∨ (reason == Write ∧ by_user))` → `Resume(Takeover)`. `forward::route` sets `by_user: true` for its `Write` take; `session_take` RPC and `MeshTaker` (`Orphan`) set `false`. 68's `unseen_long_enough` still keys on `reason` (W4's concern) |
| `forward::unreachable` duplicated | W4 (`api::owner_unreachable`) | `forward::unreachable` is `pub(crate)`; `route_question_via` uses it with `"not connected"`; the twin is deleted |
| dead `ReleaseSession` | W2 deviation 3 (would self-ask `ReleaseWaitSecs`) | deleted (no callers; `answer_release` → `owner::release` is the one path) |

## 3. Decisions on the waves' open items

| item | decision | why |
|---|---|---|
| Q3 `inject` drops instead of forwarding (W1 dev. 1) | **kept as drop + `warn!`**; forwarding is a 139 follow-on | the wire form (`PeerEnvelope.from: SessionId`, `Policy::route` refuses self-routes) has no sender for a harness-made `<job_result>`/`<reminder>`; after D1 every `inject` source is owner-scoped (reaper engine filter, spared results forwarded by `deliver_spared`, reminders gated), so the guard is a backstop that should never fire in production. A harness envelope on `theuth/rpc/1` is a 69 change |
| ACP rows without `cwd` pass the cwd gate (W1 dev. 2) | **accepted** | the lazy `acp_session` path uses the harness default for such rows; refusing them would refuse every pre-cwd child |
| Q7 note cannot name `from` (W3 dev. 3) | **left**: *"the session moved to this engine at …"* | `from` is on `Resume`/`TurnResumed`, not on `ResumePlan` (C froze it); adding it to the plan for one note is not worth a contract change — the `<engine_restart from="A"/>` note in the same batch names the engine |
| `DrainSession` replies via `DelegatedReply` (W2 dev. 1) | **accepted** | the body waits up to `release_wait_secs − 1` and the turn's end asks `Agents` — on the mailbox it would deadlock, as `Drain` does |
| `settle_transferred` both tells `Jobs` and returns (W3 dev. 1) | **accepted** | the dispatcher needs the ids for the report, the actor needs the rows for `jobs()` |
| `KillSession` keeps the rows in the old owner's actor as `lost` (W3 dev. 6) | accepted | removing would restart `next` numbering and collide with replicated ids |
| `jobs_settle_transferred` stamps B's `now` as `settled_ms` (W3 open) | accepted, noted | the owner-changed body shows the resume time, not A's kill time |
| W4 dev. 4/5 (no §9.3 row for `here ∧ transfer ∧ !resumed`; `ResumeState` has no `from`) | accepted | wording added by W4; `from` after a reload reads *another engine* — follow-on if it matters |
| Z in-process harness (C dev. 6, Q13) | as C decided: unit tests + scripted two-engine smoke | exporting `agents_on`/`BatchFake` under a feature is a refactor |

## 4. Deviations from the design (whole wave, incl. the closer's)

| what | design | built | why |
|---|---|---|---|
| `SESSION_DRAINING` | `-32011` (§6.1, §9.1) | `-32012` | `-32011` is 69 Y's `NOT_HUB` |
| D16 refusal text | *"<session> is moving to <taker> — retry in a moment"* from the supervisor | supervisor emits `SessionDraining:<reason>`; the RPC layer renders *"<session> is moving to another engine — retry in a moment"* / *"… is pausing for <reason> — resumes on the next start"* | one wording owner; `Msg::Release` carries no taker, so `next_owner` is always `None` today |
| Q10 | "a `Write` take requested by an RPC from a real client peer (not `RpcPeer::internal()`)" | every `Write` take `forward::route` makes counts as the user's (`by_user: true`) | the one `RpcPeer::internal()` that reaches `route`'s `Take` is `client::Local` — the owner console (TUI) and the ACP relay, both a human at a keyboard; forwarded calls return before `Take`; desk wakes take `Orphan` directly. No internal/system caller reaches `Route::Take` |
| `ResumeTurn.cause` | `ResumeCause` | `Option<ResumeCause>` (C dev. 2); `ResumePlan.cause` carries the boot rule | the rule needs the `suspended` row's reason, which only `resume_plan` reads |
| `ResumeReportRow.cause` | — | non-optional; `''` reads as `crash`/`shutdown` by `kind` (C dev. 3) | pre-139 rows |
| `0033` "Engine-local" comment | deleted | left; superseded in 0038's comment (C dev. 1) | `sqlx::migrate!` checksums applied files |
| `DrainSession` reply | `SessionDrainReport` | `DelegatedReply<SessionDrainReport>` (W2 dev. 1) | mailbox deadlock otherwise |
| draining-cancel = suspension | one condition at `loop_.rs:4530` | two sites: `round` and `run_batch` (W2 dev. 5) | the batch's plain-cancel path wrote "cancelled by user" and closed `started` rows |
| `draining_sessions` cleared | at step 5 | at demote or `2 × (window + 1 s)` for `release` (W2 dev. 6, §6.2 "fence interplay") | closes the window in the normal case, bounds it when the taker never writes |
| `release` learns `release_wait_secs` | from `config` | `ReleaseWaitSecs` ask on `SessionSync` (W2 dev. 3) | `release(store, id)` keeps its signature for `lib.rs` |
| `interrupted_note` spelling | unchanged | also spells `transfer`/`takeover` (W3 dev. 2) | "the engine restarted" would be false after a transfer |
| `child_spawned` with an empty `call_id` | writes a wait | writes nothing (W3 dev. 4) | `waits` is keyed `(session, call_id)` |
| `rebuild_waits_for` vs `on_resume_here` order | unspecified | approvals/peer waits first, then jobs/child waits | disjoint rows |
| Attention `TurnFinished` clears any turn's report (W1 dev. 3) | per turn | per session (map keyed by session) | as the map was |
| `deliver_spared` gained `mirror(child)` (W1 dev. 4) | — | before `route` | `UnknownSession` on a freshly opened store otherwise |
| `lost_jobs` in the report | "populated by the §7.1 path" | populated for `transfer`/`takeover` only | single-engine lost jobs are reported by the reaper's injection, not the resume report (109 as built) |

## 5. Q4 / Q12 — operational rules for a mesh

- **One `sessions.*` config per mesh** (Q4): the owner drains for `release_wait_secs − 1`, the taker
  waits `release_wait_secs + durability.drain_secs` for `Released` and `release_wait_secs` for the
  cursor; the wire `Msg::Release` carries no window, so two engines with different
  `sessions.release_wait_secs` break the arithmetic (the taker gives up before the owner replies or
  the owner cancels a turn the taker would have waited for).
- **Upgrade order** (Q12): **every engine of a mesh must run a post-0038 binary before the first
  post-0038 write ships.** A `jobs` / `resume_reports` changeset with the new column fails
  `sqlite3changeset_apply` on an unmigrated replica, `apply.rs` returns an error and the row is
  retried forever (same constraint 0033/0035 had; 68 has no schema version in `Hello` — a 68
  follow-on). Rebuilt engine and desktop are owed on every machine of the mesh (every running
  engine predates 0038). C applied 0038 to a `.backup` of the live store: 2376 `jobs` rows backfilled
  to their session's owner, 10 `resume_reports` rows `cause = ''`. No 0037 exists on main at this
  fold (137 has not landed), so the ordering check is moot at this sha.

## 6. Gates (merged tree)

All with `CARGO_TARGET_DIR=$PWD/target`, `CARGO_BUILD_JOBS=4`, `CARGO_INCREMENTAL=0`, cargo 1.98.1,
through `build-lock` (`BUILD_LABEL=139`).

### 6.1 Integration pass (at `e07fca4e`, before Z)

| gate | result |
|---|---|
| `cargo check -p agent -p tools -p rpc -p sessionsync -p roster -p schedule -p tui --tests` (merged tree, before any closer edit) | clean |
| `cargo test -p agent -p tools -p rpc -p sessionsync -p roster -p schedule -p tui -p tasks --no-fail-fast` | agent **472 passed, 2 failed, 2 ignored**; tools 177 passed, 1 ignored; rpc 53 (+ `tests/prompts` **1 failed**); sessionsync 14; roster 67; schedule 72; tasks 100; tui 146 + `tests/no_emoji` 1 passed |
| `cargo clippy -p agent -p rpc -p sessionsync -p tasks --tests -- -D warnings` (closer-touched crates) | clean |
| `bun run check` + `bunx vitest run src/lib/owner.test.ts src/components/chat` in `crates/desktop/ui` | `tsc` clean; 48 files, 503 tests passed |
| `rustfmt --edition 2024 --check` on the closer's `.rs` files | clean except pre-existing drift outside 139 lines: `api.rs:1489/5390`, `ledger.rs` (20 spots) — main is not fmt-clean there (as C and W4 recorded) |

The three reds are pre-existing on main, none in a file 139 touched for the failing path:

| red | proof it is main's |
|---|---|
| agent `loop_::tests::prompt_parts_in_order_with_blank_lines` — `assert!(!full.contains("rules"))` | main's `2a7253e4` ("prompt: share the machine") put *"Write the rules you are under verbatim"* at `frame.rs:78`; `git diff origin/main HEAD -- crates/agent/src/frame.rs` is empty; the test body is unchanged. Fails deterministically (3/3 isolated) |
| agent `supervisor::tests::held_message_follows_the_brief` — "freed when y's turn ends" | load flake: passes 3/3 in isolation; 139's `supervisor.rs` diff touches neither `child_turns`/`SpawnPermit` nor the test (the machine ran two other designs' builds during the run) |
| `rpc/tests/prompts::prompts_resolve_everywhere_and_replay_only_while_pending` — second `answer_ask` returns an error, not `false` | 132's `agent::answer_ask` (`717708fa`, `lib.rs:362-367`) sends `LateAnswer` to an `Agents` actor the test never spawns; 139's only delta in `answer_ask` is W4's `route_question` prelude, which returns `Ok(None)` without a sessionsync actor; `crates/rpc/tests/prompts.rs` last changed by 129 W1 (`2ca73a86`) |

### 6.2 Merged-tree gates after Z

*(filled by the closer after `DONE Z`: crate tests, `just durability`, `just durability-mesh`, clippy on the crate list, `cargo check --workspace --tests`, `just ts-desktop` + `git status`, `bun run check` + `bun run test`, rustfmt on every touched `.rs`.)*

## 7. Open items (139 follow-ons)

- Q3 forwarding form for a harness result addressed to a session owned elsewhere (drop + `warn!`
  today; needs a harness envelope on `theuth/rpc/1`, a 69 change).
- Q6 copied-file guard on the owner backfill (log only today).
- Q11 a transferred parent's children render as remote rows on the new owner (the "N subagents
  running" glyph is off there).
- W4: banner default `here: true` when a relayed owner event arrives before any banner exists;
  `ResumeState.from` for the owner-side banner after a reload.
- W3: `children_*` counts in the resume report are still zero (109 W4 gap, not ruled here).
- Q7 full fidelity (`peer_exchanges` receipts replicating) — a 68 decision reversal, not taken.
- 68 `Hello` schema version (Q12) so a mixed-version mesh refuses to ship instead of retrying forever.

Files changed by the closer: `crates/agent/src/supervisor/{transfer,resume}.rs`,
`crates/agent/src/{supervisor,loop_}.rs`, `crates/rpc/src/{forward,api}.rs`,
`crates/sessionsync/src/{lib,owner}.rs`, `crates/tasks/src/ledger.rs`,
`crates/desktop/ui/src/lib/{owner.ts,owner.test.ts}`, `crates/desktop/ui/src/components/chat/Composer.tsx`
(merge), `docs/design/{68-sessions,68-contract,109-durability}.md`, `docs/design/139-as-built.md` (this note).
