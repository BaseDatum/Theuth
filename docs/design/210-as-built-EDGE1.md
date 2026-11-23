# 210 as built — EDGE1 phase (`share-edge` core, no ACME)

Design: `docs/design/210-public-share.md` rev 8, contract `docs/design/210-as-built-CONTRACT.md`. This phase
builds the standalone edge binary `theuth-share-edge` up to M1 scope without ACME, plus `deploy/share-edge/`.

## Files

| path | what |
|---|---|
| `crates/share-edge/Cargo.toml` | lib `share_edge` + bin `theuth-share-edge`; deps `share-proto`, `platform` and external crates only |
| `crates/share-edge/migrations/0001_init.sql` | every table of the §5.8 edge DDL (incl. `cert_jobs`, `buckets`, `counters`, `usage`, `dns_records` for later phases) |
| `src/config.rs` | figment YAML + env (`THEUTH_SHARE_EDGE_`, `__`), secret rules (§8.3) |
| `src/store.rs` | sqlx sqlite store over that schema |
| `src/state.rs` | `Edge` (shared state), `Tunnel`, `Route`, `Session`, network keys |
| `src/control.rs` | `theuth/share/1` `ProtocolHandler` and the axum control API over hyper http1 per bi-stream |
| `src/sni.rs` | TCP visitor listener: ClientHello peek, routing, PROXY v2, splice |
| `src/admin.rs` | admin Unix socket server and client |
| `src/lib.rs` | `start`/`run`, endpoint key, ticket, `/healthz` + `/metrics` |
| `src/main.rs` | clap CLI |
| `tests/visitor.rs` | in-process integration test |
| `deploy/share-edge/` | `README.md`, `Dockerfile`, `compose.yaml`, `theuth-share-edge.service`, `config.example.yaml`, `k8s/base/{kustomization,deployment,pvc,service,configmap}.yaml` |

Verification: `cargo clippy -p share-edge --all-targets -- -D warnings` clean (covers `cargo check`);
`cargo test -p share-edge` 10 passed: config secret rules (inline refused, file+env refused, file or env alone,
durations and appendix keys, missing domain), SNI (`acme-tls/1` reset, lowercase routing with exact bytes kept,
deep/foreign/non-TLS refused, PROXY v2 TLVs), and `tests/visitor.rs` (an engine endpoint gets 403 before
`hello`, then `info`, `hello` with a signed delegation, claims a random name, gets 403 for a host under a name
it does not hold, puts `x.<name>.tun.zip`; an unknown host's visitor is reset; the routed visitor's bytes arrive
on a bi-stream as a PROXY v2 preamble with the visitor's address, SNI and conn-id TLVs, then the exact
ClientHello).

`cargo tree -p share-edge -e normal`: the only path crates are `share-edge`, `share-proto`, `platform` and the
workspace's patched `vendor/libsqlite3-sys` (through `sqlx`); no `runtime`, `mesh`, `secrets`, `replica` or plugin.
The Docker image was not built (no docker run in this phase).

## Public API (`share_edge`)

```rust
pub fn start(cfg: Config, secrets: Secrets) -> impl Future<Output = anyhow::Result<Running>>;
pub fn run(cfg: Config, secrets: Secrets) -> impl Future<Output = anyhow::Result<()>>;   // start, SIGTERM, shutdown
pub struct Running { pub edge: Arc<Edge>, pub tcp_addr: SocketAddr, pub router: iroh::protocol::Router, .. }
impl Running { pub fn endpoint_addr(&self) -> EndpointAddr; pub async fn shutdown(self) }
pub fn load_or_create_key(path: &Path) -> anyhow::Result<SecretKey>;      // <data_dir>/iroh.key, 0600
pub fn ticket(cfg: &Config, id: PublicKey) -> anyhow::Result<ShareEdgeTicket>;

config::{Config, IrohConfig, AdminConfig, AcmeConfig, CasConfig, GtsConfig, DnsConfig, Limits, Secrets,
         SecretSource { File(PathBuf), Env(String) }, ENV_PREFIX, DEFAULT_CONFIG}
Config::load(&Path) -> Result<(Config, Secrets)>; Config::from_yaml(&str, env: &dyn Fn(&str) -> Option<String>)
SecretSource::read() -> Result<String>           // a file is re-read on every call
state::{Edge, Tunnel, Route, Session, now_ms, net_key, conn_net, VERSION}
store::{Store, NameRow, MeshRow, EngineRow, NewName}
control::ShareProtocol { edge }                  // iroh ProtocolHandler
sni::{peek_client_hello, route_host, proxy_preamble, serve, Peeked, Refuse}
admin::{bind, serve, handle, request}
```

