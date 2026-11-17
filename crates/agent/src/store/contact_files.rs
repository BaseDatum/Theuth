//! 199 CONTACT-FILES: the node-local `contact_files` table (migration 0067). One row per
//! file between contacts: `out` rows are this engine's offers, `in` rows its downloads and
//! the holder's notice outbox (`notice`, `notice_state`). Every state change is a
//! conditional update (`… WHERE id = ? AND state IN (<expected>)`); a terminal row never
//! changes state again, only its outbox columns. The table is not captured, so nothing
//! here reaches the session log.

use std::collections::HashSet;

use chrono::{DateTime, TimeZone as _, Utc};
use sqlx::{FromRow, SqliteConnection};

use super::Store;
use crate::communication::files::{ContactFile, FileState, InState, OFFER_TTL, OutState, notice};

/// Columns of [`FileRow`], in table order; a macro so `concat!` splices it into
/// `&'static str` SQL (sqlx 0.9 `SqlSafeStr`, as `exchange_cols!`).
macro_rules! cols {
    () => {
        "id, direction, session, exchange, seq, contact_id, name, media_type, size, \
         blake3, source, clone, dev, inode, path, done, state, error, notice, \
         notice_state, created_ms, deadline_ms"
    };
}

/// `SELECT <cols> FROM contact_files ` followed by the rest of the statement.
macro_rules! select {
    ($rest:literal) => {
        concat!("SELECT ", cols!(), " FROM contact_files ", $rest)
    };
}

/// One `contact_files` row as stored.
#[allow(missing_docs)]
#[derive(Clone, Debug, Default, PartialEq, Eq, FromRow)]
pub struct FileRow {
    pub id: String,
    /// `out` | `in`.
    pub direction: String,
    pub session: String,
    /// The exchange id, or `pending:<request id>:<contact id>` for an offer whose
    /// exchange is being accepted (adopted by [`Store::exchange_record`]).
    pub exchange: String,
    pub seq: i64,
    pub contact_id: String,
    pub name: String,
    pub media_type: String,
    pub size: Option<i64>,
    pub blake3: Option<String>,
    pub source: String,
    pub clone: Option<String>,
    pub dev: Option<i64>,
    pub inode: Option<i64>,
    pub path: Option<String>,
    pub done: i64,
    pub state: String,
    pub error: Option<String>,
    pub notice: Option<String>,
    pub notice_state: Option<String>,
    pub created_ms: i64,
    pub deadline_ms: i64,
}

/// What a card needs beyond the row: this engine's endpoint id and machine name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Render {
    /// The holder (`Store::engine`).
    pub holder: String,
    /// Its machine name.
    pub machine: String,
}

/// What [`Store::settle_files`] did to one row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Settled {
    /// `saved` → `received`, its notice queued: send the `received` receipt.
    Received,
    /// A failed file's notice was written (its exchange was read later).
    Noticed,
    /// Failed with its exchange (in: roll the saved file back, send a failed receipt;
    /// out: drop the clone).
    Failed,
}

fn in_state(s: &str) -> InState {
    match s {
        "queued" => InState::Queued,
        "receiving" => InState::Receiving,
        "publishing" => InState::Publishing,
        "saved" => InState::Saved,
        "received" => InState::Received,
        _ => InState::Failed,
    }
}

fn out_state(s: &str) -> OutState {
    match s {
        "queued" => OutState::Queued,
        "sending" => OutState::Sending,
        "sent" => OutState::Sent,
        _ => OutState::Failed,
    }
}

/// `ms` since the epoch as a time (clamped into range).
pub fn at_ms(ms: i64) -> DateTime<Utc> {
    Utc.timestamp_millis_opt(ms).single().unwrap_or_default()
}

impl FileRow {
    /// Direction and state.
    pub fn file_state(&self) -> FileState {
        match self.direction.as_str() {
            "out" => FileState::Out(out_state(&self.state)),
            _ => FileState::In(in_state(&self.state)),
        }
    }

    /// `received`, `sent` or `failed`.
    pub fn terminal(&self) -> bool {
        self.file_state().terminal()
    }

    /// Recorded `(dev, inode)`, when both are set.
    pub fn ino(&self) -> Option<(u64, u64)> {
        Some((self.dev? as u64, self.inode? as u64))
    }

    /// The card, under `alias` (the contact's display name).
    pub fn card(&self, r: &Render, alias: &str) -> ContactFile {
        ContactFile {
            id: self.id.clone(),
            holder: r.holder.clone(),
            state: self.file_state(),
            exchange: self.exchange.parse().unwrap_or_default(),
            contact_id: self.contact_id.clone(),
            contact: alias.to_owned(),
            name: self.name.clone(),
            media_type: self.media_type.clone(),
            size: self.size.map(|n| n as u64),
            blake3: self.blake3.clone(),
            done: self.done.max(0) as u64,
            error: self.error.clone(),
            path: self
                .path
                .clone()
                .filter(|_| self.direction == "in")
                .map(Into::into),
            machine: r.machine.clone(),
            deadline: at_ms(self.deadline_ms),
        }
    }

