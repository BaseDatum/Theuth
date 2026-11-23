//! `share-edge.sqlite` (210 §5.8): sqlx over sqlite with the migrations in `migrations/`. Keys are iroh public
//! keys in their text form; times are Unix ms.

use std::path::Path;
use std::str::FromStr as _;

use anyhow::Context as _;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteLockingMode, SqlitePoolOptions};
use sqlx::{FromRow, SqlitePool};

use crate::config::DbJournal;

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
    /// Pending online transfer to this mesh (§5.2, D20).
    pub transfer_to: Option<String>,
    /// The transfer offer lapses then.
    pub transfer_until_ms: Option<i64>,
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

/// `state, owner, reclaim_after_ms, banned, kind, transfer_to, transfer_until_ms` of a name, as
/// [`Store::claim`] reads it.
type ClaimRow = (
    String,
    Option<String>,
    Option<i64>,
    Option<String>,
    String,
    Option<String>,
    Option<i64>,
);

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

/// The outcome of [`Store::claim`] (§5.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Claimed {
    /// The name is now the mesh's.
    Granted,
    /// The mesh already holds it (idempotent).
    AlreadyHeld,
    /// Banned.
    Banned,
    /// Held by another mesh.
    Taken,
    /// Released, in its cooldown, and the claimer is not the previous owner; the cooldown's end.
    Cooldown(i64),
    /// The mesh's name quota is used up.
    Quota,
    /// Held by another mesh that offered it to the claimer (§5.2 online transfer, D20): moved with no cooldown,
    /// the old certificate queued for revocation (`cessationOfOperation`), the CSR cleared for the new key.
    Transferred,
}

/// The outcome of [`Store::rekey`] (§5.7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rekeyed {
    /// Applied: every name, engine row and job of `old` is `new`'s; `old.rekeyed_to = new`.
    Applied,
    /// `old` was already rekeyed away; the end of its chain (`409 mesh_rekeyed {to}`).
    Moved(String),
    /// The edge never saw `old`.
    Unknown,
    /// `old` or `new` is banned.
    Banned,
    /// `new` already holds names or was itself rekeyed away.
    NewInUse,
}

mod certs;
pub use certs::{CertRow, JobRow, RevocationRow};

/// Longest rekey chain followed (§5.7 rekeys are rare; a cycle cannot be written).
const MAX_CHAIN: usize = 64;

/// Follow `meshes.rekeyed_to` from `mesh` to the current id.
async fn chain_end(conn: &mut sqlx::SqliteConnection, mesh: &str) -> anyhow::Result<String> {
    let mut cur = mesh.to_string();
    for _ in 0..MAX_CHAIN {
        let next: Option<Option<String>> = sqlx::query_scalar("SELECT rekeyed_to FROM meshes WHERE mesh = ?")
            .bind(&cur)
            .fetch_optional(&mut *conn)
            .await?;
        match next.flatten() {
            Some(n) => cur = n,
            None => break,
        }
    }
    Ok(cur)
}

