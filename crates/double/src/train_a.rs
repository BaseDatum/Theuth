//! Tier A (126 §7.2, §7.5): temperatures per class, the flat k-NN index and
//! the stacked logistic heads over frozen embeddings — in-process, plain Rust,
//! deterministic (f64, fixed iteration caps, BTreeMap ordering; no RNG, no
//! GPU). Running it twice on the same `data_range` yields byte-identical
//! `heads.json` / `knn.bin` / `temperatures.json`.
//!
//! The head is a conditional logit shared across the options of one card:
//! every option gets four features — the zero-shot oracle probability, the
//! k-NN vote, the class prior and `is none` — crossed with a nine-wide context
//! (`1`, requester one-hot ×4, risk one-hot ×4); softmax over the card's
//! options plus `none`. The same 36 weights serve a per-class head (fit on the
//! class when it has ≥ [`PER_CLASS_MIN`] training rows) and the global one.
//! Fit by L-BFGS from zero with L2 = 1.0 (§7.5), negative log-likelihood over
//! the §6 weighted labels (positive sources count for their choice, hindsight
//! sources against it; the trainer sees the net).
//!
//! The golden set — the first 10 % of each class's rows, at least 5 — is marked
//! here (`golden` table), excluded from every fit, and re-scored by every
//! candidate (§7.4). The promotion gate ([`gate`]) is §7.3's: beat the active
//! version's log-loss on the last-7-days prequential rows, lose at most one
//! accuracy point on golden, ECE ≤ 0.05.

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sqlx::Row as _;

use crate::store::{BASELINE, Store, VersionRow, decode_f16, encode_f16};
use crate::types::{LabelSource, Tier};

/// Rows a class needs before it gets its own head (§7.1 step 5).
pub const PER_CLASS_MIN: usize = 20;
/// Neighbours in the vote (§7.1 step 3).
pub const K: usize = 15;
/// Time-decay horizon of a neighbour's vote, days.
pub const DECAY_DAYS: f64 = 30.0;
/// Laplace floor of the vote (§7.1: an unseen option never gets zero).
pub const FLOOR: f64 = 0.1;
/// L2 penalty of the head (§7.5).
pub const L2: f64 = 1.0;
/// L-BFGS iteration cap (§7.5).
pub const ITERS: usize = 100;
/// Golden share and minimum (§2, §7.4).
pub const GOLDEN_SHARE: f64 = 0.10;
/// Golden minimum per class.
pub const GOLDEN_MIN: usize = 5;
/// The output label of the escalate option.
pub const NONE: &str = "none";
/// Gate (c): max expected calibration error.
pub const MAX_ECE: f64 = 0.05;
/// Gate (b): max golden accuracy regression.
pub const MAX_GOLDEN_DROP: f64 = 0.01;
/// Gate (a) window.
pub const PROXY_DAYS: i64 = 7;

const OPT_F: usize = 4;
const CTX_F: usize = 9;

// --- context -------------------------------------------------------------

/// `root` | `subagent` | `desk:*` | `routine:*` → 0..4.
pub fn requester_idx(requester: &str) -> usize {
    match requester.split(':').next().unwrap_or("") {
        "root" => 0,
        "subagent" => 1,
        "desk" => 2,
        _ => 3,
    }
}

/// The risk segment of an `approval:<tool>:<risk>:…` key → 1..4; other kinds 0.
pub fn risk_idx(class: &str) -> usize {
    let mut it = class.split(':');
    if it.next() != Some("approval") {
        return 0;
    }
    match it.nth(1) {
        Some("read_only") => 1,
        Some("mutating") => 2,
        Some("destructive") => 3,
        _ => 0,
    }
}

fn ctx(requester: usize, risk: usize) -> [f64; CTX_F] {
    let mut c = [0.0; CTX_F];
    c[0] = 1.0;
    c[1 + requester.min(3)] = 1.0;
    c[5 + risk.min(3)] = 1.0;
    c
}

// --- head ----------------------------------------------------------------

/// One conditional-logit head: 4 option features × 9 context features.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Head {
    /// `w[option_feature][context_feature]`.
    pub w: Vec<Vec<f64>>,
}

impl Head {
    fn zero() -> Self {
        Self {
            w: vec![vec![0.0; CTX_F]; OPT_F],
        }
    }

    /// Logit of one option.
    fn logit(&self, opt: &[f64; OPT_F], ctx: &[f64; CTX_F]) -> f64 {
        self.w
            .iter()
            .zip(opt)
            .map(|(row, o)| o * row.iter().zip(ctx).map(|(w, c)| w * c).sum::<f64>())
            .sum()
    }

    fn logits(&self, opts: &[[f64; OPT_F]], ctx: &[f64; CTX_F]) -> Vec<f64> {
        opts.iter().map(|o| self.logit(o, ctx)).collect()
    }
}

/// One fitted example: option features, context and the normalised target.
struct Fit {
    opts: Vec<[f64; OPT_F]>,
    ctx: [f64; CTX_F],
    /// Σ = 1.
    target: Vec<f64>,
    /// Σ of the net label weights.
    weight: f64,
}

fn softmax(logits: &[f64]) -> Vec<f64> {
    let m = logits.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let e: Vec<f64> = logits.iter().map(|l| (l - m).exp()).collect();
    let z: f64 = e.iter().sum();
    e.into_iter().map(|v| v / z).collect()
}

