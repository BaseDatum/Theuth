# Model catalog, `/model`, input header, `/mcp` menu

Status: draft v0 · 2026-09-09 · extends `00-architecture.md` and `01-config-and-subagents.md`.

Four things: (1) a bundled models.dev catalog so the harness knows context
limits and capabilities without asking anyone, (2) model resolution from
*configured* providers and a `/model` switch, (3) an omp-style header above
the input showing model + context, (4) a `/mcp` menu that enables/disables MCP
servers and individual tools by editing the gates file the loader already
watches.

## 1. Catalog (`crates/models`)

Purpose-named crate `models`. No network at build or run time.

**Data.** `crates/models/data/models.json` is models.dev `api.json` trimmed by
the `just models` recipe (curl + jq) to the fields we use and to providers
routable through genai. Committed; refreshed by hand. Shape (a map keyed by
provider id, identical to models.dev minus dropped fields):

```json
{ "anthropic": { "id": "anthropic", "name": "Anthropic", "env": ["ANTHROPIC_API_KEY"],
    "models": { "claude-sonnet-4-5": { "id": "claude-sonnet-4-5", "name": "Claude Sonnet 4.5",
        "limit": { "context": 200000, "output": 64000 },
        "cost": { "input": 3, "output": 15, "cache_read": 0.3, "cache_write": 3.75 },
        "reasoning": true, "tool_call": true } } } }
```

`include_str!` + `serde_json` on first use (`LazyLock`). Measured: full
api.json 4.5 MB; trimmed to these fields 1.8 MB; filtered to routable
providers is smaller still. Plain JSON in the binary; no compression dep.

**User file.** `<data_dir>/models.json`, same shape, merged over the bundled
catalog (user wins per model). This is how local models (`ollama`) and
private gateways get context limits; no bundled entry needed.

**Ids.** User-facing model ids are models.dev style: `provider/model`
(`anthropic/claude-sonnet-4-5`, `openrouter/anthropic/claude-sonnet-4-5`).
Split at the *first* `/`; model ids may themselves contain slashes. The
session row, `agent.model`, `/model`, and every event carry this id. genai
sees `adapter::model`, produced by:

| models.dev provider | genai namespace |
|---|---|
| `google` | `gemini` |
| `openrouter` | `open_router` |
| `togetherai` | `together` |
| `fireworks-ai` | `fireworks` |
| `moonshotai` | `moonshot` |
| `alibaba` | `qwen_cloud` |
| `alibaba-cn` | `aliyun` |
| `amazon-bedrock` | `bedrock_api` |
| `opencode-go` | `opencode_go` |
| `github-copilot` | `github_copilot` |
| `google-vertex`, `google-vertex-anthropic` | `vertex` |
| `ollama-cloud` | `ollama_cloud` |
| anything else | the provider id itself |

The fallback is what makes `ollama/llama3` → `ollama::llama3` work with no
entry. Providers genai cannot route (`mistral`, `azure`) are simply not in the
trimmed file; a user who adds them gets a genai "unknown adapter" error at
call time, which is the honest failure.

**Configured provider** = genai can authenticate it: `credentials.yaml` has a
key under the genai namespace (`anthropic:`, `open_router:` …), or the
adapter's default env var is set, or the adapter needs no key (`ollama`).
This is exactly the auth-resolver rule from doc 01 §1.3, exposed as a
predicate.

**Resolution** of a typed/config id:
1. Contains `/` and `provider/model` is in the catalog → that model.
2. Contains `/` but unknown → pass through: genai id per the table, no limit
   known.
3. Bare (`claude-sonnet-4-5`) → search *configured* providers for a model with
   that id: exactly one → it; several → error naming them; none → pass through
   to genai unchanged (its own prefix inference still works). Old session
   rows with bare ids keep working through this rule.

```rust
pub struct Catalog { /* providers: BTreeMap<String, Provider> */ }
pub struct Provider { pub id: String, pub name: String, pub env: Vec<String>, pub models: BTreeMap<String, Model> }
pub struct Model { pub id: String, pub name: String, pub limit: Limit, pub cost: Option<Cost>, pub reasoning: bool, pub tool_call: bool }
pub struct Limit { pub context: u32, pub output: u32 }
pub struct Cost { pub input: f64, pub output: f64, pub cache_read: Option<f64>, pub cache_write: Option<f64> }

impl Catalog {
    /// Bundled + `<data_dir>/models.json` (user wins per model).
    pub fn load(data_dir: &Path) -> Catalog;
    /// genai namespace for a models.dev provider id (table above; fallback = id).
    pub fn adapter(provider: &str) -> &str;
    pub fn providers(&self) -> impl Iterator<Item = &Provider>;
    /// Exact `provider/model` lookup.
    pub fn get(&self, id: &str) -> Option<(&Provider, &Model)>;
    /// Rules 1–3. `configured(genai_namespace) -> bool`.
    pub fn resolve(&self, id: &str, configured: &dyn Fn(&str) -> bool) -> Result<Resolved, String>;
}
pub struct Resolved { pub id: String /* canonical or raw */, pub genai: String /* adapter::model or raw */, pub provider: Option<String>, pub model: Option<Model> }
```

