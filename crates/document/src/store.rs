//! `<data_dir>/documents/` (design 59 §4.1): `index.sqlite` plus one directory
//! per document with `meta.json`, `snapshot.yjs`, an append-only `updates.log`
//! and kept `snapshots/`. Load = snapshot + replay; the snapshot policy (§4.1)
//! is the actor's, this module only knows how to take one.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, bail};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};
use yrs::updates::decoder::Decode;
use yrs::{Doc, ReadTxn, StateVector, Transact, Update};

use crate::meta::{Binding, DocId, DocKind, DocMeta, DocSummary, Principal, SnapshotInfo};
use crate::mirror::{self, MirrorFile};
use crate::notebook::{ItemKind, NotebookItem, SourceOrigin, SourceStatus};

/// Directory under the data dir.
pub const DIR: &str = "documents";
const INDEX: &str = "index.sqlite";
const META: &str = "meta.json";
const SNAPSHOT: &str = "snapshot.yjs";
const LOG: &str = "updates.log";
/// 171 §9.6: block id → session of the agent that last wrote it; local to this engine, not mirrored.
const AGENTS: &str = "agents.json";
const SNAPSHOTS: &str = "snapshots";
const TRASH: &str = ".trash";
/// The flat replicated projection (69, [`crate::mirror`]).
const MIRROR: &str = "mirror";

const CREATE: &str = "CREATE TABLE IF NOT EXISTS docs (\
    id TEXT PRIMARY KEY, kind TEXT NOT NULL, title TEXT NOT NULL, binding TEXT NOT NULL, \
    created_ms INTEGER NOT NULL, updated_ms INTEGER NOT NULL, bytes INTEGER NOT NULL DEFAULT 0, \
    members_n INTEGER NOT NULL DEFAULT 0, deleted_ms INTEGER, hlc TEXT, project TEXT)";
const CREATE_BINDING: &str = "CREATE INDEX IF NOT EXISTS docs_binding ON docs(binding)";
/// 69: the replica's clock column on a table created before it existed (a no-op error otherwise).
const ADD_HLC: &str = "ALTER TABLE docs ADD COLUMN hlc TEXT";
/// Notes and boards filed under a project (the sessions' and notebooks' label, one level). A plain
/// `docs` column: the replica re-installs its triggers at every attach, so the column replicates
/// per-column LWW like `notebooks.project`; `meta.json` (the mirror) never carries it.
const ADD_PROJECT: &str = "ALTER TABLE docs ADD COLUMN project TEXT";
/// 213 §3: how many share links `meta.json` holds (revoked and expired included), kept like
/// `members_n` by [`Store::write_meta`] so [`Store::shared`] needs no `meta.json` scan. Replicated
/// per-column like `project`; an index from before it is backfilled once from `meta.json`.
const ADD_LINKS_N: &str = "ALTER TABLE docs ADD COLUMN links_n INTEGER NOT NULL DEFAULT 0";

/// The `docs` columns a [`DocSummary`] reads, in [`SummaryTuple`] order (a macro so it splices into `concat!`).
macro_rules! summary_cols {
    () => {
        "id, kind, title, binding, created_ms, updated_ms, bytes, project"
    };
}

/// One [`summary_cols!`] row.
type SummaryTuple = (
    String,
    String,
    String,
    String,
    i64,
    i64,
    i64,
    Option<String>,
);

/// A [`SummaryTuple`] as `doc_list` shows it; `None` for an unparseable kind or binding.
fn summary_row(
    (id, kind, title, binding, created_ms, updated_ms, bytes, project): SummaryTuple,
) -> Option<DocSummary> {
    Some(DocSummary {
        id: DocId(id),
        kind: serde_json::from_value(serde_json::Value::String(kind)).ok()?,
        title,
        binding: serde_json::from_str(&binding).ok()?,
        created_ms,
        updated_ms,
        bytes,
        project,
    })
}

/// 144 §3.3: the notebook's columns that are not the document (the document carries name + description).
const CREATE_NOTEBOOKS: &str = "CREATE TABLE IF NOT EXISTS notebooks (id TEXT PRIMARY KEY, project TEXT, archived_ms INTEGER, hlc TEXT)";
/// 144 §3.3: membership, one row per `<notebook>:<kind>:<item>`; soft removal via `removed_ms`.
const CREATE_NOTEBOOK_ITEMS: &str = "CREATE TABLE IF NOT EXISTS notebook_items (\
    id TEXT PRIMARY KEY, \
    notebook TEXT NOT NULL, kind TEXT NOT NULL, item TEXT NOT NULL, \
    added_ms INTEGER NOT NULL, added_by TEXT NOT NULL, \
    removed_ms INTEGER, hlc TEXT)";
const CREATE_NBI_NOTEBOOK: &str =
    "CREATE INDEX IF NOT EXISTS nbi_notebook ON notebook_items(notebook)";
const CREATE_NBI_ITEM: &str = "CREATE INDEX IF NOT EXISTS nbi_item ON notebook_items(kind, item)";
/// 144 §3.3: the ingest state and provenance of a `DocKind::Source` document.
const CREATE_SOURCES: &str = "CREATE TABLE IF NOT EXISTS sources (\
    id TEXT PRIMARY KEY, origin TEXT NOT NULL, status TEXT NOT NULL, \
    error TEXT, attempts INTEGER NOT NULL DEFAULT 0, blob TEXT, \
    fetched_ms INTEGER, content_sha TEXT, changed_since INTEGER, \
    bytes INTEGER, words INTEGER, duration_ms INTEGER, pages INTEGER, meta TEXT, \
    insights INTEGER NOT NULL DEFAULT 0, hlc TEXT)";
/// 16 §5 (2026-11-11): the local-network grant of a source's ingest jobs ([`Store::source_lan`]).
const CREATE_SOURCE_LAN: &str =
    "CREATE TABLE IF NOT EXISTS source_lan (id TEXT PRIMARY KEY, lan INTEGER NOT NULL)";
/// 171 §13.2 (W-g): the latest run of each note cell, capped text, replicated to members and guests.
/// 184 §6.4 (W-h): `payload` is the whole `CellLatest` as JSON (`engine` inside), written whole by
/// every upsert; readers prefer it, and `''` (a pre-184 row or writer) means the old columns.
const CREATE_CELL_LATEST: &str = "CREATE TABLE IF NOT EXISTS cell_latest (\
    id TEXT PRIMARY KEY, \
    doc TEXT NOT NULL, cell TEXT NOT NULL, \
    run TEXT NOT NULL, state TEXT NOT NULL, exit INTEGER, secs REAL, \
    expect TEXT, summary TEXT, code_sha TEXT NOT NULL, ended_ms INTEGER NOT NULL, \
    text TEXT NOT NULL, hlc TEXT, payload TEXT NOT NULL DEFAULT '')";
const CREATE_CELL_LATEST_DOC: &str =
    "CREATE INDEX IF NOT EXISTS cell_latest_doc ON cell_latest(doc)";
/// 184 §9.3: `payload` on a `cell_latest` created before it existed (a no-op error otherwise).
const ADD_PAYLOAD: &str = "ALTER TABLE cell_latest ADD COLUMN payload TEXT NOT NULL DEFAULT ''";

/// The replicated tables (69 §5, 144 §3.3): attached to the record replica by the plugin; per-column
/// LWW under the replica's HLC (the engine stamps `hlc`, the store never writes it).
pub const TABLES: &[&str] = &[
    "docs",
    "notebooks",
    "notebook_items",
    "sources",
    "cell_latest",
];

// Unix milliseconds now.
pub use platform::time::now_ms_i64 as now_ms;

/// The store handle: paths + the index pool + the shas of the mirror files this process last wrote
/// (so the mirror watcher tells a foreign projection from an own write, 69) + the merge counters +
/// the vault whose mesh key seals `ShareLink::secret_hash_sealed` (SEAL).
#[derive(Clone)]
pub struct Store {
    root: PathBuf,
    db: SqlitePool,
    projected: Arc<Mutex<HashMap<String, String>>>,
    pub(crate) counters: Arc<Counters>,
    vault: Option<Arc<secrets::Vault>>,
}

/// What the mirror merges did in this process (69 E2: the echo audit); read as [`DocStats`].
#[derive(Debug, Default)]
pub struct Counters {
    /// [`DocStats::merged`].
    pub merged: AtomicU64,
    /// [`DocStats::unchanged`].
    pub unchanged: AtomicU64,
    /// [`DocStats::resnapshots`].
    pub resnapshots: AtomicU64,
    /// [`DocStats::noop_applies_avoided`].
    pub noop_applies_avoided: AtomicU64,
}

impl Counters {
    /// `+1`.
    pub fn bump(counter: &AtomicU64) {
        counter.fetch_add(1, Relaxed);
    }
}

/// `doc_stats`: how foreign snapshots were handled since start (per process, never persisted).
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
    kameo::Reply,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct DocStats {
    /// Foreign snapshots that held state this engine lacked and were joined (closed or open document).
    pub merged: u64,
    /// Foreign snapshots that held nothing new (closed or open document).
    pub unchanged: u64,
    /// Joins that had to re-encode and ship a snapshot: this engine held state the peer lacked.
    pub resnapshots: u64,
    /// `unchanged` on an open document — before E2 each went through `apply` (log, fan-out, export)
    /// whenever the document held a deletion, then re-snapshotted and shipped.
    pub noop_applies_avoided: u64,
}

/// What [`Store::mirror_apply`] did with a projected file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Applied {
    /// Not a mirror file, this process's own write, or a document deleted here.
    Ignored,
    /// Foreign, but nothing new in it.
    Unchanged,
    /// The document exists locally and grew: a snapshot merged and re-snapshotted, or `meta.json` joined.
    Merged(DocId),
    /// A document this engine had never seen: its directory was created from the mirrored `meta.json`
    /// (plus the snapshot when already there).
    Created(DocId),
    /// A snapshot whose `meta.json` is not projected yet (a new document arrives as two rows); the
    /// meta's arrival retries.
    Deferred(DocId),
}

/// A document read back from disk.
pub struct Loaded {
    /// The CRDT with snapshot and log applied.
    pub doc: Doc,
    /// `meta.json`.
    pub meta: DocMeta,
    /// Records replayed from `updates.log`.
    pub log_count: usize,
    /// Bytes of `updates.log`.
    pub log_bytes: usize,
    /// Bytes of `snapshot.yjs` (0 when none).
    pub snapshot_bytes: usize,
}