/// NLL + L2/2 and its gradient at `w` (flattened OPT_F×CTX_F).
fn loss_grad(w: &[f64], rows: &[Fit]) -> (f64, Vec<f64>) {
    let head = Head {
        w: w.chunks(CTX_F).map(|c| c.to_vec()).collect(),
    };
    let mut loss = 0.0;
    let mut g = vec![0.0; w.len()];
    for r in rows {
        let p = softmax(&head.logits(&r.opts, &r.ctx));
        for (o, (pk, tk)) in p.iter().zip(&r.target).enumerate() {
            if *tk > 0.0 {
                loss -= r.weight * tk * pk.max(1e-12).ln();
            }
            let d = r.weight * (pk - tk);
            for (i, oi) in r.opts[o].iter().enumerate() {
                for (j, cj) in r.ctx.iter().enumerate() {
                    g[i * CTX_F + j] += d * oi * cj;
                }
            }
        }
    }
    for (gi, wi) in g.iter_mut().zip(w) {
        loss += L2 * 0.5 * wi * wi;
        *gi += L2 * wi;
    }
    (loss, g)
}

/// L-BFGS (m = 5, Armijo backtracking) from zero, [`ITERS`] steps at most.
fn fit_head(rows: &[Fit]) -> Head {
    if rows.is_empty() {
        return Head::zero();
    }
    let n = OPT_F * CTX_F;
    let mut x = vec![0.0; n];
    let (mut f, mut g) = loss_grad(&x, rows);
    let mut s_hist: Vec<Vec<f64>> = Vec::new();
    let mut y_hist: Vec<Vec<f64>> = Vec::new();
    let dot = |a: &[f64], b: &[f64]| a.iter().zip(b).map(|(x, y)| x * y).sum::<f64>();
    for _ in 0..ITERS {
        if dot(&g, &g).sqrt() < 1e-6 {
            break;
        }
        // Two-loop recursion.
        let mut q = g.clone();
        let mut alpha = vec![0.0; s_hist.len()];
        for ((s_i, y_i), a_i) in s_hist.iter().zip(&y_hist).zip(alpha.iter_mut()).rev() {
            let rho = 1.0 / dot(y_i, s_i);
            *a_i = rho * dot(s_i, &q);
            for (qk, yk) in q.iter_mut().zip(y_i) {
                *qk -= *a_i * yk;
            }
        }
        let gamma = match (s_hist.last(), y_hist.last()) {
            (Some(s_l), Some(y_l)) => dot(s_l, y_l) / dot(y_l, y_l),
            _ => 1.0 / (1.0 + dot(&g, &g).sqrt()),
        };
        for qk in q.iter_mut() {
            *qk *= gamma;
        }
        for ((s_i, y_i), a_i) in s_hist.iter().zip(&y_hist).zip(&alpha) {
            let rho = 1.0 / dot(y_i, s_i);
            let beta = rho * dot(y_i, &q);
            for (qk, sk) in q.iter_mut().zip(s_i) {
                *qk += sk * (a_i - beta);
            }
        }
        let d: Vec<f64> = q.iter().map(|v| -v).collect();
        let slope = dot(&g, &d);
        if slope >= 0.0 {
            s_hist.clear();
            y_hist.clear();
            continue;
        }
        let mut t = 1.0;
        let (x_new, f_new, g_new) = loop {
            let xn: Vec<f64> = x.iter().zip(&d).map(|(xi, di)| xi + t * di).collect();
            let (fn_, gn) = loss_grad(&xn, rows);
            if fn_ <= f + 1e-4 * t * slope || t < 1e-10 {
                break (xn, fn_, gn);
            }
            t *= 0.5;
        };
        let s: Vec<f64> = x_new.iter().zip(&x).map(|(a, b)| a - b).collect();
        let y: Vec<f64> = g_new.iter().zip(&g).map(|(a, b)| a - b).collect();
        if dot(&s, &y) > 1e-12 {
            s_hist.push(s);
            y_hist.push(y);
            if s_hist.len() > 5 {
                s_hist.remove(0);
                y_hist.remove(0);
            }
        }
        if (f - f_new).abs() < 1e-10 {
            x = x_new;
            break;
        }
        x = x_new;
        f = f_new;
        g = g_new;
    }
    Head {
        w: x.chunks(CTX_F).map(|c| c.to_vec()).collect(),
    }
}

// --- artefacts -----------------------------------------------------------

/// A class's option vocabulary and prior, as the heads need them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClassVocab {
    /// Every option label seen in the class's training rows, plus `none` last.
    pub options: Vec<String>,
    /// Laplace prior per option, Σ = 1.
    pub prior: Vec<f32>,
    /// Training rows the class contributed.
    pub n: u32,
}

/// `heads.json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Heads {
    /// Fit over every training row.
    pub global: Head,
    /// Fit per class with ≥ [`PER_CLASS_MIN`] rows.
    pub per_class: BTreeMap<String, Head>,
    /// Option vocabularies and priors.
    pub classes: BTreeMap<String, ClassVocab>,
}

/// One row of the flat k-NN index.
#[derive(Clone, Debug, PartialEq)]
pub struct KnnEntry {
    /// The class key.
    pub class: String,
    /// The option voted for (its index into the class vocabulary).
    pub option: u32,
    /// Net label weight behind the vote.
    pub weight: f32,
    /// Milliseconds since the epoch.
    pub asked_at_ms: i64,
    /// The pooled embedding.
    pub embedding: Vec<f32>,
}

/// Temperatures, k-NN index and heads of one version.
#[derive(Clone, Debug, PartialEq)]
pub struct Artifacts {
    /// `heads.json`.
    pub heads: Heads,
    /// `knn.bin`, sorted by (class, asked_at, option).
    pub knn: Vec<KnnEntry>,
    /// `temperatures.json`: per class; 1.0 when absent.
    pub temperatures: BTreeMap<String, f32>,
}

const KNN_MAGIC: &[u8; 4] = b"DKN1";

impl Artifacts {
    /// Nothing learned: uniform heads, no memory.
    pub fn empty() -> Self {
        Self {
            heads: Heads {
                global: Head::zero(),
                per_class: BTreeMap::new(),
                classes: BTreeMap::new(),
            },
            knn: Vec::new(),
            temperatures: BTreeMap::new(),
        }
    }

    /// `heads.json` bytes.
    pub fn heads_json(&self) -> anyhow::Result<Vec<u8>> {
        Ok(serde_json::to_vec_pretty(&self.heads)?)
    }

