# 148 — MODEL-DEFAULTS (as built, 2026-10-08)

Branch `fix/148-model-defaults`, base `8cd9c323`.

## Sky's words

"In the settings chat in desktop/mobile I need to be able to set the default
model used for new sessions and subagents separately, and those should be
respected, but not take away an agents ability to choose them for subagents
it spawns when a user requests it. I need to be able to set a model to use
for the theuth built in agent. these selectors should all use the model
picker we've built and use throughout the app."

Rulings (binding):

- **R1.** A pick inside a chat applies to that chat only. New chats start on
  the Settings › Chat default. A pick on the new-chat draft applies to that
  one chat. Replaces 82 §2 / 83 §2 ("the last manual pick becomes the default").
- **R2.** The subagent default applies to theuth (native) subagents only;
  external harness children keep their own model unless the agent names one.
- **R3.** The theuth agent's model covers its desk and the desk's direct work
  children; subagents below those use the subagent default. Unset = "Same as
  new sessions".

## Effective model

| Session | Model |
|---|---|
| New chat (no pick) | `agent.model` (Settings › Chat › New sessions) |
| In-chat pick | that session only; default untouched |
| Draft pick (new-chat screen) | `create_session { model, effort }` for that chat; default untouched |
| theuth desk | `roster.theuth_model`, else `agent.model`; follows `DefaultModelChanged` while unset |
| Hired agent's desk | file `model:`, else `agent.model` (follows the default while unset) |
| Native child of a desk | spawn `model` → desk agent's model → `agent.subagents.model` → `agent.model` |
| Other native child | spawn `model` → `agent.subagents.model` → `agent.model` |
| External child (`agent:`) | the harness's own model unless the spawn names one (unchanged) |

Effort: in-chat / draft picks are session-only (R1); children use spawn
`effort` → `agent.subagents.effort` → `agent.effort`. No desk-effort fallback.

## What changed

- **E1 engine semantics** — `crates/agent/src/supervisor.rs` `Agents::set_model`
  / `set_effort`: the default (`default_model`/`default_effort`, config.yaml,
  `DefaultModelChanged`/`DefaultEffortChanged`) moves only when
  `session.is_none() && manual`. A session pick switches that session only.
  Docs: `crates/agent/src/lib.rs` (`SetModel`, `SetEffort`),
  `crates/rpc/src/api.rs` (`set_model`, `set_effort`). Slack/Signal/Telegram
  `/model` already pass `manual: cx.session.is_none()`; verified unchanged.
- **E2 desk children** — `agent::SetDeskModel { session, model }` (lib.rs),
  kept in `Agents::desk_models`; `session()` resolves a native child of a
  desk as spawn pick → desk model → subagent model → default. The roster
  sends it from `open_desk`, on a hired agent's `model:` change (PutAgent
  applies it right after the write; `reconcile`'s diff only catches on-disk
  edits seen by the watcher) and on `SetTheuthModel`. The entry leaves with a
  put-away or deleted desk.
- **E3 theuth model** — `crates/roster/src/config.rs` `RosterConfig::theuth_model`
  (live copy in `Roster.cfg`; persisted via `write_config_value("roster",
  "theuth_model")`, cleared with an empty value that reads back `None`).
  `file::default_agent(data_dir, model)` fills `model:`. Roster messages
  `TheuthModel` / `SetTheuthModel`; RPC `theuth_agent_model` /
  `set_theuth_agent_model` (`crates/rpc/src/api.rs`). On set/clear the open
  desk is switched session-only to the effective model. Desks without a
  model follow `AgentEvent::DefaultModelChanged`. `switch_session` asks
  `agent::SessionModel` first and skips `SetModel` when the row already stores
  the effective (canonical) id, so a reopen at boot spawns no actor and
  publishes nothing. `desk.rs manual()`: `None` renders
  "model: omit it (the user's subagent default applies)". PutAgent still
  refuses theuth.
- **E4 prompts** — `crates/agent/src/frame.rs` GUIDANCE_HEAD and the header
  line ("Subagent default model: …"); `crates/agent/src/peers.rs` spawn
  description and `model` param doc; `crates/theuth/src/main.rs` sample
  config; `site/content/docs/automation/delegation.mdx`;
  `docs/design/01-config-and-subagents.md` note.
- **E5 TUI** — `crates/tui/src/model.rs`, `lib.rs`: `Command::SetModel`/
  `SetEffort` require a session; with no session the pick lands in
  `Model.draft: Option<CreateSessionOptions>`, shown in the header and used
  by `open_draft()` → `create_session` on the first send / skill / talk.
  The TUI's default moves only on `DefaultModelChanged`/`DefaultEffortChanged`.
