//! `cases.jsonl` (design doc 136 §6): append-only, one [`Case`] per line;
//! a later line with the same id is that case's newer state (open → closed →
//! reopened → settled), so the file is a log and `all()` folds it. The split
//! is the stable hash `sha256(seed + id) % 100 < 34 → val`; synthetic cases
//! are never val. Every text field is scrubbed; `context` is clipped to
//! 1,200 chars.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::Utc;
use sha2::{Digest as _, Sha256};

use crate::scrub::{clip, scrub};
use crate::types::{Case, CaseOrigin, CaseState, Split};

/// The split seed (stable across nights and machines).
pub const SPLIT_SEED: &str = "136";
/// Val share in percent (136 §6).
pub const VAL_PERCENT: u64 = 34;
/// `context` cap (136 §6).
pub const CONTEXT_CHARS: usize = 1200;
/// Days without a reopen before a case is `settled` (136 §6).
pub const SETTLE_DAYS: i64 = 180;

/// `<data_dir>/dreaming/cases.jsonl`.
pub fn path(data_dir: &Path) -> PathBuf {
    crate::night::dir(data_dir).join("cases.jsonl")
}

/// A fresh time-ordered id (uuid v7 hex; the workspace has no ulid crate).
pub fn new_id() -> String {
    uuid::Uuid::now_v7().simple().to_string()
}

/// RFC 3339 seconds, UTC.
pub fn now_rfc3339() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// The stable split of `id` (136 §6): `sha256(seed + id) % 100 < 34 → val`;
/// synthetic never val.
pub fn split_for(id: &str, origin: CaseOrigin) -> Split {
    if origin == CaseOrigin::Synthetic {
        return Split::Train;
    }
    let digest = Sha256::digest(format!("{SPLIT_SEED}{id}").as_bytes());
    let n = u64::from_be_bytes(digest[..8].try_into().expect("8 bytes"));
    if n % 100 < VAL_PERCENT {
        Split::Val
    } else {
        Split::Train
    }
}

/// Append one line. Empty `id` / `night` / `at` are filled, the split is
/// recomputed from the id, text fields are scrubbed and `context` clipped.
/// Returns the stored record.
pub fn append(data_dir: &Path, case: &Case, night: &str) -> anyhow::Result<Case> {
    let mut c = case.clone();
    if c.id.trim().is_empty() {
        c.id = new_id();
    }
    if c.night.trim().is_empty() {
        c.night = night.to_owned();
    }
    if c.at.trim().is_empty() {
        c.at = now_rfc3339();
    }
    c.split = split_for(&c.id, c.origin);
    c.intent = scrub(&c.intent);
    c.context = clip(&scrub(&c.context), CONTEXT_CHARS);
    c.happened = scrub(&c.happened);
    c.user_words = scrub(&c.user_words);
    c.ideal = scrub(&c.ideal);
    let p = path(data_dir);
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    let mut line = serde_json::to_string(&c)?;
    line.push('\n');
    use std::io::Write as _;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&p)?
        .write_all(line.as_bytes())?;
    Ok(c)
}

/// Every case, folded by id (the last line wins), newest first.
pub fn all(data_dir: &Path) -> anyhow::Result<Vec<Case>> {
    let text = match std::fs::read_to_string(path(data_dir)) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut by_id: BTreeMap<String, Case> = BTreeMap::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        match serde_json::from_str::<Case>(line) {
            Ok(c) => {
                by_id.insert(c.id.clone(), c);
            }
            Err(e) => tracing::warn!("dreaming: cases.jsonl line skipped: {e}"),
        }
    }
    let mut v: Vec<Case> = by_id.into_values().collect();
    v.sort_by(|a, b| b.at.cmp(&a.at).then_with(|| b.id.cmp(&a.id)));
    Ok(v)
}

/// One case by id.
pub fn get(data_dir: &Path, id: &str) -> anyhow::Result<Case> {
    all(data_dir)?
        .into_iter()
        .find(|c| c.id == id)
        .ok_or_else(|| anyhow::anyhow!("no case {id}"))
}

/// Read, filtered, newest first, ≤ `limit`.
pub fn list(
    data_dir: &Path,
    state: Option<CaseState>,
    skill_hint: Option<&str>,
    split: Option<Split>,
    limit: usize,
) -> anyhow::Result<Vec<Case>> {
    Ok(all(data_dir)?
        .into_iter()
        .filter(|c| state.is_none_or(|s| c.state == s))
        .filter(|c| skill_hint.is_none_or(|h| c.skill_hint == h))
        .filter(|c| split.is_none_or(|s| c.split == s))
        .take(limit)
        .collect())
}

/// Move a case to `state` (136 §6, §7.2 step 5): `closed` records
/// `closed_by`, `reopened` stamps `reopened_at`, `settled` leaves the pool.
/// Legal moves (R-38): `open | reopened → closed`, `closed → reopened`,
/// anything → `settled`. Appends the new line, bumps the learned bullets'
/// counters (136 D10, [`crate::edits::count_case`]); returns the line.
pub fn transition(
    data_dir: &Path,
    id: &str,
    state: CaseState,
    closed_by: Option<&str>,
) -> anyhow::Result<Case> {
    let mut c = get(data_dir, id)?;
    anyhow::ensure!(
        matches!(
            (c.state, state),
            (CaseState::Open | CaseState::Reopened, CaseState::Closed)
                | (CaseState::Closed, CaseState::Reopened)
                | (_, CaseState::Settled)
        ),
        "case {id}: {:?} → {:?} is not a transition (open|reopened → closed, closed → reopened, any → settled)",
        c.state,
        state
    );
    match state {
        CaseState::Closed => c.closed_by = closed_by.map(str::to_owned).or(c.closed_by),
        CaseState::Reopened => c.reopened_at = Some(now_rfc3339()),
        CaseState::Open | CaseState::Settled => {}
    }
    c.state = state;
    let c = append(data_dir, &c, &c.night.clone())?;
    crate::edits::count_case(data_dir, &c.id, state);
    Ok(c)
}

