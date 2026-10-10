//! 159 §13.3 (W2 H5): the Merge press of a prompt study whose artefact is a learned sidecar
//! (`cases.source: dreaming`, `cases.artefact: skills/learned/<skill>.md`) lands the text
//! through dreaming's write path — backup under tonight's night, atomic write, an
//! `edits.jsonl` record with origin `study:<slug>` — and makes sure that night has a ledger
//! record, so `dreaming_undo <night>` reverts it like any night's edits. `autoresearch`
//! cannot depend on `dreaming`; the `rpc` crate sees both, so the call sits here.

use std::path::Path;

use autoresearch::{CaseSource, ControlOp, StudyState};
use dreaming::night::{NightRecord, read_record, write_record};
use dreaming::types::{NightCounts, NightId, NightSummary};

/// `skills/learned/<skill>.md` → `skill`; anything else is not a sidecar.
pub fn sidecar_skill(artefact: &str) -> Option<&str> {
    artefact
        .strip_prefix("skills/learned/")?
        .strip_suffix(".md")
        .filter(|s| !s.contains('/'))
}

/// Land `text` as `skills/learned/<skill>.md` under tonight's night and list the backup in the
/// night's record (created when dreaming has none for tonight). Returns the night id.
pub fn land(data_dir: &Path, slug: &str, skill: &str, text: &str) -> anyhow::Result<NightId> {
    let night = dreaming::ledger::tonight();
    dreaming::edits::land_text(data_dir, &night, skill, text, &format!("study:{slug}"))?;
    let mut r = read_record(data_dir, &night).unwrap_or_else(|| {
        let now = dreaming::cases::now_rfc3339();
        NightRecord {
            summary: NightSummary {
                night: night.clone(),
                started_at: now.clone(),
                finished_at: Some(now),
                counts: NightCounts::default(),
                spend_usd: 0.0,
                mode: "study".into(),
                undone: false,
                status: None,
                judge_same_model: false,
            },
            session: None,
            knowledge: Vec::new(),
            backups: Vec::new(),
            phases: Vec::new(),
            proposals: Vec::new(),
            sidecars_undone: Vec::new(),
            head_at_start: None,
            retired: Vec::new(),
            preferences_touched: false,
        }
    });
    if !r.backups.iter().any(|s| s == skill) {
        r.backups.push(skill.to_owned());
        write_record(data_dir, &r)?;
    }
    Ok(night)
}

/// Before the actor's `Merge`: when the study is a finished sidecar study and `op` names one
/// of its changesets, land that changeset's artefact text through [`land`]. Anything the actor
/// would refuse (not finished, no such changeset) is left to the actor's own message. The
/// actor's `Merge` then still runs: in the study's scratch repo it is bookkeeping (the
/// `merged` event, the branch folded into the scratch checkout).
pub async fn before_merge(data_dir: &Path, slug: &str, op: &ControlOp) -> Result<(), String> {
    let ControlOp::Merge { changeset } = op else {
        return Ok(());
    };
    let d = autoresearch::rpc_impl::get(slug.to_owned()).await?;
    let Some(cases) = d.spec.cases.as_ref() else {
        return Ok(());
    };
    if cases.source != CaseSource::Dreaming || d.info.state != StudyState::Finished {
        return Ok(());
    }
    let Some(skill) = sidecar_skill(&cases.artefact) else {
        return Ok(());
    };
    let Some(cs) = d.changesets.iter().find(|c| c.k == *changeset) else {
        return Ok(());
    };
    let (repo, spec) = (
        d.info.repo.clone(),
        format!("{}:{}", cs.branch, cases.artefact),
    );
    let text = tokio::task::spawn_blocking(move || {
        autoresearch::git::read(Path::new(&repo), &["show", &spec])
    })
    .await
    .map_err(|e| format!("show task: {e}"))?
    .map_err(|e| e.to_string())?;
    land(data_dir, slug, skill, &text)
        .map(drop)
        .map_err(|e| format!("landing skills/learned/{skill}.md: {e:#}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidecar_skill_reads_only_learned_sidecars() {
        assert_eq!(sidecar_skill("skills/learned/harness.md"), Some("harness"));
        assert_eq!(sidecar_skill("skills/harness.md"), None);
        assert_eq!(sidecar_skill("skills/learned/a/b.md"), None);
        assert_eq!(sidecar_skill("prompt.md"), None);
    }

    /// The press writes the sidecar, backs the old text up under tonight's night, logs one
    /// `study:<slug>` record, and the night's ledger record lists the backup — so an undo
    /// restores the old text.
    #[test]
    fn land_is_undoable_like_a_night() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path();
        let sidecar = dreaming::edits::sidecar_path(data_dir, "harness");
        std::fs::create_dir_all(sidecar.parent().unwrap()).unwrap();
        std::fs::write(&sidecar, "old\n").unwrap();

        let night = land(data_dir, "sort-speed", "harness", "new\n").unwrap();
        assert_eq!(night, dreaming::ledger::tonight());
        assert_eq!(std::fs::read_to_string(&sidecar).unwrap(), "new\n");
        assert_eq!(
            std::fs::read_to_string(dreaming::edits::backup_path(data_dir, &night, "harness"))
                .unwrap(),
            "old\n"
        );
        let edits = dreaming::edits::all(data_dir).unwrap();
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].rationale, "landed by study:sort-speed");
        let r = read_record(data_dir, &night).expect("a night record so undo finds the night");
        assert_eq!(r.backups, vec!["harness".to_owned()]);
        assert_eq!(r.summary.mode, "study");

        // A second landing the same night: one backup, one record, the ledger unchanged.
        land(data_dir, "sort-speed", "harness", "newer\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(dreaming::edits::backup_path(data_dir, &night, "harness"))
                .unwrap(),
            "old\n"
        );
        assert_eq!(read_record(data_dir, &night).unwrap().backups.len(), 1);

        let report = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(dreaming::undo::undo(data_dir, None, None, &night, None))
            .unwrap();
        assert!(report.errors.is_empty(), "{report:?}");
        assert_eq!(std::fs::read_to_string(&sidecar).unwrap(), "old\n");
    }
}
