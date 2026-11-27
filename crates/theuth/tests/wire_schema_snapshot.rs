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

    // Sibling wire fixtures used by `just ts-desktop-from-fixtures` and CI `prepare`.
    let out_dir = crate_root.join("../../bindings");
    let fix_dir = crate_root.join("tests/fixtures/wire");
    for name in [
        "methods.json",
        "surface-catalog.json",
        "surface-catalog-compact.txt",
        "client-auxiliary.json",
    ] {
        let gen_path = out_dir.join(name);
        let fix_path = fix_dir.join(name);
        let gen_bytes = std::fs::read(&gen_path)
            .unwrap_or_else(|e| panic!("{}: {e}; run `just wire-schema` first", gen_path.display()));
        let fix_bytes = std::fs::read(&fix_path)
            .unwrap_or_else(|e| panic!("{}: {e}; run `just wire-schema-accept` to update wire fixtures", fix_path.display()));
        assert_eq!(
            fix_bytes, gen_bytes,
            "wire fixture {name} drifted from generated bindings/{name}; run `just wire-schema-accept` and commit"
        );
    }
}
