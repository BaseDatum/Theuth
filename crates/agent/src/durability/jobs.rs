//! Design doc 109 §5.4: the job reaper at start. Runs once after
//! `rearm_reminders_for(None)` and before `resume_scan` (§4.6 step 10): every
//! `running` `jobs` row whose pgid is still alive is killed, every `running`
//! row is settled `lost`, and the rows go to the jobs actor as
//! [`Reaped`] — which lists them `lost`, serves a lost PTY's stored screen,
//! and injects `<job_result exit="lost">` for the jobs whose stub the model
//! already had (the resumed batch reports the others itself, §4.3).
//!
//! 139 §7.1 adds the transfer half: [`settle_transferred`] settles the rows
//! another engine spawned (`jobs.engine != me`) as `lost` **without a
//! signal** — the process is not ours (I6) — and reports them with
//! `LostReason::OwnerChanged`.

use std::collections::HashSet;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use chrono::{DateTime, Local, Utc};
use kameo::actor::ActorRef;
use platform::ProcessTree;
use runtime::SessionId;
use tools::jobs::{LostJob, LostReason, Reaped};
use tools::{JOBS_ACTOR, JobExit, JobKind, JobRow, JobState, Jobs, PtyScreenRow};

use crate::durability::CallState;
use crate::store::durability::OpenTurnRow;
use crate::store::{Scope, Store};

/// Bytes of a lost job's log quoted in its `<job_result>` (`jobs.rs::TAIL`).
const TAIL: u64 = 8 * 1024;

/// Whether `jobs.call_id` belongs to tool call `call_id`. A direct `shell` /
/// `start` the loop bridges into a python cell (`repl::bridge_cell`) spawns
/// its job under the runtime's inner id `<call_id>.<n>` (`repl.rs`), so the
/// plain equality the 109 rules used missed it: the call read as "never
/// spawned" and was run again, and the job was reported twice.
pub(crate) fn job_of_call(job_call_id: &str, call_id: &str) -> bool {
    job_call_id == call_id
        || job_call_id
            .strip_prefix(call_id)
            .is_some_and(|rest| rest.starts_with('.'))
}

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
/// then hand the rows to the jobs actor. Nothing is skipped. Reads the open
/// turns itself; the boot pass passes its own set ([`reap_jobs_in`], 146 D5).
pub async fn reap_jobs(store: &Store) -> ReapReport {
    let open = store.open_turns(Scope::Here).await.unwrap_or_else(|e| {
        tracing::error!("109 reaper: open_turns: {e:#}");
        Vec::new()
    });
    reap_jobs_in(store, &open).await
}

/// [`reap_jobs`] over an already computed open-turn set (`Scope::Here`).
pub async fn reap_jobs_in(store: &Store, open: &[OpenTurnRow]) -> ReapReport {
    let (report, reaped) = reap(store, open).await;
    tell_jobs(reaped).await;
    report
}

/// Hand lost rows to the jobs actor (`Reaped`); logs when it is not running.
async fn tell_jobs(reaped: Reaped) {
    if reaped.jobs.is_empty() {
        return;
    }
    match ActorRef::<Jobs>::lookup(JOBS_ACTOR) {
        Ok(Some(jobs)) => {
            let _ = jobs.tell(reaped).await;
        }
        _ => tracing::error!("jobs actor not running: lost jobs not reported"),
    }
}

