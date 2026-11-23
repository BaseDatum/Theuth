//! W2 RATCHET — the cross-study knowledge record (design doc 140 §7.3):
//! at finalize one `type: record` with id `records/autoresearch-<repo>-<slug>-<date>`
//! (goal, metric, base → final, kept changesets with deltas, top dead ends,
//! playbook); at setup/Start the most recent record for the same repo seeds
//! the first playbook. Through the knowledge actor's `Handle`, as dreaming
//! writes its records; without the knowledge plugin nothing is written.

use std::path::PathBuf;
use std::sync::Arc;

use kameo::actor::ActorRef;

use crate::error::{Error, Result};
use crate::store::{Store, StudyRecord};
use crate::types::ExperimentState;

/// Tag every record carries.
pub const TAG: &str = "autoresearch";
/// Id prefix.
pub const PREFIX: &str = "records/autoresearch-";
/// Heading of the playbook section inside the body (what [`seed_playbook`] reads back).
const PLAYBOOK_HEADING: &str = "## Playbook";
/// Dead ends listed at most.
const DEAD_ENDS: usize = 15;

/// `[a-z0-9-]` of `s`, runs of other characters collapsed to one `-`.
fn slugify(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

/// The repo's directory name (`/Users/x/Develop/liquid` → `liquid`).
pub fn repo_name(repo: &str) -> String {
    let name = repo
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(repo);
    let s = slugify(name);
    if s.is_empty() { "repo".into() } else { s }
}

/// `records/autoresearch-<repo-name>-<slug>-<date>`.
pub fn record_id(repo: &str, slug: &str, date: &str) -> String {
    format!("{PREFIX}{}-{}-{}", repo_name(repo), slugify(slug), date)
}

async fn knowledge() -> Option<Arc<knowledge::Knowledge>> {
    let k = ActorRef::<knowledge::KnowledgeActor>::lookup(knowledge::KNOWLEDGE_ACTOR).ok()??;
    k.ask(knowledge::Handle).await.ok()
}

/// The record's markdown body (pure over the ledger rows; what [`write`] puts).
pub async fn body(store: &Store, study: &StudyRecord, headline: &str) -> Result<String> {
    let spec = &study.spec;
    let mut b = format!(
        "Goal: {}\n\nMetric: {} ({}, {} is better). Result: {}\n\nBase `{}` → final `{}` on branch `autoresearch/{}`.\n",
        spec.goal.trim(),
        spec.metric.name,
        spec.metric.unit,
        match spec.metric.direction {
            crate::types::Direction::Lower => "lower",
            crate::types::Direction::Higher => "higher",
        },
        headline.trim(),
        study.base_sha.as_deref().unwrap_or("?"),
        study.incumbent_sha.as_deref().unwrap_or("?"),
        study.slug
    );
    let changesets = crate::finalize::changesets(store, &study.slug).await?;
    let rows = store.list_experiments(&study.slug, None, u32::MAX).await?;
    b.push_str("\n## Kept\n\n");
    if changesets.is_empty() {
        let kept: Vec<_> = rows
            .iter()
            .filter(|e| e.state == ExperimentState::Kept)
            .collect();
        if kept.is_empty() {
            b.push_str("Nothing kept.\n");
        }
        for e in kept.iter().rev() {
            b.push_str(&format!(
                "- #{} [{}] {} ({})\n",
                e.n,
                e.category,
                e.description.trim(),
                e.delta_pct
                    .map(|d| format!("{d:+.1} %"))
                    .unwrap_or_default()
            ));
        }
    }
    for c in &changesets {
        b.push_str(&format!(
            "- {} `{}`: alone {}, together {}; files {}; experiments {}\n",
            c.name,
            c.branch,
            c.delta_alone_pct
                .map(|d| format!("{d:+.1} %"))
                .unwrap_or_else(|| "not measured".into()),
            c.delta_together_pct
                .map(|d| format!("{d:+.1} %"))
                .unwrap_or_else(|| "—".into()),
            c.files.join(", "),
            c.experiments
                .iter()
                .map(|n| format!("#{n}"))
                .collect::<Vec<_>>()
                .join(" ")
        ));
    }
    b.push_str("\n## Dead ends\n\n");
    let dead: Vec<_> = rows
        .iter()
        .filter(|e| {
            matches!(
                e.state,
                ExperimentState::Discarded
                    | ExperimentState::Crashed
                    | ExperimentState::ChecksFailed
                    | ExperimentState::RejectedByReview
            )
        })
        .take(DEAD_ENDS)
        .collect();
    if dead.is_empty() {
        b.push_str("None.\n");
    }
    for e in dead {
        let what = if e.idea.trim().is_empty() {
            e.description.trim()
        } else {
            e.idea.trim()
        };
        b.push_str(&format!(
            "- #{} [{}] {} — {}{}\n",
            e.n,
            e.category,
            e.verdict.as_ref().map(|v| v.tag()).unwrap_or("discarded"),
            what,
            if e.learned.trim().is_empty() {
                String::new()
            } else {
                format!(" (learned: {})", e.learned.trim())
            }
        ));
    }
    b.push_str(&format!("\n{PLAYBOOK_HEADING}\n\n"));
    b.push_str(if study.playbook.trim().is_empty() {
        "(empty)"
    } else {
        study.playbook.trim()
    });
    b.push('\n');
    if b.len() > knowledge::BODY_MAX {
        let cut = b.floor_char_boundary(knowledge::BODY_MAX - 32);
        b.truncate(cut);
        b.push_str("\n… (truncated)\n");
    }
    Ok(b)
}

/// [`write_record`] — the `Backend::record` seam's shape (`rt` unused).
pub async fn write(
    store: &Store,
    study: &StudyRecord,
    headline: &str,
    rt: &runtime::Runtime,
) -> Result<String> {
    let _ = rt;
    write_record(store, study, headline).await
}

/// Write the record for a finished study; returns its knowledge id. Written
/// once, by finalize's `record` step (R-62).
pub async fn write_record(store: &Store, study: &StudyRecord, headline: &str) -> Result<String> {
    let date = study
        .finished_at
        .as_deref()
        .and_then(|s| s.get(..10))
        .map(str::to_owned)
        .unwrap_or_else(|| chrono::Utc::now().format("%Y-%m-%d").to_string());
    let id = record_id(&study.repo, &study.slug, &date);
    let body = body(store, study, headline).await?;
    let Some(k) = knowledge().await else {
        return Err(Error::refused(
            "knowledge plugin not running; record not written",
        ));
    };
    let k = match study.origin {
        Some(origin) => k.for_session(origin).await.map_err(Error::Other)?,
        None => k,
    };
    let put = knowledge::Put {
        id: id.clone(),
        type_: "record".into(),
        title: format!("Autoresearch {}: {}", study.slug, headline.trim()),
        description: Some(format!(
            "Study `{}` on {}: {}",
            study.slug,
            repo_name(&study.repo),
            study.spec.goal.trim()
        )),
        tags: vec![TAG.into(), slugify(&study.slug), repo_name(&study.repo)],
        scope: Some(PathBuf::from(&study.repo)),
        body,
        status: Some(knowledge::Status::Tidy),
        ..Default::default()
    };
    k.put(put, knowledge::Source::Harvest)
        .await
        .map_err(Error::Other)?;
    Ok(id)
}

/// The playbook section of a record body (`None` when empty or absent).
pub fn playbook_of(body: &str) -> Option<String> {
    let rest = body.split_once(PLAYBOOK_HEADING)?.1;
    let text = rest.split("\n## ").next().unwrap_or(rest).trim();
    (!text.is_empty() && text != "(empty)").then(|| text.to_owned())
}

/// The playbook of the most recent record for `repo`, if any.
pub async fn seed_playbook(
    repo: &str,
    origin: Option<runtime::SessionId>,
    rt: &runtime::Runtime,
) -> Result<Option<String>> {
    let _ = rt;
    let Some(k) = knowledge().await else {
        return Ok(None);
    };
    let k = match origin {
        Some(origin) => k.for_session(origin).await.map_err(Error::Other)?,
        None => k,
    };
    let prefix = format!("{PREFIX}{}-", repo_name(repo));
    let mut metas = k
        .list(Some(&prefix), Some("record"), Some(TAG), 1000)
        .await
        .map_err(Error::Other)?;
    metas.retain(|m| m.merged_into.is_none() && m.superseded_by.is_none());
    // Ids end in `YYYY-MM-DD`, so the newest record sorts last; ties by timestamp.
    let date = |id: &str| id[id.len().saturating_sub(10)..].to_owned();
    metas.sort_by(|a, b| {
        date(&a.id)
            .cmp(&date(&b.id))
            .then_with(|| a.timestamp.cmp(&b.timestamp))
    });
    let Some(newest) = metas.last() else {
        return Ok(None);
    };
    let concept = k.get(&newest.id).await.map_err(Error::Other)?;
    Ok(playbook_of(&concept.text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::ExperimentRecord;
    use crate::types::{StudySpec, Verdict};

    #[tokio::test]
    async fn study_records_use_the_origin_sessions_bundle() {
        use runtime::Plugin;
        struct Origin(runtime::SessionId);
        #[async_trait::async_trait]
        impl knowledge::Scoper for Origin {
            fn scope_for(&self, _: runtime::SessionId) -> Option<knowledge::AgentScope> {
                None
            }
            async fn bundle_for(
                &self,
                s: runtime::SessionId,
            ) -> std::result::Result<Option<String>, String> {
                Ok((s == self.0).then(|| "researcher".into()))
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let rt = runtime::Runtime::new(figment::Figment::new(), dir.path().to_path_buf());
        Box::new(knowledge::KnowledgePlugin)
            .start(&rt)
            .await
            .unwrap();
        let main = knowledge().await.unwrap();
        let origin = runtime::SessionId::new();
        main.set_scoper(Arc::new(Origin(origin)));
        let store = Store::open(dir.path()).await.unwrap();
        let study = StudyRecord::new(
            StudySpec {
                slug: "isolation".into(),
                repo: "/repo/project".into(),
                ..Default::default()
            },
            Some(origin),
        );
        store.create_study(&study).await.unwrap();
        let id = write_record(&store, &study, "kept one change")
            .await
            .unwrap();
        assert!(main.get(&id).await.is_err());
        assert_eq!(
            main.for_session(origin)
                .await
                .unwrap()
                .get(&id)
                .await
                .unwrap()
                .meta
                .owner,
            "agent:researcher"
        );
        rt.shutdown.cancel();
        let actor = ActorRef::<knowledge::KnowledgeActor>::lookup(knowledge::KNOWLEDGE_ACTOR)
            .unwrap()
            .unwrap();
        actor.stop_gracefully().await.unwrap();
        actor.wait_for_shutdown().await;
    }

    #[test]
    fn ids_and_playbook_section() {
        assert_eq!(
            record_id("/Users/x/Develop/My Liquid/", "sort-speed", "2026-10-08"),
            "records/autoresearch-my-liquid-sort-speed-2026-10-08"
        );
        assert_eq!(repo_name(""), "repo");
        assert_eq!(
            playbook_of("x\n## Playbook\n\nbuild with make\nhot path: parse\n"),
            Some("build with make\nhot path: parse".into())
        );
        assert_eq!(playbook_of("x\n## Playbook\n\n(empty)\n"), None);
        assert_eq!(playbook_of("no section"), None);
        assert_eq!(
            playbook_of("## Playbook\n\ntext\n## Later\nmore"),
            Some("text".into())
        );
    }

    /// The body carries goal, result, kept, dead ends with lessons, and the playbook; stays under the cap.
    #[tokio::test]
    async fn body_from_the_ledger() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).await.unwrap();
        let mut study = StudyRecord::new(
            StudySpec {
                slug: "s".into(),
                goal: "faster parse".into(),
                repo: "/r/liquid".into(),
                ..Default::default()
            },
            None,
        );
        study.base_sha = Some("b".into());
        study.incumbent_sha = Some("f".into());
        study.playbook = "x".repeat(20_000);
        store.create_study(&study).await.unwrap();
        let mut e = ExperimentRecord::open("s", 1, 0, "b");
        e.state = crate::types::ExperimentState::Kept;
        e.description = "unrolled loop".into();
        e.delta_pct = Some(-4.0);
        store.insert_experiment(&e).await.unwrap();
        let mut e = ExperimentRecord::open("s", 2, 0, "f");
        e.state = crate::types::ExperimentState::Crashed;
        e.verdict = Some(Verdict::Crash);
        e.idea = "SIMD".into();
        e.learned = "no target feature".into();
        store.insert_experiment(&e).await.unwrap();
        let b = body(&store, &study, "t 10 → 9 (−10 %)").await.unwrap();
        assert!(b.contains("Goal: faster parse") && b.contains("Result: t 10 → 9"));
        assert!(b.contains("- #1 [] unrolled loop (-4.0 %)"), "{b}");
        assert!(
            b.contains("- #2 [] crash — SIMD (learned: no target feature)"),
            "{b}"
        );
        assert!(b.contains("## Playbook\n\nxxx"));
        assert!(b.len() <= knowledge::BODY_MAX, "{}", b.len());
        assert!(b.ends_with("(truncated)\n"));
    }
}
