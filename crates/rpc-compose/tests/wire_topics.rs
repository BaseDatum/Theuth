//! Both public ABIs over one owner feed; compiled here, executed at final verification.
use std::{sync::Arc, time::Duration};
use jsonrpsee::{core::server::Subscription, rpc_params};
use rpc_core::{RpcPeer, RpcPeerGuard, Server};
use runtime::{Runtime, SessionId};
use serde_json::{Value, json};
use wire::{RuntimeWire as _, WireEvent};

fn runtime() -> Runtime {
    Runtime::new(Default::default(), std::env::temp_dir())
}

fn connection(base: &jsonrpsee::Methods) -> (jsonrpsee::Methods, RpcPeer, RpcPeerGuard) {
    let (peer, guard) = RpcPeer::open();
    let mut methods = base.clone();
    methods.extensions_mut().insert(peer.clone());
    (methods, peer, guard)
}

async fn next(stream: &mut Subscription) -> Value {
    tokio::time::timeout(Duration::from_secs(10), stream.next::<Value>())
        .await.unwrap().unwrap().unwrap().0
}

async fn topics(methods: &jsonrpsee::Methods, selected: Value, session: Option<SessionId>, descendants: bool) -> Subscription {
    methods.subscribe_unbounded("subscribe_topics", rpc_params![selected, session, descendants]).await.unwrap()
}

fn envelope(topic: &str, event: Value) -> Value {
    json!({"topic": topic, "version": 1, "event": event})
}

fn pending() -> Value {
    envelope("rpc.stream", json!({"type":"PendingPrompts", "approvals":[], "asks":[], "secrets":[]}))
}

fn fixture(topic: &str, tag: &str) -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/wire").join(format!("{topic}.json"));
    let samples: Vec<Value> = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    samples.into_iter().find(|sample| sample["notification"]["params"]["result"]["type"] == tag).unwrap()["input"].clone()
}

async fn publish<E: WireEvent + serde::de::DeserializeOwned>(rt: &Runtime, event: Value) {
    rt.publish(serde_json::from_value::<E>(event).unwrap()).await;
}

