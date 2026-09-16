//! The store: `<data_dir>/artifacts/` blobs by content hash plus the
//! `artifacts.sqlite` index (35 §3.3–§3.4). Truth is the index; blobs
//! without a row are swept at start.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Context as _;
use kameo::actor::ActorRef;
use runtime::{Runtime, SessionId};
use sha2::{Digest, Sha256};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};

use crate::kind::{self, Receipt};
use crate::pdf::{self, Source};
use crate::tools::ShowArgs;
use crate::{Artifact, ArtifactShown, Kind, MAX_BYTES, svg};

/// Directory under `data_dir`.
pub const DIR: &str = "artifacts";
/// Index file name inside [`DIR`].
pub const DB: &str = "artifacts.sqlite";

/// What one `show` produced: the record plus the numbers its receipt needs.
#[derive(Clone, Debug)]
pub struct Shown {
    /// The stored record.
    pub artifact: Artifact,
    /// Receipt extras (lines, table dims, picture dims, warnings).
    pub receipt: Receipt,
}

/// Blob directory + index + the runtime that publishes [`ArtifactShown`].
pub struct State {
    /// `<data_dir>/artifacts/`.
    pub dir: PathBuf,
    pool: SqlitePool,
    rt: Runtime,
    /// Extra font directories for PDF compiles (`artifact.fonts`).
    pub fonts: Vec<PathBuf>,
}

const COLS: &str = "call_id, id, session, name, title, kind, mime, bytes, version, \
                    source, preview, pages, origin, created_at";

type Row = (
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    i64,
    i64,
    Option<String>,
    Option<String>,
    Option<i64>,
    Option<String>,
    i64,
);

fn sha16(bytes: &[u8]) -> String {
    let mut hex = format!("{:x}", Sha256::digest(bytes));
    hex.truncate(16);
    hex
}

fn blob_name(id: &str, name: &str) -> String {
    match kind::ext(name) {
        Some(e) => format!("{id}.{e}"),
        None => id.to_owned(),
    }
}

fn first_heading(text: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let l = l.trim_start();
        let rest = l.trim_start_matches('#');
        let hashes = l.len() - rest.len();
        ((1..=6).contains(&hashes) && rest.starts_with(' '))
            .then(|| rest.trim())
            .filter(|t| !t.is_empty())
            .map(str::to_owned)
    })
}

fn count_lines(bytes: &[u8]) -> usize {
    bytes.iter().filter(|b| **b == b'\n').count()
        + usize::from(!bytes.is_empty() && bytes.last() != Some(&b'\n'))
}

impl State {
    /// Create `<data_dir>/artifacts/`, open the index, run migrations.
    pub async fn open(data_dir: &Path, rt: Runtime, fonts: Vec<PathBuf>) -> anyhow::Result<Self> {
        let dir = data_dir.join(DIR);
        std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        let db = dir.join(DB);
        let opts = SqliteConnectOptions::new()
            .filename(&db)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(opts)
            .await
            .with_context(|| format!("open {}", db.display()))?;
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .context("run migrations")?;
        Ok(Self {
            dir,
            pool,
            rt,
            fonts,
        })
    }

    fn row_to_artifact(&self, r: Row) -> anyhow::Result<Artifact> {
        let (
            call_id,
            id,
            session,
            name,
            title,
            kind,
            mime,
            bytes,
            version,
            source,
            preview,
            pages,
            origin,
            created_at,
        ) = r;
        Ok(Artifact {
            path: self.dir.join(blob_name(&id, &name)),
            id,
            session: session
                .parse()
                .with_context(|| format!("artifacts.session {session:?}"))?,
            call_id,
            kind: Kind::parse(&kind).with_context(|| format!("artifacts.kind {kind:?}"))?,
            name,
            title,
            mime,
            bytes: u64::try_from(bytes).unwrap_or_default(),
            version: u32::try_from(version).unwrap_or_default(),
            source: source.map(|s| self.dir.join(s)),
            preview: preview.map(|p| self.dir.join(p)),
            pages: pages.and_then(|p| u32::try_from(p).ok()),
            origin: origin.map(PathBuf::from),
            created_at,
        })
    }

