# 210 as built: CLIENT-M2 (mesh identity, delegation, rekey, names; `crates/share`)

Design: `docs/design/210-public-share.md` rev 8 (§5.2, §5.3, §5.7, §5.9, §6.2, §6.3, D20–D22, §10 M2) and the
appendix. Seams: MESH1, CLIENT1, FIX-A, WIRE1, FOLLOW-A, CONTRACT2 as-built notes. `share-proto` was not changed.

## Files

| path | what |
|---|---|
| `crates/share/src/rekey.rs` (new) | §5.7 rekey state machine: `RekeyState`, `RekeyRow`, `EdgeRow`, `Stash` (0600 `<data_dir>/share/rekey.stash`), `MeshKeys`/`RekeyEdges` seams, `Rekeyer {on_removal, run, step, heal, prune}`, `rekey_answer`, `unhandled_removals`, `Attention`; `Store` access to `share_edges`/`share_rekeys` |
| `crates/share/src/rekey/tests.rs` (new) | P0-6 races against a partitionable LWW vault replica and a §5.7 fake edge |
| `crates/share/src/holder.rs` (new) | holder duties: `issue` (clock rule, live check), `renew_at`, `Settle`/`MeshSettle`, `ensure_name_key` (write → settle → re-read), `row_spki`, `spki_hex`, `csr_heal`/`CsrHeal`, `CSR_HEAL_MS` |
| `crates/share/src/family.rs` (new) | `holder() -> Holder::{Me, Peer(id)}`, `call(peer, method, params)`: one JSON-RPC line over `theuth/rpc/1` via the replica transport |
| `crates/share/src/duties.rs` (new) | engine driver: `VaultMeshKeys`, `DialedEdges` (dial ticket, info, hello under a given key), `run()`: removals subscription, `RemovedPeers` at start/lease gain, 60 s tick, heal on non-holders, prune |
| `crates/share/src/names.rs` (new) | `HeldName` (RPC), `ShareName` (row), `Store::{names, put_name, set_primary, release_name}`, `reserve`, `release`, `primary`, `rotate`, `transfer`, `list`, `held`, `parse_mesh` |
| `crates/share/src/edge.rs` | `Control::{put_routes_at, transfer, mesh, revoke, rekey}`; `Attachment::routes()` (default = hosts only); state machine sends `routes()`; `mesh_rekeyed` comments resolved (redial re-delegates with the vault's key) |
| `crates/share/src/engine.rs` | `published_ms` per host (`routes_put`); `share_edges` row at every `welcome` (D22); `stop` forwards another engine's share; `stop_all` fans out (`stop_all_forwarded` for the forwarded call); `attached_control`, `welcome_names`, `engine_id()`, `rotate_cert` |
| `crates/share/src/wiring.rs` | `MeshDelegation` asks the holder (`share_delegate`) when not the holder; `delegate_as_holder`; `FamilyNameKeys` (holder creates, non-holder calls `share_name_key_ensure` and waits for replication); `name_key_as_holder`; `rotate_name_key_as_holder`; `mesh_id()` |
| `crates/share/src/rpc_share.rs` | the M2 RPCs below; `NameCheck.reclaim_after_ms`; `ShareSettings.mesh_id`; `share_stop` with extensions |
| `crates/share/src/lib.rs` | modules; spawns `duties::run` at start when a dialer exists (switch not consulted, D22) |
| `crates/share/Cargo.toml` | `zeroize` (workspace, already in the lock) |

## Public API

RPC (owner audience, snake names):

| method | params | answer | notes |
|---|---|---|---|
| `share_names_list` | — | `HeldName[]` | rows joined with the last welcome's `NameInfo`; UI-M2's shape |
| `share_names_reserve` | `name` | `HeldName` | `Consent::ask` (human-only row) first; `PUT /v1/names/{name}`; becomes primary |
| `share_names_release` | `name` | `NameReleasedInfo {name, reclaim_after_ms}` | stops the name's shares on every machine (forwarded), then `DELETE`; primary moves on |
| `share_names_primary` | `name` | `HeldName` | records a welcome-only name first |
| `share_names_rotate` | `name` | `u32` generation | non-holder forwards the whole call to the holder; holder replaces the row (CAS on version), settles, posts `?rotate=true` |
| `share_names_transfer` | `name, to_mesh` | `u64` `transfer_until_ms` | `Consent::ask` first; `to_mesh` = mesh id, bare or the first id-shaped token of the pasted text |
| `share_delegate` | `engine, edge, domain, edge_now_ms` | `Signed` as JSON | holder only; `may_delegate(engine)`; `issued_ms = min(own, edge_now_ms)` |
| `share_name_key_ensure` | `domain, name` | SPKI hex | holder only; create if missing, settle (`wait_synced` + 60 s), re-read |

`HeldName {name, domain, edge, kind, primary, state, queue_position, queue_eta_s, transfer_to, transfer_until_ms}`;
`ShareSettings.mesh_id: Option<String>`; `NameCheck.reclaim_after_ms: Option<u64>`.

Rust: see Files. `Rekeyer` is seam-only (no mesh/vault types), so it runs against fakes.

## Behaviour

- **Rekey (§5.7)**: `on_removal` creates a `revoking` row only when `mesh/share_key` exists and `share_edges`
  has a live row; a second trigger for the same engines reuses the open row. `revoking` revokes at each edge
  (per-edge progress in `edges_done`) → `waiting_vault` until `vault.current_key().members` excludes the removed
  → `writing` stashes `K` (`old`) and `K'` (`pending`), `replace_mesh_share_key(K', v)`; a lost CAS drops `K'`;
  a resumed `writing` whose stash `old` differs from the vault key does not write again → `settling` waits
  `synced()` + 60 s, records `to_pub = S` → `sending` signs `Rekey{old: K, new: S, revoke}` with both;
  `409 {to}` goes through `share_proto::rekey_conflict` (confirm / chain from stash / contested) → `confirming`
  probes `GET /v1/mesh` under every stash key and `S` (a moved-past key answers `mesh_rekeyed {to}`); a stash
  key → chained heal; an unknown key → backoff (contested surfaces after 1 h via `RekeyRow::attention`) → `done`.
  Edge failures back off 1 min doubling to 1 h (`next_at_ms`). If the vault key changes under `sending`/
  `confirming`, the row returns to `settling`.
- **Heal rule**: any engine with a non-empty stash runs `heal` each tick: if an edge's current id is a stash key
  and differs from the vault's key, it sends the chained `Rekey` itself. The chain's `revoke` is the union of all
  rows' removed engines (the edge replaces its list). Stash entries of a `done` row are kept for
  `rekey_heal_secs` (`prune`), because the loser of a partition may be the only holder of the edge's key.
- **Driver**: the lease holder runs rows on removals and every 60 s; on lease gain (and at start) it reconciles
  `mesh::RemovedPeers` with existing rows (MESH1 doubt).
- **Delegation**: holder signs itself and live peers; non-holders ask over `theuth/rpc/1`. Renewal stays the
  state machine's 12 h re-`hello` (`edge::RENEW`).
- **Name keys**: only the holder creates (`FamilyNameKeys`); a non-holder waits up to 120 s for the row to
  replicate (matched by SPKI).
- **Routes**: `RoutesPut.published_ms[host]` = earliest `live_ms` of a live share under the host.
- **Several engines per name**: each engine already publishes only its own sub-labels and answers 421 for others
  (M1); unchanged.
- **Stop**: another engine's share is stopped by forwarding `share_stop {id}` to it; Stop all fans out
  `share_stop "all"` to every other engine with an active share (10 s each, parallel); a forwarded call (internal
  peer) does not fan out again.

## Deviations

- `share_names_list` added (UI-M2's need); `ShareSettings.mesh_id` added (orchestrator request).
- Forwarding uses a local copy of the `theuth/rpc/1` line exchange (`family::call`) instead of
  `agent::rpc_forward::to_peer`, so `share` does not depend on `agent`. Consequence: the holder cannot see
  `Forwarded(peer)` and does not check that `engine` equals the caller; any family member may get a delegation for
  any live peer (harmless: the edge binds it to the QUIC key).
- The `contested` state is left by backoff polling rather than a fixed 1 h wait; the attention item appears after
  1 h as designed.
- The rekey row's `edges_done` holds `share_edges.id`s and is reset on every state change.
- A removed engine does **not** delete its `mesh/share_key` (see Not built).

## Not built

- `TODO(210-M2)`: `removed` deleting the stale `mesh/share_key`/`share/*` rows. A `revoked` answer alone does not
  prove the engine left the family, and deleting a replicated row while still paired deletes the family's key.
- `TODO(210-M2)`: post-`done` actions are not automatic: name-key rotation with `compromised` after a removal,
  and immediate delegation renewal (engines redial on `kill mesh_rekeyed` instead).
- `TODO(210-M2)`: the `csr_mismatch` heal is the decision function `holder::csr_heal` + `share_names_rotate`; no
  timer calls it from welcome SPKI drift yet.
- `TODO(210-M2)`: `share.changed` for rows replicated from other machines (needs a replica change subscription);
  RPC/events for rekey state, vault-stall and contested attention items; renewal jitter (`holder::renew_at` is not
  used by `edge::RENEW`); Stop all hand-off to the holder when detached; `share_extend`, `share_set_access`;
  random-name release when the last share stops; `share_edges` deletion on release/unknown mesh.
- The three-engine family test end to end (publish from two, stop one from the third over `theuth/rpc/1`) is not
  automated: it needs a running mesh transport. Covered parts: the rekey and name-key races below.

## Doubts

- `RemovedPeers` returns every tombstone ever; on the first lease gain after sharing starts, tombstones older
  than `mesh/share_key` start one unnecessary (harmless) rekey.
- `DialedEdges` opens a fresh connection per call (revoke, rekey, each probe); fine for rare removal events.
- Probing an edge with a stash key it never knew relies on the edge refusing the `hello` (it does: unknown
  delegation mesh with no names is a new mesh only when names are claimed — a real edge may admit it as an empty
  mesh and answer its own id; then the probe sees an unrelated id and the row backs off instead of confirming).
  [INFERENCE: not run against share-edge.]
- Tests use `synced() = true` always, matching F-m2 (a partitioned engine still reports synced).

## Verification

**Unverified: lock.** The machine-wide `/tmp/theuth-build.lock` was held by other sessions (theuth-show-file,
theuth-peek-memory) from about 15:15 to 15:40, more than the 25-minute limit, so the gates were not run after the
last edit.

What did run: one `cargo test -p share --lib -- rekey holder` under the lock at 15:13. It compiled the tree as it
stood then and failed with 5 errors: `family.rs` (a double `map_err` on `mesh::Status`) and four `rpc_share.rs`
`engine()?` deref coercions. Plus one unused-import warning in `rekey/tests.rs`. All of these are fixed in
`daac0c22f`, but the fixes were never compiled.

Not run: `cargo test -p share`, `cargo clippy -p share --all-targets -- -D warnings`,
`cargo test -p rpc-compose --test wire_golden`, `just ts-desktop`, `just wire-schema-accept`.
`crates/theuth/tests/fixtures/wire/openrpc-v1.json` has not been regenerated for the new methods
(`share_names_*`, `share_delegate`, `share_name_key_ensure`), `ShareSettings.mesh_id` or
`NameCheck.reclaim_after_ms`. mesh and secrets are not touched.

Commands to run, each under its own lock hold:

```
CARGO_BUILD_JOBS=6 cargo test -p share
CARGO_BUILD_JOBS=6 cargo clippy -p share --all-targets -- -D warnings
CARGO_BUILD_JOBS=6 cargo test -p rpc-compose --test wire_golden
just ts-desktop && cp bindings/openrpc.json crates/theuth/tests/fixtures/wire/openrpc-v1.json
```

Cargo.lock: `share` now depends on `zeroize`, a workspace crate that is already in the lock. No new package.
The lock shows as modified because of other builders' changes. Not committed.

## Green (CLIENT-M2-GREEN)

Commits: `b370ddd65` (share code), `359e8f402` (openrpc fixture). Design of `1daf8ba0f`/`daac0c22f`/`845c27c9a`
unchanged.

### Fixes

- `cargo check -p share --all-targets` was already clean on `845c27c9a` (the `daac0c22f` fixes hold).
- `engine.rs`: the doc line of `check_name` had slid above `attached_control` when `rotate_cert` was inserted;
  moved back (cosmetic).
- `rekey/tests.rs`: clippy `cloned_ref_to_slice_refs` (`&[edge.clone()]` → `std::slice::from_ref(&edge)`, 5
  sites). Test-only; no test logic changed. The six P0-6 race tests passed on their first run; no code fix
  needed.
- `rpc-compose` `wire_golden`/`wire_topics`: passed with no fixture change (share events unchanged).

### Gaps closed (both under 40 lines)

- **`csr_mismatch` heal timer** (was `TODO(210-M2)`): `Engine::csr_heal_tick(since, posted, now)` and
  `lib.rs::csr_heal_timer` (every 60 s, spawned at start). On the lease holder only (`family::holder() == Me`;
  otherwise the timers reset): for each name of the last `welcome`, reads the vault row through
  `hooks.name_keys.get` (never creates), `holder::row_spki`, then `holder::csr_heal(welcome spki, row spki,
  since, now, CSR_HEAL_MS)`. `Rotate` → `rotate_cert(name, row)` (posts the **row's** CSR with `?rotate=true`;
  no new key generation). `posted` remembers the SPKI already posted so a stale `welcome` (refreshed only by the
  12 h re-`hello`) does not re-post every 5 min; it is cleared once a `welcome` shows the row's SPKI.
- **`share.changed` for replicated rows** (was `TODO(210-M2)`): `lib.rs` keeps the `replica::attach` handle and
  spawns `announce_replicated`: for each `ChangeBatch` whose `origin` is not this replica, every distinct
  `shares` pk is re-read with `Store::get` and announced via `Engine::emit` (now `pub(crate)`). Tombstoned rows
  (no row left) are not announced. `Lagged` is skipped (the next batch re-announces its rows).

### Not built (still open)

- `TODO(210-M2)`: the `csr_mismatch {spki}` **error** answer of `POST csr` does not feed the heal timer; only the
  welcome's `names[].spki` drift does. Same decision function, so wiring it is a `since` insert in `ensure_cert`.
- `TODO(210-M2)`: everything else in "Not built" above (stale `mesh/share_key` deletion, post-`done` actions,
  rekey RPC/events and attention items, renewal jitter, Stop-all hand-off, `share_extend`, `share_set_access`,
  random-name release, `share_edges` deletion, three-engine e2e).

### Doubts

- Neither new task has a test: both are thin loops over tested pieces (`holder::csr_heal`, `rotate_cert`,
  `Store::get`, `emit`). The heal timer needs an attached edge and a lease, the announcer a running replica.
- A replicated `shares` row whose remote stop arrives as a column update is announced with its new state; a
  hard row delete (tombstone) is not announced (the UI would keep the row until its next list).
- `csr_heal_tick` dials the vault/mesh once per name per minute on the holder; fine for a handful of names.

### Wire

`just ts-desktop && just wire-schema-accept`: the `openrpc-v1.json` diff adds only `share_delegate`,
`share_name_key_ensure`, `share_names_{list,primary,release,reserve,rotate,transfer}`, `share_password`, schemas
`HeldName`, `NameReleasedInfo` (output + receiver), and the fields `NameCheck.reclaim_after_ms`,
`ShareSettings.mesh_id`, `Stopped.edge_pending`. No method removed or changed outside `share_*`.

### Verification (runs after the last edit, each under its own `/tmp/theuth-build.lock` hold, `CARGO_BUILD_JOBS=6`)

| command | result |
|---|---|
| `cargo check -p share --all-targets` | Finished, no warnings |
| `cargo clippy -p share --all-targets --no-deps -- -D warnings` | Finished, clean |
| `cargo test -p share` | 43 passed, 0 failed (incl. 6 `rekey::tests` P0-6 races) |
| `cargo test -p rpc-compose --test wire_golden --test wire_topics` | 2 + 4 passed; no fixture change |
| `just ts-desktop` | ok (643 methods, 71 topics) |
| `just wire-schema-accept` | fixture regenerated, committed `359e8f402` |
| `cargo test -p theuth --test wire_schema_snapshot` | 1 passed |

Cargo.lock: unchanged by this work (the `zeroize` line under `share` is CLIENT-M2's; no new package). Not
committed.
