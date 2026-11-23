//! Certificate columns, `cert_jobs`, the `dns_records` ledger and `buckets` (210 §5.8, §6.2–§6.5).

use sqlx::{FromRow, SqliteConnection};

use super::{Store, ms};

// Macros, not consts: sqlx takes only `'static` SQL, and `concat!` needs literals.
macro_rules! cert_cols {
    () => {
        "name, owner, state, csr, csr_spki, ca, chain_pem, not_after_ms, ari_start_ms, ari_end_ms, records, \
         last_attach_ms, net, banned"
    };
}
macro_rules! job_cols {
    () => {
        "id, domain, name, kind, state, class, mesh, net, first_of_mesh, enqueued_ms, attempts, next_at_ms, \
         last_error"
    };
}

/// The certificate columns of a `names` row.
#[derive(Debug, Clone, FromRow)]
pub struct CertRow {
    /// The label under `D`.
    pub name: String,
    /// The holding mesh.
    pub owner: Option<String>,
    /// `allocated` … `banned`.
    pub state: String,
    /// The stored CSR (DER).
    pub csr: Option<Vec<u8>>,
    /// sha256 of its SPKI.
    pub csr_spki: Option<Vec<u8>>,
    /// Issuing CA.
    pub ca: Option<String>,
    /// The current chain.
    pub chain_pem: Option<String>,
    /// Its expiry.
    pub not_after_ms: Option<i64>,
    /// ARI window start.
    pub ari_start_ms: Option<i64>,
    /// ARI window end.
    pub ari_end_ms: Option<i64>,
    /// The two D17 records exist.
    pub records: i64,
    /// Last `hello` of an engine of the owner.
    pub last_attach_ms: Option<i64>,
    /// Client network at claim.
    pub net: String,
    /// Ban mark.
    pub banned: Option<String>,
}

impl CertRow {
    /// Held (not released or banned).
    pub fn held(&self) -> bool {
        !matches!(self.state.as_str(), "released" | "banned") && self.banned.is_none()
    }
}

/// A `cert_jobs` row.
#[derive(Debug, Clone, FromRow)]
pub struct JobRow {
    /// uuid v7.
    pub id: String,
    /// `D`.
    pub domain: String,
    /// The name.
    pub name: String,
    /// `new` · `renew` · `rotate`.
    pub kind: String,
    /// `queued` · `ordering` · `done` · `failed` · `dropped`.
    pub state: String,
    /// `invited` · `open`.
    pub class: String,
    /// The mesh holding the name at enqueue.
    pub mesh: String,
    /// Its client network.
    pub net: String,
    /// The mesh's first name.
    pub first_of_mesh: i64,
    /// Unix ms.
    pub enqueued_ms: i64,
    /// Attempts so far.
    pub attempts: i64,
    /// Not before.
    pub next_at_ms: i64,
    /// The last attempt's error.
    pub last_error: Option<String>,
}

/// A `revocations` row (FIX-EDGE M6).
#[derive(Debug, Clone, FromRow)]
pub struct RevocationRow {
    /// Row id.
    pub id: i64,
    /// The name the chain was for.
    pub name: String,
    /// The chain to revoke.
    pub chain_pem: String,
    /// `cessation_of_operation` · `superseded` · `key_compromise`.
    pub reason: String,
    /// `queued` · `done` · `failed`.
    pub state: String,
    /// Attempts so far.
    pub attempts: i64,
    /// The last attempt's error.
    pub last_error: Option<String>,
}

