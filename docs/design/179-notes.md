# 179 PAUSE-SUSPEND — working notes

Worker notes for `179-pause-suspend.md`. Branch `fix/pause-suspend` off `565ac0d2`.

## Phase A — reproduced on the base build

Scratch engine: `target/debug/t` built at `565ac0d2` (copied to `/tmp/pause/t-base`), data `/tmp/pause/eng-a`,
RPC `127.0.0.1:18779` (set with `THEUTH_RPC__LISTEN`: on this Mac `rpc.listen` from the scratch `config.yaml`
failed with `EADDRINUSE` while the env var binds — not investigated), fake OpenAI server `/tmp/pause/fake.py` on
`127.0.0.1:18431` (provider `fake`, key `fake-key`, default `fake/crawler`). Desk `hans` hired by file; routine
`hansly` (`every: 1h`, `agent: hans`). The fake answers a TASK-ALPHA wake with `spawn_subagent{CRAWL-CHILD}`, makes
the child crawl (`tickN` every 10 s, 150 s), makes a TASK-SLOW wake crawl in the desk turn (90 s), acks the rest.

What made "wake messages come through in the chat" after a pause — **three paths, all reproduced**:

1. **The pause's own journal turn.** `close_desk(.., "paused")` sends `<journal> Write your journal…` into the desk
   (`roster: journal written … ; episode closed … reason=paused`), every pause. It shows in the chat as a system
   input and runs a full model turn (14:39:05, 14:41:53, 14:46:50 → 14:48:06, 14:52:25).
2. **Child results.** Paused at 14:41:53 while the ALPHA crawler ran: the child kept going, finished 14:43:30 and its
   result entered the paused desk as `<message from="session:01a1b674…">` at 14:44:23, opening a new desk turn.
3. **Inputs queued behind a running turn.** Paused at 14:46:50 mid-turn: the turn ran on to 14:48:06, then the
   queued `Wake: task … TASK-ALPHA. Reason: assigned` (a re-delivery, see 5) and the journal prompt landed in the chat.

Per path of the brief:

1. *Idle pause, then task / mention / routine*: no delivery; wakes raised (`user wake hans` + `wake … held: agent is
   paused` thread note); dispatch retried every 5 s with a WARN each time (`wake not delivered: held: agent is paused`,
   27 lines in 9 min); the routine created its task (`schedule fire: task created, wake raised`). After the un-pause
   the first held wake waited 11 s (back-off accrued while paused).
2. *Mid-turn with wakes queued*: the turn ran to its end, then queued input entered the chat (path 3 above).
3. *Live child*: the child ran its full 150 s; its result entered the paused desk (path 2).
4. *Graceful restart while paused*: drain wrote `suspended{shutdown}` for the paused desk's open turns; at boot
   `109: 1 open turn(s); durability.resume = Auto` → `109: resuming turn session=01a1b66f…` and a model call ran in
   the paused desk (fake `#25 ack msgs=26`, 14:52:46).
5. *Stale PutAgent `paused: false`*: `agent_put` of the file read while paused (paused flipped to false) at 14:53:08
   → `desk opened agent=hans` → `wake delivered … assigned` at 14:53:11: silently un-paused.

Also seen: the ALPHA `assigned` wake `01a1b673-261d…` was **delivered three times** (14:41:00, 14:46:05,
14:53:11): `EpisodeClosing` released the served wake's claim at each pause, so every un-pause re-delivered the wake
the desk was still continuing. Pause races the delivery: the routine's wake was delivered at 14:39:05.095 and the
pause recorded at 14:39:05.557 (the desk closed only when the queued `Reconcile` ran).

## Ambiguities decided (simpler option, decisions intact)

1. **No journal turn on a pause.** It is a model turn in the paused desk (path 1 above); the episode closes at once
   (`reason=paused`) and the journal of the last real close stands, as at a quit.
2. **Stale-desk close relabelled `closed`.** `sync_desks` used `"paused"` for every desk no longer allowed (removed
   file, proposal, lease); those keep `EpisodeClosing` and the journal turn, only a real pause skips them.