    /// The exact notice row text for this (terminal) row, from `peer:<alias>`.
    pub fn notice_text(&self, r: &Render, alias: &str) -> Option<String> {
        notice(&format!("peer:{alias}"), &self.card(r, alias))
    }
}

/// The contact's display alias by record id (the record itself when it has none).
async fn alias_in(conn: &mut SqliteConnection, contact: &str) -> anyhow::Result<String> {
    let row: Option<(Option<String>,)> = sqlx::query_as("SELECT alias FROM contacts WHERE id = ?")
        .bind(contact)
        .fetch_optional(conn)
        .await?;
    Ok(row
        .and_then(|(a,)| a)
        .filter(|a| !a.is_empty())
        .unwrap_or_else(|| contact.get(..8).unwrap_or(contact).to_owned()))
}

async fn row_in(conn: &mut SqliteConnection, id: &str) -> anyhow::Result<Option<FileRow>> {
    Ok(sqlx::query_as(select!("WHERE id = ?"))
        .bind(id)
        .fetch_optional(conn)
        .await?)
}

/// The exchange's state, `None` when it is unknown.
async fn exchange_state_in(
    conn: &mut SqliteConnection,
    exchange: &str,
) -> anyhow::Result<Option<String>> {
    let row: Option<(String,)> = sqlx::query_as("SELECT state FROM peer_exchanges WHERE id = ?")
        .bind(exchange)
        .fetch_optional(conn)
        .await?;
    Ok(row.map(|(s,)| s))
}

fn read(state: Option<&str>) -> bool {
    matches!(state, Some("consumed" | "replied"))
}

fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

/// Where the send path stages a message's offers until its exchange exists: the sending
/// session and the message's request id (one exchange per `(sender, request_id)`).
pub fn staged_tag(session: &str, request_id: &str) -> String {
    format!("pending:{session}:{request_id}")
}

