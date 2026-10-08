//! `dreaming_ledger` (design doc 136 §4 phase 1): opens tonight's night on
//! the [`Night`](crate::night::Night) actor, asks `agent::LedgerSince` for
//! the window since `state.last_dream_at` (or 24 h) excluding the dreaming
//! session and its children, asks knowledge `Stats` for the shown ids,
//! folds `ledger/pending/*.json` (rewind snapshots), copies the desk
//! journals into `ledger/<night>/journals/`, adds the deterministic Tidy
//! candidates and the backlog, scrubs everything, writes
//! `ledger/<night>.json` (≤ 2 MB) and returns it plus `run_request`.

use std::path::{Path, PathBuf};

use chrono::{Local, Utc};
use kameo::actor::ActorRef;
use runtime::SessionId;

use crate::config::DreamConfig;
use crate::night::{self, BeginNight, DREAMING_ACTOR, Night, read_state, write_atomic};
use crate::scrub::scrub_value;
use crate::tidy::{self, Candidates};
use crate::types::{Backlog, NightId, RunRequest};

/// `ledger/<night>.json` cap (136 §4 phase 1).
pub const MAX_BYTES: usize = 2 * 1024 * 1024;
/// Concepts listed for the candidate scan.
pub const LIST_LIMIT: u32 = 20_000;

/// What `dreaming_ledger` returns: the agent's ledger plus dreaming's folds.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct DayLedger {
    /// Tonight's night id (the undo handle's prefix).
    pub night: NightId,
    /// `nightly` | `pass` | `backlog`.
    pub mode: String,
    /// The agent's part.
    pub ledger: agent::Ledger,
    /// Rewind snapshots from `ledger/pending/`.
    pub pending_rewinds: Vec<agent::LedgerRewind>,
    /// Desk journal file names copied this night.
    pub desk_journals: Vec<String>,
    /// The rule-found Tidy candidates (ids), capped by `dreaming.caps`.
    pub tidy: Candidates,
    /// Fresh / untidied / open cases waiting.
    pub backlog: Backlog,
    /// Sessions in the window (`min_sessions` decides a quiet night).
    pub sessions: u64,
    /// `dreaming.min_sessions` (R-40): below it the night is Recall + Journal.
    #[serde(default)]
    pub min_sessions: u32,
    /// A `run_now` waiting for this run.
    pub run_request: Option<RunRequest>,
}

/// `<data_dir>/dreaming/ledger/`.
pub fn dir(data_dir: &Path) -> PathBuf {
    night::dir(data_dir).join("ledger")
}

/// `<data_dir>/dreaming/ledger/pending/`.
pub fn pending_dir(data_dir: &Path) -> PathBuf {
    dir(data_dir).join("pending")
}

/// `<data_dir>/dreaming/ledger/<night>.json`.
pub fn path(data_dir: &Path, night: &str) -> PathBuf {
    dir(data_dir).join(format!("{night}.json"))
}

/// Tonight's id: the local date.
pub fn tonight() -> NightId {
    Local::now().format("%Y-%m-%d").to_string()
}

/// The pending rewind snapshots, oldest first.
pub fn pending(data_dir: &Path) -> Vec<(PathBuf, agent::LedgerRewind)> {
    let Ok(rd) = std::fs::read_dir(pending_dir(data_dir)) else {
        return Vec::new();
    };
    let mut v: Vec<(PathBuf, agent::LedgerRewind)> = rd
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .filter_map(|p| {
            let r = serde_json::from_str(&std::fs::read_to_string(&p).ok()?).ok()?;
            Some((p, r))
        })
        .collect();
    v.sort_by(|a, b| a.0.cmp(&b.0));
    v
}

/// Copy `<data_dir>/roster/journal/*.md` into `ledger/<night>/journals/`,
/// scrubbed (R-36); the file names copied.
pub fn copy_desk_journals(data_dir: &Path, night: &str) -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(data_dir.join(roster::desk::JOURNAL_DIR)) else {
        return Vec::new();
    };
    let dest = dir(data_dir).join(night).join("journals");
    let mut names = Vec::new();
    for e in rd.filter_map(Result::ok) {
        let p = e.path();
        if p.extension().is_none_or(|x| x != "md") {
            continue;
        }
        let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if let Ok(text) = std::fs::read_to_string(&p)
            && write_atomic(&dest.join(name), crate::scrub::scrub(&text).as_bytes()).is_ok()
        {
            names.push(name.to_owned());
        }
    }
    names.sort();
    names
}

/// The window start: `since`, else `state.last_dream_at`, else 24 h ago.
pub fn window_start(data_dir: &Path, since: Option<&str>) -> String {
    since
        .map(str::to_owned)
        .or_else(|| read_state(data_dir).last_dream_at)
        .unwrap_or_else(|| {
            (Utc::now() - chrono::Duration::hours(24))
                .format("%Y-%m-%dT%H:%M:%SZ")
                .to_string()
        })
}

