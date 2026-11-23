//! In-process batch yes/no classification (design 212 §2): the core of `decide_many` for
//! `Question::Bool` rows without the tool layer — family resolution as the tool does it
//! (an unconfigured `tool.<x>` answers as `tool`), length-sorted batches of
//! `decision.tool.batch` rows on the named checkpoint, answers back in input order, and the
//! checkpoint released afterwards unless another live family runs on it (57 §2.1) or a row
//! timed out (a cold load may still be in flight; `idle_unload` frees it).

use std::time::Duration;

use anyhow::{anyhow, bail};
use kameo::actor::ActorRef;

use crate::engine::{AskMany, GetConfig, LoadedNow, Release};
use crate::question::{Ask, Decision, Question, Reason};
use crate::tool::{held_by_others, resolve_tool_family};
use crate::{Engine, actor_name};

/// How long the engine may take to answer `GetConfig` / `LoadedNow` before it counts as busy.
const ENGINE_WAIT: Duration = Duration::from_secs(2);
/// Budget of the one retry of a chunk that timed out while the checkpoint was still loading.
const COLD_RETRY: Duration = Duration::from_secs(10);
/// Prefix of every error that means the model cannot answer at all (callers match on it).
const UNAVAILABLE: &str = "decision model unavailable";

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

    /// The engine's decision for one Bool ask; an escalation is the row's `error`. A cut
    /// state is not one: the tool gate is `cut_ok`, so the answer stands with `truncated`.
    fn from_decision(d: &Decision, budget: Duration) -> Self {
        let p_true = d
            .probabilities
            .iter()
            .find_map(|(l, p)| (l == "true").then_some(*p));
        let cut = match d.disposition.reason() {
            None => false,
            Some(Reason::Truncated) if p_true.is_some() => true,
            Some(r) => return BoolVerdict::failed(bool_reason_text(r, budget)),
        };
        let Some(p_true) = p_true else {
            return BoolVerdict::failed("the model answered without a P(true)".to_owned());
        };
        BoolVerdict {
            p_true: Some(p_true),
            choice: Some(d.choice == "true"),
            truncated: cut || d.truncated,
            error: None,
        }
    }
}

/// Why a Bool row has no answer, worded for a caller that did not pick the budget or the
/// labels (`decide_many` keeps its own wording). Model missing or off starts with
/// [`UNAVAILABLE`].
fn bool_reason_text(r: &Reason, budget: Duration) -> String {
    match r {
        Reason::HardError(s) => s.clone(),
        Reason::Timeout => format!(
            "the local model did not answer within {} ms (it may still be loading)",
            budget.as_millis()
        ),
        Reason::Truncated => "the statement did not fit the model window".to_owned(),
        Reason::Disabled => format!("{UNAVAILABLE}: the decision family is off"),
        Reason::ModelMissing => format!(
            "{UNAVAILABLE}: checkpoint not installed — scripts/fetch-models.sh downloads it"
        ),
        Reason::LowTop1 | Reason::LowMargin => "the model was not confident enough".to_owned(),
    }
}

fn is_timeout(d: &Decision) -> bool {
    matches!(d.disposition.reason(), Some(Reason::Timeout))
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
    classify(
        checkpoint,
        family,
        items,
        Duration::from_millis(budget_ms),
        COLD_RETRY,
    )
    .await
}

