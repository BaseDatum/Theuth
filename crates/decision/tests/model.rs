//! End-to-end against the real checkpoint in `~/.theuth/models/decision/` (or
//! `$DECISION_MODEL_DIR`). Skips with a printed reason when it is not there.

use std::path::PathBuf;
use std::time::Duration;

use decision::engine::MODEL_FILES;
use decision::laya::{BUCKET, Loaded, Provider, Units};
use decision::log::Log;
use decision::{
    Ask, Candidate, DecisionConfig, Disposition, Engine, FamilyGate, Mode, Outcome, Question,
    Reason, Report, Stats,
};
use kameo::actor::Spawn;

/// `$THEUTH_DATA_DIR` or `~/.theuth`: the runtime lives under it (`models/onnxruntime/`).
fn data_dir() -> Option<PathBuf> {
    std::env::var_os("THEUTH_DATA_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".theuth")))
}

/// `(data_dir, model_dir)`, or `None` with a printed reason.
fn model_dir() -> Option<(PathBuf, PathBuf)> {
    let data = data_dir()?;
    let dir = std::env::var_os("DECISION_MODEL_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| data.join("models/decision"));
    if MODEL_FILES.iter().all(|f| dir.join(f).is_file()) {
        Some((data, dir))
    } else {
        eprintln!("skipped: no laya checkpoint in {}", dir.display());
        None
    }
}

fn billing_state() -> String {
    r#"{"from": "user@acme.com", "subject": "Duplicate charge on invoice #4411", "body": "Hi, we were billed twice for March. Please refund the duplicate today or we will cancel our plan."}"#.to_owned()
}

#[test]
fn open_verifies_graph_and_answers_on_cpu() {
    answers(Provider::Cpu);
}

/// Asking for the CoreML EP must always yield a working session: on macOS it either
/// registers (bucketed, partition report present) or falls back to CPU with a warning (57
/// §12.6 — both root ONNX exports fall back today); elsewhere it is the CPU test twice.
#[test]
fn open_verifies_graph_and_answers_on_coreml() {
    answers(Provider::CoreMl(Units::All));
}

fn answers(provider: Provider) {
    let Some((data, dir)) = model_dir() else {
        return;
    };
    let mut m = Loaded::open(&dir, &data, 4, provider).expect("open");
    eprintln!(
        "asked {provider:?} → {:?}, partitions {:?}",
        m.provider, m.partitions
    );
    assert_eq!(m.model_sha.len(), 64);
    assert!(m.load_time > Duration::ZERO);
    assert_eq!((m.laya.max_len, m.laya.head_max_len), (512, 192));
    let coreml = matches!(m.provider, Provider::CoreMl(_));
    assert!(!coreml || cfg!(target_os = "macos"));
    assert!(!coreml || provider == m.provider);
    assert_eq!(m.bucket, if coreml { BUCKET } else { (512, 0) });
    assert_eq!(m.partitions.is_some(), coreml);
    let q = Question::Bool {
        statement: "Does the user threaten to cancel or leave?".into(),
        state: billing_state(),
    };
    let seq = m.build(&q).unwrap();
    assert!(!seq.truncated);
    let out = m.run(&[seq]).unwrap().remove(0);
    assert_eq!(out.probs.len(), 2);
    assert!((out.probs.iter().sum::<f32>() - 1.0).abs() < 1e-4);
    assert!(out.probs[1] > 0.5, "churn risk should read true: {out:?}");
    // The act head's logits are in the thousands; its softmax saturates to exactly 0/1 (the
    // ONNX README's "max action-prob diff 0.0" says the same).
    assert!((0.0..=1.0).contains(&out.act_p));
    // A batch of mixed K and qtype comes back row-aligned.
    let choice = Question::Choice {
        instructions: "Which department should handle this request?".into(),
        candidates: vec![
            Candidate {
                id: "billing".into(),
                text: "invoices, payments, refunds".into(),
            },
            Candidate {
                id: "technical".into(),
                text: "bugs, outages, system errors".into(),
            },
            Candidate {
                id: "sales".into(),
                text: "pricing, new contracts".into(),
            },
            Candidate {
                id: "other".into(),
                text: "everything else".into(),
            },
        ],
        state: billing_state(),
    };
    let rows = [m.build(&choice).unwrap(), m.build(&q).unwrap()];
    let outs = m.run(&rows).unwrap();
    assert_eq!(outs.len(), 2);
    assert_eq!(outs[0].probs.len(), 4);
    assert_eq!(outs[1].probs.len(), 2);
    let top = outs[0]
        .probs
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .unwrap()
        .0;
    assert_eq!(top, 0, "billing should win: {:?}", outs[0].probs);
    // Batched and single answers agree (padding does not leak).
    assert!(
        (outs[1].probs[1] - out.probs[1]).abs() < 0.02,
        "{} vs {}",
        outs[1].probs[1],
        out.probs[1]
    );
}

#[tokio::test]
async fn actor_end_to_end_with_report() {
    let Some((data, dir)) = model_dir() else {
        return;
    };
    // `--nocapture` shows the engine's "model loaded" / "unloaded" info lines.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new("info"))
        .with_test_writer()
        .try_init();
    let tmp = tempfile::tempdir().unwrap();
    let log = Log::new(tmp.path().join("decisions"));
    let cfg = DecisionConfig {
        families: [
            ("meeting.topic".to_owned(), FamilyGate::shadow(0.85, 0.40)),
            (
                "meeting.brief".to_owned(),
                FamilyGate {
                    mode: Mode::Active,
                    tau_top: 0.5,
                    tau_margin: 0.1,
                },
            ),
        ]
        .into(),
        ..DecisionConfig::default()
    };
    let engine = Engine::spawn(Engine::new(cfg, dir, data, log));
    // First ask: the cold load outruns a 400 ms budget → Timeout, then loaded.
    let mut ask = Ask::new(
        "meeting.topic",
        Question::Bool {
            statement: "The speakers mention a specific ticket, system or term the listener may need to look up".into(),
            state: "s1: the deploy is blocked on SRE-2386, the image tag pipeline. s2: right, ping the on-call".into(),
        },
    );
    ask.budget = Duration::from_secs(60);
    let d = engine.ask(ask.clone()).await.unwrap();
    let Disposition::Shadow(inner) = &d.disposition else {
        panic!("{d:?}")
    };
    assert!(matches!(
        inner.disposition,
        Disposition::Accept | Disposition::Escalate(Reason::LowTop1 | Reason::LowMargin)
    ));
    assert_eq!(d.probabilities.len(), 2);
    assert_eq!(d.probabilities[0].0, "false");
    assert!(d.choice == "true" || d.choice == "false");
    assert_ne!(d.gate_version, 0);

    // Choice: `none` appended, active gate answers.
    ask.budget = Duration::from_secs(5);
    let choice = Ask {
        family: "meeting.brief".into(),
        question: Question::Choice {
            instructions: "which cached brief is the subject being discussed".into(),
            candidates: vec![
                Candidate {
                    id: "SRE-2386".into(),
                    text: "SRE-2386: image tag pipeline outage".into(),
                },
                Candidate {
                    id: "HR-12".into(),
                    text: "HR-12: parental leave policy".into(),
                },
            ],
            state: "s1: the deploy is blocked on SRE-2386, the image tag pipeline.".into(),
        },
        budget: Duration::from_secs(5),
    };
    let d2 = engine.ask(choice).await.unwrap();
    let labels: Vec<&str> = d2.probabilities.iter().map(|(l, _)| l.as_str()).collect();
    assert_eq!(labels, ["SRE-2386", "HR-12", "none"]);
    assert!(
        matches!(
            d2.disposition,
            Disposition::Accept | Disposition::Escalate(_)
        ),
        "{d2:?}"
    );
    eprintln!(
        "bool: {:?}\nchoice: {:?}",
        d.probabilities, d2.probabilities
    );

    engine
        .tell(Report {
            log_id: d.log_id,
            outcome: Outcome::Confirmed,
        })
        .await
        .unwrap();
    engine
        .tell(Report {
            log_id: d2.log_id,
            outcome: Outcome::Corrected {
                actual: "none".into(),
            },
        })
        .await
        .unwrap();
    let s = engine
        .ask(Stats {
            family: "meeting.topic".into(),
        })
        .await
        .unwrap();
    assert_eq!(s.asks, 1);
    assert_eq!(s.confirmed, 1);
    assert!(s.p50_ms > 0);
    let s2 = engine
        .ask(Stats {
            family: "meeting.brief".into(),
        })
        .await
        .unwrap();
    assert_eq!((s2.asks, s2.corrected), (1, 1));
    // The first row carries the cold-load time.
    let file = std::fs::read_dir(tmp.path().join("decisions"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let text = std::fs::read_to_string(file).unwrap();
    assert!(
        text.lines().next().unwrap().contains("\"load_ms\":"),
        "{text}"
    );
}
