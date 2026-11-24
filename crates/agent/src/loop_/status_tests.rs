//! 212 §13: `set_status` through a real [`Turn`] against the fake provider of
//! [`super::vision_fallback_tests`]: the direct call's state, events and title guards,
//! a user-named session keeping its title, the title model held off, the status nudge's
//! cadence, and a python cell's `set_status`. One tokio test: the tool registry is a
//! process-global actor bound to the runtime that spawned it. The python scenario needs a
//! worker (`$THEUTH_WORKER`, else `monty`/`t` on `PATH`) and skips without one.

use std::sync::atomic::Ordering::Relaxed;

use serde_json::{Value, json};
use wiremock::{Request, Respond, ResponseTemplate};

use super::runtime_bridge_tests::worker_pool;
use super::vision_fallback_tests::{Fake, Harness, body, sse, sse_tool_call, system};
use super::*;

const NUDGE: &str = "Status check: call set_status";

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

/// `(status, title)` of every `StatusChanged`, in order.
fn statuses(events: &[AgentEvent]) -> Vec<(String, Option<String>)> {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::StatusChanged { status, title, .. } => {
                Some((status.clone(), title.clone()))
            }
            _ => None,
        })
        .collect()
}

/// Titles of every `TitleChanged`, in order.
fn titles(events: &[AgentEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::TitleChanged { title, .. } => Some(title.clone()),
            _ => None,
        })
        .collect()
}

/// The stored title of the harness's session.
async fn stored_title(h: &Harness) -> String {
    h.store.session(h.id).await.unwrap().unwrap().title
}

#[test]
fn set_status_state_events_guards_and_nudges() {
    crate::with_test_registry(scenarios());
}

async fn scenarios() {
    let scratch = crate::test_dir("status-reg");
    let reg_rt = Runtime::new(figment::Figment::new(), scratch.path().to_path_buf());
    tools::registry::start(&reg_rt).await.unwrap();

    direct_call_sets_state_events_and_title().await;
    user_named_session_keeps_its_title().await;
    title_model_is_held_by_either_guard().await;
    suggestions_only_for_interactive_root_chats().await;
    nudge_cadence().await;
    status_tool_off().await;
    if let Some(pool) = worker_pool().await {
        cell_call_updates_the_turn(pool).await;
    }
}

/// §13.1: state, `StatusChanged`, `agent_titled` (live and stored), title + `TitleChanged`.
async fn direct_call_sets_state_events_and_title() {
    let args = json!({"status": "  reading the build lock code ", "title": "Fix the build lock"});
    let mut h = Harness::new(Fake::tool_call(status::NAME, &args)).await;
    let (turn, tid) = h.turn("openai/textonly", CancellationToken::new()).await;
    let shared = turn.shared.clone();
    let state = turn.run(tid, vec![h.input("go", false)]).await;
    let events = h.drain().await;
    assert_eq!(
        finished(&events),
        vec![(
            "call_bridge_1".to_owned(),
            false,
            "status set · title \"Fix the build lock\"".to_owned()
        )]
    );
    assert_eq!(
        statuses(&events),
        vec![(
            "reading the build lock code".to_owned(),
            Some("Fix the build lock".to_owned())
        )]
    );
    assert_eq!(titles(&events), vec!["Fix the build lock".to_owned()]);
    assert_eq!(state.status.as_deref(), Some("reading the build lock code"));
    assert_eq!(state.tool_calls_since_status, 0);
    assert!(shared.agent_titled.load(Relaxed));
    assert!(h.store.title_guard(h.id).await.unwrap().agent_titled);
    assert_eq!(stored_title(&h).await, "Fix the build lock");
}

