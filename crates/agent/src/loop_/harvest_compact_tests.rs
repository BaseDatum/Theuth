//! 87 HARVEST-DETACH, through a real [`Turn`] against the fake provider of
//! [`super::vision_fallback_tests`]: the harvest before a compaction runs
//! detached on a copy of the pre-fold frame — the compaction returns and
//! `Compacted` fires while the provider still holds the harvest reply; a
//! harvest already in flight is chained, never skipped; a cancelled
//! compaction rolls back while its harvest still lands. One tokio test: the
//! tool registry and the knowledge actor are process-global actors bound to
//! the runtime that spawned them.

use std::time::{Duration, Instant};

use super::vision_fallback_tests::{Fake, Harness, texts};
use super::*;

/// How long the fake holds its replies: long enough that a compaction and
/// its assertions run first, short enough for the 10 s event timeout.
const HOLD: Duration = Duration::from_secs(3);

/// Text that lives only in the span the compaction folds.
const MARKER: &str = "The build host is buildbox-HARVEST-MARKER.";

fn count(events: &[AgentEvent], f: impl Fn(&AgentEvent) -> bool) -> usize {
    events.iter().filter(|e| f(e)).count()
}

fn harvesting(events: &[AgentEvent]) -> usize {
    count(events, |e| matches!(e, AgentEvent::Harvesting { .. }))
}

fn harvested(events: &[AgentEvent]) -> Option<(u32, u32)> {
    events.iter().find_map(|e| match e {
        AgentEvent::Harvested { added, updated, .. } => Some((*added, *updated)),
        _ => None,
    })
}

/// Index of the first `Harvesting` and the first `Compacted` event.
fn order(events: &[AgentEvent]) -> (Option<usize>, Option<usize>) {
    let at = |f: fn(&AgentEvent) -> bool| events.iter().position(f);
    (
        at(|e| matches!(e, AgentEvent::Harvesting { .. })),
        at(|e| matches!(e, AgentEvent::Compacted { .. })),
    )
}

/// The texts of every harvest request the fake has received so far.
async fn harvest_requests(h: &Harness) -> Vec<String> {
    let first = knowledge::harvest::HARVEST_INSTRUCTION
        .lines()
        .next()
        .unwrap_or_default();
    h.requests()
        .await
        .iter()
        .map(|b| texts(b).join("\n"))
        .filter(|t| t.contains(first))
        .collect()
}

