//! Fixture trees per language — parsed alone, and indexed as roots so import
//! and name resolution (60 §6) are checked end to end — and this repository
//! indexed into a temp data dir (60 §16 rows 1–2). Nothing is written inside
//! the repo.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use codeindex::lang::Language;
use codeindex::parse::{ParsedFile, Parsers, parse};
use codeindex::{Codeindex, CodeindexConfig, RootIndex, store};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn fixture(rel: &str) -> Vec<u8> {
    std::fs::read(fixtures().join(rel)).unwrap()
}

/// Parse a fixture with the production (locals-on, `'static`) configuration.
fn parsed(lang: Language, rel: &str) -> ParsedFile<'static> {
    let mut p = Parsers::default();
    parse(&mut p, lang.tags().unwrap(), lang, &fixture(rel)).unwrap()
}

fn defs<'a>(p: &'a ParsedFile<'_>) -> Vec<(&'a str, &'a str, Option<&'a str>)> {
    p.symbols
        .iter()
        .map(|s| (s.name.as_str(), s.kind, s.parent.map(|i| p.symbols[i].name.as_str())))
        .collect()
}

fn ref_names<'a>(p: &'a ParsedFile<'_>) -> Vec<&'a str> {
    p.refs.iter().map(|r| r.name.as_str()).collect()
}

/// Locals (60 §5.3): the parameter call `f(x)` disappears, definitions do not.
fn assert_locals_filter(lang: Language, rel: &str) {
    let with = parsed(lang, rel);
    let no_locals = lang.build(false).unwrap();
    let mut p = Parsers::default();
    let without = parse(&mut p, &no_locals, lang, &fixture(rel)).unwrap();
    assert_eq!(defs(&with), defs(&without), "{lang:?}: definitions differ");
    assert!(ref_names(&without).contains(&"f"), "{lang:?}: no `f` ref even without locals");
    assert!(!ref_names(&with).contains(&"f"), "{lang:?}: parameter call kept: {:?}", ref_names(&with));
    assert!(ref_names(&with).contains(&"helper"), "{lang:?}: real call dropped");
    assert!(with.refs.len() < without.refs.len());
}

#[test]
fn rust_fixture() {
    let p = parsed(Language::Rust, "rust/crates/a/src/lib.rs");
    let d = defs(&p);
    assert!(d.contains(&("Registry", "class", None)));
    assert!(d.contains(&("new", "method", Some("Registry"))), "methods hang off their impl type: {d:?}");
    assert!(d.contains(&("get", "method", Some("Registry"))), "`impl Store for Registry` methods too: {d:?}");
    assert!(d.contains(&("Store", "interface", None)));
    assert!(d.contains(&("Kind", "class", None)));
    assert!(d.contains(&("twice", "macro", None)));
    assert!(d.contains(&("inner", "module", None)));
    assert!(d.contains(&("helper", "function", Some("inner"))), "mod fn is a function, not a method: {d:?}");
    assert!(d.contains(&("run", "function", None)));
    let reg = p.symbols.iter().find(|s| s.name == "Registry").unwrap();
    assert!(reg.exported && reg.signature.starts_with("pub struct Registry"));
    assert!(!p.symbols.iter().find(|s| s.name == "apply").unwrap().exported);
    assert!(!p.has_errors);
    // The `twice!(1)` call inside `helper` is attributed to `helper`.
    let r = p.refs.iter().find(|r| r.name == "twice").unwrap();
    assert_eq!(p.symbols[r.from.unwrap()].name, "helper");
    assert!(p.refs.iter().any(|r| r.name == "Store" && r.kind == "implementation"));
    let news: Vec<Option<&str>> = p.refs.iter().filter(|r| r.name == "new" && r.kind == "call").map(|r| r.qualifier.as_deref()).collect();
    assert!(news.contains(&Some("Registry")) && news.contains(&Some("Vec")), "qualifiers from the tag range: {news:?}");
    let pushes: Vec<Option<&str>> = p.refs.iter().filter(|r| r.name == "push" && r.kind == "call").map(|r| r.qualifier.as_deref()).collect();
    assert_eq!(pushes, [Some("self.items"), Some("r")], "receiver qualifiers");
    let helper = p.refs.iter().find(|r| r.name == "helper" && r.kind == "call").unwrap();
    assert_eq!(helper.qualifier.as_deref(), Some("inner"));
    // `self.items.iter().find(..).cloned()`: the prefix has calls in it → no qualifier.
    assert!(p.refs.iter().filter(|r| r.name == "cloned").all(|r| r.qualifier.is_none()));
    let imports: Vec<(&str, Option<&[String]>)> = p.imports.iter().map(|i| (i.specifier.as_str(), i.names.as_deref())).collect();
    assert_eq!(imports[0], ("mod:store", None));
    assert_eq!(imports[1], ("store", Some(&["open_db".to_owned()][..])));
    assert_locals_filter(Language::Rust, "rust/crates/a/src/lib.rs");
}

