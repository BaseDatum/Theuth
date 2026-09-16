# Config layout + detached subagents

Status: draft v0 · 2026-09-09 · extends `00-architecture.md` §5, §9

Two changes. (1) Config moves to `~/.wth/` as YAML with credentials and MCP
servers split out. (2) Subagents stop being a blocking tool call: the parent
keeps working, the child's result arrives as a new turn, and peers message each
other through one tool. Both are modelled on oh-my-pi (`docs/tools/task.md`,
`docs/tools/hub.md`), reduced to what the existing actors already give us.

## 1. Config

### 1.1 Layout

```
~/.wth/
  config.yaml          every plugin reads its own section (unchanged rule)
  credentials.yaml     provider -> api key; mode 0600; never merged into config; also web keys (`brave`, `firecrawl`, `github`; doc 16 §6)
  oauth/               /login tokens: <ns>.json (providers), mcp/<server>.json; 0600 (doc 05)
  acp/agents.yaml      external ACP harnesses: name -> {command, args, env, description} (doc 06)
  acp/registry.json    cached ACP registry (24 h)
  acp/bin/<id>/        binary distributions installed by /acp add
  mcp/
    <name>.json        one MCP server per file; file stem is the tool prefix
  sessions.sqlite      (existing)
  skills/**/*.md       (existing)
  logs/                (existing)
  token                (existing, rpc bearer)
```

`--data-dir` / `WTH_DATA_DIR` still relocate the whole tree. Default was
`dirs::data_dir()/wth`; becomes `dirs::home_dir()/.wth`. Nothing else in the
tree moves.

### 1.2 `config.yaml`

figment `Yaml::file(...)` replaces `Toml::file(...)` (feature `yaml`). Env
overlay `WTH_<SECTION>__<KEY>` unchanged. Same sections, same structs:

```yaml
agent:
  model: claude-sonnet-4-5
  # standing instructions live in <data_dir>/prompt.md (design doc 04)
  max_tool_rounds: 50
  subagents:                   # §2.6
    model: null                # default: agent.model
    max_depth: 1
    max_concurrent: 8
    max_runtime_secs: 600
    reply_timeout_secs: 120
tools:
  policy: { read_only: allow, mutating: ask, destructive: ask }
  approvals: true   # false: never prompt (`/approvals off always` writes this)
  approval_timeout_secs: 300
  job_grace_secs: 3
  job_timeout_secs: 1800
rpc:
  listen: 127.0.0.1:7777
```

Removed: `tools.mcp` list (moved to `mcp/`), `agent.subagent_model` (now
`agent.subagents.model`). No compatibility shim: nobody has a config file yet.

Rejected: TOML (user preference; figment makes it a one-token swap either
way), project-local config (`./.wth/`) — the harness runs for the user, not
per repository; add when a real per-directory need shows up.

### 1.3 `credentials.yaml`

```yaml
anthropic: sk-ant-...
openai: sk-...
gemini: AIza...
```

Keys are genai `AdapterKind::as_lower_str()` names, so the file needs no
schema of its own and gains providers as genai does. Wiring, in
`agent::AgentPlugin::start`:

```rust
let creds: HashMap<String, String> = read yaml or empty;   // missing file is fine
Client::builder().with_auth_resolver_fn(move |m: ModelIden| {
    Ok(match creds.get(m.adapter_kind.as_lower_str()) {
        Some(k) => Some(AuthData::Key(k.clone())),
        None => m.adapter_kind.default_key_env_name().map(AuthData::FromEnv),
    })
})
```

Precedence: file beats login beats env. Reason: the file is the thing the
user edited on purpose; env vars leak in from shells and CI. A `/login`
(`oauth/<ns>.json`, doc 05) sits between them. Startup warns (not fails) if
the file mode is wider than 0600 or if the configured model's provider has
none of the three; the model call then fails with genai's own error.

Rejected: OS keychain (`keyring` crate) — extra dep, platform prompts, and the
sqlite/token already live unencrypted in the same directory; revisit with the
Tauri build. Encrypting the file — same argument.

### 1.4 `mcp/` — drop-in server files, hot-reloaded

Goal: copy any `mcp.json` from any README into `~/.wth/mcp/`, and its servers
are usable on the next turn. No restart, no reformatting, no prefixes.

