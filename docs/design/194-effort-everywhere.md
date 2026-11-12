# 194 EFFORT-EVERYWHERE — every model picker is the full standard picker; every model setting carries its effort

Status: contract, written by the orchestrator on 2026-11-12 and frozen for the 194 workers. Base: origin/main 1784059b.
Builds on 124 (model picker), 127 (favorites, touch), 148 (model defaults) and the 2026-11-11 chips change (ca0079e1, "effort levels as chips under the selected model; every model picker renders the standard ModelPicker").

## 1. Ask

Sky, 2026-11-12 16:49: "how come in certain model pickers I don't have the effort chips showing up for the selected model. for example in settings models subagent model, I can select a model with efforts, but I don't have any option to choose the effort level?"

Sky, 16:58: "fix it. ALL model pickers should be full featured and behave the same."

## 2. Cause (verified)

- `ModelPicker` (chat/ModelPicker.tsx) draws the effort chip row under the selected model only when its caller passes `effort={{levels, current, onSelect}}`. `ModelCombobox` (chat/ModelCombobox.tsx) passes it straight through.
- There are 19 `<ModelCombobox>` call sites. Only 2 pass `effort`: Settings › Models › New sessions (`settings/ModelDefaults.tsx` ~L33) and the agent runtime model (`agents/RuntimeAccessFields.tsx` ~L42). The other 17 edit settings that store a model id and nothing else, so there was nothing to bind chips to. The 11-11 change was briefed "no engine or RPC changes".
- Half-built effort settings already exist in the engine:
  - `agent.subagents.effort` (`SubagentsConfig.effort`, crates/agent/src/lib.rs ~L4121) is honoured at spawn (supervisor.rs ~L1029: explicit pick → `agent.subagents.effort` → `agent.effort`). It is read from config.yaml at startup only, with no RPC and no UI.
  - `advisor.effort` (`AdvisorDefaults.effort`): Settings › Advisor shows it as an effort alone, with no model picker. `advisor.model` exists, but there is no setter.
  - Autoresearch has an effort setting with no UI.

## 3. Decisions

**D1 One picker, everywhere.** Every place a model is chosen uses the standard picker with the same features: desktop, web, phone, and the TUI as far as a terminal allows.
- Features: providers → models, word search, Favorites, the Current row, the effort chip row under the selected model, keyboard, the touch dialog, the `name · effort` trigger and the Local tag.
- The chip row shows a Default chip first, unless the setting requires a level.
- Call sites differ only in two ways:
  - their `none` row label, which names the fallback ("Same as new sessions", "Engine default", "Off", "Default");
  - no Default chip where the setting has no default (ACP harness thinking levels, as today).

**D2 Every model setting stores its effort beside the model, through the model's own mechanism.**
- **Config keys:** the effort key is the model key with `model` replaced by `effort`:
  - `agent.subagents.model` → `agent.subagents.effort` (exists);
  - `agent.title_model` → `agent.title_effort`;
  - `roster.theuth_model` → `roster.theuth_effort`;
  - `advisor.model` → `advisor.effort` (exists).
- **Settings structs** (endless, voice, meeting, dreaming, autoresearch …): a sibling `*_effort: Option<String>` field next to the model field, with `#[serde(default)]`. Name it the model field with `model` → `effort`.
- **Front-matter files:**
  - routines: `effort:` beside `model:`;
  - agent files: `memory_effort:` beside `memory_model:` (`effort:` already exists).
- **Per-session values:** the session compactor effort sits beside the session compactor model.
- **Client-remembered picks** (the meeting-settings store): an effort beside the model, sent wherever the model is sent.
- **Old files:** files and configs without the key keep working. Absent means Default.

