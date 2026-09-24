# Agent Client Protocol: external harnesses as subagents, theuth as an agent

Status: as-built v0 · 2026-09-12 (draft v0 2026-09-09) · extends `00-architecture.md` §5, §7; `01-config-and-subagents.md` §2. §8 lists what the build changed against the draft; where §8 disagrees with §1-§7, §8 wins. §8.1 (2026-09-18) records the boundary with agent communication (50); its integration is implemented but not yet verified against a running harness.

Verified 2026-09-12. Client half (headless, real models; harness `opencode acp` 1.18.20 from `agents.yaml`, parent claude-sonnet-4-5): `spawn_subagent{agent:"opencode"}` → `SubagentSpawned`, child `TurnStarted{model:"opencode"}`, `ToolCallStarted bash`, `ToolCallFinished`, `TextDelta "ACP-OK-17"`, `TurnFinished end_turn`, parent turn with `<subagent_result stop="end_turn">ACP-OK-17</subagent_result>`; `send_message` with `await_reply` → second `session/prompt` on the same ACP session, answered from the child's context, `MessageDelivered` published; an entry with `OPENCODE_CONFIG` permission `bash: ask` → `ApprovalRequested{tool:"echo PERM-OK-5", risk: mutating, args:{command, kind:"execute"}}` on the child session, `resolve_approval allow` ran it, `deny` gave the harness `The user rejected permission to use this specific tool call.` and a silent turn end (result falls back to `[no reply; last tool result]`, §8.17); `cancel` on a child running `sleep 120` → `session/cancel` → `TurnFinished cancelled`, `<subagent_result stop="cancelled">`, process stays (idle child); RPC `shutdown` → zero `opencode acp` processes; restart + `send_message` to a store-only child respawned and `session/load`ed (harness recalled its first turn); unknown agent → tool error `unknown agent \`nope\`; configured: opencode, opencode-ask`; `list_acp(probe:true)` → `ok` for both; `command: /nonexistent/harness` → child `Error "spawn failed: Internal error: No such file or directory (os error 2)"` + `TurnFinished error`, parent `<subagent_result stop="error">` with that text.
Registry: `list_acp_registry` → 40 agents; `add_acp("codex-acp")` wrote `codex-acp: {command: npx, args: [-y, "@agentclientprotocol/codex-acp@1.11.0"], description: "ACP adapter for OpenAI's coding assistant"}`; `spawn_subagent{agent:"codex-acp"}` streamed `CODEX-OK` and delivered the result (codex login already on disk); `remove_acp` removed entries.
Agent half (`t --acp` driven by a throwaway Bun stdio client): `initialize` → `{protocolVersion:1, agentCapabilities:{loadSession:true, promptCapabilities:{embeddedContext:true}}, authMethods:[], agentInfo:{name:"theuth", version}}`; `session/new` → theuth session id; `session/prompt` streamed `tool_call{title:"shell", kind:"execute", in_progress}`, `session/request_permission` with `allow_once`/`reject_once`, `tool_call_update completed` with the shell output, `agent_message_chunk`, `usage_update`, `{stopReason:"end_turn"}`; `reject_once` → `tool_call_update failed "shell: denied by user"` and the model explained the denial; `session/cancel` during `sleep 30` → `{stopReason:"cancelled"}`; `session/load` replayed `user_message_chunk`, `tool_call`, `agent_message_chunk` and a follow-up prompt answered from the replayed history; `session/list` returned the session with `cwd` and title; stdin EOF → exit 0, no leftover process.
TUI on a pty: `/acp` opened the menu (entries, blank, registry rows `+ id  name version  description`); `p` showed `probing…` then `ok`.
Not exercised: `binary` distribution install (download/sha256/tar|unzip), the `terminal`-type auth message, Zed/acpx as clients (not installed), `session/set_config_option`.

