//! Prediction, tiers 0 and 1 (126 §7.1): one zero-shot `Ask` to the decision
//! engine under `double.<kind>` (candidates = the card's options; the engine
//! appends `none` = escalate), the pooled embedding it returns, a k-NN vote
//! over the class's remembered decisions (cosine, k = 15, 30-day half-life)
//! and the class prior, blended without a head (the logistic heads are Tier
//! A, W2 — see [`Head`]). The gate is the double's own: `none` winning or
//! `Escalate` is L0 behaviour for the card whatever the rung (§9).
//!
//! The oracle is behind [`Oracle`] so the tests run on a stub — no checkpoint
//! is ever loaded here.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use decision::{Candidate, Disposition, FamilyGate, Question};
use serde::{Deserialize, Serialize};
use sqlx::Row as _;

use crate::class::CardKind;
use crate::events::{Explain, Neighbour, Parts};
use crate::store::{Store, decode_f16};
use crate::types::{Kpis, LabelSource};

/// The engine's `none` control (= escalate to the owner).
pub const NONE: &str = "none";
/// k-NN neighbours (§7.1 step 3).
pub const K: usize = 15;
/// Time decay of a remembered decision: weight halves every 30 days.
pub const HALF_LIFE_DAYS: f32 = 30.0;
/// Laplace floor for an option never chosen before.
pub const LAPLACE: f32 = 0.5;
/// The owner's current goal, the last user message, is cut to this many chars.
pub const GOAL_CHARS: usize = 600;
/// Neighbour excerpt length (`Neighbour::card_excerpt`).
pub const EXCERPT_CHARS: usize = 160;
// ponytail: fixed thresholds until W2's temperature fit lands; the gate is
// `decision::FamilyGate::decide` with these, so swapping in fitted values is
// a two-constant change.
/// The double's gate: minimum blended top-1.
pub const TAU_TOP: f32 = 0.6;
/// The double's gate: minimum top-1 − top-2.
pub const TAU_MARGIN: f32 = 0.2;
/// Remembered decisions loaded per class (retention keeps the last 1,000, §5).
pub const MEMORY_LIMIT: u32 = 1_000;

/// The oracle seam: the `decision` crate's `Engine` in the engine, a stub in
/// tests (the brief: no checkpoint is loaded on this machine).
#[async_trait::async_trait]
pub trait Oracle: Send + Sync {
    /// One `Ask`; an error means "nothing ran" and the predictor falls back
    /// to uniform probabilities and no embedding.
    async fn decide(&self, ask: decision::Ask) -> anyhow::Result<decision::Decision>;
}

/// The real oracle: `ActorRef::<Engine>::lookup(DECISION_ACTOR).ask(…)`.
pub struct EngineOracle;

#[async_trait::async_trait]
impl Oracle for EngineOracle {
    async fn decide(&self, ask: decision::Ask) -> anyhow::Result<decision::Decision> {
        let engine =
            kameo::actor::ActorRef::<decision::Engine>::lookup(decision::DECISION_ACTOR)?
                .ok_or_else(|| anyhow::anyhow!("decision engine is not running"))?;
        Ok(engine.ask(ask).await?)
    }
}

/// `classes.stats` (§5): the answer histogram, the owner's time-to-answer and
/// the last-30-days numbers the ladder wrote.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClassStats {
    /// Option label → times the owner chose it.
    pub histogram: BTreeMap<String, u32>,
    /// The last ≤ 50 times-to-answer, ms (the median's sample).
    pub answer_ms: Vec<u32>,
    /// Median of `answer_ms`.
    pub median_answer_ms: u32,
    /// The ladder's last evaluation.
    pub last_30d: Option<Kpis>,
    /// When the class last reached L2 (the L3 clock, §9).
    pub l2_since: Option<DateTime<Utc>>,
}

impl ClassStats {
    /// Parse the JSON column; anything unreadable is empty stats.
    pub fn from_json(v: &serde_json::Value) -> Self {
        serde_json::from_value(v.clone()).unwrap_or_default()
    }

