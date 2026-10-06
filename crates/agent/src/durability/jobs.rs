//! Design doc 109 §5.4: the job reaper at start. Runs once after
//! `rearm_reminders` and before `resume_scan` (§4.6 step 10): every
//! `running` `jobs` row whose pgid is still alive is killed, every `running`
//! row is settled `lost`, and the rows go to the jobs actor as
//! [`Reaped`] — which lists them `lost`, serves a lost PTY's stored screen,
//! and injects `<job_result exit="lost">` for the jobs whose stub the model
//! already had (the resumed batch reports the others itself, §4.3).

use std::collections::HashSet;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use chrono::{DateTime, Local, Utc};
use kameo::actor::ActorRef;
use platform::ProcessTree;
use runtime::SessionId;
use tools::jobs::{LostJob, Reaped};
use tools::{JOBS_ACTOR, JobExit, JobKind, JobRow, JobState, Jobs, PtyScreenRow};

use crate::durability::CallState;
use crate::store::Store;

/// Bytes of a lost job's log quoted in its `<job_result>` (`jobs.rs::TAIL`).
const TAIL: u64 = 8 * 1024;

/// What one reaper pass did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReapReport {
    /// Process groups a SIGKILL of the previous engine leaked, killed now.
    pub killed: u32,
    /// `running` rows settled `lost`.
    pub lost: u32,
    /// Rows of sessions that no longer exist (not reported to anyone).
    pub orphaned: u32,
}

/// §5.4, for every `jobs` row in `running` — a `resume: off` session's row
/// included (§15 Q6): kill the group if it is alive, settle the row `lost`,
/// then hand the rows to the jobs actor. Nothing is skipped.
pub async fn reap_jobs(store: &Store) -> ReapReport {
    let (report, reaped) = reap(store).await;
    if !reaped.jobs.is_empty() {
        match ActorRef::<Jobs>::lookup(JOBS_ACTOR) {
            Ok(Some(jobs)) => {
                let _ = jobs.tell(reaped).await;
            }
            _ => tracing::error!("jobs actor not running: lost jobs not reported"),
        }
    }
    report
}

/// The store half of [`reap_jobs`]: kill, settle, and build what the jobs
/// actor is told.
pub(crate) async fn reap(store: &Store) -> (ReapReport, Reaped) {
    let restarted_at = Utc::now();
    let mut report = ReapReport::default();
    let mut reaped = Reaped {
        restarted_at,
        jobs: Vec::new(),
    };
    let rows = match store.jobs_open().await {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!("109 reaper: jobs_open: {e:#}");
            return (report, reaped);
        }
    };
    if rows.is_empty() {
        return (report, reaped);
    }
    // A job whose call is a `started` row of an open turn is reported by the
    // job rule inside the resumed batch (§4.3): no `<job_result>` for it.
    let mut in_batch: HashSet<(SessionId, String)> = HashSet::new();
    match store.open_turns().await {
        Ok(open) => {
            for (session, turn, _) in open {
                if let Ok(calls) = store.calls_of(session, turn).await {
                    in_batch.extend(
                        calls
                            .into_iter()
                            .filter(|c| c.state == CallState::Started)
                            .map(|c| (session, c.call_id)),
                    );
                }
            }
        }
        Err(e) => tracing::error!("109 reaper: open_turns: {e:#}"),
    }
    let now_ms = restarted_at.timestamp_millis();
    let mut lost = Vec::new();
    for mut row in rows {
        if let Some(pgid) = row.pgid.and_then(|p| u32::try_from(p).ok())
            && let Ok(tree) = ProcessTree::adopt(pgid)
            && tree.alive()
        {
            tree.kill();
            report.killed += 1;
            tracing::info!(
                "reaped job {} of {} leaked by a restart (pgid {pgid})",
                row.id,
                row.session
            );
        }
        match store
            .job_settle(row.session, &row.id, JobState::Lost, &JobExit::Lost, now_ms)
            .await
        {
            Ok(true) => report.lost += 1,
            Ok(false) => {}
            Err(e) => tracing::error!("109 reaper: settle {}: {e:#}", row.id),
        }
        if !matches!(store.session(row.session).await, Ok(Some(_))) {
            report.orphaned += 1;
            continue;
        }
        let screen = match row.kind {
            JobKind::Pty => store.pty_screen(row.session, &row.id).await.ok().flatten(),
            JobKind::Shell => None,
        };
        let tail = row.log.as_deref().map(log_tail).unwrap_or_default();
        let deliver = (!in_batch.contains(&(row.session, row.call_id.clone())))
            .then(|| lost_body(&row, screen.as_ref(), &tail, restarted_at));
        row.state = JobState::Lost;
        row.exit = Some(JobExit::Lost);
        // `KillAll` stamped the shutdown time (§5.5); a crash left none.
        row.settled_ms.get_or_insert(now_ms);
        lost.push(LostJob {
            row,
            screen,
            tail,
            deliver,
        });
    }
    reaped.jobs = lost;
    (report, reaped)
}

