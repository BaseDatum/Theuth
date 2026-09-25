# 68 contract — sessions everywhere (worker C, as built 2026-09-25)

*Frozen after C's fifth commit on `feature/68-sessions`. Names, signatures and file locations below are the contract L, O, F, U and Z build on; change them only with a note in your as-built. Design: `68-sessions.md` v0.2; recon: `68-recon.md`. Line numbers are as of commit 5.*

## 0. Decisions C took (deviations from the brief are marked ★)

| # | Decision | Where |
|---|---|---|
| C1 | **Changeset split is clean, one `session_log` row per session, no `shared` column.** The vendored SQLite is **3.50.4**; `sqlite3changegroup_add_change` (≥ 3.44) exists in `vendor/libsqlite3-sys/sqlcipher/sqlite3.h:12325` and in the bindgen output. Pass one (`capture::scan`) reads the session id per record; pass two (`capture::split`) feeds each record into a `sqlite3changegroup` per session and outputs one changeset each. | `crates/agent/src/store/capture.rs:225`, `:263` |
| C2 | **Measured single-session ratio: 43 / 43 = 100 %** over the 38 public mutating store methods (every call of the table-driven test produced exactly one row). Only a transaction that deletes a tree with children can touch several sessions; then it yields several rows, each with the full set of that session's rows. | test `store::tests::every_mutation_is_captured_on_the_write_connection` |
| C3 | ★ **Owner guard is evaluated at commit time from the changeset itself**, not per method: `WriteTx::commit` reads the touched sessions from the changeset, asks the post-state `sessions` row for `(owner, epoch)` (a deleted row's `(owner, epoch)` comes from the DELETE record's old.*), and refuses the whole transaction with `StoreError::NotOwner` when `owner ∉ {'', me}`. A **take** (`UPDATE sessions SET owner = me, epoch = e+1`) passes because the post-state owner is `me`. No cached map: one PK lookup per touched session per commit. Every one of the 94 write statements is covered without per-method code. | `store.rs:169–246` |
| C4 | ★ Engine id type in the store API is `mesh::EngineId` (as briefed); `agent` gained a `mesh` dependency for it and for `mesh::identity::Identity::load_or_create`, which `agent::supervisor::start` calls directly (the mesh plugin starts later). On the wire and in `Hlc.origin` the same string is a `replica::PeerId` (`From` both ways exists in `mesh::types`). | `store.rs:484`, `supervisor.rs:6481` |
| C5 | ★ Detached apply is a transaction type, not a closure: `Store::begin_apply() -> ApplyTx` disables the capture session (`sqlite3session_enable(0)`) for its lifetime and re-enables on commit/drop; `ApplyTx::with_raw(|db| …)` hands out the raw `sqlite3*` with the sqlx worker locked out. `with_session_detached(|conn| …)` would have needed an async closure borrowing the connection. | `store.rs:258–301`, `:623` |
| C6 | `errors.id` is `TEXT NOT NULL PRIMARY KEY`; `record_error` mints uuid v7; `fork` copies as `<source id>:<new session id>` (unique, keeps order behind one anchor); the migration rewrites existing rows as `<16-hex rowid>-<8 random bytes>` so their order survives. | `migrations/0023_sessions_everywhere.sql`, `store.rs:756`, `:1231` |
| C7 | Migrations and the `owner` backfill run on the **read** pool before the write pool (and its session) exist: DDL on an attached table would invalidate the session. | `store.rs:496–520` |
| C8 | `replica::Clock` made `pub` (+ `pub use`) instead of copying it; the store stamps log rows with `Hlc { origin: me | "local" }` and resumes the clock from `MAX(hlc)` in `session_log`. | `crates/replica/src/engine.rs:37`, `lib.rs:31` |
| C9 | `hlc` origin is `"local"` when the store was opened without an engine id (tests, identity failure). Such rows never ship. | `store.rs` `hlc_origin` |
| C10 | `Store::close()` deletes the capture session **before** closing the pools (deleting after `sqlite3_close` is UB; sqlx panics on a failed close). Field order in `Store` does the same on drop. If sqlx ever replaces the write connection, the stale session is leaked, never deleted. | `store.rs:603`, `:531` |
| C11 | Private `:memory:` stores keep one pool, no session, no log: `is_private()`; `begin_apply` refuses them. | `store.rs:563`, `:591` |

## 1. Build

