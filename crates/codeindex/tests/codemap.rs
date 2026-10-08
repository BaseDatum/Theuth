//! Design 73 E1–E2: `CodeToolRan` / `CodeReconciled` on the bus and the
//! `codemap_*` lenses, over a scratch copy of the rust fixture so `touch`
//! can edit files. Nothing is written inside the repo.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use codeindex::lens::{self, Change, Fit, Here, Touch};
use codeindex::tools::{CodeFindArgs, CodeFindTool, CodeRefsArgs, CodeRefsTool};
use codeindex::{Codeindex, CodeindexConfig, CodeindexEvent, RootIndex, store};
use kameo::actor::{ActorRef, Spawn};
use kameo::message::{Context, Message};
use parking_lot::Mutex;
use runtime::Runtime;
use tools::Tool;

struct TempDir(PathBuf);
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn tempdir() -> TempDir {
    let d = std::env::temp_dir().join(format!("theuth-codemap-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&d).unwrap();
    TempDir(d)
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let dst = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &dst);
        } else {
            std::fs::copy(e.path(), dst).unwrap();
        }
    }
}

/// Every `CodeindexEvent` the bus delivered, with its arrival time.
type Seen = Arc<Mutex<Vec<(Instant, CodeindexEvent)>>>;

#[derive(kameo::Actor)]
struct Sink(Seen);

impl Message<CodeindexEvent> for Sink {
    type Reply = ();
    async fn handle(&mut self, ev: CodeindexEvent, _: &mut Context<Self, ()>) {
        self.0.lock().push((Instant::now(), ev));
    }
}

struct Scratch {
    _data: TempDir,
    _repo: TempDir,
    root: PathBuf,
    ix: Arc<Codeindex>,
    ri: Arc<RootIndex>,
    seen: Seen,
    _sink: ActorRef<Sink>,
}

/// A fresh runtime + bus, the rust fixture copied to a temp root, indexed once
/// (the first `CodeReconciled` is consumed here).
async fn scratch() -> Scratch {
    let data = tempdir();
    let repo = tempdir();
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rust"),
        &repo.0,
    );
    let rt = Runtime::new(figment::Figment::new(), data.0.clone());
    let seen: Seen = Arc::default();
    let sink = Sink::spawn(Sink(seen.clone()));
    rt.subscribe::<Sink, CodeindexEvent>(&sink).await;
    let ix = Arc::new(
        Codeindex::new(CodeindexConfig::default(), &data.0)
            .unwrap()
            .with_runtime(rt),
    );
    let root = repo.0.canonicalize().unwrap();
    let ri = ix.open_root(&root).await.unwrap();
    let r = ix.reconcile(&ri, false).await.unwrap();
    assert!(!r.stale && r.reparsed > 0, "{r:?}");
    wait_for(&seen, |v| v.len() == 1).await;
    seen.lock().clear();
    Scratch {
        _data: data,
        _repo: repo,
        root,
        ix,
        ri,
        seen,
        _sink: sink,
    }
}

