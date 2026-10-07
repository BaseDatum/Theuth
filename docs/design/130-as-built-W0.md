# 130 — Companion — as built (W0, engine contract E1–E4, `feat/130-w0-engine`)

Base `eba311ed`. Contract: `130-companion.md` §5.3. Crates touched: `roster`, `agent`, `tasks`, `rpc`; UI:
`components/hud/queries.ts`, `components/hud/Hud.tsx` (`KIND_LABEL` only), `components/attention/{model,Row,Screen,
PlainScreens}.tsx`, `rpc/events.ts`. Bindings regenerated locally (gitignored, not committed). `App.tsx`, `store/*`,
`Scene*`, `routes/index.tsx`, `bridge.rs` lines owned by 129 W1 — untouched beyond list-end additions.

## What landed

### E1 `AttentionChanged`
- `crates/roster/src/attention.rs:120-137` `AttentionChanged { count, ids_added, ids_removed, at_ms }`,
  `#[serde(tag = "type")]`, ts-rs exported. `:1006` `changed(prev, items, at_ms) -> Option<AttentionChanged>` is the
  pure diff (sorted id lists, `None` when the id set is equal; `count` is covered by the id set).
- `:952 Attention::recheck` = gather → derive → `changed` against `last_ids` → `rt.publish`. Called from every handler:
  `ApprovalRequested`/`ApprovalResolved` (`:1233`, `:1319`), `ArtifactShown` (`:1332`), `AskRequested`/`AskResolved`
  (new subscriptions, `:1343`, `:1352`), `tasks::activity::ActivityAppended` (new subscription, `:1361` — task and
  agent changes reach the inbox through the feed), `ResumeReport` (`:1287`), `TurnFinished` with `end_turn`
  (`:973 finish_turn`), `SnoozeAttention`/`DismissAttention` (`:1386`, `:1402`), and `ListAttention` (`:1374`, so a
  snooze expiry noticed by a list call also announces).
- rpc: `bridge.rs:24,134,190` forward + subscribe; `export.rs:243`; `client.rs:290` republish (mirrored engines).
- UI: `rpc/events.ts:43,111-112` (end of `WireEvent`); `hud/queries.ts:28,46` — `'AttentionChanged'` appended to
  `ATTENTION_EVENTS`, `refetchInterval: 60_000` removed; the 13 older kinds stay.
- Tests: `attention.rs attention_changed_diffs_id_sets` (same ids → `None`; added id → `ids_added`; dismissed id →
  `ids_removed`); `rpc/src/api.rs:6127 attention_changed_wire_shape` (`{"type":"AttentionChanged", …}`, passes
  `wanted()` for global subscribers).