#[tokio::test]
async fn filters_envelopes_opaque_old_peer_and_external_liveness() {
    let rt = runtime();
    a2a::declare_wire(&rt).await.unwrap();
    signal::declare_wire(&rt).await.unwrap();
    slack::declare_wire(&rt).await.unwrap();
    sessionsync::declare_wire(&rt).await.unwrap();
    let base = rpc_compose::methods(rt.clone(), |server| rpc_compose::rpc_module(server).module).await;
    // Repeated assembly must neither install a second actor nor duplicate events.
    let _again = rpc_compose::methods(rt.clone(), |server| rpc_compose::rpc_module(server).module).await;
    let (methods, _, _guard) = connection(&base);
    let local = rt.wire_events();
    assert_eq!(rt.wire_external_stream_count(), 0);
    let mut all = topics(&methods, Value::Null, None, false).await;
    let mut selected = topics(&methods, json!(["slack.events", "a2a.changed", "unknown.topic"]), None, false).await;
    let mut empty = topics(&methods, json!([]), None, false).await;
    let mut unknown = topics(&methods, json!(["unknown.topic"]), None, false).await;
    let mut legacy = methods.subscribe_unbounded("subscribe_events", rpc_params![Value::Null, Value::Null]).await.unwrap();
    assert_eq!(next(&mut all).await, pending());
    assert_eq!(next(&mut selected).await, pending());
    assert_eq!(next(&mut empty).await, pending());
    assert_eq!(next(&mut unknown).await, pending());
    assert_eq!(next(&mut legacy).await, pending()["event"]);
    assert_eq!(rt.wire_external_stream_count(), 5);

    // Identical inner tags remain distinct owner families, never guessed from type.
    rt.publish(signal::SignalEvent::Connected).await;
    assert_eq!(next(&mut all).await, envelope("signal.events", json!({"type":"Connected"})));
    assert_eq!(next(&mut legacy).await, json!({"type":"Connected"}));
    rt.publish(slack::SlackEvent::Connected { connections: 2 }).await;
    let connected = json!({"type":"Connected", "connections":2});
    assert_eq!(next(&mut all).await, envelope("slack.events", connected.clone()));
    assert_eq!(next(&mut selected).await, envelope("slack.events", connected.clone()));
    assert_eq!(next(&mut legacy).await, connected);

    let opaque = json!({"type":"FutureOwnerEvent", "session":null, "replay":true, "future":[1,null]});
    rt.publish(sessionsync::events::Relayed { session: SessionId::new(), kind:"FutureOwnerEvent".into(), event:opaque.clone() }).await;
    assert_eq!(next(&mut legacy).await, opaque);
    // A known raw old-peer payload keeps replay and unknown fields in both ABIs.
    tools::declare_wire(&rt).await.unwrap();
    let relayed = json!({"type":"ApprovalResolved", "id":"00000000-0000-0000-0000-000000000001", "decision":"allow", "timed_out":false, "replay":true, "future":7});
    rt.publish(sessionsync::events::Relayed { session:SessionId::new(), kind:"ApprovalResolved".into(), event:relayed.clone() }).await;
    assert_eq!(next(&mut all).await, envelope("tools.approval-resolved", relayed.clone()));
    assert_eq!(next(&mut legacy).await, relayed);
    rt.publish(a2a::A2aChanged {}).await;
    let sentinel = json!({"type":"A2aChanged"});
    assert_eq!(next(&mut all).await, envelope("a2a.changed", sentinel.clone()));
    assert_eq!(next(&mut selected).await, envelope("a2a.changed", sentinel.clone()));
    assert_eq!(next(&mut legacy).await, sentinel);
    // Synchronously overrun the real ring. Controls must reach even [] and unknown filters.
    tokio::task::yield_now().await;
    for _ in 0..1026 {
        rt.wire_feed().send(Arc::new(wire::WireFrame {
            descriptor: None, policy: Default::default(), event: json!({"type":"Opaque"}),
        })).unwrap();
    }
    for stream in [&mut empty, &mut unknown] {
        assert_eq!(next(stream).await, envelope("rpc.stream", json!({"type":"Lagged", "count":2})));
        assert_eq!(next(stream).await, pending());
    }
    drop((all, selected, empty, unknown, legacy));
    tokio::time::timeout(Duration::from_secs(10), async {
        while rt.wire_external_stream_count() != 0 || rt.wire_receiver_count() != 1 {
            tokio::task::yield_now().await;
        }
    }).await.unwrap();
    assert_eq!(rt.wire_receiver_count(), 1);
    drop(local);
}

#[tokio::test]
async fn topic_focus_merging_barriers_and_lagged_snapshot() {
    let rt = runtime();
    agent::declare_wire(&rt).await.unwrap();
    a2a::declare_wire(&rt).await.unwrap();
    let server = rpc_compose::new_server(rt.clone());
    let focus = server.focus.clone();
    let base: jsonrpsee::Methods = rpc_compose::rpc_module(server).module.into();
    let (methods, peer, _guard) = connection(&base);
    let mut stream = topics(&methods, json!(["agent.events", "a2a.changed"]), None, false).await;
    assert_eq!(next(&mut stream).await, pending());
    let s = SessionId::new();
    let r = SessionId::new();
    focus.write().insert(peer.id(), Arc::new([s.to_string()].into()));
    let mut delta = fixture("agent.events", "TextDelta");
    delta["session"] = json!(r);
    delta["text"] = json!("hidden");
    publish::<agent::AgentEvent>(&rt, delta.clone()).await;
    // Direct sends at the real transport seam deterministically queue a merge batch.
    delta["session"] = json!(s);
    let descriptor = Arc::new(rt.wire_schema().into_iter().find(|d| d.topic == "agent.events").unwrap());
    let send = |event: Value| {
        rt.wire_feed().send(Arc::new(wire::WireFrame {
            descriptor: Some(descriptor.clone()), policy: agent::AgentEvent::policy(&event), event,
        })).unwrap();
    };
    delta["text"] = json!("a"); send(delta.clone());
    delta["text"] = json!("b"); send(delta.clone());
    let mut title = fixture("agent.events", "TitleChanged");
    title["session"] = json!(s); title["title"] = json!("barrier"); send(title.clone());
    delta["text"] = json!("c"); send(delta.clone());
    let mut merged = delta.clone(); merged["text"] = json!("ab");
    assert_eq!(next(&mut stream).await, envelope("agent.events", merged));
    assert_eq!(next(&mut stream).await, envelope("agent.events", title.clone()));
    assert_eq!(next(&mut stream).await, envelope("agent.events", delta));
    // Ring loss always precedes snapshot and then the surviving event.
    for _ in 0..1026 { send(title.clone()); }
    assert_eq!(next(&mut stream).await, envelope("rpc.stream", json!({"type":"Lagged", "count":2})));
    assert_eq!(next(&mut stream).await, pending());
    assert_eq!(next(&mut stream).await, envelope("agent.events", title));
}