## CLI

`theuth-share-edge [--config <path>] run | ticket | acme register [--ca letsencrypt|gts] |
admin [--json] [--ref R] status | list [names|meshes|tunnels] | ban <name|mesh|key|net> <value> --reason R |
unban <kind> <value> | admission <open|invite|closed> | pause | resume`. Admin results print as YAML.

## Behaviour

- **Admission at connect**: `pause` → close; banned engine key → close; the pre-`hello` semaphore
  (`limits.pre_hello_conns`) → close "busy"; no valid `hello` within `limits.hello_timeout_secs` → close.
- **`hello`**: `client_ok` (426); `Delegation::verify` with the clock codes and `now_ms`; `mesh != engine`; one
  mesh per connection; bans on mesh, key, net; `rekeyed_to` → 409 `mesh_rekeyed {to}`; revoked engine; for an
  unknown mesh `closed` → `admission_closed`, `invite` → `invite_required` unless the invite's hash has uses left
  (consumed, mesh marked invited); `engines_per_mesh` over attached tunnels. Records `meshes`, `mesh_engines`,
  `names.last_attach_ms`. One tunnel per engine key: a newer `hello` replaces and closes the old connection. At
  delegation expiry without a renewed `hello`: `kill {delegation_expired}`, routes dropped.
- **Names**: `GET` availability; `POST /v1/names` random (`check_random`; quota: only while the mesh holds no
  name); `PUT` reserved (`check_reserved` with `reserved_words`, `check_brand` with `brand_blocklist_file`;
  quota `reserved_per_mesh`); first claim wins (`name_taken`), `cooldown {reclaim_after_ms}` for anyone but the
  previous owner; `terms_required` until a `hello` carried the current `terms_rev`; `DELETE` sets
  `reclaim_after_ms = max(now + reclaim_cooldown, not_after)` and drops the name's routes.
- **Routes**: `PUT /v1/routes` all or nothing: `parse_host` on each host, the name must be held by the mesh
  (`not_owner`), ≤ `routes_per_tunnel` (`quota`), conflicts with another engine → 409 `route_conflict` with every
  conflict; then this engine's set is replaced; changes go out as `routes` events to the mesh's tunnels.
- **Events**: SSE from a per-connection broadcast (`routes`, `kill`); the event request is exempt from the
  30 s request deadline.
- **Mesh**: `GET /v1/mesh`, `POST /v1/mesh/stop` (stores `stop_all_ms`, drops routes, `kill {stop_all}` to
  every tunnel), `POST /v1/mesh/revoke` (appends `revoked_ms`, `kill {revoked}` to those engines).
- **Visitor path**: global semaphore, per-IP `governor` rate and per-IP count (IPv4 or IPv6 /64); ClientHello
  peek ≤ 16 KiB / 5 s; reset on `acme-tls/1`, no SNI, host outside `D` or deeper than one label, unknown host,
  ≥ `conns_per_tunnel` open streams, or `open_bi` over `open_bi_timeout_secs`; PROXY v2 (source/destination,
  `TLV_AUTHORITY` = lowercase host, `TLV_CONN_ID`); replay; `copy_bidirectional` through a counting wrapper
  with an idle reset at `idle_secs`; a stream reset and TCP RST on error; one `conn_log` row per visitor.
