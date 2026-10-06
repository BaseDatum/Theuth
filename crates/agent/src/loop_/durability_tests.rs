//! Design doc 109 (W1): the journal a turn writes, the resume state machine
//! of §4.2 and the §11.1 "abort at step X, reopen, resume" matrix, through a
//! real [`Turn`] against the fake provider of [`super::vision_fallback_tests`].
//! The provider answers the first main call of a turn with a batch of two
//! registry tools — `probe` (read-only) and `poke` (mutating) — and every
//! call carrying a tool message with [`FINAL`]. The fake tools append their
//! name to a hits file, so invocations count across processes.
//!
//! The crash matrix spawns this test binary as a child (`crash_child`, an
//! ignored test driven by env) with `THEUTH_CRASH_AT` set: the contract's
//! `crash_point` aborts the process, there is no in-process hook. The parent
//! then reopens the same store, resumes, and compares transcripts with an
//! uninterrupted run. One tokio test runs every scenario in sequence: the
//! tool registry is a process-global actor bound to the runtime that spawned it.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures::future::BoxFuture;
use serde_json::{Value, json};
use tools::{ErasedTool, ToolCx, ToolSpec};
use wiremock::{Request, Respond, ResponseTemplate};

use super::vision_fallback_tests::{FINAL, Harness, MAIN_USAGE, body, sse};
use super::*;
use crate::durability::journal::StoreJournal;
use crate::durability::{CallState, LastStep};

const PROBE: &str = "probe";
const POKE: &str = "poke";
/// Call ids the fake provider mints for the batch.
const C1: &str = "call_probe_1";
const C2: &str = "call_poke_2";
/// A prompt containing this gets [`FINAL`] at once, no batch.
const TALK: &str = "just talk";

/// The fake provider: a batch on the first main call of a turn, [`FINAL`]
/// once a tool message is in the request (or the prompt says [`TALK`]).
struct BatchFake;

impl Respond for BatchFake {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let b = body(req);
        let msgs = b["messages"].as_array().cloned().unwrap_or_default();
        let answered = msgs.iter().any(|m| m["role"] == "tool");
        let talk = msgs
            .iter()
            .filter(|m| m["role"] == "user")
            .filter_map(|m| m["content"].as_str())
            .any(|t| t.contains(TALK));
        if answered || talk {
            return sse(FINAL, "stop", MAIN_USAGE);
        }
        let chunk = |choices: Value| json!({"id": "c", "object": "chat.completion.chunk", "model": "m", "choices": choices});
        let call = |i: u32, id: &str, name: &str, args: &str| {
            chunk(
                json!([{"index": 0, "delta": {"role": "assistant", "tool_calls": [
                {"index": i, "id": id, "type": "function", "function": {"name": name, "arguments": args}}
            ]}, "finish_reason": null}]),
            )
        };
        let body = format!(
            "data: {}\n\ndata: {}\n\ndata: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
            call(0, C1, PROBE, "{\"what\":\"x\"}"),
            call(1, C2, POKE, "{\"what\":\"y\"}"),
            chunk(json!([{"index": 0, "delta": {}, "finish_reason": "tool_calls"}])),
            json!({"id": "c", "object": "chat.completion.chunk", "model": "m", "choices": [],
                "usage": {"prompt_tokens": MAIN_USAGE.0, "completion_tokens": MAIN_USAGE.1,
                          "total_tokens": MAIN_USAGE.0 + MAIN_USAGE.1}}),
        );
        ResponseTemplate::new(200).set_body_raw(body, "text/event-stream")
    }
}

/// A registry tool that records each invocation in `hits` (one line per
/// call), optionally waits for another tool's line first, optionally aborts
/// at a crash point mid-body, optionally blocks forever (`hold`).
struct FakeTool {
    name: &'static str,
    risk: Risk,
    hits: PathBuf,
    /// Proceed only once this name is in `hits` (ordering across parallel calls).
    after: Option<&'static str>,
    /// `crash_point(..)` inside the body (the §11.1 `call.<i>.mid` point).
    crash_mid: Option<String>,
    /// Never return: the call stays `started` (in-process interruption).
    hold: Arc<AtomicBool>,
}

