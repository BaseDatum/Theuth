# 69 as-built — worker E1: replication echo fixes (replica + mesh) and the safety-net counters

Branch `feature/69-docs-e1`. Source: knowledge fact `facts/theuth-replication-echo-audit-2026-09-25`, restated in the brief. Rule: no replication path may echo or idle; every path has a bounded convergence and a counter that would surface a storm.

## Commits

### 1. `fix(mesh): availability row written on open/close/flush interval, not per heartbeat`

- `crates/mesh/src/config.rs`: `mesh.availability_flush_secs` (default 600) and `mesh.storm_frames_per_sec` (default 50).
- `crates/mesh/src/store.rs`: `pub struct Touches` — per-subject "when did the open interval last hit its row" gate (`new(every_ms)`, `wrote(subject, now)`, `due(subject, now) -> bool`). `availability_touch` itself is unchanged; the gate decides who may call it.
- `crates/mesh/src/lib.rs`: `Live.touches`. The 10 s heartbeat tick calls `availability_touch` only for peers whose gate is `due`; `Connected`/`Disconnected` still call `availability_open` (which writes the close of the previous interval and the open of the next) and record `wrote`; on lease-loop exit every connected peer's interval is closed at the true end (previously the last write was ≤ 10 s old, now it could be 10 min old, so the shutdown write is new).
- Behaviour: **before** — one replicated `UPDATE availability` per connected peer every 10 s, forever (3 heartbeats = 3 batches). **After** — 0 writes per heartbeat; one write per peer per 10 min while connected, one on connect, one on disconnect, one at shutdown. Test `store::tests::heartbeats_do_not_write_availability`: 3 heartbeats at 10 s spacing → 0 changes on the feed; the 600 s tick and the close → the expected `to_ms` change on the open row plus the new row.
- Side effect: `availability_30d` reads `to_ms` from the row, so a connected peer's current interval reads up to 10 min short until the next flush. Ratio impact is `≤ 600 s / 30 d`; election unaffected.

### 2. `fix(replica): mirror scan skips rows awaiting projection (apply/scan race)`