fn ms(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

impl Store {
    /// Open (creating) `path` with `journal` and run the migrations. [`DbJournal::Delete`] (NFS) also locks the
    /// file exclusively and keeps one connection: an exclusive lock held by one pooled connection would leave
    /// every other one `SQLITE_BUSY`.
    pub async fn open(path: &Path, journal: DbJournal) -> anyhow::Result<Store> {
        let opts = SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))?
            .create_if_missing(true)
            .foreign_keys(true);
        let (opts, max_connections) = match journal {
            DbJournal::Wal => (opts.journal_mode(SqliteJournalMode::Wal), 4),
            DbJournal::Delete => (
                opts.journal_mode(SqliteJournalMode::Delete)
                    .locking_mode(SqliteLockingMode::Exclusive),
                1,
            ),
        };
        let pool = SqlitePoolOptions::new()
            .max_connections(max_connections)
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

    /// A live tunnel of `mesh` counts as an attach: stamp its held names (renewal scan).
    pub async fn touch_attached(&self, mesh: &str, now_ms: u64) -> anyhow::Result<()> {
        sqlx::query(
            "UPDATE names SET last_attach_ms = ? WHERE owner = ?
             AND state IN ('allocated','queued','slotted','valid')",
        )
        .bind(ms(now_ms))
        .bind(mesh)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Names under `domain` that can serve: held and not banned (EDGE-K8S desired set).
    pub async fn serving_names(&self, domain: &str) -> anyhow::Result<Vec<String>> {
        Ok(sqlx::query_scalar::<_, String>(
            "SELECT name FROM names WHERE domain = ? AND banned IS NULL
             AND state IN ('allocated','queued','slotted','valid') ORDER BY name",
        )
        .bind(domain)
        .fetch_all(&self.pool)
        .await?)
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
            "SELECT name, kind, owner, state, created_ms, reclaim_after_ms, ca, csr_spki, not_after_ms,
               transfer_to, transfer_until_ms
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
            "SELECT name, kind, owner, state, created_ms, reclaim_after_ms, ca, csr_spki, not_after_ms,
               transfer_to, transfer_until_ms
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
            "SELECT name, kind, owner, state, created_ms, reclaim_after_ms, ca, csr_spki, not_after_ms,
               transfer_to, transfer_until_ms
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

    /// Claim a name for `n.owner` under the §5.2 rules, atomically (FIX-EDGE M5): one `BEGIN IMMEDIATE`
    /// transaction reads the row and the mesh's held names, checks ban, owner, cooldown and quota, then inserts
    /// a fresh row or re-claims a released one with a conditional upsert whose row count is checked. Two meshes
    /// claiming one name, or one mesh claiming two names past its quota, are serialized by SQLite's write lock.
    ///
    /// Quota: a random name only while the mesh holds no name; reserved names up to `reserved_max`. A transfer
    /// counts against the receiver's quota with the name's own kind.
    ///
    /// A name held by another mesh whose pending offer (`transfer_to`, unexpired) names the claimer moves to
    /// it ([`Claimed::Transferred`], §5.2 D20): no cooldown, the old chain to `revocations`
    /// (`cessationOfOperation`), the old mesh's open jobs dropped, the CSR cleared so the new mesh orders with
    /// its own key. A slotted or valid name stays `slotted`: same identifier set, no new-name slot (§6.3).
    ///
    /// A re-claim clears every per-holder column; a released name's chain already left for `revocations` in
    /// [`Store::release`]. The claim itself is an attach (it arrives over a live tunnel, after the `hello`), so
    /// `last_attach_ms` starts at the claim and the §6.3 "unattached for 90 days" rule counts from there.
    pub async fn claim(&self, n: NewName<'_>, reserved_max: u32) -> anyhow::Result<Claimed> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let row: Option<ClaimRow> = sqlx::query_as(
            "SELECT state, owner, reclaim_after_ms, banned, kind, transfer_to, transfer_until_ms
             FROM names WHERE domain = ? AND name = ?",
        )
        .bind(n.domain)
        .bind(n.name)
        .fetch_optional(&mut *tx)
        .await?;
        let mut transfer = None;
        if let Some((state, owner, reclaim_after, banned, kind, to, until)) = &row {
            let mine = owner.as_deref() == Some(n.owner);
            if state == "banned" || banned.is_some() {
                return Ok(Claimed::Banned);
            }
            if state != "released" {
                if mine {
                    return Ok(Claimed::AlreadyHeld);
                }
                let offered = to.as_deref() == Some(n.owner) && until.is_some_and(|u| u > n.now_ms);
                if !offered {
                    return Ok(Claimed::Taken);
                }
                transfer = Some((kind.clone(), owner.clone().unwrap_or_default()));
            } else if let Some(t) = *reclaim_after
                && t > n.now_ms
                && !mine
            {
                return Ok(Claimed::Cooldown(t));
            }
        }
        let kinds: Vec<String> = sqlx::query_scalar(
            "SELECT kind FROM names WHERE domain = ? AND owner = ?
             AND state IN ('allocated','queued','slotted','valid')",
        )
        .bind(n.domain)
        .bind(n.owner)
        .fetch_all(&mut *tx)
        .await?;
        let kind = transfer.as_ref().map_or(n.kind, |(k, _)| k.as_str());
        let over = if kind == "reserved" {
            kinds.iter().filter(|k| *k == "reserved").count() >= reserved_max as usize
        } else {
            !kinds.is_empty()
        };
        if over {
            return Ok(Claimed::Quota);
        }
        if let Some((_, from)) = &transfer {
            sqlx::query(
                "INSERT INTO revocations (domain, name, chain_pem, reason, next_at_ms, created_ms)
                 SELECT domain, name, chain_pem, 'cessation_of_operation', ?1, ?1 FROM names
                 WHERE domain = ?2 AND name = ?3 AND chain_pem IS NOT NULL",
            )
            .bind(n.now_ms)
            .bind(n.domain)
            .bind(n.name)
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "UPDATE cert_jobs SET state = 'dropped', lease_until_ms = NULL, last_error = 'transferred'
                 WHERE domain = ? AND name = ? AND state IN ('queued','ordering')",
            )
            .bind(n.domain)
            .bind(n.name)
            .execute(&mut *tx)
            .await?;
            let r = sqlx::query(
                "UPDATE names SET owner = ?1,
                   state = CASE WHEN state IN ('slotted','valid') THEN 'slotted' ELSE 'allocated' END,
                   last_attach_ms = ?2, first_live_ms = NULL, transfer_to = NULL, transfer_until_ms = NULL,
                   csr = NULL, csr_spki = NULL, chain_pem = NULL, not_after_ms = NULL, ari_start_ms = NULL,
                   ari_end_ms = NULL, rotate_compromised = 0
                 WHERE domain = ?3 AND name = ?4 AND owner = ?5 AND transfer_to = ?1 AND transfer_until_ms > ?2
                   AND banned IS NULL AND state IN ('allocated','queued','slotted','valid')",
            )
            .bind(n.owner)
            .bind(n.now_ms)
            .bind(n.domain)
            .bind(n.name)
            .bind(from)
            .execute(&mut *tx)
            .await?;
            anyhow::ensure!(r.rows_affected() == 1, "transfer of {} changed underneath", n.name);
            tx.commit().await?;
            return Ok(Claimed::Transferred);
        }
        let r = sqlx::query(
            "INSERT INTO names (domain, name, kind, owner, state, created_ms, net, last_attach_ms)
             VALUES (?1, ?2, ?3, ?4, 'allocated', ?5, ?6, ?5)
             ON CONFLICT(domain, name) DO UPDATE SET kind = excluded.kind, owner = excluded.owner,
               state = 'allocated', created_ms = excluded.created_ms, net = excluded.net,
               last_attach_ms = excluded.last_attach_ms, first_live_ms = NULL, transfer_to = NULL,
               transfer_until_ms = NULL, released_ms = NULL, reclaim_after_ms = NULL, csr = NULL, csr_spki = NULL,
               ca = NULL, chain_pem = NULL, not_after_ms = NULL, ari_start_ms = NULL, ari_end_ms = NULL,
               rotate_compromised = 0, banned = NULL
             WHERE names.state = 'released' AND names.banned IS NULL
               AND (names.reclaim_after_ms IS NULL OR names.reclaim_after_ms <= ?5 OR names.owner = ?4)",
        )
        .bind(n.domain)
        .bind(n.name)
        .bind(n.kind)
        .bind(n.owner)
        .bind(n.now_ms)
        .bind(n.net)
        .execute(&mut *tx)
        .await?;
        anyhow::ensure!(r.rows_affected() == 1, "claim of {} changed underneath", n.name);
        tx.commit().await?;
        Ok(Claimed::Granted)
    }

    /// Release a name `owner` holds: `released` with `reclaim_after_ms = max(now + cooldown, not_after)`; the
    /// owner column keeps the previous owner. In the same transaction the chain moves to `revocations`
    /// (`cessationOfOperation`), so a re-claim cannot wipe it unrevoked (FIX-EDGE M5/M6). `None` when the
    /// mesh does not hold the name.
    pub async fn release(
        &self,
        domain: &str,
        name: &str,
        owner: &str,
        now_ms: u64,
        cooldown_ms: u64,
    ) -> anyhow::Result<Option<u64>> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let at: Option<i64> = sqlx::query_scalar(
            "UPDATE names SET state = 'released', released_ms = ?1,
               reclaim_after_ms = MAX(?2, COALESCE(not_after_ms, 0)), transfer_to = NULL, transfer_until_ms = NULL
             WHERE domain = ?3 AND name = ?4 AND owner = ?5 AND state IN ('allocated','queued','slotted','valid')
               AND banned IS NULL
             RETURNING reclaim_after_ms",
        )
        .bind(ms(now_ms))
        .bind(ms(now_ms.saturating_add(cooldown_ms)))
        .bind(domain)
        .bind(name)
        .bind(owner)
        .fetch_optional(&mut *tx)
        .await?;
        if at.is_some() {
            certs::move_chain_to_revocations(&mut tx, domain, name, "cessation_of_operation", now_ms).await?;
        }
        tx.commit().await?;
        Ok(at.map(|v| u64::try_from(v).unwrap_or(0)))
    }

    /// Offer a held name to `to` until `until_ms` (§5.2 D20); a new offer replaces the previous one. False when
    /// `owner` does not hold the name or it is banned.
    pub async fn offer_transfer(
        &self,
        domain: &str,
        name: &str,
        owner: &str,
        to: &str,
        until_ms: u64,
    ) -> anyhow::Result<bool> {
        let r = sqlx::query(
            "UPDATE names SET transfer_to = ?, transfer_until_ms = ?
             WHERE domain = ? AND name = ? AND owner = ? AND banned IS NULL
               AND state IN ('allocated','queued','slotted','valid')",
        )
        .bind(to)
        .bind(ms(until_ms))
        .bind(domain)
        .bind(name)
        .bind(owner)
        .execute(&self.pool)
        .await?;
        Ok(r.rows_affected() == 1)
    }

    /// The end of `mesh`'s rekey chain (`mesh` itself when never rekeyed). Bounded: a cycle cannot be written
    /// ([`Store::rekey`] refuses a `new` that was rekeyed away), the bound only guards a hand-edited database.
    pub async fn current_mesh(&self, mesh: &str) -> anyhow::Result<String> {
        let mut conn = self.pool.acquire().await?;
        chain_end(&mut conn, mesh).await
    }

    /// The applied rekeys that lead to `mesh`, oldest first, as `(old, new, ms)`.
    pub async fn rekey_history(&self, mesh: &str) -> anyhow::Result<Vec<(String, String, i64)>> {
        let mut out = Vec::new();
        let mut cur = mesh.to_string();
        for _ in 0..MAX_CHAIN {
            let Some(row) = sqlx::query_as::<_, (String, String, i64)>(
                "SELECT old, new, ms FROM mesh_rekeys WHERE new = ?",
            )
            .bind(&cur)
            .fetch_optional(&self.pool)
            .await?
            else {
                break;
            };
            cur.clone_from(&row.0);
            out.push(row);
        }
        out.reverse();
        Ok(out)
    }

    /// Apply a verified rekey `old → new` atomically (§5.7): first valid rekey wins (`old.rekeyed_to` is set
    /// only while NULL, under `BEGIN IMMEDIATE`); every name (held or in cooldown), pending transfer offer and
    /// open job of `old` becomes `new`'s; the engine history is copied and `revoke` *replaces* the revoke list
    /// (engines not in it are un-revoked under `new`, those in it keep their earlier revocation time or get
    /// `now`); `old` gets `rekeyed_to`/`rekeyed_ms`, the history a row.
    pub async fn rekey(&self, old: &str, new: &str, revoke: &[String], now_ms: u64) -> anyhow::Result<Rekeyed> {
        let now = ms(now_ms);
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let row: Option<(Option<String>, Option<String>)> =
            sqlx::query_as("SELECT rekeyed_to, banned FROM meshes WHERE mesh = ?")
                .bind(old)
                .fetch_optional(&mut *tx)
                .await?;
        let Some((rekeyed_to, banned)) = row else {
            return Ok(Rekeyed::Unknown);
        };
        if rekeyed_to.is_some() {
            return Ok(Rekeyed::Moved(chain_end(&mut tx, old).await?));
        }
        let bans: i64 = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM bans WHERE kind = 'mesh' AND value IN (?1, ?2)
             UNION ALL SELECT COUNT(*) FROM meshes WHERE mesh = ?2 AND banned IS NOT NULL",
        )
        .bind(old)
        .bind(new)
        .fetch_all(&mut *tx)
        .await?
        .into_iter()
        .sum();
        if banned.is_some() || bans > 0 {
            return Ok(Rekeyed::Banned);
        }
        let in_use: i64 = sqlx::query_scalar(
            "SELECT (SELECT COUNT(*) FROM meshes WHERE mesh = ?1 AND rekeyed_to IS NOT NULL)
                  + (SELECT COUNT(*) FROM names WHERE owner = ?1)",
        )
        .bind(new)
        .fetch_one(&mut *tx)
        .await?;
        if in_use > 0 {
            return Ok(Rekeyed::NewInUse);
        }
        sqlx::query(
            "INSERT INTO meshes (mesh, created_ms, net, asn, invited, stop_all_ms)
             SELECT ?1, created_ms, net, asn, invited, stop_all_ms FROM meshes WHERE mesh = ?2
             ON CONFLICT(mesh) DO UPDATE SET invited = MAX(meshes.invited, excluded.invited),
               stop_all_ms = COALESCE(excluded.stop_all_ms, meshes.stop_all_ms)",
        )
        .bind(new)
        .bind(old)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO mesh_engines (mesh, engine, first_ms, last_ms, net, revoked_ms)
             SELECT ?1, engine, first_ms, last_ms, net, revoked_ms FROM mesh_engines WHERE mesh = ?2
             ON CONFLICT(mesh, engine) DO NOTHING",
        )
        .bind(new)
        .bind(old)
        .execute(&mut *tx)
        .await?;
        let engines: Vec<String> = sqlx::query_scalar("SELECT engine FROM mesh_engines WHERE mesh = ?")
            .bind(new)
            .fetch_all(&mut *tx)
            .await?;
        for e in engines.iter().filter(|e| !revoke.contains(e)) {
            sqlx::query("UPDATE mesh_engines SET revoked_ms = NULL WHERE mesh = ? AND engine = ?")
                .bind(new)
                .bind(e)
                .execute(&mut *tx)
                .await?;
        }
        for e in revoke {
            sqlx::query(
                "INSERT INTO mesh_engines (mesh, engine, first_ms, last_ms, net, revoked_ms)
                 VALUES (?1, ?2, ?3, ?3, '', ?3)
                 ON CONFLICT(mesh, engine) DO UPDATE SET revoked_ms = COALESCE(revoked_ms, excluded.revoked_ms)",
            )
            .bind(new)
            .bind(e)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        }
        for q in [
            "UPDATE names SET owner = ?1 WHERE owner = ?2",
            "UPDATE names SET transfer_to = ?1 WHERE transfer_to = ?2",
            "UPDATE cert_jobs SET mesh = ?1 WHERE mesh = ?2 AND state IN ('queued','ordering')",
        ] {
            sqlx::query(q).bind(new).bind(old).execute(&mut *tx).await?;
        }
        let r = sqlx::query(
            "UPDATE meshes SET rekeyed_to = ?1, rekeyed_ms = ?2 WHERE mesh = ?3 AND rekeyed_to IS NULL",
        )
        .bind(new)
        .bind(now)
        .bind(old)
        .execute(&mut *tx)
        .await?;
        anyhow::ensure!(r.rows_affected() == 1, "rekey of {old} changed underneath");
        sqlx::query("INSERT INTO mesh_rekeys (old, new, ms) VALUES (?, ?, ?)")
            .bind(old)
            .bind(new)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(Rekeyed::Applied)
    }

    /// Mark (or clear) the pending rotation of a name as replacing a compromised key (§6.3).
    pub async fn set_rotate_compromised(&self, domain: &str, name: &str, on: bool) -> anyhow::Result<()> {
        sqlx::query("UPDATE names SET rotate_compromised = ? WHERE domain = ? AND name = ?")
            .bind(i64::from(on))
            .bind(domain)
            .bind(name)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Whether the pending rotation of a name replaces a compromised key.
    pub async fn rotate_compromised(&self, domain: &str, name: &str) -> anyhow::Result<bool> {
        Ok(sqlx::query_scalar::<_, i64>(
            "SELECT rotate_compromised FROM names WHERE domain = ? AND name = ?",
        )
        .bind(domain)
        .bind(name)
        .fetch_optional(&self.pool)
        .await?
        .is_some_and(|v| v != 0))
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
