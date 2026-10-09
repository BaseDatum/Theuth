//! Dictation history and correction learning (153 §6): `<data_dir>/voice.db`
//! (sqlx, migrations under `crates/voice/migrations/`), audio under
//! `<data_dir>/voice/history/<id>.pcm16`, and the suggestion flow of §6.3.
//!
//! Privacy (§6.5/§6.7): the actor calls nothing here when
//! [`Store::recording_allowed`] is false; the store never checks the config itself.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;
use std::time::Duration;

use anyhow::Context as _;
use serde::{Deserialize, Serialize};
use sqlx::Row as _;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteRow};

use crate::dictionary::{Dictionary, Entry, EntryId, Kind, Negative, Scope, Source, Variant};
use crate::{DictationId, HistoryConfig, Surface};

pub mod learn;

/// §6.5 `HISTORY_ROWS`: rows kept; older rows are deleted (corrections cascade).
pub const HISTORY_ROWS: u32 = 200;
/// §6.5 `AUDIO_BUDGET`: bytes of PCM kept across all rows; oldest files unlinked first.
pub const AUDIO_BUDGET: u64 = 64 << 20;
/// §6.2 `PCM_RETENTION`: 120 s of 16 kHz i16 per dictation; the newest dictation is never evicted.
pub const PCM_RETENTION_BYTES: u64 = 120 * 16_000 * 2;

/// Identifies one pending dictionary suggestion (§6.3); answered with `dictionary_suggestion_answer`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(transparent)]
pub struct SuggestionId(pub uuid::Uuid);

impl SuggestionId {
    /// A fresh time-ordered id.
    pub fn new() -> Self {
        Self(uuid::Uuid::now_v7())
    }
}

impl Default for SuggestionId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for SuggestionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// The user's answer to a suggestion card (§6.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum SuggestionAnswer {
    /// Add the learned entry.
    Add,
    /// Cooldown; ask again later.
    NotNow,
    /// Never for this pair: write a negative.
    Never,
}

/// One row of `dictations` (§6.5), as `dictation_history` returns it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct HistoryRow {
    /// Dictation.
    pub id: DictationId,
    /// Unix ms.
    pub started_ms: u64,
    /// Unix ms; `None` while running.
    pub finished_ms: Option<u64>,
    /// Where it was dictated.
    pub surface: Surface,
    /// `agent_prompt | document | message_reply | any_app:<bundle or exe>`.
    pub destination: String,
    /// Any-app: bundle id / exe.
    pub app: Option<String>,
    /// Finals joined.
    pub raw: String,
    /// Finals joined, shaped.
    pub shaped: String,
    /// `None` unless §9 ran.
    pub polished: Option<String>,
    /// What actually went out (`polished ?? shaped`), after end-command edits.
    pub delivered: Option<String>,
    /// §8.7, relative to press.
    pub press_ms: Option<u32>,
    /// §8.7.
    pub first_partial_ms: Option<u32>,
    /// §8.7.
    pub first_final_ms: Option<u32>,
    /// §8.7.
    pub final_ms: Option<u32>,
    /// `done | stopped | error:<message>`.
    pub reason: String,
    /// Bytes of PCM kept; `0` when evicted or never recorded.
    pub audio_bytes: u64,
    /// Audio is on disk (`dictation_audio_get` works).
    pub has_audio: bool,
}

/// One `corrections` row (§6.3): a single-word edit between what was dictated and what was sent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Correction {
    /// Utterance index within the dictation.
    pub utterance: u32,
    /// What the model heard.
    pub heard: String,
    /// What the user typed instead.
    pub typed: String,
    /// Unix ms.
    pub at_ms: u64,
}

/// A suggestion card that is due (§6.3 step 3): the payload of `VoiceEvent::DictionarySuggestion`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suggestion {
    /// Answer with `dictionary_suggestion_answer`.
    pub id: SuggestionId,
    /// What the model heard.
    pub heard: String,
    /// What the user typed instead.
    pub typed: String,
    /// Occurrences of the pair in the last 7 days.
    pub occurrences: u32,
    /// Where the last occurrence was dictated.
    pub surface: Surface,
    /// The dictation's audio is on disk.
    pub has_audio: bool,
}

