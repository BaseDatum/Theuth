//! Applying a peer's [`ChangeBatch`]: per-column last-writer-wins against the
//! shadow clocks, tombstones for deletes, one transaction per table, echo
//! suppression on the capture log, then the batch goes to the outbox (so it
//! is never applied twice and can be relayed) and onto the feed.
//!
//! Delete vs. update: a delete wins iff its HLC is above every column write
//! of the row; otherwise the delete is void everywhere. A column write above
//! a tombstone voids that tombstone. Because the engine that deleted lacks
//! the row's other columns, the engine whose row survived a losing delete
//! re-announces the whole row (current values, current clocks) so both sides
//! converge on the full row.

use std::collections::{BTreeMap, HashMap};

use anyhow::Context as _;
use sqlx::Row as _;

use crate::engine::{now_ms, quote, Attached, Engine};
use crate::store;
use crate::types::{Change, ChangeBatch, Hlc};
use crate::value::Val;

impl Engine {
    /// Apply `batch` unless it is ours or already applied. Returns whether it
    /// was applied now. Never fails on one bad row: a row that cannot be
    /// written (a partial update to a row this engine never received, hitting
    /// a NOT NULL) is skipped with a warning.
    pub async fn apply(&self, batch: &ChangeBatch) -> anyhow::Result<bool> {
        if batch.origin == self.origin {
            return Ok(false);
        }
        let _guard = self.write_lock.lock().await;
        if self.store.has_batch(&batch.origin, batch.seq).await? {
            return Ok(false);
        }
        // table → pk → changes, in batch order.
        let mut by_table: BTreeMap<&str, BTreeMap<&str, Vec<&Change>>> = BTreeMap::new();
        for c in &batch.changes {
            by_table.entry(c.table.as_str()).or_default().entry(c.pk.as_str()).or_default().push(c);
        }

        // Local writes still in the capture log happened before this batch was
        // seen: stamp them now, below anything it carries and before its
        // columns overwrite them (a poll would otherwise stamp the stale log
        // row above the remote write it lost to).
        for table in by_table.keys() {
            let attached = self.tables.lock().get(*table).cloned();
            if let Some(attached) = attached {
                self.capture_locked(&attached.pool, &[(*table).to_owned()]).await?;
            }
        }
        for c in &batch.changes {
            self.clock.observe(&c.hlc);
        }

        let mut reannounce: Vec<(Attached, String, String)> = Vec::new();
        let mut book = self.store.pool().begin_with("BEGIN IMMEDIATE").await?;
        for (table, rows) in by_table {
            let attached = self.tables.lock().get(table).cloned();
            let Some(attached) = attached else {
                tracing::warn!("replica: batch {}#{} touches unattached table {table}; skipped", batch.origin, batch.seq);
                continue;
            };
            let mut tx = attached.pool.begin_with("BEGIN IMMEDIATE").await?;
            let base_seq: i64 = sqlx::query("SELECT COALESCE(MAX(seq), 0) FROM _replica_log")
                .fetch_one(&mut *tx)
                .await?
                .get(0);
            for (pk, changes) in rows {
                let lost_delete = apply_row(&attached, table, pk, &changes, &mut tx, &mut book)
                    .await
                    .with_context(|| format!("table {table} pk {pk}"))?;
                if lost_delete {
                    reannounce.push((attached.clone(), table.to_owned(), pk.to_owned()));
                }
            }
            // The writes above fired the capture triggers: they are not local writes.
            sqlx::query("DELETE FROM _replica_log WHERE seq > ?")
                .bind(base_seq)
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
        }
        let bytes = postcard::to_allocvec(batch)?;
        store::insert_batch(&mut *book, &batch.origin, batch.seq, &bytes).await?;
        let (wall, counter) = self.clock.snapshot();
        store::save_clock(&mut *book, wall, counter).await?;
        book.commit().await?;
        let _ = self.feed.send(batch.clone());

        for (attached, table, pk) in reannounce {
            match self.row_as_changes(&attached, &table, &pk).await {
                Ok(changes) if !changes.is_empty() => {
                    self.emit_locked(changes).await?;
                }
                Ok(_) => {}
                Err(e) => tracing::warn!("replica: re-announce {table} pk {pk}: {e:#}"),
            }
        }
        Ok(true)
    }

    /// The row as it stands, one change per column with the clock it holds.
    pub(crate) async fn row_as_changes(&self, attached: &Attached, table: &str, pk: &str) -> anyhow::Result<Vec<Change>> {
        let cols = attached.info.columns.iter().map(|c| quote(c)).collect::<Vec<_>>().join(", ");
        let sql = format!(
            "SELECT {cols}, \"hlc\" FROM {} WHERE {} = ?",
            quote(table),
            quote(&attached.info.pk)
        );
        let Some(row) = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(pk)
            .fetch_optional(&attached.pool)
            .await?
        else {
            return Ok(Vec::new());
        };
        let mut conn = self.store.pool().acquire().await?;
        let clocks = store::col_clocks(&mut conn, table, pk).await?;
        let row_hlc = row.try_get::<Option<&str>, _>("hlc")?.and_then(Hlc::parse);
        let mut out = Vec::with_capacity(attached.info.columns.len());
        for (i, col) in attached.info.columns.iter().enumerate() {
            let hlc = clocks
                .get(col)
                .or(row_hlc.as_ref())
                .cloned()
                .unwrap_or_else(|| self.clock.tick(now_ms()));
            let value = Val::from_row(&row, i)?.encode();
            out.push(Change {
                table: table.to_owned(),
                pk: pk.to_owned(),
                column: Some(col.clone()),
                value,
                hlc,
                deleted: false,
            });
        }
        Ok(out)
    }
}

