# 210 as built — MESH1 phase (mesh and vault glue for sharing)

Design: `docs/design/210-public-share.md` rev 8 (D8, D18, D21, D22, §5.3 "Glue", §5.7, §8.3). Contract:
`docs/design/210-as-built-CONTRACT.md`. Owned paths: `crates/mesh/**`, `crates/secrets/**`. No dependency added;
`mesh` does not depend on `share-proto`.

## Files

| path | what |
|---|---|
| `crates/mesh/src/net.rs` | `Net::dial_alpn`, `Net::subscribe_removals`, private `insert_relays` (shared with `join_network`), two `Net` fields, one test |
| `crates/mesh/src/share.rs` (new) | `ShareKey`, `is_lease_holder`, `share_key`, `ensure_share_key`, `may_delegate`, `subscribe_removals`, one test |
| `crates/mesh/src/lib.rs` | `pub mod share`; `wait_synced()` beside `synced()` |
| `crates/secrets/src/vault.rs` | `SHARE_PREFIX`, `writable_name` accepts `share/…`, `Vault::MESH_SHARE_KEY`, `mesh_share_key_if_set`, `mesh_share_key(holder)`, `replace_mesh_share_key`, private `decode_key32`; two tests |
| `crates/secrets/src/lib.rs` | re-exports `SHARE_PREFIX` |

## Public API

### `mesh::net::Net`

```rust
pub async fn dial_alpn(&self, addr: EndpointAddr, alpn: &[u8], transport: QuicTransportConfig)
    -> anyhow::Result<Connection>;
pub fn subscribe_removals(&self) -> broadcast::Receiver<EngineId>;
```

- `dial_alpn` (D18, §5.3): `Endpoint::connect_with_opts(addr, alpn, ConnectOptions::new().with_transport_config(transport))`
  under `CONNECT_TIMEOUT` (30 s), modelled on `a2a_stream`. Never calls `join_network`: no pkarr, n0 DNS or mDNS
  service is installed. It inserts `mesh.relay`'s relays into the relay map once (the same `insert_relays` that
  `join_network` now calls; idempotent through a new `relays_in` flag). The addr's own relay URL is dialed as given;
  iroh 1.4.1 opens an active (non-home) relay connection for a URL outside its map
  (`socket/transports/relay/actor.rs` `start_active_relay`). The caller owns the connection: no tracking, reuse,
  inbound handler or paired-peer check. The endpoint's own transport config is untouched.
- `subscribe_removals`: ids that leave the paired set in `refresh_paired` **and** are tombstoned in the store
  (quarantined drops are not removals, §5.7). Emitted only while someone subscribes; one store read per drop.

### `mesh::share`

```rust
pub struct ShareKey;                       // Debug: mesh id + version only
impl ShareKey {
    pub fn mesh_id(&self) -> iroh::PublicKey;
    pub fn version(&self) -> u32;          // row version: the rekey's compare-and-set precondition
    pub fn sign(&self, msg: &[u8]) -> iroh::Signature;
    pub fn with_secret<R>(&self, f: impl FnOnce(&iroh::SecretKey) -> R) -> R;   // for share_proto::Delegation::sign / Rekey::sign
}
pub async fn is_lease_holder() -> bool;    // is_hub() && synced() && Status.lease.holder == self
pub fn share_key(vault: &secrets::Vault) -> anyhow::Result<Option<ShareKey>>;
pub async fn ensure_share_key(vault: &secrets::Vault) -> anyhow::Result<ShareKey>;   // mint only if is_lease_holder
pub fn may_delegate(engine: &str) -> bool; // self, or paired (neither tombstoned nor quarantined)
pub fn subscribe_removals() -> Option<broadcast::Receiver<EngineId>>;
```

Live peers for issuance: the existing `Net::members()` / `mesh::is_paired`; reconciliation of removals missed while
not running: the existing `mesh::RemovedPeers` message.

### `mesh` (lib)

```rust
pub async fn wait_synced();                // returns once synced(); set_synced(true) wakes waiters
```

### `secrets`

