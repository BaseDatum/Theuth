//! One replica: its identity, HLC, attached tables, capture and the change
//! feed. The plugin runs one per process behind [`crate::Replica`]; tests run
//! several in one process.
//!
//! Capture is trigger-based: `attach` installs `AFTER INSERT/UPDATE/DELETE`
//! triggers on each replicated table that append one `_replica_log` row per
//! changed column (per row for a delete) in the writer's own transaction, on
//! whichever pool connection wrote. A poll every [`POLL`] (or [`ReplicaHandle::flush`])
//! turns the log into one [`ChangeBatch`], stamps it, persists it to the
//! outbox and publishes it. The session extension was spiked
//! (`docs/design/67-replica-spike.md`) and captures only the connection it is
//! attached to; a pool that recycles connections cannot be hooked from outside.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Context as _;
use parking_lot::Mutex;
use sqlx::{Row as _, SqlitePool};
use tokio::sync::broadcast;

use crate::store::{self, Store};
use crate::types::{Change, ChangeBatch, Hlc, PeerId};
use crate::value::Val;

/// Capture poll interval; a write is on the feed within this bound.
const POLL: Duration = Duration::from_millis(200);
/// Log rows folded into one batch per capture.
const BATCH_ROWS: i64 = 5000;
/// Change-feed subscribers behind by this many batches see `Lagged`.
const FEED_CAPACITY: usize = 1024;
/// A capture repeating an applied `(table, pk, column, value)` within this
/// window is counted as an echo.
const ECHO_WINDOW: Duration = Duration::from_millis(500);
/// Applied column writes remembered for echo detection.
const RECENT_CAP: usize = 1024;
/// Echo warnings are logged at most this often.
const WARN_EVERY: Duration = Duration::from_secs(60);

/// Replication counters (69 audit): every path that could echo or storm has a
/// number that would show it; `mesh_status.replica` reports them. All but
/// `outbox_rows` count column changes since start.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, rename = "ReplicaStats"))]
pub struct Stats {
    /// Captured from local writes.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub captured: u64,
    /// Applied from peers' batches.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub applied: u64,
    /// Sent on a stream on behalf of another origin.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub relayed: u64,
    /// Captured within 500 ms of an apply of the same `(table, pk, column, value)`.
    /// Rising = a plugin writes back what replication just wrote.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub echo: u64,
    /// Batches held (own and relayed) after pruning.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub outbox_rows: u64,
}

#[derive(Default)]
pub(crate) struct Counters {
    captured: AtomicU64,
    applied: AtomicU64,
    relayed: AtomicU64,
    echo: AtomicU64,
    last_echo_warn_ms: AtomicU64,
}

/// One applied column write, for echo detection.
struct Recent {
    table: String,
    pk: String,
    column: String,
    value: Option<Vec<u8>>,
    at_ms: u64,
}

/// The engine that syncs over the mesh ([`Engine::register_transport`] sets
/// it): what the process-wide [`stats`] reads.
static MESH_ENGINE: Mutex<Option<Weak<Engine>>> = Mutex::new(None);

/// Counters of the engine on the mesh; `None` before the replica plugin is
/// up and the mesh has registered its transport.
pub async fn stats() -> Option<Stats> {
    let engine = MESH_ENGINE.lock().as_ref()?.upgrade()?;
    engine.stats().await.ok()
}

/// Hybrid logical clock: `max(wall, last) + counter`, tie-broken by origin.
/// Public since 68 §3: the session log stamps its rows with it.
pub struct Clock {
    origin: PeerId,
    state: Mutex<(u64, u32)>,
}

impl Clock {
    /// A clock for `origin` that continues from `last` (`(0, 0)` for a fresh one).
    pub fn new(origin: PeerId, last: (u64, u32)) -> Self {
        Self {
            origin,
            state: Mutex::new(last),
        }
    }