- `crates/agent/Cargo.toml`: `libsqlite3-sys = { version = "0.36", features = ["session"] }` — resolves to `vendor/libsqlite3-sys` through the workspace `[patch]`; `session` ⇒ `preupdate_hook` + `buildtime_bindgen` ⇒ `-DSQLITE_ENABLE_SESSION` for **every** build (`vendor/libsqlite3-sys/build.rs:217`, `:534`). `crates/sessionsync/Cargo.toml` names the same dependency. `cargo tree -e features -i libsqlite3-sys | grep session` shows `libsqlite3-sys feature "session"` reached from `agent` (not only from `rusqlite` dev-deps).
- New workspace member `crates/sessionsync` (`Cargo.toml:44`); `theuth` depends on it and registers `sessionsync::SessionSyncPlugin` right after `replica::ReplicaPlugin` (`crates/theuth/src/main.rs:1040`). `rpc` depends on it (`ts` feature includes `sessionsync/ts`).

## 2. Store (`crates/agent/src/store.rs`, `store/capture.rs`)

### 2.1 Shape
```rust
pub struct Store {                                   // store.rs:60
    session: Arc<Mutex<Option<capture::Session>>>,   // None on a private store
    private: bool,
    me: Arc<str>,                                    // sessions.owner spelling of this engine; "" = lone
    tables: Arc<Vec<capture::TableMeta>>,            // PRAGMA table_info of every attached table
    seq: Arc<AtomicI64>,                             // last session_log.seq written
    clock: Arc<replica::Clock>,
    write: SqlitePool,                               // 1 connection, no idle/lifetime, session attached
    read: SqlitePool,                                // 4 connections
}
pub async fn open(path: &Path, engine: Option<mesh::EngineId>) -> Result<Store>   // :484  backfills owner='' → engine
pub async fn open_memory() -> Result<Store>                                        // :563  private
pub fn is_private(&self) -> bool                                                   // :591
pub fn engine(&self) -> &str                                                       // :637  "" on a lone engine
pub async fn close(&self)                                                          // :603
```
Attached tables (`capture::ATTACHED`, `capture.rs:15`), with the column that names the session: `sessions.id`, `messages.session_id`, `epochs.session_id`, `todo_snapshots.session_id`, `errors.session_id`, `inner_calls.session_id`, `reminders.session`, `model_calls.owner_id`, `remote_meters.owner_id`. Not attached (decision 8): `projects, cost_owners, peer_exchanges, peer_bindings, a2a_contexts, a2a_tasks, a2a_task_work`, and the 68 tables themselves (`session_log, session_cursors, session_epochs`).

### 2.2 Write path
`pub(crate) async fn begin_write(&self) -> Result<WriteTx<'_>>` (`:613`) — `BEGIN IMMEDIATE` on the single write connection; `WriteTx` derefs to `SqliteConnection`; **every** mutating statement in `store.rs`, `store/communication.rs`, `store/accounting.rs` runs inside one (94 statements moved: the recon's 93 plus `set_project`'s `UPDATE … RETURNING` that was on the read pool). `WriteTx::commit` (`:169`):
1. `lock_handle()` the connection, take `sqlite3session_changeset`, **reset** the session (delete + recreate; there is no reset API).
2. `capture::scan` → per record `(table, session id | pk, deleted sessions row's (owner, epoch))`. An UPDATE on `model_calls`/`remote_meters` (session column not in the PK, so undefined in the record) resolves its session with `SELECT owner_id FROM t WHERE pk = ?` inside the transaction.
3. `capture::split` → one changeset per session (changegroups).
4. Per session: `SELECT owner, epoch FROM sessions WHERE id = ?` (post-state; fallback to the DELETE record; fallback `('', 1)` for accounting rows of a session with no row). `owner ∉ {'', me}` ⇒ `Err(StoreError::NotOwner { session, owner, epoch })`, transaction dropped (rolled back), nothing logged.
5. `INSERT INTO session_log (seq, session_id, epoch, changeset, hlc)` with `seq = self.seq + 1`, `hlc = clock.tick(now).to_text()`, then `COMMIT`.

`pub enum StoreError { NotOwner { session: SessionId, owner: String, epoch: u32 } }` (`:84`), `Display + Error`; callers `downcast_ref::<StoreError>()` on the `anyhow::Error`.

