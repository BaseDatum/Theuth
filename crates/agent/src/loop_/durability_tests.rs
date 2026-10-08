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
use crate::durability::{CallState, LastStep, ResumeKind};
use crate::store::Scope;

const PROBE: &str = "probe";
const POKE: &str = "poke";
/// Call ids the fake provider mints for the batch.
const C1: &str = "call_probe_1";
const C2: &str = "call_poke_2";
/// The loop-local third call of [`BatchFake3`].
const C3: &str = "call_todo_3";
/// A prompt containing this gets [`FINAL`] at once, no batch.
const TALK: &str = "just talk";

/// The fake provider: a batch on the first main call of a turn, [`FINAL`]
/// once a tool message is in the request (or the prompt says [`TALK`]).
struct BatchFake;

/// [`BatchFake`] with a third, loop-local call (`todo`) after the two
/// registry tools: the W6 matrix's `call.2.*` points are the loop's own
/// `started` / `finished` writes (§3.3 steps 6–7), which registry tools
/// make inside the registry.
struct BatchFake3;

impl Respond for BatchFake {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        batch_response(req, false)
    }
}

impl Respond for BatchFake3 {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        batch_response(req, true)
    }
}

fn batch_response(req: &Request, with_todo: bool) -> ResponseTemplate {
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
    let mut calls = vec![
        call(0, C1, PROBE, "{\"what\":\"x\"}"),
        call(1, C2, POKE, "{\"what\":\"y\"}"),
    ];
    if with_todo {
        calls.push(call(
            2,
            C3,
            todo::NAME,
            "{\"op\":\"init\",\"items\":[\"one\",\"two\"]}",
        ));
    }
    calls.push(chunk(
        json!([{"index": 0, "delta": {}, "finish_reason": "tool_calls"}]),
    ));
    calls.push(
        json!({"id": "c", "object": "chat.completion.chunk", "model": "m", "choices": [],
        "usage": {"prompt_tokens": MAIN_USAGE.0, "completion_tokens": MAIN_USAGE.1,
                  "total_tokens": MAIN_USAGE.0 + MAIN_USAGE.1}}),
    );
    let body = calls
        .iter()
        .map(|c| format!("data: {c}\n\n"))
        .collect::<String>()
        + "data: [DONE]\n\n";
    ResponseTemplate::new(200).set_body_raw(body, "text/event-stream")
}

