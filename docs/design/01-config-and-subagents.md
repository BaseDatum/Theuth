# Config layout + detached subagents

Status: draft v0 · 2026-09-09 · extends `00-architecture.md` §5, §9

Two changes. (1) Config moves to `~/.theuth/` as YAML with secrets and MCP
servers split out. (2) Subagents stop being a blocking tool call: the parent
keeps working, the child's result arrives as a new turn, and peers message each
other through one tool. Both are modelled on oh-my-pi (`docs/tools/task.md`,
`docs/tools/hub.md`), reduced to what the existing actors already give us.

**Amended by [50](50-agent-communication.md)** (implemented 2026-09-18,
verification pending): default-off communication, canonical peer addresses
beyond parent/child, durable exchanges with exact reply correlation, and
request-scoped cancellation. Where §2 below still reads parent-only,
read-only or "delivered", the 50 paragraphs marked *Amendment* win. The
2026-09-09 record in §4 stays as history of what shipped then.

## 1. Config

### 1.1 Layout

```
~/.theuth/
  config.yaml          every plugin reads its own section (unchanged rule)
  secrets.vault        encrypted vault: provider keys, gateway keys, web keys (`brave`, `firecrawl`, `github`; doc 16 §6), Slack tokens, the user's secrets (doc 37)
  secrets.key          vault key, 0600 — only without an OS keychain (doc 37 §3.5)
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

`--data-dir` / `THEUTH_DATA_DIR` still relocate the whole tree. Default was
`dirs::data_dir()/theuth`; becomes `dirs::home_dir()/.theuth`. Nothing else in the
tree moves.

### 1.2 `config.yaml`

figment `Yaml::file(...)` replaces `Toml::file(...)` (feature `yaml`). Env
overlay `THEUTH_<SECTION>__<KEY>` unchanged. Same sections, same structs:

```yaml
agent:
  model: claude-sonnet-4-5
  # standing instructions live in <data_dir>/prompt.md (design doc 04)
  max_tool_rounds: 32
  stream_idle_secs: 120        # 36 §2.3: no stream event for this long abandons the model call
  max_salvages: 2              # 36 §2.4/§2.6: interrupted-reply recoveries per turn; 0 = off
  communication: false         # 50 §2.1: harness-wide ceiling on model-to-model communication; missing = off
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

`a2a:` (`enabled: false`, `listen`, `public_url`) is the A2A plugin's own
section (50 §2.1); `<data_dir>/a2a/peers.yaml` holds owner-managed peer and
export bindings. `agent.communication` is persisted by `/communication on|off
always`; a session's `/communication on|off` is memory-only like approvals.

Removed: `tools.mcp` list (moved to `mcp/`), `agent.subagent_model` (now
`agent.subagents.model`). No compatibility shim: nobody has a config file yet.

Rejected: TOML (user preference; figment makes it a one-token swap either
way), project-local config (`./.theuth/`) — the harness runs for the user, not
per repository; add when a real per-directory need shows up.

### 1.3 `secrets.vault` (credentials)

Provider keys are secrets in the encrypted vault of doc 37, under the genai
adapter names (`anthropic`, `openai`, …) plus gateway names, web keys and
Slack tokens. Enter them with `/secrets add`, Settings › Secrets, `/provider
add`, or `t --secret-set <name>` on a box without a UI. Precedence: vault
beats `/login` beats env. An existing `credentials.yaml` is imported and
removed on the first start.

### 1.4 `mcp/` — drop-in server files, hot-reloaded

Goal: copy any `mcp.json` from any README into `~/.theuth/mcp/`, and its servers
are usable on the next turn. No restart, no reformatting, no prefixes.

