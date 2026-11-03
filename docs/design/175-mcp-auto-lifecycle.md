# 175 MCP-AUTO-LIFECYCLE: HTTP MCP servers run the newest protocol they support

Branch `fix/175-mcp-auto-lifecycle`, base `origin/main` a206de89. Code: `crates/tools/src/mcp.rs`.

## Problem

The GitHub MCP (`https://api.githubcopilot.com/mcp/`, `Authorization: Bearer $(gh auth token)`)
stopped connecting after dedf2089 (Cargo.lock refresh, rmcp 3.2.0 → 3.8.0). The engine logged
`unexpected server response: HTTP 400 Bad Request: invalid session, when send initialized notification`.

In rmcp 3.8 `ProtocolVersion::default()` is `LATEST` = 2026-07-28 (it was 2025-11-25 in 3.2.0).
theuth's `().serve(...)` runs the legacy `initialize` handshake and so asked for 2026-07-28.
GitHub answered 2025-11-25 and issued an `Mcp-Session-Id`, then rejected the next request, which
carried `MCP-Protocol-Version: 2025-11-25`, with `400 invalid session`.

curl matrix against GitHub (same token, orchestrator, 2026-11-03):

| Request | Response |
|---|---|
| initialize asks 2025-11-25, then notify with 2025-11-25 | answers 2025-11-25; notify 202; tools/list 200 with 49 tools |
| initialize asks 2026-07-28, then notify with 2025-11-25 | answers 2025-11-25; notify **400 invalid session** (= theuth before this fix) |
| same session, notify with `MCP-Protocol-Version: 2026-07-28` + `Mcp-Method: notifications/initialized` | 202 |
| same, without `Mcp-Method` | 400 `-32020 missing required Mcp-Method header` |
| `server/discover` (`MCP-Protocol-Version: 2026-07-28`, `Mcp-Method: server/discover`, `_meta` with protocolVersion / clientInfo / clientCapabilities) | 200, `supportedVersions: ["2026-07-28","2025-11-25","2025-06-18","2025-03-26","2024-11-05"]` |
| session-less `tools/list` with the same 2026-07-28 headers + `_meta` | 200, 49 tools |

## Decisions

- **D1 Lifecycle.** Both HTTP paths in `connect()` (hand-set `Authorization` header and OAuth via
  `AuthClient`) call `().serve_with_lifecycle(transport, http_lifecycle())`. That is rmcp's
  `ClientServiceExt` method, a thin wrapper over `serve_client_with_lifecycle`.
  `http_lifecycle()` returns `ClientLifecycleMode::Auto { preferred_versions, legacy_version }`.
- **D2 Preferred versions.** `ProtocolVersion::KNOWN_VERSIONS` reversed, newest first. rmcp's
  `select_protocol_version` picks the first client-preferred version the server lists, so the
  connection runs the newest version both sides speak. An rmcp bump extends the list by itself.
- **D3 Legacy fallback.** `legacy_version: Some(ProtocolVersion::LATEST_WITH_INITIALIZE)`
  (2025-11-25), never `LATEST`. A legacy server answers its newest version that still has
  `initialize`, so the GitHub-style downgrade cannot recur on the fallback.
- **D4 stdio unchanged.** `().serve(transport)` stays. A local server that ignores
  `server/discover` would cost 10 s per start, and stdio has no HTTP session.
- **D5 No API change.** `Service` stays `Arc<RunningService<RoleClient, ()>>`; nothing public in
  `tools::mcp` changed, so no dependent crate needs checking (gate 5 not triggered).
- **D6 Log line.** `mcp server connected server=… count=… protocol=…`. rmcp keeps which lifecycle
  won private (`set_client_request_metadata` is `pub(crate)`, no getter), so the version is
  logged alone, as the brief allows. A version past `NO_INITIALIZE` (2026-07-28) can only come
  from discover; an older one is almost always the initialize fallback, but a server could also
  list only old versions in its discover answer, so the lifecycle is not inferred.
- **D7, D8**: see below.

Beyond D1–D8:

- **`CONNECT_TIMEOUT` 10 s → 20 s.** `attempt()` bounds connect + `tools/list` by
  `CONNECT_TIMEOUT`, which equalled rmcp's `DEFAULT_AUTO_DISCOVER_TIMEOUT` (10 s). A legacy
  server that never answers `server/discover` would have been cut off exactly when rmcp starts
  its `initialize` fallback, so it could never connect. 20 s leaves room for the probe, the
  handshake and the listing. It only changes how long a dead server takes to be reported
  (stdio included).
