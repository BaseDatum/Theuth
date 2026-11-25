//! Owner: B2 — embeds the plugin SDK (`sdk/templates/**` and the per-language SDK dirs) into the
//! crate so `t plugin new` / `plugin_new` work without a checkout (contract §10.2). Writes
//! `$OUT_DIR/sdk_files.rs`: `pub static SDK_FILES: &[(&str, &[u8])]`, paths relative to `sdk/`
//! with `/` separators, sorted. A missing `sdk/` yields an empty table.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// The `sdk/` subdirectories that are bundled.
const ROOTS: &[&str] = &["templates", "python", "ts", "go", "rust", "ruby", "sh"];

fn main() {
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let sdk = manifest.join("../../sdk");
    println!("cargo:rerun-if-changed=../../sdk");
    let mut files = Vec::new();
    for root in ROOTS {
        let dir = sdk.join(root);
        println!("cargo:rerun-if-changed=../../sdk/{root}");
        walk(&sdk, &dir, &mut files);
    }
    files.sort();
    let mut out = String::from("/// Every bundled SDK file: (path relative to `sdk/`, bytes).\npub static SDK_FILES: &[(&str, &[u8])] = &[\n");
    for (rel, abs) in &files {
        let _ = writeln!(out, "    ({rel:?}, include_bytes!({abs:?})),");
    }
    out.push_str("];\n");
    let dest = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR")).join("sdk_files.rs");
    std::fs::write(dest, out).expect("write sdk_files.rs");
}

/// Collect every regular file under `dir` (skipping `target/`, `node_modules/` and dot-files).
fn walk(sdk: &Path, dir: &Path, files: &mut Vec<(String, String)>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || name == "target" || name == "node_modules" {
            continue;
        }
        let path = entry.path();
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_dir() {
            println!("cargo:rerun-if-changed={}", path.display());
            walk(sdk, &path, files);
        } else if ft.is_file() {
            let Ok(rel) = path.strip_prefix(sdk) else { continue };
            let rel = rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/");
            let abs = std::fs::canonicalize(&path).unwrap_or(path);
            files.push((rel, abs.to_string_lossy().into_owned()));
        }
    }
}
