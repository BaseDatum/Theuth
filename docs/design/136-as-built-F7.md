# 136 F7 — as-built: second fold, `origin/main` 44c0d63d (139 MESH-DURABILITY) into `feat/136-dreaming`

Tree `~/Develop/theuth-136`, branch `feat/136-dreaming`. Base 89305a7b (fully gated; already holds main
68b9fc26 = 137 ENDLESS, folded by F5). Folded `origin/main` = 44c0d63d: design 139 MESH-DURABILITY
(60 files: agent loop_/supervisor/store/durability + `supervisor/{child_waits,transfer}.rs`, migration
0038, rpc api/forward, schedule, sessionsync, roster attention, tasks ledger, tools jobs/pty/journal,
tui, desktop ResumeBanner/Composer/owner/reduce/sessions, docs, `scripts/durability-mesh*`).

## Commits

| sha | what |
|---|---|
| 26c7429a | `Merge origin/main (139 MESH-DURABILITY) into feat/136-dreaming` — 1 file conflicted (1 block), resolved both-sides |
| 8af422bc | `136 F7: held_message_follows_the_brief asserts the released ChildTurn permit, not the pool count` (see below) |

## Conflict resolutions

| file | region | resolution |
|---|---|---|
| `crates/rpc/src/api.rs` | `mod tests`, after `dreaming_methods_are_registered` (136) | kept 136's `dreaming_methods_are_registered` **and** 139's `routed_methods_dispatch_on_the_owner_with_the_forwarders_params` + `answers_follow_the_questions_row_to_its_owner_or_fail_unreachable`, in that order (additive) |

Everything else auto-merged; checked by hand afterwards:

- `crates/agent/src/loop_.rs`: 136's `Reserved::open(.., budget)`/`Refused`, `Abort::Budget` arms, `Shared.budget`/`budget_note`, `harvest_frame`/`harvest_store()`, the `<knowledge>` hint skip and `mod budget_tests` all present (94 budget/harvest/consolidating references); 139's `Resume{cause, from, lost_jobs}`, `Turn.resume_from/resume_lost_jobs`, `suspending()`, `open_turn(session)`, `restart_note(.., None, ..)`, `ChatRole::User | ChatRole::System` skip landed beside them.
- `crates/agent/src/supervisor.rs`: 136's `SetBudget`/`LedgerSince`/`record_shown`/consolidating-session registration (8 references) + 137's endless handlers + 139's `draining_sessions`/`pending_takeovers`, `child_waits::*` calls in `SpawnChild`/`hand_off`, `remote_owner` guards in `inject`/result delivery, `OpenTurns(Scope)`; `spawn()` keeps `store.budget(id)` → `.with_budget(budget).with_endless(..).with_parent_view(..)`.
- `crates/agent/src/store.rs`: 136's `sessions.budget`/`may_consolidate` + 139's `pub use durability::Scope` and the D17 backfill log; `create_session` signature unchanged (139's new api.rs test calls it with five args — compiles).
- `crates/agent/src/store/capture.rs`: `ATTACHED` is 139's 20-entry list (adds `pty_screens`); 136 adds no attached tables.
- Migrations: `0037_endless.sql` (137), `0038_mesh_durability.sql` (139), `0039_budget.sql` (136) — all three present.
- `crates/schedule/src/lib.rs` / `crates/roster/src/attention.rs`: 139's `OpenTurns(agent::store::Scope::Here)`; 136's `budget:`/`cap:` keys, `FireWith`, `SetCap` intact.
- `crates/desktop/ui`: 139's banner/owner/reduce/sessions changes beside 136's; `SettingsDialog` still lists both `dreaming` and `memory` tabs.
- `Cargo.lock`: auto-merged, cargo made no further change.

Bindings: `just ts-desktop` regenerated with no diff (139 already shipped `FrontDoorError.ts`; 136's types unchanged).

## The one fix (8af422bc)

`supervisor::tests::held_message_follows_the_brief` (T2, pre-dates both designs) failed deterministically in the
full `agent` suite and single-threaded (`loop_::harvest_tests`/`endless`/`acp_child` + it), passed alone.
Traced with temporary prints: after the test's `finished(y)` the supervisor did drop y's `ChildTurn` permit
(`available_permits() == 1` right after the remove), then `loop_::reacquire_permit` took it — y's real actor
had already opened the held `"hello y"` as a peer-bounded turn whose 50 §8 gate waits on the same tree pool,
because with a process-global tool registry left behind by an earlier test y's brief turn finishes before
the held message is replayed. Correct runtime behaviour; the test's last assertion raced it. The assertion
now reads the invariant it means — `child_turns[y]` no longer holds a permit — instead of the shared
semaphore count. No production code changed. The brief's other anticipated breakages (137 test assuming no
budget field, 136 test assuming the old frame, duplicated match arm, missing `..Default::default()`) did
not occur.

## Gate (all through `build-lock`, label `136-F7`, `CARGO_TARGET_DIR=…/theuth-136-target/C`, jobs 4, incremental off; at 8af422bc unless noted)

| step | result |
|---|---|
| `BUILD_SLOTS=1 just ts-desktop` | 8 export tests ok; bindings unchanged (at 26c7429a) |
| `bun run check` (tsc -b) | 0 errors |
| `bunx vitest run --maxWorkers=2` | 1381 passed |
| `bun run build` | built (chunk-size warnings only) |
| `cargo test -p knowledge -p dreaming -p schedule -p tools -p rpc -p roster -p meeting -p sessionsync -p tasks --lib --no-fail-fast` | dreaming 61 · knowledge 55 · meeting 69 · roster 68 · rpc 54 · schedule 75 · sessionsync 14 · tasks 100 · tools 178 (+1 ignored); 0 failed, no re-runs needed |
| `cargo test -p agent --lib --no-fail-fast` | at 26c7429a: 518 passed · 1 failed (`held_message_follows_the_brief`, twice); at 8af422bc: 519 passed · 2 ignored · no SIGSEGV |
| `cargo clippy -p knowledge -p agent -p schedule -p tools -p dreaming -p rpc -p roster -p meeting -p theuth --tests -- -D warnings` | clean |
| `cargo fmt -p knowledge -p agent -p schedule -p tools -p dreaming -p rpc -p roster -p meeting -- --check` | clean |
| `BUILD_SLOTS=1 cargo check --workspace --tests` | clean (pre-existing `block v0.1.6` future-incompat note only) |
| `BUILD_SLOTS=1 cargo build -p theuth` | `…/C/debug/t` built |

## Left

Nothing. Tree is at the fold + one test fix, gate green; ready for the lander.
