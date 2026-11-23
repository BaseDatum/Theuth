//! fence-skel D3: a surface correction round publishes `InputInjected` between the broken reply and the corrected
//! one, so a frontend that appends `TextDelta`s to its open reply starts the correction as a new entry; the
//! max-tokens salvage round publishes nothing there, so its continuation still appends to the cut-off reply.
//! Driven through a real [`Turn`] against a fake provider. One tokio test: the tool registry is a process-global
//! actor bound to the runtime that spawned it.

use wiremock::{Request, Respond, ResponseTemplate};

use super::vision_fallback_tests::{Harness, MAIN_USAGE, body, sse, texts};
use super::*;

const BROKEN: &str = "Checks:\n```json-render\n{\"root\": \"r\", \"elements\": {\"r\": {\"type\": \"DataTable\", \"props\": {}}}}\n```";
const FIXED: &str = "```json-render\n{\"root\": \"r\", \"elements\": {\"r\": {\"type\": \"Separator\", \"props\": {}}}}\n```";
const CUT: &str = "The checks ran on";
const REST: &str = " the branch.";

/// Broken fence until the `<surface_error>` note is in the frame, then the corrected block; a `CUT…` opener is
/// length-capped until the `<continue>` note is in the frame, then `REST`.
struct Provider;

impl Respond for Provider {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let sent = texts(&body(req));
        let has = |s: &str| sent.iter().any(|t| t.contains(s));
        match (has("salvage me"), has("<continue>"), has("<surface_error>")) {
            (true, false, _) => sse(CUT, "length", MAIN_USAGE),
            (true, true, _) => sse(REST, "stop", MAIN_USAGE),
            (false, _, false) => sse(BROKEN, "stop", MAIN_USAGE),
            (false, _, true) => sse(FIXED, "stop", MAIN_USAGE),
        }
    }
}

/// The turn's text runs and what separates them: `"T:<text>"` per `TextDelta`, `"I"` per `InputInjected`.
fn shape(events: &[AgentEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::TextDelta { text, .. } => Some(format!("T:{text}")),
            AgentEvent::InputInjected { .. } => Some("I".to_owned()),
            _ => None,
        })
        .collect()
}

#[test]
fn correction_round_separates_the_replies_and_salvage_does_not() {
    crate::with_test_registry(scenario());
}

async fn scenario() {
    let scratch = crate::test_dir("fence-round-reg");
    let reg_rt = Runtime::new(figment::Figment::new(), scratch.path().to_path_buf());
    tools::registry::start(&reg_rt).await.unwrap();

    let mut h = Harness::new(Provider).await;
    let (turn, tid) = h.turn("openai/textonly", CancellationToken::new()).await;
    turn.run(tid, vec![h.input("Run the checks", false)]).await;
    let events = h.drain().await;
    assert_eq!(
        shape(&events),
        [format!("T:{BROKEN}"), "I".to_owned(), format!("T:{FIXED}")],
        "the correction note sits between the two replies: {events:?}"
    );

    let (turn, tid) = h.turn("openai/textonly", CancellationToken::new()).await;
    turn.run(tid, vec![h.input("salvage me", false)]).await;
    let events = h.drain().await;
    assert_eq!(
        shape(&events),
        [format!("T:{CUT}"), format!("T:{REST}")],
        "the salvage continuation follows the cut-off reply with nothing between: {events:?}"
    );
}
