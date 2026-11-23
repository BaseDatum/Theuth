use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use runtime::SessionId;
use serde::{Deserialize, Serialize};
use tools::{JobExit, JobId, JobKind, JobRow, JobView};

use crate::store::Store;

/// Exclusive position in job history, ordered by start time then id, both descending.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct JobCursor {
    /// The last returned job's start time, unix milliseconds.
    pub started_ms: i64,
    /// The last returned job's id, breaking ties at the same millisecond.
    pub id: JobId,
}

/// One UTF-8 log page. Offsets always address the stored bytes, before redaction.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct JobLogPage {
    /// Secret-scrubbed UTF-8 output.
    pub text: String,
    /// Inclusive byte offset in the stored log, before redaction.
    pub start: u64,
    /// Exclusive byte offset; pass it as offset to read subsequent output.
    pub end: u64,
    /// Stored log size at the time of the read.
    pub total_bytes: u64,
    /// This page reached the observed end of the log.
    pub eof: bool,
}

pub(super) async fn history(
    store: &Store,
    session: SessionId,
    live: Vec<JobView>,
    before: Option<JobCursor>,
    limit: Option<u32>,
) -> Result<Vec<JobView>, String> {
    let mut rows: HashMap<_, _> = store
        .jobs_of(session)
        .await
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|row| !row.is_cell_kernel())
        .map(|row| (row.id.clone(), row))
        .collect();
    let mut views = Vec::with_capacity(rows.len() + live.len());
    for mut job in live {
        if let Some(row) = rows.remove(&job.id)
            && job.state != "running"
        {
            job.ended_ms = row.settled_ms.or(job.ended_ms);
        }
        views.push((job, false));
    }
    views.extend(
        rows.into_values()
            .map(|row| (view(row, String::new()), true)),
    );
    views.retain(|(job, _)| {
        before.as_ref().is_none_or(|cursor| {
            (job.started_ms.unwrap_or(0), job.id.0.as_str())
                < (cursor.started_ms, cursor.id.0.as_str())
        })
    });
    views.sort_unstable_by(|(a, _), (b, _)| {
        b.started_ms
            .cmp(&a.started_ms)
            .then_with(|| b.id.0.cmp(&a.id.0))
    });
    views.truncate(limit.unwrap_or(50).clamp(1, 200) as usize);
    // Only this page's historical rows need filesystem reads / stored screens.
    for (job, historical) in &mut views {
        if *historical {
            job.tail = if job.pty {
                store
                    .pty_screen(session, &job.id)
                    .await
                    .map_err(|e| e.to_string())?
                    .map_or_else(String::new, |s| {
                        tools::secrets::redact(&s.text).into_owned()
                    })
            } else {
                read_log(&job.log, None, Some(8192)).map_or_else(|_| String::new(), |p| p.text)
            };
            job.log_bytes = std::fs::metadata(&job.log).ok().map(|m| m.len());
        }
    }
    Ok(views.into_iter().map(|(job, _)| job).collect())
}

fn view(row: JobRow, tail: String) -> JobView {
    let now = chrono::Utc::now().timestamp_millis();
    JobView {
        session: row.session,
        id: row.id,
        state: row
            .exit
            .as_ref()
            .map_or_else(|| row.state.as_str().to_owned(), ToString::to_string),
        secs: row
            .settled_ms
            .unwrap_or(now)
            .saturating_sub(row.started_ms)
            .max(0) as u64
            / 1000,
        exit_code: match row.exit {
            Some(JobExit::Exited(code)) => Some(i32::from(code)),
            _ => None,
        },
        started_ms: Some(row.started_ms),
        ended_ms: row.settled_ms,
        kind: Some(if row.kind == JobKind::Promoted {
            row.command.clone()
        } else {
            row.kind.as_str().to_owned()
        }),
        command: tools::secrets::redact(&row.command).into_owned(),
        name: row.name.map(|n| tools::secrets::redact(&n).into_owned()),
        call_id: Some(row.call_id),
        log_bytes: None,
        log: row.log.unwrap_or_default(),
        tail,
        pty: row.kind == JobKind::Pty,
    }
}

