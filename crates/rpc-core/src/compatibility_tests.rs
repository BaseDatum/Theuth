use super::*;
use serde_json::json;

fn document(schema: Value) -> Value {
    json!({
        "openrpc":"1.3.2", "info":{"title":"wire","version":"1.0.0"},
        "methods":[{"name":"read","paramStructure":"by-position","params":[
            {"name":"query","required":true,"schema":{"type":"string"}}
        ],"result":{"name":"result","schema":schema,"x-theuth-receiverSchema":schema}}],
        "components":{"schemas":{}},
        "x-theuth":{"protocolVersion":{"major":1,"minor":0},"plugins":["owner"],
            "events":[],"subscriptions":[],
            "delivery":{"unknownTopicVersionVariant":"ignore"}}
    })
}

fn object() -> Value {
    json!({"type":"object","properties":{"id":{"type":"string"},"note":{"type":"string"}},"required":["id"]})
}

fn output(schema: Value, mutate: impl Fn(&mut Value)) -> (Value, Value) {
    let old = document(schema);
    let mut new = old.clone();
    mutate(&mut new["methods"][0]["result"]["schema"]);
    mutate(&mut new["methods"][0]["result"]["x-theuth-receiverSchema"]);
    (old, new)
}

fn has(old: &Value, new: &Value, class: ChangeClass, suffix: &str) {
    let report = classify(old, new);
    assert!(report.changes.iter().any(|c| c.class == class && c.path.ends_with(suffix)), "{report:#?}");
}

#[test]
fn remove_or_rename_method() {
    let old = document(object());
    let mut new = old.clone();
    new["methods"] = json!([]);
    has(&old, &new, ChangeClass::Breaking, "/methods/read");
    new = old.clone();
    new["methods"][0]["name"] = json!("renamed");
    has(&old, &new, ChangeClass::Breaking, "/methods/read");
    has(&old, &new, ChangeClass::Additive, "/methods/renamed");
}

#[test]
fn remove_or_rename_optional_property() {
    let (old, new) = output(object(), |s| { s["properties"].as_object_mut().unwrap().remove("note"); });
    has(&old, &new, ChangeClass::Breaking, "/properties/note");
    let (old, new) = output(object(), |s| {
        let value = s["properties"].as_object_mut().unwrap().remove("note").unwrap();
        s["properties"]["renamed"] = value;
    });
    has(&old, &new, ChangeClass::Breaking, "/properties/note");
}

#[test]
fn scalar_container_and_nullability_changes() {
    for (before, after) in [(json!({"type":"string"}), json!({"type":"integer"})),
        (json!({"type":"array","items":{"type":"string"}}), json!({"type":"object"})),
        (json!({"type":"string"}), json!({"type":["string","null"]}))] {
        let (old, new) = output(before, |s| *s = after.clone());
        has(&old, &new, ChangeClass::Breaking, "/type");
    }
}

#[test]
fn tag_spelling_and_location_changes() {
    let tagged = json!({"type":"object","properties":{"type":{"const":"Ready"}},"required":["type"]});
    let (old, new) = output(tagged.clone(), |s| s["properties"]["type"]["const"] = json!("ready"));
    has(&old, &new, ChangeClass::Breaking, "/properties/type/const");
    let (old, new) = output(tagged, |s| {
        let tag = s["properties"].as_object_mut().unwrap().remove("type").unwrap();
        s["properties"]["kind"] = tag;
        s["required"] = json!(["kind"]);
    });
    has(&old, &new, ChangeClass::Breaking, "/properties/type");
}

#[test]
fn tuple_position_length_and_map_value_changes() {
    let tuple = json!({"type":"array","items":[{"type":"string"},{"type":"integer"}]});
    let (old, new) = output(tuple.clone(), |s| s["items"].as_array_mut().unwrap().swap(0, 1));
    has(&old, &new, ChangeClass::Breaking, "/items/0/type");
    let (old, new) = output(tuple, |s| { s["items"].as_array_mut().unwrap().pop(); });
    has(&old, &new, ChangeClass::Breaking, "/items");
    let (old, new) = output(json!({"type":"object","additionalProperties":{"type":"string"}}), |s| s["additionalProperties"]["type"] = json!("integer"));
    has(&old, &new, ChangeClass::Breaking, "/additionalProperties/type");
}

