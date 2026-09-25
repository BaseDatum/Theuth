# 69 — as built, worker Z (end-to-end smoke: devices pair to the mesh, failover, documents on the hub)

*Branch `feature/69-devices`, worktree `theuth-69`, 2026-09-25, on top of the integration head `d5222bd` (K, D, C2, M, Y, E1, E2, P merged). Brief: `/tmp/69/omp-Z.md`. Evidence and recipe: `docs/design/69-smoke.md`.*

## What was built, per commit

All twelve invariants ran on this Mac against a local `theuth-relay` (ws://127.0.0.1:8787, `--web` = main's UI bundle), engines A/B/C in `/tmp/69-z/<X>` and a **bun device** (the brief's fallback to headless Chromium): `/tmp/69-z/device.ts` drives the 31 link protocol with the UI's own `noise.ts`/`frames.ts`/`keys.ts`/`guest.ts`, edits documents through `packages/documents/src/convert.ts` + `yjs`, mirrors `transport.ts` (park on PRESENCE 0, redial on CLOSE, sticky `refused` on CLOSE 3, `info` on every open) and exposes a control port so every relay frame carries a timestamp. Nine of the invariants failed on the first run; each failure became one fix commit, then the invariant was re-run on the fixed binary.

1. `9f210e6 fix(secrets,link): 69 smoke — a hub loads the mesh identity, only a pairing mints it; vault rows written before the attach are announced` — `Vault::mesh_device_key_if_set` / `secrets::mesh_device_key_existing` (load-only), `link::existing_mesh_identity`, `cache_mesh_identity`; `follow::sync_mesh` loads (never mints), `DevicePairStart` mints then reconciles, link subscribes to `secrets::SecretsEvent::Changed`; `secrets::replicate` calls `announce_unstamped()` after `replica::attach`. Test extended: `secrets vault::tests::mesh_device_key_is_created_once` (the getter never mints; after the mint it reads the same key).
2. `b47058f fix(link): 69 smoke — no-identity-yet line at debug`.
3. `41ac159 fix(replica): 69 smoke — a peer behind the pruned outbox gets the whole state as one batch` — `Store::oldest_seq`, `Engine::snapshot_batch` (every live row of every attached table under its column clocks + every tombstone, emitted as one batch of this origin), gap detection in `sync::run_stream` before the tail. Test `tests/sync_loopback.rs::a_late_joiner_behind_the_pruned_outbox_gets_a_snapshot`.
4. `a799a1b fix(mesh): 69 smoke — mesh_meta.name and the self engine_stats row are written after the replica attach` — the attach task waits for the replica (`ATTACH_WAIT` 10 s), attaches, then `meta_default` + `write_stats`.
5. `8a0a280 fix(mesh): 69 smoke — a connected peer is a lease candidate; heartbeat age no longer gates it` — `recompute` takes every `net.connected()` peer with an `engine_stats` row; a stale heartbeat is a debug line; gossip neighbour events at debug.
6. `0ec3fb3 fix(mesh): 69 smoke — an engine with stored peers starts with no holder until one connects` — `run_lease` emits `Lease{holder: None, reason: start}` and defers the first election to the first `Connected` or `lapse_secs`; a lone engine is unchanged.
7. `3ccfb6a fix(replica): 69 smoke — apply UPDATEs an existing row; the upsert never applied a column subset to a NOT NULL table` — `apply_row`: `UPDATE … WHERE pk = ?` first, `INSERT` only when 0 rows matched. Test `a_partial_update_lands_on_a_not_null_table`.
8. `6a3c3ab` + `e9e64e6 fix(link): 69 smoke — the first reconcile waits for the mesh's self id and the lease it reports` — `SetSelf(id, Option<LeaseState>)` from `mesh_status`, `MeshAbsent` after `MESH_WAIT` 10 s, `LinkActor.mesh_ready` gates `sync_mesh`.
9. `883abe5 fix(mesh): 69 smoke — the paired set refreshes when the replica delivered a peer row first` — `merge_peers` refreshes when a live incoming id is unknown to the in-memory set; the stranger close is logged at debug.
10. `docs(69): smoke + as-built Z` — `docs/design/69-smoke.md`, this file.

## Deviations from the brief

1. **Invariant 1's shape changed by fix 1.** A lone engine registers the *engine* id at boot; the *mesh* id registers at the first `device_pair_start` (2 ms inside the handler) and at every later boot. K's deviation 2 (every hub mints eagerly) collided the moment two engines that had each run alone were paired: two `mesh/device_key` rows under one pk, the loser serving a key the vault no longer held — every device paired to it would have been stranded at the first failover. Design decision 1 ("created by the first engine that pairs a device") is now what the code does. Merging two meshes that *both* already paired devices remains the design's known ceiling.
2. **The device is a bun script**, not the web UI in headless Chromium (allowed by the brief). It runs the UI's own crypto and frame code and the app's `loadInfo`-on-open behaviour, but not the UI's React state; D's harness list/switcher was not exercised here.
3. **`doc_read` is an agent tool, not an RPC.** "doc_read on B" was read as the local replica's content: `documents/<id>/snapshot.yjs` decoded through `convert.ts` (`mdof.ts`) and B's replicated `doc_list` row; for inv 12 the `doc_open` state decoded the same way, on both engines. Driving haiku turns for `doc_read` would have added model noise to a byte comparison.
4. **`probation_secs: 0`, `handover_secs: 0`** on every engine (68's finding: fresh engines each keep their own lease for the 6 h probation). A `prefer` for inv 2–4 so the pairing lands on A deterministically; A switched to `eligible` (`mesh_set_hub`) before inv 5 so "A does NOT reclaim" is meaningful — a `prefer` engine reclaims by design.
5. **Inv 4's "sending a message works (B takes the user session)"** — `send_input` on a session owned by the dead A answers `-32010 OwnerUnreachable … takeover after 300s` (68 §5 by design); the device then did what the UI's `OwnerOfflineCard` does: `session_take(sid, "user")` (3.3 ms) and resent. Recorded as the passing path.
6. **Inv 7 step 6** happened as a graceful handover plus the kill in the same second: `mesh_set_hub(eligible)` on A let B outrank it, so A sent `HUB_MOVED 5` and the devices were on B 2.35 s later; `kill -9 A` followed 70 ms after B registered. The kill-only path (31 s) is inv 4's.
7. **Inv 10 (b)'s second edit** is confounded by a spontaneous lease handover B→C (C's 30-day availability bucket outranked B — 67 §4.5 with `handover_secs 0`), which wrote lease/availability rows. The document counters (merged, resnapshots 0, identical shas) are clean for both edits; the batch count is reported for the first.
8. **Inv 6 revoked on A** (the non-hub at that moment) rather than B, because B had become hub after its restart; the invariant's point — the hub refuses a device revoked elsewhere — is what was measured.
9. **Working rule "cargo check at most once per commit"**: fixes 1, 3, 5, 7 each needed a second `cargo check`/`cargo test` after a compile error in test code; the brief allows `cargo test -p <crate>` for fixes. Nothing else from the forbidden list ran.
10. The brief's `/tmp/67/state-Z.md` path is the 67 template; state went to `/tmp/69/state-Z.md`, this report to `/tmp/69/report-Z.md`.

## Not built / found, not fixed

- `mesh_meta.name` default resolves to the **youngest** engine's name (`meta_default` stamps at `first_seen_ms` intending oldest-wins, but LWW is newest-wins). Cosmetic; `meta_set` beats defaults as designed.
- Each non-hub re-touches the replicated `docs` row (`bytes`/`updated_ms`) after adopting a hub snapshot: N−1 two-column batches per edit, bounded, not a storm.
- A snapshot batch that reaches a fresh engine before its file mirrors attached is skipped for those tables (`touches unattached table doc_files/skills`); the mirror's own scan converges later.
- `pkarr publish` WARN every 1–13 s on an engine with stored peers and `discovery: true` when `dns.iroh.link` is unreachable (pre-69).
- A graceful *process shutdown* tells devices `HARNESS_GONE 4` (K deviation 6); only a lease loss says `HUB_MOVED 5`.
- Gossip heartbeat delivery pauses (30–60 s) between two loopback peers were worked around (fix 5: liveness = QUIC connection), not diagnosed inside iroh-gossip.
- No UI (D) path was driven; `bunx tsc` not run (no TS touched).
- Throwaway `crates/replica/examples/dump_outbox.rs` (an outbox decoder used to find fix 3) deleted before the docs commit.

## Tests written

- `crates/replica/tests/sync_loopback.rs`: `a_late_joiner_behind_the_pruned_outbox_gets_a_snapshot`, `a_partial_update_lands_on_a_not_null_table`.
- `crates/secrets/src/vault.rs`: `mesh_device_key_is_created_once` extended (load-only getter).

## Verification (exact results)

- `cargo check -p link -p secrets --tests` (fix 1): first run `error[E0277]: can't compare [u8; 32] with Zeroizing<[u8; 32]>` in the extended test; second run `Finished dev profile`. `cargo test -p secrets --lib mesh_device_key` → `ok. 1 passed`. `cargo test -p link --lib` → `ok. 27 passed`.
- `cargo check -p mesh -p replica --tests` (fixes 3–4): first run `E0382 borrow of moved value: self_stats`; second run clean. `cargo test -p replica --test sync_loopback` → `ok. 4 passed` (first run of the new test failed on its own setup — B never wrote, so the prune had no known peer — fixed in the test).
- `cargo test -p mesh --lib` → `ok. 19 passed` (fixes 5, 6, 9). `cargo test -p replica` (fix 7) → lib `13 passed`, `sync_loopback` `5 passed`, files tests `4 passed`.
- `cargo test -p link --lib` (fix 8) → `ok. 27 passed`.
- Final: see the last section of `69-smoke.md` §3 and the check below.
- `cargo build -p theuth --bin t -p relay --bin theuth-relay` after every fix; every invariant re-run on the rebuilt `t`.

Final `cargo check -p link -p secrets -p replica -p mesh -p theuth --tests` → `Finished dev profile [unoptimized + debuginfo] target(s) in 1m 46s`; one warning (`mesh: field stats_hlc is never read`, left by fix 5) fixed in `fix(mesh): 69 smoke — stats_hlc read …`; `cargo check -p mesh --tests` after it clean.

DONE Z
