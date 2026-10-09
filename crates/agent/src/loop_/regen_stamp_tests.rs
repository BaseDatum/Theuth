//! REGEN-STAMP (74 §3, §5): a regenerated turn replays the stored opener —
//! which carries the model's `<time …/>` stamp — yet what frontends get
//! (`TurnStarted.text`, the history row) is the human's words alone, as on
//! the send path. Driven through the real [`Session`] actor against the fake
//! provider of [`super::vision_fallback_tests`]. One tokio test: the tool
//! registry is a process-global actor bound to the runtime that spawned it.

use std::collections::BTreeMap;

use crate::HistoryEntry;

use super::vision_fallback_tests::{Fake, Harness, texts};
use super::*;

const HUMAN: &str = "First question.";

fn started(events: &[AgentEvent]) -> Vec<(i64, String)> {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::TurnStarted { turn_seq, text, .. } => Some((*turn_seq, text.clone())),
            _ => None,
        })
        .collect()
}

fn shown(history: &[(i64, ChatMessage)]) -> Vec<String> {
    crate::supervisor::render(history, &BTreeMap::new(), &BTreeMap::new())
        .into_iter()
        .filter_map(|(_, e)| match e {
            HistoryEntry::User { text, .. } => Some(text),
            _ => None,
        })
        .collect()
}

#[test]
fn regenerate_replays_the_stamp_to_the_model_only() {
    crate::with_test_registry(scenario());
}

async fn scenario() {
    let scratch = crate::TestDir::new("regen-stamp-reg");
    let reg_dir = scratch.0.clone();
    let reg_rt = Runtime::new(figment::Figment::new(), reg_dir.clone());
    tools::registry::start(&reg_rt).await.unwrap();

    let mut h = Harness::new(Fake::compact_held(Duration::ZERO)).await;
    let actor = h.actor("openai/textonly").await;
    actor
        .tell(UserInput {
            id: None,
            text: HUMAN.into(),
            files: Vec::new(),
            ticket: None,
            scope: None,
            defer: false,
        })
        .await
        .unwrap();
    let events = h.drain().await;
    // The actor is idle only after the handback (`TurnDone` → the settled `ContextChanged`).
    h.drain_until(
        |e| matches!(e, AgentEvent::ContextChanged { reason, .. } if reason == "settled"),
    )
    .await;
    let opened = started(&events);
    let [(turn, text)] = opened.as_slice() else {
        panic!("one turn: {events:?}");
    };
    let turn = *turn;
    assert_eq!(text, HUMAN, "the send path publishes the bare text");
    // The first input of a session is stamped: the stored opener is the
    // model's view, the history row the human's.
    let (_, _, history) = h.store.frame(h.id).await.unwrap();
    let stored = text_of(&history[0].1.content);
    assert_eq!(clock::strip(&stored), HUMAN);
    assert!(clock::parse(&stored).is_some(), "{stored}");
    assert_eq!(shown(&history), [HUMAN]);
    let before = h.requests().await.len();

    actor
        .ask(RestartTurn {
            turn,
            input: None,
            ticket: None,
        })
        .await
        .unwrap();
    let events = h.drain().await;

    // Frontends: the human's words, stamp-free.
    assert_eq!(started(&events), [(turn, HUMAN.to_owned())]);
    let (_, _, history) = h.store.frame(h.id).await.unwrap();
    assert_eq!(shown(&history), [HUMAN]);

    // The model: the replayed opener, stamp and all, byte-identical.
    assert_eq!(text_of(&history[0].1.content), stored);
    let requests = h.requests().await;
    let sent: Vec<Vec<String>> = requests[before..].iter().map(texts).collect();
    assert!(
        sent.iter().flatten().any(|t| t == &stored),
        "the model frame carries the stamped opener: {sent:?}"
    );
}
