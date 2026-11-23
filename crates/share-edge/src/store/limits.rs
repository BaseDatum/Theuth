//! 210 M3 (§6.5, §9.4, §9.6): the durable counters, token buckets, slot-queue reads, evictions, byte usage and
//! retention of `share-edge.sqlite`. Day/week/month counters and buckets survive a restart.

use super::{JobRow, Store, ms};

/// One day/week/month counter to check (and bump) against `limit`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CounterCheck {
    /// e.g. `meshes:net`, `certs:mesh`.
    pub scope: String,
    /// The counted key (a network, a mesh id, `all`).
    pub key: String,
    /// `day` · `week` · `month`.
    pub period: &'static str,
    /// Start of the period (Unix ms); a new period is a new row.
    pub start_ms: u64,
    /// Refused when the count is already at or above it.
    pub limit: u64,
}

/// One token bucket: capacity `cap`, one token per `refill_ms`, full at first use.
#[derive(Debug, Clone, PartialEq)]
pub struct BucketSpec {
    /// Row name in `buckets`.
    pub name: String,
    /// Capacity (burst).
    pub cap: f64,
    /// Ms per token.
    pub refill_ms: f64,
}

#[allow(clippy::cast_precision_loss)]
fn refilled(row: Option<(f64, i64)>, b: &BucketSpec, now: i64) -> f64 {
    match row {
        None => b.cap,
        Some((t, at)) => (t + (now - at).max(0) as f64 / b.refill_ms).min(b.cap),
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn wait_ms(tokens: f64, b: &BucketSpec) -> u64 {
    ((1.0 - tokens).max(0.0) * b.refill_ms).ceil() as u64
}

impl Store {
    /// Check every counter against its limit and, when all pass and `bump`, add one to each, in one
    /// transaction. `Err(i)`: the first counter at its limit; nothing is bumped.
    pub async fn counters_check(&self, checks: &[CounterCheck], bump: bool) -> anyhow::Result<Result<(), usize>> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        for (i, c) in checks.iter().enumerate() {
            let n: Option<i64> = sqlx::query_scalar(
                "SELECT count FROM counters WHERE scope = ? AND key = ? AND period = ? AND start_ms = ?",
            )
            .bind(&c.scope)
            .bind(&c.key)
            .bind(c.period)
            .bind(ms(c.start_ms))
            .fetch_optional(&mut *tx)
            .await?;
            if u64::try_from(n.unwrap_or(0)).unwrap_or(0) >= c.limit {
                return Ok(Err(i));
            }
        }
        if bump {
            for c in checks {
                counter_add(&mut tx, c, 1).await?;
            }
        }
        tx.commit().await?;
        Ok(Ok(()))
    }

    /// Add `n` to every counter, whatever its limit.
    pub async fn counters_add(&self, checks: &[CounterCheck], n: u64) -> anyhow::Result<()> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        for c in checks {
            counter_add(&mut tx, c, n).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// The current count of one counter row.
    pub async fn counter(&self, scope: &str, key: &str, period: &str, start_ms: u64) -> anyhow::Result<u64> {
        let n: Option<i64> = sqlx::query_scalar(
            "SELECT count FROM counters WHERE scope = ? AND key = ? AND period = ? AND start_ms = ?",
        )
        .bind(scope)
        .bind(key)
        .bind(period)
        .bind(ms(start_ms))
        .fetch_optional(&self.pool)
        .await?;
        Ok(u64::try_from(n.unwrap_or(0)).unwrap_or(0))
    }

    /// Take one token from each bucket, all or none. `Err((i, wait_ms))`: bucket `i` is empty for `wait_ms`.
    pub async fn buckets_take(&self, specs: &[BucketSpec], now_ms: u64) -> anyhow::Result<Result<(), (usize, u64)>> {
        let now = ms(now_ms);
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let mut left = Vec::with_capacity(specs.len());
        for (i, b) in specs.iter().enumerate() {
            let row: Option<(f64, i64)> = sqlx::query_as("SELECT tokens, updated_ms FROM buckets WHERE name = ?")
                .bind(&b.name)
                .fetch_optional(&mut *tx)
                .await?;
            let t = refilled(row, b, now);
            if t < 1.0 {
                return Ok(Err((i, wait_ms(t, b))));
            }
            left.push(t - 1.0);
        }
        for (b, t) in specs.iter().zip(left) {
            sqlx::query(
                "INSERT INTO buckets (name, tokens, updated_ms) VALUES (?, ?, ?)
                 ON CONFLICT(name) DO UPDATE SET tokens = excluded.tokens, updated_ms = excluded.updated_ms",
            )
            .bind(&b.name)
            .bind(t)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(Ok(()))
    }

    /// Tokens in a bucket at `now_ms`, without taking any.
    pub async fn bucket_tokens(&self, b: &BucketSpec, now_ms: u64) -> anyhow::Result<f64> {
        let row: Option<(f64, i64)> = sqlx::query_as("SELECT tokens, updated_ms FROM buckets WHERE name = ?")
            .bind(&b.name)
            .fetch_optional(&self.pool)
            .await?;
        Ok(refilled(row, b, ms(now_ms)))
    }

    /// Give one token back (a refused claim that had taken it), up to the capacity.
    pub async fn bucket_refund(&self, b: &BucketSpec, now_ms: u64) -> anyhow::Result<()> {
        let now = ms(now_ms);
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let row: Option<(f64, i64)> = sqlx::query_as("SELECT tokens, updated_ms FROM buckets WHERE name = ?")
            .bind(&b.name)
            .fetch_optional(&mut *tx)
            .await?;
        let t = (refilled(row, b, now) + 1.0).min(b.cap);
        sqlx::query(
            "INSERT INTO buckets (name, tokens, updated_ms) VALUES (?, ?, ?)
             ON CONFLICT(name) DO UPDATE SET tokens = excluded.tokens, updated_ms = excluded.updated_ms",
        )
        .bind(&b.name)
        .bind(t)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// A marker row in `buckets` (`tokens` unused): the last time something happened, e.g. an invited take.
    pub async fn mark(&self, name: &str, now_ms: u64) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO buckets (name, tokens, updated_ms) VALUES (?, 0, ?)
             ON CONFLICT(name) DO UPDATE SET updated_ms = excluded.updated_ms",
        )
        .bind(name)
        .bind(ms(now_ms))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The time of a [`Store::mark`], if ever set.
    pub async fn marked(&self, name: &str) -> anyhow::Result<Option<u64>> {
        let v: Option<i64> = sqlx::query_scalar("SELECT updated_ms FROM buckets WHERE name = ?")
            .bind(name)
            .fetch_optional(&self.pool)
            .await?;
        Ok(v.and_then(|v| u64::try_from(v).ok()))
    }

    /// Every `new` job still needing a slot (its name `allocated` or `queued`), queued or being ordered, oldest
    /// first: the input of the §6.5 order.
    pub async fn slot_jobs(&self, domain: &str) -> anyhow::Result<Vec<JobRow>> {
        Ok(sqlx::query_as::<_, JobRow>(
            "SELECT j.id, j.domain, j.name, j.kind, j.state, j.class, j.mesh, j.net, j.first_of_mesh,
               j.enqueued_ms, j.attempts, j.next_at_ms, j.last_error
             FROM cert_jobs j JOIN names n ON n.domain = j.domain AND n.name = j.name
             WHERE j.domain = ? AND j.kind = 'new' AND j.state IN ('queued','ordering')
               AND n.state IN ('allocated','queued')
             ORDER BY j.enqueued_ms, j.id",
        )
        .bind(domain)
        .fetch_all(&self.pool)
        .await?)
    }

    /// Names holding a slot (`slotted` or `valid`), the `names_max` count.
    pub async fn slotted_count(&self, domain: &str) -> anyhow::Result<u64> {
        let n: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM names WHERE domain = ? AND state IN ('slotted','valid')")
                .bind(domain)
                .fetch_one(&self.pool)
                .await?;
        Ok(u64::try_from(n).unwrap_or(0))
    }

    /// Slotted or valid names never live (`first_live_ms` null) and allocated before `before_ms`, oldest first.
    pub async fn unpublished_names(&self, domain: &str, before_ms: u64) -> anyhow::Result<Vec<String>> {
        Ok(sqlx::query_scalar(
            "SELECT name FROM names WHERE domain = ? AND state IN ('slotted','valid') AND first_live_ms IS NULL
               AND created_ms < ? ORDER BY created_ms",
        )
        .bind(domain)
        .bind(ms(before_ms))
        .fetch_all(&self.pool)
        .await?)
    }

    /// `allocated` names without a CSR, allocated before `before_ms` (`allocated_ttl`).
    pub async fn stale_allocated(&self, domain: &str, before_ms: u64) -> anyhow::Result<Vec<String>> {
        Ok(sqlx::query_scalar(
            "SELECT name FROM names WHERE domain = ? AND state = 'allocated' AND csr IS NULL AND created_ms < ?
             ORDER BY created_ms",
        )
        .bind(domain)
        .bind(ms(before_ms))
        .fetch_all(&self.pool)
        .await?)
    }

    /// Evict a held name (§9.4): no cooldown, the row deleted, its chain to `revocations`
    /// (`cessationOfOperation`), its open jobs dropped. `only_unpublished` keeps a name that went live
    /// meanwhile. The owner, or `None` when nothing was evicted.
    pub async fn evict(
        &self,
        domain: &str,
        name: &str,
        only_unpublished: bool,
        now_ms: u64,
    ) -> anyhow::Result<Option<String>> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let owner: Option<Option<String>> = sqlx::query_scalar(
            "SELECT owner FROM names WHERE domain = ? AND name = ? AND banned IS NULL
               AND state IN ('allocated','queued','slotted','valid') AND (? = 0 OR first_live_ms IS NULL)",
        )
        .bind(domain)
        .bind(name)
        .bind(i64::from(only_unpublished))
        .fetch_optional(&mut *tx)
        .await?;
        let Some(owner) = owner else {
            return Ok(None);
        };
        super::certs::move_chain_to_revocations(&mut tx, domain, name, "cessation_of_operation", now_ms).await?;
        sqlx::query(
            "UPDATE cert_jobs SET state = 'dropped', lease_until_ms = NULL, last_error = 'evicted'
             WHERE domain = ? AND name = ? AND state IN ('queued','ordering')",
        )
        .bind(domain)
        .bind(name)
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM names WHERE domain = ? AND name = ?")
            .bind(domain)
            .bind(name)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(Some(owner.unwrap_or_default()))
    }

    /// Stamp `first_live_ms` once (§9.4). True when this call set it.
    pub async fn set_first_live(&self, domain: &str, name: &str, now_ms: u64) -> anyhow::Result<bool> {
        let r = sqlx::query(
            "UPDATE names SET first_live_ms = ? WHERE domain = ? AND name = ? AND first_live_ms IS NULL
               AND state IN ('allocated','queued','slotted','valid')",
        )
        .bind(ms(now_ms))
        .bind(domain)
        .bind(name)
        .execute(&self.pool)
        .await?;
        Ok(r.rows_affected() == 1)
    }

    /// `first_live_ms` of a name.
    pub async fn first_live(&self, domain: &str, name: &str) -> anyhow::Result<Option<u64>> {
        let v: Option<Option<i64>> =
            sqlx::query_scalar("SELECT first_live_ms FROM names WHERE domain = ? AND name = ?")
                .bind(domain)
                .bind(name)
                .fetch_optional(&self.pool)
                .await?;
        Ok(v.flatten().and_then(|v| u64::try_from(v).ok()))
    }

    /// The claim networks of every held reserved name of a non-invited mesh (`reserved_per_net`).
    pub async fn open_reserved_nets(&self, domain: &str) -> anyhow::Result<Vec<String>> {
        Ok(sqlx::query_scalar(
            "SELECT n.net FROM names n JOIN meshes m ON m.mesh = n.owner
             WHERE n.domain = ? AND n.kind = 'reserved' AND n.state IN ('allocated','queued','slotted','valid')
               AND m.invited = 0",
        )
        .bind(domain)
        .fetch_all(&self.pool)
        .await?)
    }

    /// Add visitor bytes to the day's `usage` row of `(mesh, net)`.
    pub async fn usage_add(&self, day_ms: u64, mesh: &str, net: &str, invited: bool, bytes: u64) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO usage (day, mesh, net, invited, bytes) VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(day, mesh, net) DO UPDATE SET bytes = bytes + excluded.bytes",
        )
        .bind(ms(day_ms))
        .bind(mesh)
        .bind(net)
        .bind(i64::from(invited))
        .bind(ms(bytes))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Every `(mesh, net, bytes)` of a day: the in-memory day totals after a restart.
    pub async fn usage_day(&self, day_ms: u64) -> anyhow::Result<Vec<(String, String, i64)>> {
        Ok(sqlx::query_as("SELECT mesh, net, bytes FROM usage WHERE day = ?")
            .bind(ms(day_ms))
            .fetch_all(&self.pool)
            .await?)
    }

    /// Delete `conn_log` rows before `before_ms` (the 30-day retention); the number deleted.
    pub async fn purge_conn_log(&self, before_ms: u64) -> anyhow::Result<u64> {
        let r = sqlx::query("DELETE FROM conn_log WHERE ms < ?")
            .bind(ms(before_ms))
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected())
    }

    /// `conn_log` rows, all of them (tests, admin).
    pub async fn conn_log_count(&self) -> anyhow::Result<u64> {
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM conn_log")
            .fetch_one(&self.pool)
            .await?;
        Ok(u64::try_from(n).unwrap_or(0))
    }
}

