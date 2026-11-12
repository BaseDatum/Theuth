use super::*;
use runtime::kameo_actors::message_bus::Publish;
use serde_json::json;
use std::{
    borrow::Cow,
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};
use tokio::sync::broadcast::error::{RecvError, TryRecvError};

static SCHEMA_CALLS: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Serialize, JsonSchema)]
struct Body {
    seq: u64,
    session: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(transparent)]
struct Local<const N: u8>(Body);

impl<const N: u8> JsonSchema for Local<N> {
    fn schema_name() -> Cow<'static, str> {
        format!("Local{N}").into()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> Schema {
        if N == 3 {
            SCHEMA_CALLS.fetch_add(1, Ordering::SeqCst);
        }
        Body::json_schema(generator)
    }
}

impl<const N: u8> WireEvent for Local<N> {
    const TOPIC: &'static str = match N {
        0 | 2 => "test.a",
        1 => "test.b",
        3 => "test.lazy",
        _ => "test.failure",
    };

    fn to_json(&self) -> serde_json::Result<Value> {
        if N == 4 && self.0.seq == 0 {
            return Err(<serde_json::Error as serde::ser::Error>::custom(
                "local serialization failure",
            ));
        }
        serde_json::to_value(self)
    }

    fn policy(json: &Value) -> EventPolicy {
        EventPolicy {
            delta: Delta::AppendText,
            ..EventPolicy::legacy_fields(json)
        }
    }
}

#[derive(Clone)]
struct Carrier(WirePayload);

#[derive(Clone)]
enum WirePayload {
    Known(Value),
    Unknown(Value),
    Missing,
    Suppressed,
    Invalid,
}

fn runtime() -> Runtime {
    Runtime::new(figment::Figment::new(), std::env::temp_dir())
}

fn event<const N: u8>(seq: u64) -> Local<N> {
    Local(Body {
        seq,
        session: Some("not-necessarily-a-uuid".into()),
    })
}

async fn receive(rx: &mut broadcast::Receiver<Arc<WireFrame>>) -> Arc<WireFrame> {
    tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .unwrap()
        .unwrap()
}

// An acknowledged bus operation followed by a hub ask fences all queued inputs;
// negative delivery assertions need no sleeps or scheduler assumptions.
async fn flush(rt: &Runtime) {
    rt.bus.ask(Publish(())).await.unwrap();
    state(rt).hub.ask(()).await.unwrap();
}

