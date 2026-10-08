# 139 Z — TESTS (D12), as built

Branch `feat/139-z`, cut from the integrated `e07fca4e`. Revision 1 Q13 as C ruled (deviation 6): the
109 fakes stay `cfg(test)`-private in `agent`, so the rows that need a running turn are the scripted
two-engine smoke; the in-process file holds the rows the store and the log settle on their own.
Evidence: `docs/design/139-smoke.md` (76/76 checks, 186 s, two real engines on this Mac).

## What changed where

- `crates/sessionsync/tests/mesh_durability.rs` (new, 2 tests): `t9_journal_prune_travels_through_the_log_and_never_touches_a_foreign_journal` (A prunes a finished turn with `Scope::Here`, one log row ships the DELETEs, B's copy loses it, B's own tick deletes 0 of A's, `Scope::All` on B is fenced); `t1_t7_replica_displays_the_owners_turn_and_acts_on_nothing` (B holds A's open turn: `session().here == false`, `open_turn` Some, `open_turns/jobs_open/child_waits(Here)` empty, writes fenced, `log_seq == 0`).
- `scripts/durability-mesh-smoke.sh` (new) → `scripts/durability-mesh/smoke.ts` (bun driver, rows T1–T8 + T2c, `Engine` = `t --headless` on a scratch dir with RPC/WS/sqlite readers) + `scripts/durability-mesh/fake-openai.ts` (the 109 `BatchFake` as an OpenAI-compatible server driven by a `SMOKE {calls}` prompt; added to the engines as gateway `fake`).
- `justfile:38-54` `durability-mesh *ARGS`: the integration test, `sessionsync owner::`, the agent/rpc/tools two-engine unit tests of every wave, then `scripts/durability-mesh-smoke.sh` when the first arg is `smoke` (not CI).
- `docs/design/139-smoke.md` (new): recipe, the full-run table, the five findings.
- **`139 Z-fix:` commits** (production code the matrix needed; each row red before, green after):
  1. `750e03f5` `crates/agent/src/durability/jobs.rs` (`job_of_call`, both `in_batch` sites, unit `job_of_call_accepts_the_bridged_inner_id`), `crates/agent/src/loop_/resume_calls.rs:78-79` — a bridged `shell`/`start` job row is `<call_id>.<n>`; the started mutating call was re-run after a transfer and reported twice.
  2. `9a3beb33` `crates/agent/src/supervisor.rs` `deliver_result` — a child's result for a parent owned elsewhere is forwarded (`forward_peer`, 69 P) instead of spawning a local actor whose intake refuses it.
  3. `2a743524` `crates/agent/src/supervisor.rs` (`pending_takeovers`), `supervisor/resume.rs` (`TakeoverPending` message; `resume_turn` consumes it), `crates/sessionsync/src/owner.rs:216-225` (`Take::run` Pending arm tells it); `crates/agent/src/loop_.rs:3266-3269` `dangling_calls` skips `System` rows.
  4. `ef6ccafc` `crates/sessionsync/src/snapshot.rs` (`AppliedSession` carries the local fence before; `local_fences` returns `(epoch, owner)`), `crates/sessionsync/src/sync.rs:564-571` (`receive_snapshot` → `on_applied` → `Demote`).

## Gates (numbers)

- `cargo test -p sessionsync --test mesh_durability`: 2 passed. `cargo test -p sessionsync --lib`: 14 passed.
- `cargo test -p agent --lib -- loop_::durability_tests durability:: store::durability supervisor::waits supervisor::drain supervisor::resume supervisor::acp_resume supervisor::transfer supervisor::child_waits loop_::resume_calls`: 40 passed, 1 ignored (`crash_child`) — `just durability`'s agent half plus the Z-fix neighbourhoods. `cargo test -p tools --lib -- --test-threads=1 jobs:: pty::` (`just durability`'s tools half): 52 passed in 659 s. `cargo test -p rpc --lib -- forward routed_methods answers_follow durability_wire_shapes`: 10 passed.
- `just durability-mesh` (in-process): 2 + 5 + 31 + 10 passed (sessionsync test, owner::, agent, rpc; ~15 s of tests after the build). The recipe's first cut also ran two `tools` tests, which took **600 s**: W3's `kill_session_writes_lost_before_kill_and_spares_others` / `screen_after_transfer…` wait out a real `sleep 600` group under `--test-threads=1` — dropped from the recipe (they stay in `just durability`), flagged below.
- `cargo clippy -p agent -p sessionsync --tests -- -D warnings`: clean. `rustfmt --check` clean on every Z-touched file; `owner.rs` keeps W2's three pre-existing drift spots.
- `scripts/durability-mesh-smoke.sh` (all rows): 76/76 in 185.7 s; T2/T3/T6 alone ≈ 50 s + 34 s partition wait.
- Build: the warm `./target` clone was unusable (`sourcemap`/`tower-http` rlibs missing — W1/W3/W4 saw the same); `t` and every gate built cold into `CARGO_TARGET_DIR=target/z` (11.5 min for `t`).