fn hits_of(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

impl ErasedTool for FakeTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name.into(),
            description: "fake".into(),
            schema: json!({"type": "object", "properties": {"what": {"type": "string"}}}),
            risk: self.risk,
            direct: true,
            returns: "str".into(),
        }
    }

    fn call(&self, _: Value, _: ToolCx) -> BoxFuture<'_, anyhow::Result<ToolOutput>> {
        Box::pin(async move {
            if let Some(after) = self.after {
                for _ in 0..100 {
                    if hits_of(&self.hits).iter().any(|h| h == after) {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            }
            let mut text = std::fs::read_to_string(&self.hits).unwrap_or_default();
            text.push_str(self.name);
            text.push('\n');
            std::fs::write(&self.hits, text).unwrap();
            if let Some(p) = &self.crash_mid {
                crash_point(p);
            }
            if self.hold.load(Ordering::SeqCst) {
                std::future::pending::<()>().await;
            }
            Ok(ToolOutput::ok(format!("{} done", self.name)))
        })
    }
}

/// Which fake tool blocks forever when set: `(probe, poke)`.
type Holds = (Arc<AtomicBool>, Arc<AtomicBool>);

/// The registry with the policy every risk may run, `probe` and `poke`
/// registered over `hits`, and their hold switches.
async fn registry(
    dir: &Path,
    hits: &Path,
    crash_mid: Option<String>,
) -> (ActorRef<ToolRegistry>, Holds) {
    let cfg = figment::Figment::new().merge(figment::providers::Serialized::defaults(json!({
        "tools": {"policy": {"read_only": "allow", "mutating": "allow", "destructive": "allow"}}
    })));
    let reg_rt = Runtime::new(cfg, dir.to_path_buf());
    let registry = tools::registry::start(&reg_rt).await.unwrap();
    let holds: Holds = (Arc::default(), Arc::default());
    for (name, risk, after, crash, hold) in [
        (PROBE, Risk::ReadOnly, None, None, &holds.0),
        (POKE, Risk::Mutating, Some(PROBE), crash_mid, &holds.1),
    ] {
        registry
            .ask(tools::registry::RegisterTool(Arc::new(FakeTool {
                name,
                risk,
                hits: hits.to_path_buf(),
                after,
                crash_mid: crash,
                hold: hold.clone(),
            })))
            .await
            .unwrap();
    }
    (registry, holds)
}

/// Install the store-backed journal on the registry (what `supervisor::start` does).
async fn journal(registry: &ActorRef<ToolRegistry>, store: &Store) {
    registry
        .ask(tools::InstallJournal(Arc::new(StoreJournal(Arc::new(
            store.clone(),
        )))))
        .await
        .unwrap();
}

/// `turn_steps.kind` of the turn, in `n` order.
async fn kinds(store: &Store, session: SessionId, turn: TurnId) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT kind FROM turn_steps WHERE session_id = ? AND turn_id = ? ORDER BY n",
    )
    .bind(session.to_string())
    .bind(turn.to_string())
    .fetch_all(store.pool())
    .await
    .unwrap()
}

/// The session's one open turn.
async fn open_turn(store: &Store, session: SessionId) -> TurnId {
    store
        .open_turns()
        .await
        .unwrap()
        .into_iter()
        .find(|(s, _, _)| *s == session)
        .map(|(_, t, _)| t)
        .expect("an open turn")
}

/// The transcript with volatile text removed: `<time …/>` stamps and the
/// `<engine_restart …/>` note (109 §11.1 "modulo"), as `(role, text)`.
fn normalised(messages: &[(i64, ChatMessage)]) -> Vec<(String, String)> {
    messages
        .iter()
        .map(|(_, m)| {
            let text = text_of(&m.content)
                .lines()
                .filter(|l| !l.starts_with("<time ") && !l.starts_with("<engine_restart "))
                .collect::<Vec<_>>()
                .join("\n");
            (format!("{:?}", m.role), text)
        })
        .collect()
}

fn tool_texts(messages: &[(i64, ChatMessage)]) -> Vec<String> {
    messages
        .iter()
        .filter(|(_, m)| m.role == ChatRole::Tool)
        .map(|(_, m)| text_of(&m.content))
        .collect()
}

fn report(events: &[AgentEvent]) -> (Vec<String>, Vec<String>) {
    events
        .iter()
        .find_map(|e| match e {
            AgentEvent::ResumeReport {
                replayed,
                interrupted,
                ..
            } => Some((
                replayed.iter().map(|(id, _)| id.clone()).collect(),
                interrupted.iter().map(|(id, _)| id.clone()).collect(),
            )),
            _ => None,
        })
        .expect("ResumeReport")
}

