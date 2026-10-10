# 158 as-built — W5U (desktop UI: Settings › MCP server)

Design: `158-meetings-around.md` §12.6 (+ A2: tools are read and write). Branch `feat/158-w5u`, cut from C158 `ccc48db5`.

## Built

- `crates/desktop/ui/src/components/settings/Mcp.tsx` — the real pane, `export const ready = true` (the `mcp` tab now shows).
  - Switch "theuth as an MCP server" → `mcp_server_set({ enabled, port })`; status line under it (`Dot` + `running on <url>` /
    `starting…` / `off`).
  - Port field (number, 1024..65535 via `shared.bounded`, default from the engine = 7788), committed on blur → `mcp_server_set`.
  - Token row: first 8 chars + `…` (never the full token), **Rotate** → `mcp_server_rotate_token`, toast on success.
  - "Copy MCP config": client `Select` (Claude Code / Codex / Cursor / stdio) + Copy → `navigator.clipboard.writeText`.
  - "Recent calls": `mcp_server_audit(20)` as a table (time via `lib/time.formatAt`, client, tool, transport, ok/error, ms).
  - Closing line: "What is shared: meetings, notes, knowledge, notebooks, sessions, tasks, artifacts — read and write. Private
    sessions are never shared." (A2 wording, replaces §12.6's "read-only").
  - Query keys are local to the file (`['mcp-server']`, `['mcp-server','audit']`): `lib/rpc.ts` is not mine.
- `crates/desktop/ui/src/lib/mcp-config.ts` — `mcpConfig(client, status, dataDir)`: Claude Code and Cursor emit the §12.6
  `mcpServers` JSON (`type: http`, `url` from the status, `headers.Authorization: Bearer <token>`); Codex emits
  `config.toml` (`[mcp_servers.theuth] url = …` + `[mcp_servers.theuth.http_headers] Authorization = …`, Codex's documented
  streamable-HTTP form); stdio emits `{ command: "t", args: ["--data-dir", <data_dir>, "mcp"] }` with `data_dir` from
  `info()`. `mcpActions(api)` holds the three RPC call shapes so the test exercises them without a DOM.
- `crates/desktop/ui/src/components/settings/Mcp.test.ts` — the required proof (see Gates).
- `crates/desktop/ui/package.json` — `smol-toml ^1.9.1` (BSD-3-Clause) added; `bun.lock` updated by `bun install`.

## Libraries

- `smol-toml` (new, 1.9.1, BSD-3-Clause, ~zero deps, actively maintained) — TOML `stringify` for the Codex block and `parse`
  in the test. Checked first: no TOML library in `package.json` (`@codemirror/lang-yaml` is a YAML editor mode only).
  Rejected `@iarna/toml` (unmaintained since 2020) and `toml` (parse-only).
- JSON: `JSON.stringify`. Clipboard: `navigator.clipboard` (as `General.tsx`, `Link.tsx`). Time: existing `lib/time.formatAt`.

## Deviations

1. stdio `command` is `"t"` (on PATH), not `/path/to/t`: the engine does not report its binary path (`Info` has `data_dir`
   only). Marked `ponytail:` in `mcp-config.ts`; upgrade = an `Info.binary` field (Rust, not mine).
2. A missing token (`token: null`, nothing generated yet) is written as `Bearer <token>` so the copied block stays valid.
3. The Codex form uses literal `http_headers.Authorization`, not `bearer_token_env_var`: the brief asks for the token filled in.

## Hand-offs

- None required. Optional: `rpc/src/api.rs` `Info.binary` (path of `t`) so the stdio block needs no hand edit.

## Open items

- None.

## Gates (in `crates/desktop/ui`, after `.ui-ready`)

- `BUILD_LABEL=158w5u build-lock bun install` → `+ smol-toml@1.9.1`, lockfile saved.
- `BUILD_LABEL=158w5u build-lock bunx tsc --noEmit -p .` → exit 0, 0 errors (first run: 2 errors — `McpAuditLine.ms` is
  `bigint` in the bindings; test fixtures fixed to `12n`/`3n`).
- `BUILD_LABEL=158w5u build-lock bunx vitest run src/components/settings/Mcp.test.ts` → 1 file, 5 tests passed:
  §12.6 JSON per client (token filled, port respected, Cursor ≡ Claude Code, `null` token → `<token>`); Codex TOML parses back to
  `{ mcp_servers.theuth.{url, http_headers.Authorization} }`; stdio block; `mcpActions` → `mcp_server_set` {enabled,port} ×2 and
  `mcp_server_rotate_token`; `ready === true` and the pane renders the status line, masked token (full token absent), audit rows
  (`mcp_server_audit(20)`) and the shared line.
- Codex output as emitted: `[mcp_servers.theuth]` / `url = "http://127.0.0.1:7788/mcp"` / `[mcp_servers.theuth.http_headers]` /
  `Authorization = "Bearer <token>"`.