pub(super) async fn log(
    store: &Store,
    session: SessionId,
    id: &JobId,
    live: Vec<JobView>,
    offset: Option<u64>,
    limit: Option<u32>,
) -> Result<JobLogPage, String> {
    // Look up an owned row, never interpret a caller's id as a path. Persisted
    // kernel markers also protect rows from older engines, after restart.
    let row = store
        .jobs_of(session)
        .await
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|j| j.id == *id);
    if row.as_ref().is_some_and(JobRow::is_cell_kernel) {
        return Err(tools::jobs::CELL_REFUSED.into());
    }
    let job = live.into_iter().find(|j| j.id == *id);
    let (path, pty) = match (job, row) {
        (Some(j), _) => (j.log, j.pty),
        (_, Some(j)) => (j.log.unwrap_or_default(), j.kind == JobKind::Pty),
        _ => return Err("no such job".into()),
    };
    if pty {
        return Err("Open terminal to read this PTY's stored screen and scrollback".into());
    }
    tokio::task::spawn_blocking(move || read_log(&path, offset, limit))
        .await
        .map_err(|e| e.to_string())?
}

fn read_log(path: &Path, offset: Option<u64>, limit: Option<u32>) -> Result<JobLogPage, String> {
    read_log_with(
        path,
        offset,
        limit,
        tools::secrets::max_literal_len(),
        &tools::secrets::redact_spans,
        &|s| tools::secrets::redact(s).into_owned(),
    )
}

