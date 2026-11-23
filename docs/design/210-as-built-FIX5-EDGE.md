# 210 as built — FIX5-EDGE

Branch `feat/210-public-share`. Code commit `7c3555533` (`crates/share-edge/**` only). No `share-proto` change,
no migration, no `Cargo.lock` change. Regressions from the re-review of FIX4-EDGE.

## What was built, per finding

| # | fix | where | test |
|---|---|---|---|
| R1 | `Store::revoke` stores an id the mesh has no `mesh_engines` row for as a revoked row (`first_ms = 0, last_ms = 0, net = ''`, `revoked_ms = now`). `hello` already refuses any row with `revoked_ms` (`403 revoked`) before the engine quota, so a machine removed before its first `hello` cannot attach with the delegation it holds. `first_ms = 0` is before every UTC day start, so `engines_first_seen_since` (per-day engine quota) never counts it; it counts toward the revoke cap. `RevokeResult.revoked` lists it. Repeats in one request count once. `Store::rekey`'s revoke-list insert of unknown ids now uses `first_ms = last_ms = 0` too (it used `now`, which counted toward the day quota). | `store.rs`, `control.rs` | `store/ops.rs::unknown_engines_are_revoked_and_old_revocations_age_out`; `tests/mesh.rs::an_engine_revoked_before_its_first_hello_cannot_attach` |
| R3 | The `put_routes` eviction moved to `pub async fn control::evict_stale_routes(edge, domain, mesh, wanted) -> Vec<String>`. For each wanted host routed by another mesh it reads the name row again and evicts only when the row is `held()` with `owner == mesh`. A host failing that keeps the other mesh's route and `put_routes` answers `route_conflict`. | `control.rs` | `tests/mesh.rs::a_stale_route_is_evicted_only_while_the_name_is_held` (held → evicted; released → kept; held by the other mesh → kept) |
| R5 | `Certs::record_locks`: one `tokio::sync::Mutex` per name in a map (entries no one holds or waits on are dropped on the next `lock_name`). `ensure_records` holds it across the whole host loop (ownership check, `dns_add`, `dns::ensure`), `retire_inner` from the `cert_row` check through `drop_jobs` + `remove_records` (taken after `revoke_due`, so the two locks never nest), `reconcile_dns` per ledger row that has a name, from the `held` check through `remove_host`. | `certs.rs` | none (see Doubts) |
| R6 | `state::REVOKE_WINDOW_MS = DELEGATION_MAX_TTL_MS + DELEGATION_SKEW_MS` (25 h 5 min). `Store::revoke(mesh, engines, cap, since_ms, now_ms)` counts only revoked rows with `revoked_ms >= since_ms`; `mesh_revoke` passes `now − REVOKE_WINDOW_MS`. New `Store::prune_revoked(before_ms)` deletes revoked rows older than that; `queue::housekeeping` (every minute) calls it with `now − REVOKE_WINDOW_MS`. | `state.rs`, `store.rs`, `control.rs`, `queue.rs` | `store/ops.rs::unknown_engines_are_revoked_and_old_revocations_age_out` |

## Public API changes

```rust
Store::revoke(mesh, engines, cap: u64, since_ms: u64, now_ms: u64) -> Result<Option<Vec<String>>>  // + since_ms
Store::prune_revoked(before_ms: u64) -> Result<u64>
control::evict_stale_routes(&Edge, domain, &PublicKey, &[(host, name)]) -> Result<Vec<String>>
state::REVOKE_WINDOW_MS
```

## Deviations from the brief

- **R6 window is 25 h 5 min, not 24 h.** The edge accepts delegations up to `DELEGATION_MAX_TTL_MS` (25 h) issued
  up to `DELEGATION_SKEW_MS` (5 min) ahead of its clock; a 24 h window would prune a row while a delegation
  issued before the revocation could still be valid.
- **R1 rows show in `GET /v1/mesh` `engines`** with `last_ms = 0` and in `admin mesh`, next to the revoke list.
  Kept: it tells the user which ids are revoked; a client that hides never-attached engines can filter on
  `last_ms == 0`.
- **R3** narrows the window to the time between the re-read and `drop_routes` (both in the same task, no other
  await in between except the read itself); it does not make the check and the eviction atomic with the store.
- **R6 prune deletes the whole row**, so a pruned engine's attach history is gone; if the mesh delegates it again
  it counts as a new engine for the day quota.

## Not built

- TODO(210-M4): `REVOKE_WINDOW_MS` and `REVOKED_PER_MESH` as config keys (still constants).
- TODO(210-M3): a deterministic test for the R5 record lock (needs a hook to pause `ensure_records` between
  `dns_add` and `dns::ensure`; the fake zone has none).

