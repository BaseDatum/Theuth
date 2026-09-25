# 70 AGENTS-CHAT — theuth proposes hires in chat (as built)

Branch `feat/agents-chat-hiring`. Three feature commits + this document.

Sky's ruling (2026-09-25): "I want theuth to be able to propose hires and all that as well" — chat is the primary path; the Agents view / org chart observes. Same pattern as 69 TASKS' `task_draft`: OpenUI cards with `@ToAssistant` buttons, the press is the approval.

## Commit 1 — roster tools (`3c7484e`)

`feat(roster): agent_put tool (the user's chat approval writes the file; desks refused), agent_propose takes model/effort and checks reports_to, AgentOf message`

- `crates/roster/src/tools.rs`
  - **`agent_put` tool** (`AgentPutTool(Arc<Shared>)`, Mutating). Args: `AgentPutArgs { #[serde(flatten)] agent: AgentProposeArgs, paused: bool (default false) }`. Refuses when `Shared::agent_of(session)` names a desk other than `theuth` ("agent_put is the user's approval; as agent ‹d›'s desk, propose with agent_propose"). Writes through `roster::PutAgent` (create or replace; the roster validates via `file::parse(render)` and reconciles). Receipt: `hired | replaced agents/<name>.md (active | paused: …)`. `proposed_by` is never set (see Deviations).
  - `AgentProposeArgs` gains `model?`, `effort?` (were hard-coded `None`).
  - `fn file_of(args, default_reports_to, &Shared) -> Result<AgentFile, String>` shared by both tools: budget parse, `reports_to` trimmed / defaulted, and **`reports_to` must be `theuth` or a name on the roster** (refusal names `agents_list`). `agent_propose` now goes through it too (same semantics from theuth's own session: proposer defaults to `theuth`, no gating — there never was a desk-only gate; every session sees every registry tool, 69 §Tool sets).
- `crates/roster/src/lib.rs` — `AgentOf(SessionId)` message → `Option<String>` (`Shared::agent_of`): the bus form of "which agent is this session working for", for the app crate's draft tool.

## Commit 2 — `agent_draft` (`7b56627`)

`feat(app): agent_draft tool renders hire cards (Hire|Replace / Hire paused / Discard via @ToAssistant); name/reports_to/tools/budget validated up front; desk drafts route to agent_propose; MessageTool renders agent_draft like app`

- `crates/app/src/agent_draft.rs` (new) — `agent_draft { agents: [{name, title, reports_to, model?, effort?, tools?, mcp?, budget?, capabilities?, charter}] }` (`AgentDraftTool { core }`, ReadOnly).
  - Asks the roster `ListAgents` (existing names) and `AgentOf(session)` (desk or not), validates every draft before drawing anything (`validate`): `theuth` reserved; title and charter non-empty; `reports_to` ∈ {`theuth`} ∪ roster; `tools` ∈ read_only|mutating|destructive; budget parses; no name drafted twice; a desk may not draft an existing name (`agent_propose` refuses taken names). Name syntax `[a-z0-9-]{1,32}` is `roster::AgentName`'s serde `try_from`, so a bad name fails at argument parsing with the roster's own message.
  - `source(agents, existing, suffix, desk)`: `root = Stack([intro, c1…])`; per card `CardHeader("<title> · `<name>`", "agent draft <id>")`, charter preview (first 600 chars, `…` when cut), meta line (reports to · model · effort · tools · budget · mcp), optional "Hand off to it when:" line, and for an existing name a note "`x` is already on the roster — *Replace* overwrites agents/x.md with this draft." Buttons: **Hire** (or **Replace**), **Hire paused**, **Discard** → `@ToAssistant("agent draft <id>: hire | replace | hire paused | discard")`. Draft ids `a<n>-<last 4 alnum of call_id>`. The intro differs for a desk's set ("proposed by agent `d` — Hire / Hire paused file a `hire_agent` approval up its line").
  - Commits through the `app` tool's `commit_source` (app name `agent-drafts`, `Origin::Tool`) like `task_draft`, so the card renders inline and in the artifact panel.
  - `approval(choice, desk) -> Option<Approval { tool, paused }>` is the approval mapping (hire/replace → `agent_put` paused false; hire paused → `agent_put` paused true; any desk → `agent_propose`, paused true; discard → `None`); `then(desk)` turns it into the receipt's instruction so the model is told exactly which tool to call.
- `crates/app/src/lib.rs` — module, re-exports, registration next to `TaskDraftTool`. `crates/app/src/draft.rs` — `lit` is `pub(crate)` (shared). `crates/app/Cargo.toml` — `roster` dependency (cycle-free: nothing in roster's closure depends on `app`).
- `crates/desktop/ui/src/components/chat/MessageTool.tsx` — `agent_draft` results render as `ArtifactCard` (one condition). Presses reach chat through the existing `AppHost.onAction → assistantInput` path (`[app agent-drafts v<n> · <record_call_id> · action …]\nagent draft a1-…: hire`).

## Commit 3 — prompt steering (`79f60af`)

`feat(roster): hiring prompt block (draft with agent_draft, wait, agent_put per choice; desks agent_propose); desk manual: a missing role is a proposal, the user decides`

- `crates/roster/src/tools.rs` — `PROMPT` ("## Hiring"): describe-a-role / "hire" / "add an agent" → `agents_list`, `agent_draft`, wait; `agent_put` with `paused` per choice; never tell the user to write a file; desks `agent_propose`. Registered from `RosterPlugin::start` as `RegisterPrompt { plugin: "roster" }` (`crates/roster/src/lib.rs`), after the tools. Registered under `roster`, not `app`, so `loop_::prompt_blocks` keeps it for unattended (desk) sessions too.
- `crates/roster/src/desk.rs` — desk manual: "A role missing from the roster is a proposal, not a hire: `agents_list`, then `agent_propose` (…); the user decides."

## Tests written (not run — orchestrator's job)

- `crates/app/src/agent_draft.rs`
  - `card_buttons_carry_the_draft_id_and_choice_and_replace_when_the_name_exists` — ids `a1-ab12, a2-ab12`; root; every button's `@ToAssistant` message; **Replace** + `replace` + the note for an existing name, **Hire** + `hire` otherwise; card part lists (caps / note present only when set); header; JSON escaping of quotes and newlines in the charter; meta line; desk intro; charter cut at 600 chars; one statement per line.
  - `approval_maps_choice_and_proposer` — hire → `agent_put` paused false; replace → same; hire paused → `agent_put` paused true; desk proposer → `agent_propose` paused true for both; discard → none; the receipt text names the tools.
  - `validate_refuses_what_the_roster_would` — reserved `theuth`; unknown `reports_to`; blank title / charter; bad `tools`; bad budget; user may draft an existing name, a desk may not; `AgentName::new` refuses uppercase, empty, 33 chars.
- `crates/roster/src/tools.rs`
  - `file_of_checks_reports_to_and_keeps_the_fields` — fields carried (title trimmed, model, budget parsed); unknown manager refused; roster name / `theuth` accepted; blank → default; bad budget refused.
  - `hiring_block_reaches_theuth_and_the_desk_manual_points_at_propose` — `agent::frame::baseline` with the roster block contains `## Hiring`, `agent_draft`, `agent_put`, "Never tell the user to write or edit a file" for attended and unattended; the desk manual names `agent_propose` and not `agent_put`.

## Verification (run)

- `cargo check -p roster --tests` → exit 0, no warnings (commit 1).
- `cargo check -p app --tests` → exit 0, no warnings (commit 2).
- `cargo check -p roster --tests` → exit 0, no warnings (commit 3, `79f60af`).
- `bunx tsc --noEmit` not run: `crates/desktop/ui/node_modules` is absent in this worktree (no `bun install`; installing was outside the allowed commands). The change is one added `|| tool.name === 'agent_draft'` in an existing string-comparison chain.

## Deviations

- **`agent_put` is a new tool, not an existing one.** The brief says "the assistant calls `agent_put`"; in the tree `agent_put` was only an RPC (`crates/rpc/src/api.rs`, the Agents view's save). The tool wraps the same `roster::PutAgent` message.
- **`proposed_by: user` is not written.** In this tree `proposed_by.is_some()` means "not yet approved": `AgentStatus::Proposed`, excluded from routing and from `agents_list` (`crates/roster/src/lib.rs` `routable`/status). The card press *is* the approval, so `agent_put` writes `proposed_by: None`; "Hire paused" lands `paused: true` → status Paused, un-pause from the Agents view / `agent_pause`. Writing `user` would have made every chat hire unroutable until a second approval.
- **Desk-only gating of `agent_propose`**: there was none to remove (every session sees every registry tool); from theuth's own session the proposer defaults to `theuth` and the `hire_agent` task lands with the user, as before. `agent_put` is the gated one (desks refused) — a desk bypassing approval would be the security hole.
- **`agent_propose` gains `model`/`effort` and a `reports_to` existence check** (brief: "same approval semantics"); the roster's `PutAgent`/`ProposeAgent` handlers are unchanged, so the RPC/UI path keeps its behaviour.
- **`AgentOf` bus message** added to roster instead of reaching into `Shared` from the app crate (00 §1: plugins talk over the bus). `crates/app` now depends on `roster` (types + two `ask`s).
- **A desk drafting an existing name is refused** up front rather than shown a Replace button that `agent_propose` would reject.
- `crates/agent` untouched: the tools live in roster/app and prompt paragraphs are plugin-registered (the tasks precedent), so the "prompt in theuth's default prompt" test lives in roster and drives `agent::frame::baseline`.
- Draft ids are `a<n>-<suffix>` (task drafts are `d<n>-…`) so a chat with both kinds cannot confuse them.

## Not built / deferred

- No end-to-end run (engine + model + OpenUI helper); the card source and the approval mapping are unit-tested and go through the same validated `commit_source` path as `app`/`task_draft`.
- No bindings regeneration: no `ts`-exported type changed (`AgentProposeArgs`/`AgentPutArgs` are tool args, not RPC types).

DONE AGENTS-CHAT
