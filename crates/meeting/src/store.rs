//! Data on disk (01 §5): `<data_dir>/meetings/<id>/manifest.json` is truth,
//! rewritten atomically on every state change; `<data_dir>/meetings/meetings.sqlite`
//! is a derived index, rebuilt from the manifests when missing.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context as _;
use runtime::SessionId;
use serde::{Deserialize, Serialize};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};

use crate::{DIR, Degradation, MeetingId, MeetingState, ProcessInfo};

/// Index file name, inside [`DIR`].
pub const DB: &str = "meetings.sqlite";
/// Manifest file name, inside a meeting's directory.
pub const MANIFEST: &str = "manifest.json";
/// Sample rate of the three tracks.
pub const TRACK_RATE: u32 = 24_000;

/// A pause: `dur_ms` of wall time missing from the recording clock at `at_ms`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Gap {
    /// Recording-clock position of the pause.
    pub at_ms: u64,
    /// How long it lasted.
    pub dur_ms: u64,
}

/// Which file a [`TrackFile`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum Stem {
    /// `mic.opus`.
    Mic,
    /// `system.opus`.
    System,
    /// `mix.opus`: mic + system through a soft limiter.
    Mix,
}

impl Stem {
    /// File name in the meeting directory.
    pub fn file(self) -> &'static str {
        match self {
            Stem::Mic => "mic.opus",
            Stem::System => "system.opus",
            Stem::Mix => "mix.opus",
        }
    }
}

/// One Ogg Opus file of the meeting.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct TrackFile {
    /// Which.
    pub track: Stem,
    /// File name relative to the meeting directory.
    pub file: String,
    /// 24 000.
    pub sample_rate: u32,
}

/// `manifest.json` (01 §5). Every later-phase field is `#[serde(default)]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Manifest {
    /// Directory name.
    pub id: MeetingId,
    /// The meeting's agent session.
    pub session: SessionId,
    /// User-given title.
    pub title: Option<String>,
    /// User-given context (phase 2 sends it to the session).
    pub context: Option<String>,
    /// RFC 3339.
    pub started_at: String,
    /// RFC 3339.
    pub ended_at: Option<String>,
    /// Recording clock at stop.
    pub duration_ms: Option<u64>,
    /// The tapped process.
    pub process: ProcessInfo,
    /// When the consent box was ticked (53 §0.14); RFC 3339.
    pub consent_at: String,
    /// Pauses.
    #[serde(default)]
    pub gaps: Vec<Gap>,
    /// Union over the meeting.
    #[serde(default)]
    pub degraded: Vec<Degradation>,
    /// Last state.
    pub state: MeetingState,
    /// Why `state` is `Failed`.
    #[serde(default)]
    pub failure: Option<String>,
    /// Model for the session (phase 2).
    #[serde(default)]
    pub session_model: Option<String>,
    /// The three files.
    #[serde(default)]
    pub tracks: Vec<TrackFile>,
}

impl Manifest {
    /// A `Starting` manifest; `session` is filled once the session exists.
    pub fn new(id: MeetingId, process: ProcessInfo, now: String) -> Self {
        Self {
            id,
            session: SessionId(uuid::Uuid::nil()),
            title: None,
            context: None,
            started_at: now.clone(),
            ended_at: None,
            duration_ms: None,
            process,
            consent_at: now,
            gaps: Vec::new(),
            degraded: Vec::new(),
            state: MeetingState::Starting,
            failure: None,
            session_model: None,
            tracks: [Stem::Mic, Stem::System, Stem::Mix]
                .into_iter()
                .map(|t| TrackFile {
                    track: t,
                    file: t.file().to_owned(),
                    sample_rate: TRACK_RATE,
                })
                .collect(),
        }
    }

    fn summary(&self) -> MeetingSummary {
        MeetingSummary {
            id: self.id,
            started_at: self.started_at.clone(),
            ended_at: self.ended_at.clone(),
            title: self.title.clone(),
            session: self.session,
            process_name: self.process.name.clone(),
            duration_ms: self.duration_ms,
            state: self.state,
        }
    }
}

/// One index row (01 §5).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct MeetingSummary {
    /// Id.
    pub id: MeetingId,
    /// RFC 3339.
    pub started_at: String,
    /// RFC 3339.
    pub ended_at: Option<String>,
    /// Title.
    pub title: Option<String>,
    /// Session.
    pub session: SessionId,
    /// Process name at start.
    pub process_name: String,
    /// Recording clock at stop.
    pub duration_ms: Option<u64>,
    /// Last state the manifest said.
    pub state: MeetingState,
}

type Row = (
    String,
    String,
    Option<String>,
    Option<String>,
    String,
    String,
    Option<i64>,
    String,
);

fn row_to_summary(r: Row) -> anyhow::Result<MeetingSummary> {
    Ok(MeetingSummary {
        id: r.0.parse()?,
        started_at: r.1,
        ended_at: r.2,
        title: r.3,
        session: r.4.parse()?,
        process_name: r.5,
        duration_ms: r.6.map(|d| d as u64),
        state: serde_json::from_str(&r.7)?,
    })
}

/// Directory + index.
pub struct Store {
    dir: PathBuf,
    pool: SqlitePool,
}

impl Store {
    /// Create `<data_dir>/meetings/`, open the index (migrating), and rebuild
    /// it from the manifests when the file did not exist.
    pub async fn open(data_dir: &Path) -> anyhow::Result<Self> {
        let dir = data_dir.join(DIR);
        std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        let db = dir.join(DB);
        let fresh = !db.exists();
        let opts = SqliteConnectOptions::new()
            .filename(&db)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(2)
            .connect_with(opts)
            .await
            .with_context(|| format!("open {}", db.display()))?;
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .context("run migrations")?;
        let store = Self { dir, pool };
        if fresh {
            store.rebuild().await?;
        }
        Ok(store)
    }

