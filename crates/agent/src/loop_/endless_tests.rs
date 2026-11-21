//! 137 §14 (W2) / 201 §4: the endless frame through the real [`Session`]
//! actor at the fake provider of [`super::vision_fallback_tests`] — two
//! consecutive turns send byte-identical tools and system prompt, the view
//! precedes the carried tail and the input, and request 1 is a byte prefix of
//! request 2; an epochs session is untouched; an in-turn overflow folds as
//! `cut` with the view as head (D11); `/endless off` installs the view head
//! (8.6). The view itself comes from the test hook (`Shared::test_view`).

use kameo::actor::Spawn;
use serde_json::{Value, json};

use super::vision_fallback_tests::{Fake, Harness, system, texts};
use super::*;
use crate::endless::{ViewBlock, ViewRender};

const BLOCK_A: &str = "<chat>\n0+2|user: hello; talk: hi\n";
const BLOCK_B: &str = "2+1|user: what is the plan\n</chat>";

fn view() -> ViewRender {
    ViewRender {
        blocks: vec![
            ViewBlock {
                text: BLOCK_A.into(),
                mark: true,
            },
            ViewBlock {
                text: BLOCK_B.into(),
                mark: true,
            },
        ],
        settled: true,
        unbuilt: 0,
        folded: 0,
    }
}

/// A registry for `rebase`'s `ListTools`; process-global, so one per scenario.
async fn registry() -> crate::TestDir {
    let scratch = crate::TestDir::new("endless-reg");
    let reg_rt = Runtime::new(figment::Figment::new(), scratch.0.clone());
    tools::registry::start(&reg_rt).await.unwrap();
    scratch
}

/// The session actor over the harness's store, the endless turn's view
/// injected when `endless`.
async fn actor(h: &Harness, endless: bool) -> ActorRef<Session> {
    let (epoch, head, history) = h.store.frame(h.id).await.unwrap();
    let session = Session::new(
        h.id,
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
    );
    if endless {
        *session.shared.test_view.lock() = Some(view());
    }
    Session::spawn(session)
}

async fn send(h: &mut Harness, actor: &ActorRef<Session>, text: &str) {
    actor
        .tell(UserInput {
            channel: None,
            id: None,
            text: text.into(),
            files: Vec::new(),
            ticket: None,
            scope: None,
            defer: false,
        })
        .await
        .unwrap();
    h.drain_until(
        |e| matches!(e, AgentEvent::ContextChanged { reason, .. } if reason == "settled"),
    )
    .await;
}

