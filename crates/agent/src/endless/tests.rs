//! End-to-end over a real store with a scripted compactor: attach → pump →
//! settle → render → tools → `/endless off|on`.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use genai::chat::{ChatMessage, ContentPart, MessageContent, ToolCall, ToolResponse};
use parking_lot::Mutex;
use runtime::{Runtime, SessionId};

use super::compactor::{Compactor, Reply};
use super::{Attached, Endless, EndlessConfig, MemoryMode, UnitFilter};
use crate::store::Store;

/// Echoes the step input (the text after the last `bytes:\n`) cut to 400
/// bytes — grounded, no ruler, no status words of its own.
struct Echo {
    calls: Mutex<u32>,
    model: models::Resolved,
}

/// Echo's reply: the step input (after the last `bytes:\n`) cut to 400 bytes, on one line.
fn echo_line(convo: &[ChatMessage]) -> String {
    let step = convo
        .first()
        .and_then(|m| m.content.iter().next())
        .and_then(|p| match p {
            ContentPart::Text(t) => Some(t.as_str()),
            _ => None,
        })
        .unwrap_or("");
    let input = step.rsplit_once("bytes:\n").map_or(step, |(_, s)| s);
    super::guards::cut_utf8(input, 400).replace('\n', " ")
}

impl Compactor for Echo {
    fn compress<'a>(
        &'a self,
        _ctx: &'a str,
        convo: &'a [ChatMessage],
    ) -> BoxFuture<'a, Result<Reply, String>> {
        *self.calls.lock() += 1;
        let line = echo_line(convo);
        Box::pin(async move {
            Ok(Reply {
                text: line,
                usage: serde_json::from_str(r#"{"prompt_tokens":1000,"completion_tokens":50,"prompt_tokens_details":{"cached_tokens":900}}"#).ok(),
                latency: Duration::from_millis(1),
            })
        })
    }

    fn model(&self) -> &models::Resolved {
        &self.model
    }
}

async fn harness() -> (crate::TestDir, Endless, Store, Arc<Echo>, SessionId) {
    harness_with(EndlessConfig {
        view_bytes: 16_000,
        settle_secs: 5,
        ..EndlessConfig::default()
    })
    .await
}

async fn harness_with(
    cfg: EndlessConfig,
) -> (crate::TestDir, Endless, Store, Arc<Echo>, SessionId) {
    let echo = Arc::new(Echo {
        calls: Mutex::new(0),
        model: super::pump::resolved_for_tests("fake/compactor"),
    });
    let (scratch, endless, store, id) = harness_for(cfg, echo.clone()).await;
    (scratch, endless, store, echo, id)
}

async fn harness_for(
    cfg: EndlessConfig,
    compactor: Arc<dyn Compactor>,
) -> (crate::TestDir, Endless, Store, SessionId) {
    harness_factory(cfg, Arc::new(move |_: &str| compactor.clone())).await
}

/// The harness with a compactor per resolved model id (137F: the id follows settings live).
async fn harness_factory(
    cfg: EndlessConfig,
    factory: super::pump::CompactorFactory,
) -> (crate::TestDir, Endless, Store, SessionId) {
    let scratch = crate::TestDir::new("endless-e2e");
    let dir = scratch.0.clone();
    let store = Store::open(&dir.join("sessions.sqlite"), None)
        .await
        .unwrap();
    let mut endless = Endless::new(
        store.clone(),
        Runtime::new(figment::Figment::new(), dir.clone()),
        genai::Client::builder().build().unwrap(),
        crate::supervisor::Auth::stub(&dir),
        Arc::new(models::Catalog::load(&dir)),
        cfg,
        None,
    );
    Arc::get_mut(&mut endless.inner)
        .unwrap()
        .compactor_for_tests = Some(factory);
    let id = SessionId::new();
    store
        .create_session(id, "fake/session", None, None, None)
        .await
        .unwrap();
    (scratch, endless, store, id)
}

/// Every call fails: a deterministic 400 from the provider.
struct Fail {
    model: models::Resolved,
}

impl Compactor for Fail {
    fn compress<'a>(
        &'a self,
        _ctx: &'a str,
        _convo: &'a [ChatMessage],
    ) -> BoxFuture<'a, Result<Reply, String>> {
        Box::pin(async { Err("400: thinking.enabled is not supported".to_owned()) })
    }

    fn model(&self) -> &models::Resolved {
        &self.model
    }
}