/// The deterministic Tidy candidates and the backlog from the live store.
pub async fn candidates(
    data_dir: &Path,
    cfg: &DreamConfig,
    knowledge: Option<&knowledge::Knowledge>,
) -> (Candidates, Backlog) {
    let mut cand = Candidates::default();
    if let Some(k) = knowledge {
        match k.list(None, None, None, LIST_LIMIT).await {
            Ok(metas) => {
                let ids: Vec<String> = metas
                    .iter()
                    .filter(|m| m.status.is_live())
                    .map(|m| m.id.clone())
                    .collect();
                // `stats` answers a zero row for every id; those are "no counter yet"
                // and take the session-derived counts (a pre-136 store fills `uses`
                // one hinted id at a time — the way design §2.3 measured them).
                let counted: Vec<_> = k
                    .stats(&ids)
                    .await
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|u| u.shown > 0 || u.opened > 0)
                    .collect();
                let uses = crate::dry_run::merge_uses(
                    counted,
                    crate::dry_run::uses_from_sessions(data_dir).await,
                );
                cand = tidy::candidates(&metas, &uses, cfg, Utc::now());
            }
            Err(e) => tracing::warn!("dreaming: knowledge list failed: {e:#}"),
        }
    }
    let open_cases = crate::cases::list(
        data_dir,
        Some(crate::types::CaseState::Open),
        None,
        None,
        usize::MAX,
    )
    .map(|v| v.len() as u64)
    .unwrap_or(0)
        + crate::cases::list(
            data_dir,
            Some(crate::types::CaseState::Reopened),
            None,
            None,
            usize::MAX,
        )
        .map(|v| v.len() as u64)
        .unwrap_or(0);
    let backlog = Backlog {
        fresh: cand.fresh.len() as u64,
        untidied: cand.untidied(),
        open_cases,
    };
    (cand, backlog)
}

fn cap<T>(v: &mut Vec<T>, n: usize) {
    v.truncate(n);
}

