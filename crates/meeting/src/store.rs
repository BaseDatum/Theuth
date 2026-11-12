//! Data on disk (01 §5): `<data_dir>/meetings/<id>/manifest.json` is truth,
//! rewritten atomically on every state change; `<data_dir>/meetings/meetings.sqlite`
//! is a derived index, rebuilt from the manifests when missing.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context as _;
use runtime::SessionId;
use serde::{Deserialize, Serialize};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};

use crate::calendar::EventRef;
use crate::transcript::{self, Transcript};
use crate::{DIR, Degradation, MeetingId, MeetingState, ProcessInfo};

/// Summary markdown, inside a meeting's directory (04 §6).
pub const SUMMARY: &str = "summary.md";
/// Most hits `search` returns.
pub const SEARCH_CAP: u32 = 40;

/// Index file name, inside [`DIR`].
pub const DB: &str = "meetings.sqlite";
/// Manifest file name, inside a meeting's directory.
pub const MANIFEST: &str = "manifest.json";
/// Sample rate of the three tracks.
pub const TRACK_RATE: u32 = 24_000;

/// A pause: `dur_ms` of wall time missing from the recording clock at `at_ms`. 173: with
/// `track` set, a capture gap instead — the clock ran but `track` was silence-padded from
/// `at_ms` for `dur_ms` (the watchdog's episode, `reason`; `retarget`: the pid the tap moved to).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Gap {
    /// Recording-clock position of the pause.
    pub at_ms: u64,
    /// How long it lasted.
    pub dur_ms: u64,
    /// 173: the silence-padded track of a capture gap; `None` for a pause.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub track: Option<Stem>,
    /// 173: why the capture gap happened.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub reason: Option<crate::CaptureFault>,
    /// 173: the same-bundle pid the system tap moved to during the gap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub retarget: Option<u32>,
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
    /// 194: the session's effort beside `session_model`; absent = `agent.effort`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub session_effort: Option<String>,
    /// 137 R1-D6: the session's memory mode (`epochs` for meetings recorded before it).
    #[serde(default)]
    pub memory: agent::MemoryMode,
    /// The three files.
    #[serde(default)]
    pub tracks: Vec<TrackFile>,
    /// The meeting's note (59 §7.4), created at start; `None` for meetings recorded before notes existed.
    #[serde(default)]
    pub note: Option<document::DocId>,
    /// 158 §8.3 (W2): the matched calendar event, persisted because the calendar row rotates out.
    #[serde(default)]
    pub event: Option<EventRef>,
    /// 158 §9.2 (W3): the meeting's project; `None` = the `meetings` label as before.
    #[serde(default)]
    pub project: Option<String>,
    /// 158 §11.1 (W4): the note template slug; `meeting` reproduces today's headings.
    #[serde(default = "default_template")]
    pub template: String,
    /// 158 §13 (W6): the echo counter from the post pass.
    #[serde(default)]
    pub stats: Option<Stats>,
}

fn default_template() -> String {
    crate::templates::DEFAULT.to_owned()
}

/// 158 §13 (W6): mic segments whose text duplicates a system segment within 2 s — the
/// measure-first echo gate (D12) — and the user's one-per-meeting "used speakers" answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, rename = "MeetingStats"))]
pub struct Stats {
    /// Duplicated mic segments.
    pub echo_dupes: u32,
    /// Mic segments counted.
    pub mic_segments: u32,
    /// Review's "Did you use speakers (not headphones)?" — `None` until answered.
    #[serde(default)]
    pub speakers: Option<bool>,
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
            session_effort: None,
            memory: agent::MemoryMode::Epochs,
            tracks: [Stem::Mic, Stem::System, Stem::Mix]
                .into_iter()
                .map(|t| TrackFile {
                    track: t,
                    file: t.file().to_owned(),
                    sample_rate: TRACK_RATE,
                })
                .collect(),
            note: None,
            event: None,
            project: None,
            template: default_template(),
            stats: None,
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
            project: self.project.clone(),
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
    /// 158 §9.2 (W3): the meeting's project (`meetings.project`).
    #[serde(default)]
    pub project: Option<String>,
}