#[tokio::test]
async fn private_child_bookkeeping_precedes_topic_selection_and_prompts_are_projected() {
    let dir = tempfile::tempdir().unwrap();
    let rt = Runtime::new(Default::default(), dir.path().to_path_buf());
    tools::declare_wire(&rt).await.unwrap();
    agent::declare_wire(&rt).await.unwrap();
    a2a::declare_wire(&rt).await.unwrap();
    tools::registry::start(&rt).await.unwrap();
    let agents = agent::spawn_test_agents(rt.clone(), |_: &str| true).await.unwrap();
    let server = rpc_compose::new_server(rt.clone());
    let private = server.private.clone();
    let base: jsonrpsee::Methods = rpc_core::merge_described([
        rpc_compose::rpc_module(server.clone()),
        agent::rpc_module(server.clone()),
        sessionsync::rpc_module(server),
    ]).expect("test RPC descriptors").module.into();
    let (a, owner, _a_guard) = connection(&base);
    let (b, _, _b_guard) = connection(&base);
    let open: agent::SessionOpen = a.call("create_session", (agent::CreateSessionOptions {
        retention: tools::Retention::Private, view: Some("wire-topics".into()), ..Default::default()
    },)).await.unwrap();
    let root = open.session;
    let mut mine = topics(&a, json!(["tools.jobs"]), Some(root), true).await;
    let mut theirs = topics(&b, json!(["tools.jobs", "a2a.changed"]), None, false).await;
    assert_eq!(next(&mut mine).await, pending());
    assert_eq!(next(&mut theirs).await, pending());
    let child = SessionId::new();
    let mut announce = fixture("agent.events", "SubagentSpawned");
    announce["parent"] = json!(root); announce["child"] = json!(child);
    publish::<agent::AgentEvent>(&rt, announce).await;
    let job = json!({"type":"JobOutput", "session":child, "id":"child-job", "chunk":"private"});
    publish::<tools::JobEvent>(&rt, job.clone()).await;
    assert_eq!(next(&mut mine).await, envelope("tools.jobs", job));
    assert_eq!(private.read().get(&child), Some(&(root, owner.id())));
    rt.publish(a2a::A2aChanged {}).await;
    assert_eq!(next(&mut theirs).await, envelope("a2a.changed", json!({"type":"A2aChanged"})));
    // Admission rejects the foreign private root before accepting the subscription.
    let request = json!({"jsonrpc":"2.0", "id":1, "method":"subscribe_topics", "params":[[], root, true]}).to_string();
    let (response, _) = b.raw_json_request(&request, 1).await.unwrap();
    let response: Value = serde_json::from_str(response.get()).unwrap();
    assert_eq!(response["error"]["code"], rpc_core::ASK_FAILED);

    let approval = rpc_core::lookup::<tools::Approval>(tools::APPROVAL_ACTOR).unwrap();
    let ask = rpc_core::lookup::<tools::AskUser>(tools::ASK_ACTOR).unwrap();
    let mut prompt_events = topics(&a, json!(["tools.approval-requested", "tools.ask-requested"]), None, false).await;
    assert_eq!(next(&mut prompt_events).await, pending());
    let waiting = tokio::spawn({ let actor = approval.clone(); async move {
        actor.ask(tools::approval::RequestApproval { session:root, tool:"shell".into(), args:json!({}), risk:tools::Risk::ReadOnly, call_id:None }).await.unwrap()
    }});
    let approved = next(&mut prompt_events).await;
    let asking = tokio::spawn({ let actor = ask.clone(); async move {
        actor.ask(tools::ask::Request { session:root, questions:vec![], call_id:None }).await.unwrap()
    }});
    let asked = next(&mut prompt_events).await;
    let mut only_asks = topics(&a, json!(["tools.ask-requested"]), None, false).await;
    assert_eq!(next(&mut only_asks).await, envelope("rpc.stream", json!({"type":"PendingPrompts", "approvals":[], "asks":[asked["event"].clone()], "secrets":[]})));
    let mut foreign = topics(&b, Value::Null, None, false).await;
    assert_eq!(next(&mut foreign).await, pending());
    approval.ask(tools::approval::Resolve { id:serde_json::from_value(approved["event"]["id"].clone()).unwrap(), decision:tools::Decision::Deny }).await.unwrap();
    ask.ask(tools::ask::AnswerAsk { id:serde_json::from_value(asked["event"]["id"].clone()).unwrap(), answers:None }).await.unwrap();
    waiting.await.unwrap(); asking.await.unwrap();
    drop((mine, theirs, prompt_events, only_asks, foreign, agents));
}

