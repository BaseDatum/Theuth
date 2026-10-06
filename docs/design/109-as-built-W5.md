# 109 as-built — W5 (graceful drain + what the user sees)

Worktree `theuth-109-w5`, branch `feat/109-w5` off the contract commit 8a5f8b75. Built 2026-10-06.
Scope: §9 drain and its wiring, §10.3 RPC/surfaces, §8.2–8.4 desktop, TUI divider/status note,
schedule `OpenTurn` gating (§4.6 step 12). W1's journal writes and `ResumeTurn` were not in the
tree: everything here is driven by the contract's store API and events and tested against rows
written through it.

## 1. Drain — `crates/agent/src/supervisor/drain.rs`

- `Drain { deadline, reason }` (`:34`) → `DelegatedReply<DrainReport>` (`:66-139`). The handler
  snapshots the live session actors and leaves the mailbox (a turn ending inside the window asks
  `Agents`; waiting in the handler would deadlock). Off the mailbox, in order: every open turn
  (`store.open_turns`) gets `turn_steps(suspended, {"reason"})` through `Store::step` **first**
  (durable even if the desktop's 2 s `EXIT_GRACE` SIGKILLs us mid-window; `turn_finished` closes
  the row of a turn that makes it), then `wait_idle` (`:154`) polls each actor's `IsRunning` every
  100 ms until none runs or the deadline, then every turn still open gets `TurnSuspended { reason }`,
  then `Pool::close()` on the Monty pool (idle workers exit cleanly; a checked-out one goes with its
  turn and the previous dump stands, 08 §5), then the report
  (`suspended` = open turns at the end, `finished` = running actors that went idle in the window).
- `DRAINING` (`:25`): the first drain wins; a second `Drain` in the same process is a zero report.
  `draining()` is exported for anyone who needs the flag.
- `OpenTurns` (`:53`, ask → every open turn): one ask for the schedule plugin and the attention
  actor instead of one `OpenTurn` per row.
- `drain_engine(rt, reason)` (`:176`): looks up `Agents` (none → empty report at once), reads
  `durability.drain_secs` from `rt.section("durability")` (default 10), asks `Drain`, abandons it
  one second past the deadline.
- `Agents::resume_state(session)` (`:217`): `Loaded.resume` + the `Interrupted` history row from
  `open_turns` + `resume_plan`.
- Registration line `supervisor.rs:111` (`// 109 W5`).

Wired into every path that ends the engine (§9.3), always **before** `rt.shutdown.cancel()` — every
turn's cancel token is `rt.shutdown.child_token()` (`loop_.rs` `start_turn`), so a drain after the
token would find every turn already cancelled:

| path | where |
|---|---|
| SIGINT, SIGTERM (new), `parent_gone`, token already cancelled by a plugin/actor | `crates/theuth/src/main.rs:1523-1533` — one `select!` yields `(origin, reason)`, then `drain_engine`, then `cancel()`, then the existing 100 ms / `release()` / `exit(0)`; `sigterm()` at `:1564` |
| rpc `shutdown` (desktop quit, `/quit`) | `crates/rpc/src/api.rs:4580-4587` — spawned `drain_engine(…, "shutdown")` then `cancel()`; the RPC reply is immediate as before. `main.rs`'s drain after the token is then the no-op |
| `t update` relaunch | `crates/update/src/lib.rs:659-670` — `drain_engine(…, "update")`, `cancel()`, 100 ms, `exit(EXIT_RESTART)` (exit code kept: launchd `SuccessfulExit=false` / systemd `on-failure` restart on it) |

Jobs: `KillAll` still runs on the token (`tools/jobs.rs`, W2), which now fires after the window.

## 2. RPC / types (§10.3)

- `agent::HistoryEntry::Interrupted { text, at, context: None }` (`crates/agent/src/lib.rs:1666`);
  `context()`/`context_mut()` arms; `agent::ResumeState { turn, kind, interrupted_at }` (`:845`);
  `Loaded.resume: Option<ResumeState>` (`:837`, `serde(default)`, `ts(optional)`). Both ride the
  existing `Loaded` ts export (`export_all` is transitive; `bindings/ResumeState.ts` appears).
