//! Frozen, audience-specific discovery data shared by every transport.

use std::sync::{Arc, OnceLock};
use anyhow::{Result, ensure};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, value::RawValue};
use sha2::{Digest, Sha256};
use crate::{Audience, Discovery};

/// The document-member methods admitted by the mode-2 transport, plus discovery.
/// Membership/role checks remain in the document owner; discovery grants nothing.
pub const GUEST_METHODS: [&str; 14] = [
    "doc_join", "doc_open", "doc_subscribe", "doc_unsubscribe", "doc_update",
    "doc_awareness", "doc_set_name", "notebook_get", "notebook_items_list",
    "source_get", "cite_resolve", "cell_latest", "rpc.capabilities", "rpc.discover",
];

/// Authenticated visibility, independent of admission and live readiness.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DiscoveryView {
    /// Owner console or linked device: no mesh-internal methods.
    Owner,
    /// Share-link guest: only document-member methods, no global event catalog.
    Guest,
    /// Authenticated paired engine: owner methods and mesh-internal methods.
    Mesh,
}

/// RPC/envelope compatibility family, not engine or storage schema version.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ProtocolVersion {
    /// Incompatible protocol family.
    pub major: u32,
    /// Additive protocol revision.
    pub minor: u32,
}

/// A declared plugin; does not assert that its actors are enabled or ready.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct DeclaredPlugin {
    /// Stable plugin name.
    pub name: String,
}

/// Cheap summary fetched on every authenticated connection, before cache reuse.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    /// Engine build version, deliberately outside the hashed document.
    pub server_version: String,
    /// RPC/envelope compatibility family.
    pub protocol_version: ProtocolVersion,
    /// SHA-256 of canonical compact UTF-8 JSON for this audience's document.
    pub schema_hash: String,
    /// Declared composition, not service readiness.
    pub plugins: Vec<DeclaredPlugin>,
}

/// Pre-serialized OpenRPC document. Cloning a reply only clones its shared handle.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct OpenRpcDocument(#[schemars(with = "Value")] pub Arc<RawValue>);

/// One immutable audience cache entry.
#[derive(Clone, Debug)]
pub struct DiscoveryDocument {
    /// Direct `rpc.discover` response, serialized once at declaration freeze.
    pub document: OpenRpcDocument,
    /// Matching summary; its hash excludes the engine build version.
    pub capabilities: Arc<Capabilities>,
}

/// Canonical whole-document bytes; array order (params, tuples, enums) is retained.
pub fn canonical_document(document: &Value) -> Result<Vec<u8>> {
    fn sorted(value: &mut Value) {
        match value {
            Value::Object(map) => { map.sort_keys(); for child in map.values_mut() { sorted(child); } }
            Value::Array(array) => { for child in array { sorted(child); } }
            _ => {}
        }
    }
    let mut document = document.clone();
    sorted(&mut document);
    Ok(serde_json::to_vec(&document)?)
}

/// Content identity shared by the exporter, server and receiving cache.
pub fn document_hash(document: &Value) -> Result<String> {
    Ok(hex::encode(Sha256::digest(canonical_document(document)?)))
}

impl Discovery {
    /// Project before schema generation, so hidden roots/definitions cannot leak.
    pub fn document_for(&self, view: DiscoveryView) -> Result<Value> {
        let visible = |name: &str, audience| match view {
            DiscoveryView::Owner => audience == Audience::Owner,
            DiscoveryView::Mesh => true,
            DiscoveryView::Guest => GUEST_METHODS.contains(&name),
        };
        let mut projected = self.clone();
        projected.descriptors.methods.retain(|m| visible(m.name, m.audience));
        projected.descriptors.subscriptions.retain(|s| visible(s.subscribe, s.audience) && visible(s.unsubscribe, s.audience));
        if view == DiscoveryView::Guest {
            for method in &mut projected.descriptors.methods { method.aliases.retain(|name| GUEST_METHODS.contains(name)); }
            for stream in &mut projected.descriptors.subscriptions {
                stream.aliases.retain(|name| GUEST_METHODS.contains(name));
                stream.unsubscribe_aliases.retain(|name| GUEST_METHODS.contains(name));
            }
            projected.events.clear();
            let owners: std::collections::BTreeSet<_> = projected.descriptors.methods.iter().map(|m| m.owner)
                .chain(projected.descriptors.subscriptions.iter().map(|s| s.owner)).collect();
            projected.plugins.retain(|name| owners.contains(name.as_str()));
        }
        projected.document()
    }
}

impl DiscoveryDocument {
    /// Generate once from inert declarations, not running services.
    pub fn new(discovery: &Discovery, view: DiscoveryView, server_version: &str) -> Result<Self> {
        let document = discovery.document_for(view)?;
        let bytes = canonical_document(&document)?;
        let schema_hash = hex::encode(Sha256::digest(&bytes));
        let plugins = document["x-theuth"]["plugins"].as_array().expect("declared plugins").iter()
            .map(|name| DeclaredPlugin { name: name.as_str().expect("plugin name").to_owned() }).collect();
        Ok(Self {
            document: OpenRpcDocument(serde_json::from_slice(&bytes)?),
            capabilities: Arc::new(Capabilities {
                server_version: server_version.to_owned(), protocol_version: ProtocolVersion { major: 1, minor: 0 }, schema_hash, plugins,
            }),
        })
    }
}

struct FrozenDiscovery([DiscoveryDocument; 3]);
type Slot = Arc<OnceLock<FrozenDiscovery>>;

/// Seal all audience documents in the Runtime's private typed extension slot.
/// A second installation is rejected; a frozen hash can never conceal new declarations.
pub fn freeze_discovery(rt: &runtime::Runtime, discovery: &Discovery, server_version: &str) -> Result<()> {
    let slot: Slot = rt.extension(Slot::default);
    ensure!(slot.get().is_none(), "RPC declarations are frozen");
    let documents = [DiscoveryView::Owner, DiscoveryView::Guest, DiscoveryView::Mesh]
        .map(|view| DiscoveryDocument::new(discovery, view, server_version));
    let [owner, guest, mesh] = documents;
    ensure!(slot.set(FrozenDiscovery([owner?, guest?, mesh?])).is_ok(), "RPC declarations are frozen");
    Ok(())
}

/// Borrow a shared frozen response. No schema generation runs on a request path.
pub fn discovery_document(rt: &runtime::Runtime, view: DiscoveryView) -> Result<DiscoveryDocument> {
    let slot: Slot = rt.extension(Slot::default);
    let frozen = slot.get().ok_or_else(|| anyhow::anyhow!("RPC declarations are not frozen"))?;
    Ok(frozen.0[match view { DiscoveryView::Owner => 0, DiscoveryView::Guest => 1, DiscoveryView::Mesh => 2 }].clone())
}

/// Resolve visibility only from transport-injected identity, never request params.
pub fn discovery_view(ext: &jsonrpsee::Extensions) -> jsonrpsee::core::RpcResult<DiscoveryView> {
    let peer = crate::peer(ext)?;
    if peer.guest().is_some() { return Ok(DiscoveryView::Guest); }
    Ok(if ext.get::<DiscoveryView>() == Some(&DiscoveryView::Mesh) { DiscoveryView::Mesh } else { DiscoveryView::Owner })
}