async fn wait_for(seen: &Seen, ok: impl Fn(&[(Instant, CodeindexEvent)]) -> bool) {
    let t = Instant::now();
    while !ok(&seen.lock()) {
        assert!(
            t.elapsed() < Duration::from_secs(5),
            "timed out waiting for events"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn cx(data: &Path, call_id: &str) -> tools::ToolCx {
    tools::ToolCx {
        session: runtime::SessionId::new(),
        call_id: call_id.to_owned(),
        quiet: false,
        resources: tools::SessionResources::durable_arc(data),
        cancel: tools::CancellationToken::new(),
        promote_after: None,
    }
}

fn tool_ran(ev: &CodeindexEvent) -> Option<(&str, &str, &codeindex::CodeToolSummary, u64)> {
    match ev {
        CodeindexEvent::CodeToolRan {
            tool,
            call_id,
            summary,
            seq,
            ..
        } => Some((tool, call_id, summary, *seq)),
        CodeindexEvent::CodeReconciled { .. } => None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn events_are_published_once_per_call() {
    let s = scratch().await;
    let data = s._data.0.clone();
    let out = CodeFindTool(s.ix.clone())
        .call(
            CodeFindArgs {
                name: "Registry".into(),
                kind: None,
                exact: Some(true),
                root: Some(s.root.clone()),
                budget: None,
            },
            cx(&data, "toolu_1"),
        )
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content);
    let err = CodeRefsTool(s.ix.clone())
        .call(
            CodeRefsArgs {
                symbol: "NoSuchThing".into(),
                direction: None,
                kind: None,
                root: Some(s.root.clone()),
                budget: None,
            },
            cx(&data, "toolu_2"),
        )
        .await
        .unwrap();
    assert!(err.is_error);
    wait_for(&s.seen, |v| v.len() >= 2).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let seen = s.seen.lock();
    let ran: Vec<_> = seen.iter().filter_map(|(_, e)| tool_ran(e)).collect();
    assert_eq!(ran.len(), 2, "{seen:?}");
    let (tool, call_id, sum, seq1) = ran[0];
    assert_eq!((tool, call_id), ("code_find", "toolu_1"));
    assert_eq!(sum.rows, 1);
    assert!(!sum.is_error && !sum.stale);
    let (tool, call_id, sum, seq2) = ran[1];
    assert_eq!((tool, call_id), ("code_refs", "toolu_2"));
    assert!(sum.is_error);
    assert_eq!(sum.rows, 0);
    assert!(seq2 > seq1, "seq is monotonic per root");
    // No write happened: the first index's `CodeReconciled` was consumed by `scratch`.
    assert_eq!(seen.len(), 2, "{seen:?}");
    if let CodeindexEvent::CodeToolRan { args, root, .. } = &seen[0].1 {
        assert_eq!(root, &s.root);
        assert_eq!(args["name"], "Registry");
        assert!(
            args.get("root").is_none()
                && args.get("budget").is_none()
                && args.get("kind").is_none(),
            "{args}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reconciled_is_gated_250ms() {
    let s = scratch().await;
    let lib = "crates/a/src/lib.rs";
    let cli = "crates/b/src/cli.rs";
    // Past the first index's gate window: the first touch publishes at once.
    tokio::time::sleep(Duration::from_millis(300)).await;
    for (i, p) in [lib, cli].into_iter().enumerate() {
        let full = s.root.join(p);
        let mut src = std::fs::read_to_string(&full).unwrap();
        src.push_str(&format!("\n// touched {i}\n"));
        std::fs::write(&full, src).unwrap();
        let r = s.ix.touch(&s.ri, &[p.to_owned()]).await.unwrap();
        assert_eq!(r.reparsed, 1, "{r:?}");
    }
    // Both writes landed inside one gate window: exactly one event so far.
    tokio::time::sleep(Duration::from_millis(60)).await;
    let n = s.seen.lock().len();
    assert_eq!(n, 1, "second write must be deferred");
    wait_for(&s.seen, |v| v.len() == 2).await;
    let seen = s.seen.lock().clone();
    let gap = seen[1].0 - seen[0].0;
    assert!(gap >= Duration::from_millis(240), "gap {gap:?}");
    match (&seen[0].1, &seen[1].1) {
        (
            CodeindexEvent::CodeReconciled {
                reparsed: a,
                seq: s1,
                ..
            },
            CodeindexEvent::CodeReconciled {
                reparsed: b,
                seq: s2,
                ..
            },
        ) => {
            assert_eq!(a, &[lib.to_owned()]);
            assert_eq!(b, &[cli.to_owned()]);
            assert!(s2 > s1);
        }
        other => panic!("{other:?}"),
    }

    // Unchanged file: no write, no event.
    let r = s.ix.touch(&s.ri, &[lib.to_owned()]).await.unwrap();
    assert_eq!(r.reparsed, 0);
    // Escaping the root fails the whole call.
    assert!(
        s.ix.touch(&s.ri, &["../outside.rs".to_owned()])
            .await
            .is_err()
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(s.seen.lock().len(), 2);
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

fn append(root: &Path, rel: &str, text: &str) {
    let full = root.join(rel);
    let mut src = std::fs::read_to_string(&full).unwrap();
    src.push_str(text);
    std::fs::write(&full, src).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn here_is_a_cached_store_lookup_with_badges() {
    let s = scratch().await;
    let here = |limit: Option<u32>, focus: Option<Vec<String>>| {
        lens::here(
            &s.ix,
            Here {
                root: s.root.clone(),
                focus,
                edited: None,
                limit,
            },
        )
    };
    let all = here(None, None).await.unwrap();
    assert!(!all.cache_hit && !all.personalised);
    assert!(all.rows.len() > 3, "{}", all.rows.len());
    assert_eq!(all.dropped, 0);
    assert_eq!(all.meta.root, s.root);
    assert!(all.meta.indexed_files > 0);
    // Files by best rank, symbols by line: within a file lines ascend.
    for w in all.rows.windows(2) {
        if w[0].path == w[1].path {
            assert!(w[0].line <= w[1].line, "{:?}", w);
        }
    }
    let check = all
        .rows
        .iter()
        .find(|r| r.name == "check" && r.kind == "function")
        .expect("sub::check ranked");
    assert!(check.importers > 0 && check.callers > 0, "{check:?}");
    assert!(check.moniker.starts_with("rust:crates/a/src/store/sub.rs#"));
    assert!(check.line_end > check.line);
    // The same query is a cache hit; a cut reports what it dropped.
    let again = here(None, None).await.unwrap();
    assert!(again.cache_hit);
    assert_eq!(again.rows.len(), all.rows.len());
    let cut = here(Some(2), None).await.unwrap();
    assert_eq!(cut.rows.len(), 2);
    assert_eq!(cut.dropped as usize, all.rows.len() - 2);
    assert!(cut.dirs.iter().any(|d| d.ranked_rows > 0), "{:?}", cut.dirs);
    // Personalised (RANK's `focus`, tested in `rank.rs`): its own cache key.
    let p = here(None, Some(vec!["crates/b/src/cli.rs".into()]))
        .await
        .unwrap();
    assert!(p.personalised && !p.cache_hit && !p.rows.is_empty());
    assert!(
        here(None, Some(vec!["crates/b/src/cli.rs".into()]))
            .await
            .unwrap()
            .cache_hit
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fit_caps_at_limit_and_counts_more() {
    let s = scratch().await;
    let fit = |symbol: &str, limit: Option<u32>| {
        lens::fit(
            &s.ix,
            Fit {
                root: s.root.clone(),
                symbol: symbol.into(),
                limit,
            },
        )
    };
    // `Registry::new` by `path:line`: called from `run` and `open_db`.
    let full = fit("crates/a/src/lib.rs:12", None).await.unwrap();
    assert_eq!(
        (full.focus.name.as_str(), full.focus.kind.as_str()),
        ("new", "method")
    );
    assert!(full.candidates.is_empty());
    assert!(full.callers.len() >= 2, "{:?}", full.callers);
    assert_eq!(full.more_callers, 0);
    assert_eq!(full.edges.len(), full.callers.len() + full.callees.len());
    assert!(
        full.edges
            .iter()
            .all(|e| e.to == full.focus.moniker || e.from == full.focus.moniker)
    );
    let cut = fit("crates/a/src/lib.rs:12", Some(2)).await.unwrap();
    assert_eq!(cut.callers.len() + cut.callees.len(), 1, "{cut:?}");
    assert_eq!(
        cut.more_callers as usize + cut.callers.len(),
        full.callers.len()
    );
    assert_eq!(
        cut.more_callees as usize + cut.callees.len(),
        full.callees.len()
    );
    // Callers in the best-first order survive the cut.
    assert_eq!(cut.callers, full.callers[..cut.callers.len()]);
    // The moniker names the same definition.
    let by_moniker = fit(&full.focus.moniker, None).await.unwrap();
    assert_eq!(by_moniker.focus, full.focus);
    assert_eq!(by_moniker.callers, full.callers);
    // Ambiguous name: candidates, focus = the best ranked of them.
    let several = fit("new", None).await.unwrap();
    assert!(several.candidates.len() >= 2, "{:?}", several.candidates);
    assert!(several.candidates.contains(&several.focus));
    assert!(fit("NoSuchSymbol", None).await.is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn touch_reparses_only_named_paths() {
    let s = scratch().await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let (lib, store_rs) = ("crates/a/src/lib.rs", "crates/a/src/store.rs");
    append(&s.root, lib, "\npub fn touched_a() {}\n");
    append(&s.root, store_rs, "\npub fn touched_b() {}\n");
    let before = store::known_files(&s.ri.pool).await.unwrap();
    let m = lens::touch(
        &s.ix,
        Touch {
            root: s.root.clone(),
            paths: Some(vec![lib.into()]),
        },
    )
    .await
    .unwrap();
    assert!(!m.stale && m.ms < 5_000);
    let after = store::known_files(&s.ri.pool).await.unwrap();
    assert_ne!(before[lib].1, after[lib].1, "lib.rs re-stat'd");
    assert_eq!(before[store_rs], after[store_rs], "store.rs untouched");
    assert!(
        store::definitions_named(&s.ri.pool, "touched_a")
            .await
            .unwrap()
            .len()
            == 1
    );
    assert!(
        store::definitions_named(&s.ri.pool, "touched_b")
            .await
            .unwrap()
            .is_empty()
    );
    wait_for(&s.seen, |v| v.len() == 1).await;
    match &s.seen.lock()[0].1 {
        CodeindexEvent::CodeReconciled { reparsed, .. } => assert_eq!(reparsed, &[lib.to_owned()]),
        other => panic!("{other:?}"),
    }
    // `paths: None` is the forced reconcile: it finds store.rs.
    let m = lens::touch(
        &s.ix,
        Touch {
            root: s.root.clone(),
            paths: None,
        },
    )
    .await
    .unwrap();
    assert!(!m.stale);
    assert!(
        store::definitions_named(&s.ri.pool, "touched_b")
            .await
            .unwrap()
            .len()
            == 1
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lens_rpcs_never_walk() {
    let s = scratch().await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    // A new file on disk that only a walk would find.
    std::fs::write(
        s.root.join("crates/a/src/fresh.rs"),
        "pub fn fresh() -> u32 { 1 }\n",
    )
    .unwrap();
    append(&s.root, "crates/a/src/lib.rs", "\npub mod fresh;\n");
    let before = store::known_files(&s.ri.pool).await.unwrap();
    lens::here(
        &s.ix,
        Here {
            root: s.root.clone(),
            focus: None,
            edited: None,
            limit: None,
        },
    )
    .await
    .unwrap();
    lens::fit(
        &s.ix,
        Fit {
            root: s.root.clone(),
            symbol: "Registry".into(),
            limit: None,
        },
    )
    .await
    .unwrap();
    let change = lens::change(
        &s.ix,
        Change {
            root: s.root.clone(),
            git_range: None,
            files: Some(vec!["crates/a/src/lib.rs".into()]),
            include_tests: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(change.mode, "files");
    assert_eq!(change.changed[0].path, "crates/a/src/lib.rs");
    assert!(
        change.changed[0]
            .symbols
            .iter()
            .any(|x| x.name == "Registry"),
        "{:?}",
        change.changed[0]
    );
    assert!(
        change
            .direct
            .iter()
            .any(|f| f.path == "crates/a/src/store.rs"),
        "{:?}",
        change.direct
    );
    assert!(
        change
            .direct
            .iter()
            .all(|f| f.depth == 1 && !f.via.is_empty())
    );
    // Nothing indexed the new file or the edit; no event fired.
    let after = store::known_files(&s.ri.pool).await.unwrap();
    assert_eq!(before, after);
    assert!(!after.contains_key("crates/a/src/fresh.rs"));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(s.seen.lock().is_empty(), "{:?}", s.seen.lock());
    // An unindexed root is refused, not indexed.
    let other = tempdir();
    let err = lens::here(
        &s.ix,
        Here {
            root: other.0.clone(),
            focus: None,
            edited: None,
            limit: None,
        },
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("not indexed"), "{err}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn change_worktree_is_the_default_and_cached() {
    let s = scratch().await;
    git(&s.root, &["init", "-q"]);
    git(&s.root, &["add", "."]);
    git(&s.root, &["commit", "-q", "-m", "base"]);
    append(
        &s.root,
        "crates/a/src/store.rs",
        "\npub fn touched_c() {}\n",
    );
    s.ix.touch(&s.ri, &["crates/a/src/store.rs".to_owned()])
        .await
        .unwrap();
    let change = |include_tests: Option<bool>| {
        lens::change(
            &s.ix,
            Change {
                root: s.root.clone(),
                git_range: None,
                files: None,
                include_tests,
            },
        )
    };
    let c = change(None).await.unwrap();
    assert_eq!(c.mode, "worktree");
    let (old, new) = c.range.clone().unwrap();
    assert_eq!(new, "WORKTREE");
    assert_eq!(old.len(), 8, "{old}");
    assert!(!c.cache_hit);
    assert_eq!(c.changed.len(), 1, "{:?}", c.changed);
    assert_eq!(c.changed[0].status.as_deref(), Some("modified"));
    assert_eq!(
        c.changed[0]
            .symbols
            .iter()
            .map(|x| x.name.as_str())
            .collect::<Vec<_>>(),
        ["touched_c"]
    );
    assert_eq!(c.more, [0, 0, 0, 0]);
    assert!(change(None).await.unwrap().cache_hit);
    assert!(
        !change(Some(true)).await.unwrap().cache_hit,
        "a different key"
    );
}