    /// Record one answer and how long it took.
    pub fn record(&mut self, choice: &str, answer_ms: Option<u32>) {
        *self.histogram.entry(choice.to_owned()).or_insert(0) += 1;
        if let Some(ms) = answer_ms {
            self.answer_ms.push(ms);
            if self.answer_ms.len() > 50 {
                self.answer_ms.remove(0);
            }
            let mut s = self.answer_ms.clone();
            s.sort_unstable();
            self.median_answer_ms = s[s.len() / 2];
        }
    }

    /// `previously: Allow ×12 Deny ×1 (median 2 s)` — the state's class line.
    pub fn previously(&self) -> String {
        if self.histogram.is_empty() {
            return "previously: nothing".to_owned();
        }
        let mut parts: Vec<(&String, &u32)> = self.histogram.iter().collect();
        parts.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        let hist: Vec<String> = parts.iter().map(|(k, v)| format!("{k} ×{v}")).collect();
        format!(
            "previously: {} (median {} s)",
            hist.join(" "),
            self.median_answer_ms / 1000
        )
    }
}

/// One remembered decision of the class: a labelled example with its embedding.
#[derive(Clone, Debug, PartialEq)]
pub struct Memory {
    /// Pooled encoder state.
    pub embedding: Vec<f32>,
    /// Net label weight per option (direct + override − hindsight), > 0 only.
    pub votes: Vec<(String, f32)>,
    /// When it was asked.
    pub asked_at: DateTime<Utc>,
    /// ≤ [`EXCERPT_CHARS`] of the scrubbed card text.
    pub excerpt: String,
}

impl Memory {
    /// The option with the most weight.
    pub fn chose(&self) -> &str {
        self.votes
            .iter()
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map_or("", |(l, _)| l.as_str())
    }
}

/// The 4 KB state (§7.1 step 1): card text, then the class and requester, then
/// `previously: …`, then the owner's current goal. The store cuts it to 4 KB
/// on write; the engine right-truncates at the model's window (`cut_ok`).
pub fn state_text(
    card_text: &str,
    class: &str,
    requester: &str,
    stats: &ClassStats,
    goal: Option<&str>,
) -> String {
    let mut s = String::with_capacity(card_text.len() + 256);
    s.push_str(card_text);
    s.push_str("\nclass: ");
    s.push_str(class);
    s.push_str("\nrequester: ");
    s.push_str(requester);
    s.push('\n');
    s.push_str(&stats.previously());
    if let Some(g) = goal {
        s.push_str("\ngoal: ");
        s.extend(g.chars().take(GOAL_CHARS));
    }
    s
}

/// What the predictor produced for one card.
#[derive(Clone, Debug, PartialEq)]
pub struct Prediction {
    /// Top label (may be `none`).
    pub choice: String,
    /// `choice`'s index into the card's options; `None` for `none`.
    pub index: Option<u32>,
    /// Blended top-1.
    pub p: f32,
    /// Blended probabilities over the options plus `none`.
    pub probs: Vec<(String, f32)>,
    /// The zero-shot oracle alone (the `baseline` row).
    pub oracle_probs: Vec<(String, f32)>,
    /// Pooled embedding, when the oracle ran.
    pub embedding: Option<Vec<f32>>,
    /// `none` won or the gate said `Escalate`: L0 behaviour for this card.
    pub escalated: bool,
    /// Neighbours and parts; `promote_hint` is the ladder's and left empty here.
    pub explain: Explain,
}

