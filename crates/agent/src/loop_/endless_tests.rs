//! 137 §14 (W2): the endless frame through the real [`Session`] actor at the
//! fake provider of [`super::vision_fallback_tests`] — two consecutive turns
//! send byte-identical tools and system prompt, the view precedes the input,
//! no head and nothing of the previous turn is carried; an epochs session is
//! untouched; an in-turn overflow folds as `cut` with the view as head
//! (D11); `/endless off` installs the view head (8.6). The view itself comes
//! from the test hook (`Shared::test_view`): the service's rendering is W1's.

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
fn endless_turns_rebuild_the_frame_from_the_view() {
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

    // D4: [system][view blocks][input] — the view before the input, one user
    // message per block, no head, nothing of turn 1 in turn 2's request.
    let t2 = texts(r2);
    assert_eq!(t2.len(), 4, "{t2:?}");
    assert_eq!(&t2[1..3], [BLOCK_A, BLOCK_B]);
    assert!(
        t2[3].starts_with("<time now=\""),
        "the opener carries the date: {}",
        t2[3]
    );
    assert!(t2[3].ends_with("Second question."));
    assert!(!t2.iter().any(|t| t.contains("First question.")));
    assert!(!t2.iter().any(|t| t.contains("<context_summary")));
    let roles: Vec<&str> = r2["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["role"].as_str().unwrap())
        .collect();
    assert_eq!(roles, ["system", "user", "user", "user"]);

    // 8.3: nothing carried, everything persisted — both turns' rows are in the store.
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
    let folded = turn.install_view_head(1, text).await.unwrap();
    assert_eq!(folded, 2, "the input and the reply");
    assert_eq!(turn.state.epoch, 1);
    assert!(turn.state.history.is_empty(), "nothing of the tail kept");
    let (epoch, head, tail) = h.store.frame(h.id).await.unwrap();
    assert_eq!(epoch, 1);
    assert_eq!(text_of(&head.unwrap().content), text);
    assert!(tail.is_empty());
    assert_eq!(
        compact::style_of_head(text),
        CompactStyle::Summary,
        "an unknown style reads as summary"
    );
    // A session with no rows gets no head.
    let (mut empty, _) = h.turn("openai/textonly", CancellationToken::new()).await;
    empty.state.history.clear();
    assert_eq!(empty.install_view_head(2, text).await.unwrap(), 0);
}