Rejected: fetching models.dev at runtime (network dependency for a static
fact; the recipe refresh is one command). Rejected: genai-native ids
(`anthropic::…`) for users; they don't match the site people copy from.

## 2. Model resolution and `/model` (agent crate)

`Agents` holds `catalog: Arc<Catalog>` and a `configured: Arc<dyn Fn(&str)->bool>`
derived from the same credentials map the auth resolver uses (`client()` is
split so both read one parse). `agent.model` and `agent.subagents.model` are
resolved at session creation; the session row stores the canonical id; the
loop stores `Resolved` and passes `resolved.genai` to `exec_chat_stream`.
Unresolvable config id → the session's first turn errors visibly
(`model: …`), nothing panics at start.

`SetModel { session: Option<SessionId>, model: String }` → `ask` →
`Result<ModelInfo, String>`. Resolves; on success:
- `session: Some(id)`: `Session` actor gets `SetModel(Resolved)` (takes
  effect on the next model call, mid-turn included), store
  `UPDATE sessions SET model`, publish `AgentEvent::ModelChanged`.
- Always: the in-memory default (`cfg.model`, behind `RwLock` or rebuilt Arc)
  becomes the new id **and** `<data_dir>/config.yaml` is edited in place:
  the first `model:` line inside the `agent:` block is replaced, preserving
  indentation and everything else in the file; no such line → `agent:\n  model: <id>\n`
  is appended. Comments survive. Failure to write is a warning, not an error
  (the switch still happened).

`ListModels` → `ask` → `Vec<ModelInfo>`: every catalog model whose provider is
configured, sorted by provider then id. This is what the picker shows.

```rust
pub struct ModelInfo { pub id: String, pub name: String, pub provider: String, pub context: u32, pub reasoning: bool }
```

Events: `TurnStarted` gains `model: String` (so a UI joining late knows what
is running); `TurnFinished` gains `context: u32` = the *last* round's
`prompt_tokens + completion_tokens` (what the window holds now; 0 if the
provider reported nothing); new `AgentEvent::ModelChanged { session, model }`.

RPC: `list_models`, `set_model(session?, model)`.

Rejected: switching the model for running subagents (they finish on what they
started with; `subagents.model` governs new ones). Rejected: `/model` writing
through `figment` — figment is read-only and a serde round-trip drops the
user's comments; a line edit is 15 lines and lossless.

## 3. Input header (tui crate)

One line directly above the input box, dark grey, omp-style:

```
anthropic/claude-sonnet-4-5  ·  ctx 12.3k / 200k (6%)
```

Model = `TurnStarted.model` of the shown session (falls back to the config
default before the first turn). Context = `TurnFinished.context` of the shown
session vs `Catalog::get(id).limit.context`; unknown limit → `ctx 12.3k`.
Colour steps at 70 % (yellow) and 90 % (red). While streaming the value is
the previous turn's; no per-delta counting (providers only report usage at
end).

The existing status line (session id, focus, approvals) stays below the input.

## 4. Slash menu, `/model` picker, `/mcp` menu (tui crate)

**Slash popup.** Input beginning with `/` opens a list above the header:
every command whose name starts with the typed prefix, with a one-line
description. ↑/↓ moves, Tab completes the name (plus a trailing space when it
takes an argument), Enter runs it (or completes when the highlighted command
needs an argument), Esc closes without clearing the input. Commands:
`/model [id]`, `/mcp`, `/agents`, `/focus <name>`, `/sessions`,
`/resume <id>`, `/new`, `/cancel`, `/quit`.

**Picker (shared modal).** Items, cursor, optional filter typed inline.
`/model` with no argument: `Command::ListModels` → picker over `ModelInfo`
rows `provider/model    name    200k`; typing filters (substring on id and
name); Enter → `Command::SetModel { session: shown root, model }`; the reply
`UiEvent::ModelSet(info)` updates the header immediately. `/model <id>` skips
the picker.

