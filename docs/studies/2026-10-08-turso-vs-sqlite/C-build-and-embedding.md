# Turso-vs-SQLite spike — build + embedding half (REPORT)

## 1 Environment
- Date: 2026-10-08 13:08–13:43 +01:00 (35 min wall; the 75-min box was not needed), host: Sky's MacBook, 16 cores (`sysctl -n hw.ncpu` = 16), disk `/System/Volumes/Data` 926 GiB, 311 GiB free.
- Load averages at start (`uptime` 13:09): 40.56 40.06 46.12 (the brief said ~180; it had dropped). Samples every 60 s during the cold build are in `build/load.log` (quoted in §4).
- Toolchain: `cargo 1.98.1 (797e8a9bc 2026-08-05)`, `rustc 1.98.1 (48a229cea 2026-09-01)` from `~/.rustup/toolchains/1.98.1-aarch64-apple-darwin/bin` (prepended to PATH; rust-toolchain.toml ignored as instructed).
- Turso clone: `/Users/Sky.Moore/Develop/spike-turso/turso`, HEAD `8cc059bd951551ae4b9dc31c01146a24b3e87a51` (Wed Oct 7 15:46:34 2026 +0300), workspace `version = "0.8.0"`, edition 2021.
- Build protocol followed: `CARGO_BUILD_JOBS=4`, `BUILD_LABEL=spike-turso build-lock nice -n 10 cargo …`, `CARGO_TARGET_DIR=/Users/Sky.Moore/Develop/spike-turso/build/target`; `cargo tree`/`metadata` bare. Nothing was built in `~/Develop/theuth`; live `~/.theuth/*.sqlite` were only read (`.backup` / `cp`).
- Theuth's own SQLite CLI used for cross-checks: `/usr/bin/sqlite3` (macOS system build; version printed in §6).

## 2 Prebuilt CLI (T4)
- Command: `curl -sSL -o turso_cli.tar.xz https://github.com/tursodatabase/turso/releases/download/v0.8.2/turso_cli-aarch64-apple-darwin.tar.xz && tar -xJf turso_cli.tar.xz` in `build/cli/`.
- Archive: 5,472,972 bytes. Contents: `turso_cli-aarch64-apple-darwin/{tursodb, CHANGELOG.md (273,423 B), LICENSE.md, README.md}`.
- Binary: `/Users/Sky.Moore/Develop/spike-turso/build/cli/turso_cli-aarch64-apple-darwin/tursodb` — Mach-O 64-bit executable arm64, **19,285,832 bytes** (18.4 MiB).
- `tursodb --version` → `Turso 0.8.2` (exit 0). No `com.apple.quarantine` xattr was present (`xattr -l` printed nothing; curl downloads do not set it), so no `xattr -d` was needed.
- Smoke: `tursodb smoke.db 'CREATE TABLE s(a); INSERT INTO s VALUES (42); SELECT * FROM s;'` → boxed table with `42`; produced `smoke.db` (8192 B) + `smoke.db-wal` (0 B); `sqlite3 smoke.db 'SELECT * FROM s; PRAGMA journal_mode;'` → `42` / `wal` (C SQLite reads the file Turso wrote).
- `build/cli/READY` contains the absolute binary path.

## 3 Dependency closure (T1)
Commands (bare, in the clone; no lock):
- `cargo tree -p turso -e normal --prefix none | sort -u | wc -l` → **396** lines (raw; includes `Updating crates.io index`/`Downloaded …` lines and feature-variant duplicates). Unique `name vX.Y.Z` pairs parsed from that output: **238 crates** with default features (`mimalloc`, `fts`).
- Same with `--no-default-features` → **225** lines raw, **174 unique crates**. The 64 crates that exist only with default features are the `fts` closure (tantivy 0.26.2 + tantivy-{bitpacker,columnar,common,fst,query-grammar,sstable,stacker,tokenizer-api}, zstd/zstd-safe/zstd-sys, lz4_flex, rayon, memmap2, fs4, rust-stemmers, levenshtein_automata, bon, typetag, census, measure_time, …) and `mimalloc` + `libmimalloc-sys`.
- `cargo tree -p turso -e normal,build` → 603 lines, **257 unique crates** incl. build-dependencies (saved: `build/tree-normal-build.txt`).

C/C++-bearing crates in the default `normal,build` closure (from the tree; who pulls them in):
| crate | compiles C? | pulled by | feature gate |
|---|---|---|---|
| `libmimalloc-sys 0.1.43` (+ `cc 1.2.59`) | yes — mimalloc C allocator | `turso → mimalloc 0.1.47` | `turso/mimalloc` (default ON) |
| `zstd-sys 2.0.16+zstd.1.5.7` (+ `cc`) | yes — libzstd C | `turso_core → tantivy → tantivy-columnar → tantivy-sstable → zstd → zstd-safe` | `turso/fts` → `turso_sdk_kit/fts` → `turso_core/fts` (default ON) |
| `simsimd 6.5.3` (+ `cc`) | yes — SimSIMD `c/lib.c` (build.rs uses `cc::Build::try_compile("simsimd")`, falls back on failure) | `turso_core` (target cfg: not wasm, not windows-aarch64) | `turso_core/simd` (in turso_core default features, ON) |
| `aegis 0.9.8` (+ `cc` as build-dep) | **no on macOS**: `core/Cargo.toml` declares `aegis = { features = ["pure-rust"] }` under `cfg(any(target_os="android", target_os="macos"))`; build.rs checks `CARGO_FEATURE_PURE_RUST` and skips `cc`. On Linux/Windows the C AEGIS is compiled unless `turso/pure-rust-crypto` is set (forwards to `turso_sdk_kit/pure-rust-crypto` → `turso_core/pure-rust-crypto` → `aegis/pure-rust`). | `turso_core` | always (encryption is a default feature) |
| `bindgen 0.69.5` + `clang-sys 1.8.1` (runtime/libloading mode) | compiled as a build-dependency of `turso_sdk_kit` and `turso_sync_sdk_kit` (`[build-dependencies] bindgen = "0.69.5"`), but `sdk-kit/build.rs` never calls it (it only compiles a Windows `.rc` version resource and returns early off-Windows). libclang is NOT needed (clang-sys `runtime` feature → dlopen, never invoked). Pure compile-time cost only. | `turso_sdk_kit`, `turso_sync_sdk_kit` | always |
| `cc 1.2.59` | build helper | mimalloc, zstd-sys, simsimd, aegis | — |
| `libc`, `rustix 0.38.44` + `1.0.7`, `polling` | no C compiled (syscall bindings) | core, tantivy, bindgen/which | — |
| `cmake`, `ring`, `aws-lc-sys`, `openssl-sys`, `io-uring`, `lz4-sys`, `flate2`/`miniz_oxide` | **absent** from the default closure on macOS (io-uring is Linux-only behind `turso_core/io_uring`, which `turso_sdk_kit` activates — see below) | — | — |

