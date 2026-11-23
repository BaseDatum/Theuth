//! `share.sqlite` (210 §5.8): the replicated `shares`, `share_names`, `share_edges`, `share_rekeys`
//! and the local `settings`, `share_secrets`, `edge_state`. The master switch and the edge choice
//! live in `settings` only (D11).

use std::path::Path;
use std::time::Duration;

use anyhow::Context as _;
use sqlx::Row as _;
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteRow,
};

use crate::config::EdgeChoice;
use crate::model::{Access, Share, ShareState, Source, StoppedBy};
use crate::sources::DirPin;

/// Tables handed to `replica::attach` (§5.8).
pub const REPLICATED: [&str; 4] = ["shares", "share_names", "share_edges", "share_rekeys"];

/// `settings` keys.
pub mod keys {
    /// "Allow public sharing" (D11), `"true"` / `"false"`.
    pub const ENABLED: &str = "enabled";
    /// [`crate::config::EdgeChoice`] stored form.
    pub const EDGE: &str = "edge";
    /// The newest local Stop all, Unix ms: no share created before it may go live (§7.4).
    pub const STOP_ALL: &str = "stop_all_ms";
    /// A Stop all the edge has not confirmed yet, Unix ms; sent again at the next `welcome`.
    pub const STOP_ALL_PENDING: &str = "stop_all_pending_ms";
}

/// The plugin's store.
#[derive(Clone, Debug)]
pub struct Store {
    pool: SqlitePool,
}

impl Store {
    /// Open or create `path` and run the migration.
    pub async fn open(path: &Path) -> anyhow::Result<Self> {
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
            .context("run share migrations")?;
        Ok(Self { pool })
    }

