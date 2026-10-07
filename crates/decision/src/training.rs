//! Training-data capture (57 §11, revised in §11.4): the *inputs* of every batch a consumer's
//! oracle handled, with the labels the pipeline produces for free — tier 2's subject, tier 3's
//! publication, the user's tap or dismiss — appended as [`Row::Amend`] rows keyed by the
//! example id. Off unless `decision.families.<prefix>.train` is on; separate from the hashed
//! decision log ([`crate::log`]), which is always on and carries no text.
//!
//! `<data_dir>/decisions/training/<prefix>/<YYYY-MM-DD>.jsonl`, one JSON object per line,
//! `{"row":"example",…}` / `{"row":"amend",…}`, append-only, secrets scrubbed with the
//! `secrets` placeholder scrubber ([`tools::secrets::redact_value`]) before writing. No session
//! id, no meeting title, no participant names beyond what the speech contains.

use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

use crate::gate::Mode;
use crate::question::{Candidate, Decision, Question, Reason};

/// The format every row declares (57 §11.1 item 5).
pub const SCHEMA: &str = "theuth.decision.v1";

/// Keys an [`Example`] and its amendments. Time-ordered (uuid v7, like every other id here —
/// §11.1's "ulid" without a new dependency).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(transparent)]
pub struct ExampleId(pub uuid::Uuid);

impl ExampleId {
    /// A fresh id.
    pub fn new() -> Self {
        ExampleId(uuid::Uuid::now_v7())
    }
}

impl Default for ExampleId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for ExampleId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// One line of a day file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "row", rename_all = "snake_case")]
pub enum Row {
    /// A batch the oracle handled, written when its verdict is produced.
    Example(Box<Example>),
    /// A label that arrived later, keyed by the example id.
    Amend(Amend),
}

/// One batch as the oracle saw it, and what every tier said about it (57 §11.4 item 1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Example {
    /// [`SCHEMA`].
    pub schema: String,
    /// Key for the amend rows.
    pub id: ExampleId,
    /// When the verdict was produced.
    pub ts: DateTime<Utc>,
    /// The family prefix (`meeting`).
    pub family: String,
    /// Harness version.
    pub harness: String,
    /// The checkpoint's identity (`AskRow::model_sha`); filled by the engine.
    pub model_sha: String,
    /// Thresholds + model of the first tier-1 question.
    pub gate_version: u32,
    /// The switch position when it ran.
    pub mode: Mode,
    /// The state as the oracle read it (`them:`/`me:` lines). Truncation is a hard error in the
    /// engine, so this is what the model saw whenever a question ran.
    pub text: String,
    /// Step 0's deterministic mentions.
    pub mentions: Vec<String>,
    /// Every tier-1 ask of the batch, in order.
    pub questions: Vec<Asked>,
    /// Tier 2's answer or failure, when it ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier2: Option<Tier2>,
    /// What the oracle did with the batch.
    pub verdict: Verdict,
}

impl Example {
    /// A new example for `family`; `harness` is this build, `model_sha` is left for the engine.
    pub fn new(
        family: &str,
        mode: Mode,
        text: String,
        mentions: Vec<String>,
        questions: Vec<Asked>,
        tier2: Option<Tier2>,
        verdict: Verdict,
    ) -> Self {
        Example {
            schema: SCHEMA.to_owned(),
            id: ExampleId::new(),
            ts: Utc::now(),
            family: family.to_owned(),
            harness: env!("CARGO_PKG_VERSION").to_owned(),
            model_sha: String::new(),
            gate_version: questions.first().map_or(0, |q| q.gate_version),
            mode,
            text,
            mentions,
            questions,
            tier2,
            verdict,
        }
    }
}

/// One tier-1 question as rendered, with the oracle's numbers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Asked {
    /// Family.
    pub family: String,
    /// `choice` | `bool` | `score`.
    #[serde(rename = "type")]
    pub kind: String,
    /// The instructions, or the `Bool`'s statement.
    pub instructions: String,
    /// Candidate ids *and titles* (`Choice`); the level texts under `0..` (`Score`); empty (`Bool`).
    pub candidates: Vec<Candidate>,
    /// Calibrated probability per label, as the engine returned them.
    pub probabilities: BTreeMap<String, f32>,
    /// `accept` / `escalate` / `shadow`.
    pub disposition: String,
    /// The escalation reason, through a shadow wrapper.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<Reason>,
    /// Thresholds + model at the time.
    pub gate_version: u32,
    /// 1 = the oracle.
    pub tier: u8,
    /// Engine wall clock.
    pub latency_ms: u64,
}

