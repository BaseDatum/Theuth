# 210 as built — WIRE1 (the share plugin wired into the engine)

Design: `docs/design/210-public-share.md` rev 8 (§5.3, §5.5, §5.6, §5.7, §6.2, §7.2, §7.4). Seams: the CLIENT1,
CONSENT1 and MESH1 as-built notes. This phase connects `crates/share` to the engine. It adds no new protocol.

## Files

| path | what |
|---|---|
| `crates/share/src/wiring.rs` (new) | `tunnel_transport()`, `MeshDialer`, `MeshDelegation`, `VaultNameKeys`, `engine_hooks()`, one test |
| `crates/share/src/sources.rs` | own-listener registry: `register_own_port`, `OwnListeners` |
| `crates/share/src/edge.rs` | `EdgeConn::open` for `iroh::endpoint::Connection` bounded by `limits::OPEN_BI_TIMEOUT_MS`; seam docs |
| `crates/share/src/tools.rs` | `SharePublishTool::HUMAN_ONLY = true`; `HUMAN_ONLY_TOOLS` removed; `approved_by` from the call's approval |
| `crates/share/src/engine.rs` | `CallOrigin::Call.approved`; `approved_by` filled from it; no edge client without a ticket |
| `crates/share/src/rpc_share.rs` | `ShareSettings.configured` |
| `crates/share/src/lib.rs` | `pub mod wiring`; `SharePlugin::engine()`; empty `engine_id` → mesh identity at start |
| `crates/share/Cargo.toml` | deps `mesh`, `secrets` (workspace crates, no new package) |
| `crates/tools/src/approval.rs`, `lib.rs`, `registry.rs` | `Approved {id, surface}`, `approved()`, `approved_scope()`; `invoke` scopes the tool body with the approval it passed |
| `crates/rpc-compose/Cargo.toml`, `src/bootstrap.rs` | dep `share`; `serve` registers the RPC listener port |
| `crates/rpc-compose/src/client_mirror.rs` | `ShareChanged`, `ShareStatus`, `ShareTraffic` republished, matched by tag |
| `crates/rpc-compose/src/api/clients.rs`, `src/api/tests.rs` | `ShareApiClient` re-export; `share::rpc_module` in the test composition |
| `crates/rpc-compose/tests/wire_golden.rs`, `wire_parity_inc7.rs` | the three share families |
| `crates/rpc-compose/tests/fixtures/wire/share.{changed,status,traffic}.json` (new) | golden inputs and notifications |
| `crates/theuth/Cargo.toml`, `src/main.rs` | dep `share`; `share::rpc_module` in `rpc_composition`; `SharePlugin::engine()` in `engine_plugins` after `sandbox` (so after `mesh` and `replica`) |
| `crates/theuth/harness.md` | `share:` config line; `share_publish` under "Only the user can" |
| `crates/theuth/tests/fixtures/wire/openrpc-v1.json` | accepted with `just wire-schema-accept` |
| `crates/slack/src/bubble.rs` | `share_publish` in `is_deliverable` (mirrors the desktop's `isDeliverable`) |

## Public API added

```rust
// crates/share
pub mod wiring {
    pub fn tunnel_transport() -> iroh::endpoint::QuicTransportConfig; // 256 peer bidi streams, 256 KiB/stream, 64 MiB conn
    pub struct MeshDialer;                                   // EdgeDialer over mesh::net()?.dial_alpn
    pub struct MeshDelegation;                               // DelegationSource: mesh::share::ensure_share_key + Delegation::sign
    pub struct VaultNameKeys(pub Option<Arc<secrets::Vault>>); // NameKeyStore; None = mesh::net()?.vault()
    pub fn engine_hooks() -> Hooks;                          // all of the above + OwnListeners + NoProcessCheck
}
pub mod sources { pub fn register_own_port(port: u16); pub struct OwnListeners; }
impl SharePlugin { pub fn engine() -> Self; }                // engine_id from mesh::self_id() / stored_self_id
pub struct ShareSettings { pub enabled: bool, pub edge: String, pub configured: bool }
pub enum CallOrigin { Call { session, call_id, approved: Option<ApprovedBy> }, Settings }

// crates/tools
pub struct Approved { pub id: ApprovalId, pub surface: &'static str }   // "owner" for a human-only row, else "human"
pub fn approved() -> Option<Approved>;                                   // inside a tool body
pub async fn approved_scope<F: Future>(Option<Approved>, F) -> F::Output;
```

## Behaviour

- **Dial** (§5.3, D18): `MeshDialer` resolves `mesh::net()` per dial (plugin order does not matter; no mesh →
  "the mesh endpoint is not running"), offers `alpns[0]` with `tunnel_transport()`. The mesh endpoint's own transport
  config is untouched. Control streams: `open_bi` gives up after 5 s (`OPEN_BI_TIMEOUT_MS`) with `TimedOut`.
- **Delegation** (§5.7, M1): `MeshDelegation` reads `mesh/share_key` from the vault behind the mesh endpoint; when the
  row is missing only the synced lease holder mints it (`mesh::share::ensure_share_key`); signs
  `Delegation{domain, edge, mesh id, engine = mesh::self_id(), issued = min(now, edge_now)}` inside
  `ShareKey::with_secret`.
- **Name keys** (§6.2): `share/<domain>/<name>/tls` JSON (`NameKey`) via `set_if_version(…, "share", None)`; a lost
  compare-and-set refreshes and returns the stored row (first writer wins).
- **Own ports** (§5.6): `rpc-compose::bootstrap::serve` registers the bound RPC port; `OwnListeners` refuses it as a
  `port` source ("that port is theuth itself"), re-checked on every upstream connect by `PortGuard`.
- **Human-only** (§7.2): `share_publish` sets `Tool::HUMAN_ONLY`, so CONSENT1's gate asks with
  `RequestHumanOnly` whatever the policy/session pin, and the double/app/non-owner thread cannot answer. The registry
  scopes the tool body with the `ApprovalResolved.id` it passed; the share records
  `approved_by = {approval_id: <uuid>, surface: "owner"}`.
- **No edge** (§5.5): the shipped default tickets are placeholders, so `default` has no ticket. The engine starts no
  edge client (logs `share: No sharing edge configured` at info), `share_settings_get` answers `configured: false`,
  and `share_publish` (tool and RPC) answers "No sharing edge configured". Same for `none`.
- **Events**: `share.changed`, `share.status`, `share.traffic` declared by the plugin, on the wire schema, in the
  golden and parity tests, republished by the remote-client mirror.

## Deviations, and why

- **Adapters live in `crates/share`** (deps `mesh`, `secrets`), not in `rpc-compose`/`theuth`: no cycle (neither depends
  on `share`), and it keeps the composition root to one line (`SharePlugin::engine()`).
- **Own-listener registry is in `crates/share`**, not `crates/runtime` (§5.6 says "a `runtime` set"): the smallest
  registry that does the job; only the RPC listener writes to it.
- **`approved_by.surface` is `owner`**, not the answering surface (desktop, tui, slack, …): `Resolve` does not carry
  the surface and changing it touches every channel crate. For a human-only row only the owner can answer, so
  `owner` is true but coarse.
- **`engine_id` is read at plugin start** from `mesh::self_id()` or the stored identity, so `SharePlugin` must be listed
  after `mesh::MeshPlugin` (it is).
- **Settings shows the empty state from `configured`**, a new field on `ShareSettings`, instead of the UI parsing the
  default tickets.

## Not built

- `TODO(210-M1)`: the RPC path (`share_publish`, `share_settings_set {enabled: true | edge}`, `share_terms_accept`)
  still checks only that the caller is the owner; it does not raise an owner-only approval row, so a Settings publish
  records an empty `approved_by` (`engine.rs` `publish`, `rpc_share.rs`).
- `TODO(210-M1)`: `ProcessCheck` over a socket-table crate. Cargo.lock has only `procfs` (Linux only), no `listeners` or
  `netstat2`, so no new dependency; `NoProcessCheck` answers unknown. Listeners of child processes (browser debug
  port, MCP servers, sandbox VNC) and of other plugins (a2a HTTP) are therefore not refused yet.
- `TODO(210-M1)`: other plugins binding TCP listeners do not call `register_own_port`.
- `TODO(210-M2)`: the answering surface in `Approved`; `share.delegate` from a non-holder; `share.name_key.ensure`.
- `TODO(210-M4)`: real default tickets; offering every ALPN major.
- ACP mode (`t --acp`) does not load the share plugin (no mesh there).

## Doubts

- `MeshDelegation` needs `mesh::self_id()`; before the mesh binds its endpoint a dial fails and the state machine
  retries with backoff.
- `VaultNameKeys` resolves the vault through `mesh::net()?.vault()`; if the mesh plugin is off, sharing cannot mint
  name keys even though the vault is open.
- The approval scope is a task-local: a tool that spawns its body loses it unless it re-enters `approved_scope`.
  `share_publish` reads it before any spawn.
- `crates/theuth/tests/fixtures/wire/openrpc-v1.json` was last accepted long before this branch: the accept also
  carries every unaccepted change since then (CONSENT1's `human_only`, other builders' methods), +2448/−253 lines.
- Generated TS: `Source` names two types in different scoped modules (knowledge's `Source` enum and share's
  `Source`); the generator scopes them per module, but a UI file importing both needs an alias.

## Verification

Every cargo command ran under `/tmp/theuth-build.lock` with `CARGO_BUILD_JOBS=6`.

| command | result |
|---|---|
| `cargo check -p share --tests` | ok |
| `cargo clippy -p share -p rpc-compose -p tools --all-targets -- -D warnings` | fails in **foreign** crates on clippy 1.99 lints (`voice/src/history/mod.rs:465` `chunks_exact_to_as_chunks`, `talk/src/rpc_hub.rs:50` `single_match`); not in owned code |
| `cargo clippy -p share -p rpc-compose -p tools --all-targets --no-deps -- -D warnings` | ok, clean |
| `cargo test -p share` | 18 passed (incl. new `wiring::tests::name_key_row_is_written_once_and_read_back`) |
| `cargo test --no-fail-fast -p rpc-compose --test wire_golden --test wire_parity_inc2 … inc7 --test wire_topics` | first run: `wire_golden` failed on `tools.approval-requested` (CONSENT1's `human_only: false` now serialized; pre-existing), fixtures `tools.approval-requested.json` and `rpc.stream.pending.json` updated; second run: all ok (golden 2, inc2–inc7 1 each, topics 4) |
| `just ts-desktop` (runs `cargo test -p theuth --bin t export_bindings`, so `theuth` compiles) | ok: 634 methods, 71 topics, 246 scoped modules |
| `just wire-schema-accept` | done as its copy step (`cp bindings/openrpc.json crates/theuth/tests/fixtures/wire/openrpc-v1.json`) right after `ts-desktop` ran `wire-schema`, to save a second lock hold |

Not run: a separate `cargo check -p theuth` (the `export_bindings` build compiles the `t` binary); `cargo test -p
tools` (the registry change is one task-local scope; covered only by clippy/compile); an end-to-end publish (no edge
deployed).

Generated desktop types (git-ignored, `crates/desktop/ui/src/rpc/bindings/`): event modules
`event.share_2echanged`, `event.share_2estatus`, `event.share_2etraffic`; method module `method.share`. Types:
`Share`, `ShareChanged`, `ShareStatus`, `ShareTraffic`, `EdgeState`, `ShareState`, `StoppedBy`, `CreatedBy`,
`ApprovedBy`, `Access`, `Source`, `Expiry`, `SourceArg`, `PublishArgs`, `Published`, `Stopped`, `NameCheck`,
`ShareSettings` (with `configured`), and per method `ShareListParams/Result`, `ShareStopParams/Result`,
`SharePublishParams/Result`, `ShareSettingsGetParams/Result`, `ShareSettingsSetParams/Result`,
`ShareNamesCheckParams/Result`, `ShareTermsAcceptParams/Result`.

Cargo.lock: no new registry packages; only workspace edges (`share` → `mesh`, `secrets`; `rpc-compose`, `theuth` →
`share`). The lock is not committed by this phase.
