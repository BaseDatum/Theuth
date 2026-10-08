# Study: Turso as theuth's embedded database (vs C SQLite)

- **Date:** 2026-10-08 (spike 12:55–13:48, Sky's M3 Max under load 45–185)
- **Question (Sky):** "can you do a spike on what we would gain by replacing sqlite with turso in ~/Develop/theuth/" — https://github.com/tursodatabase/turso (the Rust rewrite of SQLite, formerly limbo; not libSQL).
- **Status:** answered — **keep SQLite**; revisit under the conditions in §9. This file is the living record; append to §10 as Turso moves.
- **Evidence:** appendices under `docs/studies/2026-10-08-turso-vs-sqlite/` (raw child reports); scratch tree `~/Develop/spike-turso/` (clone at 8cc059bd, prebuilt `tursodb` 0.8.2, the probe crate `build/embed`, the SQL corpus `compat/corpus/queries.jsonl`, per-statement results, file copies — not in git). Knowledge: `decisions/theuth-keep-sqlite-over-turso-2026-10-08`, `facts/turso-0-8-2-recon-and-measured-behaviour-2026-10-08`, `facts/theuth-sqlite-usage-inventory-2026-10-08`, `procedures/theuth-db-engine-swap-spike-four-children-2026-10-08`.
- **Versions measured:** Turso v0.8.2 (released 2026-10-06; prebuilt CLI) and clone HEAD 8cc059bd (2026-10-07, built in-process); controls `/usr/bin/sqlite3` 3.51.0 and `rusqlite` 0.38 bundled 3.51.1; theuth main 9468a54f, which links SQLCipher 4.10.0 / SQLite 3.50.4 in `t`.

## 1. Verdict

**Nothing theuth needs today, at a price it cannot pay today.** Turso opens theuth's real files in place — the 1 GB `sessions.sqlite`, with C-SQLite WAL frames still uncheckpointed — and runs **498 of the 501 non-FTS statements** in our SQL corpus unchanged. But:

1. **No FTS5.** Six FTS5 tables in five crates fail to create (knowledge `concepts` *is* the concept store; codeindex `symbols_fts` + `symbols_tri` trigram; meeting `segments` + `briefs_fts`; agent `endless_lines_fts`). Worse, when Turso opens an existing file that contains an FTS5 table it stops loading the schema there, so every table created later is invisible (codeindex's `imports`, `refs`, `cochanges`).
2. **No driver, no session extension.** theuth has 712 runtime sqlx 0.9 sites; the only sqlx adapters for Turso are one-person alphas. Turso's C layer has no `sqlite3session_*`/changeset API and no update/preupdate/commit hooks, which design 68's session log is built on.
3. **One process per file.** A Turso process takes an exclusive file lock: `t --secret-*`, `t dreaming dry-run`, the double journal and the codeindex's second process would be locked out, and the `sqlite3` CLI cannot even read a Turso-held file. The experimental `.tshm` multi-process WAL is Turso-to-Turso only; one C-SQLite write into it made the Turso holder fail every statement and panic.
4. **C SQLite stays in the binary anyway** — presage-store-sqlite (Signal), microsandbox → sea-orm, unjar pin it — so a swap adds a second engine (+26 MiB, +4½ min cold / +1 m 51 s per `turso_core` change at -j4) instead of removing the SQLCipher-on-BoringSSL build.

The genuine gains — 2.7× faster prepared point reads, MVCC `BEGIN CONCURRENT`, an in-file CDC table, built-in AEAD encryption — address no pain theuth has measured: 0 lock errors in seven days of logs, and the slow statements are query-shape problems (§4).

## 2. Method

Four children on fixed axes plus orchestrator hand checks, from one session (procedure recorded as `procedures/theuth-db-engine-swap-spike-four-children-2026-10-08`):

| child | kind | axis | output |
|---|---|---|---|
| turso-recon | read-only | source recon of the clone: embeddability, SQL/file/C-API compatibility, CDC, sync, encryption, views, FTS, extensions, ecosystem, maturity — every claim `path:line` | §3, §7, Appendix A |
| theuth-sqlite-inventory | read-only | dependency walk of Cargo.lock, every store and how it is opened, dialect surface, sqlx pattern, replication layer, pain from logs, tests → requirements R1–R17 | §4, Appendix B |
| turso-build | shell + cargo | one scratch crate (`turso` path dep + `rusqlite` bundled) built cold/warm under build-lock; bench, interop, concurrency, CDC, encryption probes in one binary; dependency closure | §6, §7, Appendix C |
| turso-compat | shell + python | theuth's 748 SQL sites and 75 migration files extracted and replayed on the prebuilt `tursodb` 0.8.2 vs `sqlite3`; `.backup` copies of every real store incl. the 1 GB sessions file; a decisive uncheckpointed-WAL test | §5, Appendix D |

The orchestrator re-ran the decisive edges on the same CLI: the FTS5 schema-stop reproduction, and `WITH RECURSIVE`, `ROW_NUMBER() OVER (PARTITION BY …)`, `UPDATE … FROM`, upsert + `RETURNING` (all pass on 0.8.2 — Turso's `COMPAT.md` still lists the first two as unsupported, and `docs/manual.md` still says "no triggers/views/savepoints"; only the CLI run is evidence). All builds went through `~/.theuth/bin/build-lock` at `CARGO_BUILD_JOBS=4` under `nice`; the machine's load average was 45–185, so timings are indicative.

## 3. What Turso is (recon of 0.8.2 / 8cc059bd)

- **An in-process SQL engine in Rust that reads and writes the SQLite file format**, with its own VDBE and a growing set of opt-in extensions: MVCC journal mode with `BEGIN CONCURRENT`, per-connection CDC into a table, AEAD page encryption, DBSP incremental materialized views, tantivy full-text search, vector functions, an experimental Postgres frontend, and a client/server sync engine. MIT; 24.7k stars; 21 stable + 121 pre-releases in the last 12 months; README: "runs in production today at multiple organizations … not yet reached 1.0 … keep independent backups". 978 open issues (compatibility 158, correctness 155, mvcc 47, panic 30, corruption? 17). Top three committers = 69 % of the last 300 commits (one of them an AI agent account). The README says Turso "replaces libSQL as our intended direction".
- **Embedding:** crate `turso` (`Builder::new_local(path)…build().await`, `Database::connect()`, `Connection::{query, execute, execute_batch, prepare, prepare_cached, busy_timeout}`; `Value = Null|Integer|Real|Text|Blob`; transactions are SQL only). Experimental features are Builder flags (`experimental_encryption/attach/materialized_views/vacuum/multiprocess_wal/without_rowid/index_method`). On macOS the I/O backend is `UnixIO`: `step()` is a no-op and pread/pwrite block inline — the async API is not asynchronous I/O there (`core/io/unix.rs:81-83, 317-381`). Default features pull `mimalloc` and `fts` (tantivy); `turso_core` defaults include `encryption`, `simd` (simsimd, C) and `autovacuum`; the sync crates are compiled even with `sync` off.
- **Not pure Rust by default:** `cc` builds mimalloc, libzstd (via tantivy) and SimSIMD; bindgen + clang-sys are compiled (never executed). Default closure 238 crates (174 without defaults).
- **Compatibility stance** (`COMPAT.md`): tracks SQLite 3.50.4; "you should always be able to go back to SQLite"; "we don't support mixed SQLite and Turso in multi-process scenarios". Known gaps: FTS3/4/5 (replaced by `CREATE INDEX … USING fts` + `fts_match/fts_score/fts_highlight`, no `snippet`), `agg() FILTER`, custom collations (unknown names silently default), `UPDATE/DELETE … ORDER BY/LIMIT`, INSTEAD OF triggers, in-place `VACUUM` (experimental; `VACUUM INTO` works), ~30 PRAGMAs (optimize, mmap_size, secure_delete, …). Text must be valid UTF-8 (invalid bytes become U+FFFD).
- **C API layer** `turso_sqlite3`: 120 exported symbols; 12 are `todo!()` panics (backup ×5, serialize/deserialize, limit, sleep, trace_v2, aggregate_context, create_window_function); absent entirely: update/preupdate/commit/rollback/wal hooks, unlock_notify, all `sqlite3session_*`/`sqlite3changeset_*`, load_extension. rusqlite and sqlx-sqlite cannot link it in place of libsqlite3.
- **Ecosystem:** sqlx 0.9's `Database` trait is implementable; adapters are `avencera/sqlx-turso` 0.1.0-alpha.1 (7 stars, one author, commits 2026-05-26..28, pins `turso =0.7.0-pre.3`) and `jarod/sqlx-turso-driver` 0.0.1 (2026-10-01); `diesel_turso` and `sea-orm-turso` are early; no rusqlite shim.

## 4. What theuth needs from an engine (inventory, condensed — full R1–R17 in Appendix B)

- **Dependencies:** sqlx 0.9 (`Cargo.toml:203`) in 16 crates; vendored `libsqlite3-sys` 0.36 compiling SQLCipher 4.10.0 against BoringSSL for `t` (FTS5, JSON1, RTREE, DBSTAT, SESSION, PREUPDATE_HOOK on). Cargo.lock also carries sqlx 0.8.6 (gferon fork, for presage-store-sqlite) and three third-party trees that pin C SQLite independently of theuth: **presage-store-sqlite** (sqlx 0.8 + `libsqlite3-sys` direct), **microsandbox** → sea-orm → sqlx 0.8, **unjar** → rusqlite. C SQLite is compiled 3–4 times per profile today (SQLCipher variant, plain bundled for sqlx, the session/preupdate variant) beside a 325 MB BoringSSL cmake tree.
- **Stores:** 17 database files under `~/.theuth`, all plain `SQLite format 3` in WAL mode with `busy_timeout` 5 s and pools of 4; `sessions.sqlite` 1.01 GB (108 MB on 2026-09-24; no checkpoint/vacuum policy), replica 83 MB, codeindex 49 MB, knowledge 14 MB. The agent store runs four readers plus one write connection that carries a `sqlite3session` on its raw handle and begins every write with `BEGIN IMMEDIATE`. **Second processes open some files today:** `secrets.sqlite` (`t --secret-*`, `t sandbox`, VM first run), the codeindex files, and `sessions.sqlite` read by `t dreaming dry-run` and the double journal.
- **Dialect:** upsert with `excluded.` (54 lines in 14 crates), `RETURNING` 18, `WITH RECURSIVE` 9, `AS MATERIALIZED`, `ROW_NUMBER() OVER (PARTITION BY …)`, `UPDATE … FROM`, json_extract/json_each, IIF, TEMP tables, runtime-generated triggers (replica), FK cascades, partial and unique indexes, `pragma_table_info()`, and **FTS5 with the trigram tokenizer, `MATCH`, `bm25()`, `snippet()`, `ORDER BY rank`**. 712 runtime sqlx sites (0 compile-time macros, no `.sqlx/`), 75 migration files, `sqlx::migrate!` in 10 crates.
- **Replication:** stage 1 = AFTER triggers → `_replica_log` → HLC batches (per-column LWW); stage 2 (design 68) = session extension on the one write connection → per-commit changeset per session into `session_log`, applied with `sqlite3changeset_apply` (26 FFI calls in `agent/src/store/capture.rs` + `sessionsync/src/apply.rs`).
- **Pain, measured:** logs 10-02..10-08 contain **0** `database is locked` / `SQLITE_BUSY` / `pool timed out`; `slow statement` (>1 s) counts per day 3, 10, 10, 0, 26, 40, **275** — on 10-08 247× the `ROW_NUMBER() OVER` query in `crates/agent/src/store/durability.rs:451` and 26× codeindex `DELETE FROM files WHERE id = ?` cascades (≤ 4.2 s).
- **Defect found on the way:** `~/.theuth/signal/presage.db` is opened without a key (`crates/signal/src/presage.rs:351` → presage `open_with_passphrase(url, None, …)`); its header is plaintext. SQLCipher-on-BoringSSL is built for exactly this store and is not applied — contrary to the 2026-09-22 ruling.

## 5. theuth's SQL and files on Turso 0.8.2 (compat replay)

| measure | result |
|---|---|
| corpus | 748 sites (625 static → 626 statements; 123 dynamic not replayed), 75 migration files / 196 statements, extracted with a Rust string lexer |
| migrations | **193 / 196 apply**; the 3 failures are `CREATE VIRTUAL TABLE … USING fts5` (agent 0037, meeting 0002, meeting/briefs 0001). Replayed file-by-file exactly as sqlx does (`BEGIN; file; INSERT _sqlx_migrations; COMMIT`): agent 39/40 files, meeting 1/2 — those stores would not open. `_sqlx_migrations` bookkeeping works everywhere |
| queries | 626 statements with placeholders → NULL: **498 pass**, 85 fail only from the NULL stand-ins (identically on sqlite3), 20 control failures, **23 FTS5** (3 `no such module: fts5` + 20 `no such table` on FTS tables). Nothing else failed — upsert, RETURNING, WITH RECURSIVE, MATERIALIZED, window, UPDATE…FROM, json, IIF, CAST, LIKE…ESCAPE, pragma_table_info, TEMP tables, triggers with RAISE |
| one semantic difference | `LIMIT NULL` inside `IN (…)`: sqlite3 returns no rows, Turso `datatype mismatch (19)` (irrelevant once an integer is bound) |
| schema text | `sqlite_master.sql` is re-rendered by Turso (one line, comments dropped, inline `REFERENCES` moved to table-level `FOREIGN KEY`, `NULL` dropped); AUTOINCREMENT adds a visible `__turso_internal_seq___turso_internal_autoincrement_<table>` table |
| real files | tasks, roster, replica, artifacts, schedule, **sessions (1,012,084,736 B)**: opened, every table count equal, `integrity_check ok` (sessions: Turso 1.75 s vs sqlite3 3.87 s). knowledge.sqlite and the codeindex file: opened, but only the tables created before the first FTS5 object are visible, and `integrity_check` falsely reports "Page N: never used" for the FTS pages |
| WAL | a C-SQLite WAL with 1,038 uncheckpointed frames (3,000 test rows, `wal_autocheckpoint=0`, process `_exit`ed) is read correctly by Turso in both `--readonly` and rw mode; rw mode checkpoints and truncates it on close; sqlite3 then reads the same counts. Reverse direction (sqlite3 reading Turso-written DB + WAL): `integrity_check ok` |
| timings on the 1 GB file | 4 of 5 realistic queries within ±15 %; `LIKE '%…%'` over 191 k message bodies **3.31 s vs 0.94 s** (3.5× slower) |

## 6. Build and embedding cost (scratch crate, release, -j4, load 37–59)

| measure | Turso (clone HEAD) | C SQLite today |
|---|---|---|
| cold compile of the probe crate | 4 m 26 s (critical path 267 s): `turso_core` 130 s, tantivy 69 s, zstd-sys 37 s, sync crates 33 s | `libsqlite3-sys` bundled build script 27 s + rusqlite 3 s (the baseline unit in the same build) |
| warm rebuild after touching `turso_core` | 1 m 51 s (`turso_core` 90 s → sync engine 15 s → sdk kits 14 s → link) | none — the C unit never re-runs on a Rust-side change |
| binary | `embed` 26.4 MiB with Turso + tantivy + rusqlite; prebuilt `tursodb` 18.4 MiB | `libsqlcipher.a` 2.7 MB (+ BoringSSL 27.5 MB of static libs already paid for Signal) |
| target dir | 1.0 GB | — |
| C toolchain | required (mimalloc, zstd, simsimd) | required (SQLCipher, BoringSSL) |

Microbench, same process, medians of three (20 k rows): insert in one transaction **0.98 µs/row vs 0.57** (Turso 1.7× slower); prepared point read **0.69 vs 1.88 µs** (Turso 2.7× faster); `count/sum` over 20 k rows 0.91 vs 0.87 ms; autocommit single-row insert **14.8 vs 15.8 µs** at `synchronous=NORMAL` — Turso's default is FULL, which costs 109 µs per commit. Turso never checkpoints on close (the `-wal` keeps its frames), writes no `-shm`, and silently converts a rollback-journal file to WAL on first write (`PRAGMA journal_mode=DELETE` returns `wal`).

## 7. Concurrency, CDC, encryption, sync (measured + source)

- **In-process:** no busy timeout by default (`BusyHandler::None`) — two writers lose ~93 % of transactions with `Busy("database is locked")`; with `Connection::busy_timeout(5 s)` 1,000 transactions of 10 rows complete in 0.199 s; `PRAGMA journal_mode='mvcc'` + `BEGIN CONCURRENT` 0.124 s, no errors, a new `<db>-log` logical log, and MVCC writes invisible to a non-MVCC reopen until checkpoint.
- **Cross-process:** default open = whole-file non-blocking exclusive fcntl lock (`core/io/unix.rs:278-299`). A second process — Turso or `sqlite3` — fails (`Failed locking file … File is locked by another process`; sqlite3 `database is locked (5)`); `--readonly` opens are allowed. `--experimental-multiprocess-wal` creates `<db>-tshm`; two Turso processes in that mode work; `sqlite3` can read and write such a file, but its checkpoint-and-truncate on close made the 0.8.2 holder fail every statement (`short read on WAL frame at offset 32`) and panic at `core/storage/wal.rs:5458` on quit. The manual marks the feature "not production ready".
- **CDC:** `PRAGMA capture_data_changes_conn('off|id|before|after|full'[, table])` is **per connection** (`core/connection.rs:422`, `core/translate/mod.rs:97-105`). In the probe only connection A's five statements (plus COMMIT markers) were captured; the second connection on the same `Database`, a second open and an external writer were not. `turso_cdc(change_id, change_time, change_txn_id, change_type 1/0/-1/2=COMMIT, table_name, id, before BLOB, after BLOB, updates BLOB)` is a normal durable table with SQLite-record-format blobs, transactional, pruned by hand, readable by C SQLite. Same connection-scoped shape as the session extension theuth already uses — durable in-file, but no `changeset_apply` to go with it.
- **Encryption:** `Builder::experimental_encryption(true).with_encryption(EncryptionOpts{cipher, hexkey})` or `PRAGMA cipher`/`hexkey`; AEGIS-256 reserves 48 B per 4 KiB page (1.2 %), AES-256-GCM 28 B; the header magic becomes `Turso\0<cipher id>\0`, so `sqlite3` reports `file is not a database (26)`; wrong key → `Decryption failed for page=1` at open; raw hex keys only (no passphrase KDF, no rekey); not SQLCipher-compatible; experimental.
- **Sync:** client `turso_sync_engine` (`push()/pull()`) speaks HTTP to a Turso-Cloud-shaped server (`POST /v2/pipeline`, `/pull-updates`, `/sync/{gen}/{start}/{end}`, `/info`, `/export/{gen}`); an in-tree server exists (`tursodb --sync-server`); hub-and-spoke, conflicts "last push wins"; not peer-to-peer — the opposite shape of theuth's iroh mesh with per-column LWW on HLCs.

## 8. What we would gain, honestly

| gain | real? | why it does not move the decision |
|---|---|---|
| 2.7× faster prepared point reads; 2× faster `integrity_check` | yes | theuth reads rows in pages and its slow statements are a window query and cascading deletes; bulk inserts and LIKE scans are slower |
| MVCC `BEGIN CONCURRENT` for concurrent writers | yes | single-writer by design; 0 lock errors in seven days |
| durable in-file CDC with before/after images and COMMIT markers | yes | per connection like the session extension; would replace 26 FFI calls with SQL but needs a rewritten apply path and still cannot see other connections' writes |
| built-in AEAD encryption | yes, experimental | SQLCipher is already linked (and, for Signal, not even applied) |
| Rust-native engine with DST + Antithesis testing, active development | yes | pre-1.0, 155 open correctness issues, panics in the C layer, "keep independent backups" |
| a Postgres frontend over the same files | experimental | theuth's SQL is SQLite dialect through sqlx |

## 9. Recommendation and revisit conditions

1. **Keep SQLite** (sqlx 0.9 + the vendored SQLCipher `libsqlite3-sys`). Nothing above addresses a measured theuth problem, and FTS5, the driver, the session extension and the lock model are each disqualifying on their own.
2. **Re-evaluate at Turso 1.0** — and only if **all** of these hold:
   - [ ] FTS5-compatible virtual-table support (or theuth's five FTS5 users — knowledge, codeindex, meeting segments + briefs, endless recall — moved to an engine-neutral index), and an existing file with FTS5 tables opens fully;
   - [ ] a maintained sqlx 0.9+ driver (not a one-person alpha), or a decision to own one (~712 sites, 0 macros, so a driver + dialect job, not a macro-metadata one);
   - [ ] multi-process WAL out of "experimental", with a lock model that tolerates read-only C-SQLite openers or every opener in theuth moved to Turso;
   - [ ] a file-level change log (or `sqlite3session`/changeset parity) that design 68's session log can ride, including an apply path with a conflict callback;
   - [ ] the three third-party pins resolved (presage-store-sqlite, microsandbox/sea-orm, unjar) or the two-engine binary accepted (+26 MiB, +4½ min cold, +1 m 51 s per `turso_core` change at -j4).
3. **Do now, Turso or not:** apply the Signal store passphrase or retire the SQLCipher build complexity (`crates/signal/src/presage.rs:351`); index or rewrite the `ROW_NUMBER() OVER` query in `durability.rs:451` (247 slow runs on 10-08) and the codeindex `DELETE … files` cascades; adopt a checkpoint/vacuum policy for `sessions.sqlite` (108 MB → 1 GB in two weeks); stop compiling C SQLite 3–4 times per profile.
4. **Cheap use without switching:** `tursodb --readonly` is a competent analysis shell for theuth's non-FTS files (its `integrity_check` on the 1 GB file is twice as fast), should anyone want one.

## Appendices (same directory, raw child reports as delivered)

- `2026-10-08-turso-vs-sqlite/A-turso-source-recon.md` — architecture, API, concurrency, file format, C API coverage, CDC, sync, encryption, views, vector, FTS, extensions, ecosystem, maturity, ten facts to verify (`turso-recon`).
- `…/B-theuth-sqlite-inventory.md` — dependency walk, every store and its open options, dialect surface with citations, sqlx pattern, replication layer, pain from logs, requirements R1–R17 (digest; the child's full report exceeded the harness's 64 KiB turn cap and survives only here) (`theuth-sqlite-inventory`).
- `…/C-build-and-embedding.md` — dependency closure, cold/warm build timings, bench table, interop, concurrency, CDC rows, encryption, baseline cost of today's engine, exact errors (`turso-build`).
- `…/D-sql-and-file-compatibility.md` — CLI behaviour, corpus statistics, migration replay per crate with schema diff, query replay gap table, real-file results, the decisive WAL test, Postgres note (`turso-compat`).

## 10. Changelog

- 2026-10-08 13:55 — study written from the spike (Turso v0.8.2 / 8cc059bd; theuth main 9468a54f).