impl Asked {
    /// From the question as asked (before the engine's `none`) and its decision.
    pub fn new(q: &Question, d: &Decision) -> Self {
        let (kind, instructions, candidates) = match q {
            Question::Choice {
                instructions,
                candidates,
                ..
            } => ("choice", instructions.clone(), candidates.clone()),
            Question::Bool { statement, .. } => ("bool", statement.clone(), Vec::new()),
            Question::Score {
                instructions,
                levels,
                ..
            } => (
                "score",
                instructions.clone(),
                levels
                    .iter()
                    .enumerate()
                    .map(|(i, text)| Candidate {
                        id: i.to_string(),
                        text: text.clone(),
                    })
                    .collect(),
            ),
        };
        Asked {
            family: d.family.0.clone(),
            kind: kind.to_owned(),
            instructions,
            candidates,
            probabilities: d.probabilities.iter().cloned().collect(),
            disposition: d.disposition.kind().to_owned(),
            reason: d.disposition.reason().cloned(),
            gate_version: d.gate_version,
            tier: d.tier,
            latency_ms: d.latency.as_millis() as u64,
        }
    }
}

/// Tier 2's answer (`decision::subject`) or why there is none.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Tier2 {
    /// The parsed answer.
    Answer {
        /// The named subject; `None` = nothing specific.
        subject: Option<String>,
        /// One of the offered candidate ids.
        cached: Option<String>,
        /// Worth a researched brief.
        lookup: bool,
    },
    /// Timeout, provider error, invalid answer.
    Error {
        /// The error text.
        error: String,
    },
}

/// A published brief: the cache row (`None` for a brief researched in the turn) and its title.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Brief {
    /// Cache row id.
    pub id: Option<String>,
    /// Title.
    pub title: String,
}

/// What the oracle did with the batch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Verdict {
    /// No model turn.
    Skip,
    /// A cached brief published (would be, in shadow).
    Publish {
        /// Cache row id.
        id: String,
        /// Title.
        title: String,
    },
    /// Tier 3, with why.
    Model {
        /// `not small talk`, `subject "…", no cached brief`, …
        reason: String,
    },
}

/// A later label for one example.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Amend {
    /// The example.
    pub id: ExampleId,
    /// When.
    pub ts: DateTime<Utc>,
    /// Which label.
    #[serde(flatten)]
    pub what: Amendment,
}

impl Amend {
    /// `what` for `id`, now.
    pub fn new(id: ExampleId, what: Amendment) -> Self {
        Amend {
            id,
            ts: Utc::now(),
            what,
        }
    }
}

/// `outcome: { published: {id, title} | null }` after the model turn; `user: tapped | dismissed`
/// on the brief; `label: { choice }` from `decide_label` (92 §6) for a `tool.<task>` example.
/// `ignored` is never written — the export derives it from the absence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Amendment {
    /// Tier 3's outcome.
    Outcome {
        /// What the turn published, if anything.
        published: Option<Brief>,
    },
    /// The user's reaction to the brief.
    User(Reaction),
    /// The right key for a `decide_many` row, from the calling model or the user's correction.
    Label {
        /// One of the row's keys, or `none`.
        choice: String,
    },
}

/// What the user did with a published brief.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reaction {
    /// Tapped: saw the brief.
    Tapped,
    /// Dismissed it.
    Dismissed,
    /// Neither (derived at export, never written).
    Ignored,
}

/// The row, scrubbed: every string leaf through the `secrets` placeholder scrubber.
pub fn scrubbed(row: &Row) -> serde_json::Result<serde_json::Value> {
    let mut v = serde_json::to_value(row)?;
    tools::secrets::redact_value(&mut v);
    Ok(v)
}

/// The per-family day files under `<data_dir>/decisions/training`.
#[derive(Clone, Debug)]
pub struct Store {
    dir: PathBuf,
}

/// What `GetTrain { prefix }` and the `decision_training` RPC answer (57 §11.4 item 3).
#[derive(
    Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct TrainingInfo {
    /// `decision.families.<prefix>.train`.
    pub enabled: bool,
    /// `decision.families.<prefix>.share` (57 §7.1): consent to upload; persisted now, read by
    /// the step-11 uploader. Never `true` while `enabled` is `false`.
    pub share: bool,
    /// Example rows in today's file.
    pub examples_today: u64,
    /// Example rows in every file.
    pub examples_total: u64,
    /// Bytes on disk for this prefix.
    pub bytes: u64,
    /// The prefix's directory (may not exist yet).
    pub dir: String,
}

impl Store {
    /// `<data_dir>/decisions/training`.
    pub fn new(dir: PathBuf) -> Self {
        Store { dir }
    }

    /// The prefix's directory.
    pub fn family_dir(&self, prefix: &str) -> PathBuf {
        self.dir.join(prefix)
    }

    fn day_file(dir: &Path, date: NaiveDate) -> PathBuf {
        dir.join(format!("{}.jsonl", date.format("%Y-%m-%d")))
    }

    /// Append one scrubbed row to today's file, creating the directory on first use.
    pub fn append(&self, prefix: &str, row: &Row) -> std::io::Result<()> {
        let dir = self.family_dir(prefix);
        std::fs::create_dir_all(&dir)?;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(Self::day_file(&dir, chrono::Local::now().date_naive()))?;
        let mut line = serde_json::to_vec(&scrubbed(row)?)?;
        line.push(b'\n');
        f.write_all(&line)
    }