/// Build tonight's ledger. `session` is the calling (dreaming) session: it
/// and its children are excluded and it owns the night's spend. A second
/// call the same night (a backlog pass) keeps the first call's window and
/// its rewind snapshots (R-43): the pending files were consumed by pass 1.
pub async fn build(
    data_dir: &Path,
    cfg: &DreamConfig,
    knowledge: Option<&knowledge::Knowledge>,
    session: Option<SessionId>,
    since: Option<&str>,
) -> anyhow::Result<DayLedger> {
    let state = read_state(data_dir);
    let mode = state
        .run_request
        .as_ref()
        .map_or("nightly", |r| match r.mode {
            crate::types::RunMode::Pass => "pass",
            crate::types::RunMode::Backlog => "backlog",
        })
        .to_owned();
    let night = tonight();
    // The earlier pass of this night, if any: its window and its rewinds hold.
    let earlier: Option<DayLedger> = std::fs::read_to_string(path(data_dir, &night))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok());
    // The window is read BEFORE `BeginNight` moves `state.last_dream_at` (136 D7).
    let since = match (since, &earlier) {
        (Some(s), _) => s.to_owned(),
        (None, Some(e)) => e.ledger.since.clone(),
        (None, None) => window_start(data_dir, None),
    };
    if let Ok(Some(actor)) = ActorRef::<Night>::lookup(DREAMING_ACTOR) {
        actor
            .ask(BeginNight {
                night: night.clone(),
                mode: mode.clone(),
                session,
            })
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
    }
    let until = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();

    let agents = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR)
        .ok()
        .flatten();
    let mut exclude: Vec<SessionId> = session.into_iter().collect();
    if let (Some(a), Some(s)) = (&agents, session)
        && let Ok(store) = a.ask(agent::DurableStore).await
        && let Ok(kids) = store.descendants(s).await
    {
        exclude.extend(kids.into_iter().map(|(_, k)| k.id));
    }
    let mut ledger = match &agents {
        Some(a) => a
            .ask(agent::LedgerSince {
                since: since.clone(),
                until: until.clone(),
                exclude,
                owner_only: true,
            })
            .await
            .map_err(|e| anyhow::anyhow!("LedgerSince: {e}"))?,
        None => agent::Ledger::empty(&since, &until),
    };
    if let Some(k) = knowledge
        && !ledger.hints.is_empty()
    {
        let ids: Vec<String> = ledger.hints.iter().map(|h| h.id.clone()).collect();
        if let Ok(uses) = k.stats(&ids).await {
            for h in &mut ledger.hints {
                if let Some(u) = uses.iter().find(|u| u.id == h.id) {
                    h.shown = u.shown;
                    h.opened = u.opened;
                }
            }
        }
    }
    let pending = pending(data_dir);
    let mut pending_rewinds: Vec<agent::LedgerRewind> = earlier
        .map(|e| e.pending_rewinds)
        .unwrap_or_default();
    pending_rewinds.extend(pending.iter().map(|(_, r)| r.clone()));
    let desk_journals = copy_desk_journals(data_dir, &night);
    let (mut tidy, backlog) = candidates(data_dir, cfg, knowledge).await;
    cap(&mut tidy.fresh, cfg.caps.triage as usize);
    cap(&mut tidy.retitle, cfg.caps.triage as usize);
    cap(&mut tidy.duplicate_stems, cfg.caps.merges as usize);
    cap(&mut tidy.archive_by_use, cfg.caps.archives as usize);
    let sessions = ledger.sessions.len() as u64;
    let mut day = DayLedger {
        night: night.clone(),
        mode,
        ledger,
        pending_rewinds,
        desk_journals,
        tidy,
        backlog,
        sessions,
        min_sessions: cfg.min_sessions,
        run_request: state.run_request,
    };
    // Scrub, then shrink to the cap (the long lists first).
    let mut value = serde_json::to_value(&day)?;
    scrub_value(&mut value);
    day = serde_json::from_value(value)?;
    let mut bytes = serde_json::to_vec(&day)?;
    while bytes.len() > MAX_BYTES {
        let l = &mut day.ledger;
        if l.user_messages.len() > 100 {
            l.user_messages.truncate(l.user_messages.len() / 2);
        } else if l.tool_errors.len() > 100 {
            l.tool_errors.truncate(l.tool_errors.len() / 2);
        } else if l.app_presses.len() > 50 {
            l.app_presses.truncate(l.app_presses.len() / 2);
        } else if l.sessions.len() > 100 {
            l.sessions.truncate(l.sessions.len() / 2);
        } else {
            day.pending_rewinds.clear();
            l.user_messages.clear();
            l.tool_errors.clear();
            l.truncated = true;
            bytes = serde_json::to_vec(&day)?;
            break;
        }
        l.truncated = true;
        bytes = serde_json::to_vec(&day)?;
    }
    write_atomic(&path(data_dir, &night), &bytes)?;
    // The pending snapshots are folded in; they do not fold in twice.
    for (p, _) in pending {
        let _ = std::fs::remove_file(p);
    }
    Ok(day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn build_without_actors_folds_pending_copies_journals_and_writes_the_night_file() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        let cfg = DreamConfig::default();
        let rewind = agent::LedgerRewind {
            session: SessionId::new(),
            at: "2026-10-07T21:05:00Z".into(),
            dropped_assistant_text: "I ran rm -rf with key sk-abcdefghijklmnop".into(),
        };
        write_atomic(
            &pending_dir(d).join("a.json"),
            serde_json::to_string(&rewind).unwrap().as_bytes(),
        )
        .unwrap();
        write_atomic(
            &d.join(roster::desk::JOURNAL_DIR).join("sre-1.md"),
            b"Done / Open / Next: rotate sk-abcdefghijklmnop",
        )
        .unwrap();
        let day = build(d, &cfg, None, None, Some("2026-10-07T03:00:00Z"))
            .await
            .unwrap();
        assert_eq!(day.night, tonight());
        assert_eq!(day.mode, "nightly");
        assert_eq!(day.ledger.since, "2026-10-07T03:00:00Z");
        assert_eq!(day.pending_rewinds.len(), 1);
        assert!(
            day.pending_rewinds[0]
                .dropped_assistant_text
                .contains("[redacted]")
        );
        assert_eq!(day.desk_journals, ["sre-1.md"]);
        let copied =
            std::fs::read_to_string(dir(d).join(&day.night).join("journals/sre-1.md")).unwrap();
        assert!(
            copied.contains("[redacted]") && !copied.contains("sk-abc"),
            "R-36: {copied}"
        );
        assert!(path(d, &day.night).exists());
        assert!(pending(d).is_empty(), "pending folded in and removed");
        assert_eq!(day.backlog, Backlog::default());
        // Default window: 24 h ago when no state.
        let s = window_start(d, None);
        assert!(s.ends_with('Z'));
        // R-43: the second call of the night (a backlog pass) keeps the first
        // call's window and its rewinds, and folds in anything new.
        write_atomic(
            &pending_dir(d).join("b.json"),
            serde_json::to_string(&agent::LedgerRewind {
                session: SessionId::new(),
                at: "2026-10-07T22:00:00Z".into(),
                dropped_assistant_text: "second".into(),
            })
            .unwrap()
            .as_bytes(),
        )
        .unwrap();
        let again = build(d, &cfg, None, None, None).await.unwrap();
        assert_eq!(again.ledger.since, "2026-10-07T03:00:00Z");
        assert_eq!(again.pending_rewinds.len(), 2, "{:?}", again.pending_rewinds);
        let on_disk: DayLedger =
            serde_json::from_str(&std::fs::read_to_string(path(d, &again.night)).unwrap()).unwrap();
        assert_eq!(on_disk.pending_rewinds.len(), 2);
    }
}
