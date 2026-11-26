//! `<data_dir>/link.sqlite` (69 decision 3): `devices` and `pending` as
//! replicated records. The roster store's shape (WAL, 4 conns, busy 5 s,
//! `sqlx::migrate!`). Reads are async; the in-memory registries in
//! [`crate::devices`] answer conn tasks synchronously and hand every mutation
//! to one ordered writer task ([`Store::write`]), so a rename queued after an
//! insert lands after it. `hlc` is left NULL: the replica stamps it at capture.

use std::path::Path;
use std::time::Duration;

use anyhow::Context as _;
use sqlx::Row as _;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};
use tokio::sync::mpsc;

use crate::devices::Row;
use crate::identity::{b64, unb64};

/// Sqlite file under `data_dir`.
pub const DB: &str = "link.sqlite";

/// The tables `replica::attach` takes.
pub const TABLES: &[&str] = &["devices", "pending"];

/// One `pending` row as stored (the PSK sealed).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingRow {
    /// base64url of 16 random bytes.
    pub id: String,
    /// `nonce ‖ XChaCha20-Poly1305(mesh key, aad = id, psk)`.
    pub psk_sealed: Vec<u8>,
    /// The harness name the QR shows.
    pub name: String,
    /// Unix ms.
    pub created_ms: u64,
    /// Unix ms; live while `now < expires_ms`.
    pub expires_ms: u64,
}

/// One queued mutation.
#[derive(Clone, Debug)]
pub enum Op {
    /// Upsert a device row (a pairing).
    PutDevice(Row),
    /// `revoked_ms = at_ms`: refused by every hub, the row stays.
    Revoke {
        /// `Device.id`.
        id: String,
        /// Unix ms.
        at_ms: u64,
    },
    /// `name = name`.
    Rename {
        /// `Device.id`.
        id: String,
        /// New display name.
        name: String,
    },
    /// `last_seen_ms = at_ms` (session close).
    TouchLastSeen {
        /// `Device.id`.
        id: String,
        /// Unix ms.
        at_ms: u64,
    },
    /// Insert a pending row.
    PutPending(PendingRow),
    /// Consumed or expired.
    DeletePending(String),
    /// Delete a device row (replicated as a row delete). [`crate::devices::Devices::delete`]
    /// revokes first, so a delete that loses to a newer column write leaves the row revoked.
    DeleteDevice(String),
    /// Housekeeping at `now_ms`: delete `pending` rows past `expires_ms` (whatever mesh key
    /// sealed them) and devices revoked [`crate::devices::REVOKED_TTL_MS`] ago or longer.
    Purge {
        /// Unix ms.
        now_ms: u64,
    },
}

/// Handle to `<data_dir>/link.sqlite`. Cheap to clone.
#[derive(Clone)]
pub struct Store {
    pool: SqlitePool,
    writer: mpsc::UnboundedSender<Op>,
}

impl Store {
    /// Open or create the file, run the migrations, start the writer task.
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
            .context("run migrations")?;
        let (writer, mut rx) = mpsc::unbounded_channel::<Op>();
        let p = pool.clone();
        tokio::spawn(async move {
            while let Some(op) = rx.recv().await {
                if let Err(e) = apply(&p, &op).await {
                    tracing::warn!("link.sqlite: {op:?}: {e:#}");
                }
            }
        });
        Ok(Self { pool, writer })
    }

    /// The underlying pool (`replica::attach` takes it).
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Queue one mutation; applied in order by the writer task.
    pub fn write(&self, op: Op) {
        let _ = self.writer.send(op);
    }

    /// Apply one mutation now (the writer task's path; tests and the import).
    pub async fn apply(&self, op: &Op) -> anyhow::Result<()> {
        apply(&self.pool, op).await
    }

    /// Every device row, revoked included, in pairing order.
    pub async fn devices(&self) -> anyhow::Result<Vec<Row>> {
        let rows = sqlx::query(
            "SELECT id, name, platform, static_pub, paired_ms, last_seen_ms, revoked_ms FROM devices
             ORDER BY paired_ms, id",
        )
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::with_capacity(rows.len());
        for r in &rows {
            let Some(key) = unb64(&r.get::<Option<String>, _>("static_pub").unwrap_or_default())
                .and_then(|v| <[u8; 32]>::try_from(v).ok())
            else {
                tracing::warn!(
                    "link.sqlite: devices.{}: bad static_pub; skipped",
                    r.get::<String, _>("id")
                );
                continue;
            };
            out.push(Row {
                id: r.get("id"),
                key,
                name: r.get::<Option<String>, _>("name").unwrap_or_default(),
                platform: r.get::<Option<String>, _>("platform").unwrap_or_default(),
                paired_at_ms: r.get::<Option<i64>, _>("paired_ms").unwrap_or_default() as u64,
                last_seen_ms: r.get::<Option<i64>, _>("last_seen_ms").unwrap_or_default() as u64,
                revoked_ms: r.get::<Option<i64>, _>("revoked_ms").map(|v| v as u64),
            });
        }
        Ok(out)
    }

    /// Every pending row (the caller filters expiry and opens the PSK).
    pub async fn pending(&self) -> anyhow::Result<Vec<PendingRow>> {
        let rows = sqlx::query(
            "SELECT id, psk_sealed, name, created_ms, expires_ms FROM pending ORDER BY created_ms",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .iter()
            .map(|r| PendingRow {
                id: r.get("id"),
                psk_sealed: unb64(&r.get::<Option<String>, _>("psk_sealed").unwrap_or_default())
                    .unwrap_or_default(),
                name: r.get::<Option<String>, _>("name").unwrap_or_default(),
                created_ms: r.get::<Option<i64>, _>("created_ms").unwrap_or_default() as u64,
                expires_ms: r.get::<Option<i64>, _>("expires_ms").unwrap_or_default() as u64,
            })
            .collect())
    }
}

