//! The generator's proof (147 §15 W2) over a fake side caller: validation
//! and the one repair round, perspective selection without a ceiling, edge
//! evidence rules, attachment to a running job, cancellation that keeps what
//! was written. The pipeline tests index the rust fixture into a scratch
//! data dir; the pure tests use `generator_facts::tests::synthetic`.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use runtime::{Runtime, SessionId, SideCall, SideReply};
use serde_json::{Value, json};

use super::*;
use crate::map::generator_facts::tests::synthetic;
use crate::map::validate::COUNTS_FROM_EVIDENCE;

// --- the fake side caller -----------------------------------------------------

type Handler = Box<dyn Fn(&Value) -> Result<String, String> + Send>;

#[derive(Default)]
struct Script {
    /// Per stage, replies consumed in order; empty → the default reply.
    handlers: HashMap<&'static str, VecDeque<Handler>>,
    /// Every call seen, in order.
    calls: Vec<SideCall>,
    /// Hold every call this long (attachment test).
    delay: Option<Duration>,
    /// Hold calls of this stage this long (cancel test).
    stall: Option<(&'static str, Duration)>,
}

static SCRIPT: LazyLock<Mutex<Script>> = LazyLock::new(Mutex::default);
/// Pipeline tests share the process-wide caller and script: one at a time.
static SERIAL: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

fn script() -> std::sync::MutexGuard<'static, Script> {
    SCRIPT.lock().unwrap_or_else(|e| e.into_inner())
}

fn install() {
    install_side_caller(Arc::new(|call: SideCall| {
        Box::pin(async move {
            let mut user: Value = serde_json::from_str(&call.user).unwrap_or(Value::Null);
            // A repair message is prose around the previous reply: it belongs to the
            // last stage and answers the same facts.
            if user["stage"].as_str().is_none()
                && let Some(prev) = script().calls.last()
            {
                user = serde_json::from_str(&prev.user).unwrap_or(Value::Null);
            }
            let stage = user["stage"].as_str().unwrap_or_default().to_owned();
            let (handler, delay) = {
                let mut s = script();
                s.calls.push(call.clone());
                let h = s
                    .handlers
                    .get_mut(stage.as_str())
                    .and_then(VecDeque::pop_front);
                let stall = s.stall.filter(|(st, _)| *st == stage).map(|(_, d)| d);
                (h, s.delay.or(stall))
            };
            if let Some(d) = delay {
                tokio::time::sleep(d).await;
            }
            let text = match handler {
                Some(h) => h(&user)?,
                None => default_reply(&stage, &user)?,
            };
            Ok(SideReply {
                text,
                model: "fake/model".to_owned(),
                input_tokens: Some(1000),
                output_tokens: Some(100),
                latency: Duration::from_millis(1),
            })
        })
    }));
}

