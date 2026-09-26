//! 70 COMPACT-QUEUE: input arriving while a compaction runs is queued, never
//! cancels it, and opens the next turn in arrival order once the summary
//! commits; a failed or interrupted compaction rolls back to the
//! pre-compaction transcript and leaves the queue intact. Driven through the
//! real [`Session`] actor (`UserInput` / `CompactNow` / `CancelTurn` /
//! `TurnDone`) against the fake provider of [`super::vision_fallback_tests`].
//! One tokio test: the tool registry is a process-global actor bound to the
//! runtime that spawned it.

use std::time::Duration;

use super::vision_fallback_tests::{Fake, Harness};
use super::*;

/// How long the fake holds the summariser: long enough for two inputs to
/// land mid-compaction, short enough for the 10 s event timeout.
const HOLD: Duration = Duration::from_secs(2);

const JOB: &str = "<job_result id=\"j1\" command=\"make\" exit=\"0\" secs=\"3\">\nok\n</job_result>";
const HUMAN: &str = "And then do this.";

fn user_input(id: Option<InputId>, text: &str) -> UserInput {
    UserInput {
        id,
        text: text.into(),
        files: Vec::new(),
        ticket: None,
        scope: None,
        defer: false,
    }
}

fn turn_texts(events: &[AgentEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::TurnStarted { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

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

/// A session actor with two settled turns of history (something to fold).
async fn seeded(fake: Fake) -> (Harness, ActorRef<Session>) {
    let mut h = Harness::new(fake).await;
    let actor = h.actor("openai/textonly").await;
    for text in ["First question.", "Second question."] {
        actor.tell(user_input(None, text)).await.unwrap();
        h.drain().await;
    }
    (h, actor)
}

/// Start a manual compaction off the mailbox and return once `Compacting` fired.
async fn compacting(
    h: &mut Harness,
    actor: &ActorRef<Session>,
) -> tokio::task::JoinHandle<Result<u32, String>> {
    let a = actor.clone();
    let task = tokio::spawn(async move {
        a.ask(CompactNow {
            style: Some(CompactStyle::Summary),
            head: None,
        })
        .await
        .map_err(|e| e.to_string())
    });
    h.drain_until(|e| matches!(e, AgentEvent::Compacting { .. }))
        .await;
    assert!(actor.ask(IsRunning).await.unwrap(), "busy while compacting");
    task
}

/// A job result and an identified human message land mid-compaction.
async fn two_inputs(actor: &ActorRef<Session>) -> InputId {
    actor.tell(user_input(None, JOB)).await.unwrap();
    let human = InputId::new();
    actor.tell(user_input(Some(human), HUMAN)).await.unwrap();
    human
}

/// Both inputs opened exactly one turn, in arrival order, delivered once.
fn assert_one_turn_in_order(events: &[AgentEvent], human: InputId) {
    assert_eq!(
        count(events, |e| matches!(e, AgentEvent::InputQueued { id, .. } if *id == human)),
        1,
        "the human message shows as queued while the compaction runs"
    );
    assert_eq!(
        count(events, |e| matches!(e, AgentEvent::InputInjected { id, .. } if *id == human)),
        1,
        "delivered exactly once"
    );
    let texts = turn_texts(events);
    assert_eq!(texts.len(), 1, "one turn for both inputs: {texts:?}");
    assert_eq!(texts[0], format!("{JOB}\n{HUMAN}"));
}

#[test]
fn input_during_compaction_queues_and_never_cancels_it() {
    crate::with_test_registry(scenarios());
}

async fn scenarios() {
    let reg_dir = std::env::temp_dir().join(format!("theuth-compact-queue-reg-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&reg_dir).unwrap();
    let reg_rt = Runtime::new(figment::Figment::new(), reg_dir.clone());
    tools::registry::start(&reg_rt).await.unwrap();

    manual_compaction_commits_then_the_queue_opens_a_turn().await;
    auto_compaction_mid_turn_commits_and_the_queue_follows().await;
    failed_compaction_rolls_back_and_keeps_the_queue().await;
    interrupted_compaction_rolls_back_and_keeps_the_queue().await;

    let _ = std::fs::remove_dir_all(&reg_dir);
}

/// `/compact` mini-turn: the inputs wait, the epoch commits, one turn follows.
async fn manual_compaction_commits_then_the_queue_opens_a_turn() {
    let (mut h, actor) = seeded(Fake::compact_held(HOLD)).await;
    let task = compacting(&mut h, &actor).await;
    let human = two_inputs(&actor).await;

    let events = h
        .drain_until(|e| matches!(e, AgentEvent::TurnFinished { .. }))
        .await;
    assert_eq!(task.await.unwrap(), Ok(1), "compaction committed epoch 1");
    assert_eq!(count(&events, |e| matches!(e, AgentEvent::Compacted { .. })), 1);
    assert_eq!(count(&events, |e| matches!(e, AgentEvent::Error { .. })), 0);
    assert_one_turn_in_order(&events, human);
    assert!(actor.ask(QueuedInputs).await.unwrap().is_empty());

    let (epoch, head, history) = h.store.frame(h.id).await.unwrap();
    assert_eq!(epoch, 1);
    assert!(head.is_some());
    let texts: Vec<String> = history.iter().map(|(_, m)| text_of(&m.content)).collect();
    assert!(
        texts.iter().any(|t| t.contains("<job_result")),
        "the job result is in the new epoch's tail: {texts:?}"
    );
}

/// Auto-compaction inside a running turn (07 §8.1): the inputs steer-queue
/// behind the summariser, the turn finishes normally, then they open a turn.
async fn auto_compaction_mid_turn_commits_and_the_queue_follows() {
    let (mut h, actor) = seeded(Fake::compact_held(HOLD)).await;
    // Trigger at the next call: a one-token threshold.
    actor
        .tell(SetCompactionOverride(CompactionOverride {
            at_tokens: Some(1),
            hint: None,
        }))
        .await
        .unwrap();
    actor
        .tell(user_input(None, "Third question, which compacts first."))
        .await
        .unwrap();
    h.drain_until(|e| matches!(e, AgentEvent::Compacting { .. }))
        .await;
    // Back to the configured threshold so the queued turn does not compact again.
    actor
        .tell(SetCompactionOverride(CompactionOverride::default()))
        .await
        .unwrap();
    let human = two_inputs(&actor).await;

    // `InputQueued` fires during the compacting turn; the injection and the
    // turn the inputs open come after it, so both drains are read together.
    let mut events = h.drain().await;
    assert_eq!(count(&events, |e| matches!(e, AgentEvent::Compacted { .. })), 1);
    assert_eq!(count(&events, |e| matches!(e, AgentEvent::Error { .. })), 0);
    assert_eq!(stop_reasons(&events), ["stop"], "the compacting turn finishes normally");
    // No tool round in this turn, so the inputs open the next one.
    events.extend(h.drain().await);
    assert_eq!(stop_reasons(&events), ["stop", "stop"]);
    let events: Vec<AgentEvent> = events
        .into_iter()
        .filter(|e| !matches!(e, AgentEvent::TurnStarted { text, .. } if text.starts_with("Third")))
        .collect();
    assert_one_turn_in_order(&events, human);
    assert_eq!(h.store.frame(h.id).await.unwrap().0, 1);
}

/// A summariser refusal: `Err`, nothing persisted, epoch unchanged, and the
/// queued inputs open their turn as if the compaction never ran.
async fn failed_compaction_rolls_back_and_keeps_the_queue() {
    let (mut h, actor) = seeded(Fake::compact_refused()).await;
    let before = h.store.frame(h.id).await.unwrap();
    let task = compacting(&mut h, &actor).await;
    let human = two_inputs(&actor).await;

    let events = h
        .drain_until(|e| matches!(e, AgentEvent::TurnFinished { .. }))
        .await;
    let err = task.await.unwrap().unwrap_err();
    assert!(err.starts_with("summariser:"), "{err}");
    assert_eq!(count(&events, |e| matches!(e, AgentEvent::Compacted { .. })), 0);
    assert_one_turn_in_order(&events, human);

    let after = h.store.frame(h.id).await.unwrap();
    assert_eq!(after.0, 0, "no epoch row");
    assert!(after.1.is_none(), "no head");
    let seqs = |f: &(u32, Option<ChatMessage>, Vec<(i64, ChatMessage)>)| {
        f.2.iter().map(|(s, _)| *s).collect::<Vec<_>>()
    };
    assert_eq!(
        &seqs(&after)[..before.2.len()],
        &seqs(&before)[..],
        "the pre-compaction transcript is intact; the queued turn appends after it"
    );
}

/// An explicit interrupt during compaction: the summariser call is cancelled,
/// nothing is persisted, and the queued inputs still open their turn.
async fn interrupted_compaction_rolls_back_and_keeps_the_queue() {
    let (mut h, actor) = seeded(Fake::compact_held(HOLD)).await;
    let task = compacting(&mut h, &actor).await;
    let human = two_inputs(&actor).await;
    actor.tell(CancelTurn).await.unwrap();

    let events = h
        .drain_until(|e| matches!(e, AgentEvent::TurnFinished { .. }))
        .await;
    let err = task.await.unwrap().unwrap_err();
    assert!(err.contains("cancelled"), "{err}");
    assert_eq!(count(&events, |e| matches!(e, AgentEvent::Compacted { .. })), 0);
    assert_one_turn_in_order(&events, human);
    assert_eq!(stop_reasons(&events), ["stop"], "the cancel hit the compaction, not the queued turn");

    let (epoch, head, _) = h.store.frame(h.id).await.unwrap();
    assert_eq!(epoch, 0);
    assert!(head.is_none());
}
