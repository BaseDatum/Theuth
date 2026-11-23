# 210 as built — EDGE-M2 phase (`share-edge`: names, transfer, rotation, revoke, rekey, multi-engine routes)

Design: `docs/design/210-public-share.md` rev 8 §5.2, §5.3, §5.4, §5.7, §6.2–§6.3, §10 M2; contract
`210-as-built-CONTRACT2.md` (share-proto at f2410c11f, unchanged here). Builds on EDGE1, EDGE2, FIX-EDGE, EDGE-K8S.

## Files

| path | change |
|---|---|
| `crates/share-edge/migrations/0003_mesh_identity.sql` | new: `mesh_rekeys (old PK, new, ms)` history; `names.rotate_compromised` |
| `crates/share-edge/src/store.rs` | `NameRow.{transfer_to, transfer_until_ms}`; `Claimed::Transferred`; transfer branch in `claim`; `offer_transfer`, `current_mesh`, `rekey_history`, `rekey` → `Rekeyed`, `set_rotate_compromised`, `rotate_compromised` |
| `crates/share-edge/src/store/certs.rs` | `set_first_csr` (conditional first CSR) |
| `crates/share-edge/src/certs.rs` | `submit(.., q: CsrQuery)`; compromised flag; old chain revoked whenever the new leaf's key differs; first-CSR race → `csr_mismatch` |
| `crates/share-edge/src/control.rs` | transfer, rekey, `GET /v1/mesh` revoke list + history + chain end, `stopped_all`, `protocol_mismatch`, `NameInfo.transfer_*`, availability `reclaim_after_ms` and transfer offers, CSR query parse |
| `crates/share-edge/tests/mesh.rs` | new: 4 iroh integration tests (below) |
| `crates/share-edge/tests/certs.rs` | `CsrQuery` call sites; 2 new tests |
| `crates/share-edge/tests/visitor.rs` | version gate now expects `protocol_mismatch {supported}` for `v: 2` |

No new dependency; `Cargo.lock` unchanged by this phase.

## Public API (`share_edge`)

```rust
store::Claimed::Transferred
store::Rekeyed { Applied, Moved(String /* chain end */), Unknown, Banned, NewInUse }
store::NameRow { .., transfer_to: Option<String>, transfer_until_ms: Option<i64> }
Store::offer_transfer(domain, name, owner, to, until_ms) -> Result<bool>
Store::current_mesh(mesh) -> Result<String>                    // follows rekeyed_to to the end
Store::rekey_history(mesh) -> Result<Vec<(old, new, ms)>>      // oldest first
Store::rekey(old, new, revoke: &[String], now_ms) -> Result<Rekeyed>
Store::{set_rotate_compromised, rotate_compromised, set_first_csr}
certs::submit(edge, mesh, name, der, q: share_proto::CsrQuery)  // was `rotate: bool`
```

## Behaviour

- **Reserved names** (EDGE1/FIX-EDGE rules kept): `reserved_per_mesh` (1) at once, immediately claimable; a
  random name only while the mesh holds no name; first claim wins (`name_taken`); release →
  `reclaim_after_ms = max(now + 30 d, not_after)`, `cooldown {reclaim_after_ms}` for anyone but the previous
  owner. `GET /v1/names/{name}` now carries `reclaim_after_ms` with `reason: cooldown`, and answers
  `available` to the receiver of a pending transfer. "Primary" is client-side (CONTRACT2): nothing at the edge.
- **Transfer (D20)**: `POST /v1/names/{name}/transfer {to_mesh}` by the holder (`not_owner` otherwise; `to_mesh`
  = own mesh → `400 invalid`) sets `transfer_to` (resolved through its rekey chain) and
  `transfer_until_ms = now + TRANSFER_TTL_MS`; a newer offer replaces it, a release clears it. `NameInfo`
  in `welcome` shows it. The receiver claims with the plain `PUT`: in the claim's `BEGIN IMMEDIATE`
  transaction the old chain goes to `revocations` (`cessationOfOperation`), open jobs are dropped, CSR/chain
  cleared, owner moved; a `slotted`/`valid` name stays `slotted` (same identifier set, no new-name token); the
  receiver's quota applies with the name's kind; no cooldown. The giver's routes under the name are dropped
  (announced); k8s per-name objects stay (the name keeps serving).