/// One `meeting_search` hit (04 §5).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SearchHit {
    /// The meeting.
    pub meeting: MeetingId,
    /// Its title (or the meeting app's name).
    pub title: String,
    /// Segment id.
    pub seg: u32,
    /// Segment start.
    pub t0_ms: u64,
    /// Segment end.
    pub t1_ms: u64,
    /// Speaker display name at index time.
    pub speaker: String,
    /// FTS snippet; matches in `[…]`.
    pub snippet: String,
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
    Option<String>,
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
        project: r.8,
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

    /// The index pool, for the 158 calendar tables (`calendar::store`, migration 0003).
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// `<data_dir>/meetings/<id>/`.
    pub fn meeting_dir(&self, id: MeetingId) -> PathBuf {
        self.dir.join(id.to_string())
    }

    /// `<data_dir>/meetings/speakers/`.
    pub fn speakers_dir(&self) -> PathBuf {
        self.dir.join(crate::speakers::DIR)
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
            "INSERT INTO meetings (id, started_at, ended_at, title, session, process_name, duration_ms, state, project) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(id) DO UPDATE SET ended_at = excluded.ended_at, title = excluded.title, \
             duration_ms = excluded.duration_ms, state = excluded.state, project = excluded.project",
        )
        .bind(s.id.to_string())
        .bind(&s.started_at)
        .bind(&s.ended_at)
        .bind(&s.title)
        .bind(s.session.to_string())
        .bind(&s.process_name)
        .bind(s.duration_ms.map(|d| d as i64))
        .bind(serde_json::to_string(&s.state)?)
        .bind(&s.project)
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
        self.list_in(limit, None).await
    }

    /// Newest first; `project` narrows to one project (158 §9.2).
    pub async fn list_in(
        &self,
        limit: u32,
        project: Option<&str>,
    ) -> anyhow::Result<Vec<MeetingSummary>> {
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT id, started_at, ended_at, title, session, process_name, duration_ms, state, project \
             FROM meetings WHERE (? IS NULL OR project = ?) ORDER BY started_at DESC LIMIT ?",
        )
        .bind(project)
        .bind(project)
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .context("list meetings")?;
        rows.into_iter().map(row_to_summary).collect()
    }

    /// 158 §9.4: the last `limit` `Done` meetings of `project`, newest first.
    pub async fn done_in_project(
        &self,
        project: &str,
        limit: u32,
    ) -> anyhow::Result<Vec<MeetingSummary>> {
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT id, started_at, ended_at, title, session, process_name, duration_ms, state, project \
             FROM meetings WHERE project = ? AND state = ? ORDER BY started_at DESC LIMIT ?",
        )
        .bind(project)
        .bind(serde_json::to_string(&MeetingState::Done)?)
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .context("done meetings of project")?;
        rows.into_iter().map(row_to_summary).collect()
    }

    /// The meeting whose session is `session`, if any.
    pub async fn by_session(&self, session: SessionId) -> anyhow::Result<Option<MeetingId>> {
        let row: Option<(String,)> = sqlx::query_as("SELECT id FROM meetings WHERE session = ?")
            .bind(session.to_string())
            .fetch_optional(&self.pool)
            .await
            .context("meeting by session")?;
        Ok(row.and_then(|(id,)| id.parse().ok()))
    }

    /// Directory, then its rows (index and segments).
    pub async fn delete(&self, id: MeetingId) -> anyhow::Result<()> {
        let dir = self.meeting_dir(id);
        if dir.exists() {
            std::fs::remove_dir_all(&dir).with_context(|| format!("remove {}", dir.display()))?;
        }
        sqlx::query("DELETE FROM segments WHERE meeting = ?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await
            .context("delete segments")?;
        sqlx::query("DELETE FROM meetings WHERE id = ?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await
            .context("delete meeting row")?;
        Ok(())
    }

    /// `transcript.json`, if the post pipeline has written one.
    pub fn transcript(&self, id: MeetingId) -> anyhow::Result<Transcript> {
        let path = self.meeting_dir(id).join(transcript::FILE);
        let bytes = std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))
    }

    /// Rewrite `transcript.json` (`.tmp` + rename) and re-index its segments.
    pub async fn save_transcript(&self, t: &Transcript) -> anyhow::Result<()> {
        let dir = self.meeting_dir(t.meeting);
        let tmp = dir.join(".transcript.json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(t)?)
            .with_context(|| format!("write {}", tmp.display()))?;
        std::fs::rename(&tmp, dir.join(transcript::FILE)).context("rename transcript")?;
        self.index_transcript(t).await
    }

    /// `summary.md`.
    pub fn summary(&self, id: MeetingId) -> anyhow::Result<String> {
        let path = self.meeting_dir(id).join(SUMMARY);
        std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))
    }

    /// Delete the meeting's `segments` rows and insert the transcript's (04 §5).
    pub async fn index_transcript(&self, t: &Transcript) -> anyhow::Result<()> {
        let mut tx = self.pool.begin().await.context("begin")?;
        sqlx::query("DELETE FROM segments WHERE meeting = ?")
            .bind(t.meeting.to_string())
            .execute(&mut *tx)
            .await
            .context("clear segments")?;
        for s in &t.segments {
            sqlx::query(
                "INSERT INTO segments (meeting, seg, t0, t1, speaker, text) VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(t.meeting.to_string())
            .bind(i64::from(s.id))
            .bind(s.t0_ms as i64)
            .bind(s.t1_ms as i64)
            .bind(t.name(&s.speaker))
            .bind(&s.text)
            .execute(&mut *tx)
            .await
            .context("insert segment")?;
        }
        tx.commit().await.context("commit")
    }

    /// FTS5 over `speaker` and `text`, one meeting or all, best rank first, ≤ [`SEARCH_CAP`].
    pub async fn search(
        &self,
        query: &str,
        meeting: Option<MeetingId>,
    ) -> anyhow::Result<Vec<SearchHit>> {
        type Hit = (
            String,
            Option<String>,
            String,
            i64,
            i64,
            i64,
            String,
            String,
        );
        let scope = meeting.map(|m| m.to_string());
        let rows: Vec<Hit> = sqlx::query_as(
            "SELECT s.meeting, m.title, m.process_name, s.seg, s.t0, s.t1, s.speaker, \
             snippet(segments, 5, '[', ']', '…', 16) \
             FROM segments s JOIN meetings m ON m.id = s.meeting \
             WHERE segments MATCH ? AND (? IS NULL OR s.meeting = ?) ORDER BY rank LIMIT ?",
        )
        .bind(fts_query(query))
        .bind(&scope)
        .bind(&scope)
        .bind(i64::from(SEARCH_CAP))
        .fetch_all(&self.pool)
        .await
        .context("search segments")?;
        rows.into_iter()
            .map(|(id, title, process, seg, t0, t1, speaker, snippet)| {
                Ok(SearchHit {
                    meeting: id.parse().context("meeting id")?,
                    title: title.unwrap_or(process),
                    seg: seg as u32,
                    t0_ms: t0 as u64,
                    t1_ms: t1 as u64,
                    speaker,
                    snippet,
                })
            })
            .collect()
    }

    /// Every manifest on disk (the directory is truth), unsorted.
    pub fn manifests(&self) -> Vec<Manifest> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter_map(|e| e.file_name().to_string_lossy().parse::<MeetingId>().ok())
            .filter_map(|id| match self.manifest(id) {
                Ok(m) => Some(m),
                Err(e) => {
                    tracing::warn!(meeting = %id, "skipping: {e:#}");
                    None
                }
            })
            .collect()
    }

    /// Scan `<dir>/*/manifest.json` (and `transcript.json`) into the index (01 §0.4, 04 §5).
    async fn rebuild(&self) -> anyhow::Result<()> {
        for m in self.manifests() {
            self.upsert(&m.summary()).await?;
            if let Ok(t) = self.transcript(m.id) {
                self.index_transcript(&t).await?;
            }
        }
        Ok(())
    }
}

