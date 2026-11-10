//! Files as rows (67 §5.1): a directory mirrored into a replicated table
//! `(path TEXT PK, bytes BLOB, mtime_ms, deleted INT, hlc TEXT)`. Hand edits
//! become rows stamped with the file's mtime as the wall part of the HLC;
//! rows that arrive from peers are written back to disk atomically (temp +
//! rename) or removed. Deletion is a `deleted` flag, never a row delete, so
//! it merges like any column. Top-level files only; names starting with `.`
//! are ignored (that is also where the temp files live).
//!
//! No ping-pong: the mirror compares bytes before writing in either
//! direction, so its own file writes re-read as no-ops and its own row
//! writes never come back as file changes. A row whose file is missing is
//! treated as hand-deleted only if the mirror has seen that file on disk;
//! otherwise (fresh engine, crash between apply and projection) the row is
//! projected.
//!
//! 185: a [`Policy`] lets the directory's owner keep some names local (never
//! rows, a peer's row for them ignored) and settle a peer's row that would
//! replace a *different* file of the same name by keeping both — asked only
//! on a path's first contact (no peer row for it applied here yet, and not
//! written here while every paired peer was connected); after that it is an edit.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use notify::Watcher as _;
use parking_lot::Mutex;
use sqlx::{Row as _, SqlitePool};

use crate::engine::{Engine, ReplicaHandle, now_ms, quote};

/// Editors save in bursts; coalesce events this long before rescanning.
const DEBOUNCE: Duration = Duration::from_millis(300);

/// 185: what a mirror asks the directory's owner about single files.
pub trait Policy: Send + Sync {
    /// `name` stays on this engine: never published, a peer's row (or
    /// tombstone) for it never written to disk, its file never flagged deleted.
    fn local(&self, name: &str) -> bool;
    /// A peer's live row for `name` (`theirs`) is about to replace a file on
    /// disk with other bytes (`ours`) on the path's first contact: replace
    /// it, or keep both under two names.
    fn clash(&self, name: &str, ours: &[u8], theirs: &[u8]) -> Clash;
    /// Every paired peer is connected now, so a path written here reaches
    /// all of them: from this write on it has a shared history. `false` while
    /// any paired peer is offline — its own file of that name may differ.
    fn all_peers_connected(&self) -> bool;
}

/// [`Policy::clash`]'s answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Clash {
    /// The row wins: the file takes its bytes (last writer wins).
    Replace,
    /// Ours keeps the name (and the row, re-stamped so it wins everywhere);
    /// theirs is written as `theirs_as`.
    KeepOurs {
        /// File name the peer's bytes land under.
        theirs_as: String,
    },
    /// Ours moves to `ours_as`; the row's bytes take the name.
    MoveOurs {
        /// File name our bytes move to.
        ours_as: String,
    },
}

/// Mirror `dir` into `table` of `pool` (created if missing) and replicate
/// it. The roster calls this for `agents/`; `schedules/`, `skills/`, `mcp/`
/// follow. Needs the replica plugin running.
pub async fn mirror(
    pool: &SqlitePool,
    dir: &Path,
    table: &str,
    policy: Option<Arc<dyn Policy>>,
) -> anyhow::Result<ReplicaHandle> {
    let actor = crate::actor().ok_or_else(|| anyhow::anyhow!("replica plugin is not running"))?;
    actor
        .ask(crate::Mirror {
            pool: pool.clone(),
            dir: dir.to_path_buf(),
            table: table.to_owned(),
            policy,
        })
        .await
        .map_err(|e| match e {
            kameo::error::SendError::HandlerError(msg) => anyhow::anyhow!(msg),
            other => anyhow::anyhow!("replica actor: {other}"),
        })
}

/// [`mirror`] for a plugin that has no pool of its own to lend: waits up to
/// `wait` for the replica plugin, opens `<db>` (WAL, 4 connections, 5 s
/// busy), mirrors `dir` into `table` and logs the outcome. Spawn it from a
/// plugin's `start`; the handle lives for the process. Mirrors are flat:
/// subdirectories (`skills/<repo>/` git clones, `mcp/repos/`) are never
/// rows — those sync by their own means (`sync_skills`).
pub async fn mirror_at(
    db: PathBuf,
    dir: PathBuf,
    table: &'static str,
    wait: Duration,
    policy: Option<Arc<dyn Policy>>,
) {
    let deadline = tokio::time::Instant::now() + wait;
    while crate::actor().is_none() {
        if tokio::time::Instant::now() >= deadline {
            tracing::info!(
                "{table}: replica plugin not running; {} stays local",
                dir.display()
            );
            return;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    let opts = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&db)
        .create_if_missing(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .busy_timeout(Duration::from_secs(5));
    let pool = match sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(4)
        .connect_with(opts)
        .await
    {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(
                "{table}: open {}: {e}; {} stays local",
                db.display(),
                dir.display()
            );
            return;
        }
    };
    match mirror(&pool, &dir, table, policy).await {
        Ok(_) => tracing::info!("{table}: {} replicates", dir.display()),
        Err(e) => tracing::warn!("{table}: {e:#}; {} stays local", dir.display()),
    }
}

/// 187: per live file of `table` in `db` (a [`mirror_at`] database), the
/// engine that last wrote its row — the origin of the row's `hlc`. Rows
/// without a parsable stamp are left out. Empty when `db` or `table` does not
/// exist (never creates either).
pub async fn writers(db: &Path, table: &str) -> HashMap<String, crate::PeerId> {
    let rows: anyhow::Result<Vec<(String, Option<String>)>> = async {
        let mut conn = open_existing(db).await?;
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT path, hlc FROM {} WHERE deleted = 0",
            quote(table)
        )))
        .fetch_all(&mut conn)
        .await?)
    }
    .await;
    rows.unwrap_or_default()
        .into_iter()
        .filter_map(|(path, hlc)| Some((path, crate::Hlc::parse(&hlc?)?.origin)))
        .collect()
}