- **Rotation (§6.3)**: `?rotate=true&compromised=true` (compromised only with rotate) marks the name; once the new
  chain is stored the old one is queued for revocation `keyCompromise` if marked, else `superseded`, and the mark
  clears. The revocation now triggers on "old leaf key ≠ new CSR key" for any job kind (a rotation that lands on a
  queued renewal is covered); a same-key renewal revokes nothing.
- **`csr_mismatch {spki}` and heal**: unchanged rule (a different CSR without `rotate`, or with it while
  `ordering`), plus the first-CSR race: two different first CSRs posted at once — the store is conditional
  (`csr IS NULL`), the loser gets `csr_mismatch` naming the winner's SPKI; the heal is the client's rotate.
- **`POST /v1/mesh/rekey`**: `Rekey::verify` (`403 bad_signature`, `400 invalid`); answered **without a
  `hello`** too (the two signatures are the authority; the heal sender's mesh id is already rekeyed away, so it
  cannot `hello`). `Store::rekey` in one `BEGIN IMMEDIATE`: unknown `old` → `404 not_found`; `old` already
  rekeyed → `409 mesh_rekeyed {to = end of chain}` (first valid wins); a banned `old`/`new` → `403 banned`;
  a `new` that holds names or was rekeyed away → `400 invalid` (no merges, no cycles). Applied: the `meshes`
  row is copied to `new` (invited, net, asn, `stop_all_ms`), engine history copied, the revoke list *replaced*
  (engines not in `revoke` un-revoked under `new`), names (held and in cooldown), pending offers to `old`, and open
  jobs move; `old.rekeyed_to/rekeyed_ms`; a `mesh_rekeys` row. Then `old`'s routes are dropped, every `old`
  tunnel gets `kill {mesh_rekeyed}`, and any tunnel already under `new` whose engine is in `revoke` gets
  `kill {revoked}`. `hello` under any id of the chain answers `409 {to}` with the chain's end.
- **`POST /v1/mesh/revoke`** (EDGE1) closes the removed engine's tunnel at once; tested here as the first step of
  a removal whose vault rotation fails.
- **`GET /v1/mesh`**: `mesh` = chain end, `revoked` = the revoke list, `rekeys` = history oldest first.
- **Several engines per name** (EDGE1 rule, now tested end to end): each host routes to the engine that put it;
  `route_conflict.conflicts` names the holder engine per host, nothing applied; a freed host is announced and can
  be taken by another engine.
- **`RoutesPut.published_ms`**: a host with `published_ms <= stop_all_ms` → `409 stopped_all {stop_all_ms,
  hosts}`, nothing applied. The session rule (`403 hello_required` for a session older than the Stop all) stays.
- **`protocol_mismatch`**: `Hello.v` not in `SUPPORTED_MAJORS` → `426 {code: protocol_mismatch, supported}`;
  `min_client` stays `426 client_too_old`.

## Deviations

- Rekey without a `hello` (see above): the design's table implies a delegated connection, but the heal sender
  holds only a superseded key. The signatures by `old` and `new` are stronger than a delegation.
- Rekey merges are refused (`new` with names → `400 invalid`): share-proto has no dedicated code.
- Unknown `old` → `404 not_found` (no dedicated code).
- Invites and quotas "move" implicitly: invites are not bound to meshes at the edge; `invited` is copied.
- Transfer keeps the name `slotted` instead of `allocated` so it never waits for a new-name token.

## Not built

- `TODO(210-M2)` none left in `share-edge`. The client half (send `published_ms`, `compromised`, chain heal,
  the rekey state machine) is `crates/share`'s.
- `TODO(210-M4)`: `admin recover` for a contested rekey, `admin mesh` showing `rekeys`.
- `TODO(210-P2)`: offline signed `Handover`.
- Inactivity expiry and its warnings (M3, as before).

## Doubts

- A rotation accepted while the first job is `queued` replaces the CSR in place; if the runner leased the job
  between `submit`'s read and the write, `store_chain` refuses the old-CSR leaf (revoked, job dropped) and the new
  CSR has no job until the client re-posts it (it does on the next attach, §6.2 step 1).