/// A node that keeps failing backs off (`retry_secs` doubling, 30 min cap)
/// and one success clears its failure state.
#[tokio::test]
async fn failing_node_backs_off_until_a_success() {
    let base = 10;
    let (_dir, endless, store, id) = harness_for(
        EndlessConfig {
            view_bytes: 16_000,
            retry_secs: base,
            ..EndlessConfig::default()
        },
        Arc::new(Fail {
            model: super::pump::resolved_for_tests("fake/failing"),
        }),
    )
    .await;
    store
        .set_session_memory(id, MemoryMode::Endless)
        .await
        .unwrap();
    let long = (0..60)
        .map(|k| format!("worker_processes auto; line {k} of the nginx config"))
        .collect::<Vec<_>>()
        .join("\n");
    seed(&store, id, &long).await;
    endless.attach(id, attached()).await.unwrap();

    // The pump's own first failure.
    let mut failed = Vec::new();
    for _ in 0..200 {
        failed = super::pump::failed_for_tests(&endless.inner, id);
        if !failed.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let (key, at, n) = failed.first().copied().expect("a node failed");
    assert_eq!(n, 1);
    assert!(at > Instant::now() + Duration::from_secs(u64::from(base) - 1));

    // Drive the next failures directly: no waiting out the delays.
    for k in 2..=12u32 {
        let before = Instant::now();
        super::pump::finish_for_tests(&endless.inner, id, key, Err("400".to_owned())).await;
        let (_, at, n) = super::pump::failed_for_tests(&endless.inner, id)
            .into_iter()
            .find(|(k, ..)| *k == key)
            .expect("still failed");
        assert_eq!(n, k);
        let want = Duration::from_secs(u64::from(base) << (k - 1)).min(Duration::from_secs(1800));
        assert!(at >= before + want, "k={k}: {:?} < {want:?}", at - before);
        assert!(at <= Instant::now() + want, "k={k}: not past the cap");
    }

    super::pump::finish_for_tests(
        &endless.inner,
        id,
        key,
        Ok(super::tree::Node {
            l: key.0,
            i: key.1,
            text: "built".into(),
            size: 5,
            free: false,
            model: Some("fake/failing".into()),
            tries: 1,
            mechanical: false,
        }),
    )
    .await;
    assert!(
        !super::pump::failed_for_tests(&endless.inner, id)
            .iter()
            .any(|(k, ..)| *k == key),
        "a success clears the failure state"
    );
}

fn attached() -> Attached {
    Attached {
        model: super::pump::resolved_for_tests("fake/session"),
        effort: None,
        agent: "theuth".into(),
        store: None,
    }
}

async fn seed(store: &Store, id: SessionId, long: &str) {
    store
        .append(
            id,
            &ChatMessage::user("hello, please look at the nginx config"),
        )
        .await
        .unwrap();
    store
        .append(
            id,
            &ChatMessage::assistant(MessageContent::from_parts(vec![
                ContentPart::Text("on it".into()),
                ContentPart::ToolCall(ToolCall {
                    call_id: "c1".into(),
                    fn_name: "read".into(),
                    fn_arguments: serde_json::json!({"path": "/etc/nginx/nginx.conf"}),
                    thought_signatures: None,
                }),
            ])),
        )
        .await
        .unwrap();
    store
        .append(
            id,
            &ChatMessage::from(vec![ToolResponse::new("c1", long.to_owned())]),
        )
        .await
        .unwrap();
    store
        .append(id, &ChatMessage::assistant(long.to_owned()))
        .await
        .unwrap();
    store
        .append(
            id,
            &ChatMessage::user("ok, keep the worker_processes line as it is"),
        )
        .await
        .unwrap();
}

/// A fresh endless session: units derive from the rows, free and model-built
/// nodes land, the view settles, tools answer, the ledger fills, `/endless
/// off` writes the head and `on` needs no backfill.
#[tokio::test]
async fn pump_builds_tree_and_tools_answer() {
    let (_dir, endless, store, echo, id) = harness().await;
    store
        .set_session_memory(id, MemoryMode::Endless)
        .await
        .unwrap();
    let long = (0..60)
        .map(|k| format!("worker_processes auto; line {k} of the nginx config with error_log /var/log/nginx/error.log"))
        .collect::<Vec<_>>()
        .join("\n");
    seed(&store, id, &long).await;

    endless.attach(id, attached()).await.unwrap();
    assert_eq!(endless.mode(id).await.unwrap(), MemoryMode::Endless);
    assert!(
        endless.active(id),
        "no history before the mode: no backfill"
    );
    assert!(endless.settle(id, Duration::from_secs(10)).await, "settled");

    let render = endless.render_for_turn(id).await.unwrap();
    assert!(render.settled && render.unbuilt == 0);
    let text = render.text();
    assert!(text.starts_with("<chat>\n") && text.ends_with("</chat>"));
    assert!(
        text.contains("0+1|user: hello, please look at the nginx config"),
        "{text}"
    );
    assert_eq!(
        text.lines().count(),
        6 + 2,
        "six units, one line each under budget"
    );
    assert!(
        *echo.calls.lock() >= 2,
        "the two long units needed the model"
    );

    let mem = endless.memory(id).await.unwrap();
    assert_eq!(mem.mode, MemoryMode::Endless);
    assert_eq!(mem.units, 6);
    assert!(mem.view.iter().all(|l| l.built));
    assert_eq!(mem.model, "fake/compactor");
    assert!(
        mem.stats.calls >= 2
            && mem.stats.input_tokens >= 2000
            && mem.stats.cache_read_tokens >= 1800
    );
    assert!(mem.backfill.is_none());

    // Merges run behind the view (they are not what `settle` waits for).
    eventually("a level-2 merge", || async {
        store
            .endless_nodes(id)
            .await
            .unwrap()
            .iter()
            .any(|n| n.l == 2)
    })
    .await;
    let nodes = store.endless_nodes(id).await.unwrap();
    assert!(nodes.iter().any(|n| n.l == 0 && n.free) && nodes.iter().any(|n| n.l == 0 && !n.free));
    let kinds: Vec<String> = sqlx::query_scalar("SELECT kind FROM model_calls WHERE owner_id = ?")
        .bind(id.to_string())
        .fetch_all(store.pool())
        .await
        .unwrap();
    assert!(
        !kinds.is_empty() && kinds.iter().all(|k| k == "endless"),
        "{kinds:?}"
    );

    // Tools.
    let whole = endless.zoom(id, 0, 1).await.unwrap();
    assert_eq!(
        whole[0].text,
        "user: hello, please look at the nginx config"
    );
    assert!(whole[0].at.is_some(), "n == 1 carries the row time");
    assert!(mem.view.iter().all(|l| l.at.is_some() == (l.n == 1)));
    let children = endless.zoom(id, 0, 2).await.unwrap();
    assert_eq!((children[0].id, children[0].n, children[1].id), (0, 1, 1));
    assert_eq!(endless.zoom(id, 1, 2).await.unwrap_err(), "No line 1+2.");
    assert_eq!(endless.zoom(id, 8, 1).await.unwrap_err(), "No line 8+1.");
    let hits = endless.recall(id, "worker_processes", 8).await.unwrap();
    assert!(
        !hits.is_empty() && hits.iter().all(|h| h.n == 1),
        "{hits:?}"
    );
    let raw = endless.recall(id, "zzz-nowhere", 8).await.unwrap();
    assert!(raw.is_empty());
    assert!(endless.date(id, 0).await.unwrap().is_some());
    assert_eq!(endless.date(id, 99).await.unwrap_err(), "No message 99.");

    // /endless off → head, then on → no backfill (the tree exists).
    let off = endless
        .set_mode(id, MemoryMode::Epochs, 3, UnitFilter::All)
        .await
        .unwrap();
    let head = off.head.unwrap();
    assert!(head.starts_with("<context_summary epoch=\"3\" style=\"endless\">\n<chat>\n"));
    assert!(head.ends_with("</chat>\n</context_summary>"));
    assert_eq!(off.memory.mode, MemoryMode::Epochs);
    assert!(off.memory.view.is_empty());
    assert!(!endless.active(id));
    let on = endless
        .set_mode(id, MemoryMode::Endless, 3, UnitFilter::All)
        .await
        .unwrap();
    assert!(on.head.is_none() && on.memory.backfill.is_none());
    assert!(endless.active(id));
    endless.detach(id);
}

/// D14: history before the switch is a backfill — `active` stays false (the
/// loop keeps epochs) until level 0 covers it; a second attach (restart)
/// reloads the tree from rows without rebuilding.
#[tokio::test]
async fn history_backfills_then_activates_and_resumes_from_rows() {
    let (_dir, endless, store, echo, id) = harness().await;
    seed(&store, id, "short tool output").await;
    endless.attach(id, attached()).await.unwrap();
    assert!(!endless.active(id), "epochs session");
    let on = endless
        .set_mode(id, MemoryMode::Endless, 1, UnitFilter::All)
        .await
        .unwrap();
    let mem = on.memory;
    assert!(
        mem.backfill.is_some_and(|b| b.total == 6) || endless.active(id),
        "{mem:?}"
    );
    assert!(endless.settle(id, Duration::from_secs(10)).await);
    assert!(endless.active(id), "covered → active");
    // Settle waits for the view only; let the merges above finish (6 + 3 + 1 nodes).
    for _ in 0..200 {
        if store.endless_nodes(id).await.unwrap().len() == 10 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let built_before = store.endless_nodes(id).await.unwrap().len();
    assert_eq!(built_before, 10);
    let calls_before = *echo.calls.lock();
    endless.detach(id);

    // Restart: rows are the hand-off (8.7).
    let (_dir2, endless2, ..) = harness().await;
    assert_eq!(
        endless2.resume_all().await.unwrap(),
        0,
        "another store: nothing to resume"
    );
    endless.attach(id, attached()).await.unwrap();
    assert!(endless.active(id));
    assert!(endless.settle(id, Duration::from_secs(5)).await);
    assert_eq!(store.endless_nodes(id).await.unwrap().len(), built_before);
    assert_eq!(*echo.calls.lock(), calls_before, "nothing rebuilt");
    assert_eq!(endless.resume_all().await.unwrap(), 1);
}

/// §11.2 wire shapes: snake_case modes, the event's `type` tag, and
/// `SessionSummary.memory` defaulting for rows from an older harness.
#[test]
fn wire_types_round_trip() {
    use super::{EndlessProgress, SessionMemory};
    assert_eq!(
        serde_json::to_string(&MemoryMode::Endless).unwrap(),
        "\"endless\""
    );
    assert_eq!(
        serde_json::from_str::<MemoryMode>("\"epochs\"").unwrap(),
        MemoryMode::Epochs
    );
    assert_eq!("endless".parse::<MemoryMode>().unwrap().as_str(), "endless");
    let ev = EndlessProgress {
        session: SessionId::new(),
        built: 3,
        total: 10,
        settling: true,
        error: None,
        backfill: None,
        stalled: Some(super::Stalled {
            failures: 2,
            error: "timed out".into(),
            next_retry_at: Some("2026-11-04T21:30:00Z".into()),
            model: "ollama/qwen".into(),
            blocking: true,
        }),
        backlog: Some(super::Backlog {
            lines: 5_000,
            folded: 4_900,
            eta_secs: 600,
            paused: false,
            paused_by: None,
        }),
    };
    let v = serde_json::to_value(&ev).unwrap();
    assert_eq!(v["type"], "EndlessProgress");
    assert_eq!(serde_json::from_value::<EndlessProgress>(v).unwrap(), ev);
    let mem: SessionMemory = serde_json::from_str(
        r#"{"mode":"endless","view":[{"id":0,"n":2,"level":1,"text":"x","built":true}],"settling":false,"unbuilt":0,"units":2,"stats":{"calls":1,"guard_trips":{"copy":0,"grounding":0,"status":0,"format":0,"empty":0,"size":0},"input_tokens":1,"cache_read_tokens":0,"cache_write_tokens":0,"output_tokens":1,"usd_estimate":0.0},"model":"m","view_bytes":128000,"backfill":null}"#,
    )
    .unwrap();
    assert_eq!(mem.view[0].n, 2);
    assert!(
        mem.backlog.is_none() && !mem.view[0].mechanical,
        "older shape: defaults"
    );
    let summary: crate::SessionSummary = serde_json::from_value(serde_json::json!({
        "id": SessionId::new(), "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z",
        "title": "t", "model": "m", "parent": null, "name": null, "effort": null, "agent": null, "cwd": null
    }))
    .unwrap();
    assert_eq!(summary.memory, MemoryMode::Epochs);
}

/// R1-D4: a capped backfill — the tree covers `[base, T)`, the view opens
/// with the raw-history line, `zoom(id, 1)` and recall's raw fallback still
/// reach the units below, merged lines below the base do not exist.
#[tokio::test]
async fn backfill_cap_sets_base_and_keeps_raw_history_reachable() {
    let (_dir, endless, store, _echo, id) = harness_with(EndlessConfig {
        view_bytes: 16_000,
        backfill_max_units: 3,
        ..EndlessConfig::default()
    })
    .await;
    seed(&store, id, "short tool output").await;
    endless.attach(id, attached()).await.unwrap();
    let est = endless.backfill_estimate(id).await.unwrap();
    assert_eq!((est.done, est.total), (0, 3), "{est:?}");
    assert!(
        (est.usd_estimate - 0.06).abs() < 1e-9 && est.eta_secs == 15,
        "static defaults: {est:?}"
    );
    let on = endless
        .set_mode(id, MemoryMode::Endless, 1, UnitFilter::All)
        .await
        .unwrap();
    assert_eq!(store.endless_meta(id).await.unwrap().base, 3);
    assert!(
        on.memory.view[0].n == 3 && on.memory.view[0].id == 0,
        "{:?}",
        on.memory.view
    );
    assert!(endless.settle(id, Duration::from_secs(10)).await);
    assert!(endless.active(id));
    let r = endless.render_for_turn(id).await.unwrap();
    assert!(
        r.text()
            .starts_with(&format!("<chat>\n0+3|{}\n3+1|", super::fold::EARLIER)),
        "{}",
        r.text()
    );
    assert!(
        store
            .endless_nodes(id)
            .await
            .unwrap()
            .iter()
            .all(|n| n.i * (1 << n.l) >= 3)
    );
    assert_eq!(
        endless.zoom(id, 0, 1).await.unwrap()[0].text,
        "user: hello, please look at the nginx config"
    );
    assert_eq!(endless.zoom(id, 0, 2).await.unwrap_err(), "No line 0+2.");
    let raw = endless.recall(id, "hello, please", 8).await.unwrap();
    assert!(
        raw.iter().any(|l| l.id == 0 && l.text.ends_with("(raw)")),
        "{raw:?}"
    );
    let snap = endless.render_snapshot(id).await.unwrap();
    assert_eq!(
        snap.iter().map(|b| b.text.as_str()).collect::<String>(),
        r.text()
    );
    // R1b: the budget is the user's, persisted, and shown in `SessionMemory`.
    assert_eq!(endless.memory(id).await.unwrap().view_bytes, 16_000);
    endless.set_view_bytes(id, 40_000).await.unwrap();
    assert_eq!(
        store.endless_meta(id).await.unwrap().view_bytes,
        Some(40_000)
    );
    assert_eq!(endless.memory(id).await.unwrap().view_bytes, 40_000);
    endless.detach(id);
    endless.attach(id, attached()).await.unwrap();
    assert_eq!(
        endless.memory(id).await.unwrap().view_bytes,
        40_000,
        "survives a restart"
    );
    store.set_endless_view_bytes(id, None).await.unwrap();
    endless.detach(id);
    endless.attach(id, attached()).await.unwrap();
    assert_eq!(
        endless.memory(id).await.unwrap().view_bytes,
        16_000,
        "NULL = config"
    );
    let est = endless.backfill_estimate(id).await.unwrap();
    assert_eq!(
        (est.done, est.total, est.usd_estimate),
        (3, 3, 0.0),
        "covered: {est:?}"
    );
    let small = endless.render_with_budget(id, 16_000).await.unwrap();
    assert_eq!(small.len(), on.memory.view.len());
    assert_eq!((small[0].id, small[0].n), (0, 3));
}

/// R1-D3 chain and R1-D6 filter are pure and testable alone.
#[test]
fn compactor_chain_and_meeting_filter() {
    use super::{
        resolved_compactor_model,
        tree::{Kind, Unit},
    };
    let mut cfg = crate::AgentConfig::default();
    assert_eq!(resolved_compactor_model(&cfg, "s"), "s");
    cfg.title_model = Some("t".into());
    assert_eq!(resolved_compactor_model(&cfg, "s"), "t");
    cfg.endless.model = Some("e".into());
    assert_eq!(resolved_compactor_model(&cfg, "s"), "e");
    assert_eq!(cfg.endless.view_bytes, 128_000);
    assert!(cfg.endless.default_desk && !cfg.endless.default_chat && !cfg.endless.default_meeting);
    let unit = |kind: Kind, text: &str| Unit {
        i: 0,
        seq: 0,
        part: 0,
        kind,
        text: text.into(),
        size: 0,
        at: None,
    };
    let f = UnitFilter::Meeting;
    assert!(f.keeps(&unit(Kind::User, "[meeting batch 3] ...")));
    assert!(!f.keeps(&unit(Kind::User, "hello")));
    assert!(f.keeps(&unit(Kind::Tool, "meeting_topic {\"title\":\"x\"}")));
    assert!(!f.keeps(&unit(Kind::Tool, "python {}")));
    assert!(!f.keeps(&unit(Kind::Echo, "[meeting")));
    assert!(UnitFilter::All.keeps(&unit(Kind::Echo, "anything")));
    assert_eq!(
        "meeting".parse::<UnitFilter>().unwrap(),
        UnitFilter::Meeting
    );
}

/// Addendum (W4): a `Meeting`-filter session keeps only meeting units, is
/// never `active()` (head D + verbatim tail stay), and `render_with_budget`
/// serves its lines.
#[tokio::test]
async fn meeting_filter_session_is_served_but_never_active() {
    let (_dir, endless, store, _echo, id) = harness().await;
    store
        .append(
            id,
            &ChatMessage::user("[meeting batch 1] alice: let's ship on Friday"),
        )
        .await
        .unwrap();
    store
        .append(id, &ChatMessage::assistant("noted"))
        .await
        .unwrap();
    store
        .append(
            id,
            &ChatMessage::assistant(MessageContent::from_parts(vec![ContentPart::ToolCall(
                ToolCall {
                    call_id: "c1".into(),
                    fn_name: "meeting_topic".into(),
                    fn_arguments: serde_json::json!({"title": "release"}),
                    thought_signatures: None,
                },
            )])),
        )
        .await
        .unwrap();
    store
        .append(id, &ChatMessage::from(vec![ToolResponse::new("c1", "ok")]))
        .await
        .unwrap();
    store
        .append(
            id,
            &ChatMessage::user("[meeting batch 2] bob: Friday works"),
        )
        .await
        .unwrap();
    endless.attach(id, attached()).await.unwrap();
    endless
        .set_mode(id, MemoryMode::Endless, 1, UnitFilter::Meeting)
        .await
        .unwrap();
    assert_eq!(
        store.endless_meta(id).await.unwrap().filter,
        UnitFilter::Meeting
    );
    assert!(endless.settle(id, Duration::from_secs(10)).await);
    assert!(!endless.active(id), "meetings never take the endless frame");
    let mem = endless.memory(id).await.unwrap();
    assert_eq!(
        mem.units, 3,
        "two batches + the topic call; talk and echo dropped"
    );
    let lines = endless.render_with_budget(id, 32_000).await.unwrap();
    assert_eq!(lines.len(), 3);
    assert!(lines[0].text.starts_with("user: [meeting batch 1]"));
    assert!(lines[1].text.starts_with("tool: meeting_topic"));
    // A restart keeps the filter: a new row that is not meeting material stays out.
    endless.detach(id);
    store
        .append(id, &ChatMessage::assistant("chatter"))
        .await
        .unwrap();
    endless.attach(id, attached()).await.unwrap();
    assert_eq!(endless.memory(id).await.unwrap().units, 3);
}

// ---- 137F: stalls, switching, liveness ----

#[derive(Clone, Copy, Debug, PartialEq)]
enum Act {
    Echo,
    Fail,
    /// Never answers; dropping the call counts in `dropped`.
    Hang,
    /// Panics once, then echoes.
    Panic,
}

/// A scripted compactor that records calls, concurrency and drops.
struct Probe {
    model: models::Resolved,
    act: Mutex<Act>,
    local: Option<String>,
    delay: Duration,
    calls: AtomicU32,
    inflight: AtomicU32,
    max_inflight: AtomicU32,
    dropped: Arc<AtomicU32>,
    /// `<chat>` bytes of every call.
    ctx_lens: Mutex<Vec<usize>>,
    /// The step input of every call, in call order.
    inputs: Mutex<Vec<String>>,
}

impl Probe {
    /// A cloud compactor (no E9 queue).
    fn new(id: &str, act: Act) -> Arc<Self> {
        Self::with(id, act, None, Duration::ZERO)
    }

    fn with(id: &str, act: Act, local: Option<&str>, delay: Duration) -> Arc<Self> {
        Arc::new(Self {
            model: super::pump::resolved_for_tests(id),
            act: Mutex::new(act),
            local: local.map(str::to_owned),
            delay,
            calls: AtomicU32::new(0),
            inflight: AtomicU32::new(0),
            max_inflight: AtomicU32::new(0),
            dropped: Arc::new(AtomicU32::new(0)),
            ctx_lens: Mutex::new(Vec::new()),
            inputs: Mutex::new(Vec::new()),
        })
    }

    fn set(&self, act: Act) {
        *self.act.lock() = act;
    }

    fn calls(&self) -> u32 {
        self.calls.load(Ordering::SeqCst)
    }
}

/// Counts a dropped (cancelled) call.
struct OnDrop(Arc<AtomicU32>);

impl Drop for OnDrop {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

/// Keeps `inflight` / `max_inflight` while a call runs.
struct InFlight<'a>(&'a Probe);

impl<'a> InFlight<'a> {
    fn enter(p: &'a Probe) -> Self {
        let now = p.inflight.fetch_add(1, Ordering::SeqCst) + 1;
        p.max_inflight.fetch_max(now, Ordering::SeqCst);
        Self(p)
    }
}

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        self.0.inflight.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Compactor for Probe {
    fn compress<'a>(
        &'a self,
        ctx: &'a str,
        convo: &'a [ChatMessage],
    ) -> BoxFuture<'a, Result<Reply, String>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.ctx_lens.lock().push(ctx.len());
        let act = *self.act.lock();
        let line = echo_line(convo);
        self.inputs.lock().push(line.clone());
        // Armed now, so a call dropped before its first poll counts too.
        let on_drop = (act == Act::Hang).then(|| OnDrop(self.dropped.clone()));
        Box::pin(async move {
            let _in = InFlight::enter(self);
            tokio::time::sleep(self.delay).await;
            match act {
                Act::Echo => Ok(Reply {
                    text: line,
                    usage: None,
                    latency: self.delay,
                }),
                Act::Fail => Err("400: model not loaded".to_owned()),
                Act::Hang => {
                    let _d = on_drop;
                    futures::future::pending().await
                }
                Act::Panic => {
                    self.set(Act::Echo);
                    panic!("compactor bug");
                }
            }
        })
    }

    fn model(&self) -> &models::Resolved {
        &self.model
    }

    fn local(&self) -> Option<String> {
        self.local.clone()
    }
}

