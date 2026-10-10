# 158 as built — W5E: theuth as an MCP server (engine)

Design §12, §15.2 W5 rows, §15.5, §18 A2 (write tools, no theuth-side gate). Branch `feat/158-w5e`, cut from the
contract PIN 715e22d3.

## Built

- `crates/mcp-server/src/lib.rs` — frozen types unchanged (`McpServerConfig`, `McpServerStatus`, `AuditLine`,
  `DEFAULT_PORT`); `trait Rpc { call(method, params) }` with a blanket impl over jsonrpsee's `ClientT`, so
  `rpc::client::Local` (in-process, HTTP) and `WsClient` (`t mcp`) both fit with no glue; process state
  (`STATE`: live config + the bound listener's cancellation token; `RPC`: the engine client set once by `boot`);
  `start`/`stop`/`status`/`set`/`rotate_token`/`audit_tail` bodies. `set` writes `mcp_server.enabled` / `.port` with
  `runtime::write_config_value` and binds or drops the listener live; `rotate_token` mints a fresh 64-hex token into
  vault row `mcp.server.token` and restarts the listener (rmcp ends every session when the token fires).
- `crates/mcp-server/src/http.rs` — `TcpListener` on `127.0.0.1:<port>`, hyper 1 `http1::serve_connection` per
  connection, `Authorization: Bearer` compared here (401), path `/mcp` only (404), then
  `StreamableHttpService::handle`. Host/Origin per §12.2 via rmcp's own guard: `allowed_hosts = [127.0.0.1:<port>,
  localhost:<port>]`, `allowed_origins = [http://localhost, http://127.0.0.1]` (any port; absent Origin passes);
  `json_response: false`, `sse_keep_alive: 15 s`, no CORS headers.
- `crates/mcp-server/src/catalog.rs` — `Catalog: ServerHandler` (`instructions` with routing hints), one table of
  `(name, description, schema, read-only)`, `call_tool` dispatch = one RPC per tool. Lists return
  `{items, next_cursor}` with `limit` 1..=200 (default 50) and an offset cursor; RPCs with their own `limit` are asked
  for `offset + limit` rows. Private sessions dropped from `session_list`, refused by `session_read`, their artifacts
  dropped from `artifact_list` (`retention == "private"`). Results pass `Vault::redact_value` / `redact` when a vault
  handle exists. Every call appends an `AuditLine`.
- `crates/mcp-server/src/audit.rs` — `<data_dir>/mcp/server-audit.jsonl`, `O_APPEND`, rotated to `.jsonl.1` past
  50 MB, `tail(limit)` newest first, `args_sha256` over key-sorted JSON.
- `crates/theuth/src/mcp.rs` — `run(data_dir, listen, session: Option<String>)`: `--session` → the unchanged `Shim`;
  none → `mcp_server::Catalog` over the same `WsClient` on `rmcp::transport::stdio()`, same audit file, transport
  `stdio`. `crates/theuth/src/main.rs` — `Cmd::Mcp { session: Option<String> }`; the one boot hook after
  `start_plugins`: `mcp_server::boot(&rt, Arc::new(rpc::client::Local::new(rt.clone()))).await`.
- `crates/theuth/Cargo.toml` — `mcp-server` dependency.

## The catalogue (frozen; 43 tools = 25 read + 18 write)

Read (25): `meeting_list`, `meeting_get`, `meeting_transcript`, `meeting_summary`, `meeting_briefs`, `meeting_search`,
`meeting_speakers`, `brief_search`, `brief_get`, `brief_list`, `knowledge_search`, `knowledge_get`, `knowledge_list`,
`doc_list`, `doc_markdown`, `notebook_list`, `notebook_items`, `notebook_search`, `session_list`, `session_read`,
`task_list`, `task_get`, `artifact_list`, `calendar_events`, `calendar_sources` (the 24 of §15.5 + `calendar_sources`,
which the calendar write tools need to name an id).

Write (18, A2): `meeting_edit` (`meeting_edit`), `meeting_set_project`, `meeting_set_event`, `meeting_set_template`;
`doc_create`, `doc_rename`, `doc_delete`; `knowledge_put` (`knowledge::Put` flattened), `knowledge_delete`;
`task_create` (`tasks::TaskCreate`), `task_update` (`id` + `tasks::TaskPatch` flattened), `task_move`, `task_comment`;
`brief_put` (`meeting::CachedBriefPut`), `brief_delete`; `calendar_source_add`, `calendar_source_delete`,
`calendar_source_refresh`. Each is the RPC of the same name with positional params. Names + schemas are pinned in
`crates/mcp-server/tests/catalog.snapshot.json` (`MCP_SNAPSHOT=1` rewrites after a deliberate change).

## Deviations

- `session_read` pages with the common `limit`/`cursor` (index into `history`) instead of §12.4's `from_turn`; the whole
  history is loaded per page (`ponytail:` comment; `before`/`limit` by store seq if a session outgrows one load).
- `meeting_set_project` / `meeting_set_event` / `meeting_set_template` / `calendar_source_*` / `calendar_events` are
  wired but their RPC bodies are W2–W4 stubs in this worktree (`158 Wn: not built yet`) — they return that text as
  the tool error until those waves land. `meeting_list(project)` is carried, not applied, until W3.
- `t mcp` (stdio) has no vault handle, so results are not redacted there; it is the user's own RPC view over the
  user's own token, the same trust boundary as `t` itself. Audit lines from it carry `transport: stdio`.
- Bearer check and 404 are ours (rmcp's `auth` feature is OAuth, not a static token); Host/Origin are rmcp's.
- `boot` logs a warning instead of failing the engine when the port is busy (a second engine on the same data dir);
  `set` surfaces the bind error to the UI.
- `start`/`stop` keep the frozen `(&Runtime)` signatures; the engine client arrives through `boot` (a `OnceLock`,
  runtime input), not a new parameter.

## Libraries

- rmcp 3.2.0 `server` + `transport-streamable-http-server` (`StreamableHttpService`, `LocalSessionManager`, Host/Origin
  guard, SSE) — the protocol; nothing hand-rolled.
- hyper 1 + hyper-util `TokioIo` for HTTP/1 on the raw listener (already in the graph via jsonrpsee); no axum/tower-http.
- jsonrpsee `ClientT` as the `Rpc` seam (`rpc::client::Local`, `WsClient`) — rejected a hand-written `Methods` wrapper.
- sha2 (args hash), chrono (RFC 3339), uuid v4 (token, as `rpc/lib.rs:262`), parking_lot (state mutex), tokio-util
  (`CancellationToken`), http-body-util (`Full`/`BoxBody` for the 401/404 bodies). All workspace deps already.
- New direct deps of `crates/mcp-server`: `hyper`, `hyper-util`, `http-body-util` at the lockfile's versions (no new
  crates in `Cargo.lock`).

## Hand-offs

- UI (W5 Settings › MCP, not in this brief): `mcp_server_get/set/rotate_token/audit` are live; `McpServerStatus.token`
  is filled once the listener has been enabled once (or `rotate_token` ran). "What is shared" copy in §12.6 should
  say read + write now (A2).
- `docs/design/158-meetings-around.md` §12.4/§15.5: the frozen list above (43 tools) — doc owner to paste.

## Gates (BUILD_LABEL=158w5e, CARGO_BUILD_JOBS=4)

- `build-lock mbx test -p mcp-server -p theuth --no-run` → ok; `mbx test -p mcp-server -p theuth` → mcp-server lib 3
  passed, `tests/catalog.rs` 4 passed (snapshot 43 tools / 25 read-only; paging; Private filter + error mapping;
  Bearer 401 / Host 403 / Origin 403 / loopback-Origin 200 + rmcp `StreamableHttpClientTransport` round trip with
  `list_tools` = 43, `meeting_list`, `task_comment`, `meeting_summary`, 3 audit lines with `client = test-client`,
  listener gone after cancel); `t` 3 passed (`tools_proxy_to_the_engine`: `t mcp --session` byte-identical).
- `build-lock mbx clippy -p mcp-server -p theuth` → clean (0 warnings).
- Live smoke, scratch engine `t --data-dir /tmp/158w5e-data --headless` (`rpc.listen 127.0.0.1:27777`,
  `mcp_server { enabled: true, port: 27788 }`, `secrets.key_store: file`): log `mcp server listening port=27788`;
  curl: no bearer → 401, foreign Host → 403, foreign Origin → 403, initialize → 200 + `mcp-session-id`;
  `tools/list` 43; `task_create` over MCP → row visible to `task_list`; `t mcp` on stdio: `tools/list` 43,
  `task_get` returns the row, audit line `transport: stdio, client: stdio-smoke`; RPC `mcp_server_set {enabled:false}`
  → connection refused, `{enabled:true}` → 401 again; `mcp_server_rotate_token` → old token 401, new token 200,
  `config.yaml` rewritten in place; `mcp_server_audit(2)` returns the newest lines.

## Open items

- Live proof with Claude Code (§16 W5 row) needs Sky's engine + Settings copy block — orchestrator/UI wave.
- `t mcp` without a vault: redaction only on the HTTP path (see Deviations).
