//! Event schemas use the same neutral, direction-aware discovery assembler.
use rpc_core::{Discovery, schema_for};
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde_json::{Value, json};
use wire::EventDescriptor;

#[derive(JsonSchema)]
#[schemars(rename = "Source /~é")]
#[allow(dead_code)]
struct Event {
    label: String,
    #[serde(default)]
    count: u32,
    next: Option<Box<Event>>,
}

fn resolve<'a>(document: &'a Value, schema: &'a Value) -> &'a Value {
    match schema.get("$ref").and_then(Value::as_str) {
        Some(reference) => document.pointer(reference.strip_prefix('#').unwrap()).unwrap(),
        None => schema,
    }
}

#[test]
fn event_catalog_uses_consumer_dialect_directions_and_local_collision_safe_refs() {
    let event = |topic| EventDescriptor::public(topic, 1, schema_for::<Event>);
    let discovery = Discovery {
        events: vec![event("z.events"), event("a.events")],
        plugins: vec!["z".into(), "a".into(), "a".into()],
        ..Default::default()
    };
    let document = discovery.document().unwrap();
    let events = document["x-theuth"]["events"].as_array().unwrap();
    assert_eq!(events[0]["topic"], "a.events");
    assert_eq!(events[1]["topic"], "z.events");
    assert_eq!(events[0]["version"], 1);
    assert_eq!(document["x-theuth"]["plugins"], json!(["a", "z"]));
    assert_eq!(document["x-theuth"]["protocolVersion"], json!({"major":1,"minor":0}));
    let output = resolve(&document, &events[0]["schema"]);
    let receiver = resolve(&document, &events[0]["receiverSchema"]);
    assert!(output["required"].as_array().unwrap().contains(&json!("count")));
    assert!(!receiver["required"].as_array().unwrap().contains(&json!("count")));
    assert_eq!(output["properties"]["label"]["type"], "string");
    assert_ne!(events[0]["schema"]["$ref"], events[1]["schema"]["$ref"]);
    assert_eq!(output["properties"]["next"]["anyOf"][0]["$ref"], events[0]["schema"]["$ref"]);
    for name in document["components"]["schemas"].as_object().unwrap().keys() {
        assert!(name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_'));
    }
    let mut reordered = discovery.clone();
    reordered.events.reverse();
    reordered.plugins.reverse();
    assert_eq!(document, reordered.document().unwrap());
    assert_eq!(document["x-theuth"]["eventEnvelopeSchema"]["required"], json!(["topic","version","event"]));
    assert_eq!(document["x-theuth"]["delivery"]["durableReplay"], false);
}

#[test]
fn malformed_nonlocal_and_unresolved_refs_fail_closed_but_example_data_is_inert() {
    let document_with = |schema| Discovery {
        events: vec![EventDescriptor::public("test", 1, schema)],
        ..Default::default()
    }.document();
    for schema in [
        (|_: &mut SchemaGenerator| schemars::json_schema!({"$ref":"https://example.com/schema"})) as fn(&mut SchemaGenerator) -> Schema,
        |_: &mut SchemaGenerator| schemars::json_schema!({"$ref":"#/definitions/Absent"}),
        |_: &mut SchemaGenerator| schemars::json_schema!({"$ref":"#"}),
        |_: &mut SchemaGenerator| schemars::json_schema!({"$ref":42}),
        |_: &mut SchemaGenerator| schemars::json_schema!({"$id":"https://example.com/base","type":"string"}),
    ] {
        assert!(document_with(schema).is_err());
    }
    let document = document_with(|_| schemars::json_schema!({"type":"object","default":{"$ref":"literal user data"}})).unwrap();
    assert_eq!(document["x-theuth"]["events"][0]["schema"]["default"]["$ref"], "literal user data");
    let duplicate = EventDescriptor::public("duplicate", 1, schema_for::<Event>);
    assert!(Discovery { events:vec![duplicate.clone(), duplicate], ..Default::default() }.document().unwrap_err().to_string().contains("duplicate event descriptor"));
}