/// `a` for every id but `fake/b`.
fn two(a: Arc<Probe>, b: Arc<Probe>) -> super::pump::CompactorFactory {
    Arc::new(move |id: &str| -> Arc<dyn Compactor> {
        if id == "fake/b" { b.clone() } else { a.clone() }
    })
}

fn cfg_137f(model: Option<&str>) -> EndlessConfig {
    EndlessConfig {
        view_bytes: 16_000,
        retry_secs: 600,
        model: model.map(str::to_owned),
        ..EndlessConfig::default()
    }
}

/// An endless session with two model-built units (the nginx seed), attached.
async fn endless_session(endless: &Endless, store: &Store, id: SessionId) {
    store
        .set_session_memory(id, MemoryMode::Endless)
        .await
        .unwrap();
    let long = (0..60)
        .map(|k| format!("worker_processes auto; line {k} of the nginx config"))
        .collect::<Vec<_>>()
        .join("\n");
    seed(store, id, &long).await;
    endless.attach(id, attached()).await.unwrap();
}

/// Event-driven waits get 10 s (a loaded full-suite run), never a tight bound.
async fn until(what: &str, mut f: impl FnMut() -> bool) {
    for _ in 0..2_000 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("timed out waiting for {what}");
}

/// Model-built nodes and the model that built them.
async fn built_by(store: &Store, id: SessionId) -> Vec<Option<String>> {
    store
        .endless_nodes(id)
        .await
        .unwrap()
        .into_iter()
        .filter(|n| !n.free)
        .map(|n| n.model)
        .collect()
}