    /// `temperatures.json` bytes.
    pub fn temperatures_json(&self) -> anyhow::Result<Vec<u8>> {
        Ok(serde_json::to_vec_pretty(&self.temperatures)?)
    }

    /// `knn.bin`: magic, u32 count, then per row `u32 class-len, class,
    /// u32 option, f32 weight, i64 asked_at_ms, u32 dim, f16 × dim`, all LE.
    pub fn knn_bin(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(16 + self.knn.len() * 2100);
        b.extend_from_slice(KNN_MAGIC);
        b.extend_from_slice(&(self.knn.len() as u32).to_le_bytes());
        for e in &self.knn {
            b.extend_from_slice(&(e.class.len() as u32).to_le_bytes());
            b.extend_from_slice(e.class.as_bytes());
            b.extend_from_slice(&e.option.to_le_bytes());
            b.extend_from_slice(&e.weight.to_le_bytes());
            b.extend_from_slice(&e.asked_at_ms.to_le_bytes());
            b.extend_from_slice(&(e.embedding.len() as u32).to_le_bytes());
            b.extend_from_slice(&encode_f16(&e.embedding));
        }
        b
    }

    /// Inverse of the three writers.
    pub fn read(heads: &[u8], knn: &[u8], temperatures: &[u8]) -> anyhow::Result<Self> {
        let heads: Heads = serde_json::from_slice(heads)?;
        let temperatures: BTreeMap<String, f32> = serde_json::from_slice(temperatures)?;
        anyhow::ensure!(
            knn.len() >= 8 && &knn[..4] == KNN_MAGIC,
            "knn.bin: bad magic"
        );
        let mut at = 4;
        let take = |at: &mut usize, n: usize| -> anyhow::Result<&[u8]> {
            let s = knn
                .get(*at..*at + n)
                .ok_or_else(|| anyhow::anyhow!("knn.bin: truncated"))?;
            *at += n;
            Ok(s)
        };
        let u32_at = |at: &mut usize| -> anyhow::Result<u32> {
            Ok(u32::from_le_bytes(take(at, 4)?.try_into()?))
        };
        let n = u32_at(&mut at)? as usize;
        let mut entries = Vec::with_capacity(n);
        for _ in 0..n {
            let len = u32_at(&mut at)? as usize;
            let class = std::str::from_utf8(take(&mut at, len)?)?.to_owned();
            let option = u32_at(&mut at)?;
            let weight = f32::from_le_bytes(take(&mut at, 4)?.try_into()?);
            let asked_at_ms = i64::from_le_bytes(take(&mut at, 8)?.try_into()?);
            let dim = u32_at(&mut at)? as usize;
            let embedding = decode_f16(take(&mut at, dim * 2)?);
            entries.push(KnnEntry {
                class,
                option,
                weight,
                asked_at_ms,
                embedding,
            });
        }
        Ok(Self {
            heads,
            knn: entries,
            temperatures,
        })
    }

    /// The k-NN vote of `embedding` over `options` (the card's, `none` last) in
    /// `class`, as of `now`: cosine, k = 15, weights `exp(−age_days/30)` × label
    /// weight, Laplace floor. Uniform with no memory.
    pub fn knn_vote(
        &self,
        class: &str,
        options: &[String],
        embedding: &[f32],
        now: DateTime<Utc>,
    ) -> Vec<f32> {
        let Some(vocab) = self.heads.classes.get(class) else {
            return uniform(options.len());
        };
        let now_ms = now.timestamp_millis();
        let rows = self
            .knn
            .iter()
            .filter(|e| e.class == class && e.asked_at_ms <= now_ms);
        let labels: Vec<(&str, f64)> = rows
            .map(|e| {
                let age_days = (now_ms - e.asked_at_ms) as f64 / 86_400_000.0;
                let label = vocab
                    .options
                    .get(e.option as usize)
                    .map_or(NONE, String::as_str);
                (
                    label,
                    (-age_days / DECAY_DAYS).exp()
                        * f64::from(e.weight)
                        * cosine(&e.embedding, embedding),
                )
            })
            .collect();
        vote(options, labels)
    }

    /// Calibrated probabilities over `options` (the card's options with `none`
    /// last) for one card: oracle probs ‖ k-NN vote ‖ class prior through the
    /// class head (or the global one), divided by the class temperature.
    pub fn predict(
        &self,
        class: &str,
        requester: &str,
        options: &[String],
        oracle: &[f32],
        knn: &[f32],
    ) -> Vec<f32> {
        let feats = self.features(class, options, oracle, knn);
        let head = self
            .heads
            .per_class
            .get(class)
            .unwrap_or(&self.heads.global);
        let t = f64::from(*self.temperatures.get(class).unwrap_or(&1.0)).max(1e-3);
        let logits: Vec<f64> = head
            .logits(&feats, &ctx(requester_idx(requester), risk_idx(class)))
            .into_iter()
            .map(|l| l / t)
            .collect();
        softmax(&logits).into_iter().map(|p| p as f32).collect()
    }

    fn features(
        &self,
        class: &str,
        options: &[String],
        oracle: &[f32],
        knn: &[f32],
    ) -> Vec<[f64; OPT_F]> {
        let vocab = self.heads.classes.get(class);
        options
            .iter()
            .enumerate()
            .map(|(i, o)| {
                let prior = vocab
                    .and_then(|v| {
                        v.options
                            .iter()
                            .position(|x| x == o)
                            .and_then(|k| v.prior.get(k))
                    })
                    .map_or(1.0 / options.len() as f64, |p| f64::from(*p));
                [
                    f64::from(*oracle.get(i).unwrap_or(&0.0)),
                    f64::from(*knn.get(i).unwrap_or(&0.0)),
                    prior,
                    f64::from(u8::from(o == NONE)),
                ]
            })
            .collect()
    }
}

