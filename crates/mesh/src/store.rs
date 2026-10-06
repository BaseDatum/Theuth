//! `<data_dir>/mesh.sqlite`: `peers`, `engine_stats`, `availability`, `mesh_meta`,
//! `lease_log` (67 §4, 69 decision 8). All but `lease_log` replicate
//! (`replica::attach`); every write stamps `hlc` from the process clock so
//! peers merge it per column LWW.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::Context as _;
use sqlx::Row as _;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};

use crate::lease::Candidate;
use crate::pairing::PeerEntry;
use crate::types::{EngineClass, EngineId, EngineStats, HubPref};

/// 30 days in ms: the availability window (67 §4.5).
pub const WINDOW_MS: u64 = 30 * 24 * 3600 * 1000;

/// Process-wide hybrid logical clock: `(wall_ms, counter)` of the last stamp
/// (the `tasks` store keeps the same shape).
static HLC: Mutex<(u64, u32)> = Mutex::new((0, 0));

/// Handle to `<data_dir>/mesh.sqlite`. Cheap to clone.
#[derive(Clone)]
pub struct Store {
    pool: SqlitePool,
    /// This engine: the `hlc` origin, and never listed as its own peer.
    me: EngineId,
}

impl Store {
    /// Open or create the file and run the migrations.
    pub async fn open(path: &Path, me: EngineId) -> anyhow::Result<Self> {
        let opts = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(opts)
            .await
            .with_context(|| format!("open {}", path.display()))?;
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .context("run migrations")?;
        Ok(Self { pool, me })
    }

    /// The underlying pool (`replica::attach` takes it).
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Next HLC stamp in `replica::Hlc` text form, so the replica observes
    /// our clock instead of re-stamping the row.
    fn hlc(&self, now_ms: u64) -> String {
        let mut g = HLC.lock().unwrap_or_else(|e| e.into_inner());
        if now_ms > g.0 {
            *g = (now_ms, 0);
        } else {
            g.1 += 1;
        }
        replica::Hlc {
            wall_ms: g.0,
            counter: g.1,
            origin: replica::PeerId(self.me.0.clone()),
        }
        .to_text()
    }

    // --- peers ---------------------------------------------------------

