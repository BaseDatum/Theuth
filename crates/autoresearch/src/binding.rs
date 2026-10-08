//! W3 LOOP — binding a session to a study (design doc 140 §8.2, §8.3, §12.3
//! Subscriptions). The study `Toolset` reaches the session through the
//! schedule crate's confinement registry ([`confinement`], applied by the
//! schedule itself before every prompt — the first turn is never unconfined);
//! `ScheduleEvent::Fired { name: "autoresearch-<slug>", session }` records
//! `studies.session`; `AgentEvent::TurnStarted` on a bound session records the
//! user's typed text as a steer (§8.6); `AgentEvent::TurnFinished` closes a
//! dangling experiment as `abandoned` / `interrupted` / `timeout` and
//! evaluates the stop rules off the mailbox; `ScheduleEvent::Changed`
//! reconciles the file with the ledger (file gone → finalize, `enabled:` ↔
//! paused).

use std::sync::Arc;
use std::time::Duration;

use kameo::actor::ActorRef;
use kameo::message::{Context, Message};

use crate::Studies;
use crate::actor::{self, Env};
use crate::error::{Error, Result};
use crate::store::Store;
use crate::types::{By, Control, ControlOp, ExperimentState, StopReason, StudyState};

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

/// How long a file ↔ ledger mismatch must persist before [`reconcile`] acts:
/// the plugin's own `pause()` flips the file first and the ledger a moment later.
const RECONCILE_SETTLE: Duration = Duration::from_secs(2);

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

/// The schedule crate's confinement for `autoresearch-*` files (140 §8.3):
/// a running or paused study fires under [`toolset`]; any other name under the
/// prefix (unknown slug, finished study whose file survived) is orphaned and
/// the schedule ends its loop. Registered once at plugin start with
/// `schedule::confine(SCHEDULE_PREFIX, confinement(store))`.
pub fn confinement(store: Store) -> schedule::ToolsetResolver {
    Arc::new(move |name: &str| {
        let store = store.clone();
        let slug = slug_of(name).map(str::to_owned);
        Box::pin(async move {
            let Some(slug) = slug else {
                return Ok(None);
            };
            let study = store.get_study(&slug).await.map_err(|e| e.to_string())?;
            Ok(study
                .filter(|s| matches!(s.state, StudyState::Running | StudyState::Paused { .. }))
                .map(|s| toolset(s.spec.parallel)))
        })
    })
}

/// `ScheduleEvent::Fired` for one of ours → record the session (140 §8.2).
/// The toolset is already applied by the schedule (see [`confinement`]).
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
    Ok(())
}

/// `AgentEvent::TurnStarted` on a bound session (140 §8.6): the user typed in
/// the study session — their text becomes a steer so the next fresh frame
/// still sees it. A schedule envelope (`<schedule …>`) or any other
/// harness-built input (`<…`) is not the user's.
pub async fn on_turn_started(
    a: &mut Studies,
    session: runtime::SessionId,
    text: &str,
) -> Result<()> {
    let text = text.trim();
    if text.is_empty() || text.starts_with('<') {
        return Ok(());
    }
    let Some(study) = a.store.study_of_session(session).await? else {
        return Ok(());
    };
    a.store.insert_steer(&study.slug, text, By::User).await?;
    Ok(())
}

/// `AgentEvent::TurnFinished` on a bound session → close dangling, stop rules (140 §8.4, §8.5).
/// `stop_reason == "cancelled"` with an experiment still open is the
/// schedule's `run_timeout` cancel (the plugin's own stop-now closes the
/// experiment before it cancels): the experiment is `timeout`.
pub async fn on_turn_finished(
    a: &mut Studies,
    session: runtime::SessionId,
    stop_reason: &str,
) -> Result<()> {
    let Some(study) = a.store.study_of_session(session).await? else {
        return Ok(());
    };
    let env = a.env();
    let Some(mut exp) = a.store.dangling_experiment(&study.slug, None).await? else {
        // Nothing in flight: a `stop_after` with no experiment to wait for finalizes now.
        if actor::stop_after_pending(&env, &study).await? && !actor::is_terminal(&study.state) {
            actor::begin_finalize(&env, study, StopReason::User).await?;
        }
        return Ok(());
    };
    let measuring = exp.state == ExperimentState::Measuring;
    if measuring {
        // The tool call was dropped with the turn: kill the bench group.
        actor::cancel(&a.root, &study.slug);
    }
    let (state, what) = match (stop_reason == "cancelled", measuring) {
        (true, _) => (ExperimentState::Timeout, "timeout"),
        (false, true) => (ExperimentState::Interrupted, "interrupted"),
        (false, false) => (ExperimentState::Abandoned, "abandoned"),
    };
    exp.state = state;
    exp.finished_at = Some(crate::store::now());
    if exp.line.is_empty() {
        exp.line = match state {
            ExperimentState::Timeout => "timeout: the run was cancelled at run_timeout".into(),
            ExperimentState::Interrupted => "interrupted: the turn ended while measuring".into(),
            _ => "abandoned: the turn ended without experiment_log".into(),
        };
    }
    a.store.update_experiment(&exp).await?;
    if measuring {
        actor::mark_lost(&a.store, &study.slug, exp.n).await?;
    }
    a.store
        .append_event(
            &study.slug,
            "incident",
            &serde_json::json!({ "n": exp.n, "what": what }),
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
                reason: what.into(),
            }),
        metric: exp.metric,
        delta_pct: exp.delta_pct,
        z: exp.z,
    })
    .await;
    // The stop rules may consult the advisor (a model call, minutes): off the mailbox.
    let slug = study.slug;
    tokio::spawn(async move {
        if let Err(e) = actor::after_experiment(&env, &slug).await {
            tracing::warn!(%slug, "stop rules not applied: {e}");
        }
    });
    Ok(())
}