/// 187: the replica origin stored in `db` (`<data_dir>/replica.sqlite`), the
/// id [`writers`] reports for this engine's writes; `None` before the replica
/// plugin ever ran (never creates it).
pub async fn stored_origin(db: &Path) -> Option<crate::PeerId> {
    let mut conn = open_existing(db).await.ok()?;
    sqlx::query_scalar::<_, String>("SELECT peer FROM identity WHERE id = 1")
        .fetch_optional(&mut conn)
        .await
        .ok()
        .flatten()
        .map(crate::PeerId)
}

async fn open_existing(db: &Path) -> anyhow::Result<sqlx::SqliteConnection> {
    use sqlx::ConnectOptions as _;
    Ok(sqlx::sqlite::SqliteConnectOptions::new()
        .filename(db)
        .busy_timeout(Duration::from_secs(5))
        .connect()
        .await?)
}

struct Mirror {
    handle: ReplicaHandle,
    dir: PathBuf,
    table: String,
    /// Files this mirror has seen on disk (scanned or written).
    on_disk: Mutex<HashSet<String>>,
    /// Per path, the row `hlc` the file on disk was last projected from. A
    /// scan that finds file ≠ row while the row is a peer's, newer than the
    /// file and not yet projected is looking at a projection still on its
    /// way (apply committed the row; the feed has not reached us) and must
    /// not upsert the old bytes over it (69 audit).
    projected: Mutex<HashMap<String, String>>,
    /// Scan and projection never interleave.
    io: tokio::sync::Mutex<()>,
    /// 185: local names and same-name clashes; `None` = every file replicates, last writer wins.
    policy: Option<Arc<dyn Policy>>,
}

impl Engine {
    /// [`mirror`] on this engine; see the module docs.
    pub async fn mirror(
        self: &Arc<Self>,
        pool: &SqlitePool,
        dir: &Path,
        table: &str,
        policy: Option<Arc<dyn Policy>>,
    ) -> anyhow::Result<ReplicaHandle> {
        let m = Mirror::new(self, pool, dir, table, policy).await?;
        let handle = m.handle.clone();
        m.scan().await?;

        // Hand edits → rows (the `schedule` watcher idiom).
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if res.is_ok() {
                let _ = tx.send(());
            }
        })
        .and_then(|mut w| {
            w.watch(dir, notify::RecursiveMode::NonRecursive)
                .map(|()| w)
        });
        match watcher {
            Ok(watcher) => {
                let m = m.clone();
                tokio::spawn(async move {
                    let _watcher = watcher;
                    while rx.recv().await.is_some() {
                        tokio::time::sleep(DEBOUNCE).await;
                        while rx.try_recv().is_ok() {}
                        if let Err(e) = m.scan().await {
                            tracing::warn!("replica: mirror {}: {e:#}", m.dir.display());
                        }
                    }
                });
            }
            Err(e) => tracing::warn!(
                "replica: {} not watched: {e}; hand edits need a restart",
                dir.display()
            ),
        }

        // Rows from peers → files.
        let mut feed = handle.subscribe();
        let me = self.origin.clone();
        let m2 = m;
        tokio::spawn(async move {
            loop {
                let batch = match feed.recv().await {
                    Ok(b) => b,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        if let Err(e) = m2.project_all().await {
                            tracing::warn!("replica: mirror {}: {e:#}", m2.dir.display());
                        }
                        continue;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                };
                if batch.origin == me {
                    continue;
                }
                let mut paths: Vec<&str> = batch
                    .changes
                    .iter()
                    .filter(|c| c.table == m2.table)
                    .map(|c| c.pk.as_str())
                    .collect();
                paths.sort_unstable();
                paths.dedup();
                for p in paths {
                    if let Err(e) = m2.project(p).await {
                        tracing::warn!("replica: mirror {} {p}: {e:#}", m2.dir.display());
                    }
                }
            }
        });
        Ok(handle)
    }
}