    /// Remove every file of `prefix` (57 §7.1 "Delete local data"); the switch is untouched.
    /// Nothing to delete is not an error.
    pub fn delete(&self, prefix: &str) -> std::io::Result<()> {
        match std::fs::remove_dir_all(self.family_dir(prefix)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }

    /// The day files of `prefix` from `since` on (all when `None`), oldest first; none when the
    /// directory does not exist.
    pub fn files(
        &self,
        prefix: &str,
        since: Option<NaiveDate>,
    ) -> std::io::Result<Vec<(NaiveDate, PathBuf)>> {
        let dir = self.family_dir(prefix);
        let mut out = Vec::new();
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(e) => return Err(e),
        };
        for entry in entries {
            let path = entry?.path();
            let Some(date) = path
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_suffix(".jsonl"))
                .and_then(|n| NaiveDate::parse_from_str(n, "%Y-%m-%d").ok())
            else {
                continue;
            };
            if since.is_none_or(|s| date >= s) {
                out.push((date, path));
            }
        }
        out.sort();
        Ok(out)
    }

    /// Every row of `prefix` from `since` on, file order. Unreadable lines are skipped with a warning.
    pub fn read(&self, prefix: &str, since: Option<NaiveDate>) -> std::io::Result<Vec<Row>> {
        let mut rows = Vec::new();
        for (_, path) in self.files(prefix, since)? {
            for line in std::io::BufReader::new(std::fs::File::open(&path)?).lines() {
                match serde_json::from_str::<Row>(&line?) {
                    Ok(r) => rows.push(r),
                    Err(e) => tracing::warn!(
                        "decision training: unreadable row in {}: {e}",
                        path.display()
                    ),
                }
            }
        }
        Ok(rows)
    }

    /// Counts for [`TrainingInfo`]: examples today / in every file, bytes on disk.
    pub fn info(&self, prefix: &str, enabled: bool, share: bool) -> TrainingInfo {
        let today = chrono::Local::now().date_naive();
        let mut info = TrainingInfo {
            enabled,
            share,
            examples_today: 0,
            examples_total: 0,
            bytes: 0,
            dir: self.family_dir(prefix).display().to_string(),
        };
        for (date, path) in self.files(prefix, None).unwrap_or_default() {
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            info.bytes += text.len() as u64;
            // Only the tag is parsed: the scrubber re-serialises through `serde_json::Value`,
            // whose maps sort keys, so `row` is not the first key on disk.
            #[derive(Deserialize)]
            struct Tag {
                row: String,
            }
            let n = text
                .lines()
                .filter(|l| serde_json::from_str::<Tag>(l).is_ok_and(|t| t.row == "example"))
                .count() as u64;
            info.examples_total += n;
            if date == today {
                info.examples_today += n;
            }
        }
        info
    }
}

// ---- Export (57 §11.4 item 4) -------------------------------------------------------------

/// One example with its amendments folded in (`amend` rows, last one per kind wins).
#[derive(Clone, Debug, PartialEq)]
pub struct Folded {
    /// The example.
    pub example: Example,
    /// Tier 3's outcome, when a turn ran: what it published.
    pub outcome: Option<Option<Brief>>,
    /// The user's reaction, when any.
    pub user: Option<Reaction>,
    /// `decide_label`'s key, when any (92 §6).
    pub label: Option<String>,
}

/// Fold `example` + `amend` rows by id, file order kept; an amend without its example is dropped.
pub fn fold(rows: Vec<Row>) -> Vec<Folded> {
    let mut out: Vec<Folded> = Vec::new();
    for row in rows {
        match row {
            Row::Example(e) => out.push(Folded {
                example: *e,
                outcome: None,
                user: None,
                label: None,
            }),
            Row::Amend(a) => {
                if let Some(f) = out.iter_mut().rev().find(|f| f.example.id == a.id) {
                    match a.what {
                        Amendment::Outcome { published } => f.outcome = Some(published),
                        Amendment::User(r) => f.user = Some(r),
                        Amendment::Label { choice } => f.label = Some(choice),
                    }
                }
            }
        }
    }
    out
}

/// The bench fixture's `kind` (57 §3.5 / §11.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// Greetings, scheduling, goodbyes: nothing to look up.
    SmallTalk,
    /// A subject was named or a brief published.
    NamedSubject,
    /// Neither settled.
    General,
}

/// Who produced the label (57 §11.4 item 1): the trainer chooses what to trust.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// The user tapped or dismissed the brief.
    User,
    /// A model turn ran and its publication is the label.
    Tier3,
    /// Tier 2's subject answer is the label.
    Tier2,
    /// Only the oracle's own verdict (a tier-1 skip or publish, nothing else heard).
    OracleOnly,
}

/// The labels derived from one folded example.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Label {
    /// See [`Kind`].
    pub kind: Kind,
    /// What was published: tier 3's outcome when a turn ran, else the oracle's own publish.
    pub published: Option<Brief>,
    /// The brief question's truth: the candidate id (`b1..`) of the published cached brief,
    /// confirmed by publication or a tap; `None` = `none` (or the user dismissed it).
    pub brief: Option<String>,
    /// See [`Source`].
    pub source: Source,
}