#[test]
fn existing_requiredness_changes_both_directions() {
    let (old, new) = output(object(), |s| s["required"] = json!(["id","note"]));
    has(&old, &new, ChangeClass::Breaking, "/properties/note");
    has(&new, &old, ChangeClass::Breaking, "/properties/note");
}

#[test]
fn required_input_parameter_and_property_are_breaking() {
    let old = document(object());
    let mut new = old.clone();
    new["methods"][0]["params"].as_array_mut().unwrap().push(json!({"name":"extra","required":true,"schema":{"type":"string"}}));
    has(&old, &new, ChangeClass::Breaking, "/params/1");
    let (old, new) = output(object(), |s| {
        s["properties"]["extra"] = json!({"type":"string"});
        s["required"] = json!(["id","extra"]);
    });
    has(&old, &new, ChangeClass::Breaking, "/properties/extra");
}

#[test]
fn optional_trailing_parameter_not_nullable_required_slot() {
    let old = document(object());
    let mut new = old.clone();
    new["methods"][0]["params"].as_array_mut().unwrap().push(json!({"name":"extra","required":false,"schema":{"type":["string","null"]}}));
    has(&old, &new, ChangeClass::Additive, "/params/1");
    assert!(!classify(&old, &new).fails());
    new["methods"][0]["params"][1]["required"] = json!(true);
    has(&old, &new, ChangeClass::Breaking, "/params/1");
}

#[test]
fn parameter_insertion_order_and_named_input_rename() {
    let old = document(object());
    let mut new = old.clone();
    new["methods"][0]["params"].as_array_mut().unwrap().insert(0, json!({"name":"extra","required":false,"schema":{"type":"string"}}));
    has(&old, &new, ChangeClass::Breaking, "/params/0/name");
    new = old.clone();
    new["methods"][0]["params"][0]["name"] = json!("renamed");
    has(&old, &new, ChangeClass::Breaking, "/params/0/name");
    let second = json!({"name":"second","required":true,"schema":{"type":"string"}});
    let mut old = old;
    old["methods"][0]["params"].as_array_mut().unwrap().push(second);
    new = old.clone();
    new["methods"][0]["params"].as_array_mut().unwrap().swap(0, 1);
    has(&old, &new, ChangeClass::Breaking, "/params/0/name");
}

#[test]
fn extra_method_and_optional_property_are_additive() {
    let old = document(object());
    let mut new = old.clone();
    let mut method = new["methods"][0].clone();
    method["name"] = json!("extra");
    new["methods"].as_array_mut().unwrap().push(method);
    has(&old, &new, ChangeClass::Additive, "/methods/extra");
    assert!(!classify(&old, &new).fails());
    let (old, new) = output(object(), |s| s["properties"]["extra"] = json!({"type":"string"}));
    has(&old, &new, ChangeClass::Additive, "/properties/extra");
    assert!(!classify(&old, &new).fails());
}

#[test]
fn output_addition_uses_receiver_absence_not_serializer_requiredness() {
    let old = document(object());
    let mut new = old.clone();
    for key in ["schema", "x-theuth-receiverSchema"] {
        new["methods"][0]["result"][key]["properties"]["extra"] = json!({"type":"string","default":""});
    }
    new["methods"][0]["result"]["schema"]["required"] = json!(["id","extra"]);
    has(&old, &new, ChangeClass::Additive, "/properties/extra");
    assert!(!classify(&old, &new).fails());
    new["methods"][0]["result"]["x-theuth-receiverSchema"]["required"] = json!(["id","extra"]);
    has(&old, &new, ChangeClass::Breaking, "/properties/extra");
}

fn event_doc() -> Value {
    let mut doc = document(object());
    doc["x-theuth"]["events"] = json!([{"topic":"owner.changed","version":1,
        "schema":{"enum":["Ready"]},"receiverSchema":{"enum":["Ready"]}}]);
    doc
}

#[test]
fn topic_removal_rename_and_version_replacement() {
    let old = event_doc();
    for (key, value) in [("topic", json!("owner.renamed")), ("version", json!(2))] {
        let mut new = old.clone();
        new["x-theuth"]["events"][0][key] = value;
        has(&old, &new, ChangeClass::Breaking, "/events/owner.changed@1");
    }
    let mut new = old.clone();
    new["x-theuth"]["events"] = json!([]);
    has(&old, &new, ChangeClass::Breaking, "/events/owner.changed@1");
}

