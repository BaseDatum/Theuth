# Agent Client Protocol: external harnesses as subagents, wth as an agent

Status: draft v0 · 2026-09-09 · extends `00-architecture.md` §5, §7; `01-config-and-subagents.md` §2

[ACP](https://agentclientprotocol.com) is JSON-RPC 2.0 between an **Agent**
(an LLM harness: Claude Code, Codex, Gemini CLI, OpenCode, Goose, Copilot,
Cursor, Kiro, …) and a **Client** (an editor or any other UI). One crate,
[`agent-client-protocol`](https://docs.rs/agent-client-protocol) 2.1.0,
implements both roles. wth wants both, in this order of value:

1. **Client** — `spawn_subagent{agent: "codex"}` starts an installed harness
   as a child session. The parent, the roster, the approval modal, result
   delivery and peer messaging are the ones doc 01 §2 already has; only the
   thing running the turn changes. Every harness on the machine becomes a
   worker with its own model access, tools and subscription.
2. **Agent** — `wth --acp` speaks ACP on stdio, so Zed, JetBrains, neovim,
   Emacs, VS Code, Obsidian, mobile and chat bridges (the
   [client list](https://agentclientprotocol.com/get-started/clients)) are
   wth frontends without writing one. This replaces the "Tauri later" plan in
   `00-architecture.md` §9 as the second-frontend milestone.

The `cursor` crate is the pattern *not* to repeat: per-vendor reverse
engineering (Connect-RPC, protobuf, pinned client version strings). ACP is one
protocol every vendor ships and tests against (the registry runs a daily
protocol matrix).

## 1. Protocol facts relied on

Stable v1 only. v2 is a published draft; nothing here needs it.

| Direction | Method | Used for |
|---|---|---|
| C→A | `initialize` | version + capabilities; response lists `authMethods`, `agentCapabilities.loadSession`, `sessionCapabilities` |
| C→A | `authenticate {methodId}` | when `session/new` errors `auth_required` |
| C→A | `session/new {cwd, mcpServers}` → `sessionId` | one per child |
| C→A | `session/prompt {sessionId, prompt: ContentBlock[]}` → `{stopReason}` | one per turn; the response *is* the turn end |
| C→A | `session/cancel {sessionId}` (notification) | abort; prompt then returns `cancelled` |
| C→A | `session/load` / `session/list` / `session/close` | optional, capability-gated |
| A→C | `session/update {sessionId, update}` (notification) | `agent_message_chunk`, `agent_thought_chunk`, `user_message_chunk`, `tool_call`, `tool_call_update`, `plan`, `usage_update`, `available_commands_update`, `current_mode_update`, `config_option_update` |
| A→C | `session/request_permission {toolCall, options[]}` → `{outcome}` | approval; `options[].kind` ∈ `allow_once` `allow_always` `reject_once` `reject_always`; outcome `selected{optionId}` or `cancelled` |
| A→C | `fs/read_text_file`, `fs/write_text_file`, `terminal/*` | optional; we advertise none (§3.8) |

Tool calls carry `toolCallId`, `title`, `kind` (`read` `edit` `delete` `move`
`search` `execute` `think` `fetch` `other`), `status` (`pending`
`in_progress` `completed` `failed`), `content[]` (`content` | `diff` |
`terminal`), `rawInput`, `rawOutput`, `locations[]`. All paths absolute.

SDK shape (2.1.0, rebuilt on SACP): `Client.builder().on_receive_notification(..).on_receive_request(..).connect_with(transport, |cx| …)`;
`AcpAgent::new(AcpAgentConfig::new(cmd).args(..).env(..))` is a
`ConnectTo<Agent>` that spawns the subprocess and owns its stdio. Handlers run
on a per-connection dispatch loop; a handler that awaits a response to its own
outgoing request deadlocks — long work goes through `cx.spawn`. Agent side:
`Agent.builder()…connect_to(Stdio::new())`.

Registry: `https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json`,
entries `{id, name, version, description, distribution: {binary: {<platform>: {archive, sha256, cmd, args, env}}, npx: {package, args}, uvx: {package, args}}}`.
Only agents that implement `authMethods` are listed (CI-verified).

## 2. Why not just another `genai` adapter

An ACP child is not a model. It brings its own tools, its own approval
policy, its own context management and its own login (a Claude Max
subscription through `claude-agent-acp`, ChatGPT through `codex-acp`). What
wth gets back is a finished task, a transcript, and a stop reason — exactly
what `<subagent_result>` already carries. Modelling it as a session-shaped
peer keeps every existing surface (roster, `/focus`, approvals, `send_message`,
`list_children`, RPC events) working unchanged; modelling it as a model would
force wth's loop to drive a harness that already has one.

## 3. Client: external harnesses as subagents

### 3.1 Which harnesses exist

`~/.wth/acp/agents.yaml`, one entry per runnable harness:

```yaml
claude:
  command: npx
  args: [-y, "@agentclientprotocol/claude-agent-acp"]
codex:
  command: codex-acp
gemini:
  command: gemini
  args: [--experimental-acp]
local-opencode:
  command: /Users/me/bin/opencode
  args: [acp]
  env: { OPENCODE_CONFIG: /Users/me/oc.json }
  description: OpenCode on the local build   # optional; shown to the model
```

`command`/`args`/`env` are exactly `AcpAgentConfig`; the file deserialises
into it plus a `description`. "Installed" = has an entry here. No PATH
probing: the flags that put a harness into ACP mode differ per vendor and
the registry already knows them.

`/acp` (TUI) lists entries with a probe result (`initialize` succeeded /
`auth_required` / spawn failed) and the cached registry below them.
`/acp add <registry-id>` writes an entry from the registry's `distribution`:
`npx` → `command: npx, args: [-y, <package>, ...args]`; `uvx` → `command:
uvx, args: [<package>, ...args]`; `binary` → download the current platform's
archive into `~/.wth/acp/bin/<id>/`, verify `sha256`, extract, `command:` the
extracted `cmd`. `/acp remove <name>` deletes the entry (and `bin/<id>` if
present). The registry JSON is fetched on first `/acp` and re-fetched when
older than a day (`~/.wth/acp/registry.json`); offline shows the cached one.
Hot-reloaded like `mcp/` (same `notify` watcher pattern, doc 01 §1.4): edit
the file, next spawn sees it.

Rejected: entries inside `config.yaml` (the list grows by tool, like `mcp/`,
and `/acp add` should not rewrite the user's main config); auto-detecting
`claude`/`codex`/`gemini` on PATH (the CLI binaries are not ACP servers; the
adapters are separate packages — a wrong guess spawns an interactive TUI on
our pipe); a built-in agent table (the registry is that table and updates
hourly).

### 3.2 Spawning

`spawn_subagent` gains one field:

```rust
struct SpawnSubagentArgs {
    task: String,
    name: Option<String>,
    /// An external harness from ~/.wth/acp/agents.yaml; default: a wth subagent.
    #[serde(default)]
    agent: Option<String>,
    /// Absolute working directory for an external harness; default: wth's cwd.
    #[serde(default)]
    cwd: Option<PathBuf>,
}
```

The tool description appends the configured names and descriptions
(`External harnesses: claude — …; codex — …`) so the model can choose without
a second call. `PeerContext` (§2.5 of doc 01) adds the same line under
`## Subagents`. Unknown `agent` → tool error listing the names.

`SpawnChild` grows `agent: Option<String>` and `cwd: Option<PathBuf>`. For
`Some`, the supervisor creates the session row exactly as today (name
uniquified within the parent, `SubagentSpawned` published, permit acquired,
runtime timer armed) but spawns an `AcpChild` actor instead of a `Session`:

```rust
/// One external harness process bound to one wth session (crates/acp).
pub struct AcpChild { … }
impl Actor for AcpChild {
    /// Spawns the process, runs `initialize`, `session/new`; fails the actor
    /// (and the spawn) if either fails, with the ACP error text.
    type Args = (SessionId, AcpAgentConfig, PathBuf /* cwd */, ChildSink);
}
impl Message<UserInput> for AcpChild  // → session/prompt; a second input while a
                                      //   prompt is in flight is queued (§3.5)
impl Message<Cancel>    for AcpChild  // → session/cancel
```

`ChildSink` is what the child needs from the harness: `publish(AgentEvent)`
and `approve(ApprovalRequested) -> Decision`. Both are one `Runtime` handle
and one `ActorRef<Approval>`; the crate defines the trait so it has no
dependency on `agent` or `tools` (mirrors `cursor`, which takes a token and
returns genai events).

`Live` keeps `actor: ActorRef<Session>`; it becomes an enum
`Live::Native(ActorRef<Session>) | Live::Acp(ActorRef<AcpChild>)`. The
supervisor's `input()` / `Cancel` / `Steer` dispatch on it. Every other
supervisor path keys on `SessionId` and does not care.

Process lifetime = actor lifetime. The process stays up while the child is
idle so a follow-up `send_message` is a second `session/prompt` on the same
ACP session (context intact, no re-spawn). It dies with the actor: parent
`Cancel` (fan-out already walks children), `shutdown`, or `AcpChild` failing.
No idle TTL: a harness process at rest costs memory, not tokens; add when a
transcript shows thirty idle children.

`--resume` of a parent whose ACP child exists in sqlite but not in memory:
the child is listed in the roster as `idle (process gone)`; `send_message`
to it respawns the process and, when the agent advertised `loadSession`,
calls `session/load {sessionId: <stored>}`; otherwise `session/new` and the
child starts fresh with the message (the parent is told so in the tool
output). The ACP session id is stored at spawn (§3.6) because it costs one
column now and makes this a one-call upgrade.

### 3.3 Event translation

`AcpChild` turns `session/update` into `AgentEvent`s on its wth `SessionId`,
so the TUI, RPC subscribers and the supervisor see a normal session.

| `session/update` | `AgentEvent` |
|---|---|
| (prompt sent) | `TurnStarted { text: task, model: "<agent>" }` — `model` shows the harness name; the real model is the harness's business |
| `agent_message_chunk {content: text}` | `TextDelta` |
| `agent_thought_chunk` | `ReasoningDelta` |
| `tool_call {toolCallId, title, kind, rawInput, status}` | `ToolCallStarted { call_id, name: title, args: rawInput ∪ {"kind": kind} }` |
| `tool_call_update {status: completed \| failed, content, rawOutput}` | `ToolCallFinished { output: rendered content, is_error: status == failed }` |
| `tool_call_update` with other statuses / partial content | dropped (UI already shows the pending call) |
| `plan {entries}` | `TextDelta` of a rendered checklist, once per change — no new event type until a UI wants to render plans specially |
| `usage_update {used, size}` | remembered; emitted in `TurnFinished.context = used` |
| `user_message_chunk` (only during `session/load` replay) | ignored on live turns; on replay, persisted as history (§3.6) |
| `available_commands_update`, `current_mode_update`, `config_option_update`, `session_info_update` | ignored |
| `session/prompt` response `{stopReason}` | `TurnFinished { stop_reason, usage: {0,0} unless usage_update seen, context }` |
| transport EOF / process exit mid-turn | `Error { message }` then `TurnFinished { stop_reason: "error" }` |

Content rendering: `content` blocks → their text (images: `[image]`,
resources: the `uri`); `diff` → a unified diff of `oldText`→`newText`
headed by `path`; `terminal {terminalId}` → `[terminal <id>]` (we never
create terminals, so this should not occur). Same function feeds
`ToolCallFinished.output`, so the existing tool card renders it.

Stop reasons pass through verbatim (`end_turn`, `max_tokens`,
`max_turn_requests`, `refusal`, `cancelled`); `child_finished` already
forwards the string into `<subagent_result stop="…">`.

### 3.4 Permissions

`session/request_permission` is the inverse of our `ApprovalRequested`, and
routes through the same actor so the human sees one modal, `/approvals off`
applies, and `tools.approval_timeout_secs` bounds the wait:

```
request_permission {toolCall: {toolCallId, title, kind, rawInput}, options}
  → Approval.ask(Request { session: child, tool: title, args: rawInput, risk: risk(kind) })
  → ApprovalResolved { decision }
  → decision Allow  ⇒ outcome selected{ first option with kind allow_once, else allow_always }
    decision Deny   ⇒ outcome selected{ first option with kind reject_once, else reject_always }
    turn cancelled  ⇒ outcome cancelled   (spec: MUST, when session/cancel was sent)
```

`risk(kind)`: `read`/`search`/`fetch`/`think` → `ReadOnly`; `edit`/`move`/`execute`/`other` → `Mutating`; `delete` → `Destructive`. This is only the label the modal shows and the class `tools.policy` consults when `approvals` is on; the harness has already decided the call needs a human, so wth never auto-allows a request by policy alone — except `approvals: false` (session or global), which answers `allow_once` immediately, matching what it means for native tools.

`allow_always` is never chosen on the human's behalf: wth's approval is
per-call, and "always" would be remembered by the harness, outside our
config. The handler must not await inside the dispatch loop (SDK §1):
it `cx.spawn`s the `Approval` round-trip and responds from the task.

### 3.5 Results, messaging, cancel, limits

Unchanged from doc 01 §2.3–2.6, by construction: `TurnFinished` on the child
session drives `child_finished`, which delivers `<subagent_result>` to the
parent or resolves an `await_reply` waiter. What differs:

- **Final text.** Native children read it from the store. `AcpChild`
  accumulates `agent_message_chunk` text for the turn and persists it as one
  assistant message at `TurnFinished` (§3.6), so the same store read works.
- **`send_message{to: <acp child>}`** → `UserInput` → `session/prompt`. If a
  prompt is in flight the input is queued and sent when the response arrives
  (ACP has no mid-turn steering: a turn is one `session/prompt` request and
  its response). `MessageDelivered` is published when it is actually sent,
  so the transcript shows the real order.
- **Child → parent.** The harness has no `send_message` tool, so an ACP child
  cannot ask the parent; its turn either finishes or times out. Upgrade
  (§3.8): pass a wth MCP server in `session/new.mcpServers` that exposes
  `send_message`.
- **Cancel** → `session/cancel`; the prompt response then arrives with
  `cancelled` and the normal `TurnFinished` path runs. If no response within
  5 s the process is killed and `TurnFinished{stop_reason: "cancelled"}` is
  synthesised.
- **Limits.** `max_concurrent`, `max_runtime_secs` (→ `Cancel` on the child)
  and `reply_timeout_secs` apply as-is. `max_depth`: ACP children are leaves
  (`depth(child) = depth(parent)+1` from the store, and they have no
  `spawn_subagent`), so the existing check covers them.

### 3.6 Store, events, UI

`sessions` gains two nullable columns: `agent TEXT` (the `agents.yaml` key)
and `acp_session TEXT` (the harness's session id). Messages persisted per
turn: the user text (task / delivered message with its wrapper, as today)
and one assistant text message; tool calls are not persisted (they stream
live and `load_session` shows text only — the harness keeps its own full
transcript). On `session/load` replay, `user_message_chunk` /
`agent_message_chunk` are written the same way if the local history is
empty.

`SessionSummary` gains `agent: Option<String>`. `AgentEvent` gains nothing:
`SubagentSpawned` already names the child; `TurnStarted.model` carries the
harness name for the header. The TUI roster shows `codex-fix  running
(codex)`; `/focus` and approval modals work because they key on `SessionId`.
RPC: no new methods beyond `list_acp` / `add_acp` / `remove_acp` (the
`/acp` menu over the wire, same shape as `list_mcp` / `add_mcp_server` /
`remove_mcp_server`).

### 3.7 Authentication

`initialize` returns `authMethods[]`, each `{id, name, description, type?}`;
`session/new` fails with an `auth_required` error when one is needed. Two
method types exist (spec `authentication.md`):

1. `agent` (default when `type` is absent) → call `authenticate {methodId}`
   once and retry `session/new`. The harness runs its own flow (browser,
   device code); it may print a URL to stderr, so the child's stderr goes to
   `logs/acp-<name>.log` and the tool error quotes its last lines.
2. `terminal` (`{args, env}` appended to *our* configured command; only
   advertised if we set `clientCapabilities.auth.terminal: true`, which we
   do) → the spec wants the client to run that invocation in an interactive
   terminal, wait for exit 0, then reconnect. iocraft owns our terminal, so
   v0 does the honest subset: the spawn fails with a tool error and an
   `Error` event naming the exact command — `codex: not logged in — run
   \`codex-acp --login\` in a terminal, then retry` — and `/acp` shows the
   same line next to the entry. Running it in a suspended-TUI shell (like
   `git` drops into `$EDITOR`) is the upgrade if this proves annoying.

Keys the harness reads from the environment are the entry's `env` or wth's
own environment; the child inherits both. wth does not proxy harness logins:
its `/login` (doc 05) is for providers wth calls directly, and a harness's
subscription is already on disk once the user has used the harness.

### 3.8 Deliberately out of scope

- **Client `fs`/`terminal` capabilities.** We advertise none: the harness
  uses its own tools, which is what the user runs it for. Advertising `fs`
  would route every edit through wth for unsaved-buffer awareness we do not
  have. Add when an editor-integrated wth needs it (§4.4).
- **MCP back-channel** (`session/new.mcpServers` pointing at a wth-hosted
  MCP server exposing `send_message`, skills, the shared approval gate): the
  right way to give children a voice and wth's tools. Needs an MCP *server*
  in wth (`rmcp` `server` feature) reachable from a subprocess — a stdio
  shim `wth mcp --session <id>` proxying to the daemon over RPC. One
  transcript showing a stuck child that should have asked a question
  triggers it.
- **Forwarding `~/.wth/mcp/*.json` to children.** Same mechanism
  (`mcpServers`), zero code beyond a mapping — but doubles every MCP server
  process per child and bypasses our gates. Off by default; `agents.yaml`
  entry flag `mcp: inherit` when wanted.
- **`session/list` / registry-driven install of `binary` distributions on
  Windows** (no Windows build of wth).
- **ACP proxies/conductor**: wth as a proxy that adds its skills and tools
  to any agent for any editor. Attractive, orthogonal; separate doc if ever.

## 4. Agent: `wth --acp`

`wth --acp` is `--headless` with ACP on stdio instead of jsonrpsee on a
socket: a full wth (plugins, sqlite, MCP servers, approvals) whose only
frontend is the ACP client that spawned it. Zed config, for example:
`"agent_servers": {"wth": {"command": "wth", "args": ["--acp"]}}`.

### 4.1 Mapping

Same "registry lookup + `ask`" glue as `rpc/src/api.rs`; the bridge already
serialises every wire-visible event.

| ACP | wth |
|---|---|
| `initialize` | `protocolVersion: 1`, `agentCapabilities: {loadSession: true, promptCapabilities: {image: false, embeddedContext: true}}`, `authMethods: []` (wth authenticates providers itself; a missing key surfaces as an `Error` update), `agentInfo: {name: "wth", version}` |
| `session/new {cwd}` | `Agents.session(None)`; `cwd` is recorded on the session (new nullable column) and prepended to the system prompt as the working directory — wth's own tools already run relative to the process cwd, which the client set when spawning us |
| `session/load {sessionId}` | `LoadSession`; replay `Loaded.history` as `user_message_chunk` / `agent_message_chunk` / `tool_call`+`tool_call_update` (completed), then respond |
| `session/list` | `ListSessions` |
| `session/prompt {prompt}` | text blocks and `resource` blocks (embedded context → fenced with the uri) joined → `SendInput`; the handler `cx.spawn`s a task that awaits this session's `TurnFinished` on a `broadcast::Receiver` and responds `{stopReason}`; `"cancelled"`/`"error"` map to `cancelled`/`end_turn` + an `Error` update |
| `session/cancel` | `Cancel` |
| `session/update` ← `AgentEvent` | `TextDelta`→`agent_message_chunk`, `ReasoningDelta`→`agent_thought_chunk`, `ToolCallStarted`→`tool_call {title: name, kind: kind(name, risk), rawInput: args, status: in_progress}`, `ToolCallFinished`→`tool_call_update {status, content: [text]}`, `TurnFinished`→`usage_update {used: context, size: catalog context window}`, `MessageDelivered`→`user_message_chunk` (peer traffic is visible in editors too), `SubagentSpawned`/`ModelChanged`→ ignored |
| `session/request_permission` ← `ApprovalRequested` | options `[allow_once "Allow", reject_once "Deny"]`; `selected` → `Resolve {decision}`; `cancelled` → `Resolve Deny` |
| `session/set_config_option` / `config_option_update` | `model_config` category listing `ListModels`; select → `SetModel`. v1, not v0 |
| `available_commands_update` | none in v0; wth's slash commands are UI, not model input |
| `_wth/steer`, `_wth/list_children`, … | extension methods, only if a client wants them; the jsonrpsee surface stays for the TUI and config menus |

`kind(name, risk)`: `read_file`/`glob`/`grep`→`read`/`search`,
`write_file`/`edit`→`edit`, `shell`→`execute`, `spawn_subagent`/`send_message`→`other`,
MCP tools → by risk (`ReadOnly`→`other`, else `execute`). Labels only.

### 4.2 One process per client

ACP clients spawn the agent. Two editors → two `wth --acp` processes on one
`~/.wth`: sqlite is fine (WAL, one writer at a time per statement; sqlx's
busy timeout covers it), MCP servers start twice (their cost), `token` and
the RPC port are not used. A TUI running alongside sees the ACP sessions in
`/sessions` after the fact but not live. Acceptable for v0; the fix is the
same stdio shim as §3.8's MCP back-channel (`wth --acp` connecting to a
running daemon over jsonrpsee instead of hosting plugins), and the
Transports WG's Streamable HTTP/WebSocket RFD may make the shim unnecessary.

### 4.3 What the client sees that the TUI does not

Nothing wth-specific: no `/model`, `/mcp`, `/login`, `/skills`. Those stay
in the TUI or over jsonrpsee. A user of wth-in-Zed configures wth in the
terminal once; the editor is for turns. Config options (§4.1 `model_config`)
close the most common gap.

### 4.4 Later

`fs.readTextFile`/`writeTextFile` from the client when advertised (unsaved
buffers), `plan` updates from a todo tool, `usage_update.cost` from the
catalog's pricing, `session/close`, message ids.

## 5. Crates and dependencies

```
crates/acp/          client half: AcpChild actor, agents.yaml loader, registry
                     fetch/install, ChildSink trait. Deps: agent-client-protocol,
                     kameo, tokio, serde, reqwest (registry), sha2, runtime (SessionId).
                     No dependency on agent or tools (like cursor).
crates/agent/        Live::Acp, SpawnChild.agent/cwd, store columns, PeerContext line,
                     ChildSink impl (Runtime + ActorRef<Approval>).
crates/rpc/src/acp.rs   agent half: Agent.builder() handlers over the same lookups
                     as api.rs and the same bridge receiver. Deps: agent-client-protocol.
crates/wth/          --acp flag: plugin list = everything but tui and rpc-ws; run
                     the ACP connection on stdio until EOF, then shutdown.
```

`agent-client-protocol = "2"` is the only new workspace dependency (its own
tree: `serde`, `futures`, `tokio` — nothing new transitively of note; verify
with `cargo tree -d` at implementation). The SDK's handler futures may be
`!Send`; if so each connection runs on `tokio_util::task::LocalPoolHandle`
(`tokio-util` `rt` feature is already enabled) and talks to actors via
`ActorRef`s, which are `Send`.

## 6. Implementation order

Each step leaves `wth` runnable.

1. `acp` crate: `agents.yaml` loader + watcher, `AcpChild` with
   `initialize` → `session/new` → one `session/prompt`, event translation
   (§3.3), permission routing (§3.4), cancel. Check: a throwaway `main`
   spawns `codex-acp` (or `npx -y @agentclientprotocol/claude-agent-acp`),
   prints `AgentEvent`s for "list the files here", answers one
   `request_permission` from stdin.
2. `agent`: `Live` enum, `SpawnChild.agent`, columns, `spawn_subagent.agent`
   + description line, `PeerContext` line, `SessionSummary.agent`. Check
   (headless over RPC): parent `spawn_subagent{agent: "codex", task}` →
   `SubagentSpawned`, child `TextDelta`s stream, an `ApprovalRequested`
   for a shell command appears and `resolve_approval` lets it run,
   `TurnFinished`, parent runs a `<subagent_result>` turn;
   `send_message{to: child}` produces a second child turn on the same ACP
   session; parent `cancel` kills the child process (no orphan in `ps`).
3. `/acp` menu, registry fetch, `/acp add <id>` for `npx`/`uvx`/`binary`.
   Check: `/acp add codex` on a machine without the entry writes
   `agents.yaml` and step-2's flow works from that entry.
4. `wth --acp`: `initialize`, `session/new`, `session/prompt` with streaming
   updates, `request_permission`, `session/cancel`, `session/load`. Check:
   Zed with `agent_servers.wth` → a turn streams, a `shell` call prompts in
   Zed's permission UI, Deny makes the model see the error; `acpx` (CLI
   client) runs the same script headlessly for the regression check.
5. Docs: `00-architecture.md` §7 and §9 point here; README config block;
   `01-config-and-subagents.md` §2.4 notes the `agent` field.

## 7. Risks

- **SDK churn.** 2.x is a rewrite (SACP); v2 protocol draft in flight. Pin
  `=2.1` and use only the v1 schema module; the wire protocol is stable,
  the Rust API is what may move.
- **Harness behaviour under a non-editor client.** Some adapters assume
  `fs` capabilities or a TTY for login. Mitigation: §3.7 gives the user the
  exact command; `/acp`'s probe shows which entries actually work before a
  model tries one.
- **Permission semantics.** A harness with its own "always allow" state
  will stop asking; wth cannot see those calls beyond `tool_call` updates.
  Acceptable — the user chose that harness and its policy — and stated in
  the `/acp` entry line (`approvals: the harness's own; wth relays what it
  asks`).
- **Cost visibility.** External turns bill the harness's account;
  `usage_update` is optional and often absent, so `TurnFinished.usage` is
  zero for ACP children. The header shows the harness name, not a model, so
  the gap is visible rather than misleading.