/// E1 + E2: a settings switch reaches the next node without a re-attach;
/// the old model's failures are forgotten and the new one is called at once.
#[tokio::test]
async fn settings_model_switch_is_live_and_resets_backoff() {
    let (a, b) = (
        Probe::new("fake/a", Act::Fail),
        Probe::new("fake/b", Act::Echo),
    );
    let (_dir, endless, store, id) =
        harness_factory(cfg_137f(Some("fake/a")), two(a.clone(), b.clone())).await;
    endless_session(&endless, &store, id).await;
    until("a failure", || {
        !super::pump::failed_for_tests(&endless.inner, id).is_empty()
    })
    .await;
    assert_eq!(endless.memory(id).await.unwrap().model, "fake/a");

    endless.set_config(cfg_137f(Some("fake/b")));
    assert!(
        super::pump::failed_for_tests(&endless.inner, id).is_empty(),
        "backoff reset"
    );
    // The old failure backs off 600 s: a call to the new model is the reset's.
    until("a call to the new model", || b.calls() > 0).await;
    assert!(endless.settle(id, Duration::from_secs(10)).await);
    let models = built_by(&store, id).await;
    assert!(
        !models.is_empty() && models.iter().all(|m| m.as_deref() == Some("fake/b")),
        "{models:?}"
    );
    let mem = endless.memory(id).await.unwrap();
    assert_eq!(mem.model, "fake/b");
    assert!(mem.stalled.is_none());
}

/// E1: the title model and the session's model are live links of the chain.
#[tokio::test]
async fn title_and_session_model_links_are_live() {
    let probe = Probe::new("any", Act::Echo);
    let factory: super::pump::CompactorFactory = {
        let p = probe.clone();
        Arc::new(move |_: &str| -> Arc<dyn Compactor> { p.clone() })
    };
    let (_dir, endless, store, id) = harness_factory(cfg_137f(None), factory).await;
    endless_session(&endless, &store, id).await;
    let key = |e: &Endless| e.inner.sessions.lock()[&id].state.clone();
    let state = key(&endless);
    assert_eq!(super::pump::key_for_tests(&state), "fake/session");
    endless.set_title_model(Some("fake/title".into()));
    assert_eq!(super::pump::key_for_tests(&state), "fake/title");
    endless.set_session_model(
        id,
        Some(super::pump::resolved_for_tests("fake/other")),
        None,
    );
    assert_eq!(
        super::pump::key_for_tests(&state),
        "fake/title",
        "the title model outranks the session's"
    );
    endless.set_title_model(None);
    assert_eq!(super::pump::key_for_tests(&state), "fake/other");
}

