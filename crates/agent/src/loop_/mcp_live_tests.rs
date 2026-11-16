//! Network-free MCP lifecycle through real turns, Python cells and captured model requests.
use super::vision_fallback_tests::{Harness, body, sse, sse_tool_call, system};
use super::*;
use serde_json::{Value, json};
use wiremock::{Request, Respond, ResponseTemplate};

const SIGNATURE: &str = "async def live_echo(value: str) -> Any";

fn entry(delay: u64) -> Value {
    json!({"command": "python3", "args": [
        concat!(env!("CARGO_MANIFEST_DIR"), "/../tools/tests/mcp_live.py"),
        delay.to_string()
    ]})
}

fn add(delay: u64) -> String {
    let e = entry(delay);
    format!(
        "added = await add_mcp_server(name='live', command='python3', args={})\nprint(added)",
        e["args"]
    )
}

struct Script(Vec<String>);
impl Respond for Script {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let b = body(req);
        if system(&b).contains("## Runtime") {
            let messages = b["messages"].as_array().unwrap();
            let start = messages.iter().rposition(|m| m["role"] == "user").unwrap();
            let input = messages[start]["content"].to_string();
            let count = messages[start..]
                .iter()
                .filter(|m| m["role"] == "tool")
                .count();
            let remove = [
                "removed = await remove_mcp_server(name='live')\nprint(removed)".to_owned(),
                "await live_echo(value='gone')".to_owned(),
                "print(tools('live_echo'))\nhelp('live_echo')".to_owned(),
            ];
            let cells: &[String] = if input.contains("REMOVE") {
                &remove
            } else if input.contains("INDEX") {
                &[]
            } else {
                &self.0
            };
            if let Some(code) = cells.get(count) {
                return sse_tool_call(
                    &format!("cell_{count}"),
                    "python",
                    &json!({"code": code}).to_string(),
                );
            }
        }
        sse("done", "stop", (10, 5))
    }
}

async fn setup(cells: Vec<String>) -> Option<(Harness, ActorRef<ToolRegistry>)> {
    let Some(worker) =
        std::env::var_os("THEUTH_MONTY_WORKER").or_else(|| std::env::var_os("THEUTH_WORKER"))
    else {
        eprintln!("no THEUTH_MONTY_WORKER/THEUTH_WORKER; skipping MCP Python lifecycle test");
        return None;
    };
    let mut h = Harness::new(Script(cells)).await;
    h.pool = Some(
        repl::open_pool(Path::new(&worker), 1)
            .await
            .expect("Monty worker"),
    );
    let registry = tools::registry::start(&h.rt).await.unwrap();
    registry
        .ask(tools::SetApprovals {
            session: Some(h.id),
            enabled: false,
            clear: false,
        })
        .await
        .unwrap();
    // The preceding test's runtime is gone; kameo may retain its dead name.
    if ActorRef::<tools::Mcp>::lookup(tools::MCP_ACTOR)
        .ok()
        .flatten()
        .is_some_and(|a| !a.is_alive())
    {
        kameo::registry::ACTOR_REGISTRY
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(tools::MCP_ACTOR);
    }
    tools::mcp::start(&h.rt, registry.clone(), tools::Policy::default())
        .await
        .unwrap();
    Some((h, registry))
}

async fn run(h: &mut Harness, input: &str) -> Vec<AgentEvent> {
    let (turn, tid) = h.turn("openai/textonly", CancellationToken::new()).await;
    turn.run(tid, vec![h.input(input, false)]).await;
    h.drain().await
}

