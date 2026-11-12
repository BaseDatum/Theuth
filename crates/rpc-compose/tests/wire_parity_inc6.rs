//! Increment 6: every channel/media/meeting golden input traverses the new feed.
//! Legacy notifications remain unwrapped; only top-level scope is policy.

use runtime::Runtime;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::{sync::Arc, time::Duration};
use tokio::sync::broadcast;
use wire::{EventPolicy, RuntimeWire as _, WireEvent, WireFrame};

async fn family<E: WireEvent + DeserializeOwned>(
    rt: &Runtime,
    events: &mut broadcast::Receiver<Arc<WireFrame>>,
    topic: &str,
) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/wire")
        .join(format!("{topic}.json"));
    let samples: Vec<Value> = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    for (i, sample) in samples.into_iter().enumerate() {
        let event: E = serde_json::from_value(sample["input"].clone())
            .unwrap_or_else(|error| panic!("{topic}[{i}]: {error}"));
        rt.publish(event).await;
        let frame = tokio::time::timeout(Duration::from_secs(10), events.recv())
            .await
            .expect("wire frame within 10s")
            .expect("wire feed open");
        let expected = &sample["notification"]["params"]["result"];
        assert_eq!(&frame.event, expected, "{topic}[{i}]");
        let descriptor = frame.descriptor.as_ref().expect("declared event");
        assert_eq!(descriptor.topic, topic, "{topic}[{i}]");
        assert_eq!(descriptor.version, 1, "{topic}[{i}]");
        // Speech/audio and meters remain controls, not focus-filtered deltas.
        // Talk/meeting followups and dictation remain global without session.
        assert_eq!(
            frame.policy,
            EventPolicy {
                session: expected.get("session").and_then(Value::as_str).map(str::to_owned),
                parent: expected.get("parent").and_then(Value::as_str).map(str::to_owned),
                child: expected.get("child").and_then(Value::as_str).map(str::to_owned),
                ..EventPolicy::default()
            },
            "{topic}[{i}]"
        );
    }
}

#[tokio::test]
async fn channel_media_meeting_golden_parity() {
    let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
    signal::declare_wire(&rt).await.unwrap();
    slack::declare_wire(&rt).await.unwrap();
    telegram::declare_wire(&rt).await.unwrap();
    voice::declare_wire(&rt).await.unwrap();
    talk::declare_wire(&rt).await.unwrap();
    meeting::declare_wire(&rt).await.unwrap();
    let mut events = rt.wire_events();
    family::<signal::SignalEvent>(&rt, &mut events, "signal.events").await;
    family::<slack::SlackEvent>(&rt, &mut events, "slack.events").await;
    family::<telegram::TelegramEvent>(&rt, &mut events, "telegram.events").await;
    family::<voice::VoiceEvent>(&rt, &mut events, "voice.events").await;
    family::<talk::TalkEvent>(&rt, &mut events, "talk.events").await;
    family::<meeting::MeetingEvent>(&rt, &mut events, "meeting.events").await;
    family::<meeting::BriefEvent>(&rt, &mut events, "meeting.briefs").await;
    family::<meeting::CalendarEvent>(&rt, &mut events, "meeting.calendar").await;
}