[ACP](https://agentclientprotocol.com) is JSON-RPC 2.0 between an **Agent**
(an LLM harness: Claude Code, Codex, Gemini CLI, OpenCode, Goose, Copilot,
Cursor, Kiro, …) and a **Client** (an editor or any other UI). One crate,
[`agent-client-protocol`](https://docs.rs/agent-client-protocol) 2.1.0,
implements both roles. theuth wants both, in this order of value:

1. **Client** — `spawn_subagent{agent: "codex"}` starts an installed harness
   as a child session. The parent, the roster, the approval modal, result
   delivery and peer messaging are the ones doc 01 §2 already has; only the
   thing running the turn changes. Every harness on the machine becomes a
   worker with its own model access, tools and subscription.
2. **Agent** — `t --acp` speaks ACP on stdio, so Zed, JetBrains, neovim,
   Emacs, VS Code, Obsidian, mobile and chat bridges (the
   [client list](https://agentclientprotocol.com/get-started/clients)) are
   theuth frontends without writing one. This replaces the "Tauri later" plan in
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
theuth gets back is a finished task, a transcript, and a stop reason — exactly
what `<subagent_result>` already carries. Modelling it as a session-shaped
peer keeps every existing surface (`/agents` picker, approvals, `send_message`,
`list_agents`, RPC events) working unchanged; modelling it as a model would
force theuth's loop to drive a harness that already has one.

## 3. Client: external harnesses as subagents

### 3.1 Which harnesses exist

`~/.theuth/acp/agents.yaml`, one entry per runnable harness:

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
uvx, args: [<package>, ...args]`; `binary` → the `cmd`'s basename on `PATH`
when the user already has it (the daemon runs under the login shell, doc 18),
else download the current platform's archive into `~/.theuth/acp/bin/<id>/`,
verify `sha256`, extract, `command:` the extracted `cmd`. `/acp remove <name>`
deletes the entry (and `bin/<id>` if
present). The registry JSON is fetched on first `/acp` and re-fetched when
older than a day (`~/.theuth/acp/registry.json`); offline shows the cached one.
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
    /// An external harness from ~/.theuth/acp/agents.yaml; default: a theuth subagent.
    #[serde(default)]
    agent: Option<String>,
    /// Absolute working directory for an external harness; default: theuth's cwd.
    #[serde(default)]
    cwd: Option<PathBuf>,
}
```

The tool description appends the configured names and descriptions
(`External harnesses: claude — …; codex — …`) so the model can choose without
a second call. `PeerContext` (§2.5 of doc 01) adds the same line under
`## Subagents`. Unknown `agent` → tool error listing the names. Since 32
§6.5 the same field also names a paired *node*: the supervisor resolves
`agents.yaml` first, then the nodes `RegisterNode` registered, and the
error lists both.

`SpawnChild` grows `agent: Option<String>` and `cwd: Option<PathBuf>`. For
`Some`, the supervisor creates the session row exactly as today (name
uniquified within the parent, `SubagentSpawned` published, permit acquired,
runtime timer armed) but spawns an `AcpChild` actor instead of a `Session`:

```rust
/// One external harness process bound to one theuth session (crates/acp).
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

As built for 32 §6.5, `Connect.config: AcpAgentConfig` became
`Connect.transport: Transport`, `Transport::{Process(AcpAgentConfig),
Dial(Arc<dyn Dialer>)}`; a `Dialer` opens one `LineDuplex` (one JSON-RPC
message per `String`, no line framing) per connection, and the private
`Link::spawn` under `Harness::connect` either spawns the process or dials.
`probe(Transport, stderr_log)` takes the same enum. `AcpChild` carries the
`Transport`, so a remote child is the same actor with a different first hop.

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

`AcpChild` turns `session/update` into `AgentEvent`s on its theuth `SessionId`,
so the TUI, RPC subscribers and the supervisor see a normal session.

| `session/update` | `AgentEvent` |
|---|---|
| (prompt sent) | `TurnStarted { text: task, model: "<agent>" }` — `model` shows the harness name; the real model is the harness's business |
| `agent_message_chunk {content: text}` | `TextDelta` |
| `agent_thought_chunk` | `ReasoningDelta` |
| `tool_call {toolCallId, title, kind, rawInput, status}` | `ToolCallStarted { call_id, name: title, args: rawInput ∪ {"kind": kind} }` |
| `tool_call_update {status: completed \| failed, content, rawOutput}` | `ToolCallFinished { output: rendered content, is_error: status == failed }`; the call and its result are also persisted as chat messages (§3.6) |
| `tool_call_update` with other statuses / partial content | dropped (UI already shows the pending call) |
| `plan {entries}` | `TextDelta` of a rendered checklist, once per change — no new event type until a UI wants to render plans specially |
| `usage_update {used, size}` | remembered; emitted in `TurnFinished.context = used` |
| `user_message_chunk` (only during `session/load` replay) | ignored (§8 rule 11: the local history already holds the turn) |
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

`risk(kind)`: `read`/`search`/`fetch`/`think` → `ReadOnly`; `edit`/`move`/`execute`/`other` → `Mutating`; `delete` → `Destructive`. This is only the label the modal shows and the class `tools.policy` consults when `approvals` is on; the harness has already decided the call needs a human, so theuth never auto-allows a request by policy alone — except `approvals: false` (session or global), which answers `allow_once` immediately, matching what it means for native tools.

`allow_always` is never chosen on the human's behalf: theuth's approval is
per-call, and "always" would be remembered by the harness, outside our
config. The handler must not await inside the dispatch loop (SDK §1):
it `cx.spawn`s the `Approval` round-trip and responds from the task.

### 3.5 Results, messaging, cancel, limits

Unchanged from doc 01 §2.3–2.6, by construction: `TurnFinished` on the child
session drives `child_finished`, which delivers `<subagent_result>` to the
parent or resolves an `await_reply` waiter. What differs:

- **Final text.** Native children read it from the store. `AcpChild`
  accumulates `agent_message_chunk` text for the turn and reports it whole
  as `PeerTurnFinished.text` (50 §6.3); the store holds the same text split
  into segments around the turn's tool calls (§3.6), so nothing re-reads it
  to build `<subagent_result>`.
- **`send_message{to: <acp child>}`** → `UserInput` → `session/prompt`. If a
  prompt is in flight the input is queued and sent when the response arrives
  (ACP has no mid-turn steering: a turn is one `session/prompt` request and
  its response). `MessageDelivered` is published when it is actually sent,
  so the transcript shows the real order.
- **Child → parent.** The harness has no `send_message` tool, so an ACP child
  cannot ask the parent; its turn either finishes or times out. Upgrade
  (§3.8): pass a theuth MCP server in `session/new.mcpServers` that exposes
  `send_message`. 50 does not change this (§8.1): A2A gives *independently
  operated* agents a channel, not an ACP child a reverse tool.
- **Cancel** → `session/cancel`; the prompt response then arrives with
  `cancelled` and the normal `TurnFinished` path runs. If no response within
  5 s the process is killed and `TurnFinished{stop_reason: "cancelled"}` is
  synthesised. A harness that answers the cancel but keeps working, refuses
  the next prompt as busy, or asks permission with no turn open: §8.3
  (2026-09-24).
- **Limits.** `max_concurrent`, `max_runtime_secs` (→ `Cancel` on the child)
  and `reply_timeout_secs` apply as-is. `max_depth`: ACP children are leaves
  (`depth(child) = depth(parent)+1` from the store, and they have no
  `spawn_subagent`), so the existing check covers them.

### 3.6 Store, events, UI

`sessions` gains two nullable columns: `agent TEXT` (the `agents.yaml` key)
and `acp_session TEXT` (the harness's session id). Messages persisted per
turn (as built 2026-09-24; before, tool calls were not persisted and
`load_session` showed text only): the user text (task / delivered message
with its wrapper, as today), then the harness's text and tool calls as
ordinary chat messages in arrival order, the shapes `history()` already
pairs — a `tool_call` first flushes the accumulated text as an assistant
message, then persists an assistant message carrying the call
(`call_id = toolCallId`, `fn_name = title`, `fn_arguments = rawInput ∪
{kind}`, the `ToolCallStarted` fields); its terminal `tool_call_update` (or
a `tool_call` reported finished in one message) persists a tool message
with the rendered output, capped like the live card (2000 chars) and
`ERROR: `-prefixed when `failed`. At the end of the turn the trailing text
is persisted (or the error / `[no reply; last tool result]` / `(no text)`
fallback when no text arrived); a call the harness never reported finished
(cancel, error, process death) gets `ERROR: not finished: the turn ended
before the harness reported this call`, so no card stays pending on reload;
a turn whose last row is a tool result closes with an empty assistant row so
the next user message still opens a turn. Live events are unchanged; every
client reloads the transcript with its tool cards through `load_session`.
A finish for a call never started is not stored (it would be a dangling
card). The harness keeps its own full transcript; the rows here are for
display and search, never sent to a model. On `session/load` replay,
updates arrive with no turn open and are dropped (§8 rule 11).

`SessionSummary` gains `agent: Option<String>`. `AgentEvent` gains nothing:
`SubagentSpawned` already names the child; `TurnStarted.model` carries the
harness name for the header (superseded by §8.2: the harness's own model is
discovered and shown beside it). ACP children appear in the `/agents` picker and
board (19) with `running`/`idle`/`approval` only — never `awaiting`, since
they have no `send_message` (§3.8); opening one and approval modals work
because they key on `SessionId`.
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

Keys the harness reads from the environment are the entry's `env` or theuth's
own environment; the child inherits both. theuth does not proxy harness logins:
its `/login` (doc 05) is for providers theuth calls directly, and a harness's
subscription is already on disk once the user has used the harness.

### 3.8 Deliberately out of scope

- **Client `fs`/`terminal` capabilities.** We advertise none: the harness
  uses its own tools, which is what the user runs it for. Advertising `fs`
  would route every edit through theuth for unsaved-buffer awareness we do not
  have. Add when an editor-integrated theuth needs it (§4.4).
- **MCP back-channel** (`session/new.mcpServers` pointing at a theuth-hosted
  MCP server exposing `send_message`, skills, the shared approval gate): the
  right way to give children a voice and theuth's tools. Needs an MCP *server*
  in theuth (`rmcp` `server` feature) reachable from a subprocess — a stdio
  shim `theuth mcp --session <id>` proxying to the daemon over RPC. One
  transcript showing a stuck child that should have asked a question
  triggers it. (A remote child on a node, 32, is in the same position: it
  cannot ask its parent either.)
- **Forwarding `~/.theuth/mcp/*.json` to children.** Same mechanism
  (`mcpServers`), zero code beyond a mapping — but doubles every MCP server
  process per child and bypasses our gates. Off by default; `agents.yaml`
  entry flag `mcp: inherit` when wanted.
- **`session/list` / registry-driven install of `binary` distributions on
  Windows** (no Windows build of theuth).
- **ACP proxies/conductor**: theuth as a proxy that adds its skills and tools
  to any agent for any editor. Attractive, orthogonal; separate doc if ever.

## 4. Agent: `t --acp`

`t --acp` is `--headless` with ACP on stdio instead of jsonrpsee on a
socket: a full theuth (plugins, sqlite, MCP servers, approvals) whose only
frontend is the ACP client that spawned it. Zed config, for example:
`"agent_servers": {"theuth": {"command": "t", "args": ["--acp"]}}`.

### 4.1 Mapping

Same "registry lookup + `ask`" glue as `rpc/src/api.rs`; the bridge already
serialises every wire-visible event.

| ACP | theuth |
|---|---|
| `initialize` | `protocolVersion: 1`, `agentCapabilities: {loadSession: true, promptCapabilities: {image: false, embeddedContext: true}}`, `authMethods: []` (theuth authenticates providers itself; a missing key surfaces as an `Error` update), `agentInfo: {name: "theuth", version}` |
| `session/new {cwd}` | `Agents.session(None)`; `cwd` is recorded on the session (new nullable column) and prepended to the system prompt as the working directory — theuth's own tools already run relative to the process cwd, which the client set when spawning us |
| `session/load {sessionId}` | `LoadSession`; replay `Loaded.history` as `user_message_chunk` / `agent_message_chunk` / `tool_call`+`tool_call_update` (completed), then respond |
| `session/list` | `ListSessions` |
| `session/prompt {prompt}` | text blocks and `resource` blocks (embedded context → fenced with the uri) joined → `SendInput`; the handler `cx.spawn`s a task that awaits this session's `TurnFinished` on a `broadcast::Receiver` and responds `{stopReason}`; `"cancelled"`/`"error"` map to `cancelled`/`end_turn` + an `Error` update |
| `session/cancel` | `Cancel` |
| `session/update` ← `AgentEvent` | `TextDelta`→`agent_message_chunk`, `ReasoningDelta`→`agent_thought_chunk`, `ToolCallStarted`→`tool_call {title: name, kind: kind(name, risk), rawInput: args, status: in_progress}`, `ToolCallFinished`→`tool_call_update {status, content: [text]}`, `TurnFinished`→`usage_update {used: context, size: catalog context window}`, `MessageDelivered`→`user_message_chunk` (peer traffic is visible in editors too), `SubagentSpawned`/`ModelChanged`→ ignored |
| `session/request_permission` ← `ApprovalRequested` | options `[allow_once "Allow", reject_once "Deny"]`; `selected` → `Resolve {decision}`; `cancelled` → `Resolve Deny` |
| `session/set_config_option` / `config_option_update` | `model_config` category listing `ListModels`; select → `SetModel`. v1, not v0 |
| `available_commands_update` | none in v0; theuth's slash commands are UI, not model input |
| `_theuth/steer`, `_theuth/list_agents`, … | extension methods, only if a client wants them; the jsonrpsee surface stays for the TUI and config menus |

`kind(name, risk)`: `read_file`/`glob`/`grep`→`read`/`search`,
`write_file`/`edit`→`edit`, `shell`→`execute`, `spawn_subagent`/`send_message`→`other`,
MCP tools → by risk (`ReadOnly`→`other`, else `execute`). Labels only.

### 4.2 One process per client

ACP clients spawn the agent. Two editors → two `t --acp` processes on one
`~/.theuth`: sqlite is fine (WAL, one writer at a time per statement; sqlx's
busy timeout covers it), MCP servers start twice (their cost), `token` and
the RPC port are not used. A TUI running alongside sees the ACP sessions in
`/sessions` after the fact but not live. Acceptable for v0; the fix is the
same stdio shim as §3.8's MCP back-channel (`t --acp` connecting to a
running daemon over jsonrpsee instead of hosting plugins), and the
Transports WG's Streamable HTTP/WebSocket RFD may make the shim unnecessary.
The in-daemon ACP server over a link mode-1 conn (32 §4.3;
`rpc::acp::serve_records`, one `Relay` per process with per-connection
session ownership) is the shim this paragraph anticipated, without a
shim: the running daemon serves ACP itself and the sessions are its own.
`t --acp` on stdio stays for editors.

### 4.3 What the client sees that the TUI does not

Nothing theuth-specific: no `/model`, `/mcp`, `/login`, `/skills`. Those stay
in the TUI or over jsonrpsee. A user of theuth-in-Zed configures theuth in the
terminal once; the editor is for turns. Config options (§4.1 `model_config`)
close the most common gap.

### 4.4 Later

`fs.readTextFile`/`writeTextFile` from the client when advertised (unsaved
buffers), `plan` updates from a todo tool, `usage_update.cost` from the
catalog's pricing, `session/close`, message ids.

## 5. Crates and dependencies (as built)

```
crates/acp/          library, no dependency on agent or tools (like cursor):
                       agents.rs   agents.yaml loader, describe() (tool-description line)
                       registry.rs registry fetch/cache, install (npx/uvx/binary), remove_bin
                       harness.rs  Harness: connect (Transport::{Process, Dial} → initialize → auth → session/new|load),
                                   prompt, cancel, close; probe(Transport); Dialer, LineDuplex (32 §6.5)
                       render.rs   content/diff renderers, risk(kind), kind(name, risk)
crates/agent/        acp_child.rs: AcpChild actor (session/update → AgentEvent, permissions,
                       queued input, respawn + session/load); migrations/0006_acp.sql
                       (agent, acp_session, cwd); Live.actor: LiveActor::{Native, Acp};
                       SpawnChild.agent/cwd; spawn_subagent{agent, cwd} + description line
                       from acp::describe; SessionSummary.agent/cwd; Loaded.agent; Agents
                       messages NewSession, ListAcp{probe}, AddAcp{id}, RemoveAcp{name},
                       ListRegistry.
crates/tools/        ExternalApproval on ToolRegistry, sharing Invoke's decision (gate()).
crates/rpc/src/acp.rs   agent half: AcpPlugin + Relay bus actor (AgentEvent, approvals,
                       prompt waiters; one per process, "acp.relay", sessions bound to the
                       connection that made them); serve_records for link mode-1 conns (32 §4.3).
                       RPC: list_acp(probe), add_acp(id), remove_acp(name),
                       list_acp_registry(). TS bindings EntryInfo.ts, RegistryAgent.ts.
crates/theuth/          --acp flag (conflicts with headless/serve/-p/link-signal): plugins
                       tools, knowledge, agent, signal (channel off), rpc::AcpPlugin;
                       first_run creates acp/.
crates/tui/          /acp menu (Menu::Acp; keys p/⏎/d; /acp add <id>, /acp remove <name>);
                       /agents shows (agent).
```

`agent-client-protocol = "=2.1.0"` is the only new workspace dependency. Its
handlers are `Send` (no `LocalPoolHandle`, driven with `tokio::spawn`); it has
no tokio dependency and adds `async-io`, `async-process`, `blocking`,
`futures-concurrency`, `shell-words`, `rustix` transitively.

## 6. Implementation order (as executed)

Each step left `theuth` runnable.

1. `acp` crate: `agents.yaml` loader, `Harness` (`initialize` → `session/new`
   → `session/prompt`), renderers, `risk`/`kind`, `probe`. Checked through
   step 2 against `opencode acp` 1.18.20 (no throwaway `main`; the harness
   was exercised via the supervisor) and `list_acp(probe:true)` → `ok`.
2. `agent`: `LiveActor` enum, `SpawnChild.agent/cwd`, `0006_acp.sql`,
   `spawn_subagent.agent/cwd` + description line, `SessionSummary.agent/cwd`,
   `Loaded.agent`, `AcpChild`, `ExternalApproval`. Checked headless over RPC:
   `spawn_subagent{agent:"opencode"}` → `SubagentSpawned`, `TurnStarted
   {model:"opencode"}`, `ToolCallStarted bash` / `ToolCallFinished`,
   `TextDelta`, `TurnFinished end_turn`, parent `<subagent_result>`;
   `ApprovalRequested` for `echo PERM-OK-5` → allow ran it, deny ended the
   turn silently (§8.17); `send_message` → second prompt on the same ACP
   session; `cancel` → `TurnFinished cancelled`, process kept (idle child);
   `shutdown` → no `opencode acp` left; restart + `send_message` respawned
   and `session/load`ed; unknown agent and broken `command` produced the
   tool error / `<subagent_result stop="error">` respectively.
3. `/acp` menu, registry fetch, `add_acp`/`remove_acp` for `npx`/`uvx`/`binary`.
   Checked: `list_acp_registry` → 40 agents; `add_acp("codex-acp")` wrote
   the `npx` entry and `spawn_subagent{agent:"codex-acp"}` streamed
   `CODEX-OK`; `remove_acp` removed it. `binary` install not exercised live.
4. `t --acp`: `initialize`, `session/new`, `session/prompt` with streaming
   updates, `request_permission`, `session/cancel`, `session/load`,
   `session/list`. Checked with a throwaway Bun stdio client (Zed/acpx not
   installed): a turn streamed `tool_call` → `request_permission` →
   `tool_call_update completed` → `agent_message_chunk` → `usage_update` →
   `end_turn`; `reject_once` → `tool_call_update failed "shell: denied by
   user"` and the model explained the denial; cancel during `sleep 30` →
   `cancelled`; `session/load` replayed history and a follow-up answered
   from it; stdin EOF → exit 0, no leftover process.
5. Docs: this section, §3 verification paragraph, §5, §8; `00-architecture.md`
   §7 and §9 point here; README config block; `01-config-and-subagents.md`
   §2.4 notes the `agent` field.

## 7. Risks

- **SDK churn.** 2.x is a rewrite (SACP); v2 protocol draft in flight. Pin
  `=2.1` and use only the v1 schema module; the wire protocol is stable,
  the Rust API is what may move.
- **Harness behaviour under a non-editor client.** Some adapters assume
  `fs` capabilities or a TTY for login. Mitigation: §3.7 gives the user the
  exact command; `/acp`'s probe shows which entries actually work before a
  model tries one.
- **Permission semantics.** A harness with its own "always allow" state
  will stop asking; theuth cannot see those calls beyond `tool_call` updates.
  Acceptable — the user chose that harness and its policy — and stated in
  the `/acp` entry line (`approvals: the harness's own; theuth relays what it
  asks`).
- **Cost visibility.** External turns bill the harness's account;
  `usage_update` is optional and often absent, so `TurnFinished.usage` is
  zero for ACP children. The header shows the harness name, not a model, so
  the gap is visible rather than misleading.

## 8. Revisions at build time (2026-09-12)

Re-verified against `agent-client-protocol` 2.1.0 source (schema 1.7.0) and
the tree at `93fa4bd`. Each item names the draft's claim, the fact, the
decision.

1. **SDK is `Send`; no `LocalPoolHandle`** (§5). Handlers are `AsyncFnMut + Send`
   boxed into `BoxFuture`; `ConnectionTo<R>` is `Clone + Send + Sync`;
   `connect_with` is a plain `Send` future. Drive it with `tokio::spawn`.
   The SDK has no tokio dependency (`async-io`, `async-process`, `blocking`
   helper threads). Pin `=2.1.0`.
2. **Connection = closure lifetime.** The child process dies (process group
   kill) when the `connect_with` closure returns. Pattern: the closure sends
   its `ConnectionTo<Agent>` clone out over a oneshot and then awaits a
   shutdown oneshot; the actor holds the handle. `cx.spawn` tasks run inside
   that future; a task returning `Err` kills the connection, so tasks log
   and return `Ok`.
3. **Client must reject unknown session-scoped requests.** The SDK's `Agent`
   counterpart marks every unhandled message carrying `sessionId` as
   *retry* and queues it forever. A harness that ignores our empty
   `fs`/`terminal` capabilities would hang. A last catch-all
   `on_receive_request(AgentRequest → method_not_found)` is mandatory.
4. **`AcpAgentConfig` has no cwd and denies unknown fields.** `agents.yaml`
   deserialises into our own `Entry { command, args, env, description }`
   and builds the SDK config; the process inherits theuth's cwd; `cwd` goes
   only into `session/new`.
5. **No `crates/acp` actor, no `ChildSink` trait.** `AgentEvent` lives in
   `agent`; an actor in `acp` publishing it would need `agent` (cycle) or a
   mirrored event enum (two lists). So `acp` is a *library* (plugins.md §7,
   like `cursor`): `agents.yaml` loader, registry fetch/install, the
   `Harness` connection wrapper (spawn → `initialize` → auth → `session/new`
   or `session/load`, `prompt`, `cancel`, `probe`), and pure renderers
   (`render_content`, `risk(kind)`, `kind(name, risk)`). The `AcpChild`
   actor is `crates/agent/src/acp_child.rs`, with the store, the registry
   and `AgentEvent` in reach.
6. **No watcher for `agents.yaml`.** The file is read at every spawn and
   every `/acp`; "next spawn sees it" holds with zero code. The MCP watcher
   exists because servers are long-lived processes to reconcile; harnesses
   are spawned per child.
7. **Spawn failure arrives as a result, not a tool error** (§3.2 "fails the
   spawn"). The supervisor's spawn is non-blocking past row creation and
   `npx` can take ten seconds; blocking the supervisor mailbox on it is
   wrong. `spawn_subagent` returns as today; if `initialize`/`session/new`
   fails, the child publishes `Error` + `TurnFinished { stop_reason:
   "error" }`, persists the error text as its assistant message, and the
   parent receives `<subagent_result stop="error">` naming the cause
   (auth: the exact login command, §3.7). One delivery path.
8. **Permissions go through the registry's contract, not around it** (§3.4
   "never auto-allows by policy alone"). `ToolRegistry` gains
   `ExternalApproval { session, tool, args, risk } -> Decision` running the
   same `Risk -> PolicyAction` + `approvals`/bypass + `Approval.ask` as
   `Invoke`. A policy that allows `read_only` without asking allows a
   harness's read too; one contract (12 §5). `allow_always` is still never
   selected.
9. **`PeerContext` `## Subagents` line dropped** - no such block exists;
   the tool description carries the harness list and that is where the
   model reads it.
10. **Store**: `0006_acp.sql` adds `agent TEXT`, `acp_session TEXT`,
    `cwd TEXT` (the last for §4.1). `SessionSummary.agent/cwd` and
    `Loaded.agent` are added; the TUI `/agents` list shows `(agent)` after
    the status.
11. **`session/load` replay chunks are ignored**: the local history was
    persisted per turn (§3.6, text and tool calls alike), so replay would
    duplicate it. Mechanism: replay arrives before any turn is open, and
    `AcpChild` drops every `session/update` that has no turn in flight.
12. **Agent half subscribes typed events.** There is no typed broadcast;
    the RPC bridge carries JSON and only exists with `RpcPlugin`. `rpc::acp`
    registers its own kameo actor on the bus for `AgentEvent`,
    `ApprovalRequested`, `ApprovalResolved` (same shape as both bridges) and
    keeps per-session prompt waiters. The theuth `SessionId` string *is* the ACP
    session id (no table).
13. **`--acp` builds its own plugin vector**: tools, knowledge, agent,
    signal (channel off), `rpc::AcpPlugin`. Not `--headless` (which turns on
    the WebSocket server and the Signal channel). stdout belongs to ACP;
    tracing already goes to the file in every mode.
14. **Registry `binary` install shells out to `tar`/`unzip`** (archives are
    `.tar.bz2`, `.tar.gz`, `.zip`); sha256 verified before extraction.
15. **Signal gets no `/acp`** (plugins.md §4 step 4: only if the phone
    needs it). `send_message` to an ACP child works from Signal unchanged.
16. **Local check harness**: `opencode acp` (installed, 1.18.20) answers
    `initialize` with `loadSession: true` and an `agent`-type auth method;
    it is the step-1/2 smoke target. `codex-acp`/`claude-acp` via `npx` are
    registry entries for the step-3 install check.
17. **Turn with no text: the result carries the last tool output**
    (`[no reply; last tool result]`) - found in smoke: opencode ends a turn
    silently after a rejected permission.

### 8.1 Agent communication (50), 2026-09-18 — pending integration verification

What 50 decided about ACP, recorded here so §3.5/§3.8 are not read as
superseded. Nothing in this subsection has been exercised end to end; the
2026-09-12 verification above stands for the ACP paths only.

- **Same transport, no reverse tools.** `AcpChild`, `Transport::{Process,
  Dial}`, the `session/prompt` queue and the §3.3 translation are unchanged.
  50 adds no ACP method, injects nothing into `session/new.mcpServers`, and
  does not translate ACP tool calls into theuth peer tools. An ACP child still
  cannot `discover_peers`/`send_message`/`spawn_subagent`; `PeerInfo` for a
  `PeerKind::Acp` row says so (`send: true`, `reply: false` — it cannot
  answer with `reply_to`; `supervisor.rs::directory`) and it is still never
  `awaiting` (§3.6). Claiming otherwise is listed in 50 §13 as a non-goal.
- **The switch gates delegation, not the process.** `spawn_subagent{agent}`
  and `send_message` to an ACP child run under the same agent-owned
  communication policy as any peer (50 §2; `agent/src/peers.rs` gates all
  three tools on `SharedPolicy`). Communication off (global, or inherited
  from an ancestor) refuses the spawn/send with the blocker named; it does
  not kill a running child or bypass the result path — with communication
  off there is no `<message from="session:…">` result injection either.
  Enabled peer messages to an ACP child queue behind an active prompt as in
  §3.5.
- **A2A is a separate, optional plugin.** `crates/a2a` (SDK client/server,
  `a2a.enabled: false` by default; only in the default plugin vector of
  `theuth/src/main.rs`, not `--acp`'s) is not a mode of `acp`, of `t --acp`
  or of `agents.yaml`. An external harness that implements A2A itself is
  configured as an A2A peer in `<data_dir>/a2a/peers.yaml`, with its own
  credential and export grants; bidirectionality then comes from *its*
  endpoint, not from the ACP session. ACP session ids (`acp_session`,
  §8.10) and A2A context ids stay separate; an `agents.yaml` name and an
  A2A peer name are different namespaces and one entry never implies the
  other.
- **`t --acp` agent half.** Serves ACP to the editor as before. It does
  not expose A2A (no `a2a` plugin in that vector) and an ACP client's
  `session/prompt` is not a peer message: no `PeerRecord`, no
  provenance wrapper, no communication switch consulted.

Verification owed before this subsection is "as built": with communication
off, `spawn_subagent{agent: "opencode"}` is refused with the inherited/global
reason and no child process starts; with it on, the 2026-09-12 flow above
repeats unchanged and an enabled `send_message` to the child during a prompt
is queued, not dropped.

### 8.2 Harness model (2026-09-24)

§3.6 said `AgentEvent` gains nothing and the header shows the harness name.
It now shows `omp · anthropic/claude-opus-4-8`: the harness name stays where
it was (`TurnStarted.model`, `Loaded.model`, `sessions.model`) and the LLM the
harness itself runs on is discovered and carried beside it.

- **Source, in order.** (1) The session response: `NewSessionResponse` /
  `LoadSessionResponse.configOptions[]`, the option with `category:
  "model"` (schema `SessionConfigOptionCategory::Model`; `model_config` is a
  model *parameter*, not the selector), else the first select whose id or
  name contains `model`; its `currentValue` verbatim (`acp::current_model`,
  `Harness::model()`). (2) `session/update` `config_option_update` with the
  same extraction, mid-session (`/switch` in omp) — handled in the
  `Update` mailbox before the turn check, so a switch between prompts
  counts. (3) The `agents.yaml` entry's optional `model: <label>`
  (`acp::Entry::model`; a display fallback, never sent to the harness).
  (4) Nothing: the header shows the harness name alone, as before.
- **Carried as.** `sessions.harness_model TEXT` (migration
  `0021_harness_model.sql`), written by `Store::set_harness_model` once per
  distinct value; `SessionSummary.harness_model` and `Loaded.harness_model`
  (`Option<String>`, TS optional); `AgentEvent::HarnessModelChanged {session,
  model}` published after the row is written — at connect and on every
  change. `ModelChanged` is not reused: its consumers replace the shown model
  id, which for an ACP child must stay the harness name. `TurnStarted` did
  not grow a field; the event precedes the first turn and the store row
  covers a client that loads later.
- **Rendering.** Desktop `ModelMenu`: an ACP child (`agent` set) renders
  `agent · harnessModel` (or `agent`) as plain text — no picker, no effort
  menu; `set_model` on a harness is not possible from theuth. Store slice
  `harnessModel` from `Loaded` and the event. TUI `Model::header`:
  `model_id · harness_model` before the effort/ctx segments (`↳ name  child ·
  …` when a subagent); `HarnessModelChanged` after a shown value adds
  `harness model → …`, the first report is silent; the line printer prints
  `harness model → …`.
- **Probe (throwaway Bun stdio client, `initialize` → `session/new` →
  `session/prompt "reply with the single word OK"`).** `omp acp` (Oh My Pi
  18.2.9): `session/new` → `configOptions: [{id:"mode", category:"mode",
  …}, {id:"model", name:"Model", category:"model", type:"select",
  currentValue:"anthropic/claude-fable-5-1", options:[…]}, {id:"thinking",
  category:"thought_level", …}]` plus `modes`; notifications during the
  prompt: `available_commands_update`, `session_info_update`,
  `agent_message_chunk`×2, `usage_update{used, size, cost}`; no
  `config_option_update`, no `_meta`. `opencode acp` (OpenCode 1.18.30):
  `configOptions: [{id:"model", name:"Model", category:"model",
  type:"select", currentValue:"opencode/big-pickle", options:[…]},
  {id:"mode", name:"Session Mode", category:"mode", …}]`, no `modes`;
  notifications: `available_commands_update`, `agent_thought_chunk`,
  `agent_message_chunk`, `usage_update`; `session/prompt` result carries
  `_meta: {}`. Neither puts a model anywhere but `configOptions`; neither
  emitted `config_option_update` during a plain prompt.

### 8.3 A harness that stays busy after cancel (2026-09-24)

Observed with `omp acp` 18.2.9 under `subagents.max_runtime_secs`: the cap
fired, theuth sent `session/cancel`, omp answered the in-flight
`session/prompt` with `cancelled` (so the turn finished and the parent got
the partial report, §3.5 as before) — and then **kept working** on the task
for 10+ minutes, running tools as if the cancel had been prompt-level only.
Three things went wrong in theuth: its `session/request_permission`s
arrived with no turn in flight and were answered `cancelled` (two `git
commit`s silently blocked); its other `session/update`s were dropped (§8
rule 11), so nothing showed or persisted the work; and the parent's next
`send_message` → `session/prompt` was refused with a JSON-RPC error
(`Agent is already processing. Use steer() or followUp() to queue messages,
or wait for completion.`, data `{"reason":"session_busy","hint":
"steer|followUp|wait"}`), which theuth turned into the child's turn result —
the message was lost. The cap semantics are unchanged (it still cancels);
`AcpChild` now copes with the harness that ignores it:

- **Busy prompt → keep the input, retry.** `acp_child::is_busy(error)`:
  the rendered error text contains `busy` (omp's `session_busy` is in the
  `data` the transport appends) or `already processing`, case-insensitive.
  Such a `PromptDone` does not finish the turn: the turn stays open with its
  input (already persisted and consumed, its `TurnStarted` published), an
  `AgentEvent::Error { turn }` says `<name>: harness is still busy with its
  previous task; retrying in Ns`, and the prompt is sent again after 5 s,
  10 s, 20 s, then every 30 s. The retry stops when `Agents::request_deadline`
  (`max_runtime_secs`) has passed since the actor accepted the input: the
  next busy refusal is then the turn's error, as before. The session counts
  as busy throughout (`IsRunning`, ledger `TurnEntered`). `CancelTurn`
  during the wait finishes the turn `cancelled` without `session/cancel` or
  the kill grace — nothing of ours is in flight, and the harness's own work
  is not this turn's to kill. Updates the harness emits while the retry
  waits flow into this turn: the parent's reply is everything the harness
  said from then on.
- **Permission requests outside a turn are asked.** `permission()` no
  longer needs a turn. An ask of the prompt in flight is still cancelled
  with that turn (the spec's MUST on `session/cancel`); one arriving with no
  turn, during a continuation turn, or after the turn's token was already
  cancelled goes to `tools::approval` with the usual `risk(kind)` and waits
  on `AcpChild::alive`, a token cancelled only when the harness connection
  closes (`Closed`, kill grace, actor stop).
- **Continuation turns.** A `tool_call`, `tool_call_update`,
  `agent_message_chunk` or `agent_thought_chunk` arriving with no turn, on a
  connection that has been prompted at least once (`worked`; before that the
  updates are `session/load` replay and stay dropped, §8 rule 11), opens a
  *continuation turn*: `TurnEntered` to the ledger like any turn, `TurnStarted
  { text: "<runtime>harness still working after its turn ended</runtime>",
  turn_seq: 0 }` (a wrapper no UI echoes as a user row), then the same
  `translate()` + persistence path as a prompted turn, so the live cards
  show and `load_session` keeps them (the rows render under the previous
  turn's opener). It closes when (i) the next input arrives and its prompt
  is sent, (ii) the harness closes, or (iii) no update arrived for
  `CONTINUATION_QUIET` = 120 s. Closing runs `finish()`: open calls get
  `ERROR: not finished`, trailing text is flushed, the turn ends on an
  assistant row — but `stop_reason` is always `harness_continued`, no
  `PeerTurnFinished` is sent, and `Agents::child_finished` only marks the
  turn handed off: no `<subagent_result>`, no "result is missing" error.
  Usage/plan/mode updates alone never open one.
- **Not done.** A never-prompted dialled remote that is still working from
  a previous connection gets no continuation turn (indistinguishable from
  replay). Verification against a live `omp acp` is owed: the in-process
  fake in `acp_child::tests` reproduces the wire shapes above.
