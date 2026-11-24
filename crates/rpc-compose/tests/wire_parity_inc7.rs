//! Increment 7: every remaining owner's golden input traverses the new feed.
//! Nested cell scope and activity subjects retain legacy global admission.

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
        // Only top-level strings scope these control events. In particular,
        // CodeMapProgress is scoped, and nested CellEvent session scope is not.
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
async fn remaining_owners_golden_parity() {
    let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
    tasks::declare_wire(&rt).await.unwrap();
    codeindex::declare_wire(&rt).await.unwrap();
    sandbox::declare_wire(&rt).await.unwrap();
    double::declare_wire(&rt).await.unwrap();
    roster::declare_wire(&rt).await.unwrap();
    dreaming::declare_wire(&rt).await.unwrap();
    autoresearch::declare_wire(&rt).await.unwrap();
    cells::declare_wire(&rt).await.unwrap();
    share::declare_wire(&rt).await.unwrap();
    let mut events = rt.wire_events();
    family::<tasks::activity::ActivityAppended>(&rt, &mut events, "tasks.activity").await;
    family::<tasks::TaskChanged>(&rt, &mut events, "tasks.changed").await;
    family::<codeindex::CodeindexEvent>(&rt, &mut events, "codeindex.events").await;
    family::<sandbox::SandboxEvent>(&rt, &mut events, "sandbox.events").await;
    family::<double::DoublePredicted>(&rt, &mut events, "double.predicted").await;
    family::<double::DoubleAnswered>(&rt, &mut events, "double.answered").await;
    family::<double::DoubleTrainingState>(&rt, &mut events, "double.training").await;
    family::<double::DoubleVersionActivated>(&rt, &mut events, "double.version").await;
    family::<roster::attention::AttentionChanged>(&rt, &mut events, "roster.attention").await;
    family::<dreaming::DreamNightStarted>(&rt, &mut events, "dreaming.started").await;
    family::<dreaming::DreamNightFinished>(&rt, &mut events, "dreaming.finished").await;
    family::<dreaming::DreamUndone>(&rt, &mut events, "dreaming.undone").await;
    family::<autoresearch::StudyEvent>(&rt, &mut events, "autoresearch.events").await;
    family::<cells::CellEvent>(&rt, &mut events, "cells.events").await;
    family::<share::ShareChanged>(&rt, &mut events, "share.changed").await;
    family::<share::ShareStatus>(&rt, &mut events, "share.status").await;
    family::<share::ShareTraffic>(&rt, &mut events, "share.traffic").await;
}