- `crates/replica/src/files.rs`: `Mirror::new` extracted (create table, attach, build — no scan, no tasks) so a test can drive `scan`/`project` by hand. New `Mirror.projected: Mutex<HashMap<path, hlc>>`, filled by `project_locked` with the row `hlc` it projected from. `scan` now asks `projection_pending(row, path, mtime_ms)` before upserting: skip when the row's `hlc` is from another origin, its `wall_ms` is above the file's `mtime_ms`, and it is not the `hlc` the file was last projected from. Once the pending projection lands, `projected` names that `hlc` and a later hand edit scans normally (that third condition is what makes the skip converge; without it a remote clock a few seconds ahead would leave file ≠ row forever).
- Behaviour: **before** — in `scan_between_apply_and_projection_does_not_revert`, the scan after apply upserts the old bytes with a B-origin stamp above the observed clock (row `v1`, B ships a batch, A's `v2` is reverted mesh-wide). **After** — row stays `v2`, `flush()` is `None`, B's version vector never gains B's origin; the projection then writes `v2`; a later real hand edit on B (`v3`) replicates as before.

### 3. `fix(replica): prune every origin's outbox; captured/applied/relayed/echo counters and stats()`

Prune:
- `crates/replica/src/engine.rs::prune_outbox`: per origin in the outbox, floor = `min` over every known peer of what that peer holds of the origin (`remote_vv`, 0 when the peer's `Hello` had no entry); `DELETE … WHERE origin = ? AND seq < floor AND seq < MAX(seq) of that origin`. Known peers = other origins in the outbox; any of them without a `Hello` this run → nothing pruned (kept from before).
- `crates/replica/src/apply.rs`: prune also runs after every applied batch, so a receive-only engine bounds its outbox too.
- Behaviour: **before** — only `origin = self` rows were ever deleted; relayed origins grew without bound. **After** (`tests::prunes_every_origin_below_what_all_peers_hold`, three simulated peers B, C, D): own 3 + B 3 + C 2 + D 1 rows; with D silent → 0 deleted; D hello → 1 (C#1); all peers caught up → 4 more, leaving exactly the newest batch per origin (4 rows total, version vector unchanged).

Counters:
- `replica::Stats { captured, applied, relayed, echo, outbox_rows }` (serde + schemars + `ts`), `Engine::stats(&self)`, and process-wide `replica::stats() -> Option<Stats>` reading the engine that `register_transport` marked as the one on the mesh (static `Weak<Engine>`; `None` until the mesh registers).
- `captured` (per column change captured, `capture_locked`), `applied` (per column change applied, `apply`), `relayed` (per column change sent on a stream where `batch.origin != self`, both catch-up and live paths in `sync.rs`), `echo`: `apply` pushes each applied `(table, pk, column, value)` onto a 1024-entry ring with a timestamp; `capture_locked` counts captured changes matching an entry younger than 500 ms. `tracing::warn!` when echoes appear, at most once a minute.
- Behaviour: **before** — no counter anywhere. **After** (`tests::counters_see_captures_applies_and_echoes`): A insert → A `captured 3, outbox 1`; B `applied 3, echo 0`; B deletes and re-inserts the identical row within the window → `captured 4, echo 3`; a real edit → `captured 5, echo 3`. `tests/sync_loopback.rs::relays_through_a_middle_peer` asserts `relayed`: leaf A `0`, B `≥ 3`, C `≥ 2`, `echo` 0 everywhere.
- `sync.rs`: `mark_sent`/`learn` became `pub(crate)` for the prune test.

### 4. `feat(mesh): per-ALPN traffic counters with storm sampler; mesh_status.traffic and .replica`

- `crates/mesh/src/net.rs`: `Counted` (AsyncRead/AsyncWrite over `Box<dyn ByteStream>`) books one frame per `poll_read` returning bytes / per `poll_write`, plus bytes, into per-ALPN `Counters` (`Net.traffic: Mutex<HashMap<String, Arc<Counters>>>`). `Transport::open` and the `Inbound` handler wrap every stream. `Sampler` (1 s windows, driven by the frames themselves): rate above `storm_frames_per_sec` for 10 s → `tracing::warn!("mesh: {alpn} at {rate} frames/s for 10 s …")`, at most once per minute per ALPN; a quiet window resets the sustain clock. `Net::traffic() -> Vec<AlpnTraffic>` sorted by ALPN. `BindOpts.storm_frames_per_sec`.
- `crates/mesh/src/types.rs`: `AlpnTraffic { alpn, frames_in, frames_out, bytes_in, bytes_out }`; `MeshStatus.traffic: Vec<AlpnTraffic>`, `MeshStatus.replica: Option<replica::Stats>`.
- `crates/mesh/src/lib.rs`: `Ctx::status` fills both; `AlpnTraffic` re-exported.
- Tests: `net::tests::pair_connect_and_round_trip_records` now asserts 5 bytes in / 5 out on `theuth/records/1` at both ends; `sampler_warns_on_sustained_rate_once_a_minute` (100 frames/s, limit 50: warns at 11 s and 71 s only; a quiet second resets); `counted_stream_books_both_directions` over `tokio::io::duplex`.
- "Frame" here is a stream read/write, not a protocol message — the wrapper is protocol-agnostic. For `LengthDelimitedCodec` users one message ≈ one write.

### 5. `chore(replica): Stats binding exported as ReplicaStats`

- `ts(rename = "ReplicaStats")` on `replica::Stats`: the Rust name stays as the brief says, the TS binding avoids a generic `Stats.ts`. `MeshStatus.replica: ReplicaStats | null`.

### 6. `docs(69): as-built E1`

This file.

## Deviations

- **Files outside the allow-list** (all small, all forced by the brief's own shape):
  - `crates/mesh/src/config.rs` — the two config keys the brief names (`mesh.availability_flush_secs`, `mesh.storm_frames_per_sec`).
  - `crates/replica/src/lib.rs` — one line: `pub use engine::{stats, Clock, Engine, ReplicaHandle, Stats}` so `replica::stats()` / `replica::Stats` exist as the brief names them.
  - `crates/replica/tests/sync_loopback.rs` — `relayed` assertions added to the existing relay test (the only place a stream runs in tests).
  - `crates/rpc/src/export.rs` — `mesh::AlpnTraffic`, `replica::Stats` added to the binding export list (`crates/rpc/src/api.rs` needed no change: `mesh_status` already returns `mesh::MeshStatus` whole).
  - `crates/sandbox/src/bringup.rs` — test-double `MeshStatus` literal gained `traffic: vec![]`, `replica: None`.
  - `crates/desktop/ui/src/components/tasks/mock.ts` — mock `MeshStatus` gained the two fields (TS would not compile otherwise).
- `replica::stats()` finds "the" engine through a static `Weak<Engine>` set in `Engine::register_transport` rather than an actor message — no new message type in `lib.rs`, and "the engine on the mesh" is exactly the one whose counters `mesh_status` should show. Tests that never register a transport read `None`.
- `captured`/`applied`/`relayed`/`echo` all count **column changes**, not batches, so the four are comparable and `echo` (necessarily per change) has the same unit.
- Echo ring is filled from the applied batch's changes (all non-deleted column writes), not only the LWW winners: simpler, and a losing change re-captured with the same value within 500 ms is still a write-back worth counting.
- Item 3 adds a third skip condition (row `hlc` ≠ last projected `hlc`) beyond the brief's two, for convergence under clock skew — see commit 2.
- Prune also runs after `apply` (brief only named the capture path).
- Item 1 also flushes every connected peer's interval on lease-loop exit (shutdown), since the last heartbeat write can now be 10 min old.
- Bindings: `/bindings` and `crates/desktop/ui/src/rpc/bindings` are gitignored in this tree (only `crates/agent/bindings` is tracked), so `just ts` regenerated `AlpnTraffic.ts`, `Stats.ts`, `MeshStatus.ts` (5 export tests passed, 342 files) but nothing is committed; the regenerated tree also rewrote `crates/rpc/bindings/DocId.ts` from another worker's doc comment — reverted, not mine. `just ts` ran before commit 5, so the on-disk binding is still named `Stats.ts`; the orchestrator's run yields `ReplicaStats.ts`.
- `bunx tsc --noEmit` in `crates/desktop/ui` (bindings copied in first): no error in any file I touched or in `MeshStatus` consumers; 44 pre-existing errors in `packages/apps/*` (missing `@openuidev/*` modules) plus pre-existing `tasks/mock.ts`/`helpers.ts` `Task`/`Owner`/`Checkout` shape mismatches, all unrelated to this work.

## Not built

- No `availability_30d` correction for the in-memory `to_ms` (read the row as-is; bounded at 10 min of under-count while connected).
- No per-peer traffic split (per ALPN only, as specified).

## Tests written (not run here)

- `mesh::store::tests::heartbeats_do_not_write_availability`
- `mesh::net::tests::sampler_warns_on_sustained_rate_once_a_minute`
- `mesh::net::tests::counted_stream_books_both_directions`
- `mesh::net::tests::pair_connect_and_round_trip_records` (extended: traffic assertions)
- `replica::tests::prunes_every_origin_below_what_all_peers_hold`
- `replica::tests::counters_see_captures_applies_and_echoes`
- `replica::files::tests::scan_between_apply_and_projection_does_not_revert`
- `sync_loopback::relays_through_a_middle_peer` (extended: relayed/echo assertions)

## cargo check

One per commit, all clean (warnings only from ts-rs on unrelated serde attributes):

- commit 1: `cargo check -p mesh --tests` → `Finished dev` (one `missing-docs` warning, fixed before commit).
- commits 2–3: `cargo check -p replica --tests` → `Finished dev`, no warnings.
- commit 4: `cargo check -p mesh -p rpc -p sandbox -p theuth --tests` → first run: `missing field storm_frames_per_sec` in `crates/rpc/src/acp.rs` test `BindOpts` + 5 missing-docs warnings; fixed; second run `Finished dev`, clean.
- commit 5: `cargo check -p replica --features ts` → `Finished dev`.
- `just ts` (= `cargo test -p rpc --features ts export_bindings`): 5 passed.

Not run (orchestrator's job): `cargo test -p replica -p mesh --lib`, `cargo test -p rpc`, the desktop.

DONE E1
