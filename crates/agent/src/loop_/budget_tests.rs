//! 136 §2.2 budget guard, through a real [`Turn`] against the fake provider
//! of [`super::vision_fallback_tests`]: a session whose spend already passes
//! its cap makes no model call — the turn ends `end_turn`, not `error` — and
//! the next turn's input carries one `<budget spent cap currency>` note before
//! its (again refused) call. One tokio test: the tool registry is a
//! process-global actor bound to the runtime that spawned it.

use super::vision_fallback_tests::{Fake, Harness};
use super::*;

#[test]
fn budget_ends_the_turn_cleanly_and_notes_the_next_input() {
    crate::with_test_registry(scenario());
}

fn stop_reason(events: &[AgentEvent]) -> Option<&str> {
    events.iter().find_map(|e| match e {
        AgentEvent::TurnFinished { stop_reason, .. } => Some(stop_reason.as_str()),
        _ => None,
    })
}

async fn scenario() {
    let reg_dir = std::env::temp_dir().join(format!("theuth-budget-reg-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&reg_dir).unwrap();
    let reg_rt = Runtime::new(figment::Figment::new(), reg_dir.clone());
    tools::registry::start(&reg_rt).await.unwrap();

    let mut h = Harness::new(Fake::harvest_held(Duration::ZERO)).await;
    // One settled round call of this session at $0.25 — the spend the guard reads.
    let call = h
        .store
        .reserve_call(h.id, "r0", 0, None, CallKind::Round, Some("openai"), Some("openai/textonly"))
        .await
        .unwrap();
    h.store.mark_dispatched(&call).await.unwrap();
    h.store
        .settle_call(
            &call,
            CallStatus::Settled,
            CostObservation {
                returned: Some(Money {
                    currency: "USD".into(),
                    nanos: 250_000_000,
                }),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let budget = Some(crate::Budget {
        nanos: 10_000_000,
        currency: "USD".into(),
    });

    // Turn 1: refused at round 0; `end_turn`, no request, the note queued.
    let (mut turn, tid) = h.turn("openai/textonly", CancellationToken::new()).await;
    turn.state.budget = budget.clone();
    let state = turn.run(tid, vec![h.input("first ask", false)]).await;
    let events = h.drain().await;
    assert_eq!(stop_reason(&events), Some("end_turn"), "{events:#?}");
    assert!(h.requests().await.is_empty(), "nothing was sent");
    let note = state.budget_note.clone().expect("the note is queued");
    assert!(note.starts_with("<budget spent=\"250000000\" cap=\"10000000\" currency=\"USD\">\n"));
    assert!(
        !state.history.iter().any(|(_, m)| text_of(&m.content).starts_with("<budget")),
        "not in this turn's frame yet"
    );

    // Turn 2 (the state handed back by `TurnDone`): the note rides the input,
    // once, before the call; the call is refused again.
    let (mut turn, tid) = h.turn("openai/textonly", CancellationToken::new()).await;
    turn.state = state;
    let state = turn.run(tid, vec![h.input("second ask", false)]).await;
    let events = h.drain().await;
    assert_eq!(stop_reason(&events), Some("end_turn"));
    assert!(h.requests().await.is_empty());
    let texts: Vec<String> = state
        .history
        .iter()
        .map(|(_, m)| text_of(&m.content))
        .collect();
    let input_at = texts.iter().position(|t| t == "second ask").unwrap();
    let notes: Vec<usize> = texts
        .iter()
        .enumerate()
        .filter(|(_, t)| t.starts_with("<budget spent=\"250000000\""))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(notes.len(), 1, "one note: {texts:#?}");
    assert!(notes[0] > input_at, "after the input it rides: {texts:#?}");
    assert!(state.budget_note.is_some(), "the second refusal queues again");
    assert_eq!(
        h.store.budget_spent(h.id, "USD", "openai/textonly").await.unwrap(),
        (250_000_000, 250_000_000),
        "no row was written for a refused call"
    );

    let _ = std::fs::remove_dir_all(&reg_dir);
}