/// §13.2: a user-named session keeps its title; the status still lands.
async fn user_named_session_keeps_its_title() {
    let args = json!({"status": "checking the lock", "title": "Something else entirely"});
    let mut h = Harness::new(Fake::tool_call(status::NAME, &args)).await;
    h.store.set_title(h.id, "Mine").await.unwrap();
    h.store.set_user_titled(h.id).await.unwrap();
    let (turn, tid) = h.turn("openai/textonly", CancellationToken::new()).await;
    turn.shared.user_titled.store(true, Relaxed);
    let state = turn.run(tid, vec![h.input("go", false)]).await;
    let events = h.drain().await;
    assert_eq!(
        finished(&events),
        vec![(
            "call_bridge_1".to_owned(),
            false,
            "status set · title kept: the user named this session".to_owned()
        )]
    );
    assert_eq!(
        statuses(&events),
        vec![("checking the lock".to_owned(), None)]
    );
    assert!(titles(&events).is_empty(), "{events:?}");
    assert_eq!(state.status.as_deref(), Some("checking the lock"));
    assert_eq!(stored_title(&h).await, "Mine");
    let guard = h.store.title_guard(h.id).await.unwrap();
    assert!(guard.user_titled && guard.agent_titled, "{guard:?}");
}

/// §13.3: a title-due turn runs the title model, unless the agent or the user named the
/// session, or (212 §15) the session started with `agent.auto_title` off.
async fn title_model_is_held_by_either_guard() {
    let input = "Walk me through the datacenter failover drill.";
    let titled = |agent: bool, user: bool, auto_title: bool| async move {
        let mut h = Harness::new(Fake::harvest_held(Duration::ZERO)).await;
        let cfg = AgentConfig {
            auto_title,
            ..AgentConfig::default()
        };
        let (mut turn, tid) = h
            .turn_with("openai/textonly", CancellationToken::new(), cfg)
            .await;
        // `Session::start_turn` counts the user's chars; the harness bypasses it.
        turn.state.title_chars = input.len();
        turn.shared.agent_titled.store(agent, Relaxed);
        turn.shared.user_titled.store(user, Relaxed);
        assert_eq!(turn.shared.title_held(), agent || user);
        let mut state = turn.run(tid, vec![h.input(input, false)]).await;
        h.drain().await;
        let ran = match state.title_task.take() {
            Some(task) => {
                let _ = task.await;
                true
            }
            None => false,
        };
        (state.title_model, ran)
    };
    assert_eq!(
        titled(false, false, true).await,
        (true, true),
        "unguarded: ran"
    );
    assert_eq!(
        titled(true, false, true).await,
        (false, false),
        "agent_titled"
    );
    assert_eq!(
        titled(false, true, true).await,
        (false, false),
        "user_titled"
    );
    assert_eq!(
        titled(false, false, false).await,
        (false, false),
        "auto_title off"
    );
}

/// "What's next?": a root chat's normal turn end asks the title model and publishes
/// `Suggestions` after `TurnFinished`; a child session, an unattended (schedule) session,
/// a channel-opened turn or `agent.suggestions: false` makes no call at all.
async fn suggestions_only_for_interactive_root_chats() {
    #[derive(Clone, Copy, Default)]
    struct Case {
        child: bool,
        off: bool,
        unattended: bool,
        channel: bool,
    }
    let run = |c: Case| async move {
        let mut h = Harness::new(Fake::harvest_held(Duration::ZERO)).await;
        if c.unattended {
            h.store.set_unattended(h.id).await.unwrap();
        }
        let cfg = AgentConfig {
            suggestions: !c.off,
            ..AgentConfig::default()
        };
        let (mut turn, tid) = h
            .turn_with("openai/textonly", CancellationToken::new(), cfg)
            .await;
        if c.child {
            turn.shared.depth = 1;
        }
        turn.channel_opened = c.channel;
        let mut state = turn.run(tid, vec![h.input("Fix the parser", false)]).await;
        let events = h.drain().await;
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, AgentEvent::Suggestions { .. })),
            "never before TurnFinished"
        );
        if let Some(t) = state.title_task.take() {
            let _ = t.await;
        }
        state.suggest_task.take()?.await.unwrap();
        let late = h
            .drain_until(|e| matches!(e, AgentEvent::Suggestions { .. }))
            .await;
        match late.last() {
            Some(AgentEvent::Suggestions {
                session,
                turn,
                items,
            }) => {
                assert_eq!((*session, *turn), (h.id, tid));
                Some(items.clone())
            }
            other => panic!("{other:?}"),
        }
    };
    // The fake answers every side call with `FINAL`: one line, one suggestion.
    assert_eq!(
        run(Case::default()).await,
        Some(vec![super::vision_fallback_tests::FINAL.to_owned()]),
        "interactive root chat"
    );
    let none = |c: Case, why: &'static str| async move { assert_eq!(run(c).await, None, "{why}") };
    none(
        Case {
            child: true,
            ..Case::default()
        },
        "child session",
    )
    .await;
    none(
        Case {
            off: true,
            ..Case::default()
        },
        "agent.suggestions off",
    )
    .await;
    none(
        Case {
            unattended: true,
            ..Case::default()
        },
        "schedule run",
    )
    .await;
    none(
        Case {
            channel: true,
            ..Case::default()
        },
        "channel turn",
    )
    .await;
}