3. **The hold decides, not the journal.** The pause writes `suspended{paused, ran_ms}` first; a turn reaching a round
   inside the drain window also writes the loop's own `suspended{drain}` after it (109 wording kept; `Draining` is
   shared with the quit and release drains). The resume cause reads `shutdown`.
4. **Boot rule.** The agent crate reads `agents/<name>.md` (front-matter line `paused: true`) for the desk of each
   open turn, because `resume_scan` runs before the roster is up. The roster's load then maps and holds every paused
   agent's desk (`PauseTree`, drains nothing): one `179: paused …` info pair per paused agent per boot.
5. **Quit while paused** skips the held trees' turns: a second `suspended` row without a run clock would make
   `run_ms` count the paused time.
6. **Held inputs live in memory** (`Agents.paused`). Restart while paused: a held human input is dropped; a held child
   result / peer message stays an `accepted` exchange row. A quiet (171 §9.3) input is held and replayed as waking.
7. **Served wake across a restart** is rebuilt from the ledger's claim (`ClaimedWake`) with a task-event count of 0.
8. **A desk turn that finished inside the drain window** is recorded (`PausedDesk.finished`) and judged at the
   un-pause, so its wake is neither lost nor re-delivered.
9. **"After the resumed turn ends"** is the router's `is_paused` staying true while the desk finishes its resumed turn
   (`Shared.resuming`, cleared at its `TurnFinished` or by the Tick backstop, then `tasks::Unpaused`). In that window
   the thread says `queued …` and a routine fire is skipped. When only children resumed, the restored claim keeps the
   agent busy until the served wake is judged (normal rule).
10. **Routine skip** applies to a forced fire (`fire_schedule`) too; `Skipped{reason: "agent paused"}` is published.
11. **Labels:** `Paused · 1 wake held` (singular); `held_wakes` counts unclaimed wakes.
12. **PutAgent:** an existing but unparseable file has no stored value — the sent `paused` is written. An agent-draft
   "Hire paused" on an existing name (Replace) keeps the stored value; the switch pauses it. The budget card's spread
   is left as is: the engine now ignores its `paused`.
13. **ACP children** in a paused tree are drained the same way (no `Draining` for a harness: the window, then stop) and
    come back through `resume_turn`'s ACP respawn.

## Phase C — live proof (new build `/tmp/pause/t-new`, data `/tmp/pause/eng-c`, same fake)

Fake changed for the proof: after `spawn_subagent` the desk's turn crawls 60 s (so desk and child are both mid-turn),
the child crawls 90 s. TASK-ALPHA (spawns the crawler) and TASK-BRAVO created 15:26:56.

1. **Pause mid-turn with a live child** (15:27:22.36, reply at .40):
   ```
   15:27:22.394 roster: 179: paused: desk closed, its session tree suspending agent=hans desk=01a1b69e-7c9d…
   15:27:32.398 transfer: 139: session drained session=01a1b69e-b78c… (child) reason="paused" finished=false
   15:27:32.398 transfer: 139: session drained session=01a1b69e-7c9d… (desk)  reason="paused" finished=false
   15:27:32.398 pause: 179: paused: session tree suspended desk=… turns=[(desk, 01a1b69e-b760…), (child, 01a1b69e-b790…)]
   desk  journal: … 4 model_call 15:26:57.426 · 6 suspended 15:27:22.396 {"reason":"paused","ran_ms":25016}
   child journal: … 2 model_call 15:26:57.455 · 4 suspended 15:27:22.396 {"reason":"paused","ran_ms":24968}
   fake.log: 15:27:47 #7 after-tool client went away · #8 crawl client went away   (no model call after it)
   ```
   Both actors stopped at the 10 s drain deadline (`durability.drain_secs`); `list_agents` → `crawler idle`.
