//! Tiny owner contracts exercise metadata against actual jsonrpsee registration.
use jsonrpsee::{core::{RpcResult, SubscriptionResult}, proc_macros::rpc};
use rpc_core::{DescribedModule, Descriptors, Discovery, merge_described};
use rpc_core_macros::rpc_descriptors;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Direction-sensitive, recursive subscription-independent result.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Reply {
    text: String,
    #[serde(default)]
    count: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    omitted: Option<String>,
    next: Option<Box<Reply>>,
    pair: (u32, String),
}

mod first {
    use super::*;
    #[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
    pub struct Source { pub path: String }
}
mod second {
    use super::*;
    #[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
    pub struct Source { pub number: u32 }
}

#[rpc_core::rpc_descriptors(owner = "tiny", descriptors = tiny_descriptors)]
#[rpc(server, client, namespace = "tiny", namespace_separator = ".")]
pub trait TinyApi {
    /// Nullable middle slot, required text, omittable tail and named-input rename.
    #[method(name = "send.input", aliases = ["absolute.send"], with_extensions)]
    fn send(&self, session: Option<String>, #[argument(rename = "inputText")] text: String, tail: Option<u32>) -> RpcResult<Reply>;
    /// Unit success serializes as JSON null.
    #[method(name = "unit")]
    fn unit(&self) -> RpcResult<()>;
    /// An absent optional result also serializes as JSON null.
    #[method(name = "null")]
    #[rpc_audience("mesh")]
    fn null(&self) -> RpcResult<Option<String>>;
    /// First of two distinct Rust types sharing the schema name Source.
    #[method(name = "source_a")]
    fn source_a(&self) -> RpcResult<first::Source>;
    /// Second Source must not overwrite the first.
    #[method(name = "source_b")]
    fn source_b(&self) -> RpcResult<second::Source>;
    #[cfg(any())]
    #[method(name = "hidden")]
    fn hidden(&self, value: TypeThatDoesNotExist) -> RpcResult<TypeThatDoesNotExist>;
    /// An enabled cfg method must exist in dispatch and discovery.
    #[cfg(test)]
    #[method(name = "enabled")]
    fn enabled(&self) -> RpcResult<bool>;
    /// Namespaced notifications with absolute subscribe/unsubscribe aliases.
    #[subscription(name = "subscribe_items" => "items.update", item = first::Source,
        aliases = ["absolute.subscribe"], unsubscribe_aliases = ["absolute.stop"], with_extensions)]
    async fn items(&self, session: Option<String>) -> SubscriptionResult;
}

#[rpc_descriptors(owner = "other", descriptors = other_descriptors, audience = "mesh")]
#[rpc(server, client)]
pub trait OtherApi {
    /// Another owner's Source, with named client parameter preference.
    #[method(name = "other.source", param_kind = map)]
    fn source(&self) -> RpcResult<first::Source>;
    /// An independent stream with explicit dotted unsubscribe and notification names.
    #[subscription(name = "cell.attach" => "cell.output", unsubscribe = "cell.detach", item = second::Source)]
    #[rpc_audience("owner")]
    async fn attach(&self) -> SubscriptionResult;
}

// Trait cfg also gates the generated descriptor function before jsonrpsee expands.
#[rpc_descriptors(owner = "absent", descriptors = absent_descriptors)]
#[rpc(server, client)]
#[cfg(any())]
pub trait AbsentApi {
    #[method(name = "absent")]
    fn absent(&self) -> RpcResult<TypeThatDoesNotExist>;
}

struct Owner;

#[jsonrpsee::core::async_trait]
impl TinyApiServer for Owner {
    fn send(&self, _: &jsonrpsee::Extensions, session: Option<String>, text: String, tail: Option<u32>) -> RpcResult<Reply> {
        Ok(Reply { text: format!("{}:{text}", session.unwrap_or_default()), count: tail.unwrap_or_default(), omitted: None, next: None, pair: (7, "seven".into()) })
    }
    fn unit(&self) -> RpcResult<()> { Ok(()) }
    fn null(&self) -> RpcResult<Option<String>> { Ok(None) }
    fn source_a(&self) -> RpcResult<first::Source> { Ok(first::Source { path: "a".into() }) }
    fn source_b(&self) -> RpcResult<second::Source> { Ok(second::Source { number: 2 }) }
    fn enabled(&self) -> RpcResult<bool> { Ok(true) }
    async fn items(&self, pending: jsonrpsee::PendingSubscriptionSink, _: &jsonrpsee::Extensions, _: Option<String>) -> SubscriptionResult {
        let sink = pending.accept().await?;
        sink.send(jsonrpsee::SubscriptionMessage::from(serde_json::value::to_raw_value(&first::Source { path: "stream".into() })?)).await?;
        sink.closed().await;
        Ok(())
    }
}

