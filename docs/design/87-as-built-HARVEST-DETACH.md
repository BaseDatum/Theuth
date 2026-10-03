# 87 — as built: HARVEST-DETACH

Wave 87 ("87: HARVEST-DETACH, COMPACT-NONBLOCK"). Branch
`fix/87-harvest-detach` off local main `4e618ae9`. Brief:
`/tmp/mc-2026-10-03/omp-HARVEST-DETACH.md`.

## Why (measured 2026-10-03)

`/compact snap` in the desktop app on a 640k-token session (`01a0fbf9`): the
fold itself took about 60 ms (`agent::loop_: compaction … epoch=1 folded=694
frames=6 tail=73 before=640564 after=99968 style=snap`, 10:27:03.258Z). The
3 min 17 s before it was the pre-compaction knowledge harvest —
`model_calls`: `kind=harvest 10:23:44 → 10:27:01, 643,927 input / 16,963
output tokens` — awaited inline by `Turn::compact` after `Compacting` had
been published, so the UI said "compacting…" for three minutes and the snap
looked ignored. Ruling: the pre-compaction harvest runs detached on a copy
of the pre-fold frame; the compaction completes at once. The post-turn
harvest has been detached since 2026-09-22 (`Turn::spawn_harvest`); this
was the one still inline.

## Commits

1. `29528d73 fix(agent): run the pre-compaction harvest detached and chained`
   (code + `harvest_compact_tests`).
2. `docs(87): …` — `10-knowledge.md` §5.2/§7 and this note.

## What changed

### `crates/agent/src/loop_.rs`

- 4406–4419 `spawn_harvest(reply)`: unchanged semantics (post-turn trigger
  and `/knowledge harvest`): the in-flight guard `Err("harvest in flight")`
  stays here, then `detach_harvest(reply, false)`.
- 4426–4428 `spawn_harvest_chained()`: the pre-compaction entry, no guard,
  `detach_harvest(None, true)`.
- 4434–4496 `detach_harvest(reply, chain)`: the former body of
  `spawn_harvest` — knowledge/empty checks, `resolve`, `image_index`,
  `SessionState::resume(epoch, head, history)` + `baseline`, a `Turn`-less
  `Turn` on `shared.closed.child_token()` with `tools`/`cwd` copied,
  `Harvesting` published, task stored in `state.harvest_task`, `Harvested`
  with zero counts on failure, `reply` sent. New with `chain`: after the
  fallible steps the running `harvest_task` (if any and not finished) is
  taken out of the state and moved into the new task, which awaits it first
  (`JoinError` — abort or panic — logged at warn, ignored); one
  `info!(session, chained, "harvest before compaction: detached")` at spawn
  time; the task's warn prefix is `harvest before compaction:` for the
  chained entry, `harvest:` otherwise.
- 4585–4601 `Turn::compact`: the trigger condition and the
  `turns_since_harvest = 0; calls = 0` bookkeeping are unchanged; the inline
  `self.harvest(current, transport).await` became
  `self.spawn_harvest_chained().await` (an `Err` still only warns). Head
  build, fold, epoch insert, the `compaction` log line and `Compacted`
  follow without awaiting anything of the harvest.
- Deleted: the inline-only `Turn::harvest` (`prepare_images` then
  `harvest_frame`). `prepare_images` stays — the summariser uses it.
- 5615–5617: `mod harvest_compact_tests` registered next to `harvest_tests`.

### `crates/agent/src/loop_/vision_fallback_tests.rs`

- 650–658 `Fake::harvest_and_compact_held(hold)`: holds both the harvest and
  the summariser reply (scenario C).

### `crates/agent/src/loop_/harvest_compact_tests.rs` (new)

One `#[test]` (`compaction_does_not_wait_for_the_harvest`) through
`crate::with_test_registry`, three scenarios against the fake provider:

- A `compaction_returns_before_the_harvest`: two tool-less turns (the first
  carries `MARKER`), `turns_since_harvest = 1`, `compact_now(Some(Cut),
  None)` → `Ok(1)`; `Compacted` drained in < `HOLD` (3 s) with exactly one
  `Harvesting` before it and no `Harvested`; `harvest_task` still running;
  after the hold `Harvested (1, 0)`; the only new request is the harvest, it
  contains `MARKER` and not the cut head's "Recent turns follow verbatim";
  the concept landed in the store.
- B `a_running_harvest_is_chained_not_skipped`: trigger (a) as in
  `harvest_tests` (held), then `compact_now(Some(Cut), None)` on the same
  state → `Ok(1)` at once, one more `Harvesting`, still exactly one harvest
  request at `Compacted`; then `Harvested (0, 1)` twice and two harvest
  requests, the second carrying `MARKER`; the stored task is finished.