/// The stacking hook. 126 W2: head — a per-class logistic regression over
/// `[oracle ‖ memory ‖ prior ‖ one-hot requester ‖ one-hot risk]` with a fitted
/// temperature replaces [`blend`] when a trained version is active; until
/// then [`NoHead`] is the only implementation.
pub trait Head: Send + Sync {
    /// Probabilities over the options plus `none`, from the parts.
    fn blend(&self, f: &Features<'_>) -> Vec<f32>;
}

/// The head's inputs (§7.1 step 5), each over the card's options plus `none` last.
#[derive(Clone, Debug, PartialEq)]
pub struct Features<'a> {
    /// Zero-shot oracle probabilities.
    pub oracle: &'a [f32],
    /// k-NN vote (`none` = 0).
    pub memory: &'a [f32],
    /// Class prior from the histogram (`none` = 0).
    pub prior: &'a [f32],
    /// Σ neighbour weights: how much memory there is.
    pub n_eff: f32,
}

/// The no-head blend: memory and prior take over from the oracle as the
/// class accumulates decisions, `w = n_eff / (n_eff + 3)`.
pub struct NoHead;

impl Head for NoHead {
    fn blend(&self, f: &Features<'_>) -> Vec<f32> {
        blend(f)
    }
}

/// `(1 − w)·oracle + w·(0.8·memory + 0.2·prior)`, `w = n_eff / (n_eff + 3)`,
/// renormalised. `none` keeps only its oracle share: memory never votes to escalate.
pub fn blend(f: &Features<'_>) -> Vec<f32> {
    let w = f.n_eff / (f.n_eff + 3.0);
    let mut out: Vec<f32> = f
        .oracle
        .iter()
        .zip(f.memory.iter().zip(f.prior))
        .map(|(o, (m, p))| (1.0 - w) * o + w * (0.8 * m + 0.2 * p))
        .collect();
    normalise(&mut out);
    out
}

/// In place, to sum 1; a zero vector becomes uniform.
pub fn normalise(v: &mut [f32]) {
    let sum: f32 = v.iter().sum();
    if sum <= 0.0 {
        let u = 1.0 / v.len().max(1) as f32;
        v.iter_mut().for_each(|x| *x = u);
    } else {
        v.iter_mut().for_each(|x| *x /= sum);
    }
}

/// Cosine similarity; 0 for a zero vector or a length mismatch.
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let (mut dot, mut na, mut nb) = (0.0f32, 0.0f32, 0.0f32);
    for (x, y) in a.iter().zip(b) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na.sqrt() * nb.sqrt())
    }
}

/// `0.5^(age_days / 30)`.
pub fn decay(asked_at: DateTime<Utc>, now: DateTime<Utc>) -> f32 {
    let days = (now - asked_at).num_seconds().max(0) as f32 / 86_400.0;
    0.5f32.powf(days / HALF_LIFE_DAYS)
}

/// The k-NN vote (§7.1 step 3) over `options`: the `K` memories with the
/// highest `max(cos, 0) × decay` — ranking by the decayed similarity, not the
/// raw one, is what keeps a 500-card burst from a month ago from filling the
/// whole neighbourhood of this week's cards (§15.1) — each weighted by that
/// and its net label weight, Laplace floor per option. Returns the
/// probabilities (options order, no `none`), `n_eff` and the ≤ 3 nearest as
/// neighbours.
pub fn knn(
    embedding: &[f32],
    options: &[String],
    memory: &[Memory],
    now: DateTime<Utc>,
) -> (Vec<f32>, f32, Vec<Neighbour>) {
    let mut scored: Vec<(f32, &Memory)> = memory
        .iter()
        .map(|m| {
            (
                cosine(embedding, &m.embedding).max(0.0) * decay(m.asked_at, now),
                m,
            )
        })
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    scored.truncate(K);
    let mut votes = vec![LAPLACE; options.len()];
    let mut n_eff = 0.0f32;
    for (w, m) in &scored {
        let w = *w;
        if w <= 0.0 {
            continue;
        }
        n_eff += w;
        for (label, net) in &m.votes {
            if let Some(i) = options.iter().position(|o| o == label) {
                votes[i] += w * net;
            }
        }
    }
    normalise(&mut votes);
    let neighbours = scored
        .iter()
        .take(3)
        .map(|(_, m)| Neighbour {
            asked_at: m.asked_at.to_rfc3339(),
            card_excerpt: m.excerpt.clone(),
            chose: m.chose().to_owned(),
        })
        .collect();
    (votes, n_eff, neighbours)
}