/// E3: a call hanging on the old model is dropped at the switch, the node
/// is built by the new one, the cancel is no failure and its ledger row settles.
#[tokio::test]
async fn switch_cancels_the_hung_call_and_rebuilds_on_the_new_model() {
    let (a, b) = (
        Probe::new("fake/a", Act::Hang),
        Probe::new("fake/b", Act::Echo),
    );
    let (_dir, endless, store, id) =
        harness_factory(cfg_137f(Some("fake/a")), two(a.clone(), b.clone())).await;
    endless_session(&endless, &store, id).await;
    until("the hung call", || a.calls() == 1).await;
    endless.set_config(cfg_137f(Some("fake/b")));
    until("the drop", || a.dropped.load(Ordering::SeqCst) == 1).await;
    assert!(endless.settle(id, Duration::from_secs(10)).await);
    assert!(
        built_by(&store, id)
            .await
            .iter()
            .all(|m| m.as_deref() == Some("fake/b"))
    );
    assert!(super::pump::failed_for_tests(&endless.inner, id).is_empty());
    // Only the old model's rows: a trailing merge on `fake/b` may still be in flight.
    let rows: Vec<String> = sqlx::query_scalar(
        "SELECT status FROM model_calls WHERE owner_id = ? AND requested_model = 'fake/a'",
    )
    .bind(id.to_string())
    .fetch_all(store.pool())
    .await
    .unwrap();
    assert_eq!(rows, ["cancelled"]);
}

/// E4: two failures of the frontier ⇒ `stalled` (blocking, next retry
/// time), `settle` returns at once; a success clears it.
#[tokio::test]
async fn repeated_frontier_failure_is_stalled_and_settle_stops_waiting() {
    let p = Probe::new("fake/a", Act::Fail);
    let (_dir, endless, store, id) = harness_for(cfg_137f(None), p.clone()).await;
    endless_session(&endless, &store, id).await;
    until("a failure", || {
        !super::pump::failed_for_tests(&endless.inner, id).is_empty()
    })
    .await;
    let (key, ..) = super::pump::failed_for_tests(&endless.inner, id)[0];
    let one = endless.memory(id).await.unwrap().stalled.unwrap();
    assert_eq!((one.failures, one.blocking), (1, true));
    assert_eq!(one.error, "400: model not loaded");
    assert_eq!(one.model, "fake/a");
    let t0 = Instant::now();
    assert!(
        !endless.settle(id, Duration::from_millis(300)).await,
        "one failure: settle still waits its time"
    );
    assert!(t0.elapsed() >= Duration::from_millis(300));

    super::pump::finish_for_tests(&endless.inner, id, key, Err("400: again".into())).await;
    let s = endless.memory(id).await.unwrap().stalled.unwrap();
    assert_eq!((s.failures, s.blocking), (2, true), "{s:?}");
    assert_eq!(s.error, "400: again");
    let at = chrono::DateTime::parse_from_rfc3339(s.next_retry_at.as_deref().unwrap()).unwrap();
    assert!(
        at > chrono::Utc::now() + chrono::Duration::seconds(1000),
        "{at}"
    );
    let t0 = Instant::now();
    assert!(!endless.settle(id, Duration::from_secs(45)).await);
    assert!(t0.elapsed() < Duration::from_secs(5), "{:?}", t0.elapsed());

    p.set(Act::Echo);
    // 137G G6: the two long units failed together in one batch; both land.
    for (key, ..) in super::pump::failed_for_tests(&endless.inner, id) {
        super::pump::finish_for_tests(
            &endless.inner,
            id,
            key,
            Ok(super::tree::Node {
                l: key.0,
                i: key.1,
                text: "built".into(),
                size: 5,
                free: false,
                model: Some("fake/a".into()),
                tries: 1,
                mechanical: false,
            }),
        )
        .await;
    }
    assert!(endless.memory(id).await.unwrap().stalled.is_none());
    assert!(endless.settle(id, Duration::from_secs(10)).await);
}

/// E5 / R1-10: `retry` reschedules a backed-off node at once, restarts a
/// hung call (the cancel is no failure) and never runs a node twice at once.
#[tokio::test]
async fn retry_reschedules_now_and_restarts_a_hung_call() {
    let p = Probe::new("fake/a", Act::Fail);
    let (_dir, endless, store, id) = harness_for(cfg_137f(None), p.clone()).await;
    endless_session(&endless, &store, id).await;
    until("a failure", || {
        !super::pump::failed_for_tests(&endless.inner, id).is_empty()
    })
    .await;
    p.set(Act::Hang);
    let before = p.calls();
    let mem = endless.retry(id).await.unwrap();
    assert!(mem.stalled.is_none(), "{:?}", mem.stalled);
    // The failure backs off 600 s: this call is the retry's.
    until("the retried call", || p.calls() > before).await;
    for k in 1..=3u32 {
        let calls = p.calls();
        endless.retry(id).await.unwrap();
        until("the hung call restarted", || {
            p.dropped.load(Ordering::SeqCst) == k && p.calls() > calls
        })
        .await;
    }
    assert_eq!(
        p.max_inflight.load(Ordering::SeqCst),
        1,
        "never twice at once"
    );
    assert!(
        super::pump::failed_for_tests(&endless.inner, id).is_empty(),
        "a cancelled call is no failure"
    );
    // An epochs session: untouched.
    let (_d, e2, s2, plain) = harness_for(cfg_137f(None), p.clone()).await;
    e2.attach(plain, attached()).await.unwrap();
    assert_eq!(e2.retry(plain).await.unwrap().mode, MemoryMode::Epochs);
    drop(s2);
}

/// E9: three sessions on one local endpoint run one call at a time; cloud
/// compactors keep their per-session `jobs` in parallel.
#[tokio::test]
async fn local_endpoint_is_one_queue_engine_wide() {
    // Local: never more than one call (any delay); cloud: 2 s calls overlap.
    for (local, delay) in [
        (Some("http://127.0.0.1:11434"), Duration::from_millis(100)),
        (None, Duration::from_secs(2)),
    ] {
        let p = Probe::with("fake/a", Act::Echo, local, delay);
        let (_dir, endless, store, id) = harness_for(cfg_137f(None), p.clone()).await;
        let mut ids = vec![id];
        for _ in 0..2 {
            let more = SessionId::new();
            store
                .create_session(more, "fake/session", None, None, None)
                .await
                .unwrap();
            ids.push(more);
        }
        for id in &ids {
            endless_session(&endless, &store, *id).await;
        }
        if local.is_some() {
            for id in &ids {
                assert!(endless.settle(*id, Duration::from_secs(30)).await);
            }
            assert_eq!(p.max_inflight.load(Ordering::SeqCst), 1, "local");
        } else {
            until("cloud sessions in parallel", || {
                p.max_inflight.load(Ordering::SeqCst) >= 2
            })
            .await;
        }
        for id in &ids {
            endless.detach(*id);
        }
    }
}

/// E10 / R1-4: a compactor that panics is a failure whose ledger row
/// settles; the node retries.
#[tokio::test]
async fn panicking_job_releases_its_node_and_retries() {
    let p = Probe::new("fake/a", Act::Panic);
    let (_dir, endless, store, id) = harness_for(cfg_137f(None), p.clone()).await;
    endless_session(&endless, &store, id).await;
    until("the panic's failure", || {
        !super::pump::failed_for_tests(&endless.inner, id).is_empty()
    })
    .await;
    let s = endless.memory(id).await.unwrap().stalled.unwrap();
    assert_eq!(s.error, "compactor panicked");
    assert_eq!(s.failures, 1);
    let rows: Vec<String> = sqlx::query_scalar("SELECT status FROM model_calls WHERE owner_id = ?")
        .bind(id.to_string())
        .fetch_all(store.pool())
        .await
        .unwrap();
    assert_eq!(rows, ["failed"], "settled, never left dispatched");
    // Backoff 600 s: the retry is the RPC's.
    endless.retry(id).await.unwrap();
    assert!(
        endless.settle(id, Duration::from_secs(10)).await,
        "retried and built"
    );
    assert!(endless.memory(id).await.unwrap().stalled.is_none());
}

/// R1-17: what the compactor is sent is the fitted view — a local
/// compactor never sees more than `local_ctx_bytes` of `<chat>`.
#[tokio::test]
async fn local_compactor_receives_the_capped_view() {
    let p = Probe::with("fake/a", Act::Echo, Some("ep"), Duration::ZERO);
    let (_dir, endless, store, id) = harness_for(
        EndlessConfig {
            local_ctx_bytes: 4_000,
            ..cfg_137f(None)
        },
        p.clone(),
    )
    .await;
    store
        .set_session_memory(id, MemoryMode::Endless)
        .await
        .unwrap();
    for k in 0..30 {
        let text = format!(
            "note {k}: {}",
            "rotate the nginx worker and check error_log ".repeat(15)
        );
        store.append(id, &ChatMessage::user(text)).await.unwrap();
    }
    endless.attach(id, attached()).await.unwrap();
    assert!(endless.settle(id, Duration::from_secs(20)).await);
    let full = endless.render_for_turn(id).await.unwrap().text().len();
    assert!(full > 8_000, "the view outgrew the cap: {full}");
    let sent = p.ctx_lens.lock().clone();
    assert!(sent.iter().all(|n| *n <= 4_000), "{sent:?}");
    assert!(
        sent.iter().any(|n| *n > 3_000),
        "trimmed to the cap, not below: {sent:?}"
    );
}