- `LoadSession` fills them at `supervisor.rs:5553-5556` after everything else is read (the row is
  appended last: a partial has no `seq`; it is the newest thing in the session by construction).
- `resume_turn(session) → bool` (`api.rs:1732-1735`, impl `:5508-5516`): `authorize`, `OpenTurn`,
  `ResumeTurn`; `false` when no turn is open. `bindings/methods.json` picks it up from
  `method_names()`.
- ACP replay of the new row: `rpc/src/acp.rs:599` (`[interrupted — resuming] …` chunk).

## 3. Attention (§8.3) — `crates/roster/src/attention.rs`

- `AttentionKind::Resume` (`:63`), id `resume:<session>:<turn>`, group "Needs a hand" on the
  desktop (`attention/model.ts`).
- `ResumeNeed` (`:126`) is the `Inputs.resumes` row (`:197`); `derive` renders it at `:558-576`:
  title `"<who> resumed after a restart with gaps"` when a `detail` exists, `"<who> was interrupted
  by a restart — resume?"` without one (`resume: ask`), `detail` = what was skipped.
- `resume_need(event, now_ms)` (`:949`): a `ResumeReport` with non-empty `interrupted` or
  `children_handed_off > 0` raises one; a clean report raises nothing (the banner is enough).
  The actor subscribes to `AgentEvent` (`:644`) and keeps the latest need per session (`reports`,
  `:619`); `attention_state` hides a dismissed id as for every kind.
- `resume: ask`: the actor reads `durability.resume` at start (`:627`) and, when `ask`, lists every
  open turn from `OpenTurns` as an item with no `detail` (`:747-756`) — the Resume button's item;
  it disappears when the turn closes.

## 4. Schedule (§4.6 step 12) — `crates/schedule`

- `store.rs`: `Store::open` keeps its contract (orphans → `lost`) and is now
  `open_sparing(path, &HashSet::new())`; `open_sparing` (`:68`) returns `(Store, Vec<OpenRun>)`,
  closing every orphan whose session is **not** in `spare` and returning the spared rows.
- `lib.rs:2591-2610`: the plugin asks `Agents` `OpenTurns` once (one ask, not one `OpenTurn` per
  row) and opens the store sparing those sessions; `Schedule::reattach` (`:967`) puts each spared
  row back into `running` with `turn_open = true` and a fresh deadline, so the resumed turn's
  `TurnFinished` closes it with its real stop reason through the existing `close`/`finished` path,
  and `is_running` keeps a new fire of that schedule `skipped: busy` meanwhile.

## 5. Desktop — `crates/desktop/ui`

- Store: `Entry.kind = "interrupted"` and `SessionSlice.resume: ResumeBanner` (`store/sessions.ts:145`,
  `:148-159`, `:276`). Reducer (`store/reduce.ts`): `historyEntries` `"interrupted"` (`:423`);
  `resumeFromLoaded` (`:430`, a live state already seen for the turn outranks the row);
  `TurnResumed`/`ResumeReport`/`TurnSuspended` arms (`:604-630`); `TurnStarted` of another turn
  clears the banner (`:797`); the first `TextDelta` removes the greyed partial (`:816`).
  `loadHistory` seeds the banner from `Loaded.resume` (`sessions.ts:446`).
- `components/chat/ResumeBanner.tsx`: `resumeText` (pure) + the strip, mounted after
  `OrphansBanner` in `routes/c.$session.tsx` — one line, `role="status"`, no modal; warning tone
  only when something was skipped.
- `MessageInterrupted` (`MessageInfo.tsx`), case in `Message.tsx`: greyed text under
  "interrupted — resuming".
- Jobs: `JobsSheet` shows `lost (engine restarted)` in the warning tone (`SessionMenu.tsx`); the
  tool card's job line says `lost with the engine · Ns` (`MessageTool.tsx:21-26`).
- Attention: `GROUPS` hand += `resume` (`attention/model.ts`), `KIND_LABEL.resume`, `KindActions`
  Resume (only for an `ask` item, `detail === null`) / Dismiss (`hud/Hud.tsx`), `useAttentionActions().resume`
  → `api.resume_turn` (`hud/queries.ts`), `ResumeScreen` (`attention/PlainScreens.tsx`, mapped in
  `Screen.tsx`), `api.ts` `resume_turn` + `METHODS`, empty-state copy.