/// The main model's script, by tool results so far: `steps[n]` is the call after `n`
/// results (`None` = the final reply). Every request's messages are kept.
struct Script {
    steps: Vec<Option<(&'static str, Value)>>,
}

impl Respond for Script {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let b = body(req);
        if !system(&b).contains("## Guidance") {
            return sse("ok", "stop", (10, 5));
        }
        let results = b["messages"]
            .as_array()
            .map_or(0, |m| m.iter().filter(|m| m["role"] == "tool").count());
        match self.steps.get(results).cloned().flatten() {
            Some((name, args)) => {
                sse_tool_call(&format!("call_{results}"), name, &args.to_string())
            }
            None => sse("done", "stop", (10, 5)),
        }
    }
}

/// Status nudges each main request carries, in request order.
async fn nudges(steps: Vec<Option<(&'static str, Value)>>, toolset: Option<Toolset>) -> Vec<usize> {
    let mut h = Harness::new(Script { steps }).await;
    let (turn, tid) = h.turn("openai/textonly", CancellationToken::new()).await;
    *turn.shared.toolset.write() = toolset;
    turn.run(tid, vec![h.input("go", false)]).await;
    h.drain().await;
    h.requests()
        .await
        .iter()
        .filter(|b| system(b).contains("## Guidance"))
        .map(|b| b["messages"].to_string().matches(NUDGE).count())
        .collect()
}

/// 212 §15: a session started with `agent.status_tool` off has no `set_status` in its tool
/// array, no status sentence in its prompt and no nudge, though its toolset lists the tool.
async fn status_tool_off() {
    let other = || Some(("nope_tool", json!({})));
    let mut h = Harness::new(Script {
        steps: vec![other(), other(), other(), other(), None],
    })
    .await;
    let cfg = AgentConfig {
        status_tool: false,
        ..AgentConfig::default()
    };
    let (turn, tid) = h
        .turn_with("openai/textonly", CancellationToken::new(), cfg)
        .await;
    *turn.shared.toolset.write() = Some(Toolset {
        tools: vec![status::NAME.to_owned()],
        prompts: vec![],
        runtime: false,
        hint_limit: Some(0),
        status_every: Some(1),
    });
    turn.run(tid, vec![h.input("go", false)]).await;
    h.drain().await;
    let main: Vec<Value> = h
        .requests()
        .await
        .into_iter()
        .filter(|b| system(b).contains("## Guidance"))
        .collect();
    assert_eq!(main.len(), 5);
    for b in &main {
        assert!(
            !b["tools"].to_string().contains(status::NAME),
            "{}",
            b["tools"]
        );
        assert!(!system(b).contains(status::PROMPT));
        assert!(!b["messages"].to_string().contains(NUDGE));
    }
}

/// §13.4: the first nudge after 2 calls without a status, then every `status_every`;
/// `set_status` resets the count; none with `Some(0)` or when the tool is not listed.
async fn nudge_cadence() {
    let other = || Some(("nope_tool", json!({})));
    let set = || Some((status::NAME, json!({"status": "working on it"})));
    let toolset = |tools: &[&str], every: u32| {
        Some(Toolset {
            tools: tools.iter().map(|s| (*s).to_owned()).collect(),
            prompts: vec![],
            runtime: false,
            hint_limit: Some(0),
            status_every: Some(every),
        })
    };
    // r0 → 1 call, r1 → 2: nudge before r2 (counter reset); `set_status` in r2;
    // r3 → 1, r4 → 2, r5 → 3: nudge before r6 only (2 < 3 after the first nudge).
    let steps = || vec![other(), other(), set(), other(), other(), other(), None];
    assert_eq!(
        nudges(steps(), toolset(&[status::NAME], 3)).await,
        [0, 0, 1, 1, 1, 1, 2]
    );
    // An ignored first nudge repeats only after `status_every` more calls.
    let ignored = vec![other(), other(), other(), other(), other(), None];
    assert_eq!(
        nudges(ignored, toolset(&[status::NAME], 3)).await,
        [0, 0, 1, 1, 1, 2]
    );
    assert_eq!(
        nudges(steps(), toolset(&[status::NAME], 0)).await,
        [0; 7],
        "status_every 0"
    );
    assert_eq!(
        nudges(
            vec![other(), other(), other(), other(), None],
            toolset(&["ask_user"], 3)
        )
        .await,
        [0; 5],
        "set_status not listed"
    );
}

/// The main model's script for the cell scenario: a cell that awaits `set_status` with a
/// title, then a final reply.
struct CellScript;

impl Respond for CellScript {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let b = body(req);
        if !system(&b).contains("## Runtime") {
            return sse("ok", "stop", (10, 5));
        }
        let results = b["messages"]
            .as_array()
            .map_or(0, |m| m.iter().filter(|m| m["role"] == "tool").count());
        match results {
            0 => sse_tool_call(
                "call_py",
                repl::NAME,
                &json!({"code": "r = await set_status(status=\"mapping the harness\", title=\"Map the harness\")\nprint(r)\nr = await set_status(status=\"reading  the\\n loop\")\nprint(r)"})
                    .to_string(),
            ),
            // A cell without tool calls: `python` itself does not count (212 §10).
            1 => sse_tool_call(
                "call_py2",
                repl::NAME,
                &json!({"code": "print(1)"}).to_string(),
            ),
            _ => sse("done", "stop", (10, 5)),
        }
    }
}

