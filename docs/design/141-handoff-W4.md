# 141 hand-off — W4 (TUI + CHANNELS)

Branch `feat/141-w4`, cut from C `0b5f8aa2`; `feat/141-acp-subagents` merged
before the first gate and before DONE (no late C fixes arrived). Files touched:
`crates/tui/src/{model,ui}.rs`, `crates/slack/src/{board,channel}.rs`,
`crates/telegram/src/{channel,commands}.rs` — nothing outside W4's list.

## 1. TUI (§10.1 + Revision 1 D15 c)

### Model state
- `Model.observed: Option<ObservedMeta>` — copied from `Loaded.observed` in `load()`
  (the one flag; `Loaded.kind` alone is `chat` from an older engine), kept live by
  `ObservedChildChanged` for the shown child, cleared by `reset()`.
- `Model.watch_slash: bool` — see "read-only open".
- No new `AgentRow` field: the row's `AgentInfo.observed` (contract field) carries the meta,
  so `list_agents` snapshots and live events render identically.

### Roster match (`apply_agent`, before the transcript match)
`AgentEvent::ObservedChildChanged { parent, child, name, model, observed }`:
- shown child (`self.session == Some(child)`): `self.observed` updated; a **state** change
  interjects `Entry::Info("watching · <word>")`, a detail-only update is silent and returns
  before the tail-finishing rules (so a streaming child answer is not cut). The transcript
  match's `ObservedChildChanged` arm is therefore a documented no-op, not a stub.
- otherwise: upsert the `AgentRow` (`depth` = 1 under the shown session, else parent row
  depth + 1; unknown parent → dropped), `agent: None`, `kind: Observed`, `model` from the
  event, `AgentState` from §5.2 (`running → Running`, `requires_action → Approval{tool:
  detail.currentTool ?? "permission"}`, `failed → Error`, `cancelled → Cancelled`,
  `pending | idle | completed | unknown → Idle`), `fresh` for `failed | cancelled | unknown`
  so those linger on the board like a cancelled native child; `completed` leaves the board
  (Q1: `idle` is not done — it shows `idle`, no check). **No `↳ spawned` line.**
- `track()` unchanged: the child's own `TurnStarted/ToolCall*/TurnFinished` keep driving the row.

### Row strings (`agent_row`, `/agents` picker and board)
`◎ <name:<14> <word:<9> watch · <src>[ · <lastIntent>]  <age>` — e.g.
`◎ ReadA          running   watch · omp task · Reading first line`,
`◎   ↳ ReadA.b      running   watch · opencode` (depth 2),
`◎ ReadA          failed    watch · omp task`, `… lost contact watch · omp task`,
`… done      watch · omp task`. Words: `pending running "waiting on you" idle done failed
stopped "lost contact"` (`observed_word`). Source tags (`source_tag`): `omp task` / `opencode`
(both opencode sources) / `ACP`. `WATCH_GLYPH = "◎"` is `pub`.

### Read-only open
- `resume(id)` unchanged; `input_focused()` is `false` while `observed.is_some()` (unless
  `watch_slash`).
- Header: `◎ <name>  watching · <model>` where model = the child's `harness_model` when
  reported, else `Loaded.model` (the parent's harness model, per C's `upsert_observed`);
  e.g. `◎ ReadA  watching · omp-model  ·  ctx 0`.
- Status line segment: `  ·  watch-only — <harness> runs this subagent · Esc stops it` or
  `· cannot be stopped here`; `<harness>` = `omp` / `opencode` / `the harness` (RFD; the TUI
  has no harness name for an observed row — `Loaded.agent` is `None` by design).
- Typing: `ui.rs` swallows `Char/Enter/Backspace/Tab` (no Ctrl/Alt) and pushes the same hint
  **once per run** (`watch_swallow`: not repeated while it is the last entry). A plain message
  or `/<skill>` submitted on a watch-only view is refused with the hint (`submit_with` guard).
- **Deviation (recorded):** the design's "typing is swallowed" would trap the user — the TUI
  has no non-slash way to `/parent`, `/agents`, `/sessions`. So `/` on a watch-only view sets
  `watch_slash`, inserts `/` and focuses the editor for that one slash command; `sync_slash`
  drops the focus as soon as the line no longer starts with `/`. Slash commands run as usual.
- Esc (`cancel_at`) and `/cancel`: `observed.can_cancel && streaming → Command::Cancel(child)`
  (the engine routes it to `CancelObserved` → `Harness::cancel_session`); not cancellable → the
  hint; cancellable but idle → nothing. Triple-Esc `CancelTree` is unreachable on a watch-only
  view (returns before the tap counter) — on the parent it is unchanged.

### D15 c — harness commands in the slash popup
- `Model::slash_rows(word) -> Vec<SlashRow { name, hint, desc, raw }>`: theuth's `COMMANDS`
  (via `slash_matches`) first, then `harness_commands` as `/<name>` (a name already starting
  with `/` is kept), `hint` = `HarnessCommand.hint`, `desc` = `[<harness>] <description>`
  with `<harness>` = `Loaded.agent` (e.g. `[omp] skill:ponytail from omp`); both lists filtered
  by prefix. `ui.rs` popup and `slash_busy` use it.
- Enter on a `raw` row → `send_text(line)` (the extracted message path: `Command::Send`, or
  `Command::Steer` mid-turn), never `submit` — so `/skill:ponytail` is **not** parked as a
  theuth skill even when no skill index is loaded (`skill_word` would otherwise claim it).
  Tab completes as before. A typed line (not picked) still goes through `submit`, so a typed
  `/<name>` that is a theuth skill keeps resolving to the theuth skill (parity with D15 b).