#[test]
fn journal_and_resume_contract() {
    crate::with_test_registry(scenarios());
}

async fn scenarios() {
    let reg_dir = std::env::temp_dir().join(format!("theuth-109-reg-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&reg_dir).unwrap();
    let hits = reg_dir.join("hits");
    let (registry, holds) = registry(&reg_dir, &hits, None).await;

    let reference = normal_turn_journals_every_write_point(&registry, &hits).await;
    interrupted_mutating_call_is_reported_not_rerun(&registry, &hits, &holds, &reference).await;
    interrupted_read_only_call_runs_again(&registry, &hits, &holds, &reference).await;
    open_model_call_drops_the_partial_and_reissues(&registry, &hits).await;
    python_rule_both_ways(&registry).await;
    stale_batch_closes_from_the_journal_and_pre_0033_repairs(&registry, &hits).await;
    spared_child_results_are_listed().await;
    crash_matrix(&registry, &hits, &reference).await;

    let _ = std::fs::remove_dir_all(&reg_dir);
}

/// §3.3: every write point leaves its row; the batch message is the rows.
async fn normal_turn_journals_every_write_point(
    registry: &ActorRef<ToolRegistry>,
    hits: &Path,
) -> Vec<(String, String)> {
    let _ = std::fs::remove_file(hits);
    let mut h = Harness::new(BatchFake).await;
    journal(registry, &h.store).await;
    let (turn, tid) = h.turn("openai/textonly", CancellationToken::new()).await;
    turn.run(tid, vec![h.input("go", false)]).await;
    let events = h.drain().await;
    assert_eq!(
        kinds(&h.store, h.id, tid).await,
        [
            "input_accepted",
            "model_call",
            "tool_batch",
            "model_call",
            "turn_finished"
        ]
    );
    let rows = h.store.calls_of(h.id, tid).await.unwrap();
    assert_eq!(rows.len(), 2);
    for r in &rows {
        assert_eq!(r.state, CallState::Finished, "{r:?}");
        assert_eq!(
            r.output.as_deref(),
            Some(format!("{} done", r.name).as_str())
        );
        assert_eq!(r.attempt, 1);
    }
    assert_eq!(rows[0].risk, Risk::ReadOnly);
    assert_eq!(rows[1].risk, Risk::Mutating);
    let (stop, _) = h.store.last_stop(h.id).await.unwrap().expect("last_stop");
    assert_eq!(stop, "stop");
    assert!(h.store.open_turns().await.unwrap().is_empty());
    assert_eq!(hits_of(hits), [PROBE, POKE]);
    let messages = h.store.messages(h.id).await.unwrap();
    let tools = tool_texts(&messages);
    assert_eq!(tools.len(), 1);
    assert!(
        tools[0].contains("probe done") && tools[0].contains("poke done"),
        "{}",
        tools[0]
    );
    assert!(events.iter().any(
        |e| matches!(e, AgentEvent::TurnFinished { stop_reason, .. } if stop_reason == "stop")
    ));
    normalised(&messages)
}

/// Drop the running turn while `held` is `started` and the other call is
/// `finished` (the in-process shape of a kill), ready for a fresh `Turn` over
/// the same store to resume.
async fn interrupt_mid_batch(
    registry: &ActorRef<ToolRegistry>,
    hits: &Path,
    hold: &AtomicBool,
    held: &str,
    done: &str,
) -> (Harness, TurnId) {
    let _ = std::fs::remove_file(hits);
    let h = Harness::new(BatchFake).await;
    journal(registry, &h.store).await;
    hold.store(true, Ordering::SeqCst);
    let (turn, tid) = h.turn("openai/textonly", CancellationToken::new()).await;
    let input = h.input("go", false);
    let running = tokio::spawn(async move { turn.run(tid, vec![input]).await });
    for _ in 0..200 {
        let rows = h.store.calls_of(h.id, tid).await.unwrap();
        if rows
            .iter()
            .any(|r| r.call_id == held && r.state == CallState::Started)
            && rows
                .iter()
                .any(|r| r.call_id == done && r.state == CallState::Finished)
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    running.abort();
    let _ = running.await;
    hold.store(false, Ordering::SeqCst);
    let plan = h.store.resume_plan(h.id, tid).await.unwrap();
    assert_eq!(
        plan.last,
        LastStep::ToolBatch {
            all_finished: false
        }
    );
    (h, tid)
}

/// §4.2: `started` + mutating → `interrupted` with the §4.3 note; the finished
/// read-only result is reused; the restart note leads the message.
async fn interrupted_mutating_call_is_reported_not_rerun(
    registry: &ActorRef<ToolRegistry>,
    hits: &Path,
    holds: &Holds,
    reference: &[(String, String)],
) {
    let (mut h, tid) = interrupt_mid_batch(registry, hits, &holds.1, C2, C1).await;
    let plan = h.store.resume_plan(h.id, tid).await.unwrap();
    let (turn, _) = h.turn("openai/textonly", CancellationToken::new()).await;
    turn.resume(plan, None).await;
    let events = h.drain().await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::TurnResumed { turn, .. } if *turn == tid))
    );
    let (replayed, interrupted) = report(&events);
    assert!(replayed.is_empty());
    assert_eq!(interrupted, [C2]);
    let rows = h.store.calls_of(h.id, tid).await.unwrap();
    let poke = rows.iter().find(|r| r.call_id == C2).unwrap();
    assert_eq!(poke.state, CallState::Interrupted);
    assert!(poke.is_error);
    let note = poke.output.as_deref().unwrap();
    assert!(
        note.starts_with("interrupted: the engine restarted at ")
            && note.contains(" while this call was running (started ")
            && note.ends_with("; its effect is unknown — check before repeating it."),
        "{note}"
    );
    assert_eq!(
        hits_of(hits),
        [PROBE, POKE],
        "poke ran once across both lives"
    );
    let messages = h.store.messages(h.id).await.unwrap();
    let tools = tool_texts(&messages);
    assert_eq!(tools.len(), 1);
    let first = tools[0].lines().next().unwrap();
    assert!(
        first.starts_with("<engine_restart at=\"")
            && first.contains("kind=\"crash\" replayed=\"0\" interrupted=\"1\"/>"),
        "{first}"
    );
    assert!(tools[0].contains("probe done"), "{}", tools[0]);
    assert!(tools[0].contains(&format!("ERROR: {note}")), "{}", tools[0]);
    assert_eq!(
        kinds(&h.store, h.id, tid).await.last().unwrap(),
        "turn_finished"
    );
    // The transcript equals the uninterrupted run's modulo the note.
    let got = normalised(&messages);
    assert_eq!(got.len(), reference.len());
    for (g, r) in got.iter().zip(reference) {
        assert_eq!(g.0, r.0);
        if g.0 != "Tool" {
            assert_eq!(g.1, r.1);
        }
    }
}

