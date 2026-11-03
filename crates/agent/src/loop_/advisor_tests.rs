//! 125 §11 W1, the loop side: the two step hooks, the passive channel, the
//! idle preserve/steer rules, the Stop preserve rule and the advisor-kind
//! session's `advise` dispatch and bare prompt. A real [`Session`] actor over
//! the fake provider of [`super::vision_fallback_tests`]; the `Advisors`
//! actor runs on the same store (no supervisor: its routing `tell`s are
//! dropped, which pins the hook bookkeeping without a second model).

use parking_lot::Mutex;
use std::collections::VecDeque;

use kameo::actor::Spawn as _;
use serde_json::{Value, json};
use wiremock::{Request, Respond, ResponseTemplate};

use super::vision_fallback_tests::{Harness, MAIN_USAGE, sse, system, texts};
use super::*;
use crate::advisor::{ADVISORS_ACTOR, Advisors, Attach};
use crate::{
    AdvisorConfig, AdvisorLink, AdvisorSeverity, AdvisorState, AdvisoryNote, HistoryEntry,
};

/// A scripted provider: replies in order, then `fallback` forever.
struct Script {
    queue: Mutex<VecDeque<ResponseTemplate>>,
    fallback: ResponseTemplate,
}

impl Script {
    fn new(replies: Vec<ResponseTemplate>) -> Self {
        Self {
            queue: Mutex::new(replies.into_iter().collect()),
            fallback: sse("ok.", "stop", MAIN_USAGE),
        }
    }
}

impl Respond for Script {
    fn respond(&self, _: &Request) -> ResponseTemplate {
        self.queue
            .lock()
            .pop_front()
            .unwrap_or_else(|| self.fallback.clone())
    }
}

/// One OpenAI SSE reply carrying one tool call.
fn tool_call(id: &str, name: &str, arguments: &Value) -> ResponseTemplate {
    let chunk = |choices: Value| json!({"id": "c", "object": "chat.completion.chunk", "model": "m", "choices": choices});
    let body = format!(
        "data: {}\n\ndata: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
        chunk(
            json!([{"index": 0, "delta": {"role": "assistant", "tool_calls": [
            {"index": 0, "id": id, "type": "function",
             "function": {"name": name, "arguments": arguments.to_string()}}
        ]}, "finish_reason": null}])
        ),
        chunk(json!([{"index": 0, "delta": {}, "finish_reason": "tool_calls"}])),
        json!({"id": "c", "object": "chat.completion.chunk", "model": "m", "choices": [],
            "usage": {"prompt_tokens": MAIN_USAGE.0, "completion_tokens": MAIN_USAGE.1,
                      "total_tokens": MAIN_USAGE.0 + MAIN_USAGE.1}}),
    );
    ResponseTemplate::new(200).set_body_raw(body, "text/event-stream")
}

/// Forget a dead registration left by an earlier test body's runtime.
pub(crate) fn clear_stale<A: kameo::Actor>(name: &str) {
    if ActorRef::<A>::lookup(name)
        .ok()
        .flatten()
        .is_some_and(|a| !a.is_alive())
    {
        kameo::registry::ACTOR_REGISTRY
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(name);
    }
}

async fn advisors(h: &Harness, cfg: AdvisorConfig) -> ActorRef<Advisors> {
    clear_stale::<Advisors>(ADVISORS_ACTOR);
    Advisors::start(
        h.store.clone(),
        h.rt.clone(),
        cfg,
        0.8,
        Arc::new(|t: &str| t.to_owned()),
    )
    .await
    .unwrap()
}

/// A second session actor on the harness store, with its kind.
async fn spawn(h: &Harness, id: SessionId, kind: SessionKind) -> ActorRef<Session> {
    let (epoch, head, history) = h.store.frame(id).await.unwrap();
    Session::spawn(
        Session::new(
            id,
            0,
            (
                h.catalog.resolve("openai/textonly", &|_| true).unwrap(),
                None,
            ),
            None,
            SessionState::resume(epoch, head, history),
            Arc::new(AgentConfig::default()),
            Arc::default(),
            Arc::default(),
            Arc::default(),
            Arc::default(),
            Arc::default(),
            h.store.clone(),
            h.client.clone(),
            Auth::stub(&h.dir),
            h.rt.clone(),
            h.catalog.clone(),
            None,
            Arc::new(Semaphore::new(1)),
            SessionResources::durable_arc(&h.dir),
        )
        .with_kind(kind),
    )
}

fn card(advisor: SessionId, severity: AdvisorSeverity, note: &str) -> (Advisory, String) {
    let notes = vec![AdvisoryNote {
        severity,
        note: note.into(),
    }];
    let text = crate::advisor::advise::card_text(&notes);
    (
        Advisory {
            advisor,
            step: 1,
            turn: None,
            at_ms: 0,
            notes,
            kind: Default::default(),
        },
        text,
    )
}