fn uniform(n: usize) -> Vec<f32> {
    vec![1.0 / n.max(1) as f32; n]
}

fn cosine(a: &[f32], b: &[f32]) -> f64 {
    let (mut ab, mut aa, mut bb) = (0.0f64, 0.0f64, 0.0f64);
    for (x, y) in a.iter().zip(b) {
        ab += f64::from(*x) * f64::from(*y);
        aa += f64::from(*x) * f64::from(*x);
        bb += f64::from(*y) * f64::from(*y);
    }
    if aa == 0.0 || bb == 0.0 {
        0.0
    } else {
        ab / (aa.sqrt() * bb.sqrt())
    }
}

/// The top-k weighted labels → a floored probability vector over `options`.
fn vote(options: &[String], mut labels: Vec<(&str, f64)>) -> Vec<f32> {
    labels.retain(|(_, w)| *w > 0.0);
    labels.sort_by(|a, b| b.1.total_cmp(&a.1));
    labels.truncate(K);
    let mut v: Vec<f64> = vec![FLOOR; options.len().max(1)];
    for (label, w) in labels {
        if let Some(i) = options.iter().position(|o| o == label) {
            v[i] += w;
        }
    }
    let z: f64 = v.iter().sum();
    v.into_iter().map(|x| (x / z) as f32).collect()
}

// --- the corpus ----------------------------------------------------------

/// One labelled example as the trainer sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct Example {
    /// `examples.id`.
    pub id: String,
    /// The class key.
    pub class: String,
    /// `examples.requester`.
    pub requester: String,
    /// Card options in card order, `none` appended.
    pub options: Vec<String>,
    /// Pooled embedding; rows without one join no fit.
    pub embedding: Vec<f32>,
    /// When raised.
    pub asked_at: DateTime<Utc>,
    /// Latest label time.
    pub labelled_at: DateTime<Utc>,
    /// Zero-shot probabilities over `options` (the `baseline` row); uniform when absent.
    pub oracle: Vec<f32>,
    /// Net label weight per option, ≥ 0, Σ > 0.
    pub target: Vec<f32>,
    /// In the `golden` table.
    pub golden: bool,
}

/// `(source, choice, weight)` as the `labels` table has it.
pub type Label = (LabelSource, String, f32);

/// Net label weights over `options` from the §6 rows: positive sources add
/// their weight to their choice, hindsight sources subtract it; negatives are
/// clamped at zero. `None` when nothing positive remains.
pub fn net_target(options: &[String], labels: &[Label]) -> Option<Vec<f32>> {
    let mut t = vec![0.0f32; options.len()];
    for (source, choice, weight) in labels {
        let Some(i) = options.iter().position(|o| o == choice) else {
            continue;
        };
        match source {
            LabelSource::Direct | LabelSource::Override | LabelSource::Timeout => t[i] += weight,
            LabelSource::HindsightRewind
            | LabelSource::HindsightCorrection
            | LabelSource::HindsightInterrupt => t[i] -= weight,
            LabelSource::Excluded => return None,
        }
    }
    for v in t.iter_mut() {
        *v = v.max(0.0);
    }
    (t.iter().sum::<f32>() > 0.0).then_some(t)
}

/// Every example with an embedding and at least one label, in `asked_at` order.
pub async fn load_examples(store: &Store) -> anyhow::Result<Vec<Example>> {
    let golden: std::collections::HashSet<String> = store.golden().await?.into_iter().collect();
    let rows = sqlx::query(
        "SELECT e.id, e.class, e.requester, e.options, e.embedding, e.asked_at,
                l.source, l.choice, l.weight, l.at
         FROM examples e JOIN labels l ON l.example_id = e.id
         WHERE e.embedding IS NOT NULL
         ORDER BY e.asked_at, e.id, l.source",
    )
    .fetch_all(store.pool())
    .await?;
    let baseline = sqlx::query("SELECT example_id, probs FROM predictions WHERE version = ?")
        .bind(BASELINE)
        .fetch_all(store.pool())
        .await?;
    let mut oracle: HashMap<String, Vec<(String, f32)>> = HashMap::new();
    for r in baseline {
        let probs: Vec<(String, f32)> =
            serde_json::from_str(r.get::<String, _>("probs").as_str()).unwrap_or_default();
        oracle.insert(r.get("example_id"), probs);
    }
    // Group label rows by example (the query orders by example).
    let mut grouped: Vec<(Example, Vec<Label>)> = Vec::new();
    for r in rows {
        let id: String = r.get("id");
        let at = dt(r.get("at"));
        let label = (
            LabelSource::parse(r.get::<String, _>("source").as_str())
                .unwrap_or(LabelSource::Excluded),
            r.get::<String, _>("choice"),
            r.get::<f64, _>("weight") as f32,
        );
        match grouped.last_mut() {
            Some((ex, labels)) if ex.id == id => {
                ex.labelled_at = ex.labelled_at.max(at);
                labels.push(label);
            }
            _ => {
                let mut options: Vec<String> =
                    serde_json::from_str(r.get::<String, _>("options").as_str())
                        .unwrap_or_default();
                options.push(NONE.to_owned());
                let oracle = oracle
                    .get(&id)
                    .map_or_else(|| uniform(options.len()), |p| align_probs(&options, p));
                let ex = Example {
                    golden: golden.contains(&id),
                    id,
                    class: r.get("class"),
                    requester: r.get("requester"),
                    embedding: decode_f16(&r.get::<Vec<u8>, _>("embedding")),
                    asked_at: dt(r.get("asked_at")),
                    labelled_at: at,
                    oracle,
                    target: Vec::new(),
                    options,
                };
                grouped.push((ex, vec![label]));
            }
        }
    }
    Ok(grouped
        .into_iter()
        .filter_map(|(mut ex, labels)| {
            ex.target = net_target(&ex.options, &labels)?;
            Some(ex)
        })
        .collect())
}

fn dt(s: String) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(&s)
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_default()
}