#[jsonrpsee::core::async_trait]
impl OtherApiServer for Owner {
    fn source(&self) -> RpcResult<first::Source> { Ok(first::Source { path: "other".into() }) }
    async fn attach(&self, pending: jsonrpsee::PendingSubscriptionSink) -> SubscriptionResult {
        let sink = pending.accept().await?;
        sink.send(jsonrpsee::SubscriptionMessage::from(serde_json::value::to_raw_value(&second::Source { number: 99 })?)).await?;
        Ok(())
    }
}

fn composed() -> DescribedModule<Owner> {
    merge_described([
        DescribedModule::new(TinyApiServer::into_rpc(Owner), tiny_descriptors()).unwrap(),
        DescribedModule::new(OtherApiServer::into_rpc(Owner), other_descriptors()).unwrap(),
    ]).unwrap()
}

fn method<'a>(document: &'a Value, name: &str) -> &'a Value {
    document["methods"].as_array().unwrap().iter().find(|m| m["name"] == name).unwrap()
}

fn resolve<'a>(document: &'a Value, schema: &'a Value) -> &'a Value {
    match schema.get("$ref").and_then(Value::as_str) {
        Some(reference) => document.pointer(reference.strip_prefix('#').unwrap()).unwrap(),
        None => schema,
    }
}

async fn request(module: &jsonrpsee::RpcModule<Owner>, name: &str, params: Value) -> Value {
    let (response, _) = module.raw_json_request(&json!({"jsonrpc":"2.0","id":1,"method":name,"params":params}).to_string(), 4).await.unwrap();
    serde_json::from_str(response.get()).unwrap()
}

#[tokio::test]
async fn positional_nullable_slots_and_named_aliases_match_dispatch() {
    let unit = composed();
    let document = Discovery { descriptors: unit.descriptors, ..Default::default() }.document().unwrap();
    let params = &method(&document, "tiny.send.input")["params"];
    assert_eq!(params[0]["required"], true); // nullable middle slot needs a placeholder
    assert_eq!(params[1]["required"], true);
    assert_eq!(params[2]["required"], false);
    assert_eq!(params[0]["schema"]["type"], json!(["string", "null"]));
    assert_eq!(params[1]["name"], "inputText");
    assert_eq!(params[1]["x-theuth-aliases"], json!(["input_text"]));
    let reply = request(&unit.module, "tiny.send.input", json!([null,"hello"])).await;
    assert_eq!(reply["result"]["text"], ":hello");
    assert_eq!(reply["result"]["count"], 0);
    let named = request(&unit.module, "absolute.send", json!({"session":"s","input_text":"named"})).await;
    assert_eq!(named["result"]["text"], "s:named");
    let camel = request(&unit.module, "tiny.send.input", json!({"inputText":"camel"})).await;
    assert_eq!(camel["result"]["text"], ":camel");
    assert_eq!(request(&unit.module, "tiny.unit", json!([])).await["result"], Value::Null);
    assert_eq!(request(&unit.module, "tiny.null", json!([])).await["result"], Value::Null);
    assert_eq!(method(&document, "tiny.unit")["result"]["schema"]["type"], "null");
    assert_eq!(request(&unit.module, "tiny.enabled", json!([])).await["result"], true);
    assert_eq!(request(&unit.module, "tiny.hidden", json!([])).await["error"]["code"], -32601);
    assert_eq!(method(&document, "absolute.send")["x-theuth-aliasFor"], "tiny.send.input");
    assert_eq!(method(&document, "other.source")["x-theuth-audience"], "mesh");
}

#[test]
fn method_audience_overrides_trait_default_for_requests_and_stream_pairs() {
    let unit = composed();
    let document = Discovery { descriptors: unit.descriptors, ..Default::default() }.document().unwrap();
    assert_eq!(method(&document, "tiny.null")["x-theuth-audience"], "mesh");
    assert_eq!(method(&document, "tiny.unit")["x-theuth-audience"], "owner");
    assert_eq!(method(&document, "other.source")["x-theuth-audience"], "mesh");
    assert_eq!(method(&document, "cell.attach")["x-theuth-audience"], "owner");
    assert_eq!(method(&document, "cell.detach")["x-theuth-audience"], "owner");
}