fn user_input(text: &str) -> UserInput {
    UserInput {
        channel: None,
        id: None,
        text: text.into(),
        files: Vec::new(),
        ticket: None,
        scope: None,
        defer: false,
    }
}

/// `(step, backlog, state)` of every `AdvisorChanged` for `session`.
fn changes(events: &[AgentEvent], session: SessionId) -> Vec<(u32, u32, AdvisorState)> {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::AdvisorChanged { session: s, status } if *s == session => {
                Some((status.step, status.backlog, status.state))
            }
            _ => None,
        })
        .collect()
}

fn count(events: &[AgentEvent], pick: impl Fn(&AgentEvent) -> bool) -> usize {
    events.iter().filter(|e| pick(e)).count()
}

#[test]
fn hooks_fire_per_batch_and_at_turn_end() {
    crate::with_test_registry(async {
        let scratch = crate::TestDir::new("adv-reg");
        let reg_dir = scratch.0.clone();
        tools::registry::start(&Runtime::new(figment::Figment::new(), reg_dir.clone()))
            .await
            .unwrap();
        // One refused tool round (a batch all the same), then a final answer.
        let mut h = Harness::new(Script::new(vec![tool_call("c1", "nope_tool", &json!({}))])).await;
        let adv = advisors(&h, AdvisorConfig::default()).await;
        let advisor = SessionId::new();
        h.store
            .create_session_kind(
                advisor,
                "openai/textonly",
                Some(h.id),
                Some("advisor"),
                None,
                SessionKind::Advisor,
            )
            .await
            .unwrap();
        adv.ask(Attach {
            link: AdvisorLink {
                session: h.id,
                advisor,
                model: "openai/textonly".into(),
                effort: None,
                cursor: 0,
                state: AdvisorState::Idle,
            },
            context_limit: 0,
            warning: None,
        })
        .await
        .unwrap();
        let actor = h.actor("openai/textonly").await;
        actor.tell(SetAdvisorOn(true)).await.unwrap();
        actor.tell(user_input("go")).await.unwrap();
        let events = h.drain().await;
        // (1) a step after the batch and one at the end; the second coalesces
        // behind the first review (no supervisor answers it here).
        let steps: Vec<_> = changes(&events, h.id)
            .into_iter()
            .filter(|(_, _, s)| *s != AdvisorState::Off)
            .collect();
        assert!(
            steps.contains(&(1, 0, AdvisorState::Reviewing)),
            "mid-turn step: {steps:?}"
        );
        assert!(
            steps.contains(&(2, 1, AdvisorState::Reviewing)),
            "terminal step coalesced: {steps:?}"
        );
        let link = h.store.advisor_link(h.id).await.unwrap().unwrap();
        // The cursor covers the opener, the tool card... everything rendered at step 1.
        assert!(link.cursor >= 2, "{link:?}");
        assert_eq!(link.state, AdvisorState::Reviewing);
        // The advised prompt carries the advisory paragraph while linked.
        let reqs = h.requests().await;
        assert!(system(&reqs[0]).contains(frame::ADVISORY_PARAGRAPH));
        // Off again: the flag is cleared and no step is counted.
        actor.tell(SetAdvisorOn(false)).await.unwrap();
        actor.tell(user_input("again")).await.unwrap();
        let events = h.drain().await;
        assert!(changes(&events, h.id).is_empty(), "{events:?}");
    });
}

