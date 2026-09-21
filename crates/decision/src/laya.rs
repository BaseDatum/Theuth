//! laya's input rendering, ONNX session and post-processing (57 §1), ported from
//! `rl_common.py` (`render_options`, `build_sequence`, `collate_items`, `temp_bucket`,
//! `confidence_from_probs`) and `agent.py` (`system_one`). [`Loaded`] owns the ort
//! `Session`; everything around it is a pure function over a `tokenizers::Tokenizer`.
//!
//! Sequence: `[CLS] <qtype> question: <instructions> [SEP] [MASK] opt0 [MASK] opt1 … [SEP] <state> [SEP]`.
//! Each option is capped at [`OPTION_CAP`] tokens and shrunk evenly when the head budget
//! (`head_max_len`) runs out; the instructions get what the options leave (≥ 8 tokens); the
//! state gets the rest of `max_len` and is right-truncated.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;

use half::f16;
use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;
use ort::value::{Outlet, Tensor, TensorElementType, ValueType};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokenizers::Tokenizer;

use crate::question::Question;

/// `[CLS]` in laya's tokenizer.
pub const CLS: i64 = 50281;
/// `[SEP]`.
pub const SEP: i64 = 50282;
/// `[PAD]`.
pub const PAD: i64 = 50283;
/// `[MASK]`: one per option; the head scores its hidden state.
pub const MASK: i64 = 50284;
/// The mask token's text, replaced by a space wherever user text contains it.
pub const MASK_TEXT: &str = "[MASK]";
/// Tokens per option before the head budget shrinks them.
pub const OPTION_CAP: usize = 48;

/// Question type, the model's `qtype` input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QType {
    /// Pick one of N.
    Choice = 0,
    /// Ordinal level.
    Score = 1,
    /// Binary truth ("noul").
    Noul = 2,
}

impl QType {
    /// The text prefix and `temp_bucket` name.
    pub fn name(self) -> &'static str {
        match self {
            QType::Choice => "choice",
            QType::Score => "score",
            QType::Noul => "noul",
        }
    }

    /// Of a question.
    pub fn of(q: &Question) -> Self {
        match q {
            Question::Choice { .. } => QType::Choice,
            Question::Score { .. } => QType::Score,
            Question::Bool { .. } => QType::Noul,
        }
    }
}

/// The fields of `rl_agent_config.json` inference needs.
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct LayaConfig {
    /// Sequence budget.
    pub max_len: usize,
    /// Budget for instructions + options.
    pub head_max_len: usize,
    /// Per-qtype temperature, index = [`QType`].
    pub temperature: [f32; 3],
    /// Per-cardinality temperature, keyed like `choice:3-5`; wins over `temperature`.
    pub temperature_by_options: BTreeMap<String, f32>,
}

impl Default for LayaConfig {
    fn default() -> Self {
        LayaConfig {
            max_len: 512,
            head_max_len: 192,
            temperature: [1.0; 3],
            temperature_by_options: BTreeMap::new(),
        }
    }
}

impl LayaConfig {
    /// `temp_bucket` + lookup (agent.py `system_one`).
    pub fn temperature(&self, qtype: QType, k: usize) -> f32 {
        let size = match k {
            0..=2 => "2",
            3..=5 => "3-5",
            6..=10 => "6-10",
            _ => "11+",
        };
        self.temperature_by_options
            .get(&format!("{}:{size}", qtype.name()))
            .copied()
            .unwrap_or(self.temperature[qtype as usize])
    }
}

/// One model row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sequence {
    /// Token ids, `[CLS] … [SEP]`, at most `max_len`.
    pub input_ids: Vec<i64>,
    /// All ones (padding is the batch's job).
    pub attention_mask: Vec<i64>,
    /// Position of each option's `[MASK]`, option order.
    pub marker_pos: Vec<i64>,
    /// All true for a single row.
    pub marker_mask: Vec<bool>,
    /// [`QType`] as the model reads it.
    pub qtype: i64,
    /// The state was cut, or an option's marker fell past `max_len` (57 §0.5 hard failure).
    pub truncated: bool,
}

