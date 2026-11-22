//! Behaviour of the runtime and its thread with the real [`APPS`] bundle, and the containment guards. After
//! each failure the same handle must serve a normal check → merge_check round trip (the test process surviving
//! the failure is itself the proof that the failure stayed contained). Ported from document-js's tests.

use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::{APPS, Error, JsRuntime, JsThread, Limits, MERMAID, Reply};

const PROTOTYPE: &str = "root = Stack([title, status, controls])\n$stage = \"idle\"\ntitle = TextContent(\"## Drill\")\nstatus = TextContent(\"Stage: \" + $stage)\ncontrols = Stack([begin, reset], \"row\")\nbegin = Button(\"Begin\", Action([@Set($stage, \"cutover\")]))\nreset = Button(\"Reset\", Action([@Reset($stage)]))";

/// A short request deadline, so the runaway tests end quickly. Building a runtime (bundle load) is not under
/// it (`JsRuntime::new` loads under at least `LOAD_DEADLINE`), so a slow debug build under load only stretches
/// the normal requests, which take milliseconds.
fn limits() -> Limits {
    Limits {
        deadline: Duration::from_millis(1500),
        ..Limits::default()
    }
}

fn thread(limits: Limits) -> JsThread {
    JsThread::new(&APPS, limits, CancellationToken::new())
}

fn parse(line: &str) -> Value {
    serde_json::from_str(line).unwrap_or_else(|e| panic!("not JSON ({e}): {line}"))
}

fn check_line(id: &str, source: &str) -> String {
    json!({ "id": id, "op": "check", "source": source, "tools": [] }).to_string()
}

fn ok(reply: &Reply, id: &str) -> Value {
    let v = parse(&reply.reply);
    assert_eq!(v["id"], id, "{}", reply.reply);
    assert!(v.get("error").is_none(), "request failed: {}", reply.reply);
    assert!(reply.events.is_empty());
    v
}

/// check → merge_check on the prototype; panics unless both validate.
async fn round_trip(t: &JsThread, tag: &str) {
    let c = t
        .request(check_line(&format!("{tag}-c"), PROTOTYPE))
        .await
        .unwrap_or_else(|e| panic!("check: {e}"));
    let c = ok(&c, &format!("{tag}-c"));
    assert_eq!(c["diagnostics"], json!([]), "{c}");
    assert_eq!(c["state"], json!({ "$stage": "idle" }));
    let m = t
        .request(
            json!({ "id": format!("{tag}-m"), "op": "merge_check", "source": "title = TextContent(\"## B\")", "base": PROTOTYPE, "tools": [] })
                .to_string(),
        )
        .await
        .unwrap_or_else(|e| panic!("merge_check: {e}"));
    let m = ok(&m, &format!("{tag}-m"));
    assert_eq!(m["diagnostics"], json!([]), "{m}");
    assert!(
        m["source"]
            .as_str()
            .is_some_and(|s| s.contains("title = TextContent(\"## B\")")),
        "{m}"
    );
}