/// The label derivation as implemented: `published` = the outcome amend when present, else a
/// `publish` verdict; `named_subject` ⇐ a tier-2 subject ∨ a publication; `small_talk` ⇐ tier 2
/// said `subject: null` (or tier 1 skipped without tier 2) ∧ nothing published; else `general`.
/// `brief` = the published cached brief's candidate id (matched by title among the brief
/// question's candidates), cleared by a dismiss. `source` = user > tier3 > tier2 > oracle_only.
pub fn label(f: &Folded) -> Label {
    let e = &f.example;
    let published = match (&f.outcome, &e.verdict) {
        (Some(o), _) => o.clone(),
        (None, Verdict::Publish { id, title }) => Some(Brief {
            id: Some(id.clone()),
            title: title.clone(),
        }),
        (None, _) => None,
    };
    let subject = match &e.tier2 {
        Some(Tier2::Answer { subject, .. }) => Some(subject.as_deref()),
        _ => None,
    };
    let kind = match (&published, subject) {
        (Some(_), _) | (None, Some(Some(_))) => Kind::NamedSubject,
        (None, Some(None)) => Kind::SmallTalk,
        (None, None) if e.tier2.is_none() && e.verdict == Verdict::Skip => Kind::SmallTalk,
        (None, None) => Kind::General,
    };
    let brief = match (f.user, &published) {
        (Some(Reaction::Dismissed), _) | (_, None) => None,
        (_, Some(b)) if b.id.is_none() => None,
        (_, Some(b)) => e
            .questions
            .iter()
            .filter(|q| q.kind == "choice")
            .flat_map(|q| &q.candidates)
            .find(|c| c.text == b.title)
            .map(|c| c.id.clone()),
    };
    let source = match (f.user, f.outcome.is_some(), &e.tier2) {
        (Some(_), _, _) => Source::User,
        (None, true, _) => Source::Tier3,
        (None, false, Some(Tier2::Answer { .. })) => Source::Tier2,
        (None, false, _) => Source::OracleOnly,
    };
    Label {
        kind,
        published,
        brief,
        source,
    }
}

/// Output shape of [`export`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// `crates/decision/tests/fixtures/meeting-batches.json` — what the phrasing bench reads.
    Fixture,
    /// laya's `typed-decisions` rows: `state`, `questions`, `gold`, `<q>__label`.
    Laya,
}

impl std::str::FromStr for Format {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "fixture" => Ok(Format::Fixture),
            "laya" => Ok(Format::Laya),
            other => Err(format!("unknown format {other:?}: fixture | laya")),
        }
    }
}

/// The phrasing-bench fixture shape: `{ _doc, batches: [{ id, source, kind, state, published,
/// expected_brief, candidates, mentions, subject, cached, user }] }`. `source` is the label's
/// origin; `published` is `[{ title, cached }]` like the hand-built fixture; `candidates` are
/// the brief question's (`b1..` ids, titles, no aliases).
pub fn fixture(folded: &[Folded]) -> serde_json::Value {
    let batches: Vec<serde_json::Value> = folded
        .iter()
        .map(|f| {
            let l = label(f);
            let e = &f.example;
            let candidates: Vec<serde_json::Value> = e
                .questions
                .iter()
                .filter(|q| q.kind == "choice")
                .flat_map(|q| &q.candidates)
                .map(|c| serde_json::json!({ "id": c.id, "title": c.text, "aliases": [] }))
                .collect();
            let (subject, cached) = match &e.tier2 {
                Some(Tier2::Answer { subject, cached, .. }) => (subject.clone(), cached.clone()),
                _ => (None, None),
            };
            serde_json::json!({
                "id": e.id,
                "source": l.source,
                "kind": l.kind,
                "state": e.text,
                "published": l.published.as_ref().map(|b| vec![serde_json::json!({ "title": b.title, "cached": b.id })]),
                "expected_brief": l.brief,
                "candidates": candidates,
                "mentions": e.mentions,
                "subject": subject,
                "cached": cached,
                "user": f.user.unwrap_or(Reaction::Ignored),
            })
        })
        .collect();
    serde_json::json!({
        "_doc": format!(
            "57 §11.4 export of captured {SCHEMA} rows: `kind` and `expected_brief` derived from tier 2 / tier 3 / the user (`source`), `state` as the oracle read it, `candidates` = the brief question's b1.. ids and titles. Run the bench on it: cargo run --release -p decision --example phrasing -- --fixture FILE"
        ),
        "batches": batches,
    })
}

