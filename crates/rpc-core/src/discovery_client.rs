//! Connect-time negotiation; a cached document never substitutes for a fresh summary.

use std::sync::Arc;
use anyhow::{Result, ensure};
use jsonrpsee::{core::client::{ClientT, Error}, rpc_params};
use serde_json::Value;
use crate::{Capabilities, DiscoveryView, OpenRpcDocument, document_hash};

/// Current/previous verified documents for one authenticated endpoint and view.
/// Exclusive negotiation borrows prevent stale reconnect replies installing over
/// a newer connection. Callers must await negotiation before exposing the client.
#[derive(Default)]
pub struct DiscoveryCache {
    identity: Option<(String, DiscoveryView)>,
    documents: Vec<NegotiatedDiscovery>,
}

/// The contract selected for this connection; `None` explicitly means legacy.
#[derive(Clone, Debug)]
pub struct NegotiatedDiscovery {
    /// Fresh summary, even when the document came from cache.
    pub capabilities: Arc<Capabilities>,
    /// Verified direct OpenRPC response matching the summary hash.
    pub document: OpenRpcDocument,
}

impl DiscoveryCache {
    /// Fetch capabilities every time, discover only on a verified hash miss.
    /// Only method-not-found selects legacy; auth, transport, parse and hash
    /// failures are errors, never permission to reuse old support assumptions.
    pub async fn negotiate<C: ClientT>(&mut self, client: &C, endpoint: &str, view: DiscoveryView) -> Result<Option<NegotiatedDiscovery>> {
        if self.identity.as_ref().is_none_or(|(old, old_view)| old != endpoint || *old_view != view) {
            self.documents.clear();
            self.identity = Some((endpoint.to_owned(), view));
        }
        let capabilities: Arc<Capabilities> = match client.request("rpc.capabilities", rpc_params![]).await {
            Ok(summary) => summary,
            Err(Error::Call(error)) if error.code() == -32601 => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        ensure!(capabilities.protocol_version.major == 1, "unsupported RPC protocol major {}", capabilities.protocol_version.major);
        if let Some(index) = self.documents.iter().position(|entry| entry.capabilities.schema_hash == capabilities.schema_hash) {
            let cached = self.documents.remove(index);
            ensure!(cached.capabilities.protocol_version == capabilities.protocol_version && cached.capabilities.plugins == capabilities.plugins, "cached discovery summary mismatch");
            let selected = NegotiatedDiscovery { capabilities, document: cached.document };
            self.documents.insert(0, selected.clone());
            return Ok(Some(selected));
        }
        let document: OpenRpcDocument = client.request("rpc.discover", rpc_params![]).await?;
        ensure!(document.0.get().len() <= 64 << 20, "discovery document exceeds 64 MiB");
        let value: Value = serde_json::from_str(document.0.get())?;
        validate_document(&value)?;
        ensure!(document_hash(&value)? == capabilities.schema_hash, "discovery content hash mismatch");
        ensure!(value["x-theuth"]["protocolVersion"] == serde_json::to_value(&capabilities.protocol_version)?, "discovery protocol summary mismatch");
        let plugins: Vec<_> = capabilities.plugins.iter().map(|plugin| plugin.name.as_str()).collect();
        ensure!(value["x-theuth"]["plugins"] == serde_json::to_value(plugins)?, "discovery plugin summary mismatch");
        let selected = NegotiatedDiscovery { capabilities, document };
        self.documents.insert(0, selected.clone());
        self.documents.truncate(2);
        Ok(Some(selected))
    }
}

fn validate_document(document: &Value) -> Result<()> {
    use std::collections::BTreeSet;
    ensure!(document["openrpc"] == "1.3.2", "unsupported OpenRPC version");
    let methods = document["methods"].as_array().ok_or_else(|| anyhow::anyhow!("missing discovery methods"))?;
    let mut names = BTreeSet::new();
    for method in methods {
        let name = method["name"].as_str().ok_or_else(|| anyhow::anyhow!("missing method name"))?;
        ensure!(names.insert(name), "duplicate discovery method: {name}");
        ensure!(method["params"].is_array() && method["result"].is_object(), "malformed method: {name}");
    }
    ensure!(names.contains("rpc.capabilities") && names.contains("rpc.discover"), "discovery methods absent from catalog");
    let components = document["components"]["schemas"].as_object().ok_or_else(|| anyhow::anyhow!("missing schema components"))?;
    // Use schemars' schema-keyword walk: properties called `$ref` and inert
    // defaults/examples are data, not references or reference-scope changes.
    fn schema(value: &Value, document: &Value) -> Result<()> {
        fn walk(schema: &mut schemars::Schema, document: &Value, depth: usize) -> Result<()> {
            ensure!(depth <= 128, "discovery nesting exceeds 128");
            ensure!(schema.get("$id").is_none(), "discovery schema changes reference scope");
            if let Some(reference) = schema.get("$ref") {
                let reference = reference.as_str().ok_or_else(|| anyhow::anyhow!("invalid schema reference"))?;
                ensure!(reference.starts_with("#/components/schemas/") && document.pointer(&reference[1..]).is_some(), "nonlocal or unresolved schema reference: {reference}");
            }
            let mut result = Ok(());
            schemars::transform::transform_subschemas(&mut |child: &mut schemars::Schema| {
                if result.is_ok() { result = walk(child, document, depth + 1); }
            }, schema);
            result
        }
        walk(&mut schemars::Schema::try_from(value.clone())?, document, 0)
    }
    for component in components.values() { schema(component, document)?; }
    for method in methods {
        for param in method["params"].as_array().unwrap() { schema(&param["schema"], document)?; }
        schema(&method["result"]["schema"], document)?;
        schema(&method["result"]["x-theuth-receiverSchema"], document)?;
    }
    for (catalog, identity, fields) in [
        ("events", "topic", ["schema", "receiverSchema"]),
        ("subscriptions", "subscribe", ["itemSchema", "receiverSchema"]),
    ] {
        let entries = document["x-theuth"][catalog].as_array().ok_or_else(|| anyhow::anyhow!("missing {catalog}"))?;
        let mut identities = BTreeSet::new();
        for entry in entries {
            let name = entry[identity].as_str().ok_or_else(|| anyhow::anyhow!("missing {identity}"))?;
            if catalog == "events" {
                ensure!(entry["version"].as_u64().is_some_and(|version| version > 0 && version <= u32::MAX.into()), "invalid event version: {name}");
            } else {
                let stop = entry["unsubscribe"].as_str().ok_or_else(|| anyhow::anyhow!("missing unsubscribe"))?;
                ensure!(names.contains(name) && names.contains(stop) && entry["notification"].is_string(), "invalid subscription pair: {name}");
            }
            ensure!(identities.insert((name, entry.get("version").and_then(Value::as_u64))), "duplicate {catalog}: {name}");
            for field in fields { schema(&entry[field], document)?; }
        }
    }
    schema(&document["x-theuth"]["eventEnvelopeSchema"], document)
}


#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn receiving_catalog_rejects_nonlocal_refs_duplicates_and_malformed_events() {
        let method = |name| json!({"name":name,"params":[],"result":{"schema":true,"x-theuth-receiverSchema":true}});
        let document = json!({
            "openrpc":"1.3.2", "methods":[method("rpc.capabilities"),method("rpc.discover")],
            "components":{"schemas":{"Valid":{"type":"object","properties":{"$ref":{"type":"string"}},"default":{"$ref":"inert data"}}}},
            "x-theuth":{"events":[],"subscriptions":[],"eventEnvelopeSchema":true},
        });
        assert!(validate_document(&document).is_ok());
        for reference in ["https://example.com/remote", "#/components/schemas/Absent"] {
            let mut changed = document.clone();
            changed["components"]["schemas"]["Valid"] = json!({"$ref":reference});
            assert!(validate_document(&changed).is_err());
        }
        let mut duplicate = document.clone();
        duplicate["methods"].as_array_mut().unwrap().push(method("rpc.discover"));
        assert!(validate_document(&duplicate).is_err());
        let mut malformed = document;
        malformed["x-theuth"]["events"] = json!([{"topic":"test","schema":true,"receiverSchema":true}]);
        assert!(validate_document(&malformed).is_err());
    }
}
