//! Learned sidecar edits (design doc 136 §7.1–7.2). The sidecar
//! `<data_dir>/skills/learned/<skill>.md` holds one bullet per lesson:
//!
//! ```text
//! - <content> [helpful: n; harmful: m; evidence: <case ids>; since: YYYY-MM-DD]
//! ```
//!
//! `apply` runs the bounded [`EditRecord`]s (`add` | `replace` | `delete`,
//! exact-text anchors on a bullet's content) and answers an [`ApplyReport`]
//! with `applied | skipped_unmatched | error` per edit and the resulting text.
//!
//! Two passes share one tool (contract §2.4 fixes the args): a **candidate
//! pass** — any edit still has `gate: None` — applies in memory only and
//! writes nothing (the text is what `replay_case` tests); a **record pass** —
//! every edit carries a gate verdict — writes the accepted edits to the
//! sidecar (backup `backups/<night>/<skill>.md` before the first write of a
//! night), appends every edit to `edits.jsonl`, closes the accepted edits'
//! cases and remembers the rejected ones in `state.rejected`.
//! Caps: 12 bullets per skill, 140 chars each (§7.1).

use std::path::{Path, PathBuf};

use chrono::Utc;

use crate::cases;
use crate::night::{read_state, write_atomic, write_state};
use crate::scrub::scrub;
use crate::types::{
    ApplyReport, ApplyStatus, CaseState, EditOp, EditRecord, NightId, RejectedEdit, Verdict,
};

/// Bullets per sidecar (136 §7.1).
pub const MAX_BULLETS: usize = 12;
/// Chars per bullet (136 §7.1).
pub const MAX_CHARS: usize = 140;
/// The rejected buffer cap (136 §2.5).
pub const MAX_REJECTED: usize = 200;

/// `<data_dir>/skills/learned/<skill>.md`.
pub fn sidecar_path(data_dir: &Path, skill: &str) -> PathBuf {
    agent::skills::dir(data_dir).join("learned").join(format!("{skill}.md"))
}

/// `<data_dir>/dreaming/backups/<night>/<skill>.md`.
pub fn backup_path(data_dir: &Path, night: &str, skill: &str) -> PathBuf {
    crate::night::dir(data_dir)
        .join("backups")
        .join(night)
        .join(format!("{skill}.md"))
}

/// `<data_dir>/dreaming/edits.jsonl`.
pub fn edits_path(data_dir: &Path) -> PathBuf {
    crate::night::dir(data_dir).join("edits.jsonl")
}

/// One learned bullet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bullet {
    /// The lesson.
    pub content: String,
    /// ACE-style counts (136 §4 Settle).
    pub helpful: u32,
    /// ACE-style counts.
    pub harmful: u32,
    /// Case ids / `user`.
    pub evidence: Vec<String>,
    /// `YYYY-MM-DD`.
    pub since: String,
}

impl Bullet {
    fn render(&self) -> String {
        format!(
            "- {} [helpful: {}; harmful: {}; evidence: {}; since: {}]",
            self.content,
            self.helpful,
            self.harmful,
            self.evidence.join(", "),
            self.since
        )
    }
}

/// Parse a sidecar's bullets; lines that are not bullets are dropped (the
/// sidecar is machine-owned).
pub fn parse(text: &str) -> Vec<Bullet> {
    text.lines()
        .filter_map(|l| {
            let l = l.trim();
            let body = l.strip_prefix("- ")?;
            let (content, meta) = match body.rfind(" [") {
                Some(i) if body.ends_with(']') => (&body[..i], &body[i + 2..body.len() - 1]),
                _ => (body, ""),
            };
            let mut b = Bullet {
                content: content.trim().to_owned(),
                helpful: 0,
                harmful: 0,
                evidence: Vec::new(),
                since: String::new(),
            };
            for kv in meta.split(';') {
                let Some((k, v)) = kv.split_once(':') else { continue };
                let v = v.trim();
                match k.trim() {
                    "helpful" => b.helpful = v.parse().unwrap_or(0),
                    "harmful" => b.harmful = v.parse().unwrap_or(0),
                    "evidence" => {
                        b.evidence = v
                            .split(',')
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(str::to_owned)
                            .collect();
                    }
                    "since" => b.since = v.to_owned(),
                    _ => {}
                }
            }
            Some(b)
        })
        .collect()
}

