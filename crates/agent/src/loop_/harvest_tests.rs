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

#[tokio::test]
async fn harvest_runs_detached_from_the_turn() {
    let _registry = crate::REGISTRY_TEST_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let reg_dir = std::env::temp_dir().join(format!("theuth-harvest-reg-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&reg_dir).unwrap();
    let reg_rt = Runtime::new(figment::Figment::new(), reg_dir.clone());
    tools::registry::start(&reg_rt).await.unwrap();
    // The knowledge bundle every turn's `rebase` finds (12 §9.2).
    runtime::Plugin::start(Box::new(knowledge::KnowledgePlugin), &reg_rt)
        .await
        .unwrap();

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
        state.harvest_task.as_ref().is_some_and(|t| !t.is_finished()),
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
        .run(tid2, vec![h.input("And the deploy host is deploybox.", false)])
        .await;
    let events = h.drain().await;
    assert_eq!(harvesting(&events), 0, "skipped while one is in flight");
    assert!(state.harvest_task.as_ref().is_some_and(|t| !t.is_finished()));

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

    let _ = std::fs::remove_dir_all(&reg_dir);
}