    /// A stamp strictly above every earlier one, at least `wall_ms`.
    pub fn tick(&self, wall_ms: u64) -> Hlc {
        let mut s = self.state.lock();
        if wall_ms > s.0 {
            *s = (wall_ms, 0);
        } else {
            s.1 += 1;
        }
        Hlc {
            wall_ms: s.0,
            counter: s.1,
            origin: self.origin.clone(),
        }
    }

    /// Learn a stamp seen elsewhere so later ticks stay above it.
    pub fn observe(&self, seen: &Hlc) {
        let mut s = self.state.lock();
        if (seen.wall_ms, seen.counter) > *s {
            *s = (seen.wall_ms, seen.counter);
        }
    }

    pub(crate) fn snapshot(&self) -> (u64, u32) {
        *self.state.lock()
    }
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// What `attach` learned about one replicated table.
#[derive(Clone, Debug)]
pub(crate) struct TableInfo {
    pub(crate) pk: String,
    /// Replicated data columns: everything but the pk and `hlc`, in order.
    pub(crate) columns: Vec<String>,
}

#[derive(Clone)]
pub(crate) struct Attached {
    pub(crate) pool: SqlitePool,
    pub(crate) info: TableInfo,
}

/// One replica. See the module docs.
pub struct Engine {
    pub(crate) store: Store,
    pub(crate) origin: PeerId,
    pub(crate) clock: Clock,
    /// Next `seq` of this origin's feed.
    next_seq: Mutex<u64>,
    /// Replicated table name → its pool and shape. Names are global: two
    /// plugins may not both replicate a table called `files`.
    pub(crate) tables: Mutex<HashMap<String, Attached>>,
    /// Every batch applied or captured, origin preserved: the UI feed and the
    /// live sync input.
    pub(crate) feed: broadcast::Sender<ChangeBatch>,
    /// Serializes capture-persist and apply: one writer of `outbox`, the
    /// clocks and `seq` at a time.
    pub(crate) write_lock: tokio::sync::Mutex<()>,
    /// Per remote origin, the highest `seq` per origin it is known to hold
    /// (from its `Hello`, advanced as batches go each way).
    pub(crate) remote_vv: Mutex<HashMap<PeerId, HashMap<PeerId, u64>>>,
    /// The mesh transport once registered, for other ALPN users (knowledge sync).
    pub(crate) transport: Mutex<Option<Arc<dyn crate::Transport>>>,
    pub(crate) counters: Counters,
    /// Applied column writes of the last [`ECHO_WINDOW`], oldest first.
    recent: Mutex<VecDeque<Recent>>,
    /// 180: `(origin, table)` pairs an apply problem was already warned
    /// about; repeats log at debug (a mixed fleet replays ~10⁵ batches).
    warned: Mutex<HashSet<(String, String)>>,
}

impl Engine {
    /// Open (or create) the bookkeeping file at `path`.
    pub async fn open(path: &Path) -> anyhow::Result<Arc<Self>> {
        let store = Store::open(path).await?;
        Self::with_store(store).await
    }

    pub(crate) async fn with_store(store: Store) -> anyhow::Result<Arc<Self>> {
        let origin = store.identity().await?;
        let clock = Clock::new(origin.clone(), store.load_clock().await?);
        let next_seq = store
            .version_vector()
            .await?
            .get(&origin)
            .copied()
            .unwrap_or(0)
            + 1;
        let (feed, _) = broadcast::channel(FEED_CAPACITY);
        Ok(Arc::new(Self {
            store,
            origin,
            clock,
            next_seq: Mutex::new(next_seq),
            tables: Mutex::new(HashMap::new()),
            feed,
            write_lock: tokio::sync::Mutex::new(()),
            remote_vv: Mutex::new(HashMap::new()),
            transport: Mutex::new(None),
            counters: Counters::default(),
            recent: Mutex::new(VecDeque::new()),
            warned: Mutex::new(HashSet::new()),
        }))
    }

