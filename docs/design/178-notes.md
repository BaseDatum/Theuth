# 178 DESK-WEDGE — working notes

Worker notes for `178-desk-wedge.md`. Branch `fix/desk-wedge` off `042255a1`.

## Verified before changing (all held on `042255a1`)

K (timer armed only for the spawn turn; `TurnStarted|TurnResumed` arm armed ACP only), R (slot held to the turn's end),
L (judge deferral + `dispatch` busy skip), status (`working` only on `TurnStarted`, `serving` ignored), silent child
failures, `delivered()` resetting the retry counter — each line of the brief read at the cited code. One more cause
found live (below, "restart wedge").

## Ambiguities decided (simpler option, decisions intact)

1. **Floor never above the cap.** `remaining_run = max(cap − ran, min(60 s, cap))`. A cap under 60 s (tests, odd configs) is not raised by the floor; one formula, no fresh/resumed special case (`ran = 0` gives the cap).
2. **`ran` for ACP** = the open journal turn's first → newest step. A drained ACP child has the `suspended` row, so it continues its own cap; a crashed one has only `input_accepted` (ran 0 → full cap). The journal has nothing better.
3. **`ran` for native resumes** is read from the store (`Store::turn_span`) at `TurnResumed`, ending at the event's `interrupted_at`, so steps the resumed turn writes meanwhile never count. `TurnResumed` gained no field (no ts/event churn).
4. **W2 journaling**: the opener's `input_accepted.source` already says `peer`; a scoped depth-0 user input now journals `scoped` (nothing read `source` before). `ResumePlan.{started_at, bounded}`. The bound applies to any resumed bounded turn; at depth > 0 only the permit discipline returns (the wall clock is the supervisor's, W1).
5. **W2 remaining time** uses the same `remaining_run` (60 s floor) as W1. The resume-slot wait counts against it (the deadline is set when `Resume` arrives); W3 keeps that wait short.
6. **W3 hold** is `RESUME_SLOT_HOLD` = 60 s, 2 s under `cfg(test)` so the durability test does not wait a minute. The slot is released after the first `round()` call returns (the prelude ran before it), or when `drive` returns.
7. **`waiting_on` labels**: `AgentInfo` has no title; a nameless child shows `session:<first 8>`.
8. **`summary_lists_children_the_desk_waits_on`** tests the pure `nudges::waiting_on` + `AgentSummary::waiting_label` the summary calls: `children_of` needs a live agent actor with child rows, which the roster harness cannot create without running turns.
9. **W6 error reason**: the supervisor now records a child turn's `AgentEvent::Error` (`child_errors`, keyed by turn); the prefix takes its first non-empty line, ≤ 200 chars; `unknown error` without one. `timed_out` is also taken before the missing-result early return now (it used to leak into the next turn's result).
10. **W8 telling the parent**: `child_gone` clears every row on the child and injects one `<message from="session:<child>">` unless the parent is deleted with it or closing. Called from `drop_queued` (all its callers: stop, scope cancel, archive, unload, delete, private close) and once at the top of `DeleteSession` for the whole tree (parents inside the tree quiet).
11. **Resume wake to a taken session** (W5): `DeliverWake` finishes the wake (`FinishWake`) once the taken session has its input, instead of serving it on the desk.
12. **TUI**: `agents_table` shows the same `waiting on …` text (one call to `waiting_label`).
13. **Org card**: the chip got `min-w-0` (it truncates; full list in `title`) and the open count `shrink-0`, so the card row stays one line.

## Restart wedge — found live, fixed (beyond the brief's list)

On restart the roster opens the desks **before** the tasks plugin starts (plugin order: `roster` 05:52:11.044, `tasks`
05:52:11.059). `open_desk` sends `ReleaseClaims(desk)` only `if let Some(t) = tasks::actor()`, so the dead turn's
claim stayed in `wakes.claimed_by` while the new roster had no `serving`: `wakes held` for good, `idle` in the view, and
neither the judge nor the W5 backstop (no `serving`) could release it. That matches "a restart did not help".
Fix (`roster/src/lib.rs`): a reopened desk with no tasks actor goes to `Roster.unreleased`; `TasksUp` (sent right after
`RegisterDeskRouter`) releases their claims before enabling dispatch. Safe: while the stale claim stands the agent is
busy, so no live claim by that desk can exist yet. `ponytail:` a wake another desk of the same agent claimed through the
router in the ms between `RegisterDeskRouter` and `TasksUp` would be released too (none can: one desk per agent).

## Live proof (scratch engine, `target/debug/t` @ worktree, data `/tmp/wedge/eng*`, fake on 127.0.0.1:18431)