/// Baseline probs by label onto `options`; missing labels get 0, then renormalised (uniform if empty).
fn align_probs(options: &[String], probs: &[(String, f32)]) -> Vec<f32> {
    let mut v: Vec<f32> = options
        .iter()
        .map(|o| probs.iter().find(|(l, _)| l == o).map_or(0.0, |(_, p)| *p))
        .collect();
    let z: f32 = v.iter().sum();
    if z <= 0.0 {
        return uniform(options.len());
    }
    for x in v.iter_mut() {
        *x /= z;
    }
    v
}

/// Mark the golden rows (§2): per class, the first `max(5, ⌈10 %⌉)` rows by
/// `asked_at` that are not yet golden join the set. Idempotent; the set only grows.
pub async fn assign_golden(store: &Store, examples: &mut [Example]) -> anyhow::Result<usize> {
    let mut by_class: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (i, e) in examples.iter().enumerate() {
        by_class.entry(e.class.as_str()).or_default().push(i);
    }
    let mut added = Vec::new();
    for idxs in by_class.values() {
        let want = golden_target(idxs.len());
        let have = idxs.iter().filter(|&&i| examples[i].golden).count();
        for &i in idxs
            .iter()
            .filter(|&&i| !examples[i].golden)
            .take(want.saturating_sub(have))
        {
            added.push(i);
        }
    }
    for &i in &added {
        store.put_golden(&examples[i].id).await?;
        examples[i].golden = true;
    }
    Ok(added.len())
}

/// Golden rows a class of `n` rows owes: `max(5, ⌈n/10⌉)`, at most `n`.
pub fn golden_target(n: usize) -> usize {
    ((n as f64 * GOLDEN_SHARE).ceil() as usize)
        .max(GOLDEN_MIN)
        .min(n)
}

// --- the fit -------------------------------------------------------------

/// Fit temperatures, k-NN index and heads over the non-golden rows of
/// `examples`, as of `now`. Deterministic.
pub fn fit(examples: &[Example], now: DateTime<Utc>) -> Artifacts {
    let train: Vec<&Example> = examples.iter().filter(|e| !e.golden).collect();

    // Vocabularies and priors.
    let mut classes: BTreeMap<String, ClassVocab> = BTreeMap::new();
    let mut counts: BTreeMap<String, BTreeMap<String, f64>> = BTreeMap::new();
    for e in &train {
        let c = counts.entry(e.class.clone()).or_default();
        for (o, t) in e.options.iter().zip(&e.target) {
            *c.entry(o.clone()).or_default() += f64::from(*t);
        }
    }
    for (class, c) in &counts {
        let mut options: Vec<String> = c.keys().filter(|o| *o != NONE).cloned().collect();
        options.push(NONE.to_owned());
        let z: f64 = c.values().sum::<f64>() + options.len() as f64;
        let prior = options
            .iter()
            .map(|o| ((c.get(o).copied().unwrap_or(0.0) + 1.0) / z) as f32)
            .collect();
        let n = train.iter().filter(|e| &e.class == class).count() as u32;
        classes.insert(class.clone(), ClassVocab { options, prior, n });
    }

    // k-NN index.
    let mut knn: Vec<KnnEntry> = train
        .iter()
        .map(|e| {
            let (best, w) = e
                .target
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.total_cmp(b.1).then(b.0.cmp(&a.0)))
                .map(|(i, w)| (i, *w))
                .unwrap_or((0, 0.0));
            let label = &e.options[best];
            let option = classes[&e.class]
                .options
                .iter()
                .position(|o| o == label)
                .unwrap_or(0) as u32;
            KnnEntry {
                class: e.class.clone(),
                option,
                weight: w,
                asked_at_ms: e.asked_at.timestamp_millis(),
                embedding: e.embedding.clone(),
            }
        })
        .collect();
    knn.sort_by(|a, b| {
        a.class
            .cmp(&b.class)
            .then(a.asked_at_ms.cmp(&b.asked_at_ms))
            .then(a.option.cmp(&b.option))
    });

    let mut art = Artifacts {
        heads: Heads {
            global: Head::zero(),
            per_class: BTreeMap::new(),
            classes,
        },
        knn,
        temperatures: BTreeMap::new(),
    };

    // Heads: features as the predictor would have seen them (k-NN over strictly earlier rows).
    let fits: Vec<(String, Fit)> = train
        .iter()
        .map(|e| {
            let before = e.asked_at - Duration::milliseconds(1);
            let knn = art.knn_vote(&e.class, &e.options, &e.embedding, before);
            let opts = art.features(&e.class, &e.options, &e.oracle, &knn);
            let weight: f64 = e.target.iter().map(|t| f64::from(*t)).sum();
            let target = e.target.iter().map(|t| f64::from(*t) / weight).collect();
            (
                e.class.clone(),
                Fit {
                    opts,
                    ctx: ctx(requester_idx(&e.requester), risk_idx(&e.class)),
                    target,
                    weight,
                },
            )
        })
        .collect();
    let all: Vec<Fit> = fits.iter().map(|(_, f)| clone_fit(f)).collect();
    art.heads.global = fit_head(&all);
    for class in art.heads.classes.keys().cloned().collect::<Vec<_>>() {
        let rows: Vec<Fit> = fits
            .iter()
            .filter(|(c, _)| *c == class)
            .map(|(_, f)| clone_fit(f))
            .collect();
        if rows.len() >= PER_CLASS_MIN {
            art.heads.per_class.insert(class, fit_head(&rows));
        }
    }

    // Temperatures on the held-out golden rows (fixed grid, §7.5).
    let mut by_class: BTreeMap<&str, Vec<&Example>> = BTreeMap::new();
    for e in examples.iter().filter(|e| e.golden) {
        by_class.entry(e.class.as_str()).or_default().push(e);
    }
    for (class, rows) in by_class {
        let scored: Vec<(Vec<f64>, Vec<f64>)> = rows
            .iter()
            .map(|e| {
                let knn = art.knn_vote(class, &e.options, &e.embedding, now);
                let feats = art.features(class, &e.options, &e.oracle, &knn);
                let head = art.heads.per_class.get(class).unwrap_or(&art.heads.global);
                let logits =
                    head.logits(&feats, &ctx(requester_idx(&e.requester), risk_idx(class)));
                let w: f32 = e.target.iter().sum();
                (logits, e.target.iter().map(|t| f64::from(*t / w)).collect())
            })
            .collect();
        let mut best = (f64::INFINITY, 1.0f32);
        for step in 0..=50u32 {
            let t = 0.5 + step as f64 * 0.05;
            let nll: f64 = scored
                .iter()
                .map(|(l, tg)| {
                    let p = softmax(&l.iter().map(|x| x / t).collect::<Vec<_>>());
                    -tg.iter()
                        .zip(&p)
                        .map(|(t, p)| t * p.max(1e-12).ln())
                        .sum::<f64>()
                })
                .sum();
            if nll < best.0 - 1e-12 {
                best = (nll, t as f32);
            }
        }
        art.temperatures.insert(class.to_owned(), best.1);
    }
    art
}