    /// Every row, tombstones included (what travels to other engines). Never
    /// this engine: replication brings in what peers wrote about us, which is
    /// not a peer.
    pub async fn peer_entries(&self) -> anyhow::Result<Vec<PeerEntry>> {
        let rows = sqlx::query(
            "SELECT id, name, addrs_json, hub_pref, added_ms, removed_ms FROM peers WHERE id != ? ORDER BY id",
        )
        .bind(&self.me.0)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .iter()
            .map(|r| PeerEntry {
                id: EngineId(r.get::<String, _>("id")),
                name: r.get("name"),
                addrs: serde_json::from_str(&r.get::<String, _>("addrs_json")).unwrap_or_default(),
                hub_pref: hub_pref_from(&r.get::<String, _>("hub_pref")),
                added_ms: r.get::<i64, _>("added_ms") as u64,
                removed_ms: r.get::<Option<i64>, _>("removed_ms").map(|v| v as u64),
            })
            .collect())
    }

    /// Live (not removed) peers.
    pub async fn peers(&self) -> anyhow::Result<Vec<PeerEntry>> {
        Ok(self
            .peer_entries()
            .await?
            .into_iter()
            .filter(|p| !p.removed())
            .collect())
    }

    /// Upsert one row verbatim.
    pub async fn put_peer(&self, p: &PeerEntry) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO peers (id, name, addrs_json, hub_pref, added_ms, removed_ms, hlc) VALUES (?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET name = excluded.name, addrs_json = excluded.addrs_json,
             hub_pref = excluded.hub_pref, added_ms = excluded.added_ms, removed_ms = excluded.removed_ms,
             hlc = excluded.hlc",
        )
        .bind(&p.id.0)
        .bind(&p.name)
        .bind(serde_json::to_string(&p.addrs).unwrap_or_else(|_| "[]".into()))
        .bind(hub_pref_str(p.hub_pref))
        .bind(p.added_ms as i64)
        .bind(p.removed_ms.map(|v| v as i64))
        .bind(self.hlc(crate::identity::now_ms()))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    // --- engine_stats --------------------------------------------------

    /// Upsert this engine's advertisement (the row is the truth; peers get it
    /// by replication). Returns the `hlc` stamped, for the heartbeat.
    pub async fn put_stats(
        &self,
        s: &EngineStats,
        hub_pref: HubPref,
        first_seen_ms: u64,
        updated_ms: u64,
    ) -> anyhow::Result<String> {
        let hlc = self.hlc(updated_ms);
        sqlx::query(
            "INSERT INTO engine_stats (id, uptime_since_ms, class, harnesses_json, repos_json, display, gpu,
                                       hub_pref, first_seen_ms, updated_ms, hlc)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET uptime_since_ms = excluded.uptime_since_ms, class = excluded.class,
             harnesses_json = excluded.harnesses_json, repos_json = excluded.repos_json, display = excluded.display,
             gpu = excluded.gpu, hub_pref = excluded.hub_pref, first_seen_ms = excluded.first_seen_ms,
             updated_ms = excluded.updated_ms, hlc = excluded.hlc",
        )
        .bind(&s.id.0)
        .bind(s.uptime_since_ms as i64)
        .bind(class_str(s.class))
        .bind(serde_json::to_string(&s.harnesses).unwrap_or_else(|_| "[]".into()))
        .bind(serde_json::to_string(&s.repos).unwrap_or_else(|_| "[]".into()))
        .bind(i32::from(s.display))
        .bind(i32::from(s.gpu))
        .bind(hub_pref_str(hub_pref))
        .bind(first_seen_ms as i64)
        .bind(updated_ms as i64)
        .bind(&hlc)
        .execute(&self.pool)
        .await?;
        Ok(hlc)
    }

    /// Election inputs for `id` from its replicated row plus our ledger;
    /// `None` until the row has arrived (67 §4.5: nothing to request, the
    /// change feed delivers it).
    pub async fn candidate(
        &self,
        id: &EngineId,
        connected_since_ms: u64,
        now_ms: u64,
    ) -> anyhow::Result<Option<Candidate>> {
        let Some(r) = sqlx::query(
            "SELECT class, uptime_since_ms, hub_pref, first_seen_ms FROM engine_stats WHERE id = ?",
        )
        .bind(&id.0)
        .fetch_optional(&self.pool)
        .await?
        else {
            return Ok(None);
        };
        Ok(Some(Candidate {
            id: id.clone(),
            hub_pref: hub_pref_from(&r.get::<String, _>("hub_pref")),
            availability_30d: self.availability_30d(id, now_ms).await?,
            class: class_from(&r.get::<String, _>("class")),
            uptime_since_ms: r.get::<i64, _>("uptime_since_ms") as u64,
            first_seen_ms: r.get::<i64, _>("first_seen_ms") as u64,
            connected_since_ms,
        }))
    }

    /// Every known engine's stats (`availability_30d` filled from the ledger).
    pub async fn stats(&self) -> anyhow::Result<Vec<EngineStats>> {
        let rows = sqlx::query(
            "SELECT id, uptime_since_ms, class, harnesses_json, repos_json, display, gpu FROM engine_stats ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::with_capacity(rows.len());
        for r in &rows {
            let id = EngineId(r.get::<String, _>("id"));
            let availability_30d = self
                .availability_30d(&id, crate::identity::now_ms())
                .await?;
            out.push(EngineStats {
                id,
                uptime_since_ms: r.get::<i64, _>("uptime_since_ms") as u64,
                class: class_from(&r.get::<String, _>("class")),
                availability_30d,
                harnesses: serde_json::from_str(&r.get::<String, _>("harnesses_json"))
                    .unwrap_or_default(),
                repos: serde_json::from_str(&r.get::<String, _>("repos_json")).unwrap_or_default(),
                display: r.get::<i32, _>("display") != 0,
                gpu: r.get::<i32, _>("gpu") != 0,
            });
        }
        Ok(out)
    }

    // --- availability --------------------------------------------------

    /// Open a new interval for `subject` as seen by `observer` (closing the
    /// previous open one at `now_ms`).
    pub async fn availability_open(
        &self,
        observer: &EngineId,
        subject: &EngineId,
        reachable: bool,
        now_ms: u64,
    ) -> anyhow::Result<()> {
        self.availability_touch(observer, subject, now_ms).await?;
        sqlx::query(
            "INSERT OR REPLACE INTO availability (id, observer, subject, from_ms, to_ms, reachable, hlc)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(format!("{}|{}|{now_ms}", observer.0, subject.0))
        .bind(&observer.0)
        .bind(&subject.0)
        .bind(now_ms as i64)
        .bind(now_ms as i64)
        .bind(i32::from(reachable))
        .bind(self.hlc(now_ms))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Extend the latest interval of `subject` (as seen by `observer`) to `now_ms`.
    /// A replicated write: [`Touches`] decides when a heartbeat gets to call it.
    pub async fn availability_touch(
        &self,
        observer: &EngineId,
        subject: &EngineId,
        now_ms: u64,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "UPDATE availability SET to_ms = ?, hlc = ? WHERE observer = ? AND subject = ?
             AND from_ms = (SELECT MAX(from_ms) FROM availability WHERE observer = ? AND subject = ?)",
        )
        .bind(now_ms as i64)
        .bind(self.hlc(now_ms))
        .bind(&observer.0)
        .bind(&subject.0)
        .bind(&observer.0)
        .bind(&subject.0)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Reachable time ÷ observed time over the last 30 days, every observer's
    /// rows together (67 §4.5). No observations = `1.0`: an engine nobody has
    /// seen fail is not penalised (probation guards the newcomer case).
    pub async fn availability_30d(&self, subject: &EngineId, now_ms: u64) -> anyhow::Result<f32> {
        let since = now_ms.saturating_sub(WINDOW_MS) as i64;
        let rows = sqlx::query(
            "SELECT from_ms, to_ms, reachable FROM availability WHERE subject = ? AND to_ms > ?",
        )
        .bind(&subject.0)
        .bind(since)
        .fetch_all(&self.pool)
        .await?;
        let (mut up, mut total) = (0u64, 0u64);
        for r in &rows {
            let from = (r.get::<i64, _>("from_ms") as u64).max(since as u64);
            let to = (r.get::<i64, _>("to_ms") as u64).min(now_ms);
            let len = to.saturating_sub(from);
            total += len;
            if r.get::<i32, _>("reachable") != 0 {
                up += len;
            }
        }
        Ok(if total == 0 {
            1.0
        } else {
            up as f32 / total as f32
        })
    }

    // --- lease_log -----------------------------------------------------

    /// Append one lease change.
    pub async fn lease_log(
        &self,
        at_ms: u64,
        holder: Option<&EngineId>,
        reason: &str,
    ) -> anyhow::Result<()> {
        sqlx::query("INSERT INTO lease_log (at_ms, holder, reason) VALUES (?, ?, ?)")
            .bind(at_ms as i64)
            .bind(holder.map(|h| h.0.as_str()))
            .bind(reason)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // --- mesh_meta (69 decision 8) --------------------------------------

    /// One mesh-wide value (`name`, …); `None` when no engine wrote it yet.
    pub async fn meta_get(&self, key: &str) -> anyhow::Result<Option<String>> {
        let row = sqlx::query("SELECT value FROM mesh_meta WHERE key = ?")
            .bind(key)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.and_then(|r| r.get::<Option<String>, _>("value")))
    }

    /// Set a mesh-wide value (replicates LWW by `hlc`).
    pub async fn meta_set(&self, key: &str, value: &str) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO mesh_meta (key, value, hlc) VALUES (?, ?, ?)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, hlc = excluded.hlc",
        )
        .bind(key)
        .bind(value)
        .bind(self.hlc(crate::identity::now_ms()))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// A default: write `value` only when `key` has no row yet, stamped at
    /// wall-clock zero instead of now — so any explicit [`Self::meta_set`]
    /// (stamped at its real time) beats every default in the per-column LWW
    /// merge, whichever engine wrote it first. Two engines' defaults meet
    /// when they pair (both started alone): the counter ranks them by age,
    /// `u32::MAX - first_seen_s`, so the OLDER engine's default wins — the
    /// host outlives the VM it creates (79 smoke bug 5: the tie fell to the
    /// larger origin id and the guest renamed the mesh). Returns whether this
    /// call wrote.
    pub async fn meta_default(
        &self,
        key: &str,
        value: &str,
        first_seen_ms: u64,
    ) -> anyhow::Result<bool> {
        let hlc = replica::Hlc {
            wall_ms: 0,
            counter: u32::MAX - u32::try_from(first_seen_ms / 1000).unwrap_or(u32::MAX),
            origin: replica::PeerId(self.me.0.clone()),
        }
        .to_text();
        let r = sqlx::query(
            "INSERT INTO mesh_meta (key, value, hlc) VALUES (?, ?, ?) ON CONFLICT(key) DO NOTHING",
        )
        .bind(key)
        .bind(value)
        .bind(hlc)
        .execute(&self.pool)
        .await?;
        Ok(r.rows_affected() == 1)
    }
}

/// When each connected peer's open interval last reached its row. Every
/// heartbeat used to `UPDATE availability` (a replicated batch every 10 s
/// per peer, idling forever); now open and close write, and a heartbeat
/// writes only when `every_ms` has passed since the last write.
pub struct Touches {
    every_ms: u64,
    last: HashMap<EngineId, u64>,
}

impl Touches {
    /// A gate that lets a heartbeat write every `every_ms`.
    pub fn new(every_ms: u64) -> Self {
        Self {
            every_ms,
            last: HashMap::new(),
        }
    }

    /// The row of `subject` was written at `now_ms` (interval opened or closed).
    pub fn wrote(&mut self, subject: &EngineId, now_ms: u64) {
        self.last.insert(subject.clone(), now_ms);
    }

    /// Heartbeat for `subject`: is a row write due? Records it when so.
    pub fn due(&mut self, subject: &EngineId, now_ms: u64) -> bool {
        let last = self.last.entry(subject.clone()).or_insert(now_ms);
        if now_ms.saturating_sub(*last) < self.every_ms {
            return false;
        }
        *last = now_ms;
        true
    }
}

/// `prefer | eligible | never` (the `hub_pref` column).
pub fn hub_pref_str(p: HubPref) -> &'static str {
    match p {
        HubPref::Prefer => "prefer",
        HubPref::Eligible => "eligible",
        HubPref::Never => "never",
    }
}