#[test]
fn output_receiver_recursion_tuples_and_colliding_names() {
    let unit = composed();
    let document = Discovery { descriptors: unit.descriptors, ..Default::default() }.document().unwrap();
    assert_eq!(document["openrpc"], "1.3.2");
    let result = &method(&document, "tiny.send.input")["result"];
    let output = resolve(&document, &result["schema"]);
    let receiver = resolve(&document, &result["x-theuth-receiverSchema"]);
    assert!(output["required"].as_array().unwrap().contains(&json!("count")));
    assert!(!receiver["required"].as_array().unwrap().contains(&json!("count")));
    assert!(!output["required"].as_array().unwrap().contains(&json!("omitted")));
    assert_eq!(output["properties"]["pair"]["items"][0]["type"], "integer");
    assert_eq!(output["properties"]["pair"]["items"][1]["type"], "string");
    assert_eq!(output["properties"]["pair"]["minItems"], 2);
    assert_eq!(output["properties"]["pair"]["maxItems"], 2);
    assert!(output["properties"]["pair"].get("prefixItems").is_none());
    let a = resolve(&document, &method(&document, "tiny.source_a")["result"]["schema"]);
    let b = resolve(&document, &method(&document, "tiny.source_b")["result"]["schema"]);
    assert_eq!(a["properties"]["path"]["type"], "string");
    assert_eq!(b["properties"]["number"]["type"], "integer");
    let other = &method(&document, "other.source")["result"]["schema"];
    assert_ne!(other["$ref"], method(&document, "tiny.source_a")["result"]["schema"]["$ref"]);
    // Recursion must refer to its component, never the OpenRPC document root.
    let recursive = &output["properties"]["next"]["anyOf"][0]["$ref"];
    assert_eq!(recursive, &result["schema"]["$ref"]);
}

#[tokio::test]
async fn separate_streams_preserve_notification_item_and_unsubscribe_contracts() {
    let unit = composed();
    let document = Discovery { descriptors: unit.descriptors, ..Default::default() }.document().unwrap();
    let streams = document["x-theuth"]["subscriptions"].as_array().unwrap();
    let stream = streams.iter().find(|s| s["subscribe"] == "tiny.subscribe_items").unwrap();
    assert_eq!(stream["unsubscribe"], "tiny.unsubscribe_items");
    assert_eq!(stream["notification"], "tiny.items.update");
    assert_eq!(stream["params"][0]["required"], false);
    assert_eq!(resolve(&document, &stream["itemSchema"])["properties"]["path"]["type"], "string");
    assert_eq!(method(&document, "tiny.subscribe_items")["result"]["schema"]["type"], json!(["integer","string"]));
    assert_eq!(method(&document, "absolute.stop")["result"]["schema"]["type"], "boolean");
    let (response, mut notifications) = unit.module.raw_json_request(r#"{"jsonrpc":"2.0","id":1,"method":"absolute.subscribe","params":[]}"#, 4).await.unwrap();
    let response: Value = serde_json::from_str(response.get()).unwrap();
    assert!(response.get("error").is_none());
    let notification: Value = serde_json::from_str(notifications.recv().await.unwrap().get()).unwrap();
    assert_eq!(notification["method"], "tiny.items.update");
    assert_eq!(notification["params"]["result"], json!({"path":"stream"}));
    assert_eq!(method(&document, "absolute.stop")["x-theuth-namedParams"], false);
    let stop = request(&unit.module, "absolute.stop", json!([response["result"]])).await;
    assert_eq!(stop["result"], true);
    let (response, mut notifications) = unit.module.raw_json_request(r#"{"jsonrpc":"2.0","id":2,"method":"cell.attach","params":[]}"#, 4).await.unwrap();
    let response: Value = serde_json::from_str(response.get()).unwrap();
    assert!(response.get("error").is_none());
    let notification: Value = serde_json::from_str(notifications.recv().await.unwrap().get()).unwrap();
    assert_eq!(notification["method"], "cell.output");
    assert_eq!(notification["params"]["result"], json!({"number":99}));
}

#[test]
fn module_audit_rejects_missing_duplicate_and_unregistered_descriptors() {
    let mut missing = tiny_descriptors();
    missing.methods.remove(0);
    let error = DescribedModule::new(TinyApiServer::into_rpc(Owner), missing).err().unwrap().to_string();
    assert!(error.contains("missing descriptors"));
    assert!(error.contains("tiny.send.input"));
    let mut extra = tiny_descriptors();
    extra.methods[0].aliases.push("not.registered");
    let error = DescribedModule::new(TinyApiServer::into_rpc(Owner), extra).err().unwrap().to_string();
    assert!(error.contains("descriptors without methods [\"not.registered\"]"));
    let mut duplicate = tiny_descriptors();
    duplicate.methods.push(duplicate.methods[0].clone());
    assert!(duplicate.validate_methods(std::iter::empty()).unwrap_err().to_string().contains("duplicate RPC descriptor"));
    assert!(merge_described([
        DescribedModule::new(TinyApiServer::into_rpc(Owner), tiny_descriptors()).unwrap(),
        DescribedModule::new(TinyApiServer::into_rpc(Owner), tiny_descriptors()).unwrap(),
    ]).is_err());
    assert!(merge_described::<Owner>([]).is_err());
    let mut no_unsubscribe = tiny_descriptors();
    no_unsubscribe.subscriptions.clear();
    assert!(no_unsubscribe.validate_methods(TinyApiServer::into_rpc(Owner).method_names()).unwrap_err().to_string().contains("tiny.unsubscribe_items"));
    assert!(DescribedModule::new(TinyApiServer::into_rpc(Owner), Descriptors::default()).is_err());
}
