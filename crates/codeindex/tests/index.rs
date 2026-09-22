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
    // `self.items.iter().find(..).cloned()`: the prefix has calls in it → the receiver marker `.`.
    assert!(p.refs.iter().filter(|r| r.name == "cloned").all(|r| r.qualifier.as_deref() == Some(".")));
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
    // `const { resolve, reject } = …` / `const [first, second] = …`: destructured bindings are locals.
    let names = ref_names(&p);
    for local in ["reject", "first", "second"] {
        assert!(!names.contains(&local), "destructured local {local} kept: {names:?}");
    }
    assert!(names.contains(&"setTimeout"));
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
    // Receiver calls (60 §21): `opts.clone_from()` is stoplisted → not attributed to the one `fn clone_from`;
    // `opts.describe()` → the one `describe` in the package at 0.4; `opts.summarize()` → root-wide unique at 0.3.
    let r = rf.iter().find(|r| r.path == main && r.name == "clone_from").unwrap();
    assert_eq!((r.qualifier.as_deref(), r.target.as_deref(), r.provenance.as_str(), r.candidates), (Some("opts"), None, "ambiguous", 1), "{r:?}");
    let r = ref_of(&rf, main, "describe");
    assert_eq!((r.target.as_deref(), r.provenance.as_str(), r.confidence), (Some("crates/b/src/cli.rs:19"), "package", 0.4), "{r:?}");
    let r = ref_of(&rf, main, "summarize");
    assert_eq!((r.target.as_deref(), r.provenance.as_str(), r.confidence), (Some("crates/a/src/store/sub.rs:11"), "unique", 0.3), "{r:?}");
    // `impl<T> Wrapper<T>`: the generic header is an implementation tag, so the method has an owner.
    let owner: Option<String> =
        sqlx::query_scalar("SELECT p.name FROM symbols s JOIN symbols p ON p.id = s.parent_id WHERE s.name = 'unwrap_inner'")
            .fetch_optional(&ri.pool)
            .await
            .unwrap();
    assert_eq!(owner.as_deref(), Some("Wrapper"));
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
        "first index: {:?}; files {} (rust {rs}); symbols {}; refs {} (resolved {}, ambiguous {}); co-change walk (commits, ms): {:?}",
        first, c.files, c.symbols, c.refs, c.resolved, c.ambiguous, r.cochange_walk
    );
    assert!(r.cochange_walk.is_some_and(|(n, _)| n > 100), "{:?}", r.cochange_walk);
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
    let t = std::time::Instant::now();
    let r = ix.reconcile(&ri, false).await.unwrap();
    eprintln!("second call (nothing changed): {:?}", t.elapsed());
    assert_eq!((r.reparsed, r.removed, r.stale, r.cochange_walk), (0, 0, false, None), "HEAD unchanged → no second walk");

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
    assert!(stats.last_reconcile_at.is_some() && stats.rank_iterations.is_some() && stats.rank_ms.is_some());
    assert!(stats.cochange_pairs > 0 && stats.git_head.is_some(), "co-change walk on this repo: {stats:?}");
    eprintln!("codeindex_stats: {}", serde_json::to_string_pretty(&stats).unwrap());

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

