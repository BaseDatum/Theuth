# 210 as built — CLIENT1 phase (`crates/share`, M1)

Design: `docs/design/210-public-share.md` rev 8; contract: `210-as-built-CONTRACT.md`. This phase builds the
client plugin against `share-proto`, without wiring it into `rpc-compose` or `theuth`.

## Files

| path | what |
|---|---|
| `crates/share/Cargo.toml` | deps: `share-proto`, `rpc-core(+macros)`, `jsonrpsee`, `runtime`, `wire`, `tools`, `platform`, `replica`, `iroh`, `iroh-base`, `rcgen`, `ppp`, `kameo`, `sqlx`, `hyper` 1 (`client server http1 http2`), `hyper-util` 0.1.21 (`server-auto client-legacy service`), `http-body-util`, `tokio-rustls` 0.26.6 (`aws_lc_rs`), `rustls` 0.23 (`aws_lc_rs`), `arc-swap` 1.9.3, `argon2` 0.6.0, `cookie` 0.18.2 (`signed`), `tokio-util` (`io`), `mime_guess`, `libc`, `uuid`, `data-encoding`; feature `ts` |
| `crates/share/migrations/0001_share.sql` | §5.8 client DDL verbatim (4 replicated + 3 local tables) |
| `src/lib.rs` | modules, `SharePlugin`, `declare_wire`, `rpc_module`, `rpc_support::engine()` |
| `src/config.rs` | `ShareConfig`, `Expiry`, `EdgeChoice`, `DEFAULT_EDGE_TICKETS` (placeholders), `default_edges()` |
| `src/model.rs` | `Share` record (§5.1), `Source`, `Access`, `ShareState`, `StoppedBy`, `CreatedBy`, `ApprovedBy`; events `ShareChanged`, `ShareTraffic`, `ShareStatus` + `EdgeState` |
| `src/store.rs` | `Store` over `share.sqlite`: switch, edge, terms, edge_state, shares, share_secrets |
| `src/edge.rs` (+ `edge/tests.rs`) | `EdgeConn`, `EdgeDialer`, `DelegationSource`, `SelfHolder`, `Control` client, `SseParser`, `Attachment`, `Client` state machine (§5.9) |
| `src/visitor.rs` | PROXY v2 preamble (`ppp`), `Certs` (`ResolvesServerCert`, one label stripped), `certified_key`, `acceptor`, `serve` (hyper-util auto, upgrades) |
| `src/proxy.rs` | `Proxy` service: routing, 421, gate, header policy, Location rewrite, upstream with upgrades, `dir` serving |
| `src/routes.rs` | `Table` (`ArcSwap<HashMap<host, Vec<Mount>>>`), longest prefix, `Lookup::{Misdirected, NotFound, Found}` |
| `src/sources.rs` | `PortGuard`, `OwnPorts`, `ProcessCheck`, `check_root`, `resolve`, `open_nofollow` |
| `src/gate.rs` | interstitial + password gate: argon2id, signed `__Host-` cookies, rate limit, bounded hashing |
| `src/keys.rs` | `name_key` (rcgen P-256 + CSR for `{<name>.D, *.<name>.D}`), `NameKey`, `NameKeyStore`, `ensure` |
| `src/tools.rs` | `share_publish`, `share_stop`, `share_list`, `plan()` validation, `HUMAN_ONLY_TOOLS` |
| `src/rpc_share.rs` | `ShareApi` RPC trait |
| `src/engine.rs` | `Engine`: publish, stop, Stop all, expiry, cert install, `Attachment` impl |

## Public API (summary)

- `SharePlugin { engine_id: String, hooks: Hooks }` implements `runtime::Plugin` (name `share`).
- `Hooks { dialer: Option<Arc<dyn EdgeDialer>>, delegation: Option<Arc<dyn DelegationSource>>,
  name_keys: Option<Arc<dyn NameKeyStore>>, own_ports: Arc<dyn OwnPorts>, process: Arc<dyn ProcessCheck> }`.
- `edge::EdgeDialer::dial(addr: EndpointAddr, alpns: &[&[u8]]) -> Arc<dyn EdgeConn>`; `EdgeConn` is implemented for
  `iroh::endpoint::Connection`.
