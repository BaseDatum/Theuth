//! laya's input rendering and post-processing (57 §1), ported from `rl_common.py`
//! (`render_options`, `build_sequence`, `collate_items`, `temp_bucket`,
//! `confidence_from_probs`) and `agent.py` (`system_one`). Everything here is a pure function
//! over a `tokenizers::Tokenizer`; the forward pass is [`crate::candle::Loaded`].
//!
//! Sequence: `[CLS] <qtype> question: <instructions> [SEP] [MASK] opt0 [MASK] opt1 … [SEP] <state> [SEP]`.
//! Each option is capped at [`OPTION_CAP`] tokens and shrunk evenly when the head budget
//! (`head_max_len`) runs out; the instructions get what the options leave (≥ 8 tokens); the
//! state gets the rest of `max_len` and is right-truncated.

use std::borrow::Borrow;
use std::collections::BTreeMap;

use serde::Deserialize;
use tokenizers::Tokenizer;

use crate::engine::{Row, Scores};
use crate::question::Question;

/// `[CLS]` in ModernBERT's tokenizer (the root laya checkpoint); [`Special::MODERNBERT`].
pub const CLS: i64 = 50281;
/// `[SEP]`.
pub const SEP: i64 = 50282;
/// `[PAD]`.
pub const PAD: i64 = 50283;
/// `[MASK]`: one per option; the head scores its hidden state.
pub const MASK: i64 = 50284;
/// Tokens per option before the head budget shrinks them.
pub const OPTION_CAP: usize = 48;

/// The special tokens laya's `predict` reads off the checkpoint's tokenizer
/// (`tok.cls_token_id` …): ModernBERT's `[CLS] [SEP] [PAD] [MASK]` for the root checkpoint,
/// Gemma's `<bos> <eos> <pad> <mask>` for the mmBERT-base fine-tunes (cklxx/laya-browser v17s,
/// whose `tokenizer_config.json` maps cls→`<bos>`, sep→`<eos>`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Special {
    /// Sequence start.
    pub cls: i64,
    /// Segment separator.
    pub sep: i64,
    /// Batch padding.
    pub pad: i64,
    /// Option marker.
    pub mask: i64,
    /// The mask token's text, replaced by a space wherever user text contains it.
    pub mask_text: &'static str,
}

impl Special {
    /// ModernBERT-large's ids (the root `decision/` checkpoint).
    pub const MODERNBERT: Special = Special {
        cls: CLS,
        sep: SEP,
        pad: PAD,
        mask: MASK,
        mask_text: "[MASK]",
    };

    /// Read the ids off `tok`: the BERT names, else the Gemma names; neither → error naming both.
    pub fn from_tokenizer(tok: &Tokenizer) -> Result<Special, String> {
        for (cls, sep, pad, mask) in [
            ("[CLS]", "[SEP]", "[PAD]", "[MASK]"),
            ("<bos>", "<eos>", "<pad>", "<mask>"),
        ] {
            let id = |t: &str| tok.token_to_id(t).map(i64::from);
            if let (Some(c), Some(s), Some(p), Some(m)) = (id(cls), id(sep), id(pad), id(mask)) {
                return Ok(Special {
                    cls: c,
                    sep: s,
                    pad: p,
                    mask: m,
                    mask_text: mask,
                });
            }
        }
        Err("tokenizer has neither [CLS]/[SEP]/[PAD]/[MASK] nor <bos>/<eos>/<pad>/<mask>".into())
    }
}

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

/// The fields of `rl_agent_config.json` inference needs, plus the tokenizer's special ids.
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct LayaConfig {
    /// Sequence budget.
    pub max_len: usize,
    /// Budget for instructions + options.
    pub head_max_len: usize,
    /// The head budget a fine-tune was trained with (cklxx/laya-browser: 768); wins over
    /// `head_max_len` when present, as their systemone server does (57 §12.4).
    pub head_max_len_train: Option<usize>,
    /// Per-qtype temperature, index = [`QType`].
    pub temperature: [f32; 3],
    /// Per-cardinality temperature, keyed like `choice:3-5`; wins over `temperature`.
    pub temperature_by_options: BTreeMap<String, f32>,
    /// Not in the json: [`Loaded::open`](crate::candle::Loaded::open) sets it from the tokenizer.
    #[serde(skip)]
    pub special: Special,
}

