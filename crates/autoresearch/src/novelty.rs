//! W6 — the novelty check at `experiment_run` (design doc 140 §7.3): a
//! normalised patch hash equal to a past discarded experiment's → refused
//! (no sample spent); FTS over past `idea + description + learned` ≥
//! `novelty.threshold` in the same category → `similar` rows, refused only
//! when flagged twice in a row; `force: true` + `reason` overrides. Also marks
//! matching ideas `tried`.
//!
//! Similarity is the overlap coefficient of the candidate idea's tokens with a
//! past row's text (0–1, so `threshold` 0.6 = 60 % of the idea's words recur);
//! FTS5 bm25 picks the candidates, the overlap decides.

use sqlx::Row;

use crate::brief::tokens;
use crate::error::Result;
use crate::store::Store;
use crate::types::{IdeaStatus, RunArgs, SimilarExperiment, Verdict};

/// What the check found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Novelty {
    /// Refusal text, when the run must not spend a sample.
    pub refused: Option<String>,
    /// Similar past experiments (named in the result either way).
    pub similar: Vec<SimilarExperiment>,
}

/// Experiment states that count as dead ends (140 §7.3).
const DEAD_ENDS: &str =
    "('discarded', 'crashed', 'checks_failed', 'rejected_by_review', 'refused')";
/// FTS candidates examined per check.
const CANDIDATES: i64 = 20;

/// `force: true` with a non-empty `reason` (140 §7.3).
pub fn forced(args: &RunArgs) -> bool {
    args.force && args.reason.as_deref().is_some_and(|r| !r.trim().is_empty())
}

/// Check `args` + `patch_hash` against the study's past (140 §7.3 Novelty check).
pub async fn check(
    store: &Store,
    slug: &str,
    args: &RunArgs,
    patch_hash: &str,
    threshold: f64,
    flagged_before: bool,
) -> Result<Novelty> {
    let forced = forced(args);
    let mut out = Novelty::default();

    // (a) The same normalised patch as a past dead end.
    if !patch_hash.is_empty() {
        let dup = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT n, verdict, delta_pct, idea FROM experiments WHERE slug = ?1 AND patch_hash = ?2 \
             AND state IN {DEAD_ENDS} ORDER BY n DESC LIMIT 1"
        )))
        .bind(slug)
        .bind(patch_hash)
        .fetch_optional(store.pool())
        .await?;
        if let Some(r) = dup {
            let s = similar_row(&r)?;
            if !forced {
                out.refused = Some(format!(
                    "duplicate of #{} ({}): the same patch was already measured — pass force: true with a reason to re-measure it",
                    s.n,
                    verdict_words(&s)
                ));
            }
            out.similar.push(s);
            return Ok(out);
        }
    }

    // (b) Similar ideas in the same category.
    let toks = tokens(&args.idea);
    if toks.is_empty() {
        return Ok(out);
    }
    let query = toks
        .iter()
        .map(|t| format!("\"{}\"", t.replace('"', "")))
        .collect::<Vec<_>>()
        .join(" OR ");
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT e.n, e.verdict, e.delta_pct, e.idea, e.description, e.learned \
         FROM experiments_fts f JOIN experiments e ON e.slug = f.slug AND e.n = f.n \
         WHERE f.slug = ?1 AND experiments_fts MATCH ?2 AND e.category = ?3 AND e.state IN {DEAD_ENDS} \
         ORDER BY bm25(experiments_fts) LIMIT {CANDIDATES}"
    )))
    .bind(slug)
    .bind(&query)
    .bind(args.category.trim())
    .fetch_all(store.pool())
    .await?;
    for r in &rows {
        let text = format!(
            "{} {} {}",
            r.try_get::<String, _>("idea")?,
            r.try_get::<String, _>("description")?,
            r.try_get::<String, _>("learned")?
        );
        if overlap(&toks, &tokens(&text)) >= threshold {
            out.similar.push(similar_row(r)?);
        }
    }
    out.similar.sort_by_key(|s| std::cmp::Reverse(s.n));
    if !forced && flagged_before && !out.similar.is_empty() {
        let s = &out.similar[0];
        out.refused = Some(format!(
            "similar to #{} ({}) and the previous experiment was flagged as a duplicate too — say what is different with force: true and a reason",
            s.n,
            verdict_words(s)
        ));
    }
    Ok(out)
}