fn read_log_with(
    path: &Path,
    offset: Option<u64>,
    limit: Option<u32>,
    context: usize,
    spans: &impl Fn(&str) -> Vec<std::ops::Range<usize>>,
    redact: &impl Fn(&str) -> String,
) -> Result<JobLogPage, String> {
    let mut file = std::fs::File::open(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            "log no longer kept".to_owned()
        } else {
            e.to_string()
        }
    })?;
    let total = file.metadata().map_err(|e| e.to_string())?.len();
    let limit = u64::from(limit.unwrap_or(64 * 1024).clamp(1, 64 * 1024));
    let start = offset
        .unwrap_or_else(|| total.saturating_sub(limit))
        .min(total);
    let end = start.saturating_add(limit).min(total);
    let base = start.saturating_sub(context as u64 + 3);
    let stop = end.saturating_add(context as u64 + 3).min(total);
    file.seek(SeekFrom::Start(base))
        .map_err(|e| e.to_string())?;
    let mut bytes = Vec::with_capacity((stop - base) as usize);
    file.take(stop - base)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    let mut lo = (start - base) as usize;
    let mut hi = ((end - base) as usize).min(bytes.len());
    while lo < bytes.len() && bytes[lo] & 0xc0 == 0x80 {
        lo += 1;
    }
    hi = hi.max(lo);
    while hi > lo && hi < bytes.len() && bytes[hi] & 0xc0 == 0x80 {
        hi -= 1;
    }
    if let Err(error) = std::str::from_utf8(&bytes[lo..hi])
        && error.error_len().is_none()
    {
        hi = lo + error.valid_up_to();
    }
    // Mask only matches crossing a page boundary. Interior matches go through
    // the same redactor as tool output. Context prevents partial-secret leaks.
    let text = String::from_utf8_lossy(&bytes);
    let (text_lo, text_hi) = if matches!(text, std::borrow::Cow::Borrowed(_)) {
        (lo, hi)
    } else {
        let start = String::from_utf8_lossy(&bytes[..lo]).len();
        (start, start + String::from_utf8_lossy(&bytes[lo..hi]).len())
    };
    let mut page = std::borrow::Cow::Borrowed(&text.as_bytes()[text_lo..text_hi]);
    for span in spans(&text) {
        if (span.start < text_lo && span.end > text_lo)
            || (span.start < text_hi && span.end > text_hi)
        {
            let a = span.start.max(text_lo) - text_lo;
            let b = span.end.min(text_hi) - text_lo;
            if a < b {
                page.to_mut()[a..b].fill(b'*');
            }
        }
    }
    let text = redact(&String::from_utf8_lossy(&page));
    Ok(JobLogPage {
        text,
        start: base + lo as u64,
        end: base + hi as u64,
        total_bytes: total,
        eof: base + hi as u64 >= total,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use kameo::actor::Spawn;
    use std::time::Duration;
    use tools::{JobState, Jobs, ListJobs};

    fn row(session: SessionId, id: &str, started_ms: i64, log: &Path) -> JobRow {
        JobRow {
            session,
            id: JobId(id.into()),
            kind: JobKind::Shell,
            call_id: format!("call-{id}"),
            command: "echo done".into(),
            name: None,
            pgid: None,
            log: Some(log.into()),
            started_ms,
            state: JobState::Settled,
            exit: Some(JobExit::Exited(0)),
            settled_ms: Some(started_ms + 1500),
            engine: String::new(),
        }
    }

    #[tokio::test]
    async fn history_survives_restart_merges_live_and_excludes_cells() {
        let dir = crate::test_dir("job-history");
        let path = dir.path().join("sessions.sqlite");
        let logfile = dir.path().join("j.log");
        std::fs::write(&logfile, "earlier\nfinished\n").unwrap();
        let session = SessionId::new();
        {
            let store = Store::open(&path, None).await.unwrap();
            store
                .create_session(session, "m", None, None, None)
                .await
                .unwrap();
            for n in 1..=55 {
                store
                    .job_put(&row(session, &format!("j{n}"), n * 1000, &logfile))
                    .await
                    .unwrap();
            }
            for (n, marker) in ["name", "command", "log"].into_iter().enumerate() {
                let mut cell = row(session, &format!("cell{n}"), 99_000, &logfile);
                match marker {
                    "name" => cell.name = Some("cell:private".into()),
                    "command" => cell.command.push_str(tools::jobs::CELL_COMMAND),
                    _ => cell.log = Some(dir.path().join("j.cell.log")),
                }
                store.job_put(&cell).await.unwrap();
            }
        }
        let store = Store::open(&path, None).await.unwrap();
        let rt = runtime::Runtime::new(figment::Figment::new(), dir.path().to_path_buf());
        let actor = Jobs::spawn(Jobs::new(
            rt,
            Duration::from_millis(10),
            Duration::from_secs(5),
        ));
        let fresh = actor.ask(ListJobs(session)).await.unwrap();
        assert!(fresh.is_empty());
        let page = history(&store, session, fresh, None, None).await.unwrap();
        assert_eq!(page.len(), 50);
        assert_eq!(page[0].id.0, "j55");
        assert_eq!(page[0].exit_code, Some(0));
        assert_eq!(page[0].ended_ms, Some(56_500));
        assert_eq!(page[0].tail, "earlier\nfinished\n");
        let older = history(
            &store,
            session,
            vec![],
            Some(JobCursor {
                started_ms: page.last().unwrap().started_ms.unwrap(),
                id: page.last().unwrap().id.clone(),
            }),
            None,
        )
        .await
        .unwrap();
        assert_eq!(
            older.iter().map(|j| j.id.0.as_str()).collect::<Vec<_>>(),
            ["j5", "j4", "j3", "j2", "j1"]
        );
        let mut live = page[0].clone();
        live.state = "running".into();
        live.ended_ms = None;
        let merged = history(&store, session, vec![live], None, Some(2000))
            .await
            .unwrap();
        assert_eq!(merged.len(), 55);
        assert_eq!(merged[0].state, "running");
        for n in 0..3 {
            assert_eq!(
                log(
                    &store,
                    session,
                    &JobId(format!("cell{n}")),
                    vec![],
                    None,
                    None
                )
                .await
                .unwrap_err(),
                tools::jobs::CELL_REFUSED
            );
        }
        assert_eq!(
            log(
                &store,
                SessionId::new(),
                &JobId("j55".into()),
                vec![],
                None,
                None
            )
            .await
            .unwrap_err(),
            "no such job"
        );
        assert_eq!(
            log(&store, session, &JobId("j55".into()), vec![], None, Some(8))
                .await
                .unwrap()
                .text,
            "inished\n"
        );
        std::fs::remove_file(&logfile).unwrap();
        assert_eq!(
            log(&store, session, &JobId("j55".into()), vec![], None, None)
                .await
                .unwrap_err(),
            "log no longer kept"
        );
    }

    #[tokio::test]
    async fn history_cursor_pages_equal_start_times_without_skips_or_duplicates() {
        let dir = crate::test_dir("job-history-cursor");
        let store = Store::open(&dir.path().join("sessions.sqlite"), None)
            .await
            .unwrap();
        let session = SessionId::new();
        store
            .create_session(session, "m", None, None, None)
            .await
            .unwrap();
        let log = dir.path().join("j.log");
        for (id, started_ms) in [
            ("j1", 1000),
            ("j4", 1000),
            ("j10", 1000),
            ("j2", 1000),
            ("j3", 1000),
            ("j5", 2000),
            ("j0", 0),
        ] {
            store
                .job_put(&row(session, id, started_ms, &log))
                .await
                .unwrap();
        }
        let mut before = None;
        let mut ids = Vec::new();
        for _ in 0..5 {
            let page = history(&store, session, vec![], before, Some(2))
                .await
                .unwrap();
            let Some(last) = page.last() else { break };
            before = Some(JobCursor {
                started_ms: last.started_ms.unwrap(),
                id: last.id.clone(),
            });
            ids.extend(page.into_iter().map(|job| job.id.0));
        }
        assert_eq!(ids, ["j5", "j4", "j3", "j2", "j10", "j1", "j0"]);
    }

    #[test]
    fn log_pages_keep_utf8_offsets_and_caps() {
        let dir = crate::test_dir("job-log-pages");
        let path = dir.path().join("j.log");
        let content = "aé𐍈z".repeat(20_000);
        std::fs::write(&path, &content).unwrap();
        let tail = read_log(&path, None, None).unwrap();
        assert!(tail.eof);
        assert!(tail.text.len() <= 65536);
        assert_eq!(&content[tail.start as usize..tail.end as usize], tail.text);
        let mut offset = 0;
        let mut joined = String::new();
        while offset < content.len() as u64 {
            let page = read_log(&path, Some(offset), Some(4097)).unwrap();
            assert_eq!(page.start, offset);
            assert!(page.end > offset);
            joined.push_str(&page.text);
            offset = page.end;
        }
        assert_eq!(joined, content);
        let partial = read_log(&path, Some(2), Some(8)).unwrap();
        assert!(content.is_char_boundary(partial.start as usize));
        assert!(content.is_char_boundary(partial.end as usize));
        let beyond = read_log(&path, Some(u64::MAX), None).unwrap();
        assert!(beyond.text.is_empty() && beyond.eof);
    }

    #[test]
    fn log_redacts_secrets_including_page_edges() {
        let dir = crate::test_dir("job-log-secrets");
        let path = dir.path().join("j.log");
        let secret = "private-token";
        std::fs::write(&path, format!("é𐍈 {secret} tail {secret}")).unwrap();
        let spans = |s: &str| {
            s.match_indices(secret)
                .map(|(at, v)| at..at + v.len())
                .collect()
        };
        let redact = |s: &str| s.replace(secret, "{{secret:TOKEN}}");
        let full = read_log_with(&path, None, None, secret.len(), &spans, &redact).unwrap();
        assert_eq!(full.text, "é𐍈 {{secret:TOKEN}} tail {{secret:TOKEN}}");
        for offset in 0..35 {
            let page =
                read_log_with(&path, Some(offset), Some(8), secret.len(), &spans, &redact).unwrap();
            assert!(!page.text.contains("private"));
            assert!(!page.text.contains("token"));
        }
    }
}
