# wth

A single-process AI harness that does things for the user. Plugins are Rust
crates talking over a typed message bus; any frontend attaches over JSON-RPC.
Design: [`docs/design/00-architecture.md`](docs/design/00-architecture.md).

```sh
just setup                 # rustup toolchain; `just toolchain` prints its bin dir
export ANTHROPIC_API_KEY=… # any genai provider works; default model in config
just run                   # terminal UI; data dir is $XDG_DATA_HOME/wth (macOS: ~/Library/Application Support/wth)
just run -- --headless     # JSON-RPC only, ws://127.0.0.1:7777 with <data_dir>/token
just run -- --resume <id>  # reopen a session
just ts                    # TypeScript bindings for the wire types → bindings/
```

Crates: `runtime` (plugin trait), `tools` (registry, approval gate, shell via
brush, fs, MCP), `agent` (genai loop, sqlite sessions, skills, subagents),
`rpc` (jsonrpsee), `tui` (iocraft), `wth` (binary).

Config lives at `<data_dir>/config.toml` (created on first run). Skills are
markdown files under `<data_dir>/skills`; see [`docs/skills.md`](docs/skills.md).