### E2 `waiting_since_ms`
- `attention.rs:107-109` `AttentionItem.waiting_since_ms: Option<u64>` (serde default; TS `?: number | null`).
  `derive` sets it to `at_ms` for every kind (`:294-295`), overrides asks with `Inputs.ask_since` (`:361`, the
  actor's first-seen time, `:884-889` and the `AskRequested` handler), `None` for Finished (`:657`).
- `crates/agent/src/lib.rs:944-947` `SessionActivity.waiting_since_ms` (serde default; TS `?: number | null`).
  `supervisor.rs:218` approvals carry their arrival ms, `:269` `Wait.since_ms`; `:5236-5246 assemble_activity`
  fills it while `state` is `approval` / `awaiting`, `None` otherwise (so it clears when the state leaves).
- `UI/rpc/extra.ts`: nothing to mirror by hand — `SessionActivity` there is imported from the generated binding
  (`extra.ts:13`), which carries the field.
- Tests: `attention.rs waiting_since_for_approvals_and_asks`; `supervisor.rs:10951
  activity_waiting_since_follows_the_pending_approval` (set, serialised as a number, cleared, pre-130 row parses).

### E3 `Finished`
- `attention.rs:66-68` `AttentionKind::Finished` (serde `finished`); `:110-117` `summary: Option<String>`,
  `next_steps: Vec<String>` (TS `summary?: string | null`, `next_steps?: Array<string>`); `:140-151 Finished` row;
  `:153-158` caps `SUMMARY_MAX = 160`, `NEXT_STEP_MAX = 80`, `NEXT_STEPS_MAX = 2`.
- Store: `crates/roster/migrations/0006_attention_finished.sql` (`session` PRIMARY KEY, `turn`, `at_ms`, `summary`,
  `next_steps` JSON); `attention.rs:1036 record_finished` (upsert per session — the newer turn replaces),
  `:1054 finished()` (LEFT JOIN `attention_state` on `'finished:' || session || ':' || turn`, dismissed rows out).
- Production: the actor keeps `texts: session → assistant text since the last tool call` from `TurnStarted` /
  `ToolCallStarted` (reset) and `TextDelta` (append) (`:1276-1299`); on `TurnFinished { stop_reason == "end_turn" }`
  `:973 finish_turn` skips when the session has a pending approval (`self.pending`) or ask (`self.ask_since`), asks
  `agent::Ancestors { session }` and skips any chain longer than one (a subagent — the parent reports), parses, upserts,
  rechecks. Item: id `finished:<session>:<turn>`, title `<who> finished`, `detail = summary` (so the HUD row, which
  renders `detail`, shows it without a change), `waiting_since_ms: None` (`:651-667`).
- Parser `:685 parse_finished(text, fallback)`: last line equal to `## next` (case-insensitive) splits the text; items
  are `- `, `* ` or `N. ` lines, first two, each cut to 80 chars with `…`; summary is the first non-empty line of the
  part before (leading `#` stripped) cut to 160, else the fallback (the session title), else `None`.
- Prompt: `crates/roster/src/desk.rs:115-116` — one paragraph at the end of the desk manual asking for a trailing
  `## Next` list of ≤ 2 one-line replies under 80 chars (the journal excluded). No other prompt changed.
- UI: `attention/model.ts:22` group `done` / "Done" (end of `GROUPS`); `hud/Hud.tsx:183` `finished: 'Finished'`;
  `attention/Row.tsx:23-24` one truncated `summary` line; `attention/Screen.tsx:31` + `PlainScreens.tsx:182
  FinishedScreen` (the `Plain` frame: summary as title, the `Next` list, age — needed because `SCREEN` is an
  exhaustive `Record<kind, …>` and tsc refuses a missing key; no new route).
- Tests: `attention.rs next_section_parses_two_items_none_and_long`, `finished_rows_replace_per_session_and_dismiss_hides`
  (one item per session, the newer turn wins, `summary`/`next_steps`/`waiting_since_ms: None`, dismissed stays out);
  `attention/model.test.ts` Done group; `api.rs attention_changed_wire_shape` also pins `"finished"`.

### E4 `actor = user@<client>`
- Choice: **header**, not query parameter. `crates/rpc/src/lib.rs:58-89 Client(&'static str)`: `HEADER =
  "x-theuth-client"`, `KNOWN = desktop | tui | ios | companion` (case-insensitive), `DEFAULT = desktop` for absent or
  unknown values. `Auth::call` (`:311-323`) parses it on the upgrade and inserts it into the request extensions beside
  the `RpcPeer`; jsonrpsee copies the upgrade's extensions into every call on the socket, so it is per connection.
  `Client::actor(ext) -> "user@<client>"`.
- Handlers (`#[method(…, with_extensions)]`, wire signatures unchanged): `api.rs:3377 resolve_approval` and `:3402
  answer_ask` record a `tasks::activity::Record` when the prompt was still pending — kinds
  `ActivityKind::ApprovalResolved` / `AskAnswered` (new, end of the enum, `tasks/src/activity.rs:71-74`), subject the
  id, summary `allowed|denied approval <id>` / `answered|dismissed question <id>`; `:5472 agent_approve` passes the
  actor into `roster::ApproveAgent { name, actor }` (`roster/src/lib.rs:79-84`, was a tuple struct; one caller);
  `:5511 task_move` and `:5586 task_decide` pass it as the ledger actor, so the tailed `task_events` rows and the
  activity entries derived from them carry it. `send_input`, `attention_dismiss`, `attention_snooze` write no entry
  today and got nothing.
- Ledger: `tasks/src/ledger.rs:225 is_user()` — `user` or `user@…` is the user for `acts_for`, the back-off reset in
  `update`/`comment`, and the unblock rule in `move_to` (4 sites). Task-thread lines now read `**user@desktop**` for
  user actions from the desktop (was `**user**`).
- Tests: `rpc/src/lib.rs:444 client_header_stamps_the_connection` — `companion` → `user@companion` on two calls of
  one socket, `TUI` → `user@tui`, unknown and absent → `user@desktop`, empty `Extensions` → `user@desktop`;
  `ledger.rs a_client_stamped_user_acts_for_anyone`.

## Gates (all wrapped in `build-lock`, crate-level)
| Gate | Result |
|---|---|
| `cargo fmt -p roster -p rpc -p tasks -p agent` | clean on touched files; it also reformats the untouched `crates/roster/src/pin.rs` (main is not fmt-clean there) — reverted, not committed |
| `cargo clippy -p roster -p rpc -p tasks -p agent -- -D warnings` | ok |
| `cargo test -p roster` | 64 passed |
| `cargo test -p rpc` | 43 + 1 passed |
| `cargo test -p tasks` | 90 passed |
| `cargo test -p agent --lib` | 429 passed, 1 ignored (pre-existing `crash_child`), 38 s |
| `cargo test -p rpc --features ts --lib export_bindings` + copy | ok; `AttentionChanged.ts`, `AttentionItem.ts`, `SessionActivity.ts`, `AttentionKind.ts` carry the shapes |
| `bunx tsc --noEmit -p crates/desktop/ui` | ok |
| `bunx vitest run src/components/attention src/components/hud src/rpc` | 7 files, 31 tests passed |

## Deviations from §5.3 (and the brief)
- **E1 trigger set.** `AgentEvent` rechecks only on the variants the actor acts on (`ResumeReport`, `TurnFinished`
  end_turn); every `TextDelta`/`ToolCall*` is not a derive. Asks were added as a trigger (`AskRequested`/`AskResolved`
  subscriptions) — §5.3 lists approvals/shows/agent events; the companion needs asks too.
- **E2 decisions/reviews** use `at_ms` (the task's `updated_ms`, or the review packet's `at_ms`), not a `task_events`
  lookup: `gather` only reads events for checkout-less tasks, so a uniform "blocker time" is not cheaply available.
- **E3 title-model fallback is not implemented** (no model call this wave): when the final text has no line, `summary`
  is the session title from `Ancestors`, else `null`. The "final-answer prompt" §5.3 mentions was not changed — only the
  desk manual, per the brief.
- **E3 root check** is `Ancestors` chain length 1 over live sessions. A subagent whose actor was already unloaded when
  its `TurnFinished` is handled would look like a root `[INFERENCE: not observed]`.
- **E4 TUI** calls go through `client::Local` with no `Client` in the extensions → `user@desktop`, not `user@tui`
  (the design's `tui`). One line in `Local::new` (`methods.extensions_mut().insert(Client("tui"))`) would set it, but
  `Local` also serves the ACP relay; left for W1/owner to decide.
- **E4 ActivityKind names**: `ApprovalResolved` shares its name with the `tools::ApprovalResolved` bus event; they live
  in different enums/namespaces.
- **`just companion-fixtures`** (design §7 row W0) does not exist in the justfile and was not in the brief; not added.
- `KIND_LABEL.finished = 'Finished'` as the brief spells it; the other labels are lowercase (CSS uppercases either).
- `AttentionItem.detail` of a Finished item duplicates `summary` so `Hud.tsx` rows show it unchanged.

## Tests the brief named that are not unit-level here
- "subagent `TurnFinished` produces none" and "derive produces exactly one Finished item per session and replaces it on
  the next turn" — the second is `finished_rows_replace_per_session_and_dismiss_hides` (store + derive); the first is the
  `chain.len() != 1` guard in `finish_turn`, which needs a live `Agents` actor and is not covered by a unit test.
- "an upgrade with the header yields `user@companion` on a `resolve_approval` activity entry" — covered in two halves:
  the header → `Client::actor(ext)` chain (`client_header_stamps_the_connection`) and the handler calling it
  (`api.rs:3377`); no test spins the approval actor + activity actor end to end.

## Open items for W1/W2
- Title-model summary when a turn has no text (needs a model call); the final-answer prompt for root sessions (not the
  desk manual) does not ask for `## Next` yet — root (non-desk) turns produce Finished items with `next_steps: []`
  unless the model writes the section on its own.
- `recheck` runs one full `gather` per `ActivityAppended`; a burst of task events is a burst of derives (ponytail: no
  debounce; add a 250 ms coalesce in the actor if it shows in traces).
- `user@tui` stamping for in-process callers (see above); `ios` is never set by the link client yet.
- A companion wanting the old 13-kind refetch path still works; after E1 it can listen to `AttentionChanged` alone.