- Two identical first CSRs posted at the same instant may enqueue two `new` jobs; the second finds the chain and
  stores nothing new, but costs an order.
- `rekey` copies `stop_all_ms` from `old` only when `new` has none.

## Verification

Commands, each under its own `/tmp/theuth-build.lock` hold:

```sh
CARGO_BUILD_JOBS=6 cargo test -p share-edge --all-features
CARGO_BUILD_JOBS=6 cargo clippy -p share-edge --all-targets --all-features -- -D warnings
cargo tree -p share-edge -e normal | grep -E '\(/'
```

- `cargo test -p share-edge --all-features` (run at 15:14, before the last edit): unit 25/25, `certs` 14/14
  (new: `a_compromised_rotation_revokes_key_compromise_once`,
  `an_online_transfer_moves_the_name_and_revokes_the_old_chain`), `mesh` 3/4 (`three_engine_family`,
  `two_machines_publish_a_first_name_in_one_sync_gap`, `rekey_races_first_valid_wins_and_the_chain_heals`
  passed). `reserved_names_cooldown_and_online_transfer` failed at its non-holder transfer check: the test had
  `b` offer the name to its *own* mesh, so `400 invalid` correctly came before `403 not_owner`. Fixed in the
  test only (offer to `a`'s mesh). The `visitor` result was not in the captured output (the grep kept failures
  and summaries; `mesh` failing stopped cargo before `visitor` ran). **Re-run after the fix: unverified (lock)**
  — queued from ~15:16 to 15:41 behind other worktrees' runs, cancelled per the house rule.
- `cargo clippy -p share-edge --all-targets --all-features -- -D warnings`: **unverified (lock)**. The code type-checks
  (the test build above compiled lib, bin and every test target with `--all-features`). One possible lint:
  `leaf_info(&old).map_or(true, …)` in `certs::process`.
- `cargo tree -p share-edge -e normal`: **unverified (lock)**. No dependency was added (`Cargo.toml`
  unchanged), so the path crates stay `share-edge`, `share-proto`, `platform` (+ `vendor/libsqlite3-sys` via
  sqlx). [INFERENCE from the unchanged manifest]
- `Cargo.lock` shows a one-line change in the worktree that this phase did not make (no manifest edit); left
  uncommitted.

Tests and what they cover:

- `tests/mesh.rs` (iroh, in-process edge, fakes): `reserved_names_cooldown_and_online_transfer` (quota, random
  while reserved held, `name_taken`, availability, transfer offer/not_owner/self, `welcome.names[].transfer_*`,
  receiver's plain PUT, giver's routes dropped, cooldown with `reclaim_after_ms`, previous owner re-claims);
  `three_engine_family` (sub-labels per engine, `route_conflict` naming the holder, freed host re-taken, Stop all
  closes all three, `stopped_all {hosts}` vs a newer `published_ms`, revoke closes the removed engine at once
  and keeps it out with the vault rotation still pending, then a rekey from the share-less hub: tunnels killed,
  `409 {to}` under the old id, names/revoke list/Stop all under the new one, history in `GET /v1/mesh`);
  `two_machines_publish_a_first_name_in_one_sync_gap` (concurrent random allocations → `201` + `quota`,
  concurrent reserved claims, concurrent different first CSRs → one `csr_mismatch {spki of the winner}`, heal by
  rotation to the settled key ends with that key's certificate); `rekey_races_first_valid_wins_and_the_chain_heals`
  (a thief's revoke under the old key undone by the replacing revoke list, two holders after a heal: first wins,
  second `409 {to}`, `rekey_conflict` Contested/Chain/Confirming, chained rekey sent without `hello`, history
  `k→a→b`, end-of-chain `to`, forged signature 403, unknown mesh 404, `new` in use 400).
- `tests/visitor.rs` `control_gates` (edited, not yet run): `v: 2` → `426 protocol_mismatch {supported: [1]}`,
  old client → `426 client_too_old`.

Not covered by an automated test here: a removal whose vault rotation *fails forever* is the client's
`waiting_vault` state; the edge side (revoke closes tunnels, names stay, no rekey) is in `three_engine_family`.
The k8s per-name objects follow claim/release/transfer through the existing `k8s::granted`/`gone` hooks (rekey
changes no name); no new k8s test.
