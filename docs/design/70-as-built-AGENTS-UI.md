# 70 — as built: AGENTS-UI (paperclip-style agent management in the desktop app)

Branch `feat/agents-ui-paperclip`. Sky's ruling (2026-09-25): copy paperclip's information architecture and flow for
managing agents — org chart, onboarding, hiring — in theuth's stack (React + TanStack Router/Query + Tailwind +
shadcn-style components), on theuth's model (67 §6: an agent is `agents/<name>.md`; `theuth` is the built-in root).

Second ruling (same day, mid-build): an agent is simply **Active or Paused** — one switch on the card and in the detail
header (`agent_pause`). What it is doing right now (working / idle / blocked / unreachable) is an **activity chip**
beneath, not a peer state. **Proposed** appears only in the Hiring inbox. The wizard ends in **Hire** (active) and
**Hire paused**.

## 196: isolated memory (desktop phase 1)

- Hire › Runtime and the agent's Runtime & Access expose **Isolated memory**, saved by `agent_put`; Review includes the choice. An `isolated_memory:` refusal appears verbatim beneath the switch and reverts it.
- Isolated agents have a neutral **Isolated memory** badge in list rows and the header, plus a **Memory** tab. Other agents never mount that tab, including a direct Memory URL.
- Memory lists, searches and previews `knowledge_*` RPCs with `bundle: "agent:<name>"`. Its cache keys include the agent, separate from main memory. The existing knowledge renderer is read-only; internal links stay in this bundle and missing links cannot create notes. No history, editing, sharing or main-memory navigation is exposed here.
- The footer says the notes are local to this machine. Empty and RPC-error states are inline. At phone widths the list sits above the preview.
- Engine bindings declare `isolated_memory` and the optional bundle selector. `src/lib/agent-memory.ts` uses generated agent types directly and calls the connected API's methods with explicit agent bundles.
- Fixture: `/src/dev/agent-memory-fixture.html?page=agents|memory|runtime|hire&state=notes|empty|error|refuse&theme=light|dark`. `AgentMemory.test.ts` covers toggles, refusals, badges, bundle reads and cache separation. Main Knowledge behavior is unchanged.

## Screens