/// The history store: `voice.db` + the audio directory.
#[derive(Clone, Debug)]
pub struct Store {
    /// `<data_dir>/voice/history`.
    audio_dir: PathBuf,
    db: sqlx::SqlitePool,
    rows: u32,
    audio_budget: u64,
    session: Arc<Mutex<Session>>,
}

/// Engine-lifetime state (§6.2 `IGNORES_PER_SESSION`, open audio files, unanswered cards).
#[derive(Debug, Default)]
struct Session {
    audio: HashMap<DictationId, AudioFile>,
    /// Published cards awaiting an answer, with their publish time.
    cards: HashMap<SuggestionId, (Suggestion, u64)>,
    ignores: u32,
    #[cfg(test)]
    now: Option<u64>,
}

#[derive(Debug)]
struct AudioFile {
    file: std::fs::File,
    bytes: u64,
    /// Cumulative byte end offset per utterance.
    ends: Vec<u64>,
}

fn surface_str(s: Surface) -> &'static str {
    match s {
        Surface::Composer => "composer",
        Surface::Tui => "tui",
        Surface::Companion => "companion",
        Surface::Phone => "phone",
        Surface::AnyApp => "any_app",
    }
}

fn parse_surface(s: &str) -> anyhow::Result<Surface> {
    Ok(match s {
        "composer" => Surface::Composer,
        "tui" => Surface::Tui,
        "companion" => Surface::Companion,
        "phone" => Surface::Phone,
        "any_app" => Surface::AnyApp,
        other => anyhow::bail!("unknown surface {other:?}"),
    })
}

