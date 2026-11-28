//! Host-side sustained Deck trigger validation, state machine, and routine dispatch.
//!
//! Security & correctness invariants:
//! - Triggers are declared in `ScriptRevision.triggers`, bound into `revision_sha256` (v2),
//!   and reviewed/approved by the human owner.
//! - HTML sections can never invoke `deck_metric` or fire triggers.
//! - Evaluation is purely host-side after the `(run_epoch, script_rev)` fence in `DeckOwner`.
//! - Sustained duration (`sustain_secs`) is ANDed with `min_samples` and requires a confirming
//!   matching sample at or after `first_match + sustain_secs` (never fires on silence alone).
//! - Dispatch targets only existing owner-approved routines via `schedule::FireTrigger` (never
//!   `FireWith` raw payload injection or `forced = true`), preserving `enabled`, `until`,
//!   `max_runs`, `approvals`, and `cap`.

use std::collections::HashSet;

use anyhow::Result;
use chrono::Utc;
use serde_json::{Value, json};

use crate::model::{
    MAX_METRIC_NAME_BYTES, MAX_ROUTINE_NAME_BYTES, MAX_TRIGGER_COOLDOWN_SECS, MAX_TRIGGER_ID_BYTES,
    MAX_TRIGGER_MIN_SAMPLES, MAX_TRIGGER_SUSTAIN_SECS, MAX_TRIGGERS_PER_REVISION, MetricSample,
    MetricValue, TriggerOp, TriggerResetMode, TriggerSpec, canonical_json_string,
};
use crate::store::sha256_hex;

/// Persisted per-(deck_id, trigger_id) host evaluation state.
/// Survives restarts and `run_epoch` changes as long as `(script_rev, spec_sha256)` match;
/// resets automatically when the approved script revision or trigger spec changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TriggerState {
    pub deck_id: String,
    pub trigger_id: String,
    pub script_rev: u64,
    pub spec_sha256: String,
    pub first_match_at_ms: Option<i64>,
    pub consecutive_matches: u32,
    pub latched: bool,
    pub last_fired_at_ms: Option<i64>,
    pub episode_seq: u64,
}

impl TriggerState {
    pub fn fresh(deck_id: &str, trigger_id: &str, script_rev: u64, spec_sha256: String) -> Self {
        Self {
            deck_id: deck_id.to_owned(),
            trigger_id: trigger_id.to_owned(),
            script_rev,
            spec_sha256,
            first_match_at_ms: None,
            consecutive_matches: 0,
            latched: false,
            last_fired_at_ms: None,
            episode_seq: 0,
        }
    }

    /// Reset all condition/latch/cooldown state if the revision or trigger spec changed.
    pub fn reset_if_stale(&mut self, script_rev: u64, spec_sha256: &str) -> bool {
        if self.script_rev != script_rev || self.spec_sha256 != spec_sha256 {
            self.script_rev = script_rev;
            self.spec_sha256 = spec_sha256.to_owned();
            self.first_match_at_ms = None;
            self.consecutive_matches = 0;
            self.latched = false;
            self.last_fired_at_ms = None;
            self.episode_seq = 0;
            true
        } else {
            false
        }
    }
}

/// Deterministic decision returned by evaluating one metric sample against a [`TriggerSpec`].
#[derive(Clone, Debug, PartialEq)]
pub enum TriggerDecision {
    /// Condition not yet satisfied, in cooldown, or still latched.
    Hold,
    /// Condition cleared / streak reset / latch released.
    Cleared,
    /// Condition satisfied (samples AND sustain AND cooldown) and ready to dispatch.
    Fire(TriggerFirePlan),
}

/// Ready-to-dispatch trigger firing plan with restart-safe dedup key and post-fire state transition.
#[derive(Clone, Debug, PartialEq)]
pub struct TriggerFirePlan {
    pub dedup_key: String,
    pub first_match_at_ms: i64,
    pub fired_at_ms: i64,
    pub sustained_ms: i64,
    pub consecutive_matches: u32,
    pub next_episode_seq: u64,
}

/// Canonicalize a trigger spec (`-0.0` normalized to `0.0`) so digests and storage are deterministic.
pub fn normalize_trigger(spec: &TriggerSpec) -> TriggerSpec {
    let mut out = spec.clone();
    out.threshold = out.threshold.normalized();
    if let Some(r) = out.reset_threshold
        && r == 0.0
    {
        out.reset_threshold = Some(0.0);
    }
    out
}

