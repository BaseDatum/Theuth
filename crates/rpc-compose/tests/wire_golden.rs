//! Legacy wire contract captured at c0af0bc8 before the bridge cutover.
//! Fixtures contain independent publication inputs and captured JSON-RPC notifications.
//! Subscription IDs are server-assigned: checked against the response, then normalized.
//! Owner dependencies belong in dev-dependencies when step 13 removes production edges.

use runtime::Runtime;
use serde::de::DeserializeOwned;
use serde_json::{Value, value::RawValue};
use std::time::Duration;
use tokio::sync::mpsc;
use wire::{RuntimeWire as _, WireEvent, WireFrame};

struct Stream {
    id: Value,
    rx: mpsc::Receiver<Box<RawValue>>,
}

impl Stream {
    async fn open(methods: &jsonrpsee::Methods) -> Self {
        let (response, rx) = methods
            .raw_json_request(
                r#"{"jsonrpc":"2.0","id":1,"method":"subscribe_events","params":[null,null]}"#,
                16,
            )
            .await
            .unwrap();
        let response: Value = serde_json::from_str(response.get()).unwrap();
        assert!(response.get("error").is_none(), "{response}");
        Self {
            id: response["result"].clone(),
            rx,
        }
    }

    async fn next(&mut self) -> Value {
        let raw = tokio::time::timeout(Duration::from_secs(10), self.rx.recv())
            .await
            .expect("wire notification within 10s")
            .expect("stream open");
        let mut value: Value = serde_json::from_str(raw.get()).unwrap();
        assert_eq!(value["params"]["subscription"], self.id);
        value["params"]["subscription"] = Value::String("<subscription>".into());
        value
    }
}

async fn methods(rt: Runtime) -> (jsonrpsee::Methods, rpc_core::RpcPeerGuard) {
    let mut methods = rpc_compose::methods(rt, |server| rpc_compose::rpc_module(server).module).await;
    let (peer, guard) = rpc_core::RpcPeer::open();
    methods.extensions_mut().insert(peer);
    (methods, guard)
}

fn load(name: &str) -> Vec<Value> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/wire")
        .join(format!("{name}.json"));
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

async fn family<E>(rt: &Runtime, stream: &mut Stream, name: &str)
where
    E: DeserializeOwned + WireEvent,
{
    rt.register_wire_event::<E>().await.unwrap();
    for (i, sample) in load(name).into_iter().enumerate() {
        let input = &sample["input"];
        let event: E = serde_json::from_value(input.clone())
            .unwrap_or_else(|e| panic!("{name}[{i}]: {e}; input={input}"));
        rt.publish(event).await;
        assert_eq!(stream.next().await, sample["notification"], "{name}[{i}]");
    }
}

fn golden(name: &str, output: &[Value]) {
    assert_eq!(output, load(name), "{name}");
}

