# 182 TASK-HOME — working notes

Worker notes for `182-task-home.md`. Branch `fix/task-home` off `8316913d` (v2.18.11). Scratch: `/tmp/taskhome`.

## Rig (both runs)

As 180/181: `setup.sh` writes `mesh.relay: none`, `mesh.discovery: false`, `mesh.bind: 127.0.0.1:1865x`,
`mesh.hub: eligible`, `rpc.listen: 127.0.0.1:1855x`, link/nodes/signal/slack/telegram/update/sandbox and the other
outside plugins off, `secrets.key_store: file`, `tools.approvals: false`, fake provider. Started with
`THEUTH_RPC__LISTEN` / `THEUTH_LAUNCH` unset (`start.sh`); log line `mesh: engine … on relay none (discovery false)`.
Fake model `fake.py` (one per engine, 127.0.0.1:18451 for A, :18452 for B, fake key): a desk wake whose brief says
`FAKE-CHECKOUT` gets a `task_checkout` then a `task_update {continuing}` tool call; `HOLD` streams 240 s, `SLOW` 20 s.
Paired with `mesh_pairing_string` on A → `mesh_add` on B. A has `agents/hans.md`; theuth is on both. UI: vite dev on
18587 (→ A) and 18588 (→ B), headless Chromium (localhost only). Every process started was stopped at the end.

## Before — the bug on the unmodified build (`/tmp/taskhome/t-base` = `8316913d`)

`repro.sh`: on A, theuth tasks in todo (`SLOW` briefs), one checked out by A's theuth desk, one in review, a hans task
(hans only on A), an `agent: theuth` schedule `every: 1m`; then B paired (`/tmp/taskhome/evidence-base.txt`).

```
B 01:03:17 mesh: lease → Some("eb254474") (elected)          A keeps its own lease → two holders
B 01:03:21 schedule fire: task created … 01a1b8ae-6ed1        (A fired the same schedule at 01:03:00, 01:04:00 …)
B 01:03:37 wake delivered agent=theuth … task=01a1b8ae-4a73   "Summarise the week", created on A
fake-b 01:03:37 call 1 role=user wake_task=01a1b8ae-4a73 …    B's model works A's task
B desk 01a1b8ae-acf4 (desk=theuth) seq 0: "Wake: task 01a1b8ae-4a73… — Summarise the week. Reason: assigned."
B 01:04:02 wake delivered … task=01a1b8af-06f1 reason=schedule   (A's 01:04:00 fire, delivered on B)
A 01:03:59 wake delivered … task=01a1b8ae-6ed1 reason=schedule   (B's fire, delivered on A)
```

Schedule fires in 3 min: A 4 (`:00` each minute), B 4 (`:21` each minute) — two tasks per minute, never coalesced
(the unique key holds each engine's own `fire_ms`).

## Ambiguities decided (simpler option, decisions intact)

1. **The lease flag in the ledger** is a closure (`Ledger.lease`, `mesh::is_hub` outside tests), not a message:
   `SetDispatching` is gone (D4 removes its only meaning).
2. **A legacy (NULL-home) task the lease holder checks out is homed there** by the checkout. That makes D4's "except
   NULL-home tasks, which follow the lease" vacuous after D7: a lease loss blocks nothing (nothing of ours is unhomed
   and in doing), so `lease_changed` only logs.
