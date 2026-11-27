use super::*;
use crate::communication::mesh::loopback::{Loop, nodes};
use replica::{ByteStream, PeerEvent};
use rpc_core::{RpcPeer, RpcPeerGuard};
use serde_json::json;
use tokio::sync::broadcast;

const SEALED_MSG: &str = "the vault is sealed on P50 — unlock it from an owner device or with the master password";

/// A target engine's API: `set_model` reports how it was called; `config_flag_set` answers
/// `restart_required` on `b` and a sealed vault on `d`; `list_prompt_sections` the sealed
/// code; `shutdown` (not allowlisted) reports the press like `set_model`.
fn target_api(label: &'static str) -> jsonrpsee::Methods {
    let mut m = jsonrpsee::RpcModule::new(());
    let seen = move |params: jsonrpsee::types::Params<'_>, ext: &Extensions| {
        json!({
            "on": label,
            "press": secrets::rpc::human_press(ext),
            "forwarded": ext.get::<forward::Forwarded>().is_some(),
            "params": params.parse::<Value>().unwrap_or(Value::Null),
        })
    };
    m.register_method("set_model", move |p, _, ext| -> RpcResult<Value> { Ok(seen(p, ext)) })
        .unwrap();
    m.register_method("shutdown", move |p, _, ext| -> RpcResult<Value> { Ok(seen(p, ext)) })
        .unwrap();
    m.register_method("config_flag_set", move |_, _, _| -> RpcResult<Value> {
        match label {
            "d" => Err(rpc_core::failed(SEALED_MSG)),
            _ => Ok(json!({ "restart_required": label == "b" })),
        }
    })
    .unwrap();
    m.register_method("set_approvals", |_, _, _| -> RpcResult<Value> {
        Err(rpc_core::failed(SEALED_MSG))
    })
    .unwrap();
    m.register_method("list_prompt_sections", |_, _, _| -> RpcResult<Value> {
        Err(ErrorObjectOwned::owned(SEALED, "sealed", None::<()>))
    })
    .unwrap();
    m.into()
}

/// The owner's app connection (keep the guard: a closed peer is no press).
fn owner() -> (Extensions, RpcPeerGuard) {
    let (peer, guard) = RpcPeer::open();
    let mut ext = Extensions::new();
    ext.insert(peer);
    (ext, guard)
}

/// An agent's call: the internal peer (MCP console, ACP agent half).
fn agent() -> Extensions {
    let mut ext = Extensions::new();
    ext.insert(RpcPeer::internal());
    ext
}

fn token() -> PressToken {
    PressToken { at: 1 }
}

/// `a`'s reach over `t`: its own API locally.
fn reach_a(t: &Arc<Loop>) -> Reach {
    Reach {
        me: vec!["a".into()],
        local: Some(target_api("a")),
        transport: Some(t.clone() as Arc<dyn Transport>),
    }
}

/// `node` answers `theuth/rpc/1` with `target_api(label)` over `t`.
fn serve(t: Arc<dyn Transport>, label: &'static str) {
    let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
    tokio::spawn(forward::serve(rt, t, target_api(label)));
}

/// `inner` with a paired set that leaves `outsiders` out (an engine no longer paired).
struct Gate {
    inner: Arc<Loop>,
    outsiders: Vec<PeerId>,
}

#[async_trait]
impl Transport for Gate {
    fn peers(&self) -> Vec<PeerId> {
        self.inner.peers().into_iter().filter(|p| !self.outsiders.contains(p)).collect()
    }
    async fn open(&self, peer: &PeerId, alpn: &str) -> anyhow::Result<Box<dyn ByteStream>> {
        self.inner.open(peer, alpn).await
    }
    async fn accept(&self, alpn: &str) -> anyhow::Result<(PeerId, Box<dyn ByteStream>)> {
        self.inner.accept(alpn).await
    }
    fn subscribe(&self) -> broadcast::Receiver<PeerEvent> {
        self.inner.subscribe()
    }
}

