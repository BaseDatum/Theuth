//! Walk a root with `ignore` (60 §5.1): what `grep` can see is what gets
//! indexed. Metadata only — file contents are read at parse time, for the
//! changed set.

use std::path::Path;
use std::time::UNIX_EPOCH;

use crate::lang::Language;

/// Hard cap on files per root (60 §11).
pub const MAX_FILES: usize = 50_000;

/// One walked file.
#[derive(Clone, Debug)]
pub struct Entry {
    /// Root-relative, forward slashes.
    pub path: String,
    /// Modified time as unix nanoseconds (sync key).
    pub mtime_ns: i64,
    /// Byte size (sync key).
    pub size: i64,
    /// By extension.
    pub language: Language,
}

/// The walk result.
#[derive(Debug, Default)]
pub struct Walk {
    /// Files in path order.
    pub entries: Vec<Entry>,
    /// The walk stopped at [`MAX_FILES`].
    pub truncated: bool,
}

/// Walk `root`: gitignore/`.ignore`/hidden rules plus `.codeindexignore`; skip
/// files over `max_bytes` and generated paths; stop at [`MAX_FILES`].
pub fn walk(root: &Path, max_bytes: u64) -> Walk {
    let skip_vendor = root.join("Cargo.toml").is_file() || root.join("go.mod").is_file();
    let mut b = ignore::WalkBuilder::new(root);
    b.add_custom_ignore_filename(".codeindexignore");
    b.sort_by_file_path(|a, b| a.cmp(b));
    b.filter_entry(move |e| {
        let name = e.file_name().to_string_lossy();
        let is_dir = e.file_type().is_some_and(|t| t.is_dir());
        if is_dir {
            // `depth() == 1` = a direct child of the root.
            !(name == "node_modules"
                || name == "target"
                || (skip_vendor && name == "vendor" && e.depth() == 1))
        } else {
            !generated_file(&name)
        }
    });
    let mut out = Walk::default();
    for entry in b.build() {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if meta.len() > max_bytes {
            continue;
        }
        if out.entries.len() >= MAX_FILES {
            out.truncated = true;
            break;
        }
        let rel = entry.path().strip_prefix(root).unwrap_or(entry.path());
        let path = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        let language = rel
            .extension()
            .and_then(|e| e.to_str())
            .map_or(Language::Other, Language::from_ext);
        let mtime_ns = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| i64::try_from(d.as_nanos()).unwrap_or(i64::MAX));
        out.entries.push(Entry {
            path,
            mtime_ns,
            size: i64::try_from(meta.len()).unwrap_or(i64::MAX),
            language,
        });
    }
    out
}

/// `*.min.js`, `*.pb.go`, `*_generated.*` (60 §5.1).
fn generated_file(name: &str) -> bool {
    name.ends_with(".min.js")
        || name.ends_with(".pb.go")
        || name
            .rsplit_once('.')
            .is_some_and(|(stem, _)| stem.ends_with("_generated"))
}

/// Path heuristic for `files.is_test` (60 §4): a `test`/`tests`/`__tests__`
/// directory, `*_test.go`, `test_*.py`/`*_test.py`, `*.test.*`/`*.spec.*`.
pub fn is_test_path(path: &str) -> bool {
    let (dir, name) = path.rsplit_once('/').unwrap_or(("", path));
    dir.split('/')
        .any(|d| matches!(d, "test" | "tests" | "__tests__" | "testdata"))
        || name.ends_with("_test.go")
        || name.starts_with("test_") && name.ends_with(".py")
        || name.ends_with("_test.py")
        || name.contains(".test.")
        || name.contains(".spec.")
}

