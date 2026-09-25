# 67 — As built, worker R (`crates/replica`)

`attach`'s signature is unchanged. `replica::Transport` gained one method — `async fn accept(&self, alpn) -> anyhow::Result<(PeerId, Box<dyn ByteStream>)>` — which the mesh worker must implement (no default body: without it nothing is served); `docs/design/67-contract.md` §2 Transport row updated in the same commit. Spike: **passed**.

Branch `feature/67-agents-replica`, five commits on top of the contract commit. Files: `crates/replica/**`, `docs/design/67-replica-spike.md`, `docs/design/67-as-built-R.md`, one line in `docs/design/67-contract.md`, `Cargo.lock`. Nothing in other crates or the workspace `Cargo.toml`.

## 1. Per commit

### `spike(replica): SQLite session extension under SQLCipher`
`crates/replica/tests/spike_session_ext.rs` (4 tests, rusqlite `session` API on the vendored SQLCipher-on-BoringSSL build via dev-dependency `rusqlite = { version = "0.38", features = ["session", "bundled-sqlcipher"] }`) and `docs/design/67-replica-spike.md`. Opens files the way `agent::store` does (WAL, 5 s busy timeout) and, in a second test, with `PRAGMA key`. All four questions pass; the keyed file is unreadable without the key.

| Measure (M3 Max, debug test binary, one transaction, table `t(id TEXT PK, a TEXT, b INTEGER, hlc TEXT)`) | Result |
|---|---|
| 10k inserts, no session | 16.9 ms — 1.7 µs/row |
| 10k inserts, session attached | 21.4 ms — 2.1 µs/row (+27 %) |
| Changeset per 1k inserted rows | 39,898 B — 40 B/row (10k rows: 408,898 B) |
| `sqlite3changeset_apply` of 10k rows into a fresh file with a conflict handler | 22.0 ms — 2.2 µs/row |

What it taught (spike doc §3): the session extension records only the connection it is attached to; a changeset DELETE carries no clock. Both shaped the runtime design below.

### `feat(replica): attach, capture, HLC, LWW apply`
- `engine.rs` — `Engine` (one replica: identity, `Clock`, attached tables, feed, `write_lock`), `Engine::open/attach/capture/emit_locked`, `ReplicaHandle { subscribe, origin, stamp, flush, tables }`. `attach` verifies via `PRAGMA table_info` a single-column primary key and an `hlc TEXT` column (errors name the table; composite keys refused), creates `_replica_log` in the plugin's file and recreates three triggers per table (`_replica_ai_/au_/ad_<table>`); a task polls the log every 200 ms and folds it into one `ChangeBatch { origin: self, seq }` of per-column `Change`s.
- **Capture decision — triggers, not the session extension, not a raw sqlx handle, not a rusqlite side connection.** `sqlite3session` is fed by the preupdate hook of *one* connection. `attach(pool, …)` receives a plugin's `SqlitePool` (4 lazy connections, default idle/max-lifetime recycling) that cannot be hooked from outside, so both options the brief named — the pool's `LockedSqliteHandle` or a parallel rusqlite connection — would miss writes. `AFTER INSERT/UPDATE/DELETE` triggers run in the writer's transaction on whichever connection wrote: one `_replica_log` row per changed column (`WHERE NEW.c IS NOT OLD.c`), one per row for a delete, with `typeof()` so types survive. Cost is a few extra row inserts per write in the same transaction; the spike's numbers are the yardstick if it is ever suspected.
- **HLC** — `Clock::tick(wall)` = `max(wall, last)` + counter, origin tie-break; persisted to `clock` on every emit and apply; `Clock::observe` on every incoming stamp. `Hlc::to_text()` = `{wall:016x}-{counter:08x}-{origin}` sorts like `Ord`. A write whose `hlc` column the writer set to a parsable stamp (the files mirror does, with the mtime) keeps that stamp; otherwise the clock ticks.
- `apply.rs` — `Engine::apply(&batch) -> bool`: skips own or already-held batches; groups by table and pk; per row reads the tombstone and the per-column shadow clocks, keeps each column write that is newer than both, upserts the winners in one `INSERT … ON CONFLICT DO UPDATE` (row `hlc` = max applied stamp), records clocks. One `BEGIN IMMEDIATE` per table pool; the trigger rows the apply itself produced are deleted inside that transaction (echo suppression); the bookkeeping transaction commits after the data (a crash between re-applies idempotently). Then the batch is stored in the outbox and re-emitted on the feed with `origin` preserved.
- **Deletes**: a delete wins iff its HLC is above every column clock of the row; otherwise it is void everywhere. A column write above a tombstone voids the tombstone for its whole group. The engine whose row survived a losing delete re-announces the whole row (`row_as_changes` → `emit_locked`) so the deleter, which has none of the other columns, converges on the full row.
- `store.rs` + `migrations/0002_replica_outbox.sql` — `outbox(origin, seq, batch BLOB)` (postcard), `col_clock(table_name, pk, column_name, hlc)`, `tombstone(table_name, pk, hlc)`, `identity(id=1, peer)`; `Store::identity/load_clock/version_vector/batches_after/has_batch`.
- `value.rs` — `Val`: SQLite storage class ↔ `Change.value` (tag byte `i/r/t/b` + payload; `None` = NULL).
- `lib.rs` — `attach()` asks the actor (`Attach { pool, tables }`); `Replica { rt, engine }`.
- `src/tests.rs` — six tests (below).