/// Option texts in label order (`render_options`). Bool is always `[false, true]`.
pub fn render_options(q: &Question) -> Vec<String> {
    match q {
        Question::Choice { candidates, .. } => candidates
            .iter()
            .map(|c| {
                if c.text.is_empty() {
                    c.id.clone()
                } else {
                    format!("{}: {}", c.id, c.text)
                }
            })
            .collect(),
        Question::Score { levels, .. } => levels
            .iter()
            .enumerate()
            .map(|(i, c)| format!("level {i}: {c}"))
            .collect(),
        Question::Bool { .. } => vec![
            "false: no, the statement does not hold".to_owned(),
            "true: yes, the statement holds".to_owned(),
        ],
    }
}

/// The option labels [`render_options`] order maps back to: candidate ids, `0..n`, `false`/`true`.
pub fn labels(q: &Question) -> Vec<String> {
    match q {
        Question::Choice { candidates, .. } => candidates.iter().map(|c| c.id.clone()).collect(),
        Question::Score { levels, .. } => (0..levels.len()).map(|i| i.to_string()).collect(),
        Question::Bool { .. } => vec!["false".to_owned(), "true".to_owned()],
    }
}

fn encode(tok: &Tokenizer, text: &str) -> Result<Vec<i64>, String> {
    let enc = tok
        .encode(text.replace(MASK_TEXT, " "), false)
        .map_err(|e| format!("tokenize: {e}"))?;
    Ok(enc.get_ids().iter().map(|&i| i64::from(i)).collect())
}

/// `build_sequence` (rl_common.py), option order = insertion order, right-truncated state.
pub fn build_sequence(tok: &Tokenizer, q: &Question, cfg: &LayaConfig) -> Result<Sequence, String> {
    let qtype = QType::of(q);
    let (instructions, state) = match q {
        Question::Choice {
            instructions,
            state,
            ..
        }
        | Question::Score {
            instructions,
            state,
            ..
        } => (instructions.as_str(), state.as_str()),
        Question::Bool { statement, state } => (statement.as_str(), state.as_str()),
    };
    let mut head = encode(tok, &format!("{} question: {instructions}", qtype.name()))?;
    let mut opts: Vec<Vec<i64>> = Vec::new();
    for o in render_options(q) {
        let mut ids = encode(tok, &format!(" {o}"))?;
        ids.truncate(OPTION_CAP);
        ids.insert(0, MASK);
        opts.push(ids);
    }
    let used = |opts: &[Vec<i64>]| opts.iter().map(Vec::len).sum::<usize>();
    let mut budget = cfg.head_max_len as isize - used(&opts) as isize;
    if budget < 16 {
        // Too many / too long options: shrink every option evenly.
        let per = ((cfg.head_max_len.saturating_sub(16)) / opts.len().max(1)).max(4);
        for o in &mut opts {
            o.truncate(per);
        }
        budget = cfg.head_max_len as isize - used(&opts) as isize;
    }
    head.truncate(budget.max(8) as usize);
    let mut ids = Vec::with_capacity(cfg.max_len);
    ids.push(CLS);
    ids.extend(head);
    ids.push(SEP);
    let mut markers = Vec::with_capacity(opts.len());
    for o in &opts {
        markers.push(ids.len() as i64);
        ids.extend(o);
    }
    ids.push(SEP);
    let room = cfg.max_len.saturating_sub(ids.len() + 1);
    let mut st = encode(tok, state)?;
    let state_cut = st.len() > room;
    st.truncate(room);
    ids.extend(st);
    ids.push(SEP);
    ids.truncate(cfg.max_len);
    let k = markers.len();
    markers.retain(|&m| (m as usize) < cfg.max_len);
    let truncated = state_cut || markers.len() < k;
    Ok(Sequence {
        attention_mask: vec![1; ids.len()],
        marker_mask: vec![true; markers.len()],
        input_ids: ids,
        marker_pos: markers,
        qtype: qtype as i64,
        truncated,
    })
}

/// Softmax over the first `k` logits at temperature `t` (floored at 1e-3), as `system_one`.
pub fn softmax_with_temperature(logits: &[f32], k: usize, t: f32) -> Vec<f32> {
    let t = t.max(1e-3);
    let z: Vec<f32> = logits.iter().take(k).map(|l| l / t).collect();
    let max = z.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let e: Vec<f32> = z.iter().map(|v| (v - max).exp()).collect();
    let sum: f32 = e.iter().sum();
    e.into_iter().map(|v| v / sum).collect()
}

/// `confidence_from_probs`: 1 − normalised entropy over the first `k`; 1 when K < 2.
pub fn entropy_confidence(p: &[f32], k: usize) -> f32 {
    if k < 2 {
        return 1.0;
    }
    let ent: f32 = p
        .iter()
        .take(k)
        .map(|&v| {
            let v = v.clamp(1e-12, 1.0);
            -(v * v.ln())
        })
        .sum();
    1.0 - ent / (k as f32).ln()
}