impl Store {
    /// Insert rows as given, in one transaction.
    pub async fn insert_contact_files(&self, rows: &[FileRow]) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        for r in rows {
            insert_in(&mut tx, r).await?;
        }
        tx.commit().await
    }

    /// 199 admission: the `in` rows of `exchange`, written once (a redelivery of the same
    /// exchange keeps the first set). `true` when they were written now.
    pub async fn admit_in_files(&self, exchange: &str, rows: &[FileRow]) -> anyhow::Result<bool> {
        if rows.is_empty() {
            return Ok(false);
        }
        let mut tx = self.begin_write().await?;
        let have: bool =
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM contact_files WHERE exchange = ?)")
                .bind(exchange)
                .fetch_one(&mut *tx)
                .await?;
        if have {
            return Ok(false);
        }
        for r in rows {
            insert_in(&mut tx, r).await?;
        }
        tx.commit().await?;
        Ok(true)
    }

    /// `PeerRecord.files` of exchange `id`: its rows as cards, in message order. An
    /// outbound exchange first adopts the offers its send staged under
    /// [`staged_tag`]`(session, request id)`.
    pub async fn exchange_cards(&self, id: &str) -> anyhow::Result<Vec<ContactFile>> {
        let mut rows = self.exchange_files(id).await?;
        if rows.is_empty() {
            let staged: Option<(String, String)> = sqlx::query_as(
                "SELECT e.sender_session, e.request_id FROM peer_exchanges e WHERE e.id = ? \
                 AND e.sender_session IS NOT NULL AND e.recipient LIKE 'peer:%' AND EXISTS \
                 (SELECT 1 FROM contact_files f WHERE f.exchange = \
                  'pending:' || e.sender_session || ':' || e.request_id)",
            )
            .bind(id)
            .fetch_optional(&self.read)
            .await?;
            let Some((session, request)) = staged else {
                return Ok(Vec::new());
            };
            self.adopt_offers(&staged_tag(&session, &request), id)
                .await?;
            rows = self.exchange_files(id).await?;
        }
        if rows.is_empty() {
            return Ok(Vec::new());
        }
        let r = Render {
            holder: self.engine().to_owned(),
            machine: crate::communication::mesh::engine_names().await.0,
        };
        self.file_cards(&r, &rows).await
    }

    /// One row by local id.
    pub async fn contact_file(&self, id: &str) -> anyhow::Result<Option<FileRow>> {
        let mut conn = self.read.acquire().await?;
        row_in(&mut conn, id).await
    }

    /// The rows of `exchange` (or of a pending tag), in message order.
    pub async fn exchange_files(&self, exchange: &str) -> anyhow::Result<Vec<FileRow>> {
        Ok(sqlx::query_as(select!("WHERE exchange = ? ORDER BY seq"))
            .bind(exchange)
            .fetch_all(&self.read)
            .await?)
    }

    /// Cards of `rows` (aliases read once per contact).
    pub async fn file_cards(
        &self,
        r: &Render,
        rows: &[FileRow],
    ) -> anyhow::Result<Vec<ContactFile>> {
        let mut conn = self.read.acquire().await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let alias = alias_in(&mut conn, &row.contact_id).await?;
            out.push(row.card(r, &alias));
        }
        Ok(out)
    }

    /// The contact's display alias.
    pub async fn contact_alias(&self, contact: &str) -> anyhow::Result<String> {
        let mut conn = self.read.acquire().await?;
        alias_in(&mut conn, contact).await
    }

    /// Offers staged under `tag` become `exchange`'s (the send path's hand-off).
    pub async fn adopt_offers(&self, tag: &str, exchange: &str) -> anyhow::Result<u64> {
        let mut tx = self.begin_write().await?;
        let n = sqlx::query("UPDATE contact_files SET exchange = ? WHERE exchange = ?")
            .bind(exchange)
            .bind(tag)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        tx.commit().await?;
        Ok(n)
    }

    /// Delete the offers still staged under `tag` (the send never got an exchange) or,
    /// with `"pending:%"`, every staged offer (startup). Returns them.
    pub async fn drop_staged_offers(&self, tag: &str) -> anyhow::Result<Vec<FileRow>> {
        let mut tx = self.begin_write().await?;
        let rows: Vec<FileRow> = sqlx::query_as(concat!(
            "DELETE FROM contact_files WHERE exchange LIKE ? AND direction = 'out' RETURNING ",
            cols!()
        ))
        .bind(tag)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows)
    }

    /// `id` from one of `from` to `to` (a conditional update): `Some(row)` after it.
    pub async fn file_to(
        &self,
        id: &str,
        from: &[&str],
        to: &str,
    ) -> anyhow::Result<Option<FileRow>> {
        let mut tx = self.begin_write().await?;
        let n = sqlx::query(
            "UPDATE contact_files SET state = ? WHERE id = ? \
             AND state IN (SELECT value FROM json_each(?))",
        )
        .bind(to)
        .bind(id)
        .bind(serde_json::to_string(from)?)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        let row = if n == 1 {
            row_in(&mut tx, id).await?
        } else {
            None
        };
        tx.commit().await?;
        Ok(row)
    }

    /// A download's bytes so far (no state change).
    pub async fn file_done(&self, id: &str, done: u64) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query("UPDATE contact_files SET done = ? WHERE id = ? AND state = 'receiving'")
            .bind(done as i64)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await
    }

    /// Back to the queue: `receiving` (or, after a restart, `publishing`) → `queued`,
    /// `done` as given (0 restarts from byte 0).
    pub async fn file_requeue(&self, id: &str, done: u64) -> anyhow::Result<bool> {
        let mut tx = self.begin_write().await?;
        let n = sqlx::query(
            "UPDATE contact_files SET state = 'queued', done = ?, path = NULL, dev = NULL, \
             inode = NULL WHERE id = ? AND state IN ('receiving', 'publishing')",
        )
        .bind(done as i64)
        .bind(id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        tx.commit().await?;
        Ok(n == 1)
    }

    /// Publication step 4: `publishing`, the target and the partial's inode, in one write.
    pub async fn file_publishing(
        &self,
        id: &str,
        path: &str,
        dev: u64,
        inode: u64,
    ) -> anyhow::Result<bool> {
        let mut tx = self.begin_write().await?;
        let n = sqlx::query(
            "UPDATE contact_files SET state = 'publishing', path = ?, dev = ?, inode = ? \
             WHERE id = ? AND state IN ('receiving', 'publishing')",
        )
        .bind(path)
        .bind(dev as i64)
        .bind(inode as i64)
        .bind(id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        tx.commit().await?;
        Ok(n == 1)
    }

    /// Publication step 6, one transaction: the actual size and BLAKE3, and `received`
    /// with its notice queued when the exchange reads `consumed` or `replied`, else
    /// `saved`. `None` when the row is no longer `publishing` (it failed meanwhile).
    pub async fn file_published(
        &self,
        r: &Render,
        id: &str,
        size: u64,
        blake3: &str,
    ) -> anyhow::Result<Option<FileRow>> {
        let mut tx = self.begin_write().await?;
        let Some(mut row) = row_in(&mut tx, id)
            .await?
            .filter(|r| r.state == "publishing")
        else {
            return Ok(None);
        };
        row.size = Some(size as i64);
        row.blake3 = Some(blake3.to_owned());
        row.done = size as i64;
        let ex = exchange_state_in(&mut tx, &row.exchange).await?;
        if read(ex.as_deref()) {
            row.state = "received".into();
            let alias = alias_in(&mut tx, &row.contact_id).await?;
            row.notice = row.notice_text(r, &alias);
            row.notice_state = Some("queued".into());
        } else {
            row.state = "saved".into();
        }
        sqlx::query(
            "UPDATE contact_files SET state = ?, size = ?, blake3 = ?, done = ?, \
             notice = COALESCE(notice, ?), notice_state = COALESCE(notice_state, ?) \
             WHERE id = ? AND state = 'publishing'",
        )
        .bind(&row.state)
        .bind(row.size)
        .bind(&row.blake3)
        .bind(row.done)
        .bind(&row.notice)
        .bind(&row.notice_state)
        .bind(id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(Some(row))
    }

    /// Fail a non-terminal `in` row with `error`; its failure notice is queued in the same
    /// transaction when the exchange already reads `consumed` or `replied`. `None` when it
    /// was terminal already.
    pub async fn file_fail(
        &self,
        r: &Render,
        id: &str,
        error: &str,
    ) -> anyhow::Result<Option<FileRow>> {
        let mut tx = self.begin_write().await?;
        let row = fail_in(&mut tx, r, id, error).await?;
        tx.commit().await?;
        Ok(row)
    }

    /// The States rules for every row whose exchange moved (one transaction): `saved` →
    /// `received` with its notice, a failed file's waiting notice, and every non-terminal
    /// file of a `failed`/`suppressed` exchange failed with its reason (no notice); an `in`
    /// file of an `unknown` one too. An offer of an `unknown` exchange stays: its text may
    /// have arrived (a restart while it was out) and the receiver may be fetching it; its
    /// receipt or its deadline settles it. Rows in `skip` (a transfer holds them) wait for
    /// the next pass.
    pub async fn settle_files(
        &self,
        r: &Render,
        skip: &HashSet<String>,
    ) -> anyhow::Result<Vec<(FileRow, Settled)>> {
        let mut tx = self.begin_write().await?;
        // Exchange states: `consumed`/`replied` read a saved file (and a failed one's
        // notice); `failed`/`suppressed` end every non-terminal one, `unknown` the `in` ones.
        let rows: Vec<(String, String, String, Option<String>)> = sqlx::query_as(
            "SELECT f.id, f.direction, f.state, e.error FROM contact_files f \
             JOIN peer_exchanges e ON e.id = f.exchange WHERE \
             (f.direction = 'in' AND f.state = 'saved' AND e.state IN ('consumed', 'replied')) \
             OR (f.direction = 'in' AND f.state = 'failed' AND f.notice IS NULL \
                 AND e.state IN ('consumed', 'replied')) \
             OR (f.state NOT IN ('received', 'sent', 'failed') \
                 AND (e.state IN ('failed', 'suppressed') \
                      OR (f.direction = 'in' AND e.state = 'unknown')))",
        )
        .fetch_all(&mut *tx)
        .await?;
        let mut out = Vec::new();
        for (id, direction, state, error) in rows {
            if skip.contains(&id) {
                continue;
            }
            let Some(mut row) = row_in(&mut tx, &id).await? else {
                continue;
            };
            let alias = alias_in(&mut tx, &row.contact_id).await?;
            let ex = exchange_state_in(&mut tx, &row.exchange).await?;
            if direction == "in" && state == "saved" && read(ex.as_deref()) {
                row.state = "received".into();
                row.notice = row.notice_text(r, &alias);
                row.notice_state = Some("queued".into());
                sqlx::query(
                    "UPDATE contact_files SET state = 'received', notice = ?, \
                     notice_state = 'queued' WHERE id = ? AND state = 'saved'",
                )
                .bind(&row.notice)
                .bind(&id)
                .execute(&mut *tx)
                .await?;
                out.push((row, Settled::Received));
            } else if direction == "in" && state == "failed" {
                row.notice = row.notice_text(r, &alias);
                sqlx::query(
                    "UPDATE contact_files SET notice = ?, notice_state = 'queued' \
                     WHERE id = ? AND state = 'failed' AND notice IS NULL",
                )
                .bind(&row.notice)
                .bind(&id)
                .execute(&mut *tx)
                .await?;
                row.notice_state = Some("queued".into());
                out.push((row, Settled::Noticed));
            } else {
                let why = error.unwrap_or_else(|| "the message was not delivered".into());
                sqlx::query(
                    "UPDATE contact_files SET state = 'failed', error = ? \
                     WHERE id = ? AND state NOT IN ('received', 'sent', 'failed')",
                )
                .bind(&why)
                .bind(&id)
                .execute(&mut *tx)
                .await?;
                row.state = "failed".into();
                row.error = Some(why);
                out.push((row, Settled::Failed));
            }
        }
        tx.commit().await?;
        Ok(out)
    }

    /// Rows still moving bytes past their deadline at `now_ms`, not in `skip`: queued or
    /// receiving downloads and unsettled offers. A file already on disk (`publishing`,
    /// `saved`) only waits for its exchange.
    pub async fn expired_files(
        &self,
        now_ms: i64,
        skip: &HashSet<String>,
    ) -> anyhow::Result<Vec<FileRow>> {
        let rows: Vec<FileRow> = sqlx::query_as(select!(
            "WHERE deadline_ms <= ? AND state IN ('queued', 'receiving', 'sending') \
             AND exchange NOT LIKE 'pending:%'"
        ))
        .bind(now_ms)
        .fetch_all(&self.read)
        .await?;
        Ok(rows.into_iter().filter(|r| !skip.contains(&r.id)).collect())
    }

    /// Fail a non-terminal `out` row with `error`. `None` when it was terminal already.
    pub async fn offer_fail(&self, id: &str, error: &str) -> anyhow::Result<Option<FileRow>> {
        let mut tx = self.begin_write().await?;
        let n = sqlx::query(
            "UPDATE contact_files SET state = 'failed', error = ? \
             WHERE id = ? AND direction = 'out' AND state IN ('queued', 'sending')",
        )
        .bind(error)
        .bind(id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        let row = if n == 1 {
            row_in(&mut tx, id).await?
        } else {
            None
        };
        tx.commit().await?;
        Ok(row)
    }

    /// The first terminal receipt of offer `id`: `sent`, or `failed` with `reason`.
    /// `None` when the offer was terminal already (a repeat changes nothing).
    pub async fn offer_settle(
        &self,
        id: &str,
        reason: Option<&str>,
    ) -> anyhow::Result<Option<FileRow>> {
        let mut tx = self.begin_write().await?;
        let n = sqlx::query(
            "UPDATE contact_files SET state = ?, error = ?, \
             done = CASE WHEN ? IS NULL THEN COALESCE(size, done) ELSE done END \
             WHERE id = ? AND direction = 'out' AND state IN ('queued', 'sending')",
        )
        .bind(if reason.is_some() { "failed" } else { "sent" })
        .bind(reason)
        .bind(reason)
        .bind(id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        let row = if n == 1 {
            row_in(&mut tx, id).await?
        } else {
            None
        };
        tx.commit().await?;
        Ok(row)
    }

    /// Queued downloads, oldest first.
    pub async fn queued_files(&self) -> anyhow::Result<Vec<FileRow>> {
        Ok(sqlx::query_as(select!(
            "WHERE direction = 'in' AND state = 'queued' ORDER BY created_ms, seq"
        ))
        .fetch_all(&self.read)
        .await?)
    }

    /// `in` rows in `state`.
    pub async fn files_in_state(&self, state: &str) -> anyhow::Result<Vec<FileRow>> {
        Ok(
            sqlx::query_as(select!("WHERE direction = 'in' AND state = ?"))
                .bind(state)
                .fetch_all(&self.read)
                .await?,
        )
    }

    /// Terminal `in` rows whose offer still lives at `now_ms`: their receipts (re-sent at
    /// startup; the sender applies the first only).
    pub async fn receipts_due(&self, now_ms: i64) -> anyhow::Result<Vec<FileRow>> {
        Ok(sqlx::query_as(select!(
            "WHERE direction = 'in' AND deadline_ms > ? \
             AND (state = 'received' OR (state = 'failed' AND source LIKE 'iroh://%'))"
        ))
        .bind(now_ms)
        .fetch_all(&self.read)
        .await?)
    }

    /// The notice outbox: `queued` and `sent` notices.
    pub async fn notice_outbox(&self) -> anyhow::Result<Vec<FileRow>> {
        Ok(sqlx::query_as(select!(
            "WHERE notice_state IN ('queued', 'sent') AND notice IS NOT NULL \
             ORDER BY created_ms, seq"
        ))
        .fetch_all(&self.read)
        .await?)
    }

    /// The outbox column `from` → `to` (conditional).
    pub async fn notice_to(&self, id: &str, from: &str, to: &str) -> anyhow::Result<bool> {
        let mut tx = self.begin_write().await?;
        let n = sqlx::query(
            "UPDATE contact_files SET notice_state = ? WHERE id = ? AND notice_state = ?",
        )
        .bind(to)
        .bind(id)
        .bind(from)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        tx.commit().await?;
        Ok(n == 1)
    }

    /// 199: the contact record bound to `binding` (`peer:<uuid>`, as `peer_exchanges.sender`
    /// and `contacts.binding` spell it): `(record id, alias)`, following merges.
    pub async fn contact_of_binding(
        &self,
        binding: &str,
    ) -> anyhow::Result<Option<(String, String)>> {
        let row: Option<(String, Option<String>)> = sqlx::query_as(
            "SELECT id, alias FROM contacts WHERE binding = ? AND removed_ms IS NULL \
             ORDER BY merged_into IS NOT NULL, id LIMIT 1",
        )
        .bind(binding)
        .fetch_optional(&self.read)
        .await?;
        let Some((id, alias)) = row else {
            return Ok(None);
        };
        let id = self.contact_root(&id).await?;
        Ok(Some((id, alias.unwrap_or_default())))
    }

    /// 199: the live contact record named `alias` (the local display key): `(record id,
    /// binding)`.
    pub async fn contact_of_alias(&self, alias: &str) -> anyhow::Result<Option<(String, String)>> {
        let row: Option<(String, Option<String>)> = sqlx::query_as(
            "SELECT id, binding FROM contacts WHERE alias = ?1 AND removed_ms IS NULL \
             AND merged_into IS NULL \
             UNION ALL SELECT c.id, c.binding FROM contacts c JOIN peer_bindings b \
             ON c.binding = 'peer:' || b.id WHERE b.alias = ?1 AND b.revoked_at IS NULL \
             AND c.removed_ms IS NULL LIMIT 1",
        )
        .bind(alias)
        .fetch_optional(&self.read)
        .await?;
        let Some((id, binding)) = row else {
            return Ok(None);
        };
        Ok(Some((
            self.contact_root(&id).await?,
            binding.unwrap_or_default(),
        )))
    }

    /// Whether records `a` and `b` are one contact (merges followed).
    pub async fn share_root_eq(&self, a: &str, b: &str) -> anyhow::Result<bool> {
        Ok(self.contact_root(a).await? == self.contact_root(b).await?)
    }

    /// Restart: a queued download that had bytes starts over from byte 0.
    pub async fn file_requeue_queued(&self, id: &str) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query("UPDATE contact_files SET done = 0 WHERE id = ? AND state = 'queued'")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await
    }

    /// The record `id` merged into, followed to its root (at most 16 hops).
    async fn contact_root(&self, id: &str) -> anyhow::Result<String> {
        let mut id = id.to_owned();
        for _ in 0..16 {
            let next: Option<(Option<String>,)> =
                sqlx::query_as("SELECT merged_into FROM contacts WHERE id = ?")
                    .bind(&id)
                    .fetch_optional(&self.read)
                    .await?;
            match next.and_then(|(m,)| m).filter(|m| !m.is_empty()) {
                Some(m) => id = m,
                None => break,
            }
        }
        Ok(id)
    }

    /// D17 share re-check: `contact` (a record id, merges followed) is neither removed nor
    /// blocked, and still shares `session` with us: an export of it granted to the
    /// contact (no member refusing), or a live `~return` conversation into it.
    pub async fn share_live(&self, session: &str, contact: &str) -> anyhow::Result<bool> {
        let root = self.contact_root(contact).await?;
        let live: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM contacts WHERE id = ?1 AND removed_ms IS NULL \
             AND COALESCE(blocked, 0) = 0) AND (\
               (EXISTS (SELECT 1 FROM contact_grants g JOIN a2a_exports x ON x.id = g.export \
                 WHERE g.contact = ?1 AND g.granted = 1 AND x.session = ?2 AND x.removed_ms IS NULL) \
                AND NOT EXISTS (SELECT 1 FROM contact_grants g JOIN a2a_exports x ON x.id = g.export \
                 WHERE g.contact = ?1 AND g.granted = 0 AND x.session = ?2)) \
               OR EXISTS (SELECT 1 FROM contact_threads t JOIN contacts c ON c.id = ?1 \
                 WHERE 'peer:' || t.binding = c.binding AND t.session = ?2 AND t.last_ms >= ?3))",
        )
        .bind(&root)
        .bind(session)
        .bind(now_ms() - OFFER_TTL.as_millis() as i64)
        .fetch_one(&self.read)
        .await?;
        Ok(live)
    }
}

async fn insert_in(conn: &mut SqliteConnection, r: &FileRow) -> anyhow::Result<()> {
    sqlx::query(concat!(
        "INSERT INTO contact_files (",
        cols!(),
        ") VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
    ))
    .bind(&r.id)
    .bind(&r.direction)
    .bind(&r.session)
    .bind(&r.exchange)
    .bind(r.seq)
    .bind(&r.contact_id)
    .bind(&r.name)
    .bind(&r.media_type)
    .bind(r.size)
    .bind(&r.blake3)
    .bind(&r.source)
    .bind(&r.clone)
    .bind(r.dev)
    .bind(r.inode)
    .bind(&r.path)
    .bind(r.done)
    .bind(&r.state)
    .bind(&r.error)
    .bind(&r.notice)
    .bind(&r.notice_state)
    .bind(r.created_ms)
    .bind(r.deadline_ms)
    .execute(conn)
    .await?;
    Ok(())
}

async fn fail_in(
    conn: &mut SqliteConnection,
    r: &Render,
    id: &str,
    error: &str,
) -> anyhow::Result<Option<FileRow>> {
    let Some(mut row) = row_in(conn, id).await?.filter(|r| !r.terminal()) else {
        return Ok(None);
    };
    row.state = "failed".into();
    row.error = Some(error.to_owned());
    let ex = exchange_state_in(conn, &row.exchange).await?;
    if row.direction == "in" && read(ex.as_deref()) {
        let alias = alias_in(conn, &row.contact_id).await?;
        row.notice = row.notice_text(r, &alias);
        row.notice_state = Some("queued".into());
    }
    sqlx::query(
        "UPDATE contact_files SET state = 'failed', error = ?, notice = COALESCE(notice, ?), \
         notice_state = COALESCE(notice_state, ?) \
         WHERE id = ? AND state NOT IN ('received', 'sent', 'failed')",
    )
    .bind(&row.error)
    .bind(&row.notice)
    .bind(&row.notice_state)
    .bind(id)
    .execute(conn)
    .await?;
    Ok(Some(row))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::communication::AcceptExchange;
    use runtime::SessionId;

    async fn fresh() -> (Store, SessionId, crate::TestDir) {
        let scratch = crate::TestDir::new("cfiles");
        let store = Store::open(&scratch.0.join("sessions.sqlite"), None)
            .await
            .unwrap();
        let id = SessionId::new();
        store
            .create_session(id, "m", None, None, None)
            .await
            .unwrap();
        (store, id, scratch)
    }

    /// An inbound exchange from a contact into `session`.
    async fn inbound(store: &Store, session: SessionId) -> uuid::Uuid {
        let r = store
            .accept_exchange(&AcceptExchange {
                sender: "peer:00000000-0000-0000-0000-000000000001",
                recipient: &format!("session:{session}"),
                sender_session: None,
                recipient_session: Some(session),
                request_id: None,
                reply_to: None,
                request_scope: None,
                generation: 1,
                body: "hi",
            })
            .await
            .unwrap();
        r.id
    }

    fn in_row(session: SessionId, exchange: uuid::Uuid, id: &str) -> FileRow {
        FileRow {
            id: id.into(),
            direction: "in".into(),
            session: session.to_string(),
            exchange: exchange.to_string(),
            contact_id: "rec".into(),
            name: "a.txt".into(),
            media_type: "text/plain".into(),
            size: Some(3),
            blake3: Some("b".repeat(64)),
            source: format!("iroh://host/a2a/files/{}", "0".repeat(32)),
            state: "queued".into(),
            created_ms: 1,
            deadline_ms: i64::MAX,
            ..Default::default()
        }
    }

    async fn set_exchange(store: &Store, id: uuid::Uuid, state: &str) {
        sqlx::query("UPDATE peer_exchanges SET state = ? WHERE id = ?")
            .bind(state)
            .bind(id.to_string())
            .execute(&store.write)
            .await
            .unwrap();
    }

    fn render() -> Render {
        Render {
            holder: "h".into(),
            machine: "Mac".into(),
        }
    }

    /// States: a file published before its exchange is read waits `saved`; consumption
    /// makes it `received` with exactly one queued notice, however often settled.
    #[tokio::test]
    async fn saved_waits_for_its_exchange_then_one_notice() {
        let (store, s, _d) = fresh().await;
        let ex = inbound(&store, s).await;
        let row = in_row(s, ex, &"1".repeat(32));
        assert!(
            store
                .admit_in_files(&ex.to_string(), std::slice::from_ref(&row))
                .await
                .unwrap()
        );
        assert!(
            !store
                .admit_in_files(&ex.to_string(), std::slice::from_ref(&row))
                .await
                .unwrap()
        );
        store
            .file_to(&row.id, &["queued"], "receiving")
            .await
            .unwrap();
        assert!(
            store
                .file_publishing(&row.id, "/x/a.txt", 1, 2)
                .await
                .unwrap()
        );
        let r = render();
        let saved = store
            .file_published(&r, &row.id, 3, "c")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(saved.state, "saved");
        assert!(saved.notice.is_none());
        let skip = HashSet::new();
        assert!(store.settle_files(&r, &skip).await.unwrap().is_empty());
        set_exchange(&store, ex, "consumed").await;
        let done = store.settle_files(&r, &skip).await.unwrap();
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].1, Settled::Received);
        assert!(store.settle_files(&r, &skip).await.unwrap().is_empty());
        let now = store.contact_file(&row.id).await.unwrap().unwrap();
        assert_eq!(now.state, "received");
        assert_eq!(now.notice_state.as_deref(), Some("queued"));
        assert!(
            now.notice
                .unwrap()
                .starts_with("<message_file from=\"peer:rec\"")
        );
        assert_eq!(store.notice_outbox().await.unwrap().len(), 1);
    }

    /// A download failed before its text was consumed: consumption writes one failure
    /// notice and leaves the state; an exchange that fails instead writes none.
    #[tokio::test]
    async fn failed_file_notice_waits_for_the_exchange() {
        let (store, s, _d) = fresh().await;
        let r = render();
        let skip = HashSet::new();
        for (end, noticed) in [("consumed", true), ("failed", false)] {
            let ex = inbound(&store, s).await;
            let id = uuid::Uuid::new_v4().simple().to_string();
            store
                .admit_in_files(&ex.to_string(), &[in_row(s, ex, &id)])
                .await
                .unwrap();
            let f = store
                .file_fail(&r, &id, "the sender's file changed")
                .await
                .unwrap()
                .unwrap();
            assert!(f.notice.is_none());
            set_exchange(&store, ex, end).await;
            store.settle_files(&r, &skip).await.unwrap();
            store.settle_files(&r, &skip).await.unwrap();
            let now = store.contact_file(&id).await.unwrap().unwrap();
            assert_eq!(now.state, "failed");
            assert_eq!(now.error.as_deref(), Some("the sender's file changed"));
            assert_eq!(now.notice.is_some(), noticed, "{end}");
        }
    }

    /// HOLD_MOVED after a file was saved: the file fails with the exchange's reason and no
    /// notice is written.
    #[tokio::test]
    async fn an_ended_exchange_fails_its_files_without_notice() {
        let (store, s, _d) = fresh().await;
        let ex = inbound(&store, s).await;
        let (a, b) = (&"a".repeat(32), &"b".repeat(32));
        store
            .admit_in_files(&ex.to_string(), &[in_row(s, ex, a), in_row(s, ex, b)])
            .await
            .unwrap();
        store.file_to(a, &["queued"], "receiving").await.unwrap();
        store.file_publishing(a, "/x/a.txt", 1, 2).await.unwrap();
        let r = render();
        store.file_published(&r, a, 3, "c").await.unwrap();
        store
            .fail_exchange(ex, crate::communication::network::HOLD_MOVED)
            .await
            .unwrap();
        let done = store.settle_files(&r, &HashSet::new()).await.unwrap();
        assert_eq!(done.len(), 2);
        assert!(
            done.iter()
                .all(|(row, k)| *k == Settled::Failed && row.notice.is_none())
        );
        let a = store.contact_file(a).await.unwrap().unwrap();
        assert_eq!(
            a.error.as_deref(),
            Some(crate::communication::network::HOLD_MOVED)
        );
        assert_eq!(a.ino(), Some((1, 2)), "the inode stays for the rollback");
        assert!(store.notice_outbox().await.unwrap().is_empty());
    }

    /// Offers: the first terminal receipt wins; staged offers are adopted or dropped.
    #[tokio::test]
    async fn the_first_receipt_wins_and_staged_offers_move() {
        let (store, s, _d) = fresh().await;
        let id = "c".repeat(32);
        let row = FileRow {
            direction: "out".into(),
            exchange: "pending:r1:rec".into(),
            ..in_row(s, uuid::Uuid::nil(), &id)
        };
        store.insert_contact_files(&[row]).await.unwrap();
        assert_eq!(
            store.adopt_offers("pending:r1:rec", "ex1").await.unwrap(),
            1
        );
        assert_eq!(store.exchange_files("ex1").await.unwrap().len(), 1);
        assert!(
            store
                .drop_staged_offers("pending:%")
                .await
                .unwrap()
                .is_empty()
        );
        store.file_to(&id, &["queued"], "sending").await.unwrap();
        let sent = store.offer_settle(&id, None).await.unwrap().unwrap();
        assert_eq!((sent.state.as_str(), sent.done), ("sent", 3));
        assert!(
            store
                .offer_settle(&id, Some("late"))
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            store.contact_file(&id).await.unwrap().unwrap().state,
            "sent"
        );
    }

    /// 199-Z1 (found live): the sender restarts while its message is out, so the exchange
    /// reads `unknown` ("interrupted_by_restart") though the contact has its text and is
    /// fetching. Its offers stay (the receiver resumes; a receipt or the deadline settles
    /// them); an offer of a `failed` exchange fails with its reason.
    #[tokio::test]
    async fn an_unknown_exchange_keeps_its_offers_and_a_failed_one_ends_them() {
        let (store, s, _d) = fresh().await;
        let r = render();
        for (end, kept) in [("unknown", true), ("failed", false)] {
            let ex = inbound(&store, s).await;
            let id = uuid::Uuid::new_v4().simple().to_string();
            let row = FileRow {
                direction: "out".into(),
                ..in_row(s, ex, &id)
            };
            store.insert_contact_files(&[row]).await.unwrap();
            store.file_to(&id, &["queued"], "sending").await.unwrap();
            set_exchange(&store, ex, end).await;
            let done = store.settle_files(&r, &HashSet::new()).await.unwrap();
            assert_eq!(done.is_empty(), kept, "{end}");
            let now = store.contact_file(&id).await.unwrap().unwrap();
            assert_eq!(now.state, if kept { "sending" } else { "failed" }, "{end}");
        }
    }

    async fn exec(store: &Store, sql: &'static str) {
        sqlx::query(sql).execute(&store.write).await.unwrap();
    }

    /// D17 share re-check: a granted export of the chat (no member refusing), or a live
    /// return conversation into it; a blocked or removed contact shares nothing. A merged
    /// record is followed to its root.
    #[tokio::test]
    async fn the_share_re_check() {
        let (store, s, _d) = fresh().await;
        let sid = s.to_string();
        exec(
            &store,
            "INSERT INTO contacts (id, alias, binding) VALUES ('rec', 'maya', 'peer:b1'), \
             ('old', 'maya', 'peer:b0')",
        )
        .await;
        exec(
            &store,
            "UPDATE contacts SET merged_into = 'rec' WHERE id = 'old'",
        )
        .await;
        assert!(!store.share_live(&sid, "rec").await.unwrap());
        sqlx::query("INSERT INTO a2a_exports (id, name, session) VALUES ('x1', 'ops', ?)")
            .bind(&sid)
            .execute(&store.write)
            .await
            .unwrap();
        exec(
            &store,
            "INSERT INTO contact_grants (id, contact, export, granted) \
             VALUES ('rec|x1', 'rec', 'x1', 1)",
        )
        .await;
        assert!(store.share_live(&sid, "rec").await.unwrap());
        assert!(
            store.share_live(&sid, "old").await.unwrap(),
            "merged into rec"
        );
        let other = SessionId::new().to_string();
        assert!(!store.share_live(&other, "rec").await.unwrap());
        exec(&store, "UPDATE contacts SET blocked = 1 WHERE id = 'rec'").await;
        assert!(!store.share_live(&sid, "rec").await.unwrap());
        exec(&store, "UPDATE contacts SET blocked = 0 WHERE id = 'rec'").await;
        exec(&store, "DELETE FROM contact_grants").await;
        sqlx::query(
            "INSERT INTO contact_threads (id, binding, context_id, session, export, outbound, \
             last_ms) VALUES ('b1|c', 'b1', 'c', ?, '', 1, ?)",
        )
        .bind(&sid)
        .bind(now_ms())
        .execute(&store.write)
        .await
        .unwrap();
        assert!(
            store.share_live(&sid, "rec").await.unwrap(),
            "a live return conversation"
        );
        exec(
            &store,
            "UPDATE contacts SET removed_ms = 1 WHERE id = 'rec'",
        )
        .await;
        assert!(!store.share_live(&sid, "rec").await.unwrap());
    }
}