/// §4.2: `started` + read-only → runs again as attempt 2.
async fn interrupted_read_only_call_runs_again(
    registry: &ActorRef<ToolRegistry>,
    hits: &Path,
    holds: &Holds,
    reference: &[(String, String)],
) {
    let (mut h, tid) = interrupt_mid_batch(registry, hits, &holds.0, C1, C2).await;
    let plan = h.store.resume_plan(h.id, tid).await.unwrap();
    let (turn, _) = h.turn("openai/textonly", CancellationToken::new()).await;
    turn.resume(plan, None).await;
    let events = h.drain().await;
    let (replayed, interrupted) = report(&events);
    assert_eq!(replayed, [C1]);
    assert!(interrupted.is_empty());
    let rows = h.store.calls_of(h.id, tid).await.unwrap();
    let probe = rows.iter().find(|r| r.call_id == C1).unwrap();
    assert_eq!((probe.state, probe.attempt), (CallState::Finished, 2));
    assert_eq!(hits_of(hits), [PROBE, POKE, PROBE]);
    let messages = h.store.messages(h.id).await.unwrap();
    assert_eq!(normalised(&messages), reference);
}

/// §4.2 row 2 / §8.1: an open `model_call` is re-issued as `attempt + 1` of
/// the same request; its partial is published as dropped and deleted.
async fn open_model_call_drops_the_partial_and_reissues(
    registry: &ActorRef<ToolRegistry>,
    hits: &Path,
) {
    let _ = std::fs::remove_file(hits);
    let mut h = Harness::new(BatchFake).await;
    journal(registry, &h.store).await;
    let tid = TurnId::new();
    let model = h.catalog.resolve("openai/textonly", &|_| true).unwrap();
    h.store
        .append_input(h.id, tid, &ChatMessage::user(TALK), None, "user")
        .await
        .unwrap();
    let call = h
        .store
        .reserve_journaled_call(
            h.id,
            tid,
            "req-1",
            0,
            CallKind::Round,
            Some("openai"),
            Some(&model.id),
        )
        .await
        .unwrap();
    h.store.mark_dispatched(&call).await.unwrap();
    h.store
        .partial_upsert(
            h.id,
            tid,
            &call.0,
            &MessageContent::from_text("The squ"),
            None,
        )
        .await
        .unwrap();
    h.store.unresolve_open_calls().await.unwrap();
    let plan = h.store.resume_plan(h.id, tid).await.unwrap();
    assert_eq!(
        plan.last,
        LastStep::ModelCall {
            request_id: "req-1".into(),
            attempt: 0
        }
    );
    assert!(plan.partial.is_some());
    let (turn, _) = h.turn("openai/textonly", CancellationToken::new()).await;
    turn.resume(plan, None).await;
    let events = h.drain().await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::PartialDropped { call_id, .. } if *call_id == call.0)),
        "{events:?}"
    );
    assert!(events.iter().any(|e| matches!(
        e,
        AgentEvent::TurnResumed {
            partial_dropped: true,
            ..
        }
    )));
    let attempts: Vec<(String, i64, String)> = sqlx::query_as(
        "SELECT request_id, attempt, status FROM model_calls WHERE owner_id = ? ORDER BY attempt",
    )
    .bind(h.id.to_string())
    .fetch_all(h.store.pool())
    .await
    .unwrap();
    assert_eq!(attempts.len(), 2, "{attempts:?}");
    assert_eq!(
        (
            attempts[0].0.as_str(),
            attempts[0].1,
            attempts[0].2.as_str()
        ),
        ("req-1", 0, "unresolved")
    );
    assert_eq!(
        (
            attempts[1].0.as_str(),
            attempts[1].1,
            attempts[1].2.as_str()
        ),
        ("req-1", 1, "settled")
    );
    let steps = kinds(&h.store, h.id, tid).await;
    assert_eq!(
        steps,
        [
            "input_accepted",
            "model_call",
            "model_call",
            "turn_finished"
        ]
    );
    let messages = h.store.messages(h.id).await.unwrap();
    assert_eq!(text_of(&messages.last().unwrap().1.content), FINAL);
    assert!(hits_of(hits).is_empty(), "no tool ran");
}

