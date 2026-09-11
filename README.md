# wth

A single-process AI harness that does things for the user. Plugins are Rust
crates talking over a typed message bus; any frontend attaches over JSON-RPC.
Design: [`docs/design/00-architecture.md`](docs/design/00-architecture.md).

```sh
just setup                 # rustup toolchain; `just toolchain` prints its bin dir
export ANTHROPIC_API_KEY=… # any genai provider works; default model in config
just run                   # terminal UI; data dir is ~/.wth (--data-dir / WTH_DATA_DIR relocate it)
just run -- --headless     # JSON-RPC only, ws://127.0.0.1:7777 with <data_dir>/token
just run -- --resume <id>  # reopen a session
just run -- -p "prompt"    # print mode: stream the reply, exit when the turn ends
echo "prompt" | just run -- -p   # same, prompt from stdin
just run -- --link-signal --owner +15551234567   # pair as a Signal linked device (one-shot QR), then run --headless
just ts                    # TypeScript bindings for the wire types → bindings/
```

Crates: `runtime` (plugin trait), `models` (bundled models.dev catalog),
`markdown` (streaming markdown: raw tail, blocks rendered once), `oauth`
(loopback callback, PKCE, provider token store), `tools` (registry, approval
gate, shell via brush, fs, MCP), `agent` (genai loop, sqlite sessions with
compaction epochs, skills, subagents, persistent Python runtime over monty
worker subprocesses — [`docs/design/08-repl.md`](docs/design/08-repl.md)),
`signal` (phone frontend over Signal messenger), `rpc` (jsonrpsee), `tui`
(iocraft), `wth` (binary).

Config (`~/.wth/`, created on first run; details in
[`docs/design/01-config-and-subagents.md`](docs/design/01-config-and-subagents.md) §1):

- `config.yaml` — every plugin reads its own section; `WTH_<SECTION>__<KEY>` env overrides. `agent.model` is a models.dev id (`anthropic/claude-sonnet-4-5`); `/model` rewrites that one line in place. The `agent` section with its defaults:

  ```yaml
  agent:
    model: anthropic/claude-sonnet-4-5
    compact_at: 0.8        # compact when the context is this full; 0 disables
    compact_style: summary  # summary | cut | snap
    knowledge:
      enabled: true        # false: no hints, no knowledge_* tools, no harvest
      hint_limit: 5        # lines per <knowledge> hint
      hint_rounds: 4       # mid-turn hint cadence in tool rounds
      harvest_every: 3     # tool-using turns per harvest; 0 disables harvest
      harvest_max: 8       # concepts accepted per harvest
  ```

  The Python runtime is always on: the worker is the `wth` binary itself (`wth subprocess`, nothing to install); every tool is an `async` function inside the sandbox and state persists across turns.
- `credentials.yaml` — `provider: key` (genai adapter names), mode 0600; beats env vars. A provider with a key, a login, or its env var is "configured": `/model` lists only those. Named OpenAI-compatible gateways (`/provider add`) store their key here under the gateway name.
- `providers.yaml` — named OpenAI-compatible gateways (`name: { base_url }`); keys live in `credentials.yaml`. `/provider add` writes both and fetches `GET {base}models` into `models.json`.
- `models.json` — optional, same shape as the bundled catalog; adds or overrides models (local `ollama/...`, private gateways). Refresh the bundled data with `just models`. `/provider add` writes the fetched model list here.
- `oauth/` — tokens from `/login` (`openai.json` = ChatGPT subscription via the Codex client, `anthropic.json` = Claude Pro/Max via the Claude Code client, `cursor.json` = Cursor subscription, `xai.json` = SuperGrok via the Grok CLI client; `mcp/<server>.json` = MCP servers that answered 401), mode 0600; `/logout <name>` deletes one. See [`docs/design/05-oauth.md`](docs/design/05-oauth.md).
- `prompt.md` — standing instructions, read into the system prompt on every model call; `/prompt` shows the assembled prompt.
- `mcp/*.json` — drop in any `mcp.json` (Claude Desktop, Cursor, OpenCode, …); hot-reloaded, tools registered without prefix. `cwd` (relative → `mcp/`) points a server at a checkout under `mcp/repos/`, where `/mcp add <url>` clones and `/mcp sync` pulls.
- `mcp/config.yaml` — per-server / per-tool `allow` | `ask` | `deny` gating; `/mcp` edits it (`deny` on a server = disabled).
- `skills/` — `<name>/SKILL.md` directories, loose `*.md`, or git clones (`/skills add <url>`, `/skills sync`); see [`docs/skills.md`](docs/skills.md).
- `knowledge/` — the knowledge bundle (OKF markdown, one `<id>.md` per concept, a git repo kept in-process); `knowledge.sqlite` beside it is its search index and can be deleted.