/// What one row comes back as.
#[derive(Clone, Debug, PartialEq)]
pub struct RowOut {
    /// Raw marker logits (f32 from f16), option order, K of them.
    pub logits: Vec<f32>,
    /// Calibrated probabilities, option order, sum to 1.
    pub probs: Vec<f32>,
    /// Temperature applied.
    pub temperature: f32,
    /// `softmax(act_logits)[0]`.
    pub act_p: f32,
    /// Entropy-derived (choice/score) or `max(p, 1−p)` (noul).
    pub confidence: f32,
}

/// Rows padded to a common `[B, S]` / `[B, K]` (`collate_items`): `[PAD]` with attention 0,
/// marker position 0 with mask false.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Batch {
    /// Rows.
    pub b: usize,
    /// Longest row.
    pub s: usize,
    /// Most markers.
    pub k: usize,
    /// `[B, S]`.
    pub input_ids: Vec<i64>,
    /// `[B, S]`.
    pub attention_mask: Vec<i64>,
    /// `[B, K]`.
    pub marker_pos: Vec<i64>,
    /// `[B, K]`.
    pub marker_mask: Vec<bool>,
    /// `[B]`.
    pub qtype: Vec<i64>,
}

/// Pad `rows` into one batch.
pub fn collate(rows: &[Sequence]) -> Batch {
    let b = rows.len();
    let s = rows.iter().map(|r| r.input_ids.len()).max().unwrap_or(0);
    let k = rows.iter().map(|r| r.marker_pos.len()).max().unwrap_or(0);
    let mut batch = Batch {
        b,
        s,
        k,
        input_ids: Vec::with_capacity(b * s),
        attention_mask: Vec::with_capacity(b * s),
        marker_pos: Vec::with_capacity(b * k),
        marker_mask: Vec::with_capacity(b * k),
        qtype: Vec::with_capacity(b),
    };
    for r in rows {
        let n = r.input_ids.len();
        batch.input_ids.extend_from_slice(&r.input_ids);
        batch.input_ids.resize(batch.input_ids.len() + s - n, PAD);
        batch.attention_mask.extend(std::iter::repeat_n(1, n));
        batch.attention_mask.extend(std::iter::repeat_n(0, s - n));
        let m = r.marker_pos.len();
        batch.marker_pos.extend_from_slice(&r.marker_pos);
        batch.marker_pos.extend(std::iter::repeat_n(0, k - m));
        batch.marker_mask.extend(std::iter::repeat_n(true, m));
        batch.marker_mask.extend(std::iter::repeat_n(false, k - m));
        batch.qtype.push(r.qtype);
    }
    batch
}

/// `system_one`'s per-row tail: temperature by bucket, softmax over the row's K, act
/// probability, confidence. `logits` is the padded `[K_batch]` slice; `act` the two act logits.
pub fn postprocess(row: &Sequence, logits: &[f32], act: &[f32], cfg: &LayaConfig) -> RowOut {
    let k = row.marker_pos.len();
    let qtype = match row.qtype {
        1 => QType::Score,
        2 => QType::Noul,
        _ => QType::Choice,
    };
    let temperature = cfg.temperature(qtype, k);
    let probs = softmax_with_temperature(logits, k, temperature);
    let confidence = match qtype {
        QType::Noul if probs.len() == 2 => probs[1].max(1.0 - probs[1]),
        _ => entropy_confidence(&probs, k),
    };
    let act_p = softmax_with_temperature(act, act.len().min(2), 1.0)
        .first()
        .copied()
        .unwrap_or(0.0);
    RowOut {
        logits: logits[..k.min(logits.len())].to_vec(),
        probs,
        temperature,
        act_p,
        confidence,
    }
}

/// f16 → f32, the outputs' element type (`ort/half`).
pub fn f16_to_f32(v: &[f16]) -> Vec<f32> {
    v.iter().map(|x| x.to_f32()).collect()
}