/// §4.2 python rule: all inner calls read-only → the cell is re-planned as
/// attempt 2; a mutating inner call unfinished → the cell note and U closed.
async fn python_rule_both_ways(registry: &ActorRef<ToolRegistry>) {
    let h = Harness::new(BatchFake).await;
    journal(registry, &h.store).await;
    let tid = TurnId::new();
    let cell = |id: &str| ToolCall {
        call_id: id.into(),
        fn_name: repl::NAME.into(),
        fn_arguments: json!({"code": "x"}),
        thought_signatures: None,
    };
    h.store
        .append_input(h.id, tid, &ChatMessage::user("go"), None, "user")
        .await
        .unwrap();
    let msg = ChatMessage::from(vec![cell("py1"), cell("py2")]);
    h.store
        .append_tool_use(
            h.id,
            tid,
            &msg,
            None,
            0,
            &[(cell("py1"), Risk::ReadOnly), (cell("py2"), Risk::ReadOnly)],
        )
        .await
        .unwrap();
    for id in ["py1", "py2"] {
        assert!(h.store.call_started(h.id, id).await.unwrap());
    }
    // py1: two read-only inner calls, one finished, one not.
    h.store
        .inner_call_started(h.id, "py1.1", PROBE, &json!({}))
        .await
        .unwrap();
    h.store
        .inner_call_finished(h.id, "py1.1", "ok", false, &[])
        .await
        .unwrap();
    h.store
        .inner_call_started(h.id, "py1.2", PROBE, &json!({}))
        .await
        .unwrap();
    // py2: a finished read-only call and a mutating one still running.
    h.store
        .inner_call_started(h.id, "py2.1", PROBE, &json!({}))
        .await
        .unwrap();
    h.store
        .inner_call_finished(h.id, "py2.1", "ok", false, &[])
        .await
        .unwrap();
    h.store
        .inner_call_started(h.id, "py2.2", POKE, &json!({}))
        .await
        .unwrap();
    let plan = h.store.resume_plan(h.id, tid).await.unwrap();
    let (mut turn, _) = h.turn("openai/textonly", CancellationToken::new()).await;
    turn.risks = [
        (PROBE.to_owned(), Risk::ReadOnly),
        (POKE.to_owned(), Risk::Mutating),
    ]
    .into_iter()
    .collect();
    let rows = plan.calls.clone();
    let (preset, replay) = turn.resume_python(&rows[0], &plan).await;
    assert!(preset.is_none() && replay, "all read-only: run again");
    let (preset, replay) = turn.resume_python(&rows[1], &plan).await;
    assert!(!replay);
    let note = preset.expect("cell note").content;
    assert!(
        note.starts_with("RuntimeError: the engine restarted at ")
            && note.contains("state restored to after the previous cell — this cell's effects are lost from the runtime, not from the world. Inner calls that finished before the restart: probe (ok). Started, outcome unknown: poke."),
        "{note}"
    );
    let rows = h.store.calls_of(h.id, tid).await.unwrap();
    assert_eq!((rows[0].state, rows[0].attempt), (CallState::Planned, 2));
    assert_eq!(rows[1].state, CallState::Interrupted);
    assert_eq!(rows[1].output.as_deref(), Some(note.as_str()));
    let inner = h.store.inner_calls_of(h.id, "py2").await.unwrap();
    assert_eq!(
        inner[1].result,
        Some((INNER_INTERRUPTED.to_owned(), true)),
        "U closed"
    );
    assert!(
        h.store.inner_calls_of(h.id, "py1").await.unwrap()[1]
            .result
            .is_none(),
        "a re-run cell's inner rows are left alone"
    );
}