#[tokio::test]
async fn legacy_event_families() {
    let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
    tools::declare_wire(&rt).await.unwrap();
    sessionsync::declare_wire(&rt).await.unwrap();
    let events = rt.wire_feed();
    let (methods, _guard) = methods(rt.clone()).await;
    let mut stream = Stream::open(&methods).await;
    golden("rpc.stream.initial", &[stream.next().await]);
    family::<agent::AgentEvent>(&rt, &mut stream, "agent.events").await;
    family::<agent::LoginFinished>(&rt, &mut stream, "agent.login").await;
    family::<agent::CommunicationChanged>(&rt, &mut stream, "agent.communication").await;
    family::<agent::PeerMessageChanged>(&rt, &mut stream, "agent.peer-message").await;
    family::<agent::PeerDelivered>(&rt, &mut stream, "agent.peer-delivered").await;
    family::<agent::AgentGraphChanged>(&rt, &mut stream, "agent.graph").await;
    family::<a2a::A2aChanged>(&rt, &mut stream, "a2a.changed").await;
    family::<a2a::contacts::ContactEvent>(&rt, &mut stream, "a2a.contacts").await;
    family::<tools::ApprovalRequested>(&rt, &mut stream, "tools.approval-requested").await;
    family::<tools::ApprovalResolved>(&rt, &mut stream, "tools.approval-resolved").await;
    family::<tools::AskRequested>(&rt, &mut stream, "tools.ask-requested").await;
    family::<tools::AskResolved>(&rt, &mut stream, "tools.ask-resolved").await;
    family::<tools::McpProgress>(&rt, &mut stream, "tools.mcp-progress").await;
    family::<tools::JobEvent>(&rt, &mut stream, "tools.jobs").await;
    family::<signal::SignalEvent>(&rt, &mut stream, "signal.events").await;
    family::<slack::SlackEvent>(&rt, &mut stream, "slack.events").await;
    family::<telegram::TelegramEvent>(&rt, &mut stream, "telegram.events").await;
    family::<knowledge::KnowledgeChanged>(&rt, &mut stream, "knowledge.changed").await;
    family::<document::DocumentChanged>(&rt, &mut stream, "document.changed").await;
    family::<document::CellLatestChanged>(&rt, &mut stream, "document.cell-latest").await;
    family::<document::SourceEvent>(&rt, &mut stream, "document.sources").await;
    family::<agent::ask::NotebookAsk>(&rt, &mut stream, "agent.notebook-ask").await;
    family::<browser::BrowserEvent>(&rt, &mut stream, "browser.events").await;
    family::<browser::show_element::ElementFormRequested>(
        &rt,
        &mut stream,
        "browser.element-form-requested",
    )
    .await;
    family::<browser::show_element::ElementFormResolved>(
        &rt,
        &mut stream,
        "browser.element-form-resolved",
    )
    .await;
    family::<link::LinkEvent>(&rt, &mut stream, "link.events").await;
    family::<nodes::NodeEvent>(&rt, &mut stream, "nodes.events").await;
    family::<schedule::ScheduleEvent>(&rt, &mut stream, "schedule.events").await;
    family::<artifact::ArtifactShown>(&rt, &mut stream, "artifact.shown").await;
    family::<artifact::WebViewOpened>(&rt, &mut stream, "artifact.web-view-opened").await;
    family::<artifact::PtyViewOpened>(&rt, &mut stream, "artifact.pty-view-opened").await;
    family::<artifact::NoteViewOpened>(&rt, &mut stream, "artifact.note-view-opened").await;
    family::<artifact::WhiteboardViewOpened>(&rt, &mut stream, "artifact.whiteboard-view-opened")
        .await;
    family::<artifact::FileViewOpened>(&rt, &mut stream, "artifact.file-view-opened").await;
    family::<artifact::DiffViewOpened>(&rt, &mut stream, "artifact.diff-view-opened").await;
    family::<artifact::preview::PagePreviewRequested>(
        &rt,
        &mut stream,
        "artifact.page-preview-requested",
    )
    .await;
    family::<artifact::preview::PagePreviewResolved>(
        &rt,
        &mut stream,
        "artifact.page-preview-resolved",
    )
    .await;
    family::<artifact::WebViewCallRequested>(
        &rt,
        &mut stream,
        "artifact.web-view-call-requested",
    )
    .await;
    family::<artifact::WebViewCallResolved>(&rt, &mut stream, "artifact.web-view-call-resolved")
        .await;
    family::<artifact::snapshot::FenceSnapshotRequested>(
        &rt,
        &mut stream,
        "artifact.fence-snapshot-requested",
    )
    .await;
    family::<artifact::snapshot::FenceSnapshotResolved>(
        &rt,
        &mut stream,
        "artifact.fence-snapshot-resolved",
    )
    .await;
    family::<secrets::SecretsEvent>(&rt, &mut stream, "secrets.events").await;
    family::<web::progress::ScrapeProgress>(&rt, &mut stream, "web.scrape-progress").await;
    family::<update::UpdateReady>(&rt, &mut stream, "update.ready").await;
    family::<voice::VoiceEvent>(&rt, &mut stream, "voice.events").await;
    family::<talk::TalkEvent>(&rt, &mut stream, "talk.events").await;
    family::<meeting::MeetingEvent>(&rt, &mut stream, "meeting.events").await;
    family::<meeting::BriefEvent>(&rt, &mut stream, "meeting.briefs").await;
    family::<meeting::CalendarEvent>(&rt, &mut stream, "meeting.calendar").await;
    family::<app::AppCallChanged>(&rt, &mut stream, "app.calls").await;
    family::<tasks::activity::ActivityAppended>(&rt, &mut stream, "tasks.activity").await;
    family::<tasks::TaskChanged>(&rt, &mut stream, "tasks.changed").await;
    family::<codeindex::CodeindexEvent>(&rt, &mut stream, "codeindex.events").await;
    family::<mesh::MeshEvent>(&rt, &mut stream, "mesh.events").await;
    family::<sandbox::SandboxEvent>(&rt, &mut stream, "sandbox.events").await;
    family::<double::DoublePredicted>(&rt, &mut stream, "double.predicted").await;
    family::<double::DoubleAnswered>(&rt, &mut stream, "double.answered").await;
    family::<double::DoubleTrainingState>(&rt, &mut stream, "double.training").await;
    family::<double::DoubleVersionActivated>(&rt, &mut stream, "double.version").await;
    family::<roster::attention::AttentionChanged>(&rt, &mut stream, "roster.attention").await;
    family::<webprofile::WebProfileChanged>(&rt, &mut stream, "browser.web-profile-changed").await;
    family::<tools::ReminderEvent>(&rt, &mut stream, "tools.reminders").await;
    family::<dreaming::DreamNightStarted>(&rt, &mut stream, "dreaming.started").await;
    family::<dreaming::DreamNightFinished>(&rt, &mut stream, "dreaming.finished").await;
    family::<dreaming::DreamUndone>(&rt, &mut stream, "dreaming.undone").await;
    family::<autoresearch::StudyEvent>(&rt, &mut stream, "autoresearch.events").await;
    family::<cells::CellEvent>(&rt, &mut stream, "cells.events").await;
    family::<share::ShareChanged>(&rt, &mut stream, "share.changed").await;
    family::<share::ShareStatus>(&rt, &mut stream, "share.status").await;
    family::<share::ShareTraffic>(&rt, &mut stream, "share.traffic").await;
    family::<agent::EndlessProgress>(&rt, &mut stream, "agent.endless-progress").await;
    family::<agent::CostCubeChanged>(&rt, &mut stream, "agent.cost-cube").await;
    // 216: external plugins' events and their placement/lifecycle frames.
    family::<plugins::PluginEvent>(&rt, &mut stream, "plugins.events").await;
    family::<plugins::PluginUi>(&rt, &mut stream, "plugins.ui").await;
    relay(&rt, &mut stream).await;
    drop(stream);
    lagged_then_pending_snapshot(&methods, &events).await;
    let mut live = Stream::open(&methods).await;
    golden("rpc.stream.initial", &[live.next().await]);
    pending_snapshot(&rt, &methods, &mut live).await;
}