/// Canonicalize a slice of triggers.
pub fn normalize_triggers(triggers: &[TriggerSpec]) -> Vec<TriggerSpec> {
    triggers.iter().map(normalize_trigger).collect()
}

/// Canonical SHA-256 digest of a single trigger specification.
pub fn trigger_spec_sha256(spec: &TriggerSpec) -> String {
    let norm = normalize_trigger(spec);
    let val = serde_json::to_value(&norm).unwrap_or(Value::Null);
    sha256_hex(&canonical_json_string(&val))
}

/// Restart-safe dedup key independent of `run_epoch`, tied to the specific revision, spec,
/// episode start timestamp, and episode sequence.
pub fn compute_dedup_key(
    deck_id: &str,
    trigger_id: &str,
    script_rev: u64,
    spec_sha256: &str,
    first_match_at_ms: i64,
    episode_seq: u64,
) -> String {
    sha256_hex(&format!(
        "v1:{deck_id}:{trigger_id}:{script_rev}:{spec_sha256}:{first_match_at_ms}:{episode_seq}"
    ))
}

fn is_safe_trigger_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_TRIGGER_ID_BYTES
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}

pub fn is_safe_metric_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_METRIC_NAME_BYTES
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b':' | b'/'))
}

/// Validate safety caps, finite numbers, operator compatibility, hysteresis direction, and unique IDs.
pub fn validate_triggers(triggers: &[TriggerSpec]) -> Result<()> {
    if triggers.len() > MAX_TRIGGERS_PER_REVISION {
        anyhow::bail!(
            "triggers count ({}) exceeds cap ({MAX_TRIGGERS_PER_REVISION})",
            triggers.len()
        );
    }
    let mut seen_ids = HashSet::with_capacity(triggers.len());
    for t in triggers {
        if t.id.trim() != t.id || !is_safe_trigger_id(&t.id) {
            anyhow::bail!(
                "trigger id {:?} must be 1..={MAX_TRIGGER_ID_BYTES} ASCII chars [A-Za-z0-9_.-]",
                t.id
            );
        }
        if !seen_ids.insert(t.id.as_str()) {
            anyhow::bail!("duplicate trigger id `{}`", t.id);
        }
        if t.metric.trim() != t.metric || !is_safe_metric_name(&t.metric) {
            anyhow::bail!(
                "trigger `{}` metric name {:?} must be 1..={MAX_METRIC_NAME_BYTES} ASCII chars [A-Za-z0-9_.:/-]",
                t.id,
                t.metric
            );
        }
        if t.routine.trim() != t.routine
            || t.routine.len() > MAX_ROUTINE_NAME_BYTES
            || !schedule::file::valid_name(&t.routine)
        {
            anyhow::bail!(
                "trigger `{}` routine name {:?} is invalid (must match [a-z0-9][a-z0-9-]*, <= {MAX_ROUTINE_NAME_BYTES} chars)",
                t.id,
                t.routine
            );
        }
        if !t.threshold.is_finite() {
            anyhow::bail!(
                "trigger `{}` threshold must be a finite number or bool",
                t.id
            );
        }
        match t.threshold {
            MetricValue::Bool(_) => {
                if !matches!(t.op, TriggerOp::Eq | TriggerOp::Ne) {
                    anyhow::bail!(
                        "trigger `{}` boolean threshold only supports `eq` or `ne`",
                        t.id
                    );
                }
                if t.reset_threshold.is_some() {
                    anyhow::bail!(
                        "trigger `{}` reset_threshold is only supported for numeric ordering triggers",
                        t.id
                    );
                }
            }
            MetricValue::Number(th) => {
                if let Some(r) = t.reset_threshold {
                    if !r.is_finite() {
                        anyhow::bail!("trigger `{}` reset_threshold must be finite", t.id);
                    }
                    match t.op {
                        TriggerOp::Gt | TriggerOp::Gte => {
                            if r > th {
                                anyhow::bail!(
                                    "trigger `{}` reset_threshold ({r}) must be <= threshold ({th}) for `{:?}`",
                                    t.id,
                                    t.op
                                );
                            }
                        }
                        TriggerOp::Lt | TriggerOp::Lte => {
                            if r < th {
                                anyhow::bail!(
                                    "trigger `{}` reset_threshold ({r}) must be >= threshold ({th}) for `{:?}`",
                                    t.id,
                                    t.op
                                );
                            }
                        }
                        TriggerOp::Eq | TriggerOp::Ne => {
                            anyhow::bail!(
                                "trigger `{}` reset_threshold is only supported for ordering ops (gt/gte/lt/lte)",
                                t.id
                            );
                        }
                    }
                }
            }
        }
        if t.min_samples == 0 || t.min_samples > MAX_TRIGGER_MIN_SAMPLES {
            anyhow::bail!(
                "trigger `{}` min_samples ({}) must be in 1..={MAX_TRIGGER_MIN_SAMPLES}",
                t.id,
                t.min_samples
            );
        }
        if t.sustain_secs > MAX_TRIGGER_SUSTAIN_SECS {
            anyhow::bail!(
                "trigger `{}` sustain_secs ({}) exceeds cap ({MAX_TRIGGER_SUSTAIN_SECS})",
                t.id,
                t.sustain_secs
            );
        }
        if t.cooldown_secs > MAX_TRIGGER_COOLDOWN_SECS {
            anyhow::bail!(
                "trigger `{}` cooldown_secs ({}) exceeds cap ({MAX_TRIGGER_COOLDOWN_SECS})",
                t.id,
                t.cooldown_secs
            );
        }
    }
    Ok(())
}

