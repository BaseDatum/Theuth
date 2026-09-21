//! 57 §3 / §3.5 end to end against the real checkpoint in `~/.theuth/models/decision/` (or
//! `$DECISION_MODEL_DIR`): fake transcript batches through [`meeting::oracle::run`] with the
//! decision actor registered and a fake tier-2 side caller. Shadow runs the whole pipeline
//! (tier 2 included) and still takes today's path; active acts on the verdict. Skips with a
//! printed reason when the model is not there.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use decision::engine::MODEL_FILES;
use decision::log::Log;
use decision::{DecisionConfig, Engine, Mode};
use kameo::actor::Spawn;
use kameo::message::{Context, Message};
use meeting::oracle::{self, Action, OracleDisposition};
use meeting::{MeetingEvent, MeetingId, Track, Utterance};
use parking_lot::Mutex;
use runtime::{Runtime, SessionId, SideCall, SideReply};

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
        eprintln!("skipping: no decision checkpoint in {}", dir.display());
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

/// A tier-2 side caller that answers `reply` and records what it was sent.
fn fake_side(reply: &'static str, seen: Arc<Mutex<Vec<SideCall>>>) -> Arc<decision::SideCaller> {
    Arc::new(move |call: SideCall| {
        seen.lock().push(call);
        Box::pin(async move {
            Ok(SideReply {
                text: reply.to_owned(),
                model: "fake/cheap".to_owned(),
                input_tokens: Some(10),
                output_tokens: Some(5),
                latency: Duration::from_millis(3),
            })
        })
    })
}

const KUIPER: [(Track, &str); 4] = [
    (Track::System, "So Mike was telling me about this at lunch. They were looking at the orbits of a few distant objects in the Kuiper belt, way out past Neptune, and the orbits were all clustered in a weird way."),
    (Track::System, "Constantin said the simplest explanation was a hidden planet, something maybe five or ten times the mass of Earth, and they started calling it Planet Nine."),
    (Track::Mic, "Has anyone actually seen it?"),
    (Track::System, "No, that's the thing. It's all inferred from the clustering. They think it could take years to find it with a telescope because it would be so faint that far out."),
];

