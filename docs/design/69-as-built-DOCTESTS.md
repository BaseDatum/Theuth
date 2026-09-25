# 69 as-built — DOCTESTS: `crates/document/tests/actor.rs` runs for real and passes

Branch `fix/document-actor-tests-real`, worktree `theuth-doctests`, 2026-09-25.

## Result

`cargo test -p document`: **38 unit + 10 actor tests, 0 failed**, none skipped. The actor suite passes
with default test threads (3 runs) and with `--test-threads=1` (1 run) — before this branch it had
never actually run on the 69 tree (it skipped for want of a helper), and with a helper present it was
1 passed / 9 failed.

Run it for real (one command; debug builds find the helper at `packages/documents/dist/theuth-documents`):

```
just documents-helper && cargo test -p document
```

## Commits

1. `d22aa3e test(document)` — the harness and the two stale tests (`crates/document/tests/actor.rs` only).
2. `b441d61 fix(vendor)` — SQLite 3.51.1 → 3.51.3 in `vendor/libsqlite3-sys/sqlite3/` (production fix, see test 10 below). Files: `sqlite3.c`, `sqlite3.h`, `sqlite3ext.h`, `bindgen_bundled_version.rs`, `bindgen_bundled_version_ext.rs` (the bindings differ from ours only in the three version constants), `README.md` version note.
3. this report → `docs/design/69-as-built-DOCTESTS.md`.

## Step 1 — the helper

`helper::helper_path()` (crates/document/src/helper.rs:65): `$THEUTH_DOCUMENTS_HELPER`, else
`theuth-documents-<protocol_version>` beside the test exe, else — debug builds only — the dev fallback
`packages/documents/dist/theuth-documents`. So nothing needs copying beside `target/debug`: `just
documents-helper` (→ `scripts/app-parser-env.sh` `build_doc_helper` → `bun run build:helper`) produces
`packages/documents/dist/theuth-documents` (+ `theuth-documents-59.5`), `--version` prints
`theuth-documents 59.5` = `protocol_version()`. Used that, not the `/Applications/theuth.app` sidecar.

## Step 2 — harness

- **Cascade**: every test registers `DOCUMENT_ACTOR`; a panicking test's runtime drops without the
  actor's `on_stop`, so the name stayed in kameo's global `ACTOR_REGISTRY` and every later test died
  with `NameAlreadyRegistered` at `actor.register` (9 of 10 in the first real run). Fix in `start_in`:
  `kameo::registry::ACTOR_REGISTRY.lock().unwrap().remove(DOCUMENT_ACTOR)` before `register`. One line,
  no guard type; `SERIAL` (a tokio mutex, no poisoning) already serialises the tests. Not a
  parking_lot candidate: the mutex is kameo's.
- **Skip visibility**: the ten identical 4-line `let Some(bin) = helper_bin() else { eprintln!(…); return }`
  blocks became one `helper_or_skip!()` macro that prints
  `SKIPPED: no documents helper at <path> — run `just documents-helper` or set THEUTH_DOCUMENTS_HELPER`
  and returns. Cargo has no runtime `#[ignore]`; the test still counts `ok` — documented on the macro.
  Verified with `THEUTH_DOCUMENTS_HELPER=/nonexistent/helper … -- --nocapture`.

## Step 3 — per test (before → root cause → fix)

