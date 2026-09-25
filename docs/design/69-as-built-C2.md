# 69 C2 — as built (2026-09-25)

Branch `feature/69-docs` (cut from `feature/69-devices` at `21af0ff`). Four commits + this report. Contract for M/Y/Z2: `docs/design/69-contract-docs.md`.

## Commit 1 — `feat(document): deterministic DocIds per binding; docs gains hlc + soft delete; touch only at snapshot/export` (`7f0196c`)
- `meta.rs`: `DocId::NAMESPACE` (fixed v5 namespace) + `DocId::for_binding(&Binding)` — uuid v5 of `Binding::key()` for bound kinds, v7 for `Note`. Used by `Ensure` and `create_doc` (the only minters).
- `store.rs`: `docs.hlc TEXT` (in `CREATE`, `ALTER TABLE … ADD COLUMN` on old files, duplicate-column error ignored); `store::TABLES = ["docs"]`; `Store::pool()`; `Store::migrate_ids` at `open` (several live rows per binding → keep newest `updated_ms`, load the others (snapshot + log), `apply_update` into the kept doc, snapshot, trash them, `info` per merge; any kept/lone row not at `for_binding` → `Store::rename`: dir, `meta.json` id/`derived` (artifacts), index row id, `info` per move); `Store::create` upserts (`deleted_ms = NULL`, revives a soft-deleted deterministic id); `Store::lookup` no longer deletes a row whose dir is missing (returns `None`, warns); `sweep_trash` removes `.trash` dirs, never rows; `touch` → `touch_at_snapshot`.
- `actor.rs`: per-update `store.touch` removed from `apply`; `snapshot()` spawns `touch_at_snapshot(id, meta.updated_ms, n)`.
- `lib.rs`: `lookup_when::<replica::Replica>` → `replica::attach(pool, TABLES)` + `announce_unstamped()`; local-only on `Err`.
- Disclosed one-liners outside `crates/document`: root `Cargo.toml` uuid features `+ "v5"`.

## Commit 2 — `feat(document): flat replicated mirror dir with merge-on-apply; hub snapshots on quiet` (`2e577ec`)
- New `crates/document/src/mirror.rs`: `parse(path) -> Option<(DocId, MirrorFile)>`, `file_name(id, kind)`, `start(mirror_dir, actor)` (notify, non-recursive, 300 ms debounce → `MirrorChanged`), `merge_meta(local, incoming) -> bool` (the written-down rule, contract §3.2) + tests.
- `store.rs`: `Store.projected` (name → sha of own mirror writes); `mirror_dir()`, `mirror_write(id)` (dot-temp atomic `write_hidden`, both files, called from `create`/`write_meta`/`snapshot`/`rename`), `mirror_remove` (from `delete`/`rename`), `last_projected(id)`, `is_own_projection(name, bytes)`, `mirror_apply(path) -> Applied {Ignored|Unchanged|Merged|Created|Deferred}` (closed-document merge on disk: `diff_from` + `merge_into` → re-snapshot only when the state grew; meta → `merge_meta`; missing dir → created from both mirror files; removed meta + tombstone row → local dir to `.trash`); free fns `diff_from(snapshot, sv)`, `merge_into(doc, update) -> changed`.
- `actor.rs`: `MirrorChanged` handler (own write → skip; open doc → `diff_from` → `apply(.., Principal::File)` + `schedule_export`; open meta → `merge_meta` → `write_meta` + `Members`/`Meta` events; closed → `mirror_apply` + `DocumentChanged`); `Tick` snapshots after `documents.snapshot_quiet_ms` (default 2000) of quiet with a non-empty log.
- `lib.rs`: `DocumentsConfig.snapshot_quiet_ms`; `replica::files::mirror_at(documents/mirror.sqlite, mirror_dir, "doc_files", 60 s)` (disclosed replica-shape one-liner); `mirror::start`.

