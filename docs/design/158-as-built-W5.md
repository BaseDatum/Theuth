# 158 as built — W5: theuth as an MCP server

Design §12, §15.2 W5 rows, §15.5, §18 A2. Closed on `feat/158-w5e`. Worker notes: `158-as-built-W5E.md` (engine),
`158-as-built-W5U.md` (Settings pane), `158-as-built-CONTRACT.md` (stubs).

## Workers and commits

| Worker | Branch | Commits |
|---|---|---|
| C158 contract | `feat/158-contract` | 715e22d3 (PIN) … ccc48db5; merged at b6676f34 |
| W5E engine | `feat/158-w5e` | 8a00128d, ace450fc, 6b65c2c5, 89d658cf; phase 1 60f15f01; phase 2 08d0603e + close |
| W5U UI | `feat/158-w5u` | c11f9070, 6d8024c0; merged at 25f4b298 |

## What shipped

- `crates/mcp-server` — rmcp 3.2.0 streamable HTTP on `127.0.0.1:<mcp_server.port>` (7788), OFF by default, started from
  the engine boot hook iff enabled and flipped live by `mcp_server_set`; `Authorization: Bearer <vault mcp.server.token>`
  (401), rmcp's Host/Origin guard pinned to the loopback authority (403), SSE replies, 15 s keep-alive, no CORS; one
  `Catalog` (43 tools) over `trait Rpc` (jsonrpsee `ClientT` blanket impl: `rpc::client::Local` in-process); every result
  through `Vault::redact_value`; every call → `<data_dir>/mcp/server-audit.jsonl` (args hashed, 50 MB rotation).
- `t mcp` (no `--session`) — a transport only (`mcp_server::Bridge`): static tool table on stdio, each call forwarded as
  `mcp_server_call(tool, args, client)`; the engine runs the same catalogue with the vault and writes the audit line
  `transport: stdio`. `t mcp --session <id>` byte-identical (75 §9 shim untouched).
- RPC: `mcp_server_get / set / rotate_token / audit / call`; `McpServerStatus` gained `binary` (`current_exe`) so the
  stdio config names this engine's `t`, not an older one on PATH.
- Settings › MCP server (`settings/Mcp.tsx`, `lib/mcp-config.ts`): switch, status line, port, token (8 chars + Rotate),
  "Copy MCP config" for Claude Code / Codex (TOML via smol-toml) / Cursor / stdio, last 20 audit lines, the A2 "read and
  write" closing line.

## The catalogue (final, 43 = 25 read + 18 write)

Read: `meeting_list`, `meeting_get`, `meeting_transcript`, `meeting_summary`, `meeting_briefs`, `meeting_search`,
`meeting_speakers`, `brief_search`, `brief_get`, `brief_list`, `knowledge_search`, `knowledge_get`, `knowledge_list`,
`doc_list`, `doc_markdown`, `notebook_list`, `notebook_items`, `notebook_search`, `session_list`, `session_read`,
`task_list`, `task_get`, `artifact_list`, `calendar_events`, `calendar_sources`.
Write (A2, no theuth-side gate): `meeting_edit`, `meeting_set_project`, `meeting_set_event`, `meeting_set_template`,
`doc_create`, `doc_rename`, `doc_delete`, `knowledge_put`, `knowledge_delete`, `task_create`, `task_update`, `task_move`,
`task_comment`, `brief_put`, `brief_delete`, `calendar_source_add`, `calendar_source_delete`, `calendar_source_refresh`.
Lists return `{items, next_cursor}` (`limit` 1..=200, default 50). Private sessions are dropped from `session_list` /
`artifact_list` and refused by `session_read`. Names + schemas pinned in `crates/mcp-server/tests/catalog.snapshot.json`.

## Gates (close, BUILD_LABEL=158w5e)

- `build-lock mbx test -p mcp-server -p theuth -p rpc --no-run` → ok; `mbx test -p mcp-server -p theuth -p rpc` → all
  green (mcp-server 3 + 5, rpc 56 + 1 + 1, t 3).
- `build-lock mbx clippy -p mcp-server -p theuth -p rpc` → clean.
- `BUILD_SLOTS=1 build-lock mbx test -p rpc --features ts --lib --quiet export_bindings` → 8 passed; bindings copied
  (`McpServerStatus.binary`, `McpCallResult`); `api.ts` gained `mcp_server_call` (its METHODS test pins `methods.json`).
- `build-lock bun install` (ui, and `packages/apps` for `@openuidev/*`); `build-lock bunx tsc --noEmit -p .` → 0;
  `build-lock bunx vitest run` on `settings/Mcp.test.ts`, `lib/mcpEntry.test.ts`, `rpc/api.test.ts` → 3 files, 10 passed.
- `BUILD_SLOTS=1 build-lock mbx check --workspace --tests` → 0 (only the pre-existing `block v0.1.6` future-incompat note).

## Live proof (2026-10-10 16:05, free memory 7.3 GB)

Scratch headless engine from the rebuilt `target/debug/t` (`rpc.listen 127.0.0.1:27777`, `mcp_server { enabled: true,
port: 27788 }`, file key store); vite dev on `127.0.0.1:25173` with `VITE_THEUTH_URL/TOKEN`; headless Chromium.
- Settings › MCP server: switch on, `running on http://127.0.0.1:27788/mcp`, port 27788, token `11591c26…` + Rotate,
  client picker, Copy; screenshot `docs/design/assets/158-settings-mcp-live.png` (after the calls below: the audit table
  shows `copied-stdio-block task_list stdio ok` and `copied-http-block task_create http ok`).
- Copied blocks (clipboard intercepted in-page): Claude Code = `{type: http, url: http://127.0.0.1:27788/mcp,
  headers.Authorization: Bearer <64 hex>}`; stdio = `{command: "/Users/…/theuth-158-w5e/target/debug/t", args:
  ["--data-dir", "/tmp/158w5e-data", "mcp"]}` — the engine's own binary, not `/opt/homebrew/bin/t` (2.6.0).
- The Claude Code block driven verbatim over HTTP: initialize 200, `tools/list` 43, `task_create` with a vault secret in
  the title → the reply already reads `MCP proof {{secret:proof.key}}`.
- The stdio block spawned verbatim (`command` + `args`): `serverInfo.name = theuth`, 43 tools, `task_list` returns the
  same redacted title; the secret string appears nowhere in the stdio bytes.
- Real MCP client: none available without touching Sky's own config (no `claude`/`codex` CLI on this Mac; Cursor.app
  would read his user config) — skipped; the two copied blocks were exercised directly instead.

## Deviations

- `session_read` pages by index with the common `limit`/`cursor` (not `from_turn`); whole history per page.
- `meeting_set_project` / `meeting_set_event` / `meeting_set_template` / `calendar_*` are wired but their RPC bodies are
  W2–W4 stubs on this branch; they return the stub text as the tool error until those waves land.
- Codex block uses literal `http_headers.Authorization` (token filled in), not `bearer_token_env_var`.
- `boot` warns instead of failing the engine on a busy port; `set` surfaces the bind error.

## Open items

- None blocking. Per-meeting restriction (`Manifest.restricted`) stays in §14.