/// Mark backlog ideas whose text matches `idea` as `tried` in experiment `n`.
pub async fn mark_tried(store: &Store, slug: &str, idea: &str, n: u32) -> Result<Vec<i64>> {
    let toks = tokens(idea);
    if toks.is_empty() {
        return Ok(Vec::new());
    }
    let mut ids = Vec::new();
    for mut i in store.list_ideas(slug, Some(IdeaStatus::Open)).await? {
        let it = tokens(&i.text);
        if overlap(&it, &toks) >= 0.6 {
            i.status = IdeaStatus::Tried;
            if !i.tried_in.contains(&n) {
                i.tried_in.push(n);
            }
            store.update_idea(&i).await?;
            ids.push(i.id);
        }
    }
    Ok(ids)
}

/// Fraction of `a`'s tokens found in `b` (both sorted, deduplicated).
fn overlap(a: &[String], b: &[String]) -> f64 {
    if a.is_empty() {
        return 0.0;
    }
    a.iter().filter(|t| b.binary_search(t).is_ok()).count() as f64 / a.len() as f64
}

fn similar_row(r: &sqlx::sqlite::SqliteRow) -> Result<SimilarExperiment> {
    let verdict: Option<String> = r.try_get("verdict")?;
    let verdict = verdict
        .and_then(|v| serde_json::from_str::<Verdict>(&v).ok())
        .map_or_else(|| "discarded".to_owned(), |v| v.tag().to_owned());
    Ok(SimilarExperiment {
        n: r.try_get::<i64, _>("n")? as u32,
        verdict,
        delta_pct: r.try_get("delta_pct")?,
        idea: r.try_get("idea")?,
    })
}

