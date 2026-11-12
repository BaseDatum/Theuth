//! Prompts across subscribers (31 §7.3, 70 as-built ASK-SYNC). Its own binary:
//! it registers the `ask` and `approval` actors, which the process-wide kameo
//! registry allows once, and `search`'s in-crate test starts `ToolsPlugin`.

use std::time::Duration;

use jsonrpsee::core::server::Subscription;
use kameo::actor::Spawn;
use rpc::into_rpc_module;
use rpc::api::{PendingPrompts, Server};
use runtime::{Runtime, SessionId};
use tools::{Approval, AskUser, Decision};

/// The next `subscribe_events` item, decoded.
async fn next(sub: &mut Subscription) -> serde_json::Value {
    let item = tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("an event within 5s")
        .expect("stream open")
        .expect("decodable");
    item.0
}

/// Items until one of `ty` (inclusive), or panic after 5s.
async fn until(sub: &mut Subscription, ty: &str) -> serde_json::Value {
    loop {
        let v = next(sub).await;
        if v["type"] == ty {
            return v;
        }
    }
}

/// The snapshot, typed; `type` tag checked on the way.
fn snapshot(v: &serde_json::Value) -> PendingPrompts {
    assert_eq!(v["type"], "PendingPrompts", "{v}");
    serde_json::from_value(v.clone()).expect("PendingPrompts")
}

/// A prompt answered through one connection is `*Resolved` on every other; a
/// subscriber that arrives while prompts are pending gets them in the
/// `PendingPrompts` snapshot that opens its stream; one that arrives after the
/// answers gets an empty snapshot; a second answer is refused with `false`.
#[tokio::test]
async fn prompts_resolve_everywhere_and_replay_only_while_pending() {
    let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
    let events = rpc::bridge::start(&rt).await;
    let ask = AskUser::spawn(AskUser::new(rt.clone(), Duration::from_secs(30)));
    ask.register(tools::ASK_ACTOR).unwrap();
    let approval = Approval::spawn(Approval::new(rt.clone(), Duration::from_secs(30)));
    approval.register(tools::APPROVAL_ACTOR).unwrap();
    let mut methods: jsonrpsee::Methods = into_rpc_module(Server {
        events,
        rt: rt.clone(),
        private: Default::default(),
        focus: Default::default(),
        seen: Default::default(),
        follows: Default::default(),
    })
    .into();
    let (peer, _guard) = rpc_core::RpcPeer::open();
    methods.extensions_mut().insert(peer);
    let subscribe = || {
        methods.subscribe_unbounded(
            "subscribe_events",
            jsonrpsee::rpc_params![serde_json::Value::Null, serde_json::Value::Null],
        )
    };
    let call = |req: String| {
        let methods = &methods;
        async move {
            let (resp, _) = methods.raw_json_request(&req, 1).await.unwrap();
            serde_json::from_str::<serde_json::Value>(resp.get()).unwrap()["result"].clone()
        }
    };

    // A: nothing pending yet, an empty snapshot opens the stream.
    let mut a = subscribe().await.unwrap();
    let empty = snapshot(&next(&mut a).await);
    assert!(empty.asks.is_empty() && empty.approvals.is_empty() && empty.secrets.is_empty());

    let session = SessionId::new();
    let asked = tokio::spawn({
        let ask = ask.clone();
        async move {
            ask.ask(tools::ask::Request {
                session,
                questions: vec![tools::Question {
                    id: "q".into(),
                    question: "which?".into(),
                    options: vec![],
                    recommended: None,
                }],
                call_id: None,
            })
            .await
            .unwrap()
        }
    });
    let approved = tokio::spawn({
        let approval = approval.clone();
        async move {
            approval
                .ask(tools::approval::RequestApproval {
                    session,
                    tool: "shell".into(),
                    args: serde_json::json!({ "command": "rm x" }),
                    risk: tools::Risk::Destructive,
                    call_id: None,
                })
                .await
                .unwrap()
        }
    });
    let ask_id = until(&mut a, "AskRequested").await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let approval_id = until(&mut a, "ApprovalRequested").await["id"]
        .as_str()
        .unwrap()
        .to_owned();

    // B: subscribes while both are pending; the snapshot carries both.
    let mut b = subscribe().await.unwrap();
    let pending = snapshot(&next(&mut b).await);
    assert_eq!(pending.asks.len(), 1);
    assert_eq!(pending.asks[0].id.0.to_string(), ask_id);
    assert_eq!(pending.asks[0].session, session);
    assert_eq!(pending.approvals.len(), 1);
    assert_eq!(pending.approvals[0].id.0.to_string(), approval_id);
    assert_eq!(pending.approvals[0].tool, "shell");

    // Answered once: `true`; again: `false`, nothing else happens.
    let answer = |id: &str| {
        format!(
            r#"{{"jsonrpc":"2.0","method":"answer_ask","params":["{id}",[{{"kind":"text","text":"this"}}]],"id":1}}"#
        )
    };
    assert_eq!(call(answer(&ask_id)).await, true);
    assert_eq!(call(answer(&ask_id)).await, false);
    let decide = |id: &str| {
        format!(
            r#"{{"jsonrpc":"2.0","method":"resolve_approval","params":["{id}","allow"],"id":1}}"#
        )
    };
    assert_eq!(call(decide(&approval_id)).await, true);
    assert_eq!(call(decide(&approval_id)).await, false);
    assert!(asked.await.unwrap().answers.is_some());
    assert_eq!(approved.await.unwrap().decision, Decision::Allow);

    // Both live subscribers see both resolutions, keyed by the request id.
    for sub in [&mut a, &mut b] {
        assert_eq!(until(sub, "AskResolved").await["id"], ask_id);
        assert_eq!(until(sub, "ApprovalResolved").await["id"], approval_id);
    }

    // C: subscribes after the answers; the snapshot is empty again.
    let mut c = subscribe().await.unwrap();
    let after = snapshot(&next(&mut c).await);
    assert!(after.asks.is_empty() && after.approvals.is_empty());
}
