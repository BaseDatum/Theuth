//! 191 (D2): the mesh-wide contact records — `contacts`, `contact_endpoints`, `contact_grants`
//! and `a2a_exports` (plus `contact_votes` from P2 and `contact_threads`) — ride the record
//! replica beside `projects`. Only the a2a plugin interprets them (its reconcile stages them
//! row by row and quarantines a bad one); this module reads them whole and writes single
//! columns, so a write never rolls back a column another machine changed since the read.
//! `contact_addrs` is local. `sessions.contact_inbox` (D4) is a session-log column.

use std::collections::{BTreeMap, BTreeSet};
use std::hash::{Hash, Hasher as _};

use runtime::SessionId;
use sqlx::FromRow;

use super::Store;

/// The replicated 191 tables, attached by [`Store::replicate_records`].
pub const RECORD_TABLES: [&str; 6] = [
    "contacts",
    "contact_endpoints",
    "contact_votes",
    "contact_grants",
    "a2a_exports",
    "contact_threads",
];

/// One `contacts` row. Every column may be NULL: a replicated row can arrive a column at a
/// time, and validity is the reconcile's.
#[allow(missing_docs)]
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, FromRow)]
pub struct ContactRow {
    pub id: String,
    pub alias: Option<String>,
    pub name: Option<String>,
    pub machine: Option<String>,
    /// `peer:<uuid>`: the binding id every machine uses for this contact.
    pub binding: Option<String>,
    pub paired_ms: Option<i64>,
    /// The engine id of our machine that ran the ceremony (or the import).
    pub paired_on: Option<String>,
    pub blocked: Option<i64>,
    /// Tombstone: set once, never cleared.
    pub removed_ms: Option<i64>,
    /// The record this one merged into (always a lower key), or NULL.
    pub merged_into: Option<String>,
}

/// One `contact_endpoints` row: `id` = `<contact>|<eid>`, an anchor a ceremony pinned.
#[allow(missing_docs)]
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, FromRow)]
pub struct EndpointRow {
    pub id: String,
    pub contact: Option<String>,
    pub eid: Option<String>,
    pub paired_ms: Option<i64>,
    /// NULL = an anchor; set when a later ceremony barred it (P2).
    pub barred_ms: Option<i64>,
}

/// One `contact_grants` row: `id` = `<contact>|<export id>`.
#[allow(missing_docs)]
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, FromRow)]
pub struct GrantRow {
    pub id: String,
    pub contact: Option<String>,
    pub export: Option<String>,
    pub granted: Option<i64>,
    /// `ask` | `allow` | `block`; NULL = ask.
    pub first_contact: Option<String>,
}

/// One `a2a_exports` row; exactly one of `session` and `agent` is set on a valid one.
#[allow(missing_docs)]
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, FromRow)]
pub struct ExportRow {
    pub id: String,
    pub name: Option<String>,
    pub session: Option<String>,
    pub agent: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    /// The engine id of the machine that made it.
    pub home: Option<String>,
    pub removed_ms: Option<i64>,
}

/// Everything the a2a reconcile stages, read in one snapshot. Its content (never a row's
/// `hlc`, which is a max over columns and misses a remote column write older than the
/// row's newest) is what [`Records::content_hash`] covers.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Records {
    /// This engine's id (`""` on an engine without a mesh identity).
    pub me: String,
    /// By id.
    pub contacts: Vec<ContactRow>,
    /// By id.
    pub endpoints: Vec<EndpointRow>,
    /// By id.
    pub grants: Vec<GrantRow>,
    /// By id.
    pub exports: Vec<ExportRow>,
    /// `contact_addrs`: endpoint id → the direct addresses this machine last saw.
    pub addrs: BTreeMap<String, Vec<String>>,
    /// Binding ids (bare uuids) revoked on this machine.
    pub revoked: BTreeSet<String>,
    /// Alias → live binding id here (what an import keeps).
    pub live_bindings: BTreeMap<String, String>,
    /// A chat export's session → whether it is native (`sessions.agent IS NULL`); absent
    /// when this machine has no such session (yet).
    pub sessions: BTreeMap<String, bool>,
}