TUI: `/` opens the command list; `/model` picks a model (header above the
input shows model and context used); `/mcp` toggles servers and tools (`l`
logs a `login required` server in), `/mcp add <url|json>` installs one,
`/mcp sync` pulls repos; `/login` picks a provider or MCP server to log in
to; `/provider add` prompts for name, base url, then API key (an OpenAI-compatible
gateway such as `https://api.ai.rwx.dev/v1`); `/provider` lists them, `/provider rm <name>`
forgets one; `/skills` lists skills; `/prompt` shows the system prompt; `/compact` folds into a summary by default; `/compact cut` drops the folded span with no model call; `/compact style` lists/sets the default;
`/knowledge` browses the knowledge base (below); `/approvals off` stops
approval prompts for this session, `/approvals off always` for good
(`tools.approvals` in config.yaml). Shift+Enter inserts a newline; select
text with the mouse to copy it (terminal-native). See
[`docs/design/02-models-and-menus.md`](docs/design/02-models-and-menus.md).

Knowledge: durable facts about your machines, projects, accounts and
decisions, kept across sessions as plain markdown concepts in
`~/.wth/knowledge/` (Google's Open Knowledge Format — any editor works). The
agent reads it through `knowledge_search`/`knowledge_get`, and matching
concepts are surfaced to it as `<knowledge>` hints at the start of a turn and
every few tool rounds. It is built while you work: after every third turn
that used tools (and right before a compaction) the harness asks the model,
on the frame it already has cached, what is worth keeping, and writes the
answer as concepts; the model can also record with `knowledge_put` at any
time. Every write is a git commit made in-process (`gix`), so no `git`
binary is needed and history and undo work everywhere: `/knowledge` lists
concepts, `/knowledge <query>` searches, `/knowledge history [id]` shows the
commits (and the latest diff), `/knowledge restore <id> [commit]` puts a
concept back, `/knowledge harvest` runs a harvest now. Tune or turn off under
`agent.knowledge` in `config.yaml`. See
[`docs/design/10-knowledge.md`](docs/design/10-knowledge.md).

Signal: a phone-side frontend that runs only in the daemon. Pair once with
`wth --link-signal --owner +15551234567` (scan the QR from Signal → Settings
→ Linked devices on the phone that owns the account; the daemon is a linked
device, so that account needs a primary phone — usually a dedicated number
you text from your own), then `wth --headless`. The owner's number must be
saved as a contact on that primary phone, or its messages arrive without a
number and are dropped. Texts become turns in a per-contact session (👀
while working, ✅/🛑/❌ when done; a message mid-turn steers it); the slash
commands mirror the TUI (`/status`, `/new`, `/sessions`, `/resume <id>`,
`/model`, `/compact`, `/signal allow <n>`, …); tool approvals arrive as a
message you answer with 👍/👎 or `/approve <code>`. Inbound files land in
`~/.wth/signal/inbox/`. See
[`docs/design/09-signal.md`](docs/design/09-signal.md).
