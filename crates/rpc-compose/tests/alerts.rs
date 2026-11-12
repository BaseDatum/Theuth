//! 155 §7.3–§7.5: `attention_list` masks `alert` per connection. Its own
//! binary: it registers the attention actor, which the process-wide kameo
//! registry allows once.

use std::time::Duration;

use jsonrpsee::core::server::Subscription;
use kameo::actor::ActorRef;
use roster::alert::AlertClass;
use roster::attention::{Attention, AttentionItem, AttentionKind, Finished};
use rpc_core::{FocusMap, SeenMap, Server};
use runtime::{Runtime, SessionId};
use wire::RuntimeWire as _;

/// One authenticated connection to the shared server state.
struct Conn {
    methods: jsonrpsee::Methods,
    peer: rpc_core::RpcPeer,
    guard: Option<rpc_core::RpcPeerGuard>,
}

impl Conn {
    fn new(
        rt: &Runtime,
        events: &tokio::sync::broadcast::Sender<std::sync::Arc<wire::WireFrame>>,
        focus: &FocusMap,
        seen: &SeenMap,
    ) -> Self {
        let mut methods: jsonrpsee::Methods = rpc_module(Server {
            events: events.clone(),
            rt: rt.clone(),
            private: Default::default(),
            focus: focus.clone(),
            seen: seen.clone(),
            follows: Default::default(),
        })
        .into();
        let (peer, guard) = rpc_core::RpcPeer::open();
        methods.extensions_mut().insert(peer.clone());
        Self {
            methods,
            peer,
            guard: Some(guard),
        }
    }

    async fn list(&self) -> Vec<AttentionItem> {
        self.methods
            .call("attention_list", jsonrpsee::rpc_params![])
            .await
            .unwrap()
    }

    async fn focus(&self, sessions: &[SessionId]) {
        self.methods
            .call::<_, ()>("set_event_focus", (sessions.to_vec(),))
            .await
            .unwrap()
    }
}

/// `(finished alert id, approval alert id)` as this connection sees them.
fn alerts(items: &[AttentionItem]) -> (Option<String>, Option<String>) {
    let of = |k: AttentionKind| {
        items
            .iter()
            .find(|i| i.kind == k)
            .unwrap_or_else(|| panic!("{k:?} item"))
            .alert
            .as_ref()
            .map(|a| a.id.clone())
    };
    (of(AttentionKind::Finished), of(AttentionKind::Approval))
}

/// The next `AttentionChanged` that `pred` accepts.
async fn until(
    sub: &mut Subscription,
    pred: impl Fn(&serde_json::Value) -> bool,
) -> serde_json::Value {
    loop {
        let v = tokio::time::timeout(Duration::from_secs(5), sub.next::<serde_json::Value>())
            .await
            .expect("an event within 5s")
            .expect("stream open")
            .expect("json")
            .0;
        if v["type"] == "AttentionChanged" && pred(&v) {
            return v;
        }
    }
}