/// [`classify_bools`] with the cold-load retry budget as a parameter (tests).
async fn classify(
    checkpoint: &str,
    family: &str,
    items: Vec<BoolItem>,
    budget: Duration,
    retry: Duration,
) -> anyhow::Result<Vec<BoolVerdict>> {
    if items.is_empty() {
        return Ok(Vec::new());
    }
    let engine = ActorRef::<Engine>::lookup(actor_name(checkpoint).as_str())
        .ok()
        .flatten()
        .ok_or_else(|| anyhow!("{UNAVAILABLE}: checkpoint engine `{checkpoint}` is not running"))?;
    let cfg = match tokio::time::timeout(ENGINE_WAIT, engine.ask(GetConfig)).await {
        Err(_) => bail!("{UNAVAILABLE}: engine busy"),
        Ok(r) => r.map_err(|e| anyhow!("{UNAVAILABLE}: checkpoint engine `{checkpoint}`: {e}"))?,
    };
    if !cfg.enabled {
        bail!("{UNAVAILABLE}: decision is disabled (decision.enabled)");
    }
    let (family, _, _) =
        resolve_tool_family(&cfg, family).map_err(|e| anyhow!("{UNAVAILABLE}: {e}"))?;

    let lens: Vec<usize> = items
        .iter()
        .map(|i| i.statement.len() + i.state.len())
        .collect();
    let asks: Vec<Ask> = items
        .into_iter()
        .map(|i| {
            Ask::new(
                family.as_str(),
                Question::Bool {
                    statement: i.statement,
                    state: i.state,
                },
            )
        })
        .collect();
    let mut out: Vec<Option<BoolVerdict>> = vec![None; asks.len()];
    let (mut retried, mut timed_out) = (false, false);
    for chunk in plan(&lens, cfg.tool.batch) {
        let mut used = budget;
        let mut answer = ask_chunk(&engine, checkpoint, &asks, &chunk, used).await;
        // A cold load longer than the budget: wait for it once, longer, instead of failing
        // every row (the engine keeps the load running between asks).
        if !retried
            && answer.as_ref().is_ok_and(|ds| ds.iter().any(is_timeout))
            && cold_load(&engine, checkpoint).await
        {
            retried = true;
            used = retry;
            answer = ask_chunk(&engine, checkpoint, &asks, &chunk, used).await;
        }
        let verdicts = match answer {
            Ok(ds) => {
                timed_out |= ds.iter().any(is_timeout);
                ds.iter()
                    .map(|d| BoolVerdict::from_decision(d, used))
                    .collect()
            }
            Err(e) => vec![BoolVerdict::failed(format!("engine: {e}")); chunk.len()],
        };
        place(&mut out, &chunk, verdicts);
        // As `decide_many`: other families' asks interleave between batches.
        tokio::task::yield_now().await;
    }
    // `Release` drops a load still in flight, so after a timeout the next call would start
    // the cold load over and time out again; leave the session to `idle_unload` then.
    if !timed_out && !held_by_others(&cfg, checkpoint) {
        let _ = engine
            .tell(Release {
                checkpoint: None,
                reason: "classify_bools returned",
            })
            .await;
    }
    Ok(finish(out))
}

/// One chunk's asks (cloned: a cold-load retry sends them again) under `budget`.
async fn ask_chunk(
    engine: &ActorRef<Engine>,
    checkpoint: &str,
    asks: &[Ask],
    chunk: &[usize],
    budget: Duration,
) -> Result<Vec<Decision>, String> {
    let asks = chunk
        .iter()
        .map(|&i| {
            let mut ask = asks[i].clone();
            ask.budget = budget;
            ask
        })
        .collect();
    engine
        .ask(AskMany {
            asks,
            checkpoint: Some(checkpoint.to_owned()),
        })
        .await
        .map_err(|e| e.to_string())
}

