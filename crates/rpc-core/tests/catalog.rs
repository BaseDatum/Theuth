//! Inert projection/hash/freeze boundaries; no actor producers or listeners.

use rpc_core::{Audience, Descriptors, Discovery, DiscoveryDocument, DiscoveryView, MethodDescriptor, schema_for};
use serde_json::{Value, json};

fn discovery() -> Discovery {
    let method = |name, audience, result| MethodDescriptor {
        owner: "document", name, aliases: vec![], params: vec![], result, audience,
    };
    Discovery {
        descriptors: Descriptors { methods: vec![
            method("doc_open", Audience::Owner, schema_for::<String>),
            method("owner_secret", Audience::Owner, schema_for::<Vec<u64>>),
            method("peer_deliver", Audience::Mesh, schema_for::<bool>),
        ], ..Default::default() },
        events: vec![wire::EventDescriptor { topic: "global.events", version: 1, schema: schema_for::<Vec<String>> }],
        plugins: vec!["document".into(), "hidden-owner".into()],
    }
}

fn value(document: &DiscoveryDocument) -> Value { serde_json::from_str(document.document.0.get()).unwrap() }
fn names(document: &Value) -> Vec<&str> { document["methods"].as_array().unwrap().iter().map(|m| m["name"].as_str().unwrap()).collect() }

#[test]
fn same_view_has_same_document_and_hash_independent_of_build_and_declaration_order() {
    let first = DiscoveryDocument::new(&discovery(), DiscoveryView::Owner, "build-a").unwrap();
    let mut reordered = discovery();
    reordered.descriptors.methods.reverse();
    reordered.plugins.reverse();
    let second = DiscoveryDocument::new(&reordered, DiscoveryView::Owner, "build-b").unwrap();
    assert_eq!(first.document.0.get(), second.document.0.get());
    assert_eq!(first.capabilities.schema_hash, second.capabilities.schema_hash);
    assert_ne!(first.capabilities.server_version, second.capabilities.server_version);
    assert_eq!(first.capabilities.schema_hash, rpc_core::document_hash(&value(&first)).unwrap());
}

#[test]
fn guest_projection_excludes_owner_methods_globals_and_unreachable_schemas() {
    let guest = DiscoveryDocument::new(&discovery(), DiscoveryView::Guest, "build").unwrap();
    let document = value(&guest);
    assert_eq!(names(&document), ["doc_open"]);
    assert_eq!(document["x-theuth"]["events"], json!([]));
    assert_eq!(document["x-theuth"]["plugins"], json!(["document"]));
    assert!(!guest.document.0.get().contains("global.events"));
    assert!(!guest.document.0.get().contains("owner_secret"));
    assert!(!guest.document.0.get().contains("hidden-owner"));
    let owner = DiscoveryDocument::new(&discovery(), DiscoveryView::Owner, "build").unwrap();
    assert_ne!(owner.capabilities.schema_hash, guest.capabilities.schema_hash);
}

#[test]
fn mesh_view_contains_mesh_only_methods_owner_does_not() {
    let owner = DiscoveryDocument::new(&discovery(), DiscoveryView::Owner, "build").unwrap();
    let mesh = DiscoveryDocument::new(&discovery(), DiscoveryView::Mesh, "build").unwrap();
    assert_eq!(names(&value(&owner)), ["doc_open", "owner_secret"]);
    assert_eq!(names(&value(&mesh)), ["doc_open", "owner_secret", "peer_deliver"]);
    assert_ne!(owner.capabilities.schema_hash, mesh.capabilities.schema_hash);
}

#[tokio::test]
async fn late_rpc_declaration_cannot_replace_a_frozen_document() {
    let rt = runtime::Runtime::new(Default::default(), std::env::temp_dir());
    rpc_core::freeze_discovery(&rt, &discovery(), "build").unwrap();
    let first = rpc_core::discovery_document(&rt, DiscoveryView::Owner).unwrap();
    let mut changed = discovery();
    changed.descriptors.methods.pop();
    assert!(rpc_core::freeze_discovery(&rt, &changed, "new").unwrap_err().to_string().contains("frozen"));
    let after = rpc_core::discovery_document(&rt, DiscoveryView::Owner).unwrap();
    assert!(std::sync::Arc::ptr_eq(&first.document.0, &after.document.0));
    assert!(std::sync::Arc::ptr_eq(&first.capabilities, &after.capabilities));
}