    /// Log an apply problem of `origin`'s rows in `table`: a warning the
    /// first time per pair, debug after.
    pub(crate) fn note(&self, origin: &PeerId, table: &str, msg: std::fmt::Arguments<'_>) {
        if self
            .warned
            .lock()
            .insert((origin.0.clone(), table.to_owned()))
        {
            tracing::warn!("replica: {msg} (further ones from {origin} in {table} at debug)");
        } else {
            tracing::debug!("replica: {msg}");
        }
    }

    /// This replica's origin id.
    pub fn origin(&self) -> &PeerId {
        &self.origin
    }

    /// Change notifications: every batch captured here or applied from a
    /// peer, `origin` preserved.
    pub fn subscribe(&self) -> broadcast::Receiver<ChangeBatch> {
        self.feed.subscribe()
    }

    /// A fresh stamp at least `wall_ms` (a file mtime, or [`now_ms`]).
    pub fn stamp(&self, wall_ms: u64) -> Hlc {
        self.clock.tick(wall_ms)
    }

    /// The transport the mesh registered; `None` before it did.
    pub fn transport(&self) -> Option<Arc<dyn crate::Transport>> {
        self.transport.lock().clone()
    }

    /// Mark `tables` of `pool` as replicated: verify shape, install the
    /// capture triggers, start polling. Errors name the offending table.
    pub async fn attach(
        self: &Arc<Self>,
        pool: &SqlitePool,
        tables: &[&str],
    ) -> anyhow::Result<ReplicaHandle> {
        let mut infos = Vec::with_capacity(tables.len());
        for &t in tables {
            infos.push((t.to_owned(), inspect(pool, t).await?));
        }
        install_log(pool).await?;
        for (t, info) in &infos {
            install_triggers(pool, t, info)
                .await
                .with_context(|| format!("table {t}: install triggers"))?;
        }
        {
            let mut map = self.tables.lock();
            for (t, _) in &infos {
                anyhow::ensure!(!map.contains_key(t), "table {t}: already attached");
            }
            for (t, info) in &infos {
                map.insert(
                    t.clone(),
                    Attached {
                        pool: pool.clone(),
                        info: info.clone(),
                    },
                );
            }
        }
        // 180 D1b: changes parked before a restart (a migration may have
        // fixed what broke them) get their retry as the table comes back.
        {
            let _guard = self.write_lock.lock().await;
            let tables: Vec<&str> = infos.iter().map(|(t, _)| t.as_str()).collect();
            match self.retry_parked_locked(&tables).await {
                Ok(again) => {
                    for (attached, table, pk) in again {
                        let changes = self
                            .row_as_changes(&attached, &table, &pk, &crate::Hlc::stand_in())
                            .await?;
                        if !changes.is_empty() {
                            self.emit_locked(changes).await?;
                        }
                    }
                }
                Err(e) => tracing::warn!("replica: retry parked {tables:?}: {e:#}"),
            }
        }
        let names: Arc<[String]> = infos.into_iter().map(|(t, _)| t).collect();
        let handle = ReplicaHandle {
            engine: self.clone(),
            pool: pool.clone(),
            tables: names,
        };
        let h = handle.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(POLL);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tick.tick().await;
                if let Err(e) = h.engine.capture(&h.pool, &h.tables).await {
                    tracing::warn!("replica capture {:?}: {e:#}", h.tables);
                }
            }
        });
        Ok(handle)
    }

    /// Fold pending `_replica_log` rows of `tables` into one batch: stamp,
    /// persist, publish, then clear the rows. Returns the batch if any.
    pub(crate) async fn capture(
        &self,
        pool: &SqlitePool,
        tables: &[String],
    ) -> anyhow::Result<Option<ChangeBatch>> {
        let _guard = self.write_lock.lock().await;
        self.capture_locked(pool, tables).await
    }

    /// [`Engine::capture`] with `write_lock` held by the caller.
    pub(crate) async fn capture_locked(
        &self,
        pool: &SqlitePool,
        tables: &[String],
    ) -> anyhow::Result<Option<ChangeBatch>> {
        let placeholders = vec!["?"; tables.len()].join(",");
        let sql = format!(
            "SELECT seq, tbl, pk, col, vtype, value, deleted, hlc FROM _replica_log
             WHERE tbl IN ({placeholders}) ORDER BY seq LIMIT {BATCH_ROWS}"
        );
        let mut q = sqlx::query(sqlx::AssertSqlSafe(sql));
        for t in tables {
            q = q.bind(t);
        }
        let rows = q.fetch_all(pool).await?;
        let Some(last) = rows.last() else {
            return Ok(None);
        };
        let max_seq: i64 = last.get("seq");

        let mut changes = Vec::with_capacity(rows.len());
        for row in &rows {
            let deleted: bool = row.get::<i64, _>("deleted") != 0;
            let written: Option<&str> = row.get("hlc");
            let hlc = match written.and_then(Hlc::parse) {
                Some(h) => {
                    self.clock.observe(&h);
                    h
                }
                None => self.clock.tick(now_ms()),
            };
            let value = if deleted {
                None
            } else {
                let vtype: &str = row.get("vtype");
                Val::from_log(row, vtype)?.encode()
            };
            changes.push(Change {
                table: row.get("tbl"),
                pk: row.get("pk"),
                column: row.get("col"),
                value,
                hlc,
                deleted,
            });
        }

        self.count_captured(&changes);
        let batch = self.emit_locked(changes).await?;

        let sql = format!("DELETE FROM _replica_log WHERE seq <= ? AND tbl IN ({placeholders})");
        let mut q = sqlx::query(sqlx::AssertSqlSafe(sql)).bind(max_seq);
        for t in tables {
            q = q.bind(t);
        }
        q.execute(pool).await?;
        Ok(Some(batch))
    }

    /// Counters plus the outbox size.
    pub async fn stats(&self) -> anyhow::Result<Stats> {
        let outbox_rows: i64 = sqlx::query("SELECT COUNT(*) FROM outbox")
            .fetch_one(self.store.pool())
            .await?
            .get(0);
        Ok(Stats {
            captured: self.counters.captured.load(Relaxed),
            applied: self.counters.applied.load(Relaxed),
            relayed: self.counters.relayed.load(Relaxed),
            echo: self.counters.echo.load(Relaxed),
            outbox_rows: outbox_rows as u64,
        })
    }

    /// This engine is the one syncing over the mesh: [`stats`] reads it.
    pub(crate) fn set_mesh_engine(self: &Arc<Self>) {
        *MESH_ENGINE.lock() = Some(Arc::downgrade(self));
    }

    /// `changes` were sent on a stream for `origin`.
    pub(crate) fn count_relayed(&self, changes: &[Change]) {
        self.counters
            .relayed
            .fetch_add(changes.len() as u64, Relaxed);
    }

    /// `changes` were applied from a peer: count them and remember the
    /// column writes so a capture that repeats one is an echo.
    pub(crate) fn count_applied(&self, changes: &[Change]) {
        self.counters
            .applied
            .fetch_add(changes.len() as u64, Relaxed);
        let at_ms = now_ms();
        let mut recent = self.recent.lock();
        for c in changes {
            let Some(column) = &c.column else { continue };
            if c.deleted {
                continue;
            }
            if recent.len() == RECENT_CAP {
                recent.pop_front();
            }
            recent.push_back(Recent {
                table: c.table.clone(),
                pk: c.pk.clone(),
                column: column.clone(),
                value: c.value.clone(),
                at_ms,
            });
        }
    }

    /// `changes` were captured locally: count them, and count as echoes the
    /// ones repeating an applied write of the last [`ECHO_WINDOW`]. Warns
    /// (at most every [`WARN_EVERY`]) when there are any.
    fn count_captured(&self, changes: &[Change]) {
        self.counters
            .captured
            .fetch_add(changes.len() as u64, Relaxed);
        let now = now_ms();
        let echoes = {
            let mut recent = self.recent.lock();
            while recent
                .front()
                .is_some_and(|r| now.saturating_sub(r.at_ms) > ECHO_WINDOW.as_millis() as u64)
            {
                recent.pop_front();
            }
            changes
                .iter()
                .filter(|c| {
                    c.column.as_ref().is_some_and(|col| {
                        recent.iter().any(|r| {
                            r.table == c.table
                                && r.pk == c.pk
                                && r.column == *col
                                && r.value == c.value
                        })
                    })
                })
                .count() as u64
        };
        if echoes == 0 {
            return;
        }
        let total = self.counters.echo.fetch_add(echoes, Relaxed) + echoes;
        let last = self.counters.last_echo_warn_ms.load(Relaxed);
        if now.saturating_sub(last) >= WARN_EVERY.as_millis() as u64
            && self
                .counters
                .last_echo_warn_ms
                .compare_exchange(last, now, Relaxed, Relaxed)
                .is_ok()
        {
            let sample = changes
                .iter()
                .find(|c| c.column.is_some())
                .map(|c| format!("{}.{}", c.table, c.pk));
            tracing::warn!(
                "replica: {echoes} captured change(s) echo what a peer just wrote (total {total}); e.g. {}",
                sample.unwrap_or_default()
            );
        }
    }

    /// Publish `changes` as the next batch of this origin: outbox, shadow
    /// clocks, saved clock, feed. Caller holds `write_lock`.
    pub(crate) async fn emit_locked(&self, changes: Vec<Change>) -> anyhow::Result<ChangeBatch> {
        let seq = {
            let mut s = self.next_seq.lock();
            let seq = *s;
            *s += 1;
            seq
        };
        let batch = ChangeBatch {
            origin: self.origin.clone(),
            seq,
            changes,
        };
        let bytes = postcard::to_allocvec(&batch)?;

        let mut tx = self.store.pool().begin_with("BEGIN IMMEDIATE").await?;
        store::insert_batch(&mut *tx, &batch.origin, batch.seq, &bytes).await?;
        let mut revived = HashSet::new();
        for c in &batch.changes {
            if c.deleted {
                store::bury(&mut tx, &c.table, &c.pk, &c.hlc).await?;
            } else {
                if revived.insert((c.table.as_str(), c.pk.as_str())) {
                    store::unbury(&mut tx, &c.table, &c.pk).await?;
                }
                if let Some(col) = &c.column {
                    store::set_col_clock(&mut tx, &c.table, &c.pk, col, &c.hlc).await?;
                }
            }
        }
        let (wall, counter) = self.clock.snapshot();
        store::save_clock(&mut *tx, wall, counter).await?;
        tx.commit().await?;
        if let Err(e) = self.prune_outbox().await {
            tracing::warn!("replica: prune outbox: {e:#}");
        }

        let _ = self.feed.send(batch.clone());
        Ok(batch)
    }

    /// Drop batches every known peer already holds (67 loose ends 2.4, 68 §6,
    /// 69 audit): per origin, `seq < min` over what each peer holds of that
    /// origin, never the newest — `MAX(seq)` seeds `next_seq` at open and is
    /// what a `Hello` compares against. Known peers are the other origins in
    /// the outbox (every engine writes its own presence rows); one of them
    /// unseen since start means nothing is pruned, so an offline peer's
    /// catch-up survives restarts. A solo engine keeps its outbox: stage 1
    /// has no snapshot path. Returns the rows deleted.
    pub async fn prune_outbox(&self) -> anyhow::Result<u64> {
        let known = self.store.version_vector().await?;
        let floors: Vec<(PeerId, u64)> = {
            let vv = self.remote_vv.lock();
            let mut held = Vec::with_capacity(known.len());
            for peer in known.keys().filter(|p| **p != self.origin) {
                let Some(theirs) = vv.get(peer) else {
                    return Ok(0);
                };
                held.push(theirs);
            }
            if held.is_empty() {
                return Ok(0);
            }
            known
                .keys()
                .map(|origin| {
                    let min = held
                        .iter()
                        .map(|m| m.get(origin).copied().unwrap_or(0))
                        .min()
                        .unwrap_or(0);
                    (origin.clone(), min)
                })
                .collect()
        };
        let mut n = 0;
        for (origin, floor) in floors {
            n += sqlx::query(
                "DELETE FROM outbox WHERE origin = ?1 AND seq < ?2 \
                 AND seq < (SELECT MAX(seq) FROM outbox WHERE origin = ?1)",
            )
            .bind(&origin.0)
            .bind(floor as i64)
            .execute(self.store.pool())
            .await?
            .rows_affected();
        }
        Ok(n)
    }
}

