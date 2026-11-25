//! 191 (D2): the mesh-wide contact records — `contacts`, `contact_endpoints`, `contact_grants`
//! and `a2a_exports` (plus, from P2, `contact_rosters` and `contact_roster_heads`, from P3
//! `contact_holds`, and `contact_threads`) — ride the record replica beside `projects`. Only
//! the a2a plugin interprets them (its reconcile stages them row by row and quarantines a bad
//! one); this module reads them whole and writes single columns, so a write never rolls back
//! a column another machine changed since the read. `contact_addrs` and
//! `contact_roster_local` are local. `sessions.contact_inbox` (D4) is a session-log column.

use std::collections::{BTreeMap, BTreeSet};
use std::hash::{Hash, Hasher};

use runtime::SessionId;
use sqlx::FromRow;

use super::Store;

/// The replicated 191 tables, attached by [`Store::replicate_records`].
pub const RECORD_TABLES: [&str; 8] = [
    "contacts",
    "contact_endpoints",
    "contact_grants",
    "a2a_exports",
    "contact_threads",
    "contact_rosters",
    "contact_roster_heads",
    "contact_holds",
];

/// 191 (D3): whose a session is, as the contact paths read it ([`Store::owner_strict`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Owner {
    /// This engine.
    Here,
    /// Another machine of the mesh: its engine id.
    Elsewhere(String),
    /// Neither: no row here, or an empty owner on a mesh engine.
    Unknown,
}

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
    /// 200 (D1): this record's full access: 1 on; NULL or 0 off (fail closed).
    pub full_access: Option<i64>,
}

/// One `contact_endpoints` row: `id` = `<contact>|<eid>`, an anchor a ceremony pinned.
#[allow(missing_docs)]
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, FromRow)]
pub struct EndpointRow {
    pub id: String,
    pub contact: Option<String>,
    pub eid: Option<String>,
    pub paired_ms: Option<i64>,
    /// NULL = an anchor; set when a later ceremony barred it (unused since D1's roster).
    pub barred_ms: Option<i64>,
    /// 191 (P2): the engine ids of our machines whose ceremonies wrote this pin, a JSON array
    /// (a set: a later ceremony adds itself; NULL before P2, a bare id from P2's first builds).
    pub pinned_by: Option<String>,
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

/// 191 (D1, P2): one `contact_rosters` row, a signed copy of ours: `id` =
/// `<contact>|<chain>|<version>|<signer>`, immutable once written.
#[allow(missing_docs)]
#[derive(Clone, Debug, Default, PartialEq, Eq, FromRow)]
pub struct RosterRow {
    pub id: String,
    pub contact: Option<String>,
    pub chain: Option<String>,
    pub version: Option<i64>,
    pub signer: Option<String>,
    pub hash: Option<String>,
    /// The signed copy, JSON.
    pub roster: Option<String>,
}

/// The `contact_rosters` rows and what the reconcile keys on: the rows are immutable, so
/// `(count, max hlc)` and their ids stand for their content (D2). The ids are hashed too: a
/// delete plus an older row's late arrival can leave both stamp values equal.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rosters {
    /// By id.
    pub rows: Vec<RosterRow>,
    /// `(count, max hlc)`.
    pub stamp: (i64, Option<String>),
}

impl Hash for Rosters {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.stamp.hash(state);
        for r in &self.rows {
            r.id.hash(state);
        }
    }
}

/// 191 (D1, P2): one `contact_roster_heads` row: `id` = `<record>|<machine eid>`, written
/// only by that machine.
#[allow(missing_docs)]
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, FromRow)]
pub struct HeadRow {
    pub id: String,
    pub record: Option<String>,
    pub machine: Option<String>,
    pub chain: Option<String>,
    pub version: Option<i64>,
    pub hash: Option<String>,
    /// The last 64 `[version, hash]` pairs, JSON.
    pub history: Option<String>,
    /// The head's members, JSON (untrusted dial hints).
    pub members: Option<String>,
    pub conflict: Option<i64>,
    pub at_ms: Option<i64>,
}