fn tool_names(b: &Value) -> Vec<String> {
    b["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| t["function"]["name"].as_str().map(str::to_owned))
        .collect()
}

/// The main (round) calls: title, summary and harvest frames carry no `tools` key (07 §0).
async fn main_calls(h: &Harness) -> Vec<Value> {
    h.requests()
        .await
        .into_iter()
        .filter(|r| r["tools"].is_array())
        .collect()
}

#[test]
fn endless_turns_carry_the_tail_after_the_frozen_view() {
    crate::with_test_registry(scenario_fresh_turns());
}

async fn scenario_fresh_turns() {
    let _reg_dir = registry().await;
    let mut h = Harness::new(Fake::compact_held(Duration::ZERO)).await;
    let actor = actor(&h, true).await;
    send(&mut h, &actor, "First question.").await;
    send(&mut h, &actor, "Second question.").await;
    let requests = main_calls(&h).await;
    assert_eq!(requests.len(), 2, "one call per turn");
    let (r1, r2) = (&requests[0], &requests[1]);

    // 7.3: tools and system byte-identical; the date left the system prompt,
    // VIEW_DOC closes it; the three tools are in the sorted list.
    assert_eq!(system(r1), system(r2));
    assert_eq!(r1["tools"], r2["tools"]);
    assert!(system(r1).ends_with(Endless::view_doc()), "VIEW_DOC last");
    assert!(!system(r1).contains("\nDate: "), "{}", system(r1));
    let names = tool_names(r1);
    assert!(names.windows(2).all(|w| w[0] <= w[1]), "sorted: {names:?}");
    for t in ["zoom", "recall", "date"] {
        assert!(names.iter().any(|n| n == t), "{t} in {names:?}");
    }

    // 201 D2/D3: [system][view blocks][tail][input] — the view first, then
    // turn 1 as it was sent, then the new input; no head.
    let t2 = texts(r2);
    assert_eq!(t2.len(), 6, "{t2:?}");
    assert_eq!(&t2[1..3], [BLOCK_A, BLOCK_B]);
    assert!(t2[3].ends_with("First question."), "{}", t2[3]);
    assert!(
        t2[5].starts_with("<time now=\""),
        "the opener carries the date: {}",
        t2[5]
    );
    assert!(t2[5].ends_with("Second question."));
    assert!(!t2.iter().any(|t| t.contains("<context_summary")));
    let roles: Vec<&str> = r2["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["role"].as_str().unwrap())
        .collect();
    assert_eq!(
        roles,
        ["system", "user", "user", "user", "assistant", "user"]
    );

    // 201 D4: within a generation request 1 is a byte prefix of request 2.
    let m1 = r1["messages"].as_array().unwrap();
    let m2 = r2["messages"].as_array().unwrap();
    assert_eq!(
        serde_json::to_string(&m1[..]).unwrap(),
        serde_json::to_string(&m2[..m1.len()]).unwrap(),
        "request 1 is a prefix of request 2"
    );

    // Everything persisted — both turns' rows are in the store.
    let (_, head, history) = h.store.frame(h.id).await.unwrap();
    assert!(head.is_none());
    assert_eq!(history.len(), 4, "two inputs, two replies");
}

#[test]
fn epochs_session_keeps_the_dated_prompt_and_no_endless_tools() {
    crate::with_test_registry(scenario_epochs());
}

async fn scenario_epochs() {
    let _reg_dir = registry().await;
    let mut h = Harness::new(Fake::compact_held(Duration::ZERO)).await;
    let actor = actor(&h, false).await;
    send(&mut h, &actor, "First question.").await;
    send(&mut h, &actor, "Second question.").await;
    let requests = main_calls(&h).await;
    let r2 = &requests[1];
    assert!(system(r2).contains("\nDate: "));
    assert!(!system(r2).contains(Endless::view_doc()));
    let names = tool_names(r2);
    assert!(
        !names
            .iter()
            .any(|n| n == "zoom" || n == "recall" || n == "date"),
        "{names:?}"
    );
    // The carried history: turn 1's input and reply precede turn 2's input.
    let t2 = texts(r2);
    assert!(t2.iter().any(|t| t.contains("First question.")));
}

#[test]
fn overflow_in_an_endless_turn_cuts_with_the_view_as_head() {
    crate::with_test_registry(scenario_overflow());
}

async fn scenario_overflow() {
    let _reg_dir = registry().await;
    let mut h = Harness::new(Fake::tool_call_then_overflow(
        "todo",
        &json!({"op": "view"}),
    ))
    .await;
    let actor = actor(&h, true).await;
    actor
        .tell(UserInput {
            channel: None,
            id: None,
            text: "Plan it.".into(),
            files: Vec::new(),
            ticket: None,
            scope: None,
            defer: false,
        })
        .await
        .unwrap();
    let events = h
        .drain_until(
            |e| matches!(e, AgentEvent::ContextChanged { reason, .. } if reason == "settled"),
        )
        .await;
    let compacted: Vec<(u32, CompactStyle)> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Compacted { epoch, style, .. } => Some((*epoch, *style)),
            _ => None,
        })
        .collect();
    assert_eq!(compacted, [(1, CompactStyle::Cut)], "{events:?}");
    let requests = main_calls(&h).await;
    // tool round, the overflowing call, the retry after the cut — no summariser call.
    assert_eq!(requests.len(), 3, "{requests:?}");
    assert!(
        !requests.iter().any(|r| texts(r)
            .iter()
            .any(|t| t.contains(compact::COMPACT_INSTRUCTION))),
        "no summariser"
    );
    let retry = texts(&requests[2]);
    // D11: the head is the view (style cut), and the view blocks are no longer
    // prepended separately.
    let head = &retry[1];
    assert!(
        head.starts_with("<context_summary epoch=\"1\" style=\"cut\">"),
        "{head}"
    );
    assert!(head.contains(BLOCK_A) && head.contains(BLOCK_B), "{head}");
    assert_eq!(retry.iter().filter(|t| t.as_str() == BLOCK_A).count(), 0);
    let (epoch, stored_head, _) = h.store.frame(h.id).await.unwrap();
    assert_eq!(epoch, 1);
    assert!(text_of(&stored_head.unwrap().content).contains("</chat>"));
}

#[test]
fn endless_off_installs_the_view_head_as_the_next_epoch() {
    crate::with_test_registry(scenario_off());
}