    /// Write `<dir>/<file>` unless it already exists (same hash = same bytes).
    async fn put_blob(&self, file: &str, bytes: &[u8]) -> anyhow::Result<PathBuf> {
        let path = self.dir.join(file);
        if tokio::fs::try_exists(&path).await.unwrap_or(false) {
            return Ok(path);
        }
        let part = self.dir.join(format!("{file}.part"));
        tokio::fs::write(&part, bytes)
            .await
            .with_context(|| format!("write {}", part.display()))?;
        tokio::fs::rename(&part, &path)
            .await
            .with_context(|| format!("rename {}", path.display()))?;
        Ok(path)
    }

    /// 35 §3.3 `Show`: validate → kind → compile/rasterise → blob → version →
    /// insert → publish. `Err` is what the model reads.
    pub async fn show(
        &self,
        session: SessionId,
        call_id: String,
        args: ShowArgs,
    ) -> Result<Shown, String> {
        let t0 = Instant::now();
        let (name, bytes, origin, content) = match (args.path, args.content) {
            (Some(_), Some(_)) => return Err("pass either path or content, not both".into()),
            (None, None) => return Err("pass path (an existing file) or content + name".into()),
            (None, Some(content)) => {
                let name = args.name.ok_or("content requires name")?;
                if kind::ext(&name).is_none() {
                    return Err(format!("{name}: name needs an extension"));
                }
                (name, content.clone().into_bytes(), None, Some(content))
            }
            (Some(path), None) => {
                let meta = tokio::fs::metadata(&path)
                    .await
                    .map_err(|e| format!("{}: {e}", path.display()))?;
                if !meta.is_file() {
                    return Err(format!("{}: not a regular file", path.display()));
                }
                if meta.len() > MAX_BYTES {
                    return Err(format!(
                        "{}: {} exceeds {}",
                        path.display(),
                        kind::human(meta.len()),
                        kind::human(MAX_BYTES)
                    ));
                }
                let name = args
                    .name
                    .or_else(|| path.file_name().map(|n| n.to_string_lossy().into_owned()))
                    .ok_or_else(|| format!("{}: no file name", path.display()))?;
                let bytes = tokio::fs::read(&path)
                    .await
                    .map_err(|e| format!("{}: {e}", path.display()))?;
                (name, bytes, Some(path), None)
            }
        };
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_BYTES {
            return Err(format!(
                "{name}: {} exceeds {}",
                kind::human(bytes.len() as u64),
                kind::human(MAX_BYTES)
            ));
        }

        let kind = kind::of(&name);
        let mut receipt = Receipt::default();
        let mut source_file = None;
        let mut pages = None;
        let mut preview_png = None;
        let mut heading_src: Option<&str> = None;
        let mut compiled = false;

        let bytes = match (kind, &content) {
            (Kind::Pdf, Some(content)) => {
                let source = args.source.unwrap_or(Source::Markdown);
                let src_ext = if matches!(source, Source::Typst) {
                    "typ"
                } else {
                    "md"
                };
                if src_ext == "md" {
                    heading_src = Some(content.as_str());
                }
                let src_name = format!("{}.{src_ext}", sha16(content.as_bytes()));
                self.put_blob(&src_name, content.as_bytes())
                    .await
                    .map_err(|e| format!("{name}: {e:#}"))?;
                source_file = Some(src_name);
                let out = pdf::compile(content, source, &self.fonts)
                    .await
                    .map_err(|e| format!("{name}: {e:#}"))?
                    .map_err(|d| format!("{name}: compile failed\n{}", pdf::format_diags(&d)))?;
                compiled = true;
                pages = Some(out.pages);
                receipt.warnings = out.warnings;
                preview_png = Some(out.preview_png);
                out.pdf
            }
            (Kind::Svg, _) => {
                let r = svg::rasterize(&bytes).map_err(|e| format!("{name}: {e:#}"))?;
                receipt.dims = Some((r.width, r.height));
                preview_png = Some(r.png);
                bytes
            }
            (Kind::Markdown | Kind::Text | Kind::Html, _) => {
                receipt.lines = Some(count_lines(&bytes));
                bytes
            }
            (Kind::Csv, _) => {
                let tsv = kind::ext(&name).as_deref() == Some("tsv");
                if let Ok((h, _, rows)) = kind::csv_table(&bytes, tsv, 0) {
                    receipt.table = Some((rows, h.len()));
                }
                bytes
            }
            _ => bytes,
        };
        let text_for_title = match kind {
            Kind::Markdown => std::str::from_utf8(&bytes).ok(),
            _ => heading_src,
        };
        let title = args
            .title
            .or_else(|| text_for_title.and_then(first_heading))
            .unwrap_or_else(|| name.clone());

        let id = sha16(&bytes);
        let path = self
            .put_blob(&blob_name(&id, &name), &bytes)
            .await
            .map_err(|e| format!("{name}: {e:#}"))?;
        let preview_file = match preview_png {
            Some(png) => {
                let f = format!("{id}.preview.png");
                self.put_blob(&f, &png)
                    .await
                    .map_err(|e| format!("{name}: {e:#}"))?;
                Some(f)
            }
            None => None,
        };

        let (count,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM artifacts WHERE session = ? AND name = ?")
                .bind(session.to_string())
                .bind(&name)
                .fetch_one(&self.pool)
                .await
                .map_err(|e| format!("{name}: index: {e}"))?;
        let artifact = Artifact {
            mime: kind::mime(&name),
            id,
            session,
            call_id,
            name,
            title,
            kind,
            bytes: bytes.len() as u64,
            version: u32::try_from(count).unwrap_or_default() + 1,
            path,
            source: source_file.as_deref().map(|s| self.dir.join(s)),
            preview: preview_file.as_deref().map(|p| self.dir.join(p)),
            pages,
            origin,
            created_at: chrono::Utc::now().timestamp(),
        };
        // Only `COLS` (a const) is interpolated; every value is bound.
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "INSERT OR REPLACE INTO artifacts ({COLS}) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?)"
        )))
        .bind(&artifact.call_id)
        .bind(&artifact.id)
        .bind(artifact.session.to_string())
        .bind(&artifact.name)
        .bind(&artifact.title)
        .bind(artifact.kind.as_str())
        .bind(&artifact.mime)
        .bind(i64::try_from(artifact.bytes).unwrap_or(i64::MAX))
        .bind(i64::from(artifact.version))
        .bind(source_file)
        .bind(preview_file)
        .bind(artifact.pages.map(i64::from))
        .bind(
            artifact
                .origin
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
        )
        .bind(artifact.created_at)
        .execute(&self.pool)
        .await
        .map_err(|e| format!("{}: index: {e}", artifact.name))?;

        tracing::info!(
            target: "artifact",
            kind = artifact.kind.as_str(),
            bytes = artifact.bytes,
            ms = t0.elapsed().as_millis() as u64,
            compiled,
            pages = artifact.pages,
            ok = true,
            "show"
        );
        self.rt
            .publish(ArtifactShown {
                session,
                call_id: artifact.call_id.clone(),
                artifact: artifact.clone(),
            })
            .await;
        Ok(Shown { artifact, receipt })
    }

    async fn select(&self, sessions: &[SessionId]) -> anyhow::Result<Vec<Artifact>> {
        if sessions.is_empty() {
            return Ok(Vec::new());
        }
        // Only `COLS` (a const) and a `?` list sized to `sessions` are interpolated; ids are bound.
        let sql = format!(
            "SELECT {COLS} FROM artifacts WHERE session IN ({}) \
             ORDER BY created_at DESC, rowid DESC",
            vec!["?"; sessions.len()].join(",")
        );
        let mut q = sqlx::query_as::<_, Row>(sqlx::AssertSqlSafe(sql));
        for s in sessions {
            q = q.bind(s.to_string());
        }
        q.fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(|r| self.row_to_artifact(r))
            .collect()
    }

    /// The session's artifacts, newest first. `descendants` adds every
    /// session under it, asked from the agent supervisor
    /// ([`agent::ListAgents`]); when the agent is absent only the session's
    /// own rows are returned.
    pub async fn list(
        &self,
        session: SessionId,
        descendants: bool,
    ) -> anyhow::Result<Vec<Artifact>> {
        let mut sessions = vec![session];
        if descendants
            && let Ok(Some(agents)) = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR)
            && let Ok(tree) = agents.ask(agent::ListAgents { session }).await
        {
            sessions.extend(tree.into_iter().map(|a| a.id));
        }
        self.select(&sessions).await
    }

    /// Copy the blob of `id` to `dest` (parent created). `Err` when `dest`
    /// exists or `id` is unknown.
    pub async fn export(&self, id: &str, dest: &Path) -> Result<PathBuf, String> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT name FROM artifacts WHERE id = ? LIMIT 1")
                .bind(id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| format!("index: {e}"))?;
        let (name,) = row.ok_or_else(|| format!("{id}: no such artifact"))?;
        if tokio::fs::try_exists(dest).await.unwrap_or(false) {
            return Err(format!("{}: exists", dest.display()));
        }
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        tokio::fs::copy(self.dir.join(blob_name(id, &name)), dest)
            .await
            .map_err(|e| format!("{}: {e}", dest.display()))?;
        Ok(dest.to_path_buf())
    }

    /// Drop every row of `session`; the next sweep collects the blobs.
    pub async fn delete_session(&self, session: SessionId) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM artifacts WHERE session = ?")
            .bind(session.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Delete blobs (and previews, sources) no row references. Returns how many.
    pub async fn sweep(&self) -> anyhow::Result<usize> {
        let ids: Vec<(String,)> = sqlx::query_as("SELECT DISTINCT id FROM artifacts")
            .fetch_all(&self.pool)
            .await?;
        let sources: Vec<(String,)> =
            sqlx::query_as("SELECT DISTINCT source FROM artifacts WHERE source IS NOT NULL")
                .fetch_all(&self.pool)
                .await?;
        let live: HashSet<String> = ids
            .into_iter()
            .map(|(id,)| id)
            .chain(sources.into_iter().map(|(s,)| stem(&s).to_owned()))
            .collect();
        let mut removed = 0;
        let mut rd = tokio::fs::read_dir(&self.dir).await?;
        while let Some(entry) = rd.next_entry().await? {
            let file = entry.file_name();
            let file = file.to_string_lossy();
            if file.starts_with(DB) || !entry.file_type().await?.is_file() {
                continue;
            }
            if !live.contains(stem(&file)) {
                tokio::fs::remove_file(entry.path()).await?;
                removed += 1;
            }
        }
        Ok(removed)
    }
}

