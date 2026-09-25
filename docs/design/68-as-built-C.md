# 68 worker C — as built (contract commit), 2026-09-25

Branch `feature/68-sessions`, worktree `/Users/Sky.Moore/Develop/theuth-68`. Five commits on top of 279ea92. The contract itself is `docs/design/68-contract.md` (every name, signature, table, message, config key, RPC and event with file:line, plus the per-worker allow-lists).

## Commits

1. `b67b214` **feat(agent): store splits into one write connection + read pool; session extension attached** — `Store { session, private, me, write (1 conn, no idle/lifetime, `sqlite3session` attached in `after_connect`), read (4) }`; `begin_write() -> WriteTx` (BEGIN IMMEDIATE, derefs to the connection) wraps every mutating statement; `is_private()`, `close()`. New `store/capture.rs` (`ATTACHED` 9 tables, `Session` create/attach/changeset/reset/Drop). `agent` depends on `libsqlite3-sys = { version = "0.36", features = ["session"] }` so `SQLITE_ENABLE_SESSION` is compiled for every target. Test: table-driven `every_mutation_is_captured_on_the_write_connection`.
2. `191d5d9` **feat(agent): migrations 0023 — owner/epoch, session_log, cursors, text ids** — `0023_sessions_everywhere.sql`; `Store::open(path, Option<mesh::EngineId>)` backfills `owner`; `create_session`/`fork` write `owner = me, epoch = 1`; `SessionSummary { owner, epoch, here }`; `errors` ids are uuid v7 text; `supervisor::start` loads the mesh identity file. Tests: owner backfill / `here`, error ids across fork.
3. `f822e37` **feat(agent): per-commit changesets → session_log rows; NotOwner guard** — `WriteTx::commit` takes the changeset inside the transaction, splits it per session with `sqlite3changegroup_add_change`, refuses foreign-owned sessions with `StoreError::NotOwner`, writes one `session_log` row per session (seq counter from `MAX(seq)`, `replica::Clock` HLC). API: `log_after`, `log_orphans`, `log_orphan`, `prune_log_below`, `prune_orphans_before`, `log_seq`, `cursors`, `set_cursor`, `fence`, `set_fence`, `owner`, `set_owner`, `engine`, `begin_apply() -> ApplyTx` (+ `with_raw`). `agent::DurableStore` ask → `Store`. Tests: guard/take, log API round trip. `sessions:` documented in `harness.md`.
4. `87a44f6` **feat(sessionsync): crate skeleton — wire types, ALPN, apply/snapshot stubs, config** — `crates/sessionsync` (`types.rs` `Msg` + `LogRow` + tag/postcard framing, `apply.rs` real `apply_log_row` + L stubs, `config.rs` `SessionsConfig`, `lib.rs` `SessionSync` actor + plugin). Registered in `theuth/src/main.rs` after replica. Test: frame round trip.
5. (this commit) **feat(rpc): session_take / session_owner / session_orphans stubs; SessionOwnerChanged; contract doc** — RPCs in `rpc/src/api.rs`; `AgentEvent::SessionOwnerChanged { session, owner, epoch, here }`; `SessionOwner`/`SessionOrphan` TS-exported reply types in `sessionsync::types`; `docs/design/68-contract.md`; this report as `docs/design/68-as-built-C.md`.

## Numbers asked for

- **Write statements moved:** 94 — recon's 93 (store.rs 51, communication.rs 27, accounting.rs 15) plus `Store::set_project`, whose `UPDATE … RETURNING` ran through `fetch_optional` on the pool and would have escaped capture. Every `INSERT|UPDATE|DELETE|REPLACE` in the three files now executes on a `WriteTx`; the read pool has none (checked with an awk pass over every fn mixing `&self.read` and a write keyword: zero).
- **Single-session changeset ratio:** 43 / 43 = **100 %** over the 38 public mutating methods exercised by the table-driven test (printed by the test). Multi-session transactions exist only when a tree with children is deleted; those yield one row per session.
- **`sqlite3changegroup_add_change` in the vendored SQLite:** **yes** — SQLite 3.50.4 (`vendor/libsqlite3-sys/sqlcipher/sqlite3.h:149`, declaration at `:12325`, bindgen at `bindgen_bundled_version.rs:2898`). Used; no `shared` column.
- **`cargo check -p theuth --tests`:** see "Verification" below.
- **`cargo test -p agent --lib store::`:** 53 passed, 0 failed.

## Deviations from the brief