#[test]
fn rust_constants() {
    let p = parsed(Language::Rust, "../../../tools/src/fs.rs");
    assert!(defs(&p).contains(&("MAX_BYTES", "constant", None)), "{:?}", defs(&p));
}

#[test]
fn typescript_fixture() {
    let p = parsed(Language::TypeScript, "ts/src/app.ts");
    let d = defs(&p);
    assert!(d.contains(&("Config", "interface", None)), "{d:?}");
    assert!(d.contains(&("Loader", "class", None)));
    assert!(d.contains(&("load", "method", Some("Loader"))), "{d:?}");
    assert!(d.contains(&("apply", "function", None)));
    assert!(d.contains(&("helper", "function", None)));
    let loader = p.symbols.iter().find(|s| s.name == "Loader").unwrap();
    assert!(loader.exported);
    // `/** doc */ export class X` leaves the comment beside the export
    // statement, not the class: the shipped JS query attaches no docs there.
    assert_eq!(loader.docs, None);
    assert_locals_filter(Language::TypeScript, "ts/src/app.ts");
}

#[test]
fn javascript_fixture() {
    let p = parsed(Language::JavaScript, "js/lib.js");
    let d = defs(&p);
    assert!(d.contains(&("Queue", "class", None)), "{d:?}");
    assert!(d.contains(&("push", "method", Some("Queue"))), "{d:?}");
    let q = p.symbols.iter().find(|s| s.name == "Queue").unwrap();
    assert!(q.docs.as_deref().is_some_and(|d| d.contains("A queue.")), "{:?}", q.docs);
    assert!(d.contains(&("apply", "function", None)));
    assert!(d.contains(&("helper", "function", None)));
    assert!(d.contains(&("run", "function", None)));
    assert!(p.refs.iter().any(|r| r.name == "Queue" && r.kind == "class"));
    assert_locals_filter(Language::JavaScript, "js/lib.js");
}

#[test]
fn python_fixture() {
    let p = parsed(Language::Python, "py/pkg/store.py");
    let d = defs(&p);
    assert!(d.contains(&("DEFAULT_PATH", "constant", None)), "{d:?}");
    assert!(d.contains(&("Store", "class", None)));
    assert!(d.contains(&("get", "method", Some("Store"))) || d.contains(&("get", "function", Some("Store"))), "{d:?}");
    assert!(d.contains(&("apply", "function", None)));
    assert!(p.symbols.iter().find(|s| s.name == "_private").is_some_and(|s| !s.exported));
    assert!(p.symbols.iter().find(|s| s.name == "run").is_some_and(|s| s.exported));
    assert_locals_filter(Language::Python, "py/pkg/store.py");
}

#[test]
fn go_fixture() {
    let p = parsed(Language::Go, "go/pkg/store.go");
    let d = defs(&p);
    assert!(d.contains(&("Store", "type", None)), "{d:?}");
    assert!(d.contains(&("Getter", "type", None)));
    assert!(d.contains(&("NewStore", "function", None)));
    assert!(d.contains(&("Get", "method", None)));
    assert!(d.contains(&("helper", "function", None)));
    let ns = p.symbols.iter().find(|s| s.name == "NewStore").unwrap();
    assert!(ns.exported);
    assert_eq!(ns.docs.as_deref(), Some("NewStore makes one."));
    assert!(!p.symbols.iter().find(|s| s.name == "helper").unwrap().exported);
    assert!(p.refs.iter().any(|r| r.name == "NewStore" && r.kind == "call"));
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

struct TempDir(PathBuf);
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn tempdir() -> TempDir {
    let d = std::env::temp_dir().join(format!("theuth-codeindex-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&d).unwrap();
    TempDir(d)
}

/// Index one fixture directory as a root (temp data dir, dropped with it).
async fn index_fixture(rel: &str) -> (TempDir, Arc<Codeindex>, Arc<RootIndex>) {
    let data = tempdir();
    let ix = Arc::new(Codeindex::new(CodeindexConfig::default(), &data.0).unwrap());
    let root = fixtures().join(rel).canonicalize().unwrap();
    let ri = ix.open_root(&root).await.unwrap();
    let r = ix.reconcile(&ri, false).await.unwrap();
    assert!(!r.stale && r.reparsed > 0, "{r:?}");
    (data, ix, ri)
}

/// `(file path, specifier, target path, provenance)` for every import row.
async fn import_rows(ri: &RootIndex) -> Vec<(String, String, Option<String>, String)> {
    sqlx::query_as(
        "SELECT f.path, i.specifier, t.path, i.provenance FROM imports i JOIN files f ON f.id = i.file_id \
         LEFT JOIN files t ON t.id = i.target_file ORDER BY f.path, i.line, i.id",
    )
    .fetch_all(&ri.pool)
    .await
    .unwrap()
}

/// `(file path, name, kind, target "path:line", provenance, confidence, candidates)` for every ref.
#[derive(Debug, sqlx::FromRow)]
struct RefView {
    path: String,
    name: String,
    kind: String,
    qualifier: Option<String>,
    target: Option<String>,
    provenance: String,
    confidence: f64,
    candidates: i64,
}

async fn ref_rows(ri: &RootIndex) -> Vec<RefView> {
    sqlx::query_as(
        "SELECT f.path, r.name, r.kind, r.qualifier, CASE WHEN t.id IS NULL THEN NULL ELSE tf.path || ':' || t.line_start END AS target, \
         r.provenance, r.confidence, r.candidates FROM refs r JOIN files f ON f.id = r.file_id \
         LEFT JOIN symbols t ON t.id = r.to_symbol LEFT JOIN files tf ON tf.id = t.file_id ORDER BY f.path, r.line",
    )
    .fetch_all(&ri.pool)
    .await
    .unwrap()
}

fn import_of<'a>(rows: &'a [(String, String, Option<String>, String)], file: &str, spec: &str) -> (Option<&'a str>, &'a str) {
    let r = rows
        .iter()
        .find(|(f, s, _, _)| f == file && s == spec)
        .unwrap_or_else(|| panic!("no import {spec:?} in {file}: {rows:?}"));
    (r.2.as_deref(), r.3.as_str())
}

fn ref_of<'a>(rows: &'a [RefView], file: &str, name: &str) -> &'a RefView {
    rows.iter()
        .find(|r| r.path == file && r.name == name)
        .unwrap_or_else(|| panic!("no ref {name:?} in {file}"))
}

