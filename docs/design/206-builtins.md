# 206 — Builtins: switch the harness's own tools by category (as built, backend)

The MCP tab's twin for builtin tools. A builtin is a tool in `ToolRegistry` whose `ToolSource` is `Native`; MCP tools never are.
Builtins are grouped in fixed categories; a whole category or a single tool can be on, off for one chat ("Off here"), or off for
every session on the engine ("Off always").

## Categories

`crates/tools/src/builtins.rs`: `CATEGORIES` (id, label, one-line description, members) in display order, `category_of(name)`
(exact member names across all categories first, then name prefixes such as `browser_`, `task_`, `doc_`; fallback `other`, so a
new tool is never uncontrollable), `category_key(id)` = `cat:<id>` (`BUILTIN_CATEGORY_PREFIX`). Ids: files, shell, code, web,
browser, computer, sandbox, knowledge, documents, output, agents, tasks, schedules, autoresearch, meetings, secrets, decisions,
dreaming, setup, channels, other. A category key names every builtin of the category, including tools registered later. Tool
names cannot contain `:`, so a key never matches a tool by name.

## Two scopes

- Off always: `tools.builtins.disabled` in `<data_dir>/config.yaml` (`ToolsConfig.builtins: BuiltinsConfig { disabled }`,
  tolerant of a missing key and of an empty one, YAML null; any other non-list value fails `tools` start like every other key),
  a sorted list of builtin names and category keys. `registry::start` seeds `ToolRegistry.builtins_disabled` from it;
  `SetBuiltinsDisabled` computes the new set, rewrites the key with `runtime::write_nested_config_value` (comments preserved;
  the value is written as a JSON array so `cat:<id>` stays a string; a hand-written block list under the key is replaced whole)
  and only after a successful write swaps it into memory and bumps `hidden_epoch`. A write that fails changes nothing and is
  an error. A call that changes nothing (no names, or every name already in the wanted state) writes nothing and bumps
  nothing. Node-local like the rest of config.yaml. Applies to every session on the engine.
- Off here: the existing per-session hidden set (`ToolRegistry.hidden`, persisted in `sessions.hidden_tools`, inherited by
  subagents through `parents`), written by the existing `set_mcp_hidden`. `cat:<id>` entries in it are honoured exactly like
  `mcp:<server>` entries. No new column, no migration.

## Enforcement

One place: `ToolRegistry::is_hidden` (`crates/tools/src/registry.rs`) also asks `builtin_switched`: a native tool whose name or
category key is in the harness-wide set, or whose category key is in the hidden set of the session or an ancestor, is hidden.
Through `is_hidden` this covers `ListTools(Some(_))` (direct tools, the python "Runtime functions" index, the live python
runtime's per-cell re-list), `visible()` (`Invoke`, `AppInvoke`, `InspectTool`). `ListTools(None)` and `list_tools` stay
unfiltered. MCP tools are never touched by `cat:` keys or the harness-wide set. `Invoke` of a tool switched off by a builtin
switch answers `<name> is turned off (Settings > Tools > Builtins)`; a hide by plain name, an MCP hide, a contact allowlist and
unregistered names keep `unknown tool: <name>`. Every change bumps `hidden_epoch`, so a running turn re-lists before its next
model call (`rebase`). The `t mcp --session` shim leaves out of its listing the registry tools `list_builtins(session)`
reports as disabled or hidden.

The switch is for the model's tools. The harness's own calls go through `HarnessInvoke` (a sibling of `Invoke`), which skips
only `builtin_switched`; hides by name or MCP server, allowlists, caps, policy and approvals still apply. Callers:
`agent::notebooks::invoke` (notebook context, `ask`, overviews: `source_get`, `doc_read`, `notebook_get`, `nb_search`, `show`,
`notebook_add`), `document::ingest::routes::invoke` (ingest's web and session tools), the web Settings search test
(`search_test`), autoresearch's launch card (`autoresearch_launch_card`) and its `code_map` study context. Model-facing paths
keep `Invoke`: the turn loop, resumed calls, the python runtime, app queries and mutations (`AppInvoke`), the MCP shim
(`session_call_tool`), `secret_capture`, and autoresearch `calls` made for a study.

## Wire

On `AgentToolingApi` (`crates/agent/src/rpc_tooling.rs`, owner `agent`, audience `owner`), next to `set_mcp_hidden`:

- `list_builtins(session: Option<SessionId>) -> Vec<BuiltinCategory>` — registry `ListBuiltins`. Categories in table order,
  empty ones left out, tools sorted by name. `BuiltinTool { name, description, risk, direct, disabled, hidden }`: `disabled` =
  name or category key in the harness-wide set; `hidden` = name or category key in the session's or an ancestor's hidden set,
  always false without a session. A session owned by another engine is forwarded to it (as `list_mcp`).
- `set_builtins_disabled(names: Vec<String>, disabled: bool, session: Option<SessionId>) -> ()` — registry
  `SetBuiltinsDisabled`. With `session`, authorised and forwarded exactly like `list_builtins`: the engine that owns the
  session changes its own harness-wide set (the set that chat's sheet lists); without, this engine. Adding: every name must
  be a registered builtin or `cat:<known id>`. Removing: also any name currently in the set (a stale entry whose tool is not
  registered now). Anything else is invalid params and nothing is written. A config file that cannot be written is an
  internal error (`the config file could not be written, nothing changed: <path>: <reason>`) and nothing changes.
- `set_mcp_hidden(session, names, hidden)` — unchanged signature, now also meaningful for `cat:<id>`.

`BuiltinCategory` / `BuiltinTool` live in `crates/tools/src/builtins.rs` (serde, schemars, ts-exported). `description` is the
tool description's first paragraph, cut at 400 characters (char boundary) with a trailing `…` when longer.

## Deliberately out

- Loop-local tools that are not in the registry (`python`, `todo`, `advise`, `consult`, the endless tools): not listed, not
  switchable.
- A TUI view.
- Replicating the harness-wide set across engines of a mesh: it is config.yaml, node-local.
- System-prompt prose blocks (plugin prompts) and the skills index that still mention a switched-off tool by name.
- Sensors call file, web and knowledge tool structs directly, not through the registry: the switch does not affect them.
- `AppInvoke` and `InspectTool` answer `unknown tool` (not "turned off") for a switched-off builtin.
- The harness's own features bypass the switch (`HarnessInvoke`, see Enforcement): notebook context, `ask`, overviews,
  document ingest, the Settings search test, autoresearch's launch card and `code_map`.