    /// `<data_dir>/meetings/<id>/`.
    pub fn meeting_dir(&self, id: MeetingId) -> PathBuf {
        self.dir.join(id.to_string())
    }

    /// Directory + first manifest + index row. Returns the directory.
    pub async fn create(&self, m: &Manifest) -> anyhow::Result<PathBuf> {
        let dir = self.meeting_dir(m.id);
        std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        self.save(m).await?;
        Ok(dir)
    }

    /// Rewrite the manifest (`.tmp` + rename, 01 §0.5) and upsert its row.
    pub async fn save(&self, m: &Manifest) -> anyhow::Result<()> {
        let dir = self.meeting_dir(m.id);
        let tmp = dir.join(".manifest.json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(m)?)
            .with_context(|| format!("write {}", tmp.display()))?;
        std::fs::rename(&tmp, dir.join(MANIFEST)).context("rename manifest")?;
        self.upsert(&m.summary()).await
    }

    async fn upsert(&self, s: &MeetingSummary) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO meetings (id, started_at, ended_at, title, session, process_name, duration_ms, state) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(id) DO UPDATE SET ended_at = excluded.ended_at, title = excluded.title, \
             duration_ms = excluded.duration_ms, state = excluded.state",
        )
        .bind(s.id.to_string())
        .bind(&s.started_at)
        .bind(&s.ended_at)
        .bind(&s.title)
        .bind(s.session.to_string())
        .bind(&s.process_name)
        .bind(s.duration_ms.map(|d| d as i64))
        .bind(serde_json::to_string(&s.state)?)
        .execute(&self.pool)
        .await
        .context("upsert meeting")?;
        Ok(())
    }

    /// The manifest as on disk.
    pub fn manifest(&self, id: MeetingId) -> anyhow::Result<Manifest> {
        let path = self.meeting_dir(id).join(MANIFEST);
        let bytes = std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))
    }

    /// Newest first.
    pub async fn list(&self, limit: u32) -> anyhow::Result<Vec<MeetingSummary>> {
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT id, started_at, ended_at, title, session, process_name, duration_ms, state \
             FROM meetings ORDER BY started_at DESC LIMIT ?",
        )
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .context("list meetings")?;
        rows.into_iter().map(row_to_summary).collect()
    }

    /// Directory, then row.
    pub async fn delete(&self, id: MeetingId) -> anyhow::Result<()> {
        let dir = self.meeting_dir(id);
        if dir.exists() {
            std::fs::remove_dir_all(&dir).with_context(|| format!("remove {}", dir.display()))?;
        }
        sqlx::query("DELETE FROM meetings WHERE id = ?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await
            .context("delete meeting row")?;
        Ok(())
    }

    /// Scan `<dir>/*/manifest.json` into the index (01 §0.4).
    async fn rebuild(&self) -> anyhow::Result<()> {
        let entries =
            std::fs::read_dir(&self.dir).with_context(|| format!("scan {}", self.dir.display()))?;
        for entry in entries.flatten() {
            let Ok(id) = entry.file_name().to_string_lossy().parse::<MeetingId>() else {
                continue;
            };
            match self.manifest(id) {
                Ok(m) => self.upsert(&m.summary()).await?,
                Err(e) => tracing::warn!(meeting = %id, "skipping: {e:#}"),
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> Manifest {
        let mut m = Manifest::new(
            MeetingId::new(),
            ProcessInfo {
                pid: 42,
                name: "Music".into(),
                bundle_id: Some("com.apple.Music".into()),
                active: true,
            },
            "2026-09-20T10:00:00+02:00".into(),
        );
        m.session = SessionId::new();
        m.gaps.push(Gap {
            at_ms: 20_000,
            dur_ms: 5_000,
        });
        m.degraded.push(Degradation::MicNoAec);
        m.state = MeetingState::Done;
        m.duration_ms = Some(35_000);
        m
    }

    #[tokio::test]
    async fn manifest_round_trips_and_rebuild_reindexes() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(tmp.path()).await.unwrap();
        let m = manifest();
        store.create(&m).await.unwrap();
        assert_eq!(store.manifest(m.id).unwrap(), m, "manifest round trip");
        let rows = store.list(10).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].state, MeetingState::Done);
        assert_eq!(rows[0].duration_ms, Some(35_000));
        drop(store);

        // Lose the index: a second meeting exists only as a directory.
        let m2 = manifest();
        let dir2 = tmp.path().join(DIR).join(m2.id.to_string());
        std::fs::create_dir_all(&dir2).unwrap();
        std::fs::write(dir2.join(MANIFEST), serde_json::to_vec(&m2).unwrap()).unwrap();
        std::fs::write(tmp.path().join(DIR).join("not-a-meeting"), b"").unwrap();
        for f in [DB, "meetings.sqlite-wal", "meetings.sqlite-shm"] {
            let _ = std::fs::remove_file(tmp.path().join(DIR).join(f));
        }
        let store = Store::open(tmp.path()).await.unwrap();
        let mut ids: Vec<_> = store
            .list(10)
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.id)
            .collect();
        ids.sort_by_key(|i| i.0);
        let mut want = vec![m.id, m2.id];
        want.sort_by_key(|i| i.0);
        assert_eq!(ids, want, "rebuilt from both manifests");

        store.delete(m.id).await.unwrap();
        assert!(!store.meeting_dir(m.id).exists());
        assert_eq!(store.list(10).await.unwrap().len(), 1);
    }
}