async fn packages(ri: &RootIndex) -> HashMap<String, Option<String>> {
    sqlx::query_as::<_, (String, Option<String>)>("SELECT path, package FROM files")
        .fetch_all(&ri.pool)
        .await
        .unwrap()
        .into_iter()
        .collect()
}

/// 60 §6.1 Rust: a two-crate workspace with a path dependency (and a rename),
/// a `mod` tree two levels deep, `crate::`/`super::` paths, a one-level
/// `pub use` re-export, an excluded member, std/registry crates as external.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rust_workspace_resolution() {
    let (_d, _ix, ri) = index_fixture("rust").await;
    let pk = packages(&ri).await;
    assert_eq!(pk["crates/a/src/store/sub.rs"].as_deref(), Some("fixture-a"));
    assert_eq!(pk["crates/b/src/cli.rs"].as_deref(), Some("fixture-b"));
    assert_eq!(pk["crates/skipped/src/lib.rs"], None, "excluded member has no package");

    let im = import_rows(&ri).await;
    let lib = "crates/a/src/lib.rs";
    assert_eq!(import_of(&im, lib, "mod:store"), (Some("crates/a/src/store.rs"), "mod"));
    assert_eq!(import_of(&im, lib, "store"), (Some("crates/a/src/store.rs"), "cargo"));
    assert_eq!(import_of(&im, "crates/a/src/store.rs", "mod:sub"), (Some("crates/a/src/store/sub.rs"), "mod"));
    assert_eq!(import_of(&im, "crates/a/src/store.rs", "crate"), (Some(lib), "cargo"));
    assert_eq!(import_of(&im, "crates/a/src/store/sub.rs", "super"), (Some("crates/a/src/store.rs"), "cargo"));
    let main = "crates/b/src/main.rs";
    // `use fixture_a::open_db`: `open_db` is re-exported from `store` → one level followed.
    assert_eq!(import_of(&im, main, "fixture_a"), (Some("crates/a/src/store.rs"), "cargo"));
    assert_eq!(import_of(&im, main, "fixture_a::store"), (Some("crates/a/src/store.rs"), "cargo"));
    assert_eq!(import_of(&im, main, "alias_a"), (Some(lib), "cargo"), "renamed dependency");
    assert_eq!(import_of(&im, main, "serde"), (None, "external"));
    assert_eq!(import_of(&im, main, "std::collections"), (None, "external"));
    assert_eq!(import_of(&im, main, "mod:cli"), (Some("crates/b/src/cli.rs"), "mod"));
    assert_eq!(import_of(&im, "crates/b/src/cli.rs", "crate"), (Some(main), "cargo"));
    // `use fixture_a::{Kind, Registry}` → two rows on the lib root.
    assert_eq!(im.iter().filter(|(f, s, t, _)| f == main && s == "fixture_a" && t.as_deref() == Some(lib)).count(), 2, "{im:?}");
    assert!(im.iter().all(|(_, _, _, p)| p != "unresolved"), "{im:?}");

    let rf = ref_rows(&ri).await;
    let r = ref_of(&rf, main, "open_db");
    assert_eq!((r.target.as_deref(), r.provenance.as_str(), r.confidence), (Some("crates/a/src/store.rs:8"), "import", 1.0), "{r:?}");
    let r = ref_of(&rf, main, "helper");
    assert_eq!((r.qualifier.as_deref(), r.provenance.as_str()), (Some("inner"), "import"), "qualifier through a renamed dep: {r:?}");
    assert!(r.target.as_deref().unwrap().starts_with(lib));
    let r = ref_of(&rf, main, "check");
    assert_eq!((r.qualifier.as_deref(), r.provenance.as_str()), (Some("fixture_a::store::sub"), "import"), "{r:?}");
    assert_eq!(r.target.as_deref(), Some("crates/a/src/store/sub.rs:5"));
    let r = ref_of(&rf, main, "run");
    assert_eq!((r.qualifier.as_deref(), r.target.as_deref(), r.provenance.as_str()), (Some("cli"), Some("crates/b/src/cli.rs:7"), "import"));
    // `::fixture_a::run()`: a global path names the crate even though `run` also exists in `cli`.
    let r = rf.iter().find(|r| r.path == main && r.name == "run" && r.qualifier.as_deref() == Some("::fixture_a")).unwrap();
    assert_eq!((r.target.as_deref(), r.provenance.as_str()), (Some("crates/a/src/lib.rs:54"), "import"), "{r:?}");
    // `Store::new("y")`: the qualifier picks `store.rs`, where there is one `new`.
    let r = rf.iter().find(|r| r.path == main && r.name == "new" && r.qualifier.as_deref() == Some("Store")).unwrap();
    assert_eq!((r.target.as_deref(), r.provenance.as_str()), (Some("crates/a/src/store.rs:18"), "import"), "{r:?}");
    // `HashMap::new()`: the qualifier is std → none, not a guess at some `new`.
    let r = rf.iter().find(|r| r.path == main && r.name == "new" && r.qualifier.as_deref() == Some("HashMap")).unwrap();
    assert_eq!((r.target.as_deref(), r.provenance.as_str()), (None, "none"), "{r:?}");
    assert!(r.candidates >= 2);
    // `impl Store for Registry` in lib.rs: same_file.
    let r = rf.iter().find(|r| r.path == lib && r.name == "Store" && r.kind == "implementation").unwrap();
    assert_eq!((r.target.as_deref(), r.provenance.as_str()), (Some("crates/a/src/lib.rs:21"), "same_file"));
    // `Registry::new()` in lib.rs: two `new` in the fixture tree; the qualifier is a type (unknown head) and
    // the file has one `new` → same_file.
    let r = rf.iter().find(|r| r.path == lib && r.name == "new" && r.qualifier.as_deref() == Some("Registry")).unwrap();
    assert_eq!(r.provenance, "same_file", "{r:?}");
    // `Registry::new()` in store.rs: `crate::Registry` was imported → lib.rs → its one `new`.
    let r = rf.iter().find(|r| r.path == "crates/a/src/store.rs" && r.name == "new").unwrap();
    assert_eq!((r.qualifier.as_deref(), r.target.as_deref(), r.provenance.as_str()), (Some("Registry"), Some("crates/a/src/lib.rs:12"), "import"), "{r:?}");
}

