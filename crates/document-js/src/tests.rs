//! Behaviour of the runtime and its thread, and the five containment guards. After each failure the same
//! handle must serve a normal open → import → markdown round trip (the test process surviving the failure is
//! itself the proof that the failure stayed contained).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::{DocumentsJs, DocumentsThread, Error, Limits, Reply};

/// `Y.encodeStateAsUpdate(new Y.Doc())`.
const EMPTY_STATE: &str = "AAA=";

fn limits() -> Limits {
    Limits {
        deadline: Duration::from_millis(1500),
        ..Limits::default()
    }
}

fn thread(limits: Limits) -> DocumentsThread {
    DocumentsThread::new(limits, CancellationToken::new())
}

fn parse(line: &str) -> Value {
    serde_json::from_str(line).unwrap_or_else(|e| panic!("not JSON ({e}): {line}"))
}

fn ok(reply: &Reply) -> Value {
    let v = parse(&reply.reply);
    assert_eq!(v["ok"], true, "request failed: {}", reply.reply);
    v
}

async fn req(t: &DocumentsThread, id: u64, op: &str, doc: &str, extra: Value) -> Reply {
    let mut body = json!({ "id": id, "op": op, "doc": doc });
    for (k, v) in extra.as_object().expect("object") {
        body[k] = v.clone();
    }
    t.request(body.to_string())
        .await
        .unwrap_or_else(|e| panic!("{op}: {e}"))
}

/// open → import → markdown → close on a fresh document; panics unless every step succeeds.
async fn round_trip(t: &DocumentsThread, doc: &str) {
    ok(&req(t, 1, "open", doc, json!({ "state": EMPTY_STATE })).await);
    let import = req(t, 2, "import", doc, json!({ "markdown": "# Title\n\nBody **bold**.", "mode": "replace" })).await;
    ok(&import);
    assert!(!import.events.is_empty(), "import pushed no update");
    let md = ok(&req(t, 3, "markdown", doc, json!({})).await);
    assert_eq!(md["result"]["markdown"], "# Title\n\nBody **bold**.\n");
    ok(&req(t, 4, "close", doc, json!({})).await);
}

