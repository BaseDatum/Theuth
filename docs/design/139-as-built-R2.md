# 139 as-built — R2 HARNESS-ENVELOPE (the Q3 follow-on)

Branch `feat/139-r2-harness-envelope`, worktree `theuth-139r2`, from `origin/main` `589a72f2` (139 at
`44c0d63d`, 136 at `589a72f2`). Design: `139-mesh-durability.md` Revision 2 (R2-D1–D6, appended;
§13 line). Brief: `~/.theuth/briefs/2026-10-08/139-R2.md`. Closes `139-as-built.md` §7 first bullet.
Line numbers are of the tree at the last commit below.

## 1. What R2 changed, where

| area | before | after | files |
|---|---|---|---|
| Wire (R2-D1) | nine `Msg` frames, tags 0–8 | `Msg::Harness { session, kind, origin, id, text, wrapped, scope, at }` (tag 9), `Msg::HarnessAck { id, error }` (tag 10); frame test covers both | `sessionsync/src/types.rs` |
| Store (R2-D1) | — | `harness_outbox(id, session, kind, origin, text, wrapped, scope, at, attempts, last_error)`, `harness_seen(id, at)`, both engine-local (not in `capture::ATTACHED`); `Store::{harness_enqueue, harness_outbox, harness_sent, harness_refused, harness_delete, harness_mark_seen, harness_unsee, harness_prune_seen, harness_watch}`; `Store.harness_tx` watch | `agent/migrations/0040_harness_outbox.sql`, `agent/src/store/harness.rs` (new), `store.rs` (+field, +mod) |
| Sender (R2-D2) | `inject` → `warn!` + drop for a remote owner | `inject` remote branch: `(produced on <me> after the session moved to <owner>)` inside the closing tag, `HarnessRow` enqueued, `info!("139: {from}: queued for the owner")`; `HarnessDeliver { session, kind, text, wrapped }` tell → the same `inject` | `agent/src/supervisor.rs` |
| Shipper (R2-D2, D3) | `other =>` debug for unknown frames | `flush_harness` after the `Hello` exchange, on `harness_watch`, every `HARNESS_RETRY` (60 s): owner re-resolved per row from `sessions.owner`, sent only to the stream's peer; rows of sessions back here → local `HarnessDeliver`; TTL → `warn!` + `append_local_note` + delete; `harness_seen` pruned past the TTL. `receive_harness`: known session → `harness_mark_seen` → `HarnessDeliver` tell → `HarnessAck`; dup → ack; unknown → `error: "unknown session"`; no supervisor → unsee + `"no supervisor here"` | `sessionsync/src/sync.rs` (`Node.harness_ttl`, `Node::new(store, rt, ttl)`), `ship.rs` |
| Config | — | `sessions.harness_outbox_ttl_secs` (default 604 800 = 7 days) | `sessionsync/src/config.rs`, `crates/theuth/harness.md` |
| Reminders (R2-D4) | boot-only `rearm_reminders`; no cancel on a move | `rearm_reminders_for(Option<SessionId>)` (boot `None`); `RearmReminders { session }` tell from `Take::take` after `set_fence`; `tools::CancelReminders { session }` (timers + guards dropped, **no** `ReminderCancelled`, rows kept) from `drain_session` step 4 and `transfer::demoted`; `close_reminders` keeps publishing via the shared `drop_reminders` | `agent/src/supervisor.rs`, `supervisor/transfer.rs`, `sessionsync/src/owner.rs`, `tools/src/reminder.rs`, `tools/src/lib.rs` |

## 2. Deviations from the brief (closest correct thing, recorded)

| what | brief | built | why |
|---|---|---|---|
| `kind` | enum `{job_result, reminder, spared_result, other(String)}` | `String`, the `from` label verbatim (`job j3`, `reminder r1`, `session:<child>`) | the receiver needs the label for `MessageDelivered.from`; no reader of a discriminant exists |
| Ack frame | one frame, "on success delete the row" | `HarnessAck { id, error }` (tag 10) | a frame written to a stream is not a delivery; the error carries "unknown session" / "no supervisor here" back so the sender keeps the row with `last_error` |
| `scope` on the receiver | — | carried on the wire, delivered unscoped | `network.scopes` is engine-local; an unknown scope drops the body as "closed" |
| Rearm hook | `transfer::on_resume_here` | `owner::Take::take` after `set_fence` | `on_resume_here` runs only inside `resume_turn`; a take with no open turn would never arm; two hooks would double-arm |
| `CancelReminders` location | `tools/src/jobs.rs` | `tools/src/reminder.rs` (+ re-export in `lib.rs`) | reminders live there; `close_reminders` publishes `Cancelled` → `delete_reminder`, which on the release path precedes the fence move and would replicate the delete to the taker |
| Migration | 0039 | **0040** | 0039 is 136's `budget`; checked `~/Develop/theuth-*/crates/agent/migrations` at commit time |
| T10 | `shell("sleep 20; echo done")`, connected transfer, real result on B ~20 s later | T10: reminders follow the owner on a connected transfer; T10b: the envelope on a takeover | the release drain's `KillSession` (139 §7.1) kills the job on a transfer — no later result exists; a takeover signals nothing (T3), so the job outlives the demote. `job_grace_secs: 120` in the smoke config keeps `shell` inline; a PTY `start` is the background job |
| Files outside the list | — | `justfile` (recipe filters for the new tests, comment), `crates/theuth/harness.md` (config line), `crates/agent/src/store.rs` (+2 lines), `tools/src/lib.rs` (re-export), `agent/src/store/harness.rs` (new) | needed for the gates / the config doc |

