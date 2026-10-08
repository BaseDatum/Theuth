//! `dreaming_undo` (design doc 136 §3.3 item 5): for every knowledge id the
//! night's record lists (optionally one `group` — the knowledge op), knowledge
//! `restore(id, commit_before)`; for every learned sidecar backed up that
//! night (`group` None, `edits` or `skill:<name>`), restore
//! `backups/<night>/<skill>.md` (an empty backup removes the sidecar); a
//! skill `retire`d that night moves back from `skills/retired/`; the
//! preferences block is restored from `backups/<night>/preferences.md` once
//! (R-9); mark the night's applied edits `undone_at` in `edits.jsonl`;
//! publish `DreamUndone`; one "Undone" journal line. Idempotent: what is
//! already undone is skipped, and a call that restores nothing journals
//! nothing (R-37). A `group` that selects nothing of the night is an error.

use std::path::Path;

use runtime::Runtime;

use crate::night::{backed_up_skills, read_record, read_state, write_record, write_state};
use crate::types::{NightId, UndoReport, Verdict};
use crate::{DreamUndone, cases, edits, journal};

/// Does `group` select the learned sidecars (`edits`, `skills`, `skill:<name>`)?
fn group_selects_skill(group: Option<&str>, skill: &str) -> bool {
    match group {
        None => true,
        Some("edits" | "skills") => true,
        Some(g) => g.strip_prefix("skill:") == Some(skill),
    }
}