3. **"Agent work is gated"** = the owner agent's own writes (`agent:<owner>` on update / move / reassign / comment /
   note / disposition) on a task not worked here, plus checkout for anyone. Managers, reviewers, the user and the
   system still write from any engine (a manager commenting on a report's task across machines stays allowed).
4. **Moving clears the checkout** by fencing it (`orphaned`, epoch + 1, session kept) rather than deleting it, so the
   next checkout's epoch keeps rising and the old session reads as superseded. The move also deletes the wakes the
   old home's desks had **claimed** on the task (delivered there: not delivered twice; and a claim left behind kept the
   new home's agent "busy" until the 30 s claim sweep — seen live in run 1, 32 s delay). Unclaimed wakes stay.
5. **D10 RPC vs tool.** RPC `task_move_home(id, to)` (with the 181 session move for a live, non-desk, non-harness
   checkout session owned here); a desk-session checkout is fenced, not moved (181 refuses desks). Tool
   `task_move_home{task, to}` (name or id) moves without the session (the forward is the RPC's).
6. **D9 state.** Existing Attention machinery: a derived kind `home` (id `home:<engine>`) on the lease holder only;
   "Keep waiting" is the existing `attention_snooze` (1 h); "Move them to …" is one RPC `task_move_home_waiting(from)`
   (no new item fields, so no other Attention producer changed). The unreachable clock is the ledger's
   `unreachable_since` (also the orphan sweep's), fed by the 30 s sweep and by the ask. The item stays while a task
   that cannot move here still waits (it names it); it goes when the engine returns or nothing waits.
7. **D8 storage**: a new replicated table (`engine_agents`), the old `agents` table untouched; the tasks router reads it
   from the store (no actor hop: tasks asks inside its handlers, the roster asks tasks inside its own).
8. **D6 home rule** applies to every schedule fire, stamped or not (a stamped fire on an engine that lacks the agent
   then homes on the one that has it instead of stranding it). The editor's create (`Put` of a new name) stamps too.
   `ScheduleInfo.engine` + a routines chip `other machine` (otherwise a stamped schedule showed `not hub` on a non-hub
   engine that does fire it). Autoresearch's `disable_all` skips study files stamped for another engine (same class:
   its switch-off replicated).
9. **D5 in code**: heartbeat `is_hub` was already on the wire; `Seen.is_hub` + pure `lease::resolve`. Probation still
   applies between two holders (otherwise a fresh server would grab the lease at pairing, against mesh.mdx). A starting
   engine with stored peers ends its warm-up on the first heartbeat instead of the connect, so it sees the holder.
10. **`last_seen_ms`** of a disconnected peer is now the disconnect time (`presence_remove`): run 1 showed
    "a is offline (since 01:47)" for an engine stopped at 01:52 (the connect time). Also fixes 181's `offline Nm`.

## Live proof — final run (`/tmp/taskhome/t-final` = `0f35f0a5`, `final/evidence.txt`, `shots/final/`)

Engines A = `a` (a0862ba7…), B = `b` (67e50a44…). Paired 02:20:45.

1. **Pairing.** `02:20:55.295 B mesh: lease → Some("a0862ba7") (two holders)` — one holder 10 s after pairing (D5).
   02:20:45 → 02:24:11: B delivered 2 wakes, both of the task moved to B in step 3 (home `67e50a44` on both stores);
   none for an A-homed task. `minute` (stamped `engine: a0862ba7…` by `put_schedule`) fired on A 7×, on B 0× over the
   whole run, including after A stopped. B's board: `on a` chips, foreign cards dimmed (`1-b-board-muted-on-a.png`).
2. **Move.** A's task ⋯ → Move to ▸ lists `b` (`3-a-move-submenu.png`); CDP stepped pointer path → toast `Moved to b`,
   `Home: theuth on b`, events `home a → b`, `wake theuth` (`3-a-after-move-toast.png`). A 02:21:34 `182: task moved`;
   B 02:21:37 / 02:21:42 delivered its `assigned` (never delivered on A: A's desk was busy) and `reassigned` wakes; B's
   desk checked it out (checkout on B). A delivered nothing for it.
3. **Agent missing.** On B, the hans task's Move to ▸: `b — no hans agent there`, `data-disabled`
   (`4-b-hans-no-agent-there.png`). Agents page, List view: `theuth — on b`, `Release writer — on a`
   (`agents/hans.md · only on a`), `theuth — on a` (`2-b-agents-per-engine.png`).
4. **Offline home** (B `tasks.home_offline_ask_secs: 30`). A stopped 02:25:12.88 (SIGTERM); B `lease → 67e50a44
   (disconnect)`. B's detail: `Home: theuth on a · a is offline (since 02:25)` + callout
   (`5-b-detail-home-offline.png`). 02:26:16: exactly one `home` item, `a has been offline 1 min — 8 tasks are waiting:
   …`, detail `Not movable here: Draft the hans changelog (no hans agent here).` (`5-b-attention-home-item.png`).
   **Move them to b** → toast `Moved 7 tasks to b / Draft the hans changelog: b — no hans agent there`; the item now
   lists the one hans task (`5-b-moved-toast.png`). B delivered the 7 moved tasks' wakes 02:26:37–02:27:07 (3 s after
   the move).
5. **Old home returns.** A restarted 02:27:27; adopted B's lease (`lease → Some("67e50a44") (elected)`, grace kept).
   For all 8 moved tasks: 0 wakes delivered on A after the restart, 0 model calls naming them, home `67e50a44` on both
   stores. Limit seen: A's boot resumed its theuth desk's interrupted turn (a nudge on "Weekly digest", moved) —
   one text reply, no task tool call, no write (the row stayed `todo` on B's home); see the design's Limits.
6. **Checks.** `PRAGMA foreign_key_check` on tasks/sessions/roster/mesh/schedule: 0 rows on A and B; no `FOREIGN KEY`
   log line. Deliveries per wake id across both logs: no wake delivered by two engines. One same-engine repeat: A
   delivered the hans changelog wake again after its own restart (178: a reopened desk's claims are released) — A's
   own task, not a cross-engine duplicate.

Run 1 (`t-fix` = `acde8e93`, `run1/`) showed the same plus two bugs fixed before the final run: the 32 s busy delay
after a move (note 4) and the stale "offline since" (note 10); and one wake delivered by both engines (A's desk had
claimed it, B redelivered it after the move) — gone with note 4.

## Gates (final tree)

1. `mbx check -p tasks -p roster -p schedule -p mesh -p agent -p rpc -p sessionsync -p autoresearch --tests`: 0 errors,
   0 warnings (`Finished dev profile`).
2. `mbx clippy` same set `--tests --no-deps -- -D warnings`: 0.
3./5. `mbx test -p tasks -p roster -p schedule -p mesh -p rpc -p autoresearch` (built `--no-run` on that set), twice:
   tasks 118, roster 99, schedule 81, mesh 29, rpc 79 + 4 integration, autoresearch 304 — all ok both runs.
4. Not applicable: no agent code changed.
6. Bindings export (`8 passed`), `tsc -b --noEmit` 0 `error TS`, vitest (tasks, attention model, hud, routines,
   rpc/api) twice: 11 files, 109 tests passed.

## Review round 1 — live proof (`/tmp/taskhome/t-r1`, `r1/evidence*.txt`, `shots/r1/`)

Rig as above (`after2.sh <binary A> [binary B]`).

1. **M1 staggered upgrade** (`r1/evidence-m1.txt`): A on t-r1, B on `t-base` paired; B logged `tasks.home is not a
   replicated column here` and `touches unattached table engine_agents; skipped`. B restarted on t-r1: on connect A
   re-announced, B then held every task's `home` (= A) and A's `engine_agents` rows (hans, theuth). That run also
   exposed a latch bug: `caught_up_with` was keyed by the replica origin, the latch asks by mesh id, so B waited the
   150 s ceiling; fixed (keyed by the transport peer) before the rest — A's restarts below log `first sync done after
   1 s (caught up with every connected peer)`.
2. **Step 1:** no wake of A's five tasks reached B's model. **S1:** a hans task created on B is homed on A (hans only
   on A); A's hans desk got its wake. **Step 3:** user move A→B from A: home = B on both, B's model woken, A's
   model never again for it.
3. **S10:** a review on a task homed on B shows on A (`on b`, shot 2); Approve on A → `done` on B.
4. **M2:** A's theuth desk mid-turn (HOLD) on T_HOLD; A killed; T_HOLD moved to B on B; A restarted:
   `182: desk tree with an open turn held until the first sync` → `first sync done after 1 s` → `the held turn served
   a task now worked on another engine; closed, not resumed`. No turn for it on A (the one model call naming it is the
   endless memory pump resumed at boot: no tool call, no write); home stays B. The earlier variant (a task moved while
   the desk served another one) resumed the other one and never touched the moved task.
5. **Misplaced:** kim's file moved A→B after a kim task was created on A: A's Attention shows `misplaced:<task>`
   `kim isn't on a; it's on b` (chip `agent elsewhere`, shot 4); dispatch logs `wake not delivered: no such agent
   here` once. **Move to ▸** for a hans task on A lists `b — no hans agent there` (shot 3).
6. **Step 5:** A stopped; after 30 s (`home_offline_ask_secs` on B) one item `node:<A>` kind `home`, `a is offline — 9
   tasks are waiting: …` (shot 5); **Move them to b** → `Moved 9 tasks to b` (shot 6); kim's task woke B's kim.

## Gates (round 1 tree)

1./2. `mbx clippy -p tasks -p roster -p schedule -p mesh -p agent -p rpc -p sessionsync -p autoresearch -p replica
   --tests -- -A clippy::chunks_exact_to_as_chunks`: 0 (the allow is for a lint new in this toolchain that fires in
   `voice`, untouched here).
3./5. `mbx test -p tasks -p roster -p schedule -p mesh -p rpc -p autoresearch -p replica --no-fail-fast`, twice:
   tasks 119, roster 99, schedule 81, mesh 29, replica 18 + 10, rpc 79 + 4 — ok both runs; autoresearch 304 ok in
   run 2, one timing test (`world::…rollback_failure…`, then `runner::…timeout_kills…` in an earlier run) failed once
   under load in run 1; neither file is touched by 182.
4. `mbx test -p agent --lib --no-fail-fast` (`THEUTH_WORKER` = fresh `target/debug/t`): 785 passed, 2 failed —
   `sensor::…monty_budgets_and_allowlist` (a < 3 s deadline measured 5.15 s under load; passes twice alone, 3.3 s)
   and `repl::…capping_binds_out_name` (fails the same with the `t-base` worker: not this branch; neither file is
   touched by 182). `boot_resume_waits_for_first_sync` passes.
6. Bindings export `8 passed`; `tsc --noEmit` 0 errors; full vitest twice: 2008 passed, 2 failed both runs —
   `disclosure.test.ts` (`chat/SessionMenu.tsx`) and `no-emoji.test.ts` (cells `▶`, CELLAFTER `042255a1`), files
   this branch does not touch.

## Review round 2 (`/tmp/taskhome/t-r2`, `r2/evidence.txt`, `shots/r2/`)

1. **Step 1:** no wake of A's five tasks reached B's model.
2. **M2 restart, B's changes while A was down:** A stopped; on B a review moved to `done`, a new task was created, a
   user comment woke hans on A's T_HANS. Before A's restart A held none of it. After: `first sync done after 3 s
   (caught up with every connected peer)`, no `skipped` line, the review `done`, the new task present (homed on B),
   the `comment` wake for hans present, `parked` empty. A second restart caught batches arriving before their
   tables attached: `touches unattached table doc_files/schedules/skills; parked until it attaches`, each `parked …
   applied` a moment later, `parked` empty again, B's new task present. (Tasks attach inside the tasks plugin's start,
   before `plugins_started`, so a tasks batch parked there holds the first sync; mirror tables attach a moment later
   and do not.)
3. **Offline item** (shot `1-b-attention-a-offline-short-title.png`): title `a is offline — 5 tasks are waiting`, the
   titles in the detail (`and N more` past five: `offline_home_is_one_node_item_with_a_short_title`).
4. Tests: `batch_for_an_unattached_table_applies_after_attach` (replica).

## Gates (round 2 tree)

1./2. `mbx clippy -p tasks -p roster -p schedule -p mesh -p rpc -p sessionsync -p autoresearch -p replica -p theuth
   --tests -- -A clippy::chunks_exact_to_as_chunks`: 0.
3./5. `mbx test -p tasks -p roster -p schedule -p mesh -p rpc -p autoresearch -p replica --no-fail-fast`, twice: tasks
   119, roster 100, schedule 81, mesh 29, replica 19 + 10, rpc 79 + 4 — ok both runs; autoresearch 304 ok in run 1, in
   run 2 one `world::…` test failed at `world.rs:2444` (the study's `Paused` read right after `settle_launch` flips the
   experiment — a polling race seen under load in earlier runs on other `world::` tests; 182 touches neither). The 15
   `world::` tests pass alone.
4. Not run: no agent code changed in round 2.
6. `tsc --noEmit` 0 errors; full vitest 312 files / 2010 tests, 0 failures, twice on an idle machine (one more run
   under the Rust tests' load hit 9 `Failed to start forks worker` timeouts, no test failure). The `disclosure` and
   `no-emoji` failures are fixed (`SessionMenu` in `CHROME`; the cells' `▶` → `►`).