fn clone_fit(f: &Fit) -> Fit {
    Fit {
        opts: f.opts.clone(),
        ctx: f.ctx,
        target: f.target.clone(),
        weight: f.weight,
    }
}

// --- measurement ---------------------------------------------------------

/// The gate's numbers over one row set (§7.4 subset).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Metrics {
    /// Rows scored.
    pub n: u32,
    /// Top-1 equals the heaviest label.
    pub accuracy: f64,
    /// Mean −log p(label).
    pub log_loss: f64,
    /// Mean squared error of the probability vector against the label.
    pub brier: f64,
    /// Expected calibration error, 10 equal-width bins of the top probability.
    pub ece: f64,
}

/// Metrics over `(probs, target)` pairs; targets are the net label weights.
pub fn metrics(rows: &[(Vec<f32>, Vec<f32>)]) -> Metrics {
    let mut m = Metrics::default();
    if rows.is_empty() {
        return m;
    }
    let mut bins = vec![(0.0f64, 0.0f64, 0u32); 10];
    for (p, t) in rows {
        let z: f32 = t.iter().sum();
        let label = t
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1).then(b.0.cmp(&a.0)))
            .map_or(0, |x| x.0);
        let top = p
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1).then(b.0.cmp(&a.0)))
            .map_or(0, |x| x.0);
        let hit = f64::from(u8::from(top == label));
        m.accuracy += hit;
        m.log_loss -= t
            .iter()
            .zip(p)
            .map(|(t, p)| f64::from(t / z) * f64::from(p.max(1e-7)).ln())
            .sum::<f64>();
        m.brier += p
            .iter()
            .zip(t)
            .map(|(p, t)| (f64::from(*p) - f64::from(t / z)).powi(2))
            .sum::<f64>();
        let conf = f64::from(p[top]);
        let b = ((conf * 10.0) as usize).min(9);
        bins[b].0 += conf;
        bins[b].1 += hit;
        bins[b].2 += 1;
    }
    let n = rows.len() as f64;
    m.n = rows.len() as u32;
    m.accuracy /= n;
    m.log_loss /= n;
    m.brier /= n;
    m.ece = bins
        .iter()
        .filter(|b| b.2 > 0)
        .map(|b| (b.2 as f64 / n) * ((b.0 - b.1) / b.2 as f64).abs())
        .sum();
    m
}

/// Score `rows` with `art` (or, with `None`, the zero-shot oracle alone) as of `now`.
pub fn score(
    art: Option<&Artifacts>,
    rows: &[&Example],
    now: DateTime<Utc>,
) -> Vec<(Vec<f32>, Vec<f32>)> {
    rows.iter()
        .map(|e| {
            let p = match art {
                Some(a) => {
                    let knn = a.knn_vote(&e.class, &e.options, &e.embedding, now);
                    a.predict(&e.class, &e.requester, &e.options, &e.oracle, &knn)
                }
                None => e.oracle.clone(),
            };
            (p, e.target.clone())
        })
        .collect()
}

/// The promotion gate (§7.3): `Ok` to activate, `Err(which)` otherwise.
/// (a) is skipped when there are no prequential rows yet (cold start); (b)
/// needs at least one golden row.
pub fn gate(
    candidate_proxy: Metrics,
    incumbent_proxy: Metrics,
    candidate_golden: Metrics,
    incumbent_golden: Metrics,
    candidate_all: Metrics,
) -> Result<(), String> {
    if candidate_golden.n == 0 {
        return Err("rejected: no golden rows".into());
    }
    if candidate_proxy.n > 0 && candidate_proxy.log_loss >= incumbent_proxy.log_loss {
        return Err(format!(
            "rejected: proxy log-loss {:.3} ≥ active {:.3}",
            candidate_proxy.log_loss, incumbent_proxy.log_loss
        ));
    }
    if candidate_golden.accuracy < incumbent_golden.accuracy - MAX_GOLDEN_DROP {
        return Err(format!(
            "rejected: golden accuracy {:.3} < active {:.3} − 0.01",
            candidate_golden.accuracy, incumbent_golden.accuracy
        ));
    }
    if candidate_all.ece > MAX_ECE {
        return Err(format!("rejected: ECE {:.3} > 0.05", candidate_all.ece));
    }
    Ok(())
}

/// Metrics JSON the `versions` row and the event carry.
pub fn metrics_json(
    proxy: Metrics,
    golden: Metrics,
    all: Metrics,
    incumbent_proxy: Metrics,
    incumbent_golden: Metrics,
    verdict: &Result<(), String>,
) -> serde_json::Value {
    serde_json::json!({
        "proxy_7d": proxy,
        "golden": golden,
        "all": all,
        "incumbent": { "proxy_7d": incumbent_proxy, "golden": incumbent_golden },
        "verdict": match verdict { Ok(()) => "promoted".to_owned(), Err(e) => e.clone() },
    })
}