/// 60 §6.1 TS/JS: `tsconfig.json` `paths` alias, relative import, `node:` and
/// bare specifiers external, missing relative file unresolved, `* as ns`
/// qualifier; `require` in JS.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ts_and_js_resolution() {
    let (_d, _ix, ri) = index_fixture("ts").await;
    assert_eq!(packages(&ri).await["src/main.ts"].as_deref(), Some("fixture-ts"));
    let im = import_rows(&ri).await;
    let main = "src/main.ts";
    assert_eq!(import_of(&im, main, "@lib/util"), (Some("src/lib/util.ts"), "tsconfig"));
    assert_eq!(import_of(&im, main, "./app"), (Some("src/app.ts"), "relative"));
    assert_eq!(import_of(&im, main, "node:fs/promises"), (None, "external"));
    assert_eq!(import_of(&im, main, "./missing-side-effect"), (None, "unresolved"));
    let rf = ref_rows(&ri).await;
    let r = ref_of(&rf, main, "fmt");
    assert_eq!((r.target.as_deref(), r.provenance.as_str()), (Some("src/lib/util.ts:2"), "import"), "{r:?}");
    let r = ref_of(&rf, main, "helper2");
    assert_eq!((r.qualifier.as_deref(), r.target.as_deref(), r.provenance.as_str()), (Some("util"), Some("src/lib/util.ts:6"), "import"), "{r:?}");
    let r = ref_of(&rf, main, "Loader");
    assert_eq!((r.kind.as_str(), r.target.as_deref(), r.provenance.as_str()), ("class", Some("src/app.ts:8"), "import"), "{r:?}");
    let r = ref_of(&rf, main, "readFile");
    assert_eq!((r.target.as_deref(), r.provenance.as_str()), (None, "none"));

    let (_d, _ix, ri) = index_fixture("js").await;
    let im = import_rows(&ri).await;
    assert_eq!(import_of(&im, "lib.js", "./util"), (Some("util.js"), "relative"));
    assert_eq!(import_of(&im, "lib.js", "fs"), (None, "external"));
    let rf = ref_rows(&ri).await;
    let r = ref_of(&rf, "lib.js", "double");
    assert_eq!((r.target.as_deref(), r.provenance.as_str()), (Some("util.js:2"), "import"), "{r:?}");
    // `fs.existsSync(".")`: the qualifier is an external import → none.
    let r = ref_of(&rf, "lib.js", "existsSync");
    assert_eq!((r.qualifier.as_deref(), r.provenance.as_str()), (Some("fs"), "none"));
}

