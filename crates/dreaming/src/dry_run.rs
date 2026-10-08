//! `dreaming::dry_run(data_dir)` (design doc 136 §3.3 item 7): Recall plus
//! the deterministic parts of Tidy over a data dir with **zero writes** — the
//! knowledge files are parsed directly (no store open, which would sync the
//! index), `knowledge.sqlite`'s `uses` and `sessions.sqlite` are read through
//! read-only connections, and nothing under the dir is created or changed
//! (SQLite may leave a `-shm` beside a WAL database it reads; the test allows
//! exactly that). Prints the counts the journal would show.

use std::path::Path;

use chrono::Utc;
use knowledge::{Meta, Status, Uses};
use sqlx::ConnectOptions as _;
use sqlx::sqlite::SqliteConnectOptions;

use crate::config::DreamConfig;
use crate::tidy;
use crate::types::Report;

/// A knowledge file's front matter, the keys the rules read.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
struct Fm {
    #[serde(rename = "type")]
    type_: String,
    title: String,
    description: Option<String>,
    status: Option<Status>,
    timestamp: Option<String>,
    verified: Option<String>,
}

/// The `dreaming:` section of `<data_dir>/config.yaml`, else the defaults.
pub fn config_of(data_dir: &Path) -> DreamConfig {
    std::fs::read_to_string(data_dir.join("config.yaml"))
        .ok()
        .and_then(|t| serde_yaml::from_str::<serde_yaml::Value>(&t).ok())
        .and_then(|v| v.get("dreaming").cloned())
        .and_then(|v| serde_yaml::from_value(v).ok())
        .unwrap_or_default()
}

/// Every concept under `<data_dir>/knowledge/**/*.md` with front matter.
pub fn read_metas(data_dir: &Path) -> Vec<Meta> {
    let root = data_dir.join(knowledge::DIR);
    let mut out = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.filter_map(Result::ok) {
            let p = e.path();
            if p.is_dir() {
                if p.file_name().is_some_and(|n| n != ".git") {
                    stack.push(p);
                }
                continue;
            }
            if p.extension().is_none_or(|x| x != "md") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&p) else {
                continue;
            };
            let Some(rest) = text.strip_prefix("---\n") else {
                continue;
            };
            let Some(end) = rest.find("\n---") else {
                continue;
            };
            let Ok(fm) = serde_yaml::from_str::<Fm>(&rest[..end]) else {
                continue;
            };
            if fm.type_.is_empty() {
                continue;
            }
            let Ok(rel) = p.strip_prefix(&root) else {
                continue;
            };
            let id = rel.with_extension("").to_string_lossy().replace('\\', "/");
            out.push(Meta {
                id,
                type_: fm.type_,
                title: fm.title,
                description: fm.description.unwrap_or_default(),
                status: fm.status.unwrap_or_default(),
                timestamp: fm.timestamp,
                verified: fm.verified,
                ..Default::default()
            });
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

async fn open_ro(path: &Path) -> Option<sqlx::SqliteConnection> {
    if !path.exists() {
        return None;
    }
    SqliteConnectOptions::new()
        .filename(path)
        .read_only(true)
        .connect()
        .await
        .ok()
}

/// The `uses` table of `<data_dir>/knowledge.sqlite`, read-only.
pub async fn read_uses(data_dir: &Path) -> Vec<Uses> {
    let Some(mut c) = open_ro(&data_dir.join(knowledge::INDEX_FILE)).await else {
        return Vec::new();
    };
    sqlx::query_as::<_, (String, i64, i64, Option<String>, Option<String>)>(
        "SELECT id, shown, opened, last_shown, last_opened FROM uses",
    )
    .fetch_all(&mut c)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|(id, shown, opened, last_shown, last_opened)| Uses {
        id,
        shown: shown.max(0) as u64,
        opened: opened.max(0) as u64,
        last_shown,
        last_opened,
    })
    .collect()
}

/// `(sessions, friction events)` in `<data_dir>/sessions.sqlite` since
/// `since`: non-advisor sessions created in the window; tool errors,
/// denied questions and cancelled turns as the friction the miner would see.
pub async fn read_sessions(data_dir: &Path, since: &str) -> (u64, u64) {
    let Some(mut c) = open_ro(&data_dir.join("sessions.sqlite")).await else {
        return (0, 0);
    };
    async fn count(c: &mut sqlx::SqliteConnection, sql: &'static str, since: &str) -> u64 {
        sqlx::query_scalar::<_, i64>(sql)
            .bind(since)
            .fetch_one(c)
            .await
            .unwrap_or(0)
            .max(0) as u64
    }
    let sessions = count(
        &mut c,
        "SELECT count(*) FROM sessions WHERE created_at >= ?1 AND COALESCE(kind, 'chat') != 'advisor'",
        since,
    )
    .await;
    let errors = count(
        &mut c,
        "SELECT count(*) FROM tool_calls WHERE is_error = 1 AND COALESCE(finished_at, started_at, '') >= ?1",
        since,
    )
    .await;
    let denials = count(
        &mut c,
        "SELECT count(*) FROM questions WHERE asked_at >= ?1 AND state = 'answered' AND answer LIKE '%deny%'",
        since,
    )
    .await;
    let stops = count(
        &mut c,
        "SELECT count(*) FROM turn_steps WHERE at >= ?1 AND kind = 'turn_finished' AND payload LIKE '%cancelled%'",
        since,
    )
    .await;
    (sessions, errors + denials + stops)
}

/// Recall + rule-only Tidy, zero writes.
pub async fn dry_run(data_dir: &Path) -> anyhow::Result<Report> {
    let cfg = config_of(data_dir);
    let metas = read_metas(data_dir);
    let uses = read_uses(data_dir).await;
    let c = tidy::candidates(&metas, &uses, &cfg, Utc::now());
    let since = crate::ledger::window_start(data_dir, None);
    let (sessions, friction_events) = read_sessions(data_dir, &since).await;
    Ok(Report {
        fresh: c.fresh.len() as u64,
        retitle_candidates: c.retitle.len() as u64,
        duplicate_stems: c.duplicate_stems.len() as u64,
        archive_by_use_candidates: c.archive_by_use.len() as u64,
        verification_sample: c.verify_sample.len() as u64,
        sessions,
        friction_events,
    })
}

/// The report as the CLI prints it.
pub fn render(r: &Report) -> String {
    format!(
        "dreaming dry run\n  sessions in window      {}\n  friction events         {}\n  fresh entries           {}\n  retitle candidates      {}\n  duplicate stems         {}\n  archive-by-use          {}\n  verification sample     {}\n",
        r.sessions,
        r.friction_events,
        r.fresh,
        r.retitle_candidates,
        r.duplicate_stems,
        r.archive_by_use_candidates,
        r.verification_sample
    )
}