/// Whether `checkpoint` is not resident yet (a timeout then means a load in flight). An
/// engine that does not answer in time counts as cold.
async fn cold_load(engine: &ActorRef<Engine>, checkpoint: &str) -> bool {
    match tokio::time::timeout(ENGINE_WAIT, engine.ask(LoadedNow)).await {
        Ok(Ok(loaded)) => !loaded.iter().any(|l| l.checkpoint == checkpoint),
        _ => true,
    }
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
        assert!(err(Reason::ModelMissing).starts_with("decision model unavailable: checkpoint"));
        assert_eq!(
            err(Reason::Disabled),
            "decision model unavailable: the decision family is off"
        );
        assert_eq!(
            err(Reason::Timeout),
            "the local model did not answer within 3000 ms (it may still be loading)"
        );
        // A truncation without probabilities (the statement itself did not fit) has no answer.
        assert_eq!(
            err(Reason::Truncated),
            "the statement did not fit the model window"
        );
        // Accepted but without a "true" entry: no made-up probability.
        let d = decision("true", None, Disposition::Accept);
        assert_eq!(
            BoolVerdict::from_decision(&d, budget).error.as_deref(),
            Some("the model answered without a P(true)")
        );
    }

    #[test]
    fn a_cut_state_is_an_answer_not_an_error() {
        // A `tool.memory` gate configured without `cut_ok` escalates a cut state; the scores
        // are there, so the row answers and says it was cut.
        let d = decision("true", Some(0.8), Disposition::Escalate(Reason::Truncated));
        assert_eq!(
            BoolVerdict::from_decision(&d, Duration::ZERO),
            BoolVerdict {
                p_true: Some(0.8),
                choice: Some(true),
                truncated: true,
                error: None,
            }
        );
    }

    /// A Bool row; the engine reads only its flags.
    struct Plain;

    impl crate::Row for Plain {
        fn truncated(&self) -> bool {
            false
        }
        fn state_cut(&self) -> bool {
            false
        }
        fn markers_lost(&self) -> bool {
            false
        }
        fn ids(&self) -> &[i64] {
            &[]
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    /// Answers P(true) = 0.8 for every row.
    struct Yes;

    impl crate::Oracle for Yes {
        fn model_sha(&self) -> crate::engine::ModelSha {
            std::sync::Arc::default()
        }
        fn build(&self, _: &Question) -> Result<Box<dyn crate::Row>, String> {
            Ok(Box::new(Plain))
        }
        fn run(&mut self, rows: &[Box<dyn crate::Row>]) -> anyhow::Result<Vec<crate::Scores>> {
            Ok(rows
                .iter()
                .map(|_| crate::Scores {
                    probs: vec![0.2, 0.8],
                    logits: vec![0.0; 2],
                    temperature: 1.0,
                    act_p: 0.0,
                    embedding: None,
                })
                .collect())
        }
        fn load_time(&self) -> Duration {
            Duration::from_millis(1)
        }
    }

    /// An engine for checkpoint `name` whose load takes `load`; `loads` counts load starts.
    fn cold_engine(
        dir: &std::path::Path,
        name: &str,
        load: Duration,
        loads: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    ) {
        use kameo::actor::Spawn;
        let model_dir = dir.join(name);
        for f in crate::engine::model_files(crate::Kind::Laya) {
            let p = model_dir.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"x").unwrap();
        }
        let loader: crate::Loader = std::sync::Arc::new(move |_, _, _| {
            loads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            std::thread::sleep(load);
            Ok(Box::new(Yes) as Box<dyn crate::Oracle>)
        });
        let engine = Engine::spawn(
            Engine::with_loader(
                crate::DecisionConfig::default(),
                dir.join("default"),
                crate::log::Log::new(dir.join("log")),
                loader,
            )
            .with_checkpoint(name, model_dir),
        );
        engine.register(actor_name(name)).unwrap();
    }

    fn items(n: usize) -> Vec<BoolItem> {
        (0..n)
            .map(|i| BoolItem {
                statement: "This memory helps answer the question: q".into(),
                state: format!("memory {i}"),
            })
            .collect()
    }

    #[tokio::test]
    async fn a_cold_load_past_the_budget_is_waited_for_once() {
        let dir = tempfile::tempdir().unwrap();
        let loads = std::sync::Arc::default();
        cold_engine(
            dir.path(),
            "classify-212-retry",
            Duration::from_millis(300),
            std::sync::Arc::clone(&loads),
        );
        let got = classify(
            "classify-212-retry",
            "tool.memory",
            items(3),
            Duration::from_millis(30),
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        assert_eq!(got, vec![answered(0.8, true); 3]);
        assert_eq!(loads.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_timeout_does_not_release_the_load_in_flight() {
        let dir = tempfile::tempdir().unwrap();
        let loads = std::sync::Arc::default();
        cold_engine(
            dir.path(),
            "classify-212-timeout",
            Duration::from_millis(600),
            std::sync::Arc::clone(&loads),
        );
        let got = classify(
            "classify-212-timeout",
            "tool.memory",
            items(2),
            Duration::from_millis(20),
            Duration::from_millis(40),
        )
        .await
        .unwrap();
        for v in &got {
            assert_eq!(
                v.error.as_deref(),
                Some("the local model did not answer within 40 ms (it may still be loading)")
            );
        }
        // The next call finds the same load still running and gets its answers: no restart.
        let got = classify(
            "classify-212-timeout",
            "tool.memory",
            items(2),
            Duration::from_secs(5),
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        assert_eq!(got, vec![answered(0.8, true); 2]);
        assert_eq!(loads.load(std::sync::atomic::Ordering::SeqCst), 1);
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