## 6. TUI — `crates/tui`

- `Entry::Divider(String)` (`model.rs:1034`, height 3, rendered in the `Compacted` shape at
  `ui.rs:2007`). `TurnResumed` interjects `── resumed after restart ──` (`── resumed after update ──`
  for `Suspended`) and sets `Model.resume_note` (`model.rs:3206`), appended to the status line
  (`:6778-6783`), cleared by `TurnFinished` (`:5827`); `ResumeReport` rewrites the note with the
  counts and, when something was skipped or lost, adds one `Info` line; `TurnSuspended` interjects
  `── suspended for <reason> ──`. `HistoryEntry::Interrupted` replays as the
  `── interrupted — resuming ──` divider plus the greyed text (`:9687`); print mode prints the same
  (`lib.rs:1297`, events at `:1386-1400`).
- `/jobs`: the table prints `JobView.state` verbatim (`tools::jobs::table`), so `lost` shows once
  W2's `list_jobs` emits it; the live `JobEvent::Finished { exit: lost }` already renders
  `lost · Ns` through `JobExit`'s `Display`. No TUI change needed for it.

## Deviations from the design / brief

| what | design says | built | why |
|---|---|---|---|
| "stop accepting input" (§9.2 step 1) | new input accepted durably, no turn starts | not gated: input routes normally during the window; a turn it starts is waited on like any other and suspended at the deadline | the user row is persisted inside the loop's `start_turn` (`loop_.rs:3203`, W1's file, off-limits); a supervisor-side gate would drop the input or hold it in memory only — neither is "durably" |
| `suspended` row timing | after the window (step 4) | written at drain start, event at the end | survives the desktop's `EXIT_GRACE` (2 s, `desktop/src/launch.rs:534`) SIGKILLing a 10 s drain; `resume_plan` defers to the row before `suspended` so the order does not change the plan |
| `Drain` fields | `{ deadline }` | `{ deadline, reason }` | `TurnSuspended.reason` must come from the caller (`shutdown` / `update` / `parent_gone`) |
| `Drain` reply | `DrainReport` | `DelegatedReply<DrainReport>` (same `ask` result) | the handler must not hold the mailbox for the window |
| `HistoryEntry::Interrupted { content, at }` | `content` (the `MessageContent`) | `{ text, at, context: None }` | sibling variants carry `text`; `text_of` is what `render` shows for an assistant row; `MessageContent` has no ts/JsonSchema |
| `Loaded.resume.report` | `(kind, interrupted_at, report)` | `(turn, kind, interrupted_at)` — no `report` | nothing persists a `ResumeReport`; the live event carries it. Add once W1's `ResumeTurn` records it (or `Agents` keeps the last report per session) |
| schedule asks `OpenTurn(session)` per row | per-row ask | one `OpenTurns` ask, then `open_sparing` | one round trip; the answer is the same set |
| `t update` drain in `main.rs` | "the relaunch sends `shutdown` first" | the update tick loop drains itself before `exit(EXIT_RESTART)` | the relaunch exits from `update/lib.rs:660`, never through `main.rs`, and the exit code must stay 75 |
| `JobView.state = lost` | W5 renders it | rendered where the string arrives; the producer is W2's | brief: W2 owns `JobView.state=lost` |

## Cross-seam touches

- `crates/agent/src/lib.rs`: `HistoryEntry::Interrupted`, `ResumeState`, `Loaded.resume` (the
  types the brief lists under `crates/rpc/**` live here).
- `crates/agent/src/supervisor.rs:5553-5556`, `:5582`: `LoadSession` fills `resume` and appends the
  partial (4 lines + 1); `:111` the registration line.
- `crates/agent/src/session_read.rs:242-244`, `:306-308`: the new variant in two exhaustive matches
  (ignored: the partial is for the human).