```rust
pub const SHARE_PREFIX: &str = "share/";   // share/<domain>/<name>/<key>, every segment a valid_name
impl Vault {
    pub const MESH_SHARE_KEY: &'static str = "mesh/share_key";
    pub fn mesh_share_key_if_set(&self) -> anyhow::Result<Option<(u32, Zeroizing<[u8; 32]>)>>;
    pub async fn mesh_share_key(&self, holder: bool) -> anyhow::Result<(u32, Zeroizing<[u8; 32]>)>;
    pub async fn replace_mesh_share_key(&self, fresh: &[u8; 32], expected: u32) -> anyhow::Result<Option<u32>>;
}
```

- `mesh_share_key(holder)`: existing row → returned; no row and `!holder` → error, nothing written; holder →
  32 random bytes, base64url, `set_if_version(…, "share", None)`, then the row as read back (on a lost
  compare-and-set it refreshes first, so the replicated winner is returned). Same pattern as `mesh_device_key`.
- `share/` rows: `set` refuses them (`valid_name` has no `/`); `set_if_version` writes and versions them; `list`
  hides them (any `/`); `expand` cannot name them (placeholder grammar has no `/`); the unpair preview leaves them
  out (it names only `oauth/` and slash-free rows). `share/<domain>/<name>/tls` fits since `.` is a `valid_name`
  character.

## Tests

- `mesh::net::tests::dial_alpn_uses_its_config_and_publishes_nothing`: a `Net` bound with discovery **on** dials a
  plain endpoint on `theuth/test-share/1`; the remote can open two bidi streams and the third waits (the
  per-connection `max_concurrent_bidi_streams(2)` applies); afterwards `joined` is false and the endpoint's
  address-lookup services are empty.
- `mesh::share::tests::only_the_holder_mints_and_the_handle_signs`: without a mesh actor nobody is the holder, so
  `ensure_share_key` fails and writes nothing; after a holder mint it loads the row; signatures verify under
  `mesh_id`.
- `secrets::vault::tests::share_rows_are_versioned_hidden_and_unexpandable`, `share_key_is_minted_only_by_the_holder`.

## Deviations from the design, and why

- **Relay URLs**: §5.3 says `dial_alpn` "inserts only the relay URLs it needs (the ticket's and `mesh.relay`)". It
  inserts `mesh.relay`'s only. Inserting the ticket's relay would make it a home-relay candidate; iroh dials a
  relay URL outside its map anyway (cited above). With `mesh.relay: none` the endpoint was bound with
  `RelayMode::Disabled`, so only direct addresses work, as for every other mesh dial.
- **Signing handle**: `ShareKey::with_secret` lends `&SecretKey` for `share_proto::{Delegation, Rekey}::sign`,
  which take a `&SecretKey`. A handle that never lends the key would need `share-proto` in `mesh` (forbidden) or a
  re-implementation of the record encoding in the plugin. The borrow is a convention, not a barrier.
- **`is_lease_holder`** asks the mesh actor's `Status` (the link predicate). It duplicates
  `link::rpc_support::mint_as_holder`'s inline check; link is not this phase's path, so it was not switched over.

## Not built (M2, marked `TODO(210-M2)` / `TODO(210-M4)`)

- The settle rule (wait `synced()`, then `rekey_settle_secs`, then re-read) inside the mint: `wait_synced` and
  `replace_mesh_share_key` are the primitives; the rekey state machine, stash and chained rekey are the share
  plugin's (M2).
- `share.name_key.ensure` and `share.delegate` RPCs (share plugin, M2).
- A removed engine dropping its stale `mesh/share_key` and `share/*` rows: `Vault::delete` already accepts any name.
- `dial_alpn` offering the previous major (`with_additional_alpns`), `TODO(210-M4)`.

## Open doubts

- Removal events come from `refresh_paired`; a tombstone that arrives by the record replica runs through it only at
  the next peer-set merge or local refresh. The rekey trigger must also reconcile with `RemovedPeers` (on start, on
  lease gain); this phase does not add a timer.
- `is_lease_holder` is `false` without a running mesh actor. An engine without the mesh plugin also has no `Net`,
  so it cannot dial an edge anyway; "an engine without a family is its own holder" holds when the mesh plugin runs
  alone (lone engine elects itself).
- The `dial_alpn` test cannot observe pkarr traffic directly; it asserts the cause (no `join_network`, no lookup
  service installed).