/// 191 (D1, P2): this machine's `contact_roster_local` row (never replicated).
#[allow(missing_docs)]
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, FromRow)]
pub struct LocalRow {
    pub record: String,
    pub chain: String,
    pub version: i64,
    pub hash: String,
    /// The head, JSON.
    pub roster: String,
    /// The last 64 `[version, hash, members]` entries, JSON.
    pub history: String,
}

/// 191 (D4, P3): one `contact_holds` row: `id` = the oldest held task's id of one (contact,
/// export) on `owner`. Another machine's row is data: every column may be NULL or hostile,
/// and the reader cleans and cuts it.
#[allow(missing_docs)]
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, FromRow)]
pub struct HoldRow {
    pub id: String,
    /// The contact's alias.
    pub contact: Option<String>,
    /// The contact's machine, from its card.
    pub machine: Option<String>,
    /// The export's name.
    pub export: Option<String>,
    pub session: Option<String>,
    /// The engine id that holds the tasks.
    pub owner: Option<String>,
    pub since_ms: Option<i64>,
    pub waiting: Option<i64>,
    pub preview: Option<String>,
    /// When the newest held task expires (ms since epoch).
    pub until_ms: Option<i64>,
    /// Read only, never written: `session`'s row here, by its `contact_inbox` marker
    /// (`<principal>|<export id>`): `None` when no such row is here, `""` for a session that
    /// is no inbox. The reader checks an agent export's row names that export's inbox.
    pub inbox: Option<String>,
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
    /// Alias → (live binding id, its endpoint) here (what an import keeps).
    pub live_bindings: BTreeMap<String, (String, String)>,
    /// A chat export's session → whether it is native (`sessions.agent IS NULL`); absent
    /// when this machine has no such session (yet).
    pub sessions: BTreeMap<String, bool>,
    /// 191 (P2): `contact_rosters`, hashed by `(count, max hlc)` and the row ids.
    pub rosters: Rosters,
    /// 191 (P2): `contact_roster_heads`, by id.
    pub heads: Vec<HeadRow>,
    /// 191 (P2): `contact_roster_local`, by record.
    pub local: Vec<LocalRow>,
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
    /// `contact_rosters` (P2): immutable rows, inserted or pruned whole.
    Rosters,
    /// `contact_roster_heads` (P2): this machine's own rows only.
    Heads,
    /// `contact_roster_local` (P2, local): keyed by `record`.
    Local,
    /// `contact_holds` (P3): the owner's own rows only.
    Holds,
}

impl RecordTable {
    fn name(self) -> &'static str {
        match self {
            Self::Contacts => "contacts",
            Self::Endpoints => "contact_endpoints",
            Self::Grants => "contact_grants",
            Self::Exports => "a2a_exports",
            Self::Rosters => "contact_rosters",
            Self::Heads => "contact_roster_heads",
            Self::Local => "contact_roster_local",
            Self::Holds => "contact_holds",
        }
    }

    fn pk(self) -> &'static str {
        match self {
            Self::Local => "record",
            _ => "id",
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
                "full_access",
            ],
            Self::Endpoints => &["contact", "eid", "paired_ms", "barred_ms", "pinned_by"],
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
            Self::Rosters => &["contact", "chain", "version", "signer", "hash", "roster"],
            Self::Heads => &[
                "record", "machine", "chain", "version", "hash", "history", "members", "conflict",
                "at_ms",
            ],
            Self::Local => &["chain", "version", "hash", "roster", "history"],
            Self::Holds => &[
                "contact", "machine", "export", "session", "owner", "since_ms", "waiting",
                "preview", "until_ms",
            ],
        }
    }
}

/// Set `set`'s columns of row `id` (inserting it when missing). Only the named columns are
/// written, so a column another machine changed since the caller's read keeps its value.
/// An empty `set` deletes the row ([`RecordWrite::delete`]: P2 prunes roster rows).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordWrite {
    /// Which table.
    pub table: RecordTable,
    /// The row's key.
    pub id: String,
    /// Column → value; empty = delete the row.
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

    /// Delete row `id` of `table`.
    pub fn delete(table: RecordTable, id: impl Into<String>) -> Self {
        Self::new(table, id, Vec::new())
    }
}