async fn scenario_off() {
    let _reg_dir = registry().await;
    let mut h = Harness::new(Fake::compact_held(Duration::ZERO)).await;
    let actor = actor(&h, true).await;
    send(&mut h, &actor, "First question.").await;
    drop(actor);
    // A `Turn`-less host over the stored frame, as `SetMemoryNow` runs one.
    let (mut turn, _) = h.turn("openai/textonly", CancellationToken::new()).await;
    let text = "<context_summary epoch=\"1\" style=\"endless\">\n<chat>\n0+2|user: first; talk: ok\n</chat>\n</context_summary>";
    // 201 (P2): the tail from the reply on stays as the epoch's history.
    let reply_seq = turn.state.history[1].0;
    let folded = turn
        .install_view_head(1, text, Some(reply_seq))
        .await
        .unwrap();
    assert_eq!(folded, 1, "the input; the reply is the kept tail");
    assert_eq!(turn.state.epoch, 1);
    assert_eq!(turn.state.history.len(), 1, "the tail kept as history");
    let (epoch, head, tail) = h.store.frame(h.id).await.unwrap();
    assert_eq!(epoch, 1);
    assert_eq!(text_of(&head.unwrap().content), text);
    assert_eq!(tail.len(), 1, "the next epochs turn sees head + the tail");
    assert_eq!(
        compact::style_of_head(text),
        CompactStyle::Summary,
        "an unknown style reads as summary"
    );
    // A session with no rows gets no head.
    let (mut empty, _) = h.turn("openai/textonly", CancellationToken::new()).await;
    empty.state.history.clear();
    assert_eq!(empty.install_view_head(2, text, None).await.unwrap(), 0);
}

/// 137G: refuses the first round call as too long (Anthropic's wording),
/// then answers like [`Fake`].
struct RefuseFirst {
    inner: Fake,
    refused: std::sync::atomic::AtomicBool,
}

impl wiremock::Respond for RefuseFirst {
    fn respond(&self, r: &wiremock::Request) -> wiremock::ResponseTemplate {
        if super::vision_fallback_tests::body(r)["tools"].is_array()
            && !self.refused.swap(true, Ordering::SeqCst)
        {
            return wiremock::ResponseTemplate::new(400).set_body_string(
                json!({"error": {"type": "invalid_request_error",
                    "message": "prompt is too long: 300000 tokens > 128000 maximum"}})
                .to_string(),
            );
        }
        self.inner.respond(r)
    }
}

#[test]
fn endless_turn_view_is_bounded_and_shrinks_after_a_refusal() {
    crate::with_test_registry(scenario_bounded_view());
}