/// `PRAGMA table_info`: one pk column and an `hlc TEXT` column, or an error
/// naming the table.
async fn inspect(pool: &SqlitePool, table: &str) -> anyhow::Result<TableInfo> {
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "PRAGMA table_info({})",
        quote(table)
    )))
    .fetch_all(pool)
    .await
    .with_context(|| format!("table {table}: table_info"))?;
    anyhow::ensure!(!rows.is_empty(), "table {table}: does not exist");
    let mut pk = None;
    let mut columns = Vec::new();
    let mut hlc_ok = false;
    for r in &rows {
        let name: String = r.get("name");
        let ty: String = r.get("type");
        let pk_pos: i64 = r.get("pk");
        if pk_pos == 1 {
            pk = Some(name);
        } else if pk_pos > 1 {
            anyhow::bail!("table {table}: composite primary key is not supported");
        } else if name == "hlc" {
            hlc_ok = ty.to_ascii_uppercase().contains("TEXT");
        } else {
            columns.push(name);
        }
    }
    let pk = pk.ok_or_else(|| anyhow::anyhow!("table {table}: no primary key"))?;
    anyhow::ensure!(hlc_ok, "table {table}: needs an `hlc TEXT` column");
    Ok(TableInfo { pk, columns })
}

async fn install_log(pool: &SqlitePool) -> anyhow::Result<()> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS _replica_log (
            seq     INTEGER PRIMARY KEY AUTOINCREMENT,
            tbl     TEXT    NOT NULL,
            pk      TEXT    NOT NULL,
            col     TEXT,
            vtype   TEXT,
            value,
            deleted INTEGER NOT NULL,
            hlc     TEXT
        )",
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Recreate the three triggers; run at every attach so a migration that
/// added a column is picked up.
async fn install_triggers(pool: &SqlitePool, table: &str, info: &TableInfo) -> anyhow::Result<()> {
    let t = quote(table);
    let pk = quote(&info.pk);
    let lit = format!("'{}'", table.replace('\'', "''"));
    let mut sql = String::new();
    for kind in ["ai", "au", "ad"] {
        sql.push_str(&format!(
            "DROP TRIGGER IF EXISTS {};\n",
            quote(&format!("_replica_{kind}_{table}"))
        ));
    }
    sql.push_str(&format!(
        "CREATE TRIGGER {} AFTER INSERT ON {t} BEGIN\n",
        quote(&format!("_replica_ai_{table}"))
    ));
    for c in &info.columns {
        let c = quote(c);
        sql.push_str(&format!(
            "INSERT INTO _replica_log (tbl, pk, col, vtype, value, deleted, hlc) \
             VALUES ({lit}, CAST(NEW.{pk} AS TEXT), '{}', typeof(NEW.{c}), NEW.{c}, 0, NEW.\"hlc\");\n",
            c.trim_matches('"').replace('\'', "''")
        ));
    }
    sql.push_str("END;\n");
    sql.push_str(&format!(
        "CREATE TRIGGER {} AFTER UPDATE ON {t} BEGIN\n",
        quote(&format!("_replica_au_{table}"))
    ));
    for c in &info.columns {
        let c = quote(c);
        sql.push_str(&format!(
            "INSERT INTO _replica_log (tbl, pk, col, vtype, value, deleted, hlc) \
             SELECT {lit}, CAST(NEW.{pk} AS TEXT), '{}', typeof(NEW.{c}), NEW.{c}, 0, \
             CASE WHEN NEW.\"hlc\" IS NOT OLD.\"hlc\" THEN NEW.\"hlc\" END \
             WHERE NEW.{c} IS NOT OLD.{c};\n",
            c.trim_matches('"').replace('\'', "''")
        ));
    }
    sql.push_str("END;\n");
    sql.push_str(&format!(
        "CREATE TRIGGER {} AFTER DELETE ON {t} BEGIN\n\
         INSERT INTO _replica_log (tbl, pk, col, vtype, value, deleted, hlc) \
         VALUES ({lit}, CAST(OLD.{pk} AS TEXT), NULL, NULL, NULL, 1, NULL);\nEND;",
        quote(&format!("_replica_ad_{table}"))
    ));
    // ponytail: a pk rewrite (UPDATE … SET pk = …) is logged as column writes under the new pk
    // and no delete of the old one; split into delete + insert if a plugin ever does that.
    sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
        .execute(pool)
        .await?;
    Ok(())
}

