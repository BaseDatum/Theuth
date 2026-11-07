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

use crate::engine::{Attached, Engine, now_ms, quote};
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
            by_table
                .entry(c.table.as_str())
                .or_default()
                .entry(c.pk.as_str())
                .or_default()
                .push(c);
        }

        // Local writes still in the capture log happened before this batch was
        // seen: stamp them now, below anything it carries and before its
        // columns overwrite them (a poll would otherwise stamp the stale log
        // row above the remote write it lost to).
        for table in by_table.keys() {
            let attached = self.tables.lock().get(*table).cloned();
            if let Some(attached) = attached {
                self.capture_locked(&attached.pool, &[(*table).to_owned()])
                    .await?;
            }
        }
        for c in &batch.changes {
            self.clock.observe(&c.hlc);
        }

        let mut reannounce: Vec<(Attached, String, String)> = Vec::new();
        let mut parked: Vec<String> = Vec::new();
        let mut book = self.store.pool().begin_with("BEGIN IMMEDIATE").await?;
        for (table, rows) in &by_table {
            let attached = self.tables.lock().get(*table).cloned();
            let Some(attached) = attached else {
                // ponytail: not parked — an engine that runs without a plugin would park every
                // batch of its tables forever; a table that attaches late misses what came before.
                self.note(
                    &batch.origin,
                    table,
                    format_args!(
                        "batch {}#{} touches unattached table {table}; skipped",
                        batch.origin, batch.seq
                    ),
                );
                continue;
            };
            // 180 D1b: one table that cannot be written (a constraint a local
            // row breaks) no longer aborts the batch — that ended the stream,
            // re-sent the batch on reconnect and failed it again, so every
            // table behind it (`cost_owners` behind `agents`, `availability`…)
            // never landed. Its rows and shadow clocks roll back and its
            // changes are parked for retry; the other tables land.
            match self
                .apply_table_saved(&mut book, &attached, &batch.origin, table, rows)
                .await?
            {
                Ok(lost) => reannounce.extend(
                    lost.into_iter()
                        .map(|pk| (attached.clone(), (*table).to_owned(), pk)),
                ),
                Err(e) => {
                    let changes: Vec<&Change> = rows.values().flatten().copied().collect();
                    sqlx::query(
                        "INSERT OR IGNORE INTO parked (origin, seq, tbl, changes) VALUES (?, ?, ?, ?)",
                    )
                    .bind(&batch.origin.0)
                    .bind(i64::try_from(batch.seq)?)
                    .bind(*table)
                    .bind(postcard::to_allocvec(&changes)?)
                    .execute(&mut *book)
                    .await?;
                    parked.push((*table).to_owned());
                    self.note(
                        &batch.origin,
                        table,
                        format_args!(
                            "batch {}#{} table {table}: {e:#}; parked for retry",
                            batch.origin, batch.seq
                        ),
                    );
                }
            }
        }
        let bytes = postcard::to_allocvec(batch)?;
        store::insert_batch(&mut *book, &batch.origin, batch.seq, &bytes).await?;
        let (wall, counter) = self.clock.snapshot();
        store::save_clock(&mut *book, wall, counter).await?;
        book.commit().await?;
        self.count_applied(&batch.changes);
        let _ = self.feed.send(batch.clone());
        // A receive-only engine prunes too (69 audit: relayed origins grew forever).
        if let Err(e) = self.prune_outbox().await {
            tracing::warn!("replica: prune outbox: {e:#}");
        }
        // What this batch wrote may be what an earlier parked change of the
        // same table waited for.
        let touched: Vec<&str> = by_table
            .keys()
            .copied()
            .filter(|t| !parked.iter().any(|p| p == t))
            .collect();
        reannounce.extend(self.retry_parked_locked(&touched).await?);

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

    /// [`apply_table`] under a savepoint of `book`: `Ok(Err(e))` = rolled
    /// back (rows and shadow clocks), nothing of the table written. Rows it
    /// had to skip are noted once per `(origin, table)`.
    async fn apply_table_saved(
        &self,
        book: &mut sqlx::SqliteConnection,
        attached: &Attached,
        origin: &crate::PeerId,
        table: &str,
        rows: &BTreeMap<&str, Vec<&Change>>,
    ) -> anyhow::Result<Result<Vec<String>, anyhow::Error>> {
        sqlx::query("SAVEPOINT replica_table")
            .execute(&mut *book)
            .await?;
        let mut skipped = Vec::new();
        match apply_table(attached, table, rows, book, &mut skipped).await {
            Ok(lost) => {
                sqlx::query("RELEASE replica_table")
                    .execute(&mut *book)
                    .await?;
                if let Some(first) = skipped.first() {
                    self.note(
                        origin,
                        table,
                        format_args!("{} row(s) of {table} skipped: {first}", skipped.len()),
                    );
                }
                Ok(Ok(lost))
            }
            Err(e) => {
                sqlx::query("ROLLBACK TO replica_table")
                    .execute(&mut *book)
                    .await?;
                sqlx::query("RELEASE replica_table")
                    .execute(&mut *book)
                    .await?;
                Ok(Err(e))
            }
        }
    }

    /// Retry every parked change of `tables` (last-writer-wins makes a late
    /// apply safe: what newer writes superseded loses); drop what applies.
    /// Returns the rows to re-announce. Caller holds `write_lock`.
    pub(crate) async fn retry_parked_locked(
        &self,
        tables: &[&str],
    ) -> anyhow::Result<Vec<(Attached, String, String)>> {
        let mut reannounce = Vec::new();
        for &table in tables {
            let attached = self.tables.lock().get(table).cloned();
            let Some(attached) = attached else { continue };
            let parked: Vec<(String, i64, Vec<u8>)> = sqlx::query_as(
                "SELECT origin, seq, changes FROM parked WHERE tbl = ? ORDER BY origin, seq",
            )
            .bind(table)
            .fetch_all(self.store.pool())
            .await?;
            for (origin, seq, bytes) in parked {
                let changes: Vec<Change> = postcard::from_bytes(&bytes)?;
                let mut rows: BTreeMap<&str, Vec<&Change>> = BTreeMap::new();
                for c in &changes {
                    rows.entry(c.pk.as_str()).or_default().push(c);
                }
                let origin = crate::PeerId(origin);
                let mut book = self.store.pool().begin_with("BEGIN IMMEDIATE").await?;
                match self
                    .apply_table_saved(&mut book, &attached, &origin, table, &rows)
                    .await?
                {
                    Ok(lost) => {
                        sqlx::query("DELETE FROM parked WHERE origin = ? AND seq = ? AND tbl = ?")
                            .bind(&origin.0)
                            .bind(seq)
                            .bind(table)
                            .execute(&mut *book)
                            .await?;
                        tracing::info!("replica: parked {origin}#{seq} {table} applied");
                        reannounce.extend(
                            lost.into_iter()
                                .map(|pk| (attached.clone(), table.to_owned(), pk)),
                        );
                    }
                    Err(e) => {
                        tracing::debug!("replica: parked {origin}#{seq} {table} still fails: {e:#}")
                    }
                }
                book.commit().await?;
            }
        }
        Ok(reannounce)
    }

    /// The row as it stands, one change per column with the clock it holds.
    pub(crate) async fn row_as_changes(
        &self,
        attached: &Attached,
        table: &str,
        pk: &str,
    ) -> anyhow::Result<Vec<Change>> {
        let cols = attached
            .info
            .columns
            .iter()
            .map(|c| quote(c))
            .collect::<Vec<_>>()
            .join(", ");
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

    /// Every live row of every attached table as whole-row changes (each column under the clock
    /// it holds) plus every tombstone, emitted as one batch of this origin: the stage-1 "snapshot"
    /// (68 §6 decision 7) for a peer whose catch-up point lies below what the outbox still holds —
    /// a newcomer, or an engine away long enough for the prune to pass it. Peers that already hold
    /// the state apply it as a no-op (same clocks). `None` when there is nothing to send.
    // ponytail: one batch of the whole state to every connected peer per newcomer; chunk it when a
    // table outgrows the frame.
    pub(crate) async fn snapshot_batch(&self) -> anyhow::Result<Option<ChangeBatch>> {
        let _guard = self.write_lock.lock().await;
        let attached: Vec<(String, Attached)> = self
            .tables
            .lock()
            .iter()
            .map(|(t, a)| (t.clone(), a.clone()))
            .collect();
        let mut changes = Vec::new();
        for (table, a) in &attached {
            let pks: Vec<(String,)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "SELECT CAST({} AS TEXT) FROM {}",
                quote(&a.info.pk),
                quote(table)
            )))
            .fetch_all(&a.pool)
            .await?;
            for (pk,) in &pks {
                changes.extend(self.row_as_changes(a, table, pk).await?);
            }
        }
        let tombs: Vec<(String, String, String)> =
            sqlx::query_as("SELECT table_name, pk, hlc FROM tombstone")
                .fetch_all(self.store.pool())
                .await?;
        for (table, pk, hlc) in tombs {
            let Some(hlc) = Hlc::parse(&hlc) else {
                continue;
            };
            changes.push(Change {
                table,
                pk,
                column: None,
                value: None,
                hlc,
                deleted: true,
            });
        }
        if changes.is_empty() {
            return Ok(None);
        }
        Ok(Some(self.emit_locked(changes).await?))
    }
}

