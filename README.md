# wth

A single-process AI harness that does things for the user. Plugins are Rust
crates talking over a typed message bus; any frontend attaches over JSON-RPC.
Design: [`docs/design/00-architecture.md`](docs/design/00-architecture.md).

```sh
just setup                 # rustup toolchain + `monty` Python worker; `just toolchain` prints the bin dir
export ANTHROPIC_API_KEY=… # any genai provider works; default model in config
just run                   # terminal UI; data dir is ~/.wth (--data-dir / WTH_DATA_DIR relocate it)
just run -- --headless     # JSON-RPC only, ws://127.0.0.1:7777 with <data_dir>/token
just run -- --resume <id>  # reopen a session
just run -- -p "prompt"    # print mode: stream the reply, exit when the turn ends
echo "prompt" | just run -- -p   # same, prompt from stdin
just ts                    # TypeScript bindings for the wire types → bindings/
```

Crates: `runtime` (plugin trait), `models` (bundled models.dev catalog),
`markdown` (streaming markdown: raw tail, blocks rendered once), `oauth`
(loopback callback, PKCE, provider token store), `tools` (registry, approval
gate, shell via brush, fs, MCP), `agent` (genai loop, sqlite sessions with
compaction epochs, skills, subagents, persistent Python runtime over monty
worker subprocesses — [`docs/design/08-repl.md`](docs/design/08-repl.md)),
`rpc` (jsonrpsee), `tui` (iocraft), `wth` (binary).

Config (`~/.wth/`, created on first run; details in
[`docs/design/01-config-and-subagents.md`](docs/design/01-config-and-subagents.md) §1):

- `config.yaml` — every plugin reads its own section; `WTH_<SECTION>__<KEY>` env overrides. `agent.model` is a models.dev id (`anthropic/claude-sonnet-4-5`); `/model` rewrites that one line in place. The `agent` section with its defaults:

  ```yaml
  agent:
    model: anthropic/claude-sonnet-4-5
    compact_at: 0.8        # compact when the context is this full; 0 disables
    repl:
      monty: null          # `monty` worker path; default: PATH, then the toolchain bin dir
  ```

  The Python runtime is on whenever the `monty` worker is found (`just setup` installs it); every tool is then an `async` function inside the sandbox and state persists across turns.
- `credentials.yaml` — `provider: key` (genai adapter names), mode 0600; beats env vars. A provider with a key, a login, or its env var is "configured": `/model` lists only those.
- `oauth/` — tokens from `/login` (`openai.json` = ChatGPT subscription via the Codex client, `anthropic.json` = Claude Pro/Max via the Claude Code client, `cursor.json` = Cursor subscription, `xai.json` = SuperGrok via the Grok CLI client; `mcp/<server>.json` = MCP servers that answered 401), mode 0600; `/logout <name>` deletes one. See [`docs/design/05-oauth.md`](docs/design/05-oauth.md).
- `models.json` — optional, same shape as the bundled catalog; adds or overrides models (local `ollama/...`, private gateways). Refresh the bundled data with `just models`.
- `prompt.md` — standing instructions, read into the system prompt on every model call; `/prompt` shows the assembled prompt.
- `mcp/*.json` — drop in any `mcp.json` (Claude Desktop, Cursor, OpenCode, …); hot-reloaded, tools registered without prefix. `cwd` (relative → `mcp/`) points a server at a checkout under `mcp/repos/`, where `/mcp add <url>` clones and `/mcp sync` pulls.
- `mcp/config.yaml` — per-server / per-tool `allow` | `ask` | `deny` gating; `/mcp` edits it (`deny` on a server = disabled).
- `skills/` — `<name>/SKILL.md` directories, loose `*.md`, or git clones (`/skills add <url>`, `/skills sync`); see [`docs/skills.md`](docs/skills.md).

TUI: `/` opens the command list; `/model` picks a model (header above the
input shows model and context used); `/mcp` toggles servers and tools (`l`
logs a `login required` server in), `/mcp add <url|json>` installs one,
`/mcp sync` pulls repos; `/login` picks a provider or MCP server to log in
to; `/skills` lists skills; `/prompt` shows the system prompt; `/compact`
folds the conversation so far into a summary the model keeps working from;
`/approvals off` stops approval prompts for this session, `/approvals off
always` for good (`tools.approvals` in config.yaml). Shift+Enter inserts a
newline; select text with the mouse to copy it (terminal-native). See
[`docs/design/02-models-and-menus.md`](docs/design/02-models-and-menus.md).
