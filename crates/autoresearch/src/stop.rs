//! W3 LOOP — stop and pause rules (design doc 140 §8.4), evaluated after
//! every experiment closes, first match wins: user stop, experiments,
//! money, hours/deadline, plateau (explore once, then stop), overfit, crash
//! streak, custody refusals, review flags. [`decide`] is the pure table over
//! a [`View`]; [`evaluate`] builds the view from the ledger.
//!
//! Ledger facts the view reads (all `events` rows the actor appends):
//! `stop_after` (the user's "stop after this"), `explore {after_n}` (the
//! first plateau), `overfit`, `resumed {after_n}` (review flags count from
//! the last resume so a pause fires once per threshold), `custody_refusal {n}`.

use crate::error::Result;
use crate::store::{Store, StudyRecord};
use crate::types::{ExperimentState, PauseReason, StopReason, Verdict};

/// What the rules said.
#[derive(Clone, Debug, PartialEq)]
pub enum Rule {
    /// Keep going.
    Continue,
    /// Next brief in explore mode + one advisor consult (first plateau).
    Explore,
    /// Pause with a reason.
    Pause(PauseReason),
    /// Finalize with a reason.
    Stop(StopReason),
}

/// The stop view: everything §8.4 needs, as numbers.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct View {
    /// The user asked to stop after this experiment.
    pub stop_after: bool,
    /// Experiments closed (every state but open/measuring/measured).
    pub closed: u32,
    /// Study spend so far, USD (the session tree's ledger sum).
    pub spend_usd: f64,
    /// Hours since Start excluding paused time.
    pub hours: f64,
    /// Closed experiments since the last keep (or since Start).
    pub since_keep: u32,
    /// An `explore` event was recorded after the last keep.
    pub explored: bool,
    /// An `overfit` event was recorded after the last keep.
    pub overfit: bool,
    /// Trailing run of `crashed` experiments.
    pub crash_streak: u32,
    /// Trailing run of custody refusals.
    pub custody_refusals: u32,
    /// Review flags (reverted or flagged keeps) since the last resume.
    pub review_flags: u32,
}

/// The §8.4 table over `view` for `spec`'s budgets and `cfg`'s thresholds; first match wins.
pub fn decide(view: &View, spec: &crate::types::StudySpec, cfg: &crate::config::AutoresearchConfig) -> Rule {
    if view.stop_after {
        return Rule::Stop(StopReason::User);
    }
    if view.closed >= spec.budget.experiments {
        return Rule::Stop(StopReason::Experiments);
    }
    if view.spend_usd >= spec.budget.usd {
        return Rule::Stop(StopReason::Budget);
    }
    if view.hours >= spec.budget.hours {
        return Rule::Stop(StopReason::Hours);
    }
    let window = cfg.plateau.window.max(1);
    if view.overfit {
        return if view.explored {
            Rule::Stop(StopReason::Overfit)
        } else {
            Rule::Explore
        };
    }
    if view.since_keep >= window {
        if view.since_keep >= 2 * window && view.explored {
            return Rule::Stop(StopReason::Plateau);
        }
        if !view.explored {
            return Rule::Explore;
        }
    }
    if cfg.pause.crash_streak > 0 && view.crash_streak >= cfg.pause.crash_streak {
        return Rule::Pause(PauseReason::CrashStreak);
    }
    if cfg.pause.custody_refusals > 0 && view.custody_refusals >= cfg.pause.custody_refusals {
        return Rule::Pause(PauseReason::CustodyRefusals);
    }
    if cfg.review.pause_after > 0 && view.review_flags >= cfg.review.pause_after {
        return Rule::Pause(PauseReason::ReviewFlags);
    }
    Rule::Continue
}

/// Is this a custody refusal (as opposed to a duplicate / off-limits one)?
pub fn is_custody_refusal(v: &Verdict) -> bool {
    matches!(v, Verdict::Refused { reason } if reason.starts_with("evaluator changed"))
}

/// Build the [`View`] for `study` from the ledger and `spend_usd`.
pub async fn view(store: &Store, study: &StudyRecord, spend_usd: f64) -> Result<View> {
    let all = store.list_experiments(&study.slug, None, u32::MAX).await?; // newest first
    let closed: Vec<_> = all
        .iter()
        .filter(|e| !matches!(e.state, ExperimentState::Open | ExperimentState::Measuring | ExperimentState::Measured))
        .collect();
    let last_keep_n = closed
        .iter()
        .find(|e| e.state == ExperimentState::Kept)
        .map_or(0, |e| e.n);
    let since_keep = closed.iter().filter(|e| e.n > last_keep_n).count() as u32;
    let crash_streak = closed
        .iter()
        .take_while(|e| e.state == ExperimentState::Crashed)
        .count() as u32;
    let custody_refusals = closed
        .iter()
        .take_while(|e| e.verdict.as_ref().is_some_and(is_custody_refusal))
        .count() as u32;

    let events = store.list_events(&study.slug, None, 10_000).await?; // newest first
    let after_n = |kind: &str| -> Option<u32> {
        events
            .iter()
            .find(|e| e.kind == kind)
            .map(|e| e.payload.get("after_n").and_then(|v| v.as_u64()).unwrap_or(0) as u32)
    };
    // A stop_after is consumed when the study resumes or finalizes; here it is any row after the last resume.
    let last_resume = events.iter().position(|e| e.kind == "resumed");
    let stop_after = events
        .iter()
        .take(last_resume.unwrap_or(events.len()))
        .any(|e| e.kind == "stop_after");
    let explored = after_n("explore").is_some_and(|n| n >= last_keep_n);
    let overfit = after_n("overfit").is_some_and(|n| n >= last_keep_n);
    let resumed_after = after_n("resumed").unwrap_or(0);
    let review_flags = closed
        .iter()
        .filter(|e| e.n > resumed_after)
        .filter(|e| {
            matches!(
                e.review,
                Some(crate::types::ReviewVerdict::Reverted { .. } | crate::types::ReviewVerdict::Flagged { .. })
            )
        })
        .count() as u32;

    Ok(View {
        stop_after,
        closed: closed.len() as u32,
        spend_usd,
        hours: hours_running(study, chrono::Utc::now()),
        since_keep,
        explored,
        overfit,
        crash_streak,
        custody_refusals,
        review_flags,
    })
}