Setup: `secrets.key_store: file`, `tools.approvals: false`, `rpc.listen 127.0.0.1:18531`, mesh local, communication on,
`agent.subagents.max_runtime_secs: 90`, plugins off. Provider `fake` via `add_provider` (fake key), default
`fake/crawler`. Desk `hans` hired by file; two tasks `TASK-ALPHA…`, `TASK-BRAVO…` owned by hans (`task_create`).
The fake (`/tmp/wedge/fake.py`) answers the ALPHA wake with `spawn_subagent{task: "CRAWL-CHILD: …"}` and streams
`tickN ` every 10 s for 10 min to any conversation whose first user message is that brief. Scripts:
`/tmp/wedge/{fake.py,rpc.ts,setup.sh,shot.ts}`.

1. **Before** (from the code at `042255a1`, not rebuilt): a native child's resumed/follow-up turn had no timer
   (`supervisor.rs:2642-2647`), so the crawler would have run its full 10 min after a restart and the desk stayed
   `serving` → `wakes held` for the whole crawl; `idle` in the view (`working` false between turns). The restart
   wedge above was **observed live** on run 2 with the W1–W8 build before the `TasksUp` fix:
   ```
   05:52:11Z roster: desk opened agent=hans …               (tasks plugin starts 15 ms later)
   05:52:16Z tasks::ledger: wakes held: desk still serving an earlier wake agent=hans behind=01a1b48f-43b5…
   05:54:39  wakes: 01a1b48f-43b5|assigned|<desk session>   01a1b48f-47dc|assigned|<none>   (child long finished)
   agents_list: hans idle None
   ```
2. **Cap** (run 2): `05:50:53 wake delivered agent=hans` → crawler spawned 05:50:53; run 3: spawned 05:59:53,
   `crawler|cancelled|06:01:23.6`, desk transcript row `[timed out after 90s, stop=cancelled]`.
3. **Cap across a restart** (run 2): SIGTERM at 05:51:39 (46 s into the child's turn), 20 s down, start 05:52:10:
   ```
   child journal: 1 input_accepted 05:50:53.28 · 2 model_call 05:50:53.31 · 3 assistant_partial 05:51:33 · 4 suspended 05:51:39.85
   05:52:11Z agent::supervisor: 178: resumed child turn capped with its remaining run time session=01a1b48f-4f5c… ran_secs=46 budget_secs=60
   crawler|cancelled|2026-11-07T05:53:11.011788+00:00          (60 s after the resume, not 90; the 20 s down not counted)
   peer_exchanges newest: "[timed out after 90s, stop=cancelled]\n…"
   ```
4. **Status** (run 3, child running): `agents_list` → `hans working ['crawler']` (run 2: `['crawler-2']` after the
   re-delivery). Screenshots (vite dev server on 18577 against the scratch engine, headless Chrome, dark):
   `/tmp/wedge/shots/agents-waiting-on.png` (org card chip `waiting on crawler-2`, truncated, one line),
   `/tmp/wedge/shots/agent-detail-waiting-on.png` (detail header chip `waiting on crawler-2`).
5. **Queue moves.** Run 2 after the `TasksUp` fix (same data dir, restart 05:56:55):
   ```
   05:56:55Z roster: 178: claims of a desk reopened before tasks started released session=01a1b48f-38e9…
   05:57:00Z tasks: wake delivered agent=hans wake=01a1b48f-43b5… (ALPHA, re-delivered) → turn not judged (child live)
   ```
   Run 3 after the cap: desk answers the result → judged (`Todo` unmoved, no live child) → one nudge → `blocked: no
   continuation` (`stale wakes dropped task=…ALPHA state="blocked"` 06:02:30) →
   `06:02:33Z tasks: wake delivered agent=hans wake=01a1b497-8af2… (BRAVO)`; no `wakes held` after it. Run 1 (first fake
   heuristic crawled the desk's own peer-opened turn too) also showed the depth-0 bounded turn's wall clock:
   `05:49:30 wake released: turn did not yield` → `05:49:35 wake delivered … (BRAVO)`.
6. **Starvation** (run 3, top-level `durability.resume_parallel: 1`, two crawling root turns, SIGTERM 06:07:32):
   ```
   06:07:38Z 109: 2 open turn(s); resuming d905…, resuming d4d1…
   d905: 8 model_call 06:07:38.75 · 9 assistant_partial 06:08:58   (holds the one slot, crawling)
   d4d1: 8 model_call 06:08:38.89 · 9 assistant_partial 06:08:58   (resumed 60 s later while d905 still runs)
   fake.log: 06:07:38 CRAWL start · 06:08:38 CRAWL start
   ```
   Before W3 the second turn waits for the first turn's end (10 min here).
   (A first attempt put `resume_parallel` under `agent.durability`; it is the top-level `durability:` section — both
   then resumed at once, 06:06:01.)

All processes started for the proof (engine, fake, vite, Chrome) were stopped.
