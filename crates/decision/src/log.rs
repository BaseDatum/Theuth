//! The decision log (57 §2): `<data_dir>/decisions/YYYY-MM-DD.jsonl`, one [`Row::Ask`]
//! per question and one [`Row::Outcome`] per `Report`, keyed by `log_id`; readers fold.
//! No text by default (`decision.log_text`): the question is a sha256 of the rendered ids.

use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::gate::Mode;
use crate::question::{Decision, Family, LogId, Outcome, Question, Reason};

/// One line of the file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "row", rename_all = "snake_case")]
pub enum Row {
    /// A question and its decision.
    Ask(Box<AskRow>),
    /// What happened afterwards.
    Outcome {
        /// The ask this patches.
        log_id: LogId,
        /// When reported.
        ts: DateTime<Utc>,
        /// The outcome.
        outcome: Outcome,
    },
}

/// The ask row: everything threshold fitting needs, nothing the text owners would mind.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AskRow {
    /// When asked.
    pub ts: DateTime<Utc>,
    /// Key for the outcome row.
    pub log_id: LogId,
    /// Family.
    pub family: Family,
    /// Thresholds + model at the time.
    pub gate_version: u32,
    /// sha256 of `model.safetensors` — `size:<bytes>` on rows written before the background hash landed; empty when no model ran.
    pub model_sha: String,
    /// Gate mode at the time.
    pub mode: Mode,
    /// sha256 hex of the rendered token ids; empty when nothing was rendered.
    pub question_hash: String,
    /// Option count.
    pub k: usize,
    /// Raw logits (f32), option order.
    pub logits: Vec<f32>,
    /// Temperature applied.
    pub temperature: f32,
    /// Calibrated probabilities, option order.
    pub probs: Vec<(String, f32)>,
    /// laya's act head.
    pub act_p: f32,
    /// The chosen label.
    pub choice: String,
    /// `accept` / `escalate` / `shadow`.
    pub disposition: String,
    /// Escalation reason, through a shadow wrapper.
    pub reason: Option<Reason>,
    /// Engine wall-clock.
    pub latency_ms: u64,
    /// 1 = oracle, 2 = cheap-model side call.
    #[serde(default = "tier_one")]
    pub tier: u8,
    /// The question itself, only with `decision.log_text: true`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<Question>,
    /// Cold-load wall clock, on the first row after a load.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load_ms: Option<u64>,
    /// A short annotation on tier-2 rows — the subject tier-2 extracted (57 §3.5). The one
    /// text the log carries by design; never the state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl AskRow {
    /// From a finished decision. `rendered` = the token ids (hashed), `logits`/`temperature` as run.
    pub fn new(
        d: &Decision,
        mode: Mode,
        model_sha: &str,
        rendered: &[i64],
        logits: Vec<f32>,
        temperature: f32,
        text: Option<Question>,
    ) -> Self {
        AskRow {
            ts: Utc::now(),
            log_id: d.log_id,
            family: d.family.clone(),
            gate_version: d.gate_version,
            model_sha: model_sha.to_owned(),
            mode,
            question_hash: if rendered.is_empty() {
                String::new()
            } else {
                question_hash(rendered)
            },
            k: d.probabilities.len(),
            logits,
            temperature,
            probs: d.probabilities.clone(),
            act_p: d.act_probability,
            choice: d.choice.clone(),
            disposition: d.disposition.kind().to_owned(),
            reason: d.disposition.reason().cloned(),
            latency_ms: d.latency.as_millis() as u64,
            tier: d.tier,
            text,
            load_ms: None,
            note: None,
        }
    }
}

const fn tier_one() -> u8 {
    1
}

/// sha256 hex of the rendered ids, little-endian i64s.
pub fn question_hash(ids: &[i64]) -> String {
    let mut h = Sha256::new();
    for id in ids {
        h.update(id.to_le_bytes());
    }
    format!("{:x}", h.finalize())
}