/// A valid reply for any stage, read off the facts in the user message.
fn default_reply(stage: &str, user: &Value) -> Result<String, String> {
    Ok(match stage {
        "select" => json!({ "perspectives": [{ "kind": "overall-architecture", "title": "Overall architecture", "why": "always" }] }).to_string(),
        "top" => top_reply(user, "uses it", None).to_string(),
        "flow" => {
            let handles: Vec<&str> = user["handles"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
            let mut mermaid = String::from("flowchart LR\n");
            for c in user["flow"]["calls"].as_array().into_iter().flatten() {
                mermaid.push_str(&format!("  {} -->|\"calls it\"| {}\n", c["from"].as_str().unwrap_or(""), c["to"].as_str().unwrap_or("")));
            }
            json!({
                "title": "Entry flow", "fields": { "description": "The entry point and what it calls, in order." },
                "steps": handles.iter().map(|h| json!({ "id": h, "name": format!("step {h}") })).collect::<Vec<_>>(),
                "mermaid": mermaid, "edges": [],
            })
            .to_string()
        }
        "describe" => {
            let elements: Vec<Value> = user["elements"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|e| {
                    let labels: serde_json::Map<String, Value> = e["edges"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|x| (x["handle"].as_str().unwrap_or("").to_owned(), json!("feeds the store")))
                        .collect();
                    json!({ "id": e["id"], "name": e["current_name"], "fields": { "description": "Described by the fake caller in two sentences. It covers the listed paths." }, "edge_labels": labels })
                })
                .collect();
            json!({ "elements": elements }).to_string()
        }
        other => return Err(format!("fake: unexpected stage {other}")),
    })
}

/// A stage-B reply: one group per unit, every fact edge drawn with `label`.
fn top_reply(user: &Value, label: &str, extra_edge: Option<(&str, &str, Option<&str>)>) -> Value {
    let units: Vec<&str> = user["units"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|u| u["handle"].as_str())
        .collect();
    let group = |n: &str| format!("g{}", &n[1..]);
    let nodes: Vec<Value> = units
        .iter()
        .map(|n| json!({ "id": group(n), "name": format!("Group {n}"), "members": [n] }))
        .collect();
    let mut mermaid = String::from("flowchart LR\n");
    for n in &units {
        mermaid.push_str(&format!("  {}\n", group(n)));
    }
    for e in user["edges"].as_array().into_iter().flatten() {
        mermaid.push_str(&format!(
            "  {} -->|\"{label}\"| {}\n",
            group(e["from"].as_str().unwrap_or("n1")),
            group(e["to"].as_str().unwrap_or("n1"))
        ));
    }
    let mut edges = Vec::new();
    if let Some((from, to, reason)) = extra_edge {
        mermaid.push_str(&format!("  {from} -.->|\"guessed link\"| {to}\n"));
        if let Some(r) = reason {
            edges.push(json!({ "from": from, "to": to, "reason": r }));
        }
    }
    json!({
        "fields": { "description": "The whole workspace as the index measured it. Two crates and a stray one." },
        "nodes": nodes, "mermaid": mermaid, "edges": edges,
    })
}

// --- scratch index ----------------------------------------------------------------

struct TempDir(PathBuf);
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
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

struct Scratch {
    _data: TempDir,
    _repo: TempDir,
    root: PathBuf,
    ix: Arc<Codeindex>,
    pool: SqlitePool,
}

async fn scratch() -> Scratch {
    install();
    let data = TempDir(std::env::temp_dir().join(format!("codemap-gen-{}", new_id())));
    let repo = TempDir(std::env::temp_dir().join(format!("codemap-gen-repo-{}", new_id())));
    std::fs::create_dir_all(&data.0).unwrap();
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rust"),
        &repo.0,
    );
    let rt = Runtime::new(figment::Figment::new(), data.0.clone());
    let ix = Arc::new(
        Codeindex::new(crate::CodeindexConfig::default(), &data.0)
            .unwrap()
            .with_runtime(rt),
    );
    let root = repo.0.canonicalize().unwrap();
    let ri = ix.open_root(&root).await.unwrap();
    let r = ix.reconcile(&ri, false).await.unwrap();
    assert!(r.reparsed > 0, "{r:?}");
    let pool = store::open(&store::path(ix.dir(), &root), &root)
        .await
        .unwrap();
    Scratch {
        _data: data,
        _repo: repo,
        root,
        ix,
        pool,
    }
}