/// LWW for one row: walk its changes in order against the tombstone and the
/// column clocks; delete when the delete is newer than every column, write
/// the winning columns in one upsert. Returns whether a delete in `changes`
/// lost to a newer column here (the caller re-announces the row).
async fn apply_row(
    attached: &Attached,
    table: &str,
    pk: &str,
    changes: &[&Change],
    tx: &mut sqlx::SqliteConnection,
    book: &mut sqlx::SqliteConnection,
) -> anyhow::Result<bool> {
    let mut tomb = store::tombstone(book, table, pk).await?;
    let mut clocks = store::col_clocks(book, table, pk).await?;
    let mut winners: HashMap<&str, (&Hlc, Val)> = HashMap::new();
    let mut deleted = false;
    let mut lost_delete = false;

    // A live write newer than the tombstone voids it for the whole group, so
    // the rest of a re-announced row lands too.
    if let Some(t) = &tomb
        && changes.iter().any(|c| !c.deleted && c.hlc > *t)
    {
        tomb = None;
    }

    for c in changes {
        if c.deleted {
            let newest = clocks.values().chain(winners.values().map(|w| w.0)).max().cloned();
            let beats_row = newest.is_none_or(|n| c.hlc > n);
            let beats_tomb = tomb.as_ref().is_none_or(|t| c.hlc > *t);
            if beats_row && beats_tomb {
                winners.clear();
                clocks.clear();
                tomb = Some(c.hlc.clone());
                deleted = true;
            } else if !beats_row {
                lost_delete = true;
            }
            continue;
        }
        let Some(col) = c.column.as_deref() else {
            continue; // whole-row without a tombstone: not produced by capture
        };
        if !attached.info.columns.iter().any(|k| k == col) {
            tracing::warn!("replica: {table}.{col} is not a replicated column here; skipped");
            continue;
        }
        if tomb.as_ref().is_some_and(|t| *t >= c.hlc) {
            continue;
        }
        if clocks.get(col).is_some_and(|k| *k >= c.hlc) {
            continue;
        }
        clocks.insert(col.to_owned(), c.hlc.clone());
        winners.insert(col, (&c.hlc, Val::decode(c.value.as_deref())?));
    }

    if deleted {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DELETE FROM {} WHERE {} = ?",
            quote(table),
            quote(&attached.info.pk)
        )))
        .bind(pk)
        .execute(&mut *tx)
        .await?;
        if let Some(t) = &tomb {
            store::bury(book, table, pk, t).await?;
        }
    }
    if winners.is_empty() {
        return Ok(lost_delete);
    }

    let row_hlc = winners.values().map(|w| w.0).max().expect("non-empty").to_text();
    let cols: Vec<&str> = attached
        .info
        .columns
        .iter()
        .map(String::as_str)
        .filter(|c| winners.contains_key(c))
        .collect();
    let sets: Vec<String> = cols.iter().map(|c| format!("{0} = excluded.{0}", quote(c))).collect();
    let sql = format!(
        "INSERT INTO {t} ({pk}, {cols}, \"hlc\") VALUES (?, {marks}, ?)
         ON CONFLICT ({pk}) DO UPDATE SET {sets}, \"hlc\" = max(coalesce({t}.\"hlc\", ''), excluded.\"hlc\")",
        t = quote(table),
        pk = quote(&attached.info.pk),
        cols = cols.iter().map(|c| quote(c)).collect::<Vec<_>>().join(", "),
        marks = vec!["?"; cols.len()].join(", "),
        sets = sets.join(", "),
    );
    let mut q = sqlx::query(sqlx::AssertSqlSafe(sql)).bind(pk);
    for c in &cols {
        q = winners[c].1.clone().bind(q);
    }
    q = q.bind(row_hlc);
    if let Err(e) = q.execute(&mut *tx).await {
        // ponytail: a partial row for a pk this engine never received (its insert is still in
        // flight from a third engine) trips NOT NULL; buffer such orphans if it ever bites.
        tracing::warn!("replica: {table} pk {pk}: {e}; changes skipped");
        return Ok(lost_delete);
    }
    for (col, (hlc, _)) in &winners {
        store::set_col_clock(book, table, pk, col, hlc).await?;
    }
    if !deleted {
        store::unbury(book, table, pk).await?;
    }
    Ok(lost_delete)
}