/// Inside a transaction: queue the chain (if any) of a released or banned name for revocation and clear it from
/// the row. A held name (re-claimed meanwhile) is left alone: its chain is the new holder's.
pub(super) async fn move_chain_to_revocations(
    conn: &mut SqliteConnection,
    domain: &str,
    name: &str,
    reason: &str,
    now_ms: u64,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO revocations (domain, name, chain_pem, reason, next_at_ms, created_ms)
         SELECT domain, name, chain_pem, ?1, ?2, ?2 FROM names
         WHERE domain = ?3 AND name = ?4 AND chain_pem IS NOT NULL
           AND (state IN ('released','banned') OR banned IS NOT NULL)",
    )
    .bind(reason)
    .bind(ms(now_ms))
    .bind(domain)
    .bind(name)
    .execute(&mut *conn)
    .await?;
    sqlx::query(
        "UPDATE names SET chain_pem = NULL, ari_start_ms = NULL, ari_end_ms = NULL WHERE domain = ? AND name = ?
           AND (state IN ('released','banned') OR banned IS NOT NULL)",
    )
    .bind(domain)
    .bind(name)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

impl Store {
    /// The certificate columns of a name.
    pub async fn cert_row(&self, domain: &str, name: &str) -> anyhow::Result<Option<CertRow>> {
        Ok(sqlx::query_as::<_, CertRow>(concat!(
            "SELECT ",
            cert_cols!(),
            " FROM names WHERE domain = ? AND name = ?"
        ))
        .bind(domain)
        .bind(name)
        .fetch_optional(&self.pool)
        .await?)
    }

    /// Names in `valid` with a chain (renewal scan).
    pub async fn valid_names(&self, domain: &str) -> anyhow::Result<Vec<CertRow>> {
        Ok(sqlx::query_as::<_, CertRow>(concat!(
            "SELECT ",
            cert_cols!(),
            " FROM names WHERE domain = ? AND state = 'valid' AND chain_pem IS NOT NULL"
        ))
        .bind(domain)
        .fetch_all(&self.pool)
        .await?)
    }