#[test]
fn extra_topic_or_retained_topic_version_is_additive() {
    let old = event_doc();
    let mut new = old.clone();
    let mut event = new["x-theuth"]["events"][0].clone();
    event["version"] = json!(2);
    new["x-theuth"]["events"].as_array_mut().unwrap().push(event);
    has(&old, &new, ChangeClass::Additive, "/events/owner.changed@2");
    assert!(!classify(&old, &new).fails());
}

#[test]
fn event_variants_require_explicit_unknown_variant_ignore() {
    let old = event_doc();
    let mut new = old.clone();
    for key in ["schema", "receiverSchema"] { new["x-theuth"]["events"][0][key]["enum"] = json!(["Ready","Done"]); }
    has(&old, &new, ChangeClass::Additive, "/enum");
    assert!(!classify(&old, &new).fails());
    let mut old = old;
    old["x-theuth"]["delivery"].as_object_mut().unwrap().remove("unknownTopicVersionVariant");
    new["x-theuth"]["delivery"].as_object_mut().unwrap().remove("unknownTopicVersionVariant");
    has(&old, &new, ChangeClass::ReviewRequired, "/enum");
}

#[test]
fn input_enum_addition_output_enum_review_and_member_removal() {
    let mut old = document(json!({"enum":["A"]}));
    let mut new = old.clone();
    new["methods"][0]["result"]["schema"]["enum"] = json!(["A","B"]);
    has(&old, &new, ChangeClass::ReviewRequired, "/enum");
    has(&new, &old, ChangeClass::Breaking, "/enum");
    old["methods"][0]["params"][0]["schema"] = json!({"enum":["A"]});
    new = old.clone();
    new["methods"][0]["params"][0]["schema"]["enum"] = json!(["A","B"]);
    has(&old, &new, ChangeClass::Additive, "/enum");
    assert!(!classify(&old, &new).fails());
}

#[test]
fn tagged_union_addition_and_tag_change() {
    let schema = json!({"oneOf":[{"type":"object","properties":{"type":{"const":"A"}},"required":["type"]}]});
    let mut old = document(object());
    old["methods"][0]["params"][0]["schema"] = schema;
    let mut new = old.clone();
    new["methods"][0]["params"][0]["schema"]["oneOf"].as_array_mut().unwrap().push(json!({"type":"object","properties":{"type":{"const":"B"}},"required":["type"]}));
    has(&old, &new, ChangeClass::Additive, "/oneOf");
    assert!(!classify(&old, &new).fails());
    new["methods"][0]["params"][0]["schema"]["oneOf"][0]["properties"]["type"]["const"] = json!("renamed");
    has(&old, &new, ChangeClass::Breaking, "/properties/type/const");
}

fn subscription_doc() -> Value {
    let mut doc = document(object());
    doc["x-theuth"]["subscriptions"] = json!([{"subscribe":"watch","unsubscribe":"unwatch","notification":"changed","params":[],
        "itemSchema":{"type":"string"},"receiverSchema":{"type":"string"}}]);
    doc
}

#[test]
fn subscription_pair_notification_and_item_shape_changes() {
    let old = subscription_doc();
    for key in ["subscribe", "unsubscribe", "notification"] {
        let mut new = old.clone();
        new["x-theuth"]["subscriptions"][0][key] = json!("renamed");
        assert!(classify(&old, &new).changes.iter().any(|c| c.class == ChangeClass::Breaking));
    }
    let mut new = old.clone();
    new["x-theuth"]["subscriptions"][0]["itemSchema"]["type"] = json!("integer");
    has(&old, &new, ChangeClass::Breaking, "/itemSchema/type");
    new = old.clone();
    new["x-theuth"]["subscriptions"] = json!([]);
    has(&old, &new, ChangeClass::Breaking, "/subscriptions/watch");
}

#[test]
fn tightened_constraints_and_unknown_relaxation() {
    for (key, old_value, new_value) in [("minimum",json!(1),json!(2)), ("maximum",json!(10),json!(9)),
        ("maxItems",json!(10),json!(9)), ("pattern",json!("a"),json!("b")),
        ("additionalProperties",json!(true),json!(false))] {
        let mut schema = json!({"type":"object"});
        schema[key] = old_value;
        let (old, new) = output(schema, |s| s[key] = new_value.clone());
        has(&old, &new, ChangeClass::Breaking, &format!("/{key}"));
    }
    let (old, new) = output(json!({"type":"integer","minimum":2}), |s| s["minimum"] = json!(1));
    has(&old, &new, ChangeClass::ReviewRequired, "/minimum");
}