**Files.** Every `*.json` in `~/.wth/mcp/` (non-recursive; `repos/` holds git
clones and is never scanned) may hold one server or many; the user organises
by file however they like (`research.json`, `home-automation.json`,
`everything.json`). Formats are detected structurally, not by a wrapper-key
list:

```
node is a server entry  ⇔  it is an object with `command` or `url`
root is a server        →  one server, name = file stem
else, for each (k, v) in root with v an object:
    v is a server       →  server k
    else                →  wrapper (`mcpServers`, `mcp`, `servers`, `context_servers`, …):
                           for each (k2, v2) in v with v2 a server → server k2
```

Entry normalisation covers the shapes in the wild:

| Field | Accepted | Notes |
|---|---|---|
| exe | `command: "npx"` + `args: [...]` (Claude/Cursor/LM Studio/VS Code) · `command: ["npx", ...]` (OpenCode) | array form: first element is the exe |
| env | `env` · `environment` | map of string→string |
| remote | `url` with `type` ∈ `http`/`sse`/`remote`/`streamable-http`/absent | all via rmcp streamable-HTTP; `headers` map passed through |
| cwd | `cwd` | working directory for the exe; relative → against `mcp/` (`repos/my-server`) |
| off | `enabled: false` · `disabled: true` | entry skipped |

**Shell expansion.** `command`, `args`, `env` values, `url` and `headers`
values go through the user's shell (`$SHELL`, else `/bin/sh`) at connect
time, as an unquoted heredoc: `${GRAFANA_TOKEN}`, `$(gh auth token)`,
`${VAR:-default}` and backticks expand, quotes stay literal, `\$` is a
literal dollar. Strings without `$` or a backtick are not touched. Connect
time, not parse time, so a token subshell runs once per connection instead of
on every watcher tick, and reconcile equality stays on the file text. A
failed substitution is empty (POSIX) with a warning naming the value; the
server's own auth error names the consequence. (jarvis had a hand-rolled
`${}`/`$()` parser; the shell already is that parser.)

Anything else in an entry is ignored. A file that fails to parse is logged
(`mcp/foo.json: expected …`) and skipped; the other files still load. Same
server name in two files: first by sorted path wins, warning names both files.

**Hot reload.** `tools` watches `~/.wth/mcp/` with `notify` (create, modify,
remove, rename), debounces 300 ms, then reconciles desired vs running by
server name: new → connect + register tools; removed → unregister its tools +
drop the connection (child process exits with it); changed (normalised entry
differs) → drop + reconnect; `Restart(name)` forces the drop + reconnect for
one server whose entry is unchanged (after `sync_mcp` rebuilt its repo).
Startup is the same reconcile with an empty running set. A server that fails
to connect is logged and retried on the next reconcile (any change in the
dir), not in a loop. Registry gains `UnregisterTool(name)`. MCP tools
register with `direct: false`: while the Python runtime is on
(`08-repl.md` §0.4) they are callable inside `python` by name and are not
direct tool definitions; with the runtime off they are direct tools and the
model's tool list is read once at the start of every turn (`ListTools` in
`Turn::rebase`), so a server added mid-turn is visible on the next turn
(`07-context.md` §5, 12 §4).

**Connects never block.** Reconcile only decides; each connect (+ first
`list_tools`, bounded by a 10 s timeout) runs in its own task that `tell`s
the actor a `Settled { name, spec, result }`. The actor stays responsive
(`/mcp`, `ListServers`, `SetGate` answer while twenty servers boot), startup
does not wait for any of them (`start` tells `Reconcile` and returns), and a
settled server whose entry changed meanwhile is dropped and reconnected.
Every state change publishes `McpProgress { connecting, connected, failed }`
on the bus; the TUI shows it bottom-right (`⠹ mcp 3/12 · connecting a, b, c
(+4)`, then `mcp 11/12 · 1 failed`) and asks `Progress` once on first render
for the snapshots published before it subscribed. RPC forwards it on
`subscribe_events`. `add_mcp_server` is the one deliberate wait: it connects
first and writes the file only on success, so nothing is ever half-added.

**No prefix unless it clashes.** A server's `echo` registers as `echo`.
When two running servers offer the same name (four `grafana-*` entries
pointing at one server binary is the common case), or a native tool already
has it, *every* server offering that name registers it as `<server>__<name>`
(`grafana-qe__query_prometheus`), so each server keeps its own slot and can
be gated, hidden or turned off on its own. The prefix disappears again when
only one server is left offering the name. `/mcp` shows the registered name
(`as grafana-qe__query_prometheus`) next to the remote one.

