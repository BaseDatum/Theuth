//! The `autoresearch.*` RPC methods (design doc 140 §12.5) as plain async fns
//! the `rpc` crate's `ApiServer` delegates to: each asks the [`Studies`]
//! actor; absent actor → `Err("autoresearch is not running")`. Owned by W3
//! after the skeleton (bodies here are complete delegation; W3 changes the
//! actor, not this file, unless a method's shape changes).

use kameo::actor::ActorRef;

use crate::config::AutoresearchConfig;
use crate::types::*;
use crate::{AUTORESEARCH_ACTOR, Studies};

/// The method names the `rpc` crate registers (140 §12.5, 151 §4.4, 159 §5.11).
pub const METHODS: [&str; 21] = [
    "autoresearch.list",
    "autoresearch.get",
    "autoresearch.experiments",
    "autoresearch.experiment",
    "autoresearch.control",
    "autoresearch.steer",
    "autoresearch.idea",
    "autoresearch.create",
    "autoresearch.settings.get",
    "autoresearch.settings.set",
    "autoresearch.draft",
    "autoresearch.drafts",
    "autoresearch.draft_edit",
    "autoresearch.draft_confirm",
    "autoresearch.discard_draft",
    "autoresearch.archive",
    "autoresearch.cases",
    "autoresearch.judgments",
    "autoresearch.human_pair",
    "autoresearch.launch",
    "autoresearch.mechanism",
];

fn actor() -> std::result::Result<ActorRef<Studies>, String> {
    ActorRef::<Studies>::lookup(AUTORESEARCH_ACTOR)
        .ok()
        .flatten()
        .ok_or_else(|| "autoresearch is not running".to_owned())
}

async fn ask<M>(
    m: M,
) -> std::result::Result<<<Studies as kameo::message::Message<M>>::Reply as kameo::Reply>::Ok, String>
where
    Studies: kameo::message::Message<M>,
    M: Send + 'static,
    <<Studies as kameo::message::Message<M>>::Reply as kameo::Reply>::Error: std::fmt::Display,
{
    actor()?.ask(m).await.map_err(|e| e.to_string())
}

/// `autoresearch.list` → every study, newest first.
pub async fn list() -> std::result::Result<Vec<StudyInfo>, String> {
    ask(Status { slug: None }).await
}

/// `autoresearch.get {slug}` → the Study view's data.
pub async fn get(slug: String) -> std::result::Result<StudyDetail, String> {
    ask(Detail { slug }).await
}

/// `autoresearch.experiments {slug, before?, limit}`.
pub async fn experiments(
    slug: String,
    before: Option<u32>,
    limit: u32,
) -> std::result::Result<Vec<ExperimentRow>, String> {
    ask(Experiments {
        slug,
        before,
        limit,
    })
    .await
}

/// `autoresearch.experiment {slug, n}`.
pub async fn experiment(slug: String, n: u32) -> std::result::Result<ExperimentDetail, String> {
    ask(Experiment { slug, n }).await
}

/// `autoresearch.control {slug, op}` (the user's buttons).
pub async fn control(slug: String, op: ControlOp) -> std::result::Result<StudyInfo, String> {
    ask(Control {
        slug,
        op,
        by: By::User,
    })
    .await
}

/// `autoresearch.steer {slug, text}`.
pub async fn steer(slug: String, text: String) -> std::result::Result<SteerId, String> {
    ask(SteerMsg {
        slug,
        text,
        by: By::User,
    })
    .await
}

/// `autoresearch.idea {slug, op}`.
pub async fn idea(slug: String, op: IdeaChange) -> std::result::Result<Idea, String> {
    ask(IdeaOp {
        slug,
        op,
        by: By::User,
    })
    .await
}

/// `autoresearch.create {draft_id, start}` — the draft card's press.
pub async fn create(draft_id: String, start: StartWhen) -> std::result::Result<StudyInfo, String> {
    ask(StartDraft { draft_id, start }).await
}

/// `autoresearch.settings.get`.
pub async fn settings() -> std::result::Result<AutoresearchConfig, String> {
    ask(Settings).await
}

/// `autoresearch.settings.set`.
pub async fn set_settings(
    cfg: AutoresearchConfig,
) -> std::result::Result<AutoresearchConfig, String> {
    ask(SetSettings(cfg)).await
}

// ───────────────────────────── 151 §4.4 the living draft ─────────────────────────────

/// The drafts' root and whether a study with `draft_id` exists, from the actor.
async fn draft_ctx(draft_id: &str) -> std::result::Result<(std::path::PathBuf, bool), String> {
    let root = ask(DraftRoot).await?;
    let slugs = ask(StudySlugs).await?;
    Ok((root, slugs.iter().any(|s| s == draft_id)))
}

/// `autoresearch.draft {draft_id}` → the panel's `DraftView` (151 §5).
pub async fn draft(draft_id: String) -> std::result::Result<DraftView, String> {
    let (root, started) = draft_ctx(&draft_id).await?;
    crate::setup::view(&root, &draft_id, started).map_err(|e| e.to_string())
}

/// `autoresearch.drafts` → setting-up rows (drafts with no study yet), newest first (151 D6).
pub async fn drafts() -> std::result::Result<Vec<DraftSummary>, String> {
    let root = ask(DraftRoot).await?;
    let slugs = ask(StudySlugs).await?;
    Ok(crate::setup::drafts(&root, &slugs))
}