/// Alerts appear after `roster.alert_delay_ms` with an `AttentionChanged`
/// nudge; a focused session's alert is blanked on that connection only; a
/// `finished` alert blanked once by focus stays blanked there and visible
/// elsewhere; a needs-you alert returns when focus leaves; the seen set dies
/// with the connection.
#[tokio::test]
async fn attention_list_masks_alerts_per_connection() {
    let cfg = figment::Figment::new().merge(figment::providers::Serialized::defaults(
        serde_json::json!({ "roster": { "alert_delay_ms": 150 } }),
    ));
    let rt = Runtime::new(cfg, std::env::temp_dir());
    roster::declare_wire(&rt).await.unwrap();
    let events = rt.wire_feed();
    let dir = tempfile::tempdir().unwrap();
    let store = roster::store::Store::open(&dir.path().join("r.sqlite"))
        .await
        .unwrap();
    let attention: ActorRef<Attention> = Attention::start(&rt, &store).await.unwrap();
    let (done, asking) = (SessionId::new(), SessionId::new());
    roster::attention::record_finished(
        store.pool(),
        &Finished {
            session: done.to_string(),
            turn: "t1".into(),
            at_ms: 1,
            summary: Some("all green".into()),
            next_steps: vec![],
        },
    )
    .await
    .unwrap();
    attention
        .tell(tools::ApprovalRequested {
            id: tools::ApprovalId(uuid::Uuid::new_v4()),
            session: asking,
            tool: "shell".into(),
            args: serde_json::json!({ "command": "rm x" }),
            risk: tools::Risk::Destructive,
        })
        .await
        .unwrap();

    let (focus, seen) = (FocusMap::default(), SeenMap::default());
    let mut a = Conn::new(&rt, &events, &focus, &seen);
    let b = Conn::new(&rt, &events, &focus, &seen);
    let mut stream = a
        .methods
        .subscribe_unbounded(
            "subscribe_events",
            jsonrpsee::rpc_params![serde_json::Value::Null, serde_json::Value::Null],
        )
        .await
        .unwrap();

    // Inside the delay: items, no alerts; the AlertDue nudge then shows them.
    let first = a.list().await;
    assert_eq!(first.len(), 2, "{first:?}");
    assert_eq!(alerts(&first), (None, None), "inside alert_delay_ms");
    // The first list raised an id-set change; the AlertDue nudge is the empty-diff one (§7.4).
    until(&mut stream, |v| {
        v["ids_added"].as_array().is_some_and(Vec::is_empty)
    })
    .await;
    let shown = a.list().await;
    let (fin, app) = alerts(&shown);
    let fin = fin.expect("finished alert after the delay");
    let app = app.expect("needs-you alert after the delay");
    assert!(fin.starts_with(&format!("finished:{done}:t1@")), "{fin}");
    let fin_alert = shown
        .iter()
        .find(|i| i.kind == AttentionKind::Finished)
        .and_then(|i| i.alert.clone())
        .unwrap();
    assert_eq!(fin_alert.class, AlertClass::Finished);
    assert_eq!(fin_alert.why, "turn ended: all green");
    assert_eq!(
        alerts(&b.list().await),
        (Some(fin.clone()), Some(app.clone())),
        "the other connection sees both"
    );

    // Focus on the asking session: blanked on A while in view, back after.
    a.focus(&[asking]).await;
    assert_eq!(alerts(&a.list().await), (Some(fin.clone()), None));
    assert_eq!(
        alerts(&b.list().await),
        (Some(fin.clone()), Some(app.clone()))
    );
    a.focus(&[]).await;
    assert_eq!(
        alerts(&a.list().await),
        (Some(fin.clone()), Some(app.clone())),
        "needs-you returns when focus leaves"
    );

    // Focus on the finished session: seen on A for good, untouched on B.
    a.focus(&[done]).await;
    a.focus(&[]).await;
    assert_eq!(
        alerts(&a.list().await),
        (None, Some(app.clone())),
        "finished stays seen after focus left"
    );
    assert_eq!(
        alerts(&b.list().await),
        (Some(fin), Some(app)),
        "another connection's look changes nothing"
    );
    assert!(seen.read().get(&a.peer.id()).is_some_and(|s| s.len() == 1));

    // The seen set dies with the connection.
    drop(a.guard.take());
    for _ in 0..100 {
        if !seen.read().contains_key(&a.peer.id()) && !focus.read().contains_key(&a.peer.id()) {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("seen entry outlived its connection");
}

fn rpc_module(server: rpc_core::Server) -> jsonrpsee::RpcModule<rpc_core::Server> {
    rpc_core::merge_described([
        rpc_compose::rpc_module(server.clone()),
        a2a::rpc_module(server.clone()),
        agent::rpc_module(server.clone()),
        sessionsync::rpc_module(server.clone()),
        tasks::rpc_module(server.clone()),
        artifact::rpc_module(server.clone()),
        link::rpc_module(server.clone()),
        talk::rpc_module(server.clone()),
        search_providers::rpc_module(server.clone()),
        app::rpc_module(server.clone()),
        rpc::rpc_module(server.clone()),
        roster::rpc_module(server),
    ]).expect("test RPC descriptors").module
}
