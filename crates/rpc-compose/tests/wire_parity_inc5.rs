//! Increment 5: request/resolution scopes and JSON stay identical to the old feed.

use runtime::Runtime;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::{sync::Arc, time::Duration};
use tokio::sync::broadcast;
use wire::{EventPolicy, RuntimeWire as _, WireEvent, WireFrame};

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
        // Requests do not lend their session to unscoped resolution events.
        let field = |key: &str| golden[key].as_str().map(str::to_owned);
        assert_eq!(
            frame.policy,
            EventPolicy {
                session: field("session"),
                parent: field("parent"),
                child: field("child"),
                ..EventPolicy::default()
            },
            "{topic}[{i}]"
        );
    }
}

#[tokio::test]
async fn feature_golden_parity() {
    let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
    artifact::declare_wire(&rt).await.unwrap();
    app::declare_wire(&rt).await.unwrap();
    secrets::declare_wire(&rt).await.unwrap();
    browser::declare_wire(&rt).await.unwrap();
    plugins::declare_wire(&rt).await.unwrap();
    let mut feed = rt.wire_events();
    family::<artifact::ArtifactShown>(&rt, &mut feed, "artifact.shown").await;
    family::<artifact::WebViewOpened>(&rt, &mut feed, "artifact.web-view-opened").await;
    family::<artifact::PtyViewOpened>(&rt, &mut feed, "artifact.pty-view-opened").await;
    family::<artifact::NoteViewOpened>(&rt, &mut feed, "artifact.note-view-opened").await;
    family::<artifact::FileViewOpened>(&rt, &mut feed, "artifact.file-view-opened").await;
    family::<artifact::DiffViewOpened>(&rt, &mut feed, "artifact.diff-view-opened").await;
    family::<artifact::preview::PagePreviewRequested>(
        &rt,
        &mut feed,
        "artifact.page-preview-requested",
    )
    .await;
    family::<artifact::preview::PagePreviewResolved>(
        &rt,
        &mut feed,
        "artifact.page-preview-resolved",
    )
    .await;
    family::<artifact::WebViewCallRequested>(&rt, &mut feed, "artifact.web-view-call-requested")
        .await;
    family::<artifact::WebViewCallResolved>(&rt, &mut feed, "artifact.web-view-call-resolved")
        .await;
    family::<artifact::snapshot::FenceSnapshotRequested>(
        &rt,
        &mut feed,
        "artifact.fence-snapshot-requested",
    )
    .await;
    family::<artifact::snapshot::FenceSnapshotResolved>(
        &rt,
        &mut feed,
        "artifact.fence-snapshot-resolved",
    )
    .await;
    family::<app::AppCallChanged>(&rt, &mut feed, "app.calls").await;
    family::<secrets::SecretsEvent>(&rt, &mut feed, "secrets.events").await;
    family::<browser::BrowserEvent>(&rt, &mut feed, "browser.events").await;
    family::<browser::show_element::ElementFormRequested>(
        &rt,
        &mut feed,
        "browser.element-form-requested",
    )
    .await;
    family::<browser::show_element::ElementFormResolved>(
        &rt,
        &mut feed,
        "browser.element-form-resolved",
    )
    .await;
    // 216: the plugins topics keep their JSON and their top-level session scope.
    family::<plugins::PluginEvent>(&rt, &mut feed, "plugins.events").await;
    family::<plugins::PluginUi>(&rt, &mut feed, "plugins.ui").await;
}
