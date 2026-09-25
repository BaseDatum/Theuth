# 69 E2 — as built (2026-09-25)

Branch `feature/69-docs-e2` (cut from `feature/69-docs` at the Y merge, `8c75bed`). Context: Sky's standing rule — no replication path may echo; the audit (`facts/theuth-replication-echo-audit-2026-09-25`) named two document-mirror risks. Contract `69-contract-docs.md`; M and Y's as-builts are the state found. One code commit + this report.

## Commit 1 — `feat(document): mirror echo fixes — adds_anything gate, verbatim snapshot adoption, doc_stats, Edit forwarding off the hub`

### 1. Hub no-op merge (`store.rs`, `actor.rs` `MirrorChanged`)
- `store::decode_state(snapshot) -> Doc` (a peer's full state as a document of its own), `store::exceeds(a, b) -> bool` — true only if `a`'s state vector has a clock above `b`'s for some client, or `a`'s delete set minus `b`'s (`IdSet::diff`) is non-empty — and `store::adds_anything(bytes, &doc)` = `exceeds(&decode_state(bytes)?, doc)`. The hot paths decode once and call `exceeds` directly (the brief's `adds_anything` is the same predicate; using it beside `diff_from` would decode the snapshot twice).
- `actor.rs` open-document path: `decode_state` → `exceeds(theirs, o.doc)` or return; the diff (`theirs.transact().encode_state_as_update_v1(&ours_sv)`) goes through `apply` only when there is news. Before: `d.len() > 2` — never false once the document held a deletion, since `encode_state_as_update_v1(sv)` always appends the whole delete set, so every foreign snapshot went through `apply` (log append, `updated_ms`, fan-out, `schedule_export`), re-snapshotted and shipped.
- `store.rs` closed-document path: same predicate replaces `diff_from` + `merge_into` (both removed; their only non-test caller was the actor; M's test rewritten to `adds_anything`/`decode_state`/`exceeds`).

### 2. Re-snapshot amplification (`store.rs` `mirror_apply`)
- After `exceeds(theirs, local)`: if `exceeds(local, theirs)` (this engine holds state the peer lacks) → the join is applied and **re-snapshotted** (`Store::snapshot` → mirror write → ships once; `resnapshots += 1`). Otherwise → `Store::adopt_snapshot(id, bytes)`: the incoming bytes become `documents/<id>/snapshot.yjs` verbatim, `updates.log` (which they cover) is truncated, and their sha is recorded as `Store::projected[<id>.snapshot.yjs]` (`last_projected`) **without** `mirror_write` — the mirror file the replica projected already equals its row, so the scan rows nothing and zero batches ship.
- `Store::touch_if_changed(id, updated_ms, bytes)`: `touch_at_snapshot` only when the row's `bytes` differs (a same-value `UPDATE` is still a replication batch).
- `Applied` semantics unchanged (`Merged` for both the adoption and the real merge).

### 3. Counters
- `store::Counters { merged, unchanged, resnapshots, noop_applies_avoided: AtomicU64 }` on `Store` (`Arc`, shared by clones), `Store::stats() -> DocStats`; `store::DocStats` (serde, schemars, `kameo::Reply`, `ts_rs` under `ts`), re-exported as `document::DocStats`.
  - `merged`: foreign snapshots that held news and were joined (closed or open document).
  - `unchanged`: foreign snapshots that held nothing (both paths).
  - `resnapshots`: real merges that re-encoded and shipped a snapshot (closed path; the open path's re-snapshot is the hub's normal `Tick`).
  - `noop_applies_avoided`: `unchanged` on the open-document path — each of those went through `apply` before E2 whenever the document held a deletion.
- `document::Stats` (`ask` → `DocStats`, served anywhere) and RPC `doc_stats` (`crates/rpc/src/api.rs`, through `doc_send`).

### 4. Tool edits off the hub (`actor.rs` `Edit`, `relay.rs`)
- `Message<Edit>` now matches `Hub::route()` like `OpenDoc`/`ApplyUpdate`: `Ok(None)` → `start_edit` (unchanged); `Ok(Some(hub))` → `forward(hub, doc, Client::of(&by), Cmd::Edit { edits, by, reply })`; `Err` (nobody holds the lease) → `EditError::Other("NotHub:")`. Every `Edit` is routed, whoever `by` is: the tools (`Agent{session}`), the task-thread appender (`Owner{device: None}`); `UndoLastAgentEdit`/`Rename` still gate at their own handler and never reach it off the hub.
- `relay::Msg::Edit { doc, edits: Vec<u8>, by: Principal }` (tag 9, appended beside Y's 7–8): `edits` is the `Vec<BlockEdit>` as JSON (internally tagged + `serde_json::Value` inside `Restore` — not postcard's), `by` is the principal itself (postcard-clean). `Cmd::Edit`, `Pending::Edit`. Hub side (`serve_stream`): `Ok(outcome)` → `Event` carrying `Result<EditOutcome, EditError>` JSON; a typed `EditError` (`UnknownBlock`, `FrontmatterImmutable`, `BadEdit`) → the same `Event` with `Err`, the leg lives on; `EditError::Other` (not serialisable, `#[serde(skip)]`) → `Closed{reason}` like any handler error. Front door: `Pending::Edit.answer` parses the `Result`; `fail(reason)` → `EditError::Other(reason)`.
- Hub unreachable (no transport, holder not in `peers()`, leg queue full, stream lost) → `EditError::Other("NotHub:<id>")`, the same `Display` form the rest of the String surface answers; `tools.rs` shows it verbatim (`model_error` = `Display`), the tasks writer logs it. Nothing is dropped silently.
- `doc_read`/`Outline`/`Markdown` untouched (local).

### 5. Convergence proof — see tests.

## Deviations
1. **`document::stats()`** is `Store::stats()` + the `Stats` actor message + the `doc_stats` RPC, not a free function: counters are per `Store` so the three-Store test can tell A's from B's and C's; a process-global would not. No `mesh_status` hook exists — `MeshStatus` is `mesh`'s type and the RPC handler returns it straight from the mesh actor, so the RPC route was taken (brief's fallback). **`just ts` not run** (the working rules forbid `just`; `DocStats` is `ts(export)`, the orchestrator's `just ts` picks it up).
2. **Contract §3.2 "foreign bytes never replace `documents/<id>/snapshot.yjs`"** is superseded by brief 2: they replace it exactly when they hold everything here (the join *is* their state), never when this engine holds state the peer lacks. Local state is never lost; the contract file itself is not edited (its rule is about names).
3. **`diff_from` / `merge_into` removed** rather than left beside the new predicate (one convention; only the actor and one test used them). The brief said "instead of the length/byte checks".
4. **`adds_anything` is not the call the hot paths make** — they call `decode_state` + `exceeds` to decode the snapshot once for the check and the diff; `adds_anything` is the one-line composition (tests use it).
5. **Counter placement**: `unchanged` counts both paths and `noop_applies_avoided` the open path (see 3); `resnapshots` counts closed-path re-encodes only — the open path's snapshot is the hub's `Tick`, not attributable to one foreign file.
6. **Files outside the allow-list** (each disclosed): `crates/document/src/relay.rs` (`Msg::Edit` + `Cmd`/`Pending`/leg/serve arms; tests), `crates/document/src/lib.rs` (`pub use actor::Stats`, `pub use store::DocStats`), `crates/rpc/src/api.rs` (`doc_stats` trait method + impl). `mirror.rs` needed no change.
7. **Brief 4's test** ("two-actor loopback — an Edit on the non-hub lands in the hub's Doc and the non-hub's mirror catches up") is a unit test in `relay.rs` with a `sh` fake helper (`edit_helper`: answers `edit` with one canned Yjs update as the transaction, `ok` to everything else), because the real helper binary is only available to `tests/actor.rs`, whose loopback transport is private to `relay.rs`'s tests. The hub's `Doc` holds the text after the forwarded edit; the mirror files are copied by hand (as M's and Y's round trips do) and B ends with the hub's bytes verbatim (`resnapshots == 0`).
8. Test-only: `register_hub` clears a stale `DOCUMENT_ACTOR` registration (a previous test's runtime tears its actor down without unregistering) and a `SERIAL` mutex orders the two registering tests — the first run failed with `NameAlreadyRegistered` when they ran in parallel.
9. Pre-existing flake seen once: `meta::tests::doc_id_is_26_base32_chars_and_round_trips_the_alphabet` (`a.0 < b.0` after a 2 ms sleep; uuid v7 ordering) failed in the second run and passed in the first; untouched.

## Not built
- Nothing from the brief. `ImportMarkdown` / `Ensure` off the hub (Y's "not built" list, decision 11's last clause beyond `Edit`) still answer `NotHub:<id>` — not in this brief.
- Contract text update for §3.2 (see deviation 2).

## Tests written
- `store.rs`: `contained_snapshot_with_deletions_writes_nothing` (brief 1: same state + deletions → `diff.len() > 2` yet `adds_anything == false`; a further deletion elsewhere is news; on disk the foreign copy changes nothing — `snapshot.yjs`, `updates.log`, the row's `updated_ms`/`bytes`, the mirror rows and `last_projected` are byte-identical before/after; `merged == resnapshots == 0`), `catching_up_ships_no_rows_and_a_real_merge_ships_one` (brief 2: two Stores exchanging files as the replica would; after A's snapshot lands B ships 0 rows and holds A's bytes and sha, log compacted; with a local B edit first B ships exactly 1 row and A adopts it without re-encoding; texts equal, nothing duplicated, a further round moves 0 bytes), `three_stores_converge_in_one_round_without_echo` (brief 5: A hub, B, C; one edit on A → one round moves 2 files, B and C each `merged +1`, `resnapshots +0`, hold A's bytes; A `merged +0`; the second round moves 0 bytes; all three mirror dirs identical). Helpers `rows`, `exchange`, `delete_update`. `mirror_files_create_then_merge_and_never_loop` updated to the new API (M's assertions hold).
- `relay.rs`: `frames_round_trip_with_matching_tag` (+ tag 9), `forward_edit_lands_on_the_hub_and_the_mirror_catches_up` (brief 4), `forward_refuses_with_not_hub_when_the_hub_cannot_be_reached` (+ `Edit` → `EditError::Other("NotHub:zz")`).

## cargo check / test
```
cargo test -p document --lib          (run 1)  36 tests: 35 passed; 1 failed — forward_edit_… NameAlreadyRegistered (fixed: register_hub + SERIAL)
cargo test -p document --lib          (run 2)  36 tests: 35 passed; 1 failed — meta::doc_id_is_26_base32_chars… (pre-existing flake, deviation 9); every E2 test passed
cargo check -p rpc -p document        Finished `dev` profile [unoptimized + debuginfo] target(s) in 7m 44s — 0 errors, 0 warnings
```
The 4 `missing-docs` warnings the test build showed on `Counters` fields were fixed before the commit. Code commit: `f0d5571`.

DONE E2