/// What `Stats { family }` answers: today's file folded for one family.
#[derive(
    Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct FamilyStats {
    /// Family.
    pub family: String,
    /// The gate's mode right now (filled by the engine, not the fold).
    #[serde(default = "off")]
    pub mode: Mode,
    /// Ask rows.
    pub asks: u64,
    /// Tier-2 rows (cheap-model answers) among `asks`, and how many of them were valid picks.
    pub tier2: u64,
    /// See `tier2`.
    pub tier2_accepted: u64,
    /// Rows whose (inner) disposition was accept: the coverage numerator in shadow, the saved turns in active.
    pub accepted: u64,
    /// Escalations by reason (`low_top1`, `hard_error`, …).
    pub escalations: BTreeMap<String, u64>,
    /// Median latency.
    pub p50_ms: u64,
    /// 95th percentile latency.
    pub p95_ms: u64,
    /// Outcomes reported.
    pub confirmed: u64,
    /// Outcomes reported as corrected.
    pub corrected: u64,
    /// Outcomes reported unknown.
    pub unknown: u64,
    /// `confirmed / (confirmed + corrected)` over accepted rows with an outcome; the shadow agreement rate.
    pub agreement: Option<f32>,
}

const fn off() -> Mode {
    Mode::Off
}

/// The append-only day files under one directory.
#[derive(Clone, Debug)]
pub struct Log {
    dir: PathBuf,
}

impl Log {
    /// `<data_dir>/decisions`.
    pub fn new(dir: PathBuf) -> Self {
        Log { dir }
    }

    /// The directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn today(&self) -> PathBuf {
        self.dir
            .join(format!("{}.jsonl", chrono::Local::now().format("%Y-%m-%d")))
    }

    /// Append one row to today's file, creating the directory on first use.
    pub fn append(&self, row: &Row) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.today())?;
        let mut line = serde_json::to_vec(row)?;
        line.push(b'\n');
        f.write_all(&line)
    }

    /// Fold today's file for `family`. A missing file is zero rows.
    pub fn stats(&self, family: &Family) -> std::io::Result<FamilyStats> {
        fold(&self.today(), family)
    }
}

/// Fold one day file for one family.
pub fn fold(path: &Path, family: &Family) -> std::io::Result<FamilyStats> {
    let mut s = FamilyStats {
        family: family.0.clone(),
        ..FamilyStats::default()
    };
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(s),
        Err(e) => return Err(e),
    };
    let mut latencies = Vec::new();
    // log_id → accepted (inner disposition); only rows of this family.
    let mut accepted_by_id: HashMap<LogId, bool> = HashMap::new();
    let mut outcomes: Vec<(LogId, Outcome)> = Vec::new();
    for line in std::io::BufReader::new(file).lines() {
        let line = line?;
        let Ok(row) = serde_json::from_str::<Row>(&line) else {
            tracing::warn!("decision log: unreadable row in {}", path.display());
            continue;
        };
        match row {
            Row::Ask(a) if &a.family == family => {
                s.asks += 1;
                latencies.push(a.latency_ms);
                if a.tier == 2 {
                    s.tier2 += 1;
                    if a.reason.is_none() {
                        s.tier2_accepted += 1;
                    }
                }
                match &a.reason {
                    None => s.accepted += 1,
                    Some(r) => *s.escalations.entry(reason_key(r)).or_default() += 1,
                }
                accepted_by_id.insert(a.log_id, a.reason.is_none());
            }
            Row::Ask(_) => {}
            Row::Outcome {
                log_id, outcome, ..
            } => outcomes.push((log_id, outcome)),
        }
    }
    let (mut agree, mut disagree) = (0u64, 0u64);
    for (id, outcome) in outcomes {
        let Some(&accepted) = accepted_by_id.get(&id) else {
            continue; // another family, or another day
        };
        match outcome {
            Outcome::Confirmed => {
                s.confirmed += 1;
                if accepted {
                    agree += 1;
                }
            }
            Outcome::Corrected { .. } => {
                s.corrected += 1;
                if accepted {
                    disagree += 1;
                }
            }
            Outcome::Unknown => s.unknown += 1,
        }
    }
    if agree + disagree > 0 {
        s.agreement = Some(agree as f32 / (agree + disagree) as f32);
    }
    latencies.sort_unstable();
    let pct = |p: f64| -> u64 {
        if latencies.is_empty() {
            0
        } else {
            latencies[((latencies.len() - 1) as f64 * p).round() as usize]
        }
    };
    s.p50_ms = pct(0.5);
    s.p95_ms = pct(0.95);
    Ok(s)
}

