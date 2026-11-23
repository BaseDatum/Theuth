//! In-process batch yes/no classification (design 212 §2): the core of `decide_many` for
//! `Question::Bool` rows without the tool layer — family resolution as the tool does it
//! (an unconfigured `tool.<x>` answers as `tool`), length-sorted batches of
//! `decision.tool.batch` rows on the named checkpoint, answers back in input order, and the
//! checkpoint released afterwards unless another live family runs on it (57 §2.1).

use std::time::Duration;

use anyhow::{anyhow, bail};
use kameo::actor::ActorRef;

use crate::engine::{AskMany, GetConfig, Release};
use crate::question::{Ask, Decision, Question};
use crate::tool::{escalation_text, held_by_others, resolve_tool_family};
use crate::{Engine, actor_name};

/// One yes/no question: does `statement` hold for `state`?
#[derive(Debug, Clone)]
pub struct BoolItem {
    /// The claim to judge, e.g. "This memory helps answer the question: …".
    pub statement: String,
    /// The text it is judged against; right-truncated to the model window.
    pub state: String,
}

/// The answer for one item, in input order.
#[derive(Debug, Clone, PartialEq)]
pub struct BoolVerdict {
    /// P("true") from the model; `None` when the row failed.
    pub p_true: Option<f32>,
    /// The model's choice; `None` when the row failed.
    pub choice: Option<bool>,
    /// The state was cut to fit the model window.
    pub truncated: bool,
    /// Why this row has no answer (model missing, timeout, disabled, hard error).
    pub error: Option<String>,
}

impl BoolVerdict {
    fn failed(error: String) -> Self {
        BoolVerdict {
            p_true: None,
            choice: None,
            truncated: false,
            error: Some(error),
        }
    }

    /// The engine's decision for one Bool ask; an escalation is the row's `error`.
    fn from_decision(d: &Decision, budget: Duration) -> Self {
        if let Some(error) = escalation_text(d, budget) {
            return BoolVerdict::failed(error);
        }
        let Some(p_true) = d
            .probabilities
            .iter()
            .find_map(|(l, p)| (l == "true").then_some(*p))
        else {
            return BoolVerdict::failed("the model answered without a P(true)".to_owned());
        };
        BoolVerdict {
            p_true: Some(p_true),
            choice: Some(d.choice == "true"),
            truncated: d.truncated,
            error: None,
        }
    }
}

/// Classify `items` with the engine of `checkpoint` (e.g. "gliner") under `family`
/// (e.g. "tool.memory"; an unconfigured `tool.<x>` behaves as `tool`, as decide_many does).
/// `budget_ms` is per chunk. Rows are sorted by length and chunked by the tool batch size
/// (8) internally; results come back in input order.
/// Err only when nothing can run at all (no engine for the checkpoint, decision disabled);
/// per-row failures are `BoolVerdict { error: Some(..), .. }`.
pub async fn classify_bools(
    checkpoint: &str,
    family: &str,
    items: Vec<BoolItem>,
    budget_ms: u64,
) -> anyhow::Result<Vec<BoolVerdict>> {
    if items.is_empty() {
        return Ok(Vec::new());
    }
    let engine = ActorRef::<Engine>::lookup(actor_name(checkpoint).as_str())
        .ok()
        .flatten()
        .ok_or_else(|| {
            anyhow!("decision model unavailable: checkpoint engine `{checkpoint}` is not running")
        })?;
    let cfg = engine.ask(GetConfig).await.map_err(|e| {
        anyhow!("decision model unavailable: checkpoint engine `{checkpoint}`: {e}")
    })?;
    if !cfg.enabled {
        bail!("decision model unavailable: decision is disabled (decision.enabled)");
    }
    let (family, _, _) = resolve_tool_family(&cfg, family)
        .map_err(|e| anyhow!("decision model unavailable: {e}"))?;
    let budget = Duration::from_millis(budget_ms);

    let lens: Vec<usize> = items
        .iter()
        .map(|i| i.statement.len() + i.state.len())
        .collect();
    let mut asks: Vec<Option<Ask>> = items
        .into_iter()
        .map(|i| {
            let mut ask = Ask::new(
                family.as_str(),
                Question::Bool {
                    statement: i.statement,
                    state: i.state,
                },
            );
            ask.budget = budget;
            Some(ask)
        })
        .collect();
    let mut out: Vec<Option<BoolVerdict>> = vec![None; asks.len()];
    for chunk in plan(&lens, cfg.tool.batch) {
        let batch: Vec<Ask> = chunk.iter().filter_map(|&i| asks[i].take()).collect();
        let verdicts = match engine
            .ask(AskMany {
                asks: batch,
                checkpoint: Some(checkpoint.to_owned()),
            })
            .await
        {
            Ok(ds) => ds
                .iter()
                .map(|d| BoolVerdict::from_decision(d, budget))
                .collect(),
            Err(e) => vec![BoolVerdict::failed(format!("engine: {e}")); chunk.len()],
        };
        place(&mut out, &chunk, verdicts);
        // As `decide_many`: other families' asks interleave between batches.
        tokio::task::yield_now().await;
    }
    if !held_by_others(&cfg, checkpoint) {
        let _ = engine
            .tell(Release {
                checkpoint: None,
                reason: "classify_bools returned",
            })
            .await;
    }
    Ok(finish(out))
}

/// Row indices, shortest first (a batch pads to its longest row, so like lengths share a
/// forward), in chunks of `batch` (at least 1). Ties keep input order.
fn plan(lens: &[usize], batch: usize) -> Vec<Vec<usize>> {
    let mut order: Vec<usize> = (0..lens.len()).collect();
    order.sort_by_key(|&i| lens[i]);
    order.chunks(batch.max(1)).map(<[usize]>::to_vec).collect()
}