2. **While paused** (15:28:01): TASK-CHARLIE created, `@hans` comment on ALPHA, `fire_schedule hansly` →
   `-32000 agent paused`, `schedule_history` → `skipped: agent paused`. Desk chat unchanged (last row the 15:26:57
   tool result); 0 `wake not delivered` lines, no warn from tasks/roster; wakes table: four rows, only ALPHA's claimed
   (by the desk). `agents_list` → `hans paused held_wakes=3 suspended=true`. A task created after the restart
   (documents helper copied from the app bundle by then) shows `* 15:31 **user** queued for hans (paused) (assigned)`.
   Screenshots (vite dev on 127.0.0.1:18577 against the scratch engine, headless Chromium): `/tmp/pause/shots/agents-paused.png`
   (org card `Paused · 3 wak…`, truncated as the 178 chip is), `/tmp/pause/shots/agent-detail-paused.png`
   (`Paused · 2 wakes held · suspended mid-turn`, taken at a second pause after step 4); TaskDetail placeholder
   `Comment — queues a wake for hans (paused)`.
3. **Graceful restart while paused** (SIGTERM 15:29:26): `109: drained reason="shutdown" suspended=0` (the held
   tree is left alone); boot `179: agent is paused; its open turns stay suspended agent=hans`; no `109: N open
   turn(s)`, no `resuming turn`, no fake call.
4. **Un-pause** (15:31:54):
   ```
   15:31:55.068 109: resuming turn session=01a1b69e-7c9d… (desk)   · 15:31:55.072 … session=01a1b69e-b78c… (child)
   15:31:55.072 pause: 179: un-paused: session tree resumed turns=[desk, child]
   15:31:55.072 roster: 179: un-paused: tree resumed, desk reopening turns=2 replayed=0 desk_turn=true
   15:31:55.072 178: resumed child turn capped with its remaining run time session=01a1b69e-b78c… ran_secs=24 budget_secs=875
   15:32:55     desk's resumed turn ends → turn not judged: a child of the desk is still running (ALPHA kept)
   15:33:25     child ends → its result enters the desk → nudge → 15:36:25 ALPHA blocked, its wake finished
   15:36:30 wake delivered … TASK-BRAVO (assigned)   ·   15:38:05 wake delivered … TASK-CHARLIE (assigned)
   ```
   The ALPHA `assigned` wake was delivered once in the whole run (15:26:57); each held wake once, after the resumed
   work ended, oldest first.
5. **Stale PutAgent while paused** (15:29:18, `paused: false`, title changed): title written, `paused: true` kept,
   no desk opened.

All processes started for the proof (engines, fake, vite, headless Chromium) were stopped.

## Not reproduced / environment

- `rpc.listen` from the scratch `config.yaml` failed with `EADDRINUSE` even on `127.0.0.1:0`; `THEUTH_RPC__LISTEN`
  works. Not investigated (scratch only).
- **Deviation:** the Phase A engine and the first Phase C run had the `signal` plugin on by default: it tried Signal
  provisioning (`signal provisioning could not start … reqwest error`, 48 + 14 attempts). `signal.enabled: false` and
  `update.enabled: false` were set from the Phase C restart on. No model call went anywhere but the fake.

## Fix round (review of `d77fa098`: CHANGES)

Decided while fixing (simpler option, decisions intact):

1. **Un-pause during the drain** (MUST 1): `ResumeTree` is a delegated reply; while `Agents.pausing` has the desk it
   waits and `TreeStopped` answers it after the stop. The roster's `PauseAgent{false}` therefore replies after the
   drain (≤ `durability.drain_secs`).
2. **ACP cut** (SHOULD 7): the stop's `cancelled` end of an ACP child can reach the supervisor after a quick un-pause;
   `acp_cut` pairs it with `TreeStopped` in either order, so it is never a turn end (no journal close, no
   `[child turn cancelled]` for the parent). A user's cancel of a paused tree's turn is swallowed too (nothing runs).
3. **Human text refused** (SHOULD 4) for `InputOrigin::External` only (typed text, channels, the roster's own
   `SendInput`); harness inputs (`inject`: results, notices, reminders) are still held — in memory, so a harness
   notice held at a restart is lost; child results and local peers' messages are durable `accepted` rows. A kept
   exchange is re-admitted under the current policy generation at the un-pause (`readmit_exchange`): the generation
   from before a restart no longer exists and the intake suppressed it as revoked (found live, below).
