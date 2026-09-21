//! The candle engine against the real laya checkpoint in `~/.theuth/models/decision-candle/`
//! (or `$DECISION_CANDLE_DIR`): the README's four questions must match the ORT fp32 CPU
//! probabilities (`tests/fixtures/probs-cpu-fp32.json`, step 2d) — argmax and |Δp| ≤ 0.01
//! (57 §12.8). Skips with a printed reason when the checkpoint is not there.

#![cfg(feature = "candle")]

use std::path::PathBuf;

use candle_core::DType;
use decision::candle::CandleLoaded;
use decision::engine::CANDLE_FILES;
use decision::{Candidate, Question};

fn model_dir() -> Option<PathBuf> {
    let dir = std::env::var_os("DECISION_CANDLE_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".theuth/models/decision-candle"))
        })?;
    if CANDLE_FILES.iter().all(|f| dir.join(f).is_file()) {
        Some(dir)
    } else {
        eprintln!("skipped: no laya safetensors checkpoint in {}", dir.display());
        None
    }
}

/// The README example (bench (c)): one state, four questions, in the reference's order.
fn readme() -> Vec<Question> {
    let state = r#"{"from": "user@acme.com", "subject": "Duplicate charge on invoice #4411", "body": "Hi, we were billed twice for March. Please refund the duplicate today or we will cancel our plan."}"#.to_owned();
    vec![
        Question::Choice {
            instructions: "Which department should handle this request?".into(),
            candidates: [
                ("billing", "invoices, payments, refunds"),
                ("technical", "bugs, outages, system errors"),
                ("sales", "pricing, new contracts"),
                ("other", "everything else"),
            ]
            .into_iter()
            .map(|(id, text)| Candidate {
                id: id.into(),
                text: text.into(),
            })
            .collect(),
            state: state.clone(),
        },
        Question::Score {
            instructions: "How urgent is this request?".into(),
            levels: vec![
                "not urgent".into(),
                "soon".into(),
                "critical deadline or blocking issue".into(),
            ],
            state: state.clone(),
        },
        Question::Bool {
            statement: "Does the user threaten to cancel or leave?".into(),
            state: state.clone(),
        },
        Question::Bool {
            statement: "Does the user explicitly request a refund?".into(),
            state,
        },
    ]
}

fn argmax(p: &[f32]) -> usize {
    p.iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map_or(0, |(i, _)| i)
}

fn parity(metal: bool, dtype: DType) {
    let Some(dir) = model_dir() else {
        return;
    };
    let reference: Vec<Vec<f32>> =
        serde_json::from_str(include_str!("fixtures/probs-cpu-fp32.json")).unwrap();
    let m = CandleLoaded::open(&dir, metal, dtype).expect("open");
    eprintln!(
        "{:?} {:?}: load {:?} (hash {:?})",
        m.device.location(),
        m.dtype,
        m.load_time,
        m.hash_time
    );
    assert_eq!(m.model_sha.len(), 64);
    assert_eq!((m.laya.max_len, m.laya.head_max_len), (512, 192));
    let rows: Vec<_> = readme().iter().map(|q| m.build(q).unwrap()).collect();
    assert!(rows.iter().all(|r| !r.truncated));
    let outs = m.run(&rows).unwrap();
    assert_eq!(outs.len(), 4);
    let mut worst = 0.0f32;
    for (out, want) in outs.iter().zip(&reference) {
        assert_eq!(out.probs.len(), want.len());
        assert!((out.probs.iter().sum::<f32>() - 1.0).abs() < 1e-4);
        assert_eq!(argmax(&out.probs), argmax(want), "{:?} vs {want:?}", out.probs);
        for (p, q) in out.probs.iter().zip(want) {
            worst = worst.max((p - q).abs());
        }
        assert!((0.0..=1.0).contains(&out.act_p));
    }
    eprintln!("max |Δp| vs ORT fp32: {worst:.4}");
    assert!(worst <= 0.01, "max |Δp| {worst} > 0.01");
    // One row alone equals its batched answer (padding to the batch's S/K does not leak).
    let alone = m.run(&rows[2..3]).unwrap().remove(0);
    assert!((alone.probs[1] - outs[2].probs[1]).abs() < 0.005);
}

#[test]
fn cpu_f32_matches_the_ort_reference() {
    parity(false, DType::F32);
}

/// Metal f16 is the shipped macOS configuration; off macOS this is the CPU test again.
#[test]
fn metal_f16_matches_the_ort_reference() {
    parity(true, DType::F16);
}