/// laya `typed-decisions` rows (the `LocalLLaMA/typed-decisions` columns: `id`, `workflow`,
/// `state`, `questions`, `gold`, `<question>__label`): one row per example, `kind` as a `noul`
/// (true = small talk), `brief` as a `choice` over the candidates + `none`, gold one-hot on
/// the derived label.
pub fn laya(folded: &[Folded]) -> serde_json::Value {
    let rows: Vec<serde_json::Value> = folded
        .iter()
        .map(|f| {
            let l = label(f);
            let e = &f.example;
            let mut questions = serde_json::Map::new();
            let mut gold = serde_json::Map::new();
            let mut row = serde_json::Map::new();
            for q in &e.questions {
                let name = q.family.rsplit('.').next().unwrap_or(&q.family).to_owned();
                match q.kind.as_str() {
                    "bool" => {
                        let truth = l.kind == Kind::SmallTalk;
                        questions.insert(name.clone(), serde_json::json!({ "type": "noul", "instructions": q.instructions }));
                        gold.insert(name.clone(), serde_json::json!({ "label": truth, "probability_true": if truth { 1.0 } else { 0.0 } }));
                        row.insert(format!("{name}__label"), serde_json::json!(truth));
                    }
                    "choice" => {
                        let mut criteria = serde_json::Map::new();
                        for c in &q.candidates {
                            criteria.insert(c.id.clone(), serde_json::json!(c.text));
                        }
                        criteria.insert(crate::engine::NONE_ID.to_owned(), serde_json::json!("no listed brief fits"));
                        let truth = l.brief.clone().unwrap_or_else(|| crate::engine::NONE_ID.to_owned());
                        let probabilities: serde_json::Map<String, serde_json::Value> = criteria
                            .keys()
                            .map(|k| (k.clone(), serde_json::json!(if *k == truth { 1.0 } else { 0.0 })))
                            .collect();
                        questions.insert(name.clone(), serde_json::json!({ "type": "choice", "instructions": q.instructions, "criteria": criteria }));
                        gold.insert(name.clone(), serde_json::json!({ "label": truth, "probabilities": probabilities }));
                        row.insert(format!("{name}__label"), serde_json::json!(truth));
                    }
                    _ => {}
                }
            }
            row.insert("id".into(), serde_json::json!(e.id));
            row.insert("workflow".into(), serde_json::json!(e.family));
            row.insert("state".into(), serde_json::json!(e.text));
            row.insert("questions".into(), serde_json::Value::Object(questions));
            row.insert("gold".into(), serde_json::Value::Object(gold));
            row.insert("label_source".into(), serde_json::json!(l.source));
            serde_json::Value::Object(row)
        })
        .collect();
    serde_json::Value::Array(rows)
}