/// Double-quote an identifier.
pub(crate) fn quote(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
}

/// Handle to a set of replicated tables on one pool, returned by
/// [`Engine::attach`]. Cloneable; dropping every clone does not detach.
#[derive(Clone)]
pub struct ReplicaHandle {
    pub(crate) engine: Arc<Engine>,
    pub(crate) pool: SqlitePool,
    pub(crate) tables: Arc<[String]>,
}

impl std::fmt::Debug for ReplicaHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReplicaHandle")
            .field("origin", &self.engine.origin)
            .field("tables", &self.tables)
            .finish()
    }
}

impl ReplicaHandle {
    /// Change notifications for the UI and for anyone projecting rows
    /// elsewhere: every batch captured here or applied from a peer, `origin`
    /// preserved. A lagging receiver sees `Lagged`, never a stop.
    pub fn subscribe(&self) -> broadcast::Receiver<ChangeBatch> {
        self.engine.subscribe()
    }

    /// This replica's origin id (the `origin` of batches written here).
    pub fn origin(&self) -> &PeerId {
        &self.engine.origin
    }

    /// A fresh HLC at least `wall_ms`, in text form for an `hlc` column.
    pub fn stamp(&self, wall_ms: u64) -> Hlc {
        self.engine.stamp(wall_ms)
    }