/// 137G G1 + G5 + G10 through the real service: 5,000 unsummarized lines
/// (settling paused) → the turn's view fits the turn budget (half the 128k
/// window less the system prompt and the reserve), never the raw backlog;
/// the provider's "too long" refusal re-renders it once at M/N × 0.85 and
/// the retried request carries the fold note.
async fn scenario_bounded_view() {
    let _reg_dir = registry().await;
    // R1-10: a model id no other scenario teaches (sizing stays at 2.2 B/token).
    crate::tokens::forget_for_tests("openai/bounded");
    let mut h = Harness::new(RefuseFirst {
        inner: Fake::compact_held(Duration::ZERO),
        refused: std::sync::atomic::AtomicBool::new(false),
    })
    .await;
    let endless = Endless::new(
        h.store.clone(),
        h.rt.clone(),
        h.client.clone(),
        Auth::stub(&h.dir),
        h.catalog.clone(),
        crate::endless::EndlessConfig::default(),
        None,
    );
    h.store
        .set_session_memory(h.id, MemoryMode::Endless)
        .await
        .unwrap();
    let attached = crate::endless::Attached {
        model: h.resolve("openai/bounded"),
        effort: None,
        agent: "theuth".into(),
        store: None,
    };
    endless.attach(h.id, attached).await.unwrap();
    // A rendered turn first, so the rows below are live units, not a backfill.
    endless.render_for_turn(h.id).await.unwrap();
    endless.pause(h.id, true).await.unwrap();
    for k in 0..5_000 {
        let text = format!(
            "message {k}: {}",
            "the backlog line repeats here ".repeat(20)
        );
        h.store
            // 201 P3: talk units (a long user message needs no model, D8).
            .append(h.id, &ChatMessage::assistant(text))
            .await
            .unwrap();
    }
    // 201 P1: settle never waits (paused or not); it wakes the pump, which
    // ingests the rows and gives each its draft.
    let t0 = std::time::Instant::now();
    endless.settle(h.id, Duration::from_secs(30)).await;
    assert!(
        t0.elapsed() < Duration::from_secs(5),
        "settle returns at once"
    );
    let mut lines = 0;
    for _ in 0..200 {
        lines = endless
            .memory(h.id)
            .await
            .unwrap()
            .backlog
            .map_or(0, |b| b.lines);
        if lines == 5_000 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(lines, 5_000);
    let (epoch, head, history) = h.store.frame(h.id).await.unwrap();
    let mut session = Session::new(
        h.id,
        0,
        (h.resolve("openai/bounded"), None),
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
    );
    session.shared.endless = Some(endless.clone());
    let actor = Session::spawn(session);
    send(&mut h, &actor, "What did the backlog say?").await;
    let calls = main_calls(&h).await;
    assert_eq!(calls.len(), 2, "refused once, retried once");
    let view_bytes = |b: &Value| -> usize {
        texts(b)
            .iter()
            // 201 §10 (P6b): the `<recall>` prefetch note after the input is no view.
            .filter(|t| t.contains("+1|talk: message") && !t.contains("<recall>"))
            .map(String::len)
            .sum()
    };
    let (first, second) = (view_bytes(&calls[0]), view_bytes(&calls[1]));
    // Turn target: 64k tokens × 2.2 B = 140.8 KB, less the system prompt and
    // the reserve (a quarter of it: 35.2 KB).
    assert!(
        first > 0 && first <= 140_800 - 35_200,
        "first view {first} B"
    );
    assert!(
        second < first && second as f64 <= first as f64 * 128.0 / 300.0 * 0.85 + 2_000.0,
        "{first} → {second}"
    );
    let all = texts(&calls[1]).concat();
    // 201: with drafts nothing is unsummarized; the note says the view shrank.
    assert!(
        all.contains("the summary view was shrunk to fit")
            || all.contains("older unsummarized lines were folded to fit"),
        "the G10 note"
    );
    // 201 P3: a paused session still builds its draft merges, so the old
    // lines fold into merged lines (or, without merges, the prefix line).
    assert!(
        !all.contains("0+1|talk: message 0:"),
        "the oldest lines are folded, not sent one by one"
    );
}

/// 201 P5: answers each of the first `rounds` main calls with one `todo`
/// call whose arguments carry `pad` bytes (a large round), then like [`Fake`].
struct Rounds {
    inner: Fake,
    rounds: usize,
    pad: usize,
    n: std::sync::atomic::AtomicUsize,
}

impl Rounds {
    fn new(rounds: usize, pad: usize) -> Self {
        Self {
            inner: Fake::compact_held(Duration::ZERO),
            rounds,
            pad,
            n: std::sync::atomic::AtomicUsize::new(0),
        }
    }
}

impl wiremock::Respond for Rounds {
    fn respond(&self, r: &wiremock::Request) -> wiremock::ResponseTemplate {
        if super::vision_fallback_tests::body(r)["tools"].is_array() {
            let k = self.n.fetch_add(1, Ordering::SeqCst);
            if k < self.rounds {
                let args = json!({"op": "view", "pad": format!("{k} {}", "p".repeat(self.pad))});
                return super::vision_fallback_tests::sse_tool_call(
                    &format!("call_{k}"),
                    "todo",
                    &args.to_string(),
                );
            }
        }
        self.inner.respond(r)
    }
}

/// 201 P5: the session actor on the real endless service (memory
/// `endless`), attached with `openai/<model>` and paused (drafts, no
/// compactor call) when `attach`.
async fn real_endless(h: &Harness, model: &str, attach: bool) -> (Endless, ActorRef<Session>) {
    real_endless_with(h, model, attach, crate::endless::EndlessConfig::default()).await
}

/// [`real_endless`] with `cfg`.
async fn real_endless_with(
    h: &Harness,
    model: &str,
    attach: bool,
    cfg: crate::endless::EndlessConfig,
) -> (Endless, ActorRef<Session>) {
    let id = format!("openai/{model}");
    crate::tokens::forget_for_tests(&id);
    let endless = Endless::new(
        h.store.clone(),
        h.rt.clone(),
        h.client.clone(),
        Auth::stub(&h.dir),
        h.catalog.clone(),
        cfg,
        None,
    );
    h.store
        .set_session_memory(h.id, MemoryMode::Endless)
        .await
        .unwrap();
    if attach {
        attach_paused(h, &endless, &id).await;
    }
    let (epoch, head, history) = h.store.frame(h.id).await.unwrap();
    let mut session = Session::new(
        h.id,
        0,
        (h.resolve(&id), None),
        None,
        SessionState::resume(epoch, head, history),
        // The 200-round scenario runs past the default `max_tool_rounds` (32).
        Arc::new(AgentConfig {
            max_tool_rounds: 250,
            ..AgentConfig::default()
        }),
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
    );
    session.shared.endless = Some(endless.clone());
    (endless, Session::spawn(session))
}

async fn attach_paused(h: &Harness, endless: &Endless, model: &str) {
    let attached = crate::endless::Attached {
        model: h.resolve(model),
        effort: None,
        agent: "theuth".into(),
        store: None,
    };
    endless.attach(h.id, attached).await.unwrap();
    endless.render_for_turn(h.id).await.unwrap();
    endless.pause(h.id, true).await.unwrap();
}

/// One turn's events, through its `settled`.
async fn turn(h: &mut Harness, actor: &ActorRef<Session>, text: &str) -> Vec<AgentEvent> {
    actor
        .tell(UserInput {
            channel: None,
            id: None,
            text: text.into(),
            files: Vec::new(),
            ticket: None,
            scope: None,
            defer: false,
        })
        .await
        .unwrap();
    h.drain_until(|e| matches!(e, AgentEvent::ContextChanged { reason, .. } if reason == "settled"))
        .await
}

fn compactions(events: &[AgentEvent]) -> Vec<CompactStyle> {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Compacted { style, .. } => Some(*style),
            _ => None,
        })
        .collect()
}