**`/sessions` picker.** `Command::ListSessions` → picker over top-level
sessions, newest first (`● id  mm-dd hh:mm  model  title`, ● = shown); Enter
→ `resume(id)`, which is `Command::Load`. Switching never touches the agent:
the session left behind keeps its turn running in its actor and its events
keep flowing; the TUI only changes which session it renders. `LoadSession`
replies `Loaded { history, running }` so the status line shows `streaming…`
on return; a tool call in flight at switch time is not in the store yet, so
only its result line appears when it finishes. Ids display as their last 8
chars (UUIDv7's first 8 are the timestamp and collide).

**`/mcp` menu.** `Command::ListMcp` → `UiEvent::Mcp(Vec<McpServer>)` → menu:

```
▸ everything      13 tools   allow        ● connected
▾ filesystem       4 tools   ask          ● connected
    read_file                 (ask)
    write_file                deny
    …
▸ broken           –          allow        ○ not connected
```

→/Enter expands a server, ← collapses. **Space** on a server row cycles its
gate allow → ask → deny → allow (deny = server killed, its tools gone; that's
"disable"). Space on a tool row cycles the tool's own gate; **Backspace**
clears the tool override so it inherits (shown in parentheses). Every change
is `Command::SetGate { server, tool, action }` → on reply `Command::ListMcp`
again, so the menu always shows what the loader has. Esc closes.

Menus are modal in the same sense the approval box is: while one is open the
input has no focus and the key handler routes to the menu. An approval
request arriving while a menu is open takes precedence (renders on top,
`y`/`n` still work); the menu resumes after.

Rejected: a generic command palette with fuzzy matching (prefix is enough for
ten commands). Mouse: see 00 §8 (native selection instead of capture).

## 5. MCP actor (tools crate)

`mcp::start` today parks its state in a task. It becomes a kameo actor `Mcp`
registered as `MCP_ACTOR = "mcp"`, holding `State` and answering:

```rust
pub struct ListServers;                                       // ask → Vec<McpServer>
pub struct SetGate { pub server: String, pub tool: Option<String>, pub action: Option<PolicyAction> } // ask → Result<(), String>
pub struct McpServer { pub name: String, pub connected: bool, pub gate: PolicyAction, pub tools: Vec<McpToolInfo> }
pub struct McpToolInfo { pub name: String, pub description: String, pub gate: Option<PolicyAction> /* own override */, pub effective: PolicyAction, pub registered: bool }
```

`ListServers` lists every server the files *want* (connected or not; a
deny-gated server is listed unconnected with `gate: Deny`) and, for connected
ones, every remote tool with its override, effective action, and whether it
is currently registered (false when denied or hidden by a name clash).
`SetGate` rewrites `<data_dir>/mcp/config.yaml` from the in-memory `Gates`
via `serde_yaml` (`action: None` removes the key; an emptied server entry is
dropped) and calls `reconcile` immediately rather than waiting for the
watcher. The gates file is machine-written from now on: hand edits still
work (the watcher reconciles them) but comments do not survive the next
`SetGate`. `Gates` gains `Serialize`; `PolicyAction` already has serde.

Reconcile itself is unchanged; the watcher task just sends `Reconcile` to the
actor instead of owning the state.

RPC: `list_mcp`, `set_mcp_gate(server, tool?, action?)`.

## 6. Implementation order

1. `models` crate + `just models` recipe + data file. Independent.
2. Agent: catalog wiring, `SetModel`/`ListModels`, event fields, write-back.
3. Tools: `Mcp` actor, `ListServers`/`SetGate`.
4. TUI: header, slash popup, picker, `/model`, `/mcp`.
5. RPC methods; headless e2e: `set_model` mid-session then a turn on the new
   model, `set_mcp_gate` deny → tool gone → allow → back; TUI on a real pty.

2–4 run in parallel against the contracts above; 1 is a prerequisite only
for compiling 2 and 4 (they can code against the signatures first).

## 7. Status (2026-09-09)

Shipped and verified. Headless over RPC with the key only in
`credentials.yaml`: `list_models` → 14 anthropic models; turn on
`anthropic/claude-sonnet-4-5` (`TurnFinished.context` 3435); `set_model` to
haiku → `ModelChanged`, `config.yaml` line replaced with its inline comment and
neighbours intact, next turn ran on haiku (row + event agree); bare
`claude-haiku-4-5` resolved; `set_mcp_gate` deny tool → gone from
`list_tools`; deny server → killed, 0 tools; clear → back with the tool
override intact; `ask` → `ApprovalRequested` for `echo`. TUI on a real pty
(pyte): slash popup filters `/mo`, Tab completes, picker filters `sonnet`,
Enter switches and the header flips to `ctx 0 / 1.0M`; `/mcp` expand, Space
cycles allow→ask→deny→allow, `deny` shows `○ not connected`; after a turn the
header reads `ctx 3.4k / 1.0M (0%)`.

Found and fixed during verification: the MCP child inherited our tty on
stderr, printed its banner into the TUI, and reset termios when killed (keys
echoed raw afterwards) — stderr is now piped to the log. A refresh reply after
a slow reconnect reopened a menu the user had closed — replies only open the
menu when `/mcp` asked for it.

Deviations from the draft: `Catalog::adapter` returns `&str` (borrows the
input); `MenuKey` is `{Consumed, Input(String)}`; cycling deny→allow writes an
explicit `gate: allow` rather than clearing; the status line no longer repeats
the model. Not exercised: non-anthropic providers live, `models.json` user
file end to end, `--config <other>` write-back (only `<data_dir>/config.yaml`
is edited).