async fn register_carrier(rt: &Runtime) {
    rt.register_wire_source::<Carrier, _>(|carrier| match &carrier.0 {
        WirePayload::Known(event) => Ok(Some(WireIngress::Registered {
            topic: "test.a",
            event: event.clone(),
        })),
        WirePayload::Unknown(event) => Ok(Some(WireIngress::Legacy {
            event: event.clone(),
        })),
        WirePayload::Missing => Ok(Some(WireIngress::Registered {
            topic: "undeclared",
            event: json!({}),
        })),
        WirePayload::Suppressed => Ok(None),
        WirePayload::Invalid => Err(<serde_json::Error as serde::ser::Error>::custom(
            "invalid carrier",
        )),
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn duplicate_concurrent_registration_is_idempotent_and_runtimes_are_isolated() {
    let rt = runtime();
    let other = runtime();
    let clone = rt.clone();
    let (a, b, c) = tokio::join!(
        rt.register_wire_event::<Local<0>>(),
        clone.register_wire_event::<Local<0>>(),
        rt.register_wire_event::<Local<0>>(),
    );
    a.unwrap();
    b.unwrap();
    c.unwrap();
    other.register_wire_event::<Local<0>>().await.unwrap();
    let mut rx = clone.wire_events();
    let mut other_rx = other.wire_events();
    rt.publish(event::<0>(7)).await;
    flush(&rt).await;
    assert_eq!(receive(&mut rx).await.event["seq"], 7);
    assert_eq!(rx.try_recv().unwrap_err(), TryRecvError::Empty);
    assert_eq!(other_rx.try_recv().unwrap_err(), TryRecvError::Empty);
    assert_eq!(
        clone
            .wire_schema()
            .iter()
            .map(|d| d.topic)
            .collect::<Vec<_>>(),
        ["test.a"]
    );
}

#[tokio::test]
async fn topic_collisions_and_mixed_source_event_registration_are_errors() {
    let rt = runtime();
    rt.register_wire_event::<Local<0>>().await.unwrap();
    let collision = rt.register_wire_event::<Local<2>>().await.unwrap_err();
    assert!(collision.to_string().contains("topic collision"));
    assert!(
        rt.register_wire_source::<Local<0>, _>(|_| Ok(None))
            .await
            .is_err()
    );
    let other = runtime();
    other
        .register_wire_source::<Local<0>, _>(|_| Ok(None))
        .await
        .unwrap();
    assert!(other.register_wire_event::<Local<0>>().await.is_err());
    let mut rx = rt.wire_events();
    rt.publish(event::<2>(1)).await;
    rt.publish(event::<0>(2)).await;
    flush(&rt).await;
    assert_eq!(receive(&mut rx).await.event["seq"], 2);
    assert_eq!(rx.try_recv().unwrap_err(), TryRecvError::Empty);
}

#[tokio::test]
async fn late_registration_and_cross_type_bus_order_are_preserved() {
    let rt = runtime();
    let mut rx = rt.wire_events();
    rt.publish(event::<1>(99)).await;
    flush(&rt).await;
    assert_eq!(rx.try_recv().unwrap_err(), TryRecvError::Empty);
    rt.register_wire_event::<Local<0>>().await.unwrap();
    rt.publish(event::<0>(0)).await;
    rt.register_wire_event::<Local<1>>().await.unwrap();
    for seq in 1..=60 {
        if seq % 2 == 0 {
            rt.publish(event::<0>(seq)).await;
        } else {
            rt.publish(event::<1>(seq)).await;
        }
    }
    for seq in 0..=60 {
        let frame = receive(&mut rx).await;
        assert_eq!(frame.event["seq"], seq);
        assert_eq!(
            frame.descriptor.as_ref().unwrap().topic,
            if seq % 2 == 0 { "test.a" } else { "test.b" }
        );
    }
}

#[tokio::test]
async fn local_and_ingress_share_projection_state_without_reset_or_reencoding() {
    let rt = runtime();
    let mut count = 0;
    rt.register_wire_event_with::<Local<0>, _>(move |input| {
        count += 1;
        let mut json = match input {
            EventInput::Typed(event) => event.to_json()?,
            EventInput::Json(json) => json.clone(),
        };
        if json["seq"] == 0 {
            return Ok(None);
        }
        json["projection_count"] = count.into();
        Ok(Some(json))
    })
    .await
    .unwrap();
    register_carrier(&rt).await;
    let mut rx = rt.wire_events();
    rt.publish(event::<0>(0)).await;
    rt.publish(event::<0>(1)).await;
    rt.register_wire_event_with::<Local<0>, _>(|_| panic!("duplicate must not replace projector"))
        .await
        .unwrap();
    rt.register_wire_event::<Local<0>>().await.unwrap();
    rt.register_wire_source::<Carrier, _>(|_| panic!("duplicate must not replace source"))
        .await
        .unwrap();
    rt.publish(Carrier(WirePayload::Known(
        json!({"seq":2,"session":"remote","replay":true,"future":42}),
    )))
    .await;
    rt.publish(event::<0>(3)).await;
    flush(&rt).await;
    let first = receive(&mut rx).await;
    let remote = receive(&mut rx).await;
    let last = receive(&mut rx).await;
    assert_eq!(first.event["projection_count"], 2);
    assert_eq!(
        remote.event,
        json!({"seq":2,"session":"remote","replay":true,"future":42,"projection_count":3})
    );
    assert_eq!(remote.policy.session.as_deref(), Some("remote"));
    assert_eq!(remote.policy.delta, Delta::AppendText);
    assert_eq!(last.event["projection_count"], 4);
    assert!(Arc::ptr_eq(
        first.descriptor.as_ref().unwrap(),
        remote.descriptor.as_ref().unwrap()
    ));
    assert_eq!(rx.try_recv().unwrap_err(), TryRecvError::Empty);
}

#[tokio::test]
async fn bad_serialization_and_ingress_are_dropped_without_stopping_the_hub() {
    let rt = runtime();
    rt.register_wire_event::<Local<4>>().await.unwrap();
    register_carrier(&rt).await;
    let mut rx = rt.wire_events();
    rt.publish(event::<4>(0)).await;
    rt.publish(Carrier(WirePayload::Invalid)).await;
    rt.publish(Carrier(WirePayload::Missing)).await;
    rt.publish(Carrier(WirePayload::Suppressed)).await;
    rt.publish(Carrier(WirePayload::Unknown(
        json!({"session":null,"future":true}),
    )))
    .await;
    rt.publish(event::<4>(1)).await;
    flush(&rt).await;
    let legacy = receive(&mut rx).await;
    assert!(legacy.descriptor.is_none());
    assert_eq!(legacy.event, json!({"session":null,"future":true}));
    assert_eq!(legacy.policy, EventPolicy::default());
    assert_eq!(receive(&mut rx).await.event["seq"], 1);
    assert_eq!(rx.try_recv().unwrap_err(), TryRecvError::Empty);
}

#[tokio::test]
async fn default_projection_preserves_raw_ingress_fields_and_nulls() {
    let rt = runtime();
    rt.register_wire_event::<Local<0>>().await.unwrap();
    register_carrier(&rt).await;
    let mut rx = rt.wire_events();
    let raw = json!({"seq":9,"session":null,"parent":"invalid-uuid","child":42,"scope":{"session":"nested"},"replay":true});
    rt.publish(Carrier(WirePayload::Known(raw.clone()))).await;
    let frame = receive(&mut rx).await;
    assert_eq!(frame.event, raw);
    assert_eq!(frame.policy.session, None);
    assert_eq!(frame.policy.parent.as_deref(), Some("invalid-uuid"));
    assert_eq!(frame.policy.child, None);
    assert!(!frame.policy.announces_child);
    assert!(!frame.policy.always_wanted);
    assert_eq!(
        EventPolicy::legacy_fields(&json!({})),
        EventPolicy::default()
    );
}

#[tokio::test]
async fn broadcast_loss_is_exact_and_receiver_count_has_no_internal_subscriber() {
    let rt = runtime();
    assert_eq!(rt.wire_receiver_count(), 0);
    rt.register_wire_event::<Local<0>>().await.unwrap();
    let mut slow = rt.wire_events();
    let second = rt.clone().wire_events();
    assert_eq!(rt.wire_receiver_count(), 2);
    drop(second);
    for seq in 0..1030 {
        rt.publish(event::<0>(seq)).await;
    }
    flush(&rt).await;
    assert_eq!(slow.recv().await.unwrap_err(), RecvError::Lagged(6));
    for seq in 6..1030 {
        assert_eq!(receive(&mut slow).await.event["seq"], seq);
    }
    assert_eq!(rt.wire_receiver_count(), 1);
    drop(slow);
    assert_eq!(rt.wire_receiver_count(), 0);
    rt.publish(event::<0>(1030)).await;
    flush(&rt).await;
    let mut late = rt.wire_events();
    rt.publish(event::<0>(1031)).await;
    assert_eq!(receive(&mut late).await.event["seq"], 1031);
}

#[tokio::test]
async fn schema_descriptors_are_lazy_and_use_consumer_draft7_contracts() {
    let rt = runtime();
    rt.register_wire_event::<Local<3>>().await.unwrap();
    rt.register_wire_event::<Local<0>>().await.unwrap();
    register_carrier(&rt).await;
    let mut rx = rt.wire_events();
    rt.publish(event::<3>(8)).await;
    let frame = receive(&mut rx).await;
    let catalog = rt.wire_schema();
    assert_eq!(SCHEMA_CALLS.load(Ordering::SeqCst), 0);
    assert_eq!(
        catalog
            .iter()
            .map(|d| (d.topic, d.version))
            .collect::<Vec<_>>(),
        [("test.a", 1), ("test.lazy", 1)]
    );
    use schemars::generate::{Contract, SchemaSettings};
    for (index, contract) in [Contract::Serialize, Contract::Deserialize]
        .into_iter()
        .enumerate()
    {
        let mut settings = SchemaSettings::draft07();
        settings.contract = contract;
        let mut generator = settings.into_generator();
        let schema = (catalog[1].schema)(&mut generator);
        assert_eq!(schema.as_value()["$ref"], "#/definitions/Local3");
        assert_eq!((rt.wire_schema()[1].schema)(&mut generator), schema);
        assert_eq!(
            (frame.descriptor.as_ref().unwrap().schema)(&mut generator),
            schema
        );
        (catalog[0].schema)(&mut generator);
        let root = generator.root_schema_for::<Value>();
        assert_eq!(
            root.as_value()["$schema"],
            "http://json-schema.org/draft-07/schema#"
        );
        assert!(root.as_value().get("$defs").is_none());
        assert_eq!(
            root.as_value()["definitions"]["Local0"]["properties"]["seq"]["type"],
            "integer"
        );
        let body = &root.as_value()["definitions"]["Local3"];
        assert_eq!(body["properties"]["seq"]["type"], "integer");
        assert_eq!(
            body["properties"]["session"]["type"],
            json!(["string", "null"])
        );
        assert_eq!(
            body["required"],
            if index == 0 {
                json!(["seq", "session"])
            } else {
                json!(["seq"])
            }
        );
        assert_eq!(SCHEMA_CALLS.load(Ordering::SeqCst), index + 1);
    }
}
