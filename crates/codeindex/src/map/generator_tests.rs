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
    /// The session the liveness probe refuses (R2-5).
    dead_session: Option<SessionId>,
}

static SCRIPT: LazyLock<Mutex<Script>> = LazyLock::new(Mutex::default);
/// Pipeline tests share the process-wide caller and script: one at a time.
static SERIAL: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));
/// USD nanos every fake reply reports as its cost.
const FAKE_CALL_COST: u64 = 2_500_000;

fn script() -> std::sync::MutexGuard<'static, Script> {
    SCRIPT.lock().unwrap_or_else(|e| e.into_inner())
}

fn install() {
    install_side_probe(Arc::new(|session: SessionId| {
        let dead = script().dead_session == Some(session);
        Box::pin(async move {
            if dead {
                Err(format!("no live session {session}"))
            } else {
                Ok(())
            }
        })
    }));
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
                cost_nanos: Some(FAKE_CALL_COST),
                request_id: Some(format!("req-{}", script().calls.len())),
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

/// Every `CodeindexEvent` the bus delivered.
type Seen = Arc<Mutex<Vec<CodeindexEvent>>>;

#[derive(kameo::Actor)]
struct Sink(Seen);

impl kameo::message::Message<CodeindexEvent> for Sink {
    type Reply = ();
    async fn handle(&mut self, ev: CodeindexEvent, _: &mut kameo::message::Context<Self, ()>) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).push(ev);
    }
}

struct Scratch {
    data: TempDir,
    _repo: TempDir,
    root: PathBuf,
    ix: Arc<Codeindex>,
    ri: Arc<RootIndex>,
    pool: SqlitePool,
    seen: Seen,
    _sink: kameo::actor::ActorRef<Sink>,
}

impl Scratch {
    /// The `CodeMapProgress` events so far as `(stage, calls, cost_nanos, error)`.
    fn progress(&self) -> Vec<(String, u32, u64, Option<String>)> {
        self.seen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter_map(|e| match e {
                CodeindexEvent::CodeMapProgress {
                    stage,
                    calls,
                    cost_nanos,
                    error,
                    ..
                } => Some((stage.clone(), *calls, *cost_nanos, error.clone())),
                _ => None,
            })
            .collect()
    }
}

/// The rust fixture, indexed into a scratch data dir.
async fn scratch() -> Scratch {
    scratch_with(|repo| {
        copy_dir(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rust"),
            repo,
        );
    })
    .await
}

/// A repo `fill` writes, indexed into a scratch data dir, with the bus events
/// captured.
async fn scratch_with(fill: impl FnOnce(&Path)) -> Scratch {
    use kameo::actor::Spawn;
    install();
    let data = TempDir(std::env::temp_dir().join(format!("codemap-gen-{}", new_id())));
    let repo = TempDir(std::env::temp_dir().join(format!("codemap-gen-repo-{}", new_id())));
    std::fs::create_dir_all(&data.0).unwrap();
    fill(&repo.0);
    let rt = Runtime::new(figment::Figment::new(), data.0.clone());
    let seen: Seen = Arc::default();
    let sink = Sink::spawn(Sink(seen.clone()));
    rt.subscribe::<Sink, CodeindexEvent>(&sink).await;
    let ix = Arc::new(
        Codeindex::new(crate::CodeindexConfig::default(), &data.0)
            .unwrap()
            .with_runtime(rt),
    );
    let root = repo.0.canonicalize().unwrap();
    let ri = ix.open_root(&root).await.unwrap();
    let r = ix.reconcile(&ri, false).await.unwrap();
    assert!(r.reparsed > 0, "{r:?}");
    let pool = ix.map_pool(&ri).await.unwrap();
    Scratch {
        data,
        _repo: repo,
        root,
        ix,
        ri,
        pool,
        seen,
        _sink: sink,
    }
}