async fn relay(rt: &Runtime, stream: &mut Stream) {
    let session = serde_json::from_str("\"00000000-0000-0000-0000-000000000002\"").unwrap();
    let mut output = Vec::new();
    for sample in load("sessionsync.relay") {
        let input = &sample["input"];
        rt.publish(sessionsync::events::Relayed {
            session,
            kind: input["kind"].as_str().unwrap().into(),
            event: input["event"].clone(),
        })
        .await;
        output.push(serde_json::json!({"input": input, "notification": stream.next().await}));
    }
    golden("sessionsync.relay", &output);
    // The kernel state is shared by typed and relayed ingress in both directions.
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
    let mut observed = Vec::new();
    for local_start in [true, false] {
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
        }
        // A bus-ordered sentinel proves that no suppressed job leaked before it.
        rt.publish(a2a::A2aChanged {}).await;
        observed.push(stream.next().await);
        // Finished removes the key: reuse without a new Started is visible.
        rt.publish(serde_json::from_value::<tools::JobEvent>(job("JobOutput")).unwrap())
            .await;
        observed.push(stream.next().await);
    }
    rt.publish(sessionsync::events::Relayed {
        session,
        kind: "JobEvent".into(),
        event: serde_json::json!({"type": "not-a-job"}),
    })
    .await;
    rt.publish(a2a::A2aChanged {}).await;
    observed.push(stream.next().await);
    golden("sessionsync.kernel", &observed);
}

/// Synchronous sends on this current-thread runtime overrun the real 1024-slot
/// wire ring by exactly two frames, without timing a stalled network client.
async fn lagged_then_pending_snapshot(
    methods: &jsonrpsee::Methods,
    events: &tokio::sync::broadcast::Sender<std::sync::Arc<WireFrame>>,
) {
    let mut stream = Stream::open(methods).await;
    let initial = stream.next().await;
    tokio::task::yield_now().await;
    for _ in 0..1026 {
        let event = serde_json::json!({ "type": "TitleChanged", "title": "2" });
        events
            .send(std::sync::Arc::new(WireFrame {
                policy: wire::EventPolicy::legacy_fields(&event),
                descriptor: None,
                event,
            }))
            .unwrap();
    }
    let lagged = stream.next().await;
    let snapshot = stream.next().await;
    let last = stream.next().await;
    golden("rpc.stream.loss", &[initial, lagged, snapshot, last]);
}