/// Evaluate §8.4 for `study` from the ledger and `spend_usd` (the session tree's ledger sum).
pub async fn evaluate(store: &Store, study: &StudyRecord, cfg: &crate::config::AutoresearchConfig, spend_usd: f64) -> Result<Rule> {
    let v = view(store, study, spend_usd).await?;
    Ok(decide(&v, &study.spec, cfg))
}

/// Hours since Start excluding paused time.
pub fn hours_running(study: &StudyRecord, now: chrono::DateTime<chrono::Utc>) -> f64 {
    let Some(started) = study
        .started_at
        .as_deref()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
    else {
        return 0.0;
    };
    let elapsed_ms = (now - started.with_timezone(&chrono::Utc)).num_milliseconds().max(0) as u64;
    elapsed_ms.saturating_sub(study.paused_ms) as f64 / 3_600_000.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AutoresearchConfig;
    use crate::types::StudySpec;

    fn spec() -> StudySpec {
        StudySpec {
            budget: crate::types::BudgetSpec {
                usd: 20.0,
                per_experiment_usd: 1.5,
                hours: 10.0,
                experiments: 120,
            },
            ..Default::default()
        }
    }

    /// Every §8.4 row fires once, with its reason, in table order.
    #[test]
    fn each_rule_fires_once() {
        let cfg = AutoresearchConfig::default();
        let s = spec();
        let base = View::default();
        assert_eq!(decide(&base, &s, &cfg), Rule::Continue);
        assert_eq!(decide(&View { stop_after: true, ..base.clone() }, &s, &cfg), Rule::Stop(StopReason::User));
        assert_eq!(decide(&View { closed: 120, ..base.clone() }, &s, &cfg), Rule::Stop(StopReason::Experiments));
        assert_eq!(decide(&View { spend_usd: 20.0, ..base.clone() }, &s, &cfg), Rule::Stop(StopReason::Budget));
        assert_eq!(decide(&View { hours: 10.01, ..base.clone() }, &s, &cfg), Rule::Stop(StopReason::Hours));
        // plateau: first window → explore; explored but under 2 windows → continue; 2 windows → stop.
        assert_eq!(decide(&View { since_keep: 12, ..base.clone() }, &s, &cfg), Rule::Explore);
        assert_eq!(decide(&View { since_keep: 18, explored: true, ..base.clone() }, &s, &cfg), Rule::Continue);
        assert_eq!(
            decide(&View { since_keep: 24, explored: true, ..base.clone() }, &s, &cfg),
            Rule::Stop(StopReason::Plateau)
        );
        // overfit is a plateau: explore first, then stop with its own reason.
        assert_eq!(decide(&View { overfit: true, ..base.clone() }, &s, &cfg), Rule::Explore);
        assert_eq!(
            decide(&View { overfit: true, explored: true, ..base.clone() }, &s, &cfg),
            Rule::Stop(StopReason::Overfit)
        );
        assert_eq!(decide(&View { crash_streak: 4, ..base.clone() }, &s, &cfg), Rule::Pause(PauseReason::CrashStreak));
        assert_eq!(decide(&View { crash_streak: 3, ..base.clone() }, &s, &cfg), Rule::Continue);
        assert_eq!(
            decide(&View { custody_refusals: 2, ..base.clone() }, &s, &cfg),
            Rule::Pause(PauseReason::CustodyRefusals)
        );
        assert_eq!(decide(&View { review_flags: 3, ..base.clone() }, &s, &cfg), Rule::Pause(PauseReason::ReviewFlags));
        // Precedence: a stop beats a pause.
        assert_eq!(
            decide(&View { stop_after: true, crash_streak: 9, ..base }, &s, &cfg),
            Rule::Stop(StopReason::User)
        );
    }

    #[test]
    fn hours_exclude_paused_time() {
        let mut study = crate::store::StudyRecord::new(spec(), None);
        let now = chrono::Utc::now();
        study.started_at = Some((now - chrono::Duration::hours(3)).to_rfc3339());
        study.paused_ms = 3_600_000;
        let h = hours_running(&study, now);
        assert!((h - 2.0).abs() < 0.01, "{h}");
        study.started_at = None;
        assert_eq!(hours_running(&study, now), 0.0);
    }
}