/// `(tool call ids of assistant messages, tool_call_ids of tool messages)`.
fn call_ids(b: &Value) -> (Vec<String>, Vec<String>) {
    let msgs = b["messages"].as_array().cloned().unwrap_or_default();
    let calls = msgs
        .iter()
        .filter(|m| m["role"] == "assistant")
        .flat_map(|m| m["tool_calls"].as_array().cloned().unwrap_or_default())
        .filter_map(|c| c["id"].as_str().map(str::to_owned))
        .collect();
    let results = msgs
        .iter()
        .filter(|m| m["role"] == "tool")
        .filter_map(|m| m["tool_call_id"].as_str().map(str::to_owned))
        .collect();
    (calls, results)
}

#[test]
fn a_long_turn_completes_through_in_turn_trims_without_the_classic_cut() {
    crate::with_test_registry(scenario_in_turn());
}

/// 201 §4.3 "In turn" (P5) through the real loop and service (the §14 P5
/// gate): 200 rounds of 6 KB each overrun the turn allowance several times; every overrun trims
/// the oldest whole rounds into a new generation and the turn goes on — no
/// 07 §8.5 cut, a call never without its results, the current round always
/// sent, and the last request carries the newest generation's frozen view.
async fn scenario_in_turn() {
    let _reg_dir = registry().await;
    let rounds = 200;
    let mut h = Harness::new(Rounds::new(rounds, 6_000)).await;
    let (endless, actor) = real_endless(&h, "inturn", true).await;
    let events = turn(&mut h, &actor, "Run the long job.").await;
    assert_eq!(compactions(&events), [], "no classic cut");
    let calls = main_calls(&h).await;
    assert_eq!(calls.len(), rounds + 1, "every round ran");
    for (i, b) in calls.iter().enumerate() {
        let (c, r) = call_ids(b);
        assert_eq!(c, r, "request {i}: every call with its results, in order");
        if i > 0 {
            assert!(
                c.contains(&format!("call_{}", i - 1)),
                "request {i}: the current round is sent: {c:?}"
            );
        }
        assert!(!texts(b).iter().any(|t| t.contains("<context_summary")));
    }
    let last = calls.last().unwrap();
    assert!(
        !call_ids(last).0.contains(&"call_0".to_owned()),
        "the oldest rounds left the tail"
    );
    let g = endless.generation(h.id).await.unwrap().unwrap();
    assert!(g.n >= 3, "several in-turn trims: generation {}", g.n);
    let sent = texts(last);
    for b in &g.render.blocks {
        assert!(
            sent.iter().any(|t| t == &b.text),
            "the frozen view of generation {} is sent",
            g.n
        );
    }
    // The next turn boundary keeps the generation (the tail is under the high mark).
    let (_, head, _) = h.store.frame(h.id).await.unwrap();
    assert!(head.is_none(), "no epoch head was written");
}

#[test]
fn one_round_too_large_for_any_trim_falls_back_to_the_classic_cut() {
    // Over the turn allowance (≤ 64k tokens × 4 B), under the window's preflight.
    crate::with_test_registry(scenario_one_big_round(300_000, "onebig"));
}

/// 201 P5: a single round over the whole allowance cannot be trimmed (the
/// current round is never cut): the 07 §8.5 cut (137 D11, style `cut`, the
/// view as head) still runs, once, and the turn completes.
async fn scenario_one_big_round(pad: usize, model: &str) {
    let _reg_dir = registry().await;
    let mut h = Harness::new(Rounds::new(1, pad)).await;
    let (_endless, actor) = real_endless(&h, model, true).await;
    let events = turn(&mut h, &actor, "Read the huge file.").await;
    // Exactly one fallback cut (the D11 view head, else the summary when
    // the fresh generation's view is still empty).
    assert_eq!(compactions(&events).len(), 1, "{events:?}");
    let calls = main_calls(&h).await;
    assert_eq!(calls.len(), 2);
    assert!(
        texts(&calls[1])
            .iter()
            .any(|t| t.starts_with("<context_summary")),
        "the retried round runs on the head"
    );
}

