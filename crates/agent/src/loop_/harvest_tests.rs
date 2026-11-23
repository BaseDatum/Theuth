//! 10 §5.2 as built, through a real [`Turn`] against the fake provider of
//! [`super::vision_fallback_tests`]: the harvest runs detached — the turn end
//! (and `/knowledge harvest`) hands the state back while the provider still
//! holds the harvest reply, `Harvesting`/`Harvested` still fire, and a second
//! trigger meanwhile is skipped. One tokio test: the tool registry and the
//! knowledge actor are process-global actors bound to the runtime that
//! spawned them.

use std::time::Duration;

use super::vision_fallback_tests::{Fake, Harness};
use super::*;

/// How long the fake holds the harvest reply: long enough that a turn end
/// and its assertions run first, short enough for the 10 s event timeout.
const HOLD: Duration = Duration::from_secs(3);

fn harvesting(events: &[AgentEvent]) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, AgentEvent::Harvesting { .. }))
        .count()
}

fn harvested(events: &[AgentEvent]) -> Option<(u32, u32)> {
    events.iter().find_map(|e| match e {
        AgentEvent::Harvested { added, updated, .. } => Some((*added, *updated)),
        _ => None,
    })
}

#[test]
fn harvest_runs_detached_from_the_turn() {
    crate::with_test_registry(harvest_scenarios());
}

async fn harvest_scenarios() {
    let scratch = crate::test_dir("harvest-reg");
    let reg_dir = scratch.path().to_path_buf();
    let reg_rt = Runtime::new(figment::Figment::new(), reg_dir.clone());
    tools::registry::start(&reg_rt).await.unwrap();
    // The knowledge bundle every turn's `rebase` finds (12 §9.2).
    super::vision_fallback_tests::start_main_knowledge(&reg_rt).await;

    let mut h = Harness::new(Fake::harvest_held(HOLD)).await;
    let every = AgentConfig::default().knowledge.harvest_every;
    assert!(every > 0, "harvest on by default");

    // Trigger (a): a tool-using root turn that makes the count due. The
    // turn returns its state while the provider still holds the harvest reply.
    let (mut turn, tid) = h.turn("openai/textonly", CancellationToken::new()).await;
    let shared = turn.shared.clone();
    turn.calls = 1;
    turn.state.turns_since_harvest = every - 1;
    let state = turn
        .run(tid, vec![h.input("The build host is buildbox.", false)])
        .await;
    assert_eq!(state.turns_since_harvest, 0, "reset at the trigger");
    assert!(
        state
            .harvest_task
            .as_ref()
            .is_some_and(|t| !t.is_finished()),
        "the harvest is still running when the turn hands its state back"
    );
    let events = h.drain().await;
    assert_eq!(harvesting(&events), 1);
    assert_eq!(harvested(&events), None, "not before the reply");

    // A second due turn while it runs: skipped, no second `Harvesting`, the
    // running task untouched.
    let (_, tid2) = h.turn("openai/textonly", CancellationToken::new()).await;
    let mut second = Turn::new(shared.clone(), Some(tid2), CancellationToken::new(), state);
    second.calls = 1;
    second.state.turns_since_harvest = every - 1;
    let state = second
        .run(
            tid2,
            vec![h.input("And the deploy host is deploybox.", false)],
        )
        .await;
    let events = h.drain().await;
    assert_eq!(harvesting(&events), 0, "skipped while one is in flight");
    assert!(
        state
            .harvest_task
            .as_ref()
            .is_some_and(|t| !t.is_finished())
    );

    // `/knowledge harvest` meanwhile is refused the same way.
    let mut manual = Turn::new(shared.clone(), None, CancellationToken::new(), state);
    assert_eq!(
        manual.harvest_now().await.err(),
        Some("harvest in flight".to_owned())
    );

    // The reply lands: the concept is written and `Harvested` reports it.
    let events = h
        .drain_until(|e| matches!(e, AgentEvent::Harvested { .. }))
        .await;
    assert_eq!(harvested(&events), Some((1, 0)));
    let knowledge = manual.knowledge().expect("knowledge is on");
    assert!(
        knowledge.get("facts/detached-harvest").await.is_ok(),
        "the put landed"
    );

    // `/knowledge harvest` between turns: `Ok` at once, the result later.
    let mut landed = manual.harvest_now().await.unwrap();
    let events = h
        .drain_until(|e| matches!(e, AgentEvent::Harvesting { .. }))
        .await;
    assert_eq!(harvesting(&events), 1);
    assert!(
        tokio::time::timeout(Duration::from_millis(200), &mut landed)
            .await
            .is_err(),
        "the reply is still held"
    );
    let result = landed.await.unwrap().unwrap();
    assert_eq!((result.added, result.updated, result.skipped), (0, 1, 0));
    let events = h
        .drain_until(|e| matches!(e, AgentEvent::Harvested { .. }))
        .await;
    assert_eq!(harvested(&events), Some((0, 1)));

    // 136 P2 D4: the dreaming session never harvests itself — a due turn
    // neither counts nor fires, compaction's pre-harvest is skipped, and
    // `/knowledge harvest` is refused by name.
    knowledge.register_consolidating_session(shared.id);
    let (mut night, tid3) = h.turn("openai/textonly", CancellationToken::new()).await;
    night.calls = 1;
    night.state.turns_since_harvest = every - 1;
    let state = night
        .run(tid3, vec![h.input("Retitled 300 concepts.", false)])
        .await;
    let events = h.drain().await;
    assert_eq!(harvesting(&events), 0, "no self-harvest");
    assert_eq!(
        state.turns_since_harvest,
        every - 1,
        "the count does not move for the dreaming session"
    );
    let mut night = Turn::new(shared.clone(), None, CancellationToken::new(), state);
    assert_eq!(
        night.harvest_now().await.err(),
        Some("the dreaming session does not harvest itself".to_owned())
    );
    night.calls = 1;
    night
        .compact_now(Some(CompactStyle::Cut), None)
        .await
        .unwrap();
    let events = h
        .drain_until(|e| matches!(e, AgentEvent::Compacted { .. }))
        .await;
    assert_eq!(harvesting(&events), 0, "no pre-compaction harvest");
    knowledge.clear_consolidating_session();
}