/// An omm-shaped repo: a FILE unit at the top level beside `src/` (the live
/// `tsup.config.ts` crash) and a scripts folder.
fn omm_shaped(repo: &Path) {
    let write = |rel: &str, body: &str| {
        let p = repo.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    };
    write(
        "package.json",
        "{ \"name\": \"omm\", \"version\": \"1.0.0\" }\n",
    );
    write(
        "tsup.config.ts",
        "export const config = { entry: ['src/index.ts'] };\n",
    );
    write(
        "src/index.ts",
        "import { run } from './cli';\nimport { store } from './lib/store';\nexport function main() { run(store); }\n",
    );
    write(
        "src/cli.ts",
        "import { Store } from './lib/store';\nexport function run(s: Store) { return s.size(); }\n",
    );
    write(
        "src/lib/store.ts",
        "export class Store { size() { return 0; } }\nexport const store = new Store();\n",
    );
    write(
        "src/lib/meta.ts",
        "import { store } from './store';\nexport function meta() { return store.size(); }\n",
    );
    write(
        "scripts/sync-version.js",
        "const fs = require('fs');\nmodule.exports = function sync() { return fs.existsSync('package.json'); };\n",
    );
    write(
        "scripts/check.js",
        "const sync = require('./sync-version');\nmodule.exports = () => sync();\n",
    );
    write(
        "scripts/release.js",
        "const check = require('./check');\nmodule.exports = () => check();\n",
    );
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
    // F1: the job sums every reply's priced cost; provenance carries the
    // agent's request ids and the element's own share.
    let (job_cost, job_calls): (i64, i64) =
        sqlx::query_as("SELECT cost_nanos, calls FROM jobs WHERE id = ?")
            .bind(&job.id)
            .fetch_one(&s.pool)
            .await
            .unwrap();
    assert_eq!(job_cost, job_calls * i64::try_from(FAKE_CALL_COST).unwrap());
    let (root_cost, root_calls): (i64, String) = sqlx::query_as(
        "SELECT cost_nanos, call_ids FROM elements WHERE id = 'overall-architecture'",
    )
    .fetch_one(&s.pool)
    .await
    .unwrap();
    let ids: Vec<String> = serde_json::from_str(&root_calls).unwrap();
    assert!(
        ids.len() >= 2 && ids.iter().all(|id| id.starts_with("req-")),
        "{ids:?}"
    );
    assert_eq!(
        root_cost,
        i64::try_from(FAKE_CALL_COST).unwrap() * i64::try_from(ids.len()).unwrap()
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
    // One facts hash: the skeleton pass right after generation finds nothing
    // stale and keeps the model's names, partition and diagram.
    let changed = crate::map::skeleton::recompute(&s.ix, &s.ri).await.unwrap();
    let stale: Vec<(String,)> = sqlx::query_as("SELECT id FROM elements WHERE stale <> 0")
        .fetch_all(&s.pool)
        .await
        .unwrap();
    assert!(
        stale.is_empty() && changed.is_empty(),
        "stale after recompute: {stale:?} (changed {changed:?})"
    );
    let (name, origin, diagram): (String, String, String) = sqlx::query_as(
        "SELECT name, origin, diagram FROM elements WHERE id = 'overall-architecture/unit:crates/a'",
    )
    .fetch_one(&s.pool)
    .await
    .unwrap();
    assert!(
        name.starts_with("Group n") && origin == "generated",
        "{name} {origin}"
    );
    assert!(diagram.contains("unit:crates/a/src/"), "{diagram}");
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
async fn bundle_is_a_leaf_door_to_its_members() {
    let _serial = SERIAL.lock().await;
    let s = scratch().await;
    {
        let mut sc = script();
        *sc = Script::default();
        // The model groups every unit but the last: that one is the bundle.
        sc.handlers
            .entry("top")
            .or_default()
            .push_back(Box::new(|u| {
                let mut r = top_reply(u, "uses it", None);
                let dropped = r["nodes"].as_array_mut().unwrap().pop().unwrap()["id"]
                    .as_str()
                    .unwrap()
                    .to_owned();
                let mermaid: String = r["mermaid"]
                    .as_str()
                    .unwrap()
                    .lines()
                    .filter(|l| {
                        !l.split(|c: char| !c.is_alphanumeric())
                            .any(|t| t == dropped)
                    })
                    .map(|l| format!("{l}\n"))
                    .collect();
                r["mermaid"] = json!(mermaid);
                Ok(r.to_string())
            }));
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
    wait_idle(&s.root).await;
    let (stage, _, _, log) = job_row(&s.pool, &job.id).await;
    assert_eq!(stage, "done", "{log:?}");
    let (id, members, leaf, origin): (String, String, i64, String) = sqlx::query_as(
        "SELECT id, members, leaf, origin FROM elements WHERE kind = 'bundle' AND depth = 1",
    )
    .fetch_one(&s.pool)
    .await
    .unwrap();
    let members: Vec<String> = serde_json::from_str(&members).unwrap();
    assert_eq!((members.len(), leaf, origin.as_str()), (1, 1, "generated"));
    let (under,): (i64,) = sqlx::query_as("SELECT count(*) FROM elements WHERE parent = ?")
        .bind(&id)
        .fetch_one(&s.pool)
        .await
        .unwrap();
    assert_eq!(under, 0, "a bundle opens its member list, not a diagram");
    // The skeleton pass agrees: nothing deleted, nothing stale, members kept.
    let dump = || async {
        let rows: Vec<(String, String, String, String, Option<String>)> = sqlx::query_as(
            "SELECT id, coalesce(parent,''), kind, members, facts_hash FROM elements ORDER BY depth, order_no",
        )
        .fetch_all(&s.pool)
        .await
        .unwrap();
        let edges: Vec<(String, String, String, i64)> = sqlx::query_as(
            "SELECT from_node, to_node, label, backed FROM edges WHERE element = 'overall-architecture'",
        )
        .fetch_all(&s.pool)
        .await
        .unwrap();
        format!("{rows:#?}\n{edges:#?}")
    };
    let d0 = dump().await;
    let (before,): (i64,) = sqlx::query_as("SELECT count(*) FROM elements")
        .fetch_one(&s.pool)
        .await
        .unwrap();
    let changed = crate::map::skeleton::recompute(&s.ix, &s.ri).await.unwrap();
    let (after, stale): (i64, i64) =
        sqlx::query_as("SELECT count(*), sum(stale <> 0) FROM elements")
            .fetch_one(&s.pool)
            .await
            .unwrap();
    assert_eq!(
        (after, stale, changed.len()),
        (before, 0, 0),
        "changed {changed:?}\nBEFORE {d0}\nAFTER {}",
        dump().await
    );
    let (again,): (String,) = sqlx::query_as("SELECT members FROM elements WHERE id = ?")
        .bind(&id)
        .fetch_one(&s.pool)
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Vec<String>>(&again).unwrap(),
        members
    );
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
        // Stage B puts one element per transaction: wait for the three the
        // assertion below needs, not the first.
        if n >= 3 {
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

// --- F1-R2: the live proof's findings --------------------------------------------

/// R2-1: a single-member group over a top-level FILE is a `file` leaf; the
/// omm-shaped repo generates to `done` (the live run panicked asking the file
/// for children). R2-7: the job's cost is the sum of the replies' and the
/// final `CodeMapProgress` carries it.
#[tokio::test]
async fn file_unit_group_is_a_leaf_and_omm_shape_generates() {
    let _serial = SERIAL.lock().await;
    let s = scratch_with(omm_shaped).await;
    *script() = Script::default();
    let facts = Facts::load(&s.ri, &s.pool).await.unwrap();
    assert!(
        facts
            .top
            .iter()
            .any(|c| c.path == "tsup.config.ts" && c.kind == "file"),
        "{:?}",
        facts
            .top
            .iter()
            .map(|c| (&c.path, c.kind))
            .collect::<Vec<_>>()
    );
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
    wait_idle(&s.root).await;
    let (stage, _, calls, log) = job_row(&s.pool, &job.id).await;
    assert_eq!(stage, "done", "{log:?}");
    let (kind, leaf, under): (String, i64, i64) = sqlx::query_as(
        "SELECT kind, leaf, (SELECT count(*) FROM elements WHERE parent = e.id) FROM elements e \
         WHERE id = 'overall-architecture/unit:tsup.config.ts'",
    )
    .fetch_one(&s.pool)
    .await
    .unwrap();
    assert_eq!((kind.as_str(), leaf, under), ("file", 1, 0));
    let (cost,): (i64,) = sqlx::query_as("SELECT cost_nanos FROM jobs WHERE id = ?")
        .bind(&job.id)
        .fetch_one(&s.pool)
        .await
        .unwrap();
    let expected = calls * i64::try_from(FAKE_CALL_COST).unwrap();
    assert_eq!(cost, expected);
    let progress = s.progress();
    let last = progress.last().expect("a final CodeMapProgress");
    assert_eq!(
        (last.0.as_str(), last.1, last.2),
        (
            "done",
            u32::try_from(calls).unwrap(),
            u64::try_from(expected).unwrap()
        ),
        "{progress:?}"
    );
}

/// R2-2: a panic inside the pipeline ends the job `failed` with the message
/// and publishes the final progress; the engine keeps serving.
#[tokio::test]
async fn panic_in_the_pipeline_fails_the_job() {
    let _serial = SERIAL.lock().await;
    let s = scratch().await;
    {
        let mut sc = script();
        *sc = Script::default();
        sc.handlers
            .entry("select")
            .or_default()
            .push_back(Box::new(|_| panic!("fake model exploded")));
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
    wait_idle(&s.root).await;
    let (stage, _, _, log) = job_row(&s.pool, &job.id).await;
    assert_eq!(stage, "failed");
    assert!(
        log.as_deref()
            .unwrap_or_default()
            .contains("generator panicked: fake model exploded"),
        "{log:?}"
    );
    let (finished,): (Option<i64>,) = sqlx::query_as("SELECT finished_ms FROM jobs WHERE id = ?")
        .bind(&job.id)
        .fetch_one(&s.pool)
        .await
        .unwrap();
    assert!(finished.is_some());
    let last = s.progress().pop().expect("a final CodeMapProgress");
    assert_eq!(last.0, "failed");
    assert!(
        last.3.as_deref().unwrap_or_default().contains("panicked"),
        "{last:?}"
    );
    // A later generate starts a fresh job.
    *script() = Script::default();
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
    assert_eq!(job_row(&s.pool, &again.id).await.0, "done");
}

/// R2-3: the exact replies the live model returned (anthropic/claude-sonnet-4-5,
/// `/tmp/147/proof/repro-topB.out`): a ```json fence and top-level prose
/// instead of `fields`, and a node carrying a `description`. Both parse now;
/// the validator still owns ids, members, labels and prose length.
#[test]
fn live_rejected_stage_b_replies_parse() {
    let fenced = "```json\n{\n  \"description\": \"The codebase comprises two top-level directories: a main source tree containing application logic, tests, and 410 exported symbols across 55 files, and a scripts directory with build or automation utilities. The source tree dominates at 93% of the codebase and references the scripts folder for tooling support. No units are bundled; both directories are accounted for in the architecture.\",\n  \"nodes\": [\n    {\n      \"id\": \"g1\",\n      \"name\": \"Application Source\",\n      \"members\": [\"n1\"]\n    },\n    {\n      \"id\": \"g2\",\n      \"name\": \"Build Scripts\",\n      \"members\": [\"n2\"]\n    }\n  ],\n  \"mermaid\": \"flowchart LR\\n  g1 -->|\\\"invokes tooling\\\"| g2\",\n  \"edges\": [\n    {\n      \"from\": \"g1\",\n      \"to\": \"g2\",\n      \"evidence\": [\"e1\"]\n    }\n  ]\n}\n```";
    let r: TopReply = parse_reply(fenced).unwrap();
    assert!(
        r.fields
            .description
            .starts_with("The codebase comprises two")
    );
    assert_eq!(r.nodes.len(), 2);
    assert_eq!(r.edges[0].evidence, ["e1"]);

    let node_prose = r#"{"fields":{"description":"Two groups. The source and the scripts."},
        "nodes":[{"id":"g1","name":"Source","members":["n1"],"description":"the app"}],
        "mermaid":"flowchart LR\n  g1","edges":[],"notes":"ignored"}"#;
    let r: TopReply = parse_reply(node_prose).unwrap();
    assert_eq!(r.nodes[0].name, "Source");

    // No prose anywhere: parses with an empty description — the validator's
    // `/fields/description` line, not a serde `missing field`, drives repair.
    let bare = r#"{"nodes":[{"id":"g1","name":"Source","members":["n1"]}],"mermaid":"flowchart LR\n  g1"}"#;
    let r: TopReply = parse_reply(bare).unwrap();
    assert!(r.fields.description.is_empty());
    assert!(check_description("/fields/description", &r.fields.description).is_some());

    // Top-level prose on a describe element folds too.
    let described = r#"{"elements":[{"id":"x","name":"X","description":"Prose here. More prose.","context":"why"}]}"#;
    let r: DescribeReply = parse_reply(described).unwrap();
    assert_eq!(r.elements[0].fields.context.as_deref(), Some("why"));
}

/// R2-3 end to end + R2-6: a stage-B reply in the live shape is accepted
/// without a repair round; a reply rejected twice leaves its text in
/// `jobs.error_log`, and the repair message re-sent the facts.
#[tokio::test]
async fn live_shape_passes_and_rejected_text_is_logged() {
    let _serial = SERIAL.lock().await;
    let s = scratch().await;
    {
        let mut sc = script();
        *sc = Script::default();
        sc.handlers
            .entry("top")
            .or_default()
            .push_back(Box::new(|u| {
                let mut r = top_reply(u, "uses it", None);
                let fields = r.as_object_mut().unwrap().remove("fields").unwrap();
                r["description"] = fields["description"].clone();
                Ok(format!("```json\n{r}\n```"))
            }));
        // Stage C: both attempts leave every description empty → invalid twice.
        for _ in 0..2 {
            sc.handlers
                .entry("describe")
                .or_default()
                .push_back(Box::new(|u| {
                    let mut v: Value =
                        serde_json::from_str(&default_reply("describe", u)?).unwrap();
                    for e in v["elements"].as_array_mut().into_iter().flatten() {
                        e["fields"]["description"] = json!("");
                    }
                    Ok(format!("REJECT-ME {v}"))
                }));
        }
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
    wait_idle(&s.root).await;
    let (stage, _, _, log) = job_row(&s.pool, &job.id).await;
    assert_eq!(stage, "done", "{log:?}");
    let (repaired,): (i64,) =
        sqlx::query_as("SELECT repaired FROM elements WHERE id = 'overall-architecture'")
            .fetch_one(&s.pool)
            .await
            .unwrap();
    assert_eq!(repaired, 0, "the live shape needs no repair round");
    let log = log.unwrap_or_default();
    assert!(log.contains("rejected reply (attempt 1"), "{log}");
    assert!(log.contains("rejected reply (attempt 2"), "{log}");
    assert!(log.contains("<reply>\nREJECT-ME"), "{log}");
    let repair = {
        let seen = script();
        seen.calls
            .iter()
            .find(|c| c.user.contains("<errors>"))
            .map(|c| c.user.clone())
            .expect("a repair message was sent")
    };
    assert!(repair.contains("<request>\n{"), "{repair}");
    assert!(repair.contains("\"stage\": \"describe\""), "{repair}");
}

/// R2-4: a `jobs` row a dead process left running is `interrupted` when the
/// map opens in a new process; the next generate starts a fresh job.
#[tokio::test]
async fn orphan_job_is_interrupted_on_open() {
    let _serial = SERIAL.lock().await;
    let s = scratch().await;
    *script() = Script::default();
    let orphan = MapJob {
        id: new_id(),
        session: SessionId::new(),
        started_ms: 1,
        stage: "describe".to_owned(),
        done: 3,
        total: 9,
        calls: 4,
        cost_nanos: 7,
        currency: "USD".to_owned(),
        error: None,
    };
    w::write_job(&s.pool, &orphan, false, None).await.unwrap();
    assert!(
        super::super::store::running_job(&s.pool)
            .await
            .unwrap()
            .is_some()
    );
    // A new process: a fresh `Codeindex` over the same data dir.
    let ix2 = Arc::new(Codeindex::new(crate::CodeindexConfig::default(), &s.data.0).unwrap());
    let ri2 = ix2.open_root(&s.root).await.unwrap();
    let pool2 = ix2.map_pool(&ri2).await.unwrap();
    assert!(
        super::super::store::running_job(&pool2)
            .await
            .unwrap()
            .is_none()
    );
    let (stage, error): (String, Option<String>) =
        sqlx::query_as("SELECT stage, error FROM jobs WHERE id = ?")
            .bind(&orphan.id)
            .fetch_one(&pool2)
            .await
            .unwrap();
    assert_eq!(stage, "interrupted");
    assert!(error.unwrap_or_default().starts_with("interrupted"));
    let ov = super::super::skeleton::overview(
        &ix2,
        super::super::Overview {
            root: s.root.clone(),
        },
    )
    .await
    .unwrap();
    assert!(ov.meta.job.is_none());
    assert_ne!(ov.meta.state, super::super::MapState::Generating);
    let job = generate(
        &ix2,
        Generate {
            root: s.root.clone(),
            session: SessionId::new(),
            element: None,
        },
    )
    .await
    .unwrap();
    assert_ne!(job.id, orphan.id);
    wait_idle(&s.root).await;
    assert_eq!(job_row(&pool2, &job.id).await.0, "done");
}

/// R2-5: a session the supervisor would refuse is refused before any job row
/// is written.
#[tokio::test]
async fn dead_session_is_refused_up_front() {
    let _serial = SERIAL.lock().await;
    let s = scratch().await;
    let dead = SessionId::new();
    {
        let mut sc = script();
        *sc = Script::default();
        sc.dead_session = Some(dead);
    }
    let err = generate(
        &s.ix,
        Generate {
            root: s.root.clone(),
            session: dead,
            element: None,
        },
    )
    .await
    .unwrap_err()
    .to_string();
    assert_eq!(
        err,
        format!("generation needs an open native session: no live session {dead}")
    );
    assert!(!jobs().contains_key(&s.root));
    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM jobs")
        .fetch_one(&s.pool)
        .await
        .unwrap();
    assert_eq!(n, 0);
    script().dead_session = None;
}

// --- fixture-backed validation tests ------------------------------------------------

/// A stage-B reply over one group per unit; `mermaid` is the edge lines only,
/// the group declarations are prepended.
fn reply(facts: &Facts, h: &Handles, mermaid: &str, edges: Vec<Value>) -> TopReply {
    let nodes: Vec<Value> = h
        .units
        .iter()
        .map(|(handle, node)| json!({ "id": format!("g{}", &handle[1..]), "name": facts.top_unit(node).unwrap().name, "members": [handle] }))
        .collect();
    let decls: String = h
        .units
        .iter()
        .map(|(handle, _)| format!("  g{}\n", &handle[1..]))
        .collect();
    let mermaid = format!("{decls}{}", mermaid.trim_start_matches("flowchart LR\n"));
    serde_json::from_value(json!({
        "fields": { "description": "A fixture workspace with a library crate and a binary crate." },
        "nodes": nodes, "mermaid": format!("flowchart LR\n{mermaid}"), "edges": edges,
    }))
    .unwrap()
}

fn group_handle(h: &Handles, path: &str) -> String {
    format!("g{}", &h.unit_handle(&format!("unit:{path}")).unwrap()[1..])
}

#[tokio::test]
async fn edges_without_evidence_need_a_reason() {
    let _serial = SERIAL.lock().await;
    let s = scratch().await;
    let facts = Facts::load(&s.ri, &s.pool).await.unwrap();
    let h = Handles::new(&facts.top, &facts.edges);
    let (a, b) = (group_handle(&h, "crates/a"), group_handle(&h, "crates/b"));
    // b → a: a Cargo path dep and imports; a → b: nothing in the index.
    let src = format!(
        "flowchart LR\n  {b} -->|\"uses the library\"| {a}\n  {a} -->|\"answers it\"| {b}\n"
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
            vec![json!({ "from": a, "to": b, "reason": "the binary re-exports the prelude" })],
        ),
        &facts,
        &h,
        "overall-architecture",
    )
    .unwrap();
    let inferred = ok.edges.iter().find(|e| e.from == "unit:crates/a").unwrap();
    assert!(!inferred.backed);
    assert_eq!(
        inferred.evidence,
        vec![MapEvidence::Inferred {
            reason: "the binary re-exports the prelude".into()
        }]
    );
    let backed = ok.edges.iter().find(|e| e.from == "unit:crates/b").unwrap();
    assert!(
        backed.backed
            && backed
                .evidence
                .iter()
                .any(|e| matches!(e, MapEvidence::Manifest { .. })),
        "{backed:?}"
    );
    assert!(ok.uncovered.is_empty());

    // A cited evidence handle must join the two groups.
    let cited = validate_top(
        &reply(
            &facts,
            &h,
            &src,
            vec![json!({ "from": a, "to": b, "evidence": ["e1"], "reason": "x" })],
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

#[tokio::test]
async fn labels_with_counts_are_rejected() {
    let _serial = SERIAL.lock().await;
    let s = scratch().await;
    let facts = Facts::load(&s.ri, &s.pool).await.unwrap();
    let h = Handles::new(&facts.top, &facts.edges);
    let (a, b) = (group_handle(&h, "crates/a"), group_handle(&h, "crates/b"));
    let src = format!("flowchart LR\n  {b} -->|\"mesh · 4 deps\"| {a}\n");
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
    let msg = validate::repair_message("{}", "{…}", &err).unwrap();
    assert!(msg.contains("/mermaid/edges/0 · label \"mesh · 4 deps\" carries a number · counts are rendered from evidence"));

    // A label override in `edges` is checked the same way; a clean one passes.
    let src = format!("flowchart LR\n  {b} --> {a}\n");
    let ok = validate_top(
        &reply(
            &facts,
            &h,
            &src,
            vec![json!({ "from": b, "to": a, "label": "drives the library" })],
        ),
        &facts,
        &h,
        "overall-architecture",
    )
    .unwrap();
    assert_eq!(ok.edges[0].label, "drives the library");
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
    let detections: Vec<Detection> = PERSPECTIVES
        .iter()
        .map(|k| Detection {
            kind: (*k).to_owned(),
            eligible: matches!(*k, "overall-architecture" | "dependency-map"),
            hits: 0,
            samples: vec![],
            confidence: 0.0,
        })
        .collect();
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
        &detections,
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

#[tokio::test]
async fn structure_fallback_and_tree_shape() {
    let _serial = SERIAL.lock().await;
    let s = scratch().await;
    let facts = Facts::load(&s.ri, &s.pool).await.unwrap();
    let plan = structure_plan(&facts, MapFields::default());
    assert_eq!(plan.groups.len(), facts.top.len());
    assert!(
        plan.edges.iter().all(|e| e.backed && !e.label.is_empty()),
        "{:?}",
        plan.edges
    );
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
    let a = rows
        .iter()
        .find(|r| r.id == "overall-architecture/unit:crates/a")
        .unwrap();
    assert_eq!(
        (a.depth, a.leaf, a.origin),
        (1, false, MapOrigin::Generated)
    );
    let g = a.diagram.as_ref().unwrap();
    assert!(
        g.nodes
            .iter()
            .any(|n| n.id.starts_with("unit:crates/a/src/")),
        "{:?}",
        g.nodes
    );
    let modules: Vec<&ElementRow> = rows
        .iter()
        .filter(|r| r.parent.as_deref() == Some("overall-architecture/unit:crates/a"))
        .collect();
    assert!(modules.len() >= 2, "{modules:#?}");
    assert!(
        modules
            .iter()
            .all(|m| m.depth == 2 && m.origin == MapOrigin::Structure)
    );
    let b = rows
        .iter()
        .find(|r| r.id == "overall-architecture/unit:crates/b")
        .unwrap();
    assert!(b.leaf && b.diagram.is_none(), "a ≤ 2-file unit is a leaf");
    assert!(rows.iter().all(|r| r.depth <= MAX_DEPTH));
    assert_eq!(
        sub_line(&["a/b".into(), "c/d".into(), "e".into(), "f".into()]).as_deref(),
        Some("a/b · c/d +2")
    );
}