| Route | Component | Paperclip counterpart |
|---|---|---|
| `/agents` | `components/agents/AgentsView.tsx` + `OrgChart.tsx` + `HiringInbox.tsx` | `pages/Agents.tsx` (list/org toggle, New Agent) + `pages/OrgChart.tsx` + `ApprovalCard`/`HireAgentPayload` (pending `hire_agent` approvals) |
| `/agents/new` | `HireWizard.tsx` over `wizard.ts` (pure state/validation) | `AgentBasicsDialog` + `NewAgentSetup` (+ `OnboardingWizard`'s stepper and localStorage resume) |
| `/agents/$name` | route redirect → `/agents/$name/overview` (the Queue's link keeps working) | `agents/:agentId` |
| `/agents/$name/$tab` | `AgentDetail.tsx` — Overview · Charter · Runtime & Access · Runs · Tasks | `pages/AgentDetail.tsx` + `AgentOverview` (Identity / Harness-Runtime / Capabilities / Recent Tasks cards) + `BudgetPolicyCard` + `AgentToolsTab` |

### `/agents`
- **Org chart**: `tree()` (by `reports_to`, rooted at `theuth`, unknown managers and cycles fall to the root) →
  `layout()` (paperclip's subtree-width algorithm: parent centred over children, siblings side by side) → cards on
  absolute coordinates over an SVG elbow-edge layer, one translate/scale transform for pan (drag) and zoom (⌘/ctrl +
  wheel, buttons, fit). Card: initials avatar on a hue hashed from the name (paperclip hashes the id into a palette),
  title, `name`, **Active/Paused switch**, activity chip, open tasks, budget bar. `theuth` shows `built-in`.
- **List** (`Table`): the same tree indented; columns Agent · Active · Activity · Reports to · Open · Budget. Phones
  always get the list. The choice persists in `localStorage["theuth.agents-view"]`.
- **Hire an agent** → `/agents/new`.
- **Hiring inbox** (`proposed()` = `status === 'proposed' || proposed_by !== null`): one row per proposed agent with
  proposer, manager, budget; **Approve** → `agent_approve`; **Decline** → dialog with an optional note → `agent_remove`
  (the note lands on the hire task as `rejected: <note>`). Proposed agents are excluded from the chart and list.

### `/agents/new`
Steps (`wizard.ts` `STEPS`): Identity (title → suggested `name`, `[a-z0-9-]{1,32}`, not `theuth`, not taken) → Role
(manager from the roster, capabilities) → Runtime (model from `list_models`, effort from the model's `efforts`, tools
ceiling) → Access (MCP allowlist from `list_mcp`: all servers (`mcp: null`), none (`[]`), or per-server checkboxes plus
optional `server.tool` narrowing) → Budget (none / monthly amount + currency; paperclip's policy wording: soft alert at 80 %, hard stop
pauses) → Charter (CodeMirror markdown; a starter template is injected on first arrival) → Review → **Hire** /
**Hire paused** = `agent_put(toFile(draft, paused))`, `proposed_by: null`, then the new agent's overview.
Validation per step (`validate`), forward jumps stop at the first incomplete step (`firstInvalid`), Back always allowed.
Resume in place: `{step, draft}` in `localStorage["theuth.hire-draft"]`, cleared on hire or "Start over".

### `/agents/$name/$tab`
- Header: avatar, title, `name`, `built-in` for theuth, **Active/Paused switch**, activity chip, manager, invalid-file
  error; actions **Open desk** (the desk session), **Remove** (AlertDialog → `agent_remove`; hidden for theuth); for a
  proposed agent instead: `proposed by …` badge, **Approve hire**, **Decline**.
- **Overview**: Identity (title, manager link, direct reports, capabilities) · Runtime & Access (model, effort, tools,
  MCP, desk; `Configure` → runtime tab) · Work (open-task count, six most recent open tasks, `See all`) · **Budget**
  (paperclip `BudgetPolicyCard`: pill Healthy / Warning / Hard stop / Paused by budget / No cap; Observed · Budget ·
  Remaining, bar, paused-by-budget wording; Set / Change / Remove cap via `agent_put`).
- **Charter**: CodeMirror over the body, Save changes / Discard; theuth's is read-only with the `prompt.md` note.
- **Runtime & Access**: the wizard's Runtime + Access fields on the file, Save / Discard; theuth: a read-only sentence.
- **Runs**: `list_sessions` filtered to the desks (`title === "agent:<name>"` or the summary's `desk`) and their
  children, newest first, running dot, model, age; opens `/c/$session`.
- **Tasks**: `tasks_list({owner: "agent:<name>"})` as read-only columns of `TaskCard`, with `Open on the board`
  (`/tasks?agent=agent:<name>&project=`).
- **Activity**: **omitted** — no RPC exposes an agent's journal/event feed (`task_events` is per task only).

## Rust (disclosed, small)
- `crates/roster/src/lib.rs`: `RemoveAgent { name, note: Option<String> }` → deletes `agents/<name>.md` (`Err("no such
  agent")`), drops the `proposals` row, `Reconcile` (closes the desk, drops the `agents` row), and — for a proposal —
  comments `rejected[: note]` on the hire task and cancels it. `theuth` refused. The approve handler's task-closing
  code moved into the shared `close_hire_task`. Test `remove_deletes_file_and_proposal_and_refuses_theuth`.
- `crates/rpc/src/api.rs`: `agent_remove(name, note)`; `crates/desktop/ui/src/rpc/api.ts`: `Api.agent_remove` + `METHODS`.
- Bindings: `just ts-desktop` regenerated (`methods.json` has `agent_remove`). Both `bindings/` dirs are gitignored
  (`/bindings`, `crates/desktop/.gitignore`), so nothing to commit there.

## Deviations from the brief
- **No harness on an agent.** The brief's Runtime step asked for "harness from Settings › Harnesses or native"; 67
  decision 1 says an agent file "never names a machine or a harness" — theuth places each task (`where`, `harness`)
  at delegation (decision 3). The Runtime step shows the configured harnesses and links to Settings › Harnesses, and
  says so. `AgentFile` has no harness field, so nothing was invented.
- **Status model** per Sky's second ruling (Active/Paused switch + activity chip; proposed only in the inbox) instead of
  the brief's flat six-state chip.
- **`agent_rename`**: not built (heavy: rename the file, rewrite every `reports_to`, the desk title, task owners; nothing
  in paperclip's flow needs it — paperclip's name edit is a display-name change; ours is the file stem).
- **Icon picker**: paperclip's `AgentIconPicker` (lucide icon names) has no field on `AgentFile`; the avatar is
  initials on a name-derived hue. Adding `icon:` to the file schema is a roster change outside this brief.
- **Org chart cards** carry status, open tasks and a budget bar (the brief asked); paperclip's show adapter and
  capabilities. Capabilities live on the detail Overview.
- **Runs** derive from sessions (desk + children); `SessionSummary.owner` is the *engine*, not the agent, and no RPC
  answers "sessions of agent X". Work sessions opened by another node under a task checkout appear on the task page.
- **Activity tab** omitted (no RPC).
- The first UI commit's `routeTree.gen.ts` references route files landing in the next two commits; the branch tip
  typechecks, intermediate commits do not.

## Needs an RPC that does not exist
- Agent activity / journal feed (`roster/journal/<name>.md`, episodes) for an Activity tab.
- Sessions by agent (`Shared::agent_of(session)` exists in Rust, not over RPC).
- `agent_rename`.

## Tests
- `components/agents/wizard.test.ts` — per-step validation (name regex, theuth, taken names, manager on roster, self,
  budget amount/currency, empty charter), review aggregation, `firstInvalid`, `toFile` (trim/null/paused), `suggestName`,
  starter charter.
- `components/agents/tree.test.ts` — existing tree tests (theuth on the roster is the root) + `layout` (centring,
  sibling spacing, edges, height).
- `components/agents/queries.test.ts` — inbox rows (`proposed`), hiring actions map to `agent_approve` /
  `agent_remove(name, trimmed note | null)`, `managerNames`.
- `crates/roster` — `remove_deletes_file_and_proposal_and_refuses_theuth`.
- `rpc/api.test.ts` — `METHODS` vs regenerated `methods.json` (passes with `agent_remove`).

## Verification run
- `cargo check -p roster -p rpc`: clean. `cargo test -p roster -p rpc --lib`: 30 + 30 passed.
- `just ts-desktop`: ok. `bun install --frozen-lockfile` (ui and `packages/apps`): ok.
- `bunx vite build`: ok (regenerated `routeTree.gen.ts`). `bunx tsc -b --noEmit`: exit 0.
- `bunx vitest run`: 67 files / 402 tests passed (before the layout test; agents suite re-run: 4 files, 16 tests).
- Smoke in a browser against `bunx vite` + the running engine with the 67 mock roster (`localStorage.theuth_mock67`):
  org chart, list, inbox + decline dialog, detail Overview/Charter/Runtime & Access/Tasks, proposed-agent detail, wizard
  identity + budget validation, starter charter, resume after reload, Review with Hire / Hire paused. Hire itself and
  Remove were not clicked (the only running engine is Sky's installed app, which predates the roster RPCs).
