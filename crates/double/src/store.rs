//! `<data_dir>/double/double.sqlite` (126 §5): own file like `knowledge.sqlite`
//! and `tasks.sqlite` (the data dir is `Runtime::data_dir`, crates/runtime/src/lib.rs),
//! engine-local, WAL, sqlx migrations under `migrations/`. Plain row access
//! for every table; no business logic (the ladder, the labeller and the
//! trainer live in their own modules). Every string leaf that came from a
//! card — `card_text`, option labels, choices — passes
//! `tools::secrets::redact_value` before it is bound (57 §11's scrubber).

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context as _;
use chrono::{DateTime, Utc};
use half::f16;
use sqlx::Row as _;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};

use crate::class::CardKind;
use crate::types::{LabelSource, Outcome, RunState, Tier};

/// Directory under the data dir: the store, `models/`, `runs/`, `export/`.
pub const DIR: &str = "double";
/// The sqlite file under [`DIR`].
pub const DB: &str = "double.sqlite";
/// `versions.id` of the frozen zero-shot oracle (§0 item 3).
pub const BASELINE: &str = "baseline";
/// `card_text` cap (§5).
pub const CARD_TEXT_MAX: usize = 4 * 1024;

/// `<data_dir>/double/double.sqlite`.
pub fn path(data_dir: &Path) -> PathBuf {
    data_dir.join(DIR).join(DB)
}

/// Handle to the store. Cheap to clone.
#[derive(Clone)]
pub struct Store {
    pool: SqlitePool,
}

/// One `classes` row.
#[derive(Clone, Debug, PartialEq)]
pub struct ClassRow {
    /// The §3 key.
    pub key: String,
    /// The key's kind prefix.
    pub kind: CardKind,
    /// Current rung, 0..=3.
    pub rung: u8,
    /// Max rung for this class.
    pub cap: u8,
    /// Per-class opt-in for L2 on a mutating class.
    pub mutating_opt_in: bool,
    /// Examples seen.
    pub n: u32,
    /// JSON: answer histogram, `median_answer_ms`, `last_30d {n, acc, ece, overrides}`.
    pub stats: serde_json::Value,
    /// Last write.
    pub updated_at: DateTime<Utc>,
}

/// One `examples` row.
#[derive(Clone, Debug, PartialEq)]
pub struct ExampleRow {
    /// uuid v7.
    pub id: String,
    /// The class key.
    pub class: String,
    /// The card id as text.
    pub question_id: String,
    /// The card's session, when it has one.
    pub session_id: Option<String>,
    /// `root` | `subagent` | `desk:<agent>` | `routine:<name>`.
    pub requester: String,
    /// Scrubbed and cut to [`CARD_TEXT_MAX`] at write.
    pub card_text: String,
    /// Labels in card order; scrubbed at write.
    pub options: Vec<String>,
    /// Pooled encoder state; stored as f16 little-endian.
    pub embedding: Option<Vec<f32>>,
    /// When the card was raised.
    pub asked_at: DateTime<Utc>,
}

/// One `labels` row.
#[derive(Clone, Debug, PartialEq)]
pub struct LabelRow {
    /// The example labelled.
    pub example_id: String,
    /// Where the label came from.
    pub source: LabelSource,
    /// The option label chosen (scrubbed at write).
    pub choice: String,
    /// §6 weight.
    pub weight: f32,
    /// When the label landed.
    pub at: DateTime<Utc>,
}

/// One `predictions` row — the prequential ledger.
#[derive(Clone, Debug, PartialEq)]
pub struct PredictionRow {
    /// The example predicted.
    pub example_id: String,
    /// `versions.id`.
    pub version: String,
    /// Predicted option label.
    pub choice: String,
    /// Every option incl. `none`.
    pub probs: Vec<(String, f32)>,
    /// Top-1 probability.
    pub p_top: f32,
    /// The class's rung when predicted.
    pub rung_at_time: u8,
    /// The double answered the card (L2/L3).
    pub acted: bool,
    /// L2 grace end.
    pub grace_until: Option<DateTime<Utc>>,
    /// How it scored once labelled.
    pub outcome: Outcome,
    /// When written — before the owner saw the card.
    pub at: DateTime<Utc>,
}