### 2.3 Log / cursors / fence API (what L and O call)
```rust
pub struct LogRow { pub seq: i64, pub session_id: String, pub epoch: i64, pub changeset: Vec<u8>, pub hlc: String, pub orphaned: bool }  // :113
pub struct Cursor { pub origin: String, pub peer: String, pub seq: i64 }                                                             // :130
pub async fn log_after(&self, seq: i64) -> Result<Vec<LogRow>>            // :643  seq > given, orphaned = 0, seq order
pub async fn log_orphans(&self, session: SessionId) -> Result<Vec<LogRow>> // :655  orphaned rows of one session
pub async fn log_orphan(&self, session: SessionId, epoch: u32) -> Result<u64>   // :668  mark (session, epoch) orphaned; count
pub async fn prune_log_below(&self, seq: i64) -> Result<u64>              // :682  deletes seq < given AND orphaned = 0
pub async fn prune_orphans_before(&self, wall_ms: u64) -> Result<u64>     // :694  orphans whose hlc wall < wall_ms
pub fn log_seq(&self) -> i64                                              // :707  MAX(seq), 0 when empty
pub async fn cursors(&self) -> Result<Vec<Cursor>>                        // :712
pub async fn set_cursor(&self, origin: &str, peer: &str, seq: i64) -> Result<()>   // :720  upsert; peer "" = our applied cursor
pub async fn fence(&self, session: SessionId) -> Result<Option<(u32, String)>>     // :737  max(session_epochs, sessions row) → (epoch, owner)
pub async fn set_fence(&self, session: SessionId, epoch: u32, owner: &str) -> Result<bool>  // :752  upsert if higher; moved?
pub async fn owner(&self, session: SessionId) -> Result<Option<(String, u32)>>     // :770  the sessions row
pub async fn set_owner(&self, session: SessionId, owner: &str, epoch: u32) -> Result<()>  // :783  the take write (guard passes iff owner == me)
pub async fn begin_apply(&self) -> Result<ApplyTx<'_>>                    // :623  capture off for its lifetime
impl ApplyTx { pub async fn with_raw<R>(&mut self, f: impl FnOnce(NonNull<sqlite3>) -> R) -> Result<R>; pub async fn commit(self) -> Result<()> }  // :281
```
`SessionSummary` (`crates/agent/src/lib.rs:1243`) gained `owner: String` (`:1293`), `epoch: u32` (`:1296`, serde default 1), `here: bool` (`:1299`, default true) — filled by `SessionRow::summary(me)` (`store.rs:405`) for `session`, `list_sessions`, `descendants`. `create_session` and `fork` write `owner = me, epoch = 1`.

`agent::DurableStore` (`lib.rs:482`): `ask` on `Agents` → `Store` (the durable one; never private). `Store: kameo::Reply`.

### 2.4 Tests (all in `store.rs`, run with `cargo test -p agent --lib store::`, 53 green)
`every_mutation_is_captured_on_the_write_connection` (table over 38 mutating methods; prints the single-session ratio), `owner_is_set_at_creation_backfilled_at_open_and_decides_here`, `error_ids_are_text_and_unique_across_fork`, `writes_on_a_foreign_session_are_refused_until_taken`, `log_api_round_trip` (split, contiguous seqs, orphans, prune, cursors, fence, apply without echo). The `#[cfg(test)]` probe `Store::assert_write_pool(what, &mut since) -> Vec<LogRow>` panics when a mutating call left no log row.

## 3. Migration `crates/agent/migrations/0023_sessions_everywhere.sql` (owned by `agent`; `sessionsync` has no sqlite file)

| Table | Change |
|---|---|
| `sessions` | `+ owner TEXT NOT NULL DEFAULT ''`, `+ epoch INTEGER NOT NULL DEFAULT 1` |
| `session_log` | new: `seq INTEGER PK, session_id TEXT, epoch INTEGER, changeset BLOB, hlc TEXT, orphaned INTEGER DEFAULT 0`; index `(session_id, epoch, seq)` |
| `session_cursors` | new: `origin TEXT, peer TEXT, seq INTEGER, PK (origin, peer)` |
| `session_epochs` | new: `session_id TEXT PK, epoch INTEGER, owner TEXT` |
| `errors` | rebuilt with `id TEXT NOT NULL PRIMARY KEY`; index `errors_by_seq` recreated |
| `projects`, `cost_owners` | `+ hlc TEXT` |

## 4. `crates/sessionsync`