#[test]
fn the_allowlist_is_sorted_and_carries_no_secret_values() {
    assert!(SETTINGS_FORWARDABLE.windows(2).all(|w| w[0] < w[1]), "sorted, no duplicates");
    for m in [
        "set_secret", "secret_get", "login", "acp_login", "add_provider", "add_account",
        "mcp_server_get", "mcp_server_rotate_token", "set_external_setting", "sync_providers",
        "apply_providers", "contact_window", "mesh_pair_answer", "engine_call", "shutdown",
    ] {
        assert!(!forwardable(m), "{m} must not be forwardable");
    }
    for m in ["set_model", "config_flag_set", "engine_caps", "list_prompt_sections", "info"] {
        assert!(forwardable(m), "{m}");
    }
}

/// A typo in the list would be a method no engine has: every name is on the accepted wire.
#[test]
fn every_allowlisted_method_is_on_the_wire() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../theuth/tests/fixtures/wire/openrpc-v1.json");
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let names: Vec<&str> =
        doc["methods"].as_array().unwrap().iter().filter_map(|m| m["name"].as_str()).collect();
    let missing: Vec<_> = SETTINGS_FORWARDABLE.iter().filter(|m| !names.contains(m)).collect();
    assert!(missing.is_empty(), "not on the wire: {missing:?}");
}

#[tokio::test]
async fn non_listed_and_secret_methods_are_refused_even_here() {
    let [a] = nodes(&["a"]).try_into().ok().unwrap();
    let (ext, _g) = owner();
    for m in ["set_secret", "login", "shutdown"] {
        let e = call_one(&reach_a(&a), &ext, "a", m, &Value::Null, true).await.unwrap_err();
        assert_eq!(e.code(), NOT_FORWARDABLE, "{m}");
        assert_eq!(e.message(), format!("not forwardable: {m}"));
    }
}

#[tokio::test]
async fn this_engine_dispatches_locally_as_the_caller() {
    let [a] = nodes(&["a"]).try_into().ok().unwrap();
    let (ext, _g) = owner();
    for engine in ["a", ""] {
        let v = call_one(&reach_a(&a), &ext, engine, "set_model", &json!({ "model": "m" }), false)
            .await
            .unwrap();
        assert_eq!(v["on"], "a");
        assert_eq!(v["forwarded"], false);
        assert_eq!(v["params"], json!({ "model": "m" }), "named params pass through");
        assert_eq!(v["press"], true, "the caller's own connection");
    }
    let v = call_one(&reach_a(&a), &agent(), "a", "set_model", &Value::Null, true).await.unwrap();
    assert_eq!(v["press"], false, "an agent's local call is no press");
    assert_eq!(v["params"], json!([]), "null params go out as none");
}

#[tokio::test]
async fn a_peer_that_is_not_connected_is_offline() {
    let [a, _b] = nodes(&["a", "b"]).try_into().ok().unwrap();
    let (ext, _g) = owner();
    let e = call_one(&reach_a(&a), &ext, "c", "set_model", &Value::Null, false).await.unwrap_err();
    assert_eq!(e.code(), OFFLINE);
    let lone = Reach { transport: None, ..reach_a(&a) };
    let e = call_one(&lone, &ext, "b", "set_model", &Value::Null, false).await.unwrap_err();
    assert_eq!(e.code(), OFFLINE, "no mesh at all");
}

