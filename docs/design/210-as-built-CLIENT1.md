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
