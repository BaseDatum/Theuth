//! Every wire method name is unique across the domain traits: the merged module (`rpc_core::merge`) panics at
//! startup on a duplicate, and two domain files can each add the same name without a compile error.

use std::{collections::BTreeMap, fs, path::Path};

fn sources(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn rpc_method_names_are_unique() {
    let mut files = Vec::new();
    sources(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut files);
    let mut seen: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for file in &files {
        let text = fs::read_to_string(file).unwrap();
        for (i, line) in text.lines().enumerate() {
            let line = line.trim_start();
            let Some(rest) = line.strip_prefix("#[method(name = \"").or_else(|| line.strip_prefix("#[subscription(name = \"")) else {
                continue;
            };
            let name = &rest[..rest.find('"').unwrap()];
            seen.entry(name.to_owned()).or_default().push(format!("{}:{}", file.display(), i + 1));
        }
    }
    assert!(seen.len() > 500, "found only {} method names; did the attribute syntax change?", seen.len());
    let dups: Vec<_> = seen.iter().filter(|(_, at)| at.len() > 1).collect();
    assert!(dups.is_empty(), "duplicate RPC method names: {dups:#?}");
}