// An old engine exposes only the original subscription; use the real legacy sink.
#[jsonrpsee::proc_macros::rpc(server)]
trait OldEngine {
    /// The original bare event stream, with no topic negotiation.
    #[subscription(name = "subscribe_events", unsubscribe = "unsubscribe_events", item = Value, with_extensions)]
    async fn subscribe_events(&self) -> jsonrpsee::core::SubscriptionResult;
}

#[jsonrpsee::core::async_trait]
impl OldEngineServer for Server {
    async fn subscribe_events(
        &self,
        pending: jsonrpsee::server::PendingSubscriptionSink,
        ext: &jsonrpsee::Extensions,
    ) -> jsonrpsee::core::SubscriptionResult {
        rpc_compose::api::CompositionApiServer::subscribe_events(self, pending, ext, None, None).await
    }
}

#[tokio::test]
async fn old_engine_probe_falls_back_to_explicit_bare_legacy_mode() {
    let rt = runtime();
    signal::declare_wire(&rt).await.unwrap();
    slack::declare_wire(&rt).await.unwrap();
    let base: jsonrpsee::Methods = OldEngineServer::into_rpc(rpc_compose::new_server(rt.clone())).into();
    let (methods, _, _guard) = connection(&base);
    let (response, _) = methods.raw_json_request(
        r#"{"jsonrpc":"2.0","id":1,"method":"subscribe_topics","params":[null,null,null]}"#, 1,
    ).await.unwrap();
    let response: Value = serde_json::from_str(response.get()).unwrap();
    assert_eq!(response["error"]["code"], -32601);
    let mut legacy = methods.subscribe_unbounded("subscribe_events", rpc_params![]).await.unwrap();
    assert_eq!(next(&mut legacy).await, pending()["event"]);
    rt.publish(signal::SignalEvent::Connected).await;
    rt.publish(slack::SlackEvent::Connected { connections: 2 }).await;
    assert_eq!(next(&mut legacy).await, json!({"type":"Connected"}));
    assert_eq!(next(&mut legacy).await, json!({"type":"Connected", "connections":2}));
}