#[test]
fn passive_cards_never_open_a_turn_and_land_at_the_boundary() {
    crate::with_test_registry(async {
        let scratch = crate::TestDir::new("adv-reg");
        let reg_dir = scratch.0.clone();
        tools::registry::start(&Runtime::new(figment::Figment::new(), reg_dir.clone()))
            .await
            .unwrap();
        // Reply 1 answers the idle blocker's turn; reply 2 (the `go` turn's
        // first call) is held 1.5 s: the busy window the card arrives in.
        let mut h = Harness::new(Script::new(vec![
            sse("ok.", "stop", MAIN_USAGE),
            tool_call("c1", "nope_tool", &json!({})).set_delay(Duration::from_millis(1500)),
        ]))
        .await;
        let advisor = SessionId::new();
        let actor = h.actor("openai/textonly").await;
        // (2) idle: an aside is a row, not a turn.
        let (a, t) = card(advisor, AdvisorSeverity::Nit, "idle nit");
        actor
            .tell(AppendAdvisory {
                advisory: a,
                text: t,
                model_visible: true,
            })
            .await
            .unwrap();
        let events = h
            .drain_until(|e| matches!(e, AgentEvent::AdvisoryAdded { .. }))
            .await;
        assert_eq!(
            count(&events, |e| matches!(e, AgentEvent::TurnStarted { .. })),
            0
        );
        let rows = h.store.rows_since(h.id, 0).await.unwrap();
        assert!(matches!(
            rows.last(),
            Some((_, HistoryEntry::Advisory { .. }))
        ));
        // (3) idle: a concern steer is preserved (card only); a blocker opens a turn.
        let (a, t) = card(advisor, AdvisorSeverity::Concern, "idle concern");
        actor
            .tell(SteerAdvisory {
                advisory: a,
                text: t,
                ticket: None,
            })
            .await
            .unwrap();
        let events = h
            .drain_until(|e| matches!(e, AgentEvent::AdvisoryAdded { .. }))
            .await;
        assert_eq!(
            count(&events, |e| matches!(e, AgentEvent::TurnStarted { .. })),
            0,
            "{events:?}"
        );
        let (a, t) = card(advisor, AdvisorSeverity::Blocker, "idle blocker");
        actor
            .tell(SteerAdvisory {
                advisory: a,
                text: t.clone(),
                ticket: None,
            })
            .await
            .unwrap();
        let events = h.drain().await;
        let opened = events.iter().find_map(|e| match e {
            AgentEvent::TurnStarted { text, .. } => Some(text.clone()),
            _ => None,
        });
        assert_eq!(opened.as_deref(), Some(t.as_str()), "{events:?}");
        assert_eq!(
            count(&events, |e| matches!(e, AgentEvent::AdvisoryAdded { .. })),
            1
        );
        // (2) busy: a nit queued mid-stream lands after the tool batch, in
        // the same turn, before the next model call sees it.
        actor.tell(user_input("go")).await.unwrap();
        h.drain_until(|e| matches!(e, AgentEvent::TurnStarted { .. }))
            .await;
        let (a, t) = card(advisor, AdvisorSeverity::Nit, "busy nit");
        actor
            .tell(AppendAdvisory {
                advisory: a,
                text: t.clone(),
                model_visible: true,
            })
            .await
            .unwrap();
        let events = h.drain().await;
        let order: Vec<&str> = events
            .iter()
            .filter_map(|e| match e {
                AgentEvent::ToolCallFinished { .. } => Some("tool"),
                AgentEvent::AdvisoryAdded { .. } => Some("advisory"),
                AgentEvent::TurnStarted { .. } => Some("start"),
                AgentEvent::TurnFinished { .. } => Some("finish"),
                _ => None,
            })
            .collect();
        assert_eq!(order, ["tool", "advisory", "finish"], "{events:?}");
        let reqs = h.requests().await;
        assert!(
            texts(reqs.last().unwrap()).iter().any(|x| x == &t),
            "the card entered the next model call"
        );
    });
}

#[test]
fn stop_preserves_queued_steers_as_cards() {
    crate::with_test_registry(async {
        let scratch = crate::TestDir::new("adv-reg");
        let reg_dir = scratch.0.clone();
        tools::registry::start(&Runtime::new(figment::Figment::new(), reg_dir.clone()))
            .await
            .unwrap();
        let mut h = Harness::new(Script::new(vec![
            sse("slow", "stop", MAIN_USAGE).set_delay(Duration::from_secs(5)),
        ]))
        .await;
        let advisor = SessionId::new();
        let actor = h.actor("openai/textonly").await;
        actor.tell(user_input("go")).await.unwrap();
        h.drain_until(|e| matches!(e, AgentEvent::TurnStarted { .. }))
            .await;
        let (a, t) = card(advisor, AdvisorSeverity::Blocker, "late blocker");
        actor
            .tell(SteerAdvisory {
                advisory: a,
                text: t,
                ticket: None,
            })
            .await
            .unwrap();
        actor.tell(CancelTurn { keep: None }).await.unwrap();
        let events = h
            .drain_until(|e| matches!(e, AgentEvent::AdvisoryAdded { .. }))
            .await;
        let kinds: Vec<&str> = events
            .iter()
            .filter_map(|e| match e {
                AgentEvent::TurnFinished { stop_reason, .. } => Some(stop_reason.as_str()),
                AgentEvent::AdvisoryAdded { .. } => Some("advisory"),
                AgentEvent::TurnStarted { .. } => Some("start"),
                _ => None,
            })
            .collect();
        // Cancelled, then the card — and no turn opened by it (D4).
        assert_eq!(kinds, ["cancelled", "advisory"], "{events:?}");
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(!actor.ask(IsRunning).await.unwrap());
    });
}