/// Put one chunk's answers back at their input positions.
fn place(out: &mut [Option<BoolVerdict>], chunk: &[usize], verdicts: Vec<BoolVerdict>) {
    for (&i, v) in chunk.iter().zip(verdicts) {
        out[i] = Some(v);
    }
}

/// Input order; a row the engine did not answer (a short reply) is an error, not a gap.
fn finish(out: Vec<Option<BoolVerdict>>) -> Vec<BoolVerdict> {
    out.into_iter()
        .map(|v| {
            v.unwrap_or_else(|| BoolVerdict::failed("the engine returned no answer".to_owned()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::question::{Disposition, Family, LogId, Reason};

    fn decision(choice: &str, p_true: Option<f32>, disposition: Disposition) -> Decision {
        Decision {
            family: Family::from("tool"),
            choice: choice.to_owned(),
            probabilities: p_true
                .map(|p| vec![("false".to_owned(), 1.0 - p), ("true".to_owned(), p)])
                .unwrap_or_default(),
            confidence: 0.0,
            act_probability: 0.0,
            disposition,
            latency: Duration::ZERO,
            gate_version: 0,
            log_id: LogId::new(),
            tier: 1,
            calibrated: true,
            truncated: false,
            embedding: None,
        }
    }

    fn answered(p_true: f32, choice: bool) -> BoolVerdict {
        BoolVerdict {
            p_true: Some(p_true),
            choice: Some(choice),
            truncated: false,
            error: None,
        }
    }

    #[test]
    fn plan_sorts_by_length_and_chunks_stably() {
        let lens = [5, 1, 3, 1, 9, 2, 3];
        assert_eq!(plan(&lens, 3), vec![vec![1, 3, 5], vec![2, 6, 0], vec![4]]);
        // A zero batch is one row per chunk, not a panic.
        assert_eq!(plan(&[2, 1], 0), vec![vec![1], vec![0]]);
        assert!(plan(&[], 8).is_empty());
    }

    #[test]
    fn chunks_reassemble_in_input_order() {
        // Each row's p encodes its input index; after length-sorted chunks the output must be
        // back in input order, and a short engine reply leaves an error, not a shifted row.
        let lens = [40, 10, 30, 20, 50];
        let mut out = vec![None; lens.len()];
        for chunk in plan(&lens, 2) {
            let mut verdicts: Vec<BoolVerdict> = chunk
                .iter()
                .map(|&i| answered(i as f32 / 10.0, i % 2 == 0))
                .collect();
            if chunk.contains(&4) {
                verdicts.clear();
            }
            place(&mut out, &chunk, verdicts);
        }
        let got = finish(out);
        for (i, v) in got.iter().enumerate().take(4) {
            assert_eq!(*v, answered(i as f32 / 10.0, i % 2 == 0), "row {i}");
        }
        assert_eq!(
            got[4].error.as_deref(),
            Some("the engine returned no answer")
        );
        assert_eq!(got[4].p_true, None);
    }

    #[test]
    fn verdict_from_an_accepted_bool() {
        let mut d = decision("true", Some(0.75), Disposition::Accept);
        d.truncated = true;
        let v = BoolVerdict::from_decision(&d, Duration::from_millis(3000));
        assert_eq!(
            v,
            BoolVerdict {
                p_true: Some(0.75),
                choice: Some(true),
                truncated: true,
                error: None,
            }
        );
        let no = decision("false", Some(0.3), Disposition::Accept);
        assert_eq!(
            BoolVerdict::from_decision(&no, Duration::ZERO),
            answered(0.3, false)
        );
        // Shadow mode: the answer stands, read through the inner disposition.
        let shadow = decision("true", Some(0.9), Disposition::Accept).shadowed();
        assert_eq!(
            BoolVerdict::from_decision(&shadow, Duration::ZERO),
            answered(0.9, true)
        );
    }

    #[test]
    fn escalations_are_row_errors() {
        let budget = Duration::from_millis(3000);
        let err = |reason: Reason| {
            let d = decision("none", None, Disposition::Escalate(reason));
            let v = BoolVerdict::from_decision(&d, budget);
            assert_eq!((v.p_true, v.choice, v.truncated), (None, None, false));
            v.error.expect("an escalation is an error")
        };
        assert_eq!(
            err(Reason::HardError("tokenizer: boom".into())),
            "tokenizer: boom"
        );
        assert!(err(Reason::ModelMissing).starts_with("checkpoint not installed"));
        assert_eq!(err(Reason::Disabled), "the decision family is off");
        assert!(err(Reason::Timeout).contains("3000 ms"));
        // Accepted but without a "true" entry: no made-up probability.
        let d = decision("true", None, Disposition::Accept);
        assert_eq!(
            BoolVerdict::from_decision(&d, budget).error.as_deref(),
            Some("the model answered without a P(true)")
        );
    }

    #[tokio::test]
    async fn empty_is_ok_and_a_missing_engine_is_err() {
        assert_eq!(
            classify_bools("no-such-212", "tool.memory", Vec::new(), 3000)
                .await
                .unwrap(),
            Vec::new()
        );
        let e = classify_bools(
            "no-such-212",
            "tool.memory",
            vec![BoolItem {
                statement: "s".into(),
                state: "x".into(),
            }],
            3000,
        )
        .await
        .unwrap_err();
        assert_eq!(
            e.to_string(),
            "decision model unavailable: checkpoint engine `no-such-212` is not running"
        );
    }
}