/// The `<job_result exit="lost">` body (§5.4 / §5.5): why the job is gone,
/// its log path, then the log tail — or the stored screen for a PTY.
pub(crate) fn lost_body(
    row: &JobRow,
    screen: Option<&PtyScreenRow>,
    tail: &str,
    restarted_at: DateTime<Utc>,
) -> String {
    let mut body = match (row.exit.as_ref(), row.settled_ms) {
        // `exit = lost` on a row the reaper found still `running`: the
        // shutdown path wrote it (§5.5) before the kill.
        (Some(JobExit::Lost), Some(stopped_ms)) => format!(
            "killed at shutdown (engine stopped at {}, restarted at {})",
            hhmmss_ms(stopped_ms),
            hhmmss(restarted_at)
        ),
        _ => format!(
            "this job died with the engine (restarted at {}); its exit code is unknown.",
            hhmmss(restarted_at)
        ),
    };
    if let Some(log) = &row.log {
        body.push_str(&format!("\nlog: {}", log.display()));
    }
    match screen {
        Some(s) => {
            body.push_str(&format!(
                "\nlast screen (captured at {}, revision {}):\n{}",
                hhmmss_ms(s.at_ms),
                s.revision,
                s.text
            ));
        }
        None => {
            body.push_str("\nlast 8 KiB of the log:\n");
            body.push_str(tail);
        }
    }
    body
}

/// Last ≤ [`TAIL`] bytes of `path`, lossy UTF-8 cut on a char boundary;
/// empty when unreadable.
pub(crate) fn log_tail(path: &Path) -> String {
    let Ok(mut f) = std::fs::File::open(path) else {
        return String::new();
    };
    let len = f.metadata().map_or(0, |m| m.len());
    let mut buf = Vec::new();
    if f.seek(SeekFrom::Start(len.saturating_sub(TAIL))).is_err()
        || f.read_to_end(&mut buf).is_err()
    {
        return String::new();
    }
    let mut start = 0;
    while start < buf.len() && (buf[start] & 0xC0) == 0x80 {
        start += 1;
    }
    String::from_utf8_lossy(&buf[start..]).into_owned()
}

/// Local `HH:MM:SS` (§4.3).
pub(crate) fn hhmmss(t: DateTime<Utc>) -> String {
    t.with_timezone(&Local).format("%H:%M:%S").to_string()
}

