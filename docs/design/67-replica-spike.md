# 67 — Spike: SQLite session extension under SQLCipher

*Status: run 2026-09-24 on Sky's M3 Max, `cargo test -p replica --test spike_session_ext -- --nocapture`. Code: `crates/replica/tests/spike_session_ext.rs`. Answers 67 §18 item 1 for stage 1.*

## 0. Verdict

**The extension works on the SQLite theuth links** — the vendored `libsqlite3-sys` 0.36 building SQLCipher against BoringSSL (58 §2.2) — with the `session` feature on, on a plain WAL file and on a `PRAGMA key`ed file alike. All four spike questions pass. The runtime capture in `crates/replica` nevertheless does **not** use it (§3): the session extension records only the writes made through the one connection it is attached to, and `attach(pool, …)` receives an sqlx pool whose connections it cannot hook.

## 1. What was proved

| # | Question | Result |
|---|---|---|
| 1 | `sqlite3session_create/attach` on a table; inserts, updates, deletes; `sqlite3session_changeset` yields a changeset | Yes. The session coalesces per row: insert+update → one INSERT with final values; insert+delete → nothing. |
| 2 | `sqlite3changeset_apply` on a second file with a conflict handler reproduces the rows | Yes, plain and keyed. |
| 3 | Concurrent conflicting update on both sides resolved by the handler on an `hlc` column, higher wins | Yes: `SQLITE_CHANGESET_DATA` → compare `sqlite3changeset_conflict(hlc)` (local row) with `sqlite3changeset_new(hlc)` (incoming); REPLACE or OMIT. Both sides converge whichever direction applies first. |
| 4 | Numbers | Table below. |

Encryption: the same test body runs with `PRAGMA key` on both files (`captures_and_replays_under_sqlcipher_key`) and asserts the keyed file is unreadable without the key. The extension hooks above the pager; SQLCipher's page encryption is invisible to it, as 67 decision 6 predicted.

## 2. Numbers

Table `t (id TEXT PRIMARY KEY, a TEXT, b INTEGER, hlc TEXT)`, rows ~40 bytes of payload, one transaction per 10k inserts, debug build of the test binary, WAL file on the local SSD. One run; the ordering is stable across reruns, the absolute figures wobble ±20 %.

| Measure | Result |
|---|---|
| 10k inserts, no session | 16.9 ms — **1.7 µs/row** |
| 10k inserts, session attached to the table | 21.4 ms — **2.1 µs/row** (+27 %, ~0.45 µs/row) |
| Changeset size, 1k inserted rows | 39,898 bytes — **40 B/row** (10k rows: 408,898 bytes) |
| `sqlite3changeset_apply` of the 10k-row changeset into a fresh file, conflict handler installed | **22.0 ms** (2.2 µs/row) |

Reading: the capture cost is real but a fraction of a microsecond per row on a store that sees ~1 write/min; a full-table first sync of 10k records is ~400 KB and applies in tens of milliseconds. Neither figure constrains the design; they are the baseline the trigger-based capture (§3) should be measured against if it is ever suspected of being slow.

## 3. What the spike taught about the design

1. **Per-connection capture.** `sqlite3session` is fed by the preupdate hook *of the connection it was created on*. Writes from any other connection to the same file — the other three connections of a plugin's `SqlitePool`, or a `sqlx` pool that recycles idle connections (default `idle_timeout` 10 min, `max_lifetime` 30 min) — are not seen. `attach(pool, tables)` gets a pool built by the plugin's `store.rs` and cannot install a hook on connections the pool opens later. So a "dedicated capture connection" (the brief's plan) sees nothing, and per-connection sessions on the pool leak on every recycle. `sqlite3session_diff` against a shadow copy would be connection-independent but scans every replicated table on every poll and doubles their storage — cr-sqlite's cost, which decision 6 rejected.
2. **A delete carries no clock.** A changeset DELETE has old values only; the deleter's HLC is not in it, so the handler cannot rank a delete against a concurrent update. Row-by-row apply with a tombstone table can (67-as-built-R §apply).
3. **Changesets are whole-row on INSERT and changed-columns on UPDATE**, which is exactly the per-column shape `replica::Change` needs; the trigger-based capture reproduces that shape in SQL.

Decision for stage 1: **capture by `AFTER INSERT/UPDATE/DELETE` triggers** into a `_replica_log` table in the plugin's own file (one row per changed column, written in the writer's transaction on whatever connection wrote), polled by the replica every 200 ms; apply row-by-row with per-column LWW. The session extension remains the right tool for stage 2's single-writer session log, where the writing connection is ours — the spike stands as its proof and the `session` feature stays available on the vendored build (`crates/replica` dev-dependency `rusqlite = { features = ["session", "bundled-sqlcipher"] }`; enabling it pulls build-time bindgen, which `boring-sys` already requires).

## 4. Fallbacks (not needed)

cr-sqlite (loadable extension against the vendored SQLCipher — linking unproven, 2.5× insert cost) and yrs maps mirrored to SQLite (already in tree for documents). Neither is required: the trigger log is ~150 lines of SQL generation and has no external dependency.