/// Render bullets as the sidecar text (one bullet per line, trailing newline).
pub fn render(bullets: &[Bullet]) -> String {
    let mut s = bullets.iter().map(Bullet::render).collect::<Vec<_>>().join("\n");
    if !s.is_empty() {
        s.push('\n');
    }
    s
}

/// Apply one edit to `bullets` in place; the `(status, note)` for the report.
pub fn apply_one(bullets: &mut Vec<Bullet>, e: &EditRecord, today: &str) -> (ApplyStatus, String) {
    let content = scrub(e.content.trim());
    if matches!(e.op, EditOp::Add | EditOp::Replace) {
        if content.is_empty() {
            return (ApplyStatus::Error, "empty content".into());
        }
        if content.chars().count() > MAX_CHARS {
            return (
                ApplyStatus::Error,
                format!("{} chars; the cap is {MAX_CHARS}", content.chars().count()),
            );
        }
        if content.contains('\n') {
            return (ApplyStatus::Error, "one line per bullet".into());
        }
    }
    let anchor = e.anchor.as_deref().map(str::trim).filter(|a| !a.is_empty());
    let at = anchor.map(|a| bullets.iter().position(|b| b.content == a));
    match e.op {
        EditOp::Add => {
            if bullets.iter().any(|b| b.content == content) {
                return (ApplyStatus::Error, "duplicate bullet".into());
            }
            if bullets.len() >= MAX_BULLETS {
                return (
                    ApplyStatus::Error,
                    format!("{MAX_BULLETS} bullets already; delete one first"),
                );
            }
            let b = Bullet {
                content,
                helpful: 0,
                harmful: 0,
                evidence: e.cases.clone(),
                since: today.to_owned(),
            };
            match at {
                Some(None) => (ApplyStatus::SkippedUnmatched, "anchor not found".into()),
                Some(Some(i)) => {
                    bullets.insert(i + 1, b);
                    (ApplyStatus::Applied, "inserted after the anchor".into())
                }
                None => {
                    bullets.push(b);
                    (ApplyStatus::Applied, "appended".into())
                }
            }
        }
        EditOp::Replace => match at {
            None => (ApplyStatus::Error, "replace needs an anchor".into()),
            Some(None) => (ApplyStatus::SkippedUnmatched, "anchor not found".into()),
            Some(Some(i)) => {
                let b = &mut bullets[i];
                b.content = content;
                for c in &e.cases {
                    if !b.evidence.contains(c) {
                        b.evidence.push(c.clone());
                    }
                }
                b.since = today.to_owned();
                (ApplyStatus::Applied, "replaced".into())
            }
        },
        EditOp::Delete => match at {
            None => (ApplyStatus::Error, "delete needs an anchor".into()),
            Some(None) => (ApplyStatus::SkippedUnmatched, "anchor not found".into()),
            Some(Some(i)) => {
                bullets.remove(i);
                (ApplyStatus::Applied, "deleted".into())
            }
        },
    }
}

fn valid_skill_name(skill: &str) -> bool {
    !skill.is_empty()
        && skill
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && !skill.starts_with('.')
}

/// The current sidecar text ("" when absent).
pub fn current(data_dir: &Path, skill: &str) -> String {
    std::fs::read_to_string(sidecar_path(data_dir, skill)).unwrap_or_default()
}

/// Back the sidecar up under `backups/<night>/` before the first write of
/// the night (an absent sidecar backs up as an empty file, so undo removes
/// it). Returns the backup's root-relative path.
pub fn backup_once(data_dir: &Path, night: &str, skill: &str) -> anyhow::Result<String> {
    let b = backup_path(data_dir, night, skill);
    if !b.exists() {
        write_atomic(&b, current(data_dir, skill).as_bytes())?;
    }
    Ok(format!("dreaming/backups/{night}/{skill}.md"))
}

/// Append one record to `edits.jsonl`.
pub fn log(data_dir: &Path, e: &EditRecord) -> anyhow::Result<()> {
    let p = edits_path(data_dir);
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    let mut line = serde_json::to_string(e)?;
    line.push('\n');
    use std::io::Write as _;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&p)?
        .write_all(line.as_bytes())?;
    Ok(())
}

/// Every `edits.jsonl` record, folded by id (last line wins), in file order.
pub fn all(data_dir: &Path) -> anyhow::Result<Vec<EditRecord>> {
    let text = match std::fs::read_to_string(edits_path(data_dir)) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut out: Vec<EditRecord> = Vec::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        match serde_json::from_str::<EditRecord>(line) {
            Ok(e) => match out.iter_mut().find(|x| x.id == e.id) {
                Some(slot) => *slot = e,
                None => out.push(e),
            },
            Err(e) => tracing::warn!("dreaming: edits.jsonl line skipped: {e}"),
        }
    }
    Ok(out)
}