**D3 RPC shape.**
- Never change an existing method's parameters or return type: older clients and mesh peers call them.
- **Dedicated model pair** (`X_model()` / `set_X_model(model)`): add `X_effort() -> Option<String>` and `set_X_effort(effort: Option<String>) -> Option<String>`. The setter returns the stored value after validation.
- **Model inside a settings struct** with one get/set pair: the effort is a new field of the same struct. Add no new method.
- **Value already carried** by `Info` or another reply (`default_effort`, `AdvisorDefaults.effort`): reuse it.
- **Advisor:** add `set_advisor_default_model(model: Option<String>) -> AdvisorDefaults` beside `set_advisor_default_effort`.
- **Placement:**
  - New methods go in the api module of their model sibling, immediately after it, in both the trait and the impl.
  - In `crates/desktop/ui/src/rpc/api.ts`, each declaration goes immediately after its sibling's. In api.ts's method-name list, insert at the alphabetical position.
  - New query keys go next to the sibling's key.

**D4 Semantics: the same for every setting, enforced in the engine.**
- **a. `None` is the Default chip, and keeps today's behaviour for that setting.**
  - Subagents and the theuth agent inherit the New sessions effort (the spawn chain already does this).
  - Routine runs and meeting sessions are new sessions, so they inherit `agent.effort` as now.
  - Side calls send no reasoning setting.
  - Nothing changes for a user until they pick a level.
- **b. Validation.** A setter refuses an effort the setting's model does not take, with `ModelInfo::validate_effort`'s message.
  - It checks the model the setting resolves to (its effective model).
  - When nothing resolves (for example, "each session's own model"), it checks only that the word is a known effort (`effort_of`).
- **c. Model change.** Changing a setting's model drops a stored effort that the new model does not take, and persists `None`. This mirrors `set_model` in a session (148 R1 F6). The next read shows the effort gone.
- **d. The `none` row.** Choosing it ("Same as new sessions", "Engine default", "Off") clears the stored effort too. `none` means "use the fallback", effort included.
- **e. Applied where the model is used.** That covers every case:
  - the spawn, the desk and its work sessions, routine runs and meeting sessions;
  - every side call: title, vision fallback, escalation tier-2, polish, compactor, summaries, dreaming tiers and autoresearch roles.
  - Each owner adds a test at the call site proving that the request or the new session carries the effort. An effort that is stored but never used fails this contract.