/// The graph 57 §1 describes: names, element types and ranks, checked on load.
pub fn verify_graph(inputs: &[Outlet], outputs: &[Outlet]) -> Result<(), String> {
    const INPUTS: [(&str, TensorElementType, usize); 5] = [
        ("input_ids", TensorElementType::Int64, 2),
        ("attention_mask", TensorElementType::Int64, 2),
        ("marker_pos", TensorElementType::Int64, 2),
        ("marker_mask", TensorElementType::Bool, 2),
        ("qtype", TensorElementType::Int64, 1),
    ];
    const OUTPUTS: [(&str, TensorElementType, usize); 2] = [
        ("logits", TensorElementType::Float16, 2),
        ("act_logits", TensorElementType::Float16, 2),
    ];
    let check = |what: &str, have: &[Outlet], want: &[(&str, TensorElementType, usize)]| {
        if have.len() != want.len() {
            return Err(format!(
                "model.onnx: {} {what}s ({}), expected {} ({})",
                have.len(),
                have.iter().map(Outlet::name).collect::<Vec<_>>().join(", "),
                want.len(),
                want.iter().map(|w| w.0).collect::<Vec<_>>().join(", ")
            ));
        }
        for (name, ty, rank) in want {
            let o = have
                .iter()
                .find(|o| o.name() == *name)
                .ok_or_else(|| format!("model.onnx: no {what} `{name}`"))?;
            match o.dtype() {
                ValueType::Tensor { ty: t, shape, .. } if t == ty && shape.len() == *rank => {}
                other => {
                    return Err(format!(
                        "model.onnx: {what} `{name}` is {other}, expected {ty} rank {rank}"
                    ));
                }
            }
        }
        Ok(())
    };
    check("input", inputs, &INPUTS)?;
    check("output", outputs, &OUTPUTS)
}

/// The checkpoint: one warm ort `Session` (CPU EP), the tokenizer, the temperatures and
/// the model's sha256. `run` takes `&mut self` (rc.13), so one owner serialises callers.
pub struct Loaded {
    session: Session,
    /// laya's fast BPE tokenizer.
    pub tokenizer: Tokenizer,
    /// Budgets and temperatures from `rl_agent_config.json`.
    pub laya: LayaConfig,
    /// sha256 hex of `model.onnx`, part of every gate version.
    pub model_sha: String,
    /// Wall-clock of `open`.
    pub load_time: std::time::Duration,
}

impl Loaded {
    /// Open `dir/{model.onnx,tokenizer.json,rl_agent_config.json}` after the process-wide
    /// ONNX Runtime init, verify the graph, hash the model. `coreml` is read and refused
    /// (57 §2: a step-2 experiment; CPU EP until it is measured).
    pub fn open(dir: &Path, threads: usize, coreml: bool) -> anyhow::Result<Loaded> {
        let t = Instant::now();
        // The dylib ships beside the Parakeet TDT checkpoint (54 §9, `release-models.sh`
        // header), so a fresh daemon whose first `ort` user is this crate still finds it.
        let tdt = dir.join("..").join("parakeet-tdt");
        models::onnx::ort_lib(&[dir, &tdt]).map_err(anyhow::Error::msg)?;
        if coreml {
            tracing::warn!(
                "decision: coreml: true is not implemented; running on the CPU execution provider"
            );
        }
        let model = dir.join("model.onnx");
        let tokenizer = Tokenizer::from_file(dir.join("tokenizer.json"))
            .map_err(|e| anyhow::anyhow!("tokenizer.json: {e}"))?;
        let laya: LayaConfig =
            serde_json::from_slice(&std::fs::read(dir.join("rl_agent_config.json"))?)
                .map_err(|e| anyhow::anyhow!("rl_agent_config.json: {e}"))?;
        let mut hasher = Sha256::new();
        std::io::copy(&mut std::fs::File::open(&model)?, &mut hasher)?;
        let model_sha = format!("{:x}", hasher.finalize());
        let session = Session::builder()
            .map_err(ort_err)?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(ort_err)?
            .with_intra_threads(threads.max(1))
            .map_err(ort_err)?
            .commit_from_file(&model)
            .map_err(|e| anyhow::anyhow!("load {}: {e}", model.display()))?;
        verify_graph(session.inputs(), session.outputs()).map_err(anyhow::Error::msg)?;
        Ok(Loaded {
            session,
            tokenizer,
            laya,
            model_sha,
            load_time: t.elapsed(),
        })
    }

    /// [`build_sequence`] with this checkpoint's tokenizer and budgets.
    pub fn build(&self, q: &Question) -> Result<Sequence, String> {
        build_sequence(&self.tokenizer, q, &self.laya)
    }