    /// The pool, for `replica::attach`.
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    async fn setting(&self, key: &str) -> anyhow::Result<Option<String>> {
        Ok(
            sqlx::query_scalar("SELECT value FROM settings WHERE key = ?")
                .bind(key)
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    async fn set_setting(&self, key: &str, value: &str) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO settings (key, value) VALUES (?, ?) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )
        .bind(key)
        .bind(value)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The master switch; off when never set (D11).
    pub async fn enabled(&self) -> anyhow::Result<bool> {
        Ok(self.setting(keys::ENABLED).await?.as_deref() == Some("true"))
    }

    /// Write the master switch. The caller holds the approval for `true` (§7.2).
    pub async fn set_enabled(&self, on: bool) -> anyhow::Result<()> {
        self.set_setting(keys::ENABLED, if on { "true" } else { "false" })
            .await
    }

    /// The edge choice; the default edge when never set.
    pub async fn edge(&self) -> anyhow::Result<EdgeChoice> {
        match self.setting(keys::EDGE).await? {
            Some(v) => EdgeChoice::parse(&v),
            None => Ok(EdgeChoice::Default),
        }
    }

    /// Write the edge choice. The caller holds the approval (§7.2).
    pub async fn set_edge(&self, edge: &EdgeChoice) -> anyhow::Result<()> {
        self.set_setting(keys::EDGE, &edge.to_setting()).await
    }

    /// The terms revision this machine accepted at `edge`.
    pub async fn terms_rev(&self, edge: &str) -> anyhow::Result<Option<String>> {
        Ok(
            sqlx::query_scalar("SELECT terms_rev FROM edge_state WHERE edge = ?")
                .bind(edge)
                .fetch_optional(&self.pool)
                .await?
                .flatten(),
        )
    }

    /// Record an accepted terms revision.
    pub async fn set_terms_rev(&self, edge: &str, rev: &str) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO edge_state (edge, terms_rev) VALUES (?, ?) \
             ON CONFLICT(edge) DO UPDATE SET terms_rev = excluded.terms_rev",
        )
        .bind(edge)
        .bind(rev)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Record an attach and the clock skew seen in `info`.
    pub async fn attached(&self, edge: &str, now_ms: u64, skew_ms: i64) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO edge_state (edge, last_attach_ms, clock_skew_ms) VALUES (?, ?, ?) \
             ON CONFLICT(edge) DO UPDATE SET last_attach_ms = excluded.last_attach_ms, \
             clock_skew_ms = excluded.clock_skew_ms",
        )
        .bind(edge)
        .bind(now_ms as i64)
        .bind(skew_ms)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Insert or replace a share row.
    pub async fn put(&self, s: &Share) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO shares (id, engine, edge, domain, host, path, url, source, access, interstitial, \
             created_ms, live_ms, expires_ms, created_by, approved_by, state, stopped_ms, stopped_by) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(id) DO UPDATE SET live_ms = excluded.live_ms, expires_ms = excluded.expires_ms, \
             access = excluded.access, state = excluded.state, stopped_ms = excluded.stopped_ms, \
             stopped_by = excluded.stopped_by",
        )
        .bind(&s.id)
        .bind(&s.engine)
        .bind(&s.edge)
        .bind(&s.domain)
        .bind(&s.host)
        .bind(&s.path)
        .bind(&s.url)
        .bind(serde_json::to_string(&s.source)?)
        .bind(s.access.as_str())
        .bind(s.interstitial)
        .bind(s.created_ms as i64)
        .bind(s.live_ms.map(|v| v as i64))
        .bind(s.expires_ms.map(|v| v as i64))
        .bind(serde_json::to_string(&s.created_by)?)
        .bind(serde_json::to_string(&s.approved_by)?)
        .bind(s.state.as_str())
        .bind(s.stopped_ms.map(|v| v as i64))
        .bind(s.stopped_by.map(StoppedBy::as_str))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// One share.
    pub async fn get(&self, id: &str) -> anyhow::Result<Option<Share>> {
        let row = sqlx::query("SELECT * FROM shares WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        row.map(|r| decode(&r)).transpose()
    }

    /// Every share of the mesh not stopped more than 30 days ago, newest first.
    pub async fn list(&self, now_ms: u64) -> anyhow::Result<Vec<Share>> {
        let keep = now_ms.saturating_sub(30 * 86_400_000) as i64;
        let rows = sqlx::query(
            "SELECT * FROM shares WHERE stopped_ms IS NULL OR stopped_ms >= ? ORDER BY created_ms DESC",
        )
        .bind(keep)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(decode).collect()
    }

    /// Store a share's password hash (PHC string); never replicated.
    pub async fn put_secret(&self, share: &str, phc: &str) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO share_secrets (share, password_phc) VALUES (?, ?) \
             ON CONFLICT(share) DO UPDATE SET password_phc = excluded.password_phc",
        )
        .bind(share)
        .bind(phc)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// A share's password hash.
    pub async fn secret(&self, share: &str) -> anyhow::Result<Option<String>> {
        Ok(
            sqlx::query_scalar("SELECT password_phc FROM share_secrets WHERE share = ?")
                .bind(share)
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    /// Drop a share's password hash (stop, or a publish that failed after hashing).
    pub async fn delete_secret(&self, share: &str) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM share_secrets WHERE share = ?")
            .bind(share)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Record the approved lifetime of a share that is not live yet (M8).
    pub async fn put_request(&self, share: &str, expiry_ms: u64) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO share_requests (share, expiry_ms) VALUES (?, ?) \
             ON CONFLICT(share) DO UPDATE SET expiry_ms = excluded.expiry_ms",
        )
        .bind(share)
        .bind(expiry_ms as i64)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The approved lifetime of a pending share.
    pub async fn request_expiry(&self, share: &str) -> anyhow::Result<Option<u64>> {
        Ok(
            sqlx::query_scalar::<_, i64>("SELECT expiry_ms FROM share_requests WHERE share = ?")
                .bind(share)
                .fetch_optional(&self.pool)
                .await?
                .map(|v| v as u64),
        )
    }

    /// Forget a share's request once it is live or stopped.
    pub async fn delete_request(&self, share: &str) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM share_requests WHERE share = ?")
            .bind(share)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Record a dir share's root as approved (M1): canonical path, device, inode. Local only.
    pub async fn put_pin(&self, share: &str, pin: &DirPin) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO share_pins (share, canon, dev, ino) VALUES (?, ?, ?, ?) \
             ON CONFLICT(share) DO UPDATE SET canon = excluded.canon, dev = excluded.dev, \
             ino = excluded.ino",
        )
        .bind(share)
        .bind(pin.canon.to_string_lossy().into_owned())
        .bind(pin.dev() as i64)
        .bind(pin.ino() as i64)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// A dir share's stored pin.
    pub async fn pin(&self, share: &str) -> anyhow::Result<Option<DirPin>> {
        Ok(
            sqlx::query("SELECT canon, dev, ino FROM share_pins WHERE share = ?")
                .bind(share)
                .fetch_optional(&self.pool)
                .await?
                .map(|r| {
                    DirPin::from_parts(
                        r.get::<String, _>(0).into(),
                        r.get::<i64, _>(1) as u64,
                        r.get::<i64, _>(2) as u64,
                    )
                }),
        )
    }

    /// Forget a share's pin once it is stopped.
    pub async fn delete_pin(&self, share: &str) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM share_pins WHERE share = ?")
            .bind(share)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Record that this engine's consent path published `s` (FIX4 #1): its approval id and what
    /// was approved (host, path, source, access). Local only: never in [`REPLICATED`].
    pub async fn put_approval(&self, s: &Share, at_ms: u64) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO share_approvals (share, approval_id, binding, approved_ms) VALUES (?, ?, ?, ?) \
             ON CONFLICT(share) DO UPDATE SET approval_id = excluded.approval_id, \
             binding = excluded.binding, approved_ms = excluded.approved_ms",
        )
        .bind(&s.id)
        .bind(&s.approved_by.approval_id)
        .bind(binding(s)?)
        .bind(at_ms as i64)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// `s` is a share this engine's consent path recorded, unchanged since: same approval id,
    /// host, path, source and access. A row replicated from elsewhere (or altered there) is not.
    pub async fn locally_approved(&self, s: &Share) -> anyhow::Result<bool> {
        let row: Option<(String, String)> = sqlx::query_as(
            "SELECT approval_id, binding FROM share_approvals WHERE share = ?",
        )
        .bind(&s.id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.is_some_and(|(a, b)| a == s.approved_by.approval_id && b == binding(s).unwrap_or_default()))
    }

    /// Forget a share's local approval once it is stopped.
    pub async fn delete_approval(&self, share: &str) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM share_approvals WHERE share = ?")
            .bind(share)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn ms_setting(&self, key: &str) -> anyhow::Result<Option<u64>> {
        Ok(self.setting(key).await?.and_then(|v| v.parse().ok()))
    }

    /// The newest local Stop all.
    pub async fn stop_all_ms(&self) -> anyhow::Result<Option<u64>> {
        self.ms_setting(keys::STOP_ALL).await
    }

    /// Record a Stop all at `ms`, pending at the edge until [`Store::stop_all_sent`].
    pub async fn stop_all_at(&self, ms: u64) -> anyhow::Result<()> {
        self.set_setting(keys::STOP_ALL, &ms.to_string()).await?;
        self.set_setting(keys::STOP_ALL_PENDING, &ms.to_string())
            .await
    }

    /// A Stop all the edge has not confirmed.
    pub async fn stop_all_pending(&self) -> anyhow::Result<Option<u64>> {
        self.ms_setting(keys::STOP_ALL_PENDING).await
    }

    /// The edge confirmed the Stop all.
    pub async fn stop_all_sent(&self) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM settings WHERE key = ?")
            .bind(keys::STOP_ALL_PENDING)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