- **f. Live.** A pick takes effect on the next use, without restarting theuth.
  - `agent.subagents.effort` becomes live, like `agent.subagents.model` (RwLock or the same mechanism).
  - If something truly cannot be live, its row says "Applies after theuth restarts" (Sky's settings rules). None is expected.
- **g. Persistence.** Persist with the writer the model already uses: `runtime::write_config_value`, the struct's own save, or the front-matter writer.
- **h. System prompt.** The subagent line names the effort when one is set. For example: "Subagent default model: X, effort Y (the user's setting; spawn_subagent uses them when you omit `model`/`effort` - pass them only when the user asks for a specific one)". Without an effort, the line stays as it is today.

**D5 UI.**
- **Every picker passes `effort`.** Every `<ModelCombobox>` and `<ModelPicker>` passes `effort`, built as ModelDefaults.tsx ~L33 does for New sessions:
  - `levels` are the selected model's `efforts`;
  - `current` is the stored effort;
  - `onSelect` calls the effort setter.
  - It is `undefined` only when the selected model has no efforts.
  - The query invalidation after a model change must also refetch the effort (D4c).
- **Trigger.** It reads `name · effort` everywhere; ModelCombobox already does this when `effort.current` is set.
- **`localFirst` is removed.** Every picker uses one ordering (Favorites, Current, providers), and local models keep their Local tag. B2 drops it from its two call sites (settings/Memory.tsx, chat/MemoryPanel.tsx); Z removes the prop from ModelCombobox and ModelPicker and from their tests.
- **Settings › Advisor.** The effort-only "Default effort" row becomes a ModelCombobox over `advisor.model`, with `advisor.effort` as its chips. One home per setting.
- **Guard test (Z).** A vitest source scan fails when any non-test `<ModelCombobox` or `<ModelPicker` in `crates/desktop/ui/src` lacks `effort=`. Exempt only the wrappers that forward `effort` (ModelCombobox's own `<ModelPicker>`).

**D6 TUI.** `/model` and every other TUI model picker gets the chip row under the selected model, with the same semantics as desktop:
- Default first unless a level is required, and the active chip marked.
- A key to move between chips that does not collide with the existing keys (⏎ set, ⇥ ★ favorite, esc, ← back).
- This replaces any separate effort list or step. `/effort` keeps working.
- Favorites, search and Current exist already (127 §3): verify parity, fix any gap.

**D7 Tools follow the files.**
- `schedule()` and `routine_draft()` gain `effort` (front matter `effort:`).
- `agent_put`, `agent_draft` and `agent_propose` gain `memory_effort`.
- Autoresearch per-role or per-leaf model fields gain matching effort fields where the draft card and tool carry models.
- Each tool description names the new field in one clause. Do not reword anything else.

**D8 Docs.** This file is the contract.
- Each worker writes `docs/design/194-as-built-<X>.md`. It records the inventory rows it built (with the final key, RPC and field names), every decision where this contract is silent, and the tests.
- Z amends 124 (a short "Effort everywhere" section) and 148 (subagent and theuth agent effort).

## 4. Inventory and owners

| # | Picker | File (≈line) | Model → effort storage | Applied at | Owner |
|---|---|---|---|---|---|
| 1 | Settings › Models › New sessions | settings/ModelDefaults.tsx L28 | `agent.model` → `agent.effort` (exists) | new sessions | done |
| 2 | Agents › runtime model | agents/RuntimeAccessFields.tsx L42 | agent file `model:` → `effort:` (exists) | agent sessions | done |
| 3 | Settings › Models › Subagents | settings/ModelDefaults.tsx L43 | `agent.subagents.model` → `agent.subagents.effort` (exists; make live + RPC) | spawn chain | A |
| 4 | Settings › Models › theuth agent | settings/ModelDefaults.tsx L54 | `roster.theuth_model` → `roster.theuth_effort` | theuth desk + its work sessions | A |
| 5 | Agents › theuth agent model | agents/AgentDetail.tsx L378 | same setting as 4 | same | A |
| 6 | Settings › Advisor › default | settings/Advisor.tsx | `advisor.model` → `advisor.effort` (both exist; add model setter) | `/advisor on`, advisor default | A |
| 7 | Settings › Helpers › Title | settings/Helpers.tsx L45 | `agent.title_model` → `agent.title_effort` | title side call | B1 |
| 8 | Settings › Helpers › Vision fallback | settings/Helpers.tsx L66 | `agent.vision_model` → `agent.vision_effort` | vision fallback call | B1 |
| 9 | Settings › Helpers › Escalation | settings/Helpers.tsx L89 | `agent.escalate_model` → `agent.escalate_effort` | tier-2 calls | B1 |
| 10 | Settings › Voice › Polish | settings/Voice.tsx L194 | polish model → polish effort | polish call | B1 |
| 11 | Settings › Memory › Compactor | settings/Memory.tsx L193 | endless compactor model → effort (compactor.rs already has an effort field) | compactor | B2 |
| 12 | Chat › Memory panel › Summary model | chat/MemoryPanel.tsx L199 | session compactor model → effort | that session's compactor | B2 |
| 13 | Agents › Memory summary model | agents/RuntimeAccessFields.tsx L54 | agent file `memory_model:` → `memory_effort:` | its endless desk compactor | B2 |
| 14 | Settings › Meetings › Summary model | settings/Meetings.tsx L92 | meeting summary model → effort | summary call | B2 |
| 15 | Settings › Dreaming › tier models | settings/Dreaming.tsx L185 | dreaming tier model → effort | dreaming tier calls | B2 |
| 16 | Settings › Meetings › Meeting model | settings/Meetings.tsx L81 | meeting session model → effort | meeting session | C |
| 17 | Meeting start › Session model | meeting/StartForm.tsx L125 | meeting-settings model → effort, sent in `meeting_start` | meeting session | C |
| 18 | Routines › Model | routines/RoutineDetail.tsx L667 | front matter `model:` → `effort:` | routine run session | C |
| 19 | Settings › Autoresearch › role models | settings/Autoresearch.tsx L129 | role model → role effort (an effort setting exists) | role calls | C |
| 20 | Autoresearch draft › leaf models | autoresearch/DraftPanel.tsx L219 | draft leaf model → effort | study runs | C |
| 21 | Chat header, compact header, notebook column, session ⋯, advisor pill, ACP harness picker | chat/ModelMenu.tsx, Header.tsx, SessionMenu.tsx, AdvisorPill.tsx, notebooks/ChatColumn.tsx | session model/effort (exist) | — | Z audits D1 parity |
| 22 | TUI `/model` and any other TUI model picker | crates/tui | session model/effort (exist) | — | T |

All paths are under `crates/desktop/ui/src/components/` unless they start with `crates/`. Rows 16 and 17 may share one setting; C decides from the code and records the decision.

## 5. Shared files (edit only inside your own hunks)

- `crates/rpc/src/api/settings.rs`: A owns the subagent and advisor blocks. B1 owns the title, polish, vision and escalate blocks. B2 owns the endless block.
- `crates/rpc/src/api/boards.rs`: A (theuth agent).
- `crates/rpc/src/api/hub.rs`: B1 (voice).
- `crates/rpc/src/api/meetings.rs`, `dreams.rs`, `research.rs`: B2 (summary, dreaming), C (start, autoresearch).
- `crates/desktop/ui/src/rpc/api.ts` and the query keys: everyone, placed next to the sibling (D3).
- `crates/agent/src/lib.rs`: A (SubagentsConfig, the Set/GetSubagent* and Advisor* messages), B1 (the Title/Vision/Escalate messages), B2 (endless types).
- `crates/agent/src/supervisor.rs`: A (spawn resolution, subagent handlers, the system-prompt line), B1 (the title, vision and escalate handlers and their side calls). Edit only inside those functions. Put new functions next to their siblings.
- `crates/roster`: A (theuth_model → theuth_effort), B2 (AgentFile memory_effort and the memory params of the agent tools).
- `crates/app/src/agent_draft.rs`: B2. `crates/app/src/routine_draft.rs`: C.
- `crates/meeting`: B2 (summary), C (session start).
- `settings/Meetings.tsx`: B2 (the Summary row), C (the Meeting model row).
- `chat/ModelCombobox.tsx`, `chat/ModelPicker.tsx` and their tests: Z only. No worker edits them. A worker that needs a change there says so in its DONE.
- `docs/design/194-as-built-<X>.md`: one file per worker.

## 6. Wire-refactor integration

The fold keeps the owner-crate split: title/polish/vision/escalation/subagent effort
getters and setters, `advisor_on`, and `set_advisor_default_model` live in
`crates/agent/src/rpc_settings.rs`; `set_session_compactor_effort` lives in
`crates/agent/src/rpc_turns.rs`. The theuth-agent effort pair lives in
`crates/roster/src/rpc.rs`, and the disabled-plugin `autoresearch.settings.set`
path lives in `crates/autoresearch/src/rpc.rs`. Each uses its owner's described
trait and module; `rpc` retains only its `rpc-core` and `runtime` workspace edges.

The disabled-autoresearch regression is in `crates/autoresearch/tests/unloaded.rs`.
Unique method names are checked by the real production composition in
`crates/theuth/src/main.rs::wire_contract_tests`, including owner assertions for
194's methods, rather than by scanning the old monolithic RPC sources.
Migration `0063_endless_efforts.sql` and `SESSIONS_WIRE_SCHEMA = 63` are unchanged.

The TUI keeps model pickers central and other panels component-owned. Effort
chips use the search component's existing terminal-cell sizing helpers.
The integration gate compiles tests for all affected crates; it does not run tests.