impl Default for LayaConfig {
    fn default() -> Self {
        LayaConfig {
            max_len: 512,
            head_max_len: 192,
            head_max_len_train: None,
            temperature: [1.0; 3],
            temperature_by_options: BTreeMap::new(),
            special: Special::MODERNBERT,
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

    /// The head budget in force: `head_max_len_train` when the checkpoint records one.
    pub fn head_budget(&self) -> usize {
        self.head_max_len_train.unwrap_or(self.head_max_len)
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
    /// The state was cut, or an option's marker fell past `max_len` (57 §0.5 hard failure) —
    /// `state_cut || markers_lost`.
    pub truncated: bool,
    /// The state was right-truncated (laya's own behaviour; a hard error only for families
    /// without `cut_ok`).
    pub state_cut: bool,
    /// An option's `[MASK]` fell past `max_len`: that option cannot be scored. Always hard.
    pub markers_lost: bool,
}

impl Row for Sequence {
    fn truncated(&self) -> bool {
        self.truncated
    }
    fn state_cut(&self) -> bool {
        self.state_cut
    }
    fn markers_lost(&self) -> bool {
        self.markers_lost
    }
    fn ids(&self) -> &[i64] {
        &self.input_ids
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
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

fn encode(tok: &Tokenizer, text: &str, mask_text: &str) -> Result<Vec<i64>, String> {
    let enc = tok
        .encode(text.replace(mask_text, " "), false)
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
    let sp = &cfg.special;
    let mut head = encode(
        tok,
        &format!("{} question: {instructions}", qtype.name()),
        sp.mask_text,
    )?;
    let mut opts: Vec<Vec<i64>> = Vec::new();
    for o in render_options(q) {
        let mut ids = encode(tok, &format!(" {o}"), sp.mask_text)?;
        ids.truncate(OPTION_CAP);
        ids.insert(0, sp.mask);
        opts.push(ids);
    }
    let used = |opts: &[Vec<i64>]| opts.iter().map(Vec::len).sum::<usize>();
    let head_max = cfg.head_budget();
    let mut budget = head_max as isize - used(&opts) as isize;
    if budget < 16 {
        // Too many / too long options: shrink every option evenly.
        let per = ((head_max.saturating_sub(16)) / opts.len().max(1)).max(4);
        for o in &mut opts {
            o.truncate(per);
        }
        budget = head_max as isize - used(&opts) as isize;
    }
    head.truncate(budget.max(8) as usize);
    let mut ids = Vec::with_capacity(cfg.max_len);
    ids.push(sp.cls);
    ids.extend(head);
    ids.push(sp.sep);
    let mut markers = Vec::with_capacity(opts.len());
    for o in &opts {
        markers.push(ids.len() as i64);
        ids.extend(o);
    }
    ids.push(sp.sep);
    let room = cfg.max_len.saturating_sub(ids.len() + 1);
    let mut st = encode(tok, state, sp.mask_text)?;
    let state_cut = st.len() > room;
    st.truncate(room);
    ids.extend(st);
    ids.push(sp.sep);
    ids.truncate(cfg.max_len);
    let k = markers.len();
    markers.retain(|&m| (m as usize) < cfg.max_len);
    let markers_lost = markers.len() < k;
    Ok(Sequence {
        attention_mask: vec![1; ids.len()],
        marker_mask: vec![true; markers.len()],
        input_ids: ids,
        marker_pos: markers,
        qtype: qtype as i64,
        truncated: state_cut || markers_lost,
        state_cut,
        markers_lost,
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
    /// Raw marker logits, option order, K of them.
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

impl From<RowOut> for Scores {
    /// `confidence` is dropped: the engine derives it from `probs` ([`Scores::confidence`]).
    fn from(r: RowOut) -> Scores {
        Scores {
            probs: r.probs,
            logits: r.logits,
            temperature: r.temperature,
            act_p: r.act_p,
        }
    }
}

/// Rows padded to a common `[B, S]` / `[B, K]` (`collate_items`): `pad` with attention 0,
/// marker position 0 with mask false.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Batch {
    /// Rows.
    pub b: usize,
    /// Longest row, or the bucket's S.
    pub s: usize,
    /// Most markers, or the bucket's K.
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

/// Pad `rows` into one batch; `floor = Some((S, K))` pads at least that far (the model wants
/// K ≥ 2 marker slots for its top-2 act features). Rows never shrink to the floor. `rows` may be
/// `&[Sequence]` or `&[&Sequence]`.
pub fn collate<R: Borrow<Sequence>>(rows: &[R], floor: Option<(usize, usize)>, pad: i64) -> Batch {
    let b = rows.len();
    let (bs, bk) = floor.unwrap_or((0, 0));
    let s = rows
        .iter()
        .map(|r| r.borrow().input_ids.len())
        .max()
        .unwrap_or(0)
        .max(bs);
    let k = rows
        .iter()
        .map(|r| r.borrow().marker_pos.len())
        .max()
        .unwrap_or(0)
        .max(bk);
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
        let r = r.borrow();
        let n = r.input_ids.len();
        batch.input_ids.extend_from_slice(&r.input_ids);
        batch.input_ids.resize(batch.input_ids.len() + s - n, pad);
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
            std::path::PathBuf::from(&home)
                .join(".theuth/models/decision/tokenizer/tokenizer.json"),
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
        // The tokenizer's own special ids are the ModernBERT constants.
        assert_eq!(Special::from_tokenizer(&tok).unwrap(), Special::MODERNBERT);
    }

    /// A Gemma-style tokenizer (mmBERT-base, cklxx/laya-browser v17s): `<pad>=0 <eos>=1 <bos>=2
    /// <unk>=3 <mask>=4`, then words.
    fn gemma() -> Tokenizer {
        let specials = ["<pad>", "<eos>", "<bos>", "<unk>", "<mask>"];
        let mut vocab = serde_json::Map::new();
        for (i, t) in specials.iter().enumerate() {
            vocab.insert((*t).into(), i.into());
        }
        for w in "choice question : deploy blocked e0 e1 button".split_whitespace() {
            let n = vocab.len();
            vocab.entry(w).or_insert(n.into());
        }
        let added: Vec<_> = specials
            .iter()
            .enumerate()
            .map(|(i, t)| serde_json::json!({"id": i, "content": t, "special": true, "single_word": false, "lstrip": false, "rstrip": false, "normalized": false}))
            .collect();
        let spec = serde_json::json!({
            "version": "1.0",
            "added_tokens": added,
            "pre_tokenizer": { "type": "Whitespace" },
            "model": { "type": "WordLevel", "vocab": vocab, "unk_token": "<unk>" },
        });
        Tokenizer::from_bytes(serde_json::to_vec(&spec).unwrap()).unwrap()
    }

    #[test]
    fn special_tokens_follow_the_tokenizer() {
        let sp = Special::from_tokenizer(&gemma()).unwrap();
        assert_eq!(
            sp,
            Special {
                cls: 2,
                sep: 1,
                pad: 0,
                mask: 4,
                mask_text: "<mask>"
            }
        );
        // The word-level test tokenizer has no special tokens at all.
        assert!(
            Special::from_tokenizer(&tiny())
                .unwrap_err()
                .contains("<bos>")
        );

        let cfg = LayaConfig {
            special: sp,
            ..LayaConfig::default()
        };
        let mut q = choice(2, "deploy blocked <mask> [MASK]");
        if let Question::Choice { instructions, .. } = &mut q {
            *instructions = "deploy <mask>".into();
        }
        let s = build_sequence(&gemma(), &q, &cfg).unwrap();
        assert_eq!(s.input_ids[0], 2);
        assert_eq!(*s.input_ids.last().unwrap(), 1);
        // Exactly the two option markers: `<mask>` in user text was blanked, `[MASK]` is plain
        // text for this tokenizer (unknown → 3).
        assert_eq!(s.input_ids.iter().filter(|&&i| i == 4).count(), 2);
        assert_eq!(s.marker_pos.len(), 2);
        assert!(s.input_ids.contains(&3));
        let b = collate(
            &[s.clone(), seq(vec![2, 1], vec![], 0)],
            None,
            cfg.special.pad,
        );
        assert_eq!(
            b.input_ids[s.input_ids.len()..s.input_ids.len() * 2][2..],
            vec![0; s.input_ids.len() - 2]
        );
    }

    fn seq(ids: Vec<i64>, markers: Vec<i64>, qtype: i64) -> Sequence {
        Sequence {
            attention_mask: vec![1; ids.len()],
            marker_mask: vec![true; markers.len()],
            input_ids: ids,
            marker_pos: markers,
            qtype,
            truncated: false,
            state_cut: false,
            markers_lost: false,
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
        let b = collate(&rows, None, PAD);
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
        assert_eq!(collate::<Sequence>(&[], None, PAD).b, 0);
    }

    #[test]
    fn collate_pads_to_floor() {
        let rows = [
            seq(
                vec![CLS, 1, SEP, MASK, 2, MASK, 3, SEP, 4, SEP],
                vec![3, 5],
                0,
            ),
            seq(vec![CLS, SEP, MASK, SEP], vec![2], 2),
        ];
        let b = collate(&rows, Some((16, 4)), PAD);
        assert_eq!((b.b, b.s, b.k), (2, 16, 4));
        assert_eq!(b.input_ids.len(), 2 * 16);
        assert_eq!(b.marker_mask.len(), 2 * 4);
        // Real tokens then [PAD]/attention 0 to S; real markers then pos 0/mask false to K.
        assert_eq!(&b.input_ids[..10], &rows[0].input_ids[..]);
        assert!(b.input_ids[10..16].iter().all(|&t| t == PAD));
        assert_eq!(b.attention_mask[..16].iter().sum::<i64>(), 10);
        assert_eq!(b.attention_mask[16..].iter().sum::<i64>(), 4);
        assert_eq!(&b.marker_pos[..3], &[3, 5, 0]);
        assert_eq!(b.marker_mask[..4].iter().filter(|&&m| m).count(), 2);
        assert_eq!(b.marker_mask[4..].iter().filter(|&&m| m).count(), 1);
        // A row longer than the floor is never cut.
        let long = seq(vec![7; 600], vec![1], 0);
        assert_eq!(collate(&[long], Some((16, 4)), PAD).s, 600);
        // The floor is a floor, not a fixed shape: the engine's `(0, 2)` leaves S alone and
        // only lifts a one-marker batch to two slots.
        assert_eq!(collate(&rows, Some((4, 1)), PAD).s, 10);
        let one = collate(&rows[1..], Some((0, 2)), PAD);
        assert_eq!((one.s, one.k), (4, 2));
        assert_eq!(one.marker_mask, [true, false]);
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
}