- **Admin socket**: 0600, peer uid must equal the socket file's owner; NDJSON `AdminRequest`/`AdminResponse`;
  `status`, `list`, `ban` (name → drop its routes with `kill {host, edge_ban}`; mesh/key/net → close tunnels with
  `kill {edge_ban}`), `unban`, `admission` (live), `pause`, `resume`; mutating commands go to `audit` with `ref`.

## Deviations from the design, and why

- **`names.owner` keeps the previous owner while `released`** (DDL comment says "NULL once released"): §5.2 lets
  only the previous owner claim before `reclaim_after_ms`, which needs that id; every "held" query filters on
  `state`. Commented in the migration.
- **Not-yet endpoints** (`csr`, `cert`, `transfer`, `rekey`) answer `501` with `{code: internal, message: "… is
  not available on this edge yet"}`: share-proto has no `not_implemented` code; the status is overridden.
- **`min_client`** is a config key (default `0.0.0`); the appendix's example has none, but `info`/`welcome`
  carry it.
- **`admission` set by `admin admission` is live only**; a restart returns to the config value (no table holds
  it; `admin reload` is M4).
- **`closed` admission** refuses only meshes the edge has not seen; known meshes keep attaching (`pause` is the
  hard stop). The design does not say which.
- **Config ignores unknown keys** so the appendix's full example (with M3/EDGE2 keys) loads; a typo is silent.
- **Peer uid check** compares with the socket file's owner instead of `geteuid()` (no `unsafe`, same result:
  the process created the file).
- **`/healthz` and `/metrics`** are served (`admin.http`) with three gauges only, because the k8s readiness
  probe needs them.

## Not built (hooks named in code)

- EDGE2: ACME, DNS records, CSR/cert endpoints, `acme register` (stub that errors), reconciliation,
  release-time record deletion and revocation (`TODO(EDGE2)`).
- M2: transfer, rekey (`TODO(210-M2)`); revoke is built.
- M3: `/v1/info` and name-check rate limits, paced buckets, `edge_full`/`names_max`, per-network limits,
  relayed newcomers, byte budgets, shaping, egress throttle, the 5 s write-stall reset, `stats` events,
  `conn_log` purge (`TODO(210-M3)`).
- M4: `drain` on shutdown, pkarr publication, `retire`, `ticket --standby`, invites (`invite new`), the other
  admin commands (`queue`, `mesh`, `release`, `evict-unpublished`, `recover`, `caa-check`, `revoke-cert`,
  `reload`, `brake-status` answer `invalid … not available`), full metrics (`TODO(210-M4)`). Allowlist
  admission is P2 (`TODO(210-P2)`).
- `just share-edge-docker-push` recipe (justfile not owned); lines below.

## For the orchestrator

- `Cargo.lock` changes (new direct deps of `share-edge`: `ppp`, `governor`, and the lock entries they pull).
  Not committed here (not an owned path).
- justfile recipe, modelled on `relay-docker-push`:

```just
# Build and push the share edge image (210 §8.4).
share-edge-docker-push tag="latest":
    docker buildx build --platform linux/amd64,linux/arm64 -f deploy/share-edge/Dockerfile \
      -t ghcr.io/skymoore/theuth/share-edge:{{tag}} --push .
```

## Open doubts

- `Tunnel::net` is taken once at connect from the selected path; a relayed connection that later goes direct
  keeps `relay:<url>` (M3's re-count rule needs path events).
- `RecvStream`/`SendStream` are joined with `tokio::io::join` for hyper and the splice; a visitor-side RST maps
  to `SendStream::reset(0)`; the engine side's handling of a reset mid-ClientHello is untested.
- `Delegation::verify` accepts a delegation whose `issued_ms` is up to 5 min in the future; the expiry timer
  uses the edge clock only.
- The Dockerfile drops the workspace `[patch.crates-io]` block, as the relay's does, so the image builds stock
  `libsqlite3-sys` 0.36 (sqlx's bundled sqlite) instead of `vendor/libsqlite3-sys` (SQLCipher over BoringSSL).
  That is intended for the edge, but `Cargo.lock` then re-resolves inside the build; unverified.