    /// One forward pass over `rows` (padded together), one [`RowOut`] per row.
    pub fn run(&mut self, rows: &[Sequence]) -> anyhow::Result<Vec<RowOut>> {
        if rows.is_empty() {
            return Ok(Vec::new());
        }
        let batch = collate(rows);
        let (b, s, k) = (batch.b as i64, batch.s as i64, batch.k as i64);
        let outputs = self.session.run(ort::inputs![
            "input_ids" => Tensor::from_array((vec![b, s], batch.input_ids))?,
            "attention_mask" => Tensor::from_array((vec![b, s], batch.attention_mask))?,
            "marker_pos" => Tensor::from_array((vec![b, k], batch.marker_pos))?,
            "marker_mask" => Tensor::from_array((vec![b, k], batch.marker_mask))?,
            "qtype" => Tensor::from_array((vec![b], batch.qtype))?,
        ])?;
        let logits = outputs
            .get("logits")
            .ok_or_else(|| anyhow::anyhow!("model.onnx: no `logits` output"))?
            .try_extract_tensor::<f16>()?
            .1;
        let act = outputs
            .get("act_logits")
            .ok_or_else(|| anyhow::anyhow!("model.onnx: no `act_logits` output"))?
            .try_extract_tensor::<f16>()?
            .1;
        let (logits, act) = (f16_to_f32(logits), f16_to_f32(act));
        let kk = batch.k.max(1);
        let na = act.len() / batch.b.max(1);
        Ok(rows
            .iter()
            .enumerate()
            .map(|(i, row)| {
                postprocess(
                    row,
                    &logits[i * kk..((i + 1) * kk).min(logits.len())],
                    &act[i * na..((i + 1) * na).min(act.len())],
                    &self.laya,
                )
            })
            .collect())
    }
}

