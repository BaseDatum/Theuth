//! GLiNER2.5-Decide backend (`Kind::Gliner`): DeBERTa-v3-large + a per-label head that scores
//! one logit per `[L]` label marker — its own tokenizer and prompt layout, nothing of laya's
//! `[MASK]` rendering. [`prompt`] renders a [`Question`] exactly as the Python `gliner2`
//! package feeds the model (GLINER-2); [`candle`] runs the checkpoint natively (the production
//! path, `Loaded`); [`onnx`] is the GLINER-3 bench/parity control through ONNX Runtime, built
//! only with the `gliner-onnx` feature. Both backends share [`collate`] (right-padded rows, label
//! positions padded to the widest row) and [`scores`] (softmax over each row's own labels).
//!
//! Checkpoint: Hugging Face `fastino/GLiNER2.5-Decide` @ `5a7adf72` (57 §"GLiNER2.5-Decide
//! checkpoint"); the files `open` needs are [`crate::engine::model_files`]`(Kind::Gliner)`.

pub mod candle;
#[cfg(feature = "gliner-onnx")]
pub mod onnx;
pub mod prompt;

use std::any::Any;

pub use candle::Loaded;

use crate::engine::{Row, Scores};
use prompt::GlinerRow;

impl Row for GlinerRow {
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
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Rows padded into one batch, row-major: `input_ids`/`attention_mask` `[b, s]` (`[PAD]` = 0,
/// attention 0 — gliner2's `_pad_batch`), `label_pos` `[b, n]` padded with position 0 (a valid
/// gather index whose logit [`scores`] discards).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Batch {
    /// Rows.
    pub b: usize,
    /// Padded sequence length.
    pub s: usize,
    /// Padded label count.
    pub n: usize,
    /// `[b, s]`.
    pub input_ids: Vec<i64>,
    /// `[b, s]`.
    pub attention_mask: Vec<i64>,
    /// `[b, n]`.
    pub label_pos: Vec<i64>,
}

/// Pad `rows` to the longest row and the widest label list.
pub fn collate(rows: &[&GlinerRow]) -> Batch {
    let b = rows.len();
    let s = rows.iter().map(|r| r.input_ids.len()).max().unwrap_or(0);
    let n = rows.iter().map(|r| r.label_count).max().unwrap_or(0);
    let mut input_ids = vec![prompt::PAD; b * s];
    let mut attention_mask = vec![0; b * s];
    let mut label_pos = vec![0; b * n];
    for (i, r) in rows.iter().enumerate() {
        input_ids[i * s..i * s + r.input_ids.len()].copy_from_slice(&r.input_ids);
        attention_mask[i * s..i * s + r.input_ids.len()].fill(1);
        label_pos[i * n..i * n + r.label_count].copy_from_slice(&r.label_pos);
    }
    Batch {
        b,
        s,
        n,
        input_ids,
        attention_mask,
        label_pos,
    }
}

/// `logits` `[b, n]` row-major → one [`Scores`] per row: its first `label_count` logits,
/// softmax (`_extract_classification_result`, single-label, temperature 1). A `Bool` row's
/// `[yes, no]` pair comes back as `[false, true]`, the order [`crate::laya::labels`] gives
/// the engine.
pub fn scores(rows: &[&GlinerRow], logits: &[f32], n: usize) -> anyhow::Result<Vec<Scores>> {
    if logits.iter().any(|v| !v.is_finite()) {
        anyhow::bail!("non-finite logits from the gliner head");
    }
    Ok(rows
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let mut logits = logits[i * n..i * n + r.label_count].to_vec();
            if r.is_bool {
                logits.reverse();
            }
            let max = logits.iter().copied().fold(f32::NEG_INFINITY, f32::max);
            let exp: Vec<f32> = logits.iter().map(|l| (l - max).exp()).collect();
            let sum: f32 = exp.iter().sum();
            Scores {
                probs: exp.iter().map(|e| e / sum).collect(),
                logits,
                temperature: 1.0,
                act_p: 0.0,
            }
        })
        .collect())
}

/// The rows back as [`GlinerRow`]s; a foreign row is an error, not a panic.
pub fn downcast(rows: &[Box<dyn Row>]) -> anyhow::Result<Vec<&GlinerRow>> {
    rows.iter()
        .map(|r| {
            r.as_any()
                .downcast_ref::<GlinerRow>()
                .ok_or_else(|| anyhow::anyhow!("gliner oracle given a row it did not build"))
        })
        .collect()
}

/// `scripts/tests/gliner-reference.py`'s output: the Python model's logits and probabilities
/// for every fixture question, what both backends' parity tests compare against.
#[cfg(test)]
pub(crate) mod reference {
    use std::path::PathBuf;