#[test]
fn documentation_and_unordered_catalog_changes_are_nonbreaking() {
    let (old, new) = output(object(), |s| s["description"] = json!("new docs"));
    has(&old, &new, ChangeClass::Nonbreaking, "/description");
    assert!(!classify(&old, &new).fails());
    let mut old = event_doc();
    let mut second = old["methods"][0].clone();
    second["name"] = json!("extra");
    old["methods"].as_array_mut().unwrap().push(second);
    let mut new = old.clone();
    new["methods"].as_array_mut().unwrap().reverse();
    has(&old, &new, ChangeClass::Nonbreaking, "/");
    assert!(!classify(&old, &new).fails());
}

#[test]
fn property_named_title_is_not_a_documentation_annotation() {
    let (old, new) = output(json!({"type":"object","properties":{"title":{"type":"string"}}}), |s| {
        s["properties"].as_object_mut().unwrap().remove("title");
    });
    has(&old, &new, ChangeClass::Breaking, "/properties/title");
}

#[test]
fn opaque_schema_semantics_are_not_detectable() {
    let old = document(json!(true));
    assert_eq!(classify(&old, &old), Report::default());
}

fn recursive_doc(name: &str) -> Value {
    let mut doc = document(json!({"$ref":format!("#/components/schemas/{name}")}));
    doc["components"]["schemas"][name] = json!({"type":"object","properties":{
        "value":{"type":"string"},"next":{"anyOf":[{"$ref":format!("#/components/schemas/{name}")},{"type":"null"}]}
    },"required":["value"]});
    doc
}

#[test]
fn recursive_refs_follow_unchanged_ref_spelling_and_terminate() {
    let old = recursive_doc("Node");
    assert!(!classify(&old, &old).fails());
    let mut new = old.clone();
    new["components"]["schemas"]["Node"]["properties"]["value"]["type"] = json!("integer");
    has(&old, &new, ChangeClass::Breaking, "/properties/value/type");
}

#[test]
fn rename_only_recursive_refs_are_nonbreaking() {
    let old = recursive_doc("Node");
    let new = recursive_doc("Renamed");
    has(&old, &new, ChangeClass::Nonbreaking, "/");
    assert!(!classify(&old, &new).fails());
}

#[test]
fn ref_to_inline_and_single_allof_layout_are_nonbreaking() {
    let old = document(object());
    let mut new = old.clone();
    new["components"]["schemas"]["Renamed"] = object();
    for key in ["schema", "x-theuth-receiverSchema"] {
        new["methods"][0]["result"][key] = json!({"allOf":[{"$ref":"#/components/schemas/Renamed"}]});
    }
    has(&old, &new, ChangeClass::Nonbreaking, "/");
    assert!(!classify(&old, &new).fails());
}

#[test]
fn identical_allof_with_refs_and_siblings_is_equal() {
    let mut doc = document(json!({"allOf":[{"$ref":"#/components/schemas/Item"}],"default":null}));
    doc["components"]["schemas"]["Item"] = object();
    assert_eq!(classify(&doc, &doc), Report::default());
}

#[test]
fn allof_changes_fail_closed_and_unchanged_refs_follow_targets() {
    let mut old = document(json!({"allOf":[{"$ref":"#/components/schemas/Item"}],"default":null}));
    old["components"]["schemas"]["Item"] = object();
    let mut new = old.clone();
    new["components"]["schemas"]["Item"]["properties"]["id"]["type"] = json!("integer");
    has(&old, &new, ChangeClass::Breaking, "/allOf/0/properties/id/type");
    new = old.clone();
    new["methods"][0]["result"]["schema"]["allOf"].as_array_mut().unwrap().push(json!({"minProperties":2}));
    has(&old, &new, ChangeClass::ReviewRequired, "/allOf");
    new = old.clone();
    new["methods"][0]["result"]["schema"]["allOf"][0] = json!({"type":"integer"});
    has(&old, &new, ChangeClass::ReviewRequired, "/allOf");
}

