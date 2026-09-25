//! `<data_dir>/mesh.sqlite`: `peers`, `engine_stats`, `availability`, `lease_log` (67 §4).
//! The first three replicate (`replica::attach`); every write stamps `hlc`
//! from the process clock so peers merge it per column LWW.

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
        replica::Hlc { wall_ms: g.0, counter: g.1, origin: replica::PeerId(self.me.0.clone()) }.to_text()
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
        Ok(self.peer_entries().await?.into_iter().filter(|p| !p.removed()).collect())
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
    pub async fn candidate(&self, id: &EngineId, connected_since_ms: u64, now_ms: u64) -> anyhow::Result<Option<Candidate>> {
        let Some(r) = sqlx::query("SELECT class, uptime_since_ms, hub_pref, first_seen_ms FROM engine_stats WHERE id = ?")
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
            let availability_30d = self.availability_30d(&id, crate::identity::now_ms()).await?;
            out.push(EngineStats {
                id,
                uptime_since_ms: r.get::<i64, _>("uptime_since_ms") as u64,
                class: class_from(&r.get::<String, _>("class")),
                availability_30d,
                harnesses: serde_json::from_str(&r.get::<String, _>("harnesses_json")).unwrap_or_default(),
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
    pub async fn availability_touch(&self, observer: &EngineId, subject: &EngineId, now_ms: u64) -> anyhow::Result<()> {
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
        let rows = sqlx::query("SELECT from_ms, to_ms, reachable FROM availability WHERE subject = ? AND to_ms > ?")
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
        Ok(if total == 0 { 1.0 } else { up as f32 / total as f32 })
    }

    // --- lease_log -----------------------------------------------------

    /// Append one lease change.
    pub async fn lease_log(&self, at_ms: u64, holder: Option<&EngineId>, reason: &str) -> anyhow::Result<()> {
        sqlx::query("INSERT INTO lease_log (at_ms, holder, reason) VALUES (?, ?, ?)")
            .bind(at_ms as i64)
            .bind(holder.map(|h| h.0.as_str()))
            .bind(reason)
            .execute(&self.pool)
            .await?;
        Ok(())
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

/// `server | desktop | laptop | container` (the `class` column).
pub fn class_str(c: EngineClass) -> &'static str {
    match c {
        EngineClass::Server => "server",
        EngineClass::Desktop => "desktop",
        EngineClass::Laptop => "laptop",
        EngineClass::Container => "container",
    }
}

fn class_from(s: &str) -> EngineClass {
    match s {
        "server" => EngineClass::Server,
        "laptop" => EngineClass::Laptop,
        "container" => EngineClass::Container,
        _ => EngineClass::Desktop,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The three tables satisfy `replica::attach` (single-column pk, `hlc TEXT`),
    /// our writes carry parseable stamps in our origin, and the capture sees them.
    #[tokio::test]
    async fn tables_attach_to_replication() {
        let dir = tempfile::tempdir().unwrap();
        let me = EngineId("me".into());
        let store = Store::open(&dir.path().join("mesh.sqlite"), me.clone()).await.unwrap();
        let engine = replica::engine::Engine::open(&dir.path().join("replica.sqlite")).await.unwrap();
        let handle = engine.attach(store.pool(), &["peers", "engine_stats", "availability"]).await.unwrap();
        let mut feed = handle.subscribe();

        let peer = EngineId("peer".into());
        store.availability_open(&me, &peer, true, 1_000).await.unwrap();
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
        let hlc = store.put_stats(&stats, HubPref::Prefer, 5, 3_000).await.unwrap();
        assert_eq!(replica::Hlc::parse(&hlc).unwrap().origin.0, "me");

        // The 200 ms poller may already have captured some rows: drain the feed
        // until every table has been seen.
        let mut seen = std::collections::HashSet::new();
        let _ = handle.flush().await.unwrap();
        while seen.len() < 2 {
            let batch = tokio::time::timeout(Duration::from_secs(5), feed.recv()).await.unwrap().unwrap();
            assert!(batch.changes.iter().all(|c| c.hlc.origin.0 == "me"), "{batch:?}");
            seen.extend(batch.changes.iter().map(|c| c.table.clone()));
            assert!(batch.changes.iter().all(|c| c.table != "availability" || c.pk == "me|peer|1000"), "{batch:?}");
        }
        assert_eq!(store.availability_30d(&peer, 2_000).await.unwrap(), 1.0);

        let c = store.candidate(&me, 0, 3_000).await.unwrap().unwrap();
        assert!((c.hub_pref, c.first_seen_ms, c.class) == (HubPref::Prefer, 5, EngineClass::Server));
        assert!(store.candidate(&peer, 0, 3_000).await.unwrap().is_none());
    }
}
