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
                ..Default::default()
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
    assert!(
        endless.quiet_for_tests(id, Duration::from_secs(10)).await,
        "settled"
    );

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
    // 201 P3: the two long units refine in one batch; merges are lazy.
    assert!(
        *echo.calls.lock() >= 1,
        "the two long units needed the model"
    );

    let mem = endless.memory(id).await.unwrap();
    assert_eq!(mem.mode, MemoryMode::Endless);
    assert_eq!(mem.units, 6);
    assert!(mem.view.iter().all(|l| l.built));
    assert_eq!(mem.model, "fake/compactor");
    assert!(
        mem.stats.calls >= 1
            && mem.stats.input_tokens >= 1000
            && mem.stats.cache_read_tokens >= 900
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
    assert!(endless.quiet_for_tests(id, Duration::from_secs(10)).await);
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
    assert!(endless.quiet_for_tests(id, Duration::from_secs(5)).await);
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
    assert!(endless.quiet_for_tests(id, Duration::from_secs(10)).await);
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
    assert!(endless.quiet_for_tests(id, Duration::from_secs(10)).await);
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
                    ..Default::default()
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
        // 137F behaviour under test: a local compactor keeps `standard`.
        profile: super::Profile::Standard,
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
        // 201 P3: a draft merge waits for `merge_at` (lazy); not a model's line.
        .filter(|n| !n.free && !(n.l > 0 && n.mechanical))
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
    assert!(endless.quiet_for_tests(id, Duration::from_secs(10)).await);
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
    assert!(endless.quiet_for_tests(id, Duration::from_secs(10)).await);
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
    // 201 P1: drafts leave no frontier for a failure to block.
    assert_eq!((one.failures, one.blocking), (1, false));
    assert_eq!(one.error, "400: model not loaded");
    assert_eq!(one.model, "fake/a");
    let t0 = Instant::now();
    assert!(
        !endless
            .quiet_for_tests(id, Duration::from_millis(300))
            .await,
        "one failure: settle still waits its time"
    );
    assert!(t0.elapsed() >= Duration::from_millis(300));

    super::pump::finish_for_tests(&endless.inner, id, key, Err("400: again".into())).await;
    let s = endless.memory(id).await.unwrap().stalled.unwrap();
    assert_eq!((s.failures, s.blocking), (2, false), "{s:?}");
    assert_eq!(s.error, "400: again");
    let at = chrono::DateTime::parse_from_rfc3339(s.next_retry_at.as_deref().unwrap()).unwrap();
    assert!(
        at > chrono::Utc::now() + chrono::Duration::seconds(1000),
        "{at}"
    );
    let t0 = Instant::now();
    assert!(!endless.quiet_for_tests(id, Duration::from_secs(45)).await);
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
    assert!(endless.quiet_for_tests(id, Duration::from_secs(10)).await);
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
                assert!(endless.quiet_for_tests(*id, Duration::from_secs(30)).await);
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
        endless.quiet_for_tests(id, Duration::from_secs(10)).await,
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
        // 201 P3: talk units (a long user message needs no model, D8).
        store
            .append(id, &ChatMessage::assistant(text))
            .await
            .unwrap();
    }
    endless.attach(id, attached()).await.unwrap();
    assert!(endless.quiet_for_tests(id, Duration::from_secs(20)).await);
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
    let cfg = EndlessConfig {
        merge_at: 0.5,
        ..cfg_137f(None)
    };
    let (_dir, endless, store, id) = harness_for(cfg, p.clone()).await;
    let gate = super::pump::gate_for_tests(&endless.inner, "ep");
    let held = gate.acquire(1, runtime::SessionId(uuid::Uuid::nil())).await;
    store
        .set_session_memory(id, MemoryMode::Endless)
        .await
        .unwrap();
    // 24 free level-0 lines (≤ 512 B each) whose merges need the model; 201
    // P3: together over `merge_at` × `view_bytes`, so merges are due.
    for k in 0..24 {
        let text = format!(
            "tag{k}: {}",
            "keep the nginx worker_processes line ".repeat(11)
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
    // A talk unit: a long user message needs no model (201 P3, D8).
    store
        .append(id, &ChatMessage::assistant(unit))
        .await
        .unwrap();
    endless.on_rows_committed(id);
    until("the frontier spawned beside the merge", || {
        super::pump::running_for_tests(&endless.inner, id) == [(0, 24), (1, 0)]
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
    assert!(endless.quiet_for_tests(id, Duration::from_secs(10)).await);
}

/// 137G G7 `endless_fold`, 201 P1: every level-0 unit already has its line
/// from ingest — long units a draft excerpt ≤ `node_bytes` (head and tail
/// kept), short ones their free line — so the view is built before any
/// model call; folding adds nothing and leaves the drafts to refinement;
/// recall still finds the rows; an epochs session is unchanged.
#[tokio::test]
async fn fold_turns_the_backlog_into_mechanical_nodes_at_once() {
    let p = Probe::new("fake/a", Act::Hang);
    let (_dir, endless, store, id) = harness_for(cfg_137f(None), p.clone()).await;
    endless_session(&endless, &store, id).await;
    until("a call in flight", || p.calls() >= 1).await;
    let before = endless.memory(id).await.unwrap();
    let backlog = before.backlog.expect("a backlog");
    // The two long drafts wait for refinement; the short ones are free lines.
    assert_eq!((backlog.lines, backlog.paused), (2, false), "{backlog:?}");

    let mem = endless.fold(id).await.unwrap();
    assert!(!mem.settling, "every part is a built line");
    let mech: Vec<_> = mem.view.iter().filter(|l| l.mechanical).collect();
    assert_eq!(mech.len(), 2, "{:?}", mem.view);
    let node_bytes = EndlessConfig::default().node_bytes as usize;
    for l in &mech {
        assert!(l.built && l.text.len() <= node_bytes, "{}", l.text.len());
        assert!(
            // 201 P3: the talk unit's mechanical cut, the echo's per-kind line.
            (l.text.contains(" … ") || l.text.contains("(60 lines, 0 errors)"))
                && l.text.contains("line 59"),
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
    assert!(super::pump::failed_for_tests(&endless.inner, id).is_empty());
    let stored = store.endless_nodes(id).await.unwrap();
    assert_eq!(
        stored
            .iter()
            .filter(|n| n.l == 0 && n.mechanical && n.tries == 0)
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
    assert!(!endless.quiet_for_tests(id, Duration::from_secs(30)).await);
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
        .set_agent_model(id, Some("fake/desk".into()), None)
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
                ..Default::default()
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
        // 201 P3: a talk unit (a long user message is final at ingest, D8).
        store
            .append(id, &ChatMessage::assistant(text))
            .await
            .unwrap();
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
    assert_eq!((b.lines, b.paused, b.eta_secs), (200, true, 0), "{b:?}");
    // 201 P1: every unit has its draft at ingest — no unsummarized run.
    // 201 P3: a paused session still builds its draft merges, so the view
    // folds with them instead of the hard-cap prefix line.
    eventually("the paused view folds with draft merges", || async {
        let m = endless.memory(id).await.unwrap();
        m.view.iter().all(|l| l.built) && m.view.iter().any(|l| l.n > 1)
    })
    .await;
    let m = endless.memory(id).await.unwrap();
    let bytes: usize = m.view.iter().map(|l| l.text.len()).sum();
    assert!(bytes <= 2 * 16_000, "bounded view: {bytes} B");

    endless.pause(id, false).await.unwrap();
    eventually("200 refined level-0 nodes", || async {
        level0(&store, id)
            .await
            .iter()
            .filter(|n| n.model.is_some())
            .count()
            == 200
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

/// Fails a call whose step holds `BAD` (every call: `all`) with `error` and
/// echoes the rest; merges never fail.
struct Picky {
    model: models::Resolved,
    error: &'static str,
    all: bool,
}

impl Picky {
    fn new(error: &'static str, all: bool) -> Arc<Self> {
        Arc::new(Self {
            model: super::pump::resolved_for_tests("fake/picky"),
            error,
            all,
        })
    }
}

impl Compactor for Picky {
    fn compress<'a>(
        &'a self,
        _ctx: &'a str,
        convo: &'a [ChatMessage],
    ) -> BoxFuture<'a, Result<Reply, String>> {
        let step = step_text(convo);
        let fail = !step.contains("Merge these two lines") && (self.all || step.contains("BAD"));
        let line = echo_line(convo);
        Box::pin(async move {
            if fail {
                Err(self.error.to_owned())
            } else {
                Ok(Reply {
                    text: line,
                    usage: None,
                    latency: Duration::ZERO,
                    ..Default::default()
                })
            }
        })
    }

    fn model(&self) -> &models::Resolved {
        &self.model
    }
}

/// One good and one `BAD` unit (`seed_many`), single-unit calls, no backoff.
async fn picky_session(
    c: Arc<Picky>,
    n: usize,
    bad: &[usize],
) -> (crate::TestDir, Endless, Store, SessionId) {
    let (dir, endless, store, id) = harness_for(
        EndlessConfig {
            view_bytes: 16_000,
            retry_secs: 0,
            batch_units: 1,
            ..EndlessConfig::default()
        },
        c,
    )
    .await;
    store
        .set_session_memory(id, MemoryMode::Endless)
        .await
        .unwrap();
    seed_many(&store, id, n, bad).await;
    endless.attach(id, attached()).await.unwrap();
    (dir, endless, store, id)
}

async fn mechanical(store: &Store, id: SessionId) -> Vec<u64> {
    level0(store, id)
        .await
        .iter()
        .filter(|n| n.mechanical)
        .map(|n| n.i)
        .collect()
}

/// 137G G7 + R1-2 + R2-2, 201 P1: a unit that fails keeps its draft from
/// ingest and never blocks another unit; after three hard failures in a row
/// (R2-2 a: an empty reply counts), 10 minutes since the first and another
/// unit built since, it is given up — its draft stays, no model line.
#[tokio::test]
async fn three_hard_failures_over_ten_minutes_fold_the_unit() {
    let (_dir, endless, store, id) =
        picky_session(Picky::new("compactor returned nothing", false), 2, &[1]).await;
    until("three failures", || {
        super::pump::failed_for_tests(&endless.inner, id)
            .iter()
            .any(|(k, _, n)| *k == (0, 1) && *n >= 3)
    })
    .await;
    eventually("the good unit refined", || async {
        level0(&store, id)
            .await
            .iter()
            .any(|n| n.i == 0 && n.model.is_some())
    })
    .await;
    assert_eq!(
        mechanical(&store, id).await,
        [1],
        "the bad unit keeps its draft"
    );
    assert!(
        !endless
            .quiet_for_tests(id, Duration::from_millis(200))
            .await,
        "under 10 minutes: still being refined"
    );
    super::pump::age_failures_for_tests(&endless.inner, id, Duration::from_secs(601));
    assert!(
        endless.quiet_for_tests(id, Duration::from_secs(10)).await,
        "given up: nothing left to refine"
    );
    let bad = level0(&store, id)
        .await
        .into_iter()
        .find(|n| n.i == 1)
        .unwrap();
    assert!(bad.mechanical && bad.model.is_none(), "{bad:?}");
}

/// R2-2 (b), 201 P1: an error that fails every unit (a revoked key, no
/// credit, an unknown model) never gives a unit up however old — every
/// unit keeps its draft and the session stays stalled, visibly, for Retry
/// or a model switch.
#[tokio::test]
async fn an_error_failing_every_unit_stalls_and_folds_nothing() {
    let (_dir, endless, store, id) = picky_session(
        Picky::new("401 Unauthorized: invalid x-api-key", true),
        3,
        &[],
    )
    .await;
    until("three failures each", || {
        let f = super::pump::failed_for_tests(&endless.inner, id);
        f.len() == 3 && f.iter().all(|(.., n)| *n >= 3)
    })
    .await;
    super::pump::age_failures_for_tests(&endless.inner, id, Duration::from_secs(3_600));
    tokio::time::sleep(Duration::from_millis(200)).await;
    let nodes = level0(&store, id).await;
    assert!(
        nodes.len() == 3 && nodes.iter().all(|n| n.mechanical && n.model.is_none()),
        "every unit keeps its draft: {nodes:?}"
    );
    assert!(
        !endless
            .quiet_for_tests(id, Duration::from_millis(200))
            .await,
        "not given up"
    );
    assert!(endless.memory(id).await.unwrap().stalled.is_some());
}

/// R1-2, 201 P1: transient failures (429/529, network) never give a unit
/// up, however many and however old — even beside units that build; the
/// unit keeps its draft and keeps being retried.
#[tokio::test]
async fn transient_failures_never_fold_a_unit() {
    let overloaded = "529 Overloaded: overloaded_error, try again later";
    assert!(matches!(
        crate::compact::classify(overloaded),
        crate::compact::Failure::Transient
    ));
    let (_dir, endless, store, id) = picky_session(Picky::new(overloaded, false), 2, &[1]).await;
    until("many failures", || {
        super::pump::failed_for_tests(&endless.inner, id)
            .iter()
            .any(|(k, _, n)| *k == (0, 1) && *n >= 5)
    })
    .await;
    eventually("the good unit refined", || async {
        level0(&store, id)
            .await
            .iter()
            .any(|n| n.i == 0 && n.model.is_some())
    })
    .await;
    super::pump::age_failures_for_tests(&endless.inner, id, Duration::from_secs(3_600));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(mechanical(&store, id).await, [1], "the draft stays");
    assert!(
        !endless
            .quiet_for_tests(id, Duration::from_millis(200))
            .await,
        "not given up"
    );
}

/// A paused endless session holding the same-size units `message k` for
/// `ks` (three-digit `k`: every unit the same size), attached, none built.
async fn unbuilt_session(
    ks: std::ops::Range<usize>,
) -> (crate::TestDir, Endless, Store, SessionId) {
    let (dir, endless, store, id) = harness_for(cfg_137f(None), Script::new(Duration::ZERO)).await;
    store
        .set_session_memory(id, MemoryMode::Endless)
        .await
        .unwrap();
    store
        .set_endless_paused(id, Some("user"), None)
        .await
        .unwrap();
    for k in ks {
        let text = format!(
            "message {k}: deploy step {k} touched /srv/app/config_{k}.yaml and {}",
            "the rollout notes repeat here ".repeat(19)
        );
        store.append(id, &ChatMessage::user(text)).await.unwrap();
    }
    endless.attach(id, attached()).await.unwrap();
    (dir, endless, store, id)
}

/// R2-6: a folded-run line whose range happens to be a tree address
/// (`0+512`: id % n == 0, n = 2^k) refuses zoom like an unaligned one.
/// R2-10: a run of one unit (`0+1`) still zooms to the unit's raw text.
/// 201 P1: every unit has its draft at ingest, so with merges paused the
/// folded run is the hard-cap prefix, not an unsummarized run.
#[tokio::test]
async fn zoom_refuses_an_aligned_folded_run_but_opens_a_lone_unit() {
    let (_d, endless, _s, id) = unbuilt_session(100..400).await;
    let shown = 300 - endless.memory(id).await.unwrap().view[0].n as usize;

    let (_d, endless, _s, id) = unbuilt_session(100..100 + 512 + shown).await;
    let run = endless.memory(id).await.unwrap().view[0].clone();
    assert_eq!((run.id, run.n, run.built), (0, 512, false), "{}", run.text);
    assert!(super::tree::addr(0, 512).is_some(), "aligned");
    let e = endless.zoom(id, 0, 512).await.unwrap_err();
    assert!(e.contains("recall"), "{e}");

    let (_d, endless, _s, id) = unbuilt_session(100..100 + 1 + shown).await;
    let run = endless.memory(id).await.unwrap().view[0].clone();
    assert_eq!((run.id, run.n, run.built), (0, 1, false), "{}", run.text);
    assert!(
        run.text.contains("older lines folded to fit the view"),
        "a folded run: {}",
        run.text
    );
    let lines = endless.zoom(id, 0, 1).await.unwrap();
    assert!(lines[0].text.contains("message 100:"), "{}", lines[0].text);
}

/// R1-3: at $10/M input a batch over `max_usd_per_call` splits into solo
/// calls (no failure); only a unit whose own call is over the cap is given
/// up (201 P1: it keeps its draft), with one notice.
#[tokio::test]
async fn over_cost_batch_splits_and_only_an_over_cost_unit_folds() {
    let s = Script::priced(Duration::ZERO, Some(10.0), 1_000);
    let (_dir, endless, store, id) = harness_for(
        EndlessConfig {
            max_usd_per_call: 0.03,
            ..cfg_137f(None)
        },
        s.clone(),
    )
    .await;
    store
        .set_session_memory(id, MemoryMode::Endless)
        .await
        .unwrap();
    // Four ~650 B units: a batch of them is over $0.03, each alone is not;
    // one 12 KB unit is over it alone.
    seed_many(&store, id, 4, &[]).await;
    let big = format!(
        "message huge: {}",
        "the rollout notes repeat here ".repeat(400)
    );
    store
        .append(id, &ChatMessage::assistant(big))
        .await
        .unwrap();
    endless.attach(id, attached()).await.unwrap();
    eventually("four units refined", || async {
        level0(&store, id)
            .await
            .iter()
            .filter(|n| n.model.is_some())
            .count()
            == 4
    })
    .await;
    let nodes = level0(&store, id).await;
    assert_eq!(nodes.len(), 5);
    let mechanical: Vec<u64> = nodes.iter().filter(|n| n.mechanical).map(|n| n.i).collect();
    assert_eq!(mechanical, [4], "only the big unit keeps its draft");
    assert!(
        nodes[..4]
            .iter()
            .all(|n| n.model.as_deref() == Some("fake/script"))
    );
    assert_eq!(s.count("batch"), 0, "the batch was never sent");
    assert_eq!(s.count("unit"), 4, "four solo calls");
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
    // 201 P1: every unit has a line at ingest; wait for the drafts' refinement.
    eventually("every unit refined", || async {
        let nodes = level0(&store, id).await;
        nodes.len() == 6 && nodes.iter().all(|n| !n.mechanical)
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
        .append(id, &ChatMessage::assistant(long.clone()))
        .await
        .unwrap();
    endless.attach(id, attached()).await.unwrap();
    assert!(endless.quiet_for_tests(id, Duration::from_secs(10)).await);
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

    // R2-3 / R2-9: each call reports 5,000 tokens at $10/M = $0.05 =
    // `max_usd_per_call` (its ~2k-token estimate passes the per-call check);
    // the day cap is $0.60. Below $0.60 − 4 jobs × $0.05 calls run in
    // parallel, then one at a time. `near_cap` is read from the settled spend
    // at call start, so one tick may start all 4 slots: the overshoot is at
    // most jobs × call = $0.20.
    let s = Script::priced(Duration::from_millis(30), Some(10.0), 4_950);
    let (_dir2, endless, store, id) = harness_for(
        EndlessConfig {
            max_usd_per_call: 0.05,
            max_usd_per_session_day: 0.6,
            batch_units: 1,
            jobs: 4,
            ..cfg_137f(None)
        },
        s.clone(),
    )
    .await;
    store
        .set_session_memory(id, MemoryMode::Endless)
        .await
        .unwrap();
    seed_many(&store, id, 24, &[]).await;
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
    assert!(
        (0.6..=0.6 + 4.0 * 0.05 + 1e-9).contains(&m.spend_today_usd),
        "{}",
        m.spend_today_usd
    );
    assert!(
        s.max_inflight.load(Ordering::SeqCst) > 1,
        "parallel below the near-cap line"
    );
    let calls = s.calls.lock().len();
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

/// The day's spend and the `model_calls` statuses after a priced compactor
/// failed every call with `error`.
async fn spend_after_failing_with(error: &'static str) -> (f64, Vec<String>) {
    let priced: models::Model = serde_json::from_value(serde_json::json!({
        "id": "busy", "name": "busy", "limit": {"context": 200_000, "output": 4096},
        "cost": {"input": 3.0, "output": 15.0}
    }))
    .unwrap();
    let c = Arc::new(Picky {
        model: models::Resolved {
            model: Some(priced),
            ..super::pump::resolved_for_tests("fake/picky-priced")
        },
        error,
        all: true,
    });
    let (_dir, endless, store, id) = harness_for(
        EndlessConfig {
            view_bytes: 16_000,
            retry_secs: 600,
            ..EndlessConfig::default()
        },
        c,
    )
    .await;
    endless_session(&endless, &store, id).await;
    // The row settles before the failure is recorded.
    until("a failure", || {
        !super::pump::failed_for_tests(&endless.inner, id).is_empty()
    })
    .await;
    let spend = endless.memory(id).await.unwrap().spend_today_usd;
    let statuses: Vec<String> = sqlx::query_scalar(
        "SELECT status FROM model_calls WHERE owner_id = ? AND kind = 'endless'",
    )
    .bind(id.to_string())
    .fetch_all(store.pool())
    .await
    .unwrap();
    (spend, statuses)
}

/// R1-4 / R2-1: a failed call is charged the estimate of its prompt only
/// when theuth dropped it after dispatch (a timeout, an idle stream — the
/// provider may bill it); a refusal the provider never bills (529, the
/// local preflight's "not sent") costs nothing, so an outage cannot reach
/// the day cap.
#[tokio::test]
async fn only_calls_dropped_after_dispatch_are_charged() {
    let (spend, statuses) = spend_after_failing_with("timed out after 120s").await;
    assert!(spend > 0.0, "a timed-out call is charged");
    assert!(
        !statuses.is_empty() && statuses.iter().all(|s| s == "failed"),
        "{statuses:?}"
    );
    for error in [
        "529 Overloaded: overloaded_error, try again later",
        "prompt is too long: 91610 tokens > 87308 maximum (theuth preflight, not sent: \
         fake/picky-priced estimated at 2.20 bytes/token, window 200000, reply 600)",
    ] {
        let (spend, statuses) = spend_after_failing_with(error).await;
        assert_eq!(spend, 0.0, "{error}");
        assert!(
            !statuses.is_empty() && statuses.iter().all(|s| s == "failed"),
            "{statuses:?}"
        );
    }
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
    // R1-7: a 32k-window session model leaves ≈ 25 KB for the view.
    let off = endless
        .set_mode_within(id, MemoryMode::Epochs, 1, UnitFilter::All, 25_000)
        .await
        .unwrap();
    let head = off.head.expect("a head");
    let cfg = cfg_137f(None).fold_cfg();
    assert!(
        cfg.high() + cfg.raw_bytes as usize > 40_000,
        "unscaled it would not fit"
    );
    assert!(head.len() <= 25_000 + 4_096, "{} bytes", head.len());
    // 201 P1: the units are drafts, the merges paused: the oldest fold into
    // the hard-cap prefix line.
    assert!(
        head.contains("older lines folded to fit the view"),
        "{}",
        &head[..300]
    );
    assert_eq!(s.calls.lock().len(), 0);
}

/// 194: the real compactor chain (no scripted factory) over a catalog with an
/// effort-taking `ollama/think-194` (low|high) and an effortless
/// `ollama/flat-194` (`ollama` needs no key, so a chat may pick either); an
/// epochs session on the flat one, so no compactor call ever goes out.
async fn effort_harness(cfg: EndlessConfig) -> (crate::TestDir, Endless, SessionId) {
    let scratch = crate::TestDir::new("endless-effort");
    let dir = scratch.0.clone();
    let model = |id: &str, efforts: &[&str]| {
        let mut m = serde_json::json!({
            "id": id, "name": id, "limit": { "context": 100_000, "output": 4_096 }
        });
        if !efforts.is_empty() {
            m["reasoning"] = true.into();
            m["reasoning_options"] = serde_json::json!([{ "type": "effort", "values": efforts }]);
        }
        m
    };
    let catalog = serde_json::json!({ "ollama": { "id": "ollama", "name": "Ollama", "models": {
        "think-194": model("think-194", &["low", "high"]),
        "flat-194": model("flat-194", &[]),
    }}});
    std::fs::write(dir.join("models.json"), catalog.to_string()).unwrap();
    let store = Store::open(&dir.join("sessions.sqlite"), None)
        .await
        .unwrap();
    let catalog = Arc::new(models::Catalog::load(&dir));
    let session_model = catalog.resolve("ollama/flat-194", &|_| true).unwrap();
    let endless = Endless::new(
        store.clone(),
        Runtime::new(figment::Figment::new(), dir.clone()),
        genai::Client::builder().build().unwrap(),
        crate::supervisor::Auth::stub(&dir),
        catalog,
        cfg,
        None,
    );
    let id = SessionId::new();
    store
        .create_session(id, "ollama/flat-194", None, None, None)
        .await
        .unwrap();
    let attached = Attached {
        model: session_model,
        effort: None,
        agent: "theuth".into(),
        store: None,
    };
    endless.attach(id, attached).await.unwrap();
    (scratch, endless, id)
}

/// 194 rows 11–13 (D4e, D4f): each link's effort rides the compactor that
/// link chose, live — `agent.endless.effort` beside the settings model, the
/// desk agent's `memory_effort` beside its `memory_model` — and this chat's
/// own effort wins over either; with none the compactor keeps thinking off.
#[tokio::test]
async fn compactor_efforts_follow_the_chain_live() {
    let (_dir, endless, id) = effort_harness(EndlessConfig::default()).await;
    let key = || super::pump::compactor_key_for_tests(&endless.inner, id);
    let effort = || async { endless.memory(id).await.unwrap().effort };
    assert!(key().starts_with("ollama/flat-194|"), "{}", key());
    assert_eq!(effort().await, None);

    // Settings › Memory: model and effort, in force at once (row 11).
    endless.set_config(EndlessConfig {
        model: Some("ollama/think-194".into()),
        effort: Some("high".into()),
        ..EndlessConfig::default()
    });
    assert!(
        key().starts_with("ollama/think-194|Some(High)|"),
        "{}",
        key()
    );
    assert_eq!(effort().await.as_deref(), Some("high"));

    // The desk agent's `memory_model` with its `memory_effort` (row 13) …
    let m = endless
        .set_agent_model(id, Some("ollama/think-194".into()), Some("low".into()))
        .await
        .unwrap();
    assert_eq!(
        (m.effort.as_deref(), m.effort_override),
        (Some("low"), None),
        "the agent's effort is not this chat's own"
    );
    assert!(
        key().starts_with("ollama/think-194|Some(Low)|"),
        "{}",
        key()
    );
    // … goes with its model: the settings link (and its effort) again.
    let m = endless
        .set_agent_model(id, None, Some("low".into()))
        .await
        .unwrap();
    assert_eq!(m.effort.as_deref(), Some("high"));

    // This chat's own effort (row 12) wins over the settings link's.
    let m = endless.set_effort(id, Some("low".into())).await.unwrap();
    assert_eq!(
        (m.effort.as_deref(), m.effort_override.as_deref()),
        (Some("low"), Some("low"))
    );
    assert!(
        key().starts_with("ollama/think-194|Some(Low)|"),
        "{}",
        key()
    );

    // Settings without an effort: thinking off unless the chat picked one.
    endless.set_effort(id, None).await.unwrap();
    endless.set_config(EndlessConfig {
        model: Some("ollama/think-194".into()),
        ..EndlessConfig::default()
    });
    assert_eq!(effort().await, None);
    assert!(!key().contains("|Some(High)|"), "{}", key());
}

/// 194 R1: Default ↔ a picked `low` switch the compactor in force on a model
/// whose thinking-off default is its listed `low` (same effort, another
/// request) — on the wire: `disabled` at Default, adaptive `low` once picked,
/// `disabled` again once cleared.
#[tokio::test]
async fn default_and_a_picked_low_switch_the_compactor_request() {
    let h = crate::loop_::thinking_tests::Harness::texting().await;
    let scratch = crate::TestDir::new("endless-pick-low");
    let dir = scratch.0.clone();
    let catalog = serde_json::json!({ "anthropic": { "id": "anthropic", "name": "Anthropic", "models": {
        "claude-haiku-5-5": {
            "id": "claude-haiku-5-5", "name": "Haiku 5.5", "reasoning": true,
            "limit": { "context": 200_000, "output": 64_000 },
            "reasoning_options": [{ "type": "effort", "values": ["low", "medium", "high"] }]
        }
    }}});
    std::fs::write(dir.join("models.json"), catalog.to_string()).unwrap();
    let store = Store::open(&dir.join("sessions.sqlite"), None)
        .await
        .unwrap();
    let catalog = Arc::new(models::Catalog::load(&dir));
    let model = catalog
        .resolve("anthropic/claude-haiku-5-5", &|_| true)
        .unwrap();
    let endless = Endless::new(
        store.clone(),
        Runtime::new(figment::Figment::new(), dir.clone()),
        h.client.clone(),
        crate::supervisor::Auth::stub(&dir),
        catalog,
        EndlessConfig::default(),
        None,
    );
    let id = SessionId::new();
    store
        .create_session(id, "anthropic/claude-haiku-5-5", None, None, None)
        .await
        .unwrap();
    let attached = Attached {
        model,
        effort: None,
        agent: "theuth".into(),
        store: None,
    };
    endless.attach(id, attached).await.unwrap();
    let body = || async {
        let step = [ChatMessage::user(
            "Compress this message into one line:\nhello",
        )];
        super::pump::compactor_in_force_for_tests(&endless.inner, id)
            .compress("<chat>\n</chat>", &step)
            .await
            .unwrap();
        h.bodies.lock().pop().unwrap()
    };
    let disabled = serde_json::json!({"type": "disabled"});
    let b = body().await;
    assert_eq!(b["thinking"], disabled, "{b}");
    endless.set_effort(id, Some("low".into())).await.unwrap();
    let b = body().await;
    assert_eq!(
        b["thinking"],
        serde_json::json!({"type": "adaptive"}),
        "{b}"
    );
    assert_eq!(b["output_config"]["effort"], "low", "{b}");
    endless.set_effort(id, None).await.unwrap();
    let b = body().await;
    assert_eq!(b["thinking"], disabled, "{b}");
}

/// 194 row 12 (D4b–d, persistence): this chat's own compactor effort is
/// refused when the model in force does not take it, persisted
/// (`sessions.endless_effort`), dropped when the chat's own new model does
/// not take it and cleared with the chat's own model.
#[tokio::test]
async fn the_chats_own_compactor_effort_is_checked_kept_and_dropped() {
    let (_dir, endless, id) = effort_harness(EndlessConfig::default()).await;
    // In force: the session's flat model — no effort at all.
    let e = endless
        .set_effort(id, Some("low".into()))
        .await
        .unwrap_err();
    assert!(e.to_string().contains("accepts no reasoning effort"), "{e}");
    endless
        .set_model(id, Some("ollama/think-194".into()))
        .await
        .unwrap();
    let e = endless
        .set_effort(id, Some("max".into()))
        .await
        .unwrap_err();
    assert!(e.to_string().contains("accepts off|low|high"), "{e}");
    endless.set_effort(id, Some("high".into())).await.unwrap();
    let meta = endless.inner.store.endless_meta(id).await.unwrap();
    assert_eq!(meta.effort_override.as_deref(), Some("high"));
    // A model that takes it keeps it (D4c: only an incompatible one drops).
    let m = endless
        .set_model(id, Some("ollama/think-194".into()))
        .await
        .unwrap();
    assert_eq!(m.effort.as_deref(), Some("high"));
    // D4c: the chat's new model takes no effort — dropped and persisted so.
    let m = endless
        .set_model(id, Some("ollama/flat-194".into()))
        .await
        .unwrap();
    assert_eq!(m.effort, None);
    let meta = endless.inner.store.endless_meta(id).await.unwrap();
    assert_eq!(meta.effort_override, None);
    // D4d: "Use default" clears the chat's effort with its model.
    endless
        .set_model(id, Some("ollama/think-194".into()))
        .await
        .unwrap();
    endless.set_effort(id, Some("low".into())).await.unwrap();
    endless.set_model(id, None).await.unwrap();
    let meta = endless.inner.store.endless_meta(id).await.unwrap();
    assert_eq!((meta.model_override, meta.effort_override), (None, None));
}

/// 194: a config written before `agent.endless.effort` loads with none.
#[test]
fn an_endless_section_without_effort_loads() {
    let c: EndlessConfig = serde_json::from_value(serde_json::json!({ "model": "p/m" })).unwrap();
    assert_eq!((c.model.as_deref(), c.effort), (Some("p/m"), None));
}

/// 201 §12.1: the defaults are the table's.
#[test]
fn the_201_defaults_are_the_table() {
    use super::{DescribeImages, Profile, TailMode};
    let c = EndlessConfig::default();
    assert_eq!(c.tail_mode, TailMode::Fixed);
    assert_eq!((c.tail_low_bytes, c.tail_high_bytes), (24_000, 64_000));
    assert_eq!((c.tail_low_share, c.tail_high_share), (0.10, 0.25));
    assert_eq!(c.cold_after_secs, 0);
    assert_eq!(c.profile, Profile::Auto);
    assert_eq!(c.merge_at, 0.75);
    assert_eq!(
        (c.ledger, c.ledger_bytes, c.ledger_suggestions),
        (true, 4_000, true)
    );
    assert_eq!((c.prefetch_bytes, c.age_by_use), (2_000, true));
    assert_eq!(c.describe_images, DescribeImages::Auto);
    assert!(!c.default_chat, "201 PC does not change default_chat");
    assert_eq!(c.validate_201(), Ok(()));
    // Lowercase on the wire.
    let v = serde_json::to_value(&c).unwrap();
    assert_eq!(
        (&v["tail_mode"], &v["profile"], &v["describe_images"]),
        (
            &serde_json::json!("fixed"),
            &serde_json::json!("auto"),
            &serde_json::json!("auto")
        )
    );
}

/// 201 §12.1: a section written before 201 (with the retired 137 keys) loads with the new defaults.
#[test]
fn a_pre_201_section_loads_with_the_new_defaults() {
    let c: EndlessConfig =
        serde_yaml::from_str("view_bytes: 96000\nsettle_secs: 30\nraw_bytes: 16000\njobs: 4\n")
            .unwrap();
    assert_eq!(
        (c.view_bytes, c.settle_secs, c.raw_bytes),
        (96_000, 30, 16_000)
    );
    let d = EndlessConfig::default();
    assert_eq!(
        EndlessConfig {
            view_bytes: d.view_bytes,
            settle_secs: d.settle_secs,
            raw_bytes: d.raw_bytes,
            jobs: d.jobs,
            ..c
        },
        d
    );
    let c: EndlessConfig =
        serde_yaml::from_str("tail_mode: scaled\nprofile: small\ndescribe_images: off\n").unwrap();
    assert_eq!(
        (c.tail_mode, c.profile, c.describe_images),
        (
            super::TailMode::Scaled,
            super::Profile::Small,
            super::DescribeImages::Off
        )
    );
}

/// 201 §12.1: out-of-range values clamp at use.
#[test]
fn the_201_keys_clamp_at_use() {
    let c = EndlessConfig {
        tail_low_bytes: 1,
        tail_high_bytes: 9_000_000,
        tail_low_share: 0.0,
        tail_high_share: 5.0,
        cold_after_secs: 1_000_000,
        merge_at: 0.1,
        ledger_bytes: 99_999,
        prefetch_bytes: 99_999,
        ..EndlessConfig::default()
    }
    .clamped();
    assert_eq!((c.tail_low_bytes, c.tail_high_bytes), (8_000, 512_000));
    assert_eq!((c.tail_low_share, c.tail_high_share), (0.02, 0.60));
    assert_eq!((c.cold_after_secs, c.merge_at), (86_400, 0.50));
    assert_eq!((c.ledger_bytes, c.prefetch_bytes), (16_000, 8_000));
}

/// 201 §12.1: `endless_settings_set` refuses a pair whose high is not above its low, naming both.
#[test]
fn a_tail_pair_with_high_not_above_low_is_refused() {
    let bytes = EndlessConfig {
        tail_low_bytes: 64_000,
        tail_high_bytes: 64_000,
        ..EndlessConfig::default()
    };
    let e = bytes.validate_201().unwrap_err();
    assert!(
        e.contains("tail_high_bytes 64000") && e.contains("tail_low_bytes 64000"),
        "{e}"
    );
    let shares = EndlessConfig {
        tail_low_share: 0.3,
        tail_high_share: 0.2,
        ..EndlessConfig::default()
    };
    let e = shares.validate_201().unwrap_err();
    assert!(
        e.contains("tail_high_share 0.2") && e.contains("tail_low_share 0.3"),
        "{e}"
    );
    let range = EndlessConfig {
        ledger_bytes: 100,
        ..EndlessConfig::default()
    };
    assert!(
        range
            .validate_201()
            .unwrap_err()
            .starts_with("ledger_bytes 100")
    );
    // The per-session override refuses the same way.
    let o = super::TailOverride {
        mode: super::TailMode::Fixed,
        low: 40_000.0,
        high: 30_000.0,
    };
    assert!(o.validate().unwrap_err().contains("must be above"));
}

/// 201 §4.2: `tail_marks` for both modes and the allowance scale-down.
#[test]
fn tail_marks_fixed_scaled_and_scaled_down() {
    let fixed = EndlessConfig::default();
    assert_eq!(fixed.tail_marks(1_000_000), (24_000, 64_000));
    let scaled = EndlessConfig {
        tail_mode: super::TailMode::Scaled,
        ..EndlessConfig::default()
    };
    assert_eq!(scaled.tail_marks(800_000), (80_000, 200_000));
    // Clamped to 8_000..=512_000: a huge allowance caps high, a small one lifts low.
    assert_eq!(scaled.tail_marks(4_000_000).1, 512_000);
    assert_eq!(scaled.tail_marks(60_000), (8_000, 15_000));
    // High above half the allowance: both scale down, keeping their ratio.
    assert_eq!(fixed.tail_marks(64_000), (12_000, 32_000));
    let (lo, hi) = fixed.tail_marks(10_000);
    assert_eq!((lo, hi), (1_875, 5_000));
    // An override replaces the mode and both marks.
    let o = super::TailOverride {
        mode: super::TailMode::Fixed,
        low: 16_000.0,
        high: 32_000.0,
    };
    assert_eq!(
        fixed.with_tail(Some(&o)).tail_marks(1_000_000),
        (16_000, 32_000)
    );
}

/// 201 §4.2: the per-session tail override persists, shows in `SessionMemory`, survives a restart and clears.
#[tokio::test]
async fn session_tail_override_round_trips_and_clears() {
    let (_dir, endless, store, _echo, id) = harness().await;
    let m = endless.memory(id).await.unwrap();
    assert_eq!(
        (m.tail_mode, m.tail_low, m.tail_high, m.tail_override),
        (super::TailMode::Fixed, 24_000, 64_000, None)
    );
    assert_eq!(
        (m.tail_bytes, m.generation, m.drafts, m.profile),
        (0, 0, 0, super::Profile::Standard)
    );
    let o = super::TailOverride {
        mode: super::TailMode::Fixed,
        low: 16_000.0,
        high: 48_000.0,
    };
    endless.set_tail(id, Some(o)).await.unwrap();
    assert_eq!(store.endless_meta(id).await.unwrap().tail, Some(o));
    let m = endless.memory(id).await.unwrap();
    assert_eq!(
        (m.tail_low, m.tail_high, m.tail_override),
        (16_000, 48_000, Some(o))
    );
    endless.detach(id);
    endless.attach(id, attached()).await.unwrap();
    assert_eq!(
        endless.memory(id).await.unwrap().tail_override,
        Some(o),
        "survives a restart"
    );
    endless.set_tail(id, None).await.unwrap();
    let m = endless.memory(id).await.unwrap();
    assert_eq!(
        (m.tail_low, m.tail_high, m.tail_override),
        (24_000, 64_000, None)
    );
    assert_eq!(store.endless_meta(id).await.unwrap().tail, None);
}

/// 201 P1: a compactor whose reply is `reply(step input)`; counts calls.
struct Scripted {
    model: models::Resolved,
    reply: fn(&str) -> Reply,
    calls: AtomicU32,
}

impl Scripted {
    fn new(reply: fn(&str) -> Reply) -> Arc<Self> {
        Arc::new(Self {
            model: super::pump::resolved_for_tests("fake/scripted"),
            reply,
            calls: AtomicU32::new(0),
        })
    }
}

impl Compactor for Scripted {
    fn compress<'a>(
        &'a self,
        _ctx: &'a str,
        convo: &'a [ChatMessage],
    ) -> BoxFuture<'a, Result<Reply, String>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let r = (self.reply)(&echo_line(convo));
        Box::pin(async move { Ok(r) })
    }

    fn model(&self) -> &models::Resolved {
        &self.model
    }
}

fn ungrounded(_: &str) -> Reply {
    Reply {
        text: "zebra crossing painted blue".into(),
        ..Default::default()
    }
}

fn cfg_p1(batch_units: u8) -> EndlessConfig {
    EndlessConfig {
        view_bytes: 16_000,
        retry_secs: 600,
        batch_units,
        ..EndlessConfig::default()
    }
}

fn quality(n: &super::tree::Node) -> super::tree::Quality {
    n.quality()
}

/// 201 P1 (D5): with a compactor that always errors, every unit has a line
/// right after ingest, and a turn start does not wait.
#[tokio::test]
async fn every_unit_has_a_line_at_ingest_and_no_turn_waits() {
    let p = Probe::new("fake/a", Act::Fail);
    let (_dir, endless, store, id) = harness_for(cfg_p1(16), p.clone()).await;
    store
        .set_session_memory(id, MemoryMode::Endless)
        .await
        .unwrap();
    seed_many(&store, id, 10, &[]).await;
    endless.attach(id, attached()).await.unwrap();
    let nodes = level0(&store, id).await;
    assert_eq!(nodes.len(), 10, "a line for every unit at once");
    assert!(
        nodes
            .iter()
            .all(|n| quality(n) == super::tree::Quality::Draft)
    );
    let t0 = Instant::now();
    assert!(endless.settle(id, Duration::from_secs(45)).await);
    let r = endless.render_for_turn(id).await.unwrap();
    assert!(t0.elapsed() < Duration::from_secs(2), "{:?}", t0.elapsed());
    assert_eq!(r.unbuilt, 0, "no placeholder");
    assert_eq!(endless.memory(id).await.unwrap().drafts, 10);
    // A unit appended later gets its draft on ingest, too.
    seed_many(&store, id, 1, &[]).await;
    endless.render_for_turn(id).await.unwrap();
    assert_eq!(level0(&store, id).await.len(), 11);
}

/// 201 P1 (F9): a reply that fails its guards never becomes a stored line;
/// the draft stays and the unit backs off.
#[tokio::test]
async fn guard_failing_reply_keeps_the_draft() {
    let c = Scripted::new(ungrounded);
    let (_dir, endless, store, id) = harness_for(cfg_p1(1), c.clone()).await;
    store
        .set_session_memory(id, MemoryMode::Endless)
        .await
        .unwrap();
    seed_many(&store, id, 3, &[]).await;
    endless.attach(id, attached()).await.unwrap();
    until("every unit failed once", || {
        super::pump::failed_for_tests(&endless.inner, id)
            .iter()
            .filter(|(k, ..)| k.0 == 0)
            .count()
            == 3
    })
    .await;
    for n in level0(&store, id).await {
        assert_eq!(quality(&n), super::tree::Quality::Draft);
        assert!(!n.text.contains("zebra"), "{}", n.text);
    }
}

/// 201 P1 (F8): a batch reply spent on thinking fails the batch once; its
/// units keep their drafts and do not fan out into solo calls.
#[tokio::test]
async fn batch_spent_on_thinking_does_not_fan_out() {
    fn thinking(_: &str) -> Reply {
        Reply {
            reasoning: true,
            truncated: true,
            ..Default::default()
        }
    }
    let c = Scripted::new(thinking);
    let (_dir, endless, store, id) = harness_for(cfg_p1(16), c.clone()).await;
    store
        .set_session_memory(id, MemoryMode::Endless)
        .await
        .unwrap();
    seed_many(&store, id, 4, &[]).await;
    endless.attach(id, attached()).await.unwrap();
    until("the batch failed", || {
        super::pump::failed_for_tests(&endless.inner, id).len() == 4
    })
    .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(c.calls.load(Ordering::SeqCst), 1, "one batch call, no solo");
    let s = endless.memory(id).await.unwrap().stalled.unwrap();
    assert!(s.error.contains("thinking"), "{}", s.error);
    assert!(
        level0(&store, id)
            .await
            .iter()
            .all(|n| quality(n) == super::tree::Quality::Draft)
    );
}

/// 201 P1: refining a child marks its draft parent for rebuilding, and the
/// rebuilt parent carries the model lines.
#[tokio::test]
async fn refined_child_rebuilds_its_draft_parent() {
    let p = Probe::new("fake/a", Act::Fail);
    let (_dir, endless, store, id) = harness_for(cfg_p1(16), p.clone()).await;
    store
        .set_session_memory(id, MemoryMode::Endless)
        .await
        .unwrap();
    seed_many(&store, id, 2, &[]).await;
    endless.attach(id, attached()).await.unwrap();
    let parent = || async {
        store
            .endless_nodes(id)
            .await
            .unwrap()
            .into_iter()
            .find(|n| (n.l, n.i) == (1, 0))
    };
    eventually("a draft parent", || async {
        parent()
            .await
            .is_some_and(|n| quality(&n) == super::tree::Quality::Draft)
    })
    .await;
    for (i, text) in [(0u64, "model line zero"), (1, "model line one")] {
        super::pump::finish_for_tests(
            &endless.inner,
            id,
            (0, i),
            Ok(super::tree::Node {
                l: 0,
                i,
                text: text.into(),
                size: text.len() as u32,
                free: false,
                model: Some("fake/a".into()),
                tries: 1,
                mechanical: false,
            }),
        )
        .await;
    }
    eventually("the parent rebuilt from the model lines", || async {
        parent().await.is_some_and(|n| {
            n.text.contains("model line zero")
                && n.text.contains("model line one")
                && quality(&n) != super::tree::Quality::Draft
        })
    })
    .await;
}

/// 201 P1: loading a session whose units have no nodes gives them drafts.
#[tokio::test]
async fn load_drafts_node_less_units() {
    let p = Probe::new("fake/a", Act::Fail);
    let (_dir, endless, store, id) = harness_for(cfg_p1(16), p.clone()).await;
    let mut rows = Vec::new();
    for k in 0..3u64 {
        let text = format!(
            "user note {k}: {}",
            "the cache keeps these rows ".repeat(30)
        );
        let seq = store
            .append(id, &ChatMessage::assistant(text.clone()))
            .await
            .unwrap();
        rows.push(crate::store::endless::UnitRow {
            i: k,
            seq,
            part: 0,
            kind: super::tree::Kind::Talk,
            size: (text.len() + "talk: ".len()) as u32,
            turn: 0,
        });
    }
    store.endless_units_put(id, &rows).await.unwrap();
    assert!(store.endless_nodes(id).await.unwrap().is_empty());
    store
        .set_session_memory(id, MemoryMode::Endless)
        .await
        .unwrap();
    endless.attach(id, attached()).await.unwrap();
    let nodes = level0(&store, id).await;
    assert_eq!(nodes.len(), 3);
    assert!(
        nodes
            .iter()
            .all(|n| quality(n) == super::tree::Quality::Draft)
    );
}

/// 201 P1 (F10): one unit that keeps failing stops neither later units nor
/// the merges over them.
#[tokio::test]
async fn failing_unit_blocks_nothing() {
    fn bad_fails(input: &str) -> Reply {
        let text = if input.contains("BAD") {
            "zebra crossing painted blue".to_owned()
        } else {
            input.to_owned()
        };
        Reply {
            text,
            ..Default::default()
        }
    }
    let c = Scripted::new(bad_fails);
    let (_dir, endless, store, id) = harness_for(cfg_p1(1), c.clone()).await;
    store
        .set_session_memory(id, MemoryMode::Endless)
        .await
        .unwrap();
    seed_many(&store, id, 4, &[0]).await;
    endless.attach(id, attached()).await.unwrap();
    eventually("units 1..4 refined and (1, 1) merged", || async {
        let nodes = store.endless_nodes(id).await.unwrap();
        let q = |l: u8, i: u64| nodes.iter().find(|n| (n.l, n.i) == (l, i)).map(quality);
        (1..4).all(|i| q(0, i) == Some(super::tree::Quality::Model))
            // 201 P3: model merges are lazy; the line over 2..4 exists.
            && q(1, 1).is_some()
    })
    .await;
    let nodes = level0(&store, id).await;
    assert_eq!(quality(&nodes[0]), super::tree::Quality::Draft);
}

/// 201 §3/§4.3 (P2): a generation's view covers only the units before the
/// tail, is persisted, and a restart reads back the same bytes; the next
/// trim opens the next generation. Generation 1 of an endless session with
/// history covers it all (an empty tail).
#[tokio::test]
async fn generation_view_is_frozen_and_survives_a_restart() {
    let p = Probe::new("fake/a", Act::Fail);
    let (_dir, endless, store, id) = harness_for(cfg_p1(16), p.clone()).await;
    let mut seqs = Vec::new();
    for k in 0..4 {
        seqs.push(
            store
                .append(id, &ChatMessage::user(format!("note {k}: the plan")))
                .await
                .unwrap(),
        );
    }
    store
        .set_session_memory(id, MemoryMode::Endless)
        .await
        .unwrap();
    endless.attach(id, attached()).await.unwrap();
    assert_eq!(endless.generation(id).await.unwrap(), None);
    let g1 = endless
        .start_generation(id, None, 1_000_000, false)
        .await
        .unwrap();
    assert_eq!((g1.n, g1.t_cut, g1.first_seq), (1, 4, seqs[3] + 1));
    assert!(g1.render.text().contains("note 3"), "{}", g1.render.text());

    // Two more rows; a trim keeps them as the tail: the view stops before them.
    let s5 = store
        .append(id, &ChatMessage::user("note 4: tail".to_owned()))
        .await
        .unwrap();
    store
        .append(id, &ChatMessage::user("note 5: tail".to_owned()))
        .await
        .unwrap();
    let g2 = endless
        .start_generation(id, Some(s5), 1_000_000, false)
        .await
        .unwrap();
    assert_eq!((g2.n, g2.t_cut, g2.first_seq), (2, 4, s5));
    assert!(!g2.render.text().contains("note 4"), "{}", g2.render.text());
    assert_eq!(endless.memory(id).await.unwrap().generation, 2);

    // A restart reproduces the frozen view byte for byte.
    endless.detach(id);
    endless.attach(id, attached()).await.unwrap();
    let back = endless.generation(id).await.unwrap().expect("persisted");
    assert_eq!(back.n, 2);
    assert_eq!(back.render.text(), g2.render.text());
    assert_eq!(back.render.blocks, g2.render.blocks);
}

/// 201 §4.2 (P2): the per-session override wins over the config; `scaled`
/// takes its marks as shares of the turn allowance.
#[tokio::test]
async fn tail_marks_follow_the_override_and_scaled_shares() {
    let p = Probe::new("fake/a", Act::Fail);
    let (_dir, endless, store, id) = harness_for(cfg_p1(16), p.clone()).await;
    store
        .set_session_memory(id, MemoryMode::Endless)
        .await
        .unwrap();
    endless.attach(id, attached()).await.unwrap();
    assert_eq!(endless.tail_marks(id, 1_000_000), (24_000, 64_000));
    let fixed = super::TailOverride {
        mode: super::TailMode::Fixed,
        low: 10_000.0,
        high: 30_000.0,
    };
    endless.set_tail(id, Some(fixed)).await.unwrap();
    assert_eq!(endless.tail_marks(id, 1_000_000), (10_000, 30_000));
    let scaled = super::TailOverride {
        mode: super::TailMode::Scaled,
        low: 0.10,
        high: 0.25,
    };
    endless.set_tail(id, Some(scaled)).await.unwrap();
    assert_eq!(endless.tail_marks(id, 400_000), (40_000, 100_000));
}

/// 201 §3 (P2): a line refined mid-generation does not touch the frozen
/// view; it shows from the next generation's render on.
#[tokio::test]
async fn refined_line_waits_for_the_next_generation() {
    let p = Probe::new("fake/a", Act::Fail);
    let (_dir, endless, store, id) = harness_for(cfg_p1(16), p.clone()).await;
    store
        .set_session_memory(id, MemoryMode::Endless)
        .await
        .unwrap();
    seed_many(&store, id, 2, &[]).await;
    endless.attach(id, attached()).await.unwrap();
    let g1 = endless
        .start_generation(id, None, 1_000_000, false)
        .await
        .unwrap();
    assert!(!g1.render.text().contains("model line zero"));
    for (i, text) in [(0u64, "model line zero"), (1, "model line one")] {
        super::pump::finish_for_tests(
            &endless.inner,
            id,
            (0, i),
            Ok(super::tree::Node {
                l: 0,
                i,
                text: text.into(),
                size: text.len() as u32,
                free: false,
                model: Some("fake/a".into()),
                tries: 1,
                mechanical: false,
            }),
        )
        .await;
    }
    eventually("the parent rebuilt from the model lines", || async {
        store
            .endless_nodes(id)
            .await
            .unwrap()
            .iter()
            .all(|n| quality(n) != super::tree::Quality::Draft)
    })
    .await;
    // Same generation: byte for byte the frozen view.
    let same = endless.generation(id).await.unwrap().unwrap();
    assert_eq!((same.n, same.render.text()), (1, g1.render.text()));
    // The next trim renders the refined lines.
    let g2 = endless
        .start_generation(id, None, 1_000_000, false)
        .await
        .unwrap();
    assert_eq!(g2.n, 2);
    assert!(
        g2.render.text().contains("model line zero"),
        "{}",
        g2.render.text()
    );
}

/// 201 §7.3 (P3, D8): a long user message, a tool call and an error echo
/// get final (free) lines at ingest and are never sent to the model; only
/// the long talk unit is refined.
#[tokio::test]
async fn kinds_that_need_no_model_are_never_sent() {
    let p = Probe::new("fake/a", Act::Echo);
    let (_dir, endless, store, id) = harness_for(cfg_p1(1), p.clone()).await;
    store
        .set_session_memory(id, MemoryMode::Endless)
        .await
        .unwrap();
    let long = "please look at the deploy notes again. ".repeat(40);
    store
        .append(id, &ChatMessage::user(long.clone()))
        .await
        .unwrap();
    store
        .append(
            id,
            &ChatMessage::assistant(MessageContent::from_parts(vec![ContentPart::ToolCall(
                ToolCall {
                    call_id: "c1".into(),
                    fn_name: "write".into(),
                    fn_arguments: serde_json::json!({"path": "/srv/a.txt", "content": long}),
                    thought_signatures: None,
                },
            )])),
        )
        .await
        .unwrap();
    let echo = format!("error: disk full\n{}", "wrote block\n".repeat(400));
    store
        .append(id, &ChatMessage::from(vec![ToolResponse::new("c1", echo)]))
        .await
        .unwrap();
    store
        .append(
            id,
            &ChatMessage::assistant("the deploy failed because the disk is full. ".repeat(30)),
        )
        .await
        .unwrap();
    endless.attach(id, attached()).await.unwrap();
    assert!(endless.quiet_for_tests(id, Duration::from_secs(10)).await);
    let inputs = p.inputs.lock().clone();
    assert_eq!(inputs.len(), 1, "{inputs:?}");
    assert!(inputs[0].starts_with("talk: "), "{inputs:?}");
    let nodes = level0(&store, id).await;
    assert_eq!(nodes.len(), 4);
    assert!(
        nodes[1].text.starts_with("tool: write path=/srv/a.txt"),
        "{}",
        nodes[1].text
    );
    assert!(nodes[2].text.contains("1 errors"), "{}", nodes[2].text);
}

/// Model-built merges in the store.
async fn model_merges(store: &Store, id: SessionId) -> usize {
    store
        .endless_nodes(id)
        .await
        .unwrap()
        .iter()
        .filter(|n| n.l > 0 && n.model.is_some())
        .count()
}

/// 201 §6 (P3): lazy merges — no model merge while the view is under
/// `merge_at` × `view_bytes`; over it, model merges are built.
#[tokio::test]
async fn model_merges_wait_for_merge_at() {
    let p = Probe::new("fake/a", Act::Echo);
    let (_dir, endless, store, id) = harness_for(cfg_p1(1), p.clone()).await;
    store
        .set_session_memory(id, MemoryMode::Endless)
        .await
        .unwrap();
    seed_many(&store, id, 4, &[]).await;
    endless.attach(id, attached()).await.unwrap();
    assert!(endless.quiet_for_tests(id, Duration::from_secs(10)).await);
    assert_eq!(p.calls(), 4, "the four units only");
    assert_eq!(model_merges(&store, id).await, 0, "under merge_at");
    // ~60 refined lines of ~400 bytes: the view passes 0.75 × 16 KB.
    seed_many(&store, id, 60, &[]).await;
    endless.on_rows_committed(id);
    let t0 = Instant::now();
    while model_merges(&store, id).await == 0 {
        assert!(
            t0.elapsed() < Duration::from_secs(10),
            "no merge over merge_at"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// 201 §8 (P4): a compactor that may accept images; counts descriptions and
/// records whether any request ever carried image bytes.
struct Seer {
    model: models::Resolved,
    vision: bool,
    described: AtomicU32,
    saw_bytes: std::sync::atomic::AtomicBool,
}

impl Seer {
    fn new(vision: bool) -> Arc<Self> {
        Arc::new(Self {
            model: super::pump::resolved_for_tests("fake/seer"),
            vision,
            described: AtomicU32::new(0),
            saw_bytes: std::sync::atomic::AtomicBool::new(false),
        })
    }
}

impl Compactor for Seer {
    fn compress<'a>(
        &'a self,
        _ctx: &'a str,
        convo: &'a [ChatMessage],
    ) -> BoxFuture<'a, Result<Reply, String>> {
        let bytes = convo.iter().any(|m| {
            m.content.iter().any(|p| match p {
                ContentPart::Binary(_) => true,
                ContentPart::ToolResponse(r) => !r.images.is_empty(),
                _ => false,
            })
        });
        if bytes {
            self.saw_bytes.store(true, Ordering::SeqCst);
        }
        Box::pin(async { Err("scripted failure".to_owned()) })
    }
    fn model(&self) -> &models::Resolved {
        &self.model
    }
    fn accepts_images(&self) -> bool {
        self.vision
    }
    fn describe_image<'a>(
        &'a self,
        _image: &'a genai::chat::Binary,
    ) -> BoxFuture<'a, Result<Reply, String>> {
        self.saw_bytes.store(true, Ordering::SeqCst);
        self.described.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {
            Ok(Reply {
                text: "a red square on white\n".into(),
                ..Default::default()
            })
        })
    }
}

/// A session with one screenshot (tool picture) and one user image.
async fn seed_images(store: &Store, id: SessionId) {
    let png = |n: &str| genai::chat::Binary::from_base64("image/png", "iVBORw0K", Some(n.into()));
    store
        .append(
            id,
            &ChatMessage::user(MessageContent::from_parts(vec![ContentPart::Binary(png(
                "photo.png",
            ))])),
        )
        .await
        .unwrap();
    store
        .append(
            id,
            &ChatMessage::from(vec![
                ToolResponse::new("c1", "shot taken").with_images(vec![png("/tmp/screen.png")]),
            ]),
        )
        .await
        .unwrap();
}

async fn l0(store: &Store, id: SessionId, i: u64) -> super::tree::Node {
    store
        .endless_nodes(id)
        .await
        .unwrap()
        .into_iter()
        .find(|n| n.l == 0 && n.i == i)
        .unwrap()
}

/// 201 §8 (P4): units 0 (`[image: photo.png]`), 1 (echo), 2 (tool picture);
/// at a trim with `auto` an image-capable compactor describes both images,
/// the line records the source, and the next generation shows it.
#[tokio::test]
async fn trim_describes_images_leaving_the_tail() {
    let seer = Seer::new(true);
    let (_dir, endless, store, id) = harness_for(cfg_p1(16), seer.clone()).await;
    store
        .set_session_memory(id, MemoryMode::Endless)
        .await
        .unwrap();
    seed_images(&store, id).await;
    endless.attach(id, attached()).await.unwrap();
    let g1 = endless
        .start_generation(id, None, 1_000_000, false)
        .await
        .unwrap();
    assert!(g1.render.text().contains("[image: photo.png]"));
    assert!(g1.render.text().contains("echo: [image: screen.png]"));
    endless
        .describe_left(id, 0, i64::MAX, None)
        .expect("a description job")
        .await
        .unwrap();
    assert_eq!(seer.described.load(Ordering::SeqCst), 2);
    for i in [0, 2] {
        let n = l0(&store, id, i).await;
        assert!(n.text.ends_with(" — a red square on white"), "{}", n.text);
        assert_eq!(n.model.as_deref(), Some("described:compactor:fake/seer"));
    }
    // Already described: nothing to do at the next trim.
    assert!(endless.describe_left(id, 0, i64::MAX, None).is_none());
    let g2 = endless
        .start_generation(id, None, 1_000_000, false)
        .await
        .unwrap();
    assert!(
        g2.render.text().contains("a red square"),
        "{}",
        g2.render.text()
    );
}

/// 201 §8 (P4): `describe_images: off` skips descriptions; a text-only
/// compactor with no vision model is never sent image bytes.
#[tokio::test]
async fn no_description_when_off_or_text_only() {
    let seer = Seer::new(true);
    let cfg = EndlessConfig {
        describe_images: super::DescribeImages::Off,
        ..cfg_p1(16)
    };
    let (_dir, endless, store, id) = harness_for(cfg, seer.clone()).await;
    store
        .set_session_memory(id, MemoryMode::Endless)
        .await
        .unwrap();
    seed_images(&store, id).await;
    endless.attach(id, attached()).await.unwrap();
    assert!(endless.describe_left(id, 0, i64::MAX, None).is_none());
    assert_eq!(seer.described.load(Ordering::SeqCst), 0);

    let text_only = Seer::new(false);
    let (_dir2, endless, store, id) = harness_for(cfg_p1(16), text_only.clone()).await;
    store
        .set_session_memory(id, MemoryMode::Endless)
        .await
        .unwrap();
    seed_images(&store, id).await;
    endless.attach(id, attached()).await.unwrap();
    if let Some(h) = endless.describe_left(id, 0, i64::MAX, None) {
        h.await.unwrap();
    }
    assert!(!text_only.saw_bytes.load(Ordering::SeqCst));
    assert_eq!(l0(&store, id, 0).await.text, "user: [image: photo.png]");
}

/// 201 §8 (P4): `zoom(id, 1)` on an artefact unit returns the stored
/// binary with its line; other units are no artefacts.
#[tokio::test]
async fn zoom_on_an_image_unit_returns_the_binary() {
    let (_dir, endless, store, id) = harness_for(cfg_p1(16), Seer::new(false)).await;
    store
        .set_session_memory(id, MemoryMode::Endless)
        .await
        .unwrap();
    seed_images(&store, id).await;
    endless.attach(id, attached()).await.unwrap();
    let (line, b) = endless.artefact(id, 2).await.unwrap().unwrap();
    assert_eq!(line, "echo: [image: screen.png]");
    assert_eq!(b.name.as_deref(), Some("/tmp/screen.png"));
    let (_, b) = endless.artefact(id, 0).await.unwrap().unwrap();
    assert_eq!(b.name.as_deref(), Some("photo.png"));
    assert!(
        endless.artefact(id, 1).await.is_none(),
        "an echo is no artefact"
    );
    let out = crate::loop_::endless_tools::run(
        &endless,
        id,
        crate::loop_::endless_tools::ZOOM,
        &serde_json::json!({"id": 2, "n": 1}),
    )
    .await;
    assert_eq!(out.files.len(), 1, "{}", out.content);
    assert!(out.content.starts_with("2+1|echo: [image: screen.png]"));
    let _ = std::fs::remove_file(&out.files[0]);
}