    /// Store a name's CSR and its SPKI hash; `state` moves the name too.
    pub async fn set_csr(
        &self,
        domain: &str,
        name: &str,
        csr: &[u8],
        spki: &[u8],
        state: Option<&str>,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "UPDATE names SET csr = ?, csr_spki = ?, state = COALESCE(?, state) WHERE domain = ? AND name = ?",
        )
        .bind(csr)
        .bind(spki)
        .bind(state)
        .bind(domain)
        .bind(name)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Store the first CSR of a name, only while it has none (P0-6: two machines of one mesh posting different
    /// CSRs in one sync gap; the loser gets `csr_mismatch`). True when this call stored it.
    pub async fn set_first_csr(
        &self,
        domain: &str,
        name: &str,
        csr: &[u8],
        spki: &[u8],
        state: Option<&str>,
    ) -> anyhow::Result<bool> {
        let r = sqlx::query(
            "UPDATE names SET csr = ?, csr_spki = ?, state = COALESCE(?, state)
             WHERE domain = ? AND name = ? AND csr IS NULL",
        )
        .bind(csr)
        .bind(spki)
        .bind(state)
        .bind(domain)
        .bind(name)
        .execute(&self.pool)
        .await?;
        Ok(r.rows_affected() == 1)
    }

    /// Move a held name to `state` (no-op once released or banned).
    pub async fn set_name_state(&self, domain: &str, name: &str, state: &str) -> anyhow::Result<()> {
        sqlx::query(
            "UPDATE names SET state = ? WHERE domain = ? AND name = ?
             AND state IN ('allocated','queued','slotted','valid')",
        )
        .bind(state)
        .bind(domain)
        .bind(name)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Whether the name's two D17 records exist.
    pub async fn set_records(&self, domain: &str, name: &str, on: bool) -> anyhow::Result<()> {
        sqlx::query("UPDATE names SET records = ? WHERE domain = ? AND name = ?")
            .bind(i64::from(on))
            .bind(domain)
            .bind(name)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// A new chain: `valid`, with its CA, expiry and ARI window; only while `owner` still holds the name with
    /// the CSR whose SPKI hash is `spki` (FIX-EDGE M6: a name released and re-claimed mid-order must not get
    /// the old owner's chain). `false` when nothing was stored; the caller revokes the orphan leaf.
    #[allow(clippy::too_many_arguments)]
    pub async fn store_chain(
        &self,
        domain: &str,
        name: &str,
        owner: &str,
        spki: &[u8],
        ca: &str,
        chain_pem: &str,
        not_after_ms: u64,
        ari: Option<(u64, u64)>,
    ) -> anyhow::Result<bool> {
        let r = sqlx::query(
            "UPDATE names SET state = 'valid', ca = ?, chain_pem = ?, not_after_ms = ?, ari_start_ms = ?,
               ari_end_ms = ?
             WHERE domain = ? AND name = ? AND owner = ? AND csr_spki = ? AND banned IS NULL
               AND state IN ('allocated','queued','slotted','valid')",
        )
        .bind(ca)
        .bind(chain_pem)
        .bind(ms(not_after_ms))
        .bind(ari.map(|a| ms(a.0)))
        .bind(ari.map(|a| ms(a.1)))
        .bind(domain)
        .bind(name)
        .bind(owner)
        .bind(spki)
        .execute(&self.pool)
        .await?;
        Ok(r.rows_affected() == 1)
    }

    /// The ARI window of a name.
    pub async fn set_ari(&self, domain: &str, name: &str, ari: (u64, u64)) -> anyhow::Result<()> {
        sqlx::query("UPDATE names SET ari_start_ms = ?, ari_end_ms = ? WHERE domain = ? AND name = ?")
            .bind(ms(ari.0))
            .bind(ms(ari.1))
            .bind(domain)
            .bind(name)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Move a name's chain (if any) to the durable revocation queue and forget it (ban, retire). The cooldown
    /// already used its `not_after_ms`, which stays.
    pub async fn retire_chain(&self, domain: &str, name: &str, reason: &str, now_ms: u64) -> anyhow::Result<()> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        move_chain_to_revocations(&mut tx, domain, name, reason, now_ms).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Queue the revocation of one chain that is in no row (a superseded or orphaned leaf).
    pub async fn queue_revocation(
        &self,
        domain: &str,
        name: &str,
        chain_pem: &str,
        reason: &str,
        now_ms: u64,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO revocations (domain, name, chain_pem, reason, next_at_ms, created_ms) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(domain)
        .bind(name)
        .bind(chain_pem)
        .bind(reason)
        .bind(ms(now_ms))
        .bind(ms(now_ms))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Queued revocations due at `now_ms`, oldest first.
    pub async fn due_revocations(&self, now_ms: u64) -> anyhow::Result<Vec<RevocationRow>> {
        Ok(sqlx::query_as::<_, RevocationRow>(
            "SELECT id, name, chain_pem, reason, state, attempts, last_error FROM revocations
             WHERE state = 'queued' AND next_at_ms <= ? ORDER BY next_at_ms, id",
        )
        .bind(ms(now_ms))
        .fetch_all(&self.pool)
        .await?)
    }

    /// Every revocation of a name, oldest first.
    pub async fn revocations(&self, domain: &str, name: &str) -> anyhow::Result<Vec<RevocationRow>> {
        Ok(sqlx::query_as::<_, RevocationRow>(
            "SELECT id, name, chain_pem, reason, state, attempts, last_error FROM revocations
             WHERE domain = ? AND name = ? ORDER BY id",
        )
        .bind(domain)
        .bind(name)
        .fetch_all(&self.pool)
        .await?)
    }

    /// Record one revocation attempt: `done`, `failed` (given up), or back to `queued` at `next_at_ms`.
    pub async fn revocation_attempt(
        &self,
        id: i64,
        state: &str,
        next_at_ms: u64,
        error: Option<&str>,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "UPDATE revocations SET state = ?, attempts = attempts + 1, next_at_ms = ?, last_error = ? WHERE id = ?",
        )
        .bind(state)
        .bind(ms(next_at_ms))
        .bind(error)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The queued or running job of a name.
    pub async fn open_job(&self, domain: &str, name: &str) -> anyhow::Result<Option<JobRow>> {
        Ok(sqlx::query_as::<_, JobRow>(concat!(
            "SELECT ",
            job_cols!(),
            " FROM cert_jobs WHERE domain = ? AND name = ? AND state IN ('queued','ordering')
             ORDER BY enqueued_ms LIMIT 1"
        ))
        .bind(domain)
        .bind(name)
        .fetch_optional(&self.pool)
        .await?)
    }

    /// Every job of a name, oldest first.
    pub async fn jobs(&self, domain: &str, name: &str) -> anyhow::Result<Vec<JobRow>> {
        Ok(sqlx::query_as::<_, JobRow>(concat!(
            "SELECT ",
            job_cols!(),
            " FROM cert_jobs WHERE domain = ? AND name = ? ORDER BY enqueued_ms, id"
        ))
        .bind(domain)
        .bind(name)
        .fetch_all(&self.pool)
        .await?)
    }

    /// Queue a job (its `state`, `attempts` and `last_error` are ignored).
    pub async fn enqueue_job(&self, j: &JobRow) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO cert_jobs (id, domain, name, kind, state, class, mesh, net, first_of_mesh, enqueued_ms,
               attempts, next_at_ms) VALUES (?, ?, ?, ?, 'queued', ?, ?, ?, ?, ?, 0, ?)",
        )
        .bind(&j.id)
        .bind(&j.domain)
        .bind(&j.name)
        .bind(&j.kind)
        .bind(&j.class)
        .bind(&j.mesh)
        .bind(&j.net)
        .bind(j.first_of_mesh)
        .bind(j.enqueued_ms)
        .bind(j.next_at_ms)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Lease the next due job: `queued` and due, or `ordering` with a lapsed lease (a crashed attempt); never a
    /// job of a name whose other job holds a live lease (one order per name at a time).
    pub async fn lease_job(&self, now_ms: u64, lease_until_ms: u64) -> anyhow::Result<Option<JobRow>> {
        let now = ms(now_ms);
        let mut tx = self.pool.begin().await?;
        let job = sqlx::query_as::<_, JobRow>(concat!(
            "SELECT ",
            job_cols!(),
            " FROM cert_jobs j
             WHERE ((state = 'queued' AND next_at_ms <= ?1) OR (state = 'ordering' AND lease_until_ms < ?1))
               AND NOT EXISTS (SELECT 1 FROM cert_jobs o WHERE o.domain = j.domain AND o.name = j.name
                 AND o.id != j.id AND o.state = 'ordering' AND o.lease_until_ms >= ?1)
             ORDER BY next_at_ms, enqueued_ms LIMIT 1"
        ))
        .bind(now)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(j) = &job {
            sqlx::query(
                "UPDATE cert_jobs SET state = 'ordering', lease_until_ms = ?, attempts = attempts + 1 WHERE id = ?",
            )
            .bind(ms(lease_until_ms))
            .bind(&j.id)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(job.map(|mut j| {
            j.attempts += 1;
            j.state = "ordering".into();
            j
        }))
    }

    /// End a job: `done`, `failed` or `dropped`.
    pub async fn finish_job(&self, id: &str, state: &str, error: Option<&str>) -> anyhow::Result<()> {
        sqlx::query("UPDATE cert_jobs SET state = ?, lease_until_ms = NULL, last_error = ? WHERE id = ?")
            .bind(state)
            .bind(error)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Put a job back in the queue at `next_at_ms`; `refund` undoes the lease's attempt count (waiting for a
    /// slot is not a failed attempt).
    pub async fn requeue_job(
        &self,
        id: &str,
        next_at_ms: u64,
        error: Option<&str>,
        refund: bool,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "UPDATE cert_jobs SET state = 'queued', lease_until_ms = NULL, next_at_ms = ?,
               last_error = COALESCE(?, last_error), attempts = attempts - ? WHERE id = ?",
        )
        .bind(ms(next_at_ms))
        .bind(error)
        .bind(i64::from(refund))
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// `cert retry`: make a `queued` job due at `now_ms` with its attempt count reset (so the backoff and the
    /// `max_attempts` budget start over). False when the job is no longer queued.
    pub async fn retry_job(&self, id: &str, now_ms: u64) -> anyhow::Result<bool> {
        let r = sqlx::query("UPDATE cert_jobs SET next_at_ms = ?, attempts = 0 WHERE id = ? AND state = 'queued'")
            .bind(ms(now_ms))
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected() == 1)
    }

    /// Drop every open job of a name (release, ban).
    pub async fn drop_jobs(&self, domain: &str, name: &str) -> anyhow::Result<()> {
        sqlx::query(
            "UPDATE cert_jobs SET state = 'dropped', lease_until_ms = NULL
             WHERE domain = ? AND name = ? AND state IN ('queued','ordering')",
        )
        .bind(domain)
        .bind(name)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// At start (one runner per edge) every `ordering` lease belongs to a dead process.
    pub async fn reset_leases(&self) -> anyhow::Result<()> {
        sqlx::query("UPDATE cert_jobs SET state = 'queued', lease_until_ms = NULL WHERE state = 'ordering'")
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// 1-based position of a queued `new` job among the queued `new` jobs.
    pub async fn queue_position(&self, id: &str) -> anyhow::Result<u32> {
        let n: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM cert_jobs WHERE state = 'queued' AND kind = 'new'
             AND enqueued_ms <= (SELECT enqueued_ms FROM cert_jobs WHERE id = ?)",
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await?;
        Ok(u32::try_from(n.max(1)).unwrap_or(u32::MAX))
    }

    /// Ledger first, record second (§6.4): a row here for every record the edge may have written.
    pub async fn dns_add(
        &self,
        host: &str,
        rtype: &str,
        value: &str,
        name: &str,
        now_ms: u64,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO dns_records (host, type, value, name, created_ms) VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(host, type, value) DO NOTHING",
        )
        .bind(host)
        .bind(rtype)
        .bind(value)
        .bind(name)
        .bind(ms(now_ms))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Forget every ledger row of `host`/`rtype` (after a read-back confirmed the delete).
    pub async fn dns_del(&self, host: &str, rtype: &str) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM dns_records WHERE host = ? AND type = ?")
            .bind(host)
            .bind(rtype)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// The whole ledger as `(host, type, value, name)`.
    pub async fn dns_all(&self) -> anyhow::Result<Vec<(String, String, String, Option<String>)>> {
        Ok(sqlx::query_as::<_, (String, String, String, Option<String>)>(
            "SELECT host, type, value, name FROM dns_records ORDER BY created_ms",
        )
        .fetch_all(&self.pool)
        .await?)
    }

    /// Take one token from bucket `name` (capacity `cap`, one token per `refill_ms`, full at first use);
    /// `Err(wait_ms)` until the next token when empty.
    pub async fn bucket_take(
        &self,
        name: &str,
        cap: f64,
        refill_ms: f64,
        now_ms: u64,
    ) -> anyhow::Result<Result<(), u64>> {
        let mut tx = self.pool.begin().await?;
        let row: Option<(f64, i64)> =
            sqlx::query_as("SELECT tokens, updated_ms FROM buckets WHERE name = ?")
                .bind(name)
                .fetch_optional(&mut *tx)
                .await?;
        let now = ms(now_ms);
        #[allow(clippy::cast_precision_loss)]
        let tokens = match row {
            None => cap,
            Some((t, at)) => (t + (now - at).max(0) as f64 / refill_ms).min(cap),
        };
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let (left, out) = if tokens >= 1.0 {
            (tokens - 1.0, Ok(()))
        } else {
            (tokens, Err(((1.0 - tokens) * refill_ms).ceil() as u64))
        };
        sqlx::query(
            "INSERT INTO buckets (name, tokens, updated_ms) VALUES (?, ?, ?)
             ON CONFLICT(name) DO UPDATE SET tokens = excluded.tokens, updated_ms = excluded.updated_ms",
        )
        .bind(name)
        .bind(left)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(out)
    }
}
