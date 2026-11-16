//! 81 MEETING-DIET, through a real [`Turn`] against the fake provider of
//! [`super::vision_fallback_tests`]: a session's `Toolset.hint_limit` caps or
//! drops the `<knowledge>` note at turn start, and a pinned title is never
//! regenerated. One tokio test: the tool registry and the knowledge actor are
//! process-global actors bound to the runtime that spawned them.

use super::vision_fallback_tests::{Fake, Harness};
use super::*;

/// The plain fake: nothing held (no harvest fires here: `calls` stays 0).
fn fake() -> Fake {
    Fake::harvest_held(Duration::ZERO)
}

/// The `<knowledge>` note's hint lines in `history`, if one was appended.
fn hint_lines(history: &[(i64, ChatMessage)]) -> Option<usize> {
    history.iter().find_map(|(_, m)| {
        let text = text_of(&m.content);
        let body = text.strip_prefix("<knowledge>\n")?;
        Some(body.lines().filter(|l| l.starts_with("- `")).count())
    })
}

#[test]
fn hint_limit_and_title_pin() {
    crate::with_test_registry(diet_scenarios());
}

async fn diet_scenarios() {
    let scratch = crate::TestDir::new("diet-reg");
    let reg_dir = scratch.0.clone();
    let reg_rt = Runtime::new(figment::Figment::new(), reg_dir.clone());
    tools::registry::start(&reg_rt).await.unwrap();
    super::vision_fallback_tests::start_main_knowledge(&reg_rt).await;
    let knowledge = ActorRef::<KnowledgeActor>::lookup(KNOWLEDGE_ACTOR)
        .unwrap()
        .unwrap()
        .ask(knowledge::Handle)
        .await
        .unwrap();
    // Three shared concepts every scenario's input matches on two terms.
    for n in 1..=3 {
        knowledge
            .put(
                knowledge::Put {
                    id: format!("facts/failover-{n}"),
                    type_: "fact".into(),
                    title: format!("Failover runbook {n}"),
                    description: Some("Datacenter failover drill notes".into()),
                    body: format!("Failover step {n} for the datacenter drill."),
                    ..Default::default()
                },
                knowledge::Source::User,
            )
            .await
            .unwrap();
    }
    let input = "Walk me through the datacenter failover drill.";
    let configured = AgentConfig::default().knowledge.hint_limit;
    assert!(configured >= 3, "the default admits all three");

    let run = |toolset: Option<Toolset>, dreaming: bool| {
        let knowledge = knowledge.clone();
        async move {
            let mut h = Harness::new(fake()).await;
            let (mut turn, tid) = h.turn("openai/textonly", CancellationToken::new()).await;
            // `Session::start_turn` sets the round-0 hint query; the harness bypasses it.
            turn.input_text = input.into();
            *turn.shared.toolset.write() = toolset;
            if dreaming {
                knowledge.register_consolidating_session(turn.shared.id);
            }
            let state = turn.run(tid, vec![h.input(input, false)]).await;
            knowledge.clear_consolidating_session();
            h.drain().await;
            hint_lines(&state.history)
        }
    };
    let live = |hint_limit| {
        Some(Toolset {
            tools: vec![],
            prompts: vec![],
            runtime: false,
            hint_limit,
        })
    };
    // 136 §5.3: every `<knowledge>` note records one impression per id shown,
    // exactly the ids shown, none when no note is appended.
    let ids: Vec<String> = (1..=3).map(|n| format!("facts/failover-{n}")).collect();
    let shown = |knowledge: Arc<knowledge::Knowledge>, ids: Vec<String>| async move {
        knowledge
            .stats(&ids)
            .await
            .unwrap()
            .into_iter()
            .map(|u| u.shown)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        run(None, false).await,
        Some(3),
        "no override: the configured limit"
    );
    assert_eq!(shown(knowledge.clone(), ids.clone()).await, [1, 1, 1]);
    assert_eq!(
        run(live(None), false).await,
        Some(3),
        "`None` defers to config"
    );
    assert_eq!(
        run(live(Some(2)), false).await,
        Some(2),
        "`Some(2)`: two lines"
    );
    let after_two = shown(knowledge.clone(), ids.clone()).await;
    assert_eq!(after_two.iter().sum::<u64>(), 3 + 3 + 2, "{after_two:?}");
    assert_eq!(
        run(live(Some(0)), false).await,
        None,
        "`Some(0)`: no note at all"
    );
    assert_eq!(shown(knowledge.clone(), ids.clone()).await, after_two);
    // 136 (P gap 1): the registered dreaming session gets no note and spends
    // no impressions, whatever its toolset says.
    assert_eq!(run(None, true).await, None, "dreaming session: no note");
    assert_eq!(shown(knowledge.clone(), ids.clone()).await, after_two);

    // A fresh session's first turn with user text is title-due (07 §15);
    // pinned, the titler is skipped and the counters it would reset stay put.
    let titled = |pinned: bool| async move {
        let mut h = Harness::new(fake()).await;
        let (mut turn, tid) = h.turn("openai/textonly", CancellationToken::new()).await;
        // `Session::start_turn` counts the user's chars; the harness bypasses it.
        turn.state.title_chars = input.len();
        turn.shared
            .title_pinned
            .store(pinned, std::sync::atomic::Ordering::Relaxed);
        let mut state = turn.run(tid, vec![h.input(input, false)]).await;
        h.drain().await;
        let ran = match state.title_task.take() {
            Some(task) => {
                let _ = task.await;
                true
            }
            None => false,
        };
        (state.title_model, state.title_turns, ran)
    };
    assert_eq!(
        titled(false).await,
        (true, 0, true),
        "unpinned: the title call ran"
    );
    assert_eq!(titled(true).await, (false, 1, false), "pinned: skipped");
}