/// The `reason` tag as serialised (`low_top1`, `hard_error`, …).
fn reason_key(r: &Reason) -> String {
    match serde_json::to_value(r) {
        Ok(serde_json::Value::Object(m)) => m
            .get("reason")
            .and_then(|v| v.as_str())
            .unwrap_or("?")
            .to_owned(),
        _ => "?".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::question::Disposition;

    fn decision(family: &str, disposition: Disposition, ms: u64) -> Decision {
        Decision {
            family: family.into(),
            choice: "a".into(),
            probabilities: vec![("a".into(), 0.9), ("none".into(), 0.1)],
            confidence: 0.7,
            act_probability: 0.4,
            disposition,
            latency: Duration::from_millis(ms),
            gate_version: 7,
            log_id: LogId::new(),
            tier: 1,
            calibrated: true,
            truncated: false,
            embedding: None,
        }
    }

    #[test]
    fn round_trip_and_fold() {
        let dir = tempfile::tempdir().unwrap();
        let log = Log::new(dir.path().join("decisions"));
        let fam: Family = "meeting.topic".into();
        let ids = [50281i64, 1, 2, 50282];
        let a = decision("meeting.topic", Disposition::Accept, 100).shadowed();
        let b = decision("meeting.topic", Disposition::Accept, 200).shadowed();
        let c = decision(
            "meeting.topic",
            Disposition::Escalate(Reason::LowMargin),
            300,
        );
        let d = decision(
            "meeting.topic",
            Disposition::Escalate(Reason::HardError("ort: boom".into())),
            1,
        );
        let other = decision("browser.target", Disposition::Accept, 999);
        for dec in [&a, &b, &c, &d, &other] {
            let row = Row::Ask(Box::new(AskRow::new(
                dec,
                Mode::Shadow,
                "sha",
                &ids,
                vec![1.0, -1.0],
                1.6,
                None,
            )));
            log.append(&row).unwrap();
        }
        log.append(&Row::Outcome {
            log_id: a.log_id,
            ts: Utc::now(),
            outcome: Outcome::Confirmed,
        })
        .unwrap();
        log.append(&Row::Outcome {
            log_id: b.log_id,
            ts: Utc::now(),
            outcome: Outcome::Corrected {
                actual: "none".into(),
            },
        })
        .unwrap();
        log.append(&Row::Outcome {
            log_id: c.log_id,
            ts: Utc::now(),
            outcome: Outcome::Confirmed,
        })
        .unwrap();
        log.append(&Row::Outcome {
            log_id: d.log_id,
            ts: Utc::now(),
            outcome: Outcome::Unknown,
        })
        .unwrap();

        // Rows read back byte-exact.
        let file = std::fs::read_to_string(log.today()).unwrap();
        assert_eq!(file.lines().count(), 9);
        let first: Row = serde_json::from_str(file.lines().next().unwrap()).unwrap();
        match first {
            Row::Ask(r) => {
                assert_eq!(r.log_id, a.log_id);
                assert_eq!(r.disposition, "shadow");
                assert_eq!(r.reason, None);
                assert_eq!(r.question_hash, question_hash(&ids));
                assert_eq!(r.k, 2);
                assert_eq!(r.text, None);
                assert!(!file.lines().next().unwrap().contains("\"text\""));
            }
            other => panic!("{other:?}"),
        }

        let s = log.stats(&fam).unwrap();
        assert_eq!(s.asks, 4);
        assert_eq!(s.accepted, 2);
        assert_eq!(s.escalations.get("low_margin"), Some(&1));
        assert_eq!(s.escalations.get("hard_error"), Some(&1));
        assert_eq!((s.confirmed, s.corrected, s.unknown), (2, 1, 1));
        assert_eq!(s.agreement, Some(0.5));
        assert_eq!(s.p50_ms, 200);
        assert_eq!(s.p95_ms, 300);
        let o = log.stats(&"browser.target".into()).unwrap();
        assert_eq!((o.asks, o.accepted, o.p50_ms), (1, 1, 999));
        assert_eq!(
            log.stats(&"nope".into()).unwrap(),
            FamilyStats {
                family: "nope".into(),
                ..FamilyStats::default()
            }
        );
    }

    #[test]
    fn missing_file_is_empty() {
        let s = fold(Path::new("/nonexistent/2020-01-01.jsonl"), &"x".into()).unwrap();
        assert_eq!(s.asks, 0);
    }

    #[test]
    fn hash_is_stable_and_order_sensitive() {
        assert_eq!(question_hash(&[1, 2]), question_hash(&[1, 2]));
        assert_ne!(question_hash(&[1, 2]), question_hash(&[2, 1]));
        assert_eq!(question_hash(&[1, 2]).len(), 64);
    }
}