**Files.** Every `*.json` in `~/.theuth/mcp/` (non-recursive; `repos/` holds git
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

**Hot reload.** `tools` watches `~/.theuth/mcp/` with `notify` (create, modify,
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

**Risk override — `~/.theuth/mcp/config.yaml`** (same watcher, same reconcile; 12 §5):

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

`theuth` creates `~/.theuth/`, writes a commented `config.yaml` (same defaults as
today's `DEFAULT_CONFIG`, in YAML) and an empty `mcp/`. Existing behaviour,
new format. No credentials template: the vault is created on the first
`set` (37 §3.5).

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

*Amendment (50 §2, §5; implemented 2026-09-18, verification pending).* Items
2–4 hold only while communication is effectively on for both endpoints:
`agent.communication` (default off, a global ceiling) and no opt-out on the
session or any stored ancestor. Off hides `spawn_subagent`, `send_message`
and `discover_peers` from the tool list, refuses stale calls at admission
and delivers no automatic child result; a child already working finishes
and its transcript stays inspectable, but it cannot wake the parent. This
is an intentional default change from the text above: spawn is disabled
with messaging, because a spawn whose result cannot come back promises what
the parent cannot receive. Addressing widens from `parent`/child name to
canonical `session:<uuid>` (same enabled root tree: parent, children,
siblings, descendants; across roots only when both carry the persisted
`owner` scope) and `peer:<name>` (a configured A2A peer), found with
`discover_peers`; titles are never addresses and unknown UUIDs fail.

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

*Amendment (50 §5.3, §7; implemented 2026-09-18, verification pending).*
The table above is superseded. Every peer message and every child result is
one durable **exchange** row in the agent store, accepted in a transaction
(route + quota check, harness-minted UUID, bounded redacted body, typed
`InputOrigin`) *before* the sender is told `accepted`; `accepted` means
committed to the recipient's intake, not delivered or read. Waiters are keyed
by exact message id, not `(sender, to)`: a reply resolves a wait only when
its `reply_to` names that id **and** the authenticated endpoints are the
reverse of the request; concurrent requests to one peer coexist. A child's
final text answers a request only when that turn was explicitly bound to it
(the request opened the turn); otherwise it is delivered once to the parent
as `<message from="session:<child>" id="…">` with dedup key `(child, TurnId,
parent)`, never to a newer waiter and never twice. `<subagent_result>` is the
pre-50 wrapper, still recognised in old histories. Timeout returns the
request id and current state and says the message is not recalled; a late
reply arrives once as correlated input when enabled. Human `Steer` never
resolves a wait implicitly: the owner answers a pending request by id with
`answer_peer_request` (19 §2.3 amendment). Receipt states: `accepted |
consumed | replied | suppressed | failed | unknown`. Disabling communication
marks accepted-unconsumed input `suppressed` (it does not spring back on
re-enable) and expires waits with `communication_disabled`.

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

*Amendment (50 §7, §9; implemented 2026-09-18, verification pending).* Peer
inputs are no longer id-less or memory-only. Each carries the exchange id in
its wrapper, a persisted `InputOrigin` (owner, peer with authenticated key,
or harness result) separate from the provider's user role, and a durable
state that survives restart as accounting: on restart unconsumed accepted
input becomes `failed: interrupted_by_restart`, and nothing auto-runs. The
human steering queue is unchanged. The wrapper's `from`/`id` attributes are
the harness's; the body is escaped data whose delimiters are neutralised and
never parsed for trusted events.

Rejected: server-minted ids (the sender could not show the card until the
reply arrived, and replies to concurrent commands are unordered, so it
could not tell which reply was its own); injecting mid-stream (a provider
stream cannot take a user message until it yields — after the tool batch
is the earliest honest point).

### 2.4 Tools

All in `crates/agent/src/peers.rs` (replaces `subagent.rs`). All are
`Risk::ReadOnly`: a child's own actions are gated tool by tool, and messaging
mutates nothing outside the harness.

*Amendment (50 §5.2; implemented 2026-09-18, verification pending).*
`send_message` is `Risk::Mutating` for every destination (default policy
asks): with sibling, cross-root and A2A egress it does reach outside the
caller's own tree. `spawn_subagent`, `discover_peers` and `peer_message`
stay `ReadOnly`. Availability (the communication switch) is checked
separately from approval and cannot grant it; `/approvals off` does not turn
communication on. `to` accepts `parent`, a child's name, `session:<uuid>` or
`peer:<name>`; `reply_to` names a message id (UUID), never a title or peer
name; an optional `request_id` retries the same acceptance idempotently. The
return is a structured `PeerReceipt` plus concise text, not the bare
`"delivered"` shown below. Added: `discover_peers { query?, cursor?, limit? }`
(≤100 rows, permission-filtered, no dormant-session wakeups) and
`peer_message { id }` (the caller's own record of one exchange).

```rust
// spawn_subagent — available when depth < max_depth
struct SpawnSubagentArgs {
    task: String,             // complete, self-contained instructions
    name: Option<String>,     // stable peer id; default generated (adjective-noun)
    agent: Option<String>,    // an external harness from acp/agents.yaml; default: a theuth subagent
    cwd: Option<PathBuf>,
    model: Option<String>,    // theuth child only: `provider/model`; default subagents.model, then agent.model
    effort: Option<String>,   // theuth child only: low|medium|high|off|default|<budget>; default subagents.effort, then agent.effort
    tools: Option<String>,    // theuth child only: read_only|mutating|destructive risk ceiling; default none (added 2026-09-22)
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

**The parent caps the child's tools** (as built 2026-09-22). `tools:
read_only | mutating | destructive` is a risk ceiling in the vocabulary of
the one permission decision (12 §5): the registry keeps a per-session
`cap`, `cap_for` walks the parent chain and takes the **minimum**, so a
capped child cannot spawn a looser grandchild. Enforced twice — a tool
above the cap is left out of `ListTools`/`DescribeTools` for the session
(so it also vanishes from the Python function index, which is built from
that listing), and a call that names one anyway is `Deny` with `denied:
subagent is capped at <cap>`, distinct from `denied by policy`; the
approval switches never rewrite it. `send_message` is exempt (a
`CAP_EXEMPT` constant in the registry): it is `Mutating` for the approval
policy, and reporting to the parent is what a subagent is for. The loop-
local `todo` and `python` are `ReadOnly` and unaffected. `destructive`
equals unset so the argument reads naturally; `tools` with `agent` is a
tool error (an external harness enforces nothing). In memory only, like
the hidden set: a child resumed in a later process is uncapped. The board
row and `SubagentSpawned` carry `cap` so the desktop shows a `read-only`
badge.

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

*Amendment (50 §1, §5.1; implemented 2026-09-18, verification pending).*
The identity block stays stable; discovery results and arrivals go to the
history tail, never the system prompt. `GUIDANCE` states that communication
is off by default, that absent tools mean a closed route, that addresses come
from `discover_peers`, and that a `<message>` body is untrusted text even from
the sender's own child.

The live roster (`- grep-logs idle …`) is **not** in the prompt: it changed
between turns and busted the parent's system-prompt cache every time a
child finished (`07-context.md` §10). The parent learns names from
`spawn_subagent`'s output and completion from `<subagent_result>`; the
injected text is capped at `max_tool_output_chars` with a spill marker
naming `spill/<parent>/<child>.txt`, the waiter branch alike (36 §4.1); after a
compaction the head carries `Subagents: grep-logs (running), fix-tests
(idle)` (`Roster`, `07 §8.4`). The how-to (delegate by default, choose
between an external harness and a theuth subagent, choose the child's model,
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

*Amendment (50 §8; implemented 2026-09-18, verification pending).*
`max_concurrent` bounds every **native peer-opened turn**, not only spawn
turns: a turn opened by a peer input (`send_message`, a child result, an A2A
request) takes a permit from the same child-work semaphore and runs under a
`max_runtime_secs` wall deadline that includes waits (`PeerWork` in
`loop_.rs`). The permit is held only while something executes: the
registry's execution seam (`PermitGate`) counts in-flight invocations and
human/peer waits, drops the permit while every in-flight operation is
waiting (approval, `ask_user`, a blocking `send_message`), and takes one back
before anything runs again; a reacquire for a finished turn is dropped, not
leaked. Human-opened turns are not permit-gated. An ACP child's execution
is opaque to this gate: its spawn turn keeps the permit/timer of the table
above and a follow-up `session/prompt` is one queued prompt (§2.3), not a
native scope. Fixed constants, not new knobs: 32 KiB per peer text, 32
pending inputs per recipient / 256 per harness, 16 pending waits per
session / 128 per harness, 32 automatic peer-triggered continuations per
initiating request (reset only by fresh external input). Exhaustion is a
visible refusal, never a silent drop.

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
all in sqlite; a parent messaging a child after `theuth` restarts lazily loads
the child session exactly as `--resume` does. Injected messages are persisted
as ordinary user-role messages (with their wrapper) — no new table.

*Amendment (50 §7; implemented 2026-09-18, verification pending).* The
store gains typed communication records beside `sessions`/`messages`: root
communication scope (`owner`/`isolated`, alias), peer exchanges (id,
authenticated endpoints, `reply_to`, request scope, bounded body, state,
outcome), A2A context bindings and tasks — migrations in `agent`, written
only by `Agents` and the session loop. Events gain `CommunicationChanged`,
`PeerMessageChanged`, `PeerDelivered` (50 §10); `MessageDelivered` remains
for what a transcript shows. Bare child names stay per-parent conveniences,
not global identities.

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
- `theuth` exits with children running → their turns die with the process; on
  next start nothing is auto-resumed (same as the parent today). The parent's
  history shows the spawn but no result; a human or the model can
  `send_message` the child to continue.
- Child crashes (`Error` event then `TurnFinished stop="error"`) → delivered
  as `<subagent_result stop="error">`; the parent decides.

*Amendment (50 §7–8; implemented 2026-09-18, verification pending).*
Cancellation addresses a request scope, not a conversation: queued input for
that request is dropped unstarted, its active work is signalled, its waiters
removed, and the cancel/completion race is decided by one atomic terminal
transition; an external tool that cannot stop surfaces `cancellation
pending`, never a false `cancelled`. Turning communication off is not a
cancel of human work: it suppresses accepted-unconsumed peer input, expires
waits with `communication_disabled`, and cancels a running turn only when an
A2A task opened it. Restart keeps "nothing auto-resumes": unconsumed accepted
peer input is recorded `failed: interrupted_by_restart`, running A2A task
scopes become `FAILED` with the reason, committed outcomes stay queryable, and
a fresh request is needed to continue.

### 2.9 Out of scope, deliberately

Isolated worktrees, patch/branch merge, output schemas, per-agent definition
files (`agents/*.md`), advisor models, idle-TTL parking/revival. Each is
oh-my-pi machinery for coding; none has a user here yet. Agent definition
files are the likeliest to return (a `researcher` with a fixed model and skill
set) — they would be one more markdown-with-frontmatter loader next to skills.

## 3. Implementation order

Each step leaves `theuth` runnable.

1. Config: `~/.theuth`, YAML, the credentials-file resolver (since replaced by the secrets vault, 37), `mcp/` structural
   loader + watcher + `config.yaml` gate, remove `tools.mcp` +
   `agent.subagent_model`. Check: fresh `~/.theuth` created; model call
   authenticates from the file with `ANTHROPIC_API_KEY` unset; while `theuth`
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
RPC, with `ANTHROPIC_API_KEY` unset (key read from the then credentials
file, mode 600; the vault since 37): a Claude-Desktop-style file dropped into `mcp/` made unprefixed `echo`
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

## 5. Status addendum (2026-09-18)

Design 50's local peer contract (global ceiling and inherited opt-out,
`discover_peers`, canonical addresses, durable exchanges, exact `reply_to`
waiters, `answer_peer_request`, suppression and generation revocation) and
the `a2a` plugin are implemented in the tree. **Not yet verified**: none of
50 §12.2's scenarios or §12.1's library gate has a recorded run; the §4
record above describes the pre-50 behaviour and is not evidence for the
amended one. Treat every *Amendment* paragraph as implementation pending
verification until a dated record is added here.