/// `ort::Error<R>` is not `Send + Sync` (it carries the failed builder); stringify.
fn ort_err<R>(e: ort::Error<R>) -> anyhow::Error {
    anyhow::anyhow!("{e}")
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::question::Candidate;

    /// A whitespace word-level tokenizer: every distinct word is one token, unknown → 0.
    fn tiny() -> Tokenizer {
        let words = "choice score noul question : deploy blocked on image tags level 0 1 2 false true \
                     no the statement does not hold yes holds a b c none e3 button sign in w";
        let mut vocab = serde_json::Map::new();
        vocab.insert("[UNK]".into(), 0.into());
        for w in words.split_whitespace() {
            let n = vocab.len();
            vocab.entry(w).or_insert(n.into());
        }
        let spec = serde_json::json!({
            "version": "1.0",
            "pre_tokenizer": { "type": "Whitespace" },
            "model": { "type": "WordLevel", "vocab": vocab, "unk_token": "[UNK]" },
        });
        Tokenizer::from_bytes(serde_json::to_vec(&spec).unwrap()).unwrap()
    }

    /// The real checkpoint's tokenizer, when a copy is around.
    fn real() -> Option<Tokenizer> {
        let home = std::env::var_os("HOME")?;
        let candidates = [
            std::path::PathBuf::from(&home).join(".theuth/models/decision/tokenizer.json"),
            std::path::PathBuf::from("/tmp/laya-design/tokenizer.json"),
        ];
        candidates
            .iter()
            .find(|p| p.is_file())
            .and_then(|p| Tokenizer::from_file(p).ok())
    }

    fn choice(n: usize, state: &str) -> Question {
        Question::Choice {
            instructions: "deploy blocked".into(),
            candidates: (0..n)
                .map(|i| Candidate {
                    id: format!("e{i}"),
                    text: "button".into(),
                })
                .collect(),
            state: state.into(),
        }
    }

    #[test]
    fn layout_and_markers() {
        let tok = tiny();
        let cfg = LayaConfig::default();
        let s = build_sequence(&tok, &choice(2, "image tags"), &cfg).unwrap();
        // [CLS] choice question : deploy blocked [SEP] [MASK] e0 : button [MASK] e1 : button [SEP] image tags [SEP]
        assert_eq!(s.input_ids[0], CLS);
        assert_eq!(s.input_ids[6], SEP);
        assert_eq!(s.marker_pos, vec![7, 11]);
        assert_eq!(s.input_ids[7], MASK);
        assert_eq!(s.input_ids[11], MASK);
        assert_eq!(s.input_ids[15], SEP);
        assert_eq!(*s.input_ids.last().unwrap(), SEP);
        assert_eq!(s.input_ids.len(), 19);
        assert_eq!(s.attention_mask, vec![1; 19]);
        assert_eq!(s.marker_mask, vec![true, true]);
        assert_eq!(s.qtype, 0);
        assert!(!s.truncated);
    }

    #[test]
    fn qtypes_and_fixed_options() {
        let tok = tiny();
        let cfg = LayaConfig::default();
        let b = Question::Bool {
            statement: "deploy blocked".into(),
            state: "w".into(),
        };
        let s = build_sequence(&tok, &b, &cfg).unwrap();
        assert_eq!(s.qtype, 2);
        assert_eq!(s.marker_pos.len(), 2);
        assert_eq!(
            render_options(&b),
            [
                "false: no, the statement does not hold",
                "true: yes, the statement holds"
            ]
        );
        assert_eq!(labels(&b), ["false", "true"]);
        let sc = Question::Score {
            instructions: "deploy".into(),
            levels: vec!["a".into(), "b".into(), "c".into()],
            state: "w".into(),
        };
        assert_eq!(build_sequence(&tok, &sc, &cfg).unwrap().qtype, 1);
        assert_eq!(
            render_options(&sc),
            ["level 0: a", "level 1: b", "level 2: c"]
        );
        assert_eq!(labels(&sc), ["0", "1", "2"]);
    }

    #[test]
    fn mask_text_is_stripped() {
        let tok = tiny();
        let q = choice(1, "image [MASK] tags");
        let s = build_sequence(&tok, &q, &LayaConfig::default()).unwrap();
        assert_eq!(s.input_ids.iter().filter(|&&i| i == MASK).count(), 1);
    }

    #[test]
    fn state_right_truncated_and_flagged() {
        let tok = tiny();
        let cfg = LayaConfig {
            max_len: 24,
            head_max_len: 16,
            ..LayaConfig::default()
        };
        let state = "image tags ".repeat(20);
        let s = build_sequence(&tok, &choice(1, &state), &cfg).unwrap();
        assert_eq!(s.input_ids.len(), 24);
        assert_eq!(*s.input_ids.last().unwrap(), SEP);
        assert!(s.truncated);
        // Right-truncated: the state's first tokens survive.
        let image = tok.encode("image", false).unwrap().get_ids()[0] as i64;
        let head_end = s.marker_pos[0] as usize + 4; // [MASK] e0 : button [SEP]
        assert_eq!(s.input_ids[head_end + 1], image);
    }

    #[test]
    fn options_shrink_evenly_when_head_overflows() {
        let tok = tiny();
        let cfg = LayaConfig {
            max_len: 128,
            head_max_len: 40,
            ..LayaConfig::default()
        };
        // 8 options × 4 tokens = 32 > 40 − 16 → per = max(4, 24 / 8) = 4: [MASK] + 3 tokens each.
        let s = build_sequence(&tok, &choice(8, "w"), &cfg).unwrap();
        assert_eq!(s.marker_pos.len(), 8);
        let gaps: Vec<i64> = s.marker_pos.windows(2).map(|w| w[1] - w[0]).collect();
        assert_eq!(gaps, vec![4; 7]);
        // Head budget 40 − 32 = 8 → instructions cut to max(8, 8) = 8 (here only 5 exist).
        assert_eq!(s.marker_pos[0], 1 + 5 + 1);
        assert!(!s.truncated);
    }

    #[test]
    fn markers_past_max_len_are_dropped_and_flagged() {
        let tok = tiny();
        let cfg = LayaConfig {
            max_len: 12,
            head_max_len: 192,
            ..LayaConfig::default()
        };
        let s = build_sequence(&tok, &choice(3, "w"), &cfg).unwrap();
        assert_eq!(s.input_ids.len(), 12);
        assert!(s.marker_pos.len() < 3);
        assert!(s.truncated);
    }

    #[test]
    fn temperatures() {
        let cfg: LayaConfig = serde_json::from_str(
            r#"{"max_len":512,"head_max_len":192,"temperature":[1.6,1.25,1.98],
                "temperature_by_options":{"choice:3-5":1.76,"noul:2":1.98,"choice:11+":0.1},
                "encoder":"x","training":{}}"#,
        )
        .unwrap();
        assert_eq!(cfg.temperature(QType::Choice, 4), 1.76);
        assert_eq!(cfg.temperature(QType::Choice, 2), 1.6);
        assert_eq!(cfg.temperature(QType::Choice, 12), 0.1);
        assert_eq!(cfg.temperature(QType::Noul, 2), 1.98);
        assert_eq!(cfg.temperature(QType::Score, 3), 1.25);
        let d: LayaConfig = serde_json::from_str("{}").unwrap();
        assert_eq!((d.max_len, d.head_max_len), (512, 192));
    }

    #[test]
    fn softmax_and_confidence() {
        let p = softmax_with_temperature(&[2.0, 0.0, -1e4, 9.0], 2, 1.0);
        assert_eq!(p.len(), 2);
        assert!((p.iter().sum::<f32>() - 1.0).abs() < 1e-6);
        assert!((p[0] - 0.880_797).abs() < 1e-5);
        // Higher temperature flattens.
        let flat = softmax_with_temperature(&[2.0, 0.0], 2, 4.0);
        assert!(flat[0] < p[0]);
        // Temperature floors at 1e-3, never divides by 0.
        let sharp = softmax_with_temperature(&[2.0, 0.0], 2, 0.0);
        assert!(sharp[0] > 0.999);
        assert_eq!(entropy_confidence(&[1.0], 1), 1.0);
        assert!((entropy_confidence(&[0.5, 0.5], 2)).abs() < 1e-6);
        assert!((entropy_confidence(&[1.0, 0.0], 2) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn real_tokenizer_when_present() {
        let Some(tok) = real() else {
            eprintln!("skipped: no laya tokenizer.json around");
            return;
        };
        let cfg = LayaConfig::default();
        let q = Question::Choice {
            instructions: "which cached brief is the subject being discussed".into(),
            candidates: vec![
                Candidate {
                    id: "SRE-2386".into(),
                    text: "image tag pipeline outage".into(),
                },
                Candidate {
                    id: "none".into(),
                    text: "none of these".into(),
                },
            ],
            state: "The deploy is blocked on the image tags. [MASK]".into(),
        };
        let s = build_sequence(&tok, &q, &cfg).unwrap();
        assert_eq!(s.input_ids[0], CLS);
        assert_eq!(*s.input_ids.last().unwrap(), SEP);
        assert_eq!(s.input_ids.iter().filter(|&&i| i == MASK).count(), 2);
        assert_eq!(s.marker_pos.len(), 2);
        assert!(s.input_ids.iter().all(|&i| i != PAD));
        assert!(!s.truncated);
        // The tokenizer's own special ids are the constants we hardcode.
        assert_eq!(tok.token_to_id("[CLS]"), Some(CLS as u32));
        assert_eq!(tok.token_to_id("[SEP]"), Some(SEP as u32));
        assert_eq!(tok.token_to_id("[PAD]"), Some(PAD as u32));
        assert_eq!(tok.token_to_id("[MASK]"), Some(MASK as u32));
    }

    fn seq(ids: Vec<i64>, markers: Vec<i64>, qtype: i64) -> Sequence {
        Sequence {
            attention_mask: vec![1; ids.len()],
            marker_mask: vec![true; markers.len()],
            input_ids: ids,
            marker_pos: markers,
            qtype,
            truncated: false,
        }
    }

    #[test]
    fn collate_pads_mixed_s_and_k() {
        let rows = [
            seq(
                vec![CLS, 1, SEP, MASK, 2, MASK, 3, SEP, 4, SEP],
                vec![3, 5],
                2,
            ),
            seq(vec![CLS, 1, SEP, MASK, 2, SEP, 9, SEP], vec![3], 0),
            seq(
                vec![CLS, SEP, MASK, MASK, MASK, SEP, 5, 6, 7, 8, 9, SEP],
                vec![2, 3, 4],
                1,
            ),
        ];
        let b = collate(&rows);
        assert_eq!((b.b, b.s, b.k), (3, 12, 3));
        assert_eq!(b.input_ids.len(), 36);
        assert_eq!(b.attention_mask.len(), 36);
        assert_eq!(b.marker_pos.len(), 9);
        assert_eq!(b.marker_mask.len(), 9);
        // Row 1: 8 real tokens, then [PAD]s with attention 0.
        assert_eq!(&b.input_ids[12 + 8..24], &[PAD; 4]);
        assert_eq!(
            &b.attention_mask[12..24],
            &[1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0]
        );
        assert_eq!(&b.marker_pos[3..6], &[3, 0, 0]);
        assert_eq!(&b.marker_mask[3..6], &[true, false, false]);
        // Row 0 keeps its two markers; row 2 fills K.
        assert_eq!(&b.marker_pos[0..3], &[3, 5, 0]);
        assert_eq!(&b.marker_mask[6..9], &[true, true, true]);
        assert_eq!(b.qtype, vec![2, 0, 1]);
        assert_eq!(collate(&[]).b, 0);
    }

    #[test]
    fn f16_round_trip() {
        let v = [
            f16::from_f32(1.5),
            f16::from_f32(-2.25),
            f16::from_f32(-1e4),
        ];
        assert_eq!(f16_to_f32(&v), vec![1.5, -2.25, -1e4]);
        assert_eq!(f16_to_f32(&[]), Vec::<f32>::new());
    }

    #[test]
    fn postprocess_uses_row_k_and_bucket_temperature() {
        let cfg = LayaConfig {
            temperature: [1.0, 1.0, 2.0],
            temperature_by_options: [("choice:2".to_owned(), 0.5)].into(),
            ..LayaConfig::default()
        };
        // Padded K = 3, the row has 2 markers; the third logit is the model's −1e4 mask fill.
        let row = seq(vec![CLS, SEP, MASK, MASK, SEP], vec![2, 3], 0);
        let out = postprocess(&row, &[1.0, 0.0, -1e4], &[0.0, 0.0], &cfg);
        assert_eq!(out.temperature, 0.5);
        assert_eq!(out.logits, vec![1.0, 0.0]);
        assert_eq!(out.probs.len(), 2);
        assert!((out.probs[0] - 0.880_797).abs() < 1e-5); // softmax([2, 0])
        assert!((out.act_p - 0.5).abs() < 1e-6);
        assert!(out.confidence > 0.0 && out.confidence < 1.0);
        // Noul: per-qtype temperature, confidence = max(p, 1 − p).
        let row = seq(vec![CLS, SEP, MASK, MASK, SEP], vec![2, 3], 2);
        let out = postprocess(&row, &[0.0, 2.0], &[3.0, 0.0], &cfg);
        assert_eq!(out.temperature, 2.0);
        assert!((out.probs[1] - 0.731_059).abs() < 1e-5); // softmax([0, 1])
        assert!((out.confidence - out.probs[1]).abs() < 1e-6);
        assert!(out.act_p > 0.95);
    }

    #[test]
    fn graph_verification() {
        use ort::value::{Shape, SymbolicDimensions};
        let t = |ty, rank: usize| ValueType::Tensor {
            ty,
            shape: Shape::new(vec![-1; rank]),
            dimension_symbols: SymbolicDimensions::empty(rank),
        };
        let good_in = || {
            vec![
                Outlet::new("input_ids", t(TensorElementType::Int64, 2)),
                Outlet::new("attention_mask", t(TensorElementType::Int64, 2)),
                Outlet::new("marker_pos", t(TensorElementType::Int64, 2)),
                Outlet::new("marker_mask", t(TensorElementType::Bool, 2)),
                Outlet::new("qtype", t(TensorElementType::Int64, 1)),
            ]
        };
        let good_out = || {
            vec![
                Outlet::new("logits", t(TensorElementType::Float16, 2)),
                Outlet::new("act_logits", t(TensorElementType::Float16, 2)),
            ]
        };
        assert_eq!(verify_graph(&good_in(), &good_out()), Ok(()));
        // fp32 export: refused, names the output.
        let mut out = good_out();
        out[0] = Outlet::new("logits", t(TensorElementType::Float32, 2));
        let e = verify_graph(&good_in(), &out).unwrap_err();
        assert!(e.contains("`logits`") && e.contains("f16"), "{e}");
        // Missing / renamed input.
        let mut inp = good_in();
        inp[4] = Outlet::new("question_type", t(TensorElementType::Int64, 1));
        assert!(
            verify_graph(&inp, &good_out())
                .unwrap_err()
                .contains("`qtype`")
        );
        // Wrong rank.
        let mut inp = good_in();
        inp[4] = Outlet::new("qtype", t(TensorElementType::Int64, 2));
        assert!(
            verify_graph(&inp, &good_out())
                .unwrap_err()
                .contains("rank 1")
        );
        // Extra input.
        let mut inp = good_in();
        inp.push(Outlet::new("past", t(TensorElementType::Int64, 2)));
        assert!(
            verify_graph(&inp, &good_out())
                .unwrap_err()
                .contains("6 inputs")
        );
    }
}