- `crates/update/src/lib.rs:659-670`: the drain before the relaunch exit (see deviations).
- `crates/rpc/src/acp.rs:599-601`: the new variant in `replay`'s exhaustive match.
- `crates/tui/src/model.rs:13614-13625`, `:13668`: the pre-existing `ScheduleInfo`/`RunInfo` test
  fixtures lacked fields added since (`for_`, `cooldown`, `created`, `timeout`, `model`,
  `run_timeout`, `sensor`, `kind`); `cargo test -p tui` did not compile at the contract commit (nor on
  main b5aaf52d). Filled with `None` / `RunKind::Run` so the tui gate runs. In my owns list, noted
  because it is not 109 work.

## Not done

- No gate on new input during the drain (see deviations).
- `Loaded.resume.report` (see deviations).
- `attention` items do not clear "when the user opens the session" — the inbox has no open hook
  (nothing else clears on open either); Dismiss, or the turn closing (`ask` items), clears them.
- Desktop component tests are pure-function tests (`vitest` here runs `src/**/*.test.ts` only, no DOM
  library): the banner wording, the reducer's banner/partial/lost-job state, the group mapping.
- `desktop/src/launch.rs` `EXIT_GRACE` (2 s) is shorter than `drain_secs` (10 s): a desktop quit
  SIGKILLs a long drain; the `suspended` row is already written by then. Not my file; flagged.

## Tests added

- `agent`: `supervisor::drain::tests::wait_idle_counts_turns_that_make_the_window` (a quick fake
  batch finishes inside the window, a slow one runs it out), `drain_suspends_open_turns_once`
  (an open turn journaled through `Store::step` gets the `suspended` row — `resume_plan.kind ==
  Suspended` — and the `TurnSuspended { reason }` event; report `{ suspended: 1, finished: 0 }`;
  a second `Drain` is a zero report).
- `roster`: `attention::tests::resume_items_lifecycle` (a clean report raises nothing; skipped
  calls + a hand-off → one item with the detail; an `ask` item without detail; Dismiss hides it).
- `schedule`: `store::tests::reopen_spares_runs_of_resuming_sessions` (the spared row comes back
  as `OpenRun`, the other orphan is `lost`, a later `finish_run` closes it with the real reason).
- `rpc`: `api::tests::durability_wire_shapes` (`Interrupted` row JSON, `Loaded.resume` round trip
  and absent → `None`, `AttentionKind::Resume` → `"resume"`, `resume_turn` registered).
- `tui`: `ui::tests::resume_divider_takes_the_compacted_shape` (bordered `── … ──`, 3 rows),
  `model::tests::resume_events_draw_divider_and_status_note` (divider, status note with the
  `HH:MM`, `ResumeReport` counts + the skipped-call `Info` line, `TurnFinished` clears, the
  `Interrupted` history row replays as divider + text).
- desktop (`vitest`): `components/chat/ResumeBanner.test.ts` (every wording), `store/reduce.test.ts`
  two tests (banner state through `TurnResumed`/`ResumeReport`/`TurnStarted`/`TurnSuspended`, the
  `lost` job exit; the interrupted partial from `Loaded` + its removal on the first `TextDelta`,
  `resumeFromLoaded` precedence), `components/attention/model.test.ts` (`resume` → "Needs a hand").

## Gates

All with `CARGO_TARGET_DIR=$HOME/Develop/theuth-109-target`, cargo 1.98.1, siblings building
concurrently (lock waits of 5–15 min per command).

1. `cargo test -p roster` — `58 passed; 0 failed`.
2. `cargo test -p schedule` — `69 passed; 0 failed`; the doctest step failed once with
   `extern location for tools does not exist: …/libtools-b4a711a16e409ace.rlib` (a sibling's
   build replaced the artifact under the shared target dir mid-run); `cargo test -p schedule --doc`
   rerun: `0 passed; 0 failed` (no doctests).
3. `cargo test -p rpc` — lib `37 passed; 0 failed` (after adding `durability_wire_shapes`),
   `1 passed` (integration), `0` (doc).
4. `cargo test -p tui` — lib `124 passed; 0 failed`; `tests/no_emoji.rs` **fails** on
   `surface/src/slack.rs:177  ▶` — pre-existing (the line is in the contract commit and main; the
   `surface` crate is not mine). No other failure.