- `edge::DelegationSource::delegate(edge, domain, edge_now_ms) -> Signed`; `SelfHolder { mesh_key, engine }` is the
  M1 implementation (signs with `mesh/share_key`).
- `keys::NameKeyStore::{get, create}` over the vault row `share/<domain>/<name>/tls` (`NameKey {gen, key_pem, csr_der}`).
- `sources::OwnPorts::is_own(port)`, `sources::ProcessCheck::owned_by_theuth(port) -> Option<bool>`.
- Events: `ShareChanged` topic `share.changed`, `ShareTraffic` topic `share.traffic`, `ShareStatus` topic `share.status`.
- Tools: `share_publish` (`Risk::Destructive`, `DIRECT`, not promotable, returns `{id, url, expires_ms}`),
  `share_stop {id | "all"}` (`ReadOnly`), `share_list {}` (`ReadOnly`).
- RPC (`ShareApi`, owner audience): `share_list`, `share_stop {id}`, `share_publish {args}`, `share_settings_get`,
  `share_settings_set {enabled?, edge?}`, `share_names_check {name}`, `share_terms_accept {edge, terms_rev}`.

## Wiring lines for the orchestrator (not applied)

- root `Cargo.toml` `[workspace.dependencies]`: `share = { path = "crates/share" }` (if not already present).
- `crates/rpc-compose`: add `share::rpc_module(server.clone())` to the merged modules, and `share/ts` to the `ts`
  feature chain where plugins list theirs.
- `crates/theuth` plugin list: `Box::new(share::SharePlugin { engine_id: <mesh identity public key string>,
  hooks: share::Hooks { dialer: Some(<adapter over mesh Net::dial_alpn>), delegation: Some(Arc::new(
  share::edge::SelfHolder { mesh_key: <vault mesh/share_key>, engine: <endpoint id> })), name_keys: Some(<vault
  adapter>), own_ports: <runtime registry adapter>, process: Arc::new(share::sources::NoProcessCheck) } })`.
- wire schema snapshot: three new topics `share.changed`, `share.traffic`, `share.status`.

## Seams (traits other crates implement or wire)

| trait / hook | implemented by | TODO tag |
|---|---|---|
| `edge::EdgeDialer` | `crates/mesh` `Net::dial_alpn` + adapter in the wiring (`max_concurrent_bidi_streams(256)`, no `join_network`) | `TODO(210-M1, crates mesh + rpc-compose)` |
| `edge::DelegationSource` | M1: `SelfHolder` here, key from `crates/secrets`; M2: `share.delegate` on the lease holder | `TODO(210-M1, crate secrets)`, `TODO(210-M2, crates agent/mesh)` |
| `keys::NameKeyStore` | `crates/secrets` (`share/` prefix in `writable_name`, `set_if_version`) | `TODO(210-M1, crate secrets)`, `TODO(210-M2)` |
| `sources::OwnPorts` | `crates/runtime` listener registry | `TODO(210-M1, crate runtime)` |
| `sources::ProcessCheck` | socket-table crate (`listeners`/`netstat2`) — needs a new dependency | `TODO(210-M1)` |
| `HUMAN_ONLY` on `SharePublishTool` | `crates/tools` adds `const HUMAN_ONLY` to `Tool`; until then `tools::HUMAN_ONLY_TOOLS` lists `share_publish` | `TODO(210-M1, crate tools)` |
| approval id/surface in `approved_by`; RPC approval rows for publish, switch-on, edge change, terms | `crates/tools` | `TODO(210-M1, crate tools)` |

## Deviations from the design or brief, and why

- **Event names**: the brief lists "SharePublished, ShareStopped, status"; the design (§7.4, normative) names
  `share.changed@1` (full record on every change) and `share.traffic@1`. Built as the design: `ShareChanged`,
  `ShareTraffic`, plus `ShareStatus` (`share.status`) for the §5.9 connection state Settings shows. Publish and stop
  are both `ShareChanged` with the record's `state`.
