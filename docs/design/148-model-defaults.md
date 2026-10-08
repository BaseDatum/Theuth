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
  sends it from `open_desk`, on a hired agent's `model:` change (`reconcile`)
  and on `SetTheuthModel`.
- **E3 theuth model** — `crates/roster/src/config.rs` `RosterConfig::theuth_model`
  (live copy in `Roster.cfg`; persisted via `write_config_value("roster",
  "theuth_model")`, cleared with an empty value that reads back `None`).
  `file::default_agent(data_dir, model)` fills `model:`. Roster messages
  `TheuthModel` / `SetTheuthModel`; RPC `theuth_agent_model` /
  `set_theuth_agent_model` (`crates/rpc/src/api.rs`). On set/clear the open
  desk is switched session-only to the effective model. Desks without a
  model follow `AgentEvent::DefaultModelChanged`; a reopened desk is switched
  to its effective model in `open_desk`. `desk.rs manual()`: `None` renders
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
  row. Draft picks (`ModelMenu.tsx`, `slash.ts`) go to `useDraftPick`
  (`lib/drafts.ts`) and ride on `create_session` in `Composer.tsx`;
  `NewChatPane.tsx` resets them. `rpc/api.ts` + `lib/rpc.ts` add the RPCs and
  query key.

## Gates

GATES_PLACEHOLDER

## Deviations

- E2 plumbing: a separate `agent::SetDeskModel` message instead of extending
  `MarkDesk` (no struct-literal churn; model changes need no re-mark).
- E3 credential check: `SetTheuthModel` resolves through `agent::SetModel
  { session: None, manual: false }` then requires the provider among
  `agent::ListModels` (keyed catalog providers) — the agent exposes no
  resolve-with-credentials ask. Same error text as `set_subagent_model`.
- E3 `open_desk`: a reopened desk is switched to its effective model
  unconditionally rather than compared with the stored row (no cheap
  per-session model query; result identical).
- E3 config write failures are best-effort (warn, live value still changes),
  mirroring `SetSubagentModel`.
- E5: a typed `/model <id>` in the TUI draft is validated at `create_session`
  on the first send, not when typed; a draft model pick keeps a previously
  picked draft effort (the chained effort picker lets the user re-pick).
- E6: draft `/effort clear` means "the engine default effort" (`create_session`
  cannot express the provider default); one global draft pick shared by split
  draft panes (`ponytail:` in `lib/drafts.ts`).
- E6 tests: ModelCombobox effort passthrough and Settings Chat rows are
  asserted via static render (trigger label / read RPCs), the existing
  settings test pattern; setter wiring is one `useAction` per row.
- `crates/theuth/src/mcp.rs` picked up an unrelated rustfmt change during the
  roster slice; reverted.

## Follow-ups

- TUI: no way to set the default model/effort (Settings UI owns it).
- Subagent default effort row in Settings › Chat (`agent.subagents.effort` is
  config.yaml-only today).
- `agent::ResolveModel` ask with the credential check, so roster-side
  validation stops reading `ListModels`.
- `Agents::desk_models` entries are never dropped when a desk session is
  deleted (bounded by the roster size).