impl Store {
    /// Create `documents/`, open the index, converge ids (69: [`DocId::for_binding`]).
    pub async fn open(data_dir: &Path) -> anyhow::Result<Self> {
        let root = data_dir.join(DIR);
        fs::create_dir_all(&root).with_context(|| format!("create {}", root.display()))?;
        let opts = SqliteConnectOptions::new()
            .filename(root.join(INDEX))
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(5));
        let db = SqlitePoolOptions::new()
            .max_connections(2)
            .connect_with(opts)
            .await?;
        sqlx::query(CREATE).execute(&db).await?;
        sqlx::query(CREATE_BINDING).execute(&db).await?;
        for sql in [
            CREATE_NOTEBOOKS,
            CREATE_NOTEBOOK_ITEMS,
            CREATE_NBI_NOTEBOOK,
            CREATE_NBI_ITEM,
            CREATE_SOURCES,
            CREATE_SOURCE_LAN,
            CREATE_CELL_LATEST,
            CREATE_CELL_LATEST_DOC,
        ] {
            sqlx::query(sql).execute(&db).await?;
        }
        for (sql, what) in [
            (ADD_HLC, "docs.hlc"),
            (ADD_PAYLOAD, "cell_latest.payload"),
            (ADD_PROJECT, "docs.project"),
        ] {
            if let Err(e) = sqlx::query(sql).execute(&db).await
                && !e.to_string().contains("duplicate column")
            {
                return Err(e).context(what);
            }
        }
        fs::create_dir_all(root.join(MIRROR))?;
        let store = Self {
            root,
            db,
            projected: Default::default(),
            counters: Default::default(),
            vault: None,
        };
        store.migrate_ids().await?;
        store.add_links_n().await?;
        Ok(store)
    }

    /// 213: add `links_n` and backfill it from every live row's `meta.json` in one transaction, so a
    /// crash in between leaves no column and the next open does both again. Links are counted on the
    /// raw JSON, so a pre-SEAL `meta.json` counts too; a missing or unreadable file stays 0.
    async fn add_links_n(&self) -> anyhow::Result<()> {
        let mut tx = self.db.begin().await?;
        match sqlx::query(ADD_LINKS_N).execute(&mut *tx).await {
            Ok(_) => {}
            Err(e) if e.to_string().contains("duplicate column") => return Ok(()),
            Err(e) => return Err(e).context("docs.links_n"),
        }
        let ids: Vec<(String,)> = sqlx::query_as("SELECT id FROM docs WHERE deleted_ms IS NULL")
            .fetch_all(&mut *tx)
            .await?;
        for (id,) in ids {
            let Ok(bytes) = fs::read(self.dir(&DocId(id.clone())).join(META)) else {
                continue;
            };
            let n = serde_json::from_slice::<serde_json::Value>(&bytes)
                .ok()
                .and_then(|v| v.get("links")?.as_array().map(Vec::len))
                .unwrap_or(0);
            if n == 0 {
                continue;
            }
            sqlx::query("UPDATE docs SET links_n = ? WHERE id = ?")
                .bind(n as i64)
                .bind(&id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// 213 §3: live documents with members or share links (any state), newest update first.
    pub async fn shared(&self) -> anyhow::Result<Vec<DocId>> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT id FROM docs WHERE deleted_ms IS NULL AND (members_n > 0 OR links_n > 0) \
             ORDER BY updated_ms DESC, rowid DESC",
        )
        .fetch_all(&self.db)
        .await?;
        Ok(rows.into_iter().map(|(id,)| DocId(id)).collect())
    }

    /// The index pool (for `replica::attach`).
    pub fn pool(&self) -> &SqlitePool {
        &self.db
    }

    /// Seal share-link hashes with `vault`'s mesh key (SEAL). Without one, links cannot be created
    /// or verified and a pre-SEAL plaintext hash is dropped on load.
    pub fn with_vault(mut self, vault: Arc<secrets::Vault>) -> Self {
        self.vault = Some(vault);
        self
    }

    /// The vault, when the secrets plugin gave one.
    pub fn vault(&self) -> Option<Arc<secrets::Vault>> {
        self.vault.clone()
    }

    /// The merge counters now.
    pub fn stats(&self) -> DocStats {
        let c = &self.counters;
        DocStats {
            merged: c.merged.load(Relaxed),
            unchanged: c.unchanged.load(Relaxed),
            resnapshots: c.resnapshots.load(Relaxed),
            noop_applies_avoided: c.noop_applies_avoided.load(Relaxed),
        }
    }

    /// One-time convergence (69 §5): every live bound document sits at [`DocId::for_binding`]. A
    /// binding with several live rows keeps the newest (`updated_ms`), merges the others' state into
    /// it as updates and trashes them; a lone row under a v7 id is moved. Notes are untouched, and so
    /// are task threads: `tasks.thread` holds their id, so it must not move under them.
    async fn migrate_ids(&self) -> anyhow::Result<()> {
        let rows: Vec<(String, String, i64)> = sqlx::query_as(
            "SELECT id, binding, updated_ms FROM docs WHERE deleted_ms IS NULL ORDER BY binding, updated_ms DESC, id",
        )
        .fetch_all(&self.db)
        .await?;
        let mut groups: Vec<(Binding, Vec<DocId>)> = Vec::new();
        for (id, binding, _) in rows {
            let Ok(binding) = serde_json::from_str::<Binding>(&binding) else {
                continue;
            };
            if binding.keyless() || matches!(binding, Binding::TaskThread(_)) {
                continue;
            }
            match groups.last_mut() {
                Some((b, ids)) if *b == binding => ids.push(DocId(id)),
                _ => groups.push((binding, vec![DocId(id)])),
            }
        }
        for (binding, ids) in groups {
            let target = DocId::for_binding(&binding);
            let (keep, rest) = ids.split_first().expect("non-empty group");
            if !self.dir(keep).join(META).is_file() {
                continue;
            }
            if !rest.is_empty() {
                let kept = self.load(keep)?;
                for other in rest {
                    match self.load(other) {
                        Ok(l) => {
                            let state = l
                                .doc
                                .transact()
                                .encode_state_as_update_v1(&StateVector::default());
                            if let Err(e) = kept
                                .doc
                                .transact_mut()
                                .apply_update(Update::decode_v1(&state)?)
                            {
                                tracing::warn!(doc = %other, into = %keep, error = %e, "documents: duplicate not merged");
                                continue;
                            }
                            tracing::info!(doc = %other, into = %keep, binding = %binding.key(), "documents: duplicate merged");
                        }
                        Err(e) => {
                            tracing::warn!(doc = %other, error = %e, "documents: duplicate unreadable; trashed")
                        }
                    }
                    self.delete(other).await?;
                }
                self.snapshot(keep, &kept.doc, None)?;
            }
            if *keep != target {
                self.rename(keep, &target, &binding).await?;
            }
        }
        Ok(())
    }

    /// Move `from` to `to` (69 id convergence): directory, `meta.json` (`id`, `derived`), index row.
    async fn rename(&self, from: &DocId, to: &DocId, binding: &Binding) -> anyhow::Result<()> {
        let mut meta = self.read_meta(from)?;
        let to_dir = self.dir(to);
        if to_dir.exists() {
            bail!("documents: {to} exists; cannot move {from} there");
        }
        // A tombstone left under the deterministic id (a deletion before this migration) yields.
        sqlx::query("DELETE FROM docs WHERE id = ? AND deleted_ms IS NOT NULL")
            .bind(to.as_str())
            .execute(&self.db)
            .await?;
        fs::rename(self.dir(from), &to_dir)?;
        meta.id = to.clone();
        if matches!(binding, Binding::Artifact { .. } | Binding::AgentCharter(_)) {
            meta.derived = binding.derived(to);
        }
        write_atomic(
            &to_dir.join(META),
            serde_json::to_vec_pretty(&meta)?.as_slice(),
        )?;
        sqlx::query("UPDATE docs SET id = ? WHERE id = ?")
            .bind(to.as_str())
            .bind(from.as_str())
            .execute(&self.db)
            .await?;
        self.mirror_remove(from);
        self.mirror_write(to)?;
        tracing::info!(from = %from, to = %to, binding = %binding.key(), "documents: id converged");
        Ok(())
    }

    /// Give `id` a new owner: `kind`, `binding` and `derived` follow `binding` in `meta.json`, the
    /// index row and the mirror; the old derived file moves to the new path. `Ok(false)` when the
    /// document is already bound so; fails when `binding` is bound to another document.
    pub async fn bind(&self, id: &DocId, binding: &Binding) -> anyhow::Result<bool> {
        let mut meta = self.read_meta(id)?;
        if meta.binding == *binding {
            return Ok(false);
        }
        if let Some(existing) = self.lookup(binding).await?
            && existing != *id
        {
            bail!("{binding:?} is already document {existing}");
        }
        let data_dir = self.root.parent().unwrap_or(&self.root);
        let old = data_dir.join(&meta.derived);
        meta.kind = binding.kind();
        meta.binding = binding.clone();
        meta.derived = binding.derived(id);
        if old.is_file() {
            let _ = fs::rename(old, data_dir.join(&meta.derived));
        }
        write_atomic(
            &self.dir(id).join(META),
            serde_json::to_vec_pretty(&meta)?.as_slice(),
        )?;
        sqlx::query("UPDATE docs SET kind = ?, binding = ? WHERE id = ?")
            .bind(meta.kind.to_string())
            .bind(binding.key())
            .bind(id.as_str())
            .execute(&self.db)
            .await?;
        self.mirror_write_meta(id)?;
        tracing::info!(doc = %id, kind = %meta.kind, binding = %binding.key(), "documents: rebound");
        Ok(true)
    }

    /// `documents/`.
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn dir(&self, id: &DocId) -> PathBuf {
        self.root.join(id.as_str())
    }

    /// `agents.json` (171 §9.6): which agent session last wrote each block; empty when absent.
    pub(crate) fn read_agents(&self, id: &DocId) -> std::collections::BTreeMap<String, uuid::Uuid> {
        fs::read(self.dir(id).join(AGENTS))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    /// Rewrite `agents.json`.
    pub(crate) fn write_agents(
        &self,
        id: &DocId,
        agents: &std::collections::BTreeMap<String, uuid::Uuid>,
    ) -> anyhow::Result<()> {
        Ok(write_atomic(
            &self.dir(id).join(AGENTS),
            serde_json::to_vec(agents)?.as_slice(),
        )?)
    }

    /// Directory, meta, empty log, index row. Fails if the binding is already bound. A row under
    /// the same id (a soft-deleted document with this deterministic id) is revived.
    pub async fn create(&self, meta: &DocMeta) -> anyhow::Result<()> {
        if !meta.binding.keyless()
            && let Some(existing) = self.lookup(&meta.binding).await?
        {
            bail!("{:?} is already document {existing}", meta.binding);
        }
        let dir = self.dir(&meta.id);
        fs::create_dir_all(dir.join(SNAPSHOTS))?;
        write_atomic(&dir.join(META), serde_json::to_vec_pretty(meta)?.as_slice())?;
        File::create(dir.join(LOG))?;
        sqlx::query(
            "INSERT INTO docs (id, kind, title, binding, created_ms, updated_ms, bytes) VALUES (?, ?, ?, ?, ?, ?, 0) \
             ON CONFLICT (id) DO UPDATE SET kind = excluded.kind, title = excluded.title, binding = excluded.binding, \
             created_ms = excluded.created_ms, updated_ms = excluded.updated_ms, bytes = 0, members_n = 0, links_n = 0, deleted_ms = NULL",
        )
        .bind(meta.id.as_str())
        .bind(meta.kind.to_string())
        .bind(&meta.title)
        .bind(meta.binding.key())
        .bind(meta.created_ms)
        .bind(meta.updated_ms)
        .execute(&self.db)
        .await?;
        self.mirror_write(&meta.id)?;
        Ok(())
    }

    /// `meta.json`; a pre-SEAL plaintext `links[].secret_hash` is sealed in place (file and mirror).
    pub fn read_meta(&self, id: &DocId) -> anyhow::Result<DocMeta> {
        let p = self.dir(id).join(META);
        let bytes = fs::read(&p).with_context(|| format!("read {}", p.display()))?;
        let (meta, sealed) = self.parse_meta(&bytes)?;
        if sealed {
            write_atomic(&p, serde_json::to_vec_pretty(&meta)?.as_slice())?;
            self.mirror_write_meta(id)?;
            tracing::info!(doc = %id, "documents: share-link secret hashes sealed with the mesh key");
        }
        Ok(meta)
    }

    /// Parse `meta.json` bytes. A pre-SEAL plaintext `links[].secret_hash` (base64url sha256) is
    /// sealed with the mesh key into `secret_hash_sealed`; `true` when that happened. Without a
    /// vault the plaintext cannot be sealed and does not survive the parse (serde drops the unknown
    /// field): the link then refuses every proof, as it would without the key.
    pub(crate) fn parse_meta(&self, bytes: &[u8]) -> anyhow::Result<(DocMeta, bool)> {
        use serde_json::Value;
        let mut v: Value = serde_json::from_slice(bytes)?;
        let mut sealed = false;
        if let Some(vault) = &self.vault
            && let Some(links) = v.get_mut("links").and_then(Value::as_array_mut)
        {
            for l in links.iter_mut().filter_map(Value::as_object_mut) {
                let Some(Value::String(plain)) = l.remove("secret_hash") else {
                    continue;
                };
                let (Some(hash), Some(id)) = (
                    crate::share::unb64url(&plain),
                    l.get("id").and_then(Value::as_str),
                ) else {
                    continue;
                };
                l.insert(
                    "secret_hash_sealed".into(),
                    Value::String(crate::share::seal_hash(vault, id, &hash)),
                );
                sealed = true;
            }
        }
        Ok((serde_json::from_value(v)?, sealed))
    }

    /// Rewrite `meta.json`, the index columns it feeds, and the mirror copy.
    pub async fn write_meta(&self, meta: &DocMeta) -> anyhow::Result<()> {
        write_atomic(
            &self.dir(&meta.id).join(META),
            serde_json::to_vec_pretty(meta)?.as_slice(),
        )?;
        sqlx::query(
            "UPDATE docs SET title = ?, updated_ms = ?, members_n = ?, links_n = ? WHERE id = ?",
        )
        .bind(&meta.title)
        .bind(meta.updated_ms)
        .bind(meta.members.len() as i64)
        .bind(meta.links.len() as i64)
        .bind(meta.id.as_str())
        .execute(&self.db)
        .await?;
        self.mirror_write_meta(&meta.id)?;
        Ok(())
    }

    /// `documents/mirror/` — the flat, replicated projection (see [`crate::mirror`]).
    pub fn mirror_dir(&self) -> PathBuf {
        self.root.join(MIRROR)
    }

    /// Copy `meta.json` and `snapshot.yjs` of `id` into the mirror (atomic, `.`-prefixed temp so the
    /// replica's scan never rows a half file) and remember their shas. A file the document lacks
    /// (no snapshot yet) is not written.
    pub fn mirror_write(&self, id: &DocId) -> anyhow::Result<()> {
        self.mirror_copy(
            id,
            &[(META, MirrorFile::Meta), (SNAPSHOT, MirrorFile::Snapshot)],
        )
    }

    /// [`Self::mirror_write`] of `meta.json` alone, for a change that touched only the meta: this
    /// engine's `snapshot.yjs` may be older than the mirror's (a newer one arrived from the
    /// document's server and is not applied yet), and re-publishing it would replace the newer
    /// snapshot everywhere (184: every engine writes meta — fences, homes — not only the hub).
    fn mirror_write_meta(&self, id: &DocId) -> anyhow::Result<()> {
        self.mirror_copy(id, &[(META, MirrorFile::Meta)])
    }

    fn mirror_copy(&self, id: &DocId, files: &[(&str, MirrorFile)]) -> anyhow::Result<()> {
        let dir = self.dir(id);
        for &(src, kind) in files {
            let Some(bytes) = read_opt(&dir.join(src))? else {
                continue;
            };
            let name = mirror::file_name(id, kind);
            let sha = sha256(&bytes);
            self.projected
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .insert(name.clone(), sha);
            write_hidden(&self.mirror_dir().join(&name), &bytes)?;
        }
        Ok(())
    }

    /// sha256 of the snapshot this process last wrote into the mirror for `id`; `None` when it never did.
    pub fn last_projected(&self, id: &DocId) -> Option<String> {
        self.projected
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(&mirror::file_name(id, MirrorFile::Snapshot))
            .cloned()
    }

    /// Whether `bytes` at mirror file `name` are what this process last wrote there.
    pub fn is_own_projection(&self, name: &str, bytes: &[u8]) -> bool {
        self.projected
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(name)
            .is_some_and(|sha| *sha == sha256(bytes))
    }

    /// Remove `id`'s mirror files (a delete; replicates as deleted rows).
    fn mirror_remove(&self, id: &DocId) {
        let mut projected = self.projected.lock().unwrap_or_else(|p| p.into_inner());
        for kind in [MirrorFile::Meta, MirrorFile::Snapshot] {
            let name = mirror::file_name(id, kind);
            projected.remove(&name);
            let _ = fs::remove_file(self.mirror_dir().join(name));
        }
    }

    /// Merge one projected mirror file into the store **for a document that is not open** (the actor
    /// merges open ones into the live `Doc`). Own writes are [`Applied::Ignored`]; a foreign snapshot
    /// that [`exceeds`] the local state is adopted verbatim when it also holds everything here, and
    /// joined + re-snapshotted (shipped once) only when this engine held state the peer lacked; a
    /// foreign `meta.json` joins by [`mirror::merge_meta`]. Foreign bytes never *lose* local state.
    pub async fn mirror_apply(&self, path: &Path) -> anyhow::Result<Applied> {
        let Some((id, kind)) = mirror::parse(path) else {
            return Ok(Applied::Ignored);
        };
        let Some(bytes) = read_opt(path)? else {
            // Removed by a peer's delete; the `docs` tombstone replicates on its own. A directory
            // still here for a row marked deleted follows it into `.trash`.
            let deleted: Option<(i64,)> = sqlx::query_as(
                "SELECT deleted_ms FROM docs WHERE id = ? AND deleted_ms IS NOT NULL",
            )
            .bind(id.as_str())
            .fetch_optional(&self.db)
            .await?;
            if deleted.is_some() && self.dir(&id).exists() {
                self.delete(&id).await?;
            }
            return Ok(Applied::Ignored);
        };
        let name = mirror::file_name(&id, kind);
        if self.is_own_projection(&name, &bytes) {
            return Ok(Applied::Ignored);
        }
        let dir = self.dir(&id);
        if !dir.join(META).is_file() {
            // New here: `meta.json` makes the directory (a document with no snapshot yet is an empty
            // one, as `create` leaves it); a snapshot arriving first waits for its meta.
            let mirror = self.mirror_dir();
            let Some(meta_bytes) =
                read_opt(&mirror.join(mirror::file_name(&id, MirrorFile::Meta)))?
            else {
                return Ok(Applied::Deferred(id));
            };
            let snap_bytes = read_opt(&mirror.join(mirror::file_name(&id, MirrorFile::Snapshot)))?;
            let meta: DocMeta = serde_json::from_slice(&meta_bytes).context("mirror meta.json")?;
            if let Some(snap) = &snap_bytes {
                Update::decode_v1(snap).context("mirror snapshot.yjs")?;
            }
            fs::create_dir_all(dir.join(SNAPSHOTS))?;
            write_atomic(&dir.join(META), &meta_bytes)?;
            if let Some(snap) = &snap_bytes {
                write_atomic(&dir.join(SNAPSHOT), snap)?;
            }
            File::create(dir.join(LOG))?;
            let mut projected = self.projected.lock().unwrap_or_else(|p| p.into_inner());
            projected.insert(
                mirror::file_name(&id, MirrorFile::Meta),
                sha256(&meta_bytes),
            );
            if let Some(snap) = &snap_bytes {
                projected.insert(mirror::file_name(&id, MirrorFile::Snapshot), sha256(snap));
            }
            tracing::info!(doc = %id, binding = %meta.binding.key(), "document arrived from the mesh");
            return Ok(Applied::Created(id));
        }
        match kind {
            MirrorFile::Meta => {
                let (incoming, _) = self.parse_meta(&bytes).context("mirror meta.json")?;
                let mut local = self.read_meta(&id)?;
                // 184 §7.4 `opens`: the seal that opens here wins.
                let vault = self.vault();
                let opens =
                    |l: &crate::meta::ShareLink| crate::share::seal_opens(vault.as_deref(), l);
                if !mirror::merge_meta(&mut local, &incoming, &opens) {
                    return Ok(Applied::Unchanged);
                }
                self.write_meta(&local).await?;
                Ok(Applied::Merged(id))
            }
            MirrorFile::Snapshot => {
                let loaded = self.load(&id)?;
                let theirs = decode_state(&bytes)?;
                if !exceeds(&theirs, &loaded.doc) {
                    Counters::bump(&self.counters.unchanged);
                    return Ok(Applied::Unchanged);
                }
                Counters::bump(&self.counters.merged);
                if exceeds(&loaded.doc, &theirs) {
                    // A real merge: state the peer lacks joins theirs and must ship once.
                    let diff = theirs
                        .transact()
                        .encode_state_as_update_v1(&loaded.doc.transact().state_vector());
                    loaded
                        .doc
                        .transact_mut()
                        .apply_update(Update::decode_v1(&diff)?)
                        .context("apply foreign snapshot.yjs")?;
                    Counters::bump(&self.counters.resnapshots);
                    let n = self.snapshot(&id, &loaded.doc, None)?;
                    self.touch_if_changed(&id, loaded.meta.updated_ms, n)
                        .await?;
                } else {
                    // Theirs holds everything here: their bytes become this document's snapshot and this
                    // process's projection. The mirror file (the replica's copy) already equals its row,
                    // so nothing ships back; a re-encoding would differ byte-wise and ship a whole document.
                    // The `docs` row is not touched either: `bytes`/`updated_ms` arrive with the hub's
                    // own row (69 Z inv 10: N−1 receivers each shipped a two-column batch per edit).
                    self.adopt_snapshot(&id, &bytes)?;
                }
                tracing::info!(doc = %id, "document snapshot merged from the mesh");
                Ok(Applied::Merged(id))
            }
        }
    }

    /// `bytes` (a peer's full state that already holds everything here) as `snapshot.yjs`; the log
    /// it covers is truncated and the mirror copy on disk is recorded as this process's projection.
    fn adopt_snapshot(&self, id: &DocId, bytes: &[u8]) -> anyhow::Result<usize> {
        let dir = self.dir(id);
        write_atomic(&dir.join(SNAPSHOT), bytes)?;
        File::create(dir.join(LOG))?;
        self.projected
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(mirror::file_name(id, MirrorFile::Snapshot), sha256(bytes));
        Ok(bytes.len())
    }

    /// [`Self::touch_at_snapshot`] unless the row already says `bytes` — a same-value write is
    /// still a replication batch.
    async fn touch_if_changed(
        &self,
        id: &DocId,
        updated_ms: i64,
        bytes: usize,
    ) -> anyhow::Result<()> {
        let row: Option<(i64,)> = sqlx::query_as("SELECT bytes FROM docs WHERE id = ?")
            .bind(id.as_str())
            .fetch_optional(&self.db)
            .await?;
        if row.is_some_and(|(b,)| b == bytes as i64) {
            return Ok(());
        }
        self.touch_at_snapshot(id, updated_ms, bytes).await
    }

    /// Snapshot + replayed log.
    pub fn load(&self, id: &DocId) -> anyhow::Result<Loaded> {
        let meta = self.read_meta(id)?;
        let dir = self.dir(id);
        let doc = Doc::new();
        let snapshot_bytes = match fs::read(dir.join(SNAPSHOT)) {
            Ok(bytes) => {
                doc.transact_mut()
                    .apply_update(Update::decode_v1(&bytes).context("snapshot.yjs")?)
                    .context("apply snapshot.yjs")?;
                bytes.len()
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => 0,
            Err(e) => return Err(e).context("read snapshot.yjs"),
        };
        let (log_count, log_bytes) = match fs::read(dir.join(LOG)) {
            Ok(log) => {
                let mut count = 0;
                for (_, update) in records(&log) {
                    match Update::decode_v1(update) {
                        Ok(u) => {
                            if let Err(e) = doc.transact_mut().apply_update(u) {
                                tracing::warn!(doc = %id, error = %e, "updates.log: record skipped");
                            }
                        }
                        Err(e) => {
                            tracing::warn!(doc = %id, error = %e, "updates.log: undecodable record skipped");
                        }
                    }
                    count += 1;
                }
                (count, log.len())
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (0, 0),
            Err(e) => return Err(e).context("read updates.log"),
        };
        Ok(Loaded {
            doc,
            meta,
            log_count,
            log_bytes,
            snapshot_bytes,
        })
    }

    /// Append one record (§5.5 framing: tag, 16-byte session for the agent, 32-byte static for a
    /// member, u32 length, update) and fsync. Returns the record's size.
    pub fn append(&self, id: &DocId, by: &Principal, update: &[u8]) -> anyhow::Result<usize> {
        let mut rec = Vec::with_capacity(update.len() + 37);
        rec.push(by.tag());
        match by {
            Principal::Agent { session } => rec.extend_from_slice(session.as_bytes()),
            Principal::Member { pub_, .. } => {
                let mut key = [0u8; 32];
                if let Ok(k) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(pub_)
                    && k.len() == 32
                {
                    key.copy_from_slice(&k);
                }
                rec.extend_from_slice(&key);
            }
            _ => {}
        }
        rec.extend_from_slice(&(update.len() as u32).to_be_bytes());
        rec.extend_from_slice(update);
        let mut f = OpenOptions::new()
            .append(true)
            .create(true)
            .open(self.dir(id).join(LOG))?;
        f.write_all(&rec)?;
        // ponytail: one fsync per accepted update; batch per tick if typing latency shows it.
        f.sync_data()?;
        Ok(rec.len())
    }

    /// Full state → `snapshot.yjs` (tmp + rename), truncate the log, refresh the mirror copy; with a
    /// label also keep a copy under `snapshots/<ts>-<label>.yjs`. Returns the
    /// encoded size (the new `bytes` estimate).
    pub fn snapshot(&self, id: &DocId, doc: &Doc, label: Option<&str>) -> anyhow::Result<usize> {
        let bytes = doc
            .transact()
            .encode_state_as_update_v1(&StateVector::default());
        let dir = self.dir(id);
        write_atomic(&dir.join(SNAPSHOT), &bytes)?;
        File::create(dir.join(LOG))?;
        if let Some(label) = label {
            let name = format!("{}-{}", now_ms(), sanitize(label));
            fs::create_dir_all(dir.join(SNAPSHOTS))?;
            write_atomic(&dir.join(SNAPSHOTS).join(format!("{name}.yjs")), &bytes)?;
        }
        self.mirror_write(id)?;
        Ok(bytes.len())
    }

    /// Kept snapshots, newest first.
    pub fn snapshots(&self, id: &DocId) -> anyhow::Result<Vec<SnapshotInfo>> {
        let dir = self.dir(id).join(SNAPSHOTS);
        let mut out = Vec::new();
        let rd = match fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(e) => return Err(e.into()),
        };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let Some(stem) = name.strip_suffix(".yjs") else {
                continue;
            };
            let Some((ts, label)) = stem.split_once('-') else {
                continue;
            };
            let Ok(at_ms) = ts.parse::<i64>() else {
                continue;
            };
            out.push(SnapshotInfo {
                name: stem.to_owned(),
                label: label.to_owned(),
                at_ms,
                bytes: e.metadata().map(|m| m.len() as i64).unwrap_or_default(),
            });
        }
        out.sort_by(|a, b| b.at_ms.cmp(&a.at_ms).then_with(|| b.name.cmp(&a.name)));
        Ok(out)
    }

    /// Drop the oldest automatic snapshots beyond `keep` (§4.6: labelled ones stay).
    pub fn prune_snapshots(&self, id: &DocId, keep: usize) -> anyhow::Result<()> {
        let auto: Vec<SnapshotInfo> = self
            .snapshots(id)?
            .into_iter()
            .filter(|s| AUTO_LABELS.contains(&s.label.as_str()))
            .collect();
        for s in auto.iter().skip(keep) {
            let _ = fs::remove_file(self.dir(id).join(SNAPSHOTS).join(format!("{}.yjs", s.name)));
        }
        Ok(())
    }

    /// The bytes of a kept snapshot.
    pub fn read_snapshot(&self, id: &DocId, name: &str) -> anyhow::Result<Vec<u8>> {
        if name.contains('/') || name.contains("..") {
            bail!("bad snapshot name");
        }
        let p = self.dir(id).join(SNAPSHOTS).join(format!("{name}.yjs"));
        fs::read(&p).with_context(|| format!("no snapshot {name}"))
    }

    /// Index bookkeeping at a snapshot or export (69: not per update — every `docs` write is a
    /// replication batch, and typing must not be one every 200 ms; `bytes` may lag by a log's worth).
    pub async fn touch_at_snapshot(
        &self,
        id: &DocId,
        updated_ms: i64,
        bytes: usize,
    ) -> anyhow::Result<()> {
        sqlx::query("UPDATE docs SET updated_ms = ?, bytes = ? WHERE id = ?")
            .bind(updated_ms)
            .bind(bytes as i64)
            .bind(id.as_str())
            .execute(&self.db)
            .await?;
        Ok(())
    }

    /// The live document bound to `binding`. A row whose directory is not here (deleted by hand, or
    /// replicated ahead of its mirror files, 69) answers `None` — never a delete: the row is shared.
    pub async fn lookup(&self, binding: &Binding) -> anyhow::Result<Option<DocId>> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT id FROM docs WHERE binding = ? AND deleted_ms IS NULL LIMIT 1")
                .bind(binding.key())
                .fetch_optional(&self.db)
                .await?;
        let Some((id,)) = row else { return Ok(None) };
        let id = DocId(id);
        if !self.dir(&id).join(META).is_file() {
            tracing::warn!(doc = %id, "document directory missing; row kept");
            return Ok(None);
        }
        Ok(Some(id))
    }

    /// Live documents, newest update first.
    pub async fn list(&self, kind: Option<DocKind>, limit: u32) -> anyhow::Result<Vec<DocSummary>> {
        let rows: Vec<SummaryTuple> = match kind {
            Some(k) => {
                sqlx::query_as(concat!(
                    "SELECT ",
                    summary_cols!(),
                    " FROM docs WHERE deleted_ms IS NULL AND kind = ? ORDER BY updated_ms DESC, rowid DESC LIMIT ?"
                ))
                .bind(k.to_string())
                .bind(limit)
                .fetch_all(&self.db)
                .await?
            }
            None => {
                sqlx::query_as(concat!(
                    "SELECT ",
                    summary_cols!(),
                    " FROM docs WHERE deleted_ms IS NULL ORDER BY updated_ms DESC, rowid DESC LIMIT ?"
                ))
                .bind(limit)
                .fetch_all(&self.db)
                .await?
            }
        };
        Ok(rows.into_iter().filter_map(summary_row).collect())
    }

    /// File a live document under `project` (`None` = ungrouped); `false` when there is no such
    /// live row. A column write the replica merges (LWW), like `notebooks.project`.
    pub async fn set_project(&self, id: &DocId, project: Option<&str>) -> anyhow::Result<bool> {
        let n = sqlx::query(
            "UPDATE docs SET project = ? WHERE id = ? AND deleted_ms IS NULL AND project IS NOT ?",
        )
        .bind(project)
        .bind(id.as_str())
        .bind(project)
        .execute(&self.db)
        .await?
        .rows_affected();
        if n > 0 {
            return Ok(true);
        }
        Ok(self.summary(id).await?.is_some())
    }

    /// Move the directory to `.trash/<id>`, mark the row deleted (§7.1), drop the mirror files.
    pub async fn delete(&self, id: &DocId) -> anyhow::Result<()> {
        let trash = self.root.join(TRASH);
        fs::create_dir_all(&trash)?;
        let from = self.dir(id);
        if from.exists() {
            let to = trash.join(id.as_str());
            let _ = fs::remove_dir_all(&to);
            fs::rename(&from, &to)?;
        }
        sqlx::query("UPDATE docs SET deleted_ms = ? WHERE id = ?")
            .bind(now_ms())
            .bind(id.as_str())
            .execute(&self.db)
            .await?;
        // 171 §13.2: a deleted note's latest outputs go with it (the delete replicates as a tombstone).
        sqlx::query("DELETE FROM cell_latest WHERE doc = ?")
            .bind(id.as_str())
            .execute(&self.db)
            .await?;
        self.mirror_remove(id);
        Ok(())
    }

    /// Remove `.trash/<id>` directories deleted more than `max_age` ago (§7.3). The row stays as a
    /// tombstone (69: a delete would lose to any newer column write on another engine). Returns how many.
    pub async fn sweep_trash(&self, max_age: Duration) -> anyhow::Result<usize> {
        let cutoff = now_ms() - max_age.as_millis() as i64;
        let rows: Vec<(String,)> =
            sqlx::query_as("SELECT id FROM docs WHERE deleted_ms IS NOT NULL AND deleted_ms < ?")
                .bind(cutoff)
                .fetch_all(&self.db)
                .await?;
        let mut n = 0;
        for (id,) in rows {
            let dir = self.root.join(TRASH).join(&id);
            match fs::remove_dir_all(&dir) {
                Ok(()) => n += 1,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => tracing::warn!(doc = %id, error = %e, "trash sweep: directory kept"),
            }
        }
        Ok(n)
    }
}