/// 60 §6.1 Python: relative (`..store`, `.`), absolute (`pkg`), stdlib
/// external; `files.package` = top package dir.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn python_resolution() {
    let (_d, _ix, ri) = index_fixture("py").await;
    let pk = packages(&ri).await;
    assert_eq!(pk["pkg/sub/app.py"].as_deref(), Some("pkg"));
    let im = import_rows(&ri).await;
    let app = "pkg/sub/app.py";
    assert_eq!(import_of(&im, app, "..store"), (Some("pkg/store.py"), "relative"));
    assert_eq!(import_of(&im, app, "pkg"), (Some("pkg/store.py"), "package"), "`from pkg import store`: the module itself");
    assert_eq!(import_of(&im, app, "."), (Some("pkg/sub/helpers.py"), "relative"), "`from . import helpers`");
    assert_eq!(import_of(&im, app, "os"), (None, "external"));
    assert_eq!(import_of(&im, "pkg/__init__.py", ".store"), (Some("pkg/store.py"), "relative"));
    let rf = ref_rows(&ri).await;
    let r = ref_of(&rf, app, "Store");
    assert_eq!((r.target.as_deref(), r.provenance.as_str()), (Some("pkg/store.py:6"), "import"), "{r:?}");
    let r = ref_of(&rf, app, "run");
    assert_eq!((r.qualifier.as_deref(), r.target.as_deref(), r.provenance.as_str()), (Some("store"), Some("pkg/store.py:28"), "import"), "{r:?}");
    let r = ref_of(&rf, app, "twice");
    assert_eq!((r.qualifier.as_deref(), r.target.as_deref(), r.provenance.as_str()), (Some("helpers"), Some("pkg/sub/helpers.py:4"), "import"), "{r:?}");
    let r = ref_of(&rf, app, "getcwd");
    assert_eq!((r.qualifier.as_deref(), r.provenance.as_str()), (Some("os"), "none"));
}

/// 60 §6.1 Go: `go.mod` module prefix → package directory; alias qualifier;
/// `pkg.Type` qualified types; builtins dropped; stdlib external; missing
/// package unresolved.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn go_resolution() {
    let (_d, _ix, ri) = index_fixture("go").await;
    let pk = packages(&ri).await;
    assert_eq!(pk["pkg/store.go"].as_deref(), Some("pkg"));
    assert_eq!(pk["cmd/main.go"].as_deref(), Some("cmd"));
    let im = import_rows(&ri).await;
    assert_eq!(import_of(&im, "cmd/main.go", "example.com/fixture/pkg"), (Some("pkg/store.go"), "gomod"));
    assert_eq!(import_of(&im, "cmd/main.go", "example.com/fixture/nope"), (None, "unresolved"));
    assert_eq!(import_of(&im, "cmd/main.go", "fmt"), (None, "external"));
    let rf = ref_rows(&ri).await;
    assert!(!rf.iter().any(|r| r.kind == "type" && (r.name == "string" || r.name == "int")), "builtins dropped: {rf:?}");
    let r = ref_of(&rf, "cmd/main.go", "NewStore");
    assert_eq!((r.qualifier.as_deref(), r.target.as_deref(), r.provenance.as_str()), (Some("st"), Some("pkg/store.go:16"), "import"), "{r:?}");
    let r = ref_of(&rf, "cmd/main.go", "Store");
    assert_eq!((r.kind.as_str(), r.qualifier.as_deref(), r.target.as_deref(), r.provenance.as_str()), ("type", Some("st"), Some("pkg/store.go:6"), "import"), "{r:?}");
    let r = ref_of(&rf, "cmd/main.go", "Getter");
    assert_eq!((r.target.as_deref(), r.provenance.as_str()), (Some("pkg/store.go:11"), "import"));
    let r = ref_of(&rf, "cmd/main.go", "Println");
    assert_eq!((r.qualifier.as_deref(), r.provenance.as_str()), (Some("fmt"), "none"));
    // Same package, bare call: `helper(...)` inside `run` in store.go.
    let r = ref_of(&rf, "pkg/store.go", "helper");
    assert_eq!((r.target.as_deref(), r.provenance.as_str()), (Some("pkg/store.go:25"), "same_file"));
}

