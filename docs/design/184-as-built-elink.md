# 184 — as built: E-link

Slice E-link of design 184 (`184-notes-any-machine.md` §7.1-§7.2, §8, the RPC parts of §10), on `feat/184-link` from the W0 contract commit f170554c. Paths are under `crates/`.

## The live gate (§8.1, W-g)

| File | What |
|---|---|
| `mesh/src/net.rs` | `Net.docs: DocsLive` (what this engine advertises, the last `DocsSchema` frame per peer with the live connections that carried one, the `heard_ms` touch gate, a `Notify`, a pass mutex). `Net::drive` calls `docs_on_connect` beside `keys::on_connect`: sends `Msg::DocsSchema{DOCS_SCHEMA, homes_on}` and spawns the silence check (`DOCS_SILENT` = 5 s: no frame on any live control connection of that peer → `peer_docs_seen` row `schema 0`; Z184: was "this connection", which a second connection up at once turned into a false silence). A received frame is recorded under `conn.remote_id()` only (`docs_frame`); a stranger never reaches `drive`. `Net::docs_gate(holder, now)` is the pass: paired set (`Store::peers`), rows listed with `first_ms`, `heard_ms` touched at most once a minute for connected peers (`DOCS_TOUCH_MS`), floor, holder's frame (live connection only), and on a flip of `homes_on` a frame to every control connection. A store error answers homes off. A frame drops out when the last connection that carried it closes. |
| `mesh/src/lib.rs` | `docs_floor(peers, seen, now)` (a peer counts unless last heard, or never heard: first listed, `DOCS_STALE_MS` ago or more; counted and below `DOCS_SCHEMA` or never heard → down; no row → down) and `docs_homes_on(floor_ok, holder_is_me, holder_frame)` (fail-closed). `Ctx::docs_gate` runs the pass under the current lease; `DocsGate` answers it; `mesh_status.docs_homes_on` is its `homes_on`. `emit(Lease)` pokes the pass. A loop runs the pass on every poke (frame, silence, connect, disconnect, lease) and every `keys::TICK` (5 s), skipped while the engine has no paired peer. |
| `mesh/src/store.rs` | `DocsSeen` row; `docs_listed`, `docs_heard`, `docs_touch`, `docs_seen` over `peer_docs_seen` (Z184: created in `Store::open`, no longer migration 0008, so a rolled-back pre-184 build still opens the database). |

## RemovedPeers (§4.9)

`Store::removed_peers` = the tombstoned rows of `peer_entries` (quarantined rows are live rows, so never listed; this engine's own row is never listed). `RemovedPeers` answers it; a read error answers `[]` (nothing re-homes this pass).

## Link (§7.2, W-j, W-v)

| File | What |
|---|---|
| `link/src/lib.rs` | `mesh_link_id(&Vault)`: pub-only read (`mesh_device_key_if_set` → `b64(public_of(key))`), never mints, caches nothing. `mint_mesh_link_id(&Vault, holder)`: refuses `not the hub` before touching the vault unless `holder`; mints once with `mesh_device_key`. `MeshLinkId` answers the Link state's cached mesh identity when it has one, else `mesh_link_id` of the runtime's vault, else `Err("the mesh has no link id yet")`; it never sets `status.mesh`. |
| `link/src/types.rs` | unchanged (W0's `MeshLinkId`, `MESH_LINK_UNAVAILABLE`). |

## RPC (§10.4, `rpc/src/api.rs`)

- `doc_link_create`: a lone engine (`mesh::has_peers() != Some(true)`, paired phones or not) runs today's code, byte-for-byte the same URL (`link_ids(status, None, false)`) and the same Link-off refusal. In a mesh, `doc_link_create_in_mesh`: `to_doc_server` (`DocServer` → `forward::to_peer`) first; on the server, Link on → today's v1 ids computed before the link is written, then v2 `2.<harness_id>.<mesh link id>` when `MetaOf.home` is this engine (`mesh::self_id`), the mesh id from `LinkStatus.mesh` or `MeshLinkId`; Link off → `1.<mesh link id>` (`via: mesh`) from `mesh_link_id`: `MeshLinkId`, else mint here when `mesh::is_hub()`, else `link_mesh_id` on the lease holder (a fresh `Extensions`, so a forwarded call can still ask), else `ASK_FAILED mesh_link_unavailable`. The id is resolved before `LinkCreate`, so no link is written without a URL. A local `NotHub` still goes through `doc_forward`.
- `link_mesh_id`: `Forwarded` only (as W0); `mint_mesh_link_id(vault, mesh::is_hub())`.
- `doc_move_home`: `to_doc_server` first, then the local `MoveHome{checks: true}` (E-doc's, a stub until integration).
- `doc_join`: `join_outcome`; `busy` (`document::share::BUSY`) never counts.
- `doc_create`: W0 already sends `Homing::Here`; unchanged.
- Guest `MayRead` (tag 14): no `api.rs` change. The actor's `MayRead` (E-doc, `document/src/notebook.rs:1042`) asks the server before it answers, and every RPC caller counts a refusal only after that answer and only for `not_member`/`viewer_readonly`/`sharing_disabled` (`member_outcome`, `cells_latest.rs`).

## Deviations

- The holder's mint is `secrets::Vault::mesh_device_key` via `link::mint_mesh_link_id`, not `load_mesh_identity` (§7.2 names it): the RPC layer has no Link message for it, and the existing reconcile caches the row and starts the mesh uplink on the holder after the vault's `SecretsEvent::Changed`.
- "Holds the lease" is `mesh::is_hub()` (set on every `MeshEvent::Lease`).
- A flip of `homes_on` is not a new `MeshEvent` (that is TS-visible). Z184: the gate-pass loop publishes the Rust-only `DocsGateState` on the bus when `homes_on` changes, and the document actor applies it (an "off" after 1 s of holding).

## Tests

`mesh`: `net::tests::docs_schema_rides_the_control_connection_on_connect_and_on_change`, `net::tests::a_silent_peer_is_old_and_a_strangers_frame_counts_for_nothing`, `tests::the_floor_counts_every_peer_heard_within_seven_days`, `tests::homes_are_on_only_when_the_holder_and_the_floor_agree`, `store::tests::removed_peers_are_the_tombstones_only`, `store::tests::peer_docs_seen_records_frames_silences_and_touches`. `link`: `mesh_link_tests::the_mesh_link_id_is_read_anywhere_and_minted_on_the_holder_only`. `rpc`: `api::tests::share_links_are_v2_on_the_home_only_and_v1_as_today_elsewhere`, `api::tests::a_busy_join_is_not_a_refusal`.

Not reachable below the live proof: the forwarded `doc_link_create` end to end (`mesh::has_peers` needs the mesh plugin's bound endpoint, plus a document actor and a replica transport in one process); §15.3 checks 9, 13 and 19 cover it.

Fail-before: the two `net` tests fail with the on-connect send disabled (W0's behaviour: no frame, no silence check), 2 failed; the others name functions W0 does not have (compile fail).

## Gates

- `mbx check -p mesh -p link -p rpc -p theuth --tests`: `Finished`, 0 warnings.
- `mbx clippy -p mesh -p link -p rpc -p theuth --tests --no-deps -- -D warnings`: `Finished`.
- `mbx test -p mesh -p link -p rpc`, twice: link 32, mesh 68, rpc 83 (+4 integration binaries, 1 each), all `ok`; the two `net` gate tests five more times, `ok`.
- Bindings: not regenerated (no TS-visible shape changed).
