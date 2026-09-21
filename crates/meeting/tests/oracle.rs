//! 57 §3 end to end against the real checkpoint in `~/.theuth/models/decision/` (or
//! `$DECISION_MODEL_DIR`): a fake transcript batch through [`meeting::oracle::run`] with the
//! decision actor registered — shadow produces an `OracleDecision` and still takes today's path;
//! active acts on the verdict. Skips with a printed reason when the model is not there. No agent
//! actor runs here, so a tier-2 attempt fails over to tier 3 (`agent is not running`).

use std::path::PathBuf;
use std::sync::Arc;

use decision::engine::MODEL_FILES;
use decision::log::Log;
use decision::{DecisionConfig, Engine, Mode};
use kameo::actor::Spawn;
use kameo::message::{Context, Message};
use meeting::oracle::{self, Action, OracleDisposition};
use meeting::{MeetingEvent, MeetingId, Track, Utterance};
use parking_lot::Mutex;
use runtime::{Runtime, SessionId};

fn model_dir() -> Option<PathBuf> {
    let data = std::env::var_os("THEUTH_DATA_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".theuth")))?;
    let dir = std::env::var_os("DECISION_MODEL_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| data.join("models/decision"));
    if MODEL_FILES.iter().all(|f| dir.join(f).is_file()) {
        Some(dir)
    } else {
        eprintln!("skipped: no laya checkpoint in {}", dir.display());
        None
    }
}

/// Collects every `MeetingEvent` the bus publishes.
#[derive(kameo::Actor)]
struct Sink(Arc<Mutex<Vec<MeetingEvent>>>);

impl Message<MeetingEvent> for Sink {
    type Reply = ();
    async fn handle(&mut self, e: MeetingEvent, _: &mut Context<Self, ()>) {
        self.0.lock().push(e);
    }
}

fn batch(lines: &[(Track, &str)]) -> Vec<Utterance> {
    lines
        .iter()
        .enumerate()
        .map(|(i, (track, text))| Utterance {
            track: *track,
            t0_ms: 872_000 + i as u64 * 2_000,
            t1_ms: 873_000 + i as u64 * 2_000,
            text: (*text).to_owned(),
            request: false,
        })
        .collect()
}

#[tokio::test]
async fn shadow_reports_and_active_gates() {
    let Some(dir) = model_dir() else {
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let rt = Runtime::new(figment::Figment::new(), tmp.path().to_owned());
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = Sink::spawn_with_mailbox(Sink(events.clone()), runtime::subscriber_mailbox());
    rt.subscribe::<Sink, MeetingEvent>(&sink).await;

    // Generous tier-1 budget: the cold load counts against the first ask (step 2 as-built).
    let mut cfg = DecisionConfig::default();
    for g in cfg.families.values_mut() {
        g.budget = std::time::Duration::from_secs(60);
    }
    let engine = Engine::spawn(Engine::new(cfg, dir, Log::new(tmp.path().join("decisions"))));
    engine.register(decision::DECISION_ACTOR).unwrap();
    let meeting = MeetingId::new();
    let session = SessionId::new();

    // Shadow (the default): an event, and today's path — no evidence line, the outcome pending.
    let v = oracle::run(
        rt.clone(),
        None,
        Vec::new(),
        batch(&[
            (Track::System, "so the deploy is blocked on SRE-2386, the image tag pipeline"),
            (Track::Mic, "right, ping the on-call"),
        ]),
        meeting,
        session,
    )
    .await;
    assert!(matches!(v.action, Action::Model { evidence: None }), "{:?}", v.action);
    let e = v.event.expect("shadow emits the verdict");
    assert_eq!((e.mode, e.batch_id.as_str(), e.outcome), (Mode::Shadow, "00:14:32", None));
    assert_eq!(e.top[0].0, "topic");
    assert!(v.expected.is_some(), "shadow waits for the turn's outcome");
    tokio::time::sleep(std::time::Duration::from_millis(100)).await; // the bus delivers by `tell`
    let seen = events.lock().clone();
    assert!(seen.iter().any(|e| matches!(e, MeetingEvent::OracleDeciding { batch_id, .. } if batch_id == "00:14:32")));
    assert!(seen.iter().any(|e| matches!(e, MeetingEvent::OracleDecision { decision, .. } if decision.mode == Mode::Shadow)));
    eprintln!("shadow: {:?} in {} ms, top {:?}", e.disposition, e.latency_ms, e.top);

    // Active: the verdict is acted on. Small talk → `topic=false` → skip, no delivery.
    engine
        .ask(decision::SetMode {
            prefix: oracle::PREFIX.to_owned(),
            mode: Mode::Active,
        })
        .await
        .unwrap();
    let v = oracle::run(
        rt.clone(),
        None,
        Vec::new(),
        batch(&[(Track::System, "yeah, sounds good"), (Track::Mic, "ok, thanks, talk later")]),
        meeting,
        session,
    )
    .await;
    let e = v.event.expect("active emits the verdict");
    assert_eq!(e.mode, Mode::Active);
    eprintln!("active: {:?} tier {} in {} ms, top {:?}", e.disposition, e.tier, e.latency_ms, e.top);
    match (&e.disposition, &v.action) {
        (OracleDisposition::Skip, Action::Skip) => {
            assert_eq!((e.tier, e.family.as_str()), (1, oracle::TOPIC));
            assert!(v.expected.is_none(), "a skipped batch has no model turn to compare with");
        }
        // No agent actor here: an open verdict goes to tier 2, fails over, and reaches tier 3 with evidence.
        (OracleDisposition::Model { reason }, Action::Model { evidence: Some(ev) }) => {
            assert!(reason.contains("tier 2"), "{reason}");
            assert!(ev.starts_with("oracle: topic "), "{ev}");
            assert_eq!(e.tier, 3);
        }
        other => panic!("disposition and action disagree: {other:?}"),
    }

    // Off: nothing asked, nothing emitted, today's path.
    engine
        .ask(decision::SetMode {
            prefix: oracle::PREFIX.to_owned(),
            mode: Mode::Off,
        })
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let before = events.lock().len();
    let v = oracle::run(rt, None, Vec::new(), batch(&[(Track::Mic, "hello")]), meeting, session).await;
    assert!(v.event.is_none() && v.expected.is_none());
    assert!(matches!(v.action, Action::Model { evidence: None }));
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(events.lock().len(), before);
}