/// 60 §5.4: touching `store.rs` re-resolves `main.rs` (its importer) — the
/// affected set — and leaves the resolution intact.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn affected_set_is_re_resolved() {
    let (_d, ix, ri) = index_fixture("rust").await;
    let before = ref_rows(&ri).await;
    let target = |rows: &[RefView]| rows.iter().find(|r| r.path == "crates/b/src/main.rs" && r.name == "open_db").unwrap().target.clone();
    assert!(target(&before).is_some());
    let touched = fixtures().join("rust/crates/a/src/store.rs");
    let t0 = std::fs::metadata(&touched).unwrap().modified().unwrap();
    let f = std::fs::File::options().append(true).open(&touched).unwrap();
    f.set_modified(t0 + std::time::Duration::from_secs(1)).unwrap();
    drop(f);
    let r = ix.reconcile(&ri, false).await.unwrap();
    f_restore(&touched, t0);
    assert_eq!((r.reparsed, r.removed), (1, 0), "{r:?}");
    let after = ref_rows(&ri).await;
    // The old symbol row is gone (cascade set the importer's ref to NULL); resolution put it back.
    assert_eq!(target(&after), target(&before));
    assert_eq!(after.len(), before.len());
    let im = import_rows(&ri).await;
    assert_eq!(import_of(&im, "crates/b/src/main.rs", "fixture_a"), (Some("crates/a/src/store.rs"), "cargo"));
}