/// `no gain, −0.3 %` / `crash`.
fn verdict_words(s: &SimilarExperiment) -> String {
    let mut w = s.verdict.replace('_', " ");
    if let Some(d) = s.delta_pct.filter(|d| d.is_finite()) {
        w.push_str(&format!(
            ", {}{:.1} %",
            if d < 0.0 { "−" } else { "+" },
            d.abs()
        ));
    }
    w
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{ExperimentRecord, StudyRecord};
    use crate::types::*;

    async fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(dir.path()).await.unwrap();
        s.create_study(&StudyRecord::new(
            StudySpec {
                slug: "s".into(),
                ..Default::default()
            },
            None,
        ))
        .await
        .unwrap();
        (dir, s)
    }

    async fn past(s: &Store, n: u32, state: ExperimentState, idea: &str, cat: &str, hash: &str) {
        let mut e = ExperimentRecord::open("s", n, 0, "inc");
        e.state = state;
        e.verdict = Some(match state {
            ExperimentState::Kept => Verdict::Improved,
            ExperimentState::Crashed => Verdict::Crash,
            _ => Verdict::NoGain,
        });
        e.idea = idea.into();
        e.category = cat.into();
        e.description = format!("did {idea}");
        e.learned = "nothing".into();
        e.delta_pct = Some(-0.3);
        e.patch_hash = Some(hash.into());
        s.insert_experiment(&e).await.unwrap();
    }

    fn args(idea: &str, cat: &str) -> RunArgs {
        RunArgs {
            idea: idea.into(),
            category: cat.into(),
            hypothesis: "h".into(),
            ..Default::default()
        }
    }

    /// The same normalised patch as a discarded experiment is refused before any sample; `force` + reason lets it through.
    #[tokio::test]
    async fn identical_patch_refused_unless_forced() {
        let (_d, s) = store().await;
        past(
            &s,
            12,
            ExperimentState::Discarded,
            "cache the parsed template",
            "caching",
            "h1",
        )
        .await;
        past(
            &s,
            13,
            ExperimentState::Kept,
            "inline the hot loop",
            "algorithm",
            "h2",
        )
        .await;
        let n = check(&s, "s", &args("something new", "io"), "h1", 0.6, false)
            .await
            .unwrap();
        assert_eq!(
            n.refused.as_deref(),
            Some(
                "duplicate of #12 (no gain, −0.3 %): the same patch was already measured — pass force: true with a reason to re-measure it"
            )
        );
        assert_eq!(n.similar.len(), 1);
        // A kept experiment's hash is not a dead end.
        let n = check(&s, "s", &args("something new", "io"), "h2", 0.6, false)
            .await
            .unwrap();
        assert_eq!(n, Novelty::default());
        // Forced with a reason: proceeds, the duplicate still named.
        let mut a = args("something new", "io");
        a.force = true;
        a.reason = Some("the compiler changed since".into());
        let n = check(&s, "s", &a, "h1", 0.6, false).await.unwrap();
        assert!(n.refused.is_none());
        assert_eq!(n.similar[0].n, 12);
        // Forced without a reason is not forced.
        a.reason = None;
        assert!(
            check(&s, "s", &a, "h1", 0.6, false)
                .await
                .unwrap()
                .refused
                .is_some()
        );
    }

    /// A similar idea in the same category is named but proceeds; flagged twice in a row it is refused; another category never matches.
    #[tokio::test]
    async fn similar_idea_warned_then_refused() {
        let (_d, s) = store().await;
        past(
            &s,
            3,
            ExperimentState::Discarded,
            "cache the parsed template between renders",
            "caching",
            "x",
        )
        .await;
        past(
            &s,
            4,
            ExperimentState::Crashed,
            "memoise the lexer token table",
            "caching",
            "y",
        )
        .await;
        let a = args("cache the parsed template across renders", "caching");
        let n = check(&s, "s", &a, "fresh", 0.6, false).await.unwrap();
        assert!(n.refused.is_none(), "{n:?}");
        assert_eq!(n.similar.iter().map(|x| x.n).collect::<Vec<_>>(), [3]);
        assert_eq!(n.similar[0].verdict, "no_gain");
        let n = check(&s, "s", &a, "fresh", 0.6, true).await.unwrap();
        assert!(
            n.refused.as_deref().unwrap().starts_with(
                "similar to #3 (no gain, −0.3 %) and the previous experiment was flagged"
            ),
            "{n:?}"
        );
        // Same words, another category: not similar.
        let n = check(
            &s,
            "s",
            &args("cache the parsed template across renders", "io"),
            "fresh",
            0.6,
            true,
        )
        .await
        .unwrap();
        assert_eq!(n, Novelty::default());
        // Unrelated idea in the same category: not similar.
        let n = check(
            &s,
            "s",
            &args("use SIMD for whitespace scanning", "caching"),
            "fresh",
            0.6,
            true,
        )
        .await
        .unwrap();
        assert_eq!(n, Novelty::default());
    }

    /// Open ideas whose words the experiment's idea covers are marked tried; others stay open.
    #[tokio::test]
    async fn marks_matching_ideas_tried() {
        let (_d, s) = store().await;
        let a = s
            .insert_idea(&Idea {
                slug: "s".into(),
                text: "cache the parsed template".into(),
                category: "caching".into(),
                ..Default::default()
            })
            .await
            .unwrap();
        let b = s
            .insert_idea(&Idea {
                slug: "s".into(),
                text: "rewrite the scanner with SIMD".into(),
                category: "algorithm".into(),
                ..Default::default()
            })
            .await
            .unwrap();
        let ids = mark_tried(&s, "s", "cache the parsed template between renders", 7)
            .await
            .unwrap();
        assert_eq!(ids, [a]);
        let ia = s.get_idea(a).await.unwrap().unwrap();
        assert_eq!(ia.status, IdeaStatus::Tried);
        assert_eq!(ia.tried_in, [7]);
        assert_eq!(
            s.get_idea(b).await.unwrap().unwrap().status,
            IdeaStatus::Open
        );
        assert!(mark_tried(&s, "s", "", 8).await.unwrap().is_empty());
    }
}