/// Cases older than [`SETTLE_DAYS`] with no reopen become `settled`
/// (136 §6); returns how many.
pub fn settle_old(data_dir: &Path, now: chrono::DateTime<Utc>) -> anyhow::Result<usize> {
    let cutoff = now - chrono::Duration::days(SETTLE_DAYS);
    let mut n = 0;
    for c in all(data_dir)? {
        let old = chrono::DateTime::parse_from_rfc3339(&c.at)
            .map(|t| t.with_timezone(&Utc) < cutoff)
            .unwrap_or(false);
        if old && c.reopened_at.is_none() && c.state != CaseState::Settled {
            transition(data_dir, &c.id, CaseState::Settled, None)?;
            n += 1;
        }
    }
    Ok(n)
}

#[cfg(test)]
pub(crate) fn sample(id: &str, skill_hint: &str) -> Case {
    Case {
        id: id.into(),
        night: String::new(),
        kind: crate::types::CaseKind::Correction,
        intent: "ask a fork without ask_user".into(),
        context: "user: pick one\nassistant: calling ask_user".into(),
        happened: "called ask_user with 4 forks".into(),
        user_words: "never use ask_user with me, numbered text".into(),
        ideal: "the same forks as a numbered list in the reply".into(),
        tags: vec!["asking".into()],
        skill_hint: skill_hint.into(),
        source_session: runtime::SessionId::new(),
        at: String::new(),
        split: Split::Train,
        origin: CaseOrigin::Real,
        state: CaseState::Open,
        closed_by: None,
        reopened_at: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_is_stable_near_34_percent_and_never_val_for_synthetic() {
        let val = (0..1000)
            .filter(|i| split_for(&format!("case-{i}"), CaseOrigin::Real) == Split::Val)
            .count();
        assert!((280..=400).contains(&val), "{val}");
        assert_eq!(
            split_for("case-7", CaseOrigin::Real),
            split_for("case-7", CaseOrigin::Real)
        );
        assert!(
            (0..1000)
                .all(|i| split_for(&format!("case-{i}"), CaseOrigin::Synthetic) == Split::Train)
        );
    }

    #[test]
    fn append_fills_scrubs_clips_and_transitions_fold() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = sample("", "harness");
        c.user_words = "never paste sk-abcdefghijklmnop or {{secret:TOKEN}} again".into();
        c.context = "x".repeat(2000);
        let stored = append(dir.path(), &c, "2026-10-08").unwrap();
        assert_eq!(stored.id.len(), 32);
        assert_eq!(stored.night, "2026-10-08");
        assert!(!stored.at.is_empty());
        assert!(!stored.user_words.contains("sk-abc") && stored.user_words.contains("[secret]"));
        assert_eq!(stored.context.chars().count(), CONTEXT_CHARS);
        let second = append(
            dir.path(),
            &sample("", "prompt:standing-preferences"),
            "2026-10-08",
        )
        .unwrap();
        assert_eq!(all(dir.path()).unwrap().len(), 2);
        // Close, then reopen: the log has four lines, the fold has two cases.
        let closed =
            transition(dir.path(), &stored.id, CaseState::Closed, Some("edit:e1")).unwrap();
        assert_eq!(closed.closed_by.as_deref(), Some("edit:e1"));
        let re = transition(dir.path(), &stored.id, CaseState::Reopened, None).unwrap();
        assert!(re.reopened_at.is_some() && re.closed_by.as_deref() == Some("edit:e1"));
        assert_eq!(
            std::fs::read_to_string(path(dir.path()))
                .unwrap()
                .lines()
                .count(),
            4
        );
        let folded = all(dir.path()).unwrap();
        assert_eq!(folded.len(), 2);
        assert_eq!(
            list(dir.path(), Some(CaseState::Reopened), None, None, 10)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            list(
                dir.path(),
                None,
                Some("prompt:standing-preferences"),
                None,
                10
            )
            .unwrap()[0]
                .id,
            second.id
        );
        assert_eq!(list(dir.path(), None, None, None, 1).unwrap().len(), 1);
        // Settle: a case dated 200 days ago without a reopen leaves the pool.
        let mut old = sample("old-1", "harness");
        old.at = (Utc::now() - chrono::Duration::days(200))
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string();
        append(dir.path(), &old, "2026-03-01").unwrap();
        assert_eq!(settle_old(dir.path(), Utc::now()).unwrap(), 1);
        assert_eq!(get(dir.path(), "old-1").unwrap().state, CaseState::Settled);
        // The reopened one is not settled (it has a reopen).
        assert_eq!(settle_old(dir.path(), Utc::now()).unwrap(), 0);
        // R-38: no state machine shortcuts — open → reopened, closed → closed, settled → closed.
        assert!(transition(dir.path(), &second.id, CaseState::Reopened, None).is_err());
        transition(dir.path(), &second.id, CaseState::Closed, None).unwrap();
        assert!(transition(dir.path(), &second.id, CaseState::Closed, None).is_err());
        assert!(transition(dir.path(), "old-1", CaseState::Closed, None).is_err());
        assert_eq!(
            get(dir.path(), &second.id).unwrap().state,
            CaseState::Closed
        );
    }
}
