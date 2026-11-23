//! `share-edge.sqlite` (210 §5.8): sqlx over sqlite with the migrations in `migrations/`. Keys are iroh public
//! keys in their text form; times are Unix ms.

use std::path::Path;
use std::str::FromStr as _;

use anyhow::Context as _;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::{FromRow, SqlitePool};

/// The edge's database.
#[derive(Clone, Debug)]
pub struct Store {
    pool: SqlitePool,
}

/// A `names` row, the columns the control API reads.
#[derive(Debug, Clone, FromRow)]
pub struct NameRow {
    /// The label under `D`.
    pub name: String,
    /// `random` · `reserved`.
    pub kind: String,
    /// The holding mesh; while `released`, the previous owner.
    pub owner: Option<String>,
    /// `allocated` … `banned`.
    pub state: String,
    /// Unix ms.
    pub created_ms: i64,
    /// End of the cooldown after a release.
    pub reclaim_after_ms: Option<i64>,
    /// Issuing CA.
    pub ca: Option<String>,
    /// sha256 of the stored CSR's SPKI.
    pub csr_spki: Option<Vec<u8>>,
    /// Certificate expiry.
    pub not_after_ms: Option<i64>,
}

impl NameRow {
    /// Held (not released or banned).
    pub fn held(&self) -> bool {
        !matches!(self.state.as_str(), "released" | "banned")
    }
}

/// A `meshes` row.
#[derive(Debug, Clone, FromRow)]
pub struct MeshRow {
    /// Mesh id.
    pub mesh: String,
    /// Admitted with an invite.
    pub invited: i64,
    /// Set by a rekey.
    pub rekeyed_to: Option<String>,
    /// Last Stop all.
    pub stop_all_ms: Option<i64>,
}

/// A `mesh_engines` row.
#[derive(Debug, Clone, FromRow)]
pub struct EngineRow {
    /// Engine id.
    pub engine: String,
    /// Last `hello`.
    pub last_ms: i64,
    /// Revocation time.
    pub revoked_ms: Option<i64>,
}

/// Columns of a new or re-claimed name.
pub struct NewName<'a> {
    /// `D`.
    pub domain: &'a str,
    /// The label.
    pub name: &'a str,
    /// `random` · `reserved`.
    pub kind: &'a str,
    /// Mesh id.
    pub owner: &'a str,
    /// Unix ms.
    pub now_ms: i64,
    /// Client network.
    pub net: &'a str,
}