/// §4.1 / §4.6: a fresh turn on a session with an open batch closes it from
/// the journal (no re-run, the note, `turn_finished(superseded)`); a dangling
/// `tool_use` without rows (pre-0033) gets `repair_dangling`'s message.
async fn stale_batch_closes_from_the_journal_and_pre_0033_repairs(
    registry: &ActorRef<ToolRegistry>,
    hits: &Path,
) {
    let _ = std::fs::remove_file(hits);
    // Journaled: an open batch, probe finished, poke started.
    let mut h = Harness::new(BatchFake).await;
    journal(registry, &h.store).await;
    let old = TurnId::new();
    let call = |id: &str, name: &str| ToolCall {
        call_id: id.into(),
        fn_name: name.into(),
        fn_arguments: json!({"what": "x"}),
        thought_signatures: None,
    };
    h.store
        .append_input(h.id, old, &ChatMessage::user("go"), None, "user")
        .await
        .unwrap();
    h.store
        .append_tool_use(
            h.id,
            old,
            &ChatMessage::from(vec![call(C1, PROBE), call(C2, POKE)]),
            None,
            0,
            &[
                (call(C1, PROBE), Risk::ReadOnly),
                (call(C2, POKE), Risk::Mutating),
            ],
        )
        .await
        .unwrap();
    h.store.call_started(h.id, C1).await.unwrap();
    h.store
        .call_finished(h.id, C1, &ToolOutput::ok("probe done"))
        .await
        .unwrap();
    h.store.call_started(h.id, C2).await.unwrap();
    let (turn, tid) = h.turn("openai/textonly", CancellationToken::new()).await;
    turn.run(tid, vec![h.input(TALK, false)]).await;
    h.drain().await;
    let messages = h.store.messages(h.id).await.unwrap();
    let tools = tool_texts(&messages);
    assert_eq!(tools.len(), 1);
    assert!(tools[0].starts_with("<engine_restart "), "{}", tools[0]);
    assert!(tools[0].contains("probe done"), "{}", tools[0]);
    assert!(
        tools[0].contains("ERROR: interrupted: the engine restarted at "),
        "{}",
        tools[0]
    );
    assert!(hits_of(hits).is_empty(), "nothing re-ran");
    assert_eq!(
        kinds(&h.store, h.id, old).await.last().unwrap(),
        "turn_finished"
    );
    assert!(h.store.open_turns().await.unwrap().is_empty());
    let rows = h.store.calls_of(h.id, old).await.unwrap();
    assert_eq!(rows[1].state, CallState::Interrupted);

    // Pre-0033: the same dangling row, no journal at all.
    let mut h = Harness::new(BatchFake).await;
    journal(registry, &h.store).await;
    h.store
        .append(h.id, &ChatMessage::user("go"))
        .await
        .unwrap();
    h.store
        .append(
            h.id,
            &ChatMessage::from(vec![call(C1, PROBE), call(C2, POKE)]),
        )
        .await
        .unwrap();
    let (turn, tid) = h.turn("openai/textonly", CancellationToken::new()).await;
    turn.run(tid, vec![h.input(TALK, false)]).await;
    h.drain().await;
    let messages = h.store.messages(h.id).await.unwrap();
    let tools = tool_texts(&messages);
    assert_eq!(tools.len(), 1);
    assert_eq!(
        tools[0],
        "ERROR: not run: the harness exited before this tool completed".repeat(2),
        "both calls answered with the verbatim pre-0033 text"
    );
    assert!(hits_of(hits).is_empty());
}