/// R2-1: a local session's merge queued at a held gate does not keep its
/// next frontier job from being spawned; that job waits too and is served
/// first on release.
#[tokio::test]
async fn queued_merge_does_not_hold_back_the_frontier() {
    let p = Probe::with("fake/a", Act::Echo, Some("ep"), Duration::ZERO);
    let (_dir, endless, store, id) = harness_for(cfg_137f(None), p.clone()).await;
    let gate = super::pump::gate_for_tests(&endless.inner, "ep");
    let held = gate.acquire(1).await;
    store
        .set_session_memory(id, MemoryMode::Endless)
        .await
        .unwrap();
    // Two free level-0 lines (≤ 512 B each) whose merge needs the model.
    for tag in ["alpha", "beta"] {
        let text = format!(
            "{tag}: {}",
            "keep the nginx worker_processes line ".repeat(7)
        );
        store.append(id, &ChatMessage::user(text)).await.unwrap();
    }
    endless.attach(id, attached()).await.unwrap();
    until("the merge queued at the gate", || {
        super::pump::running_for_tests(&endless.inner, id) == [(1, 0)]
    })
    .await;
    let unit = format!(
        "UNIT2 {}",
        "rotate the error_log and reload nginx now ".repeat(20)
    );
    store.append(id, &ChatMessage::user(unit)).await.unwrap();
    endless.on_rows_committed(id);
    until("the frontier spawned beside the merge", || {
        super::pump::running_for_tests(&endless.inner, id) == [(0, 2), (1, 0)]
    })
    .await;
    assert_eq!(p.calls(), 0, "both wait at the held gate");
    drop(held);
    until("both served", || p.calls() >= 2).await;
    let inputs = p.inputs.lock().clone();
    assert!(
        inputs[0].contains("UNIT2"),
        "the frontier first: {inputs:?}"
    );
    assert!(endless.settle(id, Duration::from_secs(10)).await);
}

/// 137G G7 `endless_fold`: every unbuilt level-0 unit becomes a node at once
/// with no model call — long units a flagged excerpt ≤ `node_bytes` (head
/// and tail kept), short ones their free line; the hung level-0 call is
/// cancelled (no failure); the view settles; recall still finds the rows;
/// an epochs session is unchanged.
#[tokio::test]
async fn fold_turns_the_backlog_into_mechanical_nodes_at_once() {
    let p = Probe::new("fake/a", Act::Hang);
    let (_dir, endless, store, id) = harness_for(cfg_137f(None), p.clone()).await;
    endless_session(&endless, &store, id).await;
    until("a call in flight", || p.calls() >= 1).await;
    let before = endless.memory(id).await.unwrap();
    let backlog = before.backlog.expect("a backlog");
    // The two long units wait in the hung batch; the short ones are free lines.
    assert_eq!((backlog.lines, backlog.paused), (2, false), "{backlog:?}");

    let mem = endless.fold(id).await.unwrap();
    assert!(mem.backlog.is_none(), "{:?}", mem.backlog);
    assert!(!mem.settling, "every part is a built line");
    let mech: Vec<_> = mem.view.iter().filter(|l| l.mechanical).collect();
    assert_eq!(mech.len(), 2, "{:?}", mem.view);
    let node_bytes = EndlessConfig::default().node_bytes as usize;
    for l in &mech {
        assert!(l.built && l.text.len() <= node_bytes, "{}", l.text.len());
        assert!(
            l.text.contains(" … ") && l.text.contains("line 59"),
            "{}",
            l.text
        );
    }
    assert!(
        mem.view
            .iter()
            .filter(|l| !l.mechanical)
            .all(|l| l.built && !l.text.contains(" … ")),
        "short units stay verbatim free lines"
    );
    until("the hung call cancelled", || {
        p.dropped.load(Ordering::SeqCst) == 1
    })
    .await;
    assert!(super::pump::failed_for_tests(&endless.inner, id).is_empty());
    assert!(endless.settle(id, Duration::from_secs(10)).await);
    let stored = store.endless_nodes(id).await.unwrap();
    assert_eq!(
        stored
            .iter()
            .filter(|n| n.mechanical && n.tries == 0)
            .count(),
        2
    );
    assert!(
        stored.iter().all(|n| n.model.is_none()),
        "no model built anything"
    );
    let hits = endless.recall(id, "worker_processes", 8).await.unwrap();
    assert!(!hits.is_empty(), "the rows stay searchable");

    let (_d, e2, s2, plain) = harness_for(cfg_137f(None), p.clone()).await;
    e2.attach(plain, attached()).await.unwrap();
    let m = e2.fold(plain).await.unwrap();
    assert_eq!(m.mode, MemoryMode::Epochs);
    assert!(m.backlog.is_none() && s2.endless_nodes(plain).await.unwrap().is_empty());
}

/// 137G G8 `endless_pause`: pausing cancels the call in flight (no failure)
/// and starts none, keeps the backlog and makes `settle` return at once; the
/// flag survives a re-attach from the store; resuming calls again.
#[tokio::test]
async fn pause_stops_calls_survives_reattach_and_resume_restarts() {
    let p = Probe::new("fake/a", Act::Hang);
    let (_dir, endless, store, id) = harness_for(cfg_137f(None), p.clone()).await;
    endless_session(&endless, &store, id).await;
    until("a call in flight", || p.calls() >= 1).await;

    let mem = endless.pause(id, true).await.unwrap();
    let b = mem.backlog.expect("paused shows a backlog");
    assert_eq!((b.lines, b.paused, b.eta_secs), (2, true, 0), "{b:?}");
    assert_eq!(b.paused_by.as_deref(), Some("user"));
    until("the hung call cancelled", || {
        p.dropped.load(Ordering::SeqCst) == 1
    })
    .await;
    let calls = p.calls();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(p.calls(), calls, "no call while paused");
    assert!(super::pump::failed_for_tests(&endless.inner, id).is_empty());
    let t0 = Instant::now();
    assert!(!endless.settle(id, Duration::from_secs(30)).await);
    assert!(
        t0.elapsed() < Duration::from_secs(5),
        "settle does not wait"
    );
    assert_eq!(
        store.endless_meta(id).await.unwrap().paused_by.as_deref(),
        Some("user")
    );

    // A restart: the state reloads from the store, still paused.
    endless.detach(id);
    endless.attach(id, attached()).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(p.calls(), calls, "no call after a restart either");
    let b = endless.memory(id).await.unwrap().backlog.unwrap();
    assert_eq!(
        b.paused_by.as_deref(),
        Some("user"),
        "still paused after a restart"
    );

    let mem = endless.pause(id, false).await.unwrap();
    assert!(!mem.backlog.unwrap().paused);
    until("calls resume", || p.calls() > calls).await;
    let meta = store.endless_meta(id).await.unwrap();
    assert_eq!((meta.paused_by, meta.spend_override), (None, None));

    // G11: resuming a spend pause is recorded as the user's override for today.
    store
        .set_endless_paused(id, Some("spend"), None)
        .await
        .unwrap();
    endless.detach(id);
    endless.attach(id, attached()).await.unwrap();
    let b = endless.memory(id).await.unwrap().backlog.unwrap();
    assert_eq!(b.paused_by.as_deref(), Some("spend"));
    endless.pause(id, false).await.unwrap();
    let meta = store.endless_meta(id).await.unwrap();
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    assert_eq!((meta.paused_by, meta.spend_override), (None, Some(today)));
}

