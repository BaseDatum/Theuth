# 70 — as built: COMPACT-QUEUE (input during compaction queues, never cancels)

Branch `fix/compaction-queues-user-input`, 2026-09-26. Brief: a message (later
widened to *any* input — `<job_result>`, reminders, peer messages, child
results) arriving while a compaction runs must queue in arrival order and be
delivered right after the summary commits; never cancel or lose either. An
explicit interrupt may cancel the compaction, which must roll back cleanly; a
failed compaction must roll back and keep the queue.

## What the code does (traced, then pinned by tests)

Every input source funnels into one admission point. `Agents::route`
(`supervisor.rs:819`) is what `Agents::input` (humans: `SendInput`/`Steer`/
`UseSkill`), `Agents::inject` (harness: `<job_result>`, `<job_idle>`,
reminders, child results) and A2A/peer delivery (`PeerInput`) end in; it
`tell`s the session actor `UserInput` / `PeerInput`. Both handlers
(`loop_.rs:1025`, `1071`) do the same thing: `inflight.is_none()` → start a
turn, else push to `shared.steer` (publishing `InputQueued` when the input
carries an id). `inflight` is set by `Session::enter` for a model turn *and*
for the `/compact` mini-turn (`CompactNow`, `loop_.rs:1637`); an in-turn
auto-compaction (`Turn::round` → `Turn::compact`, `loop_.rs:3401`) runs
under the turn's own token. Nothing in the input path touches that token:
the only cancellers are `CancelTurn` (`Agents::Cancel` ← RPC `cancel` ←
⌘↩ interject / Stop / Escape / `/cancel` / Slack stop / Signal `/cancel` /
schedule timeout), `CancelPeerScope`, `SuppressPeerInputs`, the peer wall
deadline and shutdown.

Delivery: a mini-turn hands back through `TurnDone` (`loop_.rs:1695`) →
`next_batch` → `start_turn` with the queued inputs, oldest first, in one
turn (`TurnStarted.text` is their concatenation; `InputInjected` per id).
Inside a running turn the queue drains at the next tool round
(`drain_steer`, `loop_.rs:3100`) or at `TurnDone`.

Rollback: `Turn::compact` (`loop_.rs:4099`) mutates nothing until
`store.compact` (`4350`) succeeds; a summariser `Err` — including
`"cancelled"` from the token — returns at `4214`/`4226` before any state
change. So a failed or interrupted compaction leaves epoch, head, history
and the steer queue exactly as they were; `TurnDone` then starts the queued
turn. (A store insert failing *after* the summary leaves only the
pre-written `spill::epoch` file behind.)

**Conclusion on the reported cancel:** with the code at this HEAD no plain
input — human or `<job_result>` — cancels a compaction on the engine side,
and no frontend sends a `cancel` as part of a plain send. The only
send-shaped paths that cancel are the explicit interject chords (desktop
⌘/Ctrl+Enter `Composer.tsx:499`, TUI Ctrl/Cmd+Enter `lib.rs:340`). I could
not reproduce the observed cancel from the code; the actor-level tests
below exercise exactly the reported sequence (`/compact` in flight, a
`<job_result>` then a human message arrive) and pass unchanged against the
engine. What *was* wrong and is fixed: during a `/compact` mini-turn the
desktop and TUI believed the session idle (`running`/`streaming` come from
`TurnStarted`/`TurnFinished`, which a mini-turn never emits), so a plain
send went out as `send_input` — queued by the engine but invisible (no id →
no `InputQueued` → no queued-steer chip; the transcript showed it as
already sent), and Stop/Escape were unavailable to interrupt the compaction.

## Commits