/// Validate that every routine referenced by `triggers` currently exists (and parses cleanly)
/// in the live `Schedule` actor. Called at both script proposal and script approval.
pub async fn verify_routines_exist(triggers: &[TriggerSpec]) -> Result<(), String> {
    if triggers.is_empty() {
        return Ok(());
    }
    let Some(sched) =
        kameo::actor::ActorRef::<schedule::Schedule>::lookup(schedule::SCHEDULE_ACTOR)
            .ok()
            .flatten()
    else {
        return Err(
            "schedule plugin is not running; cannot validate Deck trigger target routines".into(),
        );
    };
    let routines = sched
        .ask(schedule::ListSchedules)
        .send()
        .await
        .map_err(|e| format!("failed to query existing routines: {e}"))?;
    for t in triggers {
        let Some(info) = routines.iter().find(|r| r.name == t.routine) else {
            return Err(format!(
                "trigger `{}` references missing routine `{}` (create the routine first via routine_draft)",
                t.id, t.routine
            ));
        };
        if let Some(err) = &info.error {
            return Err(format!(
                "trigger `{}` references invalid routine `{}`: {err}",
                t.id, t.routine
            ));
        }
    }
    Ok(())
}

/// Evaluate whether `value` satisfies `spec.op` against `spec.threshold`.
pub fn matches_condition(spec: &TriggerSpec, value: MetricValue) -> bool {
    match (value, spec.threshold) {
        (MetricValue::Number(v), MetricValue::Number(t)) if v.is_finite() && t.is_finite() => {
            match spec.op {
                TriggerOp::Gt => v > t,
                TriggerOp::Gte => v >= t,
                TriggerOp::Lt => v < t,
                TriggerOp::Lte => v <= t,
                TriggerOp::Eq => v == t,
                TriggerOp::Ne => v != t,
            }
        }
        (MetricValue::Bool(v), MetricValue::Bool(t)) => match spec.op {
            TriggerOp::Eq => v == t,
            TriggerOp::Ne => v != t,
            _ => false,
        },
        _ => false,
    }
}

/// Evaluate whether `value` clears a latched trigger (respecting optional `reset_threshold` hysteresis).
pub fn matches_clear(spec: &TriggerSpec, value: MetricValue) -> bool {
    match (value, spec.threshold, spec.reset_threshold) {
        (MetricValue::Number(v), MetricValue::Number(t), Some(r))
            if v.is_finite() && t.is_finite() && r.is_finite() =>
        {
            match spec.op {
                TriggerOp::Gt => v <= r,
                TriggerOp::Gte => {
                    if r == t {
                        v < r
                    } else {
                        v <= r
                    }
                }
                TriggerOp::Lt => v >= r,
                TriggerOp::Lte => {
                    if r == t {
                        v > r
                    } else {
                        v >= r
                    }
                }
                TriggerOp::Eq | TriggerOp::Ne => !matches_condition(spec, value),
            }
        }
        (MetricValue::Number(v), MetricValue::Number(t), None)
            if v.is_finite() && t.is_finite() =>
        {
            !matches_condition(spec, value)
        }
        (MetricValue::Bool(_), MetricValue::Bool(_), _) => !matches_condition(spec, value),
        _ => false,
    }
}