So "pure Rust, no C toolchain" is **false by default**: a C compiler is required for mimalloc, zstd (via fts/tantivy) and simsimd. With `turso = { default-features = false }` (drops mimalloc + fts) the remaining C is `simsimd` (turso_core default `simd`), and on Linux/Windows `aegis` unless `pure-rust-crypto`. `turso_core`'s `simd` feature cannot be disabled through the `turso` crate (it depends on `turso_core` with default features), so zero-C needs a direct `turso_core` dependency with `default-features = false`.

`cargo tree -p turso -e features` (saved `build/tree-features.txt`, 2340 lines) — `turso_core` features enabled through `turso`'s default build: `default` = {`autovacuum`, `encryption`, `fs`, `json`, `percentile`, `series`, `simd`, `time`, `uuid`}; additionally `turso_sdk_kit` enables `conn_raw_api`, `experimental_win_iocp`, `io_uring` (+ `rustix/io_uring`, Linux-only in effect) and `fts`; `turso_sdk_kit/encryption` → `turso_core/encryption`. Not enabled: `checksum`, `stacker`, `pure-rust-crypto`, `serde`, `tracing_release`. **Encryption is on by default** (no feature to forward from the host crate).

`turso` crate on crates.io (`curl -H 'User-Agent: …' https://crates.io/api/v1/crates/turso`): max_version `0.8.3-pre.1`, newest `0.8.3-pre.1`. Last 5 published: `0.8.3-pre.1` 2026-10-08T08:12Z · `0.8.2` 2026-10-06T07:14Z · `0.8.2-pre.2` 2026-10-02T09:47Z · `0.8.2-pre.1` 2026-10-02T07:02Z · `0.8.1` 2026-09-29T12:02Z (then `0.8.0` 2026-09-29T04:34Z). The clone's version 0.8.0 is published (2026-09-29); the clone HEAD (2026-10-07) is between 0.8.2 and 0.8.3-pre.1. The prebuilt CLI tested in §2 is 0.8.2.

