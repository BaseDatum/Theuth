//! 172 CHANNEL-ORIGIN: the origin a channel puts on its input rides beside the
//! text — stored as the row's `messages.channel`, echoed on `TurnStarted` and
//! `InputQueued`, kept when the turn is regenerated — and never reaches the
//! model. Driven through the real [`Session`] actor against the fake provider
//! of [`super::vision_fallback_tests`]. One tokio test: the tool registry is a
//! process-global actor bound to the runtime that spawned it.

use std::collections::BTreeMap;
use std::time::Duration;

use super::vision_fallback_tests::{Fake, Harness, texts};
use super::*;
use crate::{ChannelChat, ChannelOrigin, ChannelVia};

/// Long enough for an input to land mid-compaction (see `compact_queue_tests`).
const HOLD: Duration = Duration::from_secs(2);
const HUMAN: &str = "[slack dm]\nhi there";
const STEER: &str = "and this";

fn origin() -> ChannelOrigin {
    ChannelOrigin {
        via: ChannelVia::Telegram,
        chat: ChannelChat::Group,
        from: Some("@ops".into()),
        title: Some("Ops".into()),
    }
}

fn input(id: Option<InputId>, text: &str, channel: Option<ChannelOrigin>) -> UserInput {
    UserInput {
        id,
        text: text.into(),
        files: Vec::new(),
        channel,
        ticket: None,
        scope: None,
        defer: false,
    }
}

/// `(turn_seq, text, channel)` of every `TurnStarted`.
fn started(events: &[AgentEvent]) -> Vec<(i64, String, Option<ChannelOrigin>)> {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::TurnStarted {
                turn_seq,
                text,
                channel,
                ..
            } => Some((*turn_seq, text.clone(), channel.clone())),
            _ => None,
        })
        .collect()
}

#[test]
fn channel_origin_rides_beside_the_text() {
    crate::with_test_registry(scenario());
}

async fn scenario() {
    let scratch = crate::test_dir("channel-origin-reg");
    let reg_rt = Runtime::new(figment::Figment::new(), scratch.path().to_path_buf());
    tools::registry::start(&reg_rt).await.unwrap();

    let mut h = Harness::new(Fake::compact_held(HOLD)).await;
    let actor = h.actor("openai/textonly").await;

    // Send: the row stores the origin, `TurnStarted` echoes it.
    actor
        .tell(input(None, HUMAN, Some(origin())))
        .await
        .unwrap();
    let events = h.drain().await;
    h.drain_until(
        |e| matches!(e, AgentEvent::ContextChanged { reason, .. } if reason == "settled"),
    )
    .await;
    let [(turn, text, channel)] = started(&events).try_into().expect("one turn");
    assert_eq!((text.as_str(), channel), (HUMAN, Some(origin())));
    assert_eq!(
        h.store.channels(h.id).await.unwrap(),
        BTreeMap::from([(turn, origin())])
    );
    // The seeded title skips Slack's marker line (the stored text keeps it).
    let title = h.store.session(h.id).await.unwrap().unwrap().title;
    assert_eq!(title, "hi there");
    // The model saw the text alone: no origin in any request.
    let sent: Vec<String> = h.requests().await.iter().flat_map(texts).collect();
    assert!(sent.iter().all(|t| !t.contains("@ops")), "{sent:?}");

    // Regenerate replays the stored opener and keeps its origin.
    actor
        .ask(RestartTurn {
            turn,
            input: None,
            ticket: None,
        })
        .await
        .unwrap();
    let events = h.drain().await;
    let [(again, _, channel)] = started(&events).try_into().expect("one turn");
    assert_eq!(channel, Some(origin()));
    assert_eq!(
        h.store.channels(h.id).await.unwrap().get(&again),
        Some(&origin())
    );
    h.drain_until(
        |e| matches!(e, AgentEvent::ContextChanged { reason, .. } if reason == "settled"),
    )
    .await;

    // A desktop input stores none.
    actor
        .tell(input(None, "from the desktop", None))
        .await
        .unwrap();
    let events = h.drain().await;
    let [(desk, _, channel)] = started(&events).try_into().expect("one turn");
    assert_eq!(channel, None);
    assert!(!h.store.channels(h.id).await.unwrap().contains_key(&desk));
    h.drain_until(
        |e| matches!(e, AgentEvent::ContextChanged { reason, .. } if reason == "settled"),
    )
    .await;

    // A queued input (here behind a running compaction) carries its origin on
    // `InputQueued`, then opens the next turn with it.
    let a = actor.clone();
    let compaction = tokio::spawn(async move {
        a.ask(CompactNow {
            style: Some(CompactStyle::Summary),
            head: None,
        })
        .await
        .map_err(|e| e.to_string())
    });
    h.drain_until(|e| matches!(e, AgentEvent::Compacting { .. }))
        .await;
    let steer = InputId::new();
    actor
        .tell(input(Some(steer), STEER, Some(origin())))
        .await
        .unwrap();
    // A UI reloading the queue (`ListQueued`) still sees the origin.
    let listed = actor.ask(QueuedInputs).await.unwrap();
    assert_eq!(
        listed
            .iter()
            .map(|q| (q.id, q.channel.clone()))
            .collect::<Vec<_>>(),
        [(steer, Some(origin()))]
    );
    let events = h
        .drain_until(|e| matches!(e, AgentEvent::TurnFinished { .. }))
        .await;
    compaction.await.unwrap().unwrap();
    let queued: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::InputQueued { id, channel, .. } if *id == steer => Some(channel.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(queued, [Some(origin())]);
    let [(seq, text, channel)] = started(&events).try_into().expect("one turn");
    assert_eq!((text.as_str(), channel), (STEER, Some(origin())));
    assert_eq!(
        h.store.channels(h.id).await.unwrap().get(&seq),
        Some(&origin())
    );
}