- **`EdgeDialer` returns `Arc<dyn EdgeConn>`, not `iroh::endpoint::Connection`**: `EdgeConn` is implemented for
  `iroh::endpoint::Connection` in this crate, so the mesh adapter returns the connection unchanged; the indirection lets
  the state-machine test run against an in-process fake edge over `tokio::io::duplex`.
- **Stream limits** (`max_concurrent_bidi_streams(256)`) are the dialer's duty (it owns the QUIC transport config);
  the trait takes only `addr` and `alpns`.
- **Upstream proxy uses `hyper::client::conn::http1` per request**, not the pooled `hyper-util` legacy `Client`: one
  fresh connection per upstream request makes "re-check on every upstream connect" literal (`PortGuard::connect`) and
  carries `with_upgrades()` for WebSocket. Costs a loopback TCP handshake per request.
- **Gate endpoints** live below each mount at `/.theuth-share/continue` and `/.theuth-share/login` (dot segment, so
  never a `dir` file and never forwarded). Cookies: `__Host-share-seen-<id>` (interstitial) and `__Host-share-<id>`
  (password; value tied to the hash so a new password voids old cookies), `Secure; HttpOnly; Path=/; SameSite=Lax`.
  Continue also requires `Sec-Fetch-Site: same-origin`.
- **RPC method names** are snake (`share_list`) per the repo's RPC convention, for the design's dotted names.
- **Password not in tool output**: `share_publish` returns the generated password only in the RPC `Published` result
  (owner surfaces); the model sees "the user sees the password, you do not".
- **Randomness** (random names, labels, passwords) comes from `uuid::Uuid::new_v4()` bytes (getrandom), not `rand`.
- **`dir` open on macOS** uses `O_NOFOLLOW_ANY` alone (with `O_NOFOLLOW` the kernel answers `EINVAL`, observed), then
  an `fstat` device/inode check against the canonical path; Linux uses `O_NOFOLLOW` + the same check.

## Not built (M1 scope left as TODO hooks, or later milestones)

- `TODO(210-M1)`: live `offline` after 10 s without reconnect (§5.9); restarting the state machine on an edge change;
  keeping the approved expiry when a `pending` share goes live (today the default expiry applies); Linux
  `openat2(RESOLVE_NO_SYMLINKS)`; the process-tree port check (needs a socket-table dependency); the approval rows
  and `HUMAN_ONLY` (crate `tools`); vault adapters (crate `secrets`); the dialer (crate `mesh`).
- Per-share limits of §7.5 (64 concurrent, 100 req/s, idle 60 s / WS 15 min) and visitor stream reset on stop are not
  enforced; the visitor accept loop has no per-tunnel cap (the edge enforces 256).
- `share.traffic` is declared but never emitted (no counters yet); `stats` and `queue` events are ignored (M3).
- RPC: `share_extend`, `share_set_access`, `share_names_reserve|release|primary|rotate|transfer`, `share_delegate`,
  `share_name_key_ensure` (M2); reserved-name claim (`PUT /v1/names/{name}`) is in `Control::claim` but no surface
  calls it yet; `share_names` rows are not written (the welcome's names are the source today).
- Standby-key failover and `welcome.retire` (M4); invite handling beyond passing the ticket's invite in `hello`.
- Stop forwarding to other engines and Stop all hand-off to the lease holder (M2); `removed` does not delete vault rows.
- `artifact` source (P2).

## Open doubts

- `Cargo.lock` is modified by this crate's new packages (`argon2` 0.6 in use, `cookie` `signed`, `hyper-util`
  features) together with other builders' entries (`governor`, …); it is **not committed** here — the orchestrator
  commits the lock once.
- The tool's approval id/surface (`approved_by`) is empty until `crates/tools` passes it; the edge never sees it.
- `share_publish` waits up to 30 s for the edge inside the tool call; a queued certificate leaves the share `pending`
  and the call returns at once with that state.
- Terms: if the accepted revision differs from the edge's, `share_publish` fails with the terms URL and revision; the
  first-run card (desktop) must call `share_terms_accept` then publish again. After acceptance the engine sends a fresh
  `hello` with `terms_rev` itself before `POST /v1/names`.
- No `share-proto` change was needed.