/// §13.5: `set_status` awaited in a cell updates the turn after the cell: state, counter,
/// events, title, guard; two calls in one cell apply in order, one `StatusChanged` each.
async fn cell_call_updates_the_turn(pool: Arc<Pool>) {
    let mut h = Harness::new(CellScript).await;
    h.pool = Some(pool);
    let (turn, tid) = h.turn("openai/textonly", CancellationToken::new()).await;
    let shared = turn.shared.clone();
    let state = turn.run(tid, vec![h.input("go", false)]).await;
    let events = h.drain().await;
    let done = finished(&events);
    let inner = |id: &str| {
        done.iter()
            .find(|(i, _, _)| i == id)
            .map(|(_, err, out)| (*err, out.clone()))
            .unwrap_or_else(|| panic!("the cell's {id} did not finish: {done:?}"))
    };
    assert_eq!(
        inner("call_py.1"),
        (false, "status set · title \"Map the harness\"".to_owned())
    );
    assert_eq!(inner("call_py.2"), (false, "status set".to_owned()));
    assert_eq!(
        statuses(&events),
        vec![
            (
                "mapping the harness".to_owned(),
                Some("Map the harness".to_owned())
            ),
            ("reading the loop".to_owned(), None),
        ]
    );
    assert_eq!(state.status.as_deref(), Some("reading the loop"));
    assert_eq!(
        state.tool_calls_since_status, 0,
        "reset after the cell; the bare cell adds nothing"
    );
    assert!(state.status_titled);
    assert!(shared.agent_titled.load(Relaxed));
    assert_eq!(stored_title(&h).await, "Map the harness");
}