impl Records {
    /// The reconcile's trigger: a hash over every staged value.
    pub fn content_hash(&self) -> u64 {
        let mut h = std::hash::DefaultHasher::new();
        self.hash(&mut h);
        h.finish()
    }
}

/// A value for [`RecordWrite`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cell {
    /// SQL NULL.
    Null,
    /// An integer.
    Int(i64),
    /// Text.
    Text(String),
}

/// The tables [`RecordWrite`] writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordTable {
    /// `contacts`.
    Contacts,
    /// `contact_endpoints`.
    Endpoints,
    /// `contact_grants`.
    Grants,
    /// `a2a_exports`.
    Exports,
}

impl RecordTable {
    fn name(self) -> &'static str {
        match self {
            Self::Contacts => "contacts",
            Self::Endpoints => "contact_endpoints",
            Self::Grants => "contact_grants",
            Self::Exports => "a2a_exports",
        }
    }

    fn columns(self) -> &'static [&'static str] {
        match self {
            Self::Contacts => &[
                "alias",
                "name",
                "machine",
                "binding",
                "paired_ms",
                "paired_on",
                "blocked",
                "removed_ms",
                "merged_into",
            ],
            Self::Endpoints => &["contact", "eid", "paired_ms", "barred_ms"],
            Self::Grants => &["contact", "export", "granted", "first_contact"],
            Self::Exports => &[
                "name",
                "session",
                "agent",
                "title",
                "description",
                "home",
                "removed_ms",
            ],
        }
    }
}

/// Set `set`'s columns of row `id` (inserting it when missing). Only the named columns are
/// written, so a column another machine changed since the caller's read keeps its value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordWrite {
    /// Which table.
    pub table: RecordTable,
    /// The row's key.
    pub id: String,
    /// Column → value; never empty.
    pub set: Vec<(&'static str, Cell)>,
}

impl RecordWrite {
    /// `set` on row `id` of `table`.
    pub fn new(table: RecordTable, id: impl Into<String>, set: Vec<(&'static str, Cell)>) -> Self {
        Self {
            table,
            id: id.into(),
            set,
        }
    }
}

impl Store {
    /// [`Records`], read in one snapshot.
    pub async fn contact_records(&self) -> anyhow::Result<Records> {
        let mut tx = self.read.begin().await?;
        let contacts = sqlx::query_as(
            "SELECT id, alias, name, machine, binding, paired_ms, paired_on, blocked, removed_ms, \
             merged_into FROM contacts ORDER BY id",
        )
        .fetch_all(&mut *tx)
        .await?;
        let endpoints = sqlx::query_as(
            "SELECT id, contact, eid, paired_ms, barred_ms FROM contact_endpoints ORDER BY id",
        )
        .fetch_all(&mut *tx)
        .await?;
        let grants = sqlx::query_as(
            "SELECT id, contact, export, granted, first_contact FROM contact_grants ORDER BY id",
        )
        .fetch_all(&mut *tx)
        .await?;
        let exports = sqlx::query_as(
            "SELECT id, name, session, agent, title, description, home, removed_ms \
             FROM a2a_exports ORDER BY id",
        )
        .fetch_all(&mut *tx)
        .await?;
        let addrs: Vec<(String, String)> = sqlx::query_as("SELECT eid, addrs FROM contact_addrs")
            .fetch_all(&mut *tx)
            .await?;
        let revoked: Vec<(String,)> =
            sqlx::query_as("SELECT id FROM peer_bindings WHERE revoked_at IS NOT NULL")
                .fetch_all(&mut *tx)
                .await?;
        let live: Vec<(String, String)> =
            sqlx::query_as("SELECT alias, id FROM peer_bindings WHERE revoked_at IS NULL")
                .fetch_all(&mut *tx)
                .await?;
        let sessions: Vec<(String, bool)> = sqlx::query_as(
            "SELECT id, agent IS NULL FROM sessions WHERE id IN \
             (SELECT session FROM a2a_exports WHERE session IS NOT NULL)",
        )
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(Records {
            me: self.me.to_string(),
            contacts,
            endpoints,
            grants,
            exports,
            addrs: addrs
                .into_iter()
                .map(|(eid, a)| {
                    (
                        eid,
                        a.split(',')
                            .filter(|s| !s.is_empty())
                            .map(str::to_owned)
                            .collect(),
                    )
                })
                .collect(),
            revoked: revoked.into_iter().map(|(id,)| id).collect(),
            live_bindings: live.into_iter().collect(),
            sessions: sessions.into_iter().collect(),
        })
    }