#[tokio::test]
async fn an_owner_press_reaches_the_target_as_a_human_press() {
    let [a, b] = nodes(&["a", "b"]).try_into().ok().unwrap();
    serve(b, "b");
    let (ext, _g) = owner();
    let press = press_for(&ext, Some(&token()));
    assert!(press);
    let v = call_one(&reach_a(&a), &ext, "b", "set_model", &json!(["m"]), press).await.unwrap();
    assert_eq!(v["on"], "b");
    assert_eq!(v["forwarded"], true);
    assert_eq!(v["press"], true, "ForwardedPress counts as human_press on the target");
    assert_eq!(v["params"], json!(["m"]));
    // The same owner without `pressed`: a plain forwarded call.
    let v = call_one(&reach_a(&a), &ext, "b", "set_model", &json!(["m"]), press_for(&ext, None))
        .await
        .unwrap();
    assert_eq!(v["press"], false);
}

#[tokio::test]
async fn an_agent_or_internal_call_never_carries_a_press() {
    let [a, b] = nodes(&["a", "b"]).try_into().ok().unwrap();
    serve(b, "b");
    let internal = agent();
    assert!(!press_for(&internal, Some(&token())), "internal peer claiming a press");
    assert!(!press_for(&Extensions::new(), Some(&token())), "unauthenticated");
    let (peer, guard) = RpcPeer::open();
    let mut closed = Extensions::new();
    closed.insert(peer);
    drop(guard);
    assert!(!press_for(&closed, Some(&token())), "closed connection");
    let v = call_one(
        &reach_a(&a),
        &internal,
        "b",
        "set_model",
        &Value::Null,
        press_for(&internal, Some(&token())),
    )
    .await
    .unwrap();
    assert_eq!(v["press"], false);
    // A forwarded request (already one hop) is never forwarded on.
    let mut fwd = internal.clone();
    fwd.insert(forward::Forwarded(PeerId("x".into())));
    let e = call_one(&reach_a(&a), &fwd, "b", "set_model", &Value::Null, true).await.unwrap_err();
    assert_eq!(e.code(), NOT_APPLICABLE);
}