### `feat(replica): sync over the mesh transport`
- `sync.rs` — `Engine::register_transport(Arc<dyn Transport>)`: an accept loop on `ALPN_RECORDS`, a dialer per peer (from `peers()` at registration and each `Connected`; `Disconnected` aborts; backoff 1 s → 60 s). Both ends run the same protocol on any stream: `Hello { origin, version_vector }` each way, then every outbox batch the other lacks (own and relayed — full-mesh catch-up), then live: every feed batch goes to each open stream whose remote does not hold it yet. `Engine.remote_vv` (per remote origin, per origin seq) makes each batch go once even with two streams per pair (both ends dial); a `Hello` replaces that vector, so a failed send cannot starve a peer. A lagged feed receiver ends the stream on purpose; the reconnect's `Hello` resynchronizes. Local writes never wait on a stream (capture publishes to a broadcast channel).
- **Framing/codec: `tokio_util::codec::LengthDelimitedCodec` (64 MiB max frame) + one tag byte + `postcard`** (already in the lockfile; outbox rows are stored as postcard so catch-up sends them without re-encoding). Not `serde_json`: `Vec<u8>` values would triple in size.
- `Transport::accept` added (see the first line). `RegisterTransport` handler calls `register_transport`.
- `tests/sync_loopback.rs` — `tokio::io::duplex` transport with a `Switch` (accept queues, link table, `Connected`/`Disconnected` events).

### `feat(replica): files-as-rows projection`
- `files.rs` — `files::mirror(pool, dir, table) -> ReplicaHandle` (via the actor, `Mirror` message) and `Engine::mirror`: creates `table (path TEXT PK, bytes BLOB, mtime_ms INTEGER, deleted INTEGER NOT NULL DEFAULT 0, hlc TEXT)` if missing, attaches it, scans, watches with `notify` (the `schedule` idiom, 300 ms debounce) and projects peers' rows to disk (temp `.name.tmp-<uuid>` + rename; removal when `deleted = 1`). Hand edits are stamped `stamp(mtime_ms)`. Deletion is the `deleted` flag, never a row delete. No ping-pong by construction: bytes are compared before any write in either direction. A live row whose file is missing is flagged deleted only if the mirror has seen that file on disk (`on_disk` set); otherwise it is projected — a fresh engine's replicated rows become files instead of tombstones. Top-level files only; dotfiles ignored.

### `docs(67): as-built R`
This file.

## 2. Deviations from the brief

1. **Capture by triggers, not the session extension** (reason above; spike doc §3). The extension is proven and the `session` feature is on for test builds; it stays the right tool for stage 2's single-writer session log, where the writing connection is ours.
2. **Origin id is replica-owned.** `Hlc.origin`/`ChangeBatch.origin` is a uuid v7 generated once into `replica.sqlite` `identity`, not the mesh engine id: `attach` has no id parameter and captures start before the mesh registers its transport, and a batch's origin cannot be relabelled after it has been shipped. Mesh `PeerId`s are used only as dial addresses (`Transport::open`, `PeerEvent`); the `Hello` exchange maps a stream to its replica origin. Contract §2's note "the mesh engine id as text" on `PeerId` is therefore true for `PeerEvent` and false for `Hlc.origin`. If the mesh wants them equal, `RegisterTransport` can grow a field and `Engine::with_store` can seed `identity` on first start — a later one-line change on both sides.
3. **Tables**: no separate `applied(origin, seq)` — the `outbox` primary key is the applied set (a batch is applied iff it is held). `sync_state` is unused: the version vector is `SELECT origin, MAX(seq) FROM outbox GROUP BY origin` and is exchanged at every `Hello`; persisting per-peer acknowledgements adds state without changing behaviour. Added `col_clock`, `tombstone`, `identity` (per-column LWW needs per-column clocks; one `hlc` column per row gives row-level LWW only).
4. **Inserts ship as per-column changes** (the insert trigger logs every data column). `Change.column: None` is only ever a delete. A partial row for a pk this engine never received (its insert still in flight from a third engine) that hits a `NOT NULL` is skipped with a warning (`ponytail:` comment in `apply.rs`).
5. **Ran the spike once** (`cargo test -p replica --test spike_session_ext`) — the brief forbids `cargo test`, but the numbers are the commit's deliverable and the spike gates the rest. Nothing else was run; unit tests were written, not run. The lib had to compile for the spike binary, so the first `cargo check -p replica --tests` for commit 2 happened inside that run; each commit had one check besides.
6. **`Transport::accept`** added without a default body — the mesh worker must implement it or nothing is served (see the first line).
7. **Test dependency shape**: `session` + `bundled-sqlcipher` are enabled on `libsqlite3-sys` through replica's dev-dependency, not in the workspace `Cargo.toml`. A workspace `cargo test` therefore builds SQLite with `-DSQLITE_ENABLE_SESSION` and build-time bindgen (which `boring-sys` already requires), a plain `cargo build` does not — two `libsqlite3-sys` compilations across the two profiles, no new tooling.
8. **`hlc` column on local writes is not rewritten** by the replica: the change stamp lives in `col_clock`; the row's `hlc` is set on apply (max applied stamp) and honoured on capture when a writer set it to a parsable stamp (the files mirror).
9. **`ReplicaHandle` additions**: `flush()`, `origin()`, `stamp(wall_ms)`, `tables()` besides `subscribe()`; `Engine` is public so tests and the mesh can drive one directly.