/// 60 §16 row 3 on this repository, through `Tool::call`: `code_map(budget=1500)`
/// fits 1 725 estimated tokens; `focus` personalises; `code_symbol` on the
/// knowledge `open_db` returns definition + body + callers + callees in one
/// call; `code_find("new")` is ordered by rank; `code_refs("clone")` after the
/// stoplist; rank compute time and PageRank iterations printed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn map_and_symbol() {
    use codeindex::tools::{CodeFindArgs, CodeFindTool, CodeMapArgs, CodeMapTool, CodeRefsArgs, CodeRefsTool, CodeSymbolArgs, CodeSymbolTool};
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
    let map = |focus: Option<Vec<&str>>, paths_only: bool, budget: u32| CodeMapArgs {
        focus: focus.map(|v| v.into_iter().map(str::to_owned).collect()),
        paths_only: Some(paths_only),
        root: root.clone(),
        budget: Some(budget),
    };

    let out = CodeMapTool(ix.clone()).call(map(None, false, 1500), cx()).await.unwrap();
    assert!(!out.is_error, "{}", out.content);
    let v = out.structured.unwrap();
    let tokens = v["estimated_tokens"].as_u64().unwrap();
    eprintln!("code_map(budget=1500): {tokens} estimated tokens, {} rows, {} dropped\n{}", v["rows"].as_array().unwrap().len(), v["dropped"], out.content);
    assert!(tokens <= 1725, "{tokens} > 1725");
    assert!(tokens >= 1000, "under-filled map: {tokens}");
    let rows = v["rows"].as_array().unwrap();
    assert!(!rows.is_empty() && rows[0]["rank"].as_f64().unwrap() > 0.0, "{v}");
    assert!(rows.iter().all(|r| r["rank"].as_f64().unwrap() > 0.0));

    // Ranked files (the unpersonalised `files.rank` lookup), top 15.
    let out = CodeMapTool(ix.clone()).call(map(None, true, 400), cx()).await.unwrap();
    let v = out.structured.unwrap();
    let top: Vec<String> = v["rows"].as_array().unwrap().iter().take(15).map(|r| format!("{} {:.5}", r["path"].as_str().unwrap(), r["rank"].as_f64().unwrap())).collect();
    eprintln!("code_map(paths_only) top 15:\n{}", top.join("\n"));
    for f in ["crates/tools/src/lib.rs", "crates/runtime/src/lib.rs", "crates/agent/src/loop_.rs"] {
        eprintln!("{f}: {}", if top.iter().any(|t| t.starts_with(f)) { "in top 15" } else { "NOT in top 15" });
    }
    assert!(top.iter().any(|t| t.starts_with("crates/tools/src/lib.rs")), "{top:?}");

    // Focus on an identifier: `crates/scrape/*` rises.
    let out = CodeMapTool(ix.clone()).call(map(Some(vec!["ScrapeUrlTool"]), true, 400), cx()).await.unwrap();
    let v = out.structured.unwrap();
    let top5: Vec<&str> = v["rows"].as_array().unwrap().iter().take(5).map(|r| r["path"].as_str().unwrap()).collect();
    eprintln!("code_map(focus=[ScrapeUrlTool]) top 5: {top5:?}; iterations {}", v["pagerank_iterations"]);
    assert_eq!(v["focus_idents"][0], "ScrapeUrlTool");
    eprintln!("crates/scrape in top 5: {}", top5.iter().any(|p| p.starts_with("crates/scrape/")));
    let out = CodeMapTool(ix.clone()).call(map(Some(vec!["ScrapeUrlTool"]), false, 1500), cx()).await.unwrap();
    let v = out.structured.unwrap();
    let files: Vec<&str> = out.content.lines().filter(|l| !l.starts_with(' ')).take(5).collect();
    eprintln!("code_map(focus=[ScrapeUrlTool]) map, first files: {files:?}");
    assert!(files.iter().any(|p| p.starts_with("crates/scrape/")), "{}", out.content);
    assert!(v["estimated_tokens"].as_u64().unwrap() <= 1725);
    // Focus on a path: the file itself and what it references lead.
    let out = CodeMapTool(ix.clone()).call(map(Some(vec!["crates/scrape/src/tools.rs"]), true, 400), cx()).await.unwrap();
    let v = out.structured.unwrap();
    assert_eq!(v["focus_files"][0], "crates/scrape/src/tools.rs");
    assert_eq!(v["rows"][0]["path"], "crates/scrape/src/tools.rs", "{v}");

    // code_symbol on the knowledge `open_db` (60 §16 row 3).
    let out = CodeSymbolTool(ix.clone())
        .call(CodeSymbolArgs { symbol: "crates/knowledge/src/store.rs:407".into(), sections: None, root: root.clone(), budget: None }, cx())
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content);
    let v = out.structured.unwrap();
    eprintln!("code_symbol(\"crates/knowledge/src/store.rs:407\"):\n{}", out.content);
    assert_eq!(v["definition"]["name"], "open_db");
    assert!(v["body"].as_str().unwrap().contains("fn open_db"), "{v}");
    assert!(!v["callers"].as_array().unwrap().is_empty(), "{v}");
    // `open_db`'s body calls only sqlx/std → no resolvable callee; the section is present and honest.
    let callees = v["callees"].as_array().unwrap();
    assert!(callees.iter().all(|c| c["target"].as_str().unwrap().contains(':')));
    assert!(out.content.contains("-- callees:"), "{}", out.content);
    assert!(u32::try_from(out.content.len() / 3).unwrap() <= 4000 + 600, "{}", out.content.len());
    let out = CodeSymbolTool(ix.clone())
        .call(CodeSymbolArgs { symbol: "open_db".into(), sections: Some(vec!["cochange".into()]), root: root.clone(), budget: None }, cx())
        .await
        .unwrap();
    let v = out.structured.unwrap();
    assert!(v["candidates"].as_array().unwrap().len() >= 2, "several open_db → candidates: {v}");
    let out = CodeSymbolTool(ix.clone())
        .call(CodeSymbolArgs { symbol: "crates/knowledge/src/store.rs:407".into(), sections: Some(vec!["cochange".into(), "bogus".into()]), root: root.clone(), budget: None }, cx())
        .await
        .unwrap();
    assert!(out.is_error && out.content.contains("unknown section"), "{}", out.content);

    // code_find("new"): ordered by rank, top hit is a ranked definition.
    let out = CodeFindTool(ix.clone())
        .call(CodeFindArgs { name: "new".into(), kind: None, exact: Some(true), root: root.clone(), budget: None }, cx())
        .await
        .unwrap();
    let v = out.structured.unwrap();
    let rows = v["rows"].as_array().unwrap();
    let ranks: Vec<f64> = rows.iter().map(|r| r["rank"].as_f64().unwrap()).collect();
    eprintln!("code_find(\"new\") top: {} {} rank {}", rows[0]["path"], rows[0]["signature"], ranks[0]);
    assert!(ranks[0] > 0.0 && ranks.windows(2).all(|w| w[0] >= w[1]), "{ranks:?}");

    // code_refs("clone") after the stoplist; the count before it is the resolved rows plus the
    // receiver calls now left ambiguous with one candidate (rules 3–4 used to take those).
    let out = CodeRefsTool(ix.clone())
        .call(CodeRefsArgs { symbol: "clone".into(), direction: None, kind: None, root: root.clone(), budget: Some(600) }, cx())
        .await
        .unwrap();
    let by_prov: Vec<(String, i64, i64)> =
        sqlx::query_as("SELECT provenance, count(*), min(candidates) FROM refs WHERE name = 'clone' GROUP BY provenance ORDER BY 2 DESC")
            .fetch_all(&ix.open_root(&repo_root()).await.unwrap().pool)
            .await
            .unwrap();
    eprintln!("code_refs(\"clone\"):\n{}\nrefs named clone by provenance (provenance, count, min candidates): {by_prov:?}", out.content);
    let v = out.structured.unwrap();
    if let Some(refs) = v["refs"].as_array() {
        assert!(refs.iter().all(|r| r["provenance"] != "unique" && r["provenance"] != "package"), "{}", out.content);
    }

    // Rank compute cost on this repo.
    let ri = ix.open_root(&repo_root()).await.unwrap();
    sqlx::query("UPDATE files SET rank = 0.0").execute(&ri.pool).await.unwrap();
    let t = std::time::Instant::now();
    let (nodes, cold_iter) = codeindex::rank::persist(&ri.pool).await.unwrap();
    let cold = t.elapsed();
    let t = std::time::Instant::now();
    let (_, warm_iter) = codeindex::rank::persist(&ri.pool).await.unwrap();
    let warm = t.elapsed();
    eprintln!("rank::persist: {nodes} nodes; cold {cold_iter} iterations {cold:?}; warm {warm_iter} iterations {warm:?}");
    let t = std::time::Instant::now();
    let g = codeindex::rank::Graph::load(&ri.pool).await.unwrap();
    let load = t.elapsed();
    let t = std::time::Instant::now();
    let personalised = g.rank(&g.focus(&["crates/scrape/src/tools.rs".to_owned()]));
    eprintln!("graph load {load:?}; personalised rank {:?}, {} iterations", t.elapsed(), personalised.iterations);
    assert!(cold.as_millis() < 500, "rank persist must be < 0.5 s: {cold:?}");
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