/// Entry points the manifests name (60 §17 item 9), root-relative and
/// existing on disk: Cargo `[lib]`/`[[bin]]` targets (implicit `src/lib.rs`,
/// `src/main.rs`, `src/bin/*.rs` included), `package.json` `main`/`bin`/
/// `exports`, `pyproject.toml` `[project.scripts]` / `[tool.poetry.scripts]`
/// modules. `manifests` are root-relative manifest paths. Go `package main`
/// files come from the index, not from here.
pub fn entry_points(root: &Path, manifests: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for m in manifests {
        let dir = m.rsplit_once('/').map_or("", |(d, _)| d);
        let name = m.rsplit('/').next().unwrap_or(m);
        let abs = root.join(m);
        let mut push = |rel: &str| {
            let rel = rel.trim_start_matches("./");
            let full = if dir.is_empty() {
                rel.to_owned()
            } else {
                format!("{dir}/{rel}")
            };
            if root.join(&full).is_file() {
                out.push(full);
            }
        };
        match name {
            // Plain `toml`, not `cargo_toml::Manifest::from_path`: its
            // workspace discovery and target completion cost ~4 ms per
            // manifest (0.2 s per rank on this repo). Cargo's implicit targets
            // are a directory listing.
            "Cargo.toml" => {
                let Some(v) = std::fs::read_to_string(&abs)
                    .ok()
                    .and_then(|s| s.parse::<toml::Table>().ok())
                else {
                    continue;
                };
                if !v.contains_key("package") {
                    continue;
                }
                // `[lib]` is a table, `[[bin]]` an array of tables.
                let paths = |section: &str| -> Vec<&str> {
                    match v.get(section) {
                        Some(toml::Value::Table(t)) => t
                            .get("path")
                            .and_then(toml::Value::as_str)
                            .into_iter()
                            .collect(),
                        Some(toml::Value::Array(a)) => a
                            .iter()
                            .filter_map(|t| t.get("path").and_then(toml::Value::as_str))
                            .collect(),
                        _ => Vec::new(),
                    }
                };
                push(paths("lib").first().copied().unwrap_or("src/lib.rs"));
                push("src/main.rs");
                for b in paths("bin") {
                    push(b);
                }
                if let Ok(rd) = std::fs::read_dir(root.join(dir).join("src/bin")) {
                    let mut names: Vec<String> = rd
                        .flatten()
                        .filter_map(|e| {
                            let n = e.file_name().to_string_lossy().into_owned();
                            let file = e.file_type().is_ok_and(|t| t.is_file());
                            if file && n.ends_with(".rs") {
                                Some(format!("src/bin/{n}"))
                            } else if !file
                                && root
                                    .join(dir)
                                    .join("src/bin")
                                    .join(&n)
                                    .join("main.rs")
                                    .is_file()
                            {
                                Some(format!("src/bin/{n}/main.rs"))
                            } else {
                                None
                            }
                        })
                        .collect();
                    names.sort_unstable();
                    for n in names {
                        push(&n);
                    }
                }
            }
            "package.json" => {
                let Some(v) = std::fs::read_to_string(&abs)
                    .ok()
                    .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                else {
                    continue;
                };
                for key in ["main", "bin", "exports"] {
                    let mut leaves = Vec::new();
                    json_strings(&v[key], &mut leaves);
                    for s in leaves {
                        push(s);
                    }
                }
            }
            "pyproject.toml" => {
                let Some(v) = std::fs::read_to_string(&abs)
                    .ok()
                    .and_then(|s| s.parse::<toml::Table>().ok())
                else {
                    continue;
                };
                let scripts = [
                    v.get("project").and_then(|p| p.get("scripts")),
                    v.get("tool")
                        .and_then(|t| t.get("poetry"))
                        .and_then(|p| p.get("scripts")),
                ];
                for spec in scripts
                    .into_iter()
                    .flatten()
                    .filter_map(toml::Value::as_table)
                    .flat_map(|t| t.values())
                    .filter_map(toml::Value::as_str)
                {
                    // `pkg.mod:func` → `pkg/mod.py` | `pkg/mod/__init__.py`, also under `src/`.
                    let module = spec.split(':').next().unwrap_or(spec).replace('.', "/");
                    for cand in [
                        format!("{module}.py"),
                        format!("{module}/__init__.py"),
                        format!("src/{module}.py"),
                        format!("src/{module}/__init__.py"),
                    ] {
                        push(&cand);
                    }
                }
            }
            _ => {}
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// Every string leaf of a JSON value (`exports` nests conditions and subpaths).
fn json_strings<'a>(v: &'a serde_json::Value, out: &mut Vec<&'a str>) {
    match v {
        serde_json::Value::String(s) => out.push(s),
        serde_json::Value::Array(a) => a.iter().for_each(|x| json_strings(x, out)),
        serde_json::Value::Object(o) => o.values().for_each(|x| json_strings(x, out)),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heuristics() {
        assert!(generated_file("app.min.js"));
        assert!(generated_file("api.pb.go"));
        assert!(generated_file("schema_generated.rs"));
        assert!(!generated_file("generated.rs"));
        assert!(is_test_path("crates/x/tests/it.rs"));
        assert!(is_test_path("pkg/a_test.go"));
        assert!(is_test_path("src/test_store.py"));
        assert!(is_test_path("src/store.test.ts"));
        assert!(!is_test_path("src/testing.rs"));
        assert!(!is_test_path("src/contest.py"));
    }

    #[test]
    fn entry_points_from_manifests() {
        let d = std::env::temp_dir().join(format!("theuth-walk-{}", uuid::Uuid::new_v4()));
        let w = |rel: &str, body: &str| {
            let p = d.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        };
        w("Cargo.toml", "[workspace]\nmembers = [\"crates/*\"]\n");
        w(
            "crates/a/Cargo.toml",
            "[package]\nname = \"a\"\nversion = \"0.1.0\"\n[[bin]]\nname = \"tool\"\npath = \"src/tool.rs\"\n",
        );
        w("crates/a/src/lib.rs", "");
        w("crates/a/src/main.rs", "");
        w("crates/a/src/tool.rs", "");
        w("crates/a/src/bin/extra.rs", "");
        w("crates/a/src/other.rs", "");
        w(
            "web/package.json",
            r#"{"main": "./dist/index.js", "bin": {"cli": "bin/cli.js"}, "exports": {".": {"import": "./src/index.ts", "types": "./missing.d.ts"}}}"#,
        );
        w("web/dist/index.js", "");
        w("web/bin/cli.js", "");
        w("web/src/index.ts", "");
        w(
            "py/pyproject.toml",
            "[project]\nname = \"p\"\n[project.scripts]\np = \"pkg.cli:main\"\n[tool.poetry.scripts]\nq = \"pkg:run\"\n",
        );
        w("py/src/pkg/cli.py", "");
        w("py/src/pkg/__init__.py", "");
        let manifests: Vec<String> = [
            "Cargo.toml",
            "crates/a/Cargo.toml",
            "web/package.json",
            "py/pyproject.toml",
        ]
        .map(str::to_owned)
        .to_vec();
        let got = entry_points(&d, &manifests);
        let _ = std::fs::remove_dir_all(&d);
        assert_eq!(
            got,
            [
                "crates/a/src/bin/extra.rs",
                "crates/a/src/lib.rs",
                "crates/a/src/main.rs",
                "crates/a/src/tool.rs",
                "py/src/pkg/__init__.py",
                "py/src/pkg/cli.py",
                "web/bin/cli.js",
                "web/dist/index.js",
                "web/src/index.ts",
            ]
        );
    }
}