/// 60 §16 rows 1–2 on this repository: ≥ 290 `.rs` files, `ToolRegistry` in
/// `registry.rs` as `class`, `fs.rs` outline, one touched file → one reparse;
/// imports `cargo` > 500 and `unresolved` < 5 % of non-external; refs by
/// provenance printed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn indexes_this_repository() {
    let data = tempdir();
    let ix = Arc::new(Codeindex::new(CodeindexConfig::default(), &data.0).unwrap());
    let root = repo_root();
    let ri = ix.open_root(&root).await.unwrap();

    let t = std::time::Instant::now();
    let r = ix.reconcile(&ri, false).await.unwrap();
    let first = t.elapsed();
    assert!(!r.stale, "first index of an empty store is synchronous");
    assert!(!r.truncated_walk);
    let c = store::counts(&ri.pool).await.unwrap();
    let rs: i64 = sqlx::query_scalar("SELECT count(*) FROM files WHERE language = 'rust'")
        .fetch_one(&ri.pool)
        .await
        .unwrap();
    eprintln!(
        "first index: {:?}; files {} (rust {rs}); symbols {}; refs {} (resolved {}, ambiguous {})",
        first, c.files, c.symbols, c.refs, c.resolved, c.ambiguous
    );
    assert!(rs >= 290, "rust files: {rs}");
    assert_eq!(r.reparsed as i64, c.files);

    // 60 §16 row 2.
    let by_prov: Vec<(String, i64)> = sqlx::query_as("SELECT provenance, count(*) FROM imports GROUP BY provenance ORDER BY 2 DESC")
        .fetch_all(&ri.pool)
        .await
        .unwrap();
    eprintln!("imports by provenance: {by_prov:?}");
    let n = |p: &str| by_prov.iter().find(|(k, _)| k == p).map_or(0, |(_, n)| *n);
    assert!(n("cargo") > 500, "{by_prov:?}");
    let non_external: i64 = by_prov.iter().filter(|(k, _)| k != "external").map(|(_, n)| n).sum();
    assert!(n("unresolved") * 20 < non_external, "unresolved {} of {non_external}: {by_prov:?}", n("unresolved"));
    let refs_by: Vec<(String, i64)> = sqlx::query_as("SELECT provenance, count(*) FROM refs GROUP BY provenance ORDER BY 2 DESC")
        .fetch_all(&ri.pool)
        .await
        .unwrap();
    eprintln!("refs by provenance: {refs_by:?}");
    assert!(refs_by.iter().any(|(k, n)| k == "import" && *n > 1000), "{refs_by:?}");
    assert!(c.ambiguous > 0 && c.resolved > c.ambiguous);
    // `SessionId::new()` across crates: many `new`s, the one under `impl SessionId` wins (60 §20).
    let owned: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM refs r JOIN symbols s ON s.id = r.to_symbol JOIN symbols p ON p.id = s.parent_id \
         WHERE r.qualifier = 'SessionId' AND r.name = 'new' AND p.name = 'SessionId' AND r.provenance = 'import'",
    )
    .fetch_one(&ri.pool)
    .await
    .unwrap();
    assert!(owned > 10, "SessionId::new resolved by owner: {owned}");

    let hits = store::find(&ri.pool, "ToolRegistry", None, true, 50).await.unwrap();
    assert!(
        hits.iter()
            .any(|h| h.path == "crates/tools/src/registry.rs" && h.kind == "class"),
        "{hits:?}"
    );
    let hits = store::find(&ri.pool, "ToolRegistry", None, false, 50).await.unwrap();
    assert_eq!(hits[0].name, "ToolRegistry", "exact-name first: {:?}", &hits[..3.min(hits.len())]);
    let hits = store::find(&ri.pool, "toolreg", Some("class"), false, 50).await.unwrap();
    assert!(hits.iter().any(|h| h.name == "ToolRegistry"), "prefix: {hits:?}");

    let f = store::file(&ri.pool, "crates/tools/src/fs.rs").await.unwrap().unwrap();
    let rows = store::outline(&ri.pool, f.id).await.unwrap();
    let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
    for n in ["ReadFile", "Grep", "grep", "MAX_BYTES"] {
        assert!(names.contains(&n), "{n} missing from {names:?}");
    }
    // Rust methods sit at module level (impl blocks are not tags); functions
    // inside `mod tests` nest under it.
    assert!(
        rows.iter()
            .any(|r| r.parent_id.is_some() && rows.iter().any(|p| Some(p.id) == r.parent_id && p.name == "tests")),
        "no symbol nested under `mod tests`"
    );

    // Second call: nothing changed, nothing reparsed.
    let r = ix.reconcile(&ri, false).await.unwrap();
    assert_eq!((r.reparsed, r.removed, r.stale), (0, 0, false));

    // Touch one file (mtime only, content unchanged): exactly one reparse.
    let touched = root.join("crates/codeindex/tests/fixtures/rust/crates/a/src/lib.rs");
    let before = std::fs::metadata(&touched).unwrap().modified().unwrap();
    let f = std::fs::File::options().append(true).open(&touched).unwrap();
    f.set_modified(before + std::time::Duration::from_secs(1)).unwrap();
    drop(f);
    let r = ix.reconcile(&ri, false).await.unwrap();
    f_restore(&touched, before);
    assert_eq!((r.reparsed, r.removed), (1, 0), "{r:?}");
    let c2 = store::counts(&ri.pool).await.unwrap();
    assert_eq!((c.files, c.symbols, c.refs, c.resolved), (c2.files, c2.symbols, c2.refs, c2.resolved), "resolution redone for the touched file and its importers");

    let stats = ix.stats(&ri).await.unwrap();
    assert_eq!(stats.files, c.files);
    assert!(stats.last_reconcile.is_some());

    // The store lives in the temp data dir with the §0.7 name, and nowhere else.
    let entries: Vec<_> = std::fs::read_dir(data.0.join("codeindex"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(entries.iter().any(|e| e.ends_with(".sqlite") && e.contains(&codeindex::root::db_file_name(&root))), "{entries:?}");
    assert!(!root.join(".codeindex").exists());
}

fn f_restore(p: &Path, t: std::time::SystemTime) {
    let f = std::fs::File::options().append(true).open(p).unwrap();
    f.set_modified(t).unwrap();
}

/// The three tools through `Tool::call` (60 §8 envelope): exact-name first,
/// budget cut counted, outline nested with resolved imports, unknown path is
/// an error the model reads; `code_refs("RegisterTool")` lists the plugin
/// call sites with `import`/1.0, a bare method name returns candidates and
/// an honest `ambiguous` count, `path:line` and `symbol_id` pick one,
/// `direction=out` lists callees.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tools_envelope() {
    use codeindex::tools::{CodeFindArgs, CodeFindTool, CodeOutlineArgs, CodeOutlineTool, CodeRefsArgs, CodeRefsTool};
    use tools::Tool;
    let data = tempdir();
    let ix = Arc::new(Codeindex::new(CodeindexConfig::default(), &data.0).unwrap());
    let cx = || tools::ToolCx {
        session: runtime::SessionId::new(),
        call_id: String::new(),
        quiet: false,
        resources: tools::SessionResources::durable_arc(&data.0),
    };
    let root = Some(repo_root());

    let out = CodeFindTool(ix.clone())
        .call(CodeFindArgs { name: "ToolRegistry".into(), kind: None, exact: None, root: root.clone(), budget: Some(60) }, cx())
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content);
    let v = out.structured.unwrap();
    assert_eq!(v["rows"][0]["name"], "ToolRegistry");
    assert_eq!(v["rows"][0]["path"], "crates/tools/src/registry.rs");
    assert_eq!(v["rows"][0]["kind"], "class");
    assert_eq!(v["stale"], false);
    assert!(v["dropped"].as_u64().unwrap() > 0, "budget 60 must drop rows: {v}");
    assert!(out.content.contains("… +"), "{}", out.content);
    assert_eq!(v["content"], out.content);

    let out = CodeOutlineTool(ix.clone())
        .call(CodeOutlineArgs { path: "crates/tools/src/fs.rs".into(), root: root.clone(), budget: None }, cx())
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content);
    let v = out.structured.unwrap();
    assert_eq!(v["language"], "rust");
    let imports = v["imports"].as_array().unwrap();
    assert!(!imports.is_empty(), "{v}");
    let ext = imports.iter().find(|i| i["specifier"] == "serde").unwrap_or_else(|| panic!("{imports:?}"));
    assert_eq!(ext["provenance"], "external");
    let local = imports.iter().find(|i| i["provenance"] == "cargo").unwrap_or_else(|| panic!("{imports:?}"));
    assert!(local["target"].as_str().unwrap().starts_with("crates/tools/src/"), "{local}");
    let top: Vec<&str> = v["symbols"].as_array().unwrap().iter().map(|s| s["name"].as_str().unwrap()).collect();
    for n in ["ReadFile", "Grep", "grep", "MAX_BYTES", "tests"] {
        assert!(top.contains(&n), "{n} not top-level in {top:?}");
    }
    let tests = v["symbols"].as_array().unwrap().iter().find(|s| s["name"] == "tests").unwrap();
    assert!(!tests["children"].as_array().unwrap().is_empty(), "mod tests has nested fns");
    assert!(out.content.starts_with("crates/tools/src/fs.rs (rust, "), "{}", out.content);

    let out = CodeOutlineTool(ix.clone())
        .call(CodeOutlineArgs { path: "does/not/exist.rs".into(), root: root.clone(), budget: None }, cx())
        .await
        .unwrap();
    assert!(out.is_error && out.content.contains("does not exist"), "{}", out.content);

    // code_refs("RegisterTool"): 60 §16 row 2.
    let refs = |symbol: &str, direction: Option<&str>, budget: Option<u32>| CodeRefsArgs {
        symbol: symbol.into(),
        direction: direction.map(str::to_owned),
        kind: None,
        root: root.clone(),
        budget,
    };
    let out = CodeRefsTool(ix.clone()).call(refs("RegisterTool", None, None), cx()).await.unwrap();
    assert!(!out.is_error, "{}", out.content);
    let v = out.structured.unwrap();
    eprintln!("code_refs(\"RegisterTool\"):\n{}", out.content);
    assert_eq!(v["definition"]["path"], "crates/tools/src/registry.rs");
    assert_eq!(v["definition"]["kind"], "class");
    let rows = v["refs"].as_array().unwrap();
    for site in ["crates/knowledge/src/tools.rs", "crates/scrape/src/tools.rs", "crates/codeindex/src/tools.rs"] {
        let hit = rows.iter().find(|r| r["path"] == site).unwrap_or_else(|| panic!("{site} missing from {}", out.content));
        assert_eq!((hit["provenance"].as_str(), hit["confidence"].as_f64(), hit["kind"].as_str()), (Some("import"), Some(1.0), Some("call")), "{hit}");
    }
    assert!(v["by_file"].as_array().unwrap().iter().any(|b| b["path"] == "crates/knowledge/src/tools.rs"));
    assert_eq!(v["ambiguous"], 0, "RegisterTool is a unique class name: {v}");

    // A bare method name: candidates, no refs, ambiguous residue counted.
    let out = CodeRefsTool(ix.clone()).call(refs("new", None, None), cx()).await.unwrap();
    assert!(!out.is_error, "{}", out.content);
    let v = out.structured.unwrap();
    assert!(v["candidates"].as_array().unwrap().len() > 10, "{v}");
    assert!(v["refs"].as_array().unwrap().is_empty());
    assert!(v["ambiguous"].as_i64().unwrap() > 0, "{v}");
    assert!(out.content.contains("definitions; pass a symbol_id or path:line"), "{}", out.content);

    // Pick one by symbol_id and by path:line; direction=out.
    let id = v["candidates"][0]["symbol_id"].as_i64().unwrap();
    let out = CodeRefsTool(ix.clone()).call(refs(&id.to_string(), None, Some(200)), cx()).await.unwrap();
    let v = out.structured.unwrap();
    assert_eq!(v["definition"]["symbol_id"], id);
    assert_eq!(v["definition"]["name"], "new");
    let out = CodeRefsTool(ix.clone())
        .call(refs("crates/tools/src/registry.rs:232", Some("out"), None), cx())
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content);
    let v = out.structured.unwrap();
    assert_eq!(v["direction"], "out");
    assert!(v["definition"]["line_start"].as_i64().unwrap() <= 232 && 232 <= v["definition"]["line_end"].as_i64().unwrap(), "{v}");

    let out = CodeRefsTool(ix).call(refs("NoSuchSymbolAnywhere", None, None), cx()).await.unwrap();
    assert!(out.is_error && out.content.contains("no definition"), "{}", out.content);
}

