//! Fixture trees per language, and this repository indexed into a temp data dir
//! (60 §16 row 1). Nothing is written inside the repo.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use codeindex::lang::Language;
use codeindex::parse::{ParsedFile, parse};
use codeindex::{Codeindex, CodeindexConfig, store};
use tree_sitter_tags::TagsContext;

fn fixture(rel: &str) -> Vec<u8> {
    std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(rel)).unwrap()
}

/// Parse a fixture with the production (locals-on, `'static`) configuration.
fn parsed(lang: Language, rel: &str) -> ParsedFile<'static> {
    let mut ctx = TagsContext::new();
    parse(&mut ctx, lang.tags().unwrap(), lang, &fixture(rel)).unwrap()
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
    let mut ctx = TagsContext::new();
    let without = parse(&mut ctx, &no_locals, lang, &fixture(rel)).unwrap();
    assert_eq!(defs(&with), defs(&without), "{lang:?}: definitions differ");
    assert!(ref_names(&without).contains(&"f"), "{lang:?}: no `f` ref even without locals");
    assert!(!ref_names(&with).contains(&"f"), "{lang:?}: parameter call kept: {:?}", ref_names(&with));
    assert!(ref_names(&with).contains(&"helper"), "{lang:?}: real call dropped");
    assert!(with.refs.len() < without.refs.len());
}

#[test]
fn rust_fixture() {
    let p = parsed(Language::Rust, "rust/src/lib.rs");
    let d = defs(&p);
    assert!(d.contains(&("Registry", "class", None)));
    assert!(d.contains(&("new", "method", None)), "{d:?}");
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
    assert!(p.refs.iter().any(|r| r.name == "new" && r.kind == "call"), "Registry::new() is a call");
    assert_locals_filter(Language::Rust, "rust/src/lib.rs");
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

/// 60 §16 row 1 on this repository: ≥ 290 `.rs` files, `ToolRegistry` in
/// `registry.rs` as `class`, `fs.rs` outline, one touched file → one reparse.
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
        "first index: {:?}; files {} (rust {rs}); symbols {}; refs {}",
        first, c.files, c.symbols, c.refs
    );
    assert!(rs >= 290, "rust files: {rs}");
    assert_eq!(r.reparsed as i64, c.files);

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
    let touched = root.join("crates/codeindex/tests/fixtures/rust/src/lib.rs");
    let before = std::fs::metadata(&touched).unwrap().modified().unwrap();
    let f = std::fs::File::options().append(true).open(&touched).unwrap();
    f.set_modified(before + std::time::Duration::from_secs(1)).unwrap();
    drop(f);
    let r = ix.reconcile(&ri, false).await.unwrap();
    f_restore(&touched, before);
    assert_eq!((r.reparsed, r.removed), (1, 0), "{r:?}");
    let c2 = store::counts(&ri.pool).await.unwrap();
    assert_eq!((c.files, c.symbols, c.refs), (c2.files, c2.symbols, c2.refs));

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

/// The two tools through `Tool::call` (60 §8 envelope): exact-name first,
/// budget cut counted, outline nested, unknown path is an error the model reads.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tools_envelope() {
    use codeindex::tools::{CodeFindArgs, CodeFindTool, CodeOutlineArgs, CodeOutlineTool};
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
    assert_eq!(v["imports"].as_array().unwrap().len(), 0);
    let top: Vec<&str> = v["symbols"].as_array().unwrap().iter().map(|s| s["name"].as_str().unwrap()).collect();
    for n in ["ReadFile", "Grep", "grep", "MAX_BYTES", "tests"] {
        assert!(top.contains(&n), "{n} not top-level in {top:?}");
    }
    let tests = v["symbols"].as_array().unwrap().iter().find(|s| s["name"] == "tests").unwrap();
    assert!(!tests["children"].as_array().unwrap().is_empty(), "mod tests has nested fns");
    assert!(out.content.starts_with("crates/tools/src/fs.rs (rust, "), "{}", out.content);

    let out = CodeOutlineTool(ix)
        .call(CodeOutlineArgs { path: "does/not/exist.rs".into(), root, budget: None }, cx())
        .await
        .unwrap();
    assert!(out.is_error && out.content.contains("does not exist"), "{}", out.content);
}

/// 60 §5.3 check: over every parsed file of this repository, `symbols` are
/// identical with and without the locals predicate and `refs` are fewer.
#[test]
fn locals_lower_refs_without_touching_symbols() {
    let root = repo_root();
    let w = codeindex::walk::walk(&root, 1 << 20);
    let mut ctx = TagsContext::new();
    let (mut with, mut without) = ((0usize, 0usize), (0usize, 0usize));
    let mut cfgs = std::collections::HashMap::new();
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