#[tokio::test]
async fn shadow_runs_the_pipeline_and_active_gates() {
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
        g.budget = Duration::from_secs(60);
    }
    let engine = Engine::spawn(Engine::new(cfg, dir, Log::new(tmp.path().join("decisions"))));
    engine.register(decision::DECISION_ACTOR).unwrap();
    let meeting = MeetingId::new();
    let session = SessionId::new();
    let seen = Arc::new(Mutex::new(Vec::new()));

    // Shadow (the default), the Kuiper batch: mentions are not empty, so tier 1 cannot skip;
    // tier 2 (the fake) names the subject; the event carries the whole pipeline and today's path
    // still runs with no evidence line.
    let side = fake_side(r#"{"subject": "Planet Nine", "cached": null, "lookup": true}"#, seen.clone());
    let v = oracle::run(rt.clone(), None, Vec::new(), batch(&KUIPER), meeting, session, side.clone()).await;
    assert!(matches!(v.action, Action::Model { evidence: None }), "{:?}", v.action);
    let e = v.event.expect("shadow emits the verdict");
    eprintln!(
        "shadow kuiper: {:?} tier {} in {} ms, stages {:?}, mentions {:?}, tier2 {:?}",
        e.disposition, e.tier, e.latency_ms, e.stages, e.mentions, e.tier2
    );
    assert_eq!((e.mode, e.batch_id.as_str(), e.outcome), (Mode::Shadow, "00:14:32", None));
    assert!(e.mentions.iter().any(|m| m.starts_with("Kuiper")), "{:?}", e.mentions);
    assert!(e.mentions.iter().any(|m| m == "Planet Nine"), "{:?}", e.mentions);
    assert_ne!(e.disposition, OracleDisposition::Skip, "mentions present: never a tier-1 skip");
    assert_eq!(e.subject.as_deref(), Some("Planet Nine"));
    assert_eq!(e.tier2.as_deref(), Some("\"Planet Nine\" → no cached brief"));
    assert_eq!(e.family, oracle::SUBJECT);
    assert!(matches!(&e.disposition, OracleDisposition::Model { reason } if reason.contains("Planet Nine")), "{:?}", e.disposition);
    assert!(e.stages[0].0.starts_with("kind: "), "{:?}", e.stages);
    let x = v.expected.expect("shadow waits for the turn's outcome");
    assert!(x.rows.iter().any(|(_, f, l)| f == oracle::SUBJECT && l == "lookup"), "{:?}", x.rows);
    {
        let calls = seen.lock();
        assert_eq!(calls.len(), 1, "shadow spends exactly one tier-2 call");
        assert!(calls[0].user.contains("Mentions: ") && calls[0].user.contains("Kuiper"), "{}", calls[0].user);
        assert!(calls[0].user.contains("A local classifier estimated: kind "), "{}", calls[0].user);
    }
    tokio::time::sleep(Duration::from_millis(100)).await; // the bus delivers by `tell`
    let seen_events = events.lock().clone();
    assert!(seen_events.iter().any(|e| matches!(e, MeetingEvent::OracleDeciding { batch_id, .. } if batch_id == "00:14:32")));
    assert!(seen_events.iter().any(|e| matches!(e, MeetingEvent::OracleDecision { decision, .. } if decision.mode == Mode::Shadow)));

    // Active: the same batch reaches tier 3 with the subject-led evidence line first.
    engine
        .ask(decision::SetMode {
            prefix: oracle::PREFIX.to_owned(),
            mode: Mode::Active,
        })
        .await
        .unwrap();
    let v = oracle::run(rt.clone(), None, Vec::new(), batch(&KUIPER), meeting, session, side).await;
    let e = v.event.expect("active emits the verdict");
    assert_eq!((e.mode, e.tier), (Mode::Active, 3));
    match &v.action {
        Action::Model { evidence: Some(ev) } => {
            eprintln!("active kuiper evidence: {ev}");
            assert!(ev.starts_with("oracle: subject \"Planet Nine\" — no cached brief; kind "), "{ev}");
            let line = meeting::session::batch_line_with(&v.batch, &[], &[], &[], Some(ev));
            let mut lines = line.lines();
            assert!(lines.next().unwrap().starts_with("[meeting 00:14:32]"));
            assert_eq!(lines.next().unwrap(), ev, "the evidence line comes first, before the transcript rows");
        }
        other => panic!("expected tier 3 with evidence, got {other:?}"),
    }
    assert!(v.expected.is_some());

    // Active, small talk with no mentions: tier 1 skips when P(small talk) clears τ; below τ the
    // batch goes to tier 2, whose `subject: null` is a skip too. Either way: no model turn.
    let side = fake_side(r#"{"subject": null, "cached": null, "lookup": false}"#, seen.clone());
    let v = oracle::run(
        rt.clone(),
        None,
        Vec::new(),
        batch(&[(Track::System, "yeah, sounds good"), (Track::Mic, "ok, thanks, talk later")]),
        meeting,
        session,
        side,
    )
    .await;
    let e = v.event.expect("active emits the verdict");
    eprintln!("active small talk: {:?} tier {} in {} ms, stages {:?}, tier2 {:?}", e.disposition, e.tier, e.latency_ms, e.stages, e.tier2);
    assert!(e.mentions.is_empty(), "{:?}", e.mentions);
    assert_eq!(e.disposition, OracleDisposition::Skip);
    assert!(matches!(v.action, Action::Skip));
    assert!(v.expected.is_none(), "a skipped batch has no model turn to compare with");
    match e.tier {
        1 => assert_eq!(e.family, oracle::KIND),
        2 => assert_eq!((e.family.as_str(), e.tier2.as_deref()), (oracle::SUBJECT, Some("nothing to look up"))),
        t => panic!("tier {t}"),
    }

    // Active, tier 2 picks a cached brief → published (needs a cache with rows; covered by the
    // unit tests on `settle`/`publish` — here only the no-cache path runs).

    // Off: nothing asked, nothing emitted, today's path.
    engine
        .ask(decision::SetMode {
            prefix: oracle::PREFIX.to_owned(),
            mode: Mode::Off,
        })
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let before = events.lock().len();
    let side = fake_side("{}", seen.clone());
    let v = oracle::run(rt, None, Vec::new(), batch(&[(Track::Mic, "hello")]), meeting, session, side).await;
    assert!(v.event.is_none() && v.expected.is_none());
    assert!(matches!(v.action, Action::Model { evidence: None }));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(events.lock().len(), before);
}