/// 60 §5.3 check: over every parsed file of this repository, `symbols` are
/// identical with and without the locals predicate and `refs` are fewer.
#[test]
fn locals_lower_refs_without_touching_symbols() {
    let root = repo_root();
    let w = codeindex::walk::walk(&root, 1 << 20);
    let mut ctx = Parsers::default();
    let (mut with, mut without) = ((0usize, 0usize), (0usize, 0usize));
    let mut cfgs = HashMap::new();
    for e in &w.entries {
        if e.language == Language::Other {
            continue;
        }
        let (a, b) = cfgs
            .entry(e.language)
            .or_insert_with(|| (e.language.build(true).unwrap(), e.language.build(false).unwrap()));
        let src = std::fs::read(root.join(&e.path)).unwrap();
        let Some(pa) = parse(&mut ctx, a, e.language, &src) else { continue };
        let pb = parse(&mut ctx, b, e.language, &src).unwrap();
        assert_eq!(defs(&pa), defs(&pb), "{}", e.path);
        with = (with.0 + pa.symbols.len(), with.1 + pa.refs.len());
        without = (without.0 + pb.symbols.len(), without.1 + pb.refs.len());
    }
    eprintln!("with locals: symbols {} refs {}; without: symbols {} refs {}", with.0, with.1, without.0, without.1);
    assert_eq!(with.0, without.0);
    assert!(with.1 < without.1, "{with:?} vs {without:?}");
}