/// 139 §7.1, on the new owner of `session` after a transfer or takeover
/// from `from`: every `running` row another engine spawned is settled
/// `lost` in the store (`jobs_settle_transferred`: no `adopt`, no `killpg`
/// — I6), handed to the jobs actor as [`Reaped`] with
/// `LostReason::OwnerChanged`, and returned for the resume report's
/// `lost_jobs`. A job whose call is `started` in the session's open turn
/// gets no `deliver` body: the resumed batch reports it (`resume_job_call`).
pub async fn settle_transferred(store: &Store, session: SessionId, from: &str) -> Reaped {
    let now = Utc::now();
    let mut reaped = Reaped {
        restarted_at: now,
        jobs: Vec::new(),
    };
    let rows = match store
        .jobs_settle_transferred(session, store.engine(), now.timestamp_millis())
        .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!(session = %session, "139 settle_transferred: {e:#}");
            return reaped;
        }
    };
    if rows.is_empty() {
        return reaped;
    }
    let mut in_batch: HashSet<String> = HashSet::new();
    if let Ok(Some((turn, _))) = store.open_turn(session).await
        && let Ok(calls) = store.calls_of(session, turn).await
    {
        in_batch.extend(
            calls
                .into_iter()
                .filter(|c| c.state == CallState::Started)
                .map(|c| c.call_id),
        );
    }
    for row in rows {
        tracing::info!(
            session = %session,
            "139: job {} ran on {} (taken from {from}); settled lost here",
            row.id,
            row.engine
        );
        // 171F3 R1: a user-cell kernel is never a `<job_result>`; its screen and
        // log are never model input (the cells actor reports its lost run).
        let cell = row.is_cell_kernel();
        let screen = match row.kind {
            JobKind::Pty if !cell => store.pty_screen(session, &row.id).await.ok().flatten(),
            _ => None,
        };
        // The log lives on the engine that ran it: empty here.
        let tail = match &row.log {
            Some(log) if !cell => log_tail(log),
            _ => String::new(),
        };
        let reason = LostReason::OwnerChanged {
            engine: row.engine.clone(),
        };
        let deliver = (!cell && !in_batch.iter().any(|c| job_of_call(&row.call_id, c)))
            .then(|| lost_body(&row, screen.as_ref(), &tail, now, &reason));
        reaped.jobs.push(LostJob {
            row,
            screen,
            tail,
            deliver,
            reason,
        });
    }
    tell_jobs(reaped.clone()).await;
    reaped
}