/// One `notebooks` row (144 §3.3); the name and description live on the `DocKind::Notebook` document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotebookRow {
    /// The notebook document.
    pub id: DocId,
    /// Optional `project` label.
    pub project: Option<String>,
    /// Archived, unix ms.
    pub archived_ms: Option<i64>,
}

/// One `sources` row (144 §3.3): ingest state and provenance of a `DocKind::Source` document.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceRow {
    /// The source document.
    pub id: DocId,
    /// Where it came from.
    pub origin: SourceOrigin,
    /// Ingest state.
    pub status: SourceStatus,
    /// Failure text.
    pub error: Option<String>,
    /// Ingest attempts.
    pub attempts: u32,
    /// Attachment path of the original bytes on the hub (`api.rs` `put_attachment`).
    pub blob: Option<String>,
    /// Last successful fetch, unix ms.
    pub fetched_ms: Option<i64>,
    /// sha256 of the derived markdown at the last success.
    pub content_sha: Option<String>,
    /// §4.2 freshness: the previous `fetched_ms` after a refresh changed the text.
    pub changed_since: Option<i64>,
    /// Original size.
    pub bytes: Option<u64>,
    /// Word count.
    pub words: Option<u32>,
    /// Media length, ms.
    pub duration_ms: Option<u64>,
    /// Page count.
    pub pages: Option<u32>,
    /// Extractor metadata, JSON.
    pub meta: serde_json::Value,
    /// Insight sections written.
    pub insights: u32,
}

