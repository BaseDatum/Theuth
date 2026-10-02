//! 81 COMPACT-FLOOR through the real [`Session`] actor against the fake
//! provider of [`super::vision_fallback_tests`]: a model whose trigger leaves
//! less than `MIN_HEADROOM` over the fixed part (a 9k window at `compact_at`
//! 0.8) turns auto-compaction off once — one `<context_pressure>` note, no
//! `Compacting` — and the next over-trigger turn neither notes nor compacts.
//! One tokio test: the tool registry is a process-global actor bound to the
//! runtime that spawned it.

use super::vision_fallback_tests::{Fake, Harness};
use super::*;

fn count(events: &[AgentEvent], f: impl Fn(&AgentEvent) -> bool) -> usize {
    events.iter().filter(|e| f(e)).count()
}

fn stop_reasons(events: &[AgentEvent]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::TurnFinished { stop_reason, .. } => Some(stop_reason.as_str()),
            _ => None,
        })
        .collect()
}

#[test]
fn floor_turns_auto_compaction_off_once() {
    crate::with_test_registry(scenario());
}

async fn scenario() {
    let reg_dir = std::env::temp_dir().join(format!("theuth-compact-floor-reg-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&reg_dir).unwrap();
    let reg_rt = Runtime::new(figment::Figment::new(), reg_dir.clone());
    tools::registry::start(&reg_rt).await.unwrap();

    let mut h = Harness::new(Fake::compact_held(Duration::ZERO)).await;
    let actor = h.actor("openai/tiny").await;
    // The Harness's `tiny` model: a 9k window.
    let limit = (AgentConfig::default().compact_at * 9_000.0) as u32;
    assert!(
        compact::below_floor(limit, 0),
        "trigger {limit} is not under the floor"
    );
    let input = |text: String| UserInput {
        id: None,
        text,
        files: Vec::new(),
        ticket: None,
        scope: None,
        defer: false,
    };
    // A small first turn: the provider's usage is what `fixed` is read from.
    actor.tell(input("First question.".into())).await.unwrap();
    let events = h.drain().await;
    assert_eq!(stop_reasons(&events), ["stop"]);

    // Two turns projecting past the trigger: the first turns compaction off
    // with the note, the second is left alone.
    let big = "x".repeat(4 * limit as usize + 4_000);
    let mut all = Vec::new();
    for _ in 0..2 {
        actor.tell(input(big.clone())).await.unwrap();
        let events = h.drain().await;
        assert_eq!(stop_reasons(&events), ["stop"]);
        all.extend(events);
    }
    assert_eq!(
        count(&all, |e| matches!(e, AgentEvent::Compacting { .. })),
        0
    );
    assert_eq!(count(&all, |e| matches!(e, AgentEvent::Error { .. })), 0);
    let rows = h.store.messages(h.id).await.unwrap();
    let notes: Vec<String> = rows
        .iter()
        .map(|(_, m)| text_of(&m.content))
        .filter(|t| t.starts_with("<context_pressure>"))
        .collect();
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(
        notes[0].contains("compaction cannot help") && notes[0].contains("context window"),
        "{}",
        notes[0]
    );
    assert_eq!(h.store.frame(h.id).await.unwrap().0, 0, "no epoch opened");

    let _ = std::fs::remove_dir_all(&reg_dir);
}