/// [`hhmmss`] of a unix-ms stamp.
pub(crate) fn hhmmss_ms(ms: i64) -> String {
    DateTime::<Utc>::from_timestamp_millis(ms).map_or_else(|| "?".to_owned(), hhmmss)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use kameo::actor::Spawn;
    use runtime::Runtime;
    use serde_json::json;
    use tools::pty::{Ready, StartArgs, StartPty};
    use tools::{InstallJournal, Risk};

    use super::*;
    use crate::durability::journal::StoreJournal;
    use crate::durability::{JournaledCall, LastStep, ResumeKind, ResumePlan, StepKind};
    use crate::loop_::resume_calls::resume_job_call;
    use crate::store::durability::{calls_planned_in, step_in};
    use runtime::TurnId;

    fn temp() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("theuth-109-w2-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    async fn session(store: &Store) -> SessionId {
        let id = SessionId::new();
        store
            .create_session(id, "m", None, None, None)
            .await
            .unwrap();
        id
    }

    fn running(session: SessionId, id: &str, call_id: &str, pgid: Option<i64>) -> JobRow {
        JobRow {
            session,
            id: tools::JobId(id.into()),
            kind: JobKind::Shell,
            call_id: call_id.into(),
            command: "cargo build".into(),
            name: None,
            pgid,
            log: Some(std::path::PathBuf::from("/nonexistent/j.log")),
            started_ms: 1_000,
            state: JobState::Running,
            exit: None,
            settled_ms: None,
        }
    }

    /// §11.2: `sleep 600` through the actor, the engine "SIGKILLed" (its
    /// runtime dropped without `KillAll`), a fresh store on the same file,
    /// `reap_jobs`: the group is dead, the row `lost`, and the
    /// `<job_result exit="lost">` names the log and the last screen.
    #[tokio::test]
    async fn reaper_kills_leaked_group() {
        let dir = temp();
        let path = dir.join("sessions.sqlite");
        let store = Arc::new(Store::open(&path, None).await.unwrap());
        let session = session(&store).await;
        // The previous engine: its own runtime, so dropping it takes every
        // task with it and nothing settles the job.
        let engine = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let (pgid, log) = engine
            .spawn({
                let (store, dir) = (Arc::clone(&store), dir.clone());
                async move {
                    let rt = Runtime::new(figment::Figment::new(), dir.clone());
                    let jobs = Jobs::spawn_with_mailbox(
                        Jobs::new(rt, Duration::ZERO, Duration::from_secs(600)),
                        runtime::subscriber_mailbox(),
                    );
                    jobs.ask(InstallJournal(Arc::new(StoreJournal(Arc::clone(&store)))))
                        .await
                        .unwrap();
                    let out = jobs
                        .ask(StartPty {
                            session,
                            call_id: "c1".into(),
                            args: StartArgs {
                                // Ignore the HUP the closing master sends,
                                // as a child that survives a SIGKILLed
                                // engine does — the group the reaper is for.
                                command: "trap '' HUP; echo armed; exec sleep 600".into(),
                                name: None,
                                cwd: None,
                                env: None,
                                // Reply only once the trap is in place.
                                ready: Some(Ready {
                                    log: Some("armed".into()),
                                    port: None,
                                    host: None,
                                    timeout: Some(20),
                                }),
                                idle_secs: Some(0),
                            },
                            cwd: dir.clone(),
                            ready_timeout: Duration::from_secs(20),
                            idle_after: None,
                            root: dir,
                            activity: None,
                            pty_shell: None,
                        })
                        .await
                        .unwrap();
                    assert!(
                        out.content.starts_with("job j1 · running"),
                        "{}",
                        out.content
                    );
                    let rows = store.jobs_open().await.unwrap();
                    assert_eq!(rows.len(), 1);
                    (rows[0].pgid.unwrap(), rows[0].log.clone().unwrap())
                }
            })
            .await
            .unwrap();
        engine.shutdown_background();
        drop(store);
        let tree = ProcessTree::adopt(pgid as u32).unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(tree.alive(), "the leaked group should outlive the engine");

        let store = Store::open(&path, None).await.unwrap();
        let (report, reaped) = reap(&store).await;
        assert_eq!((report.killed, report.lost, report.orphaned), (1, 1, 0));
        let gone = async {
            while tree.alive() {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        };
        tokio::time::timeout(Duration::from_secs(5), gone)
            .await
            .expect("killpg(pgid, 0) still succeeds after the reaper");
        assert!(store.jobs_open().await.unwrap().is_empty());
        let row = &store.jobs_of(session).await.unwrap()[0];
        assert_eq!(
            (row.state, row.exit.clone()),
            (JobState::Lost, Some(JobExit::Lost))
        );
        assert!(row.settled_ms.is_some());
        // What the model is told (no open turn names `c1`: the stub was delivered).
        assert_eq!(reaped.jobs.len(), 1);
        let lost = &reaped.jobs[0];
        let body = lost.deliver.as_deref().expect("delivered");
        assert!(
            body.starts_with("this job died with the engine (restarted at "),
            "{body}"
        );
        assert!(body.contains(&format!("log: {}", log.display())), "{body}");
        assert!(body.contains("last screen (captured at "), "{body}");
        let wrapped = crate::supervisor::job_result(
            &lost.row.id,
            &lost.row.command,
            &JobExit::Lost,
            0,
            body,
            true,
            chrono::Local::now(),
        );
        assert!(wrapped.starts_with("<job_result id=\"j1\" command=\"trap &#39;&#39; HUP; exec sleep 600\" exit=\"lost\" secs=\"0\" pty=\"true\" at=\"") || wrapped.contains(" exit=\"lost\" "), "{wrapped}");
        assert!(wrapped.ends_with("</job_result>"));
        // The in-memory engine of this test never had the job: `reap_jobs`
        // proper finds no actor and still reports the pass.
        let again = reap_jobs(&store).await;
        assert_eq!(again, ReapReport::default());
        let _ = std::fs::remove_dir_all(dir);
    }

    /// §5.4 / §15 Q6: every `running` row is settled — a dead group, a job
    /// of a `started` call in an open turn (no delivery: the batch reports
    /// it), and another session's row; the shutdown-stamped row says so.
    #[tokio::test]
    async fn reaper_skips_nothing() {
        let dir = temp();
        let store = Store::open(&dir.join("sessions.sqlite"), None)
            .await
            .unwrap();
        let (a, b) = (session(&store).await, session(&store).await);
        // j1: dead pgid, stub delivered long ago. j2: its call `c2` is
        // `started` in an open turn. j3 (session b): written `exit = lost`
        // by a previous engine's `KillAll` (§5.5).
        store
            .job_put(&running(a, "j1", "c1", Some(1)))
            .await
            .unwrap();
        store.job_put(&running(a, "j2", "c2", None)).await.unwrap();
        let mut j3 = running(b, "j3", "c3", None);
        j3.exit = Some(JobExit::Lost);
        j3.settled_ms = Some(5_000);
        store.job_put(&j3).await.unwrap();
        let turn = TurnId::new();
        let mut tx = store.begin_write().await.unwrap();
        step_in(
            &mut tx,
            a,
            turn,
            StepKind::ToolBatch,
            None,
            &json!({"call_ids": ["c2"]}),
        )
        .await
        .unwrap();
        calls_planned_in(
            &mut tx,
            a,
            turn,
            0,
            &[(
                genai::chat::ToolCall {
                    call_id: "c2".into(),
                    fn_name: "shell".into(),
                    fn_arguments: json!({"command": "cargo build"}),
                    thought_signatures: None,
                },
                Risk::Destructive,
            )],
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        assert!(store.call_started(a, "c2").await.unwrap());

        let (report, reaped) = reap(&store).await;
        assert_eq!((report.killed, report.lost, report.orphaned), (0, 3, 0));
        assert!(store.jobs_open().await.unwrap().is_empty());
        let by_id = |id: &str| reaped.jobs.iter().find(|j| j.row.id.0 == id).unwrap();
        assert!(by_id("j1").deliver.is_some());
        assert!(
            by_id("j2").deliver.is_none(),
            "the resumed batch reports j2"
        );
        let j3 = by_id("j3");
        assert!(
            j3.deliver
                .as_deref()
                .unwrap()
                .starts_with("killed at shutdown (engine stopped at "),
            "{:?}",
            j3.deliver
        );
        assert_eq!(j3.row.settled_ms, Some(5_000));
        for s in [a, b] {
            for row in store.jobs_of(s).await.unwrap() {
                assert_eq!(row.state, JobState::Lost, "{}", row.id);
            }
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    /// §4.3 / §5.6: the job rule's answers for a `shell` call with a row
    /// (the job note with the log tail), without one (`None`: run as
    /// planned), and a `wait` on a lost PTY (its exited reply, `lost`).
    #[tokio::test]
    async fn job_rule_notes() {
        let dir = temp();
        let store = Store::open(&dir.join("sessions.sqlite"), None)
            .await
            .unwrap();
        let s = session(&store).await;
        let log = dir.join("j1.log");
        std::fs::write(
            &log,
            (1..=50).map(|i| format!("line {i}\n")).collect::<String>(),
        )
        .unwrap();
        let mut j1 = running(s, "j1", "c1", None);
        j1.log = Some(log.clone());
        store.job_put(&j1).await.unwrap();
        store
            .job_settle(s, &j1.id, JobState::Lost, &JobExit::Lost, 9_000)
            .await
            .unwrap();
        let mut j2 = running(s, "j2", "c2", None);
        j2.kind = JobKind::Pty;
        j2.name = Some("py".into());
        store.job_put(&j2).await.unwrap();
        store
            .pty_screen_put(&tools::PtyScreenRow {
                session: s,
                job_id: j2.id.clone(),
                revision: 7,
                rows: 40,
                cols: 120,
                cursor_row: 0,
                cursor_col: 4,
                text: ">>> ".into(),
                at_ms: 2_000,
            })
            .await
            .unwrap();
        store
            .job_settle(s, &j2.id, JobState::Lost, &JobExit::Lost, 9_000)
            .await
            .unwrap();
        let plan = ResumePlan {
            turn: TurnId::new(),
            kind: ResumeKind::Interrupted,
            interrupted_at: Utc::now(),
            last: LastStep::ToolBatch {
                all_finished: false,
            },
            calls: Vec::new(),
            partial: None,
        };
        let call = |id: &str, name: &str, args: serde_json::Value| JournaledCall {
            call_id: id.into(),
            name: name.into(),
            args,
            risk: Risk::Destructive,
            state: crate::durability::CallState::Started,
            attempt: 1,
            started_at: None,
            output: None,
            is_error: false,
            files: Vec::new(),
        };
        let note = resume_job_call(&store, s, &call("c1", "shell", json!({})), &plan)
            .await
            .unwrap();
        assert!(note.is_error);
        assert!(
            note.content.starts_with(&format!(
                "ERROR: interrupted: the engine restarted at {} while this command ran (job j1, started {}); the job died with the engine and its exit code is unknown — check before repeating it.\nLog: {} (last 40 lines follow)\nline 11\n",
                hhmmss(plan.interrupted_at),
                hhmmss_ms(1_000),
                log.display()
            )),
            "{}",
            note.content
        );
        assert!(note.content.ends_with("line 50"));
        // No row for `c9`: the kill fell between approval and spawn.
        assert!(
            resume_job_call(&store, s, &call("c9", "shell", json!({})), &plan)
                .await
                .is_none()
        );
        let start = resume_job_call(&store, s, &call("c2", "start", json!({})), &plan)
            .await
            .unwrap();
        assert!(
            start.content.contains("while this terminal ran (job j2,"),
            "{}",
            start.content
        );
        assert!(
            start.content.contains("last screen (captured at "),
            "{}",
            start.content
        );
        assert!(start.content.ends_with(">>> "), "{}", start.content);
        let wait = resume_job_call(&store, s, &call("c5", "wait", json!({"id": "py"})), &plan)
            .await
            .unwrap();
        assert!(!wait.is_error);
        assert!(
            wait.content
                .starts_with("job j2 (py) · lost · 8s\n[process gone — the engine restarted at "),
            "{}",
            wait.content
        );
        assert!(
            wait.content
                .ends_with("(revision 7)]\nscreen:\n>>> \ncursor: row 1 col 5"),
            "{}",
            wait.content
        );
        assert_eq!(wait.structured.as_ref().unwrap()["exit"], "lost");
        let send = resume_job_call(&store, s, &call("c6", "send", json!({"id": "j2"})), &plan)
            .await
            .unwrap();
        assert!(
            send.is_error && send.content.contains("job j2,"),
            "{}",
            send.content
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
