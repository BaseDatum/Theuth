//! Compiled transport/view checks over the production declaration-only composition.
use super::*;
use jsonrpsee::{Methods, core::client::ClientT, rpc_params};
use rpc_core::{DiscoveryView, RpcPeer};
use serde_json::{Value, json};

async fn composition() -> (Runtime, Methods) {
    let rt = Runtime::new(Figment::new(), std::env::temp_dir());
    declare_plugins(&rt, &mut engine_plugins(true, true)).await.unwrap();
    let methods = rpc_module(rpc_core::Server::new(rt.clone())).into();
    (rt, methods)
}

async fn reply(methods: &Methods, name: &str) -> Value {
    let request = json!({"jsonrpc":"2.0", "id":1, "method":name, "params":[]});
    let (reply, _) = methods.raw_json_request(&request.to_string(), 1).await.unwrap();
    serde_json::from_str(reply.get()).unwrap()
}

fn names(document: &Value) -> std::collections::BTreeSet<&str> {
    document["methods"].as_array().unwrap().iter().map(|method| method["name"].as_str().unwrap()).collect()
}

#[tokio::test]
async fn node_client_methods_exist_in_production_catalog() {
    let (_, methods) = composition().await;
    let names: std::collections::BTreeSet<_> = methods.method_names().collect();
    for method in nodes::client::METHODS {
        assert!(names.contains(method), "{method} missing from production RPC catalog");
    }
}

#[tokio::test]
async fn real_composition_discovery_is_cached_per_authenticated_view() {
    let (rt, methods) = composition().await;
    assert_eq!(reply(&methods, "rpc.discover").await["error"]["code"], rpc_core::UNAUTHORIZED);
    let mut owner = methods.clone();
    owner.extensions_mut().insert(RpcPeer::internal());
    let owner_document = reply(&owner, "rpc.discover").await["result"].clone();
    let owner_summary = reply(&owner, "rpc.capabilities").await["result"].clone();
    assert_eq!(owner_summary["schemaHash"], rpc_core::document_hash(&owner_document).unwrap());
    assert!(names(&owner_document).contains("new_session"));
    for name in ["peer_deliver", "a2a_forward", "link_mesh_id"] { assert!(!names(&owner_document).contains(name)); }
    let mut reconnect = methods.clone();
    let (peer, _guard) = RpcPeer::open_remote();
    reconnect.extensions_mut().insert(peer);
    assert_eq!(owner_summary, reply(&reconnect, "rpc.capabilities").await["result"]);
    assert_eq!(owner_document, reply(&reconnect, "rpc.discover").await["result"]);
    let first = rpc_core::discovery_document(&rt, DiscoveryView::Owner).unwrap();
    let next = rpc_core::discovery_document(&rt, DiscoveryView::Owner).unwrap();
    assert!(std::sync::Arc::ptr_eq(&first.document.0, &next.document.0));

    let mut guest = methods.clone();
    let (peer, _guest_guard) = RpcPeer::open_guest([1; 32], [2; 32]);
    guest.extensions_mut().insert(peer);
    let guest_document = reply(&guest, "rpc.discover").await["result"].clone();
    assert_eq!(names(&guest_document), rpc_core::GUEST_METHODS.into_iter().collect());
    assert_eq!(guest_document["x-theuth"]["events"], json!([]));
    // origin/main admits notebook_get/notebook_items_list/source_get/cite_resolve
    // (now agent-owned) and cell_latest; owner names are not capability grants.
    assert_eq!(guest_document["x-theuth"]["plugins"], json!(["agent", "cells", "document", "rpc"]));
    assert!(!guest_document["components"]["schemas"].as_object().unwrap().keys()
        .any(|name| name.starts_with("event.") || name.ends_with(".SessionSummary")));
    let guest_summary = reply(&guest, "rpc.capabilities").await["result"].clone();
    assert_eq!(guest_summary["schemaHash"], rpc_core::document_hash(&guest_document).unwrap());
    assert_ne!(guest_summary["schemaHash"], owner_summary["schemaHash"]);

    let mut mesh = owner;
    mesh.extensions_mut().insert(DiscoveryView::Mesh);
    let mesh_document = reply(&mesh, "rpc.discover").await["result"].clone();
    for name in ["peer_deliver", "a2a_forward", "link_mesh_id"] { assert!(names(&mesh_document).contains(name)); }
    assert_ne!(reply(&mesh, "rpc.capabilities").await["result"]["schemaHash"], owner_summary["schemaHash"]);
}