- **E6 desktop/mobile** — `ModelCombobox` gains `effort` passthrough and a
  "name · effort" trigger. Settings › Chat (`settings/Chat.tsx`) opens with
  **Models**: New sessions (`set_model(null, id, true)`, effort via
  `set_effort(null, level, true)`), Subagents (`subagent_model` /
  `set_subagent_model`, none = "Same as new sessions"), theuth agent
  (`theuth_agent_model` / `set_theuth_agent_model`, invalidates
  `queryKeys.theuthAgentModel` + roster). `settings/Loop.tsx` loses the
  Subagent model row. `agents/AgentDetail.tsx` theuth tab shows the same Model
  row. Draft picks (`ModelMenu.tsx`, `slash.ts`) live on the `''` draft in
  `useDrafts` (`lib/drafts.ts`, same persisted blob as the text); every
  draft-born session opens through `openDraftSession` (`create_session` with
  the pick, else `new_session()`, pick spent) — the composer's send, Talk,
  `/<skill>` and a parked skill. `rpc/api.ts` + `lib/rpc.ts` add the RPCs
  and query key.

## Gates

All via build-lock + mbx (CARGO_BUILD_JOBS=4), re-run after the origin/main
fold (`1d1579aa`, 78 commits: 144 NOTEBOOKS, 147, 135b; additive in our seams).

- `mbx test -p agent`: 563 passed, 2 ignored (first run had 2 load-induced
  timing flakes, `busy_retries_stop_at_the_deadline` and
  `held_message_follows_the_brief`, green on rerun and post-fold).
- `mbx test -p roster`: 70 passed. `-p rpc`: 55 + 1 (prompts). `-p tui`: 156 + 1 (no_emoji).
- `mbx clippy --all-targets -- -D warnings`: agent, roster, rpc, tui clean
  (tui needed `Box<Command>` in two local `Act` enums after `Command` grew,
  and a struct literal in a test that clippy flagged).
- `mbx check -p theuth` (pulls nodes, slack, signal, telegram, schedule, meeting): clean.
- `cargo fmt --all -- --check`: 0 diffs in agent/roster/rpc/tui; 218
  pre-existing diffs in untouched crates with rustfmt 1.9.0-stable (left alone).
- Bindings: `mbx test -p rpc --features ts --lib export_bindings` 8 passed;
  730 files copied, `methods.json` has `theuth_agent_model` / `set_theuth_agent_model`.
- UI: `bun run check` 0 errors; `vitest run src/components/{settings,chat,agents} src/store src/lib`
  156 files / 1183 tests passed; `bun run build` OK.
- No live-engine smoke (rule); the UI surface is proven by static-render tests only.

R1 (no fold): agent 563 + 5 + 1 passed, 2 ignored (same two load-timing
flakes, green on rerun); roster 73; tui 157 + 1; clippy agent/roster/tui
clean; `check -p theuth` clean; fmt 0 diffs in touched crates (218
pre-existing elsewhere); no ts-rs type changed (bindings untouched);
tsc 0; vitest 156 files / 1185 tests.

**Final (R3, folded on `d5cb3e05` = verify/main: 140 AUTORESEARCH + 146
RESTART-CLARITY):** agent 566 + 1 + 5 passed, 2 ignored (no flakes this run);
roster 74; rpc 56 + 1; tui 157 + 1; clippy agent/roster/rpc/tui clean;
`check -p theuth` clean; `cargo fmt --all -- --check` 0 diffs repo-wide;
bindings export 8 passed, 796 files; tsc 0; FULL vitest 219 files / 1490
tests (one 5 s timeout flake in `src/link/pairing.test.ts`, relay round
trips under load, green on rerun; unrelated to 148); `bun run build` OK.

## Deviations

- E2 plumbing: a separate `agent::SetDeskModel` message instead of extending
  `MarkDesk` (no struct-literal churn; model changes need no re-mark).
- E3 credential check: `SetTheuthModel` resolves through `agent::SetModel
  { session: None, manual: false }` then requires the provider among
  `agent::ListModels` (keyed catalog providers) — the agent exposes no
  resolve-with-credentials ask. Same error text as `set_subagent_model`.
- E3 F7: a stored model that no longer resolves — `TheuthModel` answers a
  bare `ModelInfo` naming the id (provider = prefix before `/`) so the row
  shows the stale value; `open_desk` / `apply_desk_model` warn (`agent`,
  `model`) and let the live default stand in; a desk never stays unopened.
  Such a desk is not in the `DefaultModelChanged` follow set until its file
  is fixed (`f.model` is `Some`).
- E3 config write failures are best-effort (warn, live value still changes),
  mirroring `SetSubagentModel`.
- E5: a typed `/model <id>` in the TUI draft is validated at `create_session`
  on the first send, not when typed; a draft model pick keeps a previously
  picked draft effort (the chained effort picker lets the user re-pick).
- E6: draft `/effort clear` means "the engine default effort" (`create_session`
  cannot express the provider default); one pick on the `''` draft, shared by
  split draft panes like the text itself.
- E6 tests: ModelCombobox effort passthrough and Settings Chat rows are
  asserted via static render (trigger label / read RPCs), the existing
  settings test pattern; setter wiring is one `useAction` per row.
- `crates/theuth/src/mcp.rs` picked up an unrelated rustfmt change during the
  roster slice; reverted.
- F8 roster↔engine tests needed a live `Agents`: `agent` gained a
  `test-support` feature exposing `spawn_test_agents(rt)` (the in-crate rig,
  every provider configured, registered as `AGENT_ACTOR`); roster's dev-dep
  enables it. The feature never reaches a production build.