1. `a33eb30` **test(agent)** — `crates/agent/src/loop_/compact_queue_tests.rs`
   (new, registered in `loop_.rs`), four scenarios through the real
   `Session` actor against the wiremock fake provider:
   - `manual_compaction_commits_then_the_queue_opens_a_turn` — `/compact`
     with the summariser held; a `<job_result>` and an identified human
     message arrive mid-compaction → `CompactNow` returns `Ok(1)`, one
     `Compacted`, no `Error`, one `InputQueued` and one `InputInjected` for
     the human id, exactly one `TurnStarted` whose text is `job\nhuman`,
     the job row in epoch 1's tail, queue empty.
   - `auto_compaction_mid_turn_commits_and_the_queue_follows` — a
     one-token `SetCompactionOverride` makes the third turn compact at its
     first call; inputs land mid-summariser → the turn finishes `stop`,
     one `Compacted`, then the two inputs open one turn in order.
   - `failed_compaction_rolls_back_and_keeps_the_queue` — the fake answers
     the summariser 400 → `Err("summariser: …")`, no `Compacted`, epoch 0,
     no head, pre-compaction seqs intact, the queued inputs open their turn.
   - `interrupted_compaction_rolls_back_and_keeps_the_queue` — `CancelTurn`
     during the held summariser → `Err(…cancelled…)`, no `Compacted`, epoch
     0, queued turn runs to `stop` (the cancel hit the compaction only).
   Harness additions in `vision_fallback_tests.rs`: `Fake::compact_held`,
   `Fake::compact_refused` (`compact_delay`/`compact_status` knobs),
   `Harness::actor()` (spawns the `Session` actor over the stored frame),
   `store`/`id` made `pub(super)`.
2. `acf4012` **fix(tui,desktop)** —
   - `crates/tui/src/model.rs`: `submit_with` takes the steer arm when
     `self.streaming || self.compacting` (queued card + `Command::Steer`);
     `cancel()` (Esc) also fires while `compacting`.
   - `crates/desktop/ui/src/components/chat/Composer.tsx` (one hunk, lines
     141–145): `running` is now `turnRunning || activity === 'compacting'`.
     Effect: plain send during a `/compact` → `api.steer` with an id
     (queued chip via `InputQueued`), Stop button / Escape → `api.cancel`
     (explicit interrupt), ⌘↩ → cancel then steer, as during a turn. No
     other change to Composer (another omp edits it).
3. `7baf72d` **docs(07)** — §8.6 gains the rule and the rollback contract.
4. this file.

## Deviations / decisions

- Engine code unchanged: the single admission point already queues every
  input kind while `inflight` is set, and the compaction's cancellation
  token is reachable only through explicit cancel commands. Adding a second
  "compacting" gate would duplicate `inflight`.
- Ran `cargo test -p agent --lib compact_queue` (the 69 brief lists it) to
  reproduce before editing and to prove the pinned behaviour; did not run
  the full suite, `cargo build`, clippy or fmt.
- Slack/Signal keep their own `running` flags (from `TurnStarted`/
  `TurnFinished`); a plain message during a `/compact` there is still
  delivered (queued by the engine) but is not echoed as "queued". Not
  changed: those crates are outside this slice and the rule (queue, deliver,
  never cancel) holds without it.
- `PeerInput` (A2A/peer `<message>`) shares the `UserInput` shape and is
  covered by reading, not by a test: it needs a durable exchange row.
- No changelog file exists in the repo; the design doc carries the note.

## Not built

- A supervisor-level (`Agents`) end-to-end test with the real `Jobs` actor
  emitting `Settle::FinishedWithResult`; the actor-level test injects the
  `<job_result>` text through the same `UserInput` handler `inject` uses.

## Verification

- `cargo test -p agent --lib compact_queue` → `1 passed` (four scenarios, 6.8 s).
- `cargo check -p tui` → `Finished dev profile`.
- `bunx tsc --noEmit` in `crates/desktop/ui`: no errors in `Composer.tsx`;
  the 670 pre-existing errors are missing generated `@/rpc/bindings/*` and
  `packages/apps` types (not produced in this checkout).

DONE COMPACT-QUEUE
