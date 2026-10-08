//! `dreaming_undo` (design doc 136 §3.3 item 5): for every knowledge id the
//! night's record lists (optionally one `group` — the knowledge op), knowledge
//! `restore(id, commit_before)`; for every learned sidecar backed up that
//! night (`group` None, `edits` or `skill:<name>`), restore
//! `backups/<night>/<skill>.md` (an empty backup removes the sidecar); mark
//! the night's applied edits `undone_at` in `edits.jsonl`; publish
//! `DreamUndone`; one "Undone" journal line. Idempotent: what is already
//! undone is skipped.

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
    // Knowledge: restore each touched id to its pre-night commit.
    for t in record.knowledge.iter_mut() {
        if t.undone || group.is_some_and(|g| g != t.group && g != "knowledge") {
            continue;
        }
        if t.commit_before.is_empty() {
            report.errors.push(format!(
                "{}: created by dreaming that night; archive or delete it by hand",
                t.id
            ));
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
        if !report.skills_restored.contains(&skill) {
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
    // Preferences: `backups/<night>/preferences.md` when the night touched the block.
    if group.is_none_or(|g| g == "preferences") {
        let backup = crate::night::dir(data_dir)
            .join("backups")
            .join(night)
            .join("preferences.md");
        if let Ok(bytes) = std::fs::read(&backup) {
            match crate::night::write_atomic(&crate::preferences::path(data_dir), &bytes) {
                Ok(()) => {
                    report.skills_restored.push("preferences".into());
                    crate::preferences::republish(data_dir).await;
                }
                Err(e) => report.errors.push(format!("preferences: {e}")),
            }
        }
    }
    let all_undone = record.knowledge.iter().all(|t| t.undone)
        && backed_up_skills(data_dir, night)
            .iter()
            .all(|s| record.sidecars_undone.contains(s));
    if group.is_none() && all_undone {
        record.summary.undone = true;
    }
    write_record(data_dir, &record)?;
    let mut state = read_state(data_dir);
    if state.last_night.as_ref().is_some_and(|n| n.night == *night) {
        state.last_night = Some(record.summary.clone());
        write_state(data_dir, &state)?;
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
    use crate::night::{NightRecord, Touched};
    use crate::types::{EditOp, EditRecord, GateResult, NightCounts, NightSummary};

    #[tokio::test]
    async fn sidecar_backups_restore_edits_are_marked_and_cases_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let night = "2026-10-08".to_owned();
        write_record(
            d,
            &NightRecord {
                summary: NightSummary {
                    night: night.clone(),
                    started_at: "2026-10-08T01:00:00Z".into(),
                    finished_at: Some("2026-10-08T01:30:00Z".into()),
                    counts: NightCounts::default(),
                    spend_usd: 0.1,
                    mode: "nightly".into(),
                    undone: false,
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
            },
        )
        .unwrap();
        cases::append(d, &cases::sample("c1", "harness"), &night).unwrap();
        // One accepted edit through the record pass: sidecar written, backup empty.
        let r = edits::apply(
            d,
            "harness",
            &[EditRecord {
                id: "e1".into(),
                night: night.clone(),
                target: String::new(),
                op: EditOp::Add,
                content: "Prefer numbered lists".into(),
                anchor: None,
                rationale: String::new(),
                cases: vec!["c1".into()],
                gate: Some(GateResult {
                    baseline: 0.3,
                    candidate: 0.8,
                    min_delta: 0.1,
                    val_cases: 3,
                    verdict: Verdict::Accepted,
                }),
                applied_at: None,
                undone_at: None,
            }],
            &night,
        )
        .unwrap();
        assert!(r.backup.is_some());
        assert!(edits::sidecar_path(d, "harness").exists());
        assert_eq!(
            cases::get(d, "c1").unwrap().state,
            crate::types::CaseState::Closed
        );
        // A group that selects another skill does nothing to this one.
        let other = undo(d, None, None, &night, Some("skill:other"))
            .await
            .unwrap();
        assert!(other.skills_restored.is_empty() && other.edits_undone.is_empty());
        assert!(edits::sidecar_path(d, "harness").exists());
        // The whole night.
        let rep = undo(d, None, None, &night, None).await.unwrap();
        assert_eq!(rep.skills_restored, ["harness"]);
        assert_eq!(rep.edits_undone, ["e1"]);
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
        assert!(j.contains("### Undone\n- skill:other: 0 concepts restored, 0 sidecars restored, 0 edits undone\n- whole night: 0 concepts restored, 1 sidecars restored, 1 edits undone; 1 errors"), "{j}");
        // Not every entry was undone (the error), so the summary is not `undone`; a second run is a no-op.
        assert!(!read_record(d, &night).unwrap().summary.undone);
        let again = undo(d, None, None, &night, None).await.unwrap();
        assert!(again.skills_restored.is_empty() && again.edits_undone.is_empty());
        assert!(
            undo(d, None, None, &"2026-01-01".to_owned(), None)
                .await
                .is_err()
        );
    }
}