fn ends_str(ends: &[u64]) -> String {
    ends.iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

fn parse_ends(s: &str) -> Vec<u64> {
    s.split(',').filter_map(|n| n.parse().ok()).collect()
}

const ROW_COLS: &str = "id, started_ms, finished_ms, surface, destination, app, raw, shaped, polished, delivered, \
    press_ms, first_partial_ms, first_final_ms, final_ms, reason, audio_bytes, audio_path";

fn history_row(r: &SqliteRow) -> anyhow::Result<HistoryRow> {
    let opt_u32 = |col: &str| -> anyhow::Result<Option<u32>> {
        Ok(r.try_get::<Option<i64>, _>(col)?.map(|v| v as u32))
    };
    Ok(HistoryRow {
        id: r.try_get::<String, _>("id")?.parse()?,
        started_ms: r.try_get::<i64, _>("started_ms")? as u64,
        finished_ms: r
            .try_get::<Option<i64>, _>("finished_ms")?
            .map(|v| v as u64),
        surface: parse_surface(&r.try_get::<String, _>("surface")?)?,
        destination: r.try_get("destination")?,
        app: r.try_get("app")?,
        raw: r.try_get("raw")?,
        shaped: r.try_get("shaped")?,
        polished: r.try_get("polished")?,
        delivered: r.try_get("delivered")?,
        press_ms: opt_u32("press_ms")?,
        first_partial_ms: opt_u32("first_partial_ms")?,
        first_final_ms: opt_u32("first_final_ms")?,
        final_ms: opt_u32("final_ms")?,
        reason: r.try_get("reason")?,
        audio_bytes: r.try_get::<i64, _>("audio_bytes")? as u64,
        has_audio: r.try_get::<Option<String>, _>("audio_path")?.is_some(),
    })
}

fn system_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

impl Store {
    /// Open (and migrate) `<data_dir>/voice.db`; budgets default to [`HISTORY_ROWS`] / [`AUDIO_BUDGET`].
    pub async fn open(data_dir: &Path) -> anyhow::Result<Store> {
        let audio_dir = data_dir.join("voice").join("history");
        std::fs::create_dir_all(&audio_dir)
            .with_context(|| format!("create {}", audio_dir.display()))?;
        let db = data_dir.join("voice.db");
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
        Ok(Store {
            audio_dir,
            db: pool,
            rows: HISTORY_ROWS,
            audio_budget: AUDIO_BUDGET,
            session: Arc::default(),
        })
    }

    /// Apply `voice.history.rows` / `voice.history.audio_mib` (as bytes).
    pub fn with_budget(mut self, rows: u32, audio_bytes: u64) -> Store {
        self.rows = rows;
        self.audio_budget = audio_bytes;
        self
    }

    /// §6.5: a private session or `voice.history.enabled: false` writes neither row nor audio.
    pub fn recording_allowed(cfg: &HistoryConfig, private_session: bool) -> bool {
        cfg.enabled && !private_session
    }

    fn now_ms(&self) -> u64 {
        #[cfg(test)]
        if let Some(now) = self.session.lock().now {
            return now;
        }
        system_now_ms()
    }

    /// Insert or update the dictation's row; audio columns come from [`Store::attach_audio`].
    /// A finished row closes its audio file and enforces the budget.
    pub async fn record(&self, row: HistoryRow) -> anyhow::Result<()> {
        let (bytes, path, ends) = {
            let mut s = self.session.lock();
            let snapshot = s.audio.get(&row.id).map(|a| (a.bytes, ends_str(&a.ends)));
            if row.finished_ms.is_some() {
                s.audio.remove(&row.id);
            }
            match snapshot {
                Some((bytes, ends)) => (
                    bytes as i64,
                    Some(self.audio_path(row.id).display().to_string()),
                    ends,
                ),
                None => (0, None, String::new()),
            }
        };
        sqlx::query(
            "INSERT INTO dictations (id, started_ms, finished_ms, surface, destination, app, raw, shaped, \
             polished, delivered, press_ms, first_partial_ms, first_final_ms, final_ms, reason, \
             audio_bytes, audio_path, utterance_ends) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(id) DO UPDATE SET started_ms = excluded.started_ms, \
             finished_ms = excluded.finished_ms, surface = excluded.surface, \
             destination = excluded.destination, app = excluded.app, raw = excluded.raw, \
             shaped = excluded.shaped, polished = excluded.polished, delivered = excluded.delivered, \
             press_ms = excluded.press_ms, first_partial_ms = excluded.first_partial_ms, \
             first_final_ms = excluded.first_final_ms, final_ms = excluded.final_ms, \
             reason = excluded.reason",
        )
        .bind(row.id.to_string())
        .bind(row.started_ms as i64)
        .bind(row.finished_ms.map(|v| v as i64))
        .bind(surface_str(row.surface))
        .bind(&row.destination)
        .bind(&row.app)
        .bind(&row.raw)
        .bind(&row.shaped)
        .bind(&row.polished)
        .bind(&row.delivered)
        .bind(row.press_ms)
        .bind(row.first_partial_ms)
        .bind(row.first_final_ms)
        .bind(row.final_ms)
        .bind(&row.reason)
        .bind(bytes)
        .bind(path)
        .bind(ends)
        .execute(&self.db)
        .await?;
        if row.finished_ms.is_some() {
            self.enforce_budget().await?;
        }
        Ok(())
    }

    fn audio_path(&self, id: DictationId) -> PathBuf {
        self.audio_dir.join(format!("{id}.pcm16"))
    }

    /// Append `pcm` (16 kHz i16) to the dictation's audio file, up to [`PCM_RETENTION_BYTES`];
    /// updates the row's audio columns when the row already exists.
    pub async fn attach_audio(
        &self,
        id: DictationId,
        utterance: u32,
        pcm: &[i16],
    ) -> anyhow::Result<()> {
        let path = self.audio_path(id);
        let (bytes, ends) = {
            let mut s = self.session.lock();
            let a = match s.audio.entry(id) {
                std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
                std::collections::hash_map::Entry::Vacant(e) => {
                    let file = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&path)
                        .with_context(|| format!("open {}", path.display()))?;
                    e.insert(AudioFile {
                        file,
                        bytes: 0,
                        ends: Vec::new(),
                    })
                }
            };
            let room = (PCM_RETENTION_BYTES.saturating_sub(a.bytes) / 2) as usize;
            let pcm = &pcm[..pcm.len().min(room)];
            let buf: Vec<u8> = pcm.iter().flat_map(|v| v.to_le_bytes()).collect();
            a.file.write_all(&buf)?;
            a.bytes += buf.len() as u64;
            let u = utterance as usize;
            while a.ends.len() <= u {
                a.ends.push(a.bytes - buf.len() as u64);
            }
            // ponytail: assumes utterances arrive in order; a late chunk for an earlier utterance
            // just extends that utterance's end over the later one.
            a.ends[u] = a.bytes;
            (a.bytes as i64, ends_str(&a.ends))
        };
        sqlx::query(
            "UPDATE dictations SET audio_bytes = ?, audio_path = ?, utterance_ends = ? WHERE id = ?",
        )
        .bind(bytes)
        .bind(path.display().to_string())
        .bind(ends)
        .bind(id.to_string())
        .execute(&self.db)
        .await?;
        Ok(())
    }

    /// Newest `limit` rows.
    pub async fn list(&self, limit: u32) -> anyhow::Result<Vec<HistoryRow>> {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {ROW_COLS} FROM dictations ORDER BY started_ms DESC, id DESC LIMIT ?"
        )))
        .bind(limit)
        .fetch_all(&self.db)
        .await?
        .iter()
        .map(history_row)
        .collect()
    }

    /// Delete one row (and its audio), or everything when `None`.
    pub async fn forget(&self, id: Option<DictationId>) -> anyhow::Result<()> {
        match id {
            Some(id) => {
                self.session.lock().audio.remove(&id);
                unlink(&self.audio_path(id));
                sqlx::query("DELETE FROM dictations WHERE id = ?")
                    .bind(id.to_string())
                    .execute(&self.db)
                    .await?;
            }
            None => {
                self.session.lock().audio.clear();
                sqlx::query("DELETE FROM dictations")
                    .execute(&self.db)
                    .await?;
                std::fs::remove_dir_all(&self.audio_dir)?;
                std::fs::create_dir_all(&self.audio_dir)?;
            }
        }
        Ok(())
    }

    /// The dictation's PCM (one utterance, or all); `Err("no audio")` when evicted.
    pub async fn audio(&self, id: DictationId, utterance: Option<u32>) -> anyhow::Result<Vec<i16>> {
        let Some(row) =
            sqlx::query("SELECT audio_path, utterance_ends FROM dictations WHERE id = ?")
                .bind(id.to_string())
                .fetch_optional(&self.db)
                .await?
        else {
            anyhow::bail!("no audio")
        };
        let Some(path) = row.try_get::<Option<String>, _>("audio_path")? else {
            anyhow::bail!("no audio")
        };
        let bytes = std::fs::read(&path).with_context(|| format!("read {path}"))?;
        let bytes = match utterance {
            None => &bytes[..],
            Some(u) => {
                let ends = parse_ends(&row.try_get::<String, _>("utterance_ends")?);
                let u = u as usize;
                let (start, end) = match (u.checked_sub(1).map(|p| ends[p]), ends.get(u)) {
                    (start, Some(&end)) => (start.unwrap_or(0) as usize, end as usize),
                    _ => anyhow::bail!("no audio"),
                };
                &bytes[start.min(bytes.len())..end.min(bytes.len())]
            }
        };
        Ok(bytes
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect())
    }

    /// `dictation_correction` landed (§6.1): diff the delivered text against `sent_text`,
    /// store the single-word corrections, and return the suggestions that are due (§6.3).
    /// Unanswered cards older than [`learn::CARD_TIMEOUT_MS`] count as ignored; after
    /// [`learn::IGNORES_PER_SESSION`] nothing more is suggested until restart.
    pub async fn correction(
        &self,
        id: DictationId,
        sent_text: &str,
    ) -> anyhow::Result<Vec<Suggestion>> {
        let Some(row) = sqlx::query(
            "SELECT surface, shaped, delivered, audio_path FROM dictations WHERE id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(&self.db)
        .await?
        else {
            return Ok(Vec::new());
        };
        let surface = parse_surface(&row.try_get::<String, _>("surface")?)?;
        let has_audio = row.try_get::<Option<String>, _>("audio_path")?.is_some();
        let delivered: String = match row.try_get::<Option<String>, _>("delivered")? {
            Some(d) => d,
            None => row.try_get("shaped")?,
        };
        let now = self.now_ms();
        let cands = learn::candidates(&delivered, sent_text);
        for c in &cands {
            sqlx::query(
                "INSERT INTO corrections (dictation_id, utterance, heard, typed, at_ms) \
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(id.to_string())
            .bind(c.utterance)
            .bind(&c.heard)
            .bind(&c.typed)
            .bind(now as i64)
            .execute(&self.db)
            .await?;
        }
        {
            let mut s = self.session.lock();
            let before = s.cards.len();
            s.cards
                .retain(|_, (_, at)| *at + learn::CARD_TIMEOUT_MS > now);
            s.ignores += (before - s.cards.len()) as u32;
            if s.ignores >= learn::IGNORES_PER_SESSION {
                return Ok(Vec::new());
            }
        }
        let mut out: Vec<Suggestion> = Vec::new();
        for c in cands {
            let same =
                |x: &Suggestion| x.heard.eq_ignore_ascii_case(&c.heard) && x.typed == c.typed;
            if out.iter().any(same) || self.session.lock().cards.values().any(|(x, _)| same(x)) {
                continue;
            }
            let history: Vec<Correction> = sqlx::query(
                "SELECT utterance, heard, typed, at_ms FROM corrections WHERE typed = ?",
            )
            .bind(&c.typed)
            .fetch_all(&self.db)
            .await?
            .iter()
            .map(|r| {
                anyhow::Ok(Correction {
                    utterance: r.try_get::<i64, _>("utterance")? as u32,
                    heard: r.try_get("heard")?,
                    typed: r.try_get("typed")?,
                    at_ms: r.try_get::<i64, _>("at_ms")? as u64,
                })
            })
            .collect::<anyhow::Result<_>>()?;
            if !learn::due(&c.heard, &c.typed, &history, now) {
                continue;
            }
            let state = sqlx::query(
                "SELECT state, until_ms FROM suggestion_state WHERE heard_lc = ? AND typed = ?",
            )
            .bind(c.heard.to_lowercase())
            .bind(&c.typed)
            .fetch_optional(&self.db)
            .await?;
            if let Some(st) = state {
                let until = st.try_get::<Option<i64>, _>("until_ms")?.unwrap_or(0) as u64;
                match st.try_get::<String, _>("state")?.as_str() {
                    "cooldown" if until <= now => {}
                    _ => continue,
                }
            }
            let sug = Suggestion {
                id: SuggestionId::new(),
                occurrences: learn::occurrences(&c.heard, &c.typed, &history, now),
                heard: c.heard,
                typed: c.typed,
                surface,
                has_audio,
            };
            self.session.lock().cards.insert(sug.id, (sug.clone(), now));
            out.push(sug);
        }
        Ok(out)
    }

    /// §6.3 step 4: record the answer. `Add` puts a learned entry (heard variant) into `dict`,
    /// `NotNow` writes a 7-day cooldown, `Never` writes a negative to both stores.
    pub async fn answer(
        &self,
        id: SuggestionId,
        answer: SuggestionAnswer,
        dict: &mut Dictionary,
    ) -> anyhow::Result<()> {
        let Some((sug, _)) = self.session.lock().cards.remove(&id) else {
            anyhow::bail!("unknown suggestion {id}")
        };
        let now = self.now_ms();
        let (state, until) = match answer {
            SuggestionAnswer::Add => ("added", None),
            SuggestionAnswer::NotNow => ("cooldown", Some((now + learn::WEEK_MS) as i64)),
            SuggestionAnswer::Never => ("never", None),
        };
        sqlx::query(
            "INSERT INTO suggestion_state (heard_lc, typed, state, until_ms) VALUES (?, ?, ?, ?) \
             ON CONFLICT(heard_lc, typed) DO UPDATE SET state = excluded.state, \
             until_ms = excluded.until_ms",
        )
        .bind(sug.heard.to_lowercase())
        .bind(&sug.typed)
        .bind(state)
        .bind(until)
        .execute(&self.db)
        .await?;
        match answer {
            SuggestionAnswer::Add => {
                dict.put(Entry {
                    id: EntryId(format!("learned:{}", uuid::Uuid::now_v7())),
                    canonical: sug.typed,
                    kind: Kind::Term,
                    spoken: vec![sug.heard.clone()],
                    variants: vec![Variant {
                        heard: sug.heard,
                        metaphone: (String::new(), String::new()),
                        seen: sug.occurrences,
                        last_ms: now,
                    }],
                    scope: Scope::Global,
                    source: Source::Learned,
                    tags: Vec::new(),
                });
            }
            SuggestionAnswer::Never => dict.negative(Negative {
                heard_lc: sug.heard.to_lowercase(),
                canonical: sug.typed,
            }),
            SuggestionAnswer::NotNow => {}
        }
        Ok(())
    }

    /// §6.5 budget: audio first (oldest files unlinked, the newest dictation exempt), then rows.
    async fn enforce_budget(&self) -> anyhow::Result<()> {
        let with_audio = sqlx::query(
            "SELECT id, audio_path, audio_bytes FROM dictations WHERE audio_path IS NOT NULL \
             ORDER BY started_ms DESC, id DESC",
        )
        .fetch_all(&self.db)
        .await?;
        let mut total: u64 = with_audio
            .iter()
            .map(|r| r.get::<i64, _>("audio_bytes") as u64)
            .sum();
        for r in with_audio.iter().skip(1).rev() {
            if total <= self.audio_budget {
                break;
            }
            unlink(Path::new(&r.get::<String, _>("audio_path")));
            sqlx::query("UPDATE dictations SET audio_path = NULL, audio_bytes = 0 WHERE id = ?")
                .bind(r.get::<String, _>("id"))
                .execute(&self.db)
                .await?;
            total -= r.get::<i64, _>("audio_bytes") as u64;
        }
        let stale = sqlx::query(
            "SELECT id, audio_path FROM dictations ORDER BY started_ms DESC, id DESC \
             LIMIT -1 OFFSET ?",
        )
        .bind(self.rows)
        .fetch_all(&self.db)
        .await?;
        for r in stale {
            if let Some(p) = r.get::<Option<String>, _>("audio_path") {
                unlink(Path::new(&p));
            }
            sqlx::query("DELETE FROM dictations WHERE id = ?")
                .bind(r.get::<String, _>("id"))
                .execute(&self.db)
                .await?;
        }
        Ok(())
    }
}

fn unlink(path: &Path) {
    if let Err(e) = std::fs::remove_file(path)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(path = %path.display(), error = %e, "voice history: unlink audio");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Tmp(PathBuf);

    impl Tmp {
        fn new() -> Tmp {
            let p = std::env::temp_dir().join(format!("voice-history-{}", uuid::Uuid::now_v7()));
            std::fs::create_dir_all(&p).unwrap();
            Tmp(p)
        }
    }

    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn row(started_ms: u64, shaped: &str) -> HistoryRow {
        HistoryRow {
            id: DictationId::new(),
            started_ms,
            finished_ms: Some(started_ms + 1000),
            surface: Surface::Composer,
            destination: "agent_prompt".into(),
            app: None,
            raw: shaped.to_lowercase(),
            shaped: shaped.into(),
            polished: None,
            delivered: None,
            press_ms: Some(10),
            first_partial_ms: Some(200),
            first_final_ms: Some(900),
            final_ms: Some(300),
            reason: "done".into(),
            audio_bytes: 0,
            has_audio: false,
        }
    }

    fn set_now(store: &Store, ms: u64) {
        store.session.lock().now = Some(ms);
    }

    /// Migrations apply on a fresh dir; a row round-trips through record/list/audio/forget.
    #[tokio::test]
    async fn round_trip() {
        let tmp = Tmp::new();
        let store = Store::open(&tmp.0).await.unwrap();
        assert!(tmp.0.join("voice.db").is_file());
        let r = row(1_000, "Hello World");
        // audio streams before the finished row lands, two utterances
        store.attach_audio(r.id, 0, &[1, 2, 3]).await.unwrap();
        store.attach_audio(r.id, 1, &[4, 5]).await.unwrap();
        store.attach_audio(r.id, 1, &[6]).await.unwrap();
        store.record(r.clone()).await.unwrap();
        let listed = store.list(10).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(
            listed[0],
            HistoryRow {
                audio_bytes: 12,
                has_audio: true,
                ..r.clone()
            }
        );
        assert_eq!(
            store.audio(r.id, None).await.unwrap(),
            vec![1, 2, 3, 4, 5, 6]
        );
        assert_eq!(store.audio(r.id, Some(0)).await.unwrap(), vec![1, 2, 3]);
        assert_eq!(store.audio(r.id, Some(1)).await.unwrap(), vec![4, 5, 6]);
        assert!(store.audio(r.id, Some(2)).await.is_err());
        // re-record (e.g. delivered filled in later) keeps the audio columns
        store
            .record(HistoryRow {
                delivered: Some("Hello World!".into()),
                ..r.clone()
            })
            .await
            .unwrap();
        let again = &store.list(10).await.unwrap()[0];
        assert_eq!(again.delivered.as_deref(), Some("Hello World!"));
        assert!(again.has_audio);
        // reopen: the schema is already there, the row survives
        drop(store);
        let store = Store::open(&tmp.0).await.unwrap();
        assert_eq!(store.list(10).await.unwrap().len(), 1);
        store.forget(Some(r.id)).await.unwrap();
        assert!(store.list(10).await.unwrap().is_empty());
        assert!(!store.audio_path(r.id).exists());
        let err = store.audio(r.id, None).await.unwrap_err();
        assert_eq!(err.to_string(), "no audio");
        store.record(row(2_000, "x")).await.unwrap();
        store.forget(None).await.unwrap();
        assert!(store.list(10).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn budget_evicts_audio_first_then_rows() {
        let tmp = Tmp::new();
        // 3 rows, 5 000 bytes of audio; each dictation brings 2 000 bytes.
        let store = Store::open(&tmp.0).await.unwrap().with_budget(3, 5_000);
        let mut ids = Vec::new();
        for i in 0..3u64 {
            let r = row(1_000 * (i + 1), "a");
            store.attach_audio(r.id, 0, &[7; 1000]).await.unwrap();
            store.record(r.clone()).await.unwrap();
            ids.push(r.id);
        }
        // 6 000 > 5 000: the oldest file goes, its row stays; the newest is exempt.
        let rows = store.list(10).await.unwrap();
        assert_eq!(rows.len(), 3);
        let by_id = |id: DictationId| rows.iter().find(|r| r.id == id).unwrap().clone();
        assert!(!by_id(ids[0]).has_audio);
        assert_eq!(by_id(ids[0]).audio_bytes, 0);
        assert!(by_id(ids[1]).has_audio);
        assert!(by_id(ids[2]).has_audio);
        assert_eq!(
            store.audio(ids[0], None).await.unwrap_err().to_string(),
            "no audio"
        );
        assert_eq!(store.audio(ids[1], None).await.unwrap().len(), 1000);
        assert!(!store.audio_path(ids[0]).exists());
        // a 4th row exceeds HISTORY_ROWS: the oldest row is deleted (its corrections cascade).
        store.correction(ids[0], "a").await.unwrap();
        let r = row(4_000, "a");
        store.attach_audio(r.id, 0, &[7; 1000]).await.unwrap();
        store.record(r.clone()).await.unwrap();
        let rows = store.list(10).await.unwrap();
        assert_eq!(
            rows.iter().map(|r| r.id).collect::<Vec<_>>(),
            vec![r.id, ids[2], ids[1]]
        );
        let orphans: i64 = sqlx::query("SELECT count(*) AS n FROM corrections")
            .fetch_one(&store.db)
            .await
            .unwrap()
            .get("n");
        assert_eq!(orphans, 0);
        // audio again: 6 000 → ids[1]'s file goes, the newest (r) stays whole
        assert!(!store.list(10).await.unwrap()[2].has_audio);
        assert_eq!(store.audio(r.id, None).await.unwrap().len(), 1000);
    }

    /// Per-dictation audio is capped at `PCM_RETENTION_BYTES`.
    #[tokio::test]
    async fn audio_capped_at_retention() {
        let tmp = Tmp::new();
        let store = Store::open(&tmp.0).await.unwrap();
        let r = row(1_000, "a");
        let samples = (PCM_RETENTION_BYTES / 2) as usize;
        store
            .attach_audio(r.id, 0, &vec![1; samples - 1])
            .await
            .unwrap();
        store.attach_audio(r.id, 1, &[2, 2, 2]).await.unwrap();
        store.record(r.clone()).await.unwrap();
        assert_eq!(
            store.list(1).await.unwrap()[0].audio_bytes,
            PCM_RETENTION_BYTES
        );
        assert_eq!(store.audio(r.id, Some(1)).await.unwrap(), vec![2]);
    }

    async fn dictated(store: &Store, started_ms: u64, shaped: &str) -> DictationId {
        let r = row(started_ms, shaped);
        store.record(r.clone()).await.unwrap();
        r.id
    }

    #[tokio::test]
    async fn suggestion_flow_cooldown_never_ignores() {
        let tmp = Tmp::new();
        let store = Store::open(&tmp.0).await.unwrap();
        let mut dict = Dictionary::default();
        let day = 24 * 60 * 60 * 1000;
        set_now(&store, 100 * day);
        // first slip: stored, not due
        let d1 = dictated(&store, 1, "use github here").await;
        assert!(
            store
                .correction(d1, "use GitHub here")
                .await
                .unwrap()
                .is_empty()
        );
        // second within 7 days: due, one card
        set_now(&store, 103 * day);
        let d2 = dictated(&store, 2, "Github again").await;
        let sugs = store.correction(d2, "GitHub again").await.unwrap();
        assert_eq!(sugs.len(), 1);
        assert_eq!(
            (sugs[0].heard.as_str(), sugs[0].typed.as_str()),
            ("Github", "GitHub")
        );
        assert_eq!(sugs[0].occurrences, 2);
        assert_eq!(sugs[0].surface, Surface::Composer);
        assert!(!sugs[0].has_audio);
        // the same pair is not re-suggested while its card is pending
        let d3 = dictated(&store, 3, "github thrice").await;
        assert!(
            store
                .correction(d3, "GitHub thrice")
                .await
                .unwrap()
                .is_empty()
        );
        // Not now → cooldown 7 days
        store
            .answer(sugs[0].id, SuggestionAnswer::NotNow, &mut dict)
            .await
            .unwrap();
        assert!(
            store
                .answer(sugs[0].id, SuggestionAnswer::Add, &mut dict)
                .await
                .is_err()
        );
        set_now(&store, 106 * day);
        let d4 = dictated(&store, 4, "github four").await;
        assert!(
            store
                .correction(d4, "GitHub four")
                .await
                .unwrap()
                .is_empty()
        );
        set_now(&store, 111 * day);
        let d5 = dictated(&store, 5, "github five").await;
        let sugs = store.correction(d5, "GitHub five").await.unwrap();
        assert_eq!(sugs.len(), 1);
        assert_eq!(
            sugs[0].occurrences, 2,
            "d4, d5 are within the window; d3 fell out"
        );
        // Never → negative, permanent
        store
            .answer(sugs[0].id, SuggestionAnswer::Never, &mut dict)
            .await
            .unwrap();
        set_now(&store, 200 * day);
        let d6 = dictated(&store, 6, "github six").await;
        let d7 = dictated(&store, 7, "github seven").await;
        store.correction(d6, "GitHub six").await.unwrap();
        assert!(
            store
                .correction(d7, "GitHub seven")
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(dict.list(None).len(), 0);
        // Add → learned entry
        let d8 = dictated(&store, 8, "run clippy").await;
        let d9 = dictated(&store, 9, "run clippy").await;
        store.correction(d8, "run Clippy").await.unwrap();
        let sugs = store.correction(d9, "run Clippy").await.unwrap();
        store
            .answer(sugs[0].id, SuggestionAnswer::Add, &mut dict)
            .await
            .unwrap();
        let learned = dict.list(None);
        assert_eq!(learned.len(), 1);
        assert_eq!(learned[0].canonical, "Clippy");
        assert_eq!(learned[0].variants[0].heard, "clippy");
        assert_eq!(learned[0].source, Source::Learned);
        // 'added' pairs are not suggested again
        let d10 = dictated(&store, 10, "run clippy").await;
        assert!(
            store
                .correction(d10, "run Clippy")
                .await
                .unwrap()
                .is_empty()
        );
        // three ignored cards (unanswered past CARD_TIMEOUT_MS) end suggestions for the session
        for (i, word) in ["alpha", "bravo", "charlie"].iter().enumerate() {
            let now = (300 + i as u64) * day;
            set_now(&store, now);
            let a = dictated(&store, 20 + i as u64, &format!("say {word}")).await;
            let b = dictated(&store, 30 + i as u64, &format!("say {word}")).await;
            let up = word.to_uppercase();
            store.correction(a, &format!("say {up}")).await.unwrap();
            assert_eq!(
                store
                    .correction(b, &format!("say {up}"))
                    .await
                    .unwrap()
                    .len(),
                1
            );
        }
        set_now(&store, 400 * day);
        let a = dictated(&store, 40, "say delta").await;
        let b = dictated(&store, 41, "say delta").await;
        store.correction(a, "say DELTA").await.unwrap();
        assert!(store.correction(b, "say DELTA").await.unwrap().is_empty());
        assert_eq!(store.session.lock().ignores, 3);
    }

    #[test]
    fn recording_allowed_respects_privacy() {
        let cfg = HistoryConfig::default();
        assert!(Store::recording_allowed(&cfg, false));
        assert!(!Store::recording_allowed(&cfg, true));
        let off = HistoryConfig {
            enabled: false,
            ..cfg
        };
        assert!(!Store::recording_allowed(&off, false));
    }
}