#[test]
fn a_turn_boundary_trim_through_the_loop_starts_a_generation() {
    crate::with_test_registry(scenario_boundary_trim());
}

/// 201 §4.3 (P2, carried to P5): turns of 20 KB inputs through the real loop
/// and service: once the tail passes the high mark the next turn starts a
/// new generation, carries the last turn word for word, not the first, and
/// its view covers what was cut.
async fn scenario_boundary_trim() {
    let _reg_dir = registry().await;
    let mut h = Harness::new(Fake::compact_held(Duration::ZERO)).await;
    let (endless, actor) = real_endless(&h, "boundary", true).await;
    let big = |k: usize| format!("INPUT-{k} {}", "word ".repeat(4_000));
    for k in 0..5 {
        let events = turn(&mut h, &actor, &big(k)).await;
        assert_eq!(compactions(&events), []);
    }
    let g = endless.generation(h.id).await.unwrap().unwrap();
    assert!(g.n >= 2, "a trim happened: generation {}", g.n);
    let calls = main_calls(&h).await;
    let last = texts(calls.last().unwrap()).concat();
    assert!(last.contains(&big(4)), "the input");
    assert!(
        last.contains(&big(3)),
        "the last turn is kept word for word"
    );
    assert!(!last.contains(&big(0)), "the first turn left the tail");
    let tail_bytes = endless.memory(h.id).await.unwrap().tail_bytes;
    assert!(
        tail_bytes > 0 && tail_bytes <= 24_000,
        "the tail opened at or under the low mark: {tail_bytes}"
    );
    for b in &g.render.blocks {
        assert!(last.contains(&b.text), "the frozen view is sent");
    }
    assert!(
        g.render.text().contains("INPUT-0"),
        "the view covers the cut turn"
    );
}

#[test]
fn endless_tools_are_listed_from_creation_and_refuse_without_a_tree() {
    crate::with_test_registry(scenario_listed());
}

/// 201 §11 (P5): an epochs session lists no `zoom`/`recall`/`date`; an
/// endless one lists them before it is attached (no tree) and the list is
/// byte-identical once its tree exists.
async fn scenario_listed() {
    let _reg_dir = registry().await;
    let mut h = Harness::new(Fake::compact_held(Duration::ZERO)).await;
    let (endless, actor) = real_endless(&h, "listed", false).await;
    h.store
        .set_session_memory(h.id, MemoryMode::Epochs)
        .await
        .unwrap();
    send(&mut h, &actor, "Epochs first.").await;
    h.store
        .set_session_memory(h.id, MemoryMode::Endless)
        .await
        .unwrap();
    send(&mut h, &actor, "No tree yet.").await;
    attach_paused(&h, &endless, "openai/listed").await;
    send(&mut h, &actor, "Now with a tree.").await;
    let calls = main_calls(&h).await;
    assert_eq!(calls.len(), 3);
    let epochs = tool_names(&calls[0]);
    assert!(!epochs.iter().any(|n| n == "zoom"), "{epochs:?}");
    assert!(tool_names(&calls[1]).iter().any(|n| n == "zoom"));
    assert_eq!(
        calls[1]["tools"], calls[2]["tools"],
        "same list before and after the tree"
    );
}

#[test]
fn compact_in_an_idle_endless_chat_trims_now() {
    crate::with_test_registry(scenario_trim_now());
}

/// 201 P5 (B): `/compact` (`CompactNow`, what `endless_fold` routes to) in
/// an idle endless chat whose tail is over the low mark but under the high
/// mark trims at once: a new generation, the tail at or under the low mark,
/// the last turn kept; the next turn carries it and not the first.
async fn scenario_trim_now() {
    let _reg_dir = registry().await;
    let mut h = Harness::new(Fake::compact_held(Duration::ZERO)).await;
    let (endless, actor) = real_endless(&h, "trimnow", true).await;
    let big = |k: usize| format!("INPUT-{k} {}", "word ".repeat(4_000));
    for k in 0..3 {
        turn(&mut h, &actor, &big(k)).await;
    }
    let g0 = endless.generation(h.id).await.unwrap().unwrap().n;
    actor
        .ask(CompactNow {
            style: None,
            head: None,
        })
        .await
        .expect("a trim now");
    // The `/compact` handback publishes its own `settled`.
    h.drain_until(
        |e| matches!(e, AgentEvent::ContextChanged { reason, .. } if reason == "settled"),
    )
    .await;
    let g1 = endless.generation(h.id).await.unwrap().unwrap();
    assert_eq!(g1.n, g0 + 1, "a new generation at once");
    let tail = endless.memory(h.id).await.unwrap().tail_bytes;
    assert!(tail > 0 && tail <= 24_000, "tail {tail}");
    turn(&mut h, &actor, "And now?").await;
    let calls = main_calls(&h).await;
    let req = calls.last().unwrap();
    let last = texts(req).concat();
    assert!(last.contains(&big(2)), "the last turn is kept");
    // The carried tail is turn 2 alone: one assistant reply, not three.
    let replies = req["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == "assistant")
        .count();
    let shape: Vec<String> = req["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| {
            let t: String = m["content"].to_string().chars().take(60).collect();
            format!("{}:{t}", m["role"])
        })
        .collect();
    assert_eq!(replies, 1, "turns 0 and 1 left the tail: {shape:#?}");
}