fn ms(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

impl Store {
    /// Open (creating) `path` and run the migrations.
    pub async fn open(path: &Path) -> anyhow::Result<Store> {
        let opts = SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))?
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(opts)
            .await
            .with_context(|| format!("open {}", path.display()))?;
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .context("migrate share-edge.sqlite")?;
        Ok(Store { pool })
    }

    /// The mesh row, if the edge has seen the mesh.
    pub async fn mesh(&self, mesh: &str) -> anyhow::Result<Option<MeshRow>> {
        Ok(sqlx::query_as::<_, MeshRow>(
            "SELECT mesh, invited, rekeyed_to, stop_all_ms FROM meshes WHERE mesh = ?",
        )
        .bind(mesh)
        .fetch_optional(&self.pool)
        .await?)
    }

    /// Record a `hello`: the mesh (first seen with `net`), the engine's first and last time.
    pub async fn attach(
        &self,
        mesh: &str,
        engine: &str,
        net: &str,
        invited: bool,
        now_ms: u64,
    ) -> anyhow::Result<()> {
        let now = ms(now_ms);
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO meshes (mesh, created_ms, net, invited) VALUES (?, ?, ?, ?)
             ON CONFLICT(mesh) DO UPDATE SET invited = MAX(invited, excluded.invited)",
        )
        .bind(mesh)
        .bind(now)
        .bind(net)
        .bind(i64::from(invited))
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO mesh_engines (mesh, engine, first_ms, last_ms, net) VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(mesh, engine) DO UPDATE SET last_ms = excluded.last_ms, net = excluded.net",
        )
        .bind(mesh)
        .bind(engine)
        .bind(now)
        .bind(now)
        .bind(net)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE names SET last_attach_ms = ? WHERE owner = ?
             AND state IN ('allocated','queued','slotted','valid')",
        )
        .bind(now)
        .bind(mesh)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// The revocation time of `engine` in `mesh`.
    pub async fn revoked(&self, mesh: &str, engine: &str) -> anyhow::Result<Option<i64>> {
        Ok(sqlx::query_scalar::<_, Option<i64>>(
            "SELECT revoked_ms FROM mesh_engines WHERE mesh = ? AND engine = ?",
        )
        .bind(mesh)
        .bind(engine)
        .fetch_optional(&self.pool)
        .await?
        .flatten())
    }

    /// Every engine row of `mesh`.
    pub async fn engines(&self, mesh: &str) -> anyhow::Result<Vec<EngineRow>> {
        Ok(sqlx::query_as::<_, EngineRow>(
            "SELECT engine, last_ms, revoked_ms FROM mesh_engines WHERE mesh = ? ORDER BY first_ms",
        )
        .bind(mesh)
        .fetch_all(&self.pool)
        .await?)
    }

    /// Append `engines` to the mesh's revoke list (§5.7).
    pub async fn revoke(&self, mesh: &str, engines: &[String], now_ms: u64) -> anyhow::Result<()> {
        let mut tx = self.pool.begin().await?;
        for e in engines {
            sqlx::query(
                "INSERT INTO mesh_engines (mesh, engine, first_ms, last_ms, net, revoked_ms)
                 VALUES (?, ?, ?, ?, '', ?)
                 ON CONFLICT(mesh, engine) DO UPDATE SET revoked_ms = COALESCE(revoked_ms, excluded.revoked_ms)",
            )
            .bind(mesh)
            .bind(e)
            .bind(ms(now_ms))
            .bind(ms(now_ms))
            .bind(ms(now_ms))
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Set the mesh's Stop all time.
    pub async fn stop_all(&self, mesh: &str, now_ms: u64) -> anyhow::Result<()> {
        sqlx::query("UPDATE meshes SET stop_all_ms = ? WHERE mesh = ?")
            .bind(ms(now_ms))
            .bind(mesh)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// A name row.
    pub async fn name(&self, domain: &str, name: &str) -> anyhow::Result<Option<NameRow>> {
        Ok(sqlx::query_as::<_, NameRow>(
            "SELECT name, kind, owner, state, created_ms, reclaim_after_ms, ca, csr_spki, not_after_ms
             FROM names WHERE domain = ? AND name = ?",
        )
        .bind(domain)
        .bind(name)
        .fetch_optional(&self.pool)
        .await?)
    }

    /// Names `mesh` holds under `domain`.
    pub async fn held_names(&self, domain: &str, mesh: &str) -> anyhow::Result<Vec<NameRow>> {
        Ok(sqlx::query_as::<_, NameRow>(
            "SELECT name, kind, owner, state, created_ms, reclaim_after_ms, ca, csr_spki, not_after_ms
             FROM names WHERE domain = ? AND owner = ? AND state IN ('allocated','queued','slotted','valid')
             ORDER BY created_ms",
        )
        .bind(domain)
        .bind(mesh)
        .fetch_all(&self.pool)
        .await?)
    }

    /// Every name row (admin `list names`).
    pub async fn all_names(&self) -> anyhow::Result<Vec<NameRow>> {
        Ok(sqlx::query_as::<_, NameRow>(
            "SELECT name, kind, owner, state, created_ms, reclaim_after_ms, ca, csr_spki, not_after_ms
             FROM names ORDER BY created_ms",
        )
        .fetch_all(&self.pool)
        .await?)
    }

    /// Every mesh id (admin `list meshes`).
    pub async fn all_meshes(&self) -> anyhow::Result<Vec<MeshRow>> {
        Ok(sqlx::query_as::<_, MeshRow>(
            "SELECT mesh, invited, rekeyed_to, stop_all_ms FROM meshes ORDER BY created_ms",
        )
        .fetch_all(&self.pool)
        .await?)
    }

    /// Insert or re-claim a name as `allocated` (lazy allocation, §5.2). Clears every per-holder column.
    pub async fn claim(&self, n: NewName<'_>) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO names (domain, name, kind, owner, state, created_ms, net)
             VALUES (?, ?, ?, ?, 'allocated', ?, ?)
             ON CONFLICT(domain, name) DO UPDATE SET kind = excluded.kind, owner = excluded.owner,
               state = 'allocated', created_ms = excluded.created_ms, net = excluded.net,
               last_attach_ms = NULL, first_live_ms = NULL, transfer_to = NULL, transfer_until_ms = NULL,
               released_ms = NULL, reclaim_after_ms = NULL, csr = NULL, csr_spki = NULL, ca = NULL,
               chain_pem = NULL, not_after_ms = NULL, ari_start_ms = NULL, ari_end_ms = NULL, banned = NULL",
        )
        .bind(n.domain)
        .bind(n.name)
        .bind(n.kind)
        .bind(n.owner)
        .bind(n.now_ms)
        .bind(n.net)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Mark a name released with its cooldown end. The owner column keeps the previous owner.
    pub async fn release(
        &self,
        domain: &str,
        name: &str,
        now_ms: u64,
        reclaim_after_ms: u64,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "UPDATE names SET state = 'released', released_ms = ?, reclaim_after_ms = ?, transfer_to = NULL,
               transfer_until_ms = NULL WHERE domain = ? AND name = ?",
        )
        .bind(ms(now_ms))
        .bind(ms(reclaim_after_ms))
        .bind(domain)
        .bind(name)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Consume one use of an invite; false when unknown, used up or expired.
    pub async fn consume_invite(&self, hash: &[u8], now_ms: u64) -> anyhow::Result<bool> {
        let r = sqlx::query(
            "UPDATE invites SET uses_left = uses_left - 1 WHERE hash = ? AND uses_left > 0 AND expires_ms > ?",
        )
        .bind(hash)
        .bind(ms(now_ms))
        .execute(&self.pool)
        .await?;
        Ok(r.rows_affected() == 1)
    }

    /// The reason of a ban of `kind` (`name`, `mesh`, `key`, `net`) on `value`.
    pub async fn ban_reason(&self, kind: &str, value: &str) -> anyhow::Result<Option<String>> {
        Ok(
            sqlx::query_scalar::<_, String>("SELECT reason FROM bans WHERE kind = ? AND value = ?")
                .bind(kind)
                .bind(value)
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    /// Add a ban; a name ban also marks the name row, a mesh ban the mesh row (§9.3).
    pub async fn ban(
        &self,
        kind: &str,
        value: &str,
        reason: &str,
        now_ms: u64,
    ) -> anyhow::Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO bans (kind, value, reason, ms, by) VALUES (?, ?, ?, ?, 'admin')
             ON CONFLICT(kind, value) DO UPDATE SET reason = excluded.reason, ms = excluded.ms",
        )
        .bind(kind)
        .bind(value)
        .bind(reason)
        .bind(ms(now_ms))
        .execute(&mut *tx)
        .await?;
        let mark = serde_json::json!({"reason": reason, "ms": now_ms, "by": "admin"}).to_string();
        match kind {
            "name" => {
                sqlx::query("UPDATE names SET banned = ? WHERE name = ?")
                    .bind(&mark)
                    .bind(value)
                    .execute(&mut *tx)
                    .await?;
            }
            "mesh" => {
                sqlx::query("UPDATE meshes SET banned = ? WHERE mesh = ?")
                    .bind(&mark)
                    .bind(value)
                    .execute(&mut *tx)
                    .await?;
            }
            _ => {}
        }
        tx.commit().await?;
        Ok(())
    }

    /// Remove a ban; false when there was none.
    pub async fn unban(&self, kind: &str, value: &str) -> anyhow::Result<bool> {
        let mut tx = self.pool.begin().await?;
        let r = sqlx::query("DELETE FROM bans WHERE kind = ? AND value = ?")
            .bind(kind)
            .bind(value)
            .execute(&mut *tx)
            .await?;
        match kind {
            "name" => {
                sqlx::query("UPDATE names SET banned = NULL WHERE name = ?")
                    .bind(value)
                    .execute(&mut *tx)
                    .await?;
            }
            "mesh" => {
                sqlx::query("UPDATE meshes SET banned = NULL WHERE mesh = ?")
                    .bind(value)
                    .execute(&mut *tx)
                    .await?;
            }
            _ => {}
        }
        tx.commit().await?;
        Ok(r.rows_affected() == 1)
    }

    /// Every ban as `(kind, value, reason)`.
    pub async fn bans(&self) -> anyhow::Result<Vec<(String, String, String)>> {
        Ok(sqlx::query_as::<_, (String, String, String)>(
            "SELECT kind, value, reason FROM bans ORDER BY ms",
        )
        .fetch_all(&self.pool)
        .await?)
    }

    /// Log a mutating admin command with the operator's reference (§8.5).
    pub async fn audit(
        &self,
        cmd: &str,
        args: &str,
        reference: Option<&str>,
        now_ms: u64,
    ) -> anyhow::Result<()> {
        sqlx::query("INSERT INTO audit (ms, cmd, args, ref) VALUES (?, ?, ?, ?)")
            .bind(ms(now_ms))
            .bind(cmd)
            .bind(args)
            .bind(reference)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// One visitor connection for abuse handling (§9.2). TODO(210-M3): the 30-day purge.
    #[allow(clippy::too_many_arguments)]
    pub async fn conn_log(
        &self,
        now_ms: u64,
        sni: &str,
        visitor: &str,
        mesh: &str,
        engine: &str,
        up: u64,
        down: u64,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO conn_log (ms, sni, visitor, mesh, engine, bytes_up, bytes_down) VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(ms(now_ms))
        .bind(sni)
        .bind(visitor)
        .bind(mesh)
        .bind(engine)
        .bind(ms(up))
        .bind(ms(down))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Close the pool.
    pub async fn close(&self) {
        self.pool.close().await;
    }
}