| # | test | before (helper present, `--test-threads=1`) | root cause | fix |
|---|------|------|------|-----|
| 1 | `agent_tools_edit_attribute_and_undo` | ok | — | — |
| 2 | `hub_gate_follows_the_lease` | **FAILED** `actor.rs:329 an event within 3 s: Elapsed` — the `next_event` at the `Closed{hub_moved}` assertion after `lease(Some("other"))` | Not a timeout problem and not a production bug. `DocumentActor::sync_hub` (hub.rs:165) only runs the loss path — flush, snapshot, `unload(id, HUB_MOVED)` → `Closed` to subscribers — on the `(false, Some(watcher))` arm, i.e. when the duties were *taken up* earlier. A fresh actor holds by default (`Follower::default()`, `mesh/src/lease.rs:24`) but has taken nothing up until `hub::Reconcile`; the plugin sends `Reconcile` at start (`crates/document/src/lib.rs:279`) before subscribing to `MeshEvent`, and `relay.rs`'s own hub tests do the same. The test skipped that step, so the loss hit `(false, None) => {}` and nothing was ever emitted — the event legitimately never fires. | The test now sends `document::hub::Reconcile` right after `start`, as the plugin does (69-as-built-C2/Y hub-duty contract: exports/watcher/serving are taken up by reconcile, dropped on loss). Assertions unchanged; passes in ~1.5 s. |
| 3 | `notes_create_list_rename_delete` | `NameAlreadyRegistered` (cascade) | Already aligned with the 69 stamp-at-export rule on main (`5f6115c` added the poll on `ListDocs`); passes once the cascade is gone. | — |
| 4 | `sharing_links_join_roles_demote_revoke` | cascade; then (run 2) **FAILED** at the `Update{by: Member}` assertion on `owner_sub` | The note's own creation export (`ExportDone` → `fan_out(Exported)`, `export_debounce_ms` 100 + a 50 ms tick + a helper `markdown` round trip) lands at a load-dependent point in the owner's event stream; under load (avg 30–40) it arrived before the member's `Update`. The test asserted strict order on that stream without syncing on the export, unlike the other tests (`until_exported`). Flaky, not a production bug. | `until_exported(&mut owner_sub)` right after the owner subscribes; the failing assert now prints the offending event (`{ev:?}`). |
| 5 | `member_text_is_labelled_for_the_agent` | cascade | — | — |
| 6 | `limits_doc_size_update_size_awareness_cap` | cascade | — | — |
| 7 | `idle_unload_compacts_the_log_and_reloads_identically` | cascade | — | — |
| 8 | `sharing_off_and_disabled` | cascade; then (run 3, last test) **FAILED** at `Vault::open … pool timed out while waiting for an open connection` | see 10 | see 10 |
| 9 | `owner_hears_an_export_after_the_quiet_period_or_when_the_editor_leaves` | cascade | — | — |
| 10 | *all* (default threads): 1–4 pass, then every remaining test fails after exactly 30 s in `start_in` at `secrets::Vault::open` → `store::open` → `SqlitePoolOptions::connect_with`: **`pool timed out while waiting for an open connection`** (sqlx `acquire_timeout` 30 s). Reproduced 4/4 with default threads, 1/3 single-threaded. | **SQLite 3.51.1 deadlock — production bug in the vendored amalgamation.** `sample` of the hung process (`/tmp/69/actor-sample.txt`): no test thread holds anything; sqlx worker 13 is in `unixClose` holding the VFS mutex (`unixEnterMutex`) and waiting on `pInode->pLockMutex`; worker 12 is in `sqlite3WalClose → sqlite3OsLock → unixLock` holding `pInode->pLockMutex` and waiting on the VFS mutex inside `unixIsSharingShmNode` (3.51.1 `sqlite3.c:43874` — new in 3.51.0's "broken posix lock" detection, called from `unixLock` at 41232 *under* the inode mutex). Lock-order inversion between two connections of one WAL file closing on two threads — exactly what sqlx's pool teardown does (4 connections, one worker thread each) when the previous test's runtime drops. Everyone else then queues on the VFS mutex forever: workers 14/16/17 in `unixClose`/`unixOpen → findReusableFd`, so **every later `sqlite3_open_v2` in the process hangs** and the next test's pool times out. Upstream: fixed in SQLite 3.51.2 ("Fix an obscure deadlock in the new broken-posix-lock detection logic"); mattn/go-sqlite3#1369 is the same trace. In 3.51.3 `unixIsSharingShmNode` no longer takes the VFS mutex. | Replaced `vendor/libsqlite3-sys/sqlite3/{sqlite3.c,sqlite3.h,sqlite3ext.h,bindgen_bundled_version*.rs}` with rusqlite v0.39.0's (SQLite 3.51.3; sqlite.org returns 403 to non-browser fetches, rusqlite's vendored copy is byte-identical to the amalgamation). Bindings diff vs ours: only `SQLITE_VERSION`, `SQLITE_VERSION_NUMBER`, `SQLITE_SOURCE_ID`, `SQLITE_SCM_DATETIME`. Blast radius: every workspace crate built with the plain `bundled` feature (sqlx → libsqlite3-sys). Builds that pull `bundled-sqlcipher` (the `t` binary via `replica`/presage) compile `vendor/libsqlite3-sys/sqlcipher/sqlite3.c` = SQLCipher on SQLite **3.50.4**, which predates the buggy code — so the desktop was not exposed, test binaries and any bundled-only build were. `[INFERENCE]` on which binaries link which: from `build.rs`'s feature branches and `crates/replica/Cargo.toml`; not verified by inspecting `t`. |

After both fixes: 10/10 default threads ×3, 10/10 `--test-threads=1`, whole crate 38 + 10, no skips.

## Step 4 — whole crate

```
cargo test -p document
  unittests src/lib.rs … 38 passed; 0 failed
  tests/actor.rs        … 10 passed; 0 failed; finished in 16.57s
  doc-tests             …  0 passed
```

## cargo check

`cargo check -p document --tests` → `Finished dev profile … in 44.18s`, no warnings from this crate.

## Deviations

- The brief's header forbids `cargo test`; its steps 3–4 require it. Followed the steps (the task *is* making the suite run). Ran `cargo test -p document --test actor` several times to isolate two flaky failures and `sample` once on the hung process; `cargo check` once.
- Header says report at `/tmp/67/…` and `docs/design/67-as-built-…`; the steps say `/tmp/69/report-DOCTESTS.md`. Wrote `/tmp/69/report-DOCTESTS.md` and `docs/design/69-as-built-DOCTESTS.md` (this is 69 work; the 69-as-built-* series is where C2/M/Y/E2/Z live).
- Commits: one for the whole actor.rs (harness + two test fixes are one file, hunks interleaved) and one for the SQLite upgrade, rather than one per test. The report separates them.
- Touched outside `crates/document/tests/actor.rs`: `vendor/libsqlite3-sys/sqlite3/*` (five files replaced) and `vendor/libsqlite3-sys/README.md` (version note) — disclosed above; it is the root cause of test 10.
- `Reconcile` is sent only by the hub gate test, not by `start_in` for all tests: reconcile also starts the `knowledge/` file watcher (`watch::start`), and `knowledge_document_round_trip_export_import_restart` writes `knowledge/facts/a.md` and imports it explicitly — a watcher would race that import. The other nine tests exercise the actor as before (holding by default, no watcher).
- No production code in `crates/document` changed: the hub gate's `(false, None)` no-op is correct given the plugin always reconciles at start; the sharing/create export timing is by design (69 stamp-at-export).
- 3.51.3, not 3.53.4 (current): same minor as before, exactly the fix, bindings provably identical modulo version constants. Moving to 3.53 is a separate, larger upgrade (Upgrade.md checklist).

## Not built

- No `#[ignore]`-style skip outcome: Rust's test harness has no runtime ignore; the loud `SKIPPED:` line is the ceiling.
- No drop-guard type for the registry: the pre-register `remove` covers the cascade with one line.

## Tests written

None new; the suite itself is the deliverable. Regression evidence: 1 passed / 9 failed → 10 / 10 in both thread modes, whole crate green, skip path exercised by hand.

DONE DOCTESTS