async fn wait_until(what: &str, mut f: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(60);
    while !f() {
        assert!(Instant::now() < until, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[test]
fn bundle_js_serves_requests_directly() {
    let mut js = JsRuntime::new(&APPS, Limits::default()).expect("runtime");
    let version = js.version().expect("version");
    assert!(version.starts_with("theuth-app-parser "), "{version}");
    assert_eq!(version.len(), "theuth-app-parser ".len() + 64);
    let lines = js.call(&check_line("1", PROTOTYPE)).expect("check");
    assert_eq!(lines.len(), 1, "{lines:?}");
    let v = parse(&lines[0]);
    assert_eq!(v["id"], "1");
    assert_eq!(v["diagnostics"], json!([]));
    assert_eq!(v["statements"].as_array().map(Vec::len), Some(7));
}

#[tokio::test]
async fn check_and_merge_check_round_trip_with_real_output() {
    let t = thread(limits());
    let tools = json!([{ "name": "argo_sync", "schema": { "type": "object", "properties": { "name": { "type": "string" } }, "required": ["name"] }, "risk": "mutating", "identity": "mcp:argo/argo_sync#1" }]);
    let source = "root = Stack([b])\nsync = Mutation(\"argo_sync\", {name: \"web\"})\nb = Button(\"Sync\", Action([@Run(sync)]))";
    let r = t
        .request(json!({ "id": "a", "op": "check", "source": source, "tools": tools }).to_string())
        .await
        .expect("check");
    let v = ok(&r, "a");
    assert_eq!(v["diagnostics"], json!([]), "{v}");
    assert_eq!(
        v["bindings"],
        json!([{ "statement_id": "sync", "tool": "argo_sync", "kind": "mutation", "risk": "mutating", "identity": "mcp:argo/argo_sync#1" }])
    );
    // The same source without the tool: located diagnostic, still a result.
    let r = t
        .request(json!({ "id": "b", "op": "check", "source": source, "tools": [] }).to_string())
        .await
        .expect("check");
    let v = ok(&r, "b");
    assert_eq!(v["diagnostics"][0]["at"], "sync", "{v}");
    assert_eq!(v["diagnostics"][0]["code"], "tool-unknown", "{v}");
    // merge_check: delete with null + GC.
    let r = t
        .request(
            json!({ "id": "c", "op": "merge_check", "source": "controls = Stack([begin], \"row\")\nreset = null", "base": PROTOTYPE, "tools": [] })
                .to_string(),
        )
        .await
        .expect("merge");
    let v = ok(&r, "c");
    assert_eq!(v["diagnostics"], json!([]), "{v}");
    assert!(
        !v["source"].as_str().expect("source").contains("reset ="),
        "{v}"
    );
    // A protocol error is a reply line too.
    let r = t
        .request(json!({ "id": "d", "op": "nope" }).to_string())
        .await
        .expect("reply");
    assert_eq!(
        parse(&r.reply),
        json!({ "id": "d", "error": "op must be \"check\" or \"merge_check\"" })
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_requests_from_several_handles_each_get_their_own_reply() {
    let t = thread(limits());
    let mut tasks = Vec::new();
    for task in 0..4 {
        let t = t.clone();
        tasks.push(tokio::spawn(async move {
            let tickets: Vec<_> = (0..5)
                .map(|i| {
                    let id = format!("{task}-{i}");
                    (id.clone(), t.enqueue(check_line(&id, PROTOTYPE)))
                })
                .collect();
            for (id, ticket) in tickets {
                ok(&ticket.await.expect("check"), &id);
            }
        }));
    }
    for task in tasks {
        task.await.expect("task");
    }
    assert_eq!(t.generation(), 1);
}

/// A JS exception is an error reply naming the request and the runtime stays usable (same generation).
#[tokio::test]
async fn js_exception_is_an_error_reply_and_the_runtime_survives() {
    let t = thread(limits());
    round_trip(&t, "before").await;
    t.eval_raw(
        "const orig = globalThis.theuthApps.call;
         globalThis.theuthApps.call = () => { globalThis.theuthApps.call = orig; throw new TypeError('boom from js'); };"
            .into(),
    )
    .await
    .expect("install the throwing entry point");
    let r = t
        .request(check_line("x7", "root = Stack([])"))
        .await
        .expect("reply");
    let v = parse(&r.reply);
    assert_eq!(v["ok"], false);
    assert_eq!(v["id"], "x7");
    assert!(
        v["error"]
            .as_str()
            .is_some_and(|e| e.contains("boom from js")),
        "{v}"
    );
    let raw = t.eval_raw("throw new Error('raw boom')".into()).await;
    assert!(
        matches!(&raw, Err(Error::Js(m)) if m.contains("raw boom")),
        "{raw:?}"
    );
    assert_eq!(
        t.generation(),
        1,
        "a JS exception must not rebuild the runtime"
    );
    round_trip(&t, "after").await;
}

/// A runaway script is terminated from the watchdog; queued jobs fail with it; the runtime is rebuilt.
#[tokio::test]
async fn deadline_terminates_a_runaway_script_and_rebuilds() {
    let t = thread(limits());
    round_trip(&t, "before").await;
    assert_eq!(t.generation(), 1);
    let started = Instant::now();
    let spin = t.eval_raw("for (;;) {}".into());
    let queued = t.enqueue(check_line("q", PROTOTYPE));
    assert_eq!(spin.await, Err(Error::Timeout));
    // Not started when the runaway failed: re-run on the fresh runtime (review m1).
    ok(
        &queued.await.expect("queued behind the hang is served"),
        "q",
    );
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "{:?}",
        started.elapsed()
    );
    wait_until("rebuild", || t.generation() == 2).await;
    round_trip(&t, "after spin").await;
    // A request whose entry point spins is terminated the same way.
    t.eval_raw("globalThis.theuthApps.call = () => { for (;;) {} };".into())
        .await
        .expect("install the spinning entry point");
    assert_eq!(
        t.request(check_line("spin", PROTOTYPE)).await,
        Err(Error::Timeout)
    );
    wait_until("rebuild", || t.generation() == 3).await;
    round_trip(&t, "after request spin").await;
}

/// The heap ceiling terminates instead of aborting the process; the runtime is rebuilt.
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

/// A panic inside an op (which would otherwise unwind into V8 and abort) is contained.
#[tokio::test]
async fn panic_in_an_op_is_contained() {
    let t = thread(limits());
    round_trip(&t, "before").await;
    let r = t
        .eval_raw("Deno.core.ops.op_jsrt_test_panic(); for (;;) {}".into())
        .await;
    assert!(
        matches!(&r, Err(Error::Crashed(m)) if m.contains("panic inside a js-runtime op")),
        "{r:?}"
    );
    wait_until("rebuild", || t.generation() == 2).await;
    round_trip(&t, "after").await;
}

/// A panic on the runtime thread outside JS fails that job; the jobs queued behind it run on a fresh runtime.
#[tokio::test]
async fn panic_on_the_thread_fails_that_job_and_a_fresh_runtime_serves_the_queue() {
    let t = thread(limits());
    round_trip(&t, "before").await;
    let busy = t.eval_raw("{ const t0 = Date.now(); while (Date.now() - t0 < 300) {} }".into());
    let panicked = t.panic_on_thread();
    let queued: Vec<_> = (0..3)
        .map(|i| t.enqueue(check_line(&format!("q{i}"), PROTOTYPE)))
        .collect();
    busy.await.expect("busy script");
    let r = panicked.await;
    assert!(
        matches!(&r, Err(Error::Crashed(m)) if m.contains("panic on the app-js thread")),
        "{r:?}"
    );
    for (i, q) in queued.into_iter().enumerate() {
        ok(&q.await.expect("queued job re-run"), &format!("q{i}"));
    }
    assert!(t.is_running(), "the thread survives its job's panic");
    wait_until("rebuild", || t.generation() == 2).await;
    round_trip(&t, "after").await;
}

/// An oversized line never reaches JS (here: the thread is not even started).
#[tokio::test]
async fn oversized_line_is_rejected_before_js() {
    let t = thread(Limits {
        max_line: 1024,
        ..limits()
    });
    let big = check_line("big", &"x".repeat(2000));
    let len = big.len();
    assert_eq!(
        t.request(big.clone()).await,
        Err(Error::TooLarge { len, max: 1024 })
    );
    assert!(!t.is_running(), "rejected before the thread started");
    round_trip(&t, "a").await;
    assert_eq!(
        t.request(big).await,
        Err(Error::TooLarge { len, max: 1024 })
    );
    assert_eq!(t.generation(), 1);
    round_trip(&t, "b").await;
    // The runtime enforces the same cap when called directly. `JsRuntime` drives its own tokio runtime, so it
    // runs off this async test's runtime.
    let direct = std::thread::spawn(|| {
        let mut js = JsRuntime::new(
            &APPS,
            Limits {
                max_line: 16,
                ..limits()
            },
        )
        .expect("runtime");
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
    let alive = t.liveness();
    drop(t);
    round_trip(&other, "still served by the remaining handle").await;
    assert!(alive());
    drop(other);
    wait_until("thread exit", || !alive()).await;
}

#[tokio::test]
async fn cancelling_the_token_ends_the_thread() {
    let token = CancellationToken::new();
    let t = JsThread::new(&APPS, limits(), token.clone());
    round_trip(&t, "d").await;
    token.cancel();
    wait_until("thread exit", || !t.is_running()).await;
    assert_eq!(
        t.request(check_line("1", PROTOTYPE)).await,
        Err(Error::Stopped)
    );
}

/// The request's own promise is still pending when an unrelated rejection fails the event loop: fatal, the
/// runtime is rebuilt, and the next requests each get their own answer.
#[tokio::test]
async fn an_event_loop_error_mid_request_is_fatal_and_the_next_request_gets_its_own_reply() {
    let t = thread(limits());
    round_trip(&t, "before").await;
    t.eval_raw(
        r#"const orig = globalThis.theuthApps.call;
           globalThis.theuthApps.call = (line) => {
             globalThis.theuthApps.call = orig;
             const id = JSON.parse(line).id;
             Promise.reject(new Error("unrelated rejection"));
             return new Promise((resolve) =>
               setTimeout(() => resolve(JSON.stringify({ id, late: true })), 100));
           };"#
        .into(),
    )
    .await
    .expect("install the hook");
    let first = t.request(check_line("e1", PROTOTYPE)).await;
    assert!(
        matches!(&first, Err(Error::Crashed(m)) if m.contains("unrelated rejection")),
        "{first:?}"
    );
    for id in ["e2", "e3"] {
        let r = t.request(check_line(id, PROTOTYPE)).await.expect("reply");
        ok(&r, id);
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert_eq!(t.generation(), 2);
}

/// A request whose promise never settles (idle loop, no reply) ends the runtime too.
#[tokio::test]
async fn a_request_that_ends_without_its_reply_is_fatal() {
    let t = thread(limits());
    round_trip(&t, "before").await;
    t.eval_raw(
        "const orig = globalThis.theuthApps.call;
         globalThis.theuthApps.call = () => { globalThis.theuthApps.call = orig; return new Promise(() => {}); };"
            .into(),
    )
    .await
    .expect("install the hook");
    let r = t.request(check_line("n", PROTOTYPE)).await;
    assert!(
        matches!(&r, Err(Error::Crashed(m)) if m.contains("without its reply")),
        "{r:?}"
    );
    wait_until("rebuild", || t.generation() == 2).await;
    round_trip(&t, "after").await;
}

/// `restart`: the running request's output is discarded; the queued one runs on the fresh runtime.
#[tokio::test]
async fn restart_fails_the_running_request_and_the_queued_ones_run_fresh() {
    let t = thread(limits());
    round_trip(&t, "before").await;
    t.eval_raw("globalThis.__mark = 'old';".into())
        .await
        .expect("mark");
    let busy = t.eval_raw("{ const t0 = Date.now(); while (Date.now() - t0 < 1000) {} }".into());
    let queued = t.eval_raw(
        "if (globalThis.__mark !== undefined) throw new Error('queued ran on the abandoned runtime');".into(),
    );
    // `restart` must hit the busy job while it runs (a job not yet started simply runs on the fresh
    // runtime); it spins for 1 s, so 200 ms after enqueueing it is running.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let old = queued.epoch();
    let epoch = t.restart();
    assert!(epoch > old);
    let busy = busy.settled().await;
    assert!(
        matches!(&busy.result, Err(Error::Crashed(m)) if m.contains("given up")),
        "the running job's output is from the abandoned runtime: {busy:?}"
    );
    let queued = queued.settled().await;
    assert!(
        queued.result.is_ok(),
        "queued job runs on the fresh runtime: {queued:?}"
    );
    round_trip(&t, "after").await;
    assert_eq!(t.generation(), 2);
}

/// A thread whose loop returned (not shutdown) is respawned by the next request.
#[tokio::test]
async fn a_dead_runtime_thread_is_respawned() {
    let t = thread(limits());
    round_trip(&t, "before").await;
    assert_eq!(t.exit_thread().await, Err(Error::Stopped));
    let first = t.enqueue(check_line("r", PROTOTYPE)).settled().await;
    assert!(first.rebuilt, "{first:?}");
    ok(&first.result.expect("reply"), "r");
    round_trip(&t, "after").await;
    assert_eq!(t.generation(), 2, "the new thread built its own runtime");
    assert!(t.is_running());
}

/// Two bundles' handles are independent: each has its own thread and isolate and limits.
#[tokio::test]
async fn two_handles_run_two_isolates() {
    let a = thread(limits());
    let b = thread(Limits {
        deadline: Duration::from_millis(800),
        ..limits()
    });
    round_trip(&a, "a").await;
    round_trip(&b, "b").await;
    assert_eq!(b.eval_raw("for (;;) {}".into()).await, Err(Error::Timeout));
    round_trip(&a, "a unaffected").await;
    assert_eq!(a.generation(), 1);
    wait_until("rebuild", || b.generation() == 2).await;
}

/// `init_platform` (twice) and its body inside a multi-thread async context, where `t` calls it (`main`'s
/// `block_on`): builds an isolate under the current handle, no runtime dropped in async context.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn init_platform_builds_an_isolate_inside_a_multi_thread_async_context() {
    crate::init_platform();
    crate::init_platform();
    crate::runtime::first_isolate();
}

/// Same inside a current-thread async context.
#[tokio::test]
async fn init_platform_builds_an_isolate_inside_a_current_thread_async_context() {
    crate::runtime::first_isolate();
    crate::init_platform();
}

/// Same on a plain thread with no tokio context: a throwaway runtime, disposed without blocking.
#[test]
fn init_platform_builds_an_isolate_on_a_plain_thread() {
    std::thread::spawn(crate::runtime::first_isolate)
        .join()
        .expect("first isolate on a plain thread");
    crate::init_platform();
}

/// Review m1: 1 runaway + 5 queued checks → the runaway times out, the 5 succeed on the fresh runtime.
#[tokio::test]
async fn one_runaway_does_not_fail_the_checks_queued_behind_it() {
    let t = thread(limits());
    round_trip(&t, "before").await;
    let spin = t.eval_raw("for (;;) {}".into());
    let queued: Vec<_> = (0..5)
        .map(|i| t.enqueue(check_line(&format!("c{i}"), PROTOTYPE)))
        .collect();
    assert_eq!(spin.await, Err(Error::Timeout));
    for (i, q) in queued.into_iter().enumerate() {
        let v = ok(&q.await.expect("queued check served"), &format!("c{i}"));
        assert_eq!(v["diagnostics"], json!([]));
    }
    assert_eq!(t.generation(), 2);
}

/// The mermaid spike's ordering bug, restated for re-run semantics: no job may run on the runtime a fatal
/// failure gave up. The jobs queued before the failure and the one sent after seeing it all run on the fresh
/// runtime (no mark there).
#[tokio::test]
async fn after_a_fatal_failure_no_job_runs_on_the_abandoned_runtime() {
    let t = thread(limits());
    round_trip(&t, "before").await;
    // Marks the runtime so we can tell which one served a request.
    t.eval_raw("globalThis.__mark = 'old';".into())
        .await
        .expect("mark");
    let spin = t.eval_raw("for (;;) {}".into());
    let queued: Vec<_> = (0..3)
        .map(|i| t.eval_raw(format!("if (globalThis.__mark !== undefined) throw new Error('ran on the old runtime {i}');")))
        .collect();
    assert_eq!(spin.await, Err(Error::Timeout));
    // After seeing the failure: this job must run on the fresh runtime (no mark there).
    let after = t.eval_raw(
        "if (globalThis.__mark !== undefined) throw new Error('after: old runtime');".into(),
    );
    for q in queued {
        assert!(
            q.await.is_ok(),
            "queued before the failure: re-run on the fresh runtime"
        );
    }
    assert_eq!(
        after.await,
        Ok(Reply {
            events: Vec::new(),
            reply: String::new()
        })
    );
    assert_eq!(t.generation(), 2);
    round_trip(&t, "after").await;
}

fn mermaid_line(id: &str, source: &str) -> String {
    json!({ "id": id, "op": "check_mermaid", "source": source }).to_string()
}

/// The mermaid bundle answers through a promise; the runtime drives the event loop until the reply op fires.
#[tokio::test]
async fn mermaid_bundle_answers_asynchronously_on_its_own_lazy_thread() {
    let m = JsThread::new(
        &MERMAID,
        Limits {
            deadline: Duration::from_secs(5),
            first_deadline: Duration::from_secs(10),
            heap_limit: 256 * 1024 * 1024,
            max_line: 4 * 1024 * 1024,
        },
        CancellationToken::new(),
    );
    assert!(!m.is_running());
    let ok = m
        .request(mermaid_line("m1", "flowchart TD\n  A --> B"))
        .await
        .expect("parse");
    assert_eq!(ok.reply, r#"{"id":"m1","ok":true}"#);
    assert!(m.is_running());
    let bad = parse(
        &m.request(mermaid_line(
            "m2",
            "flowchart TD\n  A --> B\n}}} ]]] ((( @@@ !!! <<<",
        ))
        .await
        .expect("parse")
        .reply,
    );
    assert_eq!(bad["ok"], false);
    assert_eq!(bad["errors"][0]["line"], 3, "{bad}");
    let v = std::thread::spawn(|| {
        JsRuntime::new(&MERMAID, Limits::default())
            .and_then(|mut js| js.version())
            .expect("version")
    })
    .join()
    .expect("version thread");
    assert_eq!(v, "theuth-mermaid 1");
}