/// Pure host-side evaluation of one sample against `spec` and `state`.
///
/// Important: When `TriggerDecision::Fire(plan)` is returned, the caller MUST call
/// [`commit_fire_state`] only after the fire is either accepted by `schedule::FireTrigger`
/// or proven already recorded in `deck_trigger_fires`. If dispatch fails (e.g. routine disabled
/// or busy), the caller does NOT latch/advance `episode_seq` so a later confirming sample can fire.
pub fn evaluate_sample(
    spec: &TriggerSpec,
    script_rev: u64,
    spec_sha256: &str,
    state: &mut TriggerState,
    sample: &MetricSample,
) -> TriggerDecision {
    state.reset_if_stale(script_rev, spec_sha256);

    let now_ms = sample.observed_at_ms;
    let matched = matches_condition(spec, sample.value);

    // If latched in ReArmOnClear mode, stay latched until metric explicitly clears (past hysteresis).
    if state.latched {
        state.first_match_at_ms = None;
        state.consecutive_matches = 0;
        if matches_clear(spec, sample.value) {
            state.latched = false;
            return TriggerDecision::Cleared;
        }
        return TriggerDecision::Hold;
    }

    if !matched {
        let had_streak = state.consecutive_matches > 0 || state.first_match_at_ms.is_some();
        state.first_match_at_ms = None;
        state.consecutive_matches = 0;
        return if had_streak {
            TriggerDecision::Cleared
        } else {
            TriggerDecision::Hold
        };
    }

    // Matching sample! Anchor or preserve first_match_at_ms with backward-clock protection.
    let first_match = match state.first_match_at_ms {
        None => {
            state.first_match_at_ms = Some(now_ms);
            now_ms
        }
        Some(prev) if now_ms < prev => {
            // Backward wall-clock jump: re-anchor to now_ms so elapsed is 0 and we never underflow
            // or fire prematurely.
            state.first_match_at_ms = Some(now_ms);
            now_ms
        }
        Some(prev) => prev,
    };
    state.consecutive_matches = state.consecutive_matches.saturating_add(1);

    let required_samples = spec.min_samples.max(1);
    let sustain_ms = i64::try_from(spec.sustain_secs)
        .unwrap_or(i64::MAX / 1000)
        .saturating_mul(1000);
    let elapsed_ms = now_ms.saturating_sub(first_match).max(0);

    // Both consecutive sample count AND wall-clock sustain duration must be met on this confirming sample.
    if state.consecutive_matches < required_samples || elapsed_ms < sustain_ms {
        return TriggerDecision::Hold;
    }

    // Cooldown check (also safe against backward wall-clock jumps).
    if spec.cooldown_secs > 0
        && let Some(last_fired) = state.last_fired_at_ms
    {
        let cooldown_ms = i64::try_from(spec.cooldown_secs)
            .unwrap_or(i64::MAX / 1000)
            .saturating_mul(1000);
        if now_ms < last_fired || now_ms.saturating_sub(last_fired) < cooldown_ms {
            return TriggerDecision::Hold;
        }
    }

    let next_episode_seq = state.episode_seq.saturating_add(1);
    let dedup_key = compute_dedup_key(
        &state.deck_id,
        &spec.id,
        script_rev,
        spec_sha256,
        first_match,
        next_episode_seq,
    );
    TriggerDecision::Fire(TriggerFirePlan {
        dedup_key,
        first_match_at_ms: first_match,
        fired_at_ms: now_ms,
        sustained_ms: elapsed_ms,
        consecutive_matches: state.consecutive_matches,
        next_episode_seq,
    })
}

/// Apply post-fire state transition after a trigger fire has succeeded or been deduplicated.
pub fn commit_fire_state(spec: &TriggerSpec, state: &mut TriggerState, plan: &TriggerFirePlan) {
    state.last_fired_at_ms = Some(plan.fired_at_ms);
    state.episode_seq = plan.next_episode_seq;
    state.first_match_at_ms = None;
    state.consecutive_matches = 0;
    state.latched = matches!(spec.reset_mode, TriggerResetMode::ReArmOnClear);
}

/// Handle a step/collection gap where a trigger's metric was not reported.
/// Breaks any in-progress consecutive streak without unlatching an already-latched trigger.
pub fn mark_missing_metric(script_rev: u64, spec_sha256: &str, state: &mut TriggerState) -> bool {
    let stale_reset = state.reset_if_stale(script_rev, spec_sha256);
    let had_streak = state.first_match_at_ms.is_some() || state.consecutive_matches > 0;
    state.first_match_at_ms = None;
    state.consecutive_matches = 0;
    stale_reset || had_streak
}

