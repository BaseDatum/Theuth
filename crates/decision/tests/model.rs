//! End-to-end against the real checkpoint in `~/.theuth/models/decision/` (or
//! `$DECISION_MODEL_DIR`; `$THEUTH_DATA_DIR` moves the data dir). Skips with a printed reason
//! when it is not there.

use std::path::PathBuf;
use std::time::Duration;

use candle_core::DType;
use decision::candle::{Gpu, Loaded};
use decision::engine::model_files;
use decision::log::Log;
use decision::{
    Ask, Candidate, DecisionConfig, Device, Disposition, Engine, FamilyGate, LoadedNow, Mode,
    Outcome, Question, Reason, Release, Report, Stats, process_memory,
};
use kameo::actor::Spawn;

/// The model dir, or `None` with a printed reason.
fn model_dir() -> Option<PathBuf> {
    let data = std::env::var_os("THEUTH_DATA_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".theuth")))?;
    let dir = std::env::var_os("DECISION_MODEL_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| data.join("models/decision"));
    if model_files(decision::Kind::Laya)
        .iter()
        .all(|f| dir.join(f).is_file())
    {
        Some(dir)
    } else {
        eprintln!("skipped: no laya checkpoint in {}", dir.display());
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

/// The README's four questions on the default device (`Device::Auto`: Metal f16 on an
/// Apple-silicon Mac, CPU f32 elsewhere) against the step-2d ORT fp32 CPU probabilities —
/// argmax equal and max |Δp| ≤ 0.01 (57 §12.8). ORT is gone from the crate; the fixture stays
/// the reference because it was produced by an independent runtime from an independent export.
#[test]
fn readme_questions_match_the_reference() {
    let Some(dir) = model_dir() else {
        return;
    };
    let reference: Vec<Vec<f32>> =
        serde_json::from_str(include_str!("fixtures/probs-cpu-fp32.json")).unwrap();
    let metal = Device::Auto.metal();
    let dtype = if metal { DType::F16 } else { DType::F32 };
    let m = Loaded::open(&dir, if metal { Gpu::Metal } else { Gpu::None }, dtype).expect("open");
    let at_load = m.model_sha.lock().clone();
    eprintln!(
        "{:?} {:?}: load {:?}, model id at load {at_load}",
        m.device.location(),
        m.dtype,
        m.load_time
    );
    assert!(
        at_load.starts_with("size:") || at_load.len() == 64,
        "{at_load}"
    );
    assert_eq!((m.laya.max_len, m.laya.head_max_len), (512, 192));
    let rows: Vec<_> = readme().iter().map(|q| m.build(q).unwrap()).collect();
    assert!(rows.iter().all(|r| !r.truncated));
    let outs = m.run(&rows).unwrap();
    assert_eq!(outs.len(), 4);
    let mut worst = 0.0f32;
    for (out, want) in outs.iter().zip(&reference) {
        assert_eq!(out.probs.len(), want.len());
        assert!((out.probs.iter().sum::<f32>() - 1.0).abs() < 1e-4);
        assert_eq!(
            argmax(&out.probs),
            argmax(want),
            "{:?} vs {want:?}",
            out.probs
        );
        for (p, q) in out.probs.iter().zip(want) {
            worst = worst.max((p - q).abs());
        }
        assert!((0.0..=1.0).contains(&out.act_p));
    }
    eprintln!("max |Δp| vs the fp32 reference: {worst:.4}");
    assert!(worst <= 0.01, "max |Δp| {worst} > 0.01");
    // One row alone equals its batched answer (padding to the batch's S/K does not leak).
    let alone = m.run(&rows[2..3]).unwrap().remove(0);
    assert!((alone.probs[1] - outs[2].probs[1]).abs() < 0.005);
}

/// 126 §7.2: `run_with(rows, true)` returns the pooled `[CLS]` state per row, `(hidden,)` =
/// 1024 for laya base, finite, and different for different questions; `run` leaves it `None`.
#[test]
fn pooled_embedding_is_hidden_sized_and_opt_in() {
    let Some(dir) = model_dir() else {
        return;
    };
    let metal = Device::Auto.metal();
    let dtype = if metal { DType::F16 } else { DType::F32 };
    let m = Loaded::open(&dir, if metal { Gpu::Metal } else { Gpu::None }, dtype).expect("open");
    let rows: Vec<_> = readme().iter().map(|q| m.build(q).unwrap()).collect();
    let plain = m.run(&rows).unwrap();
    assert!(plain.iter().all(|r| r.embedding.is_none()));
    let outs = m.run_with(&rows, true).unwrap();
    assert_eq!(outs.len(), rows.len());
    for (a, b) in outs.iter().zip(&plain) {
        let e = a.embedding.as_ref().expect("embedding");
        assert_eq!(e.len(), 1024);
        assert!(e.iter().all(|v| v.is_finite()));
        assert!(
            a.probs.iter().zip(&b.probs).all(|(x, y)| (x - y).abs() < 1e-4),
            "the embedding copy does not change the scores: {:?} vs {:?}",
            a.probs,
            b.probs
        );
    }
    let (e0, e1) = (outs[0].embedding.as_ref().unwrap(), outs[1].embedding.as_ref().unwrap());
    assert!(e0.iter().zip(e1).any(|(x, y)| (x - y).abs() > 1e-3));
}

#[tokio::test]
async fn actor_end_to_end_with_report() {
    let Some(dir) = model_dir() else {
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
                    ..FamilyGate::shadow(0.5, 0.1)
                },
            ),
        ]
        .into(),
        ..DecisionConfig::default()
    };
    let engine = Engine::spawn(Engine::new(cfg, dir, log));
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

/// 57 §2.1 item 3: through the actor — nothing resident at spawn, a question loads,
/// `Release` drops it and the process footprint comes back to within 50 MB of "before". The
/// first load on a device pays a one-off runtime baseline (the Metal stack: ~90 MB), so the
/// bound is asserted on the second load/release cycle — the steady state the daemon lives in —
/// and the Metal driver returns its pages asynchronously, so "after" is polled for up to 15 s.
#[tokio::test]
async fn release_reclaims_the_checkpoint() {
    let Some(dir) = model_dir() else {
        return;
    };
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new("info"))
        .with_test_writer()
        .try_init();
    let tmp = tempfile::tempdir().unwrap();
    let cfg = DecisionConfig {
        families: [("meeting.kind".to_owned(), FamilyGate::shadow(0.60, 0.20))].into(),
        ..DecisionConfig::default()
    };
    let engine = Engine::spawn(Engine::new(
        cfg,
        dir,
        Log::new(tmp.path().join("decisions")),
    ));
    let mut ask = Ask::new(
        "meeting.kind",
        Question::Bool {
            statement: "The speakers are only greeting, scheduling or saying goodbye".into(),
            state: "them: hi all, can we push the standup to three? me: sure".into(),
        },
    );
    ask.budget = Duration::from_secs(60);
    assert!(engine.ask(LoadedNow).await.unwrap().is_empty());
    let mb = |b: u64| b / 1_000_000;
    let mut rows = Vec::new();
    for cycle in 1..=2 {
        let before = process_memory().footprint;
        engine.ask(ask.clone()).await.unwrap();
        let now = engine.ask(LoadedNow).await.unwrap();
        assert_eq!(now.len(), 1, "{now:?}");
        assert_eq!(now[0].checkpoint, "default");
        let loaded = process_memory().footprint;
        assert!(
            now[0].rss_mb.abs_diff(mb(loaded)) <= 2,
            "{} vs {}",
            now[0].rss_mb,
            mb(loaded)
        );
        engine
            .tell(Release {
                checkpoint: None,
                reason: "test",
            })
            .await
            .unwrap();
        assert!(engine.ask(LoadedNow).await.unwrap().is_empty());
        let t = std::time::Instant::now();
        let mut released = process_memory().footprint;
        while released > before + 50_000_000 && t.elapsed() < Duration::from_secs(15) {
            tokio::time::sleep(Duration::from_millis(250)).await;
            released = process_memory().footprint;
        }
        eprintln!(
            "cycle {cycle}: footprint before {} MB, loaded {} MB, released {} MB after {:?}",
            mb(before),
            mb(loaded),
            mb(released),
            t.elapsed()
        );
        rows.push((before, loaded, released));
    }
    let (before, loaded, released) = rows[1];
    assert!(
        loaded > before + 300_000_000,
        "the load did not show: {rows:?}"
    );
    assert!(
        released <= before + 50_000_000,
        "released {} MB > before {} MB + 50 MB",
        mb(released),
        mb(before)
    );
}