fn hub_pref_from(s: &str) -> HubPref {
    match s {
        "prefer" => HubPref::Prefer,
        "never" => HubPref::Never,
        _ => HubPref::Eligible,
    }
}

/// `server | desktop | laptop | container | vm` (the `class` column).
pub fn class_str(c: EngineClass) -> &'static str {
    match c {
        EngineClass::Server => "server",
        EngineClass::Desktop => "desktop",
        EngineClass::Laptop => "laptop",
        EngineClass::Container => "container",
        EngineClass::Vm => "vm",
    }
}

fn class_from(s: &str) -> EngineClass {
    match s {
        "server" => EngineClass::Server,
        "laptop" => EngineClass::Laptop,
        "container" => EngineClass::Container,
        "vm" => EngineClass::Vm,
        _ => EngineClass::Desktop,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The replicated tables satisfy `replica::attach` (single-column pk, `hlc TEXT`),
    /// our writes carry parseable stamps in our origin, and the capture sees them.
    #[tokio::test]
    async fn tables_attach_to_replication() {
        let dir = tempfile::tempdir().unwrap();
        let me = EngineId("me".into());
        let store = Store::open(&dir.path().join("mesh.sqlite"), me.clone())
            .await
            .unwrap();
        let engine = replica::engine::Engine::open(&dir.path().join("replica.sqlite"))
            .await
            .unwrap();
        let handle = engine
            .attach(
                store.pool(),
                &["peers", "engine_stats", "availability", "mesh_meta"],
            )
            .await
            .unwrap();
        let mut feed = handle.subscribe();

        let peer = EngineId("peer".into());
        store
            .availability_open(&me, &peer, true, 1_000)
            .await
            .unwrap();
        store.availability_touch(&me, &peer, 2_000).await.unwrap();
        let stats = EngineStats {
            id: me.clone(),
            uptime_since_ms: 0,
            class: EngineClass::Server,
            availability_30d: 1.0,
            harnesses: vec![],
            repos: vec![],
            display: false,
            gpu: false,
        };
        let hlc = store
            .put_stats(&stats, HubPref::Prefer, 5, 3_000)
            .await
            .unwrap();
        assert_eq!(replica::Hlc::parse(&hlc).unwrap().origin.0, "me");

        // The 200 ms poller may already have captured some rows: drain the feed
        // until every table has been seen.
        let mut seen = std::collections::HashSet::new();
        let _ = handle.flush().await.unwrap();
        while seen.len() < 2 {
            let batch = tokio::time::timeout(Duration::from_secs(5), feed.recv())
                .await
                .unwrap()
                .unwrap();
            assert!(
                batch.changes.iter().all(|c| c.hlc.origin.0 == "me"),
                "{batch:?}"
            );
            seen.extend(batch.changes.iter().map(|c| c.table.clone()));
            assert!(
                batch
                    .changes
                    .iter()
                    .all(|c| c.table != "availability" || c.pk == "me|peer|1000"),
                "{batch:?}"
            );
        }
        assert_eq!(store.availability_30d(&peer, 2_000).await.unwrap(), 1.0);

        let c = store.candidate(&me, 0, 3_000).await.unwrap().unwrap();
        assert!(
            (c.hub_pref, c.first_seen_ms, c.class) == (HubPref::Prefer, 5, EngineClass::Server)
        );
        assert!(store.candidate(&peer, 0, 3_000).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn meta_default_keeps_the_first_value_and_stamps_zero() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("mesh.sqlite"), EngineId("me".into()))
            .await
            .unwrap();
        assert_eq!(store.meta_get("name").await.unwrap(), None);
        assert!(store.meta_default("name", "work", 1_000).await.unwrap());
        assert!(!store.meta_default("name", "other", 1_000).await.unwrap());
        assert_eq!(
            store.meta_get("name").await.unwrap().as_deref(),
            Some("work")
        );
        let hlc: String = sqlx::query("SELECT hlc FROM mesh_meta WHERE key = 'name'")
            .fetch_one(store.pool())
            .await
            .unwrap()
            .get("hlc");
        assert_eq!(
            replica::Hlc::parse(&hlc).unwrap().wall_ms,
            0,
            "a default is stamped at zero, not now"
        );
        store.meta_set("name", "renamed").await.unwrap();
        assert_eq!(
            store.meta_get("name").await.unwrap().as_deref(),
            Some("renamed")
        );
    }

    /// One mesh store attached to its own replica engine.
    async fn side(
        name: &str,
    ) -> (
        Store,
        std::sync::Arc<replica::engine::Engine>,
        replica::engine::ReplicaHandle,
        tempfile::TempDir,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("mesh.sqlite"), EngineId(name.into()))
            .await
            .unwrap();
        let engine = replica::engine::Engine::open(&dir.path().join("replica.sqlite"))
            .await
            .unwrap();
        let handle = engine.attach(store.pool(), &["mesh_meta"]).await.unwrap();
        (store, engine, handle, dir)
    }

    /// A rename on A, then a fresh B's default written *later*: the rename
    /// wins on both sides once the batches cross (before: B's default carried
    /// its first-seen stamp, newer than the rename, and won everywhere).
    #[tokio::test]
    async fn an_explicit_rename_beats_a_later_default_across_engines() {
        let (a, a_engine, a_handle, _da) = side("a").await;
        let (b, b_engine, b_handle, _db) = side("b").await;

        a.meta_set("name", "renamed").await.unwrap();
        b.meta_default("name", "b-host", 2_000_000).await.unwrap();

        while let Some(batch) = a_handle.flush().await.unwrap() {
            b_engine.apply(&batch).await.unwrap();
        }
        while let Some(batch) = b_handle.flush().await.unwrap() {
            a_engine.apply(&batch).await.unwrap();
        }
        assert_eq!(
            a.meta_get("name").await.unwrap().as_deref(),
            Some("renamed")
        );
        assert_eq!(
            b.meta_get("name").await.unwrap().as_deref(),
            Some("renamed")
        );
    }

    /// 79 smoke bug 5: a host (older identity) and the VM it just created (newer
    /// identity, larger origin id) both start alone and write their defaults;
    /// after pairing the host's name stands on both, whichever batch lands first.
    /// Batches travel by feed (as `sync` ships the outbox): an apply captures the
    /// receiver's pending local row into a batch `flush` never returns.
    #[tokio::test]
    async fn the_older_engines_default_wins_a_tie_of_defaults() {
        for host_first in [true, false] {
            let (host, host_engine, host_handle, _dh) = side("a-host").await;
            let (vm, vm_engine, vm_handle, _dv) = side("z-vm").await;
            let (mut host_feed, mut vm_feed) = (host_handle.subscribe(), vm_handle.subscribe());
            host.meta_default("name", "smoke-host", 1_700_000_000_000)
                .await
                .unwrap();
            vm.meta_default("name", "lab", 1_700_000_100_000)
                .await
                .unwrap();
            let host_to_vm = async {
                host_handle.flush().await.unwrap();
                while let Ok(batch) = host_feed.try_recv() {
                    vm_engine.apply(&batch).await.unwrap();
                }
            };
            let vm_to_host = async {
                vm_handle.flush().await.unwrap();
                while let Ok(batch) = vm_feed.try_recv() {
                    host_engine.apply(&batch).await.unwrap();
                }
            };
            if host_first {
                host_to_vm.await;
                vm_to_host.await;
            } else {
                vm_to_host.await;
                host_to_vm.await;
            }
            for s in [&host, &vm] {
                assert_eq!(
                    s.meta_get("name").await.unwrap().as_deref(),
                    Some("smoke-host"),
                    "host_first={host_first}"
                );
            }
        }
    }

    /// 69 audit, idle writer: three heartbeats while connected write no row
    /// (before: three replicated UPDATEs); the flush interval and the close do.
    #[tokio::test]
    async fn heartbeats_do_not_write_availability() {
        let dir = tempfile::tempdir().unwrap();
        let me = EngineId("me".into());
        let store = Store::open(&dir.path().join("mesh.sqlite"), me.clone())
            .await
            .unwrap();
        let engine = replica::engine::Engine::open(&dir.path().join("replica.sqlite"))
            .await
            .unwrap();
        let handle = engine
            .attach(store.pool(), &["availability"])
            .await
            .unwrap();
        let mut feed = handle.subscribe();
        // Every batch (poller's or flush's) is on the feed once `flush` returns.
        let mut replicated = async || {
            handle.flush().await.unwrap();
            let mut changes = Vec::new();
            while let Ok(b) = feed.try_recv() {
                changes.extend(b.changes);
            }
            changes
        };
        let peer = EngineId("peer".into());
        let mut touches = Touches::new(600_000);

        // Connect: one write.
        store
            .availability_open(&me, &peer, true, 1_000)
            .await
            .unwrap();
        touches.wrote(&peer, 1_000);
        assert!(!replicated().await.is_empty());

        // Three 10 s heartbeats: zero writes.
        let mut writes = 0;
        for now in [11_000, 21_000, 31_000] {
            if touches.due(&peer, now) {
                store.availability_touch(&me, &peer, now).await.unwrap();
                writes += 1;
            }
        }
        assert_eq!(writes, 0);
        assert!(replicated().await.is_empty(), "a heartbeat replicated");

        // The flush interval elapsed: one write; disconnect closes: one write.
        assert!(touches.due(&peer, 601_000));
        assert!(!touches.due(&peer, 611_000));
        store.availability_touch(&me, &peer, 601_000).await.unwrap();
        store
            .availability_open(&me, &peer, false, 700_000)
            .await
            .unwrap();
        touches.wrote(&peer, 700_000);
        let changes = replicated().await;
        assert!(
            changes
                .iter()
                .any(|c| c.pk == "me|peer|1000" && c.column.as_deref() == Some("to_ms")),
            "{changes:?}"
        );
        assert!(
            changes.iter().any(|c| c.pk == "me|peer|700000"),
            "{changes:?}"
        );
        assert!((store.availability_30d(&peer, 700_000).await.unwrap() - 1.0).abs() < f32::EPSILON);
    }
}