/// One `versions` row.
#[derive(Clone, Debug, PartialEq)]
pub struct VersionRow {
    /// Version id; `baseline` for the zero-shot oracle.
    pub id: String,
    /// Which tier produced it.
    pub tier: Tier,
    /// Hash of the base checkpoint it was trained against.
    pub base_hash: String,
    /// `adapter.safetensors` (Tier B/C).
    pub adapter_path: Option<String>,
    /// `heads.json`.
    pub head_path: Option<String>,
    /// `knn.bin`.
    pub knn_path: Option<String>,
    /// JSON metrics from the gate.
    pub metrics: serde_json::Value,
    /// The labelled corpus range it was trained on.
    pub data_range: String,
    /// When training finished.
    pub trained_at: DateTime<Utc>,
    /// `local` | `node:<name>`.
    pub venue: String,
    /// Wall-clock seconds of training.
    pub duration_s: u32,
    /// The one version predictions use.
    pub active: bool,
}

/// One `runs` row.
#[derive(Clone, Debug, PartialEq)]
pub struct RunRow {
    /// Run id.
    pub id: String,
    /// Which tier.
    pub tier: Tier,
    /// `local` | `node:<name>`.
    pub venue: String,
    /// Where in the state machine.
    pub state: RunState,
    /// First transition.
    pub started_at: DateTime<Utc>,
    /// Terminal transition, when reached.
    pub ended_at: Option<DateTime<Utc>>,
    /// `promoted` / `rejected: <gate>` / an error.
    pub result: Option<String>,
    /// The run's log file, when kept.
    pub log_path: Option<String>,
}