/// What the owner approved of a share, as stored in `share_approvals.binding`.
fn binding(s: &Share) -> anyhow::Result<String> {
    Ok(serde_json::to_string(&(
        &s.engine,
        &s.host,
        &s.path,
        &s.source,
        s.access.as_str(),
    ))?)
}

fn decode(r: &SqliteRow) -> anyhow::Result<Share> {
    let opt = |c: &str| -> anyhow::Result<Option<u64>> {
        Ok(r.try_get::<Option<i64>, _>(c)?.map(|v| v as u64))
    };
    let source: Source = serde_json::from_str(&r.try_get::<String, _>("source")?)?;
    let access = match r.try_get::<String, _>("access")?.as_str() {
        "password" => Access::Password,
        _ => Access::Public,
    };
    let state: String = r.try_get("state")?;
    Ok(Share {
        id: r.try_get("id")?,
        engine: r.try_get("engine")?,
        edge: r.try_get("edge")?,
        domain: r.try_get("domain")?,
        host: r.try_get("host")?,
        path: r.try_get("path")?,
        url: r.try_get("url")?,
        source,
        access,
        interstitial: r.try_get("interstitial")?,
        created_ms: r.try_get::<i64, _>("created_ms")? as u64,
        live_ms: opt("live_ms")?,
        expires_ms: opt("expires_ms")?,
        created_by: serde_json::from_str(&r.try_get::<String, _>("created_by")?)?,
        approved_by: serde_json::from_str(&r.try_get::<String, _>("approved_by")?)?,
        state: ShareState::parse(&state).with_context(|| format!("share state {state}"))?,
        stopped_ms: opt("stopped_ms")?,
        stopped_by: r
            .try_get::<Option<String>, _>("stopped_by")?
            .as_deref()
            .and_then(StoppedBy::parse),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ApprovedBy, CreatedBy};

    #[tokio::test]
    async fn switch_defaults_off_and_share_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let st = Store::open(&dir.path().join("share.sqlite")).await.unwrap();
        assert!(!st.enabled().await.unwrap());
        assert_eq!(st.edge().await.unwrap(), EdgeChoice::Default);
        st.set_enabled(true).await.unwrap();
        assert!(st.enabled().await.unwrap());
        let s = Share {
            id: "01".into(),
            engine: "e".into(),
            edge: "x".into(),
            domain: "tun.zip".into(),
            host: "app.sky.tun.zip".into(),
            path: "/".into(),
            url: "https://app.sky.tun.zip/".into(),
            source: Source::Port(5173),
            access: Access::Public,
            interstitial: true,
            created_ms: 5,
            live_ms: None,
            expires_ms: None,
            created_by: CreatedBy::Settings("settings".into()),
            approved_by: ApprovedBy {
                approval_id: "a".into(),
                surface: "desktop".into(),
            },
            state: ShareState::Pending,
            stopped_ms: None,
            stopped_by: None,
        };
        st.put(&s).await.unwrap();
        assert_eq!(st.get("01").await.unwrap().unwrap(), s);
        assert_eq!(st.list(10).await.unwrap().len(), 1);
    }
}