#[test]
fn advisor_kind_session_runs_advise_locally_with_the_bare_prompt() {
    crate::with_test_registry(async {
        let scratch = crate::TestDir::new("adv-reg");
        let reg_dir = scratch.0.clone();
        tools::registry::start(&Runtime::new(figment::Figment::new(), reg_dir.clone()))
            .await
            .unwrap();
        let mut h = Harness::new(Script::new(vec![
            tool_call(
                "a1",
                "advise",
                &json!({"note": "run the check first", "severity": "concern"}),
            ),
            tool_call("a2", "todo", &json!({"op": "view"})),
        ]))
        .await;
        let adv = advisors(&h, AdvisorConfig::default()).await;
        // `h.id` is the advised session; the advisor is a second row.
        let advisor = SessionId::new();
        h.store
            .create_session_kind(
                advisor,
                "openai/textonly",
                Some(h.id),
                Some("advisor"),
                None,
                SessionKind::Advisor,
            )
            .await
            .unwrap();
        adv.ask(Attach {
            link: AdvisorLink {
                session: h.id,
                advisor,
                model: "openai/textonly".into(),
                effort: None,
                cursor: 0,
                state: AdvisorState::Idle,
            },
            context_limit: 0,
            warning: None,
        })
        .await
        .unwrap();
        let actor = spawn(&h, advisor, SessionKind::Advisor).await;
        actor
            .tell(user_input("### Session update\n\n**user**:\nhi"))
            .await
            .unwrap();
        let events = h.drain().await;
        let finished: Vec<(String, bool, String)> = events
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
            .collect();
        // `advise` ran loop-locally and the guard acknowledged it; `todo` is
        // not a tool of an advisor.
        assert_eq!(
            finished[0],
            (
                "a1".to_owned(),
                false,
                crate::advisor::guard::ACK_ACCEPTED.to_owned()
            ),
            "{finished:?}"
        );
        assert!(
            finished[1].1 && finished[1].2.contains("todo is not available"),
            "{finished:?}"
        );
        let reqs = h.requests().await;
        let sys = system(&reqs[0]);
        assert!(sys.starts_with("<conventions>"), "{sys}");
        assert!(sys.contains("<project-context>") || sys.contains("Working directory: "));
        assert!(!sys.contains("## Guidance") && !sys.contains("## Runtime"));
        let names: Vec<&str> = reqs[0]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t["function"]["name"].as_str())
            .collect();
        assert!(
            names.contains(&"advise") && !names.contains(&"todo") && !names.contains(&"consult"),
            "{names:?}"
        );
    });
}

/// R1.4 step 6: a masked card (`model_visible: false`) on a busy session is
/// written at once as a row the UIs see, excluded from the model's context
/// and never queued for the batch boundary.
#[test]
fn masked_card_lands_at_once_and_stays_out_of_context() {
    crate::with_test_registry(async {
        let scratch = crate::TestDir::new("adv-reg");
        let reg_dir = scratch.0.clone();
        tools::registry::start(&Runtime::new(figment::Figment::new(), reg_dir.clone()))
            .await
            .unwrap();
        let mut h = Harness::new(Script::new(vec![
            tool_call("c1", "nope_tool", &json!({})).set_delay(Duration::from_millis(1500)),
        ]))
        .await;
        let advisor = SessionId::new();
        let actor = h.actor("openai/textonly").await;
        actor.tell(user_input("go")).await.unwrap();
        h.drain_until(|e| matches!(e, AgentEvent::TurnStarted { .. }))
            .await;
        let (mut a, t) = card(advisor, AdvisorSeverity::Advice, "the answer");
        a.kind = crate::AdvisoryKind::Consultation {
            question: "q?".into(),
        };
        actor
            .tell(AppendAdvisory {
                advisory: a,
                text: t.clone(),
                model_visible: false,
            })
            .await
            .unwrap();
        // The card is announced before the (delayed) tool batch ends.
        let events = h
            .drain_until(|e| matches!(e, AgentEvent::AdvisoryAdded { .. }))
            .await;
        assert_eq!(
            count(&events, |e| matches!(
                e,
                AgentEvent::ToolCallFinished { .. }
            )),
            0,
            "{events:?}"
        );
        let events = h.drain().await;
        assert_eq!(
            count(&events, |e| matches!(e, AgentEvent::AdvisoryAdded { .. })),
            0,
            "not queued again: {events:?}"
        );
        let reqs = h.requests().await;
        assert!(
            reqs.iter().all(|r| !texts(r).iter().any(|x| x == &t)),
            "the masked card never entered a model call"
        );
        let rows = h.store.rows_since(h.id, 0).await.unwrap();
        let masked = rows
            .iter()
            .filter(|(_, e)| {
                matches!(e, HistoryEntry::Advisory { advisory, context: Some(c) }
                    if c.removed && matches!(advisory.kind, crate::AdvisoryKind::Consultation { .. }))
            })
            .count();
        assert_eq!(masked, 1, "{rows:?}");
    });
}
