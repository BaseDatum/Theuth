//! Learned sidecar edits (design doc 136 §7.1–7.2). The sidecar
//! `<data_dir>/skills/learned/<skill>.md` holds one bullet per lesson:
//!
//! ```text
//! - <content> [helpful: n; harmful: m; evidence: <case ids>; since: YYYY-MM-DD]
//! ```
//!
//! `apply` runs the bounded [`EditRecord`]s (`add` | `replace` | `delete`,
//! anchors on a bullet's content — [`normalise_anchor`] forgives the `- `,
//! a line number and the metadata tail) and answers an [`ApplyReport`] with
//! `applied | skipped_unmatched | error` per edit and the resulting text.
//!
//! Two passes share one tool (contract §2.4 fixes the args): a **candidate
//! pass** applies in memory only and writes nothing (the text is what
//! `replay_case` tests); a **record pass** recomputes the gate from the
//! persisted replays of that candidate text (136 D2, [`crate::replay::gate_from_rows`])
//! — the caller's `gate` is advisory, a mismatch is logged — and REFUSES
//! when no replay was logged or the gate fails; an accepted gate writes the
//! edits to the sidecar (backup `backups/<night>/<skill>.md` before the first
//! write of a night), appends every edit to `edits.jsonl`, closes the edits'
//! cases and remembers a rejected proposal in `state.rejected`. `retire`
//! (136 D4) moves a dreaming-made skill under `skills/retired/`, receipted
//! and undoable. Counters are server-side (136 D10, [`count_case`]).
//! Caps: 12 bullets per skill, 140 chars each (§7.1).

use std::path::{Path, PathBuf};

use chrono::Utc;

use crate::cases;
use crate::config::DreamConfig;
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
    agent::skills::dir(data_dir)
        .join("learned")
        .join(format!("{skill}.md"))
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
                let Some((k, v)) = kv.split_once(':') else {
                    continue;
                };
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
    let mut s = bullets
        .iter()
        .map(Bullet::render)
        .collect::<Vec<_>>()
        .join("\n");
    if !s.is_empty() {
        s.push('\n');
    }
    s
}

