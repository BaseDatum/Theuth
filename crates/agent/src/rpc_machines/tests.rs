use super::*;
use crate::communication::mesh::loopback::{Loop, nodes};
use rpc_core::{RpcPeer, RpcPeerGuard};
use serde_json::json;

const SEALED_MSG: &str = "the vault is sealed on P50 — unlock it from an owner device or with the master password";

/// A target engine's API: `set_model` reports how it was called; `config_flag_set` answers
/// `restart_required` on `b` and a sealed vault on `d`; `list_prompt_sections` the sealed
/// code; `plugin_approve` refuses without a press like the plugins crate.
fn target_api(label: &'static str) -> jsonrpsee::Methods {
    let mut m = jsonrpsee::RpcModule::new(());
    m.register_method("set_model", move |params, _, ext| -> RpcResult<Value> {
        Ok(json!({
            "on": label,
            "press": secrets::rpc::human_press(ext),
            "forwarded": ext.get::<forward::Forwarded>().is_some(),
            "params": params.parse::<Value>().unwrap_or(Value::Null),
        }))
    })
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
    m.register_method("plugin_approve", |_, _, ext| -> RpcResult<Value> {
        if secrets::rpc::human_press(ext) {
            Ok(json!("approved"))
        } else {
            Err(rpc_core::failed(
                "plugin_approve needs a human press: the Approve button in theuth or `t plugin approve` on a terminal",
            ))
        }
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

/// `a`'s reach over `t`: its own API locally.
fn reach_a(t: &Arc<Loop>) -> Reach {
    Reach {
        me: vec!["a".into()],
        local: Some(target_api("a")),
        transport: Some(t.clone() as Arc<dyn Transport>),
    }
}

/// `t` answers `theuth/rpc/1` with `target_api(label)`.
fn serve(t: Arc<dyn Transport>, label: &'static str) {
    let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
    tokio::spawn(forward::serve(rt, t, target_api(label)));
}

fn roster() -> Roster {
    Roster {
        me: vec!["a".into(), "store-a".into()],
        name: "Mac".into(),
        peers: vec![("b".into(), "P50".into()), ("c".into(), "Cloud".into())],
    }
}

#[test]
fn the_allowlist_is_sorted_and_carries_no_secret_values() {
    assert!(SETTINGS_FORWARDABLE.windows(2).all(|w| w[0] < w[1]), "sorted, no duplicates");
    for m in [
        "set_secret", "secret_get", "login", "acp_login", "add_provider", "add_account",
        "mcp_server_get", "mcp_server_set", "mcp_server_rotate_token", "set_external_setting",
        "get_external_settings", "sync_providers", "apply_providers", "contact_window",
        "mesh_pair_answer", "engine_call", "shutdown", "send_input", "steer", "desk_open",
        "sandbox_terminal", "sandbox_create", "dictation_audio_get", "meeting_get",
        "list_sessions", "vault_rotate", "mesh_set_hub", "mesh_set_nearby", "mesh_rename",
        "contact_remove", "logout", "calendar_sources", "calendar_source_add", "plugin_new",
        "plugin_uninstall",
    ] {
        assert!(!forwardable(m), "{m} must not be forwardable");
    }
    for m in ["set_model", "config_flag_set", "engine_caps", "list_prompt_sections", "info"] {
        assert!(forwardable(m), "{m}");
    }
}

/// 227 §2 review: no settings call reaching another machine is a session action, carries
/// content, a credential or controls the mesh/vault.
#[test]
fn no_allowlisted_name_matches_the_deny_patterns() {
    let hits: Vec<_> = SETTINGS_FORWARDABLE
        .iter()
        .filter(|m| NEVER_FORWARDABLE.iter().any(|p| m.contains(p)))
        .collect();
    assert!(hits.is_empty(), "denied names on the allowlist: {hits:?}");
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
    for m in ["set_secret", "login", "shutdown", "mcp_server_set"] {
        let e = call_one(&reach_a(&a), &ext, "a", m, &Value::Null).await.unwrap_err();
        assert_eq!(e.code(), NOT_FORWARDABLE, "{m}");
        assert_eq!(e.message(), format!("not forwardable: {m}"));
    }
}

#[test]
fn agents_and_guests_may_not_use_the_machine_rpcs() {
    let (ext, _g) = owner();
    assert!(owner_app(&ext, false).is_ok());
    let e = owner_app(&agent(), false).unwrap_err();
    assert_eq!(e.code(), rpc_core::UNAUTHORIZED);
    assert!(owner_app(&agent(), true).is_err(), "internal, not forwarded");
    assert!(owner_app(&Extensions::new(), true).is_err(), "unauthenticated");
    let (guest, _gg) = RpcPeer::open_guest([1; 32], [2; 32]);
    let mut g = Extensions::new();
    g.insert(guest);
    assert!(owner_app(&g, true).is_err(), "share guest");
    let mut fwd = agent();
    fwd.insert(forward::Forwarded(PeerId("hub".into())));
    assert!(owner_app(&fwd, true).is_ok(), "engine_caps for another engine's app");
    assert!(owner_app(&fwd, false).is_err(), "never engine_call / _all / settings_values");
}

#[test]
fn engines_are_deduplicated_known_and_capped() {
    let r = roster();
    let all = r.targets(None).unwrap();
    assert_eq!(all.iter().map(|(e, _)| e.as_str()).collect::<Vec<_>>(), ["a", "b", "c"]);
    let got = r.targets(Some(vec!["b".into(), "store-a".into(), "b".into(), "".into()])).unwrap();
    let got: Vec<(&str, &str)> = got.iter().map(|(e, n)| (e.as_str(), n.as_str())).collect();
    assert_eq!(got, [("b", "P50"), ("store-a", "Mac"), ("", "Mac")]);
    let e = r.targets(Some(vec!["b".into(), "zz".into()])).unwrap_err();
    assert_eq!((e.code(), e.message()), (INVALID_PARAMS_CODE, "unknown engine zz"));
    assert!(r.target("zz").is_err() && r.target("c").is_ok() && r.target("").is_ok());
    let many = Roster {
        peers: (0..MAX_ENGINES).map(|i| (format!("p{i}"), format!("P{i}"))).collect(),
        ..roster()
    };
    assert_eq!(many.targets(None).unwrap_err().code(), INVALID_PARAMS_CODE, "self + 32 peers");
    let k = getter_keys(vec!["info".into(), "info".into(), "title_model".into()]).unwrap();
    assert_eq!(k, ["info", "title_model"]);
    assert_eq!(getter_keys(vec!["set_secret".into()]).unwrap_err().code(), NOT_FORWARDABLE);
    let keys: Vec<String> = SETTINGS_FORWARDABLE.iter().take(MAX_KEYS + 1).map(|s| (*s).to_owned()).collect();
    assert_eq!(getter_keys(keys).unwrap_err().code(), INVALID_PARAMS_CODE);
}

#[tokio::test]
async fn this_engine_dispatches_locally_as_the_caller() {
    let [a] = nodes(&["a"]).try_into().ok().unwrap();
    let (ext, _g) = owner();
    for engine in ["a", ""] {
        let v = call_one(&reach_a(&a), &ext, engine, "set_model", &json!({ "model": "m" }))
            .await
            .unwrap();
        assert_eq!(v["on"], "a");
        assert_eq!(v["forwarded"], false);
        assert_eq!(v["params"], json!({ "model": "m" }), "named params pass through");
        assert_eq!(v["press"], true, "the caller's own connection");
    }
    let v = call_one(&reach_a(&a), &ext, "a", "set_model", &Value::Null).await.unwrap();
    assert_eq!(v["params"], json!([]), "null params go out as none");
}

#[tokio::test]
async fn a_peer_that_is_not_connected_is_offline() {
    let [a, _b] = nodes(&["a", "b"]).try_into().ok().unwrap();
    let (ext, _g) = owner();
    let e = call_one(&reach_a(&a), &ext, "c", "set_model", &Value::Null).await.unwrap_err();
    assert_eq!(e.code(), OFFLINE);
    let lone = Reach { transport: None, ..reach_a(&a) };
    let e = call_one(&lone, &ext, "b", "set_model", &Value::Null).await.unwrap_err();
    assert_eq!(e.code(), OFFLINE, "no mesh at all");
}

/// 227 §2 review: no press crosses the mesh. The owner's own press on the hub reaches the
/// target as the internal peer, which is never a human press; a forwarded call is not
/// forwarded again.
#[tokio::test]
async fn a_forwarded_call_is_never_a_press() {
    let [a, b] = nodes(&["a", "b"]).try_into().ok().unwrap();
    serve(b, "b");
    let (ext, _g) = owner();
    let v = call_one(&reach_a(&a), &ext, "b", "set_model", &json!(["m"])).await.unwrap();
    assert_eq!(v["on"], "b");
    assert_eq!(v["forwarded"], true);
    assert_eq!(v["press"], false, "forwarded: never a human press");
    assert_eq!(v["params"], json!(["m"]));
    let mut fwd = agent();
    fwd.insert(forward::Forwarded(PeerId("x".into())));
    let e = call_one(&reach_a(&a), &fwd, "b", "set_model", &Value::Null).await.unwrap_err();
    assert_eq!(e.code(), NOT_APPLICABLE, "one hop only");
}

fn b_id() -> PeerId {
    PeerId("b".into())
}

/// A paired engine writing `"_press": true` into the request line gets no press.
#[tokio::test]
async fn a_forged_press_field_is_ignored() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let [a, b] = nodes(&["a", "b"]).try_into().ok().unwrap();
    serve(b, "b");
    let stream = a.open(&b_id(), replica::ALPN_RPC).await.unwrap();
    let (read, mut write) = tokio::io::split(stream);
    write
        .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"set_model\",\"params\":[],\"_press\":true}\n")
        .await
        .unwrap();
    let mut reply = String::new();
    BufReader::new(read).read_line(&mut reply).await.unwrap();
    let v: Value = serde_json::from_str(&reply).unwrap();
    assert_eq!(v["result"]["press"], false, "{reply}");
}

#[tokio::test]
async fn press_gated_setters_are_needs_press_on_another_machine() {
    let [a, b] = nodes(&["a", "b"]).try_into().ok().unwrap();
    serve(b, "b");
    let (ext, _g) = owner();
    let r = reach_a(&a);
    let e = call_one(&r, &ext, "b", "plugin_approve", &json!(["p", "h"])).await.unwrap_err();
    assert_eq!(e.code(), NEEDS_PRESS, "even the owner's press does not cross");
    let off = json!({ "session": null, "tool": "shell", "on": false });
    let e = call_one(&r, &ext, "b", "set_always_ask", &off).await.unwrap_err();
    assert_eq!(e.code(), NEEDS_PRESS, "an always-ask removal would park a card");
    assert!(!needs_press("set_always_ask", &json!([null, "shell", true])));
    // Here, the owner's own connection is the press.
    assert_eq!(call_one(&r, &ext, "a", "plugin_approve", &json!(["p", "h"])).await.unwrap(), "approved");
    let e = call_one(&r, &agent(), "a", "plugin_approve", &Value::Null).await.unwrap_err();
    assert_eq!(e.code(), NEEDS_PRESS, "an internal caller is no press here either");
    // The target's own press refusal maps the same way.
    let e = classify(rpc_core::failed(
        "plugin_approve needs a human press: the Approve button in theuth or `t plugin approve` on a terminal",
    ));
    assert_eq!(e.code(), NEEDS_PRESS);
    assert_eq!(
        classify(rpc_core::failed("copying providers to another engine needs the owner's press")).code(),
        NEEDS_PRESS
    );
    let rows = call_all(&r, &ext, &[("b".into(), "P50".into())], "plugin_approve", &Value::Null).await;
    assert_eq!(rows[0].status, NodeStatus::NeedsPress);
    assert_eq!(serde_json::to_value(NodeStatus::NeedsPress).unwrap(), "needs_press");
}

#[test]
fn sealed_errors_map_to_the_sealed_code() {
    let e = classify(ErrorObjectOwned::owned(SEALED, "anything", None::<()>));
    assert_eq!((e.code(), e.message()), (SEALED, "anything"), "-32052 passes unchanged");
    let e = classify(rpc_core::failed(SEALED_MSG));
    assert_eq!((e.code(), e.message()), (SEALED, SEALED_MSG), "the sealed-vault text, any code");
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
    let e = call_one(&r, &ext, "b", "set_approvals", &Value::Null).await.unwrap_err();
    assert_eq!((e.code(), e.message()), (SEALED, SEALED_MSG), "message prefix → -32052");
    let e = call_one(&r, &ext, "b", "list_prompt_sections", &Value::Null).await.unwrap_err();
    assert_eq!(e.code(), SEALED, "-32052 from the peer unchanged");
    let e = call_one(&r, &ext, "b", "voice_status", &Value::Null).await.unwrap_err();
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
    let rows = call_all(&reach_a(&a), &ext, &targets, "config_flag_set", &params).await;
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

#[test]
fn restart_required_is_the_replys_own() {
    assert!(restart_required(&json!({ "restart_required": true, "key": "a2a.enabled" })));
    assert!(!restart_required(&json!({ "restart_required": false })));
    assert!(!restart_required(&json!({ "enabled": true, "name": "p" })), "a plugin toggles live");
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
    assert_eq!(host_caps(), HostCaps::default(), "no probe: nothing assumed");
    let node = HostCaps::default();
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