/// §7.2: a child's `result:` exchange survives `fail_interrupted` and is what
/// the boot pass lists; a plain exchange is failed and not listed.
async fn spared_child_results_are_listed() {
    use crate::store::communication::AcceptExchange;
    let store = Store::open_memory().await.unwrap();
    let parent = SessionId::new();
    let child = SessionId::new();
    for id in [parent, child] {
        store
            .create_session(id, "openai/textonly", None, None, None)
            .await
            .unwrap();
    }
    let accept = |request_id: &'static str, sender: SessionId, recipient: SessionId| {
        let store = store.clone();
        async move {
            store
                .accept_exchange(&AcceptExchange {
                    sender: &format!("session:{sender}"),
                    recipient: &format!("session:{recipient}"),
                    sender_session: Some(sender),
                    recipient_session: Some(recipient),
                    request_id: Some(request_id),
                    reply_to: None,
                    request_scope: None,
                    generation: 1,
                    body: "done",
                })
                .await
                .unwrap()
        }
    };
    let result = accept("result:t1", child, parent).await;
    let plain = accept("msg-1", parent, child).await;
    let failed = store
        .fail_interrupted("interrupted_by_restart")
        .await
        .unwrap();
    assert_eq!(
        failed.exchanges.iter().map(|r| r.id).collect::<Vec<_>>(),
        [plain.id]
    );
    let spared = store.spared_child_results().await.unwrap();
    assert_eq!(spared.len(), 1);
    assert_eq!(spared[0].receipt.id, result.id);
    assert_eq!(spared[0].from, format!("session:{child}"));
    assert_eq!(spared[0].receipt.to, format!("session:{parent}"));
}