    /// Capture pending writes now instead of at the next poll; returns the
    /// batch if there was anything.
    pub async fn flush(&self) -> anyhow::Result<Option<ChangeBatch>> {
        self.engine.capture(&self.pool, &self.tables).await
    }

    /// Announce every row of these tables that carries no `hlc` — written
    /// before the attach (a pre-existing table joining replication) or by a
    /// writer that does not stamp — as one change per column, then stamp the
    /// rows so the next call skips them. The update trigger logs nothing for
    /// a stamp-only write, hence the explicit announce. Returns the row count.
    pub async fn announce_unstamped(&self) -> anyhow::Result<u64> {
        let _guard = self.engine.write_lock.lock().await;
        let mut n = 0;
        for table in self.tables.iter() {
            let attached = self.engine.tables.lock().get(table).cloned();
            let Some(attached) = attached else { continue };
            let (t, pk) = (quote(table), quote(&attached.info.pk));
            let pks: Vec<(String,)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "SELECT CAST({pk} AS TEXT) FROM {t} WHERE \"hlc\" IS NULL"
            )))
            .fetch_all(&self.pool)
            .await?;
            if pks.is_empty() {
                continue;
            }
            let stamp = self.engine.clock.tick(now_ms());
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "UPDATE {t} SET \"hlc\" = ? WHERE \"hlc\" IS NULL"
            )))
            .bind(stamp.to_text())
            .execute(&self.pool)
            .await?;
            let mut changes = Vec::with_capacity(pks.len() * attached.info.columns.len());
            for (pk,) in &pks {
                changes.extend(
                    self.engine
                        .row_as_changes(&attached, table, pk, &stamp)
                        .await?,
                );
            }
            self.engine.emit_locked(changes).await?;
            n += pks.len() as u64;
        }
        Ok(n)
    }

    /// The tables this handle replicates.
    pub fn tables(&self) -> &[String] {
        &self.tables
    }
}