/// The store half of [`reap_jobs`]: kill, settle, and build what the jobs
/// actor is told.
pub(crate) async fn reap(store: &Store, open: &[OpenTurnRow]) -> (ReapReport, Reaped) {
    let restarted_at = Utc::now();
    let mut report = ReapReport::default();
    let mut reaped = Reaped {
        restarted_at,
        jobs: Vec::new(),
    };
    // 139 D1/D14: sessions owned here, rows this engine spawned (the SQL
    // carries the engine filter); `settle_transferred` handles the rest.
    let rows = match store.jobs_open(Scope::Here).await {
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
    for (session, turn, _) in open {
        if let Ok(calls) = store.calls_of(*session, *turn).await {
            in_batch.extend(
                calls
                    .into_iter()
                    .filter(|c| c.state == CallState::Started)
                    .map(|c| (*session, c.call_id)),
            );
        }
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
        // 171F3 R1: a user-cell kernel is never a `<job_result>`; its screen and
        // log are never model input (the cells actor reports its lost run).
        let cell = row.is_cell_kernel();
        let screen = match row.kind {
            JobKind::Pty if !cell => store.pty_screen(row.session, &row.id).await.ok().flatten(),
            _ => None,
        };
        let tail = match &row.log {
            Some(log) if !cell => log_tail(log),
            _ => String::new(),
        };
        let in_turn = in_batch
            .iter()
            .any(|(s, c)| *s == row.session && job_of_call(&row.call_id, c));
        let deliver = (!cell && !in_turn).then(|| {
            lost_body(
                &row,
                screen.as_ref(),
                &tail,
                restarted_at,
                &LostReason::Restart,
            )
        });
        row.state = JobState::Lost;
        row.exit = Some(JobExit::Lost);
        // `KillAll` stamped the shutdown time (§5.5); a crash left none.
        row.settled_ms.get_or_insert(now_ms);
        lost.push(LostJob {
            row,
            screen,
            tail,
            deliver,
            reason: LostReason::Restart,
        });
    }
    reaped.jobs = lost;
    (report, reaped)
}

/// The `<job_result exit="lost">` body (§5.4 / §5.5, 139 §7.1): why the
/// job is gone, its log path, then the log tail — or the stored screen for
/// a PTY.
pub(crate) fn lost_body(
    row: &JobRow,
    screen: Option<&PtyScreenRow>,
    tail: &str,
    restarted_at: DateTime<Utc>,
    reason: &LostReason,
) -> String {
    let mut body = match (reason, row.exit.as_ref(), row.settled_ms) {
        // The session moved here while the process lived on `engine`
        // (killed by its release drain, or still running after a takeover).
        (LostReason::OwnerChanged { engine }, _, _) => format!(
            "owner changed (ran on {engine}): the session moved here at {}; the job stayed behind \
             and its exit code is unknown.",
            hhmmss(restarted_at)
        ),
        // `exit = lost` on a row the reaper found still `running`: the
        // shutdown path wrote it (§5.5) before the kill.
        (LostReason::Restart, Some(JobExit::Lost), Some(stopped_ms)) => format!(
            "killed at shutdown (engine stopped at {}, restarted at {})",
            hhmmss_ms(stopped_ms),
            hhmmss(restarted_at)
        ),
        (LostReason::Restart, _, _) => format!(
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
        None if row.kind == JobKind::Promoted => {
            body.push_str("\nthe tool's result was never produced.");
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

    fn temp() -> crate::TestDir {
        crate::TestDir::new("109-w2")
    }

    /// 139 Z-fix: a bridged call's job (`<call_id>.<n>`) is that call's; a
    /// sibling with a longer id or another call is not.
    #[test]
    fn job_of_call_accepts_the_bridged_inner_id() {
        assert!(job_of_call("smoke_1_shell", "smoke_1_shell"));
        assert!(job_of_call("smoke_1_shell.1", "smoke_1_shell"));
        assert!(job_of_call("smoke_1_shell.12", "smoke_1_shell"));
        assert!(!job_of_call("smoke_1_shell_b", "smoke_1_shell"));
        assert!(!job_of_call("smoke_1_shell", "smoke_1_shell.1"));
        assert!(!job_of_call("smoke_2_start.1", "smoke_1_shell"));
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
            engine: String::new(),
        }
    }

    /// §11.2: `sleep 600` through the actor, the engine "SIGKILLed" (its
    /// runtime dropped without `KillAll`), a fresh store on the same file,
    /// `reap_jobs`: the group is dead, the row `lost`, and the
    /// `<job_result exit="lost">` names the log and the last screen.
    #[tokio::test]
    async fn reaper_kills_leaked_group() {
        let scratch = temp();
        let dir = scratch.0.clone();
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
                            program: None,
                            cell: None,
                        })
                        .await
                        .unwrap();
                    assert!(
                        out.content.starts_with("job j1 · running"),
                        "{}",
                        out.content
                    );
                    let rows = store.jobs_open(Scope::Here).await.unwrap();
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
        let open = store.open_turns(Scope::Here).await.unwrap();
        let (report, reaped) = reap(&store, &open).await;
        assert_eq!((report.killed, report.lost, report.orphaned), (1, 1, 0));
        let gone = async {
            while tree.alive() {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        };
        tokio::time::timeout(Duration::from_secs(5), gone)
            .await
            .expect("killpg(pgid, 0) still succeeds after the reaper");
        assert!(store.jobs_open(Scope::Here).await.unwrap().is_empty());
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
            JobKind::Pty,
            chrono::Local::now(),
        );
        assert!(wrapped.starts_with("<job_result id=\"j1\" command=\"trap &#39;&#39; HUP; exec sleep 600\" exit=\"lost\" secs=\"0\" pty=\"true\" at=\"") || wrapped.contains(" exit=\"lost\" "), "{wrapped}");
        assert!(wrapped.ends_with("</job_result>"));
        // The in-memory engine of this test never had the job: `reap_jobs`
        // proper finds no actor and still reports the pass.
        let again = reap_jobs(&store).await;
        assert_eq!(again, ReapReport::default());
    }

    /// §5.4 / §15 Q6: every `running` row is settled — a dead group, a job
    /// of a `started` call in an open turn (no delivery: the batch reports
    /// it), and another session's row; the shutdown-stamped row says so.
    #[tokio::test]
    async fn reaper_skips_nothing() {
        let scratch = temp();
        let dir = scratch.0.clone();
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

        let open = store.open_turns(Scope::Here).await.unwrap();
        let (report, reaped) = reap(&store, &open).await;
        assert_eq!((report.killed, report.lost, report.orphaned), (0, 3, 0));
        assert!(store.jobs_open(Scope::Here).await.unwrap().is_empty());
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
    }

    /// A live local process group standing in for one another engine
    /// spawned (same pid namespace in the test): `(pgid, tree, child)` —
    /// the caller kills the tree and `wait`s the child.
    fn foreign_group() -> (i64, ProcessTree, std::process::Child) {
        use std::os::unix::process::CommandExt;
        let child = std::process::Command::new("sleep")
            .arg("600")
            .process_group(0)
            .spawn()
            .unwrap();
        let pgid = child.id();
        let tree = ProcessTree::adopt(pgid).unwrap();
        assert!(tree.alive());
        (i64::from(pgid), tree, child)
    }

    /// 139 I6 / D14: on engine `b`, a `running` row with `engine = "a"`
    /// is neither adopted nor killed by the reaper — its group is not
    /// ours — and is not reported; `b`'s own row is reaped as before.
    #[tokio::test]
    async fn reaper_ignores_foreign_engine_rows() {
        let scratch = temp();
        let dir = scratch.0.clone();
        let store = Store::open(
            &dir.join("sessions.sqlite"),
            Some(mesh::EngineId("b".into())),
        )
        .await
        .unwrap();
        let s = session(&store).await;
        let (pgid, tree, mut child) = foreign_group();
        let mut foreign = running(s, "j1", "c1", Some(pgid));
        foreign.engine = "a".into();
        store.job_put(&foreign).await.unwrap();
        let mut mine = running(s, "j2", "c2", Some(1));
        mine.engine = "b".into();
        store.job_put(&mine).await.unwrap();

        let open = store.open_turns(Scope::Here).await.unwrap();
        let (report, reaped) = reap(&store, &open).await;
        assert_eq!((report.killed, report.lost, report.orphaned), (0, 1, 0));
        assert_eq!(reaped.jobs.len(), 1);
        assert_eq!(reaped.jobs[0].row.id.0, "j2");
        assert_eq!(reaped.jobs[0].reason, LostReason::Restart);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            tree.alive(),
            "the reaper signalled a group it did not spawn"
        );
        let rows = store.jobs_of(s).await.unwrap();
        let j1 = rows.iter().find(|r| r.id.0 == "j1").unwrap();
        assert_eq!(
            j1.state,
            JobState::Running,
            "foreign row settled by the reaper"
        );
        tree.kill();
        let _ = child.wait();
    }

    /// 139 §7.1: `settle_transferred` on the new owner settles the
    /// previous owner's rows `lost` with the owner-changed body — the
    /// `started` call of the open turn gets no body (the batch reports it),
    /// a PTY gets its replicated screen — and signals nothing; `b`'s own
    /// row is not touched.
    #[tokio::test]
    async fn settle_transferred_never_signals() {
        let scratch = temp();
        let dir = scratch.0.clone();
        let store = Store::open(
            &dir.join("sessions.sqlite"),
            Some(mesh::EngineId("b".into())),
        )
        .await
        .unwrap();
        let s = session(&store).await;
        let (pgid, tree, mut child) = foreign_group();
        // j1: a's shell job, stub delivered. j2: a's PTY with a replicated
        // screen, its call `c2` started in the open turn. j3: b's own.
        let mut j1 = running(s, "j1", "c1", Some(pgid));
        j1.engine = "a".into();
        j1.exit = Some(JobExit::Lost);
        j1.settled_ms = Some(5_000);
        store.job_put(&j1).await.unwrap();
        let mut j2 = running(s, "j2", "c2", None);
        j2.engine = "a".into();
        j2.kind = JobKind::Pty;
        store.job_put(&j2).await.unwrap();
        store
            .pty_screen_put(&tools::PtyScreenRow {
                session: s,
                job_id: j2.id.clone(),
                revision: 3,
                rows: 40,
                cols: 120,
                cursor_row: 0,
                cursor_col: 2,
                text: "$ ".into(),
                at_ms: 2_000,
            })
            .await
            .unwrap();
        let mut j3 = running(s, "j3", "c3", None);
        j3.engine = "b".into();
        store.job_put(&j3).await.unwrap();
        let turn = TurnId::new();
        let mut tx = store.begin_write().await.unwrap();
        step_in(
            &mut tx,
            s,
            turn,
            StepKind::ToolBatch,
            None,
            &json!({"call_ids": ["c2"]}),
        )
        .await
        .unwrap();
        calls_planned_in(
            &mut tx,
            s,
            turn,
            0,
            &[(
                genai::chat::ToolCall {
                    call_id: "c2".into(),
                    fn_name: "start".into(),
                    fn_arguments: json!({"command": "bash -l"}),
                    thought_signatures: None,
                },
                Risk::Destructive,
            )],
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        assert!(store.call_started(s, "c2").await.unwrap());

        let reaped = settle_transferred(&store, s, "a").await;
        assert_eq!(reaped.jobs.len(), 2);
        let by_id = |id: &str| reaped.jobs.iter().find(|j| j.row.id.0 == id).unwrap();
        let j1 = by_id("j1");
        assert_eq!(j1.reason, LostReason::OwnerChanged { engine: "a".into() });
        assert_eq!(
            (j1.row.state, j1.row.exit.clone()),
            (JobState::Lost, Some(JobExit::Lost))
        );
        let body = j1.deliver.as_deref().expect("stub delivered → body");
        assert!(
            body.starts_with("owner changed (ran on a): the session moved here at "),
            "{body}"
        );
        let j2 = by_id("j2");
        assert!(j2.deliver.is_none(), "the resumed batch reports j2");
        assert_eq!(j2.screen.as_ref().map(|s| s.revision), Some(3));
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(tree.alive(), "settle_transferred signalled a's group");
        let rows = store.jobs_of(s).await.unwrap();
        let state = |id: &str| rows.iter().find(|r| r.id.0 == id).unwrap().state;
        assert_eq!(
            (state("j1"), state("j2"), state("j3")),
            (JobState::Lost, JobState::Lost, JobState::Running)
        );
        // Idempotent: nothing left to settle.
        assert!(settle_transferred(&store, s, "a").await.jobs.is_empty());
        tree.kill();
        let _ = child.wait();
    }

    /// §4.3 / §5.6: the job rule's answers for a `shell` call with a row
    /// (the job note with the log tail), without one (`None`: run as
    /// planned), and a `wait` on a lost PTY (its exited reply, `lost`).
    #[tokio::test]
    async fn job_rule_notes() {
        let scratch = temp();
        let dir = scratch.0.clone();
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
            cause: crate::durability::ResumeCause::Crash,
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
    }

    /// 171F3 R1: an agent PTY and two user-cell kernels — one marked by its
    /// `cell:` name, one (an older row) by its command only — all alive at a
    /// shutdown, through the boot reaper: exactly one `<job_result>` body (the
    /// agent PTY, with its screen), none for the kernels, no kernel screen or
    /// log in any of it, and a resumed `wait` / `send` on a kernel is refused.
    #[tokio::test]
    async fn reaper_never_reports_a_cell_kernel() {
        let scratch = temp();
        let dir = scratch.0.clone();
        let store = Store::open(&dir.join("sessions.sqlite"), None)
            .await
            .unwrap();
        let s = session(&store).await;
        const SECRET: &str = "Identity added: /home/u/.ssh/id_ed25519";
        let screen = |id: &str, text: &str| tools::PtyScreenRow {
            session: s,
            job_id: tools::JobId(id.into()),
            revision: 9,
            rows: 40,
            cols: 120,
            cursor_row: 0,
            cursor_col: 0,
            text: text.into(),
            at_ms: 2_000,
        };
        let stamped = |mut row: JobRow| {
            row.kind = JobKind::Pty;
            row.exit = Some(JobExit::Lost);
            row.settled_ms = Some(5_000);
            row
        };
        let mut agent = stamped(running(s, "j1", "c1", None));
        agent.command = "bash -l".into();
        let kernel_log = dir.join("j2.log");
        std::fs::write(&kernel_log, format!("{SECRET}\n")).unwrap();
        let mut named = stamped(running(s, "j2", "", None));
        named.name = Some("cell:zsh kd71fefe799f4".into());
        named.command = format!("/bin/zsh{}", tools::jobs::CELL_COMMAND);
        named.log = Some(kernel_log);
        let mut unnamed = stamped(running(s, "j3", "", None));
        unnamed.command = format!("/bin/sh{}", tools::jobs::CELL_COMMAND);
        for (row, text) in [(&agent, "$ make"), (&named, SECRET), (&unnamed, SECRET)] {
            store.job_put(row).await.unwrap();
            store
                .pty_screen_put(&screen(&row.id.0, text))
                .await
                .unwrap();
        }

        let (report, reaped) = reap(&store, &[]).await;
        assert_eq!((report.lost, report.orphaned), (3, 0));
        let bodies: Vec<(&str, &str)> = reaped
            .jobs
            .iter()
            .filter_map(|j| Some((j.row.id.0.as_str(), j.deliver.as_deref()?)))
            .collect();
        assert_eq!(bodies.len(), 1, "{bodies:?}");
        assert_eq!(bodies[0].0, "j1");
        assert!(bodies[0].1.ends_with("\n$ make"), "{}", bodies[0].1);
        for j in reaped.jobs.iter().filter(|j| j.row.id.0 != "j1") {
            assert!(j.row.is_cell_kernel(), "{}", j.row.id);
            assert_eq!((&j.screen, j.tail.as_str()), (&None, ""), "{}", j.row.id);
        }

        let plan = ResumePlan {
            turn: TurnId::new(),
            kind: ResumeKind::Interrupted,
            cause: crate::durability::ResumeCause::Crash,
            interrupted_at: Utc::now(),
            last: LastStep::ToolBatch {
                all_finished: false,
            },
            calls: Vec::new(),
            partial: None,
        };
        for (name, id) in [
            ("wait", "j2"),
            ("wait", "j3"),
            ("send", "cell:zsh kd71fefe799f4"),
        ] {
            let call = JournaledCall {
                call_id: format!("c-{name}-{id}"),
                name: name.into(),
                args: json!({ "id": id }),
                risk: Risk::Destructive,
                state: crate::durability::CallState::Started,
                attempt: 1,
                started_at: None,
                output: None,
                is_error: false,
                files: Vec::new(),
            };
            let out = resume_job_call(&store, s, &call, &plan).await.unwrap();
            assert!(
                out.is_error && out.content == tools::jobs::CELL_REFUSED,
                "{name} {id}: {}",
                out.content
            );
        }
    }

    /// 171F4 T2-7, the 2.18.2 shape: a bash kernel (`j24`, `.cell.log`) killed at shutdown
    /// whose persisted last screen and log hold a `#| private` cell's output and an
    /// AWS-key-shaped line; through the boot reaper nothing is delivered — no `<job_result>`
    /// body, no screen, no log tail — and neither string is anywhere in what the jobs actor
    /// is told (the only road to a model input from here).
    #[tokio::test]
    async fn a_killed_kernels_private_screen_reaches_no_model_input() {
        let scratch = temp();
        let dir = scratch.0.clone();
        let store = Store::open(&dir.join("sessions.sqlite"), None)
            .await
            .unwrap();
        let s = session(&store).await;
        const KEY: &str = "AKIAIOSFODNN7EXAMPLE";
        const PRIVATE: &str = "private cell output: hunter2";
        let screen_text = format!("{PRIVATE}\n{KEY}\n$ ");
        let log = dir.join("j24.cell.log");
        std::fs::write(&log, &screen_text).unwrap();
        let mut row = running(s, "j24", "", None);
        row.kind = JobKind::Pty;
        row.command = format!("/opt/homebrew/bin/bash{}", tools::jobs::CELL_COMMAND);
        row.log = Some(log);
        // `KillAll` wrote it `exit = lost` with the shutdown time (109 §5.5).
        row.exit = Some(JobExit::Lost);
        row.settled_ms = Some(5_000);
        store.job_put(&row).await.unwrap();
        store
            .pty_screen_put(&tools::PtyScreenRow {
                session: s,
                job_id: row.id.clone(),
                revision: 3,
                rows: 30,
                cols: 100,
                cursor_row: 2,
                cursor_col: 2,
                text: screen_text.clone(),
                at_ms: 4_000,
            })
            .await
            .unwrap();

        let (report, reaped) = reap(&store, &[]).await;
        assert_eq!((report.lost, report.orphaned), (1, 0));
        let [j] = reaped.jobs.as_slice() else {
            panic!("{reaped:?}");
        };
        assert!(j.row.is_cell_kernel());
        assert_eq!((&j.deliver, &j.screen, j.tail.as_str()), (&None, &None, ""));
        let told = format!("{reaped:?}");
        assert!(!told.contains(KEY) && !told.contains("hunter2"), "{told}");
    }
}