/// A registry tool that records each invocation in `<hits>/<session>` (one
/// line per call), optionally waits for another tool's line first, optionally
/// aborts at a crash point mid-body, optionally blocks forever (`hold`).
/// Per-session files let parallel scenarios share the one process-global
/// registry.
struct FakeTool {
    name: &'static str,
    risk: Risk,
    /// Directory of per-session hit files.
    hits: PathBuf,
    /// Proceed only once this name is in the session's hits (ordering across parallel calls).
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

/// The hits file of `session` under `hits`.
fn hits_file(hits: &Path, session: SessionId) -> PathBuf {
    hits.join(session.to_string())
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

    fn call(&self, _: Value, cx: ToolCx) -> BoxFuture<'_, anyhow::Result<ToolOutput>> {
        Box::pin(async move {
            let file = hits_file(&self.hits, cx.session);
            if let Some(after) = self.after {
                for _ in 0..100 {
                    if hits_of(&file).iter().any(|h| h == after) {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            }
            let mut text = std::fs::read_to_string(&file).unwrap_or_default();
            text.push_str(self.name);
            text.push('\n');
            std::fs::write(&file, text).unwrap();
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
/// registered over the hits directory `hits`, and their hold switches. With
/// `crash`, each body carries its own `call.<i>.mid` point (the env var
/// names at most one of them).
async fn registry(dir: &Path, hits: &Path, crash: bool) -> (ActorRef<ToolRegistry>, Holds) {
    std::fs::create_dir_all(hits).unwrap();
    let cfg = figment::Figment::new().merge(figment::providers::Serialized::defaults(json!({
        "tools": {"policy": {"read_only": "allow", "mutating": "allow", "destructive": "allow"}}
    })));
    let reg_rt = Runtime::new(cfg, dir.to_path_buf());
    let registry = tools::registry::start(&reg_rt).await.unwrap();
    let holds: Holds = (Arc::default(), Arc::default());
    for (i, name, risk, after, hold) in [
        (0, PROBE, Risk::ReadOnly, None, &holds.0),
        (1, POKE, Risk::Mutating, Some(PROBE), &holds.1),
    ] {
        registry
            .ask(tools::registry::RegisterTool(Arc::new(FakeTool {
                name,
                risk,
                hits: hits.to_path_buf(),
                after,
                crash_mid: crash.then(|| crash::call_point(i, crash::CALL_MID)),
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
        .open_turn(session)
        .await
        .unwrap()
        .map(|(t, _)| t)
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
    let (registry, holds) = registry(&reg_dir, &hits, false).await;

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
    assert!(h.store.open_turns(Scope::Here).await.unwrap().is_empty());
    assert_eq!(hits_of(&hits_file(hits, h.id)), [PROBE, POKE]);
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
    hold: &AtomicBool,
    held: &str,
    done: &str,
) -> (Harness, TurnId) {
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
    let (mut h, tid) = interrupt_mid_batch(registry, &holds.1, C2, C1).await;
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
        hits_of(&hits_file(hits, h.id)),
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
    let (mut h, tid) = interrupt_mid_batch(registry, &holds.0, C1, C2).await;
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
    assert_eq!(hits_of(&hits_file(hits, h.id)), [PROBE, POKE, PROBE]);
    let messages = h.store.messages(h.id).await.unwrap();
    assert_eq!(normalised(&messages), reference);
}

/// §4.2 row 2 / §8.1: an open `model_call` is re-issued as `attempt + 1` of
/// the same request; its partial is published as dropped and deleted.
async fn open_model_call_drops_the_partial_and_reissues(
    registry: &ActorRef<ToolRegistry>,
    hits: &Path,
) {
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
    assert!(hits_of(&hits_file(hits, h.id)).is_empty(), "no tool ran");
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
    assert!(hits_of(&hits_file(hits, h.id)).is_empty(), "nothing re-ran");
    assert_eq!(
        kinds(&h.store, h.id, old).await.last().unwrap(),
        "turn_finished"
    );
    assert!(h.store.open_turns(Scope::Here).await.unwrap().is_empty());
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
    assert!(hits_of(&hits_file(hits, h.id)).is_empty());
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

// ---------------------------------------------------------------------------
// W6 — the §11.1 matrix made exhaustive (every crash point × the batch shapes
// the harness can script), the `kind = Suspended` runs, and the reaper's
// hand-off to a live jobs actor.
// ---------------------------------------------------------------------------

/// Which fake provider a matrix row runs against.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Shape {
    /// `probe` (read-only) + `poke` (mutating): two registry tools.
    Two,
    /// The two plus the loop-local `todo` as the third call.
    Todo,
}

/// What a crash at one point must leave after the second life.
#[derive(Clone, Copy, Debug)]
struct Expect {
    /// A turn was open after the crash and was resumed (a `ResumeReport`
    /// exists); `false`: nothing to resume — before the input persisted, or
    /// after `turn_finished`.
    resumed: bool,
    /// Admissible `probe` body counts across both lives.
    probe: &'static [usize],
    /// Admissible `poke` body counts across both lives (never more than one).
    poke: &'static [usize],
    /// Admissible `interrupted` lists of the report.
    interrupted: &'static [&'static [&'static str]],
    /// The lost model call was re-issued: one `request_id` has attempts
    /// `n` and `n + 1`, the first `unresolved`/`settled`, the second `settled`.
    reissued: bool,
    /// The provider saw the re-issued request byte-identical to the lost one
    /// (I5): the last two request bodies are equal.
    same_request: bool,
}

const ONE: &[usize] = &[1];
const ONE_OR_TWO: &[usize] = &[1, 2];
const ZERO_OR_ONE: &[usize] = &[0, 1];
const NONE: &[&[&str]] = &[&[]];
const MAYBE_POKE: &[&[&str]] = &[&[], &[C2]];
const POKE_ONLY: &[&[&str]] = &[&[C2]];

const PLAIN: Expect = Expect {
    resumed: true,
    probe: ONE,
    poke: ONE,
    interrupted: NONE,
    reissued: false,
    same_request: false,
};
const REISSUED: Expect = Expect {
    reissued: true,
    same_request: true,
    ..PLAIN
};
const NOT_RESUMED: Expect = Expect {
    resumed: false,
    ..PLAIN
};

/// The matrix: every `crash::*` point the scripted turn reaches, with what
/// §4.2 says the second life must do. The `call.<i>.mid` and
/// `call.<i>.after_finished` points race with the sibling call (the fake
/// `poke` waits for `probe`'s body, not for its row), so those rows admit
/// both outcomes — in every one of them `poke` ran at most once and, when
/// it did not run, the model was told so.
fn matrix() -> Vec<(Shape, String, Expect)> {
    use crash::*;
    let p = |i: usize, s: &str| call_point(i, s);
    let two = |point: &str, e: Expect| (Shape::Two, point.to_owned(), e);
    let todo = |point: String, e: Expect| (Shape::Todo, point, e);
    vec![
        // Before the input row: nothing journaled, the client's ticket was
        // never acknowledged; the input is sent again by the parent.
        two(INPUT_BEFORE_PERSIST, NOT_RESUMED),
        // §4.2 row 1: the input is in the frame, the turn starts now.
        two(INPUT_AFTER_PERSIST, PLAIN),
        // §4.2 row 2: the first model call, never dispatched / dispatched.
        two(
            MODEL_AFTER_RESERVE,
            Expect {
                same_request: false,
                ..REISSUED
            },
        ),
        two(
            MODEL_AFTER_DISPATCH,
            Expect {
                same_request: false,
                ..REISSUED
            },
        ),
        // §4.2 row 4: rows planned.
        two(BATCH_AFTER_TOOL_USE_PERSIST, PLAIN),
        two(&p(0, CALL_BEFORE_STARTED), PLAIN),
        two(&p(1, CALL_BEFORE_STARTED), PLAIN),
        // Inside `probe`: read-only → runs again; `poke` may or may not have
        // started.
        two(
            &p(0, CALL_MID),
            Expect {
                probe: &[2],
                poke: ZERO_OR_ONE,
                interrupted: MAYBE_POKE,
                ..PLAIN
            },
        ),
        // Inside `poke`: mutating → interrupted, never run again.
        two(
            &p(1, CALL_MID),
            Expect {
                probe: ONE_OR_TWO,
                interrupted: POKE_ONLY,
                ..PLAIN
            },
        ),
        two(
            &p(0, CALL_AFTER_FINISHED),
            Expect {
                poke: ZERO_OR_ONE,
                interrupted: MAYBE_POKE,
                ..PLAIN
            },
        ),
        two(
            &p(1, CALL_AFTER_FINISHED),
            Expect {
                probe: ONE_OR_TWO,
                ..PLAIN
            },
        ),
        // §4.2 row 3: every row finished, the batch message not yet written.
        two(BATCH_BEFORE_MESSAGE, PLAIN),
        // The batch message is in the frame: an ordinary round follows.
        two(BATCH_AFTER_MESSAGE, PLAIN),
        // The final reply's stream: re-issued from the frame (I5), the
        // partial dropped.
        two(STREAM_MID, REISSUED),
        two(STREAM_AFTER_PARTIAL, REISSUED),
        two(STREAM_BEFORE_END_PERSIST, REISSUED),
        two(TURN_BEFORE_FINISHED, REISSUED),
        two(TURN_AFTER_FINISHED, NOT_RESUMED),
        // The loop-local call's own journal writes (§3.3 steps 6–7).
        todo(p(2, CALL_BEFORE_STARTED), PLAIN),
        todo(p(2, CALL_AFTER_STARTED), PLAIN),
        todo(p(2, CALL_BEFORE_FINISHED), PLAIN),
        todo(p(2, CALL_AFTER_FINISHED), PLAIN),
    ]
}

/// Children run at once (each is this test binary building its own registry).
const CHILDREN_AT_ONCE: usize = 6;

/// One life that dies: the parent harness (store, session, provider) and the
/// child's exit status.
struct Crashed {
    shape: Shape,
    point: String,
    expect: Expect,
    server: Harness,
    status: std::process::ExitStatus,
}

/// Spawn the crashing life of one matrix row.
async fn crash_life(hits: &Path, shape: Shape, point: String, expect: Expect) -> Crashed {
    let server = match shape {
        Shape::Two => Harness::new(BatchFake).await,
        Shape::Todo => Harness::new(BatchFake3).await,
    };
    let status = tokio::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "loop_::durability_tests::crash_child",
            "--include-ignored",
            "--nocapture",
        ])
        .env("THEUTH_CRASH_AT", &point)
        .env("THEUTH_109_DIR", &server.dir)
        .env("THEUTH_109_ENDPOINT", server.endpoint())
        .env("THEUTH_109_SESSION", server.id.to_string())
        .env("THEUTH_109_HITS", hits)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .unwrap();
    Crashed {
        shape,
        point,
        expect,
        server,
        status,
    }
}

/// The second life of one matrix row: reopen, boot bookkeeping, resume (or
/// not), then every §11.1 check. Returns the failures, prefixed by the row.
async fn second_life(
    registry: &ActorRef<ToolRegistry>,
    hits: &Path,
    c: Crashed,
    references: &HashMap<Shape, Vec<(String, String)>>,
) -> Vec<String> {
    let tag = format!("{:?}/{}", c.shape, c.point);
    let mut fails: Vec<String> = Vec::new();
    let mut check = |ok: bool, what: String| {
        if !ok {
            fails.push(format!("{tag}: {what}"));
        }
    };
    let reference = &references[&c.shape];
    check(
        !c.status.success(),
        format!("the child must abort, got {}", c.status),
    );
    let server = c.server;
    let mut h = Harness::attach(server.dir.clone(), &server.endpoint(), server.id).await;
    journal(registry, &h.store).await;
    // Boot: accounting marks the lost attempt (§4.6 step 3).
    h.store.unresolve_open_calls().await.unwrap();
    let open = h.store.open_turn(h.id).await.unwrap().map(|(t, _)| t);
    check(
        open.is_some() == c.expect.resumed,
        format!(
            "open turn: {open:?}, expected resumed = {}",
            c.expect.resumed
        ),
    );
    let tid = match open {
        Some(tid) => {
            let plan = h.store.resume_plan(h.id, tid).await.unwrap();
            check(
                plan.kind == ResumeKind::Interrupted,
                format!("kind {:?}", plan.kind),
            );
            let (turn, _) = h.turn("openai/textonly", CancellationToken::new()).await;
            turn.resume(plan, None).await;
            let events = h.drain().await;
            check(
                events
                    .iter()
                    .any(|e| matches!(e, AgentEvent::TurnResumed { turn, .. } if *turn == tid)),
                "no TurnResumed".into(),
            );
            let (_, interrupted) = report(&events);
            let interrupted: Vec<&str> = interrupted.iter().map(String::as_str).collect();
            check(
                c.expect.interrupted.contains(&interrupted.as_slice()),
                format!("interrupted {interrupted:?}"),
            );
            if c.point == crash::STREAM_AFTER_PARTIAL {
                check(
                    events
                        .iter()
                        .any(|e| matches!(e, AgentEvent::PartialDropped { .. })),
                    "the checkpointed partial was not dropped".into(),
                );
            }
            Some(tid)
        }
        None => {
            // Nothing to resume. Before the input: the client re-sends it.
            if h.store.messages(h.id).await.unwrap().is_empty() {
                let (turn, tid) = h.turn("openai/textonly", CancellationToken::new()).await;
                turn.run(tid, vec![h.input("go", false)]).await;
                h.drain().await;
                Some(tid)
            } else {
                None
            }
        }
    };
    let hit = hits_of(&hits_file(hits, h.id));
    let probe = hit.iter().filter(|h| *h == PROBE).count();
    let poke = hit.iter().filter(|h| *h == POKE).count();
    check(
        c.expect.probe.contains(&probe),
        format!("probe ran {probe}x: {hit:?}"),
    );
    check(
        c.expect.poke.contains(&poke),
        format!("poke ran {poke}x: {hit:?}"),
    );
    check(poke <= 1, format!("poke ran twice: {hit:?}"));
    if let Some(tid) = tid {
        let rows = h.store.calls_of(h.id, tid).await.unwrap();
        check(
            rows.iter().all(|r| r.state.is_terminal()),
            format!("non-terminal rows: {rows:?}"),
        );
        let steps = kinds(&h.store, h.id, tid).await;
        check(
            steps.last().map(String::as_str) == Some("turn_finished"),
            format!("steps {steps:?}"),
        );
    }
    check(
        h.store.open_turns(Scope::Here).await.unwrap().is_empty(),
        "a turn is still open".into(),
    );
    let partials: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM turn_steps WHERE session_id = ? AND kind = 'assistant_partial'",
    )
    .bind(h.id.to_string())
    .fetch_one(h.store.pool())
    .await
    .unwrap();
    check(partials == 0, format!("{partials} partial rows left"));
    check(
        h.store.last_stop(h.id).await.unwrap().is_some(),
        "last_stop unset".into(),
    );
    // 1. The transcript equals the uninterrupted run's modulo the notes.
    let messages = h.store.messages(h.id).await.unwrap();
    let got = normalised(&messages);
    check(
        got.len() == reference.len(),
        format!(
            "{} messages, reference {}: {got:?}",
            got.len(),
            reference.len()
        ),
    );
    for (g, r) in got.iter().zip(reference) {
        check(g.0 == r.0, format!("role {} vs {}", g.0, r.0));
        if g.0 == "Tool" {
            // The interrupted note replaces `poke done` iff the report said so.
            let interrupted = poke == 0 || !g.1.contains("poke done");
            if interrupted {
                check(
                    g.1.contains("ERROR: interrupted: the engine restarted at "),
                    format!("tool text without the note: {}", g.1),
                );
            }
            check(
                g.1.contains("probe done"),
                format!("probe result missing: {}", g.1),
            );
            if c.shape == Shape::Todo {
                check(
                    g.1.contains("one") && g.1.contains("two"),
                    format!("todo result missing: {}", g.1),
                );
            }
            if !interrupted {
                check(g.1 == r.1, format!("tool text {:?} vs {:?}", g.1, r.1));
            }
        } else {
            check(g.1 == r.1, format!("text {:?} vs {:?}", g.1, r.1));
        }
    }
    // 4. Frame identity: the re-issued request is the lost one, byte for byte.
    if c.expect.same_request {
        let reqs = server.requests().await;
        let n = reqs.len();
        check(
            n >= 2 && reqs[n - 1] == reqs[n - 2],
            format!("the re-issued request differs from the lost one ({n} requests)"),
        );
    }
    // 5. `model_calls`: one request re-issued as `attempt + 1`, or none.
    let attempts: Vec<(String, i64, String)> = sqlx::query_as(
        "SELECT request_id, attempt, status FROM model_calls WHERE owner_id = ? ORDER BY request_id, attempt",
    )
    .bind(h.id.to_string())
    .fetch_all(h.store.pool())
    .await
    .unwrap();
    let mut by_request: HashMap<&str, Vec<(i64, &str)>> = HashMap::new();
    for (r, a, s) in &attempts {
        by_request.entry(r).or_default().push((*a, s));
    }
    let reissued: Vec<_> = by_request.values().filter(|v| v.len() > 1).collect();
    check(
        reissued.len() == usize::from(c.expect.reissued),
        format!(
            "re-issued requests {reissued:?}, expected {}",
            c.expect.reissued
        ),
    );
    for v in reissued {
        check(
            v.len() == 2
                && v[1].0 == v[0].0 + 1
                && matches!(v[0].1, "unresolved" | "settled")
                && v[1].1 == "settled",
            format!("attempts {v:?}"),
        );
    }
    check(
        attempts.iter().all(|(_, _, s)| s != "unresolved") || c.expect.reissued,
        format!("unresolved attempt without a re-issue: {attempts:?}"),
    );
    fails
}

/// §11.1 made exhaustive (W6): the crashing lives run as children of this
/// binary, [`CHILDREN_AT_ONCE`] at a time; the second lives run in sequence
/// here (the registry's journal is one store handle at a time). Every
/// failure is collected and reported together.
async fn crash_matrix(
    registry: &ActorRef<ToolRegistry>,
    hits: &Path,
    reference: &[(String, String)],
) {
    use futures::StreamExt;
    let mut references: HashMap<Shape, Vec<(String, String)>> = HashMap::new();
    references.insert(Shape::Two, reference.to_vec());
    references.insert(Shape::Todo, normal_turn_with_todo(registry, hits).await);
    let started = Instant::now();
    let crashed: Vec<Crashed> = futures::stream::iter(matrix())
        .map(|(shape, point, expect)| crash_life(hits, shape, point, expect))
        .buffer_unordered(CHILDREN_AT_ONCE)
        .collect()
        .await;
    let children = started.elapsed();
    let mut fails = Vec::new();
    let mut rows = 0;
    for c in crashed {
        rows += 1;
        fails.extend(second_life(registry, hits, c, &references).await);
    }
    eprintln!(
        "109 W6 matrix: {rows} rows, children {:.1}s, total {:.1}s",
        children.as_secs_f64(),
        started.elapsed().as_secs_f64()
    );
    assert!(
        fails.is_empty(),
        "{} failure(s):\n{}",
        fails.len(),
        fails.join("\n")
    );
}

/// The uninterrupted three-call turn: the reference for [`Shape::Todo`].
async fn normal_turn_with_todo(
    registry: &ActorRef<ToolRegistry>,
    hits: &Path,
) -> Vec<(String, String)> {
    let mut h = Harness::new(BatchFake3).await;
    journal(registry, &h.store).await;
    let (turn, tid) = h.turn("openai/textonly", CancellationToken::new()).await;
    turn.run(tid, vec![h.input("go", false)]).await;
    h.drain().await;
    let rows = h.store.calls_of(h.id, tid).await.unwrap();
    assert_eq!(rows.len(), 3);
    assert!(
        rows.iter().all(|r| r.state == CallState::Finished),
        "{rows:?}"
    );
    assert_eq!(rows[2].name, todo::NAME);
    assert_eq!(hits_of(&hits_file(hits, h.id)), [PROBE, POKE]);
    let messages = h.store.messages(h.id).await.unwrap();
    let tools = tool_texts(&messages);
    assert_eq!(tools.len(), 1);
    assert!(
        tools[0].contains("one") && tools[0].contains("two"),
        "{}",
        tools[0]
    );
    normalised(&messages)
}

/// The crashing life of [`crash_matrix`]: driven by env, a no-op otherwise.
/// Partials are checkpointed on every delta so `stream.after_partial` is
/// reachable in a one-chunk reply.
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
        let (registry, _) = registry(&reg_dir, &hits, true).await;
        let h = Harness::attach(PathBuf::from(dir), &endpoint, session).await;
        journal(&registry, &h.store).await;
        let (mut turn, tid) = h.turn("openai/textonly", CancellationToken::new()).await;
        Arc::make_mut(&mut turn.shared.cfg)
            .durability
            .partial_checkpoint_ms = 0;
        turn.run(tid, vec![h.input("go", false)]).await;
        let _ = std::fs::remove_dir_all(&reg_dir);
    });
}

// ---- kind = Suspended (§9.2 → §4.2 row `suspended`) ------------------------

#[test]
fn drain_suspends_and_the_next_start_resumes() {
    crate::with_test_registry(suspended_scenarios());
}

async fn suspended_scenarios() {
    let reg_dir = std::env::temp_dir().join(format!("theuth-109-w6-reg-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&reg_dir).unwrap();
    let hits = reg_dir.join("hits");
    let (registry, holds) = registry(&reg_dir, &hits, false).await;
    let reference = normal_turn_journals_every_write_point(&registry, &hits).await;
    draining_session_opens_no_model_call(&registry, &hits, &reference).await;
    shutdown_token_leaves_the_batch_open(&registry, &hits, &holds, &reference).await;
    reaper_hands_lost_rows_to_the_live_jobs_actor(&registry).await;
    // 139 W2
    release_drain_writes_suspended_or_finished(&registry, &holds).await;
    cancel_while_draining_is_a_suspension(&registry, &hits, &holds, &reference).await;
    unload_always_lands_the_cancel(&registry, &holds).await;
    let _ = std::fs::remove_dir_all(&reg_dir);
}

/// §9.2 step 2: a session told `Draining` opens no model call — the turn
/// writes `suspended` after the input and stops without `turn_finished`;
/// the next start resumes it as §4.2 row 1 (`kind = Suspended`) and the
/// transcript equals the uninterrupted run's.
async fn draining_session_opens_no_model_call(
    registry: &ActorRef<ToolRegistry>,
    hits: &Path,
    reference: &[(String, String)],
) {
    let mut h = Harness::new(BatchFake).await;
    journal(registry, &h.store).await;
    let (turn, tid) = h.turn("openai/textonly", CancellationToken::new()).await;
    turn.shared.draining.store(true, Ordering::SeqCst);
    turn.run(tid, vec![h.input("go", false)]).await;
    assert_eq!(
        kinds(&h.store, h.id, tid).await,
        ["input_accepted", "suspended"]
    );
    assert_eq!(open_turn(&h.store, h.id).await, tid);
    assert!(h.requests().await.is_empty(), "no model call opened");
    let plan = h.store.resume_plan(h.id, tid).await.unwrap();
    assert_eq!(plan.kind, ResumeKind::Suspended);
    assert_eq!(plan.last, LastStep::InputAccepted);
    // The next start: a fresh turn over the frame, the plan from the journal.
    let (turn, _) = h.turn("openai/textonly", CancellationToken::new()).await;
    turn.resume(plan, None).await;
    let events = h.drain().await;
    assert!(events.iter().any(|e| matches!(
        e,
        AgentEvent::TurnResumed {
            kind: ResumeKind::Suspended,
            ..
        }
    )));
    assert_eq!(hits_of(&hits_file(hits, h.id)), [PROBE, POKE]);
    assert_eq!(
        normalised(&h.store.messages(h.id).await.unwrap()),
        reference
    );
    assert!(h.store.open_turns(Scope::Here).await.unwrap().is_empty());
}

/// §9.2 steps 3–4 then §4.2: the drain marked the turn `suspended`, the
/// window closed with `poke` still running, the shutdown token fired. The
/// batch is left as the journal has it (no batch message, no
/// `turn_finished`); the next start reports `poke` interrupted with the
/// `kind="shutdown"` note, reuses `probe`'s result, and the transcript is
/// the uninterrupted run's modulo that note. `poke` ran once.
async fn shutdown_token_leaves_the_batch_open(
    registry: &ActorRef<ToolRegistry>,
    hits: &Path,
    holds: &Holds,
    reference: &[(String, String)],
) {
    let h = Harness::new(BatchFake).await;
    journal(registry, &h.store).await;
    holds.1.store(true, Ordering::SeqCst);
    let cancel = h.rt.shutdown.child_token();
    let (turn, tid) = h.turn("openai/textonly", cancel).await;
    let input = h.input("go", false);
    let running = tokio::spawn(async move { turn.run(tid, vec![input]).await });
    for _ in 0..200 {
        let rows = h.store.calls_of(h.id, tid).await.unwrap();
        if rows
            .iter()
            .any(|r| r.call_id == C2 && r.state == CallState::Started)
            && rows
                .iter()
                .any(|r| r.call_id == C1 && r.state == CallState::Finished)
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    // The drain's mark, then the token at the deadline.
    h.store
        .step(
            h.id,
            tid,
            StepKind::Suspended,
            None,
            &json!({"reason": "shutdown"}),
        )
        .await
        .unwrap();
    h.rt.shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(10), running)
        .await
        .expect("the turn returns once the token fires")
        .unwrap();
    holds.1.store(false, Ordering::SeqCst);
    let steps = kinds(&h.store, h.id, tid).await;
    assert_eq!(
        steps,
        ["input_accepted", "model_call", "tool_batch", "suspended"],
        "no batch message, no turn_finished"
    );
    assert!(tool_texts(&h.store.messages(h.id).await.unwrap()).is_empty());
    let rows = h.store.calls_of(h.id, tid).await.unwrap();
    assert_eq!(rows[0].state, CallState::Finished);
    assert_eq!(rows[1].state, CallState::Started);
    assert_eq!(hits_of(&hits_file(hits, h.id)), [PROBE, POKE]);

    // The next start: a fresh runtime (its token is not cancelled).
    let server = h;
    let mut h = Harness::attach(server.dir.clone(), &server.endpoint(), server.id).await;
    journal(registry, &h.store).await;
    h.store.unresolve_open_calls().await.unwrap();
    let plan = h.store.resume_plan(h.id, tid).await.unwrap();
    assert_eq!(plan.kind, ResumeKind::Suspended);
    assert_eq!(
        plan.last,
        LastStep::ToolBatch {
            all_finished: false
        }
    );
    let (turn, _) = h.turn("openai/textonly", CancellationToken::new()).await;
    turn.resume(plan, None).await;
    let events = h.drain().await;
    let (replayed, interrupted) = report(&events);
    assert!(replayed.is_empty());
    assert_eq!(interrupted, [C2]);
    let messages = h.store.messages(h.id).await.unwrap();
    let tools = tool_texts(&messages);
    assert_eq!(tools.len(), 1);
    let first = tools[0].lines().next().unwrap();
    assert!(
        first.starts_with("<engine_restart at=\"")
            && first.contains("kind=\"shutdown\" replayed=\"0\" interrupted=\"1\"/>"),
        "{first}"
    );
    assert!(tools[0].contains("probe done"), "{}", tools[0]);
    assert!(
        tools[0].contains("ERROR: interrupted: the engine restarted at "),
        "{}",
        tools[0]
    );
    assert_eq!(
        hits_of(&hits_file(hits, h.id)),
        [PROBE, POKE],
        "poke ran once across both lives"
    );
    let got = normalised(&messages);
    assert_eq!(got.len(), reference.len());
    for (g, r) in got.iter().zip(reference) {
        assert_eq!(g.0, r.0);
        if g.0 != "Tool" {
            assert_eq!(g.1, r.1);
        }
    }
    assert_eq!(
        kinds(&h.store, h.id, tid).await.last().unwrap(),
        "turn_finished"
    );
    assert!(h.store.open_turns(Scope::Here).await.unwrap().is_empty());
}

// ---- §5.4: the reaper's hand-off to the jobs actor -------------------------

/// `reap_jobs` with a live jobs actor (the registry's): the `running` row of
/// a dead group is settled `lost` in the store, handed to the actor as
/// `Reaped`, and `jobs()` lists it `lost` with the log tail; a second pass
/// finds nothing.
async fn reaper_hands_lost_rows_to_the_live_jobs_actor(registry: &ActorRef<ToolRegistry>) {
    use tools::{JobId, JobKind, JobRow, JobState, ListJobs};
    let h = Harness::new(BatchFake).await;
    journal(registry, &h.store).await;
    let log = h.dir.join("j7.log");
    std::fs::write(&log, "building…\nerror[E0308]: mismatched types\n").unwrap();
    // A pgid no process has: `fork` never hands out a pid this large.
    let dead_pgid = 2_000_000_000;
    h.store
        .job_put(&JobRow {
            session: h.id,
            id: JobId("j7".into()),
            kind: JobKind::Shell,
            call_id: "stub-delivered".into(),
            command: "cargo build".into(),
            name: None,
            pgid: Some(dead_pgid),
            log: Some(log.clone()),
            started_ms: chrono::Utc::now().timestamp_millis() - 5_000,
            state: JobState::Running,
            exit: None,
            settled_ms: None,
            engine: String::new(),
        })
        .await
        .unwrap();
    let report = crate::durability::jobs::reap_jobs(&h.store).await;
    assert_eq!((report.killed, report.lost, report.orphaned), (0, 1, 0));
    let row = &h.store.jobs_of(h.id).await.unwrap()[0];
    assert_eq!(
        (row.state, row.exit.clone()),
        (JobState::Lost, Some(tools::JobExit::Lost))
    );
    let jobs = ActorRef::<tools::Jobs>::lookup(tools::JOBS_ACTOR)
        .unwrap()
        .expect("the registry's jobs actor");
    let mut listed = Vec::new();
    for _ in 0..100 {
        listed = jobs.ask(ListJobs(h.id)).await.unwrap();
        if !listed.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(listed.len(), 1, "{listed:?}");
    let view = &listed[0];
    assert_eq!((view.id.0.as_str(), view.state.as_str()), ("j7", "lost"));
    assert_eq!(view.command, "cargo build");
    assert!(view.tail.contains("mismatched types"), "{}", view.tail);
    assert_eq!(view.log, log);
    assert!(view.secs >= 5, "{}", view.secs);
    assert_eq!(
        crate::durability::jobs::reap_jobs(&h.store).await,
        crate::durability::jobs::ReapReport::default()
    );
}

// ---- 139 §6.1: the release drain (W2) ---------------------------------------

/// [`FINAL`] after a delay: a turn whose last model call is in flight when
/// the drain begins finishes inside the window.
struct SlowFinal;

impl Respond for SlowFinal {
    fn respond(&self, _: &Request) -> ResponseTemplate {
        sse(FINAL, "stop", MAIN_USAGE).set_delay(Duration::from_millis(600))
    }
}

/// Start a turn on a real session actor over `h`.
async fn start_turn(h: &Harness, text: &str) -> ActorRef<Session> {
    let actor = h.actor("openai/textonly").await;
    actor
        .tell(UserInput {
            id: None,
            text: text.into(),
            files: Vec::new(),
            ticket: None,
            scope: None,
            defer: false,
        })
        .await
        .unwrap();
    actor
}

/// Wait until `held` is `started` and `done` is `finished` in the journal.
async fn wait_mid_batch(h: &Harness, tid: TurnId, held: &str, done: &str) {
    for _ in 0..200 {
        let rows = h.store.calls_of(h.id, tid).await.unwrap();
        if rows
            .iter()
            .any(|r| r.call_id == held && r.state == CallState::Started)
            && rows
                .iter()
                .any(|r| r.call_id == done && r.state == CallState::Finished)
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("the batch never reached {held} started / {done} finished");
}

/// Poll until the session has an open turn.
async fn wait_open_turn(h: &Harness, what: &str) -> TurnId {
    for _ in 0..200 {
        if let Some((t, _)) = h.store.open_turn(h.id).await.unwrap() {
            return t;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("{what}: no turn opened");
}

/// 139 I3: `DrainSession` over a live actor leaves exactly one of two
/// states. (a) `poke` held past the deadline → `suspended{release}` written
/// first stands, `TurnSuspended{release}` published, the actor is stopped,
/// the cancelled turn wrote no `turn_finished`, and the resume plan reads
/// `Suspended` / cause `Transfer`. (b) the final reply lands inside the
/// window → `turn_finished` closes the row; the report says so. (c) no open
/// turn → nothing written.
async fn release_drain_writes_suspended_or_finished(
    registry: &ActorRef<ToolRegistry>,
    holds: &Holds,
) {
    use crate::supervisor::transfer::{DrainSession, RELEASE, SessionDrainReport, drain_session};
    // (a) suspended.
    let mut h = Harness::new(BatchFake).await;
    journal(registry, &h.store).await;
    holds.1.store(true, Ordering::SeqCst);
    let actor = start_turn(&h, "go").await;
    let tid = wait_open_turn(&h, "a").await;
    wait_mid_batch(&h, tid, C2, C1).await;
    let live = crate::supervisor::LiveActor::Native(actor.clone());
    let report = drain_session(
        &h.store,
        &h.rt,
        Some(live),
        DrainSession {
            session: h.id,
            reason: RELEASE,
            deadline: Instant::now() + Duration::from_millis(400),
        },
    )
    .await;
    assert_eq!(
        report,
        SessionDrainReport {
            open: true,
            finished: false,
            suspended: true
        }
    );
    assert!(!actor.is_alive(), "the actor is stopped");
    let events = h
        .drain_until(|e| matches!(e, AgentEvent::TurnSuspended { .. }))
        .await;
    assert!(events.iter().any(|e| matches!(
        e,
        AgentEvent::TurnSuspended { turn, reason, .. } if *turn == tid && reason == "release"
    )));
    // The cancelled turn task (detached from the actor) returns as a
    // suspension: it writes no `turn_finished`; `poke` is still `started`
    // (the hold is released only after the plan is read).
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        kinds(&h.store, h.id, tid).await,
        ["input_accepted", "model_call", "tool_batch", "suspended"]
    );
    assert_eq!(open_turn(&h.store, h.id).await, tid);
    let plan = h.store.resume_plan(h.id, tid).await.unwrap();
    assert_eq!(
        (plan.kind, plan.cause),
        (ResumeKind::Suspended, ResumeCause::Transfer)
    );
    assert_eq!(
        plan.last,
        LastStep::ToolBatch {
            all_finished: false
        }
    );
    holds.1.store(false, Ordering::SeqCst);

    // (b) finished inside the window.
    let mut h = Harness::new(SlowFinal).await;
    journal(registry, &h.store).await;
    let actor = start_turn(&h, TALK).await;
    let tid = wait_open_turn(&h, "b").await;
    // The final call must be in flight (`model_call` reserved) before the
    // drain, or the first round would stop on `draining` instead.
    for _ in 0..200 {
        if kinds(&h.store, h.id, tid)
            .await
            .contains(&"model_call".to_owned())
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let report = drain_session(
        &h.store,
        &h.rt,
        Some(crate::supervisor::LiveActor::Native(actor.clone())),
        DrainSession {
            session: h.id,
            reason: RELEASE,
            deadline: Instant::now() + Duration::from_secs(5),
        },
    )
    .await;
    assert_eq!(
        report,
        SessionDrainReport {
            open: true,
            finished: true,
            suspended: false
        }
    );
    assert!(!actor.is_alive());
    let events = h.drain().await;
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, AgentEvent::TurnSuspended { .. }))
    );
    let steps = kinds(&h.store, h.id, tid).await;
    assert_eq!(
        steps,
        ["input_accepted", "model_call", "suspended", "turn_finished"],
        "the drain's mark, then the call that was in flight closes the row"
    );
    assert!(h.store.open_turn(h.id).await.unwrap().is_none());

    // (c) nothing open: nothing written, no actor needed.
    let report = drain_session(
        &h.store,
        &h.rt,
        None,
        DrainSession {
            session: h.id,
            reason: RELEASE,
            deadline: Instant::now(),
        },
    )
    .await;
    assert_eq!(report, SessionDrainReport::default());
    assert_eq!(kinds(&h.store, h.id, tid).await, steps, "nothing written");
}

/// 139 §6.1 loop change: a cancel that lands while the session is draining
/// is a suspension, not `turn_finished{cancelled}` — the `suspended{release}`
/// row stands, and the next owner resumes it with `kind="transfer" from="a"`.
async fn cancel_while_draining_is_a_suspension(
    registry: &ActorRef<ToolRegistry>,
    hits: &Path,
    holds: &Holds,
    reference: &[(String, String)],
) {
    let h = Harness::new(BatchFake).await;
    journal(registry, &h.store).await;
    holds.1.store(true, Ordering::SeqCst);
    let cancel = CancellationToken::new();
    let (turn, tid) = h.turn("openai/textonly", cancel.clone()).await;
    let draining = turn.shared.draining.clone();
    let input = h.input("go", false);
    let running = tokio::spawn(async move { turn.run(tid, vec![input]).await });
    wait_mid_batch(&h, tid, C2, C1).await;
    // The drain's step 1, then its step 5 cancel (not the shutdown token).
    assert_eq!(
        crate::supervisor::transfer::suspend_open_turn(&h.store, h.id, "release").await,
        Some(tid)
    );
    draining.store(true, Ordering::SeqCst);
    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(10), running)
        .await
        .expect("the turn returns on the cancel")
        .unwrap();
    holds.1.store(false, Ordering::SeqCst);
    assert_eq!(
        kinds(&h.store, h.id, tid).await,
        ["input_accepted", "model_call", "tool_batch", "suspended"],
        "no turn_finished{{cancelled}}"
    );
    assert_eq!(open_turn(&h.store, h.id).await, tid);

    // The new owner: cause transfer, from the old one.
    let server = h;
    let mut h = Harness::attach(server.dir.clone(), &server.endpoint(), server.id).await;
    journal(registry, &h.store).await;
    h.store.unresolve_open_calls().await.unwrap();
    let plan = h.store.resume_plan(h.id, tid).await.unwrap();
    assert_eq!(
        (plan.kind, plan.cause),
        (ResumeKind::Suspended, ResumeCause::Transfer)
    );
    let (mut turn, _) = h.turn("openai/textonly", CancellationToken::new()).await;
    turn.resume_from = Some("a".into());
    turn.resume(plan, None).await;
    let events = h.drain().await;
    assert!(events.iter().any(|e| matches!(
        e,
        AgentEvent::TurnResumed { turn, cause: ResumeCause::Transfer, from: Some(f), .. }
            if *turn == tid && f == "a"
    )));
    let (replayed, interrupted) = report(&events);
    assert!(replayed.is_empty());
    assert_eq!(interrupted, [C2]);
    let messages = h.store.messages(h.id).await.unwrap();
    let first = tool_texts(&messages)[0].lines().next().unwrap().to_owned();
    assert!(
        first.contains("kind=\"transfer\" from=\"a\" replayed=\"0\" interrupted=\"1\"/>"),
        "{first}"
    );
    assert_eq!(
        hits_of(&hits_file(hits, h.id)),
        [PROBE, POKE],
        "poke ran once"
    );
    let got = normalised(&messages);
    assert_eq!(got.len(), reference.len());
    assert!(h.store.open_turns(Scope::Here).await.unwrap().is_empty());
}

/// 139 D19: `tell(CancelTurn)` + `stop_gracefully` + `wait_for_shutdown`
/// lands the cancel every time (`kill()` could drop it with the mailbox):
/// a hundred live turns, each held in `poke`, each ends `cancelled`. A
/// fresh session each time — the cancelled batch leaves a tool message the
/// fake answers [`FINAL`] to, so a second turn there would open no batch.
async fn unload_always_lands_the_cancel(registry: &ActorRef<ToolRegistry>, holds: &Holds) {
    for i in 0..100 {
        let mut h = Harness::new(BatchFake).await;
        journal(registry, &h.store).await;
        holds.1.store(true, Ordering::SeqCst);
        let actor = start_turn(&h, "go").await;
        let tid = wait_open_turn(&h, &format!("unload #{i}")).await;
        wait_mid_batch(&h, tid, C2, C1).await;
        crate::supervisor::transfer::stop_actor(&actor).await;
        assert!(!actor.is_alive(), "#{i}: stopped");
        let events = h.drain().await;
        assert!(
            events.iter().any(|e| matches!(
                e,
                AgentEvent::TurnFinished { turn, stop_reason, .. } if *turn == tid && stop_reason == "cancelled"
            )),
            "#{i}: the cancel landed: {events:?}"
        );
        holds.1.store(false, Ordering::SeqCst);
        assert!(h.store.open_turns(Scope::Here).await.unwrap().is_empty());
    }
}