async fn apply(pool: &SqlitePool, op: &Op) -> anyhow::Result<()> {
    match op {
        Op::PutDevice(d) => {
            sqlx::query(
                "INSERT INTO devices (id, name, platform, static_pub, paired_ms, last_seen_ms, revoked_ms)
                 VALUES (?, ?, ?, ?, ?, ?, ?)
                 ON CONFLICT(id) DO UPDATE SET name = excluded.name, platform = excluded.platform,
                 static_pub = excluded.static_pub, paired_ms = excluded.paired_ms,
                 last_seen_ms = excluded.last_seen_ms, revoked_ms = excluded.revoked_ms",
            )
            .bind(&d.id)
            .bind(&d.name)
            .bind(&d.platform)
            .bind(b64(&d.key))
            .bind(d.paired_at_ms as i64)
            .bind(d.last_seen_ms as i64)
            .bind(d.revoked_ms.map(|v| v as i64))
            .execute(pool)
            .await?;
        }
        Op::Revoke { id, at_ms } => {
            sqlx::query("UPDATE devices SET revoked_ms = ? WHERE id = ?")
                .bind(*at_ms as i64)
                .bind(id)
                .execute(pool)
                .await?;
        }
        Op::Rename { id, name } => {
            sqlx::query("UPDATE devices SET name = ? WHERE id = ?")
                .bind(name)
                .bind(id)
                .execute(pool)
                .await?;
        }
        Op::TouchLastSeen { id, at_ms } => {
            sqlx::query("UPDATE devices SET last_seen_ms = ? WHERE id = ?")
                .bind(*at_ms as i64)
                .bind(id)
                .execute(pool)
                .await?;
        }
        Op::PutPending(p) => {
            sqlx::query(
                "INSERT INTO pending (id, psk_sealed, name, created_ms, expires_ms) VALUES (?, ?, ?, ?, ?)
                 ON CONFLICT(id) DO UPDATE SET psk_sealed = excluded.psk_sealed, name = excluded.name,
                 created_ms = excluded.created_ms, expires_ms = excluded.expires_ms",
            )
            .bind(&p.id)
            .bind(b64(&p.psk_sealed))
            .bind(&p.name)
            .bind(p.created_ms as i64)
            .bind(p.expires_ms as i64)
            .execute(pool)
            .await?;
        }
        Op::DeletePending(id) => {
            sqlx::query("DELETE FROM pending WHERE id = ?")
                .bind(id)
                .execute(pool)
                .await?;
        }
        Op::DeleteDevice(id) => {
            sqlx::query("DELETE FROM devices WHERE id = ?")
                .bind(id)
                .execute(pool)
                .await?;
        }
        Op::Purge { now_ms } => {
            sqlx::query("DELETE FROM pending WHERE expires_ms <= ?")
                .bind(*now_ms as i64)
                .execute(pool)
                .await?;
            let cutoff = now_ms.saturating_sub(crate::devices::REVOKED_TTL_MS);
            sqlx::query("DELETE FROM devices WHERE revoked_ms IS NOT NULL AND revoked_ms <= ?")
                .bind(cutoff as i64)
                .execute(pool)
                .await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::device_id;

    /// Both tables satisfy `replica::attach`; a device round-trips with its
    /// key; revoke keeps the row; pending rows delete.
    #[tokio::test]
    async fn tables_attach_and_rows_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join(DB)).await.unwrap();
        let engine = replica::engine::Engine::open(&dir.path().join("replica.sqlite"))
            .await
            .unwrap();
        engine.attach(store.pool(), TABLES).await.unwrap();

        let key = [9u8; 32];
        let row = Row {
            id: device_id(&key),
            key,
            name: "iPhone".into(),
            platform: "ios".into(),
            paired_at_ms: 10,
            last_seen_ms: 0,
            revoked_ms: None,
        };
        store.apply(&Op::PutDevice(row.clone())).await.unwrap();
        assert_eq!(store.devices().await.unwrap(), std::slice::from_ref(&row));
        store
            .apply(&Op::Rename {
                id: row.id.clone(),
                name: "Phone".into(),
            })
            .await
            .unwrap();
        store
            .apply(&Op::TouchLastSeen {
                id: row.id.clone(),
                at_ms: 20,
            })
            .await
            .unwrap();
        store
            .apply(&Op::Revoke {
                id: row.id.clone(),
                at_ms: 30,
            })
            .await
            .unwrap();
        let got = store.devices().await.unwrap();
        assert_eq!(
            (got[0].name.as_str(), got[0].last_seen_ms, got[0].revoked_ms),
            ("Phone", 20, Some(30))
        );

        let p = PendingRow {
            id: "p1".into(),
            psk_sealed: vec![1, 2, 3],
            name: "Mac".into(),
            created_ms: 1,
            expires_ms: 2,
        };
        store.apply(&Op::PutPending(p.clone())).await.unwrap();
        assert_eq!(store.pending().await.unwrap(), std::slice::from_ref(&p));
        store.apply(&Op::DeletePending(p.id.clone())).await.unwrap();
        assert!(store.pending().await.unwrap().is_empty());
    }
}