/// Build the fixed typed JSON [`schedule::TriggerEventPayload`] for a Deck trigger firing.
pub fn build_trigger_event(
    deck_id: &str,
    deck_title: &str,
    script_rev: u64,
    spec: &TriggerSpec,
    sample: &MetricSample,
    plan: &TriggerFirePlan,
) -> schedule::TriggerEventPayload {
    let mut meta = sample.meta.clone().unwrap_or(Value::Null);
    tools::secrets::redact_value(&mut meta);
    let redacted_title = tools::secrets::redact(deck_title).into_owned();
    let payload = json!({
        "kind": "deck_trigger",
        "deck_id": deck_id,
        "deck_title": redacted_title,
        "script_rev": script_rev,
        "trigger_id": spec.id,
        "metric": spec.metric,
        "op": spec.op,
        "threshold": spec.threshold.to_json(),
        "value": sample.value.to_json(),
        "consecutive_matches": plan.consecutive_matches,
        "sustained_secs": (plan.sustained_ms as f64) / 1000.0,
        "first_match_at_ms": plan.first_match_at_ms,
        "fired_at_ms": plan.fired_at_ms,
        "fired_at": Utc::now().to_rfc3339(),
        "meta": meta,
    });
    schedule::TriggerEventPayload {
        source: "deck".to_owned(),
        source_id: deck_id.to_owned(),
        trigger_id: spec.id.clone(),
        payload,
    }
}