type SourceTuple = (
    String,
    String,
    String,
    Option<String>,
    i64,
    Option<String>,
    Option<i64>,
    Option<String>,
    Option<i64>,
    Option<i64>,
    Option<i64>,
    Option<i64>,
    Option<i64>,
    Option<String>,
    i64,
);

/// The `sources` columns of [`SourceTuple`], qualified for `FROM sources s` (a macro: sqlx 0.9 takes
/// only `&'static str` SQL, so the list is spliced with `concat!`).
macro_rules! source_columns {
    () => {
        "s.id, s.origin, s.status, s.error, s.attempts, s.blob, s.fetched_ms, s.content_sha, \
         s.changed_since, s.bytes, s.words, s.duration_ms, s.pages, s.meta, s.insights"
    };
}

fn source_row(t: SourceTuple) -> Option<SourceRow> {
    let (
        id,
        origin,
        status,
        error,
        attempts,
        blob,
        fetched_ms,
        content_sha,
        changed_since,
        bytes,
        words,
        duration_ms,
        pages,
        meta,
        insights,
    ) = t;
    Some(SourceRow {
        id: DocId(id),
        origin: serde_json::from_str(&origin).ok()?,
        status: SourceStatus::parse(&status)?,
        error,
        attempts: attempts.try_into().unwrap_or_default(),
        blob,
        fetched_ms,
        content_sha,
        changed_since,
        bytes: bytes.and_then(|v| v.try_into().ok()),
        words: words.and_then(|v| v.try_into().ok()),
        duration_ms: duration_ms.and_then(|v| v.try_into().ok()),
        pages: pages.and_then(|v| v.try_into().ok()),
        meta: meta
            .and_then(|m| serde_json::from_str(&m).ok())
            .unwrap_or(serde_json::Value::Null),
        insights: insights.try_into().unwrap_or_default(),
    })
}

/// 144 §3.3: the three replicated tables beside `docs`. Every write is a column write the replica
/// merges per column (a removal is `removed_ms`, never a `DELETE`); the engine stamps `hlc`.
impl Store {
    /// Insert or update the `notebooks` row.
    pub async fn notebook_upsert(&self, row: &NotebookRow) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO notebooks (id, project, archived_ms) VALUES (?, ?, ?) \
             ON CONFLICT (id) DO UPDATE SET project = excluded.project, archived_ms = excluded.archived_ms",
        )
        .bind(row.id.as_str())
        .bind(&row.project)
        .bind(row.archived_ms)
        .execute(&self.db)
        .await?;
        Ok(())
    }

    /// The `notebooks` row, when there is one.
    pub async fn notebook_row(&self, id: &DocId) -> anyhow::Result<Option<NotebookRow>> {
        let row: Option<(String, Option<String>, Option<i64>)> =
            sqlx::query_as("SELECT id, project, archived_ms FROM notebooks WHERE id = ?")
                .bind(id.as_str())
                .fetch_optional(&self.db)
                .await?;
        Ok(row.map(|(id, project, archived_ms)| NotebookRow {
            id: DocId(id),
            project,
            archived_ms,
        }))
    }

    /// Live notebooks (document not trashed), newest update first; `archived` includes archived ones.
    pub async fn notebook_rows(&self, archived: bool) -> anyhow::Result<Vec<NotebookRow>> {
        let rows: Vec<(String, Option<String>, Option<i64>)> = sqlx::query_as(
            "SELECT n.id, n.project, n.archived_ms FROM notebooks n JOIN docs d ON d.id = n.id \
             WHERE d.deleted_ms IS NULL AND (? OR n.archived_ms IS NULL) ORDER BY d.updated_ms DESC, d.rowid DESC",
        )
        .bind(archived)
        .fetch_all(&self.db)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(id, project, archived_ms)| NotebookRow {
                id: DocId(id),
                project,
                archived_ms,
            })
            .collect())
    }

    /// Insert or revive one membership row (`removed_ms` back to NULL; `added_ms`/`added_by` kept from
    /// the first add so the row merges as a column write, §3.3).
    pub async fn item_add(&self, item: &NotebookItem) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO notebook_items (id, notebook, kind, item, added_ms, added_by, removed_ms) \
             VALUES (?, ?, ?, ?, ?, ?, NULL) ON CONFLICT (id) DO UPDATE SET removed_ms = NULL",
        )
        .bind(NotebookItem::row_id(&item.notebook, item.kind, &item.item))
        .bind(item.notebook.as_str())
        .bind(item.kind.as_str())
        .bind(&item.item)
        .bind(item.added_ms)
        .bind(&item.added_by)
        .execute(&self.db)
        .await?;
        Ok(())
    }

    /// Soft-remove one membership row at `removed_ms`. `Ok(false)` when there was no live row.
    pub async fn item_remove(
        &self,
        notebook: &DocId,
        kind: ItemKind,
        item: &str,
        removed_ms: i64,
    ) -> anyhow::Result<bool> {
        let r = sqlx::query(
            "UPDATE notebook_items SET removed_ms = ? WHERE id = ? AND removed_ms IS NULL",
        )
        .bind(removed_ms)
        .bind(NotebookItem::row_id(notebook, kind, item))
        .execute(&self.db)
        .await?;
        Ok(r.rows_affected() > 0)
    }

    /// Live items of `notebook`, oldest first.
    pub async fn items(&self, notebook: &DocId) -> anyhow::Result<Vec<NotebookItem>> {
        let rows: Vec<(String, String, String, i64, String, Option<i64>)> = sqlx::query_as(
            "SELECT notebook, kind, item, added_ms, added_by, removed_ms FROM notebook_items \
             WHERE notebook = ? AND removed_ms IS NULL ORDER BY added_ms, rowid",
        )
        .bind(notebook.as_str())
        .fetch_all(&self.db)
        .await?;
        Ok(rows
            .into_iter()
            .filter_map(|(notebook, kind, item, added_ms, added_by, removed_ms)| {
                Some(NotebookItem {
                    notebook: DocId(notebook),
                    kind: ItemKind::parse(&kind)?,
                    item,
                    added_ms,
                    added_by,
                    removed_ms,
                })
            })
            .collect())
    }

    /// Soft-remove every live item of `notebook` at `removed_ms` (notebook delete, §3.4); the rows removed.
    pub async fn items_remove_all(
        &self,
        notebook: &DocId,
        removed_ms: i64,
    ) -> anyhow::Result<Vec<NotebookItem>> {
        let items = self.items(notebook).await?;
        sqlx::query(
            "UPDATE notebook_items SET removed_ms = ? WHERE notebook = ? AND removed_ms IS NULL",
        )
        .bind(removed_ms)
        .bind(notebook.as_str())
        .execute(&self.db)
        .await?;
        Ok(items)
    }

    /// `(sources, notes)` live-item counts per notebook, for every notebook with items.
    pub async fn item_counts(&self) -> anyhow::Result<HashMap<String, (u32, u32)>> {
        let rows: Vec<(String, String, i64)> = sqlx::query_as(
            "SELECT notebook, kind, COUNT(*) FROM notebook_items WHERE removed_ms IS NULL \
             AND kind IN ('source', 'note') GROUP BY notebook, kind",
        )
        .fetch_all(&self.db)
        .await?;
        let mut out: HashMap<String, (u32, u32)> = HashMap::new();
        for (nb, kind, n) in rows {
            let e = out.entry(nb).or_default();
            let n = u32::try_from(n).unwrap_or(u32::MAX);
            if kind == "source" {
                e.0 = n;
            } else {
                e.1 = n;
            }
        }
        Ok(out)
    }

    /// The index row of a live document as `doc_list` shows it; `None` when absent or trashed.
    pub async fn summary(&self, id: &DocId) -> anyhow::Result<Option<DocSummary>> {
        let row: Option<SummaryTuple> = sqlx::query_as(concat!(
            "SELECT ",
            summary_cols!(),
            " FROM docs WHERE id = ? AND deleted_ms IS NULL"
        ))
        .bind(id.as_str())
        .fetch_optional(&self.db)
        .await?;
        Ok(row.and_then(summary_row))
    }

    /// Live notebooks (not trashed, not archived unless `archived`) holding `(kind, item)` through a live
    /// row — the `nbi_item` lookup of the derived share grant (§11) and of "also in N" (§3.4).
    pub async fn notebooks_of(
        &self,
        kind: ItemKind,
        item: &str,
        archived: bool,
    ) -> anyhow::Result<Vec<DocId>> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT i.notebook FROM notebook_items i JOIN docs d ON d.id = i.notebook \
             LEFT JOIN notebooks n ON n.id = i.notebook \
             WHERE i.kind = ? AND i.item = ? AND i.removed_ms IS NULL AND d.deleted_ms IS NULL \
             AND (? OR n.archived_ms IS NULL) ORDER BY i.added_ms",
        )
        .bind(kind.as_str())
        .bind(item)
        .bind(archived)
        .fetch_all(&self.db)
        .await?;
        Ok(rows.into_iter().map(|(id,)| DocId(id)).collect())
    }

    /// Insert or update the whole `sources` row.
    pub async fn source_upsert(&self, row: &SourceRow) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO sources (id, origin, status, error, attempts, blob, fetched_ms, content_sha, changed_since, \
             bytes, words, duration_ms, pages, meta, insights) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT (id) DO UPDATE SET origin = excluded.origin, status = excluded.status, error = excluded.error, \
             attempts = excluded.attempts, blob = excluded.blob, fetched_ms = excluded.fetched_ms, \
             content_sha = excluded.content_sha, changed_since = excluded.changed_since, bytes = excluded.bytes, \
             words = excluded.words, duration_ms = excluded.duration_ms, pages = excluded.pages, meta = excluded.meta, \
             insights = excluded.insights",
        )
        .bind(row.id.as_str())
        .bind(serde_json::to_string(&row.origin)?)
        .bind(row.status.as_str())
        .bind(&row.error)
        .bind(i64::from(row.attempts))
        .bind(&row.blob)
        .bind(row.fetched_ms)
        .bind(&row.content_sha)
        .bind(row.changed_since)
        .bind(row.bytes.map(|v| v as i64))
        .bind(row.words.map(i64::from))
        .bind(row.duration_ms.map(|v| v as i64))
        .bind(row.pages.map(i64::from))
        .bind(serde_json::to_string(&row.meta)?)
        .bind(i64::from(row.insights))
        .execute(&self.db)
        .await?;
        Ok(())
    }

    /// Status, error and attempts only — what the ingest job writes between `new` and `completed`.
    pub async fn source_set_status(
        &self,
        id: &DocId,
        status: SourceStatus,
        error: Option<&str>,
        attempts: u32,
    ) -> anyhow::Result<()> {
        sqlx::query("UPDATE sources SET status = ?, error = ?, attempts = ? WHERE id = ?")
            .bind(status.as_str())
            .bind(error)
            .bind(i64::from(attempts))
            .bind(id.as_str())
            .execute(&self.db)
            .await?;
        Ok(())
    }

    /// `insights` and `meta` only — what the insight pass writes. A whole-row upsert there would
    /// put back the status it read, clobbering a refresh's `queued` written meanwhile.
    pub async fn source_set_insight(
        &self,
        id: &DocId,
        insights: u32,
        meta: &serde_json::Value,
    ) -> anyhow::Result<()> {
        sqlx::query("UPDATE sources SET insights = ?, meta = ? WHERE id = ?")
            .bind(i64::from(insights))
            .bind(serde_json::to_string(meta)?)
            .bind(id.as_str())
            .execute(&self.db)
            .await?;
        Ok(())
    }

    /// 16 §5 (2026-11-11): the local-network grant of the caller who added `id` (written once,
    /// when the source is first added). Engine-local.
    pub async fn set_source_lan(&self, id: &DocId, lan: bool) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO source_lan (id, lan) VALUES (?, ?) \
             ON CONFLICT (id) DO UPDATE SET lan = excluded.lan",
        )
        .bind(id.as_str())
        .bind(lan)
        .execute(&self.db)
        .await?;
        Ok(())
    }

    /// [`Self::set_source_lan`]'s grant; `None` for a source added before grants were kept.
    pub async fn source_lan(&self, id: &DocId) -> anyhow::Result<Option<bool>> {
        let row: Option<(bool,)> = sqlx::query_as("SELECT lan FROM source_lan WHERE id = ?")
            .bind(id.as_str())
            .fetch_optional(&self.db)
            .await?;
        Ok(row.map(|(lan,)| lan))
    }

    /// The `sources` row, when there is one.
    pub async fn source_row(&self, id: &DocId) -> anyhow::Result<Option<SourceRow>> {
        let row: Option<SourceTuple> = sqlx::query_as(concat!(
            "SELECT ",
            source_columns!(),
            " FROM sources s WHERE s.id = ?"
        ))
        .bind(id.as_str())
        .fetch_optional(&self.db)
        .await?;
        Ok(row.and_then(source_row))
    }

    /// Sources whose document is live: those of `notebook` (live items, oldest first), or every one
    /// (newest update first) — the unfiled ones included.
    pub async fn source_rows(&self, notebook: Option<&DocId>) -> anyhow::Result<Vec<SourceRow>> {
        let rows: Vec<SourceTuple> = match notebook {
            Some(nb) => {
                sqlx::query_as(concat!(
                    "SELECT ",
                    source_columns!(),
                    " FROM sources s JOIN docs d ON d.id = s.id \
                     JOIN notebook_items i ON i.kind = 'source' AND i.item = s.id \
                     WHERE i.notebook = ? AND i.removed_ms IS NULL AND d.deleted_ms IS NULL ORDER BY i.added_ms, i.rowid"
                ))
                .bind(nb.as_str())
                .fetch_all(&self.db)
                .await?
            }
            None => {
                sqlx::query_as(concat!(
                    "SELECT ",
                    source_columns!(),
                    " FROM sources s JOIN docs d ON d.id = s.id \
                     WHERE d.deleted_ms IS NULL ORDER BY d.updated_ms DESC, d.rowid DESC"
                ))
                .fetch_all(&self.db)
                .await?
            }
        };
        Ok(rows.into_iter().filter_map(source_row).collect())
    }
}

