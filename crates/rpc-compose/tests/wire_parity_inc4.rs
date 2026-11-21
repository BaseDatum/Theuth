//! Increment 4: replay every agent golden input through the owner-declared feed.

use runtime::Runtime;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::{sync::Arc, time::Duration};
use tokio::sync::broadcast;
use wire::{Delta, EventPolicy, RuntimeWire as _, WireEvent, WireFrame};

async fn family<E: WireEvent + DeserializeOwned>(
    rt: &Runtime,
    feed: &mut broadcast::Receiver<Arc<WireFrame>>,
    topic: &str,
) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/wire")
        .join(format!("{topic}.json"));
    let samples: Vec<Value> = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    for (i, sample) in samples.iter().enumerate() {
        let event: E = serde_json::from_value(sample["input"].clone()).unwrap();
        rt.publish(event).await;
        let frame = tokio::time::timeout(Duration::from_secs(10), feed.recv())
            .await
            .expect("wire frame within 10s")
            .expect("wire feed open");
        let golden = &sample["notification"]["params"]["result"];
        assert_eq!(&frame.event, golden, "{topic}[{i}]");
        let descriptor = frame.descriptor.as_ref().expect("declared family");
        assert_eq!(descriptor.topic, topic, "{topic}[{i}]");
        assert_eq!(descriptor.version, 1, "{topic}[{i}]");

        // Independent expectations from the golden wire, not E::policy/session().
        let field = |key: &str| golden[key].as_str().map(str::to_owned);
        let mut expected = EventPolicy {
            session: field("session"),
            parent: field("parent"),
            child: field("child"),
            ..EventPolicy::default()
        };
        if topic == "agent.events" {
            match golden["type"].as_str().unwrap() {
                "SubagentSpawned" | "ObservedChildChanged" => {
                    assert!(expected.session.is_none());
                    assert!(expected.parent.is_some() && expected.child.is_some());
                    expected.announces_child = true;
                }
                "DefaultModelChanged" | "FavoriteModelsChanged" | "DefaultEffortChanged" => {
                    assert_eq!(expected, EventPolicy::default());
                }
                "TextDelta" | "ReasoningDelta" => expected.delta = Delta::AppendText,
                "ToolCallProgress" | "VisionProgress" => expected.delta = Delta::Replace,
                _ => {}
            }
        } else if topic == "agent.communication" || topic == "agent.graph" {
            expected.always_wanted = true;
        }
        assert_eq!(frame.policy, expected, "{topic}[{i}]");
    }
}

#[tokio::test]
async fn agent_golden_parity() {
    let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
    agent::declare_wire(&rt).await.unwrap();
    let mut feed = rt.wire_events();
    family::<agent::AgentEvent>(&rt, &mut feed, "agent.events").await;
    family::<agent::LoginFinished>(&rt, &mut feed, "agent.login").await;
    family::<agent::CommunicationChanged>(&rt, &mut feed, "agent.communication").await;
    family::<agent::PeerMessageChanged>(&rt, &mut feed, "agent.peer-message").await;
    family::<agent::PeerDelivered>(&rt, &mut feed, "agent.peer-delivered").await;
    family::<agent::AgentGraphChanged>(&rt, &mut feed, "agent.graph").await;
    family::<agent::ask::NotebookAsk>(&rt, &mut feed, "agent.notebook-ask").await;
    family::<agent::EndlessProgress>(&rt, &mut feed, "agent.endless-progress").await;
    family::<agent::CostCubeChanged>(&rt, &mut feed, "agent.cost-cube").await;
}