#[test]
fn compact_with_nothing_to_cut_says_so() {
    crate::with_test_registry(scenario_nothing_to_cut());
}

/// 201 P5 (B): a single small turn: nothing to cut, one plain line, no new
/// generation.
async fn scenario_nothing_to_cut() {
    let _reg_dir = registry().await;
    let mut h = Harness::new(Fake::compact_held(Duration::ZERO)).await;
    let (endless, actor) = real_endless(&h, "nothingcut", true).await;
    turn(&mut h, &actor, "Hello.").await;
    let g0 = endless.generation(h.id).await.unwrap().unwrap().n;
    let err = actor
        .ask(CompactNow {
            style: None,
            head: None,
        })
        .await
        .expect_err("nothing to cut")
        .to_string();
    assert!(err.starts_with("Nothing to cut"), "{err}");
    assert!(!err.contains('\n'));
    assert_eq!(endless.generation(h.id).await.unwrap().unwrap().n, g0);
}

/// 201 P5 (T): like [`Rounds`], and refuses main call `at` once as too long.
struct RefuseAt {
    inner: Rounds,
    at: usize,
    seen: std::sync::atomic::AtomicUsize,
}

impl wiremock::Respond for RefuseAt {
    fn respond(&self, r: &wiremock::Request) -> wiremock::ResponseTemplate {
        if super::vision_fallback_tests::body(r)["tools"].is_array()
            && self.seen.fetch_add(1, Ordering::SeqCst) == self.at
        {
            return wiremock::ResponseTemplate::new(400).set_body_string(
                json!({"error": {"type": "invalid_request_error",
                    "message": "prompt is too long: 300000 tokens > 128000 maximum"}})
                .to_string(),
            );
        }
        self.inner.respond(r)
    }
}

#[test]
fn a_too_long_refusal_trims_in_turn_and_retries() {
    crate::with_test_registry(scenario_refusal_trim());
}

/// 201 P5 (T): a provider "prompt too long" refusal mid-turn runs the
/// in-turn trim once (a new generation), the round is retried and the turn
/// completes; no classic cut, no view shrink.
async fn scenario_refusal_trim() {
    let _reg_dir = registry().await;
    let rounds = 5;
    let mut h = Harness::new(RefuseAt {
        inner: Rounds::new(rounds, 2_000),
        at: rounds,
        seen: std::sync::atomic::AtomicUsize::new(0),
    })
    .await;
    let (endless, actor) = real_endless(&h, "refusal", true).await;
    let events = turn(&mut h, &actor, "Go.").await;
    assert_eq!(compactions(&events), [], "no classic cut");
    assert!(
        !events.iter().any(|e| matches!(e, AgentEvent::Error { .. })),
        "{events:?}"
    );
    let calls = main_calls(&h).await;
    assert_eq!(calls.len(), rounds + 2, "one refused call, retried once");
    let g = endless.generation(h.id).await.unwrap().unwrap();
    assert_eq!(g.n, 2, "one in-turn trim");
    let retry = &calls[rounds + 1];
    let (c, r) = call_ids(retry);
    assert_eq!(c, r);
    assert!(
        c.contains(&format!("call_{}", rounds - 1)),
        "the current round is sent"
    );
    let sent = texts(retry);
    assert!(
        !sent
            .iter()
            .any(|t| t.contains("note: the summary view was shrunk"))
    );
    for b in &g.render.blocks {
        assert!(
            sent.iter().any(|t| t == &b.text),
            "the new frozen view is sent"
        );
    }
}

/// 201 P6c: answers the first main call with one `name(args)` tool call,
/// then like [`Fake`].
struct CallOnce {
    inner: Fake,
    name: &'static str,
    args: Value,
    done: std::sync::atomic::AtomicBool,
}

impl CallOnce {
    fn new(name: &'static str, args: Value) -> Self {
        Self {
            inner: Fake::compact_held(Duration::ZERO),
            name,
            args,
            done: std::sync::atomic::AtomicBool::new(false),
        }
    }
}