impl Store {
    /// Open or create the file (parent dir included) and run the migrations.
    pub async fn open(path: &Path) -> anyhow::Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("mkdir {}", dir.display()))?;
        }
        let opts = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .foreign_keys(true)
            .busy_timeout(Duration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(opts)
            .await
            .with_context(|| format!("open {}", path.display()))?;
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .context("run migrations")?;
        Ok(Self { pool })
    }

    /// The underlying pool, for the modules that own the queries this file does not.
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    // --- classes --------------------------------------------------------

    /// Insert or fully rewrite a class row.
    pub async fn put_class(&self, c: &ClassRow) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO classes (key, kind, rung, cap, mutating_opt_in, n, stats, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(key) DO UPDATE SET kind = excluded.kind, rung = excluded.rung, cap = excluded.cap,
                 mutating_opt_in = excluded.mutating_opt_in, n = excluded.n, stats = excluded.stats,
                 updated_at = excluded.updated_at",
        )
        .bind(&c.key)
        .bind(c.kind.as_str())
        .bind(i64::from(c.rung))
        .bind(i64::from(c.cap))
        .bind(i32::from(c.mutating_opt_in))
        .bind(i64::from(c.n))
        .bind(c.stats.to_string())
        .bind(c.updated_at.to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// One class by key.
    pub async fn class(&self, key: &str) -> anyhow::Result<Option<ClassRow>> {
        let row = sqlx::query("SELECT * FROM classes WHERE key = ?")
            .bind(key)
            .fetch_optional(&self.pool)
            .await?;
        row.map(class_of).transpose()
    }

    /// Every class, by key.
    pub async fn classes(&self) -> anyhow::Result<Vec<ClassRow>> {
        sqlx::query("SELECT * FROM classes ORDER BY key")
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(class_of)
            .collect()
    }

    // --- examples -------------------------------------------------------

    /// Insert an example; `card_text` and `options` are scrubbed and the text
    /// cut to [`CARD_TEXT_MAX`] bytes (on a char boundary) here.
    pub async fn put_example(&self, e: &ExampleRow) -> anyhow::Result<()> {
        let mut text = scrub(&e.card_text);
        if text.len() > CARD_TEXT_MAX {
            let mut cut = CARD_TEXT_MAX;
            while !text.is_char_boundary(cut) {
                cut -= 1;
            }
            text.truncate(cut);
        }
        let options: Vec<String> = e.options.iter().map(|o| scrub(o)).collect();
        sqlx::query(
            "INSERT INTO examples (id, class, question_id, session_id, requester, card_text, options, embedding, asked_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&e.id)
        .bind(&e.class)
        .bind(&e.question_id)
        .bind(&e.session_id)
        .bind(&e.requester)
        .bind(text)
        .bind(serde_json::to_string(&options)?)
        .bind(e.embedding.as_deref().map(encode_f16))
        .bind(e.asked_at.to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// One example by id.
    pub async fn example(&self, id: &str) -> anyhow::Result<Option<ExampleRow>> {
        let row = sqlx::query("SELECT * FROM examples WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        row.map(example_of).transpose()
    }

    /// One example by the card it came from.
    pub async fn example_for_question(
        &self,
        question_id: &str,
    ) -> anyhow::Result<Option<ExampleRow>> {
        let row = sqlx::query("SELECT * FROM examples WHERE question_id = ?")
            .bind(question_id)
            .fetch_optional(&self.pool)
            .await?;
        row.map(example_of).transpose()
    }

    /// A class's examples, newest first, at most `limit`.
    pub async fn examples_of_class(
        &self,
        class: &str,
        limit: u32,
    ) -> anyhow::Result<Vec<ExampleRow>> {
        sqlx::query(
            "SELECT * FROM examples WHERE class = ? ORDER BY asked_at DESC, id DESC LIMIT ?",
        )
        .bind(class)
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(example_of)
        .collect()
    }

    // --- labels ---------------------------------------------------------

    /// Insert or rewrite the `(example, source)` label; `choice` is scrubbed.
    pub async fn put_label(&self, l: &LabelRow) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO labels (example_id, source, choice, weight, at) VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(example_id, source) DO UPDATE SET choice = excluded.choice, weight = excluded.weight,
                 at = excluded.at",
        )
        .bind(&l.example_id)
        .bind(l.source.as_str())
        .bind(scrub(&l.choice))
        .bind(f64::from(l.weight))
        .bind(l.at.to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// An example's labels, by source.
    pub async fn labels(&self, example_id: &str) -> anyhow::Result<Vec<LabelRow>> {
        sqlx::query("SELECT * FROM labels WHERE example_id = ? ORDER BY source")
            .bind(example_id)
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(label_of)
            .collect()
    }

    // --- predictions ----------------------------------------------------

    /// Insert or rewrite the `(example, version)` prediction; labels in
    /// `choice` / `probs` are scrubbed.
    pub async fn put_prediction(&self, p: &PredictionRow) -> anyhow::Result<()> {
        let probs: Vec<(String, f32)> = p.probs.iter().map(|(l, v)| (scrub(l), *v)).collect();
        sqlx::query(
            "INSERT INTO predictions (example_id, version, choice, probs, p_top, rung_at_time, acted, grace_until, outcome, at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(example_id, version) DO UPDATE SET choice = excluded.choice, probs = excluded.probs,
                 p_top = excluded.p_top, rung_at_time = excluded.rung_at_time, acted = excluded.acted,
                 grace_until = excluded.grace_until, outcome = excluded.outcome, at = excluded.at",
        )
        .bind(&p.example_id)
        .bind(&p.version)
        .bind(scrub(&p.choice))
        .bind(serde_json::to_string(&probs)?)
        .bind(f64::from(p.p_top))
        .bind(i64::from(p.rung_at_time))
        .bind(i32::from(p.acted))
        .bind(p.grace_until.map(|t| t.to_rfc3339()))
        .bind(p.outcome.as_str())
        .bind(p.at.to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// An example's predictions (one per version).
    pub async fn predictions(&self, example_id: &str) -> anyhow::Result<Vec<PredictionRow>> {
        sqlx::query("SELECT * FROM predictions WHERE example_id = ? ORDER BY version")
            .bind(example_id)
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(prediction_of)
            .collect()
    }

    /// A version's predictions since `since`, in time order (the curve's input).
    pub async fn predictions_of_version(
        &self,
        version: &str,
        since: DateTime<Utc>,
    ) -> anyhow::Result<Vec<PredictionRow>> {
        sqlx::query(
            "SELECT * FROM predictions WHERE version = ? AND at >= ? ORDER BY at, example_id",
        )
        .bind(version)
        .bind(since.to_rfc3339())
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(prediction_of)
        .collect()
    }

    // --- versions -------------------------------------------------------

    /// Insert or fully rewrite a version row.
    pub async fn put_version(&self, v: &VersionRow) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO versions (id, tier, base_hash, adapter_path, head_path, knn_path, metrics, data_range,
                                   trained_at, venue, duration_s, active)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET tier = excluded.tier, base_hash = excluded.base_hash,
                 adapter_path = excluded.adapter_path, head_path = excluded.head_path, knn_path = excluded.knn_path,
                 metrics = excluded.metrics, data_range = excluded.data_range, trained_at = excluded.trained_at,
                 venue = excluded.venue, duration_s = excluded.duration_s, active = excluded.active",
        )
        .bind(&v.id)
        .bind(v.tier.as_str())
        .bind(&v.base_hash)
        .bind(&v.adapter_path)
        .bind(&v.head_path)
        .bind(&v.knn_path)
        .bind(v.metrics.to_string())
        .bind(&v.data_range)
        .bind(v.trained_at.to_rfc3339())
        .bind(&v.venue)
        .bind(i64::from(v.duration_s))
        .bind(i32::from(v.active))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Every version, newest first.
    pub async fn versions(&self) -> anyhow::Result<Vec<VersionRow>> {
        sqlx::query("SELECT * FROM versions ORDER BY trained_at DESC, id DESC")
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(version_of)
            .collect()
    }

    /// The active version, if a trained one has been activated (`baseline` is never `active`).
    pub async fn active_version(&self) -> anyhow::Result<Option<VersionRow>> {
        let row = sqlx::query("SELECT * FROM versions WHERE active = 1 LIMIT 1")
            .fetch_optional(&self.pool)
            .await?;
        row.map(version_of).transpose()
    }

    /// Make `id` the one active version (one transaction).
    pub async fn activate_version(&self, id: &str) -> anyhow::Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("UPDATE versions SET active = 0 WHERE active = 1")
            .execute(&mut *tx)
            .await?;
        let n = sqlx::query("UPDATE versions SET active = 1 WHERE id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        anyhow::ensure!(n == 1, "version {id:?} not found");
        tx.commit().await?;
        Ok(())
    }

    // --- runs -----------------------------------------------------------

    /// Insert or fully rewrite a run row (every state transition rewrites it).
    pub async fn put_run(&self, r: &RunRow) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO runs (id, tier, venue, state, started_at, ended_at, result, log_path)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET tier = excluded.tier, venue = excluded.venue, state = excluded.state,
                 started_at = excluded.started_at, ended_at = excluded.ended_at, result = excluded.result,
                 log_path = excluded.log_path",
        )
        .bind(&r.id)
        .bind(r.tier.as_str())
        .bind(&r.venue)
        .bind(r.state.as_str())
        .bind(r.started_at.to_rfc3339())
        .bind(r.ended_at.map(|t| t.to_rfc3339()))
        .bind(&r.result)
        .bind(&r.log_path)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Every run, newest first, at most `limit`.
    pub async fn runs(&self, limit: u32) -> anyhow::Result<Vec<RunRow>> {
        sqlx::query("SELECT * FROM runs ORDER BY started_at DESC, id DESC LIMIT ?")
            .bind(i64::from(limit))
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(run_of)
            .collect()
    }

    // --- golden ---------------------------------------------------------

    /// Mark an example golden (idempotent).
    pub async fn put_golden(&self, example_id: &str) -> anyhow::Result<()> {
        sqlx::query("INSERT OR IGNORE INTO golden (example_id) VALUES (?)")
            .bind(example_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Every golden example id.
    pub async fn golden(&self) -> anyhow::Result<Vec<String>> {
        Ok(
            sqlx::query_scalar("SELECT example_id FROM golden ORDER BY example_id")
                .fetch_all(&self.pool)
                .await?,
        )
    }
}

/// `tools::secrets::redact_value` over one string.
fn scrub(s: &str) -> String {
    let mut v = serde_json::Value::String(s.to_owned());
    tools::secrets::redact_value(&mut v);
    match v {
        serde_json::Value::String(s) => s,
        other => other.to_string(),
    }
}

/// f32 → f16 little-endian bytes (§5: 2 bytes per dim).
pub fn encode_f16(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 2);
    for &x in v {
        out.extend_from_slice(&f16::from_f32(x).to_le_bytes());
    }
    out
}

/// Inverse of [`encode_f16`]; a trailing odd byte is dropped.
pub fn decode_f16(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(2)
        .map(|c| f16::from_le_bytes([c[0], c[1]]).to_f32())
        .collect()
}

fn dt(s: &str) -> anyhow::Result<DateTime<Utc>> {
    Ok(DateTime::parse_from_rfc3339(s)
        .with_context(|| format!("timestamp {s:?}"))?
        .with_timezone(&Utc))
}

fn opt_dt(s: Option<String>) -> anyhow::Result<Option<DateTime<Utc>>> {
    s.as_deref().map(dt).transpose()
}

fn class_of(r: sqlx::sqlite::SqliteRow) -> anyhow::Result<ClassRow> {
    let kind: String = r.get("kind");
    Ok(ClassRow {
        key: r.get("key"),
        kind: serde_json::from_value(serde_json::Value::String(kind.clone()))
            .with_context(|| format!("classes.kind {kind:?}"))?,
        rung: r.get::<i64, _>("rung") as u8,
        cap: r.get::<i64, _>("cap") as u8,
        mutating_opt_in: r.get::<i32, _>("mutating_opt_in") != 0,
        n: r.get::<i64, _>("n") as u32,
        stats: serde_json::from_str(&r.get::<String, _>("stats")).unwrap_or_default(),
        updated_at: dt(&r.get::<String, _>("updated_at"))?,
    })
}

fn example_of(r: sqlx::sqlite::SqliteRow) -> anyhow::Result<ExampleRow> {
    Ok(ExampleRow {
        id: r.get("id"),
        class: r.get("class"),
        question_id: r.get("question_id"),
        session_id: r.get("session_id"),
        requester: r.get("requester"),
        card_text: r.get("card_text"),
        options: serde_json::from_str(&r.get::<String, _>("options")).unwrap_or_default(),
        embedding: r
            .get::<Option<Vec<u8>>, _>("embedding")
            .map(|b| decode_f16(&b)),
        asked_at: dt(&r.get::<String, _>("asked_at"))?,
    })
}

fn label_of(r: sqlx::sqlite::SqliteRow) -> anyhow::Result<LabelRow> {
    let source: String = r.get("source");
    Ok(LabelRow {
        example_id: r.get("example_id"),
        source: LabelSource::parse(&source)
            .ok_or_else(|| anyhow::anyhow!("labels.source {source:?}"))?,
        choice: r.get("choice"),
        weight: r.get::<f64, _>("weight") as f32,
        at: dt(&r.get::<String, _>("at"))?,
    })
}

fn prediction_of(r: sqlx::sqlite::SqliteRow) -> anyhow::Result<PredictionRow> {
    let outcome: String = r.get("outcome");
    Ok(PredictionRow {
        example_id: r.get("example_id"),
        version: r.get("version"),
        choice: r.get("choice"),
        probs: serde_json::from_str(&r.get::<String, _>("probs")).unwrap_or_default(),
        p_top: r.get::<f64, _>("p_top") as f32,
        rung_at_time: r.get::<i64, _>("rung_at_time") as u8,
        acted: r.get::<i32, _>("acted") != 0,
        grace_until: opt_dt(r.get("grace_until"))?,
        outcome: Outcome::parse(&outcome)
            .ok_or_else(|| anyhow::anyhow!("predictions.outcome {outcome:?}"))?,
        at: dt(&r.get::<String, _>("at"))?,
    })
}

fn version_of(r: sqlx::sqlite::SqliteRow) -> anyhow::Result<VersionRow> {
    let tier: String = r.get("tier");
    Ok(VersionRow {
        id: r.get("id"),
        tier: Tier::parse(&tier).ok_or_else(|| anyhow::anyhow!("versions.tier {tier:?}"))?,
        base_hash: r.get("base_hash"),
        adapter_path: r.get("adapter_path"),
        head_path: r.get("head_path"),
        knn_path: r.get("knn_path"),
        metrics: serde_json::from_str(&r.get::<String, _>("metrics")).unwrap_or_default(),
        data_range: r.get("data_range"),
        trained_at: dt(&r.get::<String, _>("trained_at"))?,
        venue: r.get("venue"),
        duration_s: r.get::<i64, _>("duration_s") as u32,
        active: r.get::<i32, _>("active") != 0,
    })
}

fn run_of(r: sqlx::sqlite::SqliteRow) -> anyhow::Result<RunRow> {
    let (tier, state): (String, String) = (r.get("tier"), r.get("state"));
    Ok(RunRow {
        id: r.get("id"),
        tier: Tier::parse(&tier).ok_or_else(|| anyhow::anyhow!("runs.tier {tier:?}"))?,
        venue: r.get("venue"),
        state: RunState::parse(&state).ok_or_else(|| anyhow::anyhow!("runs.state {state:?}"))?,
        started_at: dt(&r.get::<String, _>("started_at"))?,
        ended_at: opt_dt(r.get("ended_at"))?,
        result: r.get("result"),
        log_path: r.get("log_path"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn fresh() -> (Store, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&path(dir.path())).await.unwrap();
        (store, dir)
    }

    fn now() -> DateTime<Utc> {
        // Second precision: RFC 3339 round-trips exactly.
        DateTime::from_timestamp(Utc::now().timestamp(), 0).unwrap()
    }

    fn class(key: &str) -> ClassRow {
        ClassRow {
            key: key.into(),
            kind: CardKind::Approval,
            rung: 0,
            cap: 1,
            mutating_opt_in: false,
            n: 0,
            stats: serde_json::json!({}),
            updated_at: now(),
        }
    }

    /// Every table: insert, select, upsert; the file is created with its
    /// parent dir and the `baseline` version row.
    #[tokio::test]
    async fn every_table_round_trips() {
        let (s, _dir) = fresh().await;
        let versions = s.versions().await.unwrap();
        assert_eq!(versions.len(), 1);
        assert_eq!(
            (
                versions[0].id.as_str(),
                versions[0].tier,
                versions[0].active
            ),
            (BASELINE, Tier::Baseline, false)
        );
        assert!(s.active_version().await.unwrap().is_none());

        let key = "approval:shell:mutating:git push:root";
        let mut c = class(key);
        s.put_class(&c).await.unwrap();
        c.rung = 1;
        c.n = 24;
        c.stats = serde_json::json!({ "median_answer_ms": 2100 });
        s.put_class(&c).await.unwrap();
        assert_eq!(s.class(key).await.unwrap(), Some(c.clone()));
        assert_eq!(s.classes().await.unwrap(), vec![c]);

        let emb: Vec<f32> = (0..1024).map(|i| (i as f32) / 1024.0 - 0.5).collect();
        let e = ExampleRow {
            id: uuid::Uuid::now_v7().to_string(),
            class: key.into(),
            question_id: "q-1".into(),
            session_id: Some("s-1".into()),
            requester: "root".into(),
            card_text: "shell: git push origin main".into(),
            options: vec!["Allow".into(), "Deny".into()],
            embedding: Some(emb.clone()),
            asked_at: now(),
        };
        s.put_example(&e).await.unwrap();
        let got = s.example(&e.id).await.unwrap().unwrap();
        assert_eq!(got.card_text, e.card_text);
        assert_eq!(got.options, e.options);
        let got_emb = got.embedding.unwrap();
        assert_eq!(got_emb.len(), 1024);
        assert!(got_emb.iter().zip(&emb).all(|(a, b)| (a - b).abs() < 1e-3));
        assert_eq!(
            s.example_for_question("q-1").await.unwrap().map(|e| e.id),
            Some(e.id.clone())
        );
        assert_eq!(s.examples_of_class(key, 50).await.unwrap().len(), 1);
        // The question id is unique: the same card never makes two examples.
        assert!(
            s.put_example(&ExampleRow {
                id: "other".into(),
                ..e.clone()
            })
            .await
            .is_err()
        );

        let l = LabelRow {
            example_id: e.id.clone(),
            source: LabelSource::Direct,
            choice: "Allow".into(),
            weight: 1.0,
            at: now(),
        };
        s.put_label(&l).await.unwrap();
        s.put_label(&LabelRow {
            source: LabelSource::Override,
            choice: "Deny".into(),
            weight: 3.0,
            ..l.clone()
        })
        .await
        .unwrap();
        let labels = s.labels(&e.id).await.unwrap();
        assert_eq!(labels.len(), 2);
        assert_eq!(labels[0], l);

        let p = PredictionRow {
            example_id: e.id.clone(),
            version: BASELINE.into(),
            choice: "Allow".into(),
            probs: vec![
                ("Allow".into(), 0.7),
                ("Deny".into(), 0.2),
                ("none".into(), 0.1),
            ],
            p_top: 0.7,
            rung_at_time: 0,
            acted: false,
            grace_until: None,
            outcome: Outcome::Unlabelled,
            at: now(),
        };
        s.put_prediction(&p).await.unwrap();
        let settled = PredictionRow {
            outcome: Outcome::Correct,
            grace_until: Some(now()),
            ..p.clone()
        };
        s.put_prediction(&settled).await.unwrap();
        assert_eq!(s.predictions(&e.id).await.unwrap(), vec![settled.clone()]);
        assert_eq!(
            s.predictions_of_version(BASELINE, now() - chrono::Duration::hours(1))
                .await
                .unwrap(),
            vec![settled]
        );

        let v = VersionRow {
            id: "v-2026-10-07".into(),
            tier: Tier::A,
            base_hash: "abc".into(),
            adapter_path: None,
            head_path: Some("models/v/heads.json".into()),
            knn_path: Some("models/v/knn.bin".into()),
            metrics: serde_json::json!({ "acc": 0.96 }),
            data_range: "2026-09-28..2026-10-07".into(),
            trained_at: now(),
            venue: "local".into(),
            duration_s: 42,
            active: false,
        };
        s.put_version(&v).await.unwrap();
        s.activate_version(&v.id).await.unwrap();
        assert_eq!(
            s.active_version().await.unwrap().map(|v| v.id),
            Some(v.id.clone())
        );
        assert!(s.activate_version("missing").await.is_err());
        assert_eq!(
            s.active_version().await.unwrap().map(|v| v.id),
            Some(v.id.clone()),
            "a failed activate keeps the old one"
        );

        let r = RunRow {
            id: "run-1".into(),
            tier: Tier::A,
            venue: "local".into(),
            state: RunState::Running,
            started_at: now(),
            ended_at: None,
            result: None,
            log_path: None,
        };
        s.put_run(&r).await.unwrap();
        let done = RunRow {
            state: RunState::Promoted,
            ended_at: Some(now()),
            result: Some("promoted".into()),
            ..r
        };
        s.put_run(&done).await.unwrap();
        assert_eq!(s.runs(10).await.unwrap(), vec![done]);

        s.put_golden(&e.id).await.unwrap();
        s.put_golden(&e.id).await.unwrap();
        assert_eq!(s.golden().await.unwrap(), vec![e.id.clone()]);
        // An unknown example is refused (foreign keys on).
        assert!(s.put_golden("nope").await.is_err());
    }

    /// `card_text` is cut at 4 KB on a char boundary; the embedding is optional.
    #[tokio::test]
    async fn card_text_is_capped() {
        let (s, _dir) = fresh().await;
        s.put_class(&class("ask:freeform:root")).await.unwrap();
        let text: String = std::iter::repeat_n('é', CARD_TEXT_MAX).collect(); // 2 bytes each
        let e = ExampleRow {
            id: "e".into(),
            class: "ask:freeform:root".into(),
            question_id: "q".into(),
            session_id: None,
            requester: "root".into(),
            card_text: text,
            options: vec![],
            embedding: None,
            asked_at: now(),
        };
        s.put_example(&e).await.unwrap();
        let got = s.example("e").await.unwrap().unwrap();
        assert_eq!(got.card_text.len(), CARD_TEXT_MAX);
        assert_eq!(got.card_text.chars().count(), CARD_TEXT_MAX / 2);
        assert!(got.embedding.is_none());
    }

    #[test]
    fn f16_round_trip() {
        let v = [0.0f32, 1.0, -0.5, 2.71875, 65504.0];
        let back = decode_f16(&encode_f16(&v));
        assert_eq!(back.len(), v.len());
        assert!(
            back.iter()
                .zip(&v)
                .all(|(a, b)| (a - b).abs() <= b.abs() * 1e-3 + 1e-6),
            "{back:?}"
        );
        assert!(decode_f16(&[1, 2, 3]).len() == 1);
    }
}

#[cfg(test)]
mod scrub_tests {
    use super::*;

    struct Fake;
    impl tools::secrets::Secrets for Fake {
        fn expand(&self, _: &mut serde_json::Value) -> Result<usize, String> {
            Ok(0)
        }
        fn redact<'a>(&self, text: &'a str) -> std::borrow::Cow<'a, str> {
            if text.contains("hunter2") {
                std::borrow::Cow::Owned(text.replace("hunter2", "{{secret:pw}}"))
            } else {
                std::borrow::Cow::Borrowed(text)
            }
        }
        fn redact_value(&self, v: &mut serde_json::Value) {
            if let serde_json::Value::String(s) = v
                && let std::borrow::Cow::Owned(c) = self.redact(s)
            {
                *s = c;
            }
        }
    }

    /// §5 / §14: every card string is scrubbed before it is bound — text,
    /// option labels, the label's choice, the prediction's choice and probs.
    #[tokio::test]
    async fn card_strings_are_scrubbed_at_write() {
        tools::secrets::install(std::sync::Arc::new(Fake));
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(&path(dir.path())).await.unwrap();
        let at = Utc::now();
        s.put_class(&ClassRow {
            key: "k".into(),
            kind: CardKind::Ask,
            rung: 0,
            cap: 2,
            mutating_opt_in: false,
            n: 0,
            stats: serde_json::json!({}),
            updated_at: at,
        })
        .await
        .unwrap();
        s.put_example(&ExampleRow {
            id: "e".into(),
            class: "k".into(),
            question_id: "q".into(),
            session_id: None,
            requester: "root".into(),
            card_text: "export PW=hunter2".into(),
            options: vec!["use hunter2".into(), "skip".into()],
            embedding: None,
            asked_at: at,
        })
        .await
        .unwrap();
        s.put_label(&LabelRow {
            example_id: "e".into(),
            source: LabelSource::Direct,
            choice: "use hunter2".into(),
            weight: 1.0,
            at,
        })
        .await
        .unwrap();
        s.put_prediction(&PredictionRow {
            example_id: "e".into(),
            version: BASELINE.into(),
            choice: "use hunter2".into(),
            probs: vec![("use hunter2".into(), 0.9), ("skip".into(), 0.1)],
            p_top: 0.9,
            rung_at_time: 0,
            acted: false,
            grace_until: None,
            outcome: Outcome::Unlabelled,
            at,
        })
        .await
        .unwrap();
        let e = s.example("e").await.unwrap().unwrap();
        assert_eq!(e.card_text, "export PW={{secret:pw}}");
        assert_eq!(e.options[0], "use {{secret:pw}}");
        assert_eq!(s.labels("e").await.unwrap()[0].choice, "use {{secret:pw}}");
        let p = &s.predictions("e").await.unwrap()[0];
        assert_eq!(p.choice, "use {{secret:pw}}");
        assert_eq!(p.probs[0].0, "use {{secret:pw}}");
    }
}