- C `a_cancelled_compaction_rolls_back_and_the_harvest_lands`:
  `Fake::harvest_and_compact_held`, `compact_now(Some(Summary), None)` joined
  with a drainer that cancels the turn token at `Harvesting` → `Err` containing
  "cancelled", no `Compacted`, `state.epoch == 0`, `head == None`, store
  frame `(0, None)`; then `Harvested (0, 1)` arrives and the one harvest
  request carries `MARKER`.

### `docs/design/10-knowledge.md`

- §5.2 pseudo-code line for `Turn::compact` → `spawn_harvest_chained()`;
  the "pre-compaction harvest stays synchronous" sentence replaced by the
  2026-10-03 as-built paragraph (detached, chained, cancel semantics).
- §7: the harvest reads the pre-fold frame on a detached copy; summarise and
  cut do not wait.

## Semantics

- Every compaction that harvested before still harvests — manual `/compact`
  of any style, `/clear`, auto and overflow — on the pre-fold snapshot; the
  turn no longer stalls for it. `given.is_some()` (meeting head) and
  `depth > 0` never harvest here, as before.
- Chain, never skip, before a compaction: the fold is the last moment the
  folded span exists whole. The post-turn trigger and `/knowledge harvest`
  keep the skip rule (`Err("harvest in flight")`) — their span is still in
  the frame.
- Images: `image_index` (read as stored, nothing consulted), never
  `prepare_images` — the detached copy must not write a vision note into the
  session.
- Failure: a failing detached harvest warns (`harvest before compaction: …`)
  and publishes `Harvested { added: 0, updated: 0 }`; the compaction never
  fails because of it.
- Cancel: a cancelled compaction rolls back as before (the summariser call
  is cancelled, nothing persisted). The harvest it spawned holds only
  `shared.closed`'s child token and keeps running — knowledge writes are not
  session mutations. `Session::on_stop` still cancels `closed` and
  aborts/joins `harvest_task`; when the stored task is a chained one, the
  previous task's `JoinHandle` is dropped with it, but that task's call is
  cancelled by `closed` too, so it ends on its own.
- Events unchanged in shape: `Harvesting` before `Compacted`, `Harvested`
  when the task ends. `CallKind::Harvest` unchanged. No RPC/event/binding
  changes.

## Verification

Inside the worktree, `PATH=$HOME/.rustup/toolchains/1.98.1-aarch64-apple-darwin/bin:$PATH`:

1. `cargo test -p agent --no-fail-fast 2>&1 | tail -40` →
   `test result: ok. 367 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 26.61s`
   and doc-tests `test result: ok. 0 passed; 0 failed`. Includes
   `loop_::harvest_compact_tests::compaction_does_not_wait_for_the_harvest … ok`,
   `loop_::harvest_tests::harvest_runs_detached_from_the_turn … ok`,
   `loop_::compact_queue_tests`, `loop_::compact_floor_tests`,
   `loop_::vision_fallback_tests`, `loop_::meeting_diet_tests`.
2. `cargo clippy -p agent --all-targets --no-deps -- -D warnings` →
   `Finished \`dev\` profile [unoptimized + debuginfo] target(s) in 2m 03s`, no warnings.
3. `cargo check -p rpc -p tui` →
   `Finished \`dev\` profile [unoptimized + debuginfo] target(s) in 12m 45s`.
4. `git status --short` → clean after the docs commit;
   `git log --oneline 4e618ae9..HEAD` → the two commits above.

## Deviations

- Scenario C is driven directly on a `Turn` (`compact_now` + the turn's
  cancel token) rather than through the `Session` actor's `CancelTurn`: the
  actor's seeded turns are tool-less, so `turns_since_harvest` stays 0 and
  the compaction would not harvest; a `Turn` lets the test set
  `turns_since_harvest = 1`. Rollback is asserted the same way as
  `compact_queue_tests` (`Err` with "cancelled", no `Compacted`, store frame
  `(0, None)`). The scenario is deterministic — no fallback to a token-only
  unit assertion was needed.
- `Fake::harvest_and_compact_held` added to `vision_fallback_tests.rs` (the
  brief allows a new fake variant): scenario C needs the summariser held so
  the cancel lands inside the model call.
- The chained task's `JoinHandle` is taken only after the fallible steps of
  `detach_harvest` (knowledge check, `resolve`, `image_index`), so an early
  `Err` leaves the running harvest in `state.harvest_task` for `on_stop` to
  join.
- `Harvested` counts in the tests are exact and order-dependent ((1, 0)
  first, (0, 1) afterwards) because the knowledge plugin and its store are
  process-global per test body and the three scenarios share one.

## Follow-ups

- None required. If a session is stopped while a chained harvest is still
  waiting on its predecessor, the predecessor is cancelled via `closed` but
  not joined by `on_stop` (its handle lives inside the aborted task); the
  51 §6.1 guarantee holds through the token, not the join.