    use serde::Deserialize;

    use crate::question::Question;

    #[derive(Deserialize)]
    pub struct Reference {
        pub cases: Vec<Case>,
    }

    #[derive(Deserialize)]
    pub struct Case {
        pub name: String,
        pub question: Question,
        pub input_ids: Vec<i64>,
        pub label_pos: Vec<i64>,
        pub state_cut: bool,
        pub probs: Vec<f32>,
    }

    pub fn load() -> Reference {
        serde_json::from_str(include_str!("fixtures/reference.json")).unwrap()
    }

    /// `$THEUTH_MODELS_DIR/decision-gliner` (default `~/.theuth/models/decision-gliner`) when
    /// every file `open` needs is there; `None` prints why.
    pub fn checkpoint() -> Option<PathBuf> {
        let dir = std::env::var_os("THEUTH_MODELS_DIR")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".theuth/models")))?
            .join("decision-gliner");
        let missing: Vec<_> = crate::engine::model_files(crate::Kind::Gliner)
            .iter()
            .filter(|f| !dir.join(f).is_file())
            .collect();
        if missing.is_empty() {
            Some(dir)
        } else {
            eprintln!(
                "SKIP: {} lacks {missing:?} — `scripts/fetch-models.sh` downloads them",
                dir.display()
            );
            None
        }
    }

    pub fn argmax(p: &[f32]) -> usize {
        p.iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map_or(0, |(i, _)| i)
    }

    /// Worst |Δp| and the number of argmax disagreements over `pairs` of (reference, got); the
    /// reference holds a Bool as `[yes, no]`, `got` is the backend's `[false, true]`.
    pub fn compare<'a>(
        pairs: impl Iterator<Item = (&'a Case, &'a [f32])>,
    ) -> (f32, usize, Vec<String>) {
        let mut worst = 0.0f32;
        let mut flips = 0;
        let mut lines = Vec::new();
        for (c, got) in pairs {
            assert_eq!(got.len(), c.probs.len(), "{}", c.name);
            let mut want = c.probs.clone();
            if matches!(c.question, Question::Bool { .. }) {
                want.reverse();
            }
            let d = want
                .iter()
                .zip(got)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0f32, f32::max);
            let same = argmax(&want) == argmax(got);
            flips += usize::from(!same);
            worst = worst.max(d);
            lines.push(format!(
                "{:>22} labels={:3} max|Δp|={d:.2e} argmax {}",
                c.name,
                c.probs.len(),
                if same { "same" } else { "DIFFERS" }
            ));
        }
        (worst, flips, lines)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(ids: &[i64], pos: &[i64]) -> GlinerRow {
        GlinerRow {
            input_ids: ids.to_vec(),
            attention_mask: vec![1; ids.len()],
            label_pos: pos.to_vec(),
            label_count: pos.len(),
            truncated: false,
            state_cut: false,
            markers_lost: false,
            is_bool: false,
        }
    }

    #[test]
    fn collate_pads_ids_mask_and_positions_and_scores_softmax_per_row() {
        let a = row(&[5, 128007, 7, 128007, 8], &[1, 3]);
        let b = row(&[5, 128007, 9], &[1]);
        let batch = collate(&[&a, &b]);
        assert_eq!((batch.b, batch.s, batch.n), (2, 5, 2));
        assert_eq!(
            batch.input_ids,
            [5, 128007, 7, 128007, 8, 5, 128007, 9, 0, 0]
        );
        assert_eq!(batch.attention_mask, [1, 1, 1, 1, 1, 1, 1, 1, 0, 0]);
        assert_eq!(batch.label_pos, [1, 3, 1, 0]);
        // Row b's padded slot logit (99.0) is ignored; row a is a proper softmax.
        let out = scores(&[&a, &b], &[0.0, 1.0, 2.0, 99.0], 2).unwrap();
        assert_eq!(out[1].probs, [1.0]);
        assert_eq!(out[1].logits, [2.0]);
        let e = 1.0f32.exp();
        assert!((out[0].probs[1] - e / (1.0 + e)).abs() < 1e-6);
        assert!((out[0].probs.iter().sum::<f32>() - 1.0).abs() < 1e-6);
        assert!(scores(&[&b], &[f32::NAN], 1).is_err());
        // A Bool row: `[yes, no]` logits → `[false, true]` scores.
        let yn = GlinerRow {
            is_bool: true,
            ..row(&[5, 128007, 7, 128007, 8], &[1, 3])
        };
        let out = scores(&[&yn], &[2.0, 0.0], 2).unwrap();
        assert_eq!(out[0].logits, [0.0, 2.0]);
        assert!(out[0].probs[1] > 0.88);
    }
}