## Deviations from §11 Z and why

1. **No in-process T1–T8** (Q13 fallback as C ruled). `crates/sessionsync/src/testing.rs` was **not** created: the integration test needs no transport (hand-driven `apply_log_row` pulls), and a `Loop`/`Switch` hoist with a `test-support` feature would have had no consumer. The owner.rs tests keep their copy.
2. **T4 as ruled by Revision 1 Q10**: a `send_input` on B is a client's write → `TurnResumed{takeover}` at once (asserted), not `ResumePending`; the system path is proven with `session_take(.., "orphan")` → `ResumePending` → Attention item → `resume_turn` → `TurnResumed{takeover, from: A}`.
3. **T7's -32010** is asserted on `answer_ask` (D18 forward-not-take) with A down, and the forward with A up; `resume_turn` on B for a *user* session takes it (Q15), so "forwarded to A" holds only for desks — covered by W4's unit test, not re-staged.
4. **T3's orphans**: 68 §4.3 as built counts *unacked* rows; when the reconnect ships A's tail before B's take row lands on A, B drops it `StaleEpoch` and acks, so A counts 0. The check accepts either accounting and records which (full run: both — `[{epoch 1, 181–195}]` and the stale-drop line). The demote itself is asserted from A's log.
5. **T6's banner**: `pty_screen` (RPC) serves `stored_page` with `epoch: null` (the UI's lost marker, no text banner); the moved-from-A wording is asserted on the `wait` path (T3) — through the python bridge `screen()` returns the structured `{lost: True, rows}`; the wording is W3's unit `screen_after_transfer_names_the_previous_owner`.
6. **Bridged calls serialise** (one python cell per session at a time): a batch cannot hold a mutating and a read-only call at once, so T3 uses two sessions and the third call of T2's batch is `list_dir` (`todo` added a `<todo_continue>` nudge turn that broke the transcript-shape comparison).
7. `SESSION_DRAINING` is `-32012` (closer moved it; `-32011` is `NOT_HUB`). Commit prefixes `139 Z:` / `139 Z-fix:`, `BUILD_LABEL=139` (the brief's working-rules block says 138).

## Open items for the closer / owning waves (found, not fixed)

- **Drain ordering (W2/W3)**: in every T2 run B's `settle_transferred` found A's job row still `running` (B's `now` is the `settled_ms` on both copies) — A's `KillSession` rows reach B after `Released`'s `last_seq`, not before; harmless now (I6 holds, no signal) but §6.1's "every row of the drain is at or below `last_seq`" does not hold for the jobs rows. Evidence in `139-smoke.md` §3 and the T2 run of 07:53.
- **Owner-side cards**: A's transcript gains an `error` row "background work outcome unknown: j1: killed when the session moved before it settled" (activity ledger, `supervisor.rs:2512`) on every transfer; wording for a move, not a failure, is W2/W3's call.
- **`pending_takeovers` lifetime**: entries are removed when consumed; a session taken back before the user presses Resume leaves a stale entry (ponytail: clear on `demote` if it matters).
- Unreachability takes 31–35 s (QUIC idle) — the smoke's floor; `mesh` has no knob for it.
- **W3 test time**: one of `tools::jobs::tests::kill_session_writes_lost_before_kill_and_spares_others` / `pty::tests::screen_after_transfer_names_the_previous_owner` runs ~600 s (a spared `sleep 600` group is waited on); `just durability`'s tools half went 60 s → 659 s.