/// One table of a batch in its own transaction on the table's pool (shadow
/// clocks into `book`); returns the pks whose delete lost (to re-announce).
async fn apply_table(
    attached: &Attached,
    table: &str,
    rows: &BTreeMap<&str, Vec<&Change>>,
    book: &mut sqlx::SqliteConnection,
    skipped: &mut Vec<String>,
) -> anyhow::Result<Vec<String>> {
    let mut tx = attached.pool.begin_with("BEGIN IMMEDIATE").await?;
    let base_seq: i64 = sqlx::query("SELECT COALESCE(MAX(seq), 0) FROM _replica_log")
        .fetch_one(&mut *tx)
        .await?
        .get(0);
    let mut lost = Vec::new();
    for (pk, changes) in rows {
        if apply_row(attached, table, pk, changes, &mut tx, book, skipped)
            .await
            .with_context(|| format!("pk {pk}"))?
        {
            lost.push((*pk).to_owned());
        }
    }
    // The writes above fired the capture triggers: they are not local writes.
    sqlx::query("DELETE FROM _replica_log WHERE seq > ?")
        .bind(base_seq)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(lost)
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
    skipped: &mut Vec<String>,
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
            let newest = clocks
                .values()
                .chain(winners.values().map(|w| w.0))
                .max()
                .cloned();
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
            skipped.push(format!("{table}.{col} is not a replicated column here"));
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

    let row_hlc = winners
        .values()
        .map(|w| w.0)
        .max()
        .expect("non-empty")
        .to_text();
    let cols: Vec<&str> = attached
        .info
        .columns
        .iter()
        .map(String::as_str)
        .filter(|c| winners.contains_key(c))
        .collect();
    // UPDATE first, INSERT when the row is absent: SQLite checks NOT NULL before the conflict
    // target, so `INSERT … ON CONFLICT DO UPDATE` with a subset of the columns failed on every
    // table with a NOT NULL column whenever the row already existed here (69 smoke: docs,
    // cost_owners, availability, vault, engine_stats — no remote column write ever landed).
    let (t, pk_col) = (quote(table), quote(&attached.info.pk));
    let sets: Vec<String> = cols.iter().map(|c| format!("{} = ?", quote(c))).collect();
    let update = format!(
        "UPDATE {t} SET {sets}, \"hlc\" = max(coalesce(\"hlc\", ''), ?) WHERE {pk_col} = ?",
        sets = sets.join(", "),
    );
    let mut q = sqlx::query(sqlx::AssertSqlSafe(update));
    for c in &cols {
        q = winners[c].1.clone().bind(q);
    }
    let updated = q
        .bind(&row_hlc)
        .bind(pk)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    if updated == 0 {
        let insert = format!(
            "INSERT INTO {t} ({pk_col}, {cols}, \"hlc\") VALUES (?, {marks}, ?)",
            cols = cols.iter().map(|c| quote(c)).collect::<Vec<_>>().join(", "),
            marks = vec!["?"; cols.len()].join(", "),
        );
        let mut q = sqlx::query(sqlx::AssertSqlSafe(insert)).bind(pk);
        for c in &cols {
            q = winners[c].1.clone().bind(q);
        }
        if let Err(e) = q.bind(&row_hlc).execute(&mut *tx).await {
            // A partial row for a pk this engine never received: the insert is still in flight
            // from a third engine, or lies below a pruned outbox (the Hello gap snapshot covers
            // that on the next connect). Skipped; the col clocks stay unset so it lands then.
            skipped.push(format!("pk {pk}: {e}"));
            return Ok(lost_delete);
        }
    }
    for (col, (hlc, _)) in &winners {
        store::set_col_clock(book, table, pk, col, hlc).await?;
    }
    if !deleted {
        store::unbury(book, table, pk).await?;
    }
    Ok(lost_delete)
}
