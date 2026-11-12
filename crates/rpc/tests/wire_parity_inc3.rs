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
async fn tools_and_relay_contracts_match_golden() {
    let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
    tools::declare_wire(&rt).await.unwrap();
    sessionsync::declare_wire(&rt).await.unwrap();
    a2a::declare_wire(&rt).await.unwrap();
    cells::declare_wire(&rt).await.unwrap();
    let mut rx = rt.wire_events();
    family::<tools::ApprovalRequested>(&rt, &mut rx, "tools.approval-requested").await;
    family::<tools::ApprovalResolved>(&rt, &mut rx, "tools.approval-resolved").await;
    family::<tools::AskRequested>(&rt, &mut rx, "tools.ask-requested").await;
    family::<tools::AskResolved>(&rt, &mut rx, "tools.ask-resolved").await;
    family::<tools::McpProgress>(&rt, &mut rx, "tools.mcp-progress").await;
    family::<tools::JobEvent>(&rt, &mut rx, "tools.jobs").await;
    family::<tools::ReminderEvent>(&rt, &mut rx, "tools.reminders").await;
    // FollowState's typed payload has exactly the old anonymous JSON, in both states.
    for sample in load("sessionsync.relay") {
        if sample["input"]["kind"] == "FollowState" {
            let event = serde_json::from_value::<sessionsync::events::FollowState>(
                sample["input"]["event"].clone(),
            )
            .unwrap();
            rt.publish(event).await;
            check(
                next(&mut rx).await.as_ref(),
                &sample["notification"]["params"]["result"],
                Some("sessionsync.follow-state"),
            );
        }
    }
    let session = serde_json::from_str("\"00000000-0000-0000-0000-000000000002\"").unwrap();
    for sample in load("sessionsync.relay") {
        let input = &sample["input"];
        let topic = match input["kind"].as_str().unwrap() {
            "ApprovalRequested" => Some("tools.approval-requested"),
            "ApprovalResolved" => Some("tools.approval-resolved"),
            "AskRequested" => Some("tools.ask-requested"),
            "AskResolved" => Some("tools.ask-resolved"),
            "JobEvent" => Some("tools.jobs"),
            "CellEvent" => Some("cells.events"),
            "FollowState" => Some("sessionsync.follow-state"),
            "FutureOwnerEvent" => None,
            kind => panic!("unexpected golden relay kind: {kind}"),
        };
        rt.publish(sessionsync::events::Relayed {
            session,
            kind: input["kind"].as_str().unwrap().into(),
            event: input["event"].clone(),
        })
        .await;
        check(
            next(&mut rx).await.as_ref(),
            &sample["notification"]["params"]["result"],
            topic,
        );
    }
    drop(rx);
    kernel_projection(&rt).await;
}

async fn kernel_projection(rt: &Runtime) {
    let session = serde_json::from_str("\"00000000-0000-0000-0000-000000000002\"").unwrap();
    let samples: Vec<_> = load("tools.jobs")
        .into_iter()
        .map(|v| v["input"].clone())
        .collect();
    let job = |tag: &str| {
        let mut value = samples.iter().find(|v| v["type"] == tag).unwrap().clone();
        value["session"] = serde_json::json!(session);
        value["id"] = serde_json::json!("golden-kernel");
        if tag == "JobStarted" {
            value["name"] = serde_json::json!("cell:golden");
        }
        value
    };
    let expected = load("sessionsync.kernel");
    for (i, local_start) in [true, false].into_iter().enumerate() {
        // Re-declaration must not reset suppression state or duplicate delivery.
        let mut rx = rt.wire_events();
        for tag in [
            "JobStarted",
            "JobOutput",
            "JobScreen",
            "JobIdle",
            "JobFinished",
        ] {
            let event = job(tag);
            if (tag == "JobStarted") == local_start {
                rt.publish(serde_json::from_value::<tools::JobEvent>(event).unwrap())
                    .await;
            } else {
                rt.publish(sessionsync::events::Relayed {
                    session,
                    kind: "JobEvent".into(),
                    event,
                })
                .await;
            }
            if tag == "JobStarted" {
                tools::declare_wire(rt).await.unwrap();
                sessionsync::declare_wire(rt).await.unwrap();
            }
        }
        rt.publish(a2a::A2aChanged {}).await;
        check(
            next(&mut rx).await.as_ref(),
            &expected[2 * i]["params"]["result"],
            Some("a2a.changed"),
        );
        // Finished removes the key: output without a new Started is visible again.
        rt.publish(serde_json::from_value::<tools::JobEvent>(job("JobOutput")).unwrap())
            .await;
        check(
            next(&mut rx).await.as_ref(),
            &expected[2 * i + 1]["params"]["result"],
            Some("tools.jobs"),
        );
    }
    let mut rx = rt.wire_events();
    rt.publish(sessionsync::events::Relayed {
        session,
        kind: "JobEvent".into(),
        event: serde_json::json!({"type": "not-a-job"}),
    })
    .await;
    rt.publish(a2a::A2aChanged {}).await;
    check(
        next(&mut rx).await.as_ref(),
        &expected[4]["params"]["result"],
        Some("a2a.changed"),
    );
    drop(rx);
    // Kernel state exists before the first consumer attaches.
    rt.publish(serde_json::from_value::<tools::JobEvent>(job("JobStarted")).unwrap())
        .await;
    tools::declare_wire(rt).await.unwrap();
    let mut rx = rt.wire_events();
    rt.publish(serde_json::from_value::<tools::JobEvent>(job("JobOutput")).unwrap())
        .await;
    rt.publish(a2a::A2aChanged {}).await;
    check(
        next(&mut rx).await.as_ref(),
        &expected[4]["params"]["result"],
        Some("a2a.changed"),
    );
    // A second Runtime must not inherit the first one's suppressed key.
    let other = Runtime::new(figment::Figment::new(), std::env::temp_dir());
    tools::declare_wire(&other).await.unwrap();
    let mut other_rx = other.wire_events();
    other
        .publish(serde_json::from_value::<tools::JobEvent>(job("JobOutput")).unwrap())
        .await;
    check(
        next(&mut other_rx).await.as_ref(),
        &expected[1]["params"]["result"],
        Some("tools.jobs"),
    );
}