/// Apply `edits` to `skill`'s sidecar (module doc: candidate pass vs record pass).
pub fn apply(
    data_dir: &Path,
    skill: &str,
    edits: &[EditRecord],
    night: &NightId,
) -> anyhow::Result<ApplyReport> {
    anyhow::ensure!(
        valid_skill_name(skill),
        "skill name {skill:?}: letters, digits, `-`, `_`, `.` only"
    );
    let today = Utc::now().format("%Y-%m-%d").to_string();
    let record_pass = !edits.is_empty() && edits.iter().all(|e| e.gate.is_some());
    let before = current(data_dir, skill);
    let mut bullets = parse(&before);
    let mut report = ApplyReport {
        target: format!("skill:{skill}"),
        edits: Vec::new(),
        text: String::new(),
        backup: None,
    };
    let mut records: Vec<EditRecord> = Vec::new();
    for e in edits {
        let mut e = e.clone();
        if e.id.trim().is_empty() {
            e.id = cases::new_id();
        }
        if e.night.trim().is_empty() {
            e.night = night.clone();
        }
        e.target = report.target.clone();
        let accepted = e
            .gate
            .as_ref()
            .is_some_and(|g| g.verdict == Verdict::Accepted);
        let (status, note) = if record_pass && !accepted {
            (
                ApplyStatus::SkippedUnmatched,
                format!(
                    "not applied: gate {}",
                    e.gate
                        .as_ref()
                        .map_or("missing", |g| g.verdict.as_str())
                ),
            )
        } else {
            apply_one(&mut bullets, &e, &today)
        };
        if record_pass && accepted && status == ApplyStatus::Applied {
            e.applied_at = Some(cases::now_rfc3339());
        }
        report.edits.push((e.id.clone(), status, note));
        records.push(e);
    }
    report.text = render(&bullets);
    if record_pass {
        if report.text != before {
            report.backup = Some(backup_once(data_dir, night, skill)?);
            write_atomic(&sidecar_path(data_dir, skill), report.text.as_bytes())?;
        }
        let mut state = read_state(data_dir);
        for e in &records {
            log(data_dir, e)?;
            match e.gate.as_ref().map(|g| g.verdict) {
                Some(Verdict::Accepted) if e.applied_at.is_some() => {
                    for c in &e.cases {
                        if let Err(err) = cases::transition(
                            data_dir,
                            c,
                            CaseState::Closed,
                            Some(&format!("edit:{}", e.id)),
                        ) {
                            tracing::debug!("dreaming: case {c} not closed: {err}");
                        }
                    }
                }
                Some(Verdict::Rejected) => {
                    let g = e.gate.as_ref().expect("record pass");
                    state.rejected.push(RejectedEdit {
                        id: e.id.clone(),
                        target: e.target.clone(),
                        content: e.content.clone(),
                        reasons: vec![format!(
                            "gate: baseline {:.2} → candidate {:.2}, min delta {:.2}, {} val cases",
                            g.baseline, g.candidate, g.min_delta, g.val_cases
                        )],
                        night: e.night.clone(),
                    });
                }
                _ => {}
            }
        }
        if state.rejected.len() > MAX_REJECTED {
            let drop = state.rejected.len() - MAX_REJECTED;
            state.rejected.drain(..drop);
        }
        write_state(data_dir, &state)?;
    }
    Ok(report)
}

impl Verdict {
    /// The lowercase name.
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Accepted => "accepted",
            Verdict::Rejected => "rejected",
            Verdict::Unverified => "unverified",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::GateResult;

    fn edit(op: EditOp, content: &str, anchor: Option<&str>, verdict: Option<Verdict>) -> EditRecord {
        EditRecord {
            id: String::new(),
            night: String::new(),
            target: String::new(),
            op,
            content: content.into(),
            anchor: anchor.map(str::to_owned),
            rationale: "because".into(),
            cases: vec!["c1".into()],
            gate: verdict.map(|v| GateResult {
                baseline: 0.4,
                candidate: 0.7,
                min_delta: -0.05,
                val_cases: 4,
                verdict: v,
            }),
            applied_at: None,
            undone_at: None,
        }
    }