/// Labels the store may prune (§4.6): only the automatic daily one. `import-file` (the state before a
/// large foreign edit), `artifact-v*`, `meeting-*` and manual labels are the history a person may need.
const AUTO_LABELS: &[&str] = &["daily"];

fn sanitize(label: &str) -> String {
    label
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(40)
        .collect()
}

/// [`write_atomic`] for the mirror (its temp is dot-prefixed, which the replica's scan ignores, so
/// a half-written copy is never a row); refuses a path without a UTF-8 file name.
fn write_hidden(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if path.parent().is_none() || path.file_name().and_then(|n| n.to_str()).is_none() {
        return Err(std::io::Error::other("mirror path without a file name"));
    }
    write_atomic(path, bytes)
}

/// A peer's full Yjs state (`snapshot.yjs` bytes, v1) as a document of its own, to compare with and
/// diff against a local one (`theirs.transact().encode_state_as_update_v1(&ours_sv)`).
pub fn decode_state(snapshot: &[u8]) -> anyhow::Result<Doc> {
    let theirs = Doc::new();
    theirs
        .transact_mut()
        .apply_update(Update::decode_v1(snapshot).context("foreign snapshot.yjs")?)
        .context("apply foreign snapshot.yjs")?;
    Ok(theirs)
}

/// Whether `a` holds state `b` lacks: a clock above `b`'s for some client, or a deletion `b` has
/// not. This — not the length of `encode_state_as_update_v1(&b_sv)`, which always appends `a`'s
/// whole delete set — is what decides whether a peer's snapshot is news (69 E2).
pub fn exceeds(a: &Doc, b: &Doc) -> bool {
    let (a, b) = (a.transact().snapshot(), b.transact().snapshot());
    a.state_map
        .iter()
        .any(|(client, clock)| *clock > b.state_map.get(client))
        || !a.delete_set.diff(&b.delete_set).is_empty()
}

/// [`exceeds`] for a peer's `snapshot` bytes against `doc`.
pub fn adds_anything(snapshot: &[u8], doc: &Doc) -> anyhow::Result<bool> {
    Ok(exceeds(&decode_state(snapshot)?, doc))
}

/// Synced temp + rename; parents created.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    platform::files::write_atomic_synced(path, bytes)
}

/// Iterate `updates.log` records: `(tag, update)`. A truncated tail (a crash mid-append) ends the iteration.
pub fn records(log: &[u8]) -> impl Iterator<Item = (u8, &[u8])> {
    let mut at = 0;
    std::iter::from_fn(move || {
        let tag = *log.get(at)?;
        at += 1;
        at += match tag {
            2 => 16,
            4 => 32,
            _ => 0,
        };
        let len = u32::from_be_bytes(log.get(at..at + 4)?.try_into().ok()?) as usize;
        at += 4;
        let update = log.get(at..at + len)?;
        at += len;
        Some((tag, update))
    })
}

/// sha256 hex of `bytes`.
pub fn sha256(bytes: &[u8]) -> String {
    platform::sha256::hex(bytes)
}

