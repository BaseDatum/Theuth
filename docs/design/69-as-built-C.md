# 69 worker C — report (branch `feature/69-devices`, 2026-09-25)

Contract: `docs/design/69-contract.md` (every name with file:line).

## Built, per commit

1. `a594463 feat(mesh): lease::Follower moves to mesh; mesh_meta; mesh_status names`
   - `Follower` (+ its two pure tests) now in `crates/mesh/src/lease.rs`; `roster::lease::Follower` is a `pub use` — roster/slack/signal untouched (roster keeps `Duties`/`apply` + the `apply` test).
   - `crates/mesh/migrations/0004_mesh_meta.sql`; `Store::meta_get/meta_set/meta_default`; `mesh_meta` in the replica attach list; `name` seeded at start.
   - `MeshStatus { self_name, mesh_name }`; `Net.name` made `pub`; `mesh_status` fills both; `t mesh status` prints the JSON so both show. Struct literals updated: `crates/sandbox/src/bringup.rs` (test), `crates/desktop/ui/src/components/tasks/mock.ts`.
2. `d9a9e98 feat(secrets): reserved mesh/ prefix; mesh/device_key row`
   - `MESH_PREFIX`, `writable_name` accepts `mesh/<name>` (set_if_version only); `Vault::list()` hides every `/` name; internal post-write lookups use a new private `info_of` so `oauth/` writes keep returning their info.
   - `Vault::mesh_device_key()` + free `secrets::mesh_device_key(rt)`; `seal_with_mesh_key`/`open_with_mesh_key` exposed.
3. `f5223f0 feat(link): link.sqlite — devices + pending as replicated records; LinkStatus.mesh; HUB_MOVED`
   - `crates/link/migrations/0001_link.sql`, `crates/link/src/store.rs` (Store, ordered writer, `Op`, `PendingRow`), `devices.rs` rewritten (`Devices` mirror + `Pending` rows + one-time YAML import), `mesh_uplink.rs` stub with the `serve` gate, `State { pending, mesh_identity, mesh }`, `LinkStatus.mesh`, `LinkEvent::MeshRelay`, `PairHello.pending`, 4-part pairing string, `conn::respond` takes a PSK chooser (`Handshake::Pair.psk`), close code 5 in relay `frame.rs`, link `uplink.rs`, UI `frames.ts`.
   - Link now depends on `sqlx`, `replica`, `secrets` (and `mesh` from commit 4).
4. `0b4dc9e feat(rpc,theuth): devices_list / device_revoke / device_pair_start; t mesh devices`
   - `DeviceInfo`, `PairingPayload`, `DevicesList`, `DevicePairStart`, `LinkEvent::{DevicePaired, DevicesChanged}`; RPC `devices_list`, `device_revoke`, `device_rename`, `device_pair_start`; export list; `t mesh devices list|add [name]|revoke <id>|rename <id> <name>`; TUI re-asks on the three new events.
5. `docs(69): contract` — `docs/design/69-contract.md`, this report as `docs/design/69-as-built-C.md`.

## Deviations (decided alone; nobody to ask)