/// `autoresearch.draft_edit {draft_id, field, value}` — a leaf set in the panel (151 §4.3 grammar).
pub async fn draft_edit(
    draft_id: String,
    field: String,
    value: String,
) -> std::result::Result<DraftView, String> {
    let (root, started) = draft_ctx(&draft_id).await?;
    crate::setup::edit(&root, &draft_id, field.trim(), &value).map_err(|e| e.to_string())?;
    crate::setup::view(&root, &draft_id, started).map_err(|e| e.to_string())
}

/// `autoresearch.draft_confirm {draft_id, field}` — a row OK'd in the panel.
pub async fn draft_confirm(
    draft_id: String,
    field: String,
) -> std::result::Result<DraftView, String> {
    let (root, started) = draft_ctx(&draft_id).await?;
    crate::setup::confirm(&root, &draft_id, field.trim()).map_err(|e| e.to_string())?;
    crate::setup::view(&root, &draft_id, started).map_err(|e| e.to_string())
}

/// `autoresearch.discard_draft {draft_id}` — removes the drafts dir; later Starts are refused.
pub async fn discard_draft(draft_id: String) -> std::result::Result<(), String> {
    let root = ask(DraftRoot).await?;
    crate::setup::discard(&root, &draft_id).map_err(|e| e.to_string())
}

// ───────────────────────────── 159 §5.11 per-kind methods ─────────────────────────────
// Stubs that answer `not built (W<n>)` until their wave; each wave edits only its one
// `// 159:<TAG>` line here (159 §18.1).

/// 159 §5.11 `autoresearch.archive {slug}` (W1 front members, W4 cells).
pub async fn archive(slug: String) -> std::result::Result<Vec<ArchiveRow>, String> {
    let _ = slug;
    Err("not built (W1, W4)".to_owned()) // 159:W1 159:W4 — pareto::archive / population::archive on `archive.kind`
}

/// 159 §5.11 `autoresearch.cases {slug, n?}` — the case × experiment rows of the heat strip (W2).
pub async fn cases(slug: String, n: Option<u32>) -> std::result::Result<Vec<CaseRow>, String> {
    crate::cases::rows_of(ask(Detail { slug }).await?.view, n) // 159:W2
}

/// 159 §5.11 `autoresearch.judgments {slug, n?}` — blind-pair verdicts (W3).
pub async fn judgments(
    slug: String,
    n: Option<u32>,
) -> std::result::Result<Vec<JudgmentRow>, String> {
    let _ = (slug, n);
    Err("not built (W3)".to_owned()) // 159:W3 — judge::judgments(store, &slug, n)
}

/// 159 §5.11 `autoresearch.human_pair {slug, id, winner}` — the user's answer to a blind pair (W3).
pub async fn human_pair(
    slug: String,
    id: i64,
    winner: String,
) -> std::result::Result<StudyInfo, String> {
    let _ = (slug, id, winner);
    Err("not built (W3)".to_owned()) // 159:W3 — judge::human_pair(env, &slug, id, &winner)
}

/// 159 §5.11 `autoresearch.launch {slug, n, token, approve}` — the launch press (W6).
pub async fn launch(
    slug: String,
    n: u32,
    token: String,
    approve: bool,
) -> std::result::Result<StudyInfo, String> {
    let _ = (slug, n, token, approve);
    Err("not built (W6)".to_owned()) // 159:W6 — world::launch(env, &slug, n, &token, approve)
}

/// 159 §5.11 `autoresearch.mechanism {op}` — list, show or adopt a mechanism version (W7).
pub async fn mechanism(op: MechanismOp) -> std::result::Result<MechanismInfo, String> {
    let _ = op;
    Err("not built (W7)".to_owned()) // 159:W7 — mechanism::rpc(root, op)
}

#[cfg(test)]
mod tests {
    use runtime::SessionId;

    use crate::tools::{read_draft, write_draft};
    use crate::types::StudySpec;

    /// 151 D6 + addition C: the rail lists readable drafts without a study, newest first;
    /// a discarded draft is gone and a directory without a readable spec.json is skipped.
    #[test]
    fn drafts_skip_started_and_discarded() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for slug in ["older", "started", "newer"] {
            let spec = StudySpec {
                slug: slug.into(),
                goal: format!("goal {slug}"),
                repo: "/x".into(),
                ..Default::default()
            };
            write_draft(root, &spec, SessionId::new(), true, vec![]).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        // Half-written / pre-format directories: no spec.json, or one that does not parse.
        std::fs::create_dir_all(crate::drafts_dir(root, "empty")).unwrap();
        std::fs::create_dir_all(crate::drafts_dir(root, "broken")).unwrap();
        std::fs::write(
            crate::drafts_dir(root, "broken").join("spec.json"),
            b"{not json",
        )
        .unwrap();
        let list = crate::setup::drafts(root, &["started".to_owned()]);
        let ids: Vec<&str> = list.iter().map(|d| d.draft_id.as_str()).collect();
        assert_eq!(ids, ["newer", "older"]);
        assert_eq!(
            (list[0].goal.as_str(), list[0].total, list[0].ready),
            ("goal newer", 9, false)
        );
        crate::setup::discard(root, "newer").unwrap();
        let e = read_draft(root, "newer").unwrap_err().to_string();
        assert!(e.starts_with("no draft `newer`"), "{e}");
        let ids: Vec<String> = crate::setup::drafts(root, &[])
            .into_iter()
            .map(|d| d.draft_id)
            .collect();
        assert_eq!(ids, ["started", "older"]);
        let e = crate::setup::discard(root, "newer")
            .unwrap_err()
            .to_string();
        assert_eq!(e, "no draft `newer`");
    }
}
