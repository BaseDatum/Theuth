//! Ratchet the wire layer's workspace dependencies; tighten these lists on removal.
//! Wire-layer refactor plan, step 13: rpc ends up depending only on rpc-core and runtime.

use std::{collections::BTreeSet, fs, path::Path};
use toml::Value;

const ALLOWED: &[&str] = &["rpc-core-macros", "runtime", "wire"];
const ALLOWED_DEV: &[&str] = &[];

#[test]
fn workspace_dependencies_only_shrink() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let parse = |path| toml::from_str::<Value>(&fs::read_to_string(path).unwrap()).unwrap();
    let manifest = parse(dir.join("Cargo.toml"));
    let root = parse(dir.join("../../Cargo.toml"));
    let workspace = root["workspace"]["dependencies"].as_table().unwrap();
    // Root path entries identify local package names; direct paths are checked below too.
    let local: BTreeSet<_> = workspace
        .iter()
        .filter_map(|(alias, spec)| {
            spec.get("path")?;
            Some(spec.get("package").and_then(Value::as_str).unwrap_or(alias))
        })
        .collect();
    let mut actual = [BTreeSet::new(), BTreeSet::new()];
    let targets = manifest.get("target").and_then(Value::as_table);
    for section in std::iter::once(&manifest).chain(targets.into_iter().flat_map(|t| t.values())) {
        for (kind, index) in [
            ("dependencies", 0),
            ("build-dependencies", 0),
            ("dev-dependencies", 1),
        ] {
            if let Some(deps) = section.get(kind).and_then(Value::as_table) {
                for (alias, spec) in deps {
                    let spec = if spec.get("workspace").and_then(Value::as_bool) == Some(true) {
                        &workspace[alias]
                    } else {
                        spec
                    };
                    let package = spec.get("package").and_then(Value::as_str).unwrap_or(alias);
                    if local.contains(package) || spec.get("path").is_some() {
                        actual[index].insert(package);
                    }
                }
            }
        }
    }
    for (actual, allowed, kind) in [
        (&actual[0], ALLOWED, "dependencies/build-dependencies"),
        (&actual[1], ALLOWED_DEV, "dev-dependencies"),
    ] {
        let allowed: BTreeSet<_> = allowed.iter().copied().collect();
        if let Some(dependency) = actual.difference(&allowed).next() {
            panic!(
                "{}: workspace dependency `{dependency}` in {kind} is not allowed; the wire layer must not grow plugin dependencies; see docs/design/00-architecture.md §1; remove the dependency or justify an architectural change",
                env!("CARGO_PKG_NAME")
            );
        }
        if let Some(dependency) = allowed.difference(actual).next() {
            panic!(
                "{}: allowed workspace dependency `{dependency}` is no longer a {kind} dependency; remove it from the {kind} allowed list in this change",
                env!("CARGO_PKG_NAME")
            );
        }
    }
}