fn git(dir: &Path, args: &[&str]) {
    let st = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@x")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@x")
        .status()
        .unwrap();
    assert!(st.success(), "git {args:?}");
}

fn cx(data: &TempDir) -> tools::ToolCx {
    tools::ToolCx {
        session: runtime::SessionId::new(),
        call_id: String::new(),
        quiet: false,
        resources: tools::SessionResources::durable_arc(&data.0),
    }
}

/// 60 §16 row 4 on a fixture repository: two commits, the second changing
/// one function (`helper`) that another file calls and a test uses. Ranges,
/// hunk → definition mapping, `files` mode, `WORKTREE`, direct/transitive/
/// tests, `ambiguous_skipped`, and the `cochange` section of `code_symbol`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn impact_on_a_fixture_repository() {
    use codeindex::tools::{CodeImpactArgs, CodeImpactTool, CodeSymbolArgs, CodeSymbolTool};
    use tools::Tool;
    let repo = tempdir();
    let d = repo.0.canonicalize().unwrap();
    git(&d, &["init", "-q", "-b", "main"]);
    std::fs::create_dir_all(d.join("src")).unwrap();
    std::fs::create_dir_all(d.join("tests")).unwrap();
    std::fs::write(d.join("Cargo.toml"), "[package]\nname = \"fixture-impact\"\nversion = \"0.1.0\"\nedition = \"2024\"\n").unwrap();
    std::fs::write(d.join("src/util.rs"), "pub fn helper() -> u32 {\n    1\n}\n\npub fn unrelated() -> u32 {\n    2\n}\n").unwrap();
    std::fs::write(d.join("src/lib.rs"), "pub mod util;\nuse crate::util::helper;\n\npub fn run() -> u32 {\n    helper()\n}\n").unwrap();
    std::fs::write(d.join("src/top.rs"), "use crate::run;\n\npub fn top() -> u32 {\n    run()\n}\n").unwrap();
    std::fs::write(d.join("tests/it.rs"), "use fixture_impact::util::helper;\n\n#[test]\nfn t() {\n    assert_eq!(helper(), 1);\n}\n").unwrap();
    std::fs::write(d.join("notes.yaml"), "a: 1\n").unwrap();
    git(&d, &["add", "."]);
    git(&d, &["commit", "-q", "-m", "one"]);
    // Second commit: `helper`'s body and `notes.yaml`; `unrelated` untouched.
    std::fs::write(d.join("src/util.rs"), "pub fn helper() -> u32 {\n    1 + 0\n}\n\npub fn unrelated() -> u32 {\n    2\n}\n").unwrap();
    std::fs::write(d.join("notes.yaml"), "a: 2\n").unwrap();
    git(&d, &["add", "."]);
    git(&d, &["commit", "-q", "-m", "two"]);
    // `mod top;` is missing on purpose: `src/top.rs` imports through the crate root only.
    std::fs::write(d.join("src/lib.rs"), "pub mod util;\npub mod top;\nuse crate::util::helper;\n\npub fn run() -> u32 {\n    helper()\n}\n").unwrap();
    git(&d, &["add", "."]);
    git(&d, &["commit", "-q", "-m", "three"]);

    let data = tempdir();
    let ix = Arc::new(Codeindex::new(CodeindexConfig::default(), &data.0).unwrap());
    let args = |files: Option<Vec<&str>>, range: Option<&str>, include_tests: bool| CodeImpactArgs {
        files: files.map(|v| v.into_iter().map(PathBuf::from).collect()),
        git_range: range.map(str::to_owned),
        include_tests: Some(include_tests),
        root: Some(d.clone()),
        budget: None,
    };
    let tool = CodeImpactTool(ix.clone());
    let call = |a: CodeImpactArgs| tool.call(a, cx(&data));

    // The commit that changed `helper`: only `helper` is touched, `unrelated` is not.
    let out = call(args(None, Some("HEAD~1"), false)).await.unwrap();
    assert!(!out.is_error, "{}", out.content);
    let v = out.structured.unwrap();
    eprintln!("code_impact(git_range=HEAD~1) on the fixture:\n{}", out.content);
    let changed = v["changed"].as_array().unwrap();
    let util = changed.iter().find(|c| c["path"] == "src/util.rs").unwrap();
    let names: Vec<&str> = util["symbols_changed"].as_array().unwrap().iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["helper"], "{v}");
    assert_eq!(util["status"], "modified");
    let yaml = changed.iter().find(|c| c["path"] == "notes.yaml").unwrap();
    assert!(yaml["symbols_changed"].as_array().unwrap().is_empty(), "files without symbols are listed: {v}");
    let direct = v["direct"].as_array().unwrap();
    let lib = direct.iter().find(|d| d["path"] == "src/lib.rs").expect("lib.rs references helper");
    let via: Vec<&str> = lib["via"].as_array().unwrap().iter().map(|s| s.as_str().unwrap()).collect();
    assert!(via.contains(&"helper") && via.contains(&"<import>"), "{via:?}");
    assert!(!direct.iter().any(|d| d["path"] == "tests/it.rs"), "tests stay out of direct by default: {v}");
    let tests: Vec<&str> = v["tests"].as_array().unwrap().iter().map(|s| s.as_str().unwrap()).collect();
    assert_eq!(tests, ["tests/it.rs"]);
    let transitive: Vec<(&str, u64)> = v["transitive"].as_array().unwrap().iter().map(|t| (t["path"].as_str().unwrap(), t["depth"].as_u64().unwrap())).collect();
    assert_eq!(transitive, [("src/top.rs", 1)], "top.rs imports lib.rs which references helper: {v}");
    assert_eq!(v["ambiguous_skipped"], 0);
    assert_eq!(v["mode"], "range");
    assert!(v["range"]["old"].is_string() && v["range"]["new"].is_string());

    // include_tests: the test file joins direct too.
    let out = call(args(None, Some("HEAD~2..HEAD~1"), true)).await.unwrap();
    let v = out.structured.unwrap();
    assert!(v["direct"].as_array().unwrap().iter().any(|d| d["path"] == "tests/it.rs" && d["is_test"] == true), "{v}");

    // files mode: every definition of the file counts.
    let out = call(args(Some(vec!["src/util.rs"]), None, false)).await.unwrap();
    let v = out.structured.unwrap();
    let names: Vec<&str> = v["changed"][0]["symbols_changed"].as_array().unwrap().iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["helper", "unrelated"], "{v}");
    assert_eq!(v["mode"], "files");

    // WORKTREE: nothing dirty → nothing changed; dirty `top.rs` → its definition.
    let out = call(args(None, Some("WORKTREE"), false)).await.unwrap();
    let v = out.structured.unwrap();
    assert!(v["changed"].as_array().unwrap().is_empty(), "{v}");
    std::fs::write(d.join("src/top.rs"), "use crate::run;\n\npub fn top() -> u32 {\n    run() + 1\n}\n").unwrap();
    std::fs::write(d.join("src/fresh.rs"), "pub fn fresh() {}\n").unwrap();
    let out = call(args(None, Some("WORKTREE"), false)).await.unwrap();
    let v = out.structured.unwrap();
    eprintln!("code_impact(git_range=WORKTREE):\n{}", out.content);
    let rows: Vec<(&str, &str, Vec<&str>)> = v["changed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| (c["path"].as_str().unwrap(), c["status"].as_str().unwrap(), c["symbols_changed"].as_array().unwrap().iter().map(|s| s["name"].as_str().unwrap()).collect()))
        .collect();
    assert_eq!(rows, [("src/fresh.rs", "added", vec!["fresh"]), ("src/top.rs", "modified", vec!["top"])], "{v}");

    // Argument errors are content.
    let out = call(args(Some(vec!["src/util.rs"]), Some("HEAD"), false)).await.unwrap();
    assert!(out.is_error && out.content.contains("not both"));
    let out = call(args(None, None, false)).await.unwrap();
    assert!(out.is_error);
    let out = call(args(None, Some("no-such-rev"), false)).await.unwrap();
    assert!(out.is_error && out.content.contains("unknown revision"), "{}", out.content);
    let out = call(args(Some(vec!["nope.rs"]), None, false)).await.unwrap();
    assert!(out.is_error && out.content.contains("does not exist"), "{}", out.content);

    // Co-change: commit two changed util.rs + notes.yaml, three changed lib.rs alone (below the 2-file floor),
    // one added five files. `code_symbol(helper, cochange)` lists notes.yaml (2) and the rest (1).
    let out = CodeSymbolTool(ix.clone())
        .call(CodeSymbolArgs { symbol: "helper".into(), sections: Some(vec!["cochange".into()]), root: Some(d.clone()), budget: None }, cx(&data))
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content);
    let v = out.structured.unwrap();
    eprintln!("code_symbol(helper, cochange):\n{}", out.content);
    let partners: Vec<(&str, u64)> = v["cochange"].as_array().unwrap().iter().map(|p| (p["path"].as_str().unwrap(), p["count"].as_u64().unwrap())).collect();
    assert_eq!(partners[0], ("notes.yaml", 2), "{partners:?}");
    assert!(partners.contains(&("src/lib.rs", 1)) && partners.contains(&("tests/it.rs", 1)), "{partners:?}");
    assert!(v.get("note").is_none());
    let ri = ix.open_root(&d).await.unwrap();
    let stats = ix.stats(&ri).await.unwrap();
    assert!(stats.cochange_pairs >= 10 && stats.git_head.is_some(), "{stats:?}");
    // A new commit moves HEAD → the next call re-walks; `pending` fields stay 0.
    git(&d, &["add", "."]);
    git(&d, &["commit", "-q", "-m", "four"]);
    let r = ix.reconcile(&ri, false).await.unwrap();
    assert!(r.cochange_walk.is_some_and(|(n, _)| n == 4), "{r:?}");
    let stats2 = ix.stats(&ri).await.unwrap();
    assert_ne!(stats.git_head, stats2.git_head);
}