    /// Apply `writes` in one transaction ([`RecordWrite`]).
    pub async fn write_contact_records(&self, writes: &[RecordWrite]) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        for w in writes {
            anyhow::ensure!(!w.set.is_empty(), "{}: empty write", w.table.name());
            for (col, _) in &w.set {
                anyhow::ensure!(
                    w.table.columns().contains(col),
                    "{}: no column {col}",
                    w.table.name()
                );
            }
            let cols: Vec<&str> = w.set.iter().map(|(c, _)| *c).collect();
            let sql = format!(
                "INSERT INTO {} (id, {}) VALUES (?{}) ON CONFLICT (id) DO UPDATE SET {}",
                w.table.name(),
                cols.join(", "),
                ", ?".repeat(cols.len()),
                cols.iter()
                    .map(|c| format!("{c} = excluded.{c}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            let mut q = sqlx::query(sqlx::AssertSqlSafe(sql)).bind(&w.id);
            for (_, v) in &w.set {
                q = match v {
                    Cell::Null => q.bind(None::<String>),
                    Cell::Int(i) => q.bind(*i),
                    Cell::Text(t) => q.bind(t.as_str()),
                };
            }
            q.execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// `contact_addrs` (local): the direct addresses last seen for `eid`.
    pub async fn set_contact_addrs(&self, eid: &str, addrs: &[String]) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO contact_addrs (eid, addrs, at_ms) VALUES (?, ?, ?) \
             ON CONFLICT (eid) DO UPDATE SET addrs = excluded.addrs, at_ms = excluded.at_ms",
        )
        .bind(eid)
        .bind(addrs.join(","))
        .bind(chrono::Utc::now().timestamp_millis())
        .execute(&self.write)
        .await?;
        Ok(())
    }

    /// 191 (D4): `session`'s inbox marker (`<binding>|<export id>`), if any.
    pub async fn contact_inbox(&self, session: SessionId) -> anyhow::Result<Option<String>> {
        let row: Option<(Option<String>,)> =
            sqlx::query_as("SELECT contact_inbox FROM sessions WHERE id = ?")
                .bind(session.to_string())
                .fetch_optional(&self.read)
                .await?;
        Ok(row.and_then(|(m,)| m))
    }

    /// 191 (D4): mark `session` as the inbox `marker` names. The session log has one writer,
    /// the owner: a session another machine owns is left as it is (`Ok(false)`).
    pub async fn set_contact_inbox(
        &self,
        session: SessionId,
        marker: &str,
    ) -> anyhow::Result<bool> {
        let mut tx = self.begin_write().await?;
        let n = sqlx::query(
            "UPDATE sessions SET contact_inbox = ?1 WHERE id = ?2 AND (owner = ?3 OR owner = '') \
             AND contact_inbox IS NOT ?1",
        )
        .bind(marker)
        .bind(session.to_string())
        .bind(&*self.me)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        tx.commit().await?;
        Ok(n > 0)
    }

    /// 191 (D2 import): every local inbox (`a2a_contexts.inbox = 1`) of a session this machine
    /// owns and without a marker gets `<principal>|<export id>`, its export's name mapped
    /// through `exports` (name → id). Returns how many were marked.
    pub async fn backfill_contact_inboxes(
        &self,
        exports: &BTreeMap<String, String>,
    ) -> anyhow::Result<u64> {
        let rows: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT DISTINCT c.session, c.principal, c.export FROM a2a_contexts c \
             JOIN sessions s ON s.id = c.session \
             WHERE c.inbox = 1 AND s.contact_inbox IS NULL AND (s.owner = ? OR s.owner = '')",
        )
        .bind(&*self.me)
        .fetch_all(&self.read)
        .await?;
        let mut n = 0;
        for (session, principal, export) in rows {
            let Some(id) = exports.get(&export) else {
                continue;
            };
            let Ok(session) = session.parse() else {
                continue;
            };
            if self
                .set_contact_inbox(session, &format!("{principal}|{id}"))
                .await?
            {
                n += 1;
            }
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::communication::A2aScope;

    async fn fresh() -> (Store, SessionId, crate::TestDir) {
        let scratch = crate::TestDir::new("contacts");
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

    /// 191 (D2, M1): a row's `hlc` is the max over its columns, so a remote write of an
    /// older column changes the row and leaves its `hlc` as it was; the content hash the
    /// reconcile keys on still moves.
    #[tokio::test]
    async fn the_content_hash_moves_on_a_column_write_that_leaves_hlc_alone() {
        let (store, _, _dir) = fresh().await;
        store
            .write_contact_records(&[RecordWrite::new(
                RecordTable::Contacts,
                "c1",
                vec![
                    ("alias", Cell::Text("bob".into())),
                    ("blocked", Cell::Int(0)),
                ],
            )])
            .await
            .unwrap();
        sqlx::query("UPDATE contacts SET hlc = '0000000000002-0000-x' WHERE id = 'c1'")
            .execute(&store.write)
            .await
            .unwrap();
        let before = store.contact_records().await.unwrap().content_hash();
        // What the replica's apply does for a remote `blocked` older than the row's newest
        // column: that column only.
        sqlx::query("UPDATE contacts SET blocked = 1 WHERE id = 'c1'")
            .execute(&store.write)
            .await
            .unwrap();
        let hlc: String = sqlx::query_scalar("SELECT hlc FROM contacts WHERE id = 'c1'")
            .fetch_one(&store.read)
            .await
            .unwrap();
        assert_eq!(hlc, "0000000000002-0000-x", "the row's hlc did not move");
        let after = store.contact_records().await.unwrap();
        assert_ne!(after.content_hash(), before);
        assert_eq!(after.contacts[0].blocked, Some(1));
        // A write names only its columns: the others keep what another machine wrote.
        store
            .write_contact_records(&[RecordWrite::new(
                RecordTable::Contacts,
                "c1",
                vec![("alias", Cell::Text("robert".into()))],
            )])
            .await
            .unwrap();
        let row = &store.contact_records().await.unwrap().contacts[0];
        assert_eq!(
            (row.alias.as_deref(), row.blocked),
            (Some("robert"), Some(1))
        );
    }

    /// 191 (D4): the import marks every local inbox of a session this machine owns with
    /// `<principal>|<export id>` (its export's name mapped to its id); a session another
    /// machine owns is left alone (one writer per session row), and the marker is a session
    /// log write, so it travels with the session row.
    #[tokio::test]
    async fn the_import_backfills_inbox_markers_on_owned_sessions_only() {
        let (store, mine, _dir) = fresh().await;
        let theirs = SessionId::new();
        store
            .create_session(theirs, "m", None, None, None)
            .await
            .unwrap();
        for (s, export) in [(mine, "research"), (theirs, "research")] {
            let scope = A2aScope {
                principal: "peer:b1",
                export,
            };
            store
                .create_a2a_context(scope, &format!("ctx-{s}"), s)
                .await
                .unwrap();
            store
                .mark_a2a_inbox(s, None, "research", true, None)
                .await
                .unwrap();
        }
        // As the session log would apply it: on a connection the capture session is not on.
        sqlx::query("UPDATE sessions SET owner = 'other-engine' WHERE id = ?")
            .bind(theirs.to_string())
            .execute(&store.read)
            .await
            .unwrap();
        let seq = store
            .log_after(0)
            .await
            .unwrap()
            .last()
            .map_or(0, |r| r.seq);
        let ids = BTreeMap::from([("research".to_owned(), "x1".to_owned())]);
        assert_eq!(store.backfill_contact_inboxes(&ids).await.unwrap(), 1);
        assert_eq!(
            store.contact_inbox(mine).await.unwrap().as_deref(),
            Some("peer:b1|x1")
        );
        assert_eq!(store.contact_inbox(theirs).await.unwrap(), None);
        let logged = store.log_after(seq).await.unwrap();
        assert!(
            logged.iter().any(|r| r.session_id == mine.to_string()),
            "the marker is in the session log"
        );
        assert_eq!(
            store.backfill_contact_inboxes(&ids).await.unwrap(),
            0,
            "marked once"
        );
    }
}