/// These streams bypass the bus. There is no public event-injection API for
/// document rooms or PTY/cell output; use their actual to_raw_value conversion
/// for every variant rather than add production injection hooks. Both PTY
/// subscriptions use the same PtyChunk contract.
#[test]
fn bypass_item_json() {
    fn items<E: DeserializeOwned + serde::Serialize>(name: &str) {
        let output: Vec<_> = load(name)
            .into_iter()
            .map(|sample| {
                let input = &sample["input"];
                let event: E = serde_json::from_value(input.clone()).unwrap();
                let raw = serde_json::value::to_raw_value(&event).unwrap();
                let item: Value = serde_json::from_str(raw.get()).unwrap();
                serde_json::json!({"input": input, "item": item})
            })
            .collect();
        golden(name, &output);
    }
    items::<tools::pty::PtyChunk>("bypass.pty");
    items::<document::DocEvent>("bypass.document");
}

/// Real pending state, not just a serialized empty snapshot. Only the two
/// actor-generated prompt IDs are normalized; the secret request ID is fixed.
async fn pending_snapshot(rt: &Runtime, methods: &jsonrpsee::Methods, live: &mut Stream) {
    use kameo::actor::Spawn;
    let session = serde_json::from_str("\"00000000-0000-0000-0000-000000000001\"").unwrap();
    let approval =
        tools::Approval::spawn(tools::Approval::new(rt.clone(), Duration::from_secs(300)));
    approval.register(tools::APPROVAL_ACTOR).unwrap();
    let ask = tools::AskUser::spawn(tools::AskUser::new(rt.clone(), Duration::from_secs(300)));
    ask.register(tools::ASK_ACTOR).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let vault = secrets::Vault::open(dir.path(), secrets::KeyStore::File)
        .await
        .unwrap();
    let secret = secrets::SecretsActor::spawn(secrets::SecretsActor::new(
        std::sync::Arc::new(vault),
        rt.clone(),
        Duration::from_secs(300),
    ));
    secret.register(secrets::SECRETS_ACTOR).unwrap();
    let approved = tokio::spawn({
        let actor = approval.clone();
        async move {
            actor
                .ask(tools::approval::RequestApproval {
                    session,
                    tool: "shell".into(),
                    args: serde_json::json!({"command": "rm sample"}),
                    risk: tools::Risk::Destructive,
                    call_id: None,
                    always_ask: false,
                })
                .await
                .unwrap()
        }
    });
    let approval_id = live.next().await["params"]["result"]["id"].clone();
    let asked = tokio::spawn({
        let actor = ask.clone();
        async move {
            actor
                .ask(tools::ask::Request {
                    session,
                    questions: vec![tools::Question {
                        id: "q".into(),
                        question: "Which?".into(),
                        options: vec![],
                        recommended: None,
                    }],
                    call_id: None,
                })
                .await
                .unwrap()
        }
    });
    let ask_id = live.next().await["params"]["result"]["id"].clone();
    let secret_id = serde_json::from_str("\"00000000-0000-0000-0000-000000000003\"").unwrap();
    let requested = tokio::spawn({
        let actor = secret.clone();
        async move {
            actor
                .ask(secrets::Request {
                    session,
                    title: Some("Credentials".into()),
                    fields: vec![secrets::SecretRequestField {
                        name: "sample".into(),
                        label: "Token".into(),
                        masked: true,
                        current: None,
                        note: None,
                    }],
                    id: Some(secret_id),
                    expires: None,
                })
                .await
                .unwrap()
        }
    });
    let secret_event = live.next().await;
    assert_eq!(secret_event["params"]["result"]["type"], "SecretRequested");
    let mut subscriber = Stream::open(methods).await;
    let mut snapshot = subscriber.next().await;
    let pending = &mut snapshot["params"]["result"];
    assert_eq!(pending["approvals"][0]["id"], approval_id);
    assert_eq!(pending["asks"][0]["id"], ask_id);
    pending["approvals"][0]["id"] = serde_json::json!("<approval-id>");
    pending["asks"][0]["id"] = serde_json::json!("<ask-id>");
    golden("rpc.stream.pending", &[snapshot]);
    approval
        .ask(tools::approval::Resolve {
            id: serde_json::from_value(approval_id).unwrap(),
            decision: tools::Decision::Deny,
        })
        .await
        .unwrap();
    ask.ask(tools::ask::AnswerAsk {
        id: serde_json::from_value(ask_id).unwrap(),
        answers: None,
    })
    .await
    .unwrap();
    secret
        .ask(secrets::Answer {
            id: secret_id,
            value: None,
            values: Default::default(),
            expires_ms: None,
        })
        .await
        .unwrap();
    approved.await.unwrap();
    asked.await.unwrap();
    requested.await.unwrap();
}