## 4 Cold / warm build numbers (T2)
Scratch crate: `build/embed/` — `[package] embed 0.1.0, edition 2021`, `[workspace]` (empty, so the clone's workspace does not capture it), deps `turso = { path = "…/turso/bindings/rust" }` (default features `mimalloc`+`fts`; `encryption` is a `turso_core` default and reaches the crate without any forwarding — there is no `encryption` feature on `turso` itself; `pure-rust-crypto` left off to measure stock defaults; on macOS aegis is pure-Rust anyway), `tokio 1 (full)`, `rusqlite 0.38 (bundled)`, `anyhow 1`. One binary, subcommands `bench | sql | concurrency | cdc | encryption` (source: `build/embed/src/main.rs`).

Cold release build (`build/build-cold.sh`, log `build/build-cold.log`): `export CARGO_BUILD_JOBS=4; CARGO_TARGET_DIR=build/target; BUILD_LABEL=spike-turso build-lock nice -n 10 cargo build --release --timings`
- START 13:14:11; `[build-lock] slot 1 acquired after 76s` (queued behind `spike-parakeet`, `spike-burn`); cargo `Finished release … in 4m 26s`; END 13:19:55 → **wall 5 m 44 s incl. 76 s lock wait; compile 4 m 26 s (266.9 s critical path)** at `-j4`, nice 10, on a machine at load 37–59.
- Load averages sampled every 60 s during the build (`build/load.log`): 13:14 `144.19 63.86 52.23` · 13:15 `75.51 58.73 51.10` · 13:16 `50.85 54.76 50.13` · 13:17 `44.76 52.59 49.65` · 13:18 `37.40 48.73 48.39` · 13:19 `58.74 52.62 49.85`.
- 394 units compiled (fresh Cargo.lock for the scratch crate, so a few versions are newer than the clone's lock: zstd-sys 2.1.0, simsimd 6.5.16, aegis 0.9.20, clang-sys 1.9.1, libmimalloc-sys 0.1.49, tokio 1.53.2, syn 3.0.6).
- Heaviest units (from `target/cargo-timings/cargo-timing-20261008T121528478Z-af5093bed92184be.html`, `UNIT_DATA` JSON, seconds of self time at -j4):

| unit | duration | start |
|---|---|---|
| **turso_core 0.8.0** (lib) | **130.3 s** | 106.2 |
| **tantivy 0.26.2** (lib; `fts`) | **69.1 s** | 88.1 |
| zstd-sys build-script run (C libzstd; via tantivy) | 36.9 s | 14.5 |
| **libsqlite3-sys 0.36.0 build-script run (C SQLite for rusqlite `bundled`) — the C-SQLite baseline unit** | **27.0 s** | 114.3 |
| bitpacking 0.9.3 | 23.9 s | 38.2 |
| regex-automata 0.4.18 | 20.8 s | 29.2 |
| turso_sync_engine 0.8.0 | 17.9 s | 141.3 |
| turso_sync_sdk_kit 0.8.0 | 14.7 s | 242.9 |
| tokio 1.53.2 | 13.4 s | 118.1 |
| turso_parser 0.8.0 | 11.8 s | 92.8 |
| tantivy-columnar 0.7.0 | 11.5 s | 71.5 |
| embed 0.1.0 (bin, incl. link) | 7.3 s | 259.6 |
| turso_sdk_kit 0.8.0 | 6.3 s | 236.6 |
| bindgen 0.69.5 (lib; never executed) | 4.5 s | 88.4 |
| libmimalloc-sys build-script run (C mimalloc) | 3.6 s | 109.3 |
| clang-sys 1.9.1 | 3.1 s | 81.2 |
| rusqlite 0.38.0 (lib) | 2.9 s | 155.4 |
| simsimd build-script run (C) | 2.6 s | 75.8 |
| turso 0.8.0 (lib) | 2.0 s | 257.6 |
| aegis 0.9.20 (lib, pure-rust) | 1.1 s | 89.0 |

  Sum of Turso's own crates (turso_core+parser+sync_engine+sync_sdk_kit+sdk_kit+turso+macros+ext) ≈ 186 s of unit time; tantivy closure ≈ 115 s + zstd-sys 37 s; the C-SQLite baseline (libsqlite3-sys bundled build script + rusqlite) ≈ 30 s.
- Target dir after the cold build: **1.0 GB** (`du -sh build/target`). Binary `target/release/embed`: **27,647,384 B** (26.4 MiB; contains Turso + tantivy + rusqlite/C-SQLite + tokio; `debug = false`, not stripped).
- Toolchain noise (not Turso's): 5× `warning: stripping debug info with rust-objcopy failed: signal: 6 (SIGABRT) … dyld: Library not loaded: @rpath/libLLVM.dylib` from the 1.98.1 toolchain's `rust-objcopy` (build scripts/proc-macros); 2 unused-code warnings in `embed`.

Warm rebuild (`build/build-warm.sh`, log `build/build-warm.log`): `touch turso/core/lib.rs` (the clone's core crate root is `core/lib.rs`, not `core/src/lib.rs`) + one 4-line change in `embed/src/main.rs`, same lock/jobs:
- START 13:24:40 → `[build-lock] slot 1 acquired after 761s` (queued behind the design-140 wave's crate gates — the semaphore has no FIFO, so the waiter lost several slot hand-offs) → cargo `Finished release … in 1m 51s` → END 13:39:17 (load 20.61 28.64 36.76 at the end). **Warm rebuild after a `turso_core` touch: 1 m 51 s compile, 111.4 s critical path** (`cargo-timing-20261008T123726182Z-af5093bed92184be.html`): `turso_core` **90.3 s** → `turso_sync_engine` 15.1 s → `turso_sdk_kit` 4.0 s → `turso_sync_sdk_kit` 10.3 s → `turso` 0.8 s → `embed` 5.0 s (link). All 388 other units were fresh (0.0 s). That chain is what a host pays per `turso_core` change (the sync crates are compiled even though the `sync` feature is off, because `turso` depends on `turso_sync_sdk_kit` unconditionally). The equivalent for today's engine is the `libsqlite3-sys` build-script unit (27 s cold here, and it never re-runs on a Rust-side change).
- Binary after the warm build: 27,647,224 B.

## 5 Bench (phase × engine, µs/op; median of 3 runs each; `build/bench.log`)
Workload (identical code shape for both engines, same process, release build, fresh file per run, `k TEXT` ~8 B, `v BLOB` 64 B, `n REAL`): 20,000 inserts in ONE transaction via a prepared statement · 20,000 prepared point reads by `id` · one `SELECT count(*), sum(n) FROM t` · 2,000 single-row autocommit inserts (each its own implicit transaction → commit/fsync cost). Ran 13:21:13–13:21:15 at load ~40 (three runs of three engines took 2 s total, so N was NOT reduced).

Settings: **Turso defaults** as reported by the engine on a fresh file: `PRAGMA journal_mode` → `wal`, `PRAGMA synchronous` → `2` (FULL; `core/database.rs:2589 sync_mode: AtomicSyncMode::new(SyncMode::Full)`), `page_size` 4096, `wal_autocheckpoint` 1000, `busy_timeout` 0, `locking_mode` → `exclusive`. "turso-normal" = defaults + `PRAGMA synchronous=NORMAL` (verified → `1`). **rusqlite** = `PRAGMA journal_mode=WAL` (→ `wal`) + `PRAGMA synchronous=NORMAL` (→ `1`), bundled SQLite **3.51.1**.

| phase | Turso (defaults: WAL, synchronous=FULL) | Turso (WAL, synchronous=NORMAL) | rusqlite 0.38 bundled 3.51.1 (WAL, NORMAL) |
|---|---|---|---|
| insert in one tx (prepared), µs/row | **1.08** (1.13 / 1.08 / 1.07) | **0.98** (1.03 / 0.98 / 0.95) | **0.57** (0.57 / 0.55 / 0.57) |
| point read by id (prepared), µs/row | **0.76** (0.79 / 0.76 / 0.76) | **0.69** (0.74 / 0.68 / 0.69) | **1.88** (1.65 / 2.08 / 1.88) |
| `SELECT count(*), sum(n)` over 20k rows, total µs | **1030** (1211 / 981 / 1030) | **911** (955 / 895 / 911) | **872** (872 / 882 / 860) |
| autocommit single-row insert, µs/row | **108.9** (114.5 / 72.5 / 108.9) | **14.8** (14.8 / 12.3 / 15.7) | **15.8** (15.6 / 16.4 / 15.8) |

File sizes after each run (22,000 rows): Turso `turso.db` **2,011,136 B + turso.db-wal 2,463,792 B** (the WAL is left in place at close — no checkpoint on close; no `-shm` file is created); rusqlite `rusqlite.db` **1,982,464 B**, no `-wal` left (C SQLite checkpoints and deletes the WAL on last close).
Notes: the autocommit column is where `synchronous` matters — Turso's default FULL fsyncs the WAL per commit (~109 µs/commit here; macOS `fsync` without `F_FULLFSYNC`), NORMAL brings it level with C SQLite. Point reads are ~2.5× faster in Turso (the rusqlite path includes `query_row` per call); bulk insert is ~1.9× slower in Turso. No benchmark beyond this was run.

## 6 Interop results (T2 `interop`; files in `build/interop/`)
(a) **Copies of real theuth stores** made with `sqlite3 ~/.theuth/tasks.sqlite ".backup build/interop/tasks-copy.sqlite"` and the same for `roster.sqlite` (1,298,432 B and 245,760 B; header bytes 18–19 = `02 02`, i.e. WAL-mode files; schema has 11/12 tables, 14/12 indexes, 12/9 triggers incl. `_replica_ai_*/_au_*/_ad_*` triggers; no WITHOUT ROWID/STRICT/generated/FTS).
- Turso `SELECT name FROM sqlite_master WHERE type='table'` on `tasks-copy.sqlite` → `_replica_log, _sqlx_migrations, activity, activity_cursor, answered, packets, sqlite_sequence, task_events, tasks, wakes, write_budget` — identical to `sqlite3`.
- `SELECT count(*)` per table (Turso vs `sqlite3`), all equal: activity 789/789 · task_events 1382/1382 · tasks 80/80 · packets 31/31 · wakes 16/16 · write_budget 86/86 · _sqlx_migrations 8/8 · activity_cursor 1/1 · answered 0/0 · _replica_log 0/0 · sqlite_sequence 1/1. (roster-copy counts via `sqlite3`: agents 5, attention_finished 20, attention_shows 90, attention_state 129, episodes 157, _sqlx_migrations 6, …)
- Turso `INSERT INTO answered(id,sender,timestamp,hlc) VALUES ('turso:1','turso-spike',1791462000,'hlc-turso')` → ok; Turso left the row in `tasks-copy.sqlite-wal` (16,512 B = header + 4 frames) and created `tasks-copy.sqlite-shm`-less? — no: a `-shm` (32,768 B) already existed from the earlier `sqlite3` reads; Turso itself writes only the `-wal`. `sqlite3 tasks-copy.sqlite "SELECT * FROM answered"` → `turso:1|turso-spike|1791462000|hlc-turso` (C SQLite read the Turso-written WAL frames and then checkpointed them: `-wal` → 0 B, main file mtime updated).
- `sqlite3` `INSERT … ('sqlite3:2','sqlite3-cli',1791462001,'hlc-c')` → count 2; Turso `SELECT * FROM answered ORDER BY timestamp` → both rows.
- `sqlite3 tasks-copy.sqlite "PRAGMA integrity_check; PRAGMA journal_mode"` on the Turso-touched file → `ok` / `wal`. Turso's own `PRAGMA integrity_check` → `ok`.

(b) **Live WAL trio** copied with `cp` (`tasks.sqlite`, `-wal` 37,112 B = 9 frames, `-shm`; `roster.sqlite`, `-wal` 49,472 B = 12 frames, `-shm`; WAL magic `377f0682`, both at 13:22):
- `sqlite3 -readonly` on the trio: `activity` max(id)=792 count=792; `attention_state` 131. A copy of the MAIN file alone (no `-wal`): `activity` 789/789; `attention_state` 129 → rows 790–792 and two attention_state rows live only in the WAL.
- **Turso on the trio: `activity` 792/792, `attention_state` 131** (and task_events 1382, episodes 157, attention_shows 90, agents 5 — all equal to `sqlite3`) → **Turso reads C-SQLite WAL frames** (it recovers from the `-wal`; a `-shm` written by C SQLite is present but Turso does not need or write one).

(c) **Journal mode / file layout**:
- Fresh Turso DB (`embed sql fresh.db "CREATE TABLE f(a)" …`): `PRAGMA journal_mode` → `wal`; files after close: `fresh.db` 4,096 B + `fresh.db-wal` 12,392 B, **no `-shm`** (Turso keeps the WAL index in process memory; a `-shm` only appears when C SQLite opens the file). Header bytes 18–19 written as `02 02`.
- `PRAGMA journal_mode=DELETE` in Turso → returns `wal`; `PRAGMA journal_mode=WAL` → `wal`. Rollback-journal mode is not available.
- Rollback-journal DB made by `sqlite3` (`PRAGMA journal_mode=DELETE`, header `01 01`): Turso opens it, reports `journal_mode` = `wal`, reads the row, INSERTs → afterwards the header is **rewritten to `02 02`** and a `legacy.db-wal` (4,152 B) exists; `sqlite3 legacy.db "PRAGMA journal_mode; SELECT * FROM l; PRAGMA integrity_check"` → `wal` / `7`,`8` / `ok`. I.e. Turso silently converts legacy-journal files to WAL mode on first write.
- Turso on `tasks-copy.sqlite` → `wal`. With `PRAGMA journal_mode='mvcc'` Turso creates `<db>-log` (its logical log; §7) and leaves `-wal` at 0 B. With `experimental_multiprocess_wal` it creates `<db>-tshm` (86,016 B) — its own shared-memory file, distinct from SQLite's `-shm` (§7).
- Turso never checkpoints on close in any run here (`-wal` left with frames after every `embed` run; `sqlite3` subsequently reads them fine).

## 7 Concurrency results (T2 `concurrency`; `build/conc.log`, files `build/conc/`)
Each writer: 500 transactions × 10 rows, `BEGIN … COMMIT` (or `BEGIN CONCURRENT` in MVCC), no sleep, failed tx → `ROLLBACK` and counted.
Busy handling in the source: `Connection::busy_timeout(Duration)` in `bindings/rust/src/connection.rs:501` → `core/connection.rs:4791 set_busy_timeout` (`Duration::ZERO` unsets); default `busy_handler: RwLock::new(BusyHandler::None)` (`core/database.rs:2592`); `PRAGMA busy_timeout` exists (`core/pragma.rs:150`, reports `0` on a fresh connection); backoff schedule in `core/busy.rs:63-74` = 1,2,5,10,15,20,25,25,25,50,50,100 ms steps.

| case | busy_timeout | result | elapsed | errors |
|---|---|---|---|---|
| in-process, one `Database`, two `Connection`s on two tokio tasks | none | A ok=42 fail=458, B ok=29 fail=471 → 710 rows | 0.016 s | `Busy("database is locked")` (Rust `turso::Error::Busy`, Display `database is locked`) |
| same | `busy_timeout(5 s)` | A 500/0, B 500/0 → 10,000 rows | **0.199 s** | none |
| two independent `Builder::new_local` opens of the same path, same process | none | A ok=28 fail=472, B ok=53 fail=447 → 810 rows | 0.016 s | `Busy("database is locked")` |
| same | 5 s | 500/0 + 500/0 → 10,000 rows | 0.185 s | none |
| MVCC: `PRAGMA journal_mode='mvcc'` (reports `mvcc`), `BEGIN CONCURRENT` | none | 500/0 + 500/0 → 10,000 rows | **0.124 s** | none |
| MVCC | 5 s | 500/0 + 500/0 | 0.142 s | none |
| **cross-process**: two `embed` processes on one file (`cmd & cmd; wait`) | none / 5 s | **second process fails at open**: `Error: Locking error: Failed locking file '…/proc.db'. File is locked by another process`; P1 500/0 alone (0.068 s / 0.057 s) | — | open-time lock error (not Busy) |

File layout: default mode → `shared-busy.db` 135,168 B + `shared-busy.db-wal` 412,032 B (no `-shm`). MVCC → `mvcc.db` 8,192 B + **`mvcc.db-log` 207,563 B** (new logical-log file) + `mvcc.db-wal` 0 B. Cross-process P1 alone → `proc.db` 4,096 B + `proc.db-wal` 2,278,392 B (10 × 500 commits, never checkpointed).

Cross-process, probed further with the prebuilt `tursodb` 0.8.2 holding `hold.db` open in a PTY:
- holder default mode → `embed sql hold.db` (clone HEAD): `Error: Locking error: Failed locking file 'hold.db'. File is locked by another process`; second `tursodb`: same; `tursodb --experimental-multiprocess-wal`: `Error: Locking error: Failed opening database 'hold.db'. Database is already open without experimental multiprocess WAL in another process`; **`tursodb --readonly` works** (reads 1 row); **`sqlite3 hold.db 'SELECT * FROM h'` → `Error: in prepare, database is locked (5)`** (Turso takes a whole-file non-blocking exclusive `fcntl` lock at open — `core/io/unix.rs:278-299 lock_file(): fs::fcntl_lock(fd, FlockOperation::NonBlockingLockExclusive)`, `ErrorKind::WouldBlock` → `LimboError::LockingError("Failed locking file '…'. File is locked by another process")` — which conflicts with C SQLite's byte-range locks, so the C CLI cannot even read; the mode checks live in `core/database.rs:955-1005 reject_live_multiprocess_wal_for_legacy_open / reject_live_legacy_wal_for_multiprocess_open`).
- holder `tursodb --experimental-multiprocess-wal` → creates `hold.db-tshm` (86,016 B); a second `tursodb --experimental-multiprocess-wal` can `INSERT` and `SELECT` (sees 1,2); `tursodb` without the flag: `Database is already open with experimental multiprocess WAL in another process`; **`sqlite3` CLI can read (1,2) and write (`INSERT … 3`)** — it creates its own `-shm`; `embed sql hold.db` (clone HEAD, default open) then read 1,2,3 without error.
- **Surprise**: after the `sqlite3` write (C SQLite checkpointed and truncated `hold.db-wal` to 0 B on its close), the multiprocess-WAL holder (`tursodb` 0.8.2) failed every statement: `Runtime error: I/O error: short read on WAL frame at offset 32: expected 4096 bytes, got 0` (+ `ERROR turso_core::statement: 1554: Error while draining pending IO during statement reset` / `1521: Statement reset failed during best-effort cleanup`), and on `.quit` panicked: `thread 'main' panicked at core/storage/wal.rs:5458:17: read bytes does not match expected buffer length | read=0, expected=4096, frame_id=1`. The file itself stayed consistent (`sqlite3 … PRAGMA integrity_check` → `ok`, 3 rows). The Turso writes made before the sqlite3 write were visible to sqlite3; the holder's `INSERT INTO h VALUES (4)` after it was lost (error, not committed).
- The `embed` cross-process run with `experimental_multiprocess_wal(true)` on both processes (clone HEAD) is reported below once the warm rebuild delivers the flag: **done (clone HEAD, `TURSO_MP=1` → `Builder::experimental_multiprocess_wal(true)` in both processes, `cmd & cmd; wait`)**: both processes open (`mp.db-tshm` 86,016 B created). Without busy timeout: P1 ok=221 fail=279, P2 ok=117 fail=383, every failure `Busy("database is locked")`, 3,380 rows, 0.062 s / 0.036 s. With `busy_timeout(5 s)`: **P1 500/0 (0.242 s), P2 500/0 (0.110 s), 10,000 rows**; `sqlite3 mpb.db "PRAGMA integrity_check; SELECT count(*) FROM c"` → `ok` / `10000`. Files: `mpb.db` 143,360 B + `mpb.db-tshm` 86,016 B + `mpb.db-wal` 436,752 B (no `-shm`). So two Turso processes can share a file only if BOTH opt into the experimental multiprocess WAL; the C-SQLite-writer interaction in that mode is the §11.2 failure.

## 8 CDC results (T2 `cdc`; `build/cdc/`)
Source facts: pragma names `capture_data_changes_conn` (stable) and `unstable_capture_data_changes_conn` (deprecated alias) — `sqlite/parser/src/ast.rs:1990-1992`, `core/pragma.rs:162`; value `'<mode>'` or `'<mode>,<table>'`, modes `off|id|before|after|full` (`core/cdc.rs:71-85`), default table `turso_cdc` (`core/cdc.rs:10`), version table `turso_cdc_version` (`:11`), current CDC version V2 = 9 columns incl. `change_txn_id` and COMMIT records with `change_type=2` (`core/cdc.rs:26-35`). The pragma is **per connection** (`Connection::get_capture_data_changes_info`, `core/translate/pragma.rs:1554`).

Run: `embed cdc build/cdc/cdc.db capture_data_changes_conn full` — connection A (CDC on) inserts 3 rows, updates 1, deletes 1; B (same `Database`, CDC off) inserts 3; C (second independent open of the path) inserts 3; the `sqlite3` CLI is invoked while the process holds the file.
- `PRAGMA capture_data_changes_conn('full')` on A → ok; `PRAGMA capture_data_changes_conn` on A → `full|turso_cdc|v2`; on B → `off|NULL|NULL`.
- `sqlite3 cdc.db "INSERT … ('D',…)"` while held: **exit status 5, stderr `Error: in prepare, database is locked (5)`** → the C-SQLite writes never happened (Turso's exclusive lock, §7).
- Rows as seen by A afterwards: `A|2`, `B|3`, `C|3`.
- `SELECT type,name,sql FROM sqlite_master WHERE name LIKE '%cdc%'`:
  - `table|turso_cdc|CREATE TABLE turso_cdc (change_id INTEGER PRIMARY KEY AUTOINCREMENT, change_time INTEGER, change_txn_id INTEGER, change_type INTEGER, table_name TEXT, id, "before" BLOB, "after" BLOB, updates BLOB)`
  - `table|__turso_internal_seq___turso_internal_autoincrement_turso_cdc|CREATE TABLE "__turso_internal_seq___turso_internal_autoincrement_turso_cdc"(value INTEGER PRIMARY KEY,is_called INTEGER,start INTEGER,inc INTEGER,min INTEGER,max INTEGER,cycle INTEGER)`
  - `table|turso_cdc_version|CREATE TABLE turso_cdc_version (table_name TEXT PRIMARY KEY, version TEXT NOT NULL)` (+ its autoindex)
- Full dump of `turso_cdc` (10 rows; `change_time` is unix seconds; `change_type` 1=insert, 0=update, -1=delete, 2=COMMIT marker):
```
1|1791462208|1|1|items|1|NULL|x'04090f0841'|NULL
2|1791462208|1|2|NULL|NULL|NULL|NULL|NULL
3|1791462208|3|1|items|2|NULL|x'04010f090241'|NULL
4|1791462208|3|2|NULL|NULL|NULL|NULL|NULL
5|1791462208|5|1|items|3|NULL|x'04010f01034102'|NULL
6|1791462208|5|2|NULL|NULL|NULL|NULL|NULL
7|1791462208|7|0|items|2|x'04010f090241'|x'04010f01024165'|x'0708080900000165'
8|1791462208|7|2|NULL|NULL|NULL|NULL|NULL
9|1791462208|9|-1|items|3|x'04010f01034102'|NULL|NULL
10|1791462208|9|2|NULL|NULL|NULL|NULL|NULL
```
  `before`/`after` are SQLite-record-format blobs of the row (`04 09 0f 08 41` = header len 4, serial types int-1 / text-len-1('A') / int-0, payload `41`='A'); `updates` for the UPDATE is a record of changed-column flags + values. One full row: `7|1791462208|7|0|items|2|x'04010f090241'|x'04010f01024165'|x'0708080900000165'` = UPDATE of items.id=2 ('A',1) → ('A',101).
- **Captured: only connection A's own 5 statements (+5 COMMIT markers). NOT captured: B (same Database, CDC off), C (separate open), and the external C-SQLite writer (which could not write at all).** CDC is a per-connection trigger-like capture, not a file-level change log.
- Files after the run: `cdc.db` 4,096 B + `cdc.db-wal` 98,912 B (no `-shm`). After closing everything, `sqlite3 cdc.db`: tables `items, sqlite_sequence, turso_cdc, __turso_internal_seq___turso_internal_autoincrement_turso_cdc, turso_cdc_version` + index; `SELECT count(*) FROM turso_cdc` → **10**; `PRAGMA integrity_check` → `ok`; `sqlite3` then checkpointed (`cdc.db` 28,672 B, `-wal` 0 B, `-shm` created). → **`turso_cdc` is an ordinary table in the main file, fully visible to C SQLite.**

## 9 Encryption results (T2 `encryption`; `build/enc/`)
Source facts: `turso_core::EncryptionOpts { cipher: String, hexkey: String }` (`core/database.rs:185`), re-exported as `turso_sdk_kit::rsapi::EncryptionOpts`; `Builder::experimental_encryption(true).with_encryption(opts)` (`bindings/rust/src/lib.rs:208-216`; `build()` passes `experimental_features: "encryption"`); PRAGMAs `cipher` and `hexkey` (`core/pragma.rs:175-182`, parser `ast.rs:1944-1946`). Ciphers (`core/storage/encryption.rs:413-423`, names accepted case-insensitively with `-`/`_` variants): `aes128gcm` (16-byte key), `aes256gcm` (32), `aegis256` (32), `aegis128l`, `aegis128x2`, `aegis128x4`, `aegis256x2`, `aegis256x4`. Per-page overhead = `CipherMode::metadata_size()` reserved at the end of every page (`required_reserved_bytes`, `encryption.rs:626-628`): AES-GCM TAG 16 + NONCE 12; AEGIS-256 nonce 32 + tag 16. Crypto crates: `aes-gcm 0.10`, `aes 0.8`, `aegis 0.9` — on macOS/Android `aegis` is built with `features = ["pure-rust"]` by target cfg (`core/Cargo.toml`), so **`pure-rust-crypto` is NOT needed on macOS**; it is the opt-in for Linux/Windows to avoid the C AEGIS build. The `encryption` feature is in `turso_core`'s defaults (no host-side feature needed).

Run: `embed encryption build/enc/enc.db aegis256 <32-byte hex key> <wrong key>`:
- Create with key → `PRAGMA cipher` → `aegis256`; `page_size` 4096; `journal_mode` `wal`; `PRAGMA wal_checkpoint(TRUNCATE)` → `0|0|0`; 201 rows containing `THEUTH_PLAINTEXT_MARKER_7781`. Files: `enc.db` 20,480 B, `enc.db-wal` 0 B.
- `grep -c THEUTH_PLAINTEXT_MARKER_7781 enc.db` → **0**; on `enc.db-wal` → 0 (empty after the TRUNCATE checkpoint). `strings -n 8 enc.db` → random bytes only.
- Header: `xxd -l 32 enc.db` → `5475 7273 6f00 0300 …` = **magic `Turso\0\x03\0`** replaces `SQLite format 3\0`; bytes 16–17 `1000` (page size 4096), 18–19 `02 02`, **byte 20 = `0x30` = 48 reserved bytes per page** (AEGIS-256: 32 nonce + 16 tag = 1.17 % of a 4 KiB page); bytes 100–107 (page-1 body) ciphertext.
- `sqlite3 enc.db "SELECT 1"` → **`Error: in prepare, file is not a database (26)`** (same for `SELECT count(*) FROM secret`).
- Reopen with the right key → `1|THEUTH_PLAINTEXT_MARKER_7781`, 201 rows.
- Reopen with the wrong key → **`open ERROR: Decryption failed for page=1`** (fails in `Builder::build()`, before any query).
- Reopen with no key (plain `Builder`) → `open ERROR: file is not a database`; a plain open followed by `PRAGMA cipher`/`PRAGMA hexkey` is impossible because the open itself fails (the PRAGMA route exists for the CLI/`:memory:` flows, not for an already-encrypted file opened without `with_encryption`).
- AES-256-GCM run (`embed encryption enc-aes.db aes256gcm`): header `Turso\0\x02\0` (the byte after the magic is the cipher id: 02 = aes256gcm, 03 = aegis256), **byte 20 = `0x1c` = 28 reserved bytes** (12 nonce + 16 tag); marker grep 0; `sqlite3` → `file is not a database (26)`; wrong key → `Decryption failed for page=1`.
- Key size is enforced: `aes128gcm` with a 32-byte hexkey → `Invalid key size for Aes128Gcm: expected 16 bytes, got 32` at `Builder::build()`.
- Not measured: key rotation / `PRAGMA rekey` (none found in `core/pragma.rs`), and performance overhead.

## 10 Baseline cost of the current engine (T3, read-only)
- No `cargo-timings` exist for theuth: `ls theuth*/target/cargo-timings/` → nothing; `find ~/Develop -maxdepth 4 -name 'cargo-timing*.html'` finds only `spike-moli/target/cargo-timings/*`. **Build-script durations for `libsqlite3-sys` and `boring-sys`: NOT DETERMINED** (no timing data; the `invoked.timestamp`→`output` mtime delta in each `target/*/build/<crate>-<hash>/` is 0 s because cargo rewrites both at the end, so it cannot be used).
- Build output present in `~/Develop/theuth/target` (nothing was run there):
  - release: 8 `libsqlite3-sys-*` dirs (4 with `out/`), 2 `boring-sys-*` dirs. Static libs: `libsqlcipher.a` **2,697,432 B** (`libsqlite3-sys-606a5a0d983806ec/out`, the SQLCipher variant); plain `libsqlite3.a` 2,149,536 / 2,443,576 / 2,146,664 B (three other feature variants: the 2.44 MB one is the session/preupdate build); `boring-sys-e08ee5fd14b11214/out/build/libcrypto.a` **15,547,320 B** + `libssl.a` **11,955,848 B**; the boring-sys `out/` (cmake tree) is **325 MB**.
  - debug: 24 `libsqlite3-sys-*` dirs (10 with `out/`, 4.2–8.6 MB each), 4 `boring-sys-*` dirs (two `out/` at 323 MB each).
- Two `libsqlite3-sys` units exist per profile at minimum (vendored SQLCipher-on-BoringSSL for the Signal store + the plain bundled one for sqlx), plus the `session`/`preupdate_hook` variant — i.e. the C SQLite is compiled 3–4 times per profile today. `vendor/libsqlite3-sys/Cargo.toml` lines 61–63: `preupdate_hook = ["buildtime_bindgen"]`, `session = ["preupdate_hook", …]` (session implies bindgen at build time → libclang needed for that variant).
- The rusqlite `bundled` unit compiled in this spike's scratch build (§4) is the comparable C-SQLite baseline unit.

## 11 Exact errors and surprises
1. **Single-writer-process by default.** A second process opening the same file (any mode, incl. `--experimental-multiprocess-wal` against a default holder) fails at open: `Error: Locking error: Failed locking file '<path>'. File is locked by another process` / `… Database is already open without experimental multiprocess WAL in another process`. `PRAGMA locking_mode` reports `exclusive`. A read-only open (`--readonly` / `Builder::read_only(true)`) is allowed. C SQLite (`sqlite3` CLI) against a Turso-held file: `Error: in prepare, database is locked (5)` — it cannot read either.
2. **Multiprocess WAL + C SQLite writer breaks the Turso process (CLI 0.8.2):** `Runtime error: I/O error: short read on WAL frame at offset 32: expected 4096 bytes, got 0`, then `thread 'main' panicked at core/storage/wal.rs:5458:17: read bytes does not match expected buffer length | read=0, expected=4096, frame_id=1` on quit, after `sqlite3` wrote one row and truncated the WAL on close. File stayed `integrity_check` ok. (Observed with the 0.8.2 CLI as the holder; not re-run against clone HEAD. Two clone-HEAD processes both in multiprocess-WAL mode worked — §7.)
3. **No busy timeout by default:** two in-process connections writing concurrently lose ~93 % of transactions with `Busy("database is locked")` unless `Connection::busy_timeout()` / `PRAGMA busy_timeout` is set; with 5 s set, 1,000 tx of 10 rows complete in 0.19 s with no error. (C SQLite via sqlx/rusqlite also defaults to 0, but theuth sets one; same shape.)
4. **CDC is per connection**, not per file: only the connection that ran `PRAGMA capture_data_changes_conn('full')` logs its changes; the other connection on the same `Database`, a second open, and external writers are not captured. The log is an ordinary table (`turso_cdc` + `turso_cdc_version` + an internal autoincrement sequence table) inside the main file, visible to C SQLite.
5. **Journal mode is always WAL**; `PRAGMA journal_mode=DELETE` is a silent no-op (returns `wal`); a rollback-journal file is converted to WAL on first write (header bytes 18–19 → `02 02`). Turso creates no `-shm`; it leaves the `-wal` populated at close (no checkpoint-on-close); MVCC adds `<db>-log`; multiprocess WAL adds `<db>-tshm`.
6. **Encrypted files are not SQLite files**: magic `Turso\0<cipher-id>\0`; `sqlite3` → `file is not a database (26)`; wrong key → `Decryption failed for page=1`; 48 (AEGIS-256) / 28 (AES-256-GCM) reserved bytes per page. The encrypted file format is Turso-only (SQLCipher files are not readable and vice versa).
7. **Default `synchronous=FULL`** costs ~7× on autocommit inserts vs NORMAL (109 vs 15 µs/commit here); theuth's stores run WAL+NORMAL today, so the equivalent setting is `PRAGMA synchronous=NORMAL` per connection.
8. **Not pure Rust by default**: `cc` compiles mimalloc (feature `mimalloc`), libzstd (via `fts`→tantivy) and SimSIMD (`turso_core/simd`, not switchable from the `turso` crate); `bindgen`+`clang-sys` are compiled (not executed) as build-deps of the sdk-kits; on macOS AEGIS is pure Rust by target cfg.
9. **Compile cost**: `turso_core` alone is 130 s of rustc time in release at `-j4` (the single largest unit; tantivy 69 s); the C-SQLite baseline unit (`libsqlite3-sys` bundled build script) is 27 s + rusqlite 3 s.
10. Toolchain noise: `warning: stripping debug info with rust-objcopy failed: signal: 6 (SIGABRT) … Library not loaded: @rpath/libLLVM.dylib` ×5 from the 1.98.1 toolchain (not Turso).
11. Scratch-crate pitfalls hit: `read_file`-derived text is refused by `write_file`; the crate's own `Cargo.lock` resolves newer minor versions than the clone's lock (zstd-sys 2.1.0, simsimd 6.5.16, aegis 0.9.20, clang-sys 1.9.1).
12. Key-size validation is strict and early: `Invalid key size for Aes128Gcm: expected 16 bytes, got 32`.

## 12 Files produced (all under `/Users/Sky.Moore/Develop/spike-turso/build/`)
- `REPORT.md` — this report.
- `cli/turso_cli.tar.xz`, `cli/turso_cli-aarch64-apple-darwin/tursodb` (19,285,832 B), `cli/READY` (absolute binary path), `cli/smoke.db{,-wal}`.
- `tree-default.txt`, `tree-nodefault.txt`, `tree-normal-build.txt`, `tree-features.txt` — `cargo tree` outputs (T1).
- `embed/Cargo.toml`, `embed/Cargo.lock`, `embed/src/main.rs` — the scratch crate (subcommands `bench | sql | concurrency | cdc | encryption`; `TURSO_MP=1` env enables `experimental_multiprocess_wal`).
- `build-cold.sh`, `build-cold.log`, `load.log` — cold build + load samples; `build-warm.sh`, `build-warm.log` — warm rebuild.
- `target/` (1.0 GB; `target/release/embed` 27,647,384 B; `target/cargo-timings/cargo-timing-20261008T121528478Z-af5093bed92184be.html` cold, `cargo-timing-20261008T123726182Z-af5093bed92184be.html` warm).
- `bench.sh`, `bench.log`, `bench/{turso,turso-normal,rusqlite}.db*`.
- `interop/tasks-copy.sqlite*`, `interop/roster-copy.sqlite*` (`.backup` copies), `interop/*-trio.sqlite{,-wal,-shm}` (cp of the live trio at 13:22), `interop/*-main-only.sqlite`, `interop/fresh.db*`, `interop/legacy.db*`.
- `conc.sh`, `conc.log`, `conc/*.db*` (incl. `mvcc.db-log`, `hold.db-tshm`, `mp.db*`, `mpb.db*`).
- `cdc/cdc.db*`; `enc/enc.db*`, `enc/enc-aes.db*`, `enc/enc-128.db*`.
Nothing was written outside `build/` except `touch /Users/Sky.Moore/Develop/spike-turso/turso/core/lib.rs` (mtime only) for the warm-rebuild measurement.
