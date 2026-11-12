//! Tracked compatibility baseline; enable after step 9c exports the real composition.

use std::path::Path;

use rpc_core::compatibility::classify;
use serde_json::Value;

fn read_document(path: &Path) -> Value {
    let bytes = std::fs::read(path).unwrap_or_else(|error| panic!("{}: {error}; generate/export and explicitly review the wire baseline", path.display()));
    serde_json::from_slice(&bytes).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

#[test]
#[ignore = "step 9c must provide the real declaration-only composition export and the reviewed openrpc-v1.json fixture"]
fn wire_schema_snapshot() {
    let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    // Step 9c owns generation through the production composition, without
    // starting plugins. Never substitute a snapshot-only list of owner types.
    let generated = read_document(&crate_root.join("../../bindings/openrpc.json"));
    let baseline = read_document(&crate_root.join("tests/fixtures/wire/openrpc-v1.json"));
    let report = classify(&baseline, &generated);
    assert!(!report.fails(), "wire ABI change requires preservation or explicit major-cut approval:\n{report:#?}");
    // Even additive/documentation changes need deliberate baseline acceptance.
    assert_eq!(baseline, generated, "wire snapshot changed; review the classified report, then explicitly accept:\n{report:#?}");
}