#[test]
fn unknown_keyword_and_default_changes_fail_closed() {
    for key in ["x-validation", "default", "not", "if"] {
        let (old, new) = output(object(), |s| s[key] = json!({"type":"string"}));
        has(&old, &new, ChangeClass::ReviewRequired, &format!("/{key}"));
        assert!(classify(&old, &new).changes.iter().any(|c| c.reason.contains(key)));
    }
}

#[test]
fn duplicate_catalog_names_fail_closed() {
    let old = document(object());
    let mut new = old.clone();
    let duplicate = new["methods"][0].clone();
    new["methods"].as_array_mut().unwrap().push(duplicate);
    has(&old, &new, ChangeClass::ReviewRequired, "/methods/read");
}

#[test]
fn external_dangling_and_reference_only_cycles_fail_closed() {
    let old = document(object());
    for reference in ["https://invalid.example/schema", "#/components/schemas/Missing", "#/components/schemas/Cycle"] {
        let mut new = document(json!({"$ref":reference}));
        new["components"]["schemas"]["Cycle"] = json!({"$ref":"#/components/schemas/Cycle"});
        assert!(classify(&old, &new).changes.iter().any(|c| c.class == ChangeClass::ReviewRequired && c.reason.contains("ref")));
    }
}

#[test]
fn changed_ref_siblings_and_unknown_document_extension_require_review() {
    let old = document(object());
    let mut new = document(json!({"$ref":"#/components/schemas/S","minProperties":1}));
    new["components"]["schemas"]["S"] = object();
    assert!(classify(&old, &new).changes.iter().any(|c| c.class == ChangeClass::ReviewRequired && c.reason.contains("siblings")));
    new = old.clone();
    new["x-unknown"] = json!(true);
    has(&old, &new, ChangeClass::ReviewRequired, "/x-unknown");
}

#[test]
fn nullable_union_extension_is_not_an_additive_variant() {
    let mut old = document(object());
    old["methods"][0]["params"][0]["schema"] = json!({"anyOf":[{"type":"string"}]});
    let mut new = old.clone();
    new["methods"][0]["params"][0]["schema"]["anyOf"].as_array_mut().unwrap().push(json!({"type":"null"}));
    has(&old, &new, ChangeClass::Breaking, "/anyOf");
    new["methods"][0]["params"][0]["schema"]["anyOf"][1] = json!({"type":"integer"});
    has(&old, &new, ChangeClass::ReviewRequired, "/anyOf");
}

#[test]
fn shared_refs_report_each_method_and_receiver_path() {
    let mut old = recursive_doc("Node");
    let mut method = old["methods"][0].clone();
    method["name"] = json!("second");
    old["methods"].as_array_mut().unwrap().push(method);
    let mut new = old.clone();
    new["components"]["schemas"]["Node"]["properties"]["value"]["type"] = json!("integer");
    let report = classify(&old, &new);
    for method in ["read", "second"] {
        for view in ["schema", "x-theuth-receiverSchema"] {
            assert!(report.changes.iter().any(|c| c.class == ChangeClass::Breaking
                && c.path == format!("/methods/{method}/result/{view}/properties/value/type")), "{report:#?}");
        }
    }
}

#[test]
fn required_input_object_field_is_breaking() {
    let mut old = document(object());
    old["methods"][0]["params"][0]["schema"] = object();
    let mut new = old.clone();
    let schema = &mut new["methods"][0]["params"][0]["schema"];
    schema["properties"]["extra"] = json!({"type":"string"});
    schema["required"] = json!(["id","extra"]);
    has(&old, &new, ChangeClass::Breaking, "/params/0/schema/properties/extra");
}

#[test]
fn local_definition_layout_and_escaped_refs_preserve_shape() {
    let old = document(object());
    let mut new = old.clone();
    for key in ["schema", "x-theuth-receiverSchema"] {
        new["methods"][0]["result"][key] = json!({"$ref":"#/components/schemas/a~1b~0c"});
    }
    new["components"]["schemas"]["a/b~c"] = object();
    assert!(!classify(&old, &new).fails());
    let mut inline = old.clone();
    inline["methods"][0]["result"]["schema"]["definitions"] = json!({"unused":{"type":"integer"}});
    assert!(!classify(&old, &inline).fails());
}

#[test]
fn malformed_changed_schema_construct_requires_review() {
    for key in ["properties", "required"] {
        let (old, new) = output(object(), |s| s[key] = json!(42));
        has(&old, &new, ChangeClass::ReviewRequired, &format!("/{key}"));
    }
}