/// 137G chain: chat override → agent default → `agent.endless.model` →
/// title model → session model; each pick persists, is live (the compactor
/// switches without a re-attach) and reports its source.
#[tokio::test]
async fn compactor_chain_chat_agent_settings_title_session() {
    // One echoing compactor per resolved id, so `memory.model` names the link's pick.
    let factory: super::pump::CompactorFactory =
        Arc::new(|id: &str| -> Arc<dyn Compactor> { Probe::new(id, Act::Echo) });
    let (_dir, endless, store, id) = harness_factory(cfg_137f(None), factory).await;
    endless_session(&endless, &store, id).await;
    let source = |m: &super::SessionMemory| (m.model.clone(), m.model_source.clone());
    let m = endless.memory(id).await.unwrap();
    assert_eq!(source(&m), ("fake/session".into(), "session".into()));
    endless.set_title_model(Some("fake/title".into()));
    let m = endless.memory(id).await.unwrap();
    assert_eq!(source(&m), ("fake/title".into(), "title".into()));
    endless.set_config(cfg_137f(Some("fake/settings")));
    let m = endless.memory(id).await.unwrap();
    assert_eq!(source(&m), ("fake/settings".into(), "settings".into()));
    let m = endless
        .set_agent_model(id, Some("fake/desk".into()))
        .await
        .unwrap();
    assert_eq!(source(&m), ("fake/desk".into(), "agent".into()));
    let m = endless
        .set_model(id, Some(" fake/b ".into()))
        .await
        .unwrap();
    assert_eq!(source(&m), ("fake/b".into(), "chat".into()));
    assert_eq!(m.model_override.as_deref(), Some("fake/b"));
    let meta = store.endless_meta(id).await.unwrap();
    assert_eq!(
        (meta.model_override.as_deref(), meta.agent_model.as_deref()),
        (Some("fake/b"), Some("fake/desk"))
    );
    // A restart reloads both picks.
    endless.detach(id);
    endless.attach(id, attached()).await.unwrap();
    assert_eq!(source(&endless.memory(id).await.unwrap()).1, "chat");
    let m = endless.set_model(id, None).await.unwrap();
    assert_eq!(source(&m), ("fake/desk".into(), "agent".into()));
    assert!(m.model_override.is_none());
    assert_eq!(m.spend_today_usd, 0.0);
}

/// 137G: a compactor that answers batches line by line (`k| …`, a unit
/// whose text holds `BAD` gets no line), single units and merges by echo;
/// optionally refuses its first call as too long, or prices its calls.
struct Script {
    model: models::Resolved,
    delay: Duration,
    /// Refuse the first call with Anthropic's "prompt is too long".
    overflow_first: std::sync::atomic::AtomicBool,
    /// `(kind, ctx bytes, step bytes)` of every call: `batch` | `unit` | `merge`.
    calls: Mutex<Vec<(&'static str, usize, usize)>>,
    inflight: AtomicU32,
    max_inflight: AtomicU32,
    /// `prompt_tokens` reported per call (the G11 spend).
    prompt_tokens: u32,
}

impl Script {
    fn new(delay: Duration) -> Arc<Self> {
        Self::priced(delay, None, 1_000)
    }

    /// With catalog rates (USD per million input tokens) and the prompt size each call reports.
    fn priced(delay: Duration, input_usd: Option<f64>, prompt_tokens: u32) -> Arc<Self> {
        let model = input_usd.map(|p| {
            serde_json::from_value(serde_json::json!({
                "id": "s", "name": "s", "limit": {"context": 200_000, "output": 4096},
                "cost": {"input": p, "output": p}
            }))
            .unwrap()
        });
        Arc::new(Self {
            model: models::Resolved {
                model,
                ..super::pump::resolved_for_tests("fake/script")
            },
            delay,
            overflow_first: std::sync::atomic::AtomicBool::new(false),
            calls: Mutex::new(Vec::new()),
            inflight: AtomicU32::new(0),
            max_inflight: AtomicU32::new(0),
            prompt_tokens,
        })
    }