- F8 desktop: Composer's submit is not statically testable (no DOM runner);
  its no-session branch is `openDraftSession`, whose tests are the
  submit-decision tests. "NewChat mount resets a stale pick" is a `useEffect`
  and has no test (effects do not run under `renderToStaticMarkup`).

## R1 review round (2026-10-09)

- F1 PutAgent reads the old model before writing; on change → `apply_desk_model`.
- F2 site docs on R1: `getting-started/{models,first-session,index}.mdx`,
  `reference/{cli,configuration}.mdx`, `workspace/desktop.mdx` (Chat › Models).
- F3 `openDraftSession(api)` (`lib/drafts.ts`): pick → `create_session`, else
  `new_session`, then `pick.reset()`; Composer, Talk, `draftSkill`, `/<skill>`.
- F4 `switch_session` compares `agent::SessionModel` (new ask) with the
  canonical effective id; `open_desk` resolves the configured id up front.
- F5 `desk_models` dropped on `MarkDesk { agent: None }` / `DeleteSession`.
- F6 `session()` drops an inherited default effort the chosen model rejects.
- F7 stale theuth model: see Deviations.
- F8 tests — agent: `desk_models_leave_with_the_desk`,
  `an_inherited_default_effort_the_model_rejects_is_dropped`; roster:
  `desks_follow_configured_and_default_models_on_a_live_engine` (one
  `spawn_test_agents` rig test: reopen compare, SetTheuthModel on an open
  desk, DefaultModelChanged follow/leave, PutAgent re-point, F7 fallback),
  `theuth_model_unresolved_shows_as_itself`,
  `put_agent_model_change_is_in_the_read_model_before_reconcile`; tui:
  `draft_picks_ride_on_talk_mcp_and_skill_entries`; desktop: `lib/drafts.test.ts`.
- F9/F10 nits: `harness.md`, "subagent default" wording, bot comments cite
  148 R1, AgentDetail "Same as new sessions", Chat.tsx effort-pending disable.

## R2 live-proof round (2026-10-09)

Z smoke on a scratch engine: 14/14 (R1/R2/R3 behaviours, restart
persistence, TUI draft, desktop screenshots; `~/Develop/theuth-148-z-smoke.md`).
Two UI tickets:

- **T1** draft pick lost on a route round-trip (Settings remounts NewChat,
  which reset `useDraftPick`). The pick now lives in `useDrafts` next to the
  `''` draft text (`pick`, `setPick`, `resetPick`; same `composer-drafts`
  persistence); `useDraftPick` and the reset-on-mount are gone; only
  `openDraftSession` spends it. Tests: `drafts.test.ts` (pick survives
  `load()` = remount/reload, legacy blob → no pick; `openDraftSession` spends
  it in storage), `slash.test.ts` updated.
- **T2** Settings pickers trigger-anchored on phones. `ModelCombobox` opens
  `ModelPicker` in ModelMenu's top-of-band dialog (`dialogPlacement`,
  `--visible-band`) when `touch()` is coarse (`pickerSurface(touch)`), the
  popover otherwise — every ModelCombobox user. Test: `ModelCombobox.test.ts`
  `pickerSurface` coarse → dialog, fine → popover.
- Deviation: no "draft discarded" reset site exists (the `''` draft has no
  discard UI); the consumer path is the only reset.

## R3 — phone polish and the verified fold (2026-10-09)

- Settings `Row` gains `stack`: below `md` the label + description come
  first and the control stretches underneath (`max-md:flex-col
  max-md:items-stretch max-md:gap-3`; combobox `max-md:w-full
  max-md:max-w-none`). Used on every ModelCombobox row (Chat ×3, Agent loop
  helper models ×3, Memory, Dreaming, Autoresearch); the two hand-rolled
  `WideRow` copies in Dreaming/Autoresearch are gone. Test: `shared.test.ts`.
- Fold of `d5cb3e05` (verify/main): one conflict, the agent test rig —
  kept `bare_agents` and added 146's `boot`/`last_boot` fields. `cargo fmt`
  on agent/roster/rpc/tui. Full gates above.
- Smoke binary: `mbx build -p theuth` → copied to
  `~/.theuth/briefs/2026-10-08/148/smoke/t2`, `INTEGRATED2` = `sha=<HEAD>`.
- Fold of main `3649b4f0` (134/149: `#[cfg(unix)] process_group` in
  autoresearch, a smoke script line): no conflicts, nothing in our crates;
  `cargo fmt --all -- --check` 0, `check -p theuth` clean; `t2` rebuilt.

## Follow-ups

- TUI: no way to set the default model/effort (Settings UI owns it).
- Subagent default effort row in Settings › Chat (`agent.subagents.effort` is
  config.yaml-only today).
- `agent::ResolveModel` ask with the credential check, so roster-side
  validation stops reading `ListModels`.
- A desk whose configured model no longer resolves follows the default only
  after a restart or a model change, not on `DefaultModelChanged` (F7 gap).