/// Wait (bounded) until the fake has received `n` harvest requests.
async fn harvest_requests_at_least(h: &Harness, n: usize) -> Vec<String> {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let reqs = harvest_requests(h).await;
        if reqs.len() >= n || Instant::now() > deadline {
            return reqs;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[test]
fn compaction_does_not_wait_for_the_harvest() {
    crate::with_test_registry(scenarios());
}

async fn scenarios() {
    let reg_dir =
        std::env::temp_dir().join(format!("theuth-harvest-compact-reg-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&reg_dir).unwrap();
    let reg_rt = Runtime::new(figment::Figment::new(), reg_dir.clone());
    tools::registry::start(&reg_rt).await.unwrap();
    runtime::Plugin::start(Box::new(knowledge::KnowledgePlugin), &reg_rt)
        .await
        .unwrap();
    assert!(
        AgentConfig::default().knowledge.harvest_every > 0,
        "harvest on by default"
    );

    compaction_returns_before_the_harvest().await;
    a_running_harvest_is_chained_not_skipped().await;
    a_cancelled_compaction_rolls_back_and_the_harvest_lands().await;

    let _ = std::fs::remove_dir_all(&reg_dir);
}

/// A. `/compact cut` (no summariser call): `Ok(1)` and `Compacted` while the
/// fake still holds the harvest reply; the one model request is the harvest
/// over the pre-fold frame.
async fn compaction_returns_before_the_harvest() {
    let mut h = Harness::new(Fake::harvest_held(HOLD)).await;
    for text in [MARKER, "And the deploy host is deploybox."] {
        let (turn, tid) = h.turn("openai/textonly", CancellationToken::new()).await;
        turn.run(tid, vec![h.input(text, false)]).await;
        h.drain().await;
    }
    let main_calls = h.requests().await.len();
    assert_eq!(main_calls, 2, "no harvest from tool-less turns");

    let (mut turn, _) = h.turn("openai/textonly", CancellationToken::new()).await;
    turn.state.turns_since_harvest = 1;
    let started = Instant::now();
    let epoch = turn.compact_now(Some(CompactStyle::Cut), None).await;
    assert_eq!(epoch, Ok(1));
    let events = h
        .drain_until(|e| matches!(e, AgentEvent::Compacted { .. }))
        .await;
    assert!(
        started.elapsed() < HOLD,
        "compacted in {:?} while the reply is held for {HOLD:?}",
        started.elapsed()
    );
    assert_eq!(harvesting(&events), 1);
    let (harvesting_at, compacted_at) = order(&events);
    assert!(
        harvesting_at < compacted_at,
        "`Harvesting` before `Compacted`: {events:#?}"
    );
    assert_eq!(harvested(&events), None, "not before the reply");
    assert!(
        turn.state
            .harvest_task
            .as_ref()
            .is_some_and(|t| !t.is_finished()),
        "the harvest still runs when the compaction returns"
    );
    assert_eq!(turn.state.epoch, 1);
    assert!(turn.state.head.is_some());

    let events = h
        .drain_until(|e| matches!(e, AgentEvent::Harvested { .. }))
        .await;
    assert_eq!(harvested(&events), Some((1, 0)));
    let reqs = h.requests().await;
    assert_eq!(
        reqs.len(),
        main_calls + 1,
        "the harvest is the only new call: {reqs:#?}"
    );
    let harvest = harvest_requests(&h).await;
    assert_eq!(harvest.len(), 1);
    assert!(
        harvest[0].contains(MARKER),
        "the pre-fold frame: {}",
        harvest[0]
    );
    assert!(
        !harvest[0].contains("Recent turns follow verbatim"),
        "not the compaction head: {}",
        harvest[0]
    );
    let knowledge = turn.knowledge().expect("knowledge is on");
    let concept = knowledge
        .get("facts/detached-harvest")
        .await
        .expect("the put landed");
    let (meta, body) = (&concept.meta, &concept.text);
    // 136 §0.14: the harvest's puts carry evidence + fresh, a ≤ 120-char
    // title, the rest of the statement as a Summary paragraph, `type: record`.
    assert_eq!(meta.type_, "record");
    assert_eq!(meta.title, "Detached harvest landed on 2026-10-08");
    assert_eq!(meta.status, knowledge::Status::Fresh);
    assert_eq!(meta.evidence, [turn.shared.id.to_string()]);
    assert!(
        body.contains("\nSummary: The harvest instruction was answered by the fake provider"),
        "{body}"
    );
    assert!(
        body.contains("The fake provider answered the harvest instruction."),
        "{body}"
    );
}

/// B. A post-turn harvest is in flight when `/compact` runs: the compaction
/// still returns at once and its harvest waits for the running one instead
/// of being skipped — two requests, the second after the first reply.
async fn a_running_harvest_is_chained_not_skipped() {
    let mut h = Harness::new(Fake::harvest_held(HOLD)).await;
    let every = AgentConfig::default().knowledge.harvest_every;
    let (mut turn, tid) = h.turn("openai/textonly", CancellationToken::new()).await;
    let shared = turn.shared.clone();
    turn.calls = 1;
    turn.state.turns_since_harvest = every - 1;
    let state = turn.run(tid, vec![h.input(MARKER, false)]).await;
    let events = h.drain().await;
    assert_eq!(harvesting(&events), 1, "trigger (a) fired");
    assert!(
        state
            .harvest_task
            .as_ref()
            .is_some_and(|t| !t.is_finished())
    );
    assert_eq!(harvest_requests_at_least(&h, 1).await.len(), 1);

    let mut compacting = Turn::new(shared, None, CancellationToken::new(), state);
    compacting.state.turns_since_harvest = 1;
    let started = Instant::now();
    assert_eq!(
        compacting.compact_now(Some(CompactStyle::Cut), None).await,
        Ok(1)
    );
    let events = h
        .drain_until(|e| matches!(e, AgentEvent::Compacted { .. }))
        .await;
    assert!(started.elapsed() < HOLD, "{:?}", started.elapsed());
    assert_eq!(harvesting(&events), 1, "a second `Harvesting`, not a skip");
    assert_eq!(harvested(&events), None);
    assert_eq!(
        harvest_requests(&h).await.len(),
        1,
        "the chained harvest waits for the running one"
    );

    let events = h
        .drain_until(|e| matches!(e, AgentEvent::Harvested { .. }))
        .await;
    assert_eq!(harvested(&events), Some((0, 1)), "the post-turn harvest");
    let events = h
        .drain_until(|e| matches!(e, AgentEvent::Harvested { .. }))
        .await;
    assert_eq!(
        harvested(&events),
        Some((0, 1)),
        "the pre-compaction harvest"
    );
    let harvest = harvest_requests(&h).await;
    assert_eq!(harvest.len(), 2, "two requests, none skipped");
    assert!(
        harvest[1].contains(MARKER),
        "the pre-fold frame: {}",
        harvest[1]
    );
    assert!(
        compacting
            .state
            .harvest_task
            .as_ref()
            .is_some_and(tokio::task::JoinHandle::is_finished)
    );
}

/// C. The compaction is cancelled while its harvest runs: the summariser
/// call is cancelled and nothing is persisted (the existing rollback), the
/// harvest — fenced by `closed` only — still lands.
async fn a_cancelled_compaction_rolls_back_and_the_harvest_lands() {
    let mut h = Harness::new(Fake::harvest_and_compact_held(HOLD)).await;
    for text in [MARKER, "And the deploy host is deploybox."] {
        let (turn, tid) = h.turn("openai/textonly", CancellationToken::new()).await;
        turn.run(tid, vec![h.input(text, false)]).await;
        h.drain().await;
    }

    let cancel = CancellationToken::new();
    let (mut turn, _) = h.turn("openai/textonly", cancel.clone()).await;
    turn.state.turns_since_harvest = 1;
    let (result, events) =
        tokio::join!(turn.compact_now(Some(CompactStyle::Summary), None), async {
            let events = h
                .drain_until(|e| matches!(e, AgentEvent::Harvesting { .. }))
                .await;
            cancel.cancel();
            events
        });
    let err = result.unwrap_err();
    assert!(err.contains("cancelled"), "{err}");
    assert_eq!(harvesting(&events), 1);
    assert_eq!(
        count(&events, |e| matches!(e, AgentEvent::Compacted { .. })),
        0
    );
    assert_eq!(turn.state.epoch, 0, "nothing persisted in memory");
    assert!(turn.state.head.is_none());
    let (epoch, head, _) = h.store.frame(h.id).await.unwrap();
    assert_eq!(epoch, 0, "nothing persisted in the store");
    assert!(head.is_none());

    let events = h
        .drain_until(|e| matches!(e, AgentEvent::Harvested { .. }))
        .await;
    assert_eq!(
        count(&events, |e| matches!(e, AgentEvent::Compacted { .. })),
        0
    );
    assert_eq!(harvested(&events), Some((0, 1)), "the harvest still landed");
    let harvest = harvest_requests(&h).await;
    assert_eq!(harvest.len(), 1);
    assert!(harvest[0].contains(MARKER), "{}", harvest[0]);
}