fn stem(file: &str) -> &str {
    file.split('.').next().unwrap_or(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn open() -> (State, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let rt = Runtime::new(figment::Figment::new(), dir.path().to_path_buf());
        let state = State::open(dir.path(), rt, Vec::new()).await.unwrap();
        (state, dir)
    }

    fn content(name: &str, text: &str) -> ShowArgs {
        ShowArgs {
            path: None,
            content: Some(text.to_owned()),
            name: Some(name.to_owned()),
            title: None,
            source: None,
        }
    }

    fn by_path(path: PathBuf) -> ShowArgs {
        ShowArgs {
            path: Some(path),
            content: None,
            name: None,
            title: None,
            source: None,
        }
    }

    fn blobs(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| !n.starts_with(DB))
            .collect();
        v.sort();
        v
    }

    #[tokio::test]
    async fn same_bytes_twice_one_blob_two_versions() {
        let (s, _d) = open().await;
        let sess = SessionId::new();
        let a = s
            .show(sess, "c1".into(), content("notes.md", "# Hello\n\nbody\n"))
            .await
            .unwrap();
        let b = s
            .show(sess, "c2".into(), content("notes.md", "# Hello\n\nbody\n"))
            .await
            .unwrap();
        assert_eq!(a.artifact.id, b.artifact.id);
        assert_eq!((a.artifact.version, b.artifact.version), (1, 2));
        assert_eq!(a.artifact.title, "Hello");
        assert_eq!(a.artifact.kind, Kind::Markdown);
        assert_eq!(a.receipt.lines, Some(3));
        assert_eq!(blobs(&s.dir), [format!("{}.md", a.artifact.id)]);
        let list = s.list(sess, false).await.unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].call_id, "c2");
        assert_eq!(list[0].path, a.artifact.path);
    }

    #[tokio::test]
    async fn validation_errors() {
        let (s, d) = open().await;
        let sess = SessionId::new();
        let err = s
            .show(sess, "c".into(), by_path(d.path().to_path_buf()))
            .await
            .unwrap_err();
        assert!(err.ends_with("not a regular file"), "{err}");
        let err = s
            .show(sess, "c".into(), by_path(d.path().join("nope.txt")))
            .await
            .unwrap_err();
        assert!(
            err.starts_with(&format!("{}: ", d.path().join("nope.txt").display())),
            "{err}"
        );

        let big = d.path().join("big.mov");
        std::fs::File::create(&big)
            .unwrap()
            .set_len(33 * 1024 * 1024)
            .unwrap();
        let err = s
            .show(sess, "c".into(), by_path(big.clone()))
            .await
            .unwrap_err();
        assert_eq!(err, format!("{}: 33 MB exceeds 32 MB", big.display()));

        let mut args = content("x.txt", "hi");
        args.name = None;
        assert_eq!(
            s.show(sess, "c".into(), args).await.unwrap_err(),
            "content requires name"
        );
        assert!(s.list(sess, false).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn sweep_and_delete_session() {
        let (s, d) = open().await;
        let sess = SessionId::new();
        let kept = s
            .show(sess, "c1".into(), content("data.csv", "a,b\n1,2\n3,4\n"))
            .await
            .unwrap();
        assert_eq!(kept.receipt.table, Some((2, 2)));
        std::fs::write(s.dir.join("deadbeefdeadbeef.txt"), b"orphan").unwrap();
        assert_eq!(s.sweep().await.unwrap(), 1);
        assert_eq!(blobs(&s.dir), [format!("{}.csv", kept.artifact.id)]);

        let file = d.path().join("shot.png");
        std::fs::write(&file, b"not really png").unwrap();
        let shot = s
            .show(sess, "c2".into(), by_path(file.clone()))
            .await
            .unwrap();
        assert_eq!(shot.artifact.origin.as_deref(), Some(file.as_path()));
        assert_eq!(shot.artifact.kind, Kind::Image);

        let out = d.path().join("out").join("data.csv");
        assert_eq!(s.export(&kept.artifact.id, &out).await.unwrap(), out);
        assert_eq!(std::fs::read(&out).unwrap(), b"a,b\n1,2\n3,4\n");
        assert_eq!(
            s.export(&kept.artifact.id, &out).await.unwrap_err(),
            format!("{}: exists", out.display())
        );

        s.delete_session(sess).await.unwrap();
        assert!(s.list(sess, false).await.unwrap().is_empty());
        assert_eq!(s.sweep().await.unwrap(), 2);
        assert!(blobs(&s.dir).is_empty());
    }
}