**Risk override — `~/.wth/mcp/config.yaml`** (same watcher, same reconcile; 12 §5):

```yaml
servers:
  everything:
    risk: read_only            # every tool of this server; default: annotations
    tools:
      longRunningOperation: off  # unregistered, model never sees it
  filesystem:
    risk: off                  # not connected
```

Values are a `Risk` (`read_only` | `mutating` | `destructive`) or `off`. An
MCP tool's default `Risk` comes from its `ToolAnnotations`:
`read_only_hint: true` → read-only, `destructive_hint: true` → destructive,
else mutating (an *unset* `destructive_hint` is deliberately read as mutating,
not the spec's destructive: most servers set nothing). The registry then
applies `tools.policy` exactly as for a native tool — there is no second
policy for MCP tools. `off` at tool level hides the tool from the model, at
server level skips the connection entirely. Resolution: tool line → server
line → annotations. An MCP tool without annotations therefore prompts by
default (`tools.policy.mutating = ask`); a user who trusts a server sets
`risk: read_only` on it once. Old `allow | ask | deny` files fail to parse
and are ignored with a warning naming the file and the accepted words.

The agent sees no difference between native and MCP tools: same `ToolSpec`,
same approval flow, same events.

Rejected: one `mcp.json` with a fixed schema (the user's whole point is
copy-paste from anywhere); JSON5/comments (files from READMEs are plain
JSON; add `json5` when one shows up with comments); prefixing every tool
(`server__tool`) — the model gains nothing and prompts get longer, so only a
clash is prefixed; first-server-wins on a clash (the losers were dead
entries, and a per-session switch on the shared name flipped every server
offering it); subscribing to `tools/list_changed` notifications from servers
(rmcp supports it; the file watcher covers the user-driven case,
server-driven tool changes are rare).

### 1.5 First run

`wth` creates `~/.wth/`, writes a commented `config.yaml` (same defaults as
today's `DEFAULT_CONFIG`, in YAML), an empty `credentials.yaml` with a comment
listing the provider keys, and an empty `mcp/`. Existing behaviour, new format.

## 2. Detached subagents

### 2.1 Today

`spawn_subagent` blocks inside the parent's tool batch: spawns a child
session, subscribes a `Collector` to the bus, sends the task, waits up to 10
minutes for `TurnFinished`, returns the child's text as the tool output. The
parent cannot do anything else meanwhile, the child cannot ask anything, and
the human cannot talk to the child. A fork (29) has `parent = NULL`: it is
a top-level session even when the source was a child.

### 2.2 Target behaviour

1. `spawn_subagent` returns immediately with the child's name and session id.
   The parent's turn continues (more tools, more text) or ends.
2. When any child turn finishes, its result is delivered to the parent as an
   input. Parent idle → a new turn starts. Parent mid-turn → queued and
   injected after the current tool batch, the same path as human steering
   (§2.3). This is `Session::UserInput` today; no new path.
3. Peers talk with one tool, `send_message`. A child can address `parent`; a
   parent addresses children by name. `await_reply: true` blocks the caller
   until the addressee's next message to it (bounded by `reply_timeout_secs`).
   This is the "request guidance / steer" channel.
4. Children stay alive after finishing (idle session actors, as today). A
   later `send_message` to an idle child starts a new child turn; its result
   comes back the same way as the first. Follow-up never needs a respawn.
5. The human sees children in the TUI roster, opens one from the `/agents`
   picker to read its transcript, and can steer it — same commands the
   parent gets (19).

### 2.3 Delivery rules

Every message into a session is one text block wrapped so the model knows its
provenance:

```
<message from="parent" id="m7" reply_expected="true">
...text...
</message>

<subagent_result name="grep-logs" session="01a0..." stop="end_turn">
...final assistant text of the child's turn...
</subagent_result>
```

Routing lives in the supervisor (`Agents`), which already owns every session
actor and the parent column:

| Event | Action |
|---|---|
| child `TurnFinished` | if a peer is awaiting a reply *from this child* → resolve that waiter with the child's final text; else → `parent.tell(UserInput(<subagent_result>))` |
| `send_message{to, text, await_reply}` | if `to` is awaiting a reply *from the sender* → resolve that waiter with `text` (no injection); else → `to.tell(UserInput(<message>))`. If `await_reply` → register waiter `(sender, to)`, return when resolved or on timeout |
| waiter timeout | tool returns `no reply from <to> within Ns` as a normal (non-error) output; the message stays delivered |
| `Jobs` `Finished` | `parent.tell(UserInput(<job_result>))` — the session that started the job (14 §3) |

"Final assistant text of the turn" = concatenated assistant content persisted
after the turn's last user message — read from the store, not collected from
the stream, then capped by `spill::cap` (§2.5). The `Collector` actor goes away.

Consequences worth stating:

- A child asking `parent` while the parent is mid-turn gets its question
  injected after the parent's current tool batch. The parent model answers
  with `send_message{to: child, reply_to: id}` and carries on. If the parent's
  turn ends without answering, the child times out and continues with its own
  judgement. oh-my-pi runs a side-model auto-reply here; skipped — the
  timeout message already tells the child what happened.
- A parent that has finished its turn is idle; a child question wakes it for a
  reply turn, after which it is idle again. The human sees that turn.
- Two children finishing while the parent is idle → two parent turns, in
  arrival order (second is queued as steering of the first). Correct and
  simple; batching them into one turn is an optimisation for later.
- Result delivery is one-way (child → parent). A parent's turn text is never
  pushed to children; parents steer explicitly.
- An ACP child's turn is one `session/prompt`; `send_message` to it while a
  prompt is in flight is queued and sent when the response arrives. Its
  final text is the accumulated agent text (or the last tool result when the
  harness ends silently).

**Human steering** (`Steer { session, id, text }`, `Withdraw { session,
id }`, `ListQueued { session }`, all `ask` on `Agents`). A mid-turn input
has a three-state lifecycle:

- **queued** — the caller mints an `InputId` (uuid v7) and sends `Steer`;
  the loop pushes `QueuedInput { id, text }` onto the session's in-memory
  queue and publishes `InputQueued { session, id, text }`. Because the id is
  the caller's, a frontend can paint the card the moment the user presses
  Enter and address it later without waiting for any reply.
- **injected** — after the current tool batch ends, or at `TurnDone` when
  the queue is non-empty (then the queued inputs start the next turn), the
  loop appends the text as a user message and publishes `InputInjected {
  session, id, turn }`. When queued inputs start a turn, `InputInjected`
  precedes `TurnStarted` for that turn, so a frontend that already shows
  the card skips the `TurnStarted` echo. An idle session has no batch to
  wait for: `Steer` starts a turn immediately and still publishes
  `InputInjected`.
- **withdrawn** — `Withdraw` while still queued removes the item and
  returns `Ok(text)` to the caller (the TUI puts it back in the editor);
  `InputWithdrawn { session, id }` goes to everyone else. Once injected it
  is history: `Err("not queued")`.

`ListQueued` and `Loaded.queued` give a frontend attaching mid-turn the
current queue. The queue is memory only — a queued steer is a pending
action, not history, and does not survive the process. Harness inputs
(`<subagent_result>`, `<job_result>`, peer `<message>`) carry no id and publish no `Input*`
event. `SendInput` is unchanged: it starts a turn when idle and is echoed
by `TurnStarted`.

Rejected: server-minted ids (the sender could not show the card until the
reply arrived, and replies to concurrent commands are unordered, so it
could not tell which reply was its own); injecting mid-stream (a provider
stream cannot take a user message until it yields — after the tool batch
is the earliest honest point).

### 2.4 Tools

All in `crates/agent/src/peers.rs` (replaces `subagent.rs`). All are
`Risk::ReadOnly`: a child's own actions are gated tool by tool, and messaging
mutates nothing outside the harness.

```rust
// spawn_subagent — available when depth < max_depth
struct SpawnSubagentArgs {
    task: String,             // complete, self-contained instructions
    name: Option<String>,     // stable peer id; default generated (adjective-noun)
    agent: Option<String>,    // an external harness from acp/agents.yaml; default: a wth subagent
    cwd: Option<PathBuf>,
    model: Option<String>,    // wth child only: `provider/model`; default subagents.model, then agent.model
    effort: Option<String>,   // wth child only: low|medium|high|off|default|<budget>; default subagents.effort, then agent.effort
}
// -> "Spawned `grep-logs` (session 01a0…). Its result will arrive as a
//     <subagent_result> when it finishes; message it with send_message."

// send_message — available when the session has a parent or children
struct SendMessageArgs {
    to: String,               // "parent" | child name
    message: String,
    await_reply: bool,        // default false
    reply_to: Option<String>, // id of the message being answered
}
// -> "delivered" | "<reply text>" | "no reply from X within Ns"

// list_models — no arguments
// -> one line per model of every provider with credentials:
//    "anthropic/claude-opus-4-8 · 200k · reasoning low|medium|high · vision"
```

The tool description appends the configured harness names and descriptions;
an unknown name is a tool error listing them.

**The parent picks the child's model** (added 2026-09-14). `model`/`effort`
are validated at spawn: an ambiguous bare id, a provider without
credentials, or an unknown effort keyword is a tool error naming
`list_models`; both are an error with `agent` set (an external harness has
its own model). Unset, the `subagents.model`/`effort` config applies as
before. The guidance text tells the model to match the model to the task
and to err towards more capability. Rejected: a fixed tier map
(`fast`/`smart` → ids) in config - the model already knows model names,
and the list is one tool call away; a static list in the system prompt -
it changes with logins and would be hundreds of lines for gateway providers.

Why one symmetric tool instead of `ask_parent` / `steer_subagent`: same
schema, same routing table, same rendering; the direction is just `to`.

Why no `tasks[]` batch parameter: the loop already runs one assistant
message's tool calls concurrently (`join_all`), so three `spawn_subagent`
calls in one message *is* the batch. oh-my-pi needs `context` in the batch
because it renders it into every child's system prompt; we get the same by
telling the model to put shared background in each `task` (system prompt
line). Add the batch shape only if models observably fail to fan out.

Removed: `list_subagents`, `cancel_subagent`, `wait`. The model already
has the names from `spawn_subagent`'s output and the compaction head
(§2.5), cancel is a human action from the TUI, and waiting is what
`await_reply` does. Any of these returns as a tool when a transcript shows
the model needing it.

### 2.5 Prompt surface

Children get one identity block, rendered into the system prompt once per
turn from supervisor state (`PeerContext`, `07-context.md` §4):

```
## Peers
You are `grep-logs`, a subagent of session 01a0… (address it as `parent`).
Your final message is delivered to the parent when your turn ends.
```

The live roster (`- grep-logs idle …`) is **not** in the prompt: it changed
between turns and busted the parent's system-prompt cache every time a
child finished (`07-context.md` §10). The parent learns names from
`spawn_subagent`'s output and completion from `<subagent_result>`; the
injected text is capped at `max_tool_output_chars` with a spill marker
naming `spill/<parent>/<child>.txt`, the waiter branch alike (36 §4.1); after a
compaction the head carries `Subagents: grep-logs (running), fix-tests
(idle)` (`Roster`, `07 §8.4`). The how-to (delegate by default, choose
between an external harness and a wth subagent, choose the child's model,
write a complete brief, verify before reporting) is static text in
`frame::baseline`'s `GUIDANCE` (07 §4).

Children get no `yield` tool (oh-my-pi's) — our turn end *is* the yield.

### 2.6 Limits (config §1.2)

| Knob | Default | Enforcement |
|---|---|---|
| `max_depth` | 1 | `spawn_subagent` hidden from the model's tool list at depth ≥ max and rejected at call time; depth = parent chain length from the store |
| `max_concurrent` | 8 | `tokio::sync::Semaphore` in the supervisor around the child turn started by `spawn_subagent`; a spawn beyond the cap starts when a permit frees, transparently (the parent is not told). Follow-up turns started by `send_message`/human input are not permit-gated |
| `max_runtime_secs` | 600 | per spawn turn (same scope as the permit); on expiry the turn is cancelled and the parent receives `<subagent_result stop="cancelled">` prefixed `[timed out after Ns]` |
| `reply_timeout_secs` | 120 | `await_reply` bound |
| `model` / `effort` | `null` | child defaults when `spawn_subagent` names none; fall back to `agent.model` / `agent.effort` |

### 2.7 Events, store, UI

`AgentEvent` gains:

```rust
SubagentSpawned { parent: SessionId, child: SessionId, name: String }
MessageDelivered { session: SessionId, from: String, text: String }  // what was injected, for transcripts
```

`TurnStarted`/`TurnFinished` on the child session already exist; roster
status is derived from them. No `SubagentFinished` event: that is the child's
`TurnFinished` plus the parent's `MessageDelivered`.

Store: `sessions` gains `name TEXT` (nullable; unique per parent). Peer
addressing by name survives restart because names, parents and histories are
all in sqlite; a parent messaging a child after `wth` restarts lazily loads
the child session exactly as `--resume` does. Injected messages are persisted
as ordinary user-role messages (with their wrapper) — no new table.

TUI: `/agents` is a picker over the current session's subtree with live state
per row; Enter opens a child as a session switch (the transcript, status line
and input become the child's; plain input steers it), `/parent` climbs back.
A board under the transcript shows the live children (19). Approval modals
already carry `session`; they gain the peer name in the header so a child's
`shell` request is distinguishable from the parent's. RPC gains one method,
`list_agents(session)`, for rosters; events already stream with `session`
and `send_input` already takes one.

### 2.8 Restart and cancellation

- Parent cancelled (`Cancel`) → every running descendant is cancelled too
  (supervisor walks the subtree, 19 §2.3). Idle descendants are left alone.
- `wth` exits with children running → their turns die with the process; on
  next start nothing is auto-resumed (same as the parent today). The parent's
  history shows the spawn but no result; a human or the model can
  `send_message` the child to continue.
- Child crashes (`Error` event then `TurnFinished stop="error"`) → delivered
  as `<subagent_result stop="error">`; the parent decides.

### 2.9 Out of scope, deliberately

Isolated worktrees, patch/branch merge, output schemas, per-agent definition
files (`agents/*.md`), advisor models, idle-TTL parking/revival. Each is
oh-my-pi machinery for coding; none has a user here yet. Agent definition
files are the likeliest to return (a `researcher` with a fixed model and skill
set) — they would be one more markdown-with-frontmatter loader next to skills.

## 3. Implementation order

Each step leaves `wth` runnable.

1. Config: `~/.wth`, YAML, `credentials.yaml` resolver, `mcp/` structural
   loader + watcher + `config.yaml` gate, remove `tools.mcp` +
   `agent.subagent_model`. Check: fresh `~/.wth` created; model call
   authenticates from the file with `ANTHROPIC_API_KEY` unset; while `wth`
   runs, dropping a Claude-Desktop-style and an OpenCode-style file into
   `mcp/` makes `echo` callable on the next turn without a restart, and
   setting `gate: ask` in `mcp/config.yaml` makes the next `echo` prompt.
2. Store + supervisor: `name` column, children map, waiter table, result
   delivery on child `TurnFinished`, semaphore, runtime cap, cancel fan-out.
3. Tools: `spawn_subagent` (non-blocking), `send_message`; delete `Collector`.
   Prompt surface §2.5. Check (headless, over RPC): parent spawns two children
   in one message, its turn ends, two `<subagent_result>` turns follow; a
   child `send_message{to: parent, await_reply}` wakes the idle parent, whose
   reply lands as the child's tool output.
4. TUI: roster, `/agents`, peer name on approval modals (the picker and
   `/parent` came later, 19).
5. Docs: `00-architecture.md` §5 subagent paragraph and §9 point here;
   README config block.

## 4. Status (2026-09-09)

All five steps shipped and verified against the real binary, headless over
RPC, with `ANTHROPIC_API_KEY` unset (key read from `credentials.yaml`, mode
600): a Claude-Desktop-style file dropped into `mcp/` made unprefixed `echo`
callable ~1 s later with no approval; `mcp/config.yaml` `echo: ask` produced an
`ApprovalRequested`, `deny` hid it, deleting the file removed all 13 tools and
left no `server-everything` process. Parent spawned `a`/`b` in one message,
ended its turn before either child's `sleep` finished, then ran two
`<subagent_result>` turns. Child `q` asked `parent` with `await_reply`, the
idle parent woke, replied `teal` via `send_message`, and `q`'s tool output was
`teal`. `list_agents` and `load_session` show names and the injected
wrappers after the fact.

Not exercised: `max_runtime_secs` expiry, `reply_timeout_secs` expiry, remote
(HTTP+headers) MCP servers, cross-restart addressing of a child by name.
