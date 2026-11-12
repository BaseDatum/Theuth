use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use jsonrpsee::{server::{ServerBuilder, SubscriptionMessage}, types::ErrorObjectOwned, ws_client::WsClientBuilder};
use serde_json::json;

#[tokio::test]
async fn attached_topics_fall_back_only_for_method_not_found() {
    for mode in ["topics", "legacy", "denied"] {
        let legacy_calls = Arc::new(AtomicUsize::new(0));
        let mut module = jsonrpsee::RpcModule::new(());
        let calls = legacy_calls.clone();
        module.register_subscription("subscribe_events", "subscribe_events", "unsubscribe_events", move |_, pending, _, _| {
            let calls = calls.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                let sink = pending.accept().await.unwrap();
                sink.send(SubscriptionMessage::from(serde_json::value::to_raw_value(&json!({"type":"OldFact","unknown":17})).unwrap())).await.unwrap();
                sink.closed().await;
            }
        }).unwrap();
        if mode == "topics" {
            module.register_subscription("subscribe_topics", "subscribe_topics", "unsubscribe_topics", |params, pending, _, _| async move {
                assert_eq!(params.parse::<serde_json::Value>().unwrap(), json!([null,null,null]));
                let sink = pending.accept().await.unwrap();
                sink.send(SubscriptionMessage::from(serde_json::value::to_raw_value(&json!({"topic":"future.owner","version":8,"event":{"type":"NewFact","unknown":17}})).unwrap())).await.unwrap();
                sink.closed().await;
            }).unwrap();
        } else if mode == "denied" {
            module.register_method("subscribe_topics", |_, _, _| Err::<serde_json::Value, _>(ErrorObjectOwned::owned(-32001, "denied", None::<()>))).unwrap();
        }
        let server = ServerBuilder::default().build("127.0.0.1:0").await.unwrap();
        let addr = server.local_addr().unwrap();
        let handle = server.start(module);
        let client = WsClientBuilder::default().build(format!("ws://{addr}")).await.unwrap();
        let rt = Runtime::new(Default::default(), std::env::temp_dir());
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let runtime = rt.clone();
        let intake = tokio::spawn(async move { remote(&runtime, &client, &tx).await });
        if mode == "denied" {
            assert!(tokio::time::timeout(std::time::Duration::from_secs(5), intake).await.unwrap().unwrap().is_err());
        } else {
            let UiEvent::Wire(event) = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv()).await.unwrap().unwrap() else { panic!("expected one wire event") };
            if mode == "topics" {
                assert_eq!(event.topic(), Some("future.owner"));
                assert_eq!(event.version(), Some(8));
                assert_eq!(event.event(), &json!({"type":"NewFact","unknown":17}));
            } else {
                assert!(matches!(event, TopicEvent::Legacy(_)));
                assert_eq!(event.topic(), None);
                assert_eq!(event.version(), None);
                assert_eq!(event.event(), &json!({"type":"OldFact","unknown":17}));
            }
            drop(rx);
            tokio::time::timeout(std::time::Duration::from_secs(5), intake).await.unwrap().unwrap().unwrap();
        }
        assert_eq!(legacy_calls.load(Ordering::SeqCst), usize::from(mode == "legacy"));
        handle.stop().unwrap();
        handle.stopped().await;
        rt.shutdown.cancel();
    }
}