// --- the run -------------------------------------------------------------

/// What one Tier A run produced.
#[derive(Clone, Debug)]
pub struct Candidate {
    /// The `versions` row (inactive until the scheduler activates it).
    pub version: VersionRow,
    /// The fitted artefacts.
    pub artifacts: Artifacts,
    /// `Ok` → activate.
    pub verdict: Result<(), String>,
    /// Rows the fit used.
    pub trained_rows: u32,
}

/// `a-<sha256(data_range)[..12]>`: the same corpus yields the same id.
pub fn version_id(data_range: &str) -> String {
    use sha2::Digest as _;
    let h = sha2::Sha256::digest(data_range.as_bytes());
    format!("a-{}", hex12(&h))
}

fn hex12(h: &[u8]) -> String {
    h.iter().take(6).map(|b| format!("{b:02x}")).collect()
}

/// One Tier A run over the store, as of `now`: load, mark golden, fit, score
/// the candidate and the incumbent (`active`'s artefacts, or the baseline)
/// on the last-7-days rows and golden, decide. `Ok(None)` when there is
/// nothing to fit or the corpus has not changed since the last version.
pub async fn run(
    store: &Store,
    active: Option<&Artifacts>,
    base_hash: &str,
    now: DateTime<Utc>,
) -> anyhow::Result<Option<Candidate>> {
    let started = std::time::Instant::now();
    let mut examples = load_examples(store).await?;
    if examples.is_empty() {
        return Ok(None);
    }
    assign_golden(store, &mut examples).await?;
    let train: Vec<&Example> = examples.iter().filter(|e| !e.golden).collect();
    if train.is_empty() {
        return Ok(None);
    }
    let first = train.iter().map(|e| e.asked_at).min().unwrap_or(now);
    let last = train.iter().map(|e| e.labelled_at).max().unwrap_or(now);
    let data_range = format!("{}..{}", first.to_rfc3339(), last.to_rfc3339());
    let id = version_id(&data_range);
    if store.versions().await?.iter().any(|v| v.id == id) {
        return Ok(None);
    }

    let artifacts = fit(&examples, now);
    let golden: Vec<&Example> = examples.iter().filter(|e| e.golden).collect();
    let since = now - Duration::days(PROXY_DAYS);
    let recent: Vec<&Example> = examples
        .iter()
        .filter(|e| !e.golden && e.asked_at >= since)
        .collect();
    let all: Vec<&Example> = golden.iter().chain(&recent).copied().collect();

    let c_proxy = metrics(&score(Some(&artifacts), &recent, now));
    let i_proxy = metrics(&score(active, &recent, now));
    let c_golden = metrics(&score(Some(&artifacts), &golden, now));
    let i_golden = metrics(&score(active, &golden, now));
    let c_all = metrics(&score(Some(&artifacts), &all, now));
    let verdict = gate(c_proxy, i_proxy, c_golden, i_golden, c_all);
    let metrics = metrics_json(c_proxy, c_golden, c_all, i_proxy, i_golden, &verdict);

    Ok(Some(Candidate {
        version: VersionRow {
            id,
            tier: Tier::A,
            base_hash: base_hash.to_owned(),
            adapter_path: None,
            head_path: None,
            knn_path: None,
            metrics,
            data_range,
            trained_at: now,
            venue: "local".into(),
            duration_s: started.elapsed().as_secs() as u32,
            active: false,
        },
        artifacts,
        verdict,
        trained_rows: train.len() as u32,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn t0() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap()
    }

    /// `n` rows of one class: `Allow` cluster around +e1, `Deny` around −e1.
    fn corpus(class: &str, n: usize, golden_first: usize) -> Vec<Example> {
        (0..n)
            .map(|i| {
                let allow = i % 4 != 3;
                let mut emb = vec![0.0f32; 8];
                emb[0] = if allow { 1.0 } else { -1.0 };
                emb[1 + (i % 7)] = 0.3;
                let options = vec!["Allow".to_owned(), "Deny".to_owned(), NONE.to_owned()];
                let target = if allow {
                    vec![1.0, 0.0, 0.0]
                } else {
                    vec![0.0, 1.0, 0.0]
                };
                Example {
                    id: format!("e{i:03}"),
                    class: class.into(),
                    requester: "root".into(),
                    options,
                    embedding: emb,
                    asked_at: t0() + Duration::hours(i as i64),
                    labelled_at: t0() + Duration::hours(i as i64) + Duration::minutes(1),
                    // Zero-shot is near chance (the honest starting point).
                    oracle: vec![0.4, 0.35, 0.25],
                    target,
                    golden: i < golden_first,
                }
            })
            .collect()
    }

    #[test]
    fn fit_is_deterministic_and_beats_the_oracle() {
        let ex = corpus("approval:shell:mutating:git push:root", 40, 4);
        let now = t0() + Duration::days(3);
        let a = fit(&ex, now);
        let b = fit(&ex, now);
        assert_eq!(a.heads_json().unwrap(), b.heads_json().unwrap());
        assert_eq!(a.knn_bin(), b.knn_bin());
        assert_eq!(
            a.temperatures_json().unwrap(),
            b.temperatures_json().unwrap()
        );
        assert!(
            a.heads
                .per_class
                .contains_key("approval:shell:mutating:git push:root")
        );

        let golden: Vec<&Example> = ex.iter().filter(|e| e.golden).collect();
        let cand = metrics(&score(Some(&a), &golden, now));
        let base = metrics(&score(None, &golden, now));
        assert!(cand.accuracy >= base.accuracy, "{cand:?} vs {base:?}");
        assert!(cand.log_loss < base.log_loss, "{cand:?} vs {base:?}");

        let read = Artifacts::read(
            &a.heads_json().unwrap(),
            &a.knn_bin(),
            &a.temperatures_json().unwrap(),
        )
        .unwrap();
        // serde_json's default float parser may land one ulp off (no `float_roundtrip`);
        // the files are byte-identical, the read-back is equal to 1e-12.
        let close = |x: &Head, y: &Head| {
            x.w.iter()
                .flatten()
                .zip(y.w.iter().flatten())
                .all(|(p, q)| (p - q).abs() < 1e-12)
        };
        assert!(close(&read.heads.global, &a.heads.global));
        assert_eq!(read.heads.per_class.len(), a.heads.per_class.len());
        assert_eq!(read.heads.classes, a.heads.classes);
        assert_eq!(read.knn.len(), a.knn.len());
        assert_eq!(read.temperatures, a.temperatures);
    }

    #[test]
    fn knn_vote_uses_only_earlier_rows_and_floors_unseen_options() {
        let ex = corpus("c", 12, 0);
        let a = fit(&ex, t0() + Duration::days(1));
        let opts = vec!["Allow".to_owned(), "Deny".to_owned(), NONE.to_owned()];
        // Before the first row: nothing to vote with → uniform.
        let v = a.knn_vote(
            "c",
            &opts,
            &[1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            t0() - Duration::hours(1),
        );
        assert!((v[0] - v[1]).abs() < 1e-6 && (v[1] - v[2]).abs() < 1e-6);
        // After: an Allow-shaped query leans Allow; `none` keeps the floor.
        let v = a.knn_vote(
            "c",
            &opts,
            &[1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            t0() + Duration::days(1),
        );
        assert!(v[0] > v[1] && v[2] > 0.0, "{v:?}");
        // Unknown class → uniform.
        assert_eq!(a.knn_vote("other", &opts, &[1.0; 8], t0()), uniform(3));
    }

    #[test]
    fn gate_accepts_and_rejects_on_synthetic_metrics() {
        let m = |n, acc, ll, ece| Metrics {
            n,
            accuracy: acc,
            log_loss: ll,
            brier: 0.0,
            ece,
        };
        let good = m(30, 0.95, 0.20, 0.02);
        let inc = m(30, 0.90, 0.40, 0.03);
        assert_eq!(gate(good, inc, good, inc, good), Ok(()));
        // Cold start: no proxy rows → (a) skipped.
        assert_eq!(
            gate(m(0, 0.0, 0.0, 0.0), m(0, 0.0, 0.0, 0.0), good, inc, good),
            Ok(())
        );
        // Worse proxy log-loss.
        assert!(
            gate(m(30, 0.95, 0.45, 0.02), inc, good, inc, good)
                .unwrap_err()
                .contains("proxy")
        );
        // Golden regression beyond a point.
        assert!(
            gate(
                good,
                inc,
                m(10, 0.85, 0.3, 0.02),
                m(10, 0.90, 0.3, 0.02),
                good
            )
            .unwrap_err()
            .contains("golden accuracy")
        );
        // Exactly one point down is tolerated.
        assert_eq!(
            gate(
                good,
                inc,
                m(10, 0.89, 0.3, 0.02),
                m(10, 0.90, 0.3, 0.02),
                good
            ),
            Ok(())
        );
        // Miscalibrated.
        assert!(
            gate(good, inc, good, inc, m(30, 0.95, 0.2, 0.08))
                .unwrap_err()
                .contains("ECE")
        );
        // No golden rows at all.
        assert!(
            gate(good, inc, m(0, 0.0, 0.0, 0.0), inc, good)
                .unwrap_err()
                .contains("no golden")
        );
    }

    #[test]
    fn metrics_are_sane() {
        let rows = vec![
            (vec![0.9, 0.1], vec![1.0, 0.0]),
            (vec![0.2, 0.8], vec![1.0, 0.0]),
        ];
        let m = metrics(&rows);
        assert_eq!(m.n, 2);
        assert!((m.accuracy - 0.5).abs() < 1e-9);
        assert!(m.log_loss > 0.0 && m.brier > 0.0 && m.ece >= 0.0 && m.ece <= 1.0);
        assert_eq!(metrics(&[]), Metrics::default());
    }

    #[test]
    fn net_target_sums_sources_and_clamps() {
        let opts = vec!["Allow".to_owned(), "Deny".to_owned(), NONE.to_owned()];
        let t = net_target(
            &opts,
            &[
                (LabelSource::Direct, "Allow".into(), 1.0),
                (LabelSource::HindsightRewind, "Allow".into(), 0.5),
                (LabelSource::Timeout, "Deny".into(), 0.25),
            ],
        )
        .unwrap();
        assert_eq!(t, vec![0.5, 0.25, 0.0]);
        assert!(
            net_target(
                &opts,
                &[(LabelSource::HindsightRewind, "Allow".into(), 0.5)]
            )
            .is_none()
        );
        assert!(net_target(&opts, &[(LabelSource::Excluded, "Allow".into(), 0.0)]).is_none());
    }

    #[test]
    fn golden_target_is_ten_percent_min_five() {
        assert_eq!(golden_target(3), 3);
        assert_eq!(golden_target(5), 5);
        assert_eq!(golden_target(40), 5);
        assert_eq!(golden_target(60), 6);
        assert_eq!(golden_target(123), 13);
    }

    #[test]
    fn requester_and_risk_indices() {
        assert_eq!(requester_idx("root"), 0);
        assert_eq!(requester_idx("subagent"), 1);
        assert_eq!(requester_idx("desk:ops"), 2);
        assert_eq!(requester_idx("routine:nightly"), 3);
        assert_eq!(risk_idx("approval:shell:destructive:rm -rf:root"), 3);
        assert_eq!(risk_idx("approval:edit:mutating:.theuth:root"), 2);
        assert_eq!(risk_idx("approval:knowledge_put:read_only:root"), 1);
        assert_eq!(risk_idx("ask:b9474647c6a4:root"), 0);
    }
}