    fn count(&self, kind: &str) -> usize {
        self.calls.lock().iter().filter(|c| c.0 == kind).count()
    }
}

/// The step text of a call (its first user block).
fn step_text(convo: &[ChatMessage]) -> String {
    convo
        .first()
        .and_then(|m| m.content.iter().next())
        .and_then(|p| match p {
            ContentPart::Text(t) => Some(t.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

impl Compactor for Script {
    fn compress<'a>(
        &'a self,
        ctx: &'a str,
        convo: &'a [ChatMessage],
    ) -> BoxFuture<'a, Result<Reply, String>> {
        let step = step_text(convo);
        let (kind, text) = if step.contains("Compress each of these") {
            let lines: Vec<String> = step
                .split("\n\n[")
                .skip(1)
                .filter_map(|seg| {
                    let (k, rest) = seg.split_once("] ")?;
                    let body = rest.split_once(": ").map_or(rest, |(_, b)| b);
                    (!body.contains("BAD")).then(|| {
                        format!(
                            "{k}| {}",
                            super::guards::cut_utf8(body, 300).replace('\n', " ")
                        )
                    })
                })
                .collect();
            ("batch", lines.join("\n"))
        } else if step.contains("Merge these two lines") {
            ("merge", echo_line(convo))
        } else {
            ("unit", echo_line(convo))
        };
        self.calls.lock().push((kind, ctx.len(), step.len()));
        let overflow = self
            .overflow_first
            .swap(false, std::sync::atomic::Ordering::SeqCst);
        let usage = serde_json::from_value(serde_json::json!({
            "prompt_tokens": self.prompt_tokens, "completion_tokens": 50
        }))
        .ok();
        Box::pin(async move {
            let now = self.inflight.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_inflight.fetch_max(now, Ordering::SeqCst);
            tokio::time::sleep(self.delay).await;
            self.inflight.fetch_sub(1, Ordering::SeqCst);
            if overflow {
                return Err(
                    "400 Bad Request: prompt is too long: 1204913 tokens > 1000000 maximum".into(),
                );
            }
            Ok(Reply {
                text,
                usage,
                latency: self.delay,
            })
        })
    }

    fn model(&self) -> &models::Resolved {
        &self.model
    }
}

/// [`until`] for an async condition.
async fn eventually<F, Fut>(what: &str, mut f: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    for _ in 0..2_000 {
        if f().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("timed out waiting for {what}");
}

/// `n` user rows of ~600 bytes (non-free units), the ones in `bad` marked `BAD`.
async fn seed_many(store: &Store, id: SessionId, n: usize, bad: &[usize]) {
    for k in 0..n {
        let mark = if bad.contains(&k) { "BAD " } else { "" };
        let text = format!(
            "{mark}message {k}: deploy step {k} touched /srv/app/config_{k}.yaml and {}",
            "the rollout notes repeat here ".repeat(19)
        );
        store.append(id, &ChatMessage::user(text)).await.unwrap();
    }
}

/// Level-0 nodes in the store.
async fn level0(store: &Store, id: SessionId) -> Vec<super::tree::Node> {
    store
        .endless_nodes(id)
        .await
        .unwrap()
        .into_iter()
        .filter(|n| n.l == 0)
        .collect()
}

/// 137G G6 + G2: 200 unbuilt units settle in batches of 16 — at most
/// ceil(200/16) batch calls plus one solo call for the unit whose batch
/// line failed; batches run in parallel on a cloud compactor; while paused
/// the backlog shows how many lines the view folds.
#[tokio::test]
async fn batches_settle_200_units_in_parallel_and_retry_a_bad_line_alone() {
    let s = Script::new(Duration::from_millis(40));
    let (_dir, endless, store, id) = harness_for(
        EndlessConfig {
            view_bytes: 16_000,
            raw_bytes: 4_000,
            retry_secs: 600,
            ..EndlessConfig::default()
        },
        s.clone(),
    )
    .await;
    store
        .set_session_memory(id, MemoryMode::Endless)
        .await
        .unwrap();
    seed_many(&store, id, 200, &[37]).await;
    store
        .set_endless_paused(id, Some("user"), None)
        .await
        .unwrap();
    endless.attach(id, attached()).await.unwrap();
    let m = endless.memory(id).await.unwrap();
    let b = m.backlog.expect("a backlog");
    let shown = m.view.iter().filter(|l| !l.built && l.n == 1).count() as u32;
    assert_eq!((b.lines, b.paused, b.eta_secs), (200, true, 0), "{b:?}");
    assert!(shown > 0 && b.folded == 200 - shown, "{b:?} shown {shown}");
    let folded_line = m
        .view
        .iter()
        .find(|l| l.n > 1 && !l.built)
        .expect("one folded line");
    assert!(
        folded_line
            .text
            .contains(&format!("({} lines not yet summarized", b.folded))
    );

    endless.pause(id, false).await.unwrap();
    eventually("200 level-0 nodes", || async {
        level0(&store, id).await.len() == 200
    })
    .await;
    let batches = s.count("batch");
    assert!(batches <= 200usize.div_ceil(16), "{batches} batch calls");
    assert_eq!(s.count("unit"), 1, "only the bad line ran alone");
    assert!(
        s.max_inflight.load(Ordering::SeqCst) > 1,
        "cloud batches run in parallel"
    );
    let nodes = level0(&store, id).await;
    assert!(
        nodes
            .iter()
            .all(|n| n.model.as_deref() == Some("fake/script"))
    );
    assert!(
        nodes
            .iter()
            .find(|n| n.i == 37)
            .unwrap()
            .text
            .contains("BAD")
    );
    assert!(endless.memory(id).await.unwrap().backlog.is_none());
}

/// 137G G7: a unit that fails three times in a row folds mechanically and
/// the frontier moves on; nothing stays failed.
#[tokio::test]
async fn three_failures_fold_the_unit_mechanically() {
    let p = Probe::new("fake/a", Act::Fail);
    let (_dir, endless, store, id) = harness_for(
        EndlessConfig {
            view_bytes: 16_000,
            retry_secs: 0,
            ..EndlessConfig::default()
        },
        p.clone(),
    )
    .await;
    endless_session(&endless, &store, id).await;
    eventually("two mechanical units", || async {
        level0(&store, id)
            .await
            .iter()
            .filter(|n| n.mechanical)
            .count()
            == 2
    })
    .await;
    assert!(endless.settle(id, Duration::from_secs(10)).await);
    assert!(
        super::pump::failed_for_tests(&endless.inner, id)
            .iter()
            .all(|(k, ..)| k.0 > 0),
        "no level-0 failure left"
    );
    assert!(p.calls() >= 3);
}

/// 137G G5: a compactor call refused as too long ("1204913 tokens > 1000000
/// maximum") is retried once with a smaller context — never the same size.
#[tokio::test]
async fn context_length_refusal_retries_once_smaller() {
    let s = Script::new(Duration::ZERO);
    s.overflow_first
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let (_dir, endless, store, id) = harness_for(cfg_137f(None), s.clone()).await;
    endless_session(&endless, &store, id).await;
    eventually("every unit built", || async {
        level0(&store, id).await.len() == 6
    })
    .await;
    let calls = s.calls.lock().clone();
    let (first, second) = (calls[0].1, calls[1].1);
    assert_eq!(calls[0].0, calls[1].0, "the same step again");
    assert!(second < first, "smaller, never equal: {first} → {second}");
    assert!(
        second <= (first as f64 * 1_000_000.0 / 1_204_913.0 * 0.85) as usize,
        "{first} → {second}"
    );
}

/// 137G G4: a 100 KB user unit is summarised map-reduce — chunks of ≤ 16 KB,
/// then one reduce call — and lands as one node.
#[tokio::test]
async fn long_unit_is_map_reduced_into_one_node() {
    let s = Script::new(Duration::ZERO);
    let (_dir, endless, store, id) = harness_for(
        EndlessConfig {
            view_bytes: 16_000,
            cap_chars: 120_000,
            ..EndlessConfig::default()
        },
        s.clone(),
    )
    .await;
    store
        .set_session_memory(id, MemoryMode::Endless)
        .await
        .unwrap();
    let long: String = (0..10_000).map(|k| format!("w{k:04}_x ")).collect();
    assert!(long.len() >= 80_000);
    store
        .append(id, &ChatMessage::user(long.clone()))
        .await
        .unwrap();
    endless.attach(id, attached()).await.unwrap();
    assert!(endless.settle(id, Duration::from_secs(10)).await);
    let chunks = super::compactor::chunks(&long, super::compactor::CHUNK_BYTES).len();
    assert!(chunks >= 5, "{chunks}");
    let calls = s.calls.lock().clone();
    assert_eq!(calls.len(), chunks + 1, "map calls + one reduce");
    assert!(
        calls
            .iter()
            .all(|c| c.2 <= super::compactor::CHUNK_BYTES + 2_000),
        "every step under the chunk budget: {calls:?}"
    );
    let nodes = level0(&store, id).await;
    assert_eq!(nodes.len(), 1);
    assert!(!nodes[0].free && !nodes[0].mechanical && nodes[0].text.len() <= 512);
}

/// 137G G11: a call whose prompt alone costs over `max_usd_per_call` is
/// never sent (the unit folds mechanically); reaching the day cap pauses
/// settling (`paused_by: "spend"`); resuming overrides it for the day.
#[tokio::test]
async fn spend_guard_folds_over_cost_units_and_pauses_at_the_day_cap() {
    // $100/M input: the ~2.5k-token unit prompt is ≈ $0.25 > $0.10.
    let s = Script::priced(Duration::ZERO, Some(100.0), 10_000);
    let (_dir, endless, store, id) = harness_for(cfg_137f(None), s.clone()).await;
    endless_session(&endless, &store, id).await;
    eventually("over-cost units folded", || async {
        level0(&store, id)
            .await
            .iter()
            .filter(|n| n.mechanical)
            .count()
            == 2
    })
    .await;
    assert_eq!(s.count("batch") + s.count("unit"), 0, "never sent");

    // Each call now reports 10k prompt tokens = $1; the day cap is $1.5.
    let s = Script::priced(Duration::ZERO, Some(100.0), 10_000);
    let (_dir2, endless, store, id) = harness_for(
        EndlessConfig {
            max_usd_per_call: 10.0,
            max_usd_per_session_day: 1.5,
            batch_units: 1,
            jobs: 1,
            ..cfg_137f(None)
        },
        s.clone(),
    )
    .await;
    store
        .set_session_memory(id, MemoryMode::Endless)
        .await
        .unwrap();
    seed_many(&store, id, 6, &[]).await;
    endless.attach(id, attached()).await.unwrap();
    eventually("the spend pause", || async {
        endless
            .memory(id)
            .await
            .unwrap()
            .backlog
            .is_some_and(|b| b.paused_by.as_deref() == Some("spend"))
    })
    .await;
    let m = endless.memory(id).await.unwrap();
    assert!(m.spend_today_usd >= 1.5, "{}", m.spend_today_usd);
    let calls = s.calls.lock().len();
    assert_eq!(calls, 2, "two $1 calls reach the $1.5 cap");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(s.calls.lock().len(), calls, "no call while spend-paused");
    endless.pause(id, false).await.unwrap();
    until("calls resume past the cap (user override)", || {
        s.calls.lock().len() > calls
    })
    .await;
    assert!(
        store
            .endless_meta(id)
            .await
            .unwrap()
            .spend_override
            .is_some()
    );
}

/// 137G G9: `/endless off` with 5,000 unbuilt lines writes a bounded head
/// (the G2 raw budget), never the whole raw view.
#[tokio::test]
async fn endless_off_with_5000_unbuilt_writes_a_bounded_head() {
    let s = Script::new(Duration::ZERO);
    let (_dir, endless, store, id) = harness_for(cfg_137f(None), s.clone()).await;
    store
        .set_session_memory(id, MemoryMode::Endless)
        .await
        .unwrap();
    seed_many(&store, id, 5_000, &[]).await;
    store
        .set_endless_paused(id, Some("user"), None)
        .await
        .unwrap();
    endless.attach(id, attached()).await.unwrap();
    let off = endless
        .set_mode(id, MemoryMode::Epochs, 1, UnitFilter::All)
        .await
        .unwrap();
    let head = off.head.expect("a head");
    let cfg = cfg_137f(None).fold_cfg();
    assert!(
        head.len() <= (cfg.high() + cfg.raw_bytes as usize) + 4_096,
        "{} bytes",
        head.len()
    );
    assert!(
        head.contains("lines not yet summarized"),
        "{}",
        &head[..300]
    );
    assert_eq!(s.calls.lock().len(), 0);
}