/// Read a whole file, `None` when absent.
pub fn read_opt(path: &Path) -> std::io::Result<Option<Vec<u8>>> {
    match File::open(path) {
        Ok(mut f) => {
            let mut v = Vec::new();
            f.read_to_end(&mut v)?;
            Ok(Some(v))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::AgentStyle;
    use yrs::{GetString, Text};

    fn meta(binding: Binding) -> DocMeta {
        let id = DocId::new();
        DocMeta {
            derived: binding.derived(&id),
            id,
            kind: binding.kind(),
            title: "t".into(),
            binding,
            fragment: "document-store".into(),
            members: vec![],
            links: vec![],
            agent: AgentStyle::default(),
            last_export: None,
            authors: Default::default(),
            created_ms: 1,
            updated_ms: 1,
            home: None,
            home_ms: 0,
            serve_epoch: 0,
            served_by: None,
            revoked: Default::default(),
        }
    }

    fn text_update(doc: &Doc, s: &str) -> Vec<u8> {
        let t = doc.get_or_insert_text("t");
        let mut txn = doc.transact_mut();
        let len = t.len(&txn);
        t.insert(&mut txn, len, s);
        txn.encode_update_v1()
    }

    /// 144 §3.3: the three tables open beside `docs`; items are deterministic rows that revive on
    /// re-add, "also in" and the grant lookup read live notebooks only, sources list per notebook.
    #[tokio::test]
    async fn notebook_tables_items_revive_and_live_lookups() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).await.unwrap();
        let nb_a = meta(Binding::Notebook);
        let nb_b = meta(Binding::Notebook);
        let mut src = meta(Binding::Source("url:http://x/".into()));
        src.id = DocId::for_binding(&src.binding);
        let note = meta(Binding::Note);
        for m in [&nb_a, &nb_b, &src, &note] {
            store.create(m).await.unwrap();
        }
        assert_eq!(src.kind, DocKind::Source);
        assert_eq!(
            src.id,
            DocId::for_binding(&src.binding),
            "a source id is the v5 of its origin key: the same on every engine"
        );
        assert_ne!(
            DocId::for_binding(&Binding::Notebook),
            DocId::for_binding(&Binding::Notebook),
            "notebooks are keyless"
        );
        assert!(
            store
                .create(&meta(Binding::Source("url:http://x/".into())))
                .await
                .is_err(),
            "one document per origin key"
        );
        store
            .notebook_upsert(&NotebookRow {
                id: nb_a.id.clone(),
                project: Some("p".into()),
                archived_ms: None,
            })
            .await
            .unwrap();
        store
            .notebook_upsert(&NotebookRow {
                id: nb_b.id.clone(),
                project: None,
                archived_ms: Some(5),
            })
            .await
            .unwrap();
        assert_eq!(store.notebook_rows(false).await.unwrap().len(), 1);
        assert_eq!(store.notebook_rows(true).await.unwrap().len(), 2);

        let item = |nb: &DocMeta, kind, item: &DocMeta| NotebookItem {
            notebook: nb.id.clone(),
            kind,
            item: item.id.to_string(),
            added_ms: 10,
            added_by: "owner".into(),
            removed_ms: None,
        };
        store
            .item_add(&item(&nb_a, ItemKind::Source, &src))
            .await
            .unwrap();
        store
            .item_add(&item(&nb_b, ItemKind::Source, &src))
            .await
            .unwrap();
        store
            .item_add(&item(&nb_a, ItemKind::Note, &note))
            .await
            .unwrap();
        assert_eq!(store.items(&nb_a.id).await.unwrap().len(), 2);
        assert_eq!(store.item_counts().await.unwrap()[nb_a.id.as_str()], (1, 1));
        assert_eq!(
            store
                .notebooks_of(ItemKind::Source, src.id.as_str(), true)
                .await
                .unwrap(),
            vec![nb_a.id.clone(), nb_b.id.clone()]
        );
        assert_eq!(
            store
                .notebooks_of(ItemKind::Source, src.id.as_str(), false)
                .await
                .unwrap(),
            vec![nb_a.id.clone()],
            "archived notebooks are left out when asked"
        );
        assert_eq!(
            store.source_rows(Some(&nb_a.id)).await.unwrap().len(),
            0,
            "no sources row yet"
        );
        store
            .source_upsert(&SourceRow {
                id: src.id.clone(),
                origin: SourceOrigin::Url {
                    url: "http://x/".into(),
                    canonical: None,
                },
                status: SourceStatus::New,
                error: None,
                attempts: 0,
                blob: None,
                fetched_ms: None,
                content_sha: None,
                changed_since: None,
                bytes: Some(7),
                words: None,
                duration_ms: Some(1000),
                pages: None,
                meta: serde_json::json!({ "a": 1 }),
                insights: 0,
            })
            .await
            .unwrap();
        store
            .source_set_status(&src.id, SourceStatus::Failed, Some("boom"), 2)
            .await
            .unwrap();
        let row = store.source_row(&src.id).await.unwrap().unwrap();
        assert_eq!(
            (
                row.status,
                row.error.as_deref(),
                row.attempts,
                row.bytes,
                row.duration_ms
            ),
            (SourceStatus::Failed, Some("boom"), 2, Some(7), Some(1000))
        );
        assert_eq!(row.meta["a"], 1);
        assert_eq!(store.source_rows(Some(&nb_a.id)).await.unwrap().len(), 1);
        assert_eq!(store.source_rows(None).await.unwrap().len(), 1);

        // Soft removal, then revival keeps the first add.
        assert!(
            store
                .item_remove(&nb_a.id, ItemKind::Source, src.id.as_str(), 20)
                .await
                .unwrap()
        );
        assert!(
            !store
                .item_remove(&nb_a.id, ItemKind::Source, src.id.as_str(), 21)
                .await
                .unwrap(),
            "already removed"
        );
        assert_eq!(store.items(&nb_a.id).await.unwrap().len(), 1);
        assert_eq!(store.source_rows(Some(&nb_a.id)).await.unwrap().len(), 0);
        let mut again = item(&nb_a, ItemKind::Source, &src);
        again.added_ms = 99;
        again.added_by = "agent:x".into();
        store.item_add(&again).await.unwrap();
        let items = store.items(&nb_a.id).await.unwrap();
        let revived = items.iter().find(|i| i.kind == ItemKind::Source).unwrap();
        assert_eq!((revived.added_ms, revived.added_by.as_str()), (10, "owner"));

        // A trashed notebook stops holding its items; items_remove_all empties it.
        store.delete(&nb_b.id).await.unwrap();
        assert_eq!(
            store
                .notebooks_of(ItemKind::Source, src.id.as_str(), true)
                .await
                .unwrap(),
            vec![nb_a.id.clone()]
        );
        let removed = store.items_remove_all(&nb_a.id, 30).await.unwrap();
        assert_eq!(removed.len(), 2);
        assert!(store.items(&nb_a.id).await.unwrap().is_empty());
        assert_eq!(store.summary(&nb_b.id).await.unwrap(), None);
        assert_eq!(
            store.summary(&nb_a.id).await.unwrap().map(|s| s.kind),
            Some(DocKind::Notebook)
        );
    }

    /// SEAL: a pre-SEAL `meta.json` with a plaintext `links[].secret_hash` is sealed on first read —
    /// file, mirror copy and a foreign plaintext meta alike — and the sealed value opens to the same
    /// HMAC key, so the guest's proof still verifies.
    #[tokio::test]
    async fn plaintext_link_hash_is_sealed_on_load() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Arc::new(
            secrets::Vault::open(dir.path(), secrets::KeyStore::File)
                .await
                .unwrap(),
        );
        let store = Store::open(dir.path())
            .await
            .unwrap()
            .with_vault(vault.clone());
        let m = meta(Binding::Note);
        store.create(&m).await.unwrap();
        let hash = crate::share::secret_hash(b"the link secret");
        let plain_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hash);
        let mut v = serde_json::to_value(&m).unwrap();
        v["links"] = serde_json::json!([{
            "id": "lnk_old", "role": "editor", "secret_hash": plain_b64, "name": null,
            "created_ms": 5, "expires_ms": null, "revoked_ms": null
        }]);
        let p = store.dir(&m.id).join(META);
        fs::write(&p, serde_json::to_vec(&v).unwrap()).unwrap();

        let loaded = store.read_meta(&m.id).unwrap();
        let link = &loaded.links[0];
        assert!(!link.secret_hash_sealed.is_empty());
        assert_ne!(link.secret_hash_sealed, plain_b64);
        let opened = vault
            .open_with_mesh_key(
                b"lnk_old",
                &crate::share::unb64url(&link.secret_hash_sealed).unwrap(),
            )
            .unwrap();
        assert_eq!(opened.as_slice(), &hash[..], "the proof key is unchanged");
        // The guest's proof (the math is unchanged) verifies against the migrated link.
        let (h, g) = ([1u8; 32], [2u8; 32]);
        assert!(crate::share::verify(
            Some(&vault),
            link,
            &h,
            &g,
            &crate::share::proof(&hash, &h, &g)
        ));
        assert!(!crate::share::verify(
            Some(&vault),
            link,
            &h,
            &g,
            &crate::share::proof(&[0u8; 32], &h, &g)
        ));
        // Rewritten in place, and the mirror copy is the sealed one.
        for bytes in [
            fs::read(&p).unwrap(),
            fs::read(
                store
                    .mirror_dir()
                    .join(mirror::file_name(&m.id, MirrorFile::Meta)),
            )
            .unwrap(),
        ] {
            let on_disk: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert!(
                on_disk["links"][0].get("secret_hash").is_none(),
                "no plaintext left"
            );
            assert_eq!(
                on_disk["links"][0]["secret_hash_sealed"],
                link.secret_hash_sealed
            );
        }
        // Second read: nothing to migrate, same value.
        assert_eq!(
            store.read_meta(&m.id).unwrap().links[0].secret_hash_sealed,
            link.secret_hash_sealed
        );
        // A foreign plaintext meta (an engine not yet upgraded) is sealed before it merges.
        let (foreign, sealed) = store.parse_meta(&serde_json::to_vec(&v).unwrap()).unwrap();
        assert!(sealed);
        assert_eq!(
            vault
                .open_with_mesh_key(
                    b"lnk_old",
                    &crate::share::unb64url(&foreign.links[0].secret_hash_sealed).unwrap()
                )
                .unwrap()
                .as_slice(),
            &hash[..]
        );
        // Without a vault nothing is rewritten.
        let bare = Store::open(dir.path()).await.unwrap();
        assert!(!bare.parse_meta(&serde_json::to_vec(&v).unwrap()).unwrap().1);
    }

    #[tokio::test]
    async fn create_append_snapshot_load_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).await.unwrap();
        let m = meta(Binding::Knowledge("facts/a".into()));
        store.create(&m).await.unwrap();
        assert_eq!(store.lookup(&m.binding).await.unwrap(), Some(m.id.clone()));
        assert!(store.create(&m).await.is_err(), "binding bound twice");

        let doc = Doc::new();
        let u1 = text_update(&doc, "hello ");
        let u2 = text_update(&doc, "world");
        store
            .append(&m.id, &Principal::Owner { device: Some(7) }, &u1)
            .unwrap();
        store
            .append(
                &m.id,
                &Principal::Agent {
                    session: uuid::Uuid::now_v7(),
                },
                &u2,
            )
            .unwrap();
        let l = store.load(&m.id).unwrap();
        assert_eq!(l.log_count, 2);
        assert_eq!(l.snapshot_bytes, 0);
        assert_eq!(
            l.doc.get_or_insert_text("t").get_string(&l.doc.transact()),
            "hello world"
        );

        // Tags and session ids frame the records.
        let log = fs::read(store.dir(&m.id).join(LOG)).unwrap();
        let tags: Vec<u8> = records(&log).map(|(t, _)| t).collect();
        assert_eq!(tags, vec![1, 2]);

        // Snapshot truncates the log; load is identical.
        let n = store.snapshot(&m.id, &l.doc, Some("import-file")).unwrap();
        assert!(n > 0);
        let l2 = store.load(&m.id).unwrap();
        assert_eq!(l2.log_count, 0);
        assert_eq!(l2.snapshot_bytes, n);
        assert_eq!(
            l2.doc
                .get_or_insert_text("t")
                .get_string(&l2.doc.transact()),
            "hello world"
        );
        let snaps = store.snapshots(&m.id).unwrap();
        assert_eq!(snaps.len(), 1);
        assert_eq!(snaps[0].label, "import-file");
        assert_eq!(store.read_snapshot(&m.id, &snaps[0].name).unwrap().len(), n);

        // A truncated tail record is ignored, not fatal.
        let u3 = text_update(&doc, "!");
        store.append(&m.id, &Principal::File, &u3).unwrap();
        let mut log = fs::read(store.dir(&m.id).join(LOG)).unwrap();
        log.extend_from_slice(&[5, 0, 0, 0, 9, 1, 2]);
        fs::write(store.dir(&m.id).join(LOG), &log).unwrap();
        let l3 = store.load(&m.id).unwrap();
        assert_eq!(l3.log_count, 1);
        assert_eq!(
            l3.doc
                .get_or_insert_text("t")
                .get_string(&l3.doc.transact()),
            "hello world!"
        );

        // List and delete.
        assert_eq!(
            store
                .list(Some(DocKind::Knowledge), 10)
                .await
                .unwrap()
                .len(),
            1
        );
        store.delete(&m.id).await.unwrap();
        assert_eq!(store.lookup(&m.binding).await.unwrap(), None);
        assert!(
            store
                .root()
                .join(TRASH)
                .join(m.id.as_str())
                .join(META)
                .exists()
        );
        assert!(store.list(None, 10).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn prune_keeps_labelled_snapshots() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).await.unwrap();
        let m = meta(Binding::Note);
        store.create(&m).await.unwrap();
        let doc = Doc::new();
        for i in 0..6 {
            std::thread::sleep(Duration::from_millis(2));
            let label = match i {
                1 => "manual",
                3 => "import-file",
                4 => "artifact-v2",
                _ => "daily",
            };
            store.snapshot(&m.id, &doc, Some(label)).unwrap();
        }
        store.prune_snapshots(&m.id, 2).unwrap();
        let labels: Vec<String> = store
            .snapshots(&m.id)
            .unwrap()
            .into_iter()
            .map(|s| s.label)
            .collect();
        // Newest first: the two newest dailies stay, the oldest daily (i = 0) went, every other label is kept.
        assert_eq!(
            labels,
            vec!["daily", "artifact-v2", "import-file", "daily", "manual"]
        );
    }

    #[tokio::test]
    async fn trash_sweep_removes_only_old_deletions() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).await.unwrap();
        let old = meta(Binding::Note);
        let young = meta(Binding::Note);
        store.create(&old).await.unwrap();
        store.create(&young).await.unwrap();
        store.delete(&old.id).await.unwrap();
        store.delete(&young.id).await.unwrap();
        let trash = store.root().join(TRASH);
        assert!(trash.join(old.id.as_str()).is_dir());
        // Only `old` was deleted before the cutoff.
        sqlx::query("UPDATE docs SET deleted_ms = deleted_ms - 100000 WHERE id = ?")
            .bind(old.id.as_str())
            .execute(&store.db)
            .await
            .unwrap();
        assert_eq!(
            store
                .sweep_trash(Duration::from_millis(50000))
                .await
                .unwrap(),
            1
        );
        assert!(!trash.join(old.id.as_str()).exists());
        assert!(trash.join(young.id.as_str()).is_dir());
        // 69: the row is a tombstone other engines merge against, never deleted.
        let rows: Vec<(String, Option<i64>)> =
            sqlx::query_as("SELECT id, deleted_ms FROM docs ORDER BY id")
                .fetch_all(&store.db)
                .await
                .unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|(_, d)| d.is_some()));
    }

    /// 69: two v7 rows of one knowledge binding become one document at the deterministic id with both
    /// texts; a lone v7 row is moved; a soft-deleted binding can be recreated under the same id.
    #[tokio::test]
    async fn open_converges_ids_and_merges_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        let binding = Binding::Knowledge("facts/dup".into());
        let lone = Binding::MeetingNote("m9".into());
        let (a, b, l) = {
            let store = Store::open(dir.path()).await.unwrap();
            let mut a = meta(binding.clone());
            a.updated_ms = 10;
            let mut b = meta(binding.clone());
            b.updated_ms = 20;
            let l = meta(lone.clone());
            // `create` refuses a second live row; the duplicates of the old world were minted by engines apart.
            for m in [&a, &b, &l] {
                let d = store.dir(&m.id);
                fs::create_dir_all(d.join(SNAPSHOTS)).unwrap();
                write_atomic(&d.join(META), &serde_json::to_vec(m).unwrap()).unwrap();
                sqlx::query("INSERT INTO docs (id, kind, title, binding, created_ms, updated_ms) VALUES (?, ?, ?, ?, 1, ?)")
                    .bind(m.id.as_str())
                    .bind(m.kind.to_string())
                    .bind(&m.title)
                    .bind(m.binding.key())
                    .bind(m.updated_ms)
                    .execute(&store.db)
                    .await
                    .unwrap();
            }
            let da = Doc::new();
            store
                .append(&a.id, &Principal::File, &text_update(&da, "from a"))
                .unwrap();
            let db = Doc::new();
            store
                .append(&b.id, &Principal::File, &text_update(&db, "from b"))
                .unwrap();
            (a, b, l)
        };
        let store = Store::open(dir.path()).await.unwrap();
        let want = DocId::for_binding(&binding);
        assert_eq!(store.lookup(&binding).await.unwrap(), Some(want.clone()));
        assert_eq!(
            store.lookup(&lone).await.unwrap(),
            Some(DocId::for_binding(&lone))
        );
        let loaded = store.load(&want).unwrap();
        let text = loaded
            .doc
            .get_or_insert_text("t")
            .get_string(&loaded.doc.transact());
        assert!(text.contains("from a") && text.contains("from b"), "{text}");
        assert_eq!(loaded.log_count, 0, "merged state was snapshotted");
        assert!(
            !store.dir(&a.id).exists() && !store.dir(&b.id).exists() && !store.dir(&l.id).exists()
        );
        assert!(
            store.root().join(TRASH).join(a.id.as_str()).is_dir(),
            "the older duplicate is trashed"
        );
        assert_eq!(store.read_meta(&want).unwrap().id, want);
        // Idempotent.
        Store::open(dir.path()).await.unwrap();
        assert_eq!(store.list(None, 10).await.unwrap().len(), 2);
        // Soft delete, then recreate: same id, row revived.
        store.delete(&want).await.unwrap();
        let mut again = meta(binding.clone());
        again.id = want.clone();
        store.create(&again).await.unwrap();
        assert_eq!(store.lookup(&binding).await.unwrap(), Some(want));
    }

    /// TASK-NOTES-HIDDEN: a thread minted as a note is rebound to its task — off the notes list,
    /// on the thread list, its derived file moved — and keeps its id across a reopen.
    #[tokio::test]
    async fn bind_rekinds_a_note_into_a_task_thread() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).await.unwrap();
        let note = meta(Binding::Note);
        let thread = meta(Binding::Note);
        store.create(&note).await.unwrap();
        store.create(&thread).await.unwrap();
        let md = dir.path().join(&thread.derived);
        fs::write(&md, "# Task 1\n").unwrap();
        let binding = Binding::TaskThread("task-1".into());

        assert!(store.bind(&thread.id, &binding).await.unwrap());
        assert!(
            !store.bind(&thread.id, &binding).await.unwrap(),
            "idempotent"
        );
        let ids = |v: Vec<DocSummary>| v.into_iter().map(|d| d.id).collect::<Vec<_>>();
        assert_eq!(
            ids(store.list(Some(DocKind::Note), 10).await.unwrap()),
            vec![note.id.clone()]
        );
        assert_eq!(
            ids(store.list(Some(DocKind::TaskThread), 10).await.unwrap()),
            vec![thread.id.clone()]
        );
        assert_eq!(
            store.lookup(&binding).await.unwrap(),
            Some(thread.id.clone())
        );
        let m = store.read_meta(&thread.id).unwrap();
        assert_eq!(
            (m.kind, &m.binding, m.derived.as_str()),
            (DocKind::TaskThread, &binding, &*binding.derived(&thread.id))
        );
        assert!(
            !md.exists() && dir.path().join(&m.derived).is_file(),
            "derived file followed"
        );
        // Another note cannot take a bound task; the thread's id is not converged on reopen.
        let e = store
            .bind(&note.id, &binding)
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains("already document"), "{e}");
        drop(store);
        let store = Store::open(dir.path()).await.unwrap();
        assert_eq!(
            store.lookup(&binding).await.unwrap(),
            Some(thread.id.clone())
        );
        assert!(store.dir(&thread.id).is_dir());
    }

    /// 69: engine A's mirror files, projected into engine B's `mirror/`, create the document on B;
    /// a later snapshot merges; B's copy back to A brings nothing new; own writes are ignored.
    #[tokio::test]
    async fn mirror_files_create_then_merge_and_never_loop() {
        let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let a = Store::open(da.path()).await.unwrap();
        let b = Store::open(db.path()).await.unwrap();
        let m = meta(Binding::Knowledge("facts/m".into()));
        a.create(&m).await.unwrap();
        let doc = Doc::new();
        a.append(&m.id, &Principal::File, &text_update(&doc, "one "))
            .unwrap();
        a.snapshot(&m.id, &doc, None).unwrap();
        let names = [
            mirror::file_name(&m.id, MirrorFile::Meta),
            mirror::file_name(&m.id, MirrorFile::Snapshot),
        ];
        for n in &names {
            assert!(a.mirror_dir().join(n).is_file(), "{n} mirrored");
            assert_eq!(
                a.mirror_apply(&a.mirror_dir().join(n)).await.unwrap(),
                Applied::Ignored,
                "own write"
            );
        }
        assert!(
            fs::read_dir(a.mirror_dir()).unwrap().all(|e| !e
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp")),
            "no temp left in the mirror"
        );
        assert_eq!(
            a.last_projected(&m.id),
            Some(sha256(&fs::read(a.mirror_dir().join(&names[1])).unwrap()))
        );

        // Project onto B, snapshot first: deferred until its meta is there; then created from both.
        let project =
            |n: &str| fs::copy(a.mirror_dir().join(n), b.mirror_dir().join(n)).map(|_| ());
        project(&names[1]).unwrap();
        assert_eq!(
            b.mirror_apply(&b.mirror_dir().join(&names[1]))
                .await
                .unwrap(),
            Applied::Deferred(m.id.clone())
        );
        project(&names[0]).unwrap();
        assert_eq!(
            b.mirror_apply(&b.mirror_dir().join(&names[0]))
                .await
                .unwrap(),
            Applied::Created(m.id.clone())
        );
        let lb = b.load(&m.id).unwrap();
        assert_eq!(
            lb.doc
                .get_or_insert_text("t")
                .get_string(&lb.doc.transact()),
            "one "
        );
        assert_eq!(b.read_meta(&m.id).unwrap().binding, m.binding);
        // B's live copy (as the hub's open `Doc` would be) and a local edit that A has not seen.
        let open_b = lb.doc;
        b.append(&m.id, &Principal::File, &text_update(&open_b, "B!"))
            .unwrap();

        // A edits and snapshots again: B merges on disk — its text gains A's part, keeps its own, the
        // log is compacted, and A's bytes did not replace B's snapshot.
        a.append(&m.id, &Principal::File, &text_update(&doc, "two"))
            .unwrap();
        a.snapshot(&m.id, &doc, None).unwrap();
        project(&names[1]).unwrap();
        let foreign = fs::read(b.mirror_dir().join(&names[1])).unwrap();
        assert_eq!(
            b.mirror_apply(&b.mirror_dir().join(&names[1]))
                .await
                .unwrap(),
            Applied::Merged(m.id.clone())
        );
        let lb = b.load(&m.id).unwrap();
        let text_b = lb
            .doc
            .get_or_insert_text("t")
            .get_string(&lb.doc.transact());
        assert!(
            text_b.contains("one ") && text_b.contains("two") && text_b.contains("B!"),
            "{text_b}"
        );
        assert_eq!(
            text_b.len(),
            "one twoB!".len(),
            "nothing duplicated: {text_b}"
        );
        assert_eq!(lb.log_count, 0);
        assert_ne!(
            fs::read(b.dir(&m.id).join(SNAPSHOT)).unwrap(),
            foreign,
            "B held its own edit: a real merge re-encodes"
        );
        // The same foreign snapshot into the open `Doc` (the actor's path): news once, then nothing.
        assert!(adds_anything(&foreign, &open_b).unwrap());
        let theirs = decode_state(&foreign).unwrap();
        let diff = theirs
            .transact()
            .encode_state_as_update_v1(&open_b.transact().state_vector());
        open_b
            .transact_mut()
            .apply_update(Update::decode_v1(&diff).unwrap())
            .unwrap();
        assert_eq!(
            open_b
                .get_or_insert_text("t")
                .get_string(&open_b.transact()),
            text_b
        );
        assert!(!adds_anything(&foreign, &open_b).unwrap());
        assert!(exceeds(&open_b, &theirs), "B's own edit is what A lacks");
        // Same bytes again: own projection now (B re-snapshotted and wrote the mirror) or nothing new.
        assert!(matches!(
            b.mirror_apply(&b.mirror_dir().join(&names[1]))
                .await
                .unwrap(),
            Applied::Ignored | Applied::Unchanged
        ));
        // B's snapshot back on A: A merges B's edit, then a second copy brings nothing.
        fs::copy(
            b.mirror_dir().join(&names[1]),
            a.mirror_dir().join(&names[1]),
        )
        .unwrap();
        assert_eq!(
            a.mirror_apply(&a.mirror_dir().join(&names[1]))
                .await
                .unwrap(),
            Applied::Merged(m.id.clone())
        );
        let la = a.load(&m.id).unwrap();
        assert_eq!(
            la.doc
                .get_or_insert_text("t")
                .get_string(&la.doc.transact()),
            text_b,
            "A and B converge"
        );
        fs::copy(
            b.mirror_dir().join(&names[1]),
            a.mirror_dir().join(&names[1]),
        )
        .unwrap();
        assert!(matches!(
            a.mirror_apply(&a.mirror_dir().join(&names[1]))
                .await
                .unwrap(),
            Applied::Ignored | Applied::Unchanged
        ));

        // A document with no snapshot yet (created, never edited) still appears on B from its meta
        // alone; its row (replicated on its own) makes it visible; its first snapshot merges in.
        let m2 = meta(Binding::Knowledge("facts/empty".into()));
        a.create(&m2).await.unwrap();
        let n2 = [
            mirror::file_name(&m2.id, MirrorFile::Meta),
            mirror::file_name(&m2.id, MirrorFile::Snapshot),
        ];
        assert!(!a.mirror_dir().join(&n2[1]).exists());
        project(&n2[0]).unwrap();
        assert_eq!(
            b.mirror_apply(&b.mirror_dir().join(&n2[0])).await.unwrap(),
            Applied::Created(m2.id.clone())
        );
        assert_eq!(b.lookup(&m2.binding).await.unwrap(), None, "no row yet");
        sqlx::query("INSERT INTO docs (id, kind, title, binding, created_ms, updated_ms) VALUES (?, 'knowledge', 't', ?, 1, 1)")
            .bind(m2.id.as_str())
            .bind(m2.binding.key())
            .execute(&b.db)
            .await
            .unwrap();
        assert_eq!(b.lookup(&m2.binding).await.unwrap(), Some(m2.id.clone()));
        let d2 = Doc::new();
        a.append(&m2.id, &Principal::File, &text_update(&d2, "first"))
            .unwrap();
        a.snapshot(&m2.id, &d2, None).unwrap();
        project(&n2[1]).unwrap();
        assert_eq!(
            b.mirror_apply(&b.mirror_dir().join(&n2[1])).await.unwrap(),
            Applied::Merged(m2.id.clone())
        );
        let l2 = b.load(&m2.id).unwrap();
        assert_eq!(
            l2.doc
                .get_or_insert_text("t")
                .get_string(&l2.doc.transact()),
            "first"
        );

        // A foreign meta with a new member joins; A's stays local otherwise.
        let mut theirs = b.read_meta(&m.id).unwrap();
        theirs.members.push(crate::meta::Member {
            pub_: "p".into(),
            name: "Pat".into(),
            color: String::new(),
            role: crate::meta::Role::Viewer,
            added_ms: 5,
            via: None,
            last_seen_ms: 5,
        });
        fs::write(
            a.mirror_dir().join(&names[0]),
            serde_json::to_vec(&theirs).unwrap(),
        )
        .unwrap();
        assert_eq!(
            a.mirror_apply(&a.mirror_dir().join(&names[0]))
                .await
                .unwrap(),
            Applied::Merged(m.id.clone())
        );
        assert_eq!(a.read_meta(&m.id).unwrap().members.len(), 1);

        // Delete on A removes its mirror files; the projected removal on B trashes B's directory once the row is a tombstone.
        a.delete(&m.id).await.unwrap();
        assert!(!a.mirror_dir().join(&names[0]).exists());
        fs::remove_file(b.mirror_dir().join(&names[0])).unwrap();
        sqlx::query("INSERT INTO docs (id, kind, title, binding, created_ms, updated_ms, deleted_ms) VALUES (?, 'knowledge', 't', ?, 1, 1, 2)")
            .bind(m.id.as_str())
            .bind(m.binding.key())
            .execute(&b.db)
            .await
            .unwrap();
        assert_eq!(
            b.mirror_apply(&b.mirror_dir().join(&names[0]))
                .await
                .unwrap(),
            Applied::Ignored
        );
        assert!(!b.dir(&m.id).exists());
        assert!(b.root().join(TRASH).join(m.id.as_str()).is_dir());
    }

    /// 184: a meta-only write (a merged fence or home) re-publishes `meta.json` only: a newer
    /// snapshot that arrived in the mirror meanwhile is applied, not replaced by this engine's
    /// older `snapshot.yjs` (which then won everywhere and left this engine's copy stale).
    #[tokio::test]
    async fn a_meta_write_never_republishes_an_older_snapshot() {
        let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let a = Store::open(da.path()).await.unwrap();
        let b = Store::open(db.path()).await.unwrap();
        let m = meta(Binding::Note);
        a.create(&m).await.unwrap();
        let doc = Doc::new();
        a.append(&m.id, &Principal::File, &text_update(&doc, "one "))
            .unwrap();
        a.snapshot(&m.id, &doc, None).unwrap();
        let names = [
            mirror::file_name(&m.id, MirrorFile::Meta),
            mirror::file_name(&m.id, MirrorFile::Snapshot),
        ];
        let project =
            |n: &str| fs::copy(a.mirror_dir().join(n), b.mirror_dir().join(n)).map(|_| ());
        for n in [&names[1], &names[0]] {
            project(n).unwrap();
            b.mirror_apply(&b.mirror_dir().join(n)).await.unwrap();
        }
        // A (the server) types on and snapshots; its new meta (a moved home) and snapshot reach
        // B's mirror, and B applies the meta first.
        a.append(&m.id, &Principal::File, &text_update(&doc, "two "))
            .unwrap();
        a.snapshot(&m.id, &doc, None).unwrap();
        let mut moved = a.read_meta(&m.id).unwrap();
        moved.home = Some(mesh::EngineId("a".into()));
        moved.home_ms = 2;
        a.write_meta(&moved).await.unwrap();
        project(&names[1]).unwrap();
        project(&names[0]).unwrap();
        b.mirror_apply(&b.mirror_dir().join(&names[0]))
            .await
            .unwrap();
        b.mirror_apply(&b.mirror_dir().join(&names[1]))
            .await
            .unwrap();
        let lb = b.load(&m.id).unwrap();
        assert_eq!(
            lb.doc
                .get_or_insert_text("t")
                .get_string(&lb.doc.transact()),
            "one two "
        );
        assert_eq!(lb.meta.home, Some(mesh::EngineId("a".into())));
        assert_eq!(
            fs::read(b.mirror_dir().join(&names[1])).unwrap(),
            fs::read(a.mirror_dir().join(&names[1])).unwrap(),
            "B's mirror keeps A's snapshot"
        );
    }

    /// A store's mirror files as the replica rows them: name → bytes (dot files are never rows).
    fn rows(store: &Store) -> std::collections::BTreeMap<String, Vec<u8>> {
        fs::read_dir(store.mirror_dir())
            .unwrap()
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                (!name.starts_with('.')).then(|| (name, fs::read(e.path()).unwrap()))
            })
            .collect()
    }

    /// Files of `from` that `to` does not hold byte for byte: copied and applied on `to`, as one
    /// replica batch per file. Returns how many moved.
    async fn exchange(from: &Store, to: &Store) -> usize {
        let theirs = rows(to);
        let mut moved = 0;
        for (name, bytes) in rows(from) {
            if theirs.get(&name) == Some(&bytes) {
                continue;
            }
            let dst = to.mirror_dir().join(&name);
            fs::write(&dst, &bytes).unwrap();
            to.mirror_apply(&dst).await.unwrap();
            moved += 1;
        }
        moved
    }

    /// Delete `n` chars at the start of text `t`; the update.
    fn delete_update(doc: &Doc, n: u32) -> Vec<u8> {
        let t = doc.get_or_insert_text("t");
        let mut txn = doc.transact_mut();
        t.remove_range(&mut txn, 0, n);
        txn.encode_update_v1()
    }

    /// E2 brief 1: a foreign snapshot whose state is already here is not news even though the diff
    /// against our state vector is never empty once a deletion exists (the delete set rides along);
    /// applying it writes nothing — no log record, no snapshot, no row touch, no mirror write.
    #[tokio::test]
    async fn contained_snapshot_with_deletions_writes_nothing() {
        let db = tempfile::tempdir().unwrap();
        let b = Store::open(db.path()).await.unwrap();
        let m = meta(Binding::Knowledge("facts/del".into()));
        let doc = Doc::new();
        let u1 = text_update(&doc, "hello world");
        let u2 = delete_update(&doc, 6);
        b.create(&m).await.unwrap();
        b.append(&m.id, &Principal::File, &u1).unwrap();
        b.append(&m.id, &Principal::File, &u2).unwrap();
        // The crux, on the live `Doc` (the hub's open path): same state, non-empty diff, no news.
        let same = Doc::new();
        same.transact_mut()
            .apply_update(Update::decode_v1(&yrs::merge_updates_v1([&u1, &u2]).unwrap()).unwrap())
            .unwrap();
        let snap = doc
            .transact()
            .encode_state_as_update_v1(&StateVector::default());
        let diff = decode_state(&snap)
            .unwrap()
            .transact()
            .encode_state_as_update_v1(&same.transact().state_vector());
        assert!(
            diff.len() > 2,
            "the delete set makes the diff non-empty: {diff:?}"
        );
        assert!(!adds_anything(&snap, &same).unwrap());
        assert!(!exceeds(&same, &decode_state(&snap).unwrap()));
        // One more deletion elsewhere is news, and only that.
        let ahead = Doc::new();
        ahead
            .transact_mut()
            .apply_update(Update::decode_v1(&snap).unwrap())
            .unwrap();
        delete_update(&ahead, 1);
        assert!(exceeds(&ahead, &same) && !exceeds(&same, &ahead));

        // On disk: B snapshots its own copy; a peer's encoding of the same state lands (`merge_updates`
        // lays the blocks out differently from a full-state encode). Nothing changes on B.
        b.snapshot(&m.id, &doc, None).unwrap();
        b.touch_at_snapshot(&m.id, 77, 5).await.unwrap();
        let name = mirror::file_name(&m.id, MirrorFile::Snapshot);
        let foreign = yrs::merge_updates_v1([&u1, &u2]).unwrap();
        fs::write(b.mirror_dir().join(&name), &foreign).unwrap();
        let before = (
            fs::read(b.dir(&m.id).join(SNAPSHOT)).unwrap(),
            fs::read(b.dir(&m.id).join(LOG)).unwrap(),
            sqlx::query_as::<_, (i64, i64)>("SELECT updated_ms, bytes FROM docs WHERE id = ?")
                .bind(m.id.as_str())
                .fetch_one(&b.db)
                .await
                .unwrap(),
            rows(&b),
            b.last_projected(&m.id),
        );
        assert!(matches!(
            b.mirror_apply(&b.mirror_dir().join(&name)).await.unwrap(),
            Applied::Unchanged | Applied::Ignored
        ));
        let after = (
            fs::read(b.dir(&m.id).join(SNAPSHOT)).unwrap(),
            fs::read(b.dir(&m.id).join(LOG)).unwrap(),
            sqlx::query_as::<_, (i64, i64)>("SELECT updated_ms, bytes FROM docs WHERE id = ?")
                .bind(m.id.as_str())
                .fetch_one(&b.db)
                .await
                .unwrap(),
            rows(&b),
            b.last_projected(&m.id),
        );
        assert_eq!(
            before, after,
            "nothing written for a snapshot already contained"
        );
        assert_eq!((b.stats().merged, b.stats().resnapshots), (0, 0));
    }

    /// E2 brief 2: a receiver that merely catches up adopts the peer's bytes and ships nothing; only a
    /// receiver with state of its own re-snapshots, and ships exactly one row. FIXBATCH C: the adopt
    /// leaves the (replicated) `docs` row alone too — `bytes`/`updated_ms` arrive with the hub's row.
    #[tokio::test]
    async fn catching_up_ships_no_rows_and_a_real_merge_ships_one() {
        let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let a = Store::open(da.path()).await.unwrap();
        let b = Store::open(db.path()).await.unwrap();
        let m = meta(Binding::Knowledge("facts/ship".into()));
        a.create(&m).await.unwrap();
        let doc = Doc::new();
        a.append(&m.id, &Principal::File, &text_update(&doc, "one "))
            .unwrap();
        a.snapshot(&m.id, &doc, None).unwrap();
        assert_eq!(exchange(&a, &b).await, 2, "meta + snapshot arrive");
        assert_eq!(
            exchange(&b, &a).await,
            0,
            "B has nothing of its own to ship"
        );
        // (The creation counted as a merge when the snapshot half arrived after the meta; deltas from here.)
        let counts = |s: &Store| (s.stats().merged, s.stats().resnapshots, s.stats().unchanged);
        let (b0, a0) = (counts(&b), counts(&a));
        // The hub's `docs` row as the replica delivers it (stale `bytes`: the receiver used to fix it up).
        sqlx::query("INSERT INTO docs (id, kind, title, binding, created_ms, updated_ms, bytes) VALUES (?, 'knowledge', 't', ?, 1, 41, 3)")
            .bind(m.id.as_str())
            .bind(m.binding.key())
            .execute(&b.db)
            .await
            .unwrap();
        let docs_row = |s: &Store| {
            let (db, id) = (s.db.clone(), m.id.clone());
            async move {
                sqlx::query_as::<_, (i64, i64)>("SELECT updated_ms, bytes FROM docs WHERE id = ?")
                    .bind(id.as_str())
                    .fetch_one(&db)
                    .await
                    .unwrap()
            }
        };

        // A's next snapshot lands on B: B merges (Merged) but ships 0 rows — its snapshot is A's bytes.
        a.append(&m.id, &Principal::File, &text_update(&doc, "two "))
            .unwrap();
        a.snapshot(&m.id, &doc, None).unwrap();
        assert_eq!(exchange(&a, &b).await, 1);
        assert_eq!(exchange(&b, &a).await, 0, "B shipped 0 doc_files rows");
        assert_eq!(
            fs::read(b.dir(&m.id).join(SNAPSHOT)).unwrap(),
            fs::read(a.dir(&m.id).join(SNAPSHOT)).unwrap()
        );
        assert_eq!(
            b.last_projected(&m.id),
            a.last_projected(&m.id),
            "the projection is the peer's bytes"
        );
        let lb = b.load(&m.id).unwrap();
        assert_eq!(
            lb.doc
                .get_or_insert_text("t")
                .get_string(&lb.doc.transact()),
            "one two "
        );
        assert_eq!(lb.log_count, 0, "the log the snapshot covers is compacted");
        assert_eq!(
            counts(&b),
            (b0.0 + 1, b0.1, b0.2),
            "one merge, no re-snapshot"
        );
        assert_eq!(
            docs_row(&b).await,
            (41, 3),
            "the adopt wrote 0 docs-row updates on the receiver"
        );

        // B edits locally first: A's next snapshot is a real merge on B — re-snapshotted, shipped once.
        b.append(&m.id, &Principal::File, &text_update(&lb.doc, "B!"))
            .unwrap();
        a.append(&m.id, &Principal::File, &text_update(&doc, "three"))
            .unwrap();
        a.snapshot(&m.id, &doc, None).unwrap();
        assert_eq!(exchange(&a, &b).await, 1);
        assert_eq!(
            exchange(&b, &a).await,
            1,
            "B shipped exactly 1 row: the joined snapshot"
        );
        assert_eq!(
            counts(&b),
            (b0.0 + 2, b0.1 + 1, b0.2),
            "the real merge re-snapshotted once"
        );
        assert_eq!(
            counts(&a),
            (a0.0 + 1, a0.1, a0.2),
            "A catches up on B's edit without re-encoding"
        );
        let (la, lb) = (a.load(&m.id).unwrap(), b.load(&m.id).unwrap());
        let text = la
            .doc
            .get_or_insert_text("t")
            .get_string(&la.doc.transact());
        assert_eq!(
            text,
            lb.doc
                .get_or_insert_text("t")
                .get_string(&lb.doc.transact())
        );
        assert_eq!(
            text.len(),
            "one two threeB!".len(),
            "nothing duplicated: {text}"
        );
        assert_eq!(
            exchange(&a, &b).await + exchange(&b, &a).await,
            0,
            "converged: a further round moves no bytes"
        );
    }

    /// E2 brief 5: three engines exchanging files as the replica would. One edit on the hub → each
    /// replica merges once and re-snapshots never; the hub applies nothing; the next round is silent.
    #[tokio::test]
    async fn three_stores_converge_in_one_round_without_echo() {
        let dirs: Vec<_> = (0..3).map(|_| tempfile::tempdir().unwrap()).collect();
        let mut stores = Vec::new();
        for d in &dirs {
            stores.push(Store::open(d.path()).await.unwrap());
        }
        let (a, b, c) = (&stores[0], &stores[1], &stores[2]);
        let m = meta(Binding::Knowledge("facts/three".into()));
        a.create(&m).await.unwrap();
        let doc = Doc::new();
        a.append(&m.id, &Principal::File, &text_update(&doc, "hello "))
            .unwrap();
        a.append(&m.id, &Principal::File, &delete_update(&doc, 1))
            .unwrap();
        a.snapshot(&m.id, &doc, None).unwrap();
        // Every ordered pair, until a whole round moves nothing.
        async fn round(stores: &[Store]) -> usize {
            let mut moved = 0;
            for (i, from) in stores.iter().enumerate() {
                for (j, to) in stores.iter().enumerate() {
                    if i != j {
                        moved += exchange(from, to).await;
                    }
                }
            }
            moved
        }
        assert_eq!(round(&stores).await, 4, "meta + snapshot to B and to C");
        assert_eq!(round(&stores).await, 0);
        for s in [b, c] {
            assert!(s.dir(&m.id).join(SNAPSHOT).is_file());
        }
        let counts = |s: &Store| (s.stats().merged, s.stats().resnapshots);
        let before: Vec<_> = stores.iter().map(counts).collect();

        // One edit on A: B and C merge once, re-snapshot never; A applies nothing.
        a.append(&m.id, &Principal::File, &text_update(&doc, "world"))
            .unwrap();
        a.snapshot(&m.id, &doc, None).unwrap();
        assert_eq!(
            round(&stores).await,
            2,
            "one snapshot to B and to C, whoever carries it"
        );
        for (s, (merged, resnapshots)) in stores[1..].iter().zip(&before[1..]) {
            assert_eq!(counts(s), (merged + 1, *resnapshots), "{:?}", s.stats());
            let l = s.load(&m.id).unwrap();
            assert_eq!(
                l.doc.get_or_insert_text("t").get_string(&l.doc.transact()),
                "ello world"
            );
            assert_eq!(
                fs::read(s.dir(&m.id).join(SNAPSHOT)).unwrap(),
                fs::read(a.dir(&m.id).join(SNAPSHOT)).unwrap()
            );
        }
        assert_eq!(counts(a), before[0], "{:?}", a.stats());
        assert_eq!(round(&stores).await, 0, "a second round moves no bytes");
        assert_eq!(rows(a), rows(b));
        assert_eq!(rows(a), rows(c));
    }

    #[test]
    fn sha_and_atomic_write() {
        assert_eq!(
            sha256(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a/b.md");
        write_atomic(&p, b"x").unwrap();
        assert_eq!(read_opt(&p).unwrap().unwrap(), b"x");
        assert!(read_opt(&dir.path().join("nope")).unwrap().is_none());
    }

    /// 213: `shared` lists live documents with members or links (a revoked link still counts) via
    /// `links_n`, which `write_meta` keeps; an index from before the column is backfilled from
    /// `meta.json` on open, and a deleted document drops out.
    #[tokio::test]
    async fn links_n_lists_shared_documents_and_backfills() {
        let dir = tempfile::tempdir().unwrap();
        let old = Store::open(dir.path()).await.unwrap();
        let plain = meta(Binding::Note);
        let mut linked = meta(Binding::Note);
        old.create(&plain).await.unwrap();
        old.create(&linked).await.unwrap();
        linked.links.push(crate::meta::ShareLink {
            id: "lnk_a".into(),
            role: crate::meta::Role::Viewer,
            secret_hash_sealed: String::new(),
            name: None,
            created_ms: 1,
            expires_ms: None,
            revoked_ms: Some(2),
        });
        old.write_meta(&linked).await.unwrap();
        assert_eq!(old.shared().await.unwrap(), vec![linked.id.clone()]);
        // Drop the column as a pre-213 index had it, keeping the rows.
        let mut conn = old.pool().acquire().await.unwrap();
        for sql in [
            "CREATE TABLE docs_old AS SELECT id, kind, title, binding, created_ms, updated_ms, bytes, members_n, deleted_ms, hlc, project FROM docs",
            "DROP TABLE docs",
            "ALTER TABLE docs_old RENAME TO docs",
        ] {
            sqlx::query(sql).execute(&mut *conn).await.unwrap();
        }
        drop(conn);
        old.db.close().await;
        let store = Store::open(dir.path()).await.unwrap();
        assert_eq!(store.shared().await.unwrap(), vec![linked.id.clone()]);
        store.delete(&linked.id).await.unwrap();
        assert!(store.shared().await.unwrap().is_empty());
    }

    /// An index from before `docs.project`: opening adds the column, every row reads ungrouped, and
    /// `set_project` files and clears a live note (a trashed or unknown one answers `false`).
    #[tokio::test]
    async fn project_column_migrates_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let old = Store::open(dir.path()).await.unwrap();
        let note = meta(Binding::Note);
        old.create(&note).await.unwrap();
        // Rebuild the table as a pre-project index had it, keeping the row (one connection: the
        // pool's other one would race the schema change).
        let mut conn = old.pool().acquire().await.unwrap();
        for sql in [
            "CREATE TABLE docs_old AS SELECT id, kind, title, binding, created_ms, updated_ms, bytes, members_n, deleted_ms, hlc FROM docs",
            "DROP TABLE docs",
            "ALTER TABLE docs_old RENAME TO docs",
        ] {
            sqlx::query(sql).execute(&mut *conn).await.unwrap();
        }
        drop(conn);
        old.db.close().await;
        let store = Store::open(dir.path()).await.unwrap();
        let listed = store.list(Some(DocKind::Note), 10).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].project, None);
        assert!(store.set_project(&note.id, Some("alpha")).await.unwrap());
        assert!(
            store.set_project(&note.id, Some("alpha")).await.unwrap(),
            "unchanged is still found"
        );
        assert_eq!(
            store
                .summary(&note.id)
                .await
                .unwrap()
                .unwrap()
                .project
                .as_deref(),
            Some("alpha")
        );
        assert_eq!(
            store.list(None, 10).await.unwrap()[0].project.as_deref(),
            Some("alpha")
        );
        assert!(store.set_project(&note.id, None).await.unwrap());
        assert_eq!(
            store.summary(&note.id).await.unwrap().unwrap().project,
            None
        );
        assert!(!store.set_project(&DocId::new(), Some("x")).await.unwrap());
        store.delete(&note.id).await.unwrap();
        assert!(!store.set_project(&note.id, Some("x")).await.unwrap());
        // Reopening an index that has the column is a no-op.
        drop(store);
        Store::open(dir.path()).await.unwrap();
    }

    /// `docs.project` is a replicated column (`TABLES`): a project set on one engine reaches the
    /// other through the record replica, as `notebooks.project` does.
    #[tokio::test]
    async fn project_replicates_through_the_record_replica() {
        async fn side() -> (
            tempfile::TempDir,
            Store,
            Arc<replica::Engine>,
            replica::ReplicaHandle,
        ) {
            let dir = tempfile::tempdir().unwrap();
            let store = Store::open(dir.path()).await.unwrap();
            let engine = replica::Engine::open(&dir.path().join("replica.sqlite"))
                .await
                .unwrap();
            let handle = engine.attach(store.pool(), TABLES).await.unwrap();
            (dir, store, engine, handle)
        }
        async fn ship(from: &replica::ReplicaHandle, to: &replica::Engine) {
            while let Some(batch) = from.flush().await.unwrap() {
                to.apply(&batch).await.unwrap();
            }
        }
        let (_da, a, _ea, ha) = side().await;
        let (_db, b, eb, _hb) = side().await;
        let note = meta(Binding::Note);
        a.create(&note).await.unwrap();
        ship(&ha, &eb).await;
        assert_eq!(b.summary(&note.id).await.unwrap().unwrap().project, None);
        assert!(a.set_project(&note.id, Some("alpha")).await.unwrap());
        ship(&ha, &eb).await;
        assert_eq!(
            b.summary(&note.id)
                .await
                .unwrap()
                .unwrap()
                .project
                .as_deref(),
            Some("alpha")
        );
        assert!(a.set_project(&note.id, None).await.unwrap());
        ship(&ha, &eb).await;
        assert_eq!(b.summary(&note.id).await.unwrap().unwrap().project, None);
    }
}