/// The RPCs a phone's ⋯ Share with contact…, Stop sharing and Settings › Contacts call.
const SHARE_METHODS: [&str; 6] = ["contact_list", "contact_update", "contact_remove", "a2a_exports", "a2a_export_put", "a2a_export_remove"];

/// 183 §12.10, lifted 2026-11-17: a paired device's link conn carries a remote peer and no
/// `DiscoveryView::Mesh`, so it negotiates the owner contract, which names every share RPC, and the
/// real composition answers it with no audience or origin check (no a2a actor here: the empty lists).
/// A share-link guest's contract names none of them, and the link gate refuses each before dispatch.
#[tokio::test]
async fn a_paired_device_negotiates_and_calls_the_share_rpcs_and_a_guest_is_refused_them() {
    let (_, methods) = composition().await;
    let mut device = methods.clone();
    let (peer, _guard) = RpcPeer::open_remote();
    device.extensions_mut().insert(peer);
    let owner = reply(&device, "rpc.discover").await["result"].clone();
    for name in SHARE_METHODS { assert!(names(&owner).contains(name), "{name} missing from the device contract"); }
    for name in ["contact_list", "a2a_exports"] { assert_eq!(reply(&device, name).await["result"], json!([]), "{name}"); }

    let mut guest = methods;
    let (peer, _guest_guard) = RpcPeer::open_guest([1; 32], [2; 32]);
    guest.extensions_mut().insert(peer);
    let guest_document = reply(&guest, "rpc.discover").await["result"].clone();
    for name in SHARE_METHODS {
        assert!(!names(&guest_document).contains(name), "{name} offered to a guest");
        let request = json!({"jsonrpc":"2.0", "id":1, "method":name, "params":[]}).to_string();
        let refused = link::conn::guest_gate(request.as_bytes()).unwrap_or_else(|| panic!("{name} passed the guest gate"));
        assert_eq!(serde_json::from_str::<Value>(refused.get()).unwrap()["error"]["code"], -32601, "{name}");
    }
}

#[tokio::test]
async fn authenticated_websocket_serves_the_same_frozen_owner_contract() {
    let (rt, methods) = composition().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let shutdown = rt.shutdown.clone();
    let server = tokio::spawn(rpc::accept(listener, methods, "wire-test-token".into(), async move { shutdown.cancelled().await }));
    let mut headers = jsonrpsee::ws_client::HeaderMap::new();
    headers.insert("Authorization", jsonrpsee::ws_client::HeaderValue::from_static("Bearer wire-test-token"));
    let client = jsonrpsee::ws_client::WsClientBuilder::default().set_headers(headers)
        .max_response_size(64 << 20).build(format!("ws://{address}")).await.unwrap();
    let mut cache = rpc_core::DiscoveryCache::default();
    let negotiated = cache.negotiate(&client, &address.to_string(), DiscoveryView::Owner).await.unwrap().unwrap();
    let document: Value = client.request("rpc.discover", rpc_params![]).await.unwrap();
    assert_eq!(negotiated.capabilities.schema_hash, rpc_core::document_hash(&document).unwrap());
    assert!(names(&document).contains("rpc.capabilities"));
    assert!(names(&document).contains("rpc.discover"));
    assert!(!names(&document).contains("peer_deliver"));
    drop(client);
    rt.shutdown.cancel();
    server.await.unwrap();
}
