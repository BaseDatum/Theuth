//! Tracked compatibility baseline exported from the declaration-only composition.

use std::path::Path;

use rpc_core::compatibility::classify;
use serde_json::Value;

fn read_document(path: &Path) -> Value {
    let bytes = std::fs::read(path).unwrap_or_else(|error| panic!("{}: {error}; generate/export and explicitly review the wire baseline", path.display()));
    serde_json::from_slice(&bytes).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

#[test]
fn wire_schema_snapshot() {
    let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let generated = read_document(&crate_root.join("../../bindings/openrpc.json"));
    let baseline = read_document(&crate_root.join("tests/fixtures/wire/openrpc-v1.json"));
    let report = classify(&baseline, &generated);
    assert!(!report.fails(), "wire ABI change requires preservation or explicit major-cut approval:\n{report:#?}");
    // Even additive/documentation changes need deliberate baseline acceptance.
    assert_eq!(baseline, generated, "wire snapshot changed; review the classified report, then explicitly accept:\n{report:#?}");
}