fn output(events: &[AgentEvent], id: &str) -> (bool, String) {
    events
        .iter()
        .find_map(|e| match e {
            AgentEvent::ToolCallFinished {
                call_id,
                output,
                is_error,
                ..
            } if call_id == id => Some((*is_error, output.clone())),
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing {id}: {events:?}"))
}

#[test]
fn mcp_live_add_and_remove() {
    crate::with_test_registry(async {
        let Some((mut h, _)) = setup(vec![
            add(0),
            "reply = await live_echo(value='next-cell')\nprint(reply)".into(),
            "print(tools('live_echo'))\nprint(help('live_echo'))".into(),
        ])
        .await
        else {
            return;
        };
        let events = run(&mut h, "ADD").await;
        let added = output(&events, "cell_0");
        assert!(!added.0, "add: {}", added.1);
        let called = output(&events, "cell_1");
        assert!(
            !called.0 && called.1.contains("live:next-cell:"),
            "next cell: {called:?}; add: {added:?}"
        );
        let discovery = output(&events, "cell_2");
        assert!(
            !discovery.0 && discovery.1.contains(SIGNATURE),
            "live tools/help: {discovery:?}"
        );
        run(&mut h, "INDEX").await;
        assert!(
            system(h.requests().await.last().unwrap()).contains(SIGNATURE),
            "next turn index omitted new MCP signature"
        );
        let events = run(&mut h, "REMOVE").await;
        assert!(!output(&events, "cell_0").0, "remove failed");
        let removed = output(&events, "cell_1");
        assert!(
            removed.0 && removed.1.contains("live_echo") && removed.1.contains("not defined"),
            "removed tool must fail type checking, not use a stale stub: {removed:?}"
        );
        let discovery = output(&events, "cell_2");
        assert!(
            discovery.0 && discovery.1.contains("[]") && discovery.1.contains("unknown tool"),
            "removed tools/help: {discovery:?}"
        );
        run(&mut h, "INDEX").await;
        assert!(
            !system(h.requests().await.last().unwrap()).contains(SIGNATURE),
            "next turn index retained removed MCP signature"
        );
    });
}

#[test]
fn mcp_live_add_returns_python_signatures() {
    crate::with_test_registry(async {
        let Some((mut h, _)) = setup(vec![add(0)]).await else {
            return;
        };
        let events = run(&mut h, "ADD").await;
        let added = output(&events, "cell_0");
        assert!(
            !added.0 && added.1.contains(SIGNATURE),
            "add result must carry callable Python signatures in the same turn: {added:?}"
        );
    });
}

#[test]
fn mcp_live_file_watch() {
    crate::with_test_registry(async {
        let Some((mut h, registry)) = setup(vec![
            "reply = await live_echo(value='watched')\nprint(reply)".into(),
        ])
        .await
        else {
            return;
        };
        run(&mut h, "INDEX").await;
        std::fs::write(h.dir.join("mcp/watched.json"), entry(0).to_string()).unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if registry
                    .ask(ListTools(Some(h.id)))
                    .await
                    .unwrap()
                    .iter()
                    .any(|s| s.name == "live_echo")
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("watcher must register new MCP tools within five seconds");
        let events = run(&mut h, "CALL").await;
        let called = output(&events, "cell_0");
        assert!(
            !called.0 && called.1.contains("live:watched:"),
            "watched call: {called:?}"
        );
        assert!(system(h.requests().await.last().unwrap()).contains(SIGNATURE));
    });
}

#[test]
fn mcp_live_reconnect() {
    crate::with_test_registry(async {
        let Some((mut h, _)) = setup(vec![
            add(0),
            "before = await live_echo(value='pid')".into(),
            add(0),
            "after = await live_echo(value='pid')\nassert before != after\nprint('reconnected')"
                .into(),
        ])
        .await
        else {
            return;
        };
        let events = run(&mut h, "ADD").await;
        let again = output(&events, "cell_2");
        assert!(
            !again.0 && again.1.contains("live_echo"),
            "re-adding an existing server must reconnect and list tools: {again:?}"
        );
        let called = output(&events, "cell_3");
        assert!(
            !called.0 && called.1.contains("reconnected"),
            "reconnect must use a new server process: {called:?}"
        );
    });
}

#[test]
fn mcp_live_slow_server_settles_before_add_returns() {
    crate::with_test_registry(async {
        let Some((mut h, registry)) = setup(vec![
            add(3),
            "reply = await live_echo(value='slow')\nprint(reply)".into(),
        ])
        .await
        else {
            return;
        };
        // Talk-live sessions promote ordinary tools; adding a server must
        // still await the bounded handshake rather than return a job stub.
        registry
            .ask(tools::SetPromoteAfter {
                session: h.id,
                after: Some(Duration::from_millis(50)),
            })
            .await
            .unwrap();
        let events = run(&mut h, "ADD").await;
        let added = output(&events, "cell_0");
        assert!(
            !added.0 && added.1.contains("connected"),
            "slow add: {added:?}"
        );
        let called = output(&events, "cell_1");
        assert!(
            !called.0 && called.1.contains("live:slow:"),
            "slow next cell: {called:?}"
        );
        // The actor's 20-second connect ceiling is exercised separately by
        // mcp::tests::mcp_live_add_has_a_bounded_connect_wait; model/worker
        // startup is not part of that deadline.
    });
}