/// The class prior over `options` from the answer histogram, Laplace floor.
pub fn prior(options: &[String], stats: &ClassStats) -> Vec<f32> {
    let mut p: Vec<f32> = options
        .iter()
        .map(|o| LAPLACE + stats.histogram.get(o).copied().unwrap_or(0) as f32)
        .collect();
    normalise(&mut p);
    p
}

/// The oracle's probabilities re-ordered to `options` + `none`; uniform when
/// the oracle did not run (empty probabilities).
pub fn oracle_probs(options: &[String], d: &decision::Decision) -> Vec<f32> {
    let mut out: Vec<f32> = options
        .iter()
        .map(String::as_str)
        .chain(std::iter::once(NONE))
        .map(|label| {
            d.probabilities
                .iter()
                .find(|(l, _)| l == label)
                .map_or(0.0, |(_, p)| *p)
        })
        .collect();
    normalise(&mut out);
    out
}

/// Predict one card (§7.1 steps 1–5 without the head). `options` are the
/// card's labels in card order; `memory` the class's remembered decisions.
pub async fn predict(
    oracle: &dyn Oracle,
    kind: CardKind,
    options: &[String],
    state: String,
    memory: &[Memory],
    stats: &ClassStats,
    now: DateTime<Utc>,
) -> Prediction {
    let ask = decision::Ask::new(
        format!("double.{}", kind.as_str()).as_str(),
        Question::Choice {
            instructions: "Which option will the owner choose for this card? Choose none to \
                           escalate to the owner."
                .to_owned(),
            candidates: options
                .iter()
                .map(|o| Candidate {
                    id: o.clone(),
                    text: String::new(),
                })
                .collect(),
            state,
        },
    );
    let decision = match oracle.decide(ask).await {
        Ok(d) => Some(d),
        Err(e) => {
            tracing::debug!("double oracle unavailable: {e:#}");
            None
        }
    };
    let labels: Vec<String> = options
        .iter()
        .cloned()
        .chain(std::iter::once(NONE.to_owned()))
        .collect();
    let oracle_p = match &decision {
        Some(d) => oracle_probs(options, d),
        None => {
            let mut u = vec![0.0; labels.len()];
            normalise(&mut u);
            u
        }
    };
    let embedding = decision.and_then(|d| d.embedding);
    let (mut mem, n_eff, neighbours) = match &embedding {
        Some(e) if !memory.is_empty() => knn(e, options, memory, now),
        _ => (vec![0.0; options.len()], 0.0, Vec::new()),
    };
    mem.push(0.0);
    let mut pri = prior(options, stats);
    pri.push(0.0);
    let f = Features {
        oracle: &oracle_p,
        memory: &mem,
        prior: &pri,
        n_eff,
    };
    let probs = blend(&f);
    let top = probs
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map_or(labels.len() - 1, |(i, _)| i);
    let paired: Vec<(String, f32)> = labels.iter().cloned().zip(probs.iter().copied()).collect();
    let gate = FamilyGate {
        tau_top: TAU_TOP,
        tau_margin: TAU_MARGIN,
        ..FamilyGate::double()
    };
    let escalated =
        top == labels.len() - 1 || matches!(gate.decide(&paired, None), Disposition::Escalate(_));
    let w = n_eff / (n_eff + 3.0);
    Prediction {
        choice: labels[top].clone(),
        index: (top < options.len()).then_some(top as u32),
        p: probs[top],
        probs: paired,
        oracle_probs: labels.iter().cloned().zip(oracle_p.iter().copied()).collect(),
        embedding,
        escalated,
        explain: Explain {
            neighbours,
            parts: Parts {
                oracle: (1.0 - w) * oracle_p[top],
                memory: w * 0.8 * mem[top],
                prior: w * 0.2 * pri[top],
            },
            promote_hint: String::new(),
        },
    }
}