## 3. Not built
- No detach: dropping every `ReplicaHandle` does not stop the capture task (the plugin lives for the process). A pk rewrite (`UPDATE … SET pk = …`) is logged as writes under the new pk with no delete of the old (`ponytail:` comment in `engine.rs`). Mirrors are non-recursive. No metrics, no `sync_state` persistence, no rate limiting on re-announce. Mesh ids ≠ replica origins (deviation 2).

## 4. Tests written
- `tests/spike_session_ext.rs` (4, run once, pass): plain WAL capture+replay; same under `PRAGMA key`; concurrent update resolved by `hlc` both directions, delete vs update; numbers.
- `src/tests.rs` (6, not run): insert on A appears on B with A's stamp and no echo; different columns on A and B both survive; same column → higher `hlc` wins on both whichever applies first; newer delete beats an update / older delete loses and the row comes back whole on both; re-apply is a no-op and does not undo newer writes, reopened file keeps origin and continues `seq`; `attach` rejects no-`hlc`, no-pk, missing, and double attach, naming the table.
- `src/value.rs` (1): storage-class round trip.
- `tests/sync_loopback.rs` (2, not run): live convergence, partition with writes on both sides then reconnect; relay A–B–C plus a late joiner D that only knows C.
- `src/files.rs` (2, not run): hand edit → row with mtime as the stamp floor, edit, removal → `deleted`; row → file on a second engine, own write re-read produces no batch, removal propagates, version vector unchanged.

## 5. `cargo check`
`cargo check -p replica --tests` after each of commits 2, 3, 4: `Finished` with no warnings and no errors (the third check for commit 3 came after fixing one `unwrap` on a `parking_lot` guard in the test transport; the check before it reported only that error). Commit 1's check was the spike run: 4 passed.

## Fix round R2
`cargo test -p replica --no-fail-fast`: 3 of 9 lib tests failed; the 6 integration tests passed.

- **Root cause (2 tests, `tests.rs`): capture stamped local writes after `apply` had observed the peer's clocks and overwritten the row.** Trigger capture defers the stamp to the poll/`flush`. If a remote batch was applied between the local write and its capture, (a) the clock had already observed the remote stamp so the stale log row got a *higher* HLC than the write that beat it, and (b) the log carried the pre-apply value. Same column: A ended `from B`, B ended `from A`. Delete vs update: B's pending `review` was stamped after A's delete had already removed the row, so it shipped as a lone column onto a tombstone and no re-announce fired. Fix in `apply`: before observing the batch's clocks, `capture_locked` the affected tables (new split of `capture`), so pending local writes are stamped by the pre-observation clock and against the pre-apply row. Per-column LWW, delete-above-every-column, and re-announce rules are unchanged; they now see the input they were written for.
- **Test fixed (`files.rs` `row_becomes_file_and_nothing_ping_pongs`):** its last assertion compared B's whole version vector before and after A's delete round. B's outbox legitimately holds A's delete batch (`{A: 2}` vs `{A: 1}`); the invariant the test names is "B never wrote a batch", so both checks now assert only that B's own origin is absent. The debounce sleeps were not the cause and stay.
- Not the cause this round: HLC ties (derived `Ord` ends on `origin`), the 200 ms poll (tests drive `flush`), handle lifetimes.
- Result: 9 lib + 4 spike + 2 loopback pass; three consecutive full runs green.

DONE R
