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
            .append(h.id, &ChatMessage::user(text))
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
            .filter(|t| t.contains("+1|user: message"))
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
    assert!(
        all.contains("older lines folded to fit the view"),
        "the folded prefix line (201 P1: every unit has its draft)"
    );
    assert!(
        !all.contains("user: message 0:"),
        "the oldest lines are folded, not sent"
    );
}