/// The anchor as the proposer may quote it — a numbered `read_file` line
/// (`3\t- rule [...]`), a bullet with its `- ` and metadata tail, extra
/// whitespace — reduced to the bullet's content (R-29).
pub fn normalise_anchor(a: &str) -> String {
    let mut s = a.trim();
    if let Some((n, rest)) = s.split_once('\t')
        && !n.is_empty()
        && n.trim().bytes().all(|b| b.is_ascii_digit())
    {
        s = rest.trim();
    }
    if let Some(rest) = s.strip_prefix("- ") {
        s = rest.trim();
    }
    if s.ends_with(']')
        && let Some(i) = s.rfind(" [")
        && s[i + 2..].contains("helpful:")
    {
        s = s[..i].trim();
    }
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn same_text(a: &str, b: &str) -> bool {
    a.split_whitespace().eq(b.split_whitespace())
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
    let anchor = e
        .anchor
        .as_deref()
        .map(normalise_anchor)
        .filter(|a| !a.is_empty());
    let at = anchor
        .as_deref()
        .map(|a| bullets.iter().position(|b| same_text(&b.content, a)));
    match e.op {
        EditOp::Add => {
            if bullets.iter().any(|b| same_text(&b.content, &content)) {
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
        EditOp::Retire => (
            ApplyStatus::Error,
            "retire is a record-pass op on the skill file, not a bullet edit".into(),
        ),
    }
}

/// A learned skill name (136 D15): `[a-z0-9-]{1,64}`.
pub fn valid_skill_name(skill: &str) -> bool {
    (1..=64).contains(&skill.len())
        && skill
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
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

/// Append one record to `edits.jsonl` (scrubbed: the rationale and content
/// are model text, R-36).
pub fn log(data_dir: &Path, e: &EditRecord) -> anyhow::Result<()> {
    let p = edits_path(data_dir);
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    let mut value = serde_json::to_value(e)?;
    crate::scrub::scrub_value(&mut value);
    let mut line = serde_json::to_string(&value)?;
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

/// `<data_dir>/skills/<skill>.md`.
pub fn skill_path(data_dir: &Path, skill: &str) -> PathBuf {
    agent::skills::dir(data_dir).join(format!("{skill}.md"))
}

/// `<data_dir>/skills/retired/<skill>.md`.
pub fn retired_path(data_dir: &Path, skill: &str) -> PathBuf {
    agent::skills::dir(data_dir)
        .join("retired")
        .join(format!("{skill}.md"))
}

/// `<data_dir>/dreaming/backups/<night>/retired/<skill>.md`: the skill file
/// as it was before `retire` moved it (undo moves it back).
pub fn retire_backup_path(data_dir: &Path, night: &str, skill: &str) -> PathBuf {
    crate::night::dir(data_dir)
        .join("backups")
        .join(night)
        .join("retired")
        .join(format!("{skill}.md"))
}

/// Is `skills/<skill>.md` a skill the night made (`origin: dreaming` in its
/// front matter)? Human skills are never retired (136 D4).
pub fn is_dreaming_made(text: &str) -> bool {
    let Some(rest) = text.strip_prefix("---\n") else {
        return false;
    };
    let Some(end) = rest.find("\n---") else {
        return false;
    };
    rest[..end].lines().any(|l| {
        l.split_once(':')
            .is_some_and(|(k, v)| k.trim() == "origin" && v.trim() == "dreaming")
    })
}

/// `retire` (136 D4): move `skills/<skill>.md` (dreaming-made only) to
/// `skills/retired/<skill>.md` with a `retired_because:` front-matter line;
/// the original is kept under `backups/<night>/retired/` for undo.
fn retire(data_dir: &Path, night: &str, skill: &str, because: &str) -> anyhow::Result<String> {
    let src = skill_path(data_dir, skill);
    let text =
        std::fs::read_to_string(&src).map_err(|e| anyhow::anyhow!("skills/{skill}.md: {e}"))?;
    anyhow::ensure!(
        is_dreaming_made(&text),
        "skills/{skill}.md is not a dreaming-made skill (no `origin: dreaming`); never retire a human skill"
    );
    let dst = retired_path(data_dir, skill);
    anyhow::ensure!(!dst.exists(), "skills/retired/{skill}.md already exists");
    let backup = retire_backup_path(data_dir, night, skill);
    if !backup.exists() {
        write_atomic(&backup, text.as_bytes())?;
    }
    let because = scrub(because.trim());
    let because = if because.is_empty() {
        "retired by dreaming".to_owned()
    } else {
        crate::scrub::clip(&because, 200)
    };
    let retired = text.replacen(
        "---\n",
        &format!("---\nretired_because: {because:?}\nretired_on: {night}\n"),
        1,
    );
    write_atomic(&dst, retired.as_bytes())?;
    std::fs::remove_file(&src)?;
    Ok(format!("skills/retired/{skill}.md"))
}

/// Skills retired that night (`backups/<night>/retired/*.md`), sorted.
pub fn retired_that_night(data_dir: &Path, night: &str) -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(
        crate::night::dir(data_dir)
            .join("backups")
            .join(night)
            .join("retired"),
    ) else {
        return Vec::new();
    };
    let mut v: Vec<String> = rd
        .filter_map(Result::ok)
        .filter_map(|e| {
            e.path()
                .file_stem()
                .and_then(|s| s.to_str())
                .map(str::to_owned)
        })
        .collect();
    v.sort();
    v
}

/// Which pass `skill_learned_edit` runs (module doc).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pass {
    /// Apply in memory, write nothing, answer the candidate text.
    Candidate,
    /// Gate from the persisted replays, then write.
    Record,
}

/// Apply `edits` to `skill`'s sidecar (module doc: candidate pass vs record pass).
pub fn apply(
    data_dir: &Path,
    cfg: &DreamConfig,
    skill: &str,
    edits: &[EditRecord],
    night: &NightId,
    pass: Pass,
) -> anyhow::Result<ApplyReport> {
    anyhow::ensure!(
        valid_skill_name(skill),
        "skill name {skill:?}: lowercase letters, digits and `-` only, at most 64"
    );
    anyhow::ensure!(!edits.is_empty(), "no edits");
    let today = Utc::now().format("%Y-%m-%d").to_string();
    let before = current(data_dir, skill);
    let mut bullets = parse(&before);
    let mut report = ApplyReport {
        target: format!("skill:{skill}"),
        edits: Vec::new(),
        text: String::new(),
        backup: None,
    };
    let mut records: Vec<EditRecord> = Vec::new();
    let mut retires: Vec<(usize, EditRecord)> = Vec::new();
    for e in edits {
        let mut e = e.clone();
        if e.id.trim().is_empty() {
            e.id = cases::new_id();
        }
        if e.night.trim().is_empty() {
            e.night = night.clone();
        }
        e.target = report.target.clone();
        if e.op == EditOp::Retire {
            let note = if pass == Pass::Record {
                "pending".to_owned()
            } else {
                "retire needs stage: record".to_owned()
            };
            report.edits.push((e.id.clone(), ApplyStatus::Error, note));
            if pass == Pass::Record {
                retires.push((report.edits.len() - 1, e));
            }
            continue;
        }
        let (status, note) = apply_one(&mut bullets, &e, &today);
        report.edits.push((e.id.clone(), status, note));
        records.push(e);
    }
    report.text = render(&bullets);
    if pass == Pass::Candidate {
        return Ok(report);
    }
    // Retire first: independent of the bullet gate.
    for (i, mut e) in retires {
        match retire(data_dir, night, skill, &e.rationale) {
            Ok(where_) => {
                e.applied_at = Some(cases::now_rfc3339());
                report.edits[i] = (
                    e.id.clone(),
                    ApplyStatus::Applied,
                    format!("moved to {where_}"),
                );
                log(data_dir, &e)?;
            }
            Err(err) => report.edits[i] = (e.id.clone(), ApplyStatus::Error, err.to_string()),
        }
    }
    if records.is_empty() {
        return Ok(report);
    }
    // The gate (136 D2): from tonight's replays of exactly this candidate text.
    let Some(gate) = crate::replay::gate_from_rows(data_dir, night, &report.text, cfg) else {
        anyhow::bail!(
            "no replays for this candidate: run `replay_case` on the cluster's val cases \
             (baseline and candidate_text = this candidate) before the record pass"
        );
    };
    for e in &mut records {
        if let Some(claimed) = e.gate.as_ref()
            && claimed.verdict != gate.verdict
        {
            tracing::info!(
                edit = %e.id,
                claimed = claimed.verdict.as_str(),
                computed = gate.verdict.as_str(),
                "dreaming: the caller's gate differs from the replay rows; the rows win"
            );
        }
        e.gate = Some(gate);
    }
    let accepted = gate.verdict == Verdict::Accepted;
    if accepted && report.text != before {
        report.backup = Some(backup_once(data_dir, night, skill)?);
        write_atomic(&sidecar_path(data_dir, skill), report.text.as_bytes())?;
    }
    let now = cases::now_rfc3339();
    let mut state = read_state(data_dir);
    if gate.verdict == Verdict::Rejected {
        // One buffer row per rejected proposal (136 §0.9), its edits joined.
        state.rejected.push(RejectedEdit {
            id: records[0].id.clone(),
            target: report.target.clone(),
            content: records
                .iter()
                .map(|r| scrub(&r.content))
                .collect::<Vec<_>>()
                .join(" | "),
            reasons: vec![format!(
                "gate: baseline {:.2} → candidate {:.2}, min delta {:.2}, {} val cases",
                gate.baseline, gate.candidate, gate.min_delta, gate.val_cases
            )],
            night: night.clone(),
        });
        if state.rejected.len() > MAX_REJECTED {
            let drop = state.rejected.len() - MAX_REJECTED;
            state.rejected.drain(..drop);
        }
        write_state(data_dir, &state)?;
    }
    for e in &mut records {
        let slot = report
            .edits
            .iter_mut()
            .find(|(id, _, _)| *id == e.id)
            .expect("every record has a report row");
        if slot.1 == ApplyStatus::Applied {
            if accepted {
                e.applied_at = Some(now.clone());
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
            } else {
                slot.1 = ApplyStatus::SkippedUnmatched;
                slot.2 = format!("not applied: gate {}", gate.verdict.as_str());
            }
        }
        log(data_dir, e)?;
    }
    Ok(report)
}

/// Retired-bullet threshold (136 D10).
pub const HARMFUL_RETIRE: u32 = 3;

/// `<data_dir>/skills/retired/learned-<skill>.md`: bullets auto-retired by harm.
pub fn retired_bullets_path(data_dir: &Path, skill: &str) -> PathBuf {
    agent::skills::dir(data_dir)
        .join("retired")
        .join(format!("learned-{skill}.md"))
}

/// 136 D10, the server-side counters: a case `settled` → `helpful += 1`,
/// `reopened` → `harmful += 1` on every learned bullet citing it; a bullet
/// at `harmful ≥ 3` is moved to `skills/retired/learned-<skill>.md`. The
/// sidecar is backed up under tonight before its first write (so
/// `dreaming_undo(night, "skill:<skill>")` restores it) and every retired
/// bullet is journaled. Returns `(skill, bullet)` of the retired bullets.
pub fn count_case(data_dir: &Path, case_id: &str, state: CaseState) -> Vec<(String, String)> {
    let helpful = match state {
        CaseState::Settled => true,
        CaseState::Reopened => false,
        CaseState::Open | CaseState::Closed => return Vec::new(),
    };
    let Ok(rd) = std::fs::read_dir(agent::skills::dir(data_dir).join("learned")) else {
        return Vec::new();
    };
    let night = crate::ledger::tonight();
    let mut retired = Vec::new();
    for entry in rd.filter_map(Result::ok) {
        let path = entry.path();
        let Some(skill) = path
            .file_stem()
            .and_then(|s| s.to_str())
            .filter(|s| path.extension().is_some_and(|x| x == "md") && valid_skill_name(s))
        else {
            continue;
        };
        let before = std::fs::read_to_string(&path).unwrap_or_default();
        let mut bullets = parse(&before);
        let mut touched = false;
        let mut keep = Vec::with_capacity(bullets.len());
        for mut b in bullets.drain(..) {
            if b.evidence.iter().any(|e| e == case_id) {
                touched = true;
                if helpful {
                    b.helpful += 1;
                } else {
                    b.harmful += 1;
                }
                if b.harmful >= HARMFUL_RETIRE {
                    retired.push((skill.to_owned(), b.render()));
                    continue;
                }
            }
            keep.push(b);
        }
        if !touched {
            continue;
        }
        let text = render(&keep);
        let written = backup_once(data_dir, &night, skill)
            .and_then(|_| write_atomic(&path, text.as_bytes()).map_err(anyhow::Error::from));
        if let Err(e) = written {
            tracing::warn!("dreaming: counters of skills/learned/{skill}.md not written: {e}");
        }
    }
    for (skill, line) in &retired {
        let p = retired_bullets_path(data_dir, skill);
        if let Err(e) = crate::night::append_text(&p, &format!("{line}\n")) {
            tracing::warn!("dreaming: retired bullet not written: {e}");
        }
        let md = format!(
            "- retired from `{skill}` (harmful ≥ {HARMFUL_RETIRE}; case `{case_id}` reopened): {line}; undo: `dreaming_undo(\"{night}\", \"skill:{skill}\")`"
        );
        crate::journal::append_sync(data_dir, &night, "Settle", &md);
    }
    retired
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
    use crate::replay::{ReplayRow, candidate_sha256, log_row};
    use crate::types::GateResult;

    fn edit(
        op: EditOp,
        content: &str,
        anchor: Option<&str>,
        verdict: Option<Verdict>,
    ) -> EditRecord {
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

    /// Log `pairs` replay pairs (baseline, candidate) of `text` for `night`.
    fn replays(dir: &Path, night: &str, text: &str, pairs: &[(f64, f64)]) {
        let sha = candidate_sha256(text);
        for (i, (b, c)) in pairs.iter().enumerate() {
            for (sha, score) in [(None, *b), (Some(sha.clone()), *c)] {
                log_row(
                    dir,
                    &ReplayRow {
                        night: night.into(),
                        case_id: format!("val-{i}"),
                        candidate_sha256: sha,
                        score,
                        judge_model: "j".into(),
                        at: String::new(),
                    },
                )
                .unwrap();
            }
        }
    }

    /// R-2 (D2): the record pass refuses without replays of the candidate,
    /// applies on 3 logged pairs 0.4 → 0.7, stays unapplied (logged
    /// `unverified`) on 2 pairs — whatever `gate` the caller asserts.
    #[test]
    fn record_pass_gate_comes_from_the_replay_rows_not_the_caller() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let cfg = DreamConfig::default();
        let night = "2026-10-08".to_owned();
        cases::append(d, &cases::sample("c1", "harness"), &night).unwrap();
        let add = edit(
            EditOp::Add,
            "Prefer numbered lists over ask_user forks",
            None,
            Some(Verdict::Accepted),
        );
        // Candidate pass: the text, no file, no log.
        let r = apply(
            d,
            &cfg,
            "harness",
            std::slice::from_ref(&add),
            &night,
            Pass::Candidate,
        )
        .unwrap();
        assert_eq!(r.edits[0].1, ApplyStatus::Applied);
        assert!(r.text.starts_with(
            "- Prefer numbered lists over ask_user forks [helpful: 0; harmful: 0; evidence: c1; since: "
        ));
        assert!(!sidecar_path(d, "harness").exists() && r.backup.is_none());
        assert!(all(d).unwrap().is_empty());
        // Record pass with the caller's `accepted` and no replays: refused, nothing written.
        let e = apply(
            d,
            &cfg,
            "harness",
            std::slice::from_ref(&add),
            &night,
            Pass::Record,
        )
        .unwrap_err()
        .to_string();
        assert!(e.contains("no replays for this candidate"), "{e}");
        assert!(!sidecar_path(d, "harness").exists() && all(d).unwrap().is_empty());
        // Two pairs → unverified: logged, not applied, the case stays open.
        replays(d, &night, &r.text, &[(0.4, 0.7), (0.4, 0.7)]);
        let r2 = apply(
            d,
            &cfg,
            "harness",
            std::slice::from_ref(&add),
            &night,
            Pass::Record,
        )
        .unwrap();
        assert_eq!(r2.edits[0].1, ApplyStatus::SkippedUnmatched);
        assert!(
            r2.edits[0].2.contains("gate unverified"),
            "{}",
            r2.edits[0].2
        );
        assert!(!sidecar_path(d, "harness").exists());
        let logged = all(d).unwrap();
        assert_eq!(logged.len(), 1);
        assert_eq!(logged[0].gate.unwrap().verdict, Verdict::Unverified);
        assert_eq!(cases::get(d, "c1").unwrap().state, CaseState::Open);
        // A third pair → accepted: written, backed up, the case closed by the edit.
        replays(d, &night, &r.text, &[(0.4, 0.7), (0.4, 0.7), (0.4, 0.7)]);
        let r3 = apply(
            d,
            &cfg,
            "harness",
            std::slice::from_ref(&add),
            &night,
            Pass::Record,
        )
        .unwrap();
        assert_eq!(r3.edits[0].1, ApplyStatus::Applied);
        assert_eq!(
            r3.backup.as_deref(),
            Some("dreaming/backups/2026-10-08/harness.md")
        );
        assert_eq!(
            std::fs::read_to_string(backup_path(d, "2026-10-08", "harness")).unwrap(),
            ""
        );
        assert_eq!(current(d, "harness"), r3.text);
        let logged = all(d).unwrap();
        assert_eq!(logged.len(), 2);
        let applied = logged.iter().find(|e| e.applied_at.is_some()).unwrap();
        assert_eq!(applied.target, "skill:harness");
        assert_eq!(applied.gate.unwrap().val_cases, 3);
        assert_eq!(cases::get(d, "c1").unwrap().state, CaseState::Closed);
        assert_eq!(
            cases::get(d, "c1").unwrap().closed_by.as_deref(),
            Some(format!("edit:{}", applied.id).as_str())
        );
        // A rejected candidate (a regression) lands in the buffer, even with `gate: accepted` asserted.
        let bad = edit(EditOp::Add, "Always say yes", None, Some(Verdict::Accepted));
        let cand = apply(
            d,
            &cfg,
            "harness",
            std::slice::from_ref(&bad),
            &night,
            Pass::Candidate,
        )
        .unwrap();
        replays(d, &night, &cand.text, &[(0.9, 0.2), (0.9, 0.2), (0.9, 0.2)]);
        let r4 = apply(d, &cfg, "harness", &[bad], &night, Pass::Record).unwrap();
        assert_eq!(r4.edits[0].1, ApplyStatus::SkippedUnmatched);
        assert_eq!(parse(&current(d, "harness")).len(), 1);
        let st = read_state(d);
        assert_eq!(st.rejected.len(), 1);
        assert_eq!(st.rejected[0].content, "Always say yes");
        assert!(
            st.rejected[0].reasons[0].starts_with("gate: baseline 0.90 → candidate 0.20"),
            "{:?}",
            st.rejected[0].reasons
        );
        // Bad names and empty calls.
        assert!(
            apply(
                d,
                &cfg,
                "../etc",
                std::slice::from_ref(&add),
                &night,
                Pass::Candidate
            )
            .is_err()
        );
        assert!(
            apply(
                d,
                &cfg,
                "Harness_1",
                std::slice::from_ref(&add),
                &night,
                Pass::Candidate
            )
            .is_err()
        );
        assert!(apply(d, &cfg, "harness", &[], &night, Pass::Candidate).is_err());
    }

    /// R-29 (D15): the anchor may be quoted as the proposer sees it — with
    /// `- `, the metadata tail, a `read_file` line number, odd spacing.
    #[test]
    fn anchors_are_normalised_to_the_bullet_content() {
        let mut b = parse("- old rule [helpful: 0; harmful: 0; evidence: c1; since: 2026-10-08]\n");
        for anchor in [
            "old rule",
            "- old rule [helpful: 0; harmful: 0; evidence: c1; since: 2026-10-08]",
            "3\t- old rule [helpful: 0; harmful: 0; evidence: c1; since: 2026-10-08]",
            "  old   rule ",
        ] {
            let mut bb = b.clone();
            let (s, _) = apply_one(
                &mut bb,
                &edit(EditOp::Replace, "new rule", Some(anchor), None),
                "d",
            );
            assert_eq!(s, ApplyStatus::Applied, "{anchor:?}");
            assert_eq!(bb[0].content, "new rule");
        }
        assert_eq!(
            apply_one(&mut b, &edit(EditOp::Delete, "", Some("nope"), None), "d").0,
            ApplyStatus::SkippedUnmatched
        );
        assert_eq!(normalise_anchor("12\tnot a bullet [x]"), "not a bullet [x]");
        // Cap: 12 bullets; round trip of the bullet format; retire is not a bullet op.
        let mut many = parse("");
        for i in 0..12 {
            assert_eq!(
                apply_one(
                    &mut many,
                    &edit(EditOp::Add, &format!("rule {i}"), None, None),
                    "d"
                )
                .0,
                ApplyStatus::Applied
            );
        }
        assert_eq!(
            apply_one(&mut many, &edit(EditOp::Add, "rule 13", None, None), "d").0,
            ApplyStatus::Error
        );
        assert_eq!(
            apply_one(
                &mut many,
                &edit(EditOp::Add, &"x".repeat(141), None, None),
                "d"
            )
            .0,
            ApplyStatus::Error
        );
        assert_eq!(
            apply_one(&mut many, &edit(EditOp::Retire, "", None, None), "d").0,
            ApplyStatus::Error
        );
        assert_eq!(parse(&render(&many)), many);
    }

    /// D4: `retire` moves a dreaming-made skill under `skills/retired/` with
    /// a `retired_because:` line, keeps the original for undo, never touches
    /// a human skill, and needs no gate; a candidate pass refuses it.
    #[test]
    fn retire_moves_a_dreaming_made_skill_and_refuses_a_human_one() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let cfg = DreamConfig::default();
        let night = "2026-10-08".to_owned();
        let skills = agent::skills::dir(d);
        std::fs::create_dir_all(&skills).unwrap();
        std::fs::write(
            skills.join("dreaming-triage.md"),
            "---\nname: dreaming-triage\norigin: dreaming\n---\nbody\n",
        )
        .unwrap();
        std::fs::write(
            skills.join("harness.md"),
            "---\nname: harness\n---\nhuman\n",
        )
        .unwrap();
        let retire = |rationale: &str| EditRecord {
            rationale: rationale.into(),
            cases: vec![],
            ..edit(EditOp::Retire, "", None, None)
        };
        let r = apply(
            d,
            &cfg,
            "dreaming-triage",
            &[retire("0 loads in 45 days")],
            &night,
            Pass::Candidate,
        )
        .unwrap();
        assert_eq!(r.edits[0].1, ApplyStatus::Error);
        assert!(skills.join("dreaming-triage.md").exists());
        let r = apply(
            d,
            &cfg,
            "dreaming-triage",
            &[retire("0 loads in 45 days")],
            &night,
            Pass::Record,
        )
        .unwrap();
        assert_eq!(r.edits[0].1, ApplyStatus::Applied, "{:?}", r.edits);
        assert!(!skills.join("dreaming-triage.md").exists());
        let moved = std::fs::read_to_string(retired_path(d, "dreaming-triage")).unwrap();
        assert!(
            moved.starts_with(
                "---\nretired_because: \"0 loads in 45 days\"\nretired_on: 2026-10-08\nname: dreaming-triage\n"
            ),
            "{moved}"
        );
        assert_eq!(
            std::fs::read_to_string(retire_backup_path(d, &night, "dreaming-triage")).unwrap(),
            "---\nname: dreaming-triage\norigin: dreaming\n---\nbody\n"
        );
        assert_eq!(retired_that_night(d, &night), ["dreaming-triage"]);
        let logged = all(d).unwrap();
        assert_eq!(logged.len(), 1);
        assert!(logged[0].applied_at.is_some() && logged[0].op == EditOp::Retire);
        // A human skill is refused; the file is untouched.
        let r = apply(d, &cfg, "harness", &[retire("x")], &night, Pass::Record).unwrap();
        assert_eq!(r.edits[0].1, ApplyStatus::Error);
        assert!(
            r.edits[0].2.contains("never retire a human skill"),
            "{}",
            r.edits[0].2
        );
        assert!(skills.join("harness.md").exists() && !retired_path(d, "harness").exists());
    }

    /// R-12 (D10): the counters move server-side — a settled case bumps
    /// `helpful` on the bullets citing it, a reopen bumps `harmful`, and the
    /// third harm retires the bullet (moved, journaled, the sidecar backed up
    /// for undo). A case nobody cites moves nothing.
    #[test]
    fn case_transitions_bump_the_counters_and_retire_a_harmful_bullet() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let night = crate::ledger::tonight();
        cases::append(d, &cases::sample("c1", "harness"), &night).unwrap();
        cases::append(d, &cases::sample("c2", "harness"), &night).unwrap();
        let text = "- cited rule [helpful: 0; harmful: 0; evidence: c1; since: 2026-10-01]\n- other rule [helpful: 0; harmful: 0; evidence: c2; since: 2026-10-01]\n";
        write_atomic(&sidecar_path(d, "harness"), text.as_bytes()).unwrap();
        cases::transition(d, "c1", CaseState::Settled, None).unwrap();
        let b = parse(&current(d, "harness"));
        assert_eq!((b[0].helpful, b[0].harmful, b[1].helpful), (1, 0, 0));
        assert_eq!(
            std::fs::read_to_string(backup_path(d, &night, "harness")).unwrap(),
            text,
            "the pre-night sidecar is the undo backup"
        );
        // Reopen c2 three times (closed in between): harmful 1, 2, then retired.
        for n in 1..=3 {
            cases::transition(d, "c2", CaseState::Closed, Some("edit:e")).unwrap();
            cases::transition(d, "c2", CaseState::Reopened, None).unwrap();
            let b = parse(&current(d, "harness"));
            if n < HARMFUL_RETIRE {
                assert_eq!(b[1].harmful, n, "{b:?}");
            } else {
                assert_eq!(b.len(), 1, "{b:?}");
                assert_eq!(b[0].content, "cited rule");
            }
        }
        let retired = std::fs::read_to_string(retired_bullets_path(d, "harness")).unwrap();
        assert_eq!(
            retired,
            "- other rule [helpful: 0; harmful: 3; evidence: c2; since: 2026-10-01]\n"
        );
        let j = crate::journal::text(d);
        assert!(
            j.contains("### Settle\n- retired from `harness` (harmful ≥ 3"),
            "{j}"
        );
        assert!(j.contains("undo: `dreaming_undo("), "{j}");
        assert!(count_case(d, "nobody", CaseState::Reopened).is_empty());
    }
}