/// `ScheduleEvent::Changed` → reconcile every live study with its file
/// (140 §8.6 "the routines toggle is the same `enabled:` line"; §8.1 the
/// schedule's `max_runs` backstop): after [`RECONCILE_SETTLE`], a running
/// study whose file is gone finalizes (`Experiments` when the schedule
/// reached `max_runs`, `User` otherwise); a running study whose file says
/// `enabled: false` pauses; a paused study whose file says `enabled: true`
/// resumes — through `Control`, so the ledger, the events and the file agree.
pub fn reconcile(env: Env, me: ActorRef<Studies>) {
    tokio::spawn(async move {
        tokio::time::sleep(RECONCILE_SETTLE).await;
        if let Err(e) = reconcile_now(&env, &me).await {
            tracing::warn!("study files not reconciled: {e}");
        }
    });
}

async fn reconcile_now(env: &Env, me: &ActorRef<Studies>) -> Result<()> {
    let sched = ActorRef::<schedule::Schedule>::lookup(schedule::SCHEDULE_ACTOR)
        .ok()
        .flatten()
        .ok_or_else(|| Error::refused("schedule actor not running"))?;
    let files = sched
        .ask(schedule::ListSchedules)
        .await
        .map_err(|e| Error::refused(e.to_string()))?;
    for study in env.store.list_studies().await? {
        let paused = matches!(study.state, StudyState::Paused { .. });
        if !(paused || study.state == StudyState::Running) {
            continue;
        }
        let name = crate::schedule_file::name(&study.slug);
        let Some(file) = files.iter().find(|f| f.name == name) else {
            let last = sched
                .ask(schedule::History {
                    name: name.clone(),
                    limit: 1,
                })
                .await
                .ok()
                .and_then(|h| h.into_iter().next())
                .and_then(|r| r.result);
            let reason = if last.as_deref().is_some_and(|r| r.starts_with("max_runs")) {
                StopReason::Experiments
            } else {
                StopReason::User
            };
            tracing::info!(slug = %study.slug, "schedule file gone ({last:?}); finalizing");
            actor::begin_finalize(env, study, reason).await?;
            continue;
        };
        let op = match (file.enabled, paused) {
            (false, false) => ControlOp::Pause,
            (true, true) => ControlOp::Resume,
            _ => continue,
        };
        me.ask(Control {
            slug: study.slug.clone(),
            op,
            by: By::User,
        })
        .await
        .map_err(|e| Error::refused(e.to_string()))?;
    }
    Ok(())
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

    async fn handle(&mut self, e: schedule::ScheduleEvent, ctx: &mut Context<Self, Self::Reply>) {
        match e {
            schedule::ScheduleEvent::Fired { name, session, .. } if slug_of(&name).is_some() => {
                if let Err(err) = on_fired(self, &name, session).await {
                    tracing::warn!(%name, "study not bound: {err}");
                }
            }
            schedule::ScheduleEvent::Changed => reconcile(self.env(), ctx.actor_ref().clone()),
            _ => {}
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
        match &e {
            agent::AgentEvent::TurnStarted { session, text, .. } => {
                if let Err(err) = on_turn_started(self, *session, text).await {
                    tracing::warn!(%session, "steer not recorded: {err}");
                }
            }
            agent::AgentEvent::TurnFinished {
                session,
                stop_reason,
                ..
            } => {
                if let Err(err) = on_turn_finished(self, *session, stop_reason).await {
                    tracing::warn!(%session, "study turn not closed: {err}");
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{ExperimentRecord, StudyRecord};

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

    async fn study(store: &Store, slug: &str, state: StudyState, parallel: u8) -> StudyRecord {
        let mut spec = crate::types::StudySpec::default();
        spec.slug = slug.into();
        spec.parallel = parallel;
        let mut s = StudyRecord::new(spec, None);
        s.state = state;
        s.session = Some(runtime::SessionId::new());
        store.create_study(&s).await.unwrap();
        s
    }

    async fn studies(dir: &std::path::Path) -> Studies {
        let root = dir.join("autoresearch");
        Studies {
            data_dir: dir.to_path_buf(),
            root: root.clone(),
            cfg: Default::default(),
            agent: Default::default(),
            rt: runtime::Runtime::new(figment::Figment::new(), dir.to_path_buf()),
            store: Store::open(&root).await.unwrap(),
            backend: Arc::new(actor::Real),
        }
    }

    /// R-47 (§8.6): typing in the study session is a steer; harness envelopes
    /// and unbound sessions are not.
    #[tokio::test]
    async fn typed_text_in_a_bound_session_is_a_steer() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = studies(dir.path()).await;
        let s = study(&a.store, "s", StudyState::Running, 1).await;
        let sid = s.session.unwrap();
        on_turn_started(
            &mut a,
            sid,
            "<schedule name=\"autoresearch-s\">\nrun\n</schedule>",
        )
        .await
        .unwrap();
        on_turn_started(&mut a, sid, "   ").await.unwrap();
        on_turn_started(&mut a, runtime::SessionId::new(), "try SIMD")
            .await
            .unwrap();
        assert!(a.store.list_steers("s", false).await.unwrap().is_empty());
        on_turn_started(&mut a, sid, "try SIMD").await.unwrap();
        let steers = a.store.list_steers("s", true).await.unwrap();
        assert_eq!(steers.len(), 1);
        assert_eq!(steers[0].text, "try SIMD");
        assert_eq!(steers[0].by, By::User);
    }

    /// R-55 (§8.5): a cancelled turn with an experiment still open is the
    /// schedule's `run_timeout` → `timeout`; an ordinary end is `abandoned`.
    #[tokio::test]
    async fn cancelled_turn_records_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = studies(dir.path()).await;
        let s = study(&a.store, "s", StudyState::Running, 1).await;
        let sid = s.session.unwrap();
        a.store
            .insert_experiment(&ExperimentRecord::open("s", 1, 0, "base"))
            .await
            .unwrap();
        on_turn_finished(&mut a, sid, "cancelled").await.unwrap();
        let e = a.store.get_experiment("s", 1).await.unwrap().unwrap();
        assert_eq!(e.state, ExperimentState::Timeout);
        assert!(e.line.starts_with("timeout"), "{}", e.line);
        a.store
            .insert_experiment(&ExperimentRecord::open("s", 2, 0, "base"))
            .await
            .unwrap();
        on_turn_finished(&mut a, sid, "end_turn").await.unwrap();
        let e = a.store.get_experiment("s", 2).await.unwrap().unwrap();
        assert_eq!(e.state, ExperimentState::Abandoned);
    }

    /// R-5: the schedule applies the toolset itself, before the prompt, from
    /// this resolver — a live study gets §8.3 (waves add `spawn_subagent`),
    /// anything else under the prefix is orphaned so the schedule ends the file.
    #[tokio::test]
    async fn confinement_resolves_live_studies_only() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).await.unwrap();
        study(&store, "seq", StudyState::Running, 1).await;
        study(
            &store,
            "wave",
            StudyState::Paused {
                reason: crate::types::PauseReason::User,
            },
            3,
        )
        .await;
        study(&store, "done", StudyState::Finished, 1).await;
        let r = confinement(store);
        let seq = r("autoresearch-seq").await.unwrap().unwrap();
        assert!(seq.tools.iter().any(|t| t == "experiment_begin"));
        assert!(!seq.tools.iter().any(|t| t == "spawn_subagent"));
        let wave = r("autoresearch-wave").await.unwrap().unwrap();
        assert!(wave.tools.iter().any(|t| t == "spawn_subagent"));
        assert!(r("autoresearch-done").await.unwrap().is_none());
        assert!(r("autoresearch-nope").await.unwrap().is_none());
        assert!(r("autoresearch-").await.unwrap().is_none());
    }
}