async fn wait_idle(root: &Path) {
    let t = Instant::now();
    while jobs().contains_key(root) {
        assert!(t.elapsed() < Duration::from_secs(30), "job did not finish");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// `(id, depth, kind, description, error)` for failure messages.
type ElementDump = (String, i64, String, Option<String>, Option<String>);

async fn job_row(pool: &SqlitePool, id: &str) -> (String, i64, i64, Option<String>) {
    sqlx::query_as("SELECT stage, done, calls, coalesce(error, '') || ' | ' || coalesce(error_log, '') FROM jobs WHERE id = ?")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

// --- pipeline tests -------------------------------------------------------------------

#[tokio::test]
async fn generate_with_fake_side_caller_validates_and_repairs() {
    let _serial = SERIAL.lock().await;
    let s = scratch().await;
    {
        let mut sc = script();
        *sc = Script::default();
        // First stage-B reply carries a count in a label (rule 7) → one repair → valid.
        sc.handlers
            .entry("top")
            .or_default()
            .push_back(Box::new(|u| Ok(top_reply(u, "4 deps", None).to_string())));
    }
    let job = generate(
        &s.ix,
        Generate {
            root: s.root.clone(),
            session: SessionId::new(),
            element: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(job.stage, "select");
    wait_idle(&s.root).await;

    let (stage, done, calls, log) = job_row(&s.pool, &job.id).await;
    assert_eq!(stage, "done", "{log:?}");
    assert!(done >= 2 && calls >= 3, "done {done} calls {calls} {log:?}");
    {
        let seen = script();
        let repair = seen
            .calls
            .iter()
            .find(|c| c.user.contains("<errors>"))
            .expect("a repair message was sent");
        assert!(
            repair.user.contains(COUNTS_FROM_EVIDENCE),
            "{}",
            repair.user
        );
        assert!(repair.user.contains("<previous>"));
        assert!(
            seen.calls
                .iter()
                .all(|c| c.kind == CallKind::CodeMap && c.role == ModelRole::Session)
        );
        assert!(seen.calls.iter().all(|c| c.system.contains("Catalogue")));
    }

    let (origin, repaired, model, desc, diagram): (String, i64, String, String, String) =
        sqlx::query_as("SELECT origin, repaired, model, description, diagram FROM elements WHERE id = 'overall-architecture'")
            .fetch_one(&s.pool)
            .await
            .unwrap();
    assert_eq!(
        (origin.as_str(), repaired, model.as_str()),
        ("generated", 1, "fake/model")
    );
    assert!(desc.starts_with("The whole workspace"));
    let g: GraphProps = serde_json::from_str(&diagram).unwrap();
    assert!(
        g.nodes.iter().any(|n| n.id == "unit:crates/a"),
        "{:?}",
        g.nodes
    );
    assert!(
        g.edges
            .iter()
            .all(|e| e.label.as_deref() == Some("uses it")),
        "{:?}",
        g.edges
    );
    let backed: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT from_node, to_node, backed FROM edges WHERE element = 'overall-architecture'",
    )
    .fetch_all(&s.pool)
    .await
    .unwrap();
    assert!(
        !backed.is_empty() && backed.iter().all(|e| e.2 == 1),
        "{backed:?}"
    );
    // Stage C named and described the units.
    let (n,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM elements WHERE perspective = 'overall-architecture' AND depth = 1 AND description LIKE 'Described by%'",
    )
    .fetch_one(&s.pool)
    .await
    .unwrap();
    let rows: Vec<ElementDump> = sqlx::query_as(
        "SELECT id, depth, kind, description, error FROM elements ORDER BY depth, order_no",
    )
    .fetch_all(&s.pool)
    .await
    .unwrap();
    assert!(n >= 2, "{n} described; {log:?}\n{rows:#?}");
    let (labelled,): (i64,) =
        sqlx::query_as("SELECT count(*) FROM edges WHERE label = 'feeds the store'")
            .fetch_one(&s.pool)
            .await
            .unwrap();
    assert!(labelled >= 1, "module edges labelled by stage C");
    let (persp,): (String,) =
        sqlx::query_as("SELECT origin FROM perspectives WHERE id = 'overall-architecture'")
            .fetch_one(&s.pool)
            .await
            .unwrap();
    assert_eq!(persp, "generated");
    // A second open with nothing stale is a no-op job.
    let again = generate(
        &s.ix,
        Generate {
            root: s.root.clone(),
            session: SessionId::new(),
            element: None,
        },
    )
    .await
    .unwrap();
    assert_ne!(again.id, job.id);
    wait_idle(&s.root).await;
    let (stage, _, calls, _) = job_row(&s.pool, &again.id).await;
    assert_eq!((stage.as_str(), calls), ("done", 0));
}

#[tokio::test]
async fn second_open_attaches_to_running_job() {
    let _serial = SERIAL.lock().await;
    let s = scratch().await;
    {
        let mut sc = script();
        *sc = Script::default();
        sc.delay = Some(Duration::from_millis(400));
    }
    let a = generate(
        &s.ix,
        Generate {
            root: s.root.clone(),
            session: SessionId::new(),
            element: None,
        },
    )
    .await
    .unwrap();
    let b = generate(
        &s.ix,
        Generate {
            root: s.root.clone(),
            session: SessionId::new(),
            element: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(a.id, b.id);
    assert_eq!(a.session, b.session, "the first opener pays");
    let c = cancel(
        &s.ix,
        Cancel {
            root: s.root.clone(),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        (c.id.as_str(), c.stage.as_str()),
        (a.id.as_str(), "cancelled")
    );
    assert!(
        cancel(
            &s.ix,
            Cancel {
                root: s.root.clone()
            }
        )
        .await
        .is_err()
    );
    script().delay = None;
}

#[tokio::test]
async fn cancel_stops_and_keeps_written_elements() {
    let _serial = SERIAL.lock().await;
    let s = scratch().await;
    {
        let mut sc = script();
        *sc = Script::default();
        // Stage C stalls; stage B has already written the tree by then.
        sc.stall = Some(("describe", Duration::from_secs(5)));
    }
    let job = generate(
        &s.ix,
        Generate {
            root: s.root.clone(),
            session: SessionId::new(),
            element: None,
        },
    )
    .await
    .unwrap();
    let t = Instant::now();
    loop {
        let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM elements")
            .fetch_one(&s.pool)
            .await
            .unwrap();
        if n > 0 {
            break;
        }
        assert!(t.elapsed() < Duration::from_secs(20), "stage B never wrote");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let c = cancel(
        &s.ix,
        Cancel {
            root: s.root.clone(),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        (c.id.as_str(), c.stage.as_str()),
        (job.id.as_str(), "cancelled")
    );
    assert!(!jobs().contains_key(&s.root));
    let (stage, _, _, _) = job_row(&s.pool, &job.id).await;
    assert_eq!(stage, "cancelled");
    let (n,): (i64,) =
        sqlx::query_as("SELECT count(*) FROM elements WHERE origin IN ('generated', 'structure')")
            .fetch_one(&s.pool)
            .await
            .unwrap();
    assert!(n >= 3, "{n} elements kept");
    // The next open resumes with the elements that still lack prose.
    script().stall = None;
    let again = generate(
        &s.ix,
        Generate {
            root: s.root.clone(),
            session: SessionId::new(),
            element: None,
        },
    )
    .await
    .unwrap();
    wait_idle(&s.root).await;
    let (stage, done, calls, log) = job_row(&s.pool, &again.id).await;
    assert_eq!(stage, "done", "{log:?}");
    assert!(
        done >= 1 && calls >= 1,
        "resumed with describe only: done {done} calls {calls} {log:?}"
    );
    let seen = script();
    let resumed: Vec<&SideCall> = seen
        .calls
        .iter()
        .rev()
        .take(usize::try_from(calls).unwrap())
        .collect();
    assert!(
        resumed
            .iter()
            .all(|c| !c.user.contains("\"stage\": \"top\"")
                && !c.user.contains("\"stage\": \"select\"")),
        "resume must not redo stages A/B"
    );
    assert!(
        resumed
            .iter()
            .any(|c| c.user.contains("\"stage\": \"describe\""))
    );
}

// --- pure tests ---------------------------------------------------------------------

fn handles_for(facts: &Facts) -> Handles {
    Handles::new(&facts.top_units(), &facts.edges)
}

/// A stage-B reply over one group per unit; `mermaid` is the edge lines only,
/// the group declarations are prepended.
fn reply(facts: &Facts, h: &Handles, mermaid: &str, edges: Vec<Value>) -> TopReply {
    let nodes: Vec<Value> = h
        .units
        .iter()
        .map(|(handle, node)| json!({ "id": format!("g{}", &handle[1..]), "name": facts.unit(node).unwrap().name, "members": [handle] }))
        .collect();
    let decls: String = h
        .units
        .iter()
        .map(|(handle, _)| format!("  g{}\n", &handle[1..]))
        .collect();
    let mermaid = format!("{decls}{}", mermaid.trim_start_matches("flowchart LR\n"));
    serde_json::from_value(json!({
        "fields": { "description": "A synthetic workspace with an app, a core and a util crate plus docs." },
        "nodes": nodes, "mermaid": format!("flowchart LR\n{mermaid}"), "edges": edges,
    }))
    .unwrap()
}

#[test]
fn edges_without_evidence_need_a_reason() {
    let facts = synthetic();
    let h = handles_for(&facts);
    let g = |path: &str| format!("g{}", &h.unit_handle(&format!("unit:{path}")).unwrap()[1..]);
    let (app, core, util) = (g("crates/app"), g("crates/core"), g("crates/util"));
    // util → app: nothing in the index joins them.
    let src = format!(
        "flowchart LR\n  {app} -->|\"drives the core\"| {core}\n  {util} -->|\"helps the app\"| {app}\n"
    );
    let err = validate_top(
        &reply(&facts, &h, &src, vec![]),
        &facts,
        &h,
        "overall-architecture",
    )
    .unwrap_err();
    assert_eq!(err.len(), 1, "{err:#?}");
    assert!(
        err[0].what.contains("has no evidence in the facts"),
        "{}",
        err[0]
    );
    assert!(err[0].expected.contains("reason"));

    let ok = validate_top(
        &reply(
            &facts,
            &h,
            &src,
            vec![json!({ "from": util, "to": app, "reason": "the app re-exports util's prelude" })],
        ),
        &facts,
        &h,
        "overall-architecture",
    )
    .unwrap();
    let inferred = ok
        .edges
        .iter()
        .find(|e| e.from == "unit:crates/util")
        .unwrap();
    assert!(!inferred.backed);
    assert_eq!(
        inferred.evidence,
        vec![MapEvidence::Inferred {
            reason: "the app re-exports util's prelude".into()
        }]
    );
    let backed = ok
        .edges
        .iter()
        .find(|e| e.from == "unit:crates/app")
        .unwrap();
    assert!(
        backed.backed
            && backed
                .evidence
                .iter()
                .any(|e| matches!(e, MapEvidence::Manifest { .. }))
    );
    assert!(ok.uncovered.is_empty());

    // A cited evidence handle must join the two groups.
    let cited = validate_top(
        &reply(
            &facts,
            &h,
            &src,
            vec![json!({ "from": util, "to": app, "evidence": ["e1"], "reason": "x" })],
        ),
        &facts,
        &h,
        "overall-architecture",
    )
    .unwrap_err();
    assert!(
        cited.iter().any(|e| e.what.contains("does not join")),
        "{cited:#?}"
    );
}

#[test]
fn labels_with_counts_are_rejected() {
    let facts = synthetic();
    let h = handles_for(&facts);
    let g = |path: &str| format!("g{}", &h.unit_handle(&format!("unit:{path}")).unwrap()[1..]);
    let src = format!(
        "flowchart LR\n  {} -->|\"mesh · 4 deps\"| {}\n",
        g("crates/app"),
        g("crates/core")
    );
    let err = validate_top(
        &reply(&facts, &h, &src, vec![]),
        &facts,
        &h,
        "overall-architecture",
    )
    .unwrap_err();
    assert_eq!(err.len(), 1, "{err:#?}");
    assert_eq!(err[0].expected, COUNTS_FROM_EVIDENCE);
    assert_eq!(err[0].path, "/mermaid/edges/0");
    let msg = validate::repair_message("{…}", &err).unwrap();
    assert!(msg.contains("/mermaid/edges/0 · label \"mesh · 4 deps\" carries a number · counts are rendered from evidence"));

    // A label override in `edges` is checked the same way; a clean one passes.
    let src = format!(
        "flowchart LR\n  {} --> {}\n",
        g("crates/app"),
        g("crates/core")
    );
    let ok = validate_top(
        &reply(&facts, &h, &src, vec![json!({ "from": g("crates/app"), "to": g("crates/core"), "label": "drives the engine" })]),
        &facts,
        &h,
        "overall-architecture",
    )
    .unwrap();
    assert_eq!(ok.edges[0].label, "drives the engine");
    let err = validate_top(
        &reply(&facts, &h, &src, vec![]),
        &facts,
        &h,
        "overall-architecture",
    )
    .unwrap_err();
    assert_eq!(err[0].what, "edge has no label");
}

#[test]
fn ineligible_perspective_is_dropped() {
    let facts = synthetic(); // only overall-architecture and dependency-map are eligible
    let row = |k: &str| SelectRow {
        kind: k.to_owned(),
        title: title_of(k),
        why: "because".to_owned(),
    };
    let (kept, dropped) = choose(
        vec![
            row("storage"),
            row("dependency-map"),
            row("request-lifecycle"),
        ],
        &facts.detections,
    );
    assert_eq!(
        kept.iter().map(|p| p.kind.as_str()).collect::<Vec<_>>(),
        ["overall-architecture", "dependency-map"]
    );
    assert_eq!(dropped, ["storage", "request-lifecycle"]);
    assert_eq!(kept[0].title, "Overall architecture");
}

#[test]
fn no_perspective_ceiling() {
    let detections: Vec<Detection> = PERSPECTIVES
        .iter()
        .map(|k| Detection {
            kind: (*k).to_owned(),
            eligible: true,
            hits: 9,
            samples: vec![],
            confidence: 0.9,
        })
        .collect();
    let all: Vec<SelectRow> = PERSPECTIVES
        .iter()
        .rev()
        .map(|k| SelectRow {
            kind: (*k).to_owned(),
            title: String::new(),
            why: String::new(),
        })
        .collect();
    let (kept, dropped) = choose(all, &detections);
    assert_eq!(kept.len(), 12);
    assert!(dropped.is_empty());
    assert_eq!(
        kept.iter().map(|p| p.kind.as_str()).collect::<Vec<_>>(),
        PERSPECTIVES
    );
}

#[test]
fn structure_fallback_and_tree_shape() {
    let facts = synthetic();
    let plan = structure_plan(&facts, "overall-architecture", MapFields::default());
    assert_eq!(plan.groups.len(), 4);
    assert!(plan.edges.iter().all(|e| e.label.is_empty() && e.backed));
    let p = Planned {
        id: "overall-architecture".into(),
        kind: "overall-architecture".into(),
        title: "Overall architecture".into(),
        why: String::new(),
        order_no: 0,
        top: true,
    };
    let prov = MapProvenance {
        model: "m".into(),
        git_head: None,
        index_state: "1:1".into(),
        generated_ms: 0,
        call_ids: vec![],
        cost_nanos: 0,
        currency: "USD".into(),
        input_tokens: 0,
        output_tokens: 0,
        repaired: false,
        facts_hash: "h".into(),
    };
    let rows = build_tree(&facts, &p, &plan, prov, None, MAX_DEPTH);
    let root = &rows[0];
    assert_eq!(
        (root.depth, root.kind.as_str(), root.origin),
        (0, "perspective", MapOrigin::Generated)
    );
    let core = rows
        .iter()
        .find(|r| r.id == "overall-architecture/unit:crates/core")
        .unwrap();
    assert_eq!((core.depth, core.leaf), (1, false));
    let g = core.diagram.as_ref().unwrap();
    assert!(g.nodes.iter().any(|n| n.id == "unit:crates/core/src/gen"));
    assert!(g.edges.iter().all(|e| {
        e.label
            .as_deref()
            .is_some_and(|l| l.contains("import") || l.contains("ref"))
    }));
    let modules: Vec<&ElementRow> = rows
        .iter()
        .filter(|r| r.parent.as_deref() == Some("overall-architecture/unit:crates/core"))
        .collect();
    assert_eq!(modules.len(), 4);
    assert!(modules.iter().all(|m| m.depth == 2));
    let lib = modules
        .iter()
        .find(|m| m.node == "unit:crates/core/src/lib.rs")
        .unwrap();
    assert_eq!((lib.kind.as_str(), lib.leaf), ("file", true));
    let docs = rows
        .iter()
        .find(|r| r.id == "overall-architecture/unit:docs")
        .unwrap();
    assert!(
        docs.leaf && docs.diagram.is_none(),
        "a ≤ 2-file unit is a leaf"
    );
    assert!(rows.iter().all(|r| r.depth <= MAX_DEPTH));
    assert_eq!(
        sub_line(&["a/b".into(), "c/d".into(), "e".into(), "f".into()]).as_deref(),
        Some("a/b · c/d +2")
    );
}
