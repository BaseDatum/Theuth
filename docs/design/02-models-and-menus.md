# Model catalog, `/model`, input header, `/mcp` menu

Status: draft v0 · 2026-09-09 · extends `00-architecture.md` and `01-config-and-subagents.md`.

Four things: (1) a bundled models.dev catalog so the harness knows context
limits and capabilities without asking anyone, (2) model resolution from
*configured* providers and a `/model` switch, (3) an omp-style header above
the input showing model + context, (4) a `/mcp` menu that sets MCP servers'
and individual tools' risk (or turns them off) by editing the
`mcp/config.yaml` the loader already watches.

## 1. Catalog (`crates/models`)

Purpose-named crate `models`. No network at build or run time.

**Data.** `crates/models/data/models.json` is models.dev `api.json` trimmed by
the `just models` recipe (`curl | cargo run -p models --bin trim`) to the
fields we use and to providers routable through genai. The projection *is*
the Rust `Model` type: `trim` deserialises upstream into `Catalog` and
serialises it back, so adding a field means adding it to the struct — there
is no second field list to keep in step. The provider allow-list is the
`models::ROUTABLE` const. Committed; refreshed by hand. `just models-check`
diffs upstream against the committed file (network; deliberately not part of
`just check`). Shape (a map keyed by provider id, identical to models.dev
minus dropped fields):

```json
{ "anthropic": { "id": "anthropic", "name": "Anthropic", "env": ["ANTHROPIC_API_KEY"],
    "models": { "claude-sonnet-4-5": { "id": "claude-sonnet-4-5", "name": "Claude Sonnet 4.5",
        "limit": { "context": 200000, "output": 64000 },
        "cost": { "input": 3, "output": 15, "cache_read": 0.3, "cache_write": 3.75 },
        "reasoning": true, "tool_call": true, "image": true,
        "reasoning_options": [{ "type": "effort", "values": ["low", "medium", "high"] },
                              { "type": "budget_tokens", "min": 1024, "max": 32000 }] } } } }
```

`image` is the one field not copied verbatim: `trim` folds models.dev
`modalities.input ∋ "image"` into it (`#[serde(default)]`, so a silent
catalog means `false`); it gates snap compaction (13 §3).

`include_str!` + `serde_json` on first use (`LazyLock`). Measured: full
api.json 4.5 MB; trimmed to these fields 1.8 MB; filtered to routable
providers is smaller still. Plain JSON in the binary; no compression dep.