- `secrets::mesh_device_key(rt)` returns `Zeroizing<[u8; 32]>`, not `link::Identity`: secrets → link would be a cycle (link → secrets is new in commit 3). Link wraps it (`LinkActor::mesh_identity`).
- `Pending::create(name)` returns `(id, psk, expires_ms)` — the payload needs the expiry; `expires_ms(id)` exists too.
- `mesh_meta.name` default: `meta_default` stamps the row at the engine's `first_seen_ms` instead of now. Plain set-if-absent + LWW would let a *later-joining* engine's default (newer clock) overwrite the mesh's name on replication; an old stamp makes the oldest engine's default win and any real `meta_set` beat every default. Verified against replica's strict `>` merge (`crates/replica/src/apply.rs:171`).
- `Devices`/`Pending` are in-memory mirrors over the sqlite store (sync reads under the state mutex; mutations queue to one ordered writer task). Rows written by *other* engines arrive by replication into the table only — K must call `Devices::reload(store.devices())` / `Pending::reload(store.pending())` (poll or the replica feed). `hlc` is left NULL: the replica stamps at capture.
- `pending` rows are deleted when taken/expired (written once, so the tombstone is always newer) — the "never delete" rule is applied to `devices` only, as the design says.
- `respond` reads IKpsk2 msg1 under a placeholder PSK and `set_psk(2, …)` after choosing (psk2's token is in msg2). The engine path ignores the hint (single in-memory pairing); the mesh chooser is spelled out in the contract for K.
- `device_revoke`/`device_rename` reuse the `Revoke`/`RenameDevice` actor messages (same table now); both also publish `DevicesChanged`.
- `device_pair_start` default name: `mesh_status.mesh_name` → `self_name` → `link.name`/hostname. Link gained a `mesh` dependency for `mesh::actor()` + `mesh::Status` (no cycle: mesh does not depend on link).
- Relay `HUB_MOVED` carries `#[allow(dead_code)]`: the relay only forwards the byte (`relay.rs` `frame::CLOSE` arm takes `payload[0]` verbatim); counters go to `COUNT = 6`.
- One-line changes outside my list, disclosed: `crates/nodes/src/lib.rs` (`PairHello { pending: parsed.pending.clone() }` — nodes now forwards the hint; 2 test `build` calls), `crates/nodes/src/conn.rs` (3 test calls), `crates/tui/src/model.rs` (exhaustive `LinkEvent` match + one test literal), `crates/sandbox/src/bringup.rs` (test literal), `crates/desktop/ui/src/components/tasks/mock.ts` (mock literal), `crates/roster/src/lease.rs` (re-export), `crates/mesh/src/net.rs` (`name` made `pub`), `Cargo.lock`.
- Two `cargo check` runs for commit 3 and two for commit 4 (first of each failed on a one-line error; fixed and re-run).
- `MeshUplink::run_once` is the requested stub: it `bail!`s "not built (69 K)"; nothing starts it.

## Not built (by design of the brief)
The mesh uplink body (K), `Follower` wiring in link (K, `follow.rs`), `Devices::reload`/`Pending::reload` polling (K), `HUB_MOVED` sending (K), every UI surface (D), smoke (Z).

## Tests written (not run except where stated)
- mesh: `lease::tests::{follower_tracks_the_lease, set_self_without_a_lease_is_no_change}`, `store::tests::meta_default_keeps_the_first_value_and_stamps_old`; `tables_attach_to_replication` now includes `mesh_meta`.
- secrets: `names` (+ mesh/ cases), `backend_rows_are_hidden_and_unexpandable`, `mesh_device_key_is_created_once` (two racing callers on one vault, reopen, seal/open helpers).
- link: `store::tests::tables_attach_and_rows_round_trip`, `devices::tests::{yaml_is_imported_once_and_revoke_keeps_the_row, pending_rows_seal_take_and_expire}`, `mesh_uplink::tests::stop_ends_the_supervisor_without_redial`, `pairing::tests::build_parse_round_trip` (3/4/5 parts), `conn::tests::ik_then_ikpsk2_fallback` (hint → chooser, unknown field tolerated, refuse without PSK), `initiator::tests::pair_round_trip` (no hint).

## Results
- `cargo check -p mesh -p roster -p sandbox -p slack -p signal --tests` (commit 1): Finished, no warnings.
- `cargo check -p secrets --tests` (commit 2): exit 0, no warnings.
- `cargo check -p link -p nodes -p relay -p tui --tests` (commit 3, after `just ts` so `bindings/methods.json` exists for nodes' `include_str!`): exit 0, no warnings.
- `cargo check -p link -p mesh -p secrets -p rpc -p theuth -p tui --tests` (commit 4): exit 0, no warnings.
- `cargo test -p link -p mesh -p secrets --lib`: link 21 passed, mesh 16 passed, secrets 15 passed, 0 failed.
- `just ts` (= `cargo test -p rpc --features ts export_bindings`): ok; `bindings/` (gitignored) gained `MeshLink.ts`, `DeviceInfo.ts`, `PairingPayload.ts`, regenerated `MeshStatus.ts`, `LinkStatus.ts`, `LinkEvent.ts`; copied to `crates/desktop/ui/src/rpc/bindings` (also gitignored — nothing to commit; the 39 files tracked under `crates/rpc/bindings` are an older stray subset, untouched).
- `bunx tsc --noEmit`: could not run — this worktree has no `node_modules` (TS2688 for `node`/`vite/client`); `bun install` was not on my list. The only TS edits are two literal additions (`mock.ts` fields present in the regenerated `MeshStatus.ts`; `frames.ts` constants).
- Was `oauth/*` hidden from `secret_list`/`list_secrets` before? **No** — `Vault::list()` returned every entry. Now every `/` name (`oauth/*`, `mesh/*`) is hidden.
- Exact QR string `device_pair_start` produces: `<https|http>://<link.relay host[:port]>/link#1.<b64url(mesh_pub ‖ psk)>.<b64url(name)>.<b64url(pending_id)>` — `pairing::build(&cfg.relay, &mesh_identity.public, &psk, &name, Some(&pending_id))`. Not executed here (no engine run allowed); with pub = 32×0x01, psk = 32×0x02, name `work`, pending id `AbC-_123` and relay `wss://relay.example` it is exactly:
  `https://relay.example/link#1.AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQECAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAg.d29yaw.QWJDLV8xMjM`
  (real values: 86-char 2nd part, 22-char 4th part from 16 random bytes).

DONE C
