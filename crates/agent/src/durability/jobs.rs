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
    let restarted_at = Utc::now();
    let mut report = ReapReport::default();
    let rows = match store.jobs_open().await {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!("109 reaper: jobs_open: {e:#}");
            return report;
        }
    };
    if rows.is_empty() {
        return report;
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
    if !lost.is_empty() {
        match ActorRef::<Jobs>::lookup(JOBS_ACTOR) {
            Ok(Some(jobs)) => {
                let _ = jobs
                    .tell(Reaped {
                        restarted_at,
                        jobs: lost,
                    })
                    .await;
            }
            _ => tracing::error!("jobs actor not running: lost jobs not reported"),
        }
    }
    report
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
