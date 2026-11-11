//! 86 RUNTIME-BRIDGE, through a real [`Turn`] against the fake provider of
//! [`super::vision_fallback_tests`]: a direct `tool_use` of a runtime function
//! the turn's `## Runtime` index listed runs as a `python` cell under the same
//! call_id, with the teaching note first; a name the model cannot see (unknown,
//! or hidden for the session) is refused. One tokio test: the tool registry is
//! a process-global actor bound to the runtime that spawned it. The bridged
//! scenarios need a python worker (`$THEUTH_WORKER`, else `monty`/`t` on
//! `PATH`) and skip without one; the refusal needs none.

use serde_json::{Value, json};

use super::vision_fallback_tests::{Fake, Harness};
use super::*;

/// A pool over a worker binary; `None`, with a note, when absent so the
/// bridged scenarios skip.
async fn worker_pool() -> Option<Arc<Pool>> {
    let worker = std::env::var_os("THEUTH_WORKER")
        .map(PathBuf::from)
        .or_else(|| {
            let path = std::env::var_os("PATH")?;
            ["monty", "t"].iter().find_map(|n| {
                std::env::split_paths(&path)
                    .map(|d| d.join(n))
                    .find(|p| p.is_file())
            })
        });
    let Some(worker) = worker else {
        eprintln!("no monty/t worker on PATH; skipping the bridged scenarios");
        return None;
    };
    let pool = repl::open_pool(&worker, 1).await;
    if pool.is_none() {
        eprintln!("worker pool failed to start; skipping the bridged scenarios");
    }
    pool
}

/// `(call_id, name, args)` of every `ToolCallStarted`, in order.
fn started(events: &[AgentEvent]) -> Vec<(String, String, Value)> {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ToolCallStarted {
                call_id,
                name,
                args,
                ..
            } => Some((call_id.clone(), name.clone(), args.clone())),
            _ => None,
        })
        .collect()
}

/// `(call_id, is_error, output)` of every `ToolCallFinished`, in order.
fn finished(events: &[AgentEvent]) -> Vec<(String, bool, String)> {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ToolCallFinished {
                call_id,
                is_error,
                output,
                ..
            } => Some((call_id.clone(), *is_error, output.clone())),
            _ => None,
        })
        .collect()
}

/// One turn opened by `go` whose first model reply is the fake's tool call.
async fn run(h: &mut Harness) -> Vec<AgentEvent> {
    let (turn, tid) = h.turn("openai/textonly", CancellationToken::new()).await;
    turn.run(tid, vec![h.input("go", false)]).await;
    h.drain().await
}

#[test]
fn guessed_direct_calls_are_bridged_or_refused() {
    crate::with_test_registry(scenarios());
}

async fn scenarios() {
    let scratch = crate::TestDir::new("bridge-reg");
    let reg_dir = scratch.0.clone();
    let reg_rt = Runtime::new(figment::Figment::new(), reg_dir.clone());
    let registry = tools::registry::start(&reg_rt).await.unwrap();

    // (b) A name nobody registered: refused with the new text, nothing run.
    let mut h = Harness::new(Fake::tool_call("nope_tool", &json!({}))).await;
    let events = run(&mut h).await;
    assert_eq!(
        finished(&events),
        vec![(
            "call_bridge_1".to_owned(),
            true,
            "nope_tool is not available in this session (not a direct tool and not a runtime function you can see)".to_owned()
        )]
    );
    assert_eq!(started(&events).len(), 1, "no inner call ran: {events:?}");

    let Some(pool) = worker_pool().await else {
        return;
    };

    // (a) `list_dir` is a registered non-direct tool, so with the runtime on
    // it is in the index and not in the direct array: the call runs as the
    // bridge cell under the same call_id, the inner call carries the model's
    // arguments through the repl's registry path, and the result is the
    // function's value behind the note.
    let scratch = reg_dir.join("listed");
    std::fs::create_dir_all(&scratch).unwrap();
    std::fs::write(scratch.join("bridged.txt"), "x").unwrap();
    let model_args = json!({"path": scratch.to_string_lossy()});
    let mut h = Harness::new(Fake::tool_call("list_dir", &model_args)).await;
    h.pool = Some(pool.clone());
    let events = run(&mut h).await;
    let started_calls = started(&events);
    assert_eq!(
        started_calls[0],
        (
            "call_bridge_1".to_owned(),
            "list_dir".to_owned(),
            model_args.clone()
        ),
        "the card opened with the model's own call"
    );
    assert_eq!(
        started_calls[1],
        (
            "call_bridge_1.1".to_owned(),
            "list_dir".to_owned(),
            model_args
        ),
        "the cell awaited the function with the model's arguments"
    );
    let done = finished(&events);
    let outer = done
        .iter()
        .find(|(id, _, _)| id == "call_bridge_1")
        .expect("the outer call finished");
    assert!(!outer.1, "{}", outer.2);
    let note = repl::bridge_note("list_dir");
    let (first, rest) = outer.2.split_once('\n').expect("note then output");
    assert_eq!(first, note);
    assert!(
        rest.starts_with("→ ") && rest.contains("bridged.txt"),
        "the function's value: {rest}"
    );
    // The model sees the same text as its tool result on the next call.
    let requests = h.requests().await;
    let tool_results: Vec<String> = requests
        .last()
        .and_then(|b| b["messages"].as_array())
        .into_iter()
        .flatten()
        .filter(|m| m["role"] == "tool")
        .map(|m| m["content"].as_str().unwrap_or_default().to_owned())
        .collect();
    assert_eq!(tool_results.len(), 1, "{requests:?}");
    assert!(tool_results[0].starts_with(&note), "{}", tool_results[0]);

    // (c) The same name hidden for the session is in neither the index nor
    // the array: refused, not run.
    let mut h = Harness::new(Fake::tool_call("list_dir", &json!({"path": "/"}))).await;
    h.pool = Some(pool.clone());
    registry
        .ask(tools::registry::SetHidden {
            session: h.id,
            tools: vec!["list_dir".into()],
            hidden: true,
        })
        .await
        .unwrap();
    let events = run(&mut h).await;
    assert_eq!(
        finished(&events),
        vec![(
            "call_bridge_1".to_owned(),
            true,
            "list_dir is not available in this session (not a direct tool and not a runtime function you can see)".to_owned()
        )]
    );
    assert_eq!(started(&events).len(), 1, "no inner call ran: {events:?}");

    // read-file-images: a picture read inside a python cell takes the 52 path.
    super::vision_fallback_tests::python_cell_image_takes_the_same_path(pool).await;
}