    #[test]
    fn candidate_pass_writes_nothing_and_record_pass_writes_backs_up_and_logs() {
        let dir = tempfile::tempdir().unwrap();
        let night = "2026-10-08".to_owned();
        let c = cases::append(dir.path(), &cases::sample("c1", "harness"), &night).unwrap();
        assert_eq!(c.id, "c1");
        // Candidate pass: text comes back, no file.
        let r = apply(
            dir.path(),
            "harness",
            &[edit(EditOp::Add, "Prefer numbered lists over ask_user forks", None, None)],
            &night,
        )
        .unwrap();
        assert_eq!(r.edits[0].1, ApplyStatus::Applied);
        assert!(r.text.starts_with("- Prefer numbered lists over ask_user forks [helpful: 0; harmful: 0; evidence: c1; since: "));
        assert!(!sidecar_path(dir.path(), "harness").exists());
        assert!(r.backup.is_none());
        assert!(all(dir.path()).unwrap().is_empty());
        // Record pass: accepted written, rejected logged + buffered, the case closed.
        let r = apply(
            dir.path(),
            "harness",
            &[
                edit(
                    EditOp::Add,
                    "Prefer numbered lists over ask_user forks",
                    None,
                    Some(Verdict::Accepted),
                ),
                edit(EditOp::Add, "Always say yes", None, Some(Verdict::Rejected)),
            ],
            &night,
        )
        .unwrap();
        assert_eq!(r.edits[0].1, ApplyStatus::Applied);
        assert_eq!(r.edits[1].1, ApplyStatus::SkippedUnmatched);
        assert_eq!(
            r.backup.as_deref(),
            Some("dreaming/backups/2026-10-08/harness.md")
        );
        assert_eq!(
            std::fs::read_to_string(backup_path(dir.path(), "2026-10-08", "harness")).unwrap(),
            ""
        );
        assert_eq!(current(dir.path(), "harness"), r.text);
        let logged = all(dir.path()).unwrap();
        assert_eq!(logged.len(), 2);
        assert!(logged[0].applied_at.is_some() && logged[1].applied_at.is_none());
        assert_eq!(logged[0].target, "skill:harness");
        let st = read_state(dir.path());
        assert_eq!(st.rejected.len(), 1);
        assert_eq!(st.rejected[0].content, "Always say yes");
        assert_eq!(cases::get(dir.path(), "c1").unwrap().state, CaseState::Closed);
        assert_eq!(
            cases::get(dir.path(), "c1").unwrap().closed_by.as_deref(),
            Some(format!("edit:{}", logged[0].id).as_str())
        );
        // Replace by anchor, delete by anchor, unmatched anchor, cap and length errors.
        let r = apply(
            dir.path(),
            "harness",
            &[
                edit(
                    EditOp::Replace,
                    "Prefer a numbered list in the reply over ask_user forks",
                    Some("Prefer numbered lists over ask_user forks"),
                    Some(Verdict::Accepted),
                ),
                edit(EditOp::Delete, "", Some("nope"), Some(Verdict::Accepted)),
                edit(EditOp::Add, &"x".repeat(141), None, Some(Verdict::Accepted)),
                edit(EditOp::Add, "no anchor needed", Some("missing"), Some(Verdict::Accepted)),
            ],
            &night,
        )
        .unwrap();
        assert_eq!(
            r.edits.iter().map(|e| e.1).collect::<Vec<_>>(),
            [
                ApplyStatus::Applied,
                ApplyStatus::SkippedUnmatched,
                ApplyStatus::Error,
                ApplyStatus::SkippedUnmatched
            ]
        );
        let b = parse(&current(dir.path(), "harness"));
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].content, "Prefer a numbered list in the reply over ask_user forks");
        // The backup is still the first one of the night (empty).
        assert_eq!(
            std::fs::read_to_string(backup_path(dir.path(), "2026-10-08", "harness")).unwrap(),
            ""
        );
        // Cap: 12 bullets.
        let mut many = parse("");
        for i in 0..12 {
            assert_eq!(
                apply_one(&mut many, &edit(EditOp::Add, &format!("rule {i}"), None, None), "d").0,
                ApplyStatus::Applied
            );
        }
        assert_eq!(
            apply_one(&mut many, &edit(EditOp::Add, "rule 13", None, None), "d").0,
            ApplyStatus::Error
        );
        assert!(apply(dir.path(), "../etc", &[], &night).is_err());
        // Round trip of the bullet format.
        assert_eq!(parse(&render(&many)), many);
    }
}