/// Dispatch a trigger firing to an existing owner-approved routine via [`schedule::FireTrigger`].
pub async fn dispatch_trigger(
    routine: &str,
    event: schedule::TriggerEventPayload,
) -> Result<schedule::TriggerTarget, String> {
    let Some(sched) =
        kameo::actor::ActorRef::<schedule::Schedule>::lookup(schedule::SCHEDULE_ACTOR)
            .ok()
            .flatten()
    else {
        return Err("schedule plugin is not running".into());
    };
    match sched
        .ask(schedule::FireTrigger {
            name: routine.to_owned(),
            event,
        })
        .send()
        .await
    {
        Ok(res) => Ok(res),
        Err(kameo::error::SendError::HandlerError(msg)) => Err(msg),
        Err(e) => Err(format!("schedule dispatch error: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(name: &str, value: MetricValue, t_ms: i64) -> MetricSample {
        MetricSample {
            name: name.to_owned(),
            value,
            meta: None,
            observed_at_ms: t_ms,
        }
    }

    fn num_trigger(
        op: TriggerOp,
        threshold: f64,
        reset_threshold: Option<f64>,
        min_samples: u32,
        sustain_secs: u64,
        cooldown_secs: u64,
        reset_mode: TriggerResetMode,
    ) -> TriggerSpec {
        TriggerSpec {
            id: "t1".into(),
            metric: "cpu".into(),
            op,
            threshold: MetricValue::Number(threshold),
            reset_threshold,
            min_samples,
            sustain_secs,
            cooldown_secs,
            reset_mode,
            routine: "ops-alert".into(),
        }
    }

    #[test]
    fn ops_numeric_and_bool_semantics() {
        let cases = [
            (TriggerOp::Gt, 80.0, 80.1, 80.0),
            (TriggerOp::Gte, 80.0, 80.0, 79.9),
            (TriggerOp::Lt, 10.0, 9.9, 10.0),
            (TriggerOp::Lte, 10.0, 10.0, 10.1),
            (TriggerOp::Eq, 42.0, 42.0, 41.0),
            (TriggerOp::Ne, 0.0, 1.0, 0.0),
        ];
        for (op, th, yes, no) in cases {
            let spec = num_trigger(op, th, None, 1, 0, 0, TriggerResetMode::ReArmOnClear);
            assert!(matches_condition(&spec, MetricValue::Number(yes)), "{op:?}");
            assert!(!matches_condition(&spec, MetricValue::Number(no)), "{op:?}");
        }

        let b_eq = TriggerSpec {
            id: "down".into(),
            metric: "healthy".into(),
            op: TriggerOp::Eq,
            threshold: MetricValue::Bool(false),
            reset_threshold: None,
            min_samples: 1,
            sustain_secs: 0,
            cooldown_secs: 0,
            reset_mode: TriggerResetMode::ReArmOnClear,
            routine: "ops-alert".into(),
        };
        assert!(matches_condition(&b_eq, MetricValue::Bool(false)));
        assert!(!matches_condition(&b_eq, MetricValue::Bool(true)));
        assert!(!matches_condition(&b_eq, MetricValue::Number(0.0)));

        let b_ne = TriggerSpec {
            op: TriggerOp::Ne,
            ..b_eq
        };
        assert!(matches_condition(&b_ne, MetricValue::Bool(true)));
        assert!(!matches_condition(&b_ne, MetricValue::Bool(false)));
    }

    #[test]
    fn validation_enforces_caps_finite_numbers_and_hysteresis_direction() {
        // Non-finite threshold rejected.
        let bad_nan = num_trigger(
            TriggerOp::Gt,
            f64::NAN,
            None,
            1,
            0,
            0,
            TriggerResetMode::ReArmOnClear,
        );
        assert!(validate_triggers(&[bad_nan]).is_err());

        // Ordering op on bool rejected.
        let bad_bool_op = TriggerSpec {
            id: "b".into(),
            metric: "ok".into(),
            op: TriggerOp::Gt,
            threshold: MetricValue::Bool(true),
            reset_threshold: None,
            min_samples: 1,
            sustain_secs: 0,
            cooldown_secs: 0,
            reset_mode: TriggerResetMode::ReArmOnClear,
            routine: "ops-alert".into(),
        };
        assert!(validate_triggers(&[bad_bool_op]).is_err());

        // Wrong-side hysteresis rejected.
        let bad_hyst = num_trigger(
            TriggerOp::Gt,
            80.0,
            Some(90.0),
            1,
            0,
            0,
            TriggerResetMode::ReArmOnClear,
        );
        assert!(validate_triggers(&[bad_hyst]).is_err());

        // Duplicate id rejected.
        let ok = num_trigger(
            TriggerOp::Gt,
            80.0,
            Some(70.0),
            2,
            30,
            60,
            TriggerResetMode::ReArmOnClear,
        );
        assert!(validate_triggers(&[ok.clone()]).is_ok());
        assert!(validate_triggers(&[ok.clone(), ok]).is_err());

        // Count cap enforced.
        let too_many: Vec<TriggerSpec> = (0..=MAX_TRIGGERS_PER_REVISION)
            .map(|i| TriggerSpec {
                id: format!("t{i}"),
                ..num_trigger(
                    TriggerOp::Gt,
                    80.0,
                    None,
                    1,
                    0,
                    0,
                    TriggerResetMode::ReArmOnClear,
                )
            })
            .collect();
        assert!(validate_triggers(&too_many).is_err());
    }

    #[test]
    fn min_samples_and_sustain_are_anded_and_require_confirming_sample() {
        let spec = num_trigger(
            TriggerOp::Gt,
            80.0,
            None,
            3,
            10,
            0,
            TriggerResetMode::ReArmOnClear,
        );
        let sha = trigger_spec_sha256(&spec);
        let mut st = TriggerState::fresh("d1", &spec.id, 1, sha.clone());

        // Sample 1 at t=0: samples=1<3, elapsed=0<10s -> Hold
        assert_eq!(
            evaluate_sample(
                &spec,
                1,
                &sha,
                &mut st,
                &sample("cpu", MetricValue::Number(90.0), 0)
            ),
            TriggerDecision::Hold
        );
        // Sample 2 at t=11s: elapsed>=10s, but samples=2<3 -> Hold
        assert_eq!(
            evaluate_sample(
                &spec,
                1,
                &sha,
                &mut st,
                &sample("cpu", MetricValue::Number(91.0), 11_000)
            ),
            TriggerDecision::Hold
        );
        // Sample 3 at t=12s: samples=3>=3 AND elapsed=12s>=10s -> Fire!
        let dec = evaluate_sample(
            &spec,
            1,
            &sha,
            &mut st,
            &sample("cpu", MetricValue::Number(92.0), 12_000),
        );
        assert!(matches!(dec, TriggerDecision::Fire(_)), "{dec:?}");

        // Now test min_samples=1, sustain_secs=10: single sample at t=0 never fires; confirming sample at t=10s fires.
        let sustain_only = num_trigger(
            TriggerOp::Gt,
            80.0,
            None,
            1,
            10,
            0,
            TriggerResetMode::ReArmOnClear,
        );
        let sha2 = trigger_spec_sha256(&sustain_only);
        let mut st2 = TriggerState::fresh("d1", &sustain_only.id, 1, sha2.clone());
        assert_eq!(
            evaluate_sample(
                &sustain_only,
                1,
                &sha2,
                &mut st2,
                &sample("cpu", MetricValue::Number(90.0), 1_000)
            ),
            TriggerDecision::Hold
        );
        assert_eq!(
            evaluate_sample(
                &sustain_only,
                1,
                &sha2,
                &mut st2,
                &sample("cpu", MetricValue::Number(90.0), 10_999)
            ),
            TriggerDecision::Hold
        );
        let fire = evaluate_sample(
            &sustain_only,
            1,
            &sha2,
            &mut st2,
            &sample("cpu", MetricValue::Number(90.0), 11_000),
        );
        assert!(matches!(fire, TriggerDecision::Fire(_)), "{fire:?}");
    }

    #[test]
    fn non_matching_sample_and_missing_metric_gap_reset_consecutive_streak() {
        let spec = num_trigger(
            TriggerOp::Gt,
            80.0,
            None,
            3,
            0,
            0,
            TriggerResetMode::ReArmOnClear,
        );
        let sha = trigger_spec_sha256(&spec);
        let mut st = TriggerState::fresh("d1", &spec.id, 1, sha.clone());

        assert_eq!(
            evaluate_sample(
                &spec,
                1,
                &sha,
                &mut st,
                &sample("cpu", MetricValue::Number(90.0), 1_000)
            ),
            TriggerDecision::Hold
        );
        assert_eq!(
            evaluate_sample(
                &spec,
                1,
                &sha,
                &mut st,
                &sample("cpu", MetricValue::Number(90.0), 2_000)
            ),
            TriggerDecision::Hold
        );
        // Non-matching sample resets streak.
        assert_eq!(
            evaluate_sample(
                &spec,
                1,
                &sha,
                &mut st,
                &sample("cpu", MetricValue::Number(50.0), 3_000)
            ),
            TriggerDecision::Cleared
        );
        assert_eq!(st.consecutive_matches, 0);
        assert_eq!(st.first_match_at_ms, None);

        // Two matches + missing-metric gap resets streak again.
        assert_eq!(
            evaluate_sample(
                &spec,
                1,
                &sha,
                &mut st,
                &sample("cpu", MetricValue::Number(90.0), 4_000)
            ),
            TriggerDecision::Hold
        );
        assert_eq!(
            evaluate_sample(
                &spec,
                1,
                &sha,
                &mut st,
                &sample("cpu", MetricValue::Number(90.0), 5_000)
            ),
            TriggerDecision::Hold
        );
        assert!(mark_missing_metric(1, &sha, &mut st));
        assert_eq!(st.consecutive_matches, 0);
        assert_eq!(st.first_match_at_ms, None);
        // One more match is only sample 1 of 3 -> Hold.
        assert_eq!(
            evaluate_sample(
                &spec,
                1,
                &sha,
                &mut st,
                &sample("cpu", MetricValue::Number(90.0), 6_000)
            ),
            TriggerDecision::Hold
        );
    }

    #[test]
    fn hysteresis_latch_and_cooldown_modes() {
        // ReArmOnClear with reset_threshold=70, cooldown=30s
        let spec = num_trigger(
            TriggerOp::Gt,
            80.0,
            Some(70.0),
            1,
            0,
            30,
            TriggerResetMode::ReArmOnClear,
        );
        let sha = trigger_spec_sha256(&spec);
        let mut st = TriggerState::fresh("d1", &spec.id, 1, sha.clone());

        let TriggerDecision::Fire(plan1) = evaluate_sample(
            &spec,
            1,
            &sha,
            &mut st,
            &sample("cpu", MetricValue::Number(85.0), 10_000),
        ) else {
            panic!("expected first fire");
        };
        commit_fire_state(&spec, &mut st, &plan1);
        assert!(st.latched);

        // Value 75 is inside hysteresis band (> 70, <= 80): stays latched.
        assert_eq!(
            evaluate_sample(
                &spec,
                1,
                &sha,
                &mut st,
                &sample("cpu", MetricValue::Number(75.0), 20_000)
            ),
            TriggerDecision::Hold
        );
        assert!(st.latched);

        // Value 70 clears latch.
        assert_eq!(
            evaluate_sample(
                &spec,
                1,
                &sha,
                &mut st,
                &sample("cpu", MetricValue::Number(70.0), 25_000)
            ),
            TriggerDecision::Cleared
        );
        assert!(!st.latched);

        // Value 85 at t=35s is still inside 30s cooldown (10s + 30s = 40s) -> Hold.
        assert_eq!(
            evaluate_sample(
                &spec,
                1,
                &sha,
                &mut st,
                &sample("cpu", MetricValue::Number(85.0), 35_000)
            ),
            TriggerDecision::Hold
        );

        // Value 85 at t=40s -> cooldown expired -> fires!
        let dec2 = evaluate_sample(
            &spec,
            1,
            &sha,
            &mut st,
            &sample("cpu", MetricValue::Number(85.0), 40_000),
        );
        assert!(matches!(dec2, TriggerDecision::Fire(_)), "{dec2:?}");

        // CooldownOnly re-fires after cooldown without needing a clear sample.
        let cd_spec = num_trigger(
            TriggerOp::Gt,
            80.0,
            None,
            1,
            0,
            30,
            TriggerResetMode::CooldownOnly,
        );
        let cd_sha = trigger_spec_sha256(&cd_spec);
        let mut cd_st = TriggerState::fresh("d1", &cd_spec.id, 1, cd_sha.clone());
        let TriggerDecision::Fire(p1) = evaluate_sample(
            &cd_spec,
            1,
            &cd_sha,
            &mut cd_st,
            &sample("cpu", MetricValue::Number(90.0), 10_000),
        ) else {
            panic!("expected fire");
        };
        commit_fire_state(&cd_spec, &mut cd_st, &p1);
        assert!(!cd_st.latched);
        assert_eq!(
            evaluate_sample(
                &cd_spec,
                1,
                &cd_sha,
                &mut cd_st,
                &sample("cpu", MetricValue::Number(90.0), 39_999)
            ),
            TriggerDecision::Hold
        );
        assert!(matches!(
            evaluate_sample(
                &cd_spec,
                1,
                &cd_sha,
                &mut cd_st,
                &sample("cpu", MetricValue::Number(90.0), 40_000)
            ),
            TriggerDecision::Fire(_)
        ));
    }

    #[test]
    fn backward_clock_and_stale_revision_reset() {
        let spec = num_trigger(
            TriggerOp::Gt,
            80.0,
            None,
            1,
            10,
            30,
            TriggerResetMode::CooldownOnly,
        );
        let sha = trigger_spec_sha256(&spec);
        let mut st = TriggerState::fresh("d1", &spec.id, 1, sha.clone());

        // Start streak at t=100s
        assert_eq!(
            evaluate_sample(
                &spec,
                1,
                &sha,
                &mut st,
                &sample("cpu", MetricValue::Number(90.0), 100_000)
            ),
            TriggerDecision::Hold
        );
        // Clock jumps backward to t=50s: re-anchor first_match to 50s, do NOT underflow/fire.
        assert_eq!(
            evaluate_sample(
                &spec,
                1,
                &sha,
                &mut st,
                &sample("cpu", MetricValue::Number(90.0), 50_000)
            ),
            TriggerDecision::Hold
        );
        assert_eq!(st.first_match_at_ms, Some(50_000));
        // Confirm at t=60s (50s + 10s) -> fires.
        let TriggerDecision::Fire(plan) = evaluate_sample(
            &spec,
            1,
            &sha,
            &mut st,
            &sample("cpu", MetricValue::Number(90.0), 60_000),
        ) else {
            panic!("expected fire at 60s");
        };
        commit_fire_state(&spec, &mut st, &plan);
        // Clock jumps backward below last_fired_at_ms (60s) -> stays in cooldown!
        assert_eq!(
            evaluate_sample(
                &spec,
                1,
                &sha,
                &mut st,
                &sample("cpu", MetricValue::Number(90.0), 45_000)
            ),
            TriggerDecision::Hold
        );

        // Stale revision resets state completely.
        assert_eq!(
            evaluate_sample(
                &spec,
                2,
                &sha,
                &mut st,
                &sample("cpu", MetricValue::Number(90.0), 46_000)
            ),
            TriggerDecision::Hold
        );
        assert_eq!(st.script_rev, 2);
        assert_eq!(st.last_fired_at_ms, None);
        assert_eq!(st.episode_seq, 0);
        assert_eq!(st.first_match_at_ms, Some(46_000));
    }
}