4. **Boot hold** (MUST 3) moved from "trees with an open turn" to every paused agent's desk from the files
   (`boot_accounting`, also sparing their exchanges from `interrupted_by_restart`); `hold_paused_trees` is gone.
5. **One parser** (SHOULD 5): `pause::front_matter` (gray_matter YAML, CRLF read as LF) is what `roster::file::parse`
   now calls; `agent_file_paused` reads `paused` with it. Removed agent: `sync_pauses` releases its hold
   (`ReleaseTree` closes its open turns `cancelled`). Invalid file: its raw text decides; a lifted hold resumes the
   tree without opening a desk.
6. **Reminders** (MUST 2): the pause stops the timers of every tree session (rows stay), boot re-arm skips held
   trees, `ResumeTree` re-arms per session. A reminder whose time passed while paused fires `late` at the un-pause.
7. **Held child brief** (SHOULD 8): `start_child_turn` keeps the permit and arms no timer for a held brief;
   `turn_began` arms when the entry has no timer, from the real start.
8. **Test flake found**: `drain::draining()` is process-global; a 109 drain test in the same test binary makes every
   later ACP journal close a no-op. The tests that set it or need it unset hold `drain::DRAIN_FLAG` and clear it
   (`reset_draining`), so the ACP journal-close assertions always run.
9. **PAUSE3** (review of `323a0e1e`): the stop's own `cancelled` normally never comes (the stop rejects the prompt
   task's `PromptDone`), so the `acp_cut` marker outlived the pause and swallowed the child's next real cancel. It is
   now cleared by the child's next `TurnStarted`/`TurnResumed` (a stale cancel of the stopped actor is ahead of it on
   the bus) and by `resume_tree` for a session nothing re-prompts. A queued un-pause is answered at the drain
   deadline plus `UNPAUSE_GRACE` (5 s) at the latest; a late `TreeStopped` resumes what stopped after that.

Live proof (`/tmp/pause/t-r3`, data `/tmp/pause/eng-d`, fake on 127.0.0.1:18431; `signal`, `update` and `telegram`
disabled from the first start — `signal disabled by config`, `telegram disabled by config`, `update disabled by
config`, 0 provisioning lines):

- **(a) un-pause within 3 s.** TASK-SLOW's desk turn crawling; pause 16:56:04.96, un-pause 16:56:07.04 (reply
  16:56:15.06): `16:56:07.121 179: un-pause while the pause drains; resuming after the stop` → `16:56:15.042 179:
  paused: session tree suspended … turns=[(desk, 01a1b6f0-42f9…)]` → `16:56:15.047 109: resuming turn` →
  `179: un-paused … turns=1 desk_turn=true`; the resumed turn finished 16:57:15 and was judged. `wake delivered`
  for TASK-SLOW once (16:56:01).
- **(b) reminder across a pause.** TASK-REMIND set `r1` for 16:58:31; pause 16:57:50, un-pause 16:58:02; the desk
  got one `<reminder id="r1" … late="false" at="16:58">` at 16:58:31, the row gone.
- **(c) paused, no open turn, overdue reminder + held peer message, restart.** Reminder due 17:07:22; paused
  17:06:42; a side chat's `send_message` to the desk at 17:06:44 → exchange `01a1b6fa accepted`; stopped 17:07:35,
  restarted: `179: agent is paused; its desk tree is held`, no model call (fake log unchanged at 70 lines), row and
  exchange untouched. Un-pause 17:08:22: one `<message from="session:01a1b6f3…">` and one `<reminder id="r1" …
  late="true">` in the desk chat, exchange `consumed`, reminders table empty. The first run of (c) (build `t-r2`)
  delivered the reminder but logged `peer intake failed: communication_disabled` and left the exchange `suppressed`
  — the stale generation; fixed by the re-admission and re-run with `t-r3`.
- **(d) human input** to the paused desk: `send_input` → `-32000 "hans is paused — un-pause it to send"` (the
  composer shows RPC errors as a toast).