1. **Guard placement.** The brief asked every write method to check a cached `session → (owner, epoch)` map first. Built instead as one check in `WriteTx::commit`, driven by the changeset: exact set of touched sessions, one PK lookup each, no cache to invalidate when remote applies change the owner, and a take (`owner = me`) passes naturally because the post-state is read. Covers all 94 statements with zero per-method code. O's "guard" scope is that block.
2. **`with_session_detached(|conn| …)` → `begin_apply() -> ApplyTx`.** Same effect (capture disabled for the duration via `sqlite3session_enable(0)`, re-enabled on commit/drop) without an async closure borrowing the connection; `ApplyTx::with_raw` exposes the raw handle for `sqlite3changeset_apply`.
3. **`apply_log_row` is real, not a stub** (fence, duplicate cursor, apply with owner-wins, cursor upsert in one transaction). `build_snapshot`/`apply_snapshot` return `Err("… worker L")` as the brief's stubs.
4. **Ids on the wire are `replica::PeerId`** (the `Transport` type; `mesh::EngineId` converts both ways). Store API uses `mesh::EngineId` as briefed; `agent` gained a `mesh` dependency (types + identity file loader only). `supervisor::start` loads `mesh::identity::Identity::load_or_create(data_dir/mesh)` itself because the mesh plugin starts after `agent`.
5. **`replica::Clock` made `pub`** (+ `pub use engine::Clock`) rather than copying it — a two-line change in `crates/replica`.
6. **`assert_write_pool`** is `#[cfg(test)] async fn assert_write_pool(&self, what, since: &mut i64) -> Vec<LogRow>` (log-row based, since commit 3 resets the session at every commit); the table-driven test doubles as the ratio measurement.
7. **Extra store methods** beyond the brief's list because L/O need them and they are two lines each: `log_orphans`, `prune_orphans_before`, `log_seq`, `owner`, `set_owner`, `engine`. `log_after` excludes orphaned rows (they never ship).
8. **Migration detail:** `errors.id` gets `NOT NULL` (a bare `TEXT PRIMARY KEY` admits NULL in SQLite); existing rows are re-keyed `<16-hex rowid>-<8 random bytes>` to keep their order; a fork copies as `<source id>:<new session id>`.
9. **Migrations run on the read pool before the write pool exists**, so DDL never touches an attached table under a live session (the extension would otherwise be left unusable).
10. **`SessionOwnerChanged` carries `here: bool`** in addition to `{session, owner, epoch}` so UIs need no engine-id comparison.
11. Written `/tmp/68/state-C.md` (the brief's `/tmp/67/state-C.md` path was read as a 67 leftover).

## One-line changes outside "You own" (disclosed)

- `crates/tui/src/attach.rs`, `crates/tui/src/model.rs`, `crates/slack/src/ui.rs`, `crates/agent/src/session_read.rs`: `SessionSummary` struct literals gained `owner/epoch/here`.
- `crates/slack/src/channel.rs`: one match arm for `AgentEvent::SessionOwnerChanged`.
- `crates/replica/src/engine.rs`, `lib.rs`: `Clock` public.
- `crates/rpc/Cargo.toml`: `sessionsync` dependency + `sessionsync/ts` in the `ts` feature; `crates/rpc/src/export.rs`: the two new reply types exported. `crates/theuth/Cargo.toml` + `src/main.rs`: plugin registration. Root `Cargo.toml`: workspace dependency line. `Cargo.lock`.
- `crates/agent/src/{loop_.rs,loop_/vision_fallback_tests.rs,search.rs,acp_child.rs,supervisor.rs}`: `Store::open(path, None)` in tests; `supervisor.rs` also `store.close()` (was `store.pool().close()`), the identity load, and the `DurableStore` handler.

## Not built (by design of the brief)

Shipping/sync worker, snapshot build/apply, pruning schedule (L); take/release/demote/orphan marking and `SessionOwnerChanged` publication (O); forwarding, remote events, artifacts ALPN (F); UI (U); smoke (Z). `TakeSession`/`ReleaseSession`/`session_take` answer `unimplemented`.

## Tests written (not run except the store module)

`crates/agent/src/store.rs`: `every_mutation_is_captured_on_the_write_connection`, `owner_is_set_at_creation_backfilled_at_open_and_decides_here`, `error_ids_are_text_and_unique_across_fork`, `writes_on_a_foreign_session_are_refused_until_taken`, `log_api_round_trip`. `crates/sessionsync/src/types.rs`: `frames_round_trip_with_matching_tag`.

## Verification

- `cargo check -p agent --tests`: clean (one run per commit; commits 1 and 3 needed a second pass for a test import and a `SqlSafeStr`/Drop-move fix respectively).
- `cargo check -p sessionsync --tests`: clean (second pass after a missing `Spawn` import).
- `cargo check -p theuth --tests`: `Finished dev profile` — clean after two exhaustive-match arms (`slack/channel.rs`, `tui/model.rs`) and one `ask` result fix in `rpc/api.rs`.
- `cargo test -p agent --lib store::`: 53 passed (run after commits 1, 2, 3).
- `just ts`: `export_bindings` ok (4 passed); `bindings/` gained `SessionOwner.ts`, `SessionOrphan.ts`, the `owner/epoch/here` fields in `SessionSummary.ts`, the `SessionOwnerChanged` variant in `AgentEvent.ts`, and `session_orphans/session_owner/session_take` in `methods.json`. Note: `/bindings` is gitignored in this repo (the brief's "bindings are tracked" does not hold); `just ts-desktop` copies them into the Vite tree for U.

DONE C
