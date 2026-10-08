# 146 RESTART-CLARITY — design + as-built

> **The rule everything below serves (Sky, 15:37):** "as long as that resume button is a safety valve.
> priority is that they reliably and durably restart exactly where they left off when I quit the harness
> and then restart it."

Branch `fix/146-restart-clarity`, worktree `theuth-146`, from `origin/main` `8fbade19`. Brief:
`~/.theuth/briefs/2026-10-08/146.md`; recon: `143-recon/report.md`. Background: 109 §8 (ACP
children), 139 as-built §7 (`child_result` waits). Never touches `crates/acp/**` or
`crates/agent/src/acp_child.rs` (141 owns them).

## 1. What happened (recon, verified)

Restart 14:12Z: two full boots 3 s apart (rpc :53279, :53379). The boot respawn of ACP children
(`respawn_acp_children`) was triggered only by `child_waits(Here)`; an orchestrator that had ended
its turn had no such wait, so its three mid-prompt omp children (125R1 W1–W3) were never respawned,
nobody was told, and the dead children's dangling `ToolCall` rendered as a running tool card.
`open_turns` ran 4× per boot at ~1.1 s each (correlated `MAX(at)` per `turn_steps` row).

## 2. Decisions (orchestrator, 15:35) and how they landed

| # | decision | as built | files |
|---|---|---|---|
| D1 | Respawn every mid-prompt ACP child at boot, whatever the parent's wait state | Trigger set = `child_waits(Here)` ∪ `Store::midprompt_acp_children(since)`: ACP children here (`agent IS NOT NULL` — the harness marker `set_acp` writes; `acp_session` arrives only once the harness connected —, `parent IS NOT NULL`, `kind != 'advisor'`, `updated_at > now − acp_respawn_window_days`) whose **last `messages` row** is `User`, `Tool`, or `Assistant` with a `ToolCall` part. Parent gate: row exists, `here`, not archived. Deduped by child; count logged (`146: mid-prompt ACP children in the window n=…`) | `store/durability.rs`, `supervisor/acp_resume.rs`, `durability/mod.rs` (config) |
| D2 | After `resume_scan`, ≤ `acp_respawn_parallel` (4) at once, through `respawn_acp_child` | `Agents.boot: Option<Boot>` holds the queue; `pump_respawns` spawns while `inflight < parallel`; a slot frees on the child's first real `TurnStarted` (harness accepted the prompt → *respawned*) or a `TurnFinished` before one (its `fail` turn → *lost*). The actor already publishes `TurnStarted` after `session/load`; nothing was added around the call | `supervisor/acp_resume.rs`, `supervisor.rs` (`AgentEvent` hook) |
| D3 | Tell the parent about every child that cannot come back | `lose()`: `append_local_note(child, "interrupted by the engine restart at <t>; not respawned: <why>")` + `inject(parent, …)` of `<child_lost name session at why>` with the one-line advice (Automatic origin; cross-engine via 139 R2). Reasons: parent archived / owned elsewhere / gone (no inject: nobody to tell), child owned elsewhere, harness not local, `cwd` unreadable, spawn error, harness did not connect | `supervisor/acp_resume.rs` |
| D4 | One `resume:boot:<ts>` attention item with live counts + one INFO line | `agent::durability::BootPass { at_ms, resuming, respawned, waiting, lost: Vec<(session, name)> }` published on the bus by the pass and on every settle; `Attention` keeps the latest (`Inputs.boot`), `derive` emits `resume:boot:<at_ms>` (title `boot_title`: "Restarted 14:12 · 3 sessions resumed · 2 subagents back · 1 waiting · 3 lost (a, b, c)"), `session` = first lost child so the HUD's Dismiss renders; a count change with the same id publishes an empty-diff `AttentionChanged` so the HUD refetches; settled + lost = 0 → `ClearBoot` 60 s later; lost > 0 → stays until dismissed. Log: `146: boot pass resuming= respawned= waiting= lost= lost_names=` | `roster/src/attention.rs`, `durability/mod.rs` |
| D5 | Boot query: one window pass, index, computed once | `NEWEST` = `ROW_NUMBER() OVER (PARTITION BY session_id, turn_id ORDER BY (kind = 'assistant_partial'), n DESC)` + `MAX(at) OVER (PARTITION BY …)` over the whole table (partials sort last, `last_at` still sees them); migration **0043** `turn_steps_turn_at(session_id, turn_id, at)` (0041 is 141's `observed_children`); `OpenTurnRow` alias; `ActivityRegistered` computes `open_here()` once and passes it to `reap_jobs_in`, `rebuild_waits`, `resume_scan`, `deliver_spared_results`; `reap_jobs(store)` kept for other callers | `store/durability.rs`, `migrations/0043_turn_steps_turn_at.sql`, `durability/jobs.rs`, `supervisor/{resume,waits}.rs`, `supervisor.rs` |
| D6 | flock single-instance guard | **Already existed**: `rpc::engine::acquire` (`File::try_lock` on `<data_dir>/engine.lock`, main.rs before `Runtime::new`). What was missing: a refused headless/desktop daemon exited **3**, which launchd (`KeepAlive.SuccessfulExit=false`) / systemd (`on-failure`) restart-loop. Now `Host::Headless \| Desktop` print the holder line (`engine already running (pid …)`) and exit **0**; TUI/-p/--acp keep 3. Desktop `launch.rs::start`: a daemon that exits before listening is followed by one `probe()` — attach to the holder instead of an error (10 lines) | `theuth/src/main.rs`, `desktop/src/launch.rs` |
| D8 (delta 15:35) | The header's "↻ Resume" pill on an ACP child must do what the boot respawn does | `ResumeChild` → `Result<ChildResumed, String>` (`Prompted \| Idle`, ts-exported). Same gates as before, then the live actor is stopped and `Agents::relaunch_acp_child(child, Continue)` — the one function the boot pass (`respawn_acp_child`) also calls — spawns the actor from the row and tells it `Respawn` when `Store::midprompt(child)` (the D1 predicate on one session), the 75 §10 `CANCELLED_NOTE` when the last stop was `cancelled`, nothing when idle. The child's `TurnStarted` (published by the actor once the harness accepted the continue prompt, ~1–2 s) clears the pill and fills the dot. RPC `resume_child` returns the enum; desktop toast "resuming the interrupted task" / "reconnected; the harness was idle, nothing to continue" (`ModelMenu.tsx`, 3 lines; `api.ts` type); TUI `/resume` message likewise. Parent state is not a gate (user intent). Unit test `resume_child_prompts_a_midprompt_child`; smoke P7 | `supervisor.rs` (`ResumeChild`, `ChildResumed`), `supervisor/acp_resume.rs` (`relaunch_acp_child`, `Continue`), `store/durability.rs` (`MIDPROMPT`, `midprompt`), `rpc/src/api.rs`, `tui/src/lib.rs`, `ui/src/rpc/api.ts`, `ui/src/components/chat/ModelMenu.tsx` |
| D9 (delta 2) | Journal ACP prompts; no heuristics at boot | Dispatch sites are both in `supervisor.rs`: `route` → `LiveActor::tell_input` (`UserInput`) and `enqueue_peer` → `offer_peer` → `ask_peer` (`PeerInput`). Before the tell, `Agents::acp_journal_open` writes one `turn_steps input_accepted` for the child (payload `{source, text excerpt, parent, from}`; `Store::acp_turn_open`, at most one open turn per child). The bus hook `acp_journal_event` opens one on the child's `TurnStarted` when none is (harness-queued / synthetic prompts) and closes it on `TurnFinished` (`Store::acp_turn_close` → `turn_finished_in`, `sessions.last_stop`). `relaunch_acp_child` journals the respawn prompt itself. Boot: `split_acp_turns` partitions the pass's one `open_turns` into ACP (→ `respawn_acp_children`, one list one pass) and native (→ `resume_scan`); `resume_turn` on an ACP row (ask-mode button, takeover, RPC) relaunches with `Continue::Prompt` instead of "left to its harness". `midprompt_acp_children` is the fallback only for rows with no `turn_steps` at all | `supervisor.rs`, `supervisor/acp_resume.rs`, `supervisor/resume.rs`, `store/durability.rs` |
| D10 | Graceful quit = kill -9 for mid-prompt children | The drain already writes `suspended{reason: shutdown}` on every open turn — with D9 that includes the child's. The child's `TurnFinished{cancelled}` the shutdown provokes is **not** journaled while `drain::draining()` (the suspended row stays open), so the boot sees owed work and `Respawn`s: `handoff::restored_note(store, session, at)` now reads "The theuth engine restarted at <t> while you were working on: \"<interrupted prompt excerpt>\" … Continue exactly where you left off — do not start over … checkpoint-commit it first" (never 75 §10's cancelled note, which only `ResumeChild` sends and only on an in-process `last_stop == cancelled`). Shutdown-cancel vs user-cancel is durable in the journal: `suspended{shutdown}` vs `turn_finished{cancelled}` | `supervisor/acp_resume.rs`, `durability/handoff.rs`, **`acp_child.rs:1151` (1 line: the note takes `&self.store, self.id`)** |
| D11 | Fallback when `session/load` fails | The acp crate already falls back to `session/new` on a failed load (`harness.rs:554`) on the same row/parent/name/cwd/agent; `compose_handoff` is now the RESUME brief: "The theuth engine restarted at <t> and your previous harness session <acp_session> could not be loaded … read git log and git status first", `Your original brief: "<first user message>"`, `The prompt you were interrupted in: "<last user message>"`, then the delta sections, "do not start over". `BootPass.fresh` counts a respawned child whose first `TurnStarted` text carries that header; title "· N restarted fresh" (distinct from "back"); log field `fresh=`. A connect that fails after the fallback is lost (D3) | `durability/handoff.rs`, `supervisor/acp_resume.rs`, `durability/mod.rs`, `roster/src/attention.rs` |
| D7 | UI ≤ 40 lines | (a) `interruptDangling(entries)` in `reduce.ts`, applied in `loadHistory` when `loaded.agent && !loaded.running`: the trailing result-less `tool` entry gets `interrupted: true`; `MessageTool` drops the spinner and shows "interrupted by restart" greyed. (b) `TextDelta`/`ToolCallPending`/`ToolCallStarted` set `running: true`; `trackRow` wakes an `idle` parent row to `running` on those events (an `awaiting` row is kept). Vitest ×2 | `ui/src/store/{reduce,sessions}.ts`, `ui/src/components/chat/MessageTool.tsx`, `reduce.test.ts` |

## 3. Deviations from the brief (and why)

- **`finished_at` is per turn, not per session**: every active orchestrator has it set (last turn's end);
  ACP children never set it. The brief's `finished_at IS NULL` test would have excluded every live parent,
  so the parent gate is *exists ∧ here ∧ not archived* and the child filter drops it.
- **Mid-prompt predicate includes a trailing `Tool` row** (tool result the model never answered): on the
  backup DB 5 of 25 mid-prompt children were in that state; the brief named only `User` / dangling `ToolCall`.
- **"Window exceeded" is not a lost reason**: children outside `acp_respawn_window_days` are not scanned
  (bounded scan is the point); nothing is written for them.
- **Migration number 0043** (0042 is 144's `session_notebook`), not 0041 (taken by 141's `observed_children` in `theuth-141*`).
- **D6 already existed** (63 §0.1); the change is the exit code + desktop attach, not a new lock.
- **D7(a) marks the tool entry (`interrupted: true`) instead of converting it to `kind:"interrupted"`**:
  the `interrupted` kind renders through `MessageInterrupted` ("interrupted — resuming"), not the ToolCard,
  and loses the call's name/args. The flag keeps the card and the brief's wording.
- **Parent gone (row deleted)**: note on the child only — there is no session to inject into.
- **A connect failure also yields the child's own `<subagent_result stop="error">`** (109 §7.3) plus our
  `<child_lost>`: two messages, both true; kept for uniformity of the lost rule.
- **D8's "running at once" is the child's real `TurnStarted`**, not a synthetic one from the supervisor:
  `TurnStarted.text` is echoed as a user bubble by every UI, and a second start for the same turn would
  double the ledger's child-turn bookkeeping. The toast answers at once; the pill/dot follow within the
  harness's connect + prompt accept (1–2 s in P7).
- **D10 touches `acp_child.rs` by one line** (`queue_respawn`: `handoff::restored_note(&self.store, self.id, now).await`): the restored note needs the store to name the interrupted prompt; nothing else in that file or `crates/acp/**` changed. For design 141 to clear.
- **D9 journals at dispatch *and* on the child's `TurnStarted`** with supervisor-minted turn ids (the actor's ids are its own): the journal models "this child owes work" (one open turn per child), not the harness's turn boundaries. `turn_finished_in` now also sets `sessions.last_stop`/`finished_at` on ACP children.
- **141 coordination (delta 3)**: the trigger set (journal split, wait rows, messages fallback) and the ACP
  journal apply only to `sessions.kind IN ('chat')` (`store::durability::DRIVEN_KINDS`, `acp_resume::driven`):
  141's `SessionKind::Observed` watch-only children and advisors are never respawned or journaled. No
  match on `acp::Update` anywhere in 146. The boot-pass edit is one compact block in `ActivityRegistered`
  (141's `self.mark_observed_lost().await;` folds in after `self.respawn_acp_children(…)`); `Cancel` and
  `LoadSession` are untouched. The approved ≤ 5-line `acp_child.rs` hunk is **not** at the cancel path:
  D10 needed no marker there (the drain's `suspended{shutdown}` + the `draining()` skip carry it); the one
  line is in `queue_respawn` (`restored_note` takes the store) — same budget, different hunk; 141 to clear.
- **A quiet boot (0 resumed, 0 respawned, 0 lost) raises no item** — the brief did not say; a notice on
  every boot would be noise.
- **A lost child's `child_result` wait row is cleared** (`child_waits::child_result_delivered`): otherwise a
  child under an archived parent is re-reported on every boot through the 139 trigger (the smoke's P7
  caught P4's child coming back as `lost=1` a boot later). The note is its last row, so the D1 scan
  skips it too; one `<child_lost>` per child, ever.
- **A node's child (`<harness>@<node>`, or a registered node) stays *skipped*, not lost**: it runs on
  another machine and is not this engine's to declare dead; a bare harness name no longer in
  `agents.yaml` is lost.
- **`LastBootPass` ask** (not in the brief): the roster plugin starts after the agent plugin, so a pass
  that settles inside `ActivityRegistered` (P4: everything lost, nothing to wait for) was published
  before the Attention actor subscribed and the item never appeared. `Agents` keeps the last pass;
  `Attention::start` asks once and feeds itself the reply. Found by the smoke's first run.

## 4. Pacing ceiling (ponytail)

A respawned child whose actor dies without `TurnStarted`/`TurnFinished` (panic) keeps its slot; the
pass then never reports `waiting = 0` and the item stays. `acp_child`'s 120 s connect timeout covers the
realistic case (hung harness → `fail` turn). Upgrade path: a per-child timer that tells `Agents` a
`RespawnSettled` after `CONNECT_TIMEOUT + 10 s`.

## 5. Proof — `just restart-clarity` (`scripts/restart-clarity-smoke.sh` → `scripts/restart-clarity/smoke.ts`)

One scratch engine (`/tmp/146-smoke/engine`, fake OpenAI provider as the parent's model, the real
`omp acp` as the child). P1: spawn `w146` with "`sleep 150` then DONE", wait for its `ToolCall` row,
delete the parent's `child_result` wait (the 125R1 shape), `kill -9`, boot. P4: same with the parent
archived first. P7: same with `acp_respawn_window_days: 0` (boot skips it), then `resume_child` over RPC. P8: SIGTERM instead of `kill -9` (wait row and nothing else removed: the journal is the only trigger). P9: `kill -9` after deleting the child's `~/.omp/agent/sessions/*/*_<acp_session>*`. P1 deletes the child's `turn_steps` too, so it proves the pre-journal fallback. P5: second `t --headless` on the data dir. P6: `sqlite3` timing on a `.backup` copy.

Final run (`/tmp/146-smoke.log`; binary `target/w/debug/t` at the final commit); earlier runs: 17:06 failed P4's item (→ `LastBootPass`), 19:30 failed P7's first check (→ the wait-row clear), 20:52 failed P9's brief (it named the new harness id → `acp_previous`, 0044):

| proof | result |
|---|---|
| P1 `146: boot pass` respawned=1 lost=0 (pre-journal child: its `turn_steps` and wait row deleted) | ✓ `146: mid-prompt ACP children in the window n=1` → `boot pass … waiting=1 lost=0` → 0.76 s later (child's `TurnStarted`) `respawned=1 waiting=0 lost=0` |
| P2 omp process back ≤ 30 s; parent receives DONE later | ✓ new `omp acp` pid 1.3 s after boot; `<message from="session:<w146>" …> DONE </message>` in the parent ~160 s after the reboot (the respawned child re-ran `sleep 150`) |
| P3 `list_sessions` child `running: true` during the respawned prompt | ✓ `{"running":true,"name":"w146"}`; HUD item `Restarted 19:54 · 1 subagent back` |
| P4 archived parent → child lost | ✓ `boot pass … lost=1 lost_names=["w146b"]`; child's last row `System: interrupted by the engine restart at 19:57; not respawned: its parent is archived`; parent's messages carry `<child_lost name="w146b" session=… at="19:57" why="its parent is archived">`; no `omp acp` process; `attention_list` → `resume:boot:<ts>` "Restarted 19:57 · 1 lost (w146b)" with `session` = w146b (Dismiss renders) |
| P7 (D8) Resume button on a child the boot pass skipped (`acp_respawn_window_days: 0`, wait row deleted) | ✓ boot pass `respawned=0 waiting=0 lost=0`, no omp process; `resume_child` → `"prompted"`; new `omp acp` pid 3.9 s later; the child's next row is `User: <resume_handoff>… has been restored…`; `list_sessions` → `running: true` |
| P8 (D9/D10) graceful quit (SIGTERM) with the child mid-prompt | ✓ journal `input_accepted{source: input, text, parent}` while mid-prompt → after the quit `suspended{reason: shutdown}` (not `turn_finished`); boot `ACP children with an open journal turn n=…` → `respawned=n lost=0`; the child's next row and omp's own jsonl carry "restarted at <t> while you were working on: \"Run exactly one shell command: `sleep 150`…\" … Continue exactly where you left off"; parent got DONE; journal closed `turn_finished{end_turn}` |
| P9 (D11) `~/.omp/agent/sessions/*/*_<acp>*` deleted before the reboot (run 21:30) | ✓ `respawned=1 fresh=1 lost=0`; the child's next row: "your previous harness session <old id> could not be loaded … read git log and git status first", `Your original brief: "Run exactly one shell command…"`; `sessions.acp_session` is a new id, `acp_previous` the old; HUD `Restarted 21:20 · 1 restarted fresh`; parent got DONE |
| P5 second `t --headless` | ✓ exit 0 after 68 ms, stderr `engine already running (pid 49070, headless, since 19:57, ws://127.0.0.1:7961) — run `t` to attach …`; first engine's `info` still answers |
| P6 `open_turns` on the `.backup` copy (25 581 `turn_steps` rows / 1 027 turns) | ✓ old 1.12–1.26 s → new 0.085–0.09 s at 15:40/17:09 (cold-ish cache), old 0.63 s → new 0.052 s at 17:30 (warm): 12–14×; with the 0043 index the old query also drops to 0.04–0.07 s and the new is unchanged. Result sets identical (`EXCEPT` both ways = 0). The index pays for `open_turn` / `journal_prune` / `resume_report` (per-turn `at` seeks) |
| P5 second `t --headless` | ✓ exit 0 after 59–125 ms across six runs (one outlier of 1 171 ms at load average 66 with other waves' cargo builds: the 1.2 GB debug binary's own load time, not the guard), stderr `engine already running (pid 49070, headless, since 19:57, ws://127.0.0.1:7961) — run `t` to attach …`; first engine's `info` still answers |
| P6 `open_turns` on the `.backup` copy (25 581 `turn_steps` rows / 1 027 turns) | ✓ old 1.12–1.26 s → new 0.085–0.09 s at 15:40/17:09 (cold-ish cache), old 0.63 s → new 0.052 s at 17:30 (warm): 12–14×; with the 0042 index the old query also drops to 0.04–0.07 s and the new is unchanged. Result sets identical (`EXCEPT` both ways = 0). The index pays for `open_turn` / `journal_prune` / `resume_report` (per-turn `at` seeks) |

Observed while proving, not fixed here: `set_model(…, persist = true)` / `add_provider` append a second
top-level `agent:` block when the existing one is a flow mapping (`agent: { communication: true }`);
the duplicate key wins on the next boot and communication silently turns off. The smoke writes block
style. Worth its own ticket (config writer).

## 6. Gates (cold `target/w`, `CARGO_BUILD_JOBS=4`, through `build-lock` as `146`)

| gate | result |
|---|---|
| `cargo test -p agent --lib -- supervisor:: durability:: store:: acp_child::tests::respawn` | 193 passed, 0 failed (`respawn_decision_table` covers D1/D3/D9/D11, `resume_child_prompts_a_midprompt_child` D8, `restored_note_has_no_delta` D10/D11 wording) |
| `cargo test -p roster --lib` | 69 passed (+ `boot_pass_item`) |
| `cargo test -p theuth --lib` | n/a — `theuth` has no lib target |
| `cargo clippy -p agent -p rpc -p tui -p roster -p theuth -p desktop --all-targets -- -D warnings` (rpc also with `--features ts`) | clean |
| `rustfmt --check` on touched files | clean for 146's hunks; `crates/theuth/src/main.rs:1478/1505/1515` and `mcp.rs:259` were already unformatted on `origin/main` and are left alone |
| `cargo check --workspace --tests` (`BUILD_SLOTS=1`) | clean (after `just ts`: `nodes`' tests read `bindings/methods.json`, which only the export produces); one pre-existing future-incompat warning from the `block` dependency |
| `just ts-desktop` + desktop `bun run check` (tsc) + `vitest run src/store/reduce.test.ts` | 682 bindings exported (`ChildResumed` via `rpc/src/export.rs`); tsc clean; 64 passed (+2 for D7) |
| `just restart-clarity` | 41/41 checks (table above; ~10 min, five real `sleep 150` children) |

## Fold of main 25553238

`land/146` = `8f5362b2` (D8) + merge of `origin/main` `25553238` (145 TALK-PRESENCE `ea48854b` + 147 docs),
commit `8b5e4580`. Fresh worktree `~/Develop/theuth-146-land`, every cargo gate through `build-lock mbx`
(managed target, `CARGO_BUILD_JOBS=4`, label `146-land`), logs `/tmp/146-land-*.log`.

Conflict shapes: none textual. The five files both designs touched auto-merged at hunk level —
`durability/jobs.rs` (145's job cancel/promote next to 146's `turn_steps` journal), `supervisor.rs`
(145's `SetTalk`/`TalkLive`/`InputsParked` + input parking above 146's `ActivityRegistered` boot pass,
`ResumeChild` → `ChildResumed` and `relaunch_acp_child`), `rpc/src/api.rs` (talk RPCs + `resume_child`
returning `ChildResumed`), `tui/src/lib.rs` (145 ignores `InputsParked`; 146's resume toast), desktop
`reduce.test.ts` (145's `JobEvent.kind` fixtures + 146's two interrupt tests). The feared E0063 /
exhaustive-match misses did not occur: 146 adds no `ToolCx`/`JobEvent` literals and 145's new
`AgentEvent` arms sit in match sites 146 did not edit. No fix commit was needed. `<child_lost>` keeps
going through `inject`, so it respects 145's talk-window parking without a special case.

| gate | result |
|---|---|
| `mbx test -p agent --lib -- supervisor:: durability:: acp_resume::` | 115 passed, 0 failed (`resume_child_prompts_a_midprompt_child`, `respawn_decision_table`, `held_message_follows_the_brief` all green) |
| `mbx test -p roster --lib` | 69 passed |
| `mbx test -p rpc --lib` | 54 passed (incl. `engine::tests::second_acquire_in_one_process_is_running`) |
| `mbx test -p tui --lib` | 146 passed |
| `mbx test -p talk` | 29 passed |
| `mbx clippy -p agent -p rpc -p tui -p roster -p theuth -p desktop -p talk --all-targets -- -D warnings` | clean; only the pre-existing `block v0.1.6` future-incompat note |
| `cargo fmt --check` | 146's hunks clean; the `crates/theuth/src/main.rs:1478/1505/1515` diffs are identical on both parents (pre-existing) |
| `BUILD_SLOTS=1 mbx test -p rpc --features ts --lib export_bindings` + `cp -R bindings/. crates/desktop/ui/src/rpc/bindings/` | 683 files exported, no diff against the committed bindings |
| desktop `bun run check` (tsc) + `vitest run src/store/reduce.test.ts src/store/sessions.test.ts` | tsc clean (after `bun install --frozen-lockfile` for `@types/d3-sankey`); 66 passed |
| `just restart-clarity` (`BACKUP_DB` = `.backup` of the live store, 29 469 `turn_steps` / 1 357 turns) | 25/25; P6 old 0.882 s → new 0.06 s |
| `BUILD_SLOTS=1 mbx check --workspace --tests` | clean (same `block` note) |

### Second fold: main 8cd9c323 (141 ACP SUBAGENT VISIBILITY `cac2e5a2` + 125 R1 CONSULT)

Main moved again before the report (delta F1 named `cac2e5a2`; the tip was `8cd9c323`, which adds 125 R1
on top of 141 — folded whole, a land must be against the tip). Merge commit `5dcc0c36`, fixes `8a972304`,
`817e8af3`.

One textual conflict, `crates/agent/src/supervisor.rs` `ActivityRegistered`: 146's pass (`open_here()`
once, `rebuild_waits(&open)`, `resume_scan(&open)`, `respawn_acp_children(resuming)`) kept, then 141's
`mark_observed_lost()` — both run, in that order. Everything else auto-merged: `Cargo.toml`/`Cargo.lock`
(`agent-client-protocol =3.1.0`, theirs), migrations `0041_observed_children` (theirs) + `0043_turn_steps_turn_at`
(ours), `lib.rs` (`SessionKind::Observed`, `AgentEvent::ObservedChildChanged` — 146 has no exhaustive
`AgentEvent` match, `boot_child_event` uses `_ =>`), `ResumeChild` (141's `row.observed` refusal sits above
146's D8 mid-prompt decision), `acp_child.rs` + `acp_child/observed.rs` (theirs; 146 never touched them),
rpc `api.rs`/`acp.rs`, desktop `reduce.ts`/`ModelMenu.tsx` (141's `watching` gate and 146's D8 toast
coexist), `AgentTree.tsx`. Fix commits: `midprompt_acp_children` filters `c.kind = 'chat'` (was
`!= 'advisor'`) so 141's watch-only rows — never driven, no `agent` — are never a respawn candidate; the
`turn_steps` journal needs no filter (only a driven actor writes steps, observed rows have none). The tui
`model.rs:13773` test was red under `clippy -D warnings` (`field_reassign_with_default`) on `origin/main`
itself (141's `3a765294`); rewritten as a struct literal, no behaviour change.

Interaction noted, not changed: 141's `mark_observed_lost` checks which ACP actors are live when it runs;
146 spawns `acp_respawn_parallel` (4) respawns synchronously before it and queues the rest, so the
observed children of a queued respawn are marked `unknown`/"lost contact" at boot. A respawned harness is
a new process whose in-process subagents are gone anyway; its tailer re-reports anything still alive.

| gate | result |
|---|---|
| `mbx test -p agent --lib -- supervisor:: durability:: acp_resume:: acp_child::` | 157 passed, 0 failed |
| `mbx test -p acp` | 46 passed |
| `mbx test -p rpc --lib` | 55 passed |
| `mbx test -p tui --lib` | 155 passed |
| `mbx clippy -p agent -p rpc -p tui -p roster -p theuth -p desktop -p talk -p acp --all-targets -- -D warnings` | clean after `817e8af3`; same `block` note |
| `cargo fmt --check` | no diff in the touched files |
| `BUILD_SLOTS=1 mbx test -p rpc --features ts --lib export_bindings` + copy | 689 files, no diff against the committed bindings |
| desktop `bun run check` + `vitest run src/store/reduce.test.ts src/store/sessions.test.ts` | tsc clean; 67 passed |
| `just restart-clarity` (`BACKUP_DB` = fresh `.backup`, 29 807 rows / 1 396 turns) | 25/25; P6 old 1.384 s → new 0.098 s |
| `BUILD_SLOTS=1 mbx check --workspace --tests` | clean (same `block` note) |