### Tests (`cargo test -p tui`)
`board_tracks_observed_children_as_watch_rows`,
`watch_only_view_is_read_only_and_esc_routes_only_when_cancellable` (header + `input_focused`
gate + status hint + refusal + `/` escape hatch + state line + Esc routing),
`slash_popup_lists_harness_commands_labelled_and_sends_them_raw`.

## 2. Slack (§10.2)

- `route()`: `ObservedChildChanged { parent, child, name, observed, .. }` → `parent_of[child]
  = (parent, name)` (so the child's own events reach ancestor boards and `ancestors()` works),
  then `Out::Observed { child, name, depth, observed }` to every bound ancestor's worker. No
  `spawned()`: no child root post, no `Thread::new(child)`, no reply binding, no Cancel.
- `Worker::observed`: creates the `board::Row` on first sight (no link) and applies
  `Row::observed(&meta)`.
- `board::Row.watch: Option<&'static str>` (source tag); `Row::observed` maps `running →
  Running`, `requires_action → Approval`, `failed → Error (detail · <stop>)`, `cancelled →
  Cancelled`, `unknown → Error (· lost contact)`, `pending | idle | completed → Idle`; detail
  `watch · <src>[ · <stop | lastIntent>]`; `line()` prints `◎` instead of the state glyph.
  `Row::seed` applies `info.observed` so a restart rebuilds the tag.
- Board row strings: `◎ ReadA          running   watch · omp task · Reading first line    2s`,
  nested `  ◎ ReadA.b        running   watch · omp task`, failed
  `◎ ReadA          error     watch · omp task · rate limited`, lost
  `◎ ReadA          error     watch · omp task · lost contact`. Idle/done rows are not listed
  (`**agents** · all idle`), as for native children.
- `Thread::apply`: `ObservedChildChanged` stays in C's silent arm (no thread is ever bound to
  an observed id, so its `TextDelta`/tool events never reach a thread).
- Tests: `board::tests::observed_rows_are_watch_rows`,
  `channel::tests::observed_child_is_a_watch_row_on_the_board_and_gets_no_thread` (board post
  text with both rows, no thread/`child of`/`↳ spawned`/`|thread>`, the child's `Error` still
  alerts on the parent board only).

## 3. Telegram (§10.3)

- `handle_event`: `ObservedChildChanged` → `parent_of[child]`, `children[child]` created if
  new, `child_row(child, |c| c.observed(&meta))`. `spawned()` is not called: no topic, no
  `↳ spawned` marker, no `flat_children` entry, no thread (so `child_task` posts nothing).
- `ChildState.watch: bool`; `ChildState::observed` = the Slack mapping; `row()` glyph `◎`.
  Row strings: glyph `◎`, state `running | approval | error | cancelled | idle`, detail
  `watch · <src>[ · <stop | lastIntent>]`.
- `commands.rs` `cancel <name>`: a hit with `observed.is_some_and(|o| !o.can_cancel)` →
  `"<name> is a watch-only subagent of <parent name | short id>; stop the parent"`; with
  `can_cancel` → `api.cancel(child)` as any subagent.
- Tests: `channel::tests::observed_child_is_a_watch_row_without_a_topic_or_marker`,
  `commands::tests::cancel_refuses_a_watch_only_subagent_unless_cancellable`.

## 4. Assumptions about other waves

- W2: `AgentEvent::ObservedChildChanged` is published on create and on every state/title/cancel
  change (detail-only throttled); the child's transcript events arrive on the child id with
  `TurnStarted`/`TurnFinished` bracketing `running`/`requires_action` (§5.2). `Loaded.running`
  reflects the child's open turn (the TUI gates Esc-cancel on it). `Cancel(child)` on an
  observed id reaches `CancelObserved` (§7.7); for a non-cancellable child the TUI never sends it.
- C: `Loaded.observed` is `Some` iff the row is observed; `AgentInfo.observed` is filled by
  `list_agents` (the TUI picker and Slack `seed` rely on it).
- The TUI has no harness *name* for an observed row (`Loaded.agent` is `None`); the hint
  derives the word from `observed.source`. If the Closer wants the real `agents.yaml` key
  there, `Loaded` would need the parent's `agent` (not in the contract).

## 5. Gates

All through `BUILD_LABEL=141-W4 build-lock`, stable toolchain, `CARGO_BUILD_JOBS=4`,
after merging `feat/141-acp-subagents` at C's final `63281cc4` (the warm-target clone
never produced `.ready`; built from the partial clone after `cargo clean -p
sentencepiece-sys` cleared a CMakeCache pointing at Sky's target dir):

- `cargo test -p tui` — `149 passed; 0 failed` (lib) + 1 bin test.
- `cargo test -p slack` — `146 passed; 0 failed`.
- `cargo test -p telegram` — `124 passed; 0 failed`.


## 6. For the Closer

- `cargo fmt -p tui/slack/telegram` with the stable toolchain also reformatted untouched
  lines in the files W4 edited (the repo is not fmt-clean under this rustfmt: `cargo fmt
  --all --check` reports diffs across `crates/*`); `crates/tui/src/lib.rs` was restored to
  avoid pure-noise changes. Expect some formatting-only hunks in W4's six files.
- Smoke strings for `141-smoke.md` are in §1 "Row strings", §1 "Read-only open", §2, §3 above.
