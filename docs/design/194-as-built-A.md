# 194 as-built — worker A (rows 3–6, D4h)

Branch `feat/194-a`, base 1ded15b6. No S1 merge (not needed).

## Rows built

### Row 3 — Settings › Models › Subagents
- **Config key:** `agent.subagents.effort` (existing `SubagentsConfig.effort`), now the boot value of a live setting.
- **Engine:** `Agents.subagent_effort: Arc<RwLock<Option<String>>>` (supervisor.rs), initialised from config, shared with every native session (`Session::with_subagent_effort`, for the prompt line).
  - Spawn chain (unchanged order, live value): explicit pick → desk agent's effort (new, rows 4–5) → `subagent_effort` → `default_effort`; the 148 F6 filter still drops an inherited effort the child's model does not take.
  - Messages (crates/agent/src/lib.rs, after `GetSubagentModel`): `SetSubagentEffort { effort }` → `Result<Option<String>, String>`, `GetSubagentEffort` → `Option<String>`.
  - D4b: checked against the effective subagent model (`agent.subagents.model`, else `agent.model`) by `Agents::check_effort` (`Model::validate_effort`'s message; no catalog model → `effort_of` word check, "unknown effort X").
  - D4g: persisted with `runtime::write_nested_config_value(agent.subagents.effort)`, the writer `SetSubagentModel` uses; quoted (`"8000"` stays a string), `~` clears.
  - D4c/D4d in `SetSubagentModel`: `None` clears the effort; a model that does not take it drops it (persisted `~`).
- **RPC** (crates/rpc/src/api/settings.rs, after the model pair): `subagent_effort() -> Option<String>`, `set_subagent_effort(effort) -> Option<String>`.
- **UI:** settings/ModelDefaults.tsx Subagents row passes `effort` (levels of the selected subagent model, `current` = `subagent_effort()`, `onSelect` → `set_subagent_effort`); the model setter also invalidates `queryKeys.subagentEffort`. `/subagent` (chat/slash.ts) invalidates it too.

### Rows 4–5 — theuth agent (Settings › Models › theuth agent, Agents › theuth)
- **Config key:** `roster.theuth_effort` (`RosterConfig.theuth_effort`, struct-level `#[serde(default)]`).
- **Roster:** messages `TheuthEffort` → `Option<String>`, `SetTheuthEffort { effort }` → `Result<Option<String>, String>`.
  - D4b via the new agent message `ValidateEffort { model: Option<String>, effort }` → `Result<(), String>` against theuth's effective model (`roster.theuth_model`, else `GetDefaultModel`).
  - D4g: `runtime::write_config_value(roster.theuth_effort)` (the theuth_model writer), quoted / `~`.
  - D4c/D4d in `SetTheuthModel`: `None` clears it; a model that does not take it (`ValidateEffort`) drops it.
  - The synthesised theuth `AgentFile` carries `effort` (`Roster::theuth_file`), so `ListAgents`/`GetAgent` show it.
- **Applied (D4e):** `agent::SetDeskModel` gained `effort: Option<String>` (`#[serde(default)]`). The roster sends the file's `effort:` with every `SetDeskModel` (desk open and model/effort change), so the theuth desk and the desk's native work children start on it.
  - Agents keeps `desk_efforts: HashMap<SessionId, String>` (removed with the desk like `desk_models`), first fallback for a desk child's effort.
  - The desk row itself is put on the effort at once (live through `apply_effort` when its actor runs, else on the row it loads).
- **RPC** (crates/rpc/src/api/boards.rs, after the model pair): `theuth_agent_effort() -> Option<String>`, `set_theuth_agent_effort(effort) -> Option<String>`.
- **UI:** ModelDefaults.tsx theuth row and agents/AgentDetail.tsx `TheuthRuntime` pass `effort`; both setters invalidate `queryKeys.theuthAgentEffort` and `queryKeys.roster`; the model setters also invalidate the effort key.

### Row 6 — Settings › Advisor › default
- **Engine:** `SetAdvisorDefaults` gained `model: Option<Option<String>>` (`#[serde(default)]`). Resolved with `resolve_keyed` (unknown/un-keyed refused), persisted with `write_advisor_key("model", …)` (the manual pick's writer; `~` clears), set on `advisor_cfg` — what `GetAdvisorDefaults` / `Info.advisor` carry — and pushed to the `Advisors` actor (`SetDefaults`).
  - D4c/D4d: `None` clears `advisor.effort`; a model that does not take it drops it.
  - D4b: `set_advisor_default_effort` (signature unchanged) validates against `advisor.model`; with no model only the word.
  - Everything is validated before the first write.
- **RPC:** `set_advisor_default_model(model) -> AdvisorDefaults` beside `set_advisor_default_effort`.
- **UI:** settings/Advisor.tsx: the effort-only "Default effort" select is replaced by one `ModelCombobox` row "Default advisor model" over `advisor.model`, chips = `advisor.effort`, `none={{ label: 'None' }}`. Both setters re-read `info()` (it carries model and effort). Other rows untouched.

### D4h — system-prompt line (crates/agent/src/frame.rs)
- `baseline` / `baseline_dated` take `subagent_effort: Option<&str>` after `subagent_model`; the loop reads the shared slot, `ShowPrompt` the supervisor's.
- Model + effort: "Subagent default model: X, effort Y (the user's setting; spawn_subagent uses them when you omit `model`/`effort` - pass them only when the user asks for a specific one)".
- Model only: byte-for-byte today's line.

## Decisions where the contract was silent
- **Effort set, no subagent model:** the prompt gets its own line, "Subagent default effort: Y (the user's setting; spawn_subagent uses it when you omit `effort` - pass `effort` only when the user asks for a specific one)". Otherwise the model would not know a default effort exists.
- **Chips under "Same as new sessions" / "None":** none (brief: the none row shows no chips). The effort of a model-less setting is still settable over RPC and validated against the effective model.
- **Hired agents' `effort:` was not applied anywhere** (the roster never read `AgentFile.effort`). Rather than special-casing theuth, `SetDeskModel` carries the file's effort for every agent, so a hired agent's `effort:` now reaches its desk and the desk's children too. The roster re-sends it with the model (desk open, `model:` change); an `effort:`-only edit of a hired file applies at the next desk open (the PutAgent/reconcile change checks sit in B2's hunks).
- **Desk without a configured effort keeps its own `/effort`.** `SetDeskModel { effort: None }` touches the desk row only when it replaces a configured effort (then the desk goes back on `agent.effort`, filtered by its model). So nothing changes for users who never set one (D4a). Edge: an effort cleared while the desk was closed and the engine restarted in between leaves the desk on the old row value until its next `/effort`.
- **`ValidateEffort` agent message:** the roster cannot reach the catalog; `ResolveModel`'s `ModelInfo` cannot tell "no catalog entry" from "no reasoning", so a dedicated check reuses the engine's own rule.
- **Config writes are quoted** (`"high"`, `"8000"`) like `agent.effort`'s writer, so a bare budget stays a string; `~` clears.
- **Advisor `None` writes `advisor.model: ~`.**
- **Advisor row description** names `/advisor on`'s no-pick behaviour (the TUI refuses with "no remembered advisor model — pick one with /advisor").

## Tests
- agent (supervisor.rs tests): `subagent_effort_is_live_validated_and_persisted`, `a_desk_and_its_children_start_on_the_agents_effort`, `the_advisor_default_model_is_persisted_live_and_owns_its_effort`; frame.rs: `the_subagent_line_names_the_effort_only_when_set`.
- roster: `theuth_effort_is_validated_persisted_and_reaches_its_desk` (live engine), `a_roster_section_without_theuth_effort_still_loads`.
- UI: settings/ModelDefaults.test.ts (3 new: chips + effort setter + refetch; none row no chips; model change refetches the effort), settings/Advisor.test.ts (3 new: chips + setter; None no chips; model pick calls the setter and re-reads Info).

## Gates
All on the final tree (feature commit 51347b9d).
- `cargo fmt -p agent -p roster -p rpc`; unrelated rustfmt churn in rpc reverted (only my hunks remain).
- `mbx check -p agent -p roster -p rpc` and `--tests`: 0 errors, 0 warnings.
- `mbx clippy -p agent -p roster -p rpc --tests --no-deps -- -D warnings`: clean.
- `mbx test -p agent -p roster -p rpc --no-fail-fast`: 1107 passed, 5 failed, none mine. The 5 fail alone too, all on the machine's Python/monty worker: `sensor::tests::{fs_exists_through_monty, monty_budgets_and_allowlist, pr_watch_type_checks, sensor_http_follows_the_private_network_flag}` ("monty worker crashed: unsupported protocol version 6 (server supports protocol version 5)") and `loop_::runtime_bridge_tests::guessed_direct_calls_are_bridged_or_refused` (needs a worker-pool cell). None touch this change.
- The 6 new Rust tests, run alone twice: 6/6 both times.
- Bindings regenerated (`export_bindings`, 1037 files) and copied; `methods.json` lists the 5 new methods.
- `bunx tsc -b --noEmit`: 0 errors.
- `bunx vitest run` ModelDefaults.test.ts, Advisor.test.ts, rpc/api.test.ts, chat/slash.test.ts: 32/32, twice. The whole `src/components/settings` and `src/components/agents` dirs: 205/205.

## Left
- Nothing in rows 3–6 / D4h.
- Hired agents' `effort:`-only edits re-apply at the next desk open, not at once (see decisions).
- Merge notes for Z: `frame::baseline` / `baseline_dated` gained `subagent_effort` right after `subagent_model`, and `agent::SetDeskModel` gained `effort`. A call or struct literal added on another branch needs the extra `None`.
- No change was needed in ModelCombobox or ModelPicker.