Not built: a hop counter on a re-forwarded envelope (A→B→A ping-pong while two engines disagree on the owner converges with the `sessions` row; bounded by the 60 s retry and the TTL); UI for `harness_outbox` (D6: none).

## 3. Gates (this tree, build-lock `139r2`, `CARGO_BUILD_JOBS=4`, own `./target`)

| gate | result |
|---|---|
| `cargo test -p sessionsync --lib --tests` | 18 + 2 passed (4 new: `sync::tests::harness_*`) |
| `cargo test -p agent --lib -- supervisor::transfer store::harness` | 5 passed (new: `rearm_reminders_for_arms_one_sessions_rows_here_only`, `store::harness::tests::*`) |
| `cargo test -p agent --lib -- supervisor:: durability::` | 109 passed |
| `cargo test -p tools --lib -- --test-threads=1 reminder::` / `jobs::` | 7 passed (new: `cancel_reminders_stops_the_timers_and_keeps_the_rows`) / 22 passed |
| `cargo clippy -p agent -p sessionsync -p tools --tests -- -D warnings` | clean |
| rustfmt `--check` on touched files | clean |
| `cargo check --workspace --tests` (BUILD_SLOTS=1) | clean after copying the gitignored, generated `bindings/methods.json` from Sky's checkout (`crates/nodes` `include_str!`s it; absent in a fresh worktree — environment, not this wave) |
| `just durability-mesh` (in-process half, recipe now also runs `harness_`, `types::`, `store::harness`, `cancel_reminders_stops`) | green: 2 + 10 + 34 + 1 + 10 tests |
| `scripts/durability-mesh-smoke.sh T10 T10b` | T10 8/8; T10b 11/11 on the re-run (§4) |

## 4. Smoke evidence (T10/T10b, two real engines, fake provider)

T10 (first run, 8/8): reminder `r1` fired on B `+29.2 s` after it was set on A, `late="false"`, exactly one
`<reminder>` on B, `0 fired on A`, A's log `reminder timer(s) stopped`, A's PTY group dead (drain), A's
outbox empty, no reminder row left on either engine.

T10b: B saw A unreachable after ~35 s (QUIC idle), took at epoch 2, A demoted at `+36.0 s` with its PTY
still running; A's log `job j1: queued for the owner` twice — the PTY's `<job_idle>` nudge at 60 s and
the `<job_result exit="0" pty="true">` at 75 s — B's log `job j1 from <A> handed to the supervisor`
twice, 2 ms after each; both landed in B's transcript with `(produced on <A> after the session moved to
<B>)` inside the element; reminder fired on B `+90.3 s`, `0 fired on A`; A's transcript equals B's. The
first run's three T10b fails were the row's own needle matching the `SMOKE` prompt (it contains
`DONE-T10b`); fixed, re-run below.

T10b re-run (11/11): unreachable after ~35 s, demoted `+35.6 s` with the PTY alive; `<job_result exit="0"
pty="true">` with `DONE-T10b` and the `(produced on <A> after the session moved to <B>)` line arrived on
B `+75.0 s`, exactly one; `outbox=0 seen=2 envelopes=2` (the `<job_idle>` nudge and the result); B's
`jobs` row untouched (engine A, never un-lost); reminder fired on B `+88.7 s`; `3 TurnFinished on B`
(idle, result, reminder — the batch ran on A); `0 fired on A`; A's transcript shape equals B's. The
two T10b fixes between runs were the row's needle (the `SMOKE` prompt contains `DONE-T10b`) and the
TurnFinished count (B never ran the first turn); the engine was unchanged.

## 5. Open

- The `(produced on …)` line is appended by every hop: a body forwarded twice carries two lines.
- A result that settles on the old owner *before* the demote lands is that owner's orphan (R2-D5); the
  envelope starts at the demote.
- `flush_harness` runs per stream; with no stream open nothing ages out until one opens (the TTL note
  is written by the first flush after expiry).