/// The class's remembered decisions: its newest ≤ [`MEMORY_LIMIT`] examples
/// that carry an embedding and a positive net label.
pub async fn memory(store: &Store, class: &str) -> anyhow::Result<Vec<Memory>> {
    let rows = sqlx::query(
        "SELECT e.id, e.card_text, e.embedding, e.asked_at, l.source, l.choice, l.weight
         FROM examples e JOIN labels l ON l.example_id = e.id
         WHERE e.class = ? AND e.embedding IS NOT NULL
           AND e.id IN (SELECT id FROM examples WHERE class = ? ORDER BY asked_at DESC LIMIT ?)
         ORDER BY e.asked_at DESC, e.id, l.source",
    )
    .bind(class)
    .bind(class)
    .bind(i64::from(MEMORY_LIMIT))
    .fetch_all(store.pool())
    .await?;
    let mut out: Vec<Memory> = Vec::new();
    let mut last_id: Option<String> = None;
    let mut votes: BTreeMap<String, f32> = BTreeMap::new();
    let flush = |out: &mut Vec<Memory>, votes: &mut BTreeMap<String, f32>| {
        let positive: Vec<(String, f32)> = votes
            .iter()
            .filter(|(_, w)| **w > 0.0)
            .map(|(l, w)| (l.clone(), *w))
            .collect();
        votes.clear();
        if positive.is_empty() {
            out.pop();
        } else if let Some(m) = out.last_mut() {
            m.votes = positive;
        }
    };
    for r in rows {
        let id: String = r.try_get("id")?;
        if last_id.as_deref() != Some(id.as_str()) {
            if last_id.is_some() {
                flush(&mut out, &mut votes);
            }
            let text: String = r.try_get("card_text")?;
            let blob: Vec<u8> = r.try_get("embedding")?;
            let at: String = r.try_get("asked_at")?;
            out.push(Memory {
                embedding: decode_f16(&blob),
                votes: Vec::new(),
                asked_at: DateTime::parse_from_rfc3339(&at)?.with_timezone(&Utc),
                excerpt: text.chars().take(EXCERPT_CHARS).collect(),
            });
            last_id = Some(id);
        }
        let source: String = r.try_get("source")?;
        let choice: String = r.try_get("choice")?;
        let weight: f64 = r.try_get("weight")?;
        let signed = match LabelSource::parse(&source) {
            Some(LabelSource::Direct | LabelSource::Override) => weight as f32,
            Some(
                LabelSource::HindsightRewind
                | LabelSource::HindsightCorrection
                | LabelSource::HindsightInterrupt,
            ) => -(weight as f32),
            _ => 0.0,
        };
        *votes.entry(choice).or_insert(0.0) += signed;
    }
    if last_id.is_some() {
        flush(&mut out, &mut votes);
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use parking_lot::Mutex;

    use super::*;

    /// A canned oracle: answers every `Ask` with the given probabilities (in
    /// the candidates' order, `none` last) and embedding; records the asks.
    pub struct FakeOracle {
        /// Probabilities for the options then `none`; shorter → padded with 0.
        pub probs: Mutex<Vec<f32>>,
        /// The embedding to return; `None` = the engine gave none.
        pub embedding: Mutex<Option<Vec<f32>>>,
        /// Every `Ask` received.
        pub asks: Mutex<Vec<decision::Ask>>,
        /// Fail every call (the engine is not running).
        pub fail: bool,
    }

    impl FakeOracle {
        pub fn new(probs: Vec<f32>, embedding: Option<Vec<f32>>) -> Self {
            Self {
                probs: Mutex::new(probs),
                embedding: Mutex::new(embedding),
                asks: Mutex::new(Vec::new()),
                fail: false,
            }
        }
    }

    #[async_trait::async_trait]
    impl Oracle for FakeOracle {
        async fn decide(&self, ask: decision::Ask) -> anyhow::Result<decision::Decision> {
            if self.fail {
                anyhow::bail!("decision engine is not running");
            }
            let labels: Vec<String> = match &ask.question {
                Question::Choice { candidates, .. } => candidates
                    .iter()
                    .map(|c| c.id.clone())
                    .chain(std::iter::once(NONE.to_owned()))
                    .collect(),
                Question::Bool { .. } => vec!["true".into(), "false".into()],
                Question::Score { levels, .. } => {
                    (0..levels.len()).map(|i| i.to_string()).collect()
                }
            };
            let probs = self.probs.lock().clone();
            let probabilities: Vec<(String, f32)> = labels
                .iter()
                .enumerate()
                .map(|(i, l)| (l.clone(), probs.get(i).copied().unwrap_or(0.0)))
                .collect();
            let top = probabilities
                .iter()
                .max_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(l, _)| l.clone())
                .unwrap_or_default();
            self.asks.lock().push(ask.clone());
            Ok(decision::Decision {
                family: ask.family,
                choice: top,
                probabilities,
                confidence: 0.0,
                act_probability: 0.0,
                disposition: Disposition::Accept,
                latency: std::time::Duration::ZERO,
                gate_version: 0,
                log_id: decision::LogId::new(),
                tier: 1,
                calibrated: true,
                truncated: false,
                embedding: self.embedding.lock().clone(),
            })
        }
    }

    fn opts() -> Vec<String> {
        vec!["Allow".into(), "Deny".into()]
    }

    fn mem(embedding: Vec<f32>, chose: &str, days_ago: i64) -> Memory {
        Memory {
            embedding,
            votes: vec![(chose.into(), 1.0)],
            asked_at: Utc::now() - chrono::Duration::days(days_ago),
            excerpt: format!("git push ({chose})"),
        }
    }

    /// No memory → the oracle alone; `none` on top → escalated; gate thresholds apply.
    #[tokio::test]
    async fn zero_shot_alone() {
        let o = FakeOracle::new(vec![0.7, 0.2, 0.1], Some(vec![1.0, 0.0]));
        let p = predict(&o, CardKind::Approval, &opts(), "s".into(), &[], &ClassStats::default(), Utc::now()).await;
        assert_eq!(p.choice, "Allow");
        assert_eq!(p.index, Some(0));
        assert!(!p.escalated);
        assert!((p.p - 0.7).abs() < 1e-5);
        assert_eq!(p.probs.len(), 3);
        assert_eq!(p.oracle_probs, p.probs);
        assert_eq!(p.explain.neighbours.len(), 0);
        let asks = o.asks.lock();
        assert_eq!(asks[0].family.0, "double.approval");
        // none wins → escalate.
        let o = FakeOracle::new(vec![0.2, 0.1, 0.7], None);
        let p = predict(&o, CardKind::Ask, &opts(), "s".into(), &[], &ClassStats::default(), Utc::now()).await;
        assert_eq!(p.choice, NONE);
        assert_eq!(p.index, None);
        assert!(p.escalated);
        // a low margin escalates too.
        let o = FakeOracle::new(vec![0.5, 0.4, 0.1], None);
        let p = predict(&o, CardKind::Ask, &opts(), "s".into(), &[], &ClassStats::default(), Utc::now()).await;
        assert_eq!(p.choice, "Allow");
        assert!(p.escalated);
    }

    /// The engine is not running → uniform, escalated, no embedding — but still a prediction.
    #[tokio::test]
    async fn engine_down_is_uniform() {
        let mut o = FakeOracle::new(vec![], None);
        o.fail = true;
        let p = predict(&o, CardKind::Approval, &opts(), "s".into(), &[], &ClassStats::default(), Utc::now()).await;
        assert!(p.escalated);
        assert!(p.embedding.is_none());
        assert!((p.probs[0].1 - 1.0 / 3.0).abs() < 1e-5);
    }

    /// Memory overrides a wrong zero-shot: 20 near `Allow` neighbours beat an oracle saying Deny.
    #[tokio::test]
    async fn memory_is_the_early_workhorse() {
        let o = FakeOracle::new(vec![0.3, 0.6, 0.1], Some(vec![1.0, 0.0, 0.0]));
        let memory: Vec<Memory> = (0..20).map(|i| mem(vec![1.0, 0.05 * i as f32, 0.0], "Allow", i)).collect();
        let p = predict(&o, CardKind::Approval, &opts(), "s".into(), &memory, &ClassStats::default(), Utc::now()).await;
        assert_eq!(p.choice, "Allow");
        assert!(!p.escalated);
        assert_eq!(p.explain.neighbours.len(), 3);
        assert_eq!(p.explain.neighbours[0].chose, "Allow");
        assert!(p.explain.parts.memory > p.explain.parts.oracle);
        // The baseline row still says Deny.
        assert_eq!(p.oracle_probs[1].0, "Deny");
        assert!(p.oracle_probs[1].1 > p.oracle_probs[0].1);
    }

    /// §15.1: 500 `cargo test` approvals in one night do not swamp another class's vote —
    /// retrieval is per class (the memory passed in is the class's own) and decay keeps
    /// a month-old cluster from outweighing last week's.
    #[tokio::test]
    async fn burst_does_not_swamp() {
        // Class A (`git push`): 10 old Allow rows. The burst belongs to class B and is simply
        // not in A's memory.
        let o = FakeOracle::new(vec![0.45, 0.45, 0.1], Some(vec![0.0, 1.0]));
        let a: Vec<Memory> = (0..10).map(|i| mem(vec![0.0, 1.0], "Allow", 3 + i)).collect();
        let p = predict(&o, CardKind::Approval, &opts(), "s".into(), &a, &ClassStats::default(), Utc::now()).await;
        assert_eq!(p.choice, "Allow");
        assert!(!p.escalated);
        // Within one class: 500 Deny rows from 60 days ago vs 15 Allow rows from this week,
        // all equally near — ranking by decayed similarity (0.5^(60/30) = 0.25 for the
        // burst) keeps this week's answers in the k = 15 neighbourhood.
        let mut m: Vec<Memory> = (0..500).map(|_| mem(vec![0.0, 1.0], "Deny", 60)).collect();
        m.extend((0..15).map(|_| mem(vec![0.0, 1.0], "Allow", 2)));
        let p = predict(&o, CardKind::Approval, &opts(), "s".into(), &m, &ClassStats::default(), Utc::now()).await;
        assert_eq!(p.choice, "Allow", "{:?}", p.probs);
    }

    #[test]
    fn stats_and_state() {
        let mut s = ClassStats::default();
        assert_eq!(s.previously(), "previously: nothing");
        s.record("Allow", Some(1_000));
        s.record("Allow", Some(3_000));
        s.record("Deny", Some(2_000));
        assert_eq!(s.median_answer_ms, 2_000);
        assert_eq!(s.previously(), "previously: Allow ×2 Deny ×1 (median 2 s)");
        let st = state_text("git push", "approval:shell:mutating:git push:root", "root", &s, Some(&"x".repeat(700)));
        assert!(st.starts_with("git push\nclass: approval:shell:mutating:git push:root\nrequester: root\npreviously: Allow ×2"));
        assert_eq!(st.len(), st.find("goal: ").unwrap() + 6 + GOAL_CHARS);
        let j = serde_json::to_value(&s).unwrap();
        assert_eq!(ClassStats::from_json(&j), s);
        assert_eq!(ClassStats::from_json(&serde_json::json!({})), ClassStats::default());
    }

    #[test]
    fn decay_and_cosine() {
        let now = Utc::now();
        assert!((decay(now - chrono::Duration::days(30), now) - 0.5).abs() < 1e-3);
        assert!((decay(now, now) - 1.0).abs() < 1e-6);
        assert!((cosine(&[1.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < 1e-6);
        assert_eq!(cosine(&[1.0, 0.0], &[0.0, 1.0]), 0.0);
        assert_eq!(cosine(&[1.0], &[1.0, 0.0]), 0.0);
    }
}
