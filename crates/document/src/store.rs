//! `<data_dir>/documents/` (design 59 §4.1): `index.sqlite` plus one directory
//! per document with `meta.json`, `snapshot.yjs`, an append-only `updates.log`
//! and kept `snapshots/`. Load = snapshot + replay; the snapshot policy (§4.1)
//! is the actor's, this module only knows how to take one.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, bail};
use base64::Engine as _;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};
use yrs::updates::decoder::Decode;
use yrs::{Doc, ReadTxn, StateVector, Transact, Update};

use crate::meta::{Binding, DocId, DocKind, DocMeta, DocSummary, Principal, SnapshotInfo};

/// Directory under the data dir.
pub const DIR: &str = "documents";
const INDEX: &str = "index.sqlite";
const META: &str = "meta.json";
const SNAPSHOT: &str = "snapshot.yjs";
const LOG: &str = "updates.log";
const SNAPSHOTS: &str = "snapshots";
const TRASH: &str = ".trash";

const CREATE: &str = "CREATE TABLE IF NOT EXISTS docs (\
    id TEXT PRIMARY KEY, kind TEXT NOT NULL, title TEXT NOT NULL, binding TEXT NOT NULL, \
    created_ms INTEGER NOT NULL, updated_ms INTEGER NOT NULL, bytes INTEGER NOT NULL DEFAULT 0, \
    members_n INTEGER NOT NULL DEFAULT 0, deleted_ms INTEGER)";
const CREATE_BINDING: &str = "CREATE INDEX IF NOT EXISTS docs_binding ON docs(binding)";

/// Unix milliseconds now.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}

/// The store handle: paths + the index pool.
#[derive(Clone)]
pub struct Store {
    root: PathBuf,
    db: SqlitePool,
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
    /// Create `documents/`, open the index.
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
        Ok(Self { root, db })
    }

    /// `documents/`.
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn dir(&self, id: &DocId) -> PathBuf {
        self.root.join(id.as_str())
    }

    /// Directory, meta, empty log, index row. Fails if the binding is already bound.
    pub async fn create(&self, meta: &DocMeta) -> anyhow::Result<()> {
        if meta.binding != Binding::Note
            && let Some(existing) = self.lookup(&meta.binding).await?
        {
            bail!("{:?} is already document {existing}", meta.binding);
        }
        let dir = self.dir(&meta.id);
        fs::create_dir_all(dir.join(SNAPSHOTS))?;
        write_atomic(&dir.join(META), serde_json::to_vec_pretty(meta)?.as_slice())?;
        File::create(dir.join(LOG))?;
        sqlx::query(
            "INSERT INTO docs (id, kind, title, binding, created_ms, updated_ms, bytes) VALUES (?, ?, ?, ?, ?, ?, 0)",
        )
        .bind(meta.id.as_str())
        .bind(meta.kind.to_string())
        .bind(&meta.title)
        .bind(meta.binding.key())
        .bind(meta.created_ms)
        .bind(meta.updated_ms)
        .execute(&self.db)
        .await?;
        Ok(())
    }

    /// `meta.json`.
    pub fn read_meta(&self, id: &DocId) -> anyhow::Result<DocMeta> {
        let p = self.dir(id).join(META);
        let bytes = fs::read(&p).with_context(|| format!("read {}", p.display()))?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    /// Rewrite `meta.json` and the index columns it feeds.
    pub async fn write_meta(&self, meta: &DocMeta) -> anyhow::Result<()> {
        write_atomic(
            &self.dir(&meta.id).join(META),
            serde_json::to_vec_pretty(meta)?.as_slice(),
        )?;
        sqlx::query("UPDATE docs SET title = ?, updated_ms = ?, members_n = ? WHERE id = ?")
            .bind(&meta.title)
            .bind(meta.updated_ms)
            .bind(meta.members.len() as i64)
            .bind(meta.id.as_str())
            .execute(&self.db)
            .await?;
        Ok(())
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

    /// Full state → `snapshot.yjs` (tmp + rename), truncate the log; with a
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

    /// Index bookkeeping after an accepted update.
    pub async fn touch(&self, id: &DocId, updated_ms: i64, bytes: usize) -> anyhow::Result<()> {
        sqlx::query("UPDATE docs SET updated_ms = ?, bytes = ? WHERE id = ?")
            .bind(updated_ms)
            .bind(bytes as i64)
            .bind(id.as_str())
            .execute(&self.db)
            .await?;
        Ok(())
    }

    /// The live document bound to `binding`. A row whose directory is gone
    /// (deleted by hand) is marked deleted so the binding can be recreated.
    pub async fn lookup(&self, binding: &Binding) -> anyhow::Result<Option<DocId>> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT id FROM docs WHERE binding = ? AND deleted_ms IS NULL LIMIT 1")
                .bind(binding.key())
                .fetch_optional(&self.db)
                .await?;
        let Some((id,)) = row else { return Ok(None) };
        let id = DocId(id);
        if !self.dir(&id).join(META).is_file() {
            tracing::warn!(doc = %id, "document directory missing; row marked deleted");
            self.delete(&id).await?;
            return Ok(None);
        }
        Ok(Some(id))
    }

    /// Live documents, newest update first.
    pub async fn list(&self, kind: Option<DocKind>, limit: u32) -> anyhow::Result<Vec<DocSummary>> {
        let rows: Vec<(String, String, String, String, i64, i64, i64)> = match kind {
            Some(k) => {
                sqlx::query_as(
                    "SELECT id, kind, title, binding, created_ms, updated_ms, bytes FROM docs WHERE deleted_ms IS NULL AND kind = ? ORDER BY updated_ms DESC LIMIT ?",
                )
                .bind(k.to_string())
                .bind(limit)
                .fetch_all(&self.db)
                .await?
            }
            None => {
                sqlx::query_as(
                    "SELECT id, kind, title, binding, created_ms, updated_ms, bytes FROM docs WHERE deleted_ms IS NULL ORDER BY updated_ms DESC LIMIT ?",
                )
                .bind(limit)
                .fetch_all(&self.db)
                .await?
            }
        };
        Ok(rows
            .into_iter()
            .filter_map(|(id, kind, title, binding, created_ms, updated_ms, bytes)| {
                Some(DocSummary {
                    id: DocId(id),
                    kind: serde_json::from_value(serde_json::Value::String(kind)).ok()?,
                    title,
                    binding: serde_json::from_str(&binding).ok()?,
                    created_ms,
                    updated_ms,
                    bytes,
                })
            })
            .collect())
    }

    /// Move the directory to `.trash/<id>` and mark the row deleted (§7.1).
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
        Ok(())
    }

    /// Remove `.trash/<id>` directories deleted more than `max_age` ago, and their rows (§7.3). Returns how many.
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
                Err(e) => {
                    tracing::warn!(doc = %id, error = %e, "trash sweep: directory kept");
                    continue;
                }
            }
            sqlx::query("DELETE FROM docs WHERE id = ?")
                .bind(&id)
                .execute(&self.db)
                .await?;
        }
        Ok(n)
    }
}