5. `cargo test -p agent` — `393 passed; 0 failed` (lib) + `0` (doc); the two drain tests alone:
   `cargo test -p agent --lib drain` → `2 passed`.
6. `cargo clippy -p agent -p rpc -p roster -p schedule -p tui -p theuth -p update --tests` — no
   warnings in files touched by this wave.
7. `just ts-desktop` (`just ts` → `export_bindings` `8 passed`) — `bindings/methods.json` carries
   `resume_turn`; `bindings/ResumeState.ts`, `ResumeKind.ts` written; `HistoryEntry.ts` has the
   `"interrupted"` arm; `AttentionKind.ts` has `"resume"`.
8. `bun run check` (`tsc -b --noEmit`) in `crates/desktop/ui` — clean (after `bun install
   --frozen-lockfile` here and in `packages/apps`, which this checkout lacked).
9. `bun run test` in `crates/desktop/ui` — `Test Files 145 passed (145)`, `Tests 838 passed (838)`.
10. rustfmt: `rustfmt --edition 2024 <file>` on the touched files (drain.rs, attention.rs,
    schedule store.rs/lib.rs, tui model.rs/ui.rs/lib.rs, main.rs, rpc api.rs/acp.rs,
    update lib.rs, agent lib.rs/session_read.rs); rustfmt also reformatted child modules and
    untouched regions of shared files (`supervisor.rs`, `loop_.rs`, `store/*.rs`,
    `schedule/src/{tools,validate}.rs`, `theuth/src/mcp.rs`) — those were reverted with
    `git checkout`; `supervisor.rs` kept only my 5 lines (already fmt-clean).

## Smoke (a real `t`, `cargo build -p theuth`, temp data dir, `drain_secs: 2`)

1. `t --headless` with nothing open, `kill -TERM`: log `exiting: SIGTERM` → `109: drain window
   closed reason="shutdown" running=0 finished=0` → `109: drained suspended=0`; exited in 0.22 s.
2. A session row and a `turn_steps(input_accepted)` row inserted by hand (W1's writes are not in
   the tree), engine restarted: `load_session` over the RPC answered
   `resume: { turn, kind: "interrupted", interrupted_at }`; `attention_list` empty (`resume: auto`,
   no report); `kill -TERM` → `turn_steps` now `1 input_accepted`, `2 suspended {"reason":"shutdown"}`;
   log `109: drained suspended=1 finished=0`.
3. Same store with `durability.resume: ask`: `attention_list` →
   `[{ id: "resume:<session>:<turn>", kind: "resume", title: "0199c000 was interrupted by a restart —
   resume?", detail: null, session, at_ms }]`; `resume_turn(session)` → `true` (the contract's
   `ResumeTurn` stub logged `not implemented`); `attention_dismiss(id)` → the list is empty.

Not smoked: the desktop and TUI surfaces live (no live engine resume exists yet to drive them);
their behaviour is covered by the reducer/model tests above.

## Files changed

`crates/agent/src/supervisor/drain.rs`, `crates/agent/src/{lib.rs, supervisor.rs, session_read.rs}`,
`crates/theuth/src/main.rs`, `crates/update/src/lib.rs`, `crates/rpc/src/{api.rs, acp.rs}`,
`crates/roster/src/attention.rs`, `crates/schedule/src/{lib.rs, store.rs}`,
`crates/tui/src/{model.rs, ui.rs, lib.rs}`,
`crates/desktop/ui/src/store/{sessions.ts, reduce.ts, reduce.test.ts}`,
`crates/desktop/ui/src/components/chat/{ResumeBanner.tsx (new), ResumeBanner.test.ts (new), Message.tsx, MessageInfo.tsx, MessageTool.tsx, SessionMenu.tsx}`,
`crates/desktop/ui/src/components/attention/{model.ts, model.test.ts, Screen.tsx, PlainScreens.tsx}`,
`crates/desktop/ui/src/components/hud/{Hud.tsx, queries.ts}`, `crates/desktop/ui/src/rpc/api.ts`,
`crates/desktop/ui/src/routes/c.$session.tsx`, `docs/design/109-as-built-W5.md` (this note).