- **`axum` dev-dependency** for the in-process servers of the T2 tests (already in the workspace
  for relay/a2a). Cargo.lock changes by one line: `"axum"` in the `tools` package's dependency
  list. No new package, no version change.

## D7: downstream review

| Use | Discover-mode behaviour | Verdict |
|---|---|---|
| `peer_info()` | rmcp sets it from `ServerPeerInfo::from_discover_result(selected, result)`: negotiated version, capabilities, `server_info` (optional), instructions. theuth reads only `protocol_version` (new log line). | fine |
| Server instructions / capabilities | theuth does not read either (grep: no `instructions`/`capabilities` use on the client side). | nothing to change |
| `list_all_tools` pagination | Plain `tools/list` loop over `next_cursor`; in discover mode each request carries the `_meta` and `Mcp-Method` headers rmcp adds for ≥ 2026-07-28. Exercised by T2 (local) and T3 (GitHub, 49 tools). | fine |
| `call_tool` (`McpTool::call`) | Same per-request metadata path. Exercised by T2 (`echo`) and T3 (`get_me`). | fine |
| Reconnect / settle | `attempt()` → `settle()` unchanged; a failed connect is recorded in `errors` and retried on the next reconcile. Errors keep the `ClientInitializeError` type, so the `downcast_ref` in `settle` still works. | fine |
| Cancellation / shutdown / DELETE | Dropping the last `Service` Arc fires rmcp's DropGuard as before. In discover mode no `Mcp-Session-Id` exists (the worker asserts "discover bootstrap must not create session state"), so no session DELETE is sent; legacy fallback sessions keep their DELETE. | fine |
| `McpLogin` / OAuth | `McpLogin` uses the stored `WWW-Authenticate` challenge from `state.login`, independent of the lifecycle. | fine |

## D8: errors

- A 401/403 on the discover probe is excluded from rmcp's `legacy_discover_response` (it keeps
  auth failures on their original path), so it surfaces as a `TransportError` holding
  `AuthRequiredError`. Auto returns it as is (`Ok(Err(error))`); `ClientInitializeError::auth_challenge`
  finds it and `settle` records `<name>: login required (/login <name>)`. A 401 in the fallback
  phase arrives as `LegacyFallbackFailed`, which `auth_challenge` also unwraps.
  Covered by `http_401_on_the_discover_probe_is_login_required` (OAuth path, no stored tokens).
- Other 4xx answers to the probe (e.g. 400 "Mcp-Session-Id header is required") become a
  correlated JSON-RPC error and trigger the legacy fallback (T4, gitmcp.io).
- 5xx and transport errors on the probe are reported as connect failures, not fallen back from;
  that is rmcp's classification and matches the old behaviour of failing the connect.

## Tests

All in `crates/tools/src/mcp.rs` `mod tests` (`connect()` is private, so no `tests/` file).

- **T1** `http_lifecycle_prefers_newest_and_falls_back_to_newest_initialize`: Auto; first is
  `LATEST`; strictly newest → oldest; same set as `KNOWN_VERSIONS`; legacy = `LATEST_WITH_INITIALIZE`.
- **T2** rmcp's own `StreamableHttpService` behind axum on 127.0.0.1:
  - `http_connect_runs_discover_at_the_latest_version` (header-auth path): lands on 2026-07-28,
    lists 1 tool, calls `echo`, and the server saw no `initialize`.
  - `http_connect_falls_back_to_initialize_for_a_legacy_server` (OAuth path, no tokens): server
    supports ≤ 2025-11-25 and answers `server/discover` with -32601; connect lands on 2025-11-25,
    the `initialize` asked 2025-11-25 (D3), lists and calls, all in < 3 s.
  - `http_401_on_the_discover_probe_is_login_required`: see D8.
  - Regression check: with `ClientLifecycleMode::Initialize` swapped back in, both connect tests
    fail (`left: 2025-11-25, right: 2026-07-28`; `asked: 2026-07-28, expected 2025-11-25`).
- **T3** (live, `#[ignore]`) `github_mcp_live_negotiates_the_latest_version`, headers go through
  theuth's `expand` (`Bearer $(gh auth token)`). Output:
  `protocol=2026-07-28 tools=49 login=skymoore`.
- **T4** (live, `#[ignore]`) `legacy_mcp_live_falls_back_fast` against `https://gitmcp.io/docs`
  (no auth; answers discover with 400 `-32000 Mcp-Session-Id header is required`). Output:
  `protocol=2025-03-26 tools=5 connect=903.47375ms`.

Slack and Atlassian were deliberately not live-tested (OAuth token rotation risk); the
orchestrator checks them on the real engine.
