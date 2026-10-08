//! W3 LOOP — binding a session to a study (design doc 140 §8.2, §8.3, §12.3
//! Subscriptions): on `ScheduleEvent::Fired { name: "autoresearch-<slug>", session }`
//! record `studies.session` and send `agent::SetToolset` with the study
//! toolset; on `AgentEvent::TurnFinished` for a bound session close a
//! dangling experiment as `abandoned` (`interrupted` when it was measuring:
//! the bench group is killed) and evaluate the stop rules; schedule
//! run-closed rows carry timeouts.

use kameo::actor::ActorRef;
use kameo::message::{Context, Message};

use crate::Studies;
use crate::actor;
use crate::error::{Error, Result};
use crate::types::ExperimentState;

/// The coding and knowledge tools of §8.3 (plus the four study tools and `todo`).
const TOOLS: [&str; 19] = [
    "experiment_begin",
    "experiment_run",
    "experiment_log",
    "experiment_notes",
    "read_file",
    "grep",
    "glob",
    "edit",
    "write_file",
    "list_dir",
    "shell",
    "code_find",
    "code_outline",
    "code_refs",
    "code_symbol",
    "code_map",
    "knowledge_search",
    "knowledge_get",
    "todo",
];

/// The study session's `Toolset` (140 §8.3): the four experiment tools, the
/// coding tools, knowledge read tools, `todo`; `spawn_subagent` only when `parallel > 1`.
pub fn toolset(parallel: u8) -> agent::Toolset {
    let mut tools: Vec<String> = TOOLS.iter().map(|s| (*s).to_owned()).collect();
    if parallel > 1 {
        tools.push("spawn_subagent".to_owned());
    }
    agent::Toolset {
        tools,
        // The autoresearch block plus the blocks of the plugins whose tools are listed.
        prompts: vec![
            crate::PLUGIN.to_owned(),
            "codeindex".to_owned(),
            "knowledge".to_owned(),
        ],
        runtime: false,
        hint_limit: Some(0),
    }
}

/// The agent supervisor, when running.
fn agents() -> Option<ActorRef<agent::Agents>> {
    ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR)
        .ok()
        .flatten()
}

/// `ScheduleEvent::Fired` for one of ours → bind (140 §8.2). The toolset is
/// in memory only, so it is re-sent on every fire.
pub async fn on_fired(a: &mut Studies, name: &str, session: runtime::SessionId) -> Result<()> {
    let Some(slug) = slug_of(name) else {
        return Ok(());
    };
    let Some(mut study) = a.store.get_study(slug).await? else {
        tracing::warn!(%name, "fired for an unknown study");
        return Ok(());
    };
    if study.session != Some(session) {
        study.session = Some(session);
        a.store.update_study(&study).await?;
    }
    match agents() {
        Some(agents) => {
            let r = agents
                .ask(agent::SetToolset {
                    session,
                    toolset: Some(toolset(study.spec.parallel)),
                })
                .await;
            if let Err(e) = r {
                tracing::warn!(%slug, "study toolset not applied: {e}");
            }
        }
        None => tracing::warn!(%slug, "agent actor not running; study session unconfined"),
    }
    Ok(())
}

/// `AgentEvent::TurnFinished` on a bound session → close dangling, stop rules (140 §8.4, §8.5).
pub async fn on_turn_finished(a: &mut Studies, session: runtime::SessionId) -> Result<()> {
    let Some(study) = a.store.study_of_session(session).await? else {
        return Ok(());
    };
    let env = a.env();
    let Some(mut exp) = a.store.dangling_experiment(&study.slug, None).await? else {
        // Nothing in flight: a `stop_after` with no experiment to wait for finalizes now.
        if actor::stop_after_pending(&env, &study).await? && !actor::is_terminal(&study.state) {
            actor::begin_finalize(&env, study, crate::types::StopReason::User).await?;
        }
        return Ok(());
    };
    let interrupted = exp.state == ExperimentState::Measuring;
    if interrupted {
        // The tool call was dropped with the turn: kill the bench group.
        actor::cancel(&a.root, &study.slug);
    }
    exp.state = if interrupted {
        ExperimentState::Interrupted
    } else {
        ExperimentState::Abandoned
    };
    exp.finished_at = Some(crate::store::now());
    a.store.update_experiment(&exp).await?;
    a.store
        .append_event(
            &study.slug,
            "incident",
            &serde_json::json!({ "n": exp.n, "what": if interrupted { "interrupted" } else { "abandoned" } }),
        )
        .await?;
    actor::clear_phase(&a.root, &study.slug);
    a.publish(crate::types::StudyEvent::ExperimentFinished {
        slug: study.slug.clone(),
        n: exp.n,
        verdict: exp
            .verdict
            .clone()
            .unwrap_or(crate::types::Verdict::Refused {
                reason: if interrupted {
                    "interrupted".into()
                } else {
                    "abandoned".into()
                },
            }),
        metric: exp.metric,
        delta_pct: exp.delta_pct,
        z: exp.z,
    })
    .await;
    actor::after_experiment(&env, &study.slug).await
}

/// The slug of a schedule name `autoresearch-<slug>`, if it is one of ours.
pub fn slug_of(name: &str) -> Option<&str> {
    name.strip_prefix(crate::SCHEDULE_PREFIX)
        .filter(|s| !s.is_empty())
}

/// The refusal every study tool gives a session that is not bound to a study (140 §8.2).
pub fn not_a_study_session() -> Error {
    Error::refused("not an autoresearch session")
}

impl Message<schedule::ScheduleEvent> for Studies {
    type Reply = ();

    async fn handle(&mut self, e: schedule::ScheduleEvent, _: &mut Context<Self, Self::Reply>) {
        if let schedule::ScheduleEvent::Fired { name, session, .. } = e
            && slug_of(&name).is_some()
            && let Err(err) = on_fired(self, &name, session).await
        {
            tracing::warn!(%name, "study not bound: {err}");
        }
    }
}

impl Message<agent::AgentEvent> for Studies {
    type Reply = ();

    async fn handle(&mut self, e: agent::AgentEvent, _: &mut Context<Self, Self::Reply>) {
        // W5: wave children (`SubagentSpawned` → toolset, a child's turn end) and the supervisor's settle.
        if let Err(err) = crate::wave::on_agent_event(self, &e).await {
            tracing::warn!("wave event not handled: {err}");
        }
        if let agent::AgentEvent::TurnFinished { session, .. } = e
            && let Err(err) = on_turn_finished(self, session).await
        {
            tracing::warn!(%session, "study turn not closed: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toolset_is_the_section_8_3_list() {
        let t = toolset(1);
        assert!(
            t.tools.iter().any(|x| x == "experiment_begin") && t.tools.iter().any(|x| x == "shell")
        );
        assert!(!t.tools.iter().any(|x| x == "spawn_subagent"));
        assert!(
            !t.tools
                .iter()
                .any(|x| x == "message_user" || x == "ask_user" || x == "schedule")
        );
        assert!(!t.runtime && t.hint_limit == Some(0));
        assert_eq!(t.prompts[0], "autoresearch");
        assert!(toolset(2).tools.iter().any(|x| x == "spawn_subagent"));
    }

    #[test]
    fn slug_of_prefix() {
        assert_eq!(slug_of("autoresearch-sort-speed"), Some("sort-speed"));
        assert_eq!(slug_of("autoresearch-"), None);
        assert_eq!(slug_of("dreaming"), None);
    }
}
