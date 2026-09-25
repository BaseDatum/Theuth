# 69 M — as built (2026-09-25)

Branch `feature/69-docs-m` (cut from `feature/69-docs` at C2's commit 4, `16e56b8`). One code commit + this report.

## State found
C2's commits 1–2 on the base branch already hold the whole M brief as bodies, not stubs: `Store::mirror_write` / `last_projected` / `is_own_projection` / `mirror_remove` (brief 1), `mirror::start` + `DocumentActor` `MirrorChanged` handler + `Store::mirror_apply` (brief 2–3), `DocId::for_binding` + `Store::migrate_ids` / `rename` (brief 4), the `Tick` quiet snapshot (brief 5), and tests `open_converges_ids_and_merges_duplicates`, `mirror_files_create_then_merge_and_never_loop`, `meta_merge_is_a_join_that_keeps_local_export_state`. M's work is the gap between that and the brief, verified end to end against the contract (`69-contract-docs.md` §1–§3).

## Commit 1 — `feat(document): mirror creates a document from its meta alone; quiet snapshot is hub-only; two-store mirror round trip test` (`8e16491`)
- `store.rs` `Store::mirror_apply`: a document unknown here is created from the mirrored `<id>.meta.json` **alone** (plus `<id>.snapshot.yjs` when already projected); a snapshot arriving before its meta is `Applied::Deferred` and the meta's arrival retries. Before: both files were required, so a document created on the hub but never edited (no snapshot — 4 of Sky's 35 are like that) never appeared on another engine. `Applied::Created` / `Deferred` docs updated.
- `actor.rs` `Tick` (disclosed one-liner outside store/mirror): the quiet-period snapshot clause is `self.hub.holding() && …` — the non-hub never snapshots (brief 5); the `LOG_MAX_*` clauses stay as they were (off the hub no update is applied, so they cannot fire).
- `store.rs` test `mirror_files_create_then_merge_and_never_loop` extended into the two-Store round trip the brief asks for (files copied between `mirror/` dirs as the replica would): snapshot-first → `Deferred`, meta → `Created`; B keeps a live `Doc` and a local unsnapshotted edit; A's next snapshot → `Merged` on B with A's and B's text both present and nothing duplicated (length check), log compacted, B's `snapshot.yjs` ≠ A's bytes (foreign bytes never overwrite); the same foreign snapshot through `diff_from` + `merge_into` into the open `Doc` joins once (text equal to the on-disk join) and a second pass is `[0,0]` / `false`; B's snapshot back on A → `Merged`, A and B converge, a second copy is `Ignored|Unchanged`; a meta-only document (`create`, no snapshot) → `Created` on B, `lookup` is `None` until its `docs` row is inserted, then `Some`, and its first snapshot → `Merged` with the text; foreign meta with a new member → `Merged`; delete + tombstone → `.trash`.

## Dedupe migration on a copy of `~/.theuth/documents` (brief 4)
`cp -R ~/.theuth/documents /tmp/69/docs-copy-m/` then `Store::open` on the copy (a throwaway `#[ignore]` test, run once, removed before the commit; original untouched — still 35 directories):
```
PROBE rows_total=43 live_before=35 live_after=35 merges=0 moves=35 mirror_files=66
```
0 merges (no binding had two live rows on one engine, as expected), 35 moves to `DocId::for_binding` (23 artifact, 9 knowledge, 3 meeting_note), every live row's directory present under the new id, 66 mirror files (35 meta + 31 snapshots), second `open` idempotent (35 live rows). Same figures as C2's probe. The synthetic two-rows-per-binding merge path is `open_converges_ids_and_merges_duplicates` (existing).

## Deviations
1. **Brief 3 (unknown ids gated on a replicated `docs` row, retry on the row)** — not built that way. The contract (§2, §3.2) already settles both orders without a replica hook: files-before-row makes the directory (invisible to `list`/`lookup` until the row lands, harmless), row-before-files makes `lookup` answer `None` with a warning. A retry on row arrival would need a hook in `crates/replica`'s apply path; the watcher's retry on the other file's arrival covers the only real gap. Instead the directory now comes from the meta alone (see commit 1), so no document is stuck waiting for a snapshot that will never exist.
2. **Brief 2 `MergeSnapshot{id, bytes}`** — no such message: C2's actor handles `MirrorChanged { path }` for open documents directly (`diff_from` → `apply(.., Principal::File)`); adding a second message would be a second convention. `actor.rs` is outside my allow-list anyway.
3. **Brief 2 "title/agent style by the index row's `hlc`"** — the merge uses the contract's rule (§3.2: scalars from the newer `updated_ms`); the contract binds. The `docs` row remains the record of truth for title/kind/binding via replica LWW; `write_meta` after a merge writes the merged title into it.
4. **Brief 6 "quiet snapshot fires once after a burst"** — not written as a unit test: the logic is in `actor.rs` (`Tick`) and `Store` has no timer; the actor's integration tests (`tests/actor.rs`) need the helper binary and are outside my allow-list. Fires once by construction: `snapshot()` sets `log_count = 0`, the clause needs `log_count > 0`.
5. Known ceiling (not new): a peer's delete whose mirror-file removals reach an engine before the `docs` tombstone leaves the local directory in place (the row is a tombstone, `list` hides it, `sweep_trash` never sees a `.trash` dir for it). Nothing retries on the tombstone's arrival. Same class as deviation 1; would need the replica hook.
6. `cargo test -p document --lib` was run twice (allowed by the brief); `cargo check` not run separately — the test build is a superset.

## Not built
- Nothing from the brief beyond the deviations above. No `crates/replica` change was needed (`mirror_at` wiring is C2's `lib.rs:280`).

## Tests written
- `store.rs`: `mirror_files_create_then_merge_and_never_loop` (extended; see commit 1).
- Existing and unchanged: `open_converges_ids_and_merges_duplicates`, `meta_merge_is_a_join_that_keeps_local_export_state`, `mirror_names_parse_and_skip_temp_files`.

## cargo test
```
cargo test -p document --lib -- --include-ignored   (with the probe still present)
  Finished `test` profile [unoptimized + debuginfo] target(s) in 1m 15s
  test result: ok. 31 passed; 0 failed; 0 ignored
cargo test -p document --lib -- --ignored --nocapture probe_migration
  PROBE rows_total=43 live_before=35 live_after=35 merges=0 moves=35 mirror_files=66
  test result: ok. 1 passed; 0 failed
```
Probe removed afterwards; the committed tree has 30 lib tests. No TS touched.

DONE M