## Commit 3 — `feat(document): hub gate — the yrs server follows the lease` (`35a2801`)
- New `crates/document/src/hub.rs`: `DocError { NotHub { hub: Option<EngineId> }, Other(String) }` (`Display` `NotHub:<id>`, `parse`, `From<String>/<&str>`, `Into<String>`), `Hub { follower: mesh::lease::Follower, holder, watcher: Option<CancellationToken> }` (`gate()`, `gate_str()`, `holding()`, `holder()`, `set_self`, `observe`), `DocumentActor::sync_hub()` (Slack's reconcile), messages `SelfId`, `Reconcile`, `Message<mesh::MeshEvent>`, `HUB_MOVED = "hub_moved"`; tests.
- `watch.rs`: `start(data_dir, actor, cancel: CancellationToken)`.
- `actor.rs`: `hub` field; gates on `OpenDoc`, `ApplyUpdate`, `AwarenessUpdateMsg`, `Subscribe` (reply `Result<_, DocError>`), and `Ensure`, `ImportMarkdown`, `Edit`, `UndoLastAgentEdit`, `CreateNote`, `Create`, `Rename`, `DeleteNote`, `Snapshot`, `SetDerived`, `Restore`, `Delete` (String form), `FileChanged` (dropped off the hub), `schedule_export` (no exports off the hub); `MirrorChanged` unloads a read-cached doc off the hub and merges on disk.
- `share.rs`: `LinkCreate`, `LinkRevoke`, `MemberSet`, `MemberRevoke`, `SetName`, `MembersOf` → `Result<_, DocError>` + gate; `JoinError::NotHub(Option<EngineId>)`.
- New `crates/document/src/relay.rs`: `Msg` (Open/Update/Awareness/Subscribe/Event/Join/Closed, tags 0–6), `encode`/`decode` (tag + postcard), `MAX_FRAME = 64 MiB`, `framed()`; test. No runtime.
- `lib.rs`: `pub mod hub; pub mod relay;` `pub use hub::{DocError, HUB_MOVED}`; start: `tell(Reconcile)`, `rt.subscribe::<DocumentActor, mesh::MeshEvent>`, `lookup_when::<mesh::Mesh>` → `Status` → `SelfId`.
- `crates/rpc/src/api.rs`: `NOT_HUB = -32011`, `doc_send` (maps typed or parsed `NotHub` → `-32011` with data `{"NotHub":{"hub":…}}`), every `doc_*` handler and `doc_subscribe` use it.
- Disclosed one-liners: `crates/replica/src/lib.rs` `ALPN_DOCS = "theuth/docs/1"`; `crates/mesh/src/net.rs` `STREAM_ALPNS` 6 → 7 with `ALPN_DOCS`; `crates/document/Cargo.toml` deps `mesh`, `replica`, `bytes`, `postcard`, `tokio-util codec`.
- Integration test `hub_gate_follows_the_lease` in `crates/document/tests/actor.rs` (skips without the helper binary, like its siblings).

## Commit 4 — `feat(rpc): share links name the mesh; guests on the mesh uplink; DropGuest everywhere`
- `api.rs` `doc_link_create`: URL id = `LinkStatus.mesh.device_id` when present, else `harness_id`; Link must be enabled; trait doc updated.
- `crates/link/src/lib.rs` `DropGuest`: no code change — the loop over `State::conns` already covers every uplink's conns; a comment names the contract (K registers mesh conns there). Disclosed.
- `docs/design/69-contract-docs.md` (this contract) and this report (`docs/design/69-as-built-C2.md`).

## Deviations
1. **`docs/design/69-devices.md` §5 has no decisions 10–13** on this branch (the doc holds decisions 1–9); `docs/design/69-recon-docs.md` and the knowledge fact `facts/theuth-documents-store-and-live-editing-facts-2026-09-25` were not present in the worktree. Built from the brief's hard constraints and the code.
2. `DocError::NotHub.hub` is `Option<EngineId>`, not `EngineId`: `holder: None` is a loss with nobody to forward to; the error says so (`"hub": null`).
3. Gate scope widened to **every write** (`Ensure`, `Edit`, `Create*`, `Rename`, `Delete*`, `Restore`, `Snapshot`, `SetDerived`, `Undo`, `ImportMarkdown`), not only the listed RPC methods — a replica that creates or imports would fork Yjs histories. Reads (`Lookup`, `ListDocs`, `Snapshots`, `Outline`, `Markdown`) serve anywhere.
4. Those owner-plugin messages keep `Result<_, String>` replies (artifact/meeting/knowledge callers match on `String`; not in my allow-list) and answer `NotHub:<id>`; `DocError::parse` + `doc_send` make it the same typed `-32011` on the wire. The doc_* surface is typed.
5. Migration also **moves lone rows** to the deterministic id (the brief spoke only of duplicates): convergence across engines needs every id to be `for_binding`, not just the deduped ones. Artifact `derived` paths move with the dir.
6. `Store::lookup` no longer marks a row deleted when its dir is missing (would replicate a delete for a doc whose mirror files are simply not here yet); `create` upserts so a recreate under the same id revives the row.
7. `mirror_at` opens a separate `documents/mirror.sqlite` rather than sharing `index.sqlite`.
8. `DropGuest`: no link code change (see commit 4).
9. `Store::touch_at_snapshot` is called from the snapshot path only; the export path already writes `updated_ms`/`title`/`members_n` via `write_meta`. `bytes` lags by a log's worth (documented).
10. Known ceiling recorded in the contract §1: engines that imported the same concept independently before this branch double their blocks on the first merge.

## Not built
- Forwarding runtime (`relay.rs` has the wire shape only); `forward.rs` streaming leg; the guest path on the mesh uplink (K's `run_once` is still a stub). Y/K.
- TS bindings: nothing new exported (`DocError` is RPC error `data`, not a `ts_rs` type); `just ts` not run.

## Tests written
- `meta.rs`: `bound_ids_are_deterministic_and_notes_are_not`.
- `store.rs`: `open_converges_ids_and_merges_duplicates`, `mirror_files_create_then_merge_and_never_loop`; `trash_sweep_removes_only_old_deletions` updated (rows are tombstones).
- `mirror.rs`: `mirror_names_parse_and_skip_temp_files`, `meta_merge_is_a_join_that_keeps_local_export_state`.
- `hub.rs`: `not_hub_round_trips_its_display_and_json`, `gate_follows_the_lease_and_a_lone_engine_is_the_hub`.
- `relay.rs`: `frames_round_trip_with_matching_tag`.
- `tests/actor.rs`: `hub_gate_follows_the_lease` (needs the helper binary).

## Dedupe migration on a copy of `~/.theuth/documents`
Copied to `/tmp/69/docs-copy/documents` (original untouched; 36 entries: 35 doc dirs + `index.sqlite`, 43 rows of which 35 live: 23 artifact, 9 knowledge, 3 meeting_note). `Store::open` on the copy (a throwaway `#[ignore]` probe, removed before this commit):
- **0 merges** (no binding had two live rows on this single engine — as the brief predicted for the 9 knowledge docs).
- **35 moves** (`documents: id converged from=<v7> to=<v5>`), every live row now at `DocId::for_binding`; `rows_total=43` (tombstones kept); `mirror/` populated with 66 files (35 meta + 31 snapshots — 4 documents have no `snapshot.yjs` yet).
- Second `open` on the same copy: idempotent (covered by the unit test).

## cargo check / test
- Commit 1: `cargo check -p document --tests` — ok (1 `unused_mut` warning fixed before commit; 2 pre-existing warnings in `tests/actor.rs`).
- Commit 2: `cargo check -p document --tests` — ok (1 unused import fixed before commit).
- Commit 3: `cargo check -p document -p replica -p mesh -p rpc --tests` — ok, only the 2 pre-existing warnings.
- Commit 4: `cargo check -p document -p replica -p rpc -p theuth --tests` and `cargo test -p document -p replica --lib` — results appended below.

```
cargo check -p document -p replica -p rpc -p theuth --tests
  Finished `dev` profile [unoptimized + debuginfo] target(s) in 1m 42s
  warnings: 2 pre-existing (`vlink`, `elink` unused in crates/document/tests/actor.rs:774,806); 0 errors
cargo test -p document -p replica --lib
  document: test result: ok. 30 passed; 0 failed; 0 ignored
  replica:  test result: ok. 10 passed; 0 failed; 0 ignored
```
`just ts` not run: no `ts_rs` type changed.

DONE C2