async fn wait_until(what: &str, mut f: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(10);
    while !f() {
        assert!(Instant::now() < until, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[test]
fn documents_js_serves_requests_directly() {
    let mut js = DocumentsJs::new(Limits::default()).expect("runtime");
    assert!(js.version().expect("version").starts_with("theuth-documents "));
    let open = js
        .call(&json!({ "id": 1, "op": "open", "doc": "d", "state": EMPTY_STATE }).to_string())
        .expect("open");
    assert_eq!(open, vec![r#"{"id":1,"ok":true}"#.to_owned()]);
    let lines = js
        .call(&json!({ "id": 2, "op": "import", "doc": "d", "markdown": "- a\n- b", "mode": "replace" }).to_string())
        .expect("import");
    assert!(lines.len() >= 2, "{lines:?}");
    assert_eq!(parse(&lines[0])["ev"], "update");
    assert_eq!(parse(lines.last().expect("reply"))["id"], 2);
}

#[tokio::test]
async fn round_trip_open_import_markdown_outline_edit_close() {
    let t = thread(limits());
    ok(&req(&t, 1, "open", "d", json!({ "state": EMPTY_STATE })).await);
    let import = ok(&req(
        &t,
        2,
        "import",
        "d",
        json!({ "markdown": "---\ntitle: T\n---\n# Heading\n\nFirst paragraph.\n\n- one\n- two\n", "mode": "replace", "frontmatter": true }),
    )
    .await);
    assert_eq!(import["result"]["changed"], 4, "{import}");
    let md = ok(&req(&t, 3, "markdown", "d", json!({})).await);
    assert_eq!(
        md["result"]["markdown"],
        "---\ntitle: T\n---\n# Heading\n\nFirst paragraph.\n\n* one\n* two\n"
    );
    let outline = ok(&req(&t, 4, "outline", "d", json!({})).await);
    let blocks = outline["result"]["blocks"].as_array().expect("blocks");
    let heading = blocks
        .iter()
        .find(|b| b["type"] == "heading")
        .expect("heading block");
    assert_eq!(heading["text"], "[h1] Heading");
    let edit = req(
        &t,
        5,
        "edit",
        "d",
        json!({ "edits": [{ "op": "insert", "markdown": "Inserted *here*.", "after": heading["id"] }] }),
    )
    .await;
    let result = ok(&edit);
    assert_eq!(result["result"]["inserted"].as_array().map(Vec::len), Some(1));
    assert_eq!(edit.events.len(), 1);
    let md = ok(&req(&t, 6, "markdown", "d", json!({})).await);
    assert_eq!(
        md["result"]["markdown"],
        "---\ntitle: T\n---\n# Heading\n\nInserted *here*.\n\nFirst paragraph.\n\n* one\n* two\n"
    );
    ok(&req(&t, 7, "close", "d", json!({})).await);
    let gone = parse(&req(&t, 8, "markdown", "d", json!({})).await.reply);
    assert_eq!(gone["ok"], false);
    assert_eq!(gone["error"], "document d is not open");
}

#[tokio::test]
async fn update_events_come_before_the_reply() {
    let t = thread(limits());
    ok(&req(&t, 1, "open", "d", json!({ "state": EMPTY_STATE })).await);
    let import = req(&t, 9, "import", "d", json!({ "markdown": "a\n\nb\n\nc", "mode": "replace" })).await;
    ok(&import);
    assert!(!import.events.is_empty());
    for e in &import.events {
        let v = parse(e);
        assert_eq!(v["ev"], "update", "{e}");
        assert_eq!(v["doc"], "d");
        assert_eq!(v["req"], 9);
        assert!(v["update"].as_str().is_some_and(|u| !u.is_empty()));
    }
    assert_eq!(parse(&import.reply)["id"], 9);
    assert!(parse(&import.reply).get("ev").is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_requests_from_several_handles_run_in_enqueue_order() {
    let t = thread(limits());
    ok(&req(&t, 1, "open", "d", json!({ "state": EMPTY_STATE })).await);
    // Each task takes the next item number and enqueues under one lock, so item order = enqueue order;
    // tasks await their tickets concurrently.
    ok(&req(&t, 2, "import", "d", json!({ "markdown": "start", "mode": "replace" })).await);
    let order = Arc::new(std::sync::Mutex::new(0u32));
    let mut tasks = Vec::new();
    for _ in 0..4 {
        let t = t.clone();
        let order = order.clone();
        tasks.push(tokio::spawn(async move {
            let mut tickets = Vec::new();
            for _ in 0..5 {
                let ticket = {
                    let mut n = order.lock().expect("lock");
                    let i = *n;
                    *n += 1;
                    let line = json!({ "id": 100 + i, "op": "import", "doc": "d", "markdown": format!("item {i}"), "mode": "append" });
                    t.enqueue(line.to_string())
                };
                tickets.push(ticket);
                tokio::task::yield_now().await;
            }
            for ticket in tickets {
                ok(&ticket.await.expect("append"));
            }
        }));
    }
    for task in tasks {
        task.await.expect("task");
    }
    let md = ok(&req(&t, 2, "markdown", "d", json!({})).await);
    let items = (0..20).map(|i| format!("item {i}\n")).collect::<Vec<_>>().join("\n");
    let expected = format!("start\n\n{items}");
    assert_eq!(md["result"]["markdown"], expected);
}

/// 3a: a JS exception is an error reply and the runtime stays usable (same generation).
#[tokio::test]
async fn js_exception_is_an_error_reply_and_the_runtime_survives() {
    let t = thread(limits());
    round_trip(&t, "before").await;
    // The bundle's entry point throws once: the pump answers ok:false with the request's id.
    t.eval_raw(
        "const orig = globalThis.theuthDocuments.call;
         globalThis.theuthDocuments.call = async () => { globalThis.theuthDocuments.call = orig; throw new TypeError('boom from js'); };"
            .into(),
    )
    .await
    .expect("install the throwing entry point");
    let r = t
        .request(json!({ "id": 7, "op": "outline", "doc": "x" }).to_string())
        .await
        .expect("reply");
    let v = parse(&r.reply);
    assert_eq!(v["ok"], false);
    assert_eq!(v["id"], 7);
    assert!(v["error"].as_str().is_some_and(|e| e.contains("boom from js")), "{v}");
    // An exception inside a conversion (an edit naming no block) is an error reply from the helper itself.
    ok(&req(&t, 8, "open", "e", json!({ "state": EMPTY_STATE })).await);
    let bad = parse(&req(&t, 9, "edit", "e", json!({ "edits": [{ "op": "delete", "blocks": ["nope"] }] })).await.reply);
    assert_eq!(bad["ok"], false);
    // An uncaught exception in a raw script is `Error::Js`, not fatal.
    let raw = t.eval_raw("throw new Error('raw boom')".into()).await;
    assert!(matches!(&raw, Err(Error::Js(m)) if m.contains("raw boom")), "{raw:?}");
    assert_eq!(t.generation(), 1, "a JS exception must not rebuild the runtime");
    round_trip(&t, "after").await;
}

/// 3b: a runaway script is terminated from the watchdog; queued jobs fail with it; the runtime is rebuilt.
#[tokio::test]
async fn deadline_terminates_a_runaway_script_and_rebuilds() {
    let t = thread(limits());
    round_trip(&t, "before").await;
    let mut generations = t.subscribe();
    assert_eq!(*generations.borrow_and_update(), 1);
    let started = Instant::now();
    let spin = t.eval_raw("for (;;) {}".into());
    let queued = t.enqueue(json!({ "id": 5, "op": "outline", "doc": "d" }).to_string());
    assert_eq!(spin.await, Err(Error::Timeout));
    assert_eq!(queued.await, Err(Error::Timeout), "a job queued behind the hang fails with it");
    assert!(started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());
    generations.changed().await.expect("generation");
    assert_eq!(*generations.borrow_and_update(), 2);
    round_trip(&t, "after spin").await;
    // A request stuck waiting on the event loop (no JS running) hits the same deadline.
    let wait = t
        .eval_raw("new Promise(() => setTimeout(() => {}, 600000))".into())
        .await;
    assert_eq!(wait, Err(Error::Timeout));
    generations.changed().await.expect("generation");
    assert_eq!(*generations.borrow_and_update(), 3);
    round_trip(&t, "after wait").await;
}

/// 3c: the heap ceiling terminates instead of aborting the process; the runtime is rebuilt.
#[tokio::test]
async fn heap_limit_terminates_and_rebuilds() {
    let t = thread(Limits {
        deadline: Duration::from_secs(20),
        heap_limit: 128 * 1024 * 1024,
        ..Limits::default()
    });
    round_trip(&t, "before").await;
    let grow = t
        .eval_raw("const keep = []; for (;;) keep.push(new Array(100000).fill(0.5));".into())
        .await;
    assert_eq!(grow, Err(Error::OutOfMemory));
    wait_until("rebuild", || t.generation() == 2).await;
    round_trip(&t, "after").await;
}

/// 3d: a panic inside an op (which would otherwise unwind into V8 and abort) is contained.
#[tokio::test]
async fn panic_in_an_op_is_contained() {
    let t = thread(limits());
    round_trip(&t, "before").await;
    let r = t
        .eval_raw("Deno.core.ops.op_documents_test_panic(); for (;;) {}".into())
        .await;
    assert!(
        matches!(&r, Err(Error::Crashed(m)) if m.contains("panic inside a documents op")),
        "{r:?}"
    );
    wait_until("rebuild", || t.generation() == 2).await;
    round_trip(&t, "after").await;
}

/// 3d: a panic on the runtime thread outside JS fails the job and everything queued; a fresh runtime serves.
#[tokio::test]
async fn panic_on_the_thread_fails_pending_jobs_and_a_fresh_runtime_serves() {
    let t = thread(limits());
    round_trip(&t, "before").await;
    // Keep the thread busy while the panic and the jobs behind it are enqueued, so they are all queued.
    let busy = t.eval_raw("{ const t0 = Date.now(); while (Date.now() - t0 < 300) {} }".into());
    let panicked = t.panic_on_thread();
    let queued: Vec<_> = (0..3)
        .map(|i| t.enqueue(json!({ "id": 50 + i, "op": "outline", "doc": "before" }).to_string()))
        .collect();
    busy.await.expect("busy script");
    let r = panicked.await;
    assert!(
        matches!(&r, Err(Error::Crashed(m)) if m.contains("panic on the documents-js thread")),
        "{r:?}"
    );
    for q in queued {
        assert!(matches!(q.await, Err(Error::Crashed(_))));
    }
    assert!(t.is_running(), "the thread survives its job's panic");
    wait_until("rebuild", || t.generation() == 2).await;
    round_trip(&t, "after").await;
}

/// 3e: an oversized line never reaches JS (here: the thread is not even started).
#[tokio::test]
async fn oversized_line_is_rejected_before_js() {
    let t = thread(Limits {
        max_line: 1024,
        ..limits()
    });
    let big = json!({ "id": 1, "op": "import", "doc": "d", "markdown": "x".repeat(2000), "mode": "replace" }).to_string();
    let len = big.len();
    assert_eq!(t.request(big.clone()).await, Err(Error::TooLarge { len, max: 1024 }));
    assert!(!t.is_running(), "rejected before the thread started");
    round_trip(&t, "d").await;
    assert_eq!(t.request(big).await, Err(Error::TooLarge { len, max: 1024 }));
    assert_eq!(t.generation(), 1);
    round_trip(&t, "d2").await;
    // The runtime enforces the same cap when called directly.
    // `DocumentsJs` drives its own tokio runtime, so it runs off this async test's runtime.
    let direct = std::thread::spawn(|| {
        let mut js = DocumentsJs::new(Limits { max_line: 16, ..limits() }).expect("runtime");
        js.call(&"y".repeat(17))
    })
    .join()
    .expect("direct call");
    assert_eq!(direct, Err(Error::TooLarge { len: 17, max: 16 }));
}

#[tokio::test]
async fn thread_starts_lazily() {
    let t = thread(limits());
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!t.is_running());
    assert_eq!(t.generation(), 0);
    round_trip(&t, "d").await;
    assert!(t.is_running());
    assert_eq!(t.generation(), 1);
}

#[tokio::test]
async fn dropping_the_last_handle_ends_the_thread() {
    let t = thread(limits());
    let other = t.clone();
    round_trip(&t, "d").await;
    let alive: Arc<AtomicBool> = t.liveness();
    drop(t);
    round_trip(&other, "still served by the remaining handle").await;
    assert!(alive.load(Ordering::SeqCst));
    drop(other);
    wait_until("thread exit", || !alive.load(Ordering::SeqCst)).await;
}

#[tokio::test]
async fn cancelling_the_token_ends_the_thread() {
    let token = CancellationToken::new();
    let t = DocumentsThread::new(limits(), token.clone());
    round_trip(&t, "d").await;
    token.cancel();
    wait_until("thread exit", || !t.is_running()).await;
    let r = t
        .request(json!({ "id": 1, "op": "outline", "doc": "d" }).to_string())
        .await;
    assert_eq!(r, Err(Error::Stopped));
}