#[tokio::test]
async fn a_forged_press_from_an_unpaired_peer_or_on_another_method_is_ignored() {
    let [a, b] = nodes(&["a", "b"]).try_into().ok().unwrap();
    // b no longer counts a as paired, though the stream still arrives.
    serve(Arc::new(Gate { inner: b, outsiders: vec![PeerId("a".into())] }), "b");
    let (ext, _g) = owner();
    let v = call_one(&reach_a(&a), &ext, "b", "set_model", &Value::Null, true).await.unwrap();
    assert_eq!(v["on"], "b");
    assert_eq!(v["press"], false, "forged _press from a non-member");

    let [a, b] = nodes(&["a", "b"]).try_into().ok().unwrap();
    serve(b.clone(), "b");
    // A paired peer setting `_press` on a method outside the allowlist.
    let params = serde_json::value::to_raw_value(&Value::Array(vec![])).unwrap();
    let raw = forward::forward_line(a.as_ref(), &b.me, "shutdown", &params, forward::TIMEOUT, true)
        .await
        .unwrap();
    let v: Value = serde_json::from_str(raw.get()).unwrap();
    assert_eq!(v["press"], false, "press only on allowlisted methods");

    let line = |m: &str| format!(r#"{{"jsonrpc":"2.0","id":1,"method":"{m}","params":[],"_press":true}}"#);
    let members = [PeerId("a".into())];
    assert!(forward::honoured_press(&line("set_model"), &PeerId("a".into()), &members));
    assert!(!forward::honoured_press(&line("set_model"), &PeerId("z".into()), &members));
    assert!(!forward::honoured_press(&line("shutdown"), &PeerId("a".into()), &members));
    assert!(!forward::honoured_press(
        r#"{"jsonrpc":"2.0","id":1,"method":"set_model","params":{"_press":true}}"#,
        &PeerId("a".into()),
        &members
    ), "a `_press` inside params is not the line's");
}

#[test]
fn sealed_errors_map_to_the_sealed_code() {
    let e = classify(ErrorObjectOwned::owned(SEALED, "anything", None::<()>));
    assert_eq!((e.code(), e.message()), (SEALED, "anything"), "-32052 passes unchanged");
    let e = classify(rpc_core::failed(SEALED_MSG));
    assert_eq!((e.code(), e.message()), (SEALED, SEALED_MSG), "the branch's text, older code");
    assert_eq!(classify(rpc_core::failed("the vault is locked")).code(), SEALED);
    assert_eq!(classify(rpc_core::failed("no such model")).code(), rpc_core::ASK_FAILED);
    assert_eq!(classify(ErrorObjectOwned::owned(-32601, "x", None::<()>)).code(), NOT_APPLICABLE);
    assert_eq!(
        classify(ErrorObjectOwned::owned(rpc_core::NOT_RUNNING, "voice not running", None::<()>))
            .code(),
        NOT_APPLICABLE
    );
    assert_eq!(
        classify(ErrorObjectOwned::owned(forward::OWNER_UNREACHABLE, "gone", None::<()>)).code(),
        OFFLINE
    );
}

#[tokio::test]
async fn a_peers_sealed_and_missing_methods_come_back_classified() {
    let [a, b] = nodes(&["a", "b"]).try_into().ok().unwrap();
    serve(b, "b");
    let (ext, _g) = owner();
    let r = reach_a(&a);
    let e = call_one(&r, &ext, "b", "set_approvals", &Value::Null, false).await.unwrap_err();
    assert_eq!((e.code(), e.message()), (SEALED, SEALED_MSG), "message prefix → -32052");
    let e = call_one(&r, &ext, "b", "list_prompt_sections", &Value::Null, false).await.unwrap_err();
    assert_eq!(e.code(), SEALED, "-32052 from the peer unchanged");
    let e = call_one(&r, &ext, "b", "voice_status", &Value::Null, false).await.unwrap_err();
    assert_eq!(e.code(), NOT_APPLICABLE, "not registered there (cfg'd out, plugin off)");
}

#[tokio::test]
async fn call_all_aggregates_mixed_results_with_restart_required() {
    let [a, b, d] = nodes(&["a", "b", "d"]).try_into().ok().unwrap();
    serve(b, "b");
    serve(d, "d");
    let (ext, _g) = owner();
    let targets: Vec<(String, String)> = [("a", "Mac"), ("b", "P50"), ("c", "Cloud"), ("d", "Mini")]
        .iter()
        .map(|(e, n)| ((*e).to_owned(), (*n).to_owned()))
        .collect();
    let params = json!({ "key": "decision.enabled", "on": true });
    let rows = call_all(&reach_a(&a), &ext, &targets, "config_flag_set", &params, true).await;
    let got: Vec<_> = rows
        .iter()
        .map(|r| (r.engine.as_str(), r.name.as_str(), r.status, r.restart_required))
        .collect();
    assert_eq!(
        got,
        vec![
            ("a", "Mac", NodeStatus::Ok, false),
            ("b", "P50", NodeStatus::Ok, true),
            ("c", "Cloud", NodeStatus::Offline, false),
            ("d", "Mini", NodeStatus::Sealed, false),
        ]
    );
    assert_eq!(rows[3].message.as_deref(), Some(SEALED_MSG));
    assert!(rows[0].value.is_some() && rows[2].value.is_none());
    let wire = serde_json::to_value(&rows[2]).unwrap();
    assert_eq!(wire["status"], "offline");
    assert!(wire.get("value").is_none());
    assert_eq!(serde_json::to_value(NodeStatus::NotApplicable).unwrap(), "not_applicable");
}

#[tokio::test]
async fn press_gated_setters_without_a_press_are_needs_press() {
    let [a, b] = nodes(&["a", "b"]).try_into().ok().unwrap();
    serve(b, "b");
    let (ext, _g) = owner();
    let r = reach_a(&a);
    // A queued retry: the owner's connection, no `pressed`.
    let e = call_one(&r, &ext, "b", "plugin_approve", &json!(["p"]), false).await.unwrap_err();
    assert_eq!(e.code(), NEEDS_PRESS);
    let off = json!({ "session": null, "tool": "shell", "on": false });
    let e = call_one(&r, &ext, "b", "set_always_ask", &off, false).await.unwrap_err();
    assert_eq!(e.code(), NEEDS_PRESS, "an always-ask removal would park a card");
    let e = call_one(&r, &agent(), "a", "plugin_approve", &Value::Null, true).await.unwrap_err();
    assert_eq!(e.code(), NEEDS_PRESS, "here, an agent's own connection is no press");
    // Pressed: it goes through (not registered on the fake target).
    let e = call_one(&r, &ext, "b", "plugin_approve", &json!(["p"]), true).await.unwrap_err();
    assert_eq!(e.code(), NOT_APPLICABLE);
    assert!(!needs_press("set_always_ask", &json!([null, "shell", true])));
    let e = classify(rpc_core::failed(
        "plugin_approve needs a human press: the Approve button in theuth or `t plugin approve` on a terminal",
    ));
    assert_eq!(e.code(), NEEDS_PRESS, "the target's own refusal");
    let rows = call_all(&r, &ext, &[("b".into(), "P50".into())], "plugin_approve", &Value::Null, false).await;
    assert_eq!(rows[0].status, NodeStatus::NeedsPress);
    assert_eq!(serde_json::to_value(NodeStatus::NeedsPress).unwrap(), "needs_press");
}

#[test]
fn restart_required_follows_the_reply_and_subsystem_keys() {
    let none = json!({});
    assert!(restart_required("config_flag_set", &json!({ "key": "subsystems.meeting", "on": false }), &none));
    assert!(restart_required("config_flag_set", &json!(["subsystems.voice", true]), &none));
    assert!(!restart_required("config_flag_set", &json!(["agent.auto_title", true]), &none));
    assert!(restart_required("plugin_set_enabled", &none, &json!({ "restart_required": true })));
    assert!(!restart_required("set_model", &json!(["subsystems.x"]), &none));
}

#[test]
fn settings_values_omits_offline_engines_and_nulls_failures() {
    let row = |engine: &str, status, value: Option<Value>| NodeResult {
        engine: engine.into(),
        name: engine.into(),
        status,
        message: None,
        restart_required: false,
        value,
    };
    let keys = vec!["subagent_model".to_owned(), "title_model".to_owned()];
    let rows = vec![
        vec![
            row("a", NodeStatus::Ok, Some(json!("opus"))),
            row("c", NodeStatus::Offline, None),
        ],
        vec![
            row("a", NodeStatus::NotApplicable, None),
            row("c", NodeStatus::Offline, None),
        ],
    ];
    assert_eq!(
        serde_json::to_value(values_of(&keys, rows)).unwrap(),
        json!({ "a": { "subagent_model": "opus", "title_model": null } })
    );
}

#[test]
fn engine_caps_shape() {
    let node = HostCaps { interactive: false, display: false, audio: false };
    let caps = caps_of("n1".into(), "cloud-1".into(), node, false, None, false);
    let v = serde_json::to_value(&caps).unwrap();
    let mut keys: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    assert_eq!(
        keys,
        ["display", "engine", "headless", "hub", "interactive_user", "mic", "name", "platform"]
    );
    assert_eq!(v["platform"], std::env::consts::OS);
    assert_eq!(
        (caps.display, caps.mic, caps.interactive_user, caps.headless),
        (false, false, false, true),
        "a headless cloud node without a desktop"
    );
    let with_desktop = caps_of("n1".into(), "cloud-1".into(), node, true, Some(false), true);
    assert!(with_desktop.display && with_desktop.hub, "the watchable desktop counts");
    let mac = HostCaps { interactive: true, display: true, audio: true };
    let caps = caps_of("m".into(), "Mac".into(), mac, false, Some(false), false);
    assert!(caps.interactive_user && !caps.headless && !caps.mic, "the device listing wins");
}