## Doubts

- R5 has no test. The lock is acquired in three places, never nested: `revoke_due`'s `revoking` lock is
  released before `retire_inner` takes the name lock, and `remove_records`/`remove_host` don't lock. Reviewed by
  reading the code, not run under contention.
- `reconcile_dns` locks TXT ledger rows by their `name` column too (whatever was stored as the ident). That is
  harmless: the runner's own challenge writes don't take the lock, and the runner runs `reconcile_dns` between
  ticks.

## Verification (after the last code edit, each under its own lock hold)

```sh
CARGO_BUILD_JOBS=6 cargo test -p share-edge --all-features
CARGO_BUILD_JOBS=6 cargo clippy -p share-edge --all-targets --all-features -- -D warnings
CARGO_BUILD_JOBS=6 cargo clippy -p share-edge --all-targets -- -D warnings
cargo tree -p share-edge -e normal | grep -E '\(/'
```

- `cargo test -p share-edge --all-features`: all pass. Suite counts: lib 46, then 0, 8, 15, 7 (mesh, +2), 2, 0.
- `clippy --all-features -D warnings`: exit 0. `clippy` with default features `-D warnings`: exit 0.
- `cargo tree -p share-edge -e normal`: the only path crates are `share-edge`, `platform`, `share-proto` and
  `vendor/libsqlite3-sys`.
- `Cargo.lock`: unchanged. `cargo fmt` not run: the crate is not rustfmt-clean at the default width (existing
  code); the new code follows the surrounding style.

## FIX6-EDGE

Regression N1 from R6. Files: `crates/share-edge/src/store.rs`, `src/queue.rs` (doc), `src/store/ops.rs` (test),
`tests/mesh.rs` (test). No migration, no API signature change, no `Cargo.lock` change.

### What was built

- **N1.** `Store::prune_revoked(before_ms)` now deletes a revoked row only when it is older than `before_ms`
  (still `now − REVOKE_WINDOW_MS` from `housekeeping`) **and** a rekey made it moot:
  - its mesh was rekeyed away at or after the revocation (`meshes.rekeyed_ms >= revoked_ms`); `hello` on a
    rekeyed mesh answers `409 mesh_rekeyed` before the revoke check, so the row refuses nothing; or
  - its mesh was created by a rekey at or after the revocation (`mesh_rekeys.new = mesh AND ms >= revoked_ms`):
    the revoked engine never held that key. `>=` because `Store::rekey` stamps a revoke-list entry new to the
    list with the rekey time itself.
  A revoked row of a mesh not rekeyed since its revocation is kept for good: the removed machine still holds
  `mesh/share_key` and can sign itself a fresh delegation.
- The revoke cap's window (`since_ms`) is unchanged: the cap stays a rate (`REVOKED_PER_MESH` per 25 h 5 min),
  older revocations no longer count toward it but still refuse `hello`.

### Tests

- `store/ops.rs::unknown_engines_are_revoked_and_old_revocations_age_out`: no rekey → `prune_revoked` deletes 0
  and the row stays revoked; after `rekey(m1 → m2, [lost, other])` it deletes 3 (m1: `lost`, `e1`; m2: `lost`,
  which kept its t0 revocation); the recent `other` stays in m2.
- `tests/mesh.rs::a_revocation_is_pruned_only_after_a_rekey`: revoke over the wire, `queue::housekeeping` at
  now + 30 h keeps the row and the engine's `hello` is `403 revoked`; after `POST /v1/mesh/rekey m → m2` the same
  housekeeping removes the row from both meshes and `hello` under `m` is `409 mesh_rekeyed`.

### Deviations / doubts

- Revoked rows of a mesh that never rekeys grow without bound, at most `REVOKED_PER_MESH` per window. Accepted:
  dropping them is the N1 hole. TODO(210-M4): a per-mesh hard ceiling on revoked rows (refuse further revokes
  with "rekey first") if growth shows up in ops.
- A row in a rekey-created mesh revoked *after* the rekey is pruned only after the next rekey of that mesh
  (the `meshes.rekeyed_ms` branch); correct, same reasoning as above.

### Verification (after the last code edit, each under its own lock hold)

```sh
CARGO_BUILD_JOBS=6 cargo test -p share-edge --all-features
CARGO_BUILD_JOBS=6 cargo clippy -p share-edge --all-targets --all-features -- -D warnings
CARGO_BUILD_JOBS=6 cargo clippy -p share-edge --all-targets -- -D warnings
```

- `cargo test -p share-edge --all-features`: all pass. Suite counts: lib 46, then 0, 8, 15, 8 (mesh, +1), 2, 0.
- Both clippy runs `-D warnings`: exit 0. `Cargo.lock`: unchanged.