/// A user query as an FTS5 MATCH expression: every word quoted, so `-`, `:`
/// and the like are text, not operators; words are ANDed.
fn fts_query(q: &str) -> String {
    q.split_whitespace()
        .map(|w| format!("\"{}\"", w.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" ")
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
            track: None,
            reason: None,
            retarget: None,
        });
        m.degraded.push(Degradation::MicNoAec);
        m.state = MeetingState::Done;
        m.duration_ms = Some(35_000);
        m
    }

    /// The 2026-11-02 incident's `manifest.json`, verbatim (written by main 1f921ce4's types).
    const INCIDENT: &str = r#"{
  "id": "01a19e11-dc39-7912-9f9f-0ed0169b1ccf",
  "session": "01a19e11-dc39-7912-9f9f-0eeda3254f9d",
  "title": "SRE technical interview: troubleshooting questions, recommended moving forward",
  "context": null,
  "started_at": "2026-11-02T21:02:13.049500+00:00",
  "ended_at": "2026-11-02T21:46:35.401392+00:00",
  "duration_ms": 2657938,
  "process": { "pid": 51437, "name": "zoom.us", "bundle_id": "us.zoom.xos", "active": true },
  "consent_at": "2026-11-02T21:02:13.049500+00:00",
  "gaps": [],
  "degraded": [],
  "state": "Done",
  "failure": null,
  "session_model": null,
  "memory": "endless",
  "tracks": [
    { "track": "mic", "file": "mic.opus", "sample_rate": 24000 },
    { "track": "system", "file": "system.opus", "sample_rate": 24000 },
    { "track": "mix", "file": "mix.opus", "sample_rate": 24000 }
  ],
  "note": "rvtckcmvi5kw3g5fhw5jc5anqq",
  "event": null,
  "project": null,
  "template": "meeting",
  "stats": { "echo_dupes": 0, "mic_segments": 178, "speakers": null }
}"#;

    /// 173: a main-era manifest (the 2026-11-02 incident's, verbatim) loads with this build's
    /// types, and re-serializes unchanged (every 173 field is optional).
    #[test]
    fn a_main_era_manifest_loads() {
        let m: Manifest = serde_json::from_str(INCIDENT).unwrap();
        assert_eq!(m.process.bundle_id.as_deref(), Some("us.zoom.xos"));
        assert!(m.gaps.is_empty() && m.degraded.is_empty());
        let again: serde_json::Value = serde_json::to_value(&m).unwrap();
        let orig: serde_json::Value = serde_json::from_str(INCIDENT).unwrap();
        assert_eq!(again, orig);
    }

    /// 158 §13 (W6): a manifest written before `stats` (and a `stats` written before
    /// `speakers`) still parses; a counted manifest round-trips.
    #[test]
    fn manifest_without_stats_parses_and_stats_round_trip() {
        let m = manifest();
        let mut v = serde_json::to_value(&m).unwrap();
        v.as_object_mut().unwrap().remove("stats");
        assert_eq!(serde_json::from_value::<Manifest>(v).unwrap(), m);
        let old: Stats = serde_json::from_str(r#"{"echo_dupes":3,"mic_segments":40}"#).unwrap();
        assert_eq!(old.speakers, None);
        let mut m = manifest();
        m.stats = Some(Stats {
            echo_dupes: 3,
            mic_segments: 40,
            speakers: Some(true),
        });
        let back: Manifest = serde_json::from_slice(&serde_json::to_vec(&m).unwrap()).unwrap();
        assert_eq!(back, m);
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

    fn transcript(meeting: MeetingId, text: &str) -> Transcript {
        let word = |t: &str, t0: u64| crate::transcript::Word {
            text: t.into(),
            t0_ms: t0,
            t1_ms: t0 + 300,
        };
        Transcript {
            meeting,
            speakers: vec![
                crate::transcript::SpeakerLabel {
                    id: "s1".into(),
                    name: "Dana".into(),
                    profile: None,
                    proposed: None,
                },
                crate::transcript::SpeakerLabel {
                    id: "me".into(),
                    name: "Me".into(),
                    profile: None,
                    proposed: None,
                },
            ],
            segments: vec![
                crate::transcript::Segment {
                    id: 0,
                    track: crate::Track::System,
                    speaker: "s1".into(),
                    t0_ms: 5_000,
                    t1_ms: 8_000,
                    text: text.into(),
                    words: vec![word("x", 5_000)],
                },
                crate::transcript::Segment {
                    id: 1,
                    track: crate::Track::Mic,
                    speaker: "me".into(),
                    t0_ms: 9_000,
                    t1_ms: 9_500,
                    text: "Right.".into(),
                    words: vec![word("Right.", 9_000)],
                },
            ],
            reviewed_at: None,
            summarized_at: None,
            diarized: true,
            note: None,
        }
    }

    /// Index → search (speaker and text, one meeting or all, hyphenated ticket
    /// as text) → re-index after an edit finds the new text and not the old →
    /// delete removes the rows → a rebuilt index picks the file back up.
    #[tokio::test]
    async fn fts_round_trip_follows_edits_delete_and_rebuild() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(tmp.path()).await.unwrap();
        let m = manifest();
        store.create(&m).await.unwrap();
        let other = manifest();
        store.create(&other).await.unwrap();
        store
            .save_transcript(&transcript(
                m.id,
                "The DR sync for SRE-2386 is blocked on the image tags.",
            ))
            .await
            .unwrap();
        store
            .save_transcript(&transcript(other.id, "KEDA scaled the workers again."))
            .await
            .unwrap();
        assert_eq!(
            store.transcript(m.id).unwrap().segments.len(),
            2,
            "file round trip"
        );

        let hits = store.search("image tags", None).await.unwrap();
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert_eq!(
            (hits[0].meeting, hits[0].seg, hits[0].t0_ms),
            (m.id, 0, 5_000)
        );
        assert_eq!(hits[0].speaker, "Dana");
        assert_eq!(hits[0].title, "Music", "process name when untitled");
        assert!(
            hits[0].snippet.contains("[image] [tags]"),
            "{}",
            hits[0].snippet
        );
        assert_eq!(
            store.search("SRE-2386", None).await.unwrap().len(),
            1,
            "hyphen is text"
        );
        assert_eq!(
            store.search("dana", None).await.unwrap().len(),
            2,
            "speaker is indexed (both meetings)"
        );
        assert_eq!(
            store.search("keda", Some(m.id)).await.unwrap().len(),
            0,
            "scoped"
        );
        assert_eq!(store.search("keda", Some(other.id)).await.unwrap().len(), 1);
        assert!(
            store.search("", None).await.is_err()
                || store.search("", None).await.unwrap().is_empty()
        );

        store
            .save_transcript(&transcript(
                m.id,
                "The DR sync for SRE-2386 is blocked on the container tags.",
            ))
            .await
            .unwrap();
        assert!(
            store.search("image", None).await.unwrap().is_empty(),
            "old text gone"
        );
        assert_eq!(store.search("container", None).await.unwrap().len(), 1);

        store.delete(other.id).await.unwrap();
        assert!(
            store.search("keda", None).await.unwrap().is_empty(),
            "rows go with the meeting"
        );
        drop(store);
        for f in [DB, "meetings.sqlite-wal", "meetings.sqlite-shm"] {
            let _ = std::fs::remove_file(tmp.path().join(DIR).join(f));
        }
        let store = Store::open(tmp.path()).await.unwrap();
        assert_eq!(
            store.search("container", None).await.unwrap().len(),
            1,
            "rebuilt from transcript.json"
        );
    }
}