/// Labels the store may prune (§4.6); everything else was asked for by name.
const AUTO_LABELS: &[&str] = &["import-file", "daily"];

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

/// `tmp` + rename; parents created.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!(
        "{}.tmp",
        path.extension().and_then(|e| e.to_str()).unwrap_or("")
    ));
    {
        let mut f = File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_data()?;
    }
    fs::rename(&tmp, path)
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
    use sha2::Digest;
    let d = sha2::Sha256::digest(bytes);
    let mut s = String::with_capacity(64);
    for b in d {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
    }
    s
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
            created_ms: 1,
            updated_ms: 1,
        }
    }

    fn text_update(doc: &Doc, s: &str) -> Vec<u8> {
        let t = doc.get_or_insert_text("t");
        let mut txn = doc.transact_mut();
        let len = t.len(&txn);
        t.insert(&mut txn, len, s);
        txn.encode_update_v1()
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
        for i in 0..4 {
            std::thread::sleep(Duration::from_millis(2));
            store
                .snapshot(&m.id, &doc, Some(if i == 1 { "manual" } else { "daily" }))
                .unwrap();
        }
        store.prune_snapshots(&m.id, 2).unwrap();
        let labels: Vec<String> = store
            .snapshots(&m.id)
            .unwrap()
            .into_iter()
            .map(|s| s.label)
            .collect();
        assert_eq!(labels, vec!["daily", "daily", "manual"]);
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
        assert_eq!(store.sweep_trash(Duration::from_millis(50000)).await.unwrap(), 1);
        assert!(!trash.join(old.id.as_str()).exists());
        assert!(trash.join(young.id.as_str()).is_dir());
        let rows: Vec<(String,)> = sqlx::query_as("SELECT id FROM docs")
            .fetch_all(&store.db)
            .await
            .unwrap();
        assert_eq!(rows, vec![(young.id.0.clone(),)]);
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
}