/// 60 §16 row 4 on this repository: `code_impact` of the commit that landed
/// step 3 names the symbols it added and their referencers; `files` mode on
/// `crates/tools/src/lib.rs` lists the plugins as `direct`; `cochanges` is
/// non-empty and `code_symbol(…, cochange)` lists partners. Numbers printed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn impact_on_this_repository() {
    use codeindex::tools::{CodeImpactArgs, CodeImpactTool, CodeSymbolArgs, CodeSymbolTool};
    use tools::Tool;
    let data = tempdir();
    let ix = Arc::new(Codeindex::new(CodeindexConfig::default(), &data.0).unwrap());
    let root = repo_root();
    let args = |files: Option<Vec<&str>>, range: Option<&str>| CodeImpactArgs {
        files: files.map(|v| v.into_iter().map(PathBuf::from).collect()),
        git_range: range.map(str::to_owned),
        include_tests: None,
        root: Some(root.clone()),
        budget: Some(2500),
    };
    // Step 3's commit: `176bf52^..176bf52`. Skipped (not failed) when the object is missing (shallow clone).
    let t = std::time::Instant::now();
    let out = CodeImpactTool(ix.clone()).call(args(None, Some("176bf52")), cx(&data)).await.unwrap();
    let first = t.elapsed();
    if out.is_error && out.content.contains("unknown revision") {
        eprintln!("176bf52 not in this clone; skipping the step-3 commit check");
    } else {
        assert!(!out.is_error, "{}", out.content);
        let v = out.structured.unwrap();
        eprintln!("code_impact(git_range=\"176bf52\") ({first:?} incl. first index):\n{}", out.content);
        let rank_syms: Vec<&str> = v["changed"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["path"] == "crates/codeindex/src/rank.rs")
            .flat_map(|c| c["symbols_changed"].as_array().unwrap().iter().map(|s| s["name"].as_str().unwrap()))
            .collect();
        for s in ["Graph", "persist", "render", "rank"] {
            assert!(rank_syms.contains(&s), "rank.rs symbol {s} missing: {rank_syms:?}");
        }
        let direct: Vec<(&str, Vec<&str>, bool)> = v["direct"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| (d["path"].as_str().unwrap(), d["via"].as_array().unwrap().iter().map(|s| s.as_str().unwrap()).collect(), d["changed"].as_bool().unwrap()))
            .collect();
        // `index.rs` and `tools.rs` are in the diff themselves: listed as direct, flagged `changed`.
        let index_rs = direct.iter().find(|(p, ..)| *p == "crates/codeindex/src/index.rs").expect("index.rs calls rank::persist");
        assert!(index_rs.1.contains(&"persist") && index_rs.2, "{direct:?}");
        assert!(direct.iter().any(|(p, via, c)| *p == "crates/codeindex/src/tools.rs" && *c && via.iter().any(|v| *v == "Graph" || *v == "render")), "{direct:?}");
        assert!(direct.iter().any(|(_, _, c)| !*c), "files outside the diff import the changed ones: {direct:?}");
        assert!(direct.windows(2).all(|w| !w[0].2 || w[1].2), "outside-the-diff files first: {direct:?}");
    }
    let t = std::time::Instant::now();
    let out = CodeImpactTool(ix.clone()).call(args(Some(vec!["crates/tools/src/lib.rs"]), None), cx(&data)).await.unwrap();
    assert!(!out.is_error, "{}", out.content);
    let v = out.structured.unwrap();
    let direct = v["direct"].as_array().unwrap();
    let top10: Vec<String> = direct.iter().take(10).map(|d| format!("{} via {}", d["path"], d["via"])).collect();
    eprintln!("code_impact(files=[crates/tools/src/lib.rs]) in {:?}: {} direct, {} transitive, {} tests; top 10:\n{}", t.elapsed(), direct.len(), v["transitive"].as_array().unwrap().len(), v["tests"].as_array().unwrap().len(), top10.join("\n"));
    for p in ["crates/knowledge/src/tools.rs", "crates/scrape/src/tools.rs"] {
        let d = direct.iter().find(|d| d["path"] == p).unwrap_or_else(|| panic!("{p} not direct"));
        assert!(d["via"].as_array().unwrap().iter().any(|v| v == "Tool"), "{d}");
    }

    let ri = ix.open_root(&root).await.unwrap();
    let pairs: i64 = sqlx::query_scalar("SELECT count(*) FROM cochanges").fetch_one(&ri.pool).await.unwrap();
    let top: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT a.path, b.path, c.count FROM cochanges c JOIN files a ON a.id = c.file_a JOIN files b ON b.id = c.file_b ORDER BY c.count DESC, a.path, b.path LIMIT 5",
    )
    .fetch_all(&ri.pool)
    .await
    .unwrap();
    eprintln!("cochanges: {pairs} pairs; top 5: {top:?}");
    assert!(pairs > 0);
    let out = CodeSymbolTool(ix.clone())
        .call(CodeSymbolArgs { symbol: "crates/knowledge/src/store.rs:407".into(), sections: Some(vec!["cochange".into()]), root: Some(root.clone()), budget: None }, cx(&data))
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content);
    let v = out.structured.unwrap();
    eprintln!("code_symbol(\"crates/knowledge/src/store.rs:407\", cochange):\n{}", out.content);
    assert!(!v["cochange"].as_array().unwrap().is_empty(), "{v}");
    assert!(v.get("note").is_none());
}