/// Undo a night or one of its groups.
pub async fn undo(
    data_dir: &Path,
    knowledge: Option<&knowledge::Knowledge>,
    rt: Option<&Runtime>,
    night: &NightId,
    group: Option<&str>,
) -> anyhow::Result<UndoReport> {
    let mut report = UndoReport {
        night: night.clone(),
        group: group.map(str::to_owned),
        ..Default::default()
    };
    let Some(mut record) = read_record(data_dir, night) else {
        anyhow::bail!("no night {night}");
    };
    let known_groups: Vec<String> = {
        let mut g: Vec<String> = record.knowledge.iter().map(|t| t.group.clone()).collect();
        g.extend(
            backed_up_skills(data_dir, night)
                .iter()
                .map(|s| format!("skill:{s}")),
        );
        g.extend(
            edits::retired_that_night(data_dir, night)
                .iter()
                .map(|s| format!("skill:{s}")),
        );
        if crate::preferences::backup_path(data_dir, night).exists() {
            g.push("preferences".into());
        }
        g.sort();
        g.dedup();
        g
    };
    if let Some(g) = group
        && !matches!(g, "knowledge" | "edits" | "skills")
        && !known_groups.iter().any(|k| k == g)
    {
        anyhow::bail!(
            "night {night} has no group {g:?}; its groups: {}",
            if known_groups.is_empty() {
                "none".to_owned()
            } else {
                known_groups.join(", ")
            }
        );
    }
    // Knowledge: restore each touched id to its pre-night commit.
    for t in record.knowledge.iter_mut() {
        if t.undone || group.is_some_and(|g| g != t.group && g != "knowledge") {
            continue;
        }
        if t.commit_before.is_empty() {
            // Created by dreaming that night: nothing to restore to. Said once (R-37).
            report.errors.push(format!(
                "{}: created by dreaming that night; archive or delete it by hand",
                t.id
            ));
            t.undone = true;
            continue;
        }
        let Some(k) = knowledge else {
            report
                .errors
                .push(format!("{}: knowledge is not running", t.id));
            continue;
        };
        match k.restore(&t.id, Some(&t.commit_before)).await {
            Ok(_) => {
                t.undone = true;
                report.knowledge_restored.push(t.id.clone());
            }
            Err(e) => report.errors.push(format!("{}: {e:#}", t.id)),
        }
    }
    // Retired skills: move the file back (the backup is the pre-retire text).
    for skill in edits::retired_that_night(data_dir, night) {
        let handle = format!("retired:{skill}");
        if !group_selects_skill(group, &skill) || record.sidecars_undone.contains(&handle) {
            continue;
        }
        let backup = edits::retire_backup_path(data_dir, night, &skill);
        let r = std::fs::read(&backup).map_err(anyhow::Error::from).and_then(|bytes| {
            crate::night::write_atomic(&edits::skill_path(data_dir, &skill), &bytes)?;
            match std::fs::remove_file(edits::retired_path(data_dir, &skill)) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e.into()),
            }
        });
        match r {
            Ok(()) => {
                report.skills_restored.push(handle.clone());
                record.sidecars_undone.push(handle);
            }
            Err(e) => report.errors.push(format!("{handle}: {e:#}")),
        }
    }
    // Sidecars: the backup is the pre-night text.
    for skill in backed_up_skills(data_dir, night) {
        if !group_selects_skill(group, &skill) || record.sidecars_undone.contains(&skill) {
            continue;
        }
        let backup = edits::backup_path(data_dir, night, &skill);
        let sidecar = edits::sidecar_path(data_dir, &skill);
        let bytes = match std::fs::read(&backup) {
            Ok(b) => b,
            Err(e) => {
                report
                    .errors
                    .push(format!("skill:{skill}: backup unreadable: {e}"));
                continue;
            }
        };
        let r = if bytes.is_empty() {
            match std::fs::remove_file(&sidecar) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e),
            }
        } else {
            crate::night::write_atomic(&sidecar, &bytes)
        };
        match r {
            Ok(()) => {
                report.skills_restored.push(skill.clone());
                record.sidecars_undone.push(skill.clone());
            }
            Err(e) => report.errors.push(format!("skill:{skill}: {e}")),
        }
        if !record.backups.contains(&skill) {
            record.backups.push(skill);
        }
    }
    // Edits: mark the applied ones of the restored skills undone; reopen their cases.
    let now = cases::now_rfc3339();
    for mut e in edits::all(data_dir)? {
        if e.night != *night || e.applied_at.is_none() || e.undone_at.is_some() {
            continue;
        }
        let skill = e
            .target
            .strip_prefix("skill:")
            .unwrap_or(&e.target)
            .to_owned();
        let restored = match e.op {
            crate::types::EditOp::Retire => format!("retired:{skill}"),
            _ => skill,
        };
        if !report.skills_restored.contains(&restored) {
            continue;
        }
        e.undone_at = Some(now.clone());
        edits::log(data_dir, &e)?;
        report.edits_undone.push(e.id.clone());
        if e.gate
            .as_ref()
            .is_some_and(|g| g.verdict == Verdict::Accepted)
        {
            for c in &e.cases {
                let _ = cases::transition(data_dir, c, crate::types::CaseState::Reopened, None);
            }
        }
    }
    // Preferences: `backups/<night>/preferences.md` when the night touched the block; once (R-9).
    if group.is_none_or(|g| g == "preferences")
        && !record.sidecars_undone.iter().any(|s| s == "preferences")
        && let Ok(bytes) = std::fs::read(crate::preferences::backup_path(data_dir, night))
    {
        let target = crate::preferences::path(data_dir);
        let r = if bytes.is_empty() {
            match std::fs::remove_file(&target) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e),
            }
        } else {
            crate::night::write_atomic(&target, &bytes)
        };
        match r {
            Ok(()) => {
                report.skills_restored.push("preferences".into());
                record.sidecars_undone.push("preferences".into());
                crate::preferences::republish(data_dir).await;
            }
            Err(e) => report.errors.push(format!("preferences: {e}")),
        }
    }
    let all_undone = record.knowledge.iter().all(|t| t.undone)
        && backed_up_skills(data_dir, night)
            .iter()
            .all(|s| record.sidecars_undone.contains(s))
        && edits::retired_that_night(data_dir, night)
            .iter()
            .all(|s| record.sidecars_undone.contains(&format!("retired:{s}")));
    if group.is_none() && all_undone {
        record.summary.undone = true;
    }
    write_record(data_dir, &record)?;
    let mut state = read_state(data_dir);
    if state.last_night.as_ref().is_some_and(|n| n.night == *night) {
        state.last_night = Some(record.summary.clone());
        write_state(data_dir, &state)?;
    }
    // Nothing restored and nothing to report: no journal line, no event (R-37).
    if report.knowledge_restored.is_empty()
        && report.skills_restored.is_empty()
        && report.edits_undone.is_empty()
        && report.errors.is_empty()
    {
        return Ok(report);
    }
    let line = format!(
        "- {}: {} concepts restored, {} sidecars restored, {} edits undone{}",
        group.unwrap_or("whole night"),
        report.knowledge_restored.len(),
        report.skills_restored.len(),
        report.edits_undone.len(),
        if report.errors.is_empty() {
            String::new()
        } else {
            format!(
                "; {} errors: {}",
                report.errors.len(),
                report.errors.join(" · ")
            )
        }
    );
    if let Err(e) = journal::append(data_dir, night, "Undone", &line).await {
        tracing::warn!("dreaming: undo journal line not written: {e}");
    }
    if let Some(rt) = rt {
        rt.publish(DreamUndone {
            night: night.clone(),
            group: group.map(str::to_owned),
        })
        .await;
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DreamConfig;
    use crate::night::{NightRecord, Touched};
    use crate::replay::{ReplayRow, candidate_sha256, log_row};
    use crate::types::{EditOp, EditRecord, NightCounts, NightSummary};

    fn record(night: &str) -> NightRecord {
        NightRecord {
            summary: NightSummary {
                night: night.into(),
                started_at: "2026-10-08T01:00:00Z".into(),
                finished_at: Some("2026-10-08T01:30:00Z".into()),
                counts: NightCounts::default(),
                spend_usd: 0.1,
                mode: "nightly".into(),
                undone: false,
                status: None,
                judge_same_model: false,
            },
            session: None,
            knowledge: vec![Touched {
                id: "facts/new".into(),
                commit_before: String::new(),
                group: "put".into(),
                undone: false,
            }],
            backups: vec![],
            phases: vec![],
            proposals: vec![],
            sidecars_undone: vec![],
            head_at_start: None,
            retired: vec![],
            preferences_touched: false,
        }
    }

    fn edit(op: EditOp, content: &str, night: &str) -> EditRecord {
        EditRecord {
            id: String::new(),
            night: night.into(),
            target: String::new(),
            op,
            content: content.into(),
            anchor: None,
            rationale: "disused".into(),
            cases: vec!["c1".into()],
            gate: None,
            applied_at: None,
            undone_at: None,
        }
    }

    #[tokio::test]
    async fn sidecar_backups_restore_edits_are_marked_and_cases_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let cfg = DreamConfig::default();
        let night = "2026-10-08".to_owned();
        write_record(d, &record(&night)).unwrap();
        cases::append(d, &cases::sample("c1", "harness"), &night).unwrap();
        // One accepted edit through the record pass: sidecar written, backup empty.
        let e = edit(EditOp::Add, "Prefer numbered lists", &night);
        let cand = edits::apply(d, &cfg, "harness", &[e.clone()], &night, edits::Pass::Candidate)
            .unwrap();
        let sha = candidate_sha256(&cand.text);
        for i in 0..3 {
            for (s, score) in [(None, 0.3), (Some(sha.clone()), 0.8)] {
                log_row(
                    d,
                    &ReplayRow {
                        night: night.clone(),
                        case_id: format!("v{i}"),
                        candidate_sha256: s,
                        score,
                        judge_model: "j".into(),
                        at: String::new(),
                    },
                )
                .unwrap();
            }
        }
        let r = edits::apply(d, &cfg, "harness", &[e], &night, edits::Pass::Record).unwrap();
        assert!(r.backup.is_some(), "{:?}", r.edits);
        let e1 = r.edits[0].0.clone();
        assert!(edits::sidecar_path(d, "harness").exists());
        assert_eq!(
            cases::get(d, "c1").unwrap().state,
            crate::types::CaseState::Closed
        );
        // A group the night does not have is an error (R-37); nothing moves.
        let err = undo(d, None, None, &night, Some("skill:other"))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("no group \"skill:other\""), "{err}");
        assert!(err.contains("put, skill:harness"), "{err}");
        assert!(edits::sidecar_path(d, "harness").exists());
        // The whole night.
        let rep = undo(d, None, None, &night, None).await.unwrap();
        assert_eq!(rep.skills_restored, ["harness"]);
        assert_eq!(rep.edits_undone, [e1]);
        assert!(
            !edits::sidecar_path(d, "harness").exists(),
            "empty backup removes the sidecar"
        );
        assert_eq!(
            rep.errors.len(),
            1,
            "the created-by-dreaming id has no commit to restore: {:?}",
            rep.errors
        );
        assert!(rep.errors[0].contains("facts/new"));
        assert_eq!(
            cases::get(d, "c1").unwrap().state,
            crate::types::CaseState::Reopened
        );
        let logged = edits::all(d).unwrap();
        assert!(logged[0].undone_at.is_some());
        let j = journal::text(d);
        assert!(
            j.contains("### Undone\n- whole night: 0 concepts restored, 1 sidecars restored, 1 edits undone; 1 errors"),
            "{j}"
        );
        // The created id counts as undone once its line was said (R-37): the summary is `undone`.
        assert!(read_record(d, &night).unwrap().summary.undone);
        // A second run restores nothing and says nothing (R-37).
        let again = undo(d, None, None, &night, None).await.unwrap();
        assert!(again.skills_restored.is_empty() && again.edits_undone.is_empty());
        assert!(again.errors.is_empty());
        assert_eq!(journal::text(d), j, "no second Undone line");
        assert!(
            undo(d, None, None, &"2026-01-01".to_owned(), None)
                .await
                .is_err()
        );
    }

    /// R-9 (D8): `preferences_set` backs the block up once per night; undo
    /// restores the pre-set text once (a second undo restores nothing); an
    /// empty backup removes the file.
    #[tokio::test]
    async fn preferences_restore_once_from_the_nights_backup() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let night = crate::ledger::tonight();
        write_record(d, &record(&night)).unwrap();
        crate::preferences::set(d, None, "add", "commit as you go", &[], 20)
            .await
            .unwrap();
        let before = std::fs::read_to_string(crate::preferences::path(d)).unwrap();
        crate::preferences::set(d, None, "add", "caveman replies", &[], 20)
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(crate::preferences::backup_path(d, &night)).unwrap(),
            "",
            "the backup is the block before the night's FIRST write"
        );
        assert_ne!(
            std::fs::read_to_string(crate::preferences::path(d)).unwrap(),
            before
        );
        let rep = undo(d, None, None, &night, Some("preferences")).await.unwrap();
        assert_eq!(rep.skills_restored, ["preferences"]);
        assert!(!crate::preferences::path(d).exists(), "empty backup removes the file");
        let again = undo(d, None, None, &night, Some("preferences")).await.unwrap();
        assert!(again.skills_restored.is_empty(), "{again:?}");
        // A night that started with a block restores that block.
        let night2 = "2026-10-09";
        write_record(d, &record(night2)).unwrap();
        crate::night::write_atomic(
            &crate::preferences::backup_path(d, night2),
            before.as_bytes(),
        )
        .unwrap();
        undo(d, None, None, &night2.to_owned(), None).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(crate::preferences::path(d)).unwrap(),
            before
        );
    }

    /// D4: a skill `retire`d that night moves back on undo; the retire edit
    /// is marked undone.
    #[tokio::test]
    async fn a_retired_skill_moves_back() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let cfg = DreamConfig::default();
        let night = "2026-10-08".to_owned();
        write_record(d, &record(&night)).unwrap();
        let text = "---\nname: dreaming-x\norigin: dreaming\n---\nbody\n";
        crate::night::write_atomic(&edits::skill_path(d, "dreaming-x"), text.as_bytes()).unwrap();
        let r = edits::apply(
            d,
            &cfg,
            "dreaming-x",
            &[edit(EditOp::Retire, "", &night)],
            &night,
            edits::Pass::Record,
        )
        .unwrap();
        assert_eq!(r.edits[0].1, crate::types::ApplyStatus::Applied, "{:?}", r.edits);
        let rep = undo(d, None, None, &night, Some("skill:dreaming-x")).await.unwrap();
        assert_eq!(rep.skills_restored, ["retired:dreaming-x"]);
        assert_eq!(rep.edits_undone.len(), 1);
        assert_eq!(std::fs::read_to_string(edits::skill_path(d, "dreaming-x")).unwrap(), text);
        assert!(!edits::retired_path(d, "dreaming-x").exists());
        assert!(edits::all(d).unwrap()[0].undone_at.is_some());
    }
}