impl Mirror {
    /// Create `table` if missing, attach it, build the mirror (no scan, no tasks).
    async fn new(
        engine: &Arc<Engine>,
        pool: &SqlitePool,
        dir: &Path,
        table: &str,
        policy: Option<Arc<dyn Policy>>,
    ) -> anyhow::Result<Arc<Self>> {
        std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "CREATE TABLE IF NOT EXISTS {} (
                path     TEXT PRIMARY KEY,
                bytes    BLOB,
                mtime_ms INTEGER,
                deleted  INTEGER NOT NULL DEFAULT 0,
                hlc      TEXT
            )",
            quote(table)
        )))
        .execute(pool)
        .await?;
        let handle = engine.attach(pool, &[table]).await?;
        let m = Arc::new(Self {
            handle,
            dir: dir.to_path_buf(),
            table: table.to_owned(),
            on_disk: Mutex::new(HashSet::new()),
            projected: Mutex::new(HashMap::new()),
            io: tokio::sync::Mutex::new(()),
            policy,
        });
        if m.policy.is_some() {
            m.seed_shared().await?;
        }
        Ok(m)
    }

    /// 185: `name` never leaves this engine ([`Policy::local`]).
    fn local(&self, name: &str) -> bool {
        self.policy.as_ref().is_some_and(|p| p.local(name))
    }

    /// R1 C7 rule 2: the local (never replicated) table of paths with a
    /// shared history — a peer's row for them was applied here. Only a path
    /// not in it can be a first contact, the one case [`Policy::clash`] is
    /// asked; after that every difference is an edit or a claim.
    fn shared_table(&self) -> String {
        quote(&format!("{}_shared", self.table))
    }

    /// Create the shared table; rows whose last writer is a peer are shared
    /// already (an engine upgraded with paths it has had from peers).
    async fn seed_shared(&self) -> anyhow::Result<()> {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "CREATE TABLE IF NOT EXISTS {} (path TEXT PRIMARY KEY)",
            self.shared_table()
        )))
        .execute(self.pool())
        .await?;
        let rows: Vec<(String, Option<String>)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT path, hlc FROM {}",
            quote(&self.table)
        )))
        .fetch_all(self.pool())
        .await?;
        for (path, hlc) in rows {
            if hlc
                .as_deref()
                .and_then(crate::Hlc::parse)
                .is_some_and(|h| h.origin != *self.handle.origin())
            {
                self.mark_shared(&path).await?;
            }
        }
        Ok(())
    }

    async fn is_shared(&self, path: &str) -> anyhow::Result<bool> {
        Ok(sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT 1 FROM {} WHERE path = ?",
            self.shared_table()
        )))
        .bind(path)
        .fetch_optional(self.pool())
        .await?
        .is_some())
    }

    async fn mark_shared(&self, path: &str) -> anyhow::Result<()> {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "INSERT OR IGNORE INTO {} (path) VALUES (?)",
            self.shared_table()
        )))
        .bind(path)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// Upsert `name` ← `bytes` as this engine's write, stamped now (above
    /// every clock seen, so it wins), and mark the file seen. A live row with
    /// these bytes already is left as it is (no batch for nothing).
    async fn put_row(&self, name: &str, bytes: &[u8]) -> anyhow::Result<()> {
        self.on_disk.lock().insert(name.to_owned());
        let same: Option<(Option<Vec<u8>>, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT bytes, deleted FROM {} WHERE path = ?",
            quote(&self.table)
        )))
        .bind(name)
        .fetch_optional(self.pool())
        .await?;
        if same.is_some_and(|(b, d)| d == 0 && b.as_deref() == Some(bytes)) {
            return Ok(());
        }
        let now = now_ms();
        let hlc = self.handle.stamp(now).to_text();
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "INSERT INTO {t} (path, bytes, mtime_ms, deleted, hlc) VALUES (?, ?, ?, 0, ?)
             ON CONFLICT (path) DO UPDATE SET bytes = excluded.bytes, mtime_ms = excluded.mtime_ms,
             deleted = 0, hlc = excluded.hlc",
            t = quote(&self.table)
        )))
        .bind(name)
        .bind(bytes)
        .bind(now as i64)
        .bind(&hlc)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// Write `bytes` to `<dir>/<name>` atomically (dotfile temp, then rename).
    fn write_file(&self, name: &str, bytes: &[u8]) -> anyhow::Result<()> {
        let file = self.dir.join(name);
        let tmp = self
            .dir
            .join(format!(".{name}.tmp-{}", uuid::Uuid::now_v7()));
        std::fs::write(&tmp, bytes).with_context(|| format!("write {}", tmp.display()))?;
        std::fs::rename(&tmp, &file).with_context(|| format!("rename to {}", file.display()))?;
        self.on_disk.lock().insert(name.to_owned());
        Ok(())
    }

    /// 185: `name` may land under `other` — free, or already these bytes.
    fn free_for(&self, other: &str, bytes: &[u8]) -> bool {
        !other.starts_with('.')
            && !other.contains('/')
            && !self.local(other)
            && std::fs::read(self.dir.join(other)).map_or(true, |cur| cur == bytes)
    }

    fn pool(&self) -> &SqlitePool {
        &self.handle.pool
    }

    /// Disk → rows: changed or new files are upserted with the mtime as the
    /// clock; files gone since last seen flag their row deleted; live rows
    /// never seen on disk are projected.
    async fn scan(&self) -> anyhow::Result<()> {
        let _io = self.io.lock().await;
        let mut seen = HashSet::new();
        for entry in
            std::fs::read_dir(&self.dir).with_context(|| format!("read {}", self.dir.display()))?
        {
            let entry = entry?;
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if name.starts_with('.') || !entry.file_type()?.is_file() || self.local(&name) {
                continue;
            }
            let meta = entry.metadata()?;
            let mtime_ms = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or_else(now_ms, |d| d.as_millis() as u64);
            let bytes = std::fs::read(entry.path())?;
            seen.insert(name.clone());
            let row = sqlx::query(sqlx::AssertSqlSafe(format!(
                "SELECT bytes, deleted, hlc FROM {} WHERE path = ?",
                quote(&self.table)
            )))
            .bind(&name)
            .fetch_optional(self.pool())
            .await?;
            let same = row.as_ref().is_some_and(|r| {
                r.get::<i64, _>("deleted") == 0
                    && r.get::<Option<Vec<u8>>, _>("bytes").as_deref() == Some(&bytes[..])
            });
            if same {
                continue;
            }
            if self.projection_pending(row.as_ref(), &name, mtime_ms) {
                tracing::debug!(
                    "replica: mirror {}: {name} awaits projection; not rescanned",
                    self.dir.display()
                );
                continue;
            }
            let hlc = self.handle.stamp(mtime_ms).to_text();
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "INSERT INTO {t} (path, bytes, mtime_ms, deleted, hlc) VALUES (?, ?, ?, 0, ?)
                 ON CONFLICT (path) DO UPDATE SET bytes = excluded.bytes, mtime_ms = excluded.mtime_ms,
                 deleted = 0, hlc = excluded.hlc",
                t = quote(&self.table)
            )))
            .bind(&name)
            .bind(&bytes)
            .bind(mtime_ms as i64)
            .bind(&hlc)
            .execute(self.pool())
            .await?;
            // R1/R2 rule 2: written while EVERY paired peer is connected, the
            // path reaches them all now — later differences are edits. With
            // one offline, its own same-named file still meets this one first.
            if self
                .policy
                .as_ref()
                .is_some_and(|p| p.all_peers_connected())
            {
                self.mark_shared(&name).await?;
            }
        }

        let live: Vec<String> = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT path FROM {} WHERE deleted = 0",
            quote(&self.table)
        )))
        .fetch_all(self.pool())
        .await?
        .into_iter()
        .map(|r| r.get::<String, _>(0))
        .filter(|p| !self.local(p))
        .collect();
        let gone: Vec<String> = {
            let mut on_disk = self.on_disk.lock();
            let gone = live
                .iter()
                .filter(|p| !seen.contains(*p) && on_disk.contains(*p))
                .cloned()
                .collect();
            *on_disk = seen;
            gone
        };
        for p in &gone {
            let hlc = self.handle.stamp(now_ms()).to_text();
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "UPDATE {} SET deleted = 1, hlc = ? WHERE path = ?",
                quote(&self.table)
            )))
            .bind(&hlc)
            .bind(p)
            .execute(self.pool())
            .await?;
        }
        for p in live
            .iter()
            .filter(|p| !gone.contains(p) && !self.on_disk.lock().contains(*p))
        {
            self.project_locked(p).await?;
        }
        Ok(())
    }

    async fn project(&self, path: &str) -> anyhow::Result<()> {
        let _io = self.io.lock().await;
        self.project_locked(path).await
    }

    /// Every live row to disk (after a lagged feed).
    async fn project_all(&self) -> anyhow::Result<()> {
        let _io = self.io.lock().await;
        let paths: Vec<String> = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT path FROM {}",
            quote(&self.table)
        )))
        .fetch_all(self.pool())
        .await?
        .into_iter()
        .map(|r| r.get(0))
        .collect();
        for p in &paths {
            self.project_locked(p).await?;
        }
        Ok(())
    }

    /// One row → its file: remove if flagged deleted, write atomically if the
    /// bytes differ, nothing if equal. 185: a local name is left alone; a
    /// peer's row over a different file asks [`Policy::clash`].
    async fn project_locked(&self, path: &str) -> anyhow::Result<()> {
        if path.starts_with('.') || path.contains('/') || path.contains(std::path::MAIN_SEPARATOR) {
            anyhow::bail!("refusing path {path:?}");
        }
        if self.local(path) {
            return Ok(());
        }
        let row = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT bytes, deleted, hlc FROM {} WHERE path = ?",
            quote(&self.table)
        )))
        .bind(path)
        .fetch_optional(self.pool())
        .await?;
        let Some(row) = row else {
            return Ok(());
        };
        let file = self.dir.join(path);
        let hlc = row.get::<Option<String>, _>("hlc");
        if let Some(hlc) = &hlc {
            self.projected.lock().insert(path.to_owned(), hlc.clone());
        }
        // 185: a peer's row only, never our own write. R1 rule 2: the first
        // peer row this engine applies for a path is its first contact.
        let theirs = hlc
            .as_deref()
            .and_then(crate::Hlc::parse)
            .is_some_and(|h| h.origin != *self.handle.origin());
        let first_contact = theirs && self.policy.is_some() && !self.is_shared(path).await?;
        if theirs && self.policy.is_some() {
            self.mark_shared(path).await?;
        }
        if row.get::<i64, _>("deleted") != 0 {
            match std::fs::remove_file(&file) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e).with_context(|| format!("remove {}", file.display())),
            }
            self.on_disk.lock().remove(path);
            return Ok(());
        }
        let bytes: Vec<u8> = row.get::<Option<Vec<u8>>, _>("bytes").unwrap_or_default();
        let cur = std::fs::read(&file).ok();
        if cur.as_deref() == Some(&bytes[..]) {
            self.on_disk.lock().insert(path.to_owned());
            return Ok(());
        }
        if let (Some(ours), Some(policy), true) = (cur, &self.policy, first_contact) {
            match policy.clash(path, &ours, &bytes) {
                Clash::Replace => {}
                Clash::KeepOurs { theirs_as } if self.free_for(&theirs_as, &bytes) => {
                    self.write_file(&theirs_as, &bytes)?;
                    self.put_row(&theirs_as, &bytes).await?;
                    self.mark_shared(&theirs_as).await?;
                    self.put_row(path, &ours).await?;
                    tracing::info!(
                        "replica: mirror {}: {path} is another routine of the same name; theirs kept as {theirs_as}",
                        self.dir.display()
                    );
                    return Ok(());
                }
                Clash::MoveOurs { ours_as } if self.free_for(&ours_as, &ours) => {
                    self.write_file(&ours_as, &ours)?;
                    self.put_row(&ours_as, &ours).await?;
                    self.mark_shared(&ours_as).await?;
                    tracing::info!(
                        "replica: mirror {}: {path} is another routine of the same name; ours moved to {ours_as}",
                        self.dir.display()
                    );
                }
                Clash::KeepOurs { theirs_as: other } | Clash::MoveOurs { ours_as: other } => {
                    tracing::warn!(
                        "replica: mirror {}: {path} clashes but {other} is taken; the newer write wins",
                        self.dir.display()
                    );
                }
            }
        }
        self.write_file(path, &bytes)
    }

    /// Is the row behind `row` (of `path`) a peer's write that scan must not
    /// override: from another origin, stamped above the file's `mtime_ms`,
    /// and not the row the file was last projected from? Once the pending
    /// projection lands, `projected` names it and a later hand edit scans.
    fn projection_pending(
        &self,
        row: Option<&sqlx::sqlite::SqliteRow>,
        path: &str,
        mtime_ms: u64,
    ) -> bool {
        let Some(text) = row.and_then(|r| r.get::<Option<String>, _>("hlc")) else {
            return false;
        };
        let Some(hlc) = crate::Hlc::parse(&text) else {
            return false;
        };
        hlc.origin != *self.handle.origin()
            && hlc.wall_ms > mtime_ms
            && self.projected.lock().get(path) != Some(&text)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use sqlx::Row as _;

    use crate::engine::Engine;
    use crate::tests::open_pool;

    struct Side {
        engine: std::sync::Arc<Engine>,
        handle: crate::ReplicaHandle,
        files: std::path::PathBuf,
        _dir: tempfile::TempDir,
    }

    async fn side() -> Side {
        side_with(None).await
    }

    async fn side_with(policy: Option<std::sync::Arc<dyn super::Policy>>) -> Side {
        let dir = tempfile::tempdir().unwrap();
        let engine = Engine::open(&dir.path().join("replica.sqlite"))
            .await
            .unwrap();
        let pool = open_pool(&dir.path().join("data.sqlite")).await;
        let files = dir.path().join("agents");
        let handle = engine
            .mirror(&pool, &files, "agents", policy)
            .await
            .unwrap();
        Side {
            engine,
            handle,
            files,
            _dir: dir,
        }
    }

    async fn row(s: &Side, path: &str) -> Option<(Vec<u8>, i64)> {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT bytes, deleted FROM {} WHERE path = ?",
            super::quote(&s.handle.tables()[0])
        )))
        .bind(path)
        .fetch_optional(&s.handle.pool)
        .await
        .unwrap()
        .map(|r| (r.get::<Option<Vec<u8>>, _>(0).unwrap_or_default(), r.get(1)))
    }

    async fn until(mut f: impl AsyncFnMut() -> bool) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while !f().await {
            assert!(tokio::time::Instant::now() < deadline, "timed out");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Ship everything `from` holds that `to` lacks.
    async fn pump(from: &Side, to: &Side) {
        from.handle.flush().await.unwrap();
        let have = to.engine.store.version_vector().await.unwrap();
        for (origin, seq) in from.engine.store.version_vector().await.unwrap() {
            let after = have.get(&origin).copied().unwrap_or(0);
            for bytes in from
                .engine
                .store
                .batches_after(&origin, after)
                .await
                .unwrap()
            {
                let _ = seq;
                let batch: crate::ChangeBatch = postcard::from_bytes(&bytes).unwrap();
                to.engine.apply(&batch).await.unwrap();
            }
        }
    }

    #[tokio::test]
    async fn hand_edit_becomes_row() {
        let a = side().await;
        std::fs::write(a.files.join("ops.md"), b"title: Ops\n---\ncharter").unwrap();
        until(async || row(&a, "ops.md").await.is_some()).await;
        let (bytes, deleted) = row(&a, "ops.md").await.unwrap();
        assert_eq!(
            (bytes.as_slice(), deleted),
            (&b"title: Ops\n---\ncharter"[..], 0)
        );
        let hlc: String = sqlx::query("SELECT hlc FROM agents WHERE path = 'ops.md'")
            .fetch_one(&a.handle.pool)
            .await
            .unwrap()
            .get(0);
        let stamp = crate::Hlc::parse(&hlc).expect("mirror stamps its rows");
        let mtime = std::fs::metadata(a.files.join("ops.md"))
            .unwrap()
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        assert!(stamp.wall_ms >= mtime, "mtime is the floor of the stamp");

        // An edit replaces the bytes; a removal flags the row.
        std::fs::write(a.files.join("ops.md"), b"edited").unwrap();
        until(async || row(&a, "ops.md").await.is_some_and(|(b, _)| b == b"edited")).await;
        std::fs::remove_file(a.files.join("ops.md")).unwrap();
        until(async || row(&a, "ops.md").await.is_some_and(|(_, d)| d == 1)).await;
    }

    #[tokio::test]
    async fn row_becomes_file_and_nothing_ping_pongs() {
        let (a, b) = (side().await, side().await);
        std::fs::write(a.files.join("ops.md"), b"charter v1").unwrap();
        until(async || row(&a, "ops.md").await.is_some()).await;
        pump(&a, &b).await;
        until(async || std::fs::read(b.files.join("ops.md")).is_ok_and(|x| x == b"charter v1"))
            .await;
        assert!(
            std::fs::read_dir(&b.files).unwrap().count() == 1,
            "no temp file left behind"
        );

        // Let B's watcher see its own write; it must not turn into a B-origin batch.
        tokio::time::sleep(Duration::from_millis(700)).await;
        assert!(
            b.handle.flush().await.unwrap().is_none(),
            "projection re-read as a change"
        );
        assert!(
            !b.engine
                .store
                .version_vector()
                .await
                .unwrap()
                .contains_key(b.engine.origin()),
            "B never wrote a batch"
        );

        // A removes the file: B's copy goes too.
        std::fs::remove_file(a.files.join("ops.md")).unwrap();
        until(async || row(&a, "ops.md").await.is_some_and(|(_, d)| d == 1)).await;
        pump(&a, &b).await;
        until(async || !b.files.join("ops.md").exists()).await;
        tokio::time::sleep(Duration::from_millis(700)).await;
        assert!(b.handle.flush().await.unwrap().is_none());
        // B holds A's delete batch now; still nothing of its own.
        assert!(
            !b.engine
                .store
                .version_vector()
                .await
                .unwrap()
                .contains_key(b.engine.origin())
        );
    }

    /// 187: `writers` names the engine that last wrote each file's row, on
    /// both sides; `stored_origin` is how an engine recognises itself there.
    #[tokio::test]
    async fn writers_name_the_last_writer_on_every_side() {
        let (a, b) = (side().await, side().await);
        std::fs::write(a.files.join("ops.md"), b"charter").unwrap();
        until(async || row(&a, "ops.md").await.is_some()).await;
        pump(&a, &b).await;
        until(async || b.files.join("ops.md").exists()).await;
        for s in [&a, &b] {
            let db = s._dir.path().join("data.sqlite");
            assert_eq!(
                super::writers(&db, "agents").await.get("ops.md"),
                Some(a.engine.origin())
            );
            assert_eq!(
                super::stored_origin(&s._dir.path().join("replica.sqlite")).await,
                Some(s.engine.origin().clone())
            );
        }
        let none = tempfile::tempdir().unwrap();
        assert!(
            super::writers(&none.path().join("x.sqlite"), "agents")
                .await
                .is_empty()
        );
        assert!(!none.path().join("x.sqlite").exists(), "never created");
        assert_eq!(
            super::stored_origin(&none.path().join("r.sqlite")).await,
            None
        );
    }

    /// 67 worker V: the three directories the plugins mirror at start, each
    /// through its own file. A hand edit on A (including `mcp/config.yaml`)
    /// is a file on B; a removal on B is gone on A. Waits on rows, never on
    /// `flush` returning `Some`: the engine's own poll captures within
    /// `POLL` and may empty the log first; `pump` ships from the store either way.
    #[tokio::test]
    async fn schedules_skills_mcp_round_trip() {
        for (table, file) in [
            ("schedules", "nightly.md"),
            ("skills", "pdf.md"),
            ("mcp", "config.yaml"),
        ] {
            let mk = async |table: &str| {
                let dir = tempfile::tempdir().unwrap();
                let engine = Engine::open(&dir.path().join("replica.sqlite"))
                    .await
                    .unwrap();
                let pool = open_pool(&dir.path().join(format!("{table}.sqlite"))).await;
                let files = dir.path().join(table);
                let handle = engine.mirror(&pool, &files, table, None).await.unwrap();
                Side {
                    engine,
                    handle,
                    files,
                    _dir: dir,
                }
            };
            let (a, b) = (mk(table).await, mk(table).await);
            // A git clone under the dir is not a row.
            std::fs::create_dir_all(a.files.join("some-repo")).unwrap();
            std::fs::write(a.files.join("some-repo").join("SKILL.md"), b"cloned").unwrap();
            std::fs::write(a.files.join(file), b"hand edit").unwrap();
            until(async || row(&a, file).await.is_some()).await;
            pump(&a, &b).await;
            until(async || std::fs::read(b.files.join(file)).is_ok_and(|x| x == b"hand edit"))
                .await;
            assert!(
                !b.files.join("some-repo").exists(),
                "{table}: clones are not mirrored"
            );

            std::fs::remove_file(b.files.join(file)).unwrap();
            until(async || row(&b, file).await.is_some_and(|(_, d)| d == 1)).await;
            pump(&b, &a).await;
            until(async || !a.files.join(file).exists()).await;
        }
    }

    /// 69 audit: a scan between apply's row commit and the feed-driven
    /// projection saw file ≠ row and upserted the old bytes with a stamp
    /// above the observed clock, reverting the peer's write and shipping the
    /// revert back. B here is a bare [`Mirror`] with no watcher or feed task,
    /// so apply, scan and project run in the order written.
    #[tokio::test]
    async fn scan_between_apply_and_projection_does_not_revert() {
        let a = side().await;
        let bdir = tempfile::tempdir().unwrap();
        let b_engine = Engine::open(&bdir.path().join("replica.sqlite"))
            .await
            .unwrap();
        let b_pool = open_pool(&bdir.path().join("data.sqlite")).await;
        let b_files = bdir.path().join("agents");
        let b = super::Mirror::new(&b_engine, &b_pool, &b_files, "agents", None)
            .await
            .unwrap();
        let b_side = Side {
            engine: b_engine.clone(),
            handle: b.handle.clone(),
            files: b_files.clone(),
            _dir: bdir,
        };

        std::fs::write(a.files.join("ops.md"), b"v1").unwrap();
        until(async || row(&a, "ops.md").await.is_some()).await;
        pump(&a, &b_side).await;
        b.scan().await.unwrap(); // never seen on disk → projected
        assert_eq!(std::fs::read(b_files.join("ops.md")).unwrap(), b"v1");

        // A edits (later mtime); B applies the row but the projection has not run.
        tokio::time::sleep(Duration::from_millis(5)).await;
        std::fs::write(a.files.join("ops.md"), b"v2").unwrap();
        until(async || row(&a, "ops.md").await.is_some_and(|(x, _)| x == b"v2")).await;
        pump(&a, &b_side).await;
        assert_eq!(row(&b_side, "ops.md").await.unwrap().0, b"v2");
        assert_eq!(
            std::fs::read(b_files.join("ops.md")).unwrap(),
            b"v1",
            "projection pending"
        );

        // The race: a scan now sees file (v1) ≠ row (v2). Before: row ← v1 with a B stamp.
        b.scan().await.unwrap();
        assert_eq!(
            row(&b_side, "ops.md").await.unwrap().0,
            b"v2",
            "scan reverted the peer's write"
        );
        assert!(
            b.handle.flush().await.unwrap().is_none(),
            "the revert shipped back"
        );

        // The projection lands; the file follows the row.
        b.project("ops.md").await.unwrap();
        assert_eq!(std::fs::read(b_files.join("ops.md")).unwrap(), b"v2");
        b.scan().await.unwrap();
        assert!(b.handle.flush().await.unwrap().is_none());
        assert!(
            !b_engine
                .store
                .version_vector()
                .await
                .unwrap()
                .contains_key(b_engine.origin()),
            "B never wrote a batch"
        );

        // A hand edit on B after the projection is a real change and does replicate.
        tokio::time::sleep(Duration::from_millis(5)).await;
        std::fs::write(b_files.join("ops.md"), b"v3").unwrap();
        b.scan().await.unwrap();
        assert_eq!(row(&b_side, "ops.md").await.unwrap().0, b"v3");
        let batch = b
            .handle
            .flush()
            .await
            .unwrap()
            .expect("hand edit replicates");
        assert_eq!(batch.origin, *b_engine.origin());
    }

    /// 185 test policy: `dreaming.md` is local; a file's first line names its
    /// routine's machine, and the one whose machine sorts later moves to
    /// `<stem>-<machine>.md` — the rule shape `schedule`'s policy uses.
    /// `.0`: every paired peer is connected.
    struct Routines(bool);

    impl super::Policy for Routines {
        fn local(&self, name: &str) -> bool {
            name == "dreaming.md"
        }

        fn clash(&self, name: &str, ours: &[u8], theirs: &[u8]) -> super::Clash {
            let owner = |b: &[u8]| {
                String::from_utf8_lossy(b)
                    .lines()
                    .next()
                    .unwrap_or("")
                    .to_owned()
            };
            let (o, t) = (owner(ours), owner(theirs));
            let stem = name.trim_end_matches(".md");
            if o == t {
                super::Clash::Replace
            } else if t > o {
                super::Clash::KeepOurs {
                    theirs_as: format!("{stem}-{t}.md"),
                }
            } else {
                super::Clash::MoveOurs {
                    ours_as: format!("{stem}-{o}.md"),
                }
            }
        }

        fn all_peers_connected(&self) -> bool {
            self.0
        }
    }

    fn routines() -> Option<std::sync::Arc<dyn super::Policy>> {
        Some(std::sync::Arc::new(Routines(false)))
    }

    async fn wrote(s: &Side) -> bool {
        let vv = s.engine.store.version_vector().await.unwrap();
        vv.contains_key(s.engine.origin())
    }

    /// 185 A1: a local name is never a row here, an old engine's row or
    /// tombstone for it never touches the file, and a stored row never
    /// resurrects a file removed by hand.
    #[tokio::test]
    async fn local_names_never_leave_and_peer_rows_never_land() {
        let (old, new) = (side().await, side_with(routines()).await);
        std::fs::write(new.files.join("dreaming.md"), b"mine").unwrap();
        std::fs::write(old.files.join("dreaming.md"), b"old engine's").unwrap();
        until(async || row(&old, "dreaming.md").await.is_some()).await;
        tokio::time::sleep(Duration::from_millis(700)).await;
        assert_eq!(row(&new, "dreaming.md").await, None, "published");
        assert!(!wrote(&new).await, "a batch for a local file");

        // The old engine's row lands in the table, never on disk.
        pump(&old, &new).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(
            std::fs::read(new.files.join("dreaming.md")).unwrap(),
            b"mine"
        );

        // Removed by hand: no tombstone, and the stored row does not project it back.
        std::fs::remove_file(new.files.join("dreaming.md")).unwrap();
        std::fs::write(new.files.join("other.md"), b"other").unwrap();
        until(async || row(&new, "other.md").await.is_some()).await;
        tokio::time::sleep(Duration::from_millis(700)).await;
        assert!(!new.files.join("dreaming.md").exists(), "resurrected");
        assert_eq!(
            row(&new, "dreaming.md").await.map(|(_, d)| d),
            Some(0),
            "the old engine's row was flagged deleted here"
        );

        // The old engine's tombstone never deletes ours.
        std::fs::write(new.files.join("dreaming.md"), b"mine again").unwrap();
        std::fs::remove_file(old.files.join("dreaming.md")).unwrap();
        until(async || row(&old, "dreaming.md").await.is_some_and(|(_, d)| d == 1)).await;
        pump(&old, &new).await;
        tokio::time::sleep(Duration::from_millis(700)).await;
        assert_eq!(
            std::fs::read(new.files.join("dreaming.md")).unwrap(),
            b"mine again"
        );
        // And ours never reaches the old engine.
        pump(&new, &old).await;
        assert_eq!(row(&old, "dreaming.md").await.map(|(_, d)| d), Some(1));
        assert!(!old.files.join("dreaming.md").exists());
    }

    /// 185 C7: two different routines named `x` keep both — on both engines
    /// `x.md` is a's and `x-b.md` is b's — and once settled nothing moves.
    #[tokio::test]
    async fn same_name_different_routines_keep_both_and_converge() {
        let (a, b) = (side_with(routines()).await, side_with(routines()).await);
        std::fs::write(a.files.join("x.md"), b"a\nA's routine").unwrap();
        until(async || row(&a, "x.md").await.is_some()).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        std::fs::write(b.files.join("x.md"), b"b\nB's routine").unwrap();
        until(async || row(&b, "x.md").await.is_some()).await;
        for _ in 0..6 {
            pump(&a, &b).await;
            pump(&b, &a).await;
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
        for s in [&a, &b] {
            assert_eq!(
                std::fs::read(s.files.join("x.md")).unwrap(),
                b"a\nA's routine"
            );
            assert_eq!(
                std::fs::read(s.files.join("x-b.md")).unwrap(),
                b"b\nB's routine"
            );
            assert_eq!(
                std::fs::read_dir(&s.files).unwrap().count(),
                2,
                "two files, no temp"
            );
        }
        // Settled: more rounds write nothing.
        let before = (
            a.engine.store.version_vector().await.unwrap(),
            b.engine.store.version_vector().await.unwrap(),
        );
        for _ in 0..2 {
            pump(&a, &b).await;
            pump(&b, &a).await;
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
        assert!(a.handle.flush().await.unwrap().is_none());
        assert!(b.handle.flush().await.unwrap().is_none());
        assert_eq!(
            before,
            (
                a.engine.store.version_vector().await.unwrap(),
                b.engine.store.version_vector().await.unwrap(),
            ),
            "ping-pong"
        );
    }

    /// R1 C7 rule 2: once a path has a shared history (each side applied
    /// the other's row), a version naming another machine is an edit or a
    /// claim — last writer wins, no rename — even racing an edit here.
    #[tokio::test]
    async fn a_path_with_shared_history_stays_one_routine() {
        let (a, b) = (side_with(routines()).await, side_with(routines()).await);
        std::fs::write(a.files.join("x.md"), b"a\nv1").unwrap();
        until(async || row(&a, "x.md").await.is_some()).await;
        pump(&a, &b).await;
        until(async || std::fs::read(b.files.join("x.md")).is_ok_and(|x| x == b"a\nv1")).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        std::fs::write(b.files.join("x.md"), b"a\nv1 edited on b").unwrap();
        until(async || {
            row(&b, "x.md")
                .await
                .is_some_and(|(x, _)| x == b"a\nv1 edited on b")
        })
        .await;
        pump(&b, &a).await;
        until(async || {
            std::fs::read(a.files.join("x.md")).is_ok_and(|x| x == b"a\nv1 edited on b")
        })
        .await;
        // The race: an edit on a, a claim with an edit on b.
        tokio::time::sleep(Duration::from_millis(20)).await;
        std::fs::write(a.files.join("x.md"), b"a\nv2").unwrap();
        until(async || row(&a, "x.md").await.is_some_and(|(x, _)| x == b"a\nv2")).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        std::fs::write(b.files.join("x.md"), b"b\nclaimed").unwrap();
        until(async || {
            row(&b, "x.md")
                .await
                .is_some_and(|(x, _)| x == b"b\nclaimed")
        })
        .await;
        for _ in 0..4 {
            pump(&a, &b).await;
            pump(&b, &a).await;
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
        for s in [&a, &b] {
            assert_eq!(
                std::fs::read(s.files.join("x.md")).unwrap(),
                b"b\nclaimed",
                "newest wins"
            );
            assert_eq!(
                std::fs::read_dir(&s.files).unwrap().count(),
                1,
                "renamed: {:?}",
                s.files
            );
        }
    }

    /// R2 rule 2: three engines, `c` offline. `a` writes `x` while only `b`
    /// is connected — not every paired peer — so `x` is not shared yet on `a`;
    /// when `c` comes back with its own, different `x`, both are kept on all
    /// three (`x` is a's, `x-c` is c's) instead of the last writer replacing one.
    #[tokio::test]
    async fn an_offline_peers_same_name_routine_is_kept_not_replaced() {
        let (a, b, c) = (
            side_with(routines()).await,
            side_with(routines()).await,
            side_with(routines()).await,
        );
        std::fs::write(a.files.join("x.md"), b"a\nA's routine").unwrap();
        until(async || row(&a, "x.md").await.is_some()).await;
        pump(&a, &b).await;
        until(async || std::fs::read(b.files.join("x.md")).is_ok_and(|x| x == b"a\nA's routine"))
            .await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        std::fs::write(c.files.join("x.md"), b"c\nC's routine").unwrap();
        until(async || row(&c, "x.md").await.is_some()).await;
        // c is back: every pair syncs.
        for _ in 0..4 {
            for (from, to) in [(&c, &a), (&a, &c), (&a, &b), (&b, &a), (&b, &c), (&c, &b)] {
                pump(from, to).await;
            }
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
        for s in [&a, &b, &c] {
            assert_eq!(
                std::fs::read(s.files.join("x.md")).unwrap(),
                b"a\nA's routine",
                "{:?}",
                s.files
            );
            assert_eq!(
                std::fs::read(s.files.join("x-c.md")).unwrap(),
                b"c\nC's routine",
                "{:?}",
                s.files
            );
        }
    }
}