/// `THEUTH_CRASH_AT=<point>` as the §11.1 matrix: the child (this binary,
/// `crash_child`) runs the turn until the point aborts it; the parent reopens
/// the store, resumes and checks the transcript against `reference`.
async fn crash_matrix(
    registry: &ActorRef<ToolRegistry>,
    hits: &Path,
    reference: &[(String, String)],
) {
    let mid = crash::call_point(1, crash::CALL_MID);
    // (point, probe hits, poke hits, interrupted call ids)
    let matrix: [(&str, &[usize], usize, &[&str]); 4] = [
        (crash::BATCH_AFTER_TOOL_USE_PERSIST, &[1], 1, &[]),
        (mid.as_str(), &[1, 2], 1, &[C2]),
        (crash::BATCH_BEFORE_MESSAGE, &[1], 1, &[]),
        (crash::TURN_BEFORE_FINISHED, &[1], 1, &[]),
    ];
    for (point, probe_hits, poke_hits, interrupted) in matrix {
        let _ = std::fs::remove_file(hits);
        let h = Harness::new(BatchFake).await;
        journal(registry, &h.store).await;
        let status = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "loop_::durability_tests::crash_child",
                "--include-ignored",
                "--nocapture",
            ])
            .env("THEUTH_CRASH_AT", point)
            .env("THEUTH_109_DIR", &h.dir)
            .env("THEUTH_109_ENDPOINT", h.endpoint())
            .env("THEUTH_109_SESSION", h.id.to_string())
            .env("THEUTH_109_HITS", hits)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await
            .unwrap();
        assert!(
            !status.success(),
            "{point}: the child must abort, got {status}"
        );
        // The next life opens the store anew (`Store::open` reads the log's
        // seq; the first harness's handle predates the child's writes). The
        // first harness stays alive for its provider and `dir`.
        let server = h;
        let mut h = Harness::attach(server.dir.clone(), &server.endpoint(), server.id).await;
        journal(registry, &h.store).await;
        // Boot: accounting marks the lost attempt, then the scan finds the turn.
        h.store.unresolve_open_calls().await.unwrap();
        let tid = open_turn(&h.store, h.id).await;
        let plan = h.store.resume_plan(h.id, tid).await.unwrap();
        let (turn, _) = h.turn("openai/textonly", CancellationToken::new()).await;
        turn.resume(plan, None).await;
        let events = h.drain().await;
        let (_, got_interrupted) = report(&events);
        assert_eq!(got_interrupted, interrupted, "{point}");
        let hit = hits_of(hits);
        let probe = hit.iter().filter(|h| *h == PROBE).count();
        let poke = hit.iter().filter(|h| *h == POKE).count();
        assert!(
            probe_hits.contains(&probe),
            "{point}: probe ran {probe}x: {hit:?}"
        );
        assert_eq!(poke, poke_hits, "{point}: poke ran {poke}x: {hit:?}");
        let rows = h.store.calls_of(h.id, tid).await.unwrap();
        assert!(
            rows.iter().all(|r| r.state.is_terminal()),
            "{point}: {rows:?}"
        );
        assert_eq!(
            kinds(&h.store, h.id, tid).await.last().unwrap(),
            "turn_finished",
            "{point}"
        );
        let partials: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM turn_steps WHERE session_id = ? AND kind = 'assistant_partial'",
        )
        .bind(h.id.to_string())
        .fetch_one(h.store.pool())
        .await
        .unwrap();
        assert_eq!(partials, 0, "{point}");
        assert!(h.store.last_stop(h.id).await.unwrap().is_some(), "{point}");
        let messages = h.store.messages(h.id).await.unwrap();
        let got = normalised(&messages);
        assert_eq!(got.len(), reference.len(), "{point}: {got:?}");
        for (g, r) in got.iter().zip(reference) {
            assert_eq!(g.0, r.0, "{point}");
            if g.0 == "Tool" && !interrupted.is_empty() {
                assert!(
                    g.1.contains("ERROR: interrupted: the engine restarted at "),
                    "{point}: {}",
                    g.1
                );
                assert!(g.1.contains("probe done"), "{point}: {}", g.1);
            } else {
                assert_eq!(g.1, r.1, "{point}");
            }
        }
        if point == crash::TURN_BEFORE_FINISHED {
            let attempts: Vec<(String, i64, String)> = sqlx::query_as(
                "SELECT request_id, attempt, status FROM model_calls WHERE owner_id = ? AND turn_id = ? ORDER BY created_at",
            )
            .bind(h.id.to_string())
            .bind(tid.to_string())
            .fetch_all(h.store.pool())
            .await
            .unwrap();
            let last = &attempts[attempts.len() - 1];
            let lost = &attempts[attempts.len() - 2];
            // The kill fell after the stream's `End` settled the attempt and
            // before the reply persisted: the row is `settled`; a kill
            // mid-stream leaves `unresolved` (boot's `unresolve_open_calls`).
            assert!(
                lost.1 == 0 && matches!(lost.2.as_str(), "settled" | "unresolved"),
                "{attempts:?}"
            );
            assert_eq!(
                (last.0 == lost.0, last.1, last.2.as_str()),
                (true, 1, "settled"),
                "{attempts:?}"
            );
        }
    }
}

/// The crashing life of [`crash_matrix`]: driven by env, a no-op otherwise.
#[test]
#[ignore = "spawned by crash_matrix with THEUTH_109_* set"]
fn crash_child() {
    let Some(dir) = std::env::var_os("THEUTH_109_DIR") else {
        return;
    };
    let endpoint = std::env::var("THEUTH_109_ENDPOINT").unwrap();
    let session: SessionId = std::env::var("THEUTH_109_SESSION")
        .unwrap()
        .parse()
        .unwrap();
    let hits = PathBuf::from(std::env::var_os("THEUTH_109_HITS").unwrap());
    crate::with_test_registry(async move {
        let reg_dir = std::env::temp_dir().join(format!("theuth-109-child-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&reg_dir).unwrap();
        let (registry, _) =
            registry(&reg_dir, &hits, Some(crash::call_point(1, crash::CALL_MID))).await;
        let h = Harness::attach(PathBuf::from(dir), &endpoint, session).await;
        journal(&registry, &h.store).await;
        let (turn, tid) = h.turn("openai/textonly", CancellationToken::new()).await;
        turn.run(tid, vec![h.input("go", false)]).await;
        let _ = std::fs::remove_dir_all(&reg_dir);
    });
}