- `src/types.rs`: `MAX_FRAME = 64 MiB` (`:11`); `LogRow { seq: u64, session_id: String, epoch: u32, changeset: Vec<u8>, hlc: String }` (`:16`, `From<agent::store::LogRow>`); `enum Msg` (`:43`) = `Hello{cursors: Vec<(PeerId, u64)>}`(tag 0) · `Log{origin, rows}`(1) · `Snapshot{origin, current_to_seq, tables: Vec<(String, Vec<u8>)>}`(2) · `Release{session}`(3) · `Released{session, last_seq}`(4) · `Events{session, from_seq}`(5) · `Event{session, seq, bytes}`(6) · `Fetch{session}`(7); `Msg::encode() -> Bytes` = tag byte + postcard, `Msg::decode(&[u8])` checks the tag. Frame with `LengthDelimitedCodec::builder().max_frame_length(MAX_FRAME)` as `replica/src/sync.rs:148` does. ★ ids are `replica::PeerId` (the transport's type), not `mesh::EngineId`. Reply types `SessionOwner { owner, epoch, here }` (`:162`) and `SessionOrphan { epoch, from_seq, to_seq, at }` (`:175`), both `ts(export)`.
- `src/apply.rs`: `enum Applied { Applied, StaleEpoch, Duplicate }` (`:20`); `apply_log_row(store, origin: &PeerId, row: &LogRow) -> Result<Applied>` (`:31`) is **real**: fence check (`Store::fence`), duplicate check against cursor `(origin, "")`, `begin_apply` + `sqlite3changeset_apply` with `owner_wins` (`REPLACE` on `DATA`/`CONFLICT`, `OMIT` otherwise), cursor upsert in the same transaction. `build_snapshot(store, origin) -> Result<Msg>` (`:85`) and `apply_snapshot(store, &Msg)` (`:92`) return `Err(... worker L)` — L's bodies.
- `src/config.rs`: `SessionsConfig { takeover_secs: 300, release_wait_secs: 10, orphan_keep_days: 7 }` under `sessions:`; documented at `crates/theuth/harness.md:72`.
- `src/lib.rs`: `SESSIONS_ACTOR = "sessions"` (`:29`); `enum TakeReason { User, Orphan, Write }` + `parse("user"|"orphan"|"write")` (`:34`); messages `TransportReady(Arc<dyn Transport>)` (`:56`, tell), `PeerConnected(PeerId)` (`:60`), `PeerGone(PeerId)` (`:64`), `ShipNow` (`:68`, no-op until L), `TakeSession { id, reason }` (`:72`, ask → `Result<(String, u32), String>`, `Err(unimplemented)` until O), `ReleaseSession { id }` (`:82`, ask → `Result<u64, String>`, until O), `Peers` (`:89`, ask → `Vec<PeerId>`); actor `SessionSync { config, store, transport, peers }` (`:94`), accepts `ALPN_SESSIONS` streams into `SessionSync::serve` (logs and returns; L replaces) and forwards `PeerEvent`s to itself; `SessionSyncPlugin` (`:223`) reads `sessions:`, asks `Agents` for `DurableStore`, registers the actor, then waits on `replica::transport(rt)` and tells `TransportReady`. Helpers `orphans(store, id)` (`:267`) and `owner(store, id)` (`:289`) back the RPCs; `actor()` (`:298`).

## 5. RPC and events (`crates/rpc/src/api.rs`)
- `session_take(session, reason: String) -> SessionOwner` (`:412`): parses `reason`, asks `SessionSync::TakeSession`; `Err` until O.
- `session_owner(session) -> SessionOwner` (`:415`): `sessionsync::owner(&store, id)`.
- `session_orphans(session) -> Vec<SessionOrphan>` (`:419`): `sessionsync::orphans(&store, id)`, one entry per orphaned epoch.
- `list_sessions` rows carry `owner`, `epoch`, `here` (via `SessionSummary`).
- Event `AgentEvent::SessionOwnerChanged { session, owner, epoch, here }` (`crates/agent/src/lib.rs:1738`), published by O on every take/demote through `rt.publish`; flows through `subscribe_events` like every `AgentEvent`. No tool.

## 6. Allow-lists for the parallel workers
- **L** (log/shipping/snapshot/prune): `crates/sessionsync/src/{ship.rs,sync.rs,snapshot.rs,prune.rs}` (new) + the bodies of `apply.rs::{build_snapshot,apply_snapshot}` + `lib.rs::SessionSync::serve` and the `ShipNow`/`PeerConnected` handlers. Store surface: §2.3 only.
- **O** (ownership): `crates/sessionsync/src/owner.rs` (new) + the `TakeSession`/`ReleaseSession` handler bodies; `crates/agent/src/store.rs` **guard + orphans only** (`WriteTx::commit` step 4, `log_orphan*`, `set_owner`); `crates/tasks/src/ledger.rs` (resume takes the session); rpc `session_take` body.
- **F** (front door): `crates/rpc/src/{forward.rs,acp.rs}`, `crates/artifact/src/mesh.rs`, `crates/sessionsync/src/events.rs` (new; `Msg::Events`/`Msg::Event`).
- **U**: `crates/desktop/ui`, `crates/tui`.
- **Z**: `docs/design/68-two-engine-smoke.md` + fixes anywhere with disclosure.

## 7. Verification state at commit 5
`cargo check -p agent --tests`, `cargo check -p sessionsync --tests`, `cargo check -p theuth --tests`: see `68-as-built-C.md`. `cargo test -p agent --lib store::`: 53 passed.
