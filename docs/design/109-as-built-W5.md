# 109 as-built — W5 (graceful drain + what the user sees)

Worktree `theuth-109-w5`, branch `feat/109-w5` off the contract commit 8a5f8b75. Built 2026-10-06.
Scope: §9 drain and its wiring, §10.3 RPC/surfaces, §8.2–8.4 desktop, TUI divider/status note,
schedule `OpenTurn` gating (§4.6 step 12). W1's journal writes and `ResumeTurn` were not in the
tree: everything here is driven by the contract's store API and events and tested against rows
written through it.

## 1. Drain — `crates/agent/src/supervisor/drain.rs`

- `Drain { deadline, reason }` (`:33`) → `DelegatedReply<DrainReport>` (`:66-136`). The handler
  snapshots the live session actors and leaves the mailbox (a turn ending inside the window asks
  `Agents`; waiting in the handler would deadlock). Off the mailbox, in order: every open turn
  (`store.open_turns`) gets `turn_steps(suspended, {"reason"})` through `Store::step` **first**
  (durable even if the desktop's 2 s `EXIT_GRACE` SIGKILLs us mid-window; `turn_finished` closes
  the row of a turn that makes it), then `wait_idle` (`:151`) polls each actor's `IsRunning` every
  100 ms until none runs or the deadline, then every turn still open gets `TurnSuspended { reason }`,
  then `Pool::close()` on the Monty pool (idle workers exit cleanly; a checked-out one goes with its
  turn and the previous dump stands, 08 §5), then the report
  (`suspended` = open turns at the end, `finished` = running actors that went idle in the window).
- `DRAINING` (`:25`): the first drain wins; a second `Drain` in the same process is a zero report.
  `draining()` is exported for anyone who needs the flag.
- `OpenTurns` (`:52`, ask → every open turn): one ask for the schedule plugin and the attention
  actor instead of one `OpenTurn` per row.
- `drain_engine(rt, reason)` (`:173`): looks up `Agents` (none → empty report at once), reads
  `durability.drain_secs` from `rt.section("durability")` (default 10), asks `Drain`, abandons it
  one second past the deadline.
- `Agents::resume_state(session)` (`:206`): `Loaded.resume` + the `Interrupted` history row from
  `open_turns` + `resume_plan`.
- Registration line `supervisor.rs:111` (`// 109 W5`).

Wired into every path that ends the engine (§9.3), always **before** `rt.shutdown.cancel()` — every
turn's cancel token is `rt.shutdown.child_token()` (`loop_.rs` `start_turn`), so a drain after the
token would find every turn already cancelled:

| path | where |
|---|---|
| SIGINT, SIGTERM (new), `parent_gone`, token already cancelled by a plugin/actor | `crates/theuth/src/main.rs:1510-1520` — one `select!` yields `(origin, reason)`, then `drain_engine`, then `cancel()`, then the existing 100 ms / `release()` / `exit(0)`; `sigterm()` at `:1551` |
| rpc `shutdown` (desktop quit, `/quit`) | `crates/rpc/src/api.rs:4576-4583` — spawned `drain_engine(…, "shutdown")` then `cancel()`; the RPC reply is immediate as before. `main.rs`'s drain after the token is then the no-op |
| `t update` relaunch | `crates/update/src/lib.rs:660-667` — `drain_engine(…, "update")`, `cancel()`, 100 ms, `exit(EXIT_RESTART)` (exit code kept: launchd `SuccessfulExit=false` / systemd `on-failure` restart on it) |

Jobs: `KillAll` still runs on the token (`tools/jobs.rs`, W2), which now fires after the window.

## 2. RPC / types (§10.3)

- `agent::HistoryEntry::Interrupted { text, at, context: None }` (`crates/agent/src/lib.rs:1664`);
  `context()`/`context_mut()` arms; `agent::ResumeState { turn, kind, interrupted_at }` (`:841`);
  `Loaded.resume: Option<ResumeState>` (`:834`, `serde(default)`, `ts(optional)`). Both ride the
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
- `ResumeNeed` (`:125`) is the `Inputs.resumes` row (`:197`); `derive` renders it at `:558-576`:
  title `"<who> resumed after a restart with gaps"` when a `detail` exists, `"<who> was interrupted
  by a restart — resume?"` without one (`resume: ask`), `detail` = what was skipped.
- `resume_need(event, now_ms)` (`:947`): a `ResumeReport` with non-empty `interrupted` or
  `children_handed_off > 0` raises one; a clean report raises nothing (the banner is enough).
  The actor subscribes to `AgentEvent` (`:644`) and keeps the latest need per session (`reports`,
  `:619`); `attention_state` hides a dismissed id as for every kind.
- `resume: ask`: the actor reads `durability.resume` at start (`:627`) and, when `ask`, lists every
  open turn from `OpenTurns` as an item with no `detail` (`:748-754`) — the Resume button's item;
  it disappears when the turn closes.

## 4. Schedule (§4.6 step 12) — `crates/schedule`

- `store.rs`: `Store::open` keeps its contract (orphans → `lost`) and is now
  `open_sparing(path, &HashSet::new())`; `open_sparing` (`:68`) returns `(Store, Vec<OpenRun>)`,
  closing every orphan whose session is **not** in `spare` and returning the spared rows.
- `lib.rs:2566-2585`: the plugin asks `Agents` `OpenTurns` once (one ask, not one `OpenTurn` per
  row) and opens the store sparing those sessions; `Schedule::reattach` (`:960`) puts each spared
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

- `Entry::Divider(String)` (`model.rs:1033`, height 3, rendered in the `Compacted` shape at
  `ui.rs:2007`). `TurnResumed` interjects `── resumed after restart ──` (`── resumed after update ──`
  for `Suspended`) and sets `Model.resume_note` (`model.rs:3205`), appended to the status line
  (`:6777-6782`), cleared by `TurnFinished` (`:5826`); `ResumeReport` rewrites the note with the
  counts and, when something was skipped or lost, adds one `Info` line; `TurnSuspended` interjects
  `── suspended for <reason> ──`. `HistoryEntry::Interrupted` replays as the
  `── interrupted — resuming ──` divider plus the greyed text (`:9686`); print mode prints the same
  (`lib.rs:1295`, events at `:1384-1398`).
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
- `crates/update/src/lib.rs:660-667`: the drain before the relaunch exit (see deviations).
- `crates/rpc/src/acp.rs:599-601`: the new variant in `replay`'s exhaustive match.

## Not done

- No gate on new input during the drain (see deviations).
- `Loaded.resume.report` (see deviations).
- `attention` items do not clear "when the user opens the session" — the inbox has no open hook
  (nothing else clears on open either); Dismiss, or the turn closing (`ask` items), clears them.
- Desktop component tests are pure-function tests (`vitest` here runs `src/**/*.test.ts` only, no DOM
  library): the banner wording, the reducer's banner/partial/lost-job state, the group mapping.
- `desktop/src/launch.rs` `EXIT_GRACE` (2 s) is shorter than `drain_secs` (10 s): a desktop quit
  SIGKILLs a long drain; the `suspended` row is already written by then. Not my file; flagged.

## Gates

(filled in below from the actual runs)