impl wiremock::Respond for CallOnce {
    fn respond(&self, r: &wiremock::Request) -> wiremock::ResponseTemplate {
        if super::vision_fallback_tests::body(r)["tools"].is_array()
            && !self.done.swap(true, Ordering::SeqCst)
        {
            return super::vision_fallback_tests::sse_tool_call(
                "call_once",
                self.name,
                &self.args.to_string(),
            );
        }
        self.inner.respond(r)
    }
}

/// The request's ledger snapshot block (the text holding `<ledger_snapshot>`).
fn snapshot(b: &Value) -> Option<String> {
    texts(b)
        .into_iter()
        .find(|t| t.contains("<ledger_snapshot>"))
}

#[test]
fn a_ledger_entry_is_noted_then_frozen_into_the_next_snapshot() {
    crate::with_test_registry(scenario_ledger_turns());
}

/// 201 §9 (P6c, the P6a full-turn gate): the model's `ledger` add shows as a
/// `<ledger>` note right after the next turn's input; after a trim the
/// snapshot block carries the entry, the note has left the tail, and the
/// block is byte-identical over the generation's following turns.
async fn scenario_ledger_turns() {
    let _reg_dir = registry().await;
    let mut h = Harness::new(CallOnce::new(
        "ledger",
        json!({"section": "goals", "op": "add", "text": "ship the zebra gateway"}),
    ))
    .await;
    let (endless, actor) = real_endless(&h, "ledgerturns", true).await;
    turn(&mut h, &actor, "Our goal: ship the zebra gateway.").await;
    let calls = main_calls(&h).await;
    assert!(tool_names(&calls[0]).iter().any(|n| n == "ledger"));
    let (c, r) = call_ids(calls.last().unwrap());
    assert_eq!(
        (c, r),
        (vec!["call_once".to_owned()], vec!["call_once".to_owned()])
    );

    let big = |k: usize| format!("INPUT-{k} {}", "word ".repeat(4_000));
    turn(&mut h, &actor, &big(1)).await;
    let req = main_calls(&h).await.pop().unwrap();
    let sent = texts(&req);
    let input = sent.iter().position(|t| t.contains("INPUT-1")).unwrap();
    assert!(
        sent[input + 1].contains("<ledger>") && sent[input + 1].contains("ship the zebra gateway"),
        "the note follows the input: {:?}",
        &sent[input + 1]
    );
    if let Some(before) = snapshot(&req) {
        assert!(!before.contains("zebra"), "the entry waits for the trim");
    }

    turn(&mut h, &actor, &big(2)).await;
    let g0 = endless.generation(h.id).await.unwrap().unwrap().n;
    actor
        .ask(CompactNow {
            style: None,
            head: None,
        })
        .await
        .expect("a trim now");
    h.drain_until(
        |e| matches!(e, AgentEvent::ContextChanged { reason, .. } if reason == "settled"),
    )
    .await;
    assert_eq!(endless.generation(h.id).await.unwrap().unwrap().n, g0 + 1);
    turn(&mut h, &actor, "And now?").await;
    turn(&mut h, &actor, "Still there?").await;
    let calls = main_calls(&h).await;
    let (a, b) = (&calls[calls.len() - 2], &calls[calls.len() - 1]);
    let snap = snapshot(a).expect("a snapshot block after the trim");
    assert!(snap.contains("ship the zebra gateway"), "{snap}");
    assert_eq!(
        snapshot(b).as_deref(),
        Some(snap.as_str()),
        "frozen for the generation"
    );
    for req in [a, b] {
        assert!(
            !texts(req).iter().any(|t| t.contains("<ledger>")),
            "the note left the tail"
        );
    }
}

#[test]
fn with_the_ledger_off_there_is_no_tool_and_no_block() {
    crate::with_test_registry(scenario_ledger_off());
}

/// 201 §9 (P6c): `ledger: false` — no `ledger` tool, no snapshot block.
async fn scenario_ledger_off() {
    let _reg_dir = registry().await;
    let mut h = Harness::new(Fake::compact_held(Duration::ZERO)).await;
    let cfg = crate::endless::EndlessConfig {
        ledger: false,
        ..crate::endless::EndlessConfig::default()
    };
    let (_endless, actor) = real_endless_with(&h, "ledgeroff", true, cfg).await;
    turn(&mut h, &actor, "Hello.").await;
    turn(&mut h, &actor, "Again.").await;
    for req in main_calls(&h).await {
        assert!(!tool_names(&req).iter().any(|n| n == "ledger"));
        assert_eq!(snapshot(&req), None);
    }
}