**User file.** `<data_dir>/models.json`, same shape, merged over the bundled
catalog (user wins per model). This is how local models (`ollama`) and
private gateways get context limits; no bundled entry needed. After the
merge, any model with `limit.context == 0` (what `AddProvider` writes for a
gateway's `/models` list) copies `limit`, `reasoning`, `image`, and
`reasoning_options` from `Catalog::closest(model_id)` — the models.dev entry
sharing the most name tokens (`Qwen/Qwen3.8-Flash-Next` → `qwen3.8-*`,
`zai-org/GLM-5.3-Flash` → `glm-5.3-flash`). Zero-limit models are never
matcher candidates. `id`, `name`, and `cost` stay as written.

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
| `amazon-bedrock` | `bedrock_sigv4` (bearer fallback below) |
| `opencode-go` | `opencode_go` |
| `github-copilot` | `github_copilot` |
| `google-vertex`, `google-vertex-anthropic` | `vertex` |
| `ollama-cloud` | `ollama_cloud` |
| anything else | the provider id itself |

The fallback is what makes `ollama/llama3` → `ollama::llama3` work with no
entry. Providers genai cannot route (`mistral`, `azure`) are simply not in the
trimmed file; a user who adds them gets a genai "unknown adapter" error at
call time, which is the honest failure.

**Configured provider** = a key in the secrets vault under the genai namespace
(`anthropic`, `open_router`, …), a login, the adapter's default env var, or
an adapter needing no API key. Bedrock uses the SDK credential chain: its
catalog remains selectable without probing credentials or metadata services;
missing credentials fail visibly at inference time. See doc 01 §1.3 and §2.2 below.

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
pub struct Model { pub id: String, pub name: String, pub limit: Limit, pub cost: Option<Cost>, pub reasoning: bool, pub tool_call: bool,
                   #[serde(default)] pub image: bool, #[serde(default)] pub reasoning_options: Vec<ReasoningOption> }
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ReasoningOption { Effort { values: Vec<String> }, BudgetTokens { min: u32, max: Option<u32> }, Toggle }
pub const ROUTABLE: &[&str]; // models.dev provider ids the trim keeps
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

**`reasoning_options`** is models.dev's per-model description of how thinking
is dialled: `effort {values}` (keyword levels, verbatim from upstream —
`low|medium|high`, some add `minimal`/`xhigh`/`max`), `budget_tokens {min,
max}` (legacy Anthropic-style token budget), `toggle` (on/off only). Derived
accessors on `Model`:

```rust
impl Model {
    /// Keywords the model accepts: the `effort` values verbatim; only `toggle` → ["off","on"];
    /// only `budget_tokens` → ["low","medium","high"] (genai maps those to budgets); none → [].
    pub fn efforts(&self) -> Vec<String>;
    /// (min, max) when the model takes `budget_tokens`.
    pub fn budget(&self) -> Option<(u32, Option<u32>)>;
    /// Ok for a value in `efforts()` or an integer within `budget()`; Err lists what is accepted.
    pub fn validate_effort(&self, s: &str) -> Result<(), String>;
}
```

Rejected: fetching models.dev at runtime (network dependency for a static
fact; the recipe refresh is one command). Rejected: genai-native ids
(`anthropic::…`) for users; they don't match the site people copy from.
Rejected (was v0): a `jq` filter in the justfile as the projection — a second
field list that drifted from the struct and silently dropped
`reasoning_options`, which is why `trim` is Rust.

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

RPC: `list_models`, `set_model(session?, model)`, `set_effort(session?, effort?)`.

Rejected: switching the model for running subagents (they finish on what they
started with; `subagents.model` governs new ones). Rejected: `/model` writing
through `figment` — figment is read-only and a serde round-trip drops the
user's comments; a line edit is 15 lines and lossless.

### 2.1 Effort

Reasoning effort is a **setting separate from the model id**. Precedence:
session value > `agent.effort` (config) > provider default (unset, nothing
sent). `subagents.effort` is the child default; `None` → `agent.effort`.

```rust
pub struct AgentConfig { …, pub effort: Option<String> }
pub struct SubagentsConfig { …, pub effort: Option<String> }
pub struct SetEffort { pub session: Option<SessionId>, pub effort: Option<String> } // ask → Result<(), String>
pub struct ModelInfo { …, pub image: bool, pub efforts: Vec<String>, pub budget_min: Option<u32>, pub budget_max: Option<u32> }
pub struct SessionSummary { …, pub effort: Option<String> }
pub struct Loaded { …, pub model: String, pub effort: Option<String> }
pub enum AgentEvent { …, EffortChanged { session: SessionId, effort: Option<String> } }
```

`SetEffort` mirrors `SetModel`: the value is validated with
`Model::validate_effort` against the session's *current* model (a passthrough
model unknown to the catalog accepts any genai keyword or integer); on
success the `Session` actor takes it, the store writes the `sessions.effort`
column, `EffortChanged` is published, and `agent.effort` in `config.yaml` is
line-edited (or removed, for `None`) exactly as `/model` edits `model:`.
Unlike `SetModel` it takes effect at the **next turn**, not mid-turn:
Anthropic rejects adding or dropping thinking inside a tool-use turn (the
replayed assistant blocks no longer match the request's thinking config).
`/model` to a model whose `efforts()` lacks the current value clears it and
prints an Info line, so a stale level never reaches a provider that rejects it.

String → genai (`effort_of`, agent crate):

| setting | `ReasoningEffort` |
|---|---|
| `off` · `none` · `zero` | `Zero` |
| `on` | `Medium` |
| `default` | unset (option not sent) |
| all digits | `Budget(n)` |
| anything else | `ReasoningEffort::from_keyword` (`minimal|low|medium|high|xhigh|max`) |

For legacy-budget models (`budget_tokens` only) genai's Anthropic adapter
maps the presets to budgets: low = 1024, medium = 8000, high = 24000 tokens.

`capture_reasoning_content` is now always on: genai's
`into_assistant_message_for_tool_use` replays the signed thinking blocks on
every tool-use round, which Anthropic requires whenever thinking is enabled.
Cost is the reasoning text kept in memory for the turn; it is not stored.

Rejected: encoding effort in the id (`provider/model@high`). models.dev ids
already contain `:`, `@` and `-high` suffixes (`gpt-5-codex-high`,
`…:free`), so a compound id needs an escape rule and a second resolver, and
the session row would store a value that is not a catalog key. A separate
column is one `Option<String>`.

### 2.2 AWS Bedrock profiles

`amazon-bedrock/<model-or-inference-profile-id>` keeps the public model id;
genai's `bedrock-sigv4` feature supplies Converse streaming, signing and
credential refresh. No AWS parser, signer or new backend in wth.

The agent owns `ListAwsProfiles` and `SetAwsProfile { profile: Option<String> }`;
RPC exposes `list_aws_profiles` and `set_aws_profile`. Both return
`AwsProfiles { profiles, selected, default_profile }`. Discovery calls
`aws_config::profile::load`, then `ProfileSet::profiles()`; the SDK handles
merging config and credentials, `AWS_CONFIG_FILE`, `AWS_SHARED_CREDENTIALS_FILE`,
`AWS_PROFILE`, default paths and SSO sections. Only profile names cross RPC.
Discovery rereads the files, does not fetch credentials, and reports parse errors.

`agent.aws_profile` is one harness-wide selection, shared by existing clients
and read on each inference request (including subagents and title calls).
The setter validates a discovered name, writes a quoted YAML scalar using the
existing config writer, then updates the live value. A write failure leaves
the live value unchanged. `null` clears the override; never pick the first
profile automatically or change process environment variables.

An explicit profile uses the SDK's `ProfileFileCredentialsProvider`, so ambient
access-key variables cannot silently change its account. Static keys, SSO,
role chains and credential processes remain SDK responsibilities. Region uses
the SDK profile-aware chain (`AWS_REGION` overrides profile region).
Without an explicit profile: a `bedrock_api` vault key wins, then
`BEDROCK_API_KEY`, then `AWS_BEARER_TOKEN_BEDROCK`, otherwise the AWS default
credential chain. Explicit raw `bedrock_api::…` model ids remain bearer-only.

Desktop: Settings → Connections → AWS Bedrock; `/aws` opens that pane.
TUI: `/aws` shows status and opens the shared keyboard picker with multiple
profiles or an explicit selection to clear. `/aws <name>` selects directly;
`/aws clear` restores automatic authentication. The picker includes the
default chain and marks the current selection. A stale name is shown, not
silently replaced. Profiles belong to the connected harness, not the frontend.

Dependency patch: the existing `skymoore/rust-genai` fork tracks upstream main
and preserves its tool-image support plus the explicit-profile provider fix
that prevents environment credentials overriding the chosen account.
`Cargo.lock` pins the fork's merge commit; genai source lives in the fork, not wth.

## 3. Input header (tui crate)

One line directly above the input box, dark grey, omp-style:

```
anthropic/claude-sonnet-4-5  ·  high  ·  ctx 12.3k / 200k (6%)  ·  cache 94%
```

Model = `TurnStarted.model` of the shown session (falls back to the config
default before the first turn). Effort = the session's value (`Loaded.effort`,
then `EffortChanged`); the segment is omitted when unset. Context =
`TurnFinished.context` of the shown session vs
`Catalog::get(id).limit.context`; unknown limit → `ctx 12.3k`.
Colour steps at 70 % (yellow) and 90 % (red). `cache` = `usage.cached /
usage.prompt` of the last finished turn, the prefix-cache hit ratio
(`07-context.md` §7); omitted until a turn has reported usage. While
streaming the values are the previous turn's; no per-delta counting
(providers only report usage at end).

The existing status line (session id, agents, approvals) stays below the input.

## 4. Slash menu, `/model` picker, `/mcp` menu (tui crate)

**Slash popup.** Input beginning with `/` opens a list above the header:
every command whose name starts with the typed prefix, with a one-line
description. ↑/↓ moves, Tab completes the name (plus a trailing space when it
takes an argument), Enter runs it (or completes when the highlighted command
needs an argument), Esc closes without clearing the input. Commands:
`/model [id]`, `/effort [level|clear]`, `/mcp`, `/agents`, `/parent` (19),
`/sessions`, `/resume <id>`, `/new`, `/cancel`,
`/compact [summary|cut|snap|style …]` (11), `/clear` (11 §2.4: the conversation goes, the session stays), `/shake [images|documents|messages|tools|summary]` (48 §2.1, added 2026-09-18: the five literal categories are the whole
grammar — anything else prints usage and is not sent to the model; bare
`/shake` opens the picker with nothing ticked, a category pre-ticks its
matches; ↑↓ move, Space ticks, ⏎ reviews the exact targets and their
protocol companions, ⏎ again removes, Esc backs out; there is no
model-callable counterpart), `/knowledge [query | history [id]
| restore <id> [commit] | harvest]` (the knowledge bundle: list, search, git
history, restore, harvest now — `10 §8`), `/quit`. Signal mirrors `/effort`
and its `/status` shows the current level (09).

**Picker (shared modal).** Items, cursor, optional filter typed inline.
`/model` with no argument: `Command::ListModels` → picker over `ModelInfo`
rows `provider/model    name    200k`; typing filters (substring on id and
name); Enter → `Command::SetModel { session: shown root, model }`; the reply
`UiEvent::ModelSet(info)` updates the header immediately and, when
`info.efforts` is non-empty, chains into the effort picker. `/model <id>`
skips the picker.

**`/effort` picker.** No argument → picker over the current model's
`efforts()` with a trailing `clear` entry; Enter → `Command::SetEffort
{ session: shown root, effort }` (`clear` → `None`). `/effort <level>` and
`/effort clear` skip the picker. The reply updates the header; the model
call itself changes only from the next turn (§2.1).

**`/sessions` picker.** `Command::ListSessions` → picker over top-level
sessions, most recently active first (`sessions.updated_at`: creation or the
last appended message; `● id  mm-dd hh:mm  model  title`, stamp = that
activity, ● = shown); Enter
→ `resume(id)`, which is `Command::Load`. Switching never touches the agent:
the session left behind keeps its turn running in its actor and its events
keep flowing; the TUI only changes which session it renders. `LoadSession`
replies `Loaded { history, running }` so the status line shows `streaming…`
on return; a tool call in flight at switch time is in the store (the
assistant message is persisted before dispatch) and comes back as a
`HistoryEntry::Tool { result: None }` card that its `ToolCallFinished`
completes by `call_id`. Ids display as their last 8
chars (UUIDv7's first 8 are the timestamp and collide).

**`/mcp` menu.** `Command::ListMcp(session)` → `UiEvent::Mcp(Vec<McpServer>)` → menu:

```
▸ everything      13 tools   on                   risk: read_only    ● connected
    echo                     on                   read-only (allow)
    read_file                off · this session   mutating* (ask)
    write_file               off · always
    …
▸ broken           – tools   on                   risk: annotations  ○ not connected
▸ noisy            – tools   off · always         risk: annotations  ○ off
```

→ expands a server, ← collapses. Two switches, kept apart on purpose:

- **Enter** turns the row on or off. Turning *off* asks for the scope on the
  hint line: `s` this session only (`set_mcp_hidden`: the registry hides the
  tool names for the root session and its subagents, in memory, other
  sessions unaffected) or `a` always (`off` in `mcp/config.yaml`: server
  killed / tool unregistered, every session, survives restart). Turning *on*
  undoes whichever switch is off — no prompt. Before the first message there
  is no session yet: `s` creates one (`new_session`) and the TUI adopts it,
  so the hide is in place before the first turn lists tools. A server shows
  `off · this session` when every tool it offers is hidden.
- **Space** cycles the row's `mcp/config.yaml` risk line annotations →
  `read_only` → `mutating` → `destructive` → annotations and never reaches
  `off`; **Backspace** clears the line so it inherits (server line, else
  annotations). Both are inert on an `off · always` row.

Every row shows the effective `Risk` and, in parentheses, what `tools.policy`
does with it; `*` marks a tool's own line (12 §5). Every change is
`Command::SetGate { .. }` / `Command::SetHidden { .. }` → on reply
`list_mcp(session)` again, so the menu always shows what the loader has; a
reconcile settling while the menu is open re-lists too. Esc closes (or, in the
scope prompt, cancels it).

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
pub struct ListServers { pub session: Option<SessionId> }        // ask → Vec<McpServer>; session fills `hidden`
pub enum Gate { ReadOnly, Mutating, Destructive, Off }        // serde: read_only | mutating | destructive | off (tools crate)
pub struct SetGate { pub server: String, pub tool: Option<String>, pub gate: Option<Gate> } // ask → Result<(), String>
pub struct McpServer { pub name: String, pub connected: bool, pub gate: Option<Gate> /* server line; None = annotations */, pub tools: Vec<McpToolInfo>, pub repo: Option<String>, pub auth: McpAuth }
pub struct McpToolInfo { pub name: String, pub description: String, pub gate: Option<Gate> /* own line */, pub risk: Risk /* effective */, pub action: PolicyAction /* tools.policy(risk), display only */, pub registered: Option<String> /* registry name; None when off */, pub hidden: bool /* for the session asked about */ }
```

`ListServers` lists every server the files *want* (connected or not; an
`off` server is listed unconnected with `gate: Some(Off)`) and, for connected
ones, every remote tool with its own line, effective risk (tool line → server
line → annotations), the action `tools.policy` resolves that risk to, and
the name it is registered under (`<server>__<name>` when another server or
a native offers `name`; `None` when `off`). The actor receives `tools.policy` at start, next to the registry ref,
only to fill `action`; the registry alone decides at invoke time (12 §5).
`SetGate` rewrites `<data_dir>/mcp/config.yaml` from the in-memory `Gates`
via `serde_yaml` (`gate: None` removes the key; an emptied server entry is
dropped) and calls `reconcile` immediately rather than waiting for the
watcher. The gates file is machine-written from now on: hand edits still
work (the watcher reconciles them) but comments do not survive the next
`SetGate`.

Reconcile itself is unchanged; the watcher task just sends `Reconcile` to the
actor instead of owning the state.

Session switch (registry, not the actor): `SetHidden { session, tools, hidden }`
hides registered tool names for one session and its subagents in memory
(`ListTools(Some(session))` omits them, `Invoke` answers `unknown tool`);
`HiddenTools(session)` reads the set back, which `ListServers { session }`
uses to fill `McpToolInfo::hidden`. Nothing is written; the persistent switch
stays `off` in `mcp/config.yaml`.

RPC: `list_mcp(session?)`, `set_mcp_gate(server, tool?, gate?)`,
`set_mcp_hidden(session, tools, hidden)`.

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

effort: implemented 2026-09-11. Verified headless over RPC on a fresh data
dir: `list_models` carries `efforts` from the regenerated catalog (opus-4-6
`low|medium|high|max`, sonnet-4-5 `low|medium|high`, gpt-5.1
`none|low|medium|high`); `set_effort` rejects `turbo` and out-of-range
budgets with the accepted list, persists to the session row and
`config.yaml` (`effort: "high"`, `~` to clear), publishes typed
`EffortChanged`; `set_model` to a model lacking the value clears it;
`load_session`/`list_sessions` return it after a process restart.
Live Anthropic: sonnet-4-5 (legacy `budget_tokens` path) at `high` and
`low` and opus-4-6 (`output_config.effort`) at `max`/`high` each ran a
tool-use turn to `end_turn` with `ReasoningDelta`s streaming, so signed
thinking blocks replay on round 2. TUI on a pty: `/effort` picker rows +
`clear`, `/effort low` puts `· low ·` in the header, `clear` drops it,
`/effort bogus` shows the accepted list, `/model` chains into the picker.
Found during verification: genai sends a numeric budget as
`thinking.adaptive.budget_tokens` on Claude 4.6+, which Anthropic rejects
(`Extra inputs are not permitted`) - `Model::budget` is `None` when the
model also has `effort` keywords, so budgets are offered only on
budget-only models. Not exercised: OpenAI live (the ChatGPT-account Codex
endpoint rejected every model id for this account, effort or not),
Signal `/effort`, `subagents.effort`.
