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
just ts                    # TypeScript bindings for the wire types → bindings/
```

Crates: `runtime` (plugin trait), `models` (bundled models.dev catalog),
`markdown` (streaming markdown: raw tail, blocks rendered once),
`tools` (registry, approval gate, shell via brush, fs, MCP), `agent` (genai
loop, sqlite sessions, skills, subagents), `rpc` (jsonrpsee), `tui`
(iocraft), `wth` (binary).

Config (`~/.wth/`, created on first run; details in
[`docs/design/01-config-and-subagents.md`](docs/design/01-config-and-subagents.md) §1):

- `config.yaml` — every plugin reads its own section; `WTH_<SECTION>__<KEY>` env overrides. `agent.model` is a models.dev id (`anthropic/claude-sonnet-4-5`); `/model` rewrites that one line in place.
- `credentials.yaml` — `provider: key` (genai adapter names), mode 0600; beats env vars. A provider with a key (or its env var) is "configured": `/model` lists only those.
- `models.json` — optional, same shape as the bundled catalog; adds or overrides models (local `ollama/...`, private gateways). Refresh the bundled data with `just models`.
- `mcp/*.json` — drop in any `mcp.json` (Claude Desktop, Cursor, OpenCode, …); hot-reloaded, tools registered without prefix.
- `mcp/config.yaml` — per-server / per-tool `allow` | `ask` | `deny` gating; `/mcp` edits it (`deny` on a server = disabled).
- `skills/**/*.md` — see [`docs/skills.md`](docs/skills.md).

TUI: `/` opens the command list; `/model` picks a model (header above the
input shows model and context used); `/mcp` toggles servers and tools. See
[`docs/design/02-models-and-menus.md`](docs/design/02-models-and-menus.md).