impl Store {
    /// [`Records`], read in one snapshot.
    pub async fn contact_records(&self) -> anyhow::Result<Records> {
        let mut tx = self.read.begin().await?;
        let contacts = sqlx::query_as(
            "SELECT id, alias, name, machine, binding, paired_ms, paired_on, blocked, removed_ms, \
             merged_into, full_access FROM contacts ORDER BY id",
        )
        .fetch_all(&mut *tx)
        .await?;
        let endpoints = sqlx::query_as(
            "SELECT id, contact, eid, paired_ms, barred_ms, pinned_by FROM contact_endpoints \
             ORDER BY id",
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
        let live: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT alias, id, endpoint FROM peer_bindings WHERE revoked_at IS NULL",
        )
        .fetch_all(&mut *tx)
        .await?;
        let sessions: Vec<(String, bool)> = sqlx::query_as(
            "SELECT id, agent IS NULL FROM sessions WHERE id IN \
             (SELECT session FROM a2a_exports WHERE session IS NOT NULL)",
        )
        .fetch_all(&mut *tx)
        .await?;
        let rows = sqlx::query_as(
            "SELECT id, contact, chain, version, signer, hash, roster FROM contact_rosters \
             ORDER BY id",
        )
        .fetch_all(&mut *tx)
        .await?;
        let stamp: (i64, Option<String>) =
            sqlx::query_as("SELECT COUNT(*), MAX(hlc) FROM contact_rosters")
                .fetch_one(&mut *tx)
                .await?;
        let heads = sqlx::query_as(
            "SELECT id, record, machine, chain, version, hash, history, members, conflict, \
             at_ms FROM contact_roster_heads ORDER BY id",
        )
        .fetch_all(&mut *tx)
        .await?;
        let local = sqlx::query_as(
            "SELECT record, chain, version, hash, roster, history FROM contact_roster_local \
             ORDER BY record",
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
            live_bindings: live
                .into_iter()
                .map(|(alias, id, endpoint)| (alias, (id, endpoint)))
                .collect(),
            sessions: sessions.into_iter().collect(),
            rosters: Rosters { rows, stamp },
            heads,
            local,
        })
    }

    /// Apply `writes` in one transaction ([`RecordWrite`]; an empty `set` deletes).
    pub async fn write_contact_records(&self, writes: &[RecordWrite]) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        for w in writes {
            let (table, pk) = (w.table.name(), w.table.pk());
            if w.set.is_empty() {
                sqlx::query(sqlx::AssertSqlSafe(format!(
                    "DELETE FROM {table} WHERE {pk} = ?"
                )))
                .bind(&w.id)
                .execute(&mut *tx)
                .await?;
                continue;
            }
            for (col, _) in &w.set {
                anyhow::ensure!(w.table.columns().contains(col), "{table}: no column {col}");
            }
            let cols: Vec<&str> = w.set.iter().map(|(c, _)| *c).collect();
            let sql = format!(
                "INSERT INTO {table} ({pk}, {}) VALUES (?{}) ON CONFLICT ({pk}) DO UPDATE SET {}",
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

    /// 200 D1's group rule on the records: we give the contact of record `contact` full
    /// access (every live record of its group reads 1, none blocked). With the live route's
    /// grant and D5's `visible`, 199 D17's full-access branch (SEC-200-1).
    pub async fn full_access_granted(&self, contact: &str) -> anyhow::Result<bool> {
        let rows: Vec<(String, Option<i64>, Option<i64>)> = sqlx::query_as(
            "SELECT id, full_access, blocked FROM contacts WHERE removed_ms IS NULL",
        )
        .fetch_all(&self.read)
        .await?;
        let mut group = false;
        for (id, full_access, blocked) in rows {
            if self.share_root_eq(&id, contact).await? {
                if full_access != Some(1) || blocked.unwrap_or(0) != 0 {
                    return Ok(false);
                }
                group = true;
            }
        }
        Ok(group)
    }

    /// 191 (D3, S4): whose `session` is, strictly: this engine's when its row exists and reads
    /// here and this engine owns it; another machine's when the row names one; else unknown
    /// (no row, or an empty owner on a mesh engine), unlike [`super::Store`]'s `here`, which
    /// reads empty as local. A lone engine (no mesh id: `me` is `""`) owns its own `''` rows.
    /// An unreadable row is `Err`. The predicate of every contact path.
    pub async fn owner_strict(&self, session: SessionId) -> anyhow::Result<Owner> {
        Ok(match self.session(session).await? {
            Some(row) if row.owner == *self.me => Owner::Here,
            Some(row) if !row.owner.is_empty() => Owner::Elsewhere(row.owner),
            _ => Owner::Unknown,
        })
    }

    /// 191 (D4): the local inbox rows of `(principal, export)` (`a2a_contexts`): each session
    /// once, oldest first, with its owner and its marker.
    pub async fn inbox_rows(
        &self,
        principal: &str,
        export: &str,
    ) -> anyhow::Result<Vec<(SessionId, String, Option<String>)>> {
        let rows: Vec<(String, String, Option<String>)> = sqlx::query_as(
            "SELECT c.session, s.owner, s.contact_inbox FROM a2a_contexts c \
             JOIN sessions s ON s.id = c.session WHERE c.principal = ? AND c.export = ? \
             GROUP BY c.session ORDER BY MIN(c.created_at), c.session",
        )
        .bind(principal)
        .bind(export)
        .fetch_all(&self.read)
        .await?;
        rows.into_iter()
            .map(|(id, owner, marker)| {
                let id = id
                    .parse()
                    .map_err(|e| anyhow::anyhow!("a2a_contexts.session {id}: {e}"))?;
                Ok((id, owner, marker))
            })
            .collect()
    }

    /// 191 (D4): the sessions whose inbox marker is `marker`, with their owners, oldest first.
    pub async fn marked_inboxes(&self, marker: &str) -> anyhow::Result<Vec<(SessionId, String)>> {
        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT id, owner FROM sessions WHERE contact_inbox = ? ORDER BY created_at, id",
        )
        .bind(marker)
        .fetch_all(&self.read)
        .await?;
        rows.into_iter()
            .map(|(id, owner)| {
                let id = id
                    .parse()
                    .map_err(|e| anyhow::anyhow!("sessions.id {id}: {e}"))?;
                Ok((id, owner))
            })
            .collect()
    }

    /// 191 (D4): `session` is a marked inbox: its marker (`<principal>|<export id>`) is set,
    /// reads, and names a live `a2a_exports` row by id. A name-valued marker, one naming a
    /// tombstoned or unknown export, and no marker are not.
    pub async fn inbox_marked(&self, session: SessionId) -> anyhow::Result<bool> {
        Ok(sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM sessions s JOIN a2a_exports x \
             ON x.id = substr(s.contact_inbox, instr(s.contact_inbox, '|') + 1) \
             WHERE s.id = ? AND instr(s.contact_inbox, '|') > 0 AND x.removed_ms IS NULL)",
        )
        .bind(session.to_string())
        .fetch_one(&self.read)
        .await?)
    }

    /// 191 (D4, P3): every `contact_holds` row (every machine's), by id, with its session's
    /// `contact_inbox` here. Replicated cells keep whatever type another machine wrote, so
    /// the cells are typed in SQL: a cell of another type reads NULL (the reader drops the
    /// row), never failing the read of every other row; text is cut to 256 characters, a
    /// row whose id is not text of at most 256 is skipped, and at most 512 rows are read
    /// (one per held (contact, export) on each machine).
    pub async fn contact_holds(&self) -> anyhow::Result<Vec<HoldRow>> {
        self.hold_rows(false).await
    }

    /// [`Self::contact_holds`], this engine's own rows only (the holds mirror's read-back;
    /// P3 final review N2: filtered in SQL, so another machine's rows never push ours past
    /// the cap).
    pub async fn own_contact_holds(&self) -> anyhow::Result<Vec<HoldRow>> {
        self.hold_rows(true).await
    }

    async fn hold_rows(&self, own: bool) -> anyhow::Result<Vec<HoldRow>> {
        Ok(sqlx::query_as(
            "SELECT h.id, \
             CASE WHEN typeof(h.contact) = 'text' THEN substr(h.contact, 1, 256) END AS contact, \
             CASE WHEN typeof(h.machine) = 'text' THEN substr(h.machine, 1, 256) END AS machine, \
             CASE WHEN typeof(h.export) = 'text' THEN substr(h.export, 1, 256) END AS export, \
             CASE WHEN typeof(h.session) = 'text' THEN substr(h.session, 1, 256) END AS session, \
             CASE WHEN typeof(h.owner) = 'text' THEN substr(h.owner, 1, 256) END AS owner, \
             CASE WHEN typeof(h.since_ms) = 'integer' THEN h.since_ms END AS since_ms, \
             CASE WHEN typeof(h.waiting) = 'integer' THEN h.waiting END AS waiting, \
             CASE WHEN typeof(h.preview) = 'text' THEN substr(h.preview, 1, 256) END AS preview, \
             CASE WHEN typeof(h.until_ms) = 'integer' THEN h.until_ms END AS until_ms, \
             CASE WHEN s.id IS NULL THEN NULL WHEN typeof(s.contact_inbox) = 'text' \
             THEN substr(s.contact_inbox, 1, 256) ELSE '' END AS inbox \
             FROM contact_holds h LEFT JOIN sessions s ON s.id = h.session \
             WHERE typeof(h.id) = 'text' AND length(h.id) <= 256 \
             AND (NOT ?1 OR (typeof(h.owner) = 'text' AND h.owner = ?2)) ORDER BY h.id LIMIT 512",
        )
        .bind(own)
        .bind(&*self.me)
        .fetch_all(&self.read)
        .await?)
    }

    /// 191 (D4, P3): at boot, this engine's own `contact_holds` rows go: its holds lived in
    /// memory and were failed by the restart. Returns how many.
    pub async fn clear_own_contact_holds(&self) -> anyhow::Result<u64> {
        let mut tx = self.begin_write().await?;
        let n = sqlx::query("DELETE FROM contact_holds WHERE owner = ?")
            .bind(&*self.me)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        tx.commit().await?;
        Ok(n)
    }

    /// Tests: `session` is owned by `owner`, as the session log applies another machine's
    /// take (off the capture connection).
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) async fn owned_elsewhere(&self, session: SessionId, owner: &str) {
        sqlx::query("UPDATE sessions SET owner = ? WHERE id = ?")
            .bind(owner)
            .bind(session.to_string())
            .execute(&self.read)
            .await
            .unwrap();
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

    /// 191 (D2, D4): every local inbox (`a2a_contexts.inbox = 1`) of a session this machine
    /// owns and without a marker gets `<principal>|<export id>`, its export's name mapped
    /// through `exports` (name → id); an export no longer known by name keeps its name there
    /// (the marker's presence is what caps a moved inbox). Returns how many were marked.
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
            let id = exports.get(&export).unwrap_or(&export);
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
        let scratch = crate::test_dir("contacts");
        let store = Store::open(&scratch.path().join("sessions.sqlite"), None)
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

    /// 191 (P2, review NIT): a roster row pruned while an older one arrives late leaves the
    /// `(count, max hlc)` stamp as it was; the content hash still moves (the ids are in it).
    #[tokio::test]
    async fn the_content_hash_moves_when_a_prune_and_a_late_row_keep_the_stamp() {
        let (store, _, _dir) = fresh().await;
        let row = |id: &str| {
            RecordWrite::new(
                RecordTable::Rosters,
                id,
                vec![("contact", Cell::Text("c".into()))],
            )
        };
        let hlc = |id: &'static str, hlc: &'static str| {
            sqlx::query("UPDATE contact_rosters SET hlc = ? WHERE id = ?")
                .bind(hlc)
                .bind(id)
                .execute(&store.write)
        };
        store
            .write_contact_records(&[row("a"), row("b")])
            .await
            .unwrap();
        hlc("a", "0000000000005-0000-x").await.unwrap();
        hlc("b", "0000000000003-0000-x").await.unwrap();
        let before = store.contact_records().await.unwrap();
        store
            .write_contact_records(&[RecordWrite::delete(RecordTable::Rosters, "b"), row("c")])
            .await
            .unwrap();
        hlc("c", "0000000000002-0000-x").await.unwrap();
        let after = store.contact_records().await.unwrap();
        assert_eq!(after.rosters.stamp, before.rosters.stamp);
        assert_ne!(after.content_hash(), before.content_hash());
    }

    /// 191 (D3, S4): the contact paths' owner predicate is strict: this engine's row is ours;
    /// an unknown row and an empty owner on a mesh engine (where `here` reads it as local) are
    /// unknown, another engine's names it, and an unreadable row is an error (refused). A lone
    /// engine (no mesh id) owns its own `''` rows.
    #[tokio::test]
    async fn only_a_row_this_engine_owns_is_owned_here() {
        let scratch = crate::test_dir("contacts");
        let store = Store::open(
            &scratch.path().join("sessions.sqlite"),
            Some(mesh::EngineId("me-engine".into())),
        )
        .await
        .unwrap();
        let id = SessionId::new();
        store
            .create_session(id, "m", None, None, None)
            .await
            .unwrap();
        assert_eq!(store.owner_strict(id).await.unwrap(), Owner::Here);
        assert_eq!(
            store.owner_strict(SessionId::new()).await.unwrap(),
            Owner::Unknown
        );
        store.owned_elsewhere(id, "").await;
        assert!(
            store.session(id).await.unwrap().unwrap().here,
            "`here` reads it as local"
        );
        assert_eq!(
            store.owner_strict(id).await.unwrap(),
            Owner::Unknown,
            "empty, on a mesh engine"
        );
        store.owned_elsewhere(id, "other-engine").await;
        assert_eq!(
            store.owner_strict(id).await.unwrap(),
            Owner::Elsewhere("other-engine".into())
        );
        store.owned_elsewhere(id, "me-engine").await;
        store.break_session_for_test(id).await;
        assert!(store.owner_strict(id).await.is_err(), "unreadable");
        // A lone engine: its rows are `''` and so is its id.
        let (lone, mine, _dir) = fresh().await;
        assert_eq!(lone.owner_strict(mine).await.unwrap(), Owner::Here);
        lone.owned_elsewhere(mine, "other-engine").await;
        assert_eq!(
            lone.owner_strict(mine).await.unwrap(),
            Owner::Elsewhere("other-engine".into())
        );
    }

    /// 191 (D4): an inbox is marked only by a marker naming a live export by id: a
    /// name-valued marker, a tombstoned export's and no marker are not; the marker lookup finds
    /// every session carrying it.
    #[tokio::test]
    async fn an_inbox_is_marked_only_by_a_live_export_id() {
        let (store, mine, _dir) = fresh().await;
        let other = SessionId::new();
        store
            .create_session(other, "m", None, None, None)
            .await
            .unwrap();
        assert!(!store.inbox_marked(mine).await.unwrap(), "no marker");
        store
            .write_contact_records(&[
                RecordWrite::new(
                    RecordTable::Exports,
                    "x-live",
                    vec![("name", Cell::Text("research".into()))],
                ),
                RecordWrite::new(
                    RecordTable::Exports,
                    "x-gone",
                    vec![("removed_ms", Cell::Int(1))],
                ),
            ])
            .await
            .unwrap();
        for (marker, marked) in [
            ("peer:b|research", false),
            ("peer:b|x-gone", false),
            ("peer:b|x-unknown", false),
            ("peer:b|x-live", true),
        ] {
            store.set_contact_inbox(mine, marker).await.unwrap();
            assert_eq!(store.inbox_marked(mine).await.unwrap(), marked, "{marker}");
        }
        store
            .set_contact_inbox(other, "peer:b|x-live")
            .await
            .unwrap();
        let found = store.marked_inboxes("peer:b|x-live").await.unwrap();
        assert_eq!(
            found
                .iter()
                .map(|(s, _)| *s)
                .collect::<std::collections::HashSet<_>>(),
            std::collections::HashSet::from([mine, other])
        );
        assert!(
            store
                .marked_inboxes("peer:b|research")
                .await
                .unwrap()
                .is_empty()
        );
    }

    /// 191 (D4): every local inbox of a session this machine owns gets
    /// `<principal>|<export id>` (its export's name mapped to its id; an export no longer
    /// known by name keeps its name); a session another machine owns is left alone (one
    /// writer per session row), and the marker is a session log write, so it travels with
    /// the session row.
    #[tokio::test]
    async fn the_import_backfills_inbox_markers_on_owned_sessions_only() {
        let (store, mine, _dir) = fresh().await;
        let (theirs, unknown) = (SessionId::new(), SessionId::new());
        for s in [theirs, unknown] {
            store
                .create_session(s, "m", None, None, None)
                .await
                .unwrap();
        }
        for (s, export) in [(mine, "research"), (theirs, "research"), (unknown, "gone")] {
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
        assert_eq!(store.backfill_contact_inboxes(&ids).await.unwrap(), 2);
        assert_eq!(
            store.contact_inbox(unknown).await.unwrap().as_deref(),
            Some("peer:b1|gone"),
            "marked whatever the file knew"
        );
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

    /// `contact_holds` row `id` with every column `owner` mirrors.
    fn hold(id: &str, owner: &str) -> RecordWrite {
        RecordWrite::new(
            RecordTable::Holds,
            id,
            vec![
                ("contact", Cell::Text("bob".into())),
                ("export", Cell::Text("research".into())),
                ("owner", Cell::Text(owner.into())),
                ("since_ms", Cell::Int(1)),
                ("waiting", Cell::Int(1)),
                ("until_ms", Cell::Int(2)),
            ],
        )
    }

    /// 191 (P3 review M1): replicated cells keep the type another machine wrote. A row with
    /// text in an integer column or a blob in a text column reads those cells NULL (the
    /// reader then drops it) and never fails the read of the good rows; a row whose id is
    /// not text is skipped; long text is cut to 256 characters; at most 512 rows are read.
    #[tokio::test]
    async fn a_bad_typed_hold_row_never_hides_the_others() {
        let (store, _, _dir) = fresh().await;
        store
            .write_contact_records(&[hold("good", "n2")])
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO contact_holds (id, contact, owner, since_ms, waiting, until_ms, preview) \
             VALUES ('bad', x'00ff', 'n2', 'soon', 'many', 'never', ?)",
        )
        .bind("p".repeat(100_000))
        .execute(&store.write)
        .await
        .unwrap();
        sqlx::query("INSERT INTO contact_holds (id, contact) VALUES (x'07', 'bob')")
            .execute(&store.write)
            .await
            .unwrap();
        let rows = store.contact_holds().await.unwrap();
        assert_eq!(
            rows.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            ["bad", "good"],
            "the blob id is skipped"
        );
        let bad = &rows[0];
        assert_eq!(
            (
                bad.contact.as_deref(),
                bad.since_ms,
                bad.waiting,
                bad.until_ms
            ),
            (None, None, None, None)
        );
        assert_eq!(bad.owner.as_deref(), Some("n2"));
        assert_eq!(bad.preview.as_deref().map(str::len), Some(256));
        assert_eq!(rows[1].until_ms, Some(2));
        let many: Vec<RecordWrite> = (0..600).map(|i| hold(&format!("h{i:03}"), "n2")).collect();
        store.write_contact_records(&many).await.unwrap();
        assert_eq!(store.contact_holds().await.unwrap().len(), 512);
    }

    /// 191 (P3 review M2): at boot only this engine's own hold rows go; another machine's
    /// stay (it still holds them).
    #[tokio::test]
    async fn boot_clears_only_this_engines_hold_rows() {
        let scratch = crate::test_dir("contacts");
        let store = Store::open(
            &scratch.path().join("sessions.sqlite"),
            Some(mesh::EngineId("me-engine".into())),
        )
        .await
        .unwrap();
        store
            .write_contact_records(&[
                hold("a", "me-engine"),
                hold("b", "other-engine"),
                hold("c", "me-engine"),
            ])
            .await
            .unwrap();
        assert_eq!(store.clear_own_contact_holds().await.unwrap(), 2);
        let left: Vec<String> = store
            .contact_holds()
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(left, ["b"]);
    }

    /// 191 (P3 final review N2): the mirror's read-back of this engine's own rows filters in
    /// SQL: 600 other machines' rows that sort first never push ours past the 512 cap.
    #[tokio::test]
    async fn the_own_hold_read_back_is_never_crowded_out() {
        let scratch = crate::test_dir("contacts");
        let store = Store::open(
            &scratch.path().join("sessions.sqlite"),
            Some(mesh::EngineId("me-engine".into())),
        )
        .await
        .unwrap();
        let mut rows: Vec<RecordWrite> = (0..600)
            .map(|i| hold(&format!("a{i:03}"), "other-engine"))
            .collect();
        rows.push(hold("z-mine", "me-engine"));
        store.write_contact_records(&rows).await.unwrap();
        assert!(
            !store
                .contact_holds()
                .await
                .unwrap()
                .iter()
                .any(|r| r.id == "z-mine"),
            "the whole read is capped first"
        );
        let own: Vec<String> = store
            .own_contact_holds()
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(own, ["z-mine"]);
    }

    /// 191 (P3 review S4): `contact_holds` rides the real record replica: a row written on
    /// one machine is captured and applied on another, and its deletion follows.
    #[tokio::test]
    async fn a_hold_row_and_its_deletion_ride_the_record_replica() {
        let open = |name: &'static str| async move {
            let dir = crate::test_dir(name);
            let store = Store::open(&dir.path().join("sessions.sqlite"), None)
                .await
                .unwrap();
            let engine = replica::Engine::open(&dir.path().join("replica.sqlite"))
                .await
                .unwrap();
            let handle = engine.attach(&store.write, &RECORD_TABLES).await.unwrap();
            (store, engine, handle, dir)
        };
        let (a, ea, ha, _da) = open("holds-rep-a").await;
        let (b, eb, _hb, _db) = open("holds-rep-b").await;
        let mut feed = ea.subscribe();
        a.write_contact_records(&[hold("t-1", "n2")]).await.unwrap();
        ha.flush().await.unwrap();
        while let Ok(batch) = feed.try_recv() {
            eb.apply(&batch).await.unwrap();
        }
        let on_b = b.contact_holds().await.unwrap();
        assert_eq!(on_b, a.contact_holds().await.unwrap());
        assert_eq!(on_b.len(), 1);
        assert_eq!(on_b[0].owner.as_deref(), Some("n2"));
        a.write_contact_records(&[RecordWrite::delete(RecordTable::Holds, "t-1")])
            .await
            .unwrap();
        ha.flush().await.unwrap();
        while let Ok(batch) = feed.try_recv() {
            eb.apply(&batch).await.unwrap();
        }
        assert!(
            b.contact_holds().await.unwrap().is_empty(),
            "deleted there too"
        );
    }

    /// 200 (Contract): `0068_contact_full_access` adds a record-replica column (written and
    /// read like any contact column); the session log is untouched, so the wire schema stays 67.
    #[tokio::test]
    async fn full_access_is_a_record_column_and_the_wire_schema_stays() {
        let (store, _, _dir) = fresh().await;
        store
            .write_contact_records(&[RecordWrite::new(
                RecordTable::Contacts,
                "c1",
                vec![("full_access", Cell::Int(1))],
            )])
            .await
            .unwrap();
        let r = store.contact_records().await.unwrap();
        assert_eq!(r.contacts[0].full_access, Some(1));
        assert!(
            !super::super::capture::ATTACHED
                .iter()
                .any(|(t, _)| *t == "contacts")
        );
        // 0068 left it at 67; 201 PC (0069), P1 (0070) and 219 (0078) bumped it since.
        assert_eq!(crate::store::SESSIONS_WIRE_SCHEMA, 78);
    }
}