/// `t --decision-export <prefix> [--since DATE] [--format fixture|laya]`: the captured rows of
/// `prefix` from `since` on, folded and shaped.
pub fn export(
    store: &Store,
    prefix: &str,
    since: Option<NaiveDate>,
    format: Format,
) -> std::io::Result<serde_json::Value> {
    let folded = fold(store.read(prefix, since)?);
    Ok(match format {
        Format::Fixture => fixture(&folded),
        Format::Laya => laya(&folded),
    })
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::sync::Arc;
    use std::time::Duration;

    use super::*;
    use crate::question::{Disposition, LogId};

    fn decision(family: &str, choice: &str, probs: &[(&str, f32)], disp: Disposition) -> Decision {
        Decision {
            family: family.into(),
            choice: choice.into(),
            probabilities: probs.iter().map(|(l, p)| ((*l).to_owned(), *p)).collect(),
            confidence: 0.5,
            act_probability: 0.1,
            disposition: disp,
            latency: Duration::from_millis(80),
            gate_version: 7,
            log_id: LogId::new(),
            tier: 1,
            calibrated: true,
            truncated: false,
            embedding: None,
        }
    }

    pub(super) fn example(text: &str) -> Example {
        let kind_q = Question::Bool {
            statement: "The speakers are only greeting".into(),
            state: text.into(),
        };
        let kind = decision(
            "meeting.kind",
            "false",
            &[("false", 0.9), ("true", 0.1)],
            Disposition::Accept,
        )
        .shadowed();
        let brief_q = Question::Choice {
            instructions: "which cached brief".into(),
            candidates: vec![
                Candidate {
                    id: "b1".into(),
                    text: "SRE-2386".into(),
                },
                Candidate {
                    id: "b2".into(),
                    text: "DR cutover".into(),
                },
            ],
            state: text.into(),
        };
        let brief = decision(
            "meeting.brief",
            "none",
            &[("b1", 0.3), ("b2", 0.2), ("none", 0.5)],
            Disposition::Escalate(Reason::LowMargin),
        );
        Example::new(
            "meeting",
            Mode::Shadow,
            text.to_owned(),
            vec!["Planet Nine".into()],
            vec![Asked::new(&kind_q, &kind), Asked::new(&brief_q, &brief)],
            Some(Tier2::Answer {
                subject: Some("Planet Nine".into()),
                cached: None,
                lookup: true,
            }),
            Verdict::Model {
                reason: "subject \"Planet Nine\", no cached brief".into(),
            },
        )
    }

    #[test]
    fn rows_round_trip_and_carry_the_schema() {
        let ex = example("them: about Planet Nine");
        let row = Row::Example(Box::new(ex.clone()));
        let line = serde_json::to_string(&row).unwrap();
        assert!(line.starts_with("{\"row\":\"example\""), "{line}");
        assert!(line.contains("\"schema\":\"theuth.decision.v1\""));
        assert!(line.contains("\"type\":\"bool\"") && line.contains("\"type\":\"choice\""));
        assert!(
            line.contains("\"text\":\"SRE-2386\""),
            "titles, not only ids: {line}"
        );
        assert!(
            line.contains("\"reason\":{\"reason\":\"low_margin\"}"),
            "{line}"
        );
        assert_eq!(serde_json::from_str::<Row>(&line).unwrap(), row);
        assert_eq!(ex.gate_version, 7);
        assert_eq!(ex.harness, env!("CARGO_PKG_VERSION"));
        assert_eq!(ex.questions[0].probabilities["true"], 0.1);
        assert_eq!(ex.questions[0].disposition, "shadow");

        for what in [
            Amendment::Outcome {
                published: Some(Brief {
                    id: Some("row-1".into()),
                    title: "Planet Nine".into(),
                }),
            },
            Amendment::Outcome { published: None },
            Amendment::User(Reaction::Tapped),
        ] {
            let row = Row::Amend(Amend::new(ex.id, what));
            let line = serde_json::to_string(&row).unwrap();
            assert!(line.starts_with("{\"row\":\"amend\""), "{line}");
            assert_eq!(serde_json::from_str::<Row>(&line).unwrap(), row);
        }
        let line = serde_json::to_string(&Row::Amend(Amend::new(
            ex.id,
            Amendment::User(Reaction::Dismissed),
        )))
        .unwrap();
        assert!(line.contains("\"user\":\"dismissed\""), "{line}");
        let line = serde_json::to_string(&Row::Amend(Amend::new(
            ex.id,
            Amendment::Outcome { published: None },
        )))
        .unwrap();
        assert!(line.contains("\"outcome\":{\"published\":null}"), "{line}");
        // Tier 2's failure shape.
        let t: Tier2 = serde_json::from_str(r#"{"error":"timed out"}"#).unwrap();
        assert_eq!(
            t,
            Tier2::Error {
                error: "timed out".into()
            }
        );
        let t: Tier2 =
            serde_json::from_str(r#"{"subject":null,"cached":null,"lookup":false}"#).unwrap();
        assert_eq!(
            t,
            Tier2::Answer {
                subject: None,
                cached: None,
                lookup: false
            }
        );
    }

    /// A fake vault that knows one value.
    struct Fake;

    impl tools::secrets::Secrets for Fake {
        fn expand(&self, _: &mut serde_json::Value) -> Result<usize, String> {
            Ok(0)
        }
        fn redact<'a>(&self, text: &'a str) -> Cow<'a, str> {
            if text.contains("hunter22hunter22") {
                Cow::Owned(text.replace("hunter22hunter22", "{{secret:PASS}}"))
            } else {
                Cow::Borrowed(text)
            }
        }
        fn redact_value(&self, v: &mut serde_json::Value) {
            match v {
                serde_json::Value::String(s) => {
                    if let Cow::Owned(o) = self.redact(s) {
                        *s = o;
                    }
                }
                serde_json::Value::Array(a) => a.iter_mut().for_each(|v| self.redact_value(v)),
                serde_json::Value::Object(m) => m.values_mut().for_each(|v| self.redact_value(v)),
                _ => {}
            }
        }
    }

    #[test]
    fn store_writes_per_day_append_only_and_scrubbed() {
        tools::secrets::install(Arc::new(Fake));
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path().join("training"));
        // Nothing written, nothing created.
        assert!(store.files("meeting", None).unwrap().is_empty());
        assert_eq!(store.info("meeting", false, false).examples_total, 0);
        store.delete("meeting").unwrap();
        assert!(!tmp.path().join("training").exists());

        let ex = example("me: the password is hunter22hunter22, don't tell");
        store
            .append("meeting", &Row::Example(Box::new(ex.clone())))
            .unwrap();
        store
            .append(
                "meeting",
                &Row::Amend(Amend::new(ex.id, Amendment::User(Reaction::Tapped))),
            )
            .unwrap();
        let files = store.files("meeting", None).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].0, chrono::Local::now().date_naive());
        let text = std::fs::read_to_string(&files[0].1).unwrap();
        assert_eq!(text.lines().count(), 2, "{text}");
        assert!(!text.contains("hunter22hunter22"), "{text}");
        assert!(
            text.contains("the password is {{secret:PASS}}, don't tell"),
            "{text}"
        );
        // Appending keeps what was there.
        store
            .append("meeting", &Row::Example(Box::new(example("them: hi"))))
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(&files[0].1)
                .unwrap()
                .lines()
                .count(),
            3
        );
        let rows = store.read("meeting", None).unwrap();
        assert_eq!(rows.len(), 3);
        assert!(matches!(&rows[1], Row::Amend(a) if a.id == ex.id));
        let info = store.info("meeting", true, true);
        assert_eq!(
            (
                info.enabled,
                info.share,
                info.examples_today,
                info.examples_total
            ),
            (true, true, 2, 2)
        );
        assert_eq!(info.bytes, std::fs::metadata(&files[0].1).unwrap().len());
        assert!(info.dir.ends_with("training/meeting"));
        // `since` after today → nothing; another prefix → nothing.
        assert!(
            store
                .files("meeting", Some(files[0].0.succ_opt().unwrap()))
                .unwrap()
                .is_empty()
        );
        assert!(store.read("browser", None).unwrap().is_empty());
        // 57 §7.1 "Delete local data": the prefix's files go, nothing else does.
        store
            .append("browser", &Row::Example(Box::new(example("x"))))
            .unwrap();
        store.delete("meeting").unwrap();
        assert!(!tmp.path().join("training/meeting").exists());
        assert_eq!(store.info("meeting", true, false).examples_total, 0);
        assert_eq!(store.info("browser", true, false).examples_total, 1);
    }

    fn folded(
        tier2: Option<Tier2>,
        verdict: Verdict,
        outcome: Option<Option<Brief>>,
        user: Option<Reaction>,
    ) -> Folded {
        let mut example = example("them: some talk");
        example.tier2 = tier2;
        example.verdict = verdict;
        Folded {
            example,
            outcome,
            user,
            label: None,
        }
    }

    fn answer(subject: Option<&str>, cached: Option<&str>) -> Option<Tier2> {
        Some(Tier2::Answer {
            subject: subject.map(str::to_owned),
            cached: cached.map(str::to_owned),
            lookup: subject.is_some() && cached.is_none(),
        })
    }

    fn brief(id: Option<&str>, title: &str) -> Brief {
        Brief {
            id: id.map(str::to_owned),
            title: title.into(),
        }
    }

    #[test]
    fn label_derivation_table() {
        let model = |r: &str| Verdict::Model { reason: r.into() };
        let publish = || Verdict::Publish {
            id: "row-1".into(),
            title: "SRE-2386".into(),
        };
        // small_talk: tier 2 said null and the turn published nothing (tier3 source).
        let l = label(&folded(answer(None, None), Verdict::Skip, Some(None), None));
        assert_eq!(
            (l.kind, l.brief.as_deref(), l.source, l.published),
            (Kind::SmallTalk, None, Source::Tier3, None)
        );
        // small_talk: a tier-1 skip, nothing else heard (oracle_only).
        let l = label(&folded(None, Verdict::Skip, None, None));
        assert_eq!((l.kind, l.source), (Kind::SmallTalk, Source::OracleOnly));
        // named_subject from tier 2's subject, nothing published; tier2 is the source.
        let l = label(&folded(
            answer(Some("Planet Nine"), None),
            model("subject"),
            None,
            None,
        ));
        assert_eq!(
            (l.kind, l.brief.as_deref(), l.source, l.published),
            (Kind::NamedSubject, None, Source::Tier2, None)
        );
        // named_subject from a publication: the oracle's own cached publish → brief = its candidate id.
        let l = label(&folded(None, publish(), None, None));
        assert_eq!(
            (l.kind, l.brief.as_deref(), l.source),
            (Kind::NamedSubject, Some("b1"), Source::OracleOnly)
        );
        assert_eq!(l.published, Some(brief(Some("row-1"), "SRE-2386")));
        // … and from the turn's outcome: a new (uncached) brief is a publication but no cached pick.
        let l = label(&folded(
            answer(Some("Planet Nine"), None),
            model("subject"),
            Some(Some(brief(None, "Planet Nine"))),
            None,
        ));
        assert_eq!(
            (l.kind, l.brief.as_deref(), l.source),
            (Kind::NamedSubject, None, Source::Tier3)
        );
        // The outcome wins over the verdict: shadow "would publish" but the turn published nothing.
        let l = label(&folded(None, publish(), Some(None), None));
        assert_eq!(
            (l.kind, l.published, l.source),
            (Kind::General, None, Source::Tier3)
        );
        // general: tier 2 failed, tier 3 published nothing.
        let l = label(&folded(
            Some(Tier2::Error {
                error: "timed out".into(),
            }),
            model("tier 2: timed out"),
            Some(None),
            None,
        ));
        assert_eq!((l.kind, l.source), (Kind::General, Source::Tier3));
        // general: tier 2 failed and no turn outcome recorded.
        let l = label(&folded(
            Some(Tier2::Error {
                error: "timed out".into(),
            }),
            model("x"),
            None,
            None,
        ));
        assert_eq!((l.kind, l.source), (Kind::General, Source::OracleOnly));
        // A published cached brief whose title is not among the candidates → brief none.
        let l = label(&folded(
            None,
            model("x"),
            Some(Some(brief(Some("row-9"), "HR-12"))),
            None,
        ));
        assert_eq!((l.kind, l.brief), (Kind::NamedSubject, None));
        // User override: a tap keeps the brief and marks the source; a dismiss clears the brief.
        let l = label(&folded(None, publish(), None, Some(Reaction::Tapped)));
        assert_eq!((l.brief.as_deref(), l.source), (Some("b1"), Source::User));
        let l = label(&folded(None, publish(), None, Some(Reaction::Dismissed)));
        assert_eq!(
            (l.kind, l.brief, l.source),
            (Kind::NamedSubject, None, Source::User)
        );
    }

    #[test]
    fn fold_keys_amends_by_id_and_keeps_the_last() {
        let a = example("them: a");
        let b = example("them: b");
        let stray = ExampleId::new();
        let rows = vec![
            Row::Example(Box::new(a.clone())),
            Row::Amend(Amend::new(
                a.id,
                Amendment::Outcome {
                    published: Some(brief(None, "x")),
                },
            )),
            Row::Amend(Amend::new(a.id, Amendment::User(Reaction::Tapped))),
            Row::Example(Box::new(b.clone())),
            // User without an outcome; then a duplicate outcome — the last one wins.
            Row::Amend(Amend::new(b.id, Amendment::User(Reaction::Dismissed))),
            Row::Amend(Amend::new(a.id, Amendment::Outcome { published: None })),
            Row::Amend(Amend::new(stray, Amendment::User(Reaction::Tapped))),
        ];
        let f = fold(rows);
        assert_eq!(f.len(), 2);
        assert_eq!(
            (f[0].example.id, f[0].outcome.clone(), f[0].user),
            (a.id, Some(None), Some(Reaction::Tapped))
        );
        assert_eq!(
            (f[1].example.id, f[1].outcome.clone(), f[1].user),
            (b.id, None, Some(Reaction::Dismissed))
        );
    }

    #[test]
    fn export_shapes() {
        let f = vec![
            folded(
                answer(Some("Planet Nine"), None),
                Verdict::Model { reason: "s".into() },
                Some(Some(brief(None, "Planet Nine"))),
                None,
            ),
            folded(None, Verdict::Skip, None, None),
        ];
        let v = fixture(&f);
        let batches = v["batches"].as_array().unwrap();
        assert_eq!(batches.len(), 2);
        let b0 = &batches[0];
        assert_eq!(b0["kind"], "named_subject");
        assert_eq!(b0["source"], "tier3");
        assert_eq!(b0["state"], "them: some talk");
        assert_eq!(
            b0["published"],
            serde_json::json!([{ "title": "Planet Nine", "cached": null }])
        );
        assert_eq!(b0["expected_brief"], serde_json::Value::Null);
        assert_eq!(
            b0["candidates"][0],
            serde_json::json!({ "id": "b1", "title": "SRE-2386", "aliases": [] })
        );
        assert_eq!(b0["mentions"], serde_json::json!(["Planet Nine"]));
        assert_eq!(
            (&b0["subject"], &b0["cached"], &b0["user"]),
            (
                &serde_json::json!("Planet Nine"),
                &serde_json::Value::Null,
                &serde_json::json!("ignored")
            )
        );
        assert_eq!(
            (
                &batches[1]["kind"],
                &batches[1]["published"],
                &batches[1]["source"]
            ),
            (
                &serde_json::json!("small_talk"),
                &serde_json::Value::Null,
                &serde_json::json!("oracle_only")
            )
        );
        // The bench's reader accepts it.
        #[derive(Deserialize)]
        struct Batch {
            kind: String,
            state: String,
            published: Option<Vec<serde_json::Value>>,
            expected_brief: Option<String>,
            candidates: Vec<serde_json::Value>,
        }
        let parsed: Vec<Batch> = serde_json::from_value(v["batches"].clone()).unwrap();
        assert_eq!(
            (
                parsed[1].kind.as_str(),
                parsed[1].published.is_none(),
                parsed[1].expected_brief.is_none()
            ),
            ("small_talk", true, true)
        );
        assert_eq!(
            (parsed[0].state.as_str(), parsed[0].candidates.len()),
            ("them: some talk", 2)
        );

        let v = laya(&f);
        let rows = v.as_array().unwrap();
        assert_eq!(rows[0]["workflow"], "meeting");
        assert_eq!(rows[0]["questions"]["kind"]["type"], "noul");
        assert_eq!(
            rows[0]["questions"]["brief"]["criteria"]["b2"],
            "DR cutover"
        );
        assert_eq!(
            rows[0]["questions"]["brief"]["criteria"]["none"],
            "no listed brief fits"
        );
        assert_eq!(
            rows[0]["gold"]["kind"],
            serde_json::json!({ "label": false, "probability_true": 0.0 })
        );
        assert_eq!(rows[0]["gold"]["brief"]["label"], "none");
        assert_eq!(rows[0]["gold"]["brief"]["probabilities"]["none"], 1.0);
        assert_eq!(
            (&rows[0]["kind__label"], &rows[0]["brief__label"]),
            (&serde_json::json!(false), &serde_json::json!("none"))
        );
        assert_eq!(
            (&rows[1]["kind__label"], &rows[1]["label_source"]),
            (&serde_json::json!(true), &serde_json::json!("oracle_only"))
        );
        assert_eq!("laya".parse::<Format>(), Ok(Format::Laya));
        assert!("csv".parse::<Format>().is_err());
    }
}
