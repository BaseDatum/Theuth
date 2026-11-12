//! Owner feed parity against the independently captured legacy wire fixtures.

use runtime::Runtime;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::{sync::Arc, time::Duration};
use tokio::sync::broadcast;
use wire::{EventPolicy, RuntimeWire as _, WireFrame};

fn load(name: &str) -> Vec<Value> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/wire")
        .join(format!("{name}.json"));
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

async fn next(rx: &mut broadcast::Receiver<Arc<WireFrame>>) -> Arc<WireFrame> {
    tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .expect("wire frame within 10s")
        .expect("wire feed open")
}

fn check(frame: &WireFrame, expected: &Value, topic: Option<&str>) {
    assert_eq!(&frame.event, expected);
    assert_eq!(frame.descriptor.as_ref().map(|d| d.topic), topic);
    assert_eq!(
        frame.descriptor.as_ref().map(|d| d.version),
        topic.map(|_| 1)
    );
    assert_eq!(
        frame.policy,
        EventPolicy {
            session: expected
                .get("session")
                .and_then(Value::as_str)
                .map(str::to_owned),
            ..EventPolicy::default()
        }
    );
}

async fn family<E>(rt: &Runtime, rx: &mut broadcast::Receiver<Arc<WireFrame>>, topic: &str)
where
    E: DeserializeOwned + Clone + Send + 'static,
{
    for (i, sample) in load(topic).into_iter().enumerate() {
        let input = &sample["input"];
        let event: E = serde_json::from_value(input.clone())
            .unwrap_or_else(|e| panic!("{topic}[{i}]: {e}; input={input}"));
        rt.publish(event).await;
        check(
            next(rx).await.as_ref(),
            &sample["notification"]["params"]["result"],
            Some(topic),
        );
    }
}

#[tokio::test]
async fn simple_owner_contracts_match_golden() {
    let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
    a2a::declare_wire(&rt).await.unwrap();
    knowledge::declare_wire(&rt).await.unwrap();
    document::declare_wire(&rt).await.unwrap();
    link::declare_wire(&rt).await.unwrap();
    nodes::declare_wire(&rt).await.unwrap();
    schedule::declare_wire(&rt).await.unwrap();
    web::declare_wire(&rt).await.unwrap();
    update::declare_wire(&rt).await.unwrap();
    mesh::declare_wire(&rt).await.unwrap();
    let mut rx = rt.wire_events();
    family::<a2a::A2aChanged>(&rt, &mut rx, "a2a.changed").await;
    family::<a2a::contacts::ContactEvent>(&rt, &mut rx, "a2a.contacts").await;
    family::<knowledge::KnowledgeChanged>(&rt, &mut rx, "knowledge.changed").await;
    family::<document::DocumentChanged>(&rt, &mut rx, "document.changed").await;
    family::<document::CellLatestChanged>(&rt, &mut rx, "document.cell-latest").await;
    family::<document::SourceEvent>(&rt, &mut rx, "document.sources").await;
    family::<link::LinkEvent>(&rt, &mut rx, "link.events").await;
    family::<nodes::NodeEvent>(&rt, &mut rx, "nodes.events").await;
    family::<schedule::ScheduleEvent>(&rt, &mut rx, "schedule.events").await;
    family::<web::progress::ScrapeProgress>(&rt, &mut rx, "web.scrape-progress").await;
    family::<update::UpdateReady>(&rt, &mut rx, "update.ready").await;
    family::<mesh::MeshEvent>(&rt, &mut rx, "mesh.events").await;
}