async fn counter_add(conn: &mut sqlx::SqliteConnection, c: &CounterCheck, n: u64) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO counters (scope, key, period, start_ms, count) VALUES (?, ?, ?, ?, ?)
         ON CONFLICT(scope, key, period, start_ms) DO UPDATE SET count = count + excluded.count",
    )
    .bind(&c.scope)
    .bind(&c.key)
    .bind(c.period)
    .bind(ms(c.start_ms))
    .bind(ms(n))
    .execute(conn)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DbJournal;
    use crate::store::{Claimed, NewName};

    const DAY: u64 = 86_400_000;

    async fn open(dir: &std::path::Path) -> Store {
        Store::open(&dir.join("edge.sqlite"), DbJournal::Wal).await.unwrap()
    }

    fn check(key: &str, limit: u64) -> CounterCheck {
        CounterCheck {
            scope: "meshes:net".into(),
            key: key.into(),
            period: "day",
            start_ms: DAY,
            limit,
        }
    }

    #[tokio::test]
    async fn counters_and_buckets_are_all_or_nothing_and_survive_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let s = open(dir.path()).await;
        let wide = CounterCheck {
            scope: "meshes:wide".into(),
            ..check("203.0.113.0/24", 2)
        };
        assert_eq!(s.counters_check(&[check("203.0.113.1", 5), wide.clone()], true).await.unwrap(), Ok(()));
        assert_eq!(s.counters_check(&[check("203.0.113.2", 5), wide.clone()], true).await.unwrap(), Ok(()));
        // The /24 is full: refused at index 1, and the /32 of the refused call is not bumped.
        assert_eq!(s.counters_check(&[check("203.0.113.3", 5), wide.clone()], true).await.unwrap(), Err(1));
        assert_eq!(s.counter("meshes:net", "203.0.113.3", "day", DAY).await.unwrap(), 0);
        let b = BucketSpec {
            name: "claims:net:x".into(),
            cap: 2.0,
            refill_ms: 1000.0,
        };
        let g = BucketSpec {
            name: "open_new_names".into(),
            cap: 1.0,
            refill_ms: 10_000.0,
        };
        assert_eq!(s.buckets_take(&[b.clone(), g.clone()], 0).await.unwrap(), Ok(()));
        // The global bucket is empty: nothing taken from the first either.
        assert_eq!(s.buckets_take(&[b.clone(), g.clone()], 0).await.unwrap(), Err((1, 10_000)));
        assert!((s.bucket_tokens(&b, 0).await.unwrap() - 1.0).abs() < 1e-9);
        s.bucket_refund(&g, 0).await.unwrap();
        s.usage_add(DAY, "m", "203.0.113.1", false, 700).await.unwrap();
        s.usage_add(DAY, "m", "203.0.113.1", false, 300).await.unwrap();
        s.close().await;
        // Restart.
        let s = open(dir.path()).await;
        assert_eq!(s.counter("meshes:wide", "203.0.113.0/24", "day", DAY).await.unwrap(), 2);
        // A new day is a new row.
        assert_eq!(s.counter("meshes:wide", "203.0.113.0/24", "day", 2 * DAY).await.unwrap(), 0);
        assert!((s.bucket_tokens(&b, 0).await.unwrap() - 1.0).abs() < 1e-9);
        assert!((s.bucket_tokens(&g, 0).await.unwrap() - 1.0).abs() < 1e-9);
        assert_eq!(s.usage_day(DAY).await.unwrap(), vec![("m".into(), "203.0.113.1".into(), 1000)]);
    }

    #[tokio::test]
    async fn evictions_first_live_and_conn_log_retention() {
        let dir = tempfile::tempdir().unwrap();
        let s = open(dir.path()).await;
        s.attach("m", "e", "203.0.113.1", false, 0).await.unwrap();
        for name in ["old", "live", "fresh"] {
            let n = NewName {
                domain: "tun.zip",
                name,
                kind: "random",
                owner: "m",
                now_ms: 0,
                net: "203.0.113.1",
            };
            // One random name per mesh: the later ones go in as reserved.
            let r = s.claim(NewName { kind: if name == "old" { "random" } else { "reserved" }, ..n }, 5).await.unwrap();
            assert!(matches!(r, Claimed::Granted), "{name}: {r:?}");
        }
        // Allocated without a CSR for over an hour: all three are stale.
        assert_eq!(s.stale_allocated("tun.zip", 3_600_001).await.unwrap().len(), 3);
        for n in ["old", "live"] {
            s.set_name_state("tun.zip", n, "slotted").await.unwrap();
        }
        assert_eq!(s.slotted_count("tun.zip").await.unwrap(), 2);
        assert!(s.set_first_live("tun.zip", "live", 5).await.unwrap());
        assert!(!s.set_first_live("tun.zip", "live", 9).await.unwrap(), "stamped once");
        assert_eq!(s.first_live("tun.zip", "live").await.unwrap(), Some(5));
        // Never published 7 days after allocation: only "old".
        assert_eq!(s.unpublished_names("tun.zip", 7 * DAY).await.unwrap(), vec!["old".to_string()]);
        assert_eq!(s.evict("tun.zip", "live", true, 7 * DAY).await.unwrap(), None);
        assert_eq!(s.evict("tun.zip", "old", true, 7 * DAY).await.unwrap(), Some("m".into()));
        assert!(s.name("tun.zip", "old").await.unwrap().is_none(), "evicted: no row, no cooldown");
        assert_eq!(s.slotted_count("tun.zip").await.unwrap(), 1);
        // conn_log: 30-day retention.
        s.conn_log(DAY, "a.tun.zip", "192.0.2.1", "m", "e", 1, 2).await.unwrap();
        s.conn_log(40 * DAY, "a.tun.zip", "192.0.2.1", "m", "e", 1, 2).await.unwrap();
        assert_eq!(s.purge_conn_log(40 * DAY - 30 * DAY).await.unwrap(), 1);
        assert_eq!(s.conn_log_count().await.unwrap(), 1);
    }
}
