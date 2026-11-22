//! Build-time guard (review M1): `app::HASH` (packages/apps/generated/library-hash.txt) must be the library hash
//! compiled into the committed OpenUI bundle (crates/app-js/js/theuth-apps-inproc.js). If the library was
//! regenerated without `just apps-bundle`, the binary would validate apps against one library while
//! advertising another, so the build fails here instead.

use std::path::Path;

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let hash_file = root.join("packages/apps/generated/library-hash.txt");
    let bundle_file = root.join("crates/app-js/js/theuth-apps-inproc.js");
    println!("cargo:rerun-if-changed={}", hash_file.display());
    println!("cargo:rerun-if-changed={}", bundle_file.display());
    let hash = std::fs::read_to_string(&hash_file)
        .unwrap_or_else(|e| panic!("{}: {e}", hash_file.display()));
    let hash = hash.trim();
    assert!(
        hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()),
        "{} is not a sha256 hex: {hash:?}",
        hash_file.display()
    );
    let bundle = std::fs::read_to_string(&bundle_file)
        .unwrap_or_else(|e| panic!("{}: {e}", bundle_file.display()));
    assert!(
        bundle.contains(hash),
        "the committed OpenUI bundle ({}) does not embed library hash {hash} from {}: run `just apps-bundle` and commit the result",
        bundle_file.display(),
        hash_file.display()
    );
}